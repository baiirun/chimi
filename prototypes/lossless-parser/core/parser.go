// Package core implements the pure logic for the disposable Chimi parser probe.
package core

import (
	"fmt"
	"strings"
	"unicode/utf8"
)

type Token struct {
	Kind  string
	Text  string
	Start int
}

type Diagnostic struct {
	Message string
	Offset  int
}

type Event struct {
	Kind     string
	NodeKind string
	Token    int
	Expected string
}

type GreenToken struct {
	Kind string
	Text string
}

type GreenElement struct {
	Node  *GreenNode
	Token *GreenToken
}

type GreenNode struct {
	Kind     string
	Children []GreenElement
	TextLen  int
}

type Result struct {
	Source      string
	Root        *GreenNode
	Diagnostics []Diagnostic
}

var keywords = map[string]string{
	"pub":       "KW_PUB",
	"import":    "KW_IMPORT",
	"as":        "KW_AS",
	"struct":    "KW_STRUCT",
	"enum":      "KW_ENUM",
	"interface": "KW_INTERFACE",
	"fn":        "KW_FN",
	"let":       "KW_LET",
	"case":      "KW_CASE",
	"if":        "KW_IF",
	"errors":    "KW_ERRORS",
	"needs":     "KW_NEEDS",
	"using":     "KW_USING",
	"True":      "TRUE",
	"False":     "FALSE",
	"Void":      "VOID",
}

var multiTokens = []struct {
	Text string
	Kind string
}{
	{"->", "ARROW"},
	{"|>", "PIPE"},
	{">=", "GTE"},
	{"<=", "LTE"},
}

func Lex(source string) ([]Token, []Diagnostic) {
	tokens := make([]Token, 0, len(source)/3)
	diagnostics := []Diagnostic{}
	emit := func(kind string, start, end int) {
		tokens = append(tokens, Token{Kind: kind, Text: source[start:end], Start: start})
	}

	for index := 0; index < len(source); {
		start := index
		char := source[index]
		if char == ' ' || char == '\t' {
			for index < len(source) && (source[index] == ' ' || source[index] == '\t') {
				index++
			}
			emit("WHITESPACE", start, index)
			continue
		}
		if char == '\n' {
			index++
			emit("NEWLINE", start, index)
			continue
		}
		if char == '\r' {
			index++
			if index < len(source) && source[index] == '\n' {
				index++
			}
			emit("NEWLINE", start, index)
			continue
		}
		if strings.HasPrefix(source[index:], "//") {
			index += 2
			for index < len(source) && source[index] != '\n' && source[index] != '\r' {
				index++
			}
			emit("COMMENT", start, index)
			continue
		}

		matched := false
		for _, candidate := range multiTokens {
			if strings.HasPrefix(source[index:], candidate.Text) {
				index += len(candidate.Text)
				emit(candidate.Kind, start, index)
				matched = true
				break
			}
		}
		if matched {
			continue
		}

		if strings.ContainsRune("(){}[],:.<>+-*/=", rune(char)) {
			index++
			emit(string(char), start, index)
			continue
		}
		if char >= '0' && char <= '9' {
			index++
			for index < len(source) && ((source[index] >= '0' && source[index] <= '9') || source[index] == '_') {
				index++
			}
			emit("INT", start, index)
			continue
		}
		if char == '"' {
			index++
			closed := false
			for index < len(source) {
				if source[index] == '\\' && index+1 < len(source) {
					index += 2
					continue
				}
				if source[index] == '"' {
					index++
					closed = true
					break
				}
				if source[index] == '\n' || source[index] == '\r' {
					break
				}
				_, width := utf8.DecodeRuneInString(source[index:])
				if width == 0 {
					width = 1
				}
				index += width
			}
			if closed {
				emit("TEXT", start, index)
			} else {
				emit("INVALID_TEXT", start, index)
				diagnostics = append(diagnostics, Diagnostic{"unterminated text literal", start})
			}
			continue
		}
		if isIdentStart(char) {
			index++
			for index < len(source) && isIdentContinue(source[index]) {
				index++
			}
			text := source[start:index]
			kind := "IDENT"
			if keyword, ok := keywords[text]; ok {
				kind = keyword
			}
			emit(kind, start, index)
			continue
		}

		_, width := utf8.DecodeRuneInString(source[index:])
		if width == 0 {
			width = 1
		}
		index += width
		emit("INVALID", start, index)
		diagnostics = append(diagnostics, Diagnostic{fmt.Sprintf("unrecognized source text %q", source[start:index]), start})
	}
	tokens = append(tokens, Token{Kind: "EOF", Start: len(source)})
	return tokens, diagnostics
}

func isIdentStart(char byte) bool {
	return char == '_' || (char >= 'a' && char <= 'z') || (char >= 'A' && char <= 'Z')
}

func isIdentContinue(char byte) bool {
	return isIdentStart(char) || (char >= '0' && char <= '9')
}

type parser struct {
	tokens      []Token
	significant []int
	position    int
	nextRaw     int
	events      []Event
	diagnostics []Diagnostic
}

func Parse(source string) Result {
	tokens, lexical := Lex(source)
	p := parser{tokens: tokens, diagnostics: append([]Diagnostic{}, lexical...)}
	for index, token := range tokens {
		if token.Kind != "WHITESPACE" && token.Kind != "COMMENT" {
			p.significant = append(p.significant, index)
		}
	}
	p.parseFile()
	root := sink(p.events, tokens)
	return Result{
		Source:      source,
		Root:        root,
		Diagnostics: p.diagnostics,
	}
}

func (p *parser) start(kind string) {
	p.events = append(p.events, Event{Kind: "START", NodeKind: kind})
}

func (p *parser) finish() {
	p.events = append(p.events, Event{Kind: "FINISH"})
}

func (p *parser) placeholder() int {
	marker := len(p.events)
	p.events = append(p.events, Event{Kind: "TOMBSTONE"})
	return marker
}

func (p *parser) complete(marker int, kind string) {
	p.events[marker] = Event{Kind: "START", NodeKind: kind}
}

func (p *parser) current() Token {
	return p.tokens[p.significant[p.position]]
}

func (p *parser) kind() string {
	return p.current().Kind
}

func (p *parser) text() string {
	return p.current().Text
}

func (p *parser) nthKind(distance int) string {
	index := p.position + distance
	if index >= len(p.significant) {
		return "EOF"
	}
	return p.tokens[p.significant[index]].Kind
}

func (p *parser) at(kind string) bool {
	return p.kind() == kind
}

func (p *parser) bump() {
	raw := p.significant[p.position]
	for p.nextRaw <= raw {
		p.events = append(p.events, Event{Kind: "TOKEN", Token: p.nextRaw})
		p.nextRaw++
	}
	if p.position < len(p.significant)-1 {
		p.position++
	}
}

func (p *parser) accept(kind string) bool {
	if !p.at(kind) {
		return false
	}
	p.bump()
	return true
}

func (p *parser) expect(kind, label string) bool {
	if p.accept(kind) {
		return true
	}
	p.missing(label)
	return false
}

func (p *parser) missing(expected string) {
	p.diagnostics = append(p.diagnostics, Diagnostic{fmt.Sprintf("expected %s", expected), p.current().Start})
	p.events = append(p.events, Event{Kind: "MISSING", Expected: expected})
}

func (p *parser) error(message string) {
	p.diagnostics = append(p.diagnostics, Diagnostic{message, p.current().Start})
}

func (p *parser) errorAndBump(message string) {
	p.start("ERROR")
	p.error(message)
	if !p.at("EOF") {
		p.bump()
	}
	p.finish()
}

func (p *parser) skipNewlines() bool {
	consumed := false
	for p.accept("NEWLINE") {
		consumed = true
	}
	return consumed
}

func (p *parser) afterNewlines(kind string) bool {
	index := p.position
	for index < len(p.significant) && p.tokens[p.significant[index]].Kind == "NEWLINE" {
		index++
	}
	return index < len(p.significant) && p.tokens[p.significant[index]].Kind == kind
}

func (p *parser) acceptAfterNewlines(kind string) bool {
	if !p.afterNewlines(kind) {
		return false
	}
	p.skipNewlines()
	return p.accept(kind)
}

func (p *parser) emitRemaining() {
	for p.nextRaw < len(p.tokens) {
		p.events = append(p.events, Event{Kind: "TOKEN", Token: p.nextRaw})
		p.nextRaw++
	}
}

func (p *parser) parseFile() {
	p.start("FILE")
	p.skipNewlines()
	for !p.at("EOF") {
		before := p.position
		p.parseTopItem()
		if p.position == before {
			p.errorAndBump("parser made no progress at file scope")
		}
		if !p.at("EOF") && !p.skipNewlines() {
			p.error("expected a line break between top-level items")
			for !p.at("EOF") && !p.at("NEWLINE") && !p.isTopStart() {
				p.errorAndBump("unexpected token at file scope")
			}
			p.skipNewlines()
		}
	}
	p.emitRemaining()
	p.finish()
}

func (p *parser) isTopStart() bool {
	switch p.kind() {
	case "KW_PUB", "KW_IMPORT", "KW_STRUCT", "KW_ENUM", "KW_INTERFACE", "KW_FN", "KW_LET":
		return true
	default:
		return false
	}
}

func (p *parser) parseTopItem() {
	switch p.kind() {
	case "KW_IMPORT":
		p.parseImport()
	case "KW_PUB", "KW_STRUCT", "KW_ENUM", "KW_INTERFACE", "KW_FN":
		p.parseDeclaration()
	case "KW_LET":
		p.parseLet()
	default:
		p.parseExpression()
	}
}

func (p *parser) parseImport() {
	p.start("IMPORT_DECL")
	p.bump()
	p.expect("IDENT", "module path segment")
	for p.accept("/") {
		p.expect("IDENT", "module path segment")
	}
	if p.at("KW_AS") {
		p.start("IMPORT_ALIAS")
		p.bump()
		p.expect("IDENT", "module alias")
		p.finish()
	} else if p.at(".") {
		p.start("IMPORT_MEMBERS")
		p.bump()
		p.expect("{", "{")
		p.skipNewlines()
		if p.at("}") {
			p.error("member imports must name at least one type or constructor")
		} else {
			p.start("IMPORT_MEMBER")
			p.expect("IDENT", "imported member")
			p.finish()
			for p.accept(",") {
				p.skipNewlines()
				if p.at("}") {
					break
				}
				p.start("IMPORT_MEMBER")
				p.expect("IDENT", "imported member")
				p.finish()
			}
		}
		p.skipNewlines()
		p.expect("}", "}")
		p.finish()
	} else {
		p.error("module imports require an explicit alias with `as` or an explicit member list")
	}
	p.finish()
}

func (p *parser) parseDeclaration() {
	p.start("DECLARATION")
	p.accept("KW_PUB")
	p.skipNewlines()
	switch p.kind() {
	case "KW_STRUCT":
		p.parseRecordDeclaration("STRUCT_DECL", "KW_STRUCT", p.parseField)
	case "KW_INTERFACE":
		p.parseRecordDeclaration("INTERFACE_DECL", "KW_INTERFACE", p.parseField)
	case "KW_ENUM":
		p.parseEnum()
	case "KW_FN":
		p.parseFunctionDeclaration()
	default:
		p.errorAndBump("expected struct, enum, interface, or function declaration")
	}
	p.finish()
}

func (p *parser) parseRecordDeclaration(nodeKind, keyword string, member func()) {
	p.start(nodeKind)
	p.expect(keyword, strings.ToLower(strings.TrimPrefix(keyword, "KW_")))
	p.expect("IDENT", "declaration name")
	p.parseOptionalGenericParams()
	p.skipNewlines()
	p.expect("{", "{")
	p.skipNewlines()
	for !p.at("}") && !p.at("EOF") {
		before := p.position
		member()
		if p.accept(",") {
			p.skipNewlines()
		} else if !p.at("}") && !p.skipNewlines() {
			p.errorAndBump("expected a line break or comma between members")
		}
		if p.position == before {
			p.errorAndBump("unable to parse declaration member")
		}
	}
	p.expect("}", "}")
	p.finish()
}

func (p *parser) parseField() {
	p.start("FIELD")
	p.expect("IDENT", "field name")
	p.expect(":", ":")
	p.parseType()
	p.finish()
}

func (p *parser) parseEnum() {
	p.start("ENUM_DECL")
	p.bump()
	p.expect("IDENT", "enum name")
	p.parseOptionalGenericParams()
	p.skipNewlines()
	p.expect("{", "{")
	p.skipNewlines()
	for !p.at("}") && !p.at("EOF") {
		before := p.position
		p.start("VARIANT")
		p.expect("IDENT", "variant name")
		if p.accept("(") {
			p.skipNewlines()
			if !p.at(")") {
				p.parseNamedTypeField()
				for p.accept(",") {
					p.skipNewlines()
					if p.at(")") {
						break
					}
					p.parseNamedTypeField()
				}
			}
			p.skipNewlines()
			p.expect(")", ")")
		}
		p.finish()
		if p.accept(",") {
			p.skipNewlines()
		} else if !p.at("}") && !p.skipNewlines() {
			p.errorAndBump("expected a line break or comma between variants")
		}
		if p.position == before {
			p.errorAndBump("unable to parse enum variant")
		}
	}
	p.expect("}", "}")
	p.finish()
}

func (p *parser) parseOptionalGenericParams() {
	if !p.accept("<") {
		return
	}
	p.start("GENERIC_PARAMS")
	p.skipNewlines()
	p.start("GENERIC_PARAM")
	p.expect("IDENT", "generic parameter name")
	p.finish()
	for p.accept(",") {
		p.skipNewlines()
		p.start("GENERIC_PARAM")
		p.expect("IDENT", "generic parameter name")
		p.finish()
	}
	p.skipNewlines()
	p.expect(">", ">")
	p.finish()
}

func (p *parser) parseNamedTypeField() {
	p.start("NAMED_TYPE_FIELD")
	p.expect("IDENT", "field name")
	p.skipNewlines()
	p.expect(":", ":")
	p.skipNewlines()
	p.parseType()
	p.finish()
}

func (p *parser) parseFunctionDeclaration() {
	p.start("FUNCTION_DECL")
	p.bump()
	p.expect("IDENT", "function name")
	p.parseOptionalGenericParams()
	p.skipNewlines()
	p.parseValueParams()
	p.parseImplementationContract()
	p.skipNewlines()
	p.parseBlock()
	p.finish()
}

func (p *parser) parseValueParams() {
	p.start("VALUE_PARAMS")
	p.expect("(", "(")
	p.skipNewlines()
	if !p.at(")") {
		p.parseValueParam()
		for p.accept(",") {
			p.skipNewlines()
			if p.at(")") {
				break
			}
			p.parseValueParam()
		}
	}
	p.skipNewlines()
	p.expect(")", ")")
	p.finish()
}

func (p *parser) parseValueParam() {
	p.start("VALUE_PARAM")
	p.expect("IDENT", "parameter name")
	if p.accept(":") {
		p.skipNewlines()
		p.parseType()
	}
	p.finish()
}

func (p *parser) parseImplementationContract() {
	p.start("IMPLEMENTATION_CONTRACT")
	p.skipNewlines()
	if p.accept("ARROW") {
		p.skipNewlines()
		p.parseType()
		p.skipNewlines()
	}
	if p.at("KW_NEEDS") {
		p.parseRequirementClause()
	}
	p.finish()
}

func (p *parser) parseType() {
	p.start("TYPE")
	p.skipNewlines()
	if p.accept("KW_FN") {
		p.start("FUNCTION_TYPE")
		p.skipNewlines()
		p.expect("(", "(")
		p.skipNewlines()
		if !p.at(")") {
			p.parseType()
			for p.accept(",") {
				p.skipNewlines()
				if p.at(")") {
					break
				}
				p.parseType()
			}
		}
		p.skipNewlines()
		p.expect(")", ")")
		p.skipNewlines()
		p.expect("ARROW", "->")
		p.skipNewlines()
		p.parseType()
		p.skipNewlines()
		if p.accept("KW_ERRORS") {
			p.skipNewlines()
			p.parseType()
			p.skipNewlines()
		}
		if p.at("KW_NEEDS") {
			p.parseRequirementClause()
		}
		p.finish()
	} else if p.accept("IDENT") {
		for p.accept(".") {
			p.expect("IDENT", "type name")
		}
		if p.accept("<") {
			p.start("TYPE_ARGS")
			p.skipNewlines()
			p.parseType()
			for p.accept(",") {
				p.skipNewlines()
				p.parseType()
			}
			p.skipNewlines()
			p.expect(">", ">")
			p.finish()
		}
	} else if p.accept("VOID") {
		if p.accept("<") {
			p.start("TYPE_ARGS")
			p.skipNewlines()
			p.parseType()
			for p.accept(",") {
				p.skipNewlines()
				p.parseType()
			}
			p.skipNewlines()
			p.expect(">", ">")
			p.finish()
		}
	} else {
		p.missing("type")
	}
	p.finish()
}

func (p *parser) parseRequirementClause() {
	p.start("REQUIREMENT_CLAUSE")
	p.expect("KW_NEEDS", "needs")
	p.skipNewlines()
	p.parseRequirementAtom()
	for p.acceptAfterNewlines("+") {
		p.skipNewlines()
		p.parseRequirementAtom()
	}
	p.finish()
}

func (p *parser) parseRequirementAtom() {
	if p.accept("IDENT") {
		return
	}
	p.start("REQUIREMENT_RECORD")
	p.expect("{", "requirement row variable or record")
	p.skipNewlines()
	if p.at("}") {
		p.error("empty requirement records are invalid")
	} else {
		p.parseNamedTypeField()
		for p.accept(",") {
			p.skipNewlines()
			if p.at("}") {
				break
			}
			p.parseNamedTypeField()
		}
	}
	p.skipNewlines()
	p.expect("}", "}")
	p.finish()
}

func (p *parser) parseLet() {
	p.start("LET_BINDING")
	p.expect("KW_LET", "let")
	p.expect("IDENT", "binding name")
	p.expect("=", "=")
	p.parseExpression()
	p.finish()
}

func (p *parser) parseExpression() {
	p.parseUsing()
}

func (p *parser) parseUsing() {
	marker := p.placeholder()
	p.parsePipeline()
	wrapped := false
	for p.acceptAfterNewlines("KW_USING") {
		if !wrapped {
			p.complete(marker, "USING_EXPR")
			wrapped = true
		}
		p.skipNewlines()
		p.parseRecord()
	}
	if wrapped {
		p.finish()
	}
}

func (p *parser) parsePipeline() {
	marker := p.placeholder()
	p.parseComparison()
	wrapped := false
	for p.acceptAfterNewlines("PIPE") {
		if !wrapped {
			p.complete(marker, "PIPELINE_EXPR")
			wrapped = true
		}
		if p.pipelineRHSIsMissing() {
			p.missing("pipeline transformation")
			break
		}
		p.skipNewlines()
		p.parseComparison()
	}
	if wrapped {
		p.finish()
	}
}

func (p *parser) pipelineRHSIsMissing() bool {
	index := p.position
	for index < len(p.significant) && p.tokens[p.significant[index]].Kind == "NEWLINE" {
		index++
	}
	if index >= len(p.significant) {
		return true
	}
	kind := p.tokens[p.significant[index]].Kind
	switch kind {
	case "EOF", "}", "]", ")", ",", "KW_PUB", "KW_IMPORT", "KW_STRUCT", "KW_ENUM", "KW_INTERFACE", "KW_FN", "KW_LET":
		return true
	default:
		return false
	}
}

func (p *parser) parseComparison() {
	marker := p.placeholder()
	p.parseAdditive()
	wrapped := false
	for p.afterAnyNewlines("<", "LTE", ">", "GTE") {
		if !wrapped {
			p.complete(marker, "COMPARISON_EXPR")
			wrapped = true
		}
		p.skipNewlines()
		p.bump()
		p.skipNewlines()
		p.parseAdditive()
	}
	if wrapped {
		p.finish()
	}
}

func (p *parser) parseAdditive() {
	marker := p.placeholder()
	p.parseMultiplicative()
	wrapped := false
	for p.afterAnyNewlines("+", "-") {
		if !wrapped {
			p.complete(marker, "ADDITIVE_EXPR")
			wrapped = true
		}
		p.skipNewlines()
		p.bump()
		p.skipNewlines()
		p.parseMultiplicative()
	}
	if wrapped {
		p.finish()
	}
}

func (p *parser) parseMultiplicative() {
	marker := p.placeholder()
	p.parseUnary()
	wrapped := false
	for p.afterAnyNewlines("*", "/") {
		if !wrapped {
			p.complete(marker, "MULTIPLICATIVE_EXPR")
			wrapped = true
		}
		p.skipNewlines()
		p.bump()
		p.skipNewlines()
		p.parseUnary()
	}
	if wrapped {
		p.finish()
	}
}

func (p *parser) afterAnyNewlines(kinds ...string) bool {
	index := p.position
	for index < len(p.significant) && p.tokens[p.significant[index]].Kind == "NEWLINE" {
		index++
	}
	if index >= len(p.significant) {
		return false
	}
	found := p.tokens[p.significant[index]].Kind
	for _, kind := range kinds {
		if found == kind {
			return true
		}
	}
	return false
}

func (p *parser) parseUnary() {
	marker := p.placeholder()
	wrapped := false
	if p.accept("-") {
		p.complete(marker, "UNARY_EXPR")
		wrapped = true
		p.skipNewlines()
	}
	p.parsePostfix()
	if wrapped {
		p.finish()
	}
}

func (p *parser) parsePostfix() {
	marker := p.placeholder()
	p.parsePrimary()
	wrapped := false
	for {
		if p.accept(".") {
			if !wrapped {
				p.complete(marker, "POSTFIX_EXPR")
				wrapped = true
			}
			p.start("MEMBER_ACCESS")
			p.expect("IDENT", "member name")
			p.finish()
		} else if p.at("(") {
			if !wrapped {
				p.complete(marker, "POSTFIX_EXPR")
				wrapped = true
			}
			p.parseCallSuffix()
		} else {
			break
		}
	}
	if wrapped {
		p.finish()
	}
}

func (p *parser) parsePrimary() {
	switch p.kind() {
	case "INT", "TEXT", "TRUE", "FALSE":
		p.start("LITERAL")
		p.bump()
		p.finish()
	case "IDENT":
		p.start("NAME_REF")
		p.bump()
		p.finish()
	case "(":
		p.start("GROUPED_EXPR")
		p.bump()
		p.skipNewlines()
		p.parseExpression()
		p.skipNewlines()
		p.expect(")", ")")
		p.finish()
	case "[":
		p.parseList()
	case "{":
		p.parseBraceExpression()
	case "KW_CASE":
		p.parseCase()
	case "KW_FN":
		p.parseFunctionExpression()
	case "VOID":
		p.errorAndBump("Void is a type, not a value")
	case "KW_IF":
		p.parseUnsupportedIf()
	default:
		p.missing("expression")
	}
}

func (p *parser) parseCallSuffix() {
	p.start("CALL_SUFFIX")
	p.expect("(", "(")
	p.skipNewlines()
	if !p.at(")") {
		p.parseArgument()
		for p.accept(",") {
			p.skipNewlines()
			if p.at(")") {
				break
			}
			p.parseArgument()
		}
	}
	p.skipNewlines()
	p.expect(")", ")")
	p.finish()
}

func (p *parser) parseArgument() {
	p.start("ARGUMENT")
	if p.at("IDENT") && p.nthKind(1) == ":" {
		p.bump()
		p.bump()
	}
	p.parseExpression()
	p.finish()
}

func (p *parser) parseList() {
	p.start("LIST_EXPR")
	p.bump()
	p.skipNewlines()
	if !p.at("]") {
		p.parseExpression()
		for p.accept(",") {
			p.skipNewlines()
			if p.at("]") {
				break
			}
			p.parseExpression()
		}
	}
	p.skipNewlines()
	p.expect("]", "]")
	p.finish()
}

func (p *parser) parseBraceExpression() {
	index := p.position + 1
	for index < len(p.significant) && p.tokens[p.significant[index]].Kind == "NEWLINE" {
		index++
	}
	if index < len(p.significant) && p.tokens[p.significant[index]].Kind == "}" {
		p.start("EMPTY_BRACE_EXPR")
		p.bump()
		p.skipNewlines()
		p.error("empty braces are invalid")
		p.bump()
		p.finish()
		return
	}
	isRecord := p.looksLikeRecord(index)
	if isRecord {
		p.parseRecord()
	} else {
		p.parseBlock()
	}
}

func (p *parser) looksLikeRecord(index int) bool {
	if index >= len(p.significant) || p.tokens[p.significant[index]].Kind != "IDENT" {
		return false
	}
	if index+1 < len(p.significant) && p.tokens[p.significant[index+1]].Kind == ":" {
		return true
	}
	// A comma before the first line break or closing brace cannot belong to a
	// block item in the accepted grammar. Treat it as a damaged record field so
	// recovery can insert one missing colon instead of reclassifying the whole
	// brace expression and cascading diagnostics.
	for scan := index + 1; scan < len(p.significant); scan++ {
		switch p.tokens[p.significant[scan]].Kind {
		case ",":
			return true
		case "NEWLINE", "}", "EOF":
			return false
		}
	}
	return false
}

func (p *parser) parseRecord() {
	p.start("RECORD_EXPR")
	p.expect("{", "{")
	p.skipNewlines()
	p.parseRecordField()
	for p.accept(",") {
		p.skipNewlines()
		if p.at("}") {
			break
		}
		p.parseRecordField()
	}
	p.skipNewlines()
	p.expect("}", "}")
	p.finish()
}

func (p *parser) parseRecordField() {
	p.start("RECORD_FIELD")
	p.expect("IDENT", "record field name")
	p.expect(":", ":")
	p.parseExpression()
	p.finish()
}

func (p *parser) parseBlock() {
	p.start("BLOCK_EXPR")
	p.expect("{", "{")
	p.skipNewlines()
	if p.at("}") {
		p.error("empty braces are invalid")
		p.bump()
		p.finish()
		return
	}
	for !p.at("}") && !p.at("EOF") {
		before := p.position
		p.start("BLOCK_ITEM")
		if p.at("KW_LET") {
			p.parseLet()
		} else {
			p.parseExpression()
		}
		p.finish()
		if p.position == before {
			p.errorAndBump("unable to parse block item")
		}
		if !p.at("}") && !p.skipNewlines() {
			p.errorAndBump("expected a line break between block items")
		}
	}
	p.expect("}", "}")
	p.finish()
}

func (p *parser) parseCase() {
	p.start("CASE_EXPR")
	p.bump()
	p.parseExpression()
	for p.accept(",") {
		p.skipNewlines()
		p.parseExpression()
	}
	p.skipNewlines()
	p.expect("{", "{")
	p.skipNewlines()
	for !p.at("}") && !p.at("EOF") {
		before := p.position
		p.start("CASE_ARM")
		p.parsePattern()
		for p.accept(",") {
			p.skipNewlines()
			p.parsePattern()
		}
		if p.accept("KW_IF") {
			p.parseExpression()
		}
		p.skipNewlines()
		p.expect("ARROW", "->")
		p.skipNewlines()
		p.parseExpression()
		p.finish()
		if p.position == before {
			p.errorAndBump("unable to parse case arm")
		}
		if !p.at("}") && !p.skipNewlines() {
			p.errorAndBump("expected a line break between case arms")
		}
	}
	p.expect("}", "}")
	p.finish()
}

func (p *parser) parsePattern() {
	p.start("PATTERN")
	switch p.kind() {
	case "INT", "TEXT", "TRUE", "FALSE":
		p.bump()
	case "IDENT":
		p.bump()
		if p.accept("(") {
			p.skipNewlines()
			if !p.at(")") {
				p.parsePattern()
				for p.accept(",") {
					p.skipNewlines()
					p.parsePattern()
				}
			}
			p.skipNewlines()
			p.expect(")", ")")
		}
	default:
		p.missing("pattern")
	}
	p.finish()
}

func (p *parser) parseFunctionExpression() {
	p.start("FUNCTION_EXPR")
	p.bump()
	p.skipNewlines()
	p.parseValueParams()
	p.parseImplementationContract()
	p.skipNewlines()
	p.parseBlock()
	p.finish()
}

func (p *parser) parseUnsupportedIf() {
	p.start("ERROR_IF_EXPR")
	p.error("standalone if is not an expression; use case")
	p.bump()
	depth := 0
	seenBrace := false
	for !p.at("EOF") {
		if p.at("{") {
			seenBrace = true
			depth++
			p.bump()
			continue
		}
		if p.at("}") {
			if depth == 0 {
				break
			}
			depth--
			p.bump()
			if seenBrace && depth == 0 {
				if p.at("IDENT") && p.text() == "else" {
					p.bump()
					seenBrace = false
					continue
				}
				break
			}
			continue
		}
		p.bump()
	}
	p.finish()
}

func sink(events []Event, tokens []Token) *GreenNode {
	type builder struct {
		kind     string
		children []GreenElement
	}
	stack := []*builder{}
	var root *GreenNode
	for _, event := range events {
		switch event.Kind {
		case "TOMBSTONE":
			continue
		case "START":
			stack = append(stack, &builder{kind: event.NodeKind})
		case "TOKEN":
			token := tokens[event.Token]
			green := &GreenToken{Kind: token.Kind, Text: token.Text}
			current := stack[len(stack)-1]
			current.children = append(current.children, GreenElement{Token: green})
		case "MISSING":
			green := &GreenToken{Kind: "MISSING_" + event.Expected}
			current := stack[len(stack)-1]
			current.children = append(current.children, GreenElement{Token: green})
		case "FINISH":
			current := stack[len(stack)-1]
			stack = stack[:len(stack)-1]
			length := 0
			for _, child := range current.children {
				if child.Node != nil {
					length += child.Node.TextLen
				} else {
					length += len(child.Token.Text)
				}
			}
			node := &GreenNode{Kind: current.kind, Children: current.children, TextLen: length}
			if len(stack) == 0 {
				root = node
			} else {
				parent := stack[len(stack)-1]
				parent.children = append(parent.children, GreenElement{Node: node})
			}
		}
	}
	return root
}

func (result Result) RoundTrip() string {
	var output strings.Builder
	output.Grow(len(result.Source))
	writeNodeText(&output, result.Root)
	return output.String()
}

func writeNodeText(output *strings.Builder, node *GreenNode) {
	for _, child := range node.Children {
		if child.Node != nil {
			writeNodeText(output, child.Node)
		} else {
			output.WriteString(child.Token.Text)
		}
	}
}

func (result Result) Counts() (nodes, tokens, missing int) {
	var walk func(*GreenNode)
	walk = func(node *GreenNode) {
		nodes++
		for _, child := range node.Children {
			if child.Node != nil {
				walk(child.Node)
			} else {
				tokens++
				if strings.HasPrefix(child.Token.Kind, "MISSING_") {
					missing++
				}
			}
		}
	}
	walk(result.Root)
	return
}

func (result Result) EstimatedBytes() int {
	bytes := 0
	var walk func(*GreenNode)
	walk = func(node *GreenNode) {
		bytes += 48 + len(node.Children)*16
		for _, child := range node.Children {
			if child.Node != nil {
				walk(child.Node)
			} else {
				bytes += 32
			}
		}
	}
	walk(result.Root)
	return bytes
}

type Cursor struct {
	Root *GreenNode
	Path []int
}

func RootCursor(root *GreenNode) Cursor {
	return Cursor{Root: root}
}

func (cursor Cursor) Node() *GreenNode {
	node := cursor.Root
	for _, index := range cursor.Path {
		node = node.Children[index].Node
	}
	return node
}

func (cursor Cursor) Offset() int {
	node := cursor.Root
	offset := 0
	for _, target := range cursor.Path {
		for index := 0; index < target; index++ {
			child := node.Children[index]
			if child.Node != nil {
				offset += child.Node.TextLen
			} else {
				offset += len(child.Token.Text)
			}
		}
		node = node.Children[target].Node
	}
	return offset
}

func (cursor Cursor) Parent() (Cursor, bool) {
	if len(cursor.Path) == 0 {
		return Cursor{}, false
	}
	path := append([]int{}, cursor.Path[:len(cursor.Path)-1]...)
	return Cursor{Root: cursor.Root, Path: path}, true
}

func (cursor Cursor) ChildNodes() []Cursor {
	node := cursor.Node()
	children := []Cursor{}
	for index, child := range node.Children {
		if child.Node != nil {
			path := append(append([]int{}, cursor.Path...), index)
			children = append(children, Cursor{Root: cursor.Root, Path: path})
		}
	}
	return children
}

func LineColumn(source string, offset int) (line, column int) {
	line, column = 1, 1
	if offset > len(source) {
		offset = len(source)
	}
	for index := 0; index < offset; {
		if source[index] == '\n' {
			line++
			column = 1
			index++
			continue
		}
		_, width := utf8.DecodeRuneInString(source[index:])
		if width == 0 {
			width = 1
		}
		index += width
		column++
	}
	return
}

func TreeLines(root *GreenNode) []string {
	lines := []string{}
	var walk func(*GreenNode, int, int)
	walk = func(node *GreenNode, depth, offset int) {
		lines = append(lines, fmt.Sprintf("%s%s  %d..%d", strings.Repeat("  ", depth), node.Kind, offset, offset+node.TextLen))
		current := offset
		for _, child := range node.Children {
			if child.Node != nil {
				walk(child.Node, depth+1, current)
				current += child.Node.TextLen
			} else {
				text := strings.ReplaceAll(child.Token.Text, "\n", "\\n")
				text = strings.ReplaceAll(text, "\r", "\\r")
				text = strings.ReplaceAll(text, "\t", "\\t")
				if len(text) > 38 {
					text = text[:35] + "..."
				}
				lines = append(lines, fmt.Sprintf("%s%s %q  %d..%d", strings.Repeat("  ", depth+1), child.Token.Kind, text, current, current+len(child.Token.Text)))
				current += len(child.Token.Text)
			}
		}
	}
	walk(root, 0, 0)
	return lines
}
