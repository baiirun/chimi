// Package lower contains the pure CST-to-semantic lowering probe.
package lower

import (
	"fmt"
	"reflect"
	"sort"
	"strconv"
	"strings"

	"chimi-lossless-parser-prototype/core"
)

type Span struct {
	Start int
	End   int
}

type LocatedName struct {
	Text string
	Span Span
}

type File struct {
	Items []Item
	Span  Span
}

type Item struct {
	Import     *Import
	Data       *DataDeclaration
	Function   *Function
	Binding    *Binding
	Expression *Expr
}

type Import struct {
	Path    []LocatedName
	Alias   *LocatedName
	Members []LocatedName
	Span    Span
}

type DataDeclaration struct {
	Kind       string
	Name       LocatedName
	Public     bool
	Parameters []LocatedName
	Fields     []TypedField
	Variants   []Variant
	Span       Span
}

type TypedField struct {
	Name LocatedName
	Type Type
	Span Span
}

type Variant struct {
	Name   LocatedName
	Fields []TypedField
	Span   Span
}

type Function struct {
	Name              LocatedName
	Public            bool
	GenericParameters []LocatedName
	Parameters        []Parameter
	Return            *Type
	Requirements      *RequirementRow
	Body              Expr
	Span              Span
}

type Parameter struct {
	Name LocatedName
	Type *Type
	Span Span
}

type Type struct {
	Kind         string
	Name         *LocatedName
	Path         []LocatedName
	Inputs       []Type
	Success      *Type
	Errors       *Type
	Requirements *RequirementRow
	Arguments    []Type
	Span         Span
}

type RequirementRow struct {
	Terms []RequirementTerm
	Span  Span
}

type RequirementTerm struct {
	Variable *LocatedName
	Fields   []TypedField
	Span     Span
}

type Binding struct {
	Name  LocatedName
	Value Expr
	Span  Span
}

type Argument struct {
	Label *LocatedName
	Value Expr
}

type Expr struct {
	Kind       string
	Span       Span
	Origin     string
	Name       *LocatedName
	Literal    string
	Receiver   *Expr
	Callee     *Expr
	Arguments  []Argument
	Parameters []Parameter
	Return     *Type
	Requires   *RequirementRow
	Body       *Expr
	Steps      []BlockItem
	Result     *Expr
	Operator   string
	Left       *Expr
	Right      *Expr
	Elements   []Expr
	Fields     []RecordField
	Subjects   []Expr
	Arms       []CaseArm
}

type RecordField struct {
	Name  LocatedName
	Value Expr
	Span  Span
}

type CaseArm struct {
	Patterns []Pattern
	Guard    *Expr
	Body     Expr
	Span     Span
}

type Pattern struct {
	Kind      string
	Name      *LocatedName
	Literal   string
	Arguments []Pattern
	Span      Span
}

type BlockItem struct {
	Binding    *Binding
	Expression *Expr
}

type Diagnostic struct {
	Message string
	Span    Span
}

type Result struct {
	File        File
	Diagnostics []Diagnostic
}

type syntaxNode struct {
	green *core.GreenNode
	start int
}

type syntaxChild struct {
	node  *syntaxNode
	token *core.GreenToken
	start int
}

func (node syntaxNode) children() []syntaxChild {
	children := make([]syntaxChild, 0, len(node.green.Children))
	offset := node.start
	for _, child := range node.green.Children {
		if child.Node != nil {
			value := syntaxNode{green: child.Node, start: offset}
			children = append(children, syntaxChild{node: &value, start: offset})
			offset += child.Node.TextLen
		} else {
			children = append(children, syntaxChild{token: child.Token, start: offset})
			offset += len(child.Token.Text)
		}
	}
	return children
}

func (node syntaxNode) child(kind string) *syntaxNode {
	for _, child := range node.children() {
		if child.node != nil && child.node.green.Kind == kind {
			return child.node
		}
	}
	return nil
}

func (node syntaxNode) childNodes(kind string) []syntaxNode {
	result := []syntaxNode{}
	for _, child := range node.children() {
		if child.node != nil && child.node.green.Kind == kind {
			result = append(result, *child.node)
		}
	}
	return result
}

func significant(kind string) bool {
	return kind != "WHITESPACE" && kind != "NEWLINE" && kind != "COMMENT"
}

func (node syntaxNode) tokens() []struct {
	token *core.GreenToken
	start int
} {
	result := []struct {
		token *core.GreenToken
		start int
	}{}
	var walk func(syntaxNode)
	walk = func(current syntaxNode) {
		for _, child := range current.children() {
			if child.node != nil {
				walk(*child.node)
			} else {
				result = append(result, struct {
					token *core.GreenToken
					start int
				}{child.token, child.start})
			}
		}
	}
	walk(node)
	return result
}

func (node syntaxNode) span() Span {
	start := -1
	end := -1
	for _, item := range node.tokens() {
		if !significant(item.token.Kind) {
			continue
		}
		if start < 0 {
			start = item.start
		}
		end = item.start + len(item.token.Text)
	}
	if start < 0 {
		return Span{Start: node.start, End: node.start}
	}
	return Span{Start: start, End: end}
}

func (node syntaxNode) directToken(kind string) *LocatedName {
	for _, child := range node.children() {
		if child.token != nil && child.token.Kind == kind {
			return &LocatedName{Text: child.token.Text, Span: Span{child.start, child.start + len(child.token.Text)}}
		}
	}
	return nil
}

func (node syntaxNode) directTokens(kind string) []LocatedName {
	result := []LocatedName{}
	for _, child := range node.children() {
		if child.token != nil && child.token.Kind == kind {
			result = append(result, LocatedName{Text: child.token.Text, Span: Span{child.start, child.start + len(child.token.Text)}})
		}
	}
	return result
}

func (node syntaxNode) hasDirectToken(kind string) bool {
	return node.directToken(kind) != nil
}

func (node syntaxNode) hasToken(kind string) bool {
	for _, item := range node.tokens() {
		if item.token.Kind == kind {
			return true
		}
	}
	return false
}

type lowerer struct {
	diagnostics []Diagnostic
}

func Lower(root *core.GreenNode) Result {
	l := lowerer{}
	fileNode := syntaxNode{green: root}
	file := File{Span: fileNode.span()}
	for _, child := range fileNode.children() {
		if child.node == nil {
			continue
		}
		switch child.node.green.Kind {
		case "IMPORT_DECL":
			value := l.lowerImport(*child.node)
			file.Items = append(file.Items, Item{Import: &value})
		case "DECLARATION":
			public := child.node.hasDirectToken("KW_PUB")
			if functionNode := child.node.child("FUNCTION_DECL"); functionNode != nil {
				function := l.lowerFunction(*functionNode, child.node.span(), public)
				file.Items = append(file.Items, Item{Function: &function})
			} else {
				for _, kind := range []string{"STRUCT_DECL", "ENUM_DECL", "INTERFACE_DECL"} {
					if declaration := child.node.child(kind); declaration != nil {
						value := l.lowerData(*declaration, child.node.span(), public)
						file.Items = append(file.Items, Item{Data: &value})
						break
					}
				}
			}
		case "LET_BINDING":
			binding := l.lowerBinding(*child.node)
			file.Items = append(file.Items, Item{Binding: &binding})
		default:
			if expressionKinds[child.node.green.Kind] || child.node.green.Kind == "ERROR" {
				value := l.lowerExpr(*child.node)
				file.Items = append(file.Items, Item{Expression: &value})
			}
		}
	}
	return Result{File: file, Diagnostics: l.diagnostics}
}

func (l *lowerer) lowerImport(node syntaxNode) Import {
	result := Import{Path: node.directTokens("IDENT"), Span: node.span()}
	if alias := node.child("IMPORT_ALIAS"); alias != nil {
		result.Alias = alias.directToken("IDENT")
	}
	if members := node.child("IMPORT_MEMBERS"); members != nil {
		for _, member := range members.childNodes("IMPORT_MEMBER") {
			if name := member.directToken("IDENT"); name != nil {
				result.Members = append(result.Members, *name)
			}
		}
	}
	return result
}

func (l *lowerer) lowerData(node syntaxNode, span Span, public bool) DataDeclaration {
	kind := strings.TrimSuffix(strings.ToLower(node.green.Kind), "_decl")
	result := DataDeclaration{Kind: kind, Public: public, Span: span}
	if name := node.directToken("IDENT"); name != nil {
		result.Name = *name
	}
	if generics := node.child("GENERIC_PARAMS"); generics != nil {
		for _, parameterNode := range generics.childNodes("GENERIC_PARAM") {
			if name := parameterNode.directToken("IDENT"); name != nil {
				result.Parameters = append(result.Parameters, *name)
			}
		}
	}
	for _, fieldNode := range node.childNodes("FIELD") {
		result.Fields = append(result.Fields, l.lowerTypedField(fieldNode))
	}
	for _, variantNode := range node.childNodes("VARIANT") {
		variant := Variant{Span: variantNode.span()}
		if name := variantNode.directToken("IDENT"); name != nil {
			variant.Name = *name
		}
		for _, fieldNode := range variantNode.childNodes("NAMED_TYPE_FIELD") {
			variant.Fields = append(variant.Fields, l.lowerTypedField(fieldNode))
		}
		result.Variants = append(result.Variants, variant)
	}
	return result
}

func (l *lowerer) lowerTypedField(node syntaxNode) TypedField {
	result := TypedField{Span: node.span()}
	if name := node.directToken("IDENT"); name != nil {
		result.Name = *name
	}
	if typeNode := node.child("TYPE"); typeNode != nil {
		result.Type = l.lowerType(*typeNode)
	}
	return result
}

func (l *lowerer) lowerFunction(node syntaxNode, span Span, public bool) Function {
	name := node.directToken("IDENT")
	function := Function{Public: public, Span: span}
	if name != nil {
		function.Name = *name
	}
	if generics := node.child("GENERIC_PARAMS"); generics != nil {
		for _, parameterNode := range generics.childNodes("GENERIC_PARAM") {
			if name := parameterNode.directToken("IDENT"); name != nil {
				function.GenericParameters = append(function.GenericParameters, *name)
			}
		}
	}
	if params := node.child("VALUE_PARAMS"); params != nil {
		function.Parameters = l.lowerParameters(*params)
	}
	if contract := node.child("IMPLEMENTATION_CONTRACT"); contract != nil {
		if resultType := contract.child("TYPE"); resultType != nil {
			value := l.lowerType(*resultType)
			function.Return = &value
		}
		if requirements := contract.child("REQUIREMENT_CLAUSE"); requirements != nil {
			value := l.lowerRequirement(*requirements)
			function.Requirements = &value
		}
	}
	if body := node.child("BLOCK_EXPR"); body != nil {
		function.Body = l.lowerExpr(*body)
	}
	return function
}

func (l *lowerer) lowerParameters(node syntaxNode) []Parameter {
	parameters := []Parameter{}
	for _, parameterNode := range node.childNodes("VALUE_PARAM") {
		parameter := Parameter{Span: parameterNode.span()}
		if name := parameterNode.directToken("IDENT"); name != nil {
			parameter.Name = *name
		}
		if typeNode := parameterNode.child("TYPE"); typeNode != nil {
			value := l.lowerType(*typeNode)
			parameter.Type = &value
		}
		parameters = append(parameters, parameter)
	}
	return parameters
}

func (l *lowerer) lowerType(node syntaxNode) Type {
	result := Type{Span: node.span()}
	if functionNode := node.child("FUNCTION_TYPE"); functionNode != nil {
		result.Kind = "FunctionType"
		phase := "inputs"
		for _, child := range functionNode.children() {
			if child.token != nil {
				switch child.token.Kind {
				case "ARROW":
					phase = "success"
				case "KW_ERRORS":
					phase = "errors"
				}
				continue
			}
			if child.node.green.Kind == "TYPE" {
				value := l.lowerType(*child.node)
				switch phase {
				case "inputs":
					result.Inputs = append(result.Inputs, value)
				case "success":
					result.Success = &value
					phase = "after-success"
				case "errors":
					result.Errors = &value
					phase = "after-errors"
				}
			}
			if child.node.green.Kind == "REQUIREMENT_CLAUSE" {
				value := l.lowerRequirement(*child.node)
				result.Requirements = &value
			}
		}
		return result
	}

	result.Kind = "NamedType"
	result.Path = node.directTokens("IDENT")
	if len(result.Path) > 0 {
		result.Name = &result.Path[len(result.Path)-1]
	} else if name := node.directToken("VOID"); name != nil {
		result.Name = name
		result.Path = append(result.Path, *name)
	}
	if arguments := node.child("TYPE_ARGS"); arguments != nil {
		for _, argument := range arguments.childNodes("TYPE") {
			result.Arguments = append(result.Arguments, l.lowerType(argument))
		}
	}
	return result
}

func (l *lowerer) lowerRequirement(node syntaxNode) RequirementRow {
	result := RequirementRow{Span: node.span()}
	for _, child := range node.children() {
		if child.token != nil && child.token.Kind == "IDENT" {
			name := LocatedName{Text: child.token.Text, Span: Span{child.start, child.start + len(child.token.Text)}}
			result.Terms = append(result.Terms, RequirementTerm{Variable: &name, Span: name.Span})
		}
		if child.node != nil && child.node.green.Kind == "REQUIREMENT_RECORD" {
			term := RequirementTerm{Span: child.node.span()}
			for _, fieldNode := range child.node.childNodes("NAMED_TYPE_FIELD") {
				term.Fields = append(term.Fields, l.lowerTypedField(fieldNode))
			}
			result.Terms = append(result.Terms, term)
		}
	}
	return result
}

func (l *lowerer) lowerBinding(node syntaxNode) Binding {
	result := Binding{Span: node.span()}
	if name := node.directToken("IDENT"); name != nil {
		result.Name = *name
	}
	if expression := directExpression(node); expression != nil {
		result.Value = l.lowerExpr(*expression)
	} else {
		l.diagnostics = append(l.diagnostics, Diagnostic{Message: "binding has no valid expression", Span: node.span()})
		result.Value = Expr{Kind: "Invalid", Span: node.span()}
	}
	return result
}

var expressionKinds = map[string]bool{
	"NAME_REF": true, "LITERAL": true, "POSTFIX_EXPR": true,
	"FUNCTION_EXPR": true, "BLOCK_EXPR": true, "GROUPED_EXPR": true,
	"USING_EXPR": true, "PIPELINE_EXPR": true, "COMPARISON_EXPR": true, "ADDITIVE_EXPR": true,
	"MULTIPLICATIVE_EXPR": true, "UNARY_EXPR": true, "LIST_EXPR": true,
	"RECORD_EXPR": true, "CASE_EXPR": true, "EMPTY_BRACE_EXPR": true,
	"ERROR_IF_EXPR": true, "ERROR": true,
}

func directExpression(node syntaxNode) *syntaxNode {
	for _, child := range node.children() {
		if child.node != nil && expressionKinds[child.node.green.Kind] {
			return child.node
		}
	}
	return nil
}

func directExpressions(node syntaxNode) []syntaxNode {
	result := []syntaxNode{}
	for _, child := range node.children() {
		if child.node != nil && expressionKinds[child.node.green.Kind] {
			result = append(result, *child.node)
		}
	}
	return result
}

func directOperator(node syntaxNode) string {
	for _, child := range node.children() {
		if child.token == nil {
			continue
		}
		switch child.token.Kind {
		case "+", "-", "*", "/", "<", ">", "LTE", "GTE":
			return child.token.Text
		}
	}
	return ""
}

func (l *lowerer) lowerExpr(node syntaxNode) Expr {
	switch node.green.Kind {
	case "NAME_REF":
		return Expr{Kind: "Name", Name: node.directToken("IDENT"), Span: node.span()}
	case "LITERAL":
		for _, token := range node.tokens() {
			if significant(token.token.Kind) {
				return Expr{Kind: "Literal", Literal: token.token.Text, Span: node.span()}
			}
		}
	case "POSTFIX_EXPR":
		return l.lowerPostfix(node)
	case "USING_EXPR":
		parts := directExpressions(node)
		if len(parts) == 0 {
			break
		}
		result := l.lowerExpr(parts[0])
		for _, part := range parts[1:] {
			providers := l.lowerExpr(part)
			input := result
			result = Expr{
				Kind:     "Using",
				Span:     Span{Start: input.Span.Start, End: providers.Span.End},
				Receiver: &input,
				Fields:   providers.Fields,
			}
		}
		result.Span = node.span()
		return result
	case "PIPELINE_EXPR":
		parts := directExpressions(node)
		if len(parts) == 0 {
			break
		}
		result := l.lowerExpr(parts[0])
		if node.hasToken("MISSING_pipeline transformation") {
			input := result
			l.diagnostics = append(l.diagnostics, Diagnostic{Message: "pipeline has no transformation", Span: node.span()})
			return Expr{Kind: "Invalid", Origin: "pipeline", Left: &input, Span: node.span()}
		}
		for _, part := range parts[1:] {
			callee := l.lowerExpr(part)
			input := result
			result = Expr{
				Kind:      "Call",
				Origin:    "pipeline",
				Span:      Span{Start: input.Span.Start, End: callee.Span.End},
				Callee:    &callee,
				Arguments: []Argument{{Value: input}},
			}
		}
		result.Span = node.span()
		return result
	case "COMPARISON_EXPR", "ADDITIVE_EXPR", "MULTIPLICATIVE_EXPR":
		if result, ok := l.lowerBinary(node); ok {
			return result
		}
	case "UNARY_EXPR":
		if expression := directExpression(node); expression != nil {
			value := l.lowerExpr(*expression)
			return Expr{Kind: "Unary", Operator: directOperator(node), Right: &value, Span: node.span()}
		}
	case "LIST_EXPR":
		result := Expr{Kind: "List", Span: node.span()}
		for _, element := range directExpressions(node) {
			result.Elements = append(result.Elements, l.lowerExpr(element))
		}
		return result
	case "RECORD_EXPR":
		result := Expr{Kind: "Record", Span: node.span()}
		for _, fieldNode := range node.childNodes("RECORD_FIELD") {
			field := RecordField{Span: fieldNode.span()}
			if name := fieldNode.directToken("IDENT"); name != nil {
				field.Name = *name
			}
			if expression := directExpression(fieldNode); expression != nil {
				field.Value = l.lowerExpr(*expression)
			} else {
				field.Value = Expr{Kind: "Invalid", Span: fieldNode.span()}
				l.diagnostics = append(l.diagnostics, Diagnostic{Message: "record field has no valid expression", Span: fieldNode.span()})
			}
			result.Fields = append(result.Fields, field)
		}
		return result
	case "CASE_EXPR":
		result := Expr{Kind: "Case", Span: node.span()}
		for _, child := range node.children() {
			if child.node == nil {
				continue
			}
			if expressionKinds[child.node.green.Kind] {
				result.Subjects = append(result.Subjects, l.lowerExpr(*child.node))
			}
			if child.node.green.Kind == "CASE_ARM" {
				result.Arms = append(result.Arms, l.lowerCaseArm(*child.node))
			}
		}
		return result
	case "FUNCTION_EXPR":
		result := Expr{Kind: "Function", Span: node.span()}
		if params := node.child("VALUE_PARAMS"); params != nil {
			result.Parameters = l.lowerParameters(*params)
		}
		if contract := node.child("IMPLEMENTATION_CONTRACT"); contract != nil {
			if resultType := contract.child("TYPE"); resultType != nil {
				value := l.lowerType(*resultType)
				result.Return = &value
			}
			if requirements := contract.child("REQUIREMENT_CLAUSE"); requirements != nil {
				value := l.lowerRequirement(*requirements)
				result.Requires = &value
			}
		}
		if body := node.child("BLOCK_EXPR"); body != nil {
			value := l.lowerExpr(*body)
			result.Body = &value
		}
		return result
	case "BLOCK_EXPR":
		result := Expr{Kind: "Block", Span: node.span()}
		itemNodes := node.childNodes("BLOCK_ITEM")
		for index, itemNode := range itemNodes {
			item := BlockItem{}
			if bindingNode := itemNode.child("LET_BINDING"); bindingNode != nil {
				value := l.lowerBinding(*bindingNode)
				item.Binding = &value
			} else if expression := directExpression(itemNode); expression != nil {
				value := l.lowerExpr(*expression)
				if index == len(itemNodes)-1 {
					result.Result = &value
					continue
				}
				item.Expression = &value
			}
			result.Steps = append(result.Steps, item)
		}
		if result.Result == nil {
			l.diagnostics = append(l.diagnostics, Diagnostic{Message: "block has no final value expression", Span: node.span()})
		}
		return result
	case "GROUPED_EXPR":
		if expression := directExpression(node); expression != nil {
			result := l.lowerExpr(*expression)
			result.Span = node.span()
			return result
		}
	case "EMPTY_BRACE_EXPR", "ERROR_IF_EXPR", "ERROR":
		l.diagnostics = append(l.diagnostics, Diagnostic{Message: "cannot lower invalid syntax " + node.green.Kind, Span: node.span()})
		return Expr{Kind: "Invalid", Span: node.span()}
	}
	l.diagnostics = append(l.diagnostics, Diagnostic{Message: "lowering does not handle " + node.green.Kind, Span: node.span()})
	return Expr{Kind: "Invalid", Span: node.span()}
}

func (l *lowerer) lowerBinary(node syntaxNode) (Expr, bool) {
	var result Expr
	initialized := false
	operator := ""
	for _, child := range node.children() {
		if child.token != nil {
			switch child.token.Kind {
			case "+", "-", "*", "/", "<", ">", "LTE", "GTE":
				operator = child.token.Text
			}
			continue
		}
		if !expressionKinds[child.node.green.Kind] {
			continue
		}
		value := l.lowerExpr(*child.node)
		if !initialized {
			result = value
			initialized = true
			continue
		}
		left := result
		right := value
		result = Expr{Kind: "Binary", Operator: operator, Left: &left, Right: &right, Span: Span{Start: left.Span.Start, End: right.Span.End}}
		operator = ""
	}
	if initialized {
		result.Span = node.span()
	}
	return result, initialized
}

func (l *lowerer) lowerCaseArm(node syntaxNode) CaseArm {
	result := CaseArm{Span: node.span()}
	for _, patternNode := range node.childNodes("PATTERN") {
		result.Patterns = append(result.Patterns, l.lowerPattern(patternNode))
	}
	expressions := directExpressions(node)
	if node.hasDirectToken("KW_IF") && len(expressions) > 1 {
		guard := l.lowerExpr(expressions[0])
		result.Guard = &guard
		result.Body = l.lowerExpr(expressions[len(expressions)-1])
	} else if len(expressions) > 0 {
		result.Body = l.lowerExpr(expressions[len(expressions)-1])
	} else {
		result.Body = Expr{Kind: "Invalid", Span: node.span()}
		l.diagnostics = append(l.diagnostics, Diagnostic{Message: "case arm has no valid body", Span: node.span()})
	}
	return result
}

func (l *lowerer) lowerPattern(node syntaxNode) Pattern {
	result := Pattern{Span: node.span()}
	for _, child := range node.children() {
		if child.token != nil {
			switch child.token.Kind {
			case "INT", "TEXT", "TRUE", "FALSE":
				result.Kind = "LiteralPattern"
				result.Literal = child.token.Text
			case "IDENT":
				name := LocatedName{Text: child.token.Text, Span: Span{child.start, child.start + len(child.token.Text)}}
				result.Name = &name
				if child.token.Text == "_" {
					result.Kind = "WildcardPattern"
				} else {
					result.Kind = "NamePattern"
				}
			}
		}
		if child.node != nil && child.node.green.Kind == "PATTERN" {
			result.Arguments = append(result.Arguments, l.lowerPattern(*child.node))
		}
	}
	if len(result.Arguments) > 0 {
		result.Kind = "AppliedPattern"
	}
	if result.Kind == "" {
		result.Kind = "InvalidPattern"
		l.diagnostics = append(l.diagnostics, Diagnostic{Message: "cannot lower invalid pattern", Span: node.span()})
	}
	return result
}

func (l *lowerer) lowerPostfix(node syntaxNode) Expr {
	var result Expr
	initialized := false
	for _, child := range node.children() {
		if child.node == nil {
			continue
		}
		if !initialized && expressionKinds[child.node.green.Kind] {
			result = l.lowerExpr(*child.node)
			initialized = true
			continue
		}
		switch child.node.green.Kind {
		case "MEMBER_ACCESS":
			receiver := result
			result = Expr{Kind: "Select", Receiver: &receiver, Name: child.node.directToken("IDENT"), Span: Span{receiver.Span.Start, child.node.span().End}}
		case "CALL_SUFFIX":
			callee := result
			call := Expr{Kind: "Call", Callee: &callee, Span: Span{callee.Span.Start, child.node.span().End}}
			for _, argumentNode := range child.node.childNodes("ARGUMENT") {
				argument := Argument{}
				if argumentNode.hasDirectToken(":") {
					argument.Label = argumentNode.directToken("IDENT")
				}
				if expression := directExpression(argumentNode); expression != nil {
					argument.Value = l.lowerExpr(*expression)
				}
				call.Arguments = append(call.Arguments, argument)
			}
			result = call
		}
	}
	if !initialized {
		l.diagnostics = append(l.diagnostics, Diagnostic{Message: "postfix expression has no base", Span: node.span()})
		return Expr{Kind: "Invalid", Span: node.span()}
	}
	return result
}

func Format(result Result) []string {
	lines := []string{fmt.Sprintf("File %s", formatSpan(result.File.Span))}
	for _, item := range result.File.Items {
		if item.Import != nil {
			parts := []string{}
			for _, part := range item.Import.Path {
				parts = append(parts, part.Text)
			}
			detail := strings.Join(parts, "/")
			if item.Import.Alias != nil {
				detail += " as " + item.Import.Alias.Text
			}
			if len(item.Import.Members) > 0 {
				members := []string{}
				for _, member := range item.Import.Members {
					members = append(members, member.Text)
				}
				detail += ".{" + strings.Join(members, ", ") + "}"
			}
			lines = append(lines, fmt.Sprintf("  Import %s %s", detail, formatSpan(item.Import.Span)))
		}
		if item.Data != nil {
			formatData(&lines, *item.Data, "  ")
		}
		if item.Function != nil {
			formatFunction(&lines, *item.Function, "  ")
		}
		if item.Binding != nil {
			formatBinding(&lines, *item.Binding, "  ")
		}
		if item.Expression != nil {
			lines = append(lines, "  TopLevelExpression")
			formatExpr(&lines, *item.Expression, "    ")
		}
	}
	if len(result.Diagnostics) > 0 {
		lines = append(lines, "", "Lowering diagnostics")
		for _, diagnostic := range result.Diagnostics {
			lines = append(lines, fmt.Sprintf("  %s %s", diagnostic.Message, formatSpan(diagnostic.Span)))
		}
	}
	return lines
}

func formatData(lines *[]string, declaration DataDeclaration, indent string) {
	visibility := ""
	if declaration.Public {
		visibility = " public"
	}
	kind := declaration.Kind
	if kind != "" {
		kind = strings.ToUpper(kind[:1]) + kind[1:]
	}
	*lines = append(*lines, fmt.Sprintf("%s%s %s%s %s", indent, kind, declaration.Name.Text, visibility, formatSpan(declaration.Span)))
	for _, parameter := range declaration.Parameters {
		*lines = append(*lines, fmt.Sprintf("%s  Generic %s %s", indent, parameter.Text, formatSpan(parameter.Span)))
	}
	for _, field := range declaration.Fields {
		*lines = append(*lines, fmt.Sprintf("%s  Field %s %s", indent, field.Name.Text, formatSpan(field.Span)))
		formatType(lines, field.Type, indent+"    ", "type")
	}
	for _, variant := range declaration.Variants {
		*lines = append(*lines, fmt.Sprintf("%s  Variant %s %s", indent, variant.Name.Text, formatSpan(variant.Span)))
		for _, field := range variant.Fields {
			*lines = append(*lines, fmt.Sprintf("%s    Field %s %s", indent, field.Name.Text, formatSpan(field.Span)))
			formatType(lines, field.Type, indent+"      ", "type")
		}
	}
}

func formatFunction(lines *[]string, function Function, indent string) {
	visibility := ""
	if function.Public {
		visibility = " public"
	}
	*lines = append(*lines, fmt.Sprintf("%sFunction %s%s %s", indent, function.Name.Text, visibility, formatSpan(function.Span)))
	for _, parameter := range function.GenericParameters {
		*lines = append(*lines, fmt.Sprintf("%s  Generic %s %s", indent, parameter.Text, formatSpan(parameter.Span)))
	}
	for _, parameter := range function.Parameters {
		*lines = append(*lines, fmt.Sprintf("%s  Parameter %s %s", indent, parameter.Name.Text, formatSpan(parameter.Span)))
		if parameter.Type != nil {
			formatType(lines, *parameter.Type, indent+"    ", "type")
		} else {
			*lines = append(*lines, indent+"    type: infer")
		}
	}
	if function.Return != nil {
		formatType(lines, *function.Return, indent+"  ", "return annotation")
	} else {
		*lines = append(*lines, indent+"  return annotation: infer")
	}
	if function.Requirements != nil {
		formatRequirement(lines, *function.Requirements, indent+"  ", "requirements annotation")
	}
	*lines = append(*lines, indent+"  body")
	formatExpr(lines, function.Body, indent+"    ")
}

func formatBinding(lines *[]string, binding Binding, indent string) {
	*lines = append(*lines, fmt.Sprintf("%sBinding %s %s", indent, binding.Name.Text, formatSpan(binding.Span)))
	formatExpr(lines, binding.Value, indent+"  ")
}

func formatType(lines *[]string, value Type, indent, label string) {
	if value.Kind == "NamedType" {
		name := "<missing>"
		if len(value.Path) > 0 {
			parts := []string{}
			for _, part := range value.Path {
				parts = append(parts, part.Text)
			}
			name = strings.Join(parts, ".")
		} else if value.Name != nil {
			name = value.Name.Text
		}
		if len(value.Arguments) == 0 {
			*lines = append(*lines, fmt.Sprintf("%s%s: NamedType %s %s", indent, label, name, formatSpan(value.Span)))
			return
		}
		*lines = append(*lines, fmt.Sprintf("%s%s: NamedType %s %s", indent, label, name, formatSpan(value.Span)))
		for _, argument := range value.Arguments {
			formatType(lines, argument, indent+"  ", "argument")
		}
		return
	}
	*lines = append(*lines, fmt.Sprintf("%s%s: FunctionType %s", indent, label, formatSpan(value.Span)))
	for _, input := range value.Inputs {
		formatType(lines, input, indent+"  ", "input")
	}
	if value.Success != nil {
		formatType(lines, *value.Success, indent+"  ", "success")
	}
	if value.Errors != nil {
		formatType(lines, *value.Errors, indent+"  ", "errors channel")
	} else {
		*lines = append(*lines, indent+"  errors channel: omitted/infer")
	}
	if value.Requirements != nil {
		formatRequirement(lines, *value.Requirements, indent+"  ", "requirements channel")
	} else {
		*lines = append(*lines, indent+"  requirements channel: omitted/infer")
	}
}

func formatRequirement(lines *[]string, row RequirementRow, indent, label string) {
	*lines = append(*lines, fmt.Sprintf("%s%s: RequirementRow %s", indent, label, formatSpan(row.Span)))
	for _, term := range row.Terms {
		if term.Variable != nil {
			*lines = append(*lines, fmt.Sprintf("%s  RowVariable %s %s", indent, term.Variable.Text, formatSpan(term.Span)))
		}
		for _, field := range term.Fields {
			*lines = append(*lines, fmt.Sprintf("%s  Field %s", indent, field.Name.Text))
			formatType(lines, field.Type, indent+"    ", "type")
		}
	}
}

func formatExpr(lines *[]string, expression Expr, indent string) {
	switch expression.Kind {
	case "Name":
		*lines = append(*lines, fmt.Sprintf("%sName %s %s", indent, expression.Name.Text, formatSpan(expression.Span)))
	case "Literal":
		*lines = append(*lines, fmt.Sprintf("%sLiteral %s %s", indent, strconv.Quote(expression.Literal), formatSpan(expression.Span)))
	case "Select":
		*lines = append(*lines, fmt.Sprintf("%sSelect .%s %s", indent, expression.Name.Text, formatSpan(expression.Span)))
		formatExpr(lines, *expression.Receiver, indent+"  ")
	case "Call":
		origin := ""
		if expression.Origin != "" {
			origin = " origin=" + expression.Origin
		}
		*lines = append(*lines, fmt.Sprintf("%sCall%s %s", indent, origin, formatSpan(expression.Span)))
		*lines = append(*lines, indent+"  callee")
		formatExpr(lines, *expression.Callee, indent+"    ")
		for _, argument := range expression.Arguments {
			label := "positional"
			if argument.Label != nil {
				label = argument.Label.Text
			}
			*lines = append(*lines, indent+"  argument "+label)
			formatExpr(lines, argument.Value, indent+"    ")
		}
	case "Function":
		*lines = append(*lines, fmt.Sprintf("%sFunctionValue %s", indent, formatSpan(expression.Span)))
		for _, parameter := range expression.Parameters {
			*lines = append(*lines, fmt.Sprintf("%s  Parameter %s %s", indent, parameter.Name.Text, formatSpan(parameter.Span)))
			if parameter.Type != nil {
				formatType(lines, *parameter.Type, indent+"    ", "type")
			} else {
				*lines = append(*lines, indent+"    type: infer")
			}
		}
		if expression.Return != nil {
			formatType(lines, *expression.Return, indent+"  ", "return annotation")
		} else {
			*lines = append(*lines, indent+"  return annotation: infer")
		}
		if expression.Body != nil {
			formatExpr(lines, *expression.Body, indent+"  ")
		}
	case "Block":
		*lines = append(*lines, fmt.Sprintf("%sBlock %s", indent, formatSpan(expression.Span)))
		for _, item := range expression.Steps {
			*lines = append(*lines, indent+"  step")
			if item.Binding != nil {
				formatBinding(lines, *item.Binding, indent+"    ")
			}
			if item.Expression != nil {
				formatExpr(lines, *item.Expression, indent+"    ")
			}
		}
		if expression.Result != nil {
			*lines = append(*lines, indent+"  result")
			formatExpr(lines, *expression.Result, indent+"    ")
		} else {
			*lines = append(*lines, indent+"  result: missing")
		}
	case "Binary":
		*lines = append(*lines, fmt.Sprintf("%sBinary %s %s", indent, expression.Operator, formatSpan(expression.Span)))
		formatExpr(lines, *expression.Left, indent+"  ")
		formatExpr(lines, *expression.Right, indent+"  ")
	case "Unary":
		*lines = append(*lines, fmt.Sprintf("%sUnary %s %s", indent, expression.Operator, formatSpan(expression.Span)))
		formatExpr(lines, *expression.Right, indent+"  ")
	case "List":
		*lines = append(*lines, fmt.Sprintf("%sList %s", indent, formatSpan(expression.Span)))
		for _, element := range expression.Elements {
			formatExpr(lines, element, indent+"  ")
		}
	case "Record":
		*lines = append(*lines, fmt.Sprintf("%sRecord %s", indent, formatSpan(expression.Span)))
		for _, field := range expression.Fields {
			*lines = append(*lines, fmt.Sprintf("%s  Field %s %s", indent, field.Name.Text, formatSpan(field.Span)))
			formatExpr(lines, field.Value, indent+"    ")
		}
	case "Using":
		*lines = append(*lines, fmt.Sprintf("%sUsing %s", indent, formatSpan(expression.Span)))
		*lines = append(*lines, indent+"  computation")
		formatExpr(lines, *expression.Receiver, indent+"    ")
		for _, field := range expression.Fields {
			*lines = append(*lines, fmt.Sprintf("%s  provider %s %s", indent, field.Name.Text, formatSpan(field.Span)))
			formatExpr(lines, field.Value, indent+"    ")
		}
	case "Case":
		*lines = append(*lines, fmt.Sprintf("%sCase %s", indent, formatSpan(expression.Span)))
		for _, subject := range expression.Subjects {
			*lines = append(*lines, indent+"  subject")
			formatExpr(lines, subject, indent+"    ")
		}
		for _, arm := range expression.Arms {
			*lines = append(*lines, fmt.Sprintf("%s  arm %s", indent, formatSpan(arm.Span)))
			for _, pattern := range arm.Patterns {
				formatPattern(lines, pattern, indent+"    ")
			}
			if arm.Guard != nil {
				*lines = append(*lines, indent+"    guard")
				formatExpr(lines, *arm.Guard, indent+"      ")
			}
			*lines = append(*lines, indent+"    body")
			formatExpr(lines, arm.Body, indent+"      ")
		}
	case "Invalid":
		origin := ""
		if expression.Origin != "" {
			origin = " origin=" + expression.Origin
		}
		*lines = append(*lines, fmt.Sprintf("%sInvalid%s %s", indent, origin, formatSpan(expression.Span)))
		if expression.Left != nil {
			*lines = append(*lines, indent+"  preserved input")
			formatExpr(lines, *expression.Left, indent+"    ")
		}
	default:
		*lines = append(*lines, fmt.Sprintf("%s%s %s", indent, expression.Kind, formatSpan(expression.Span)))
	}
}

func formatPattern(lines *[]string, pattern Pattern, indent string) {
	detail := pattern.Literal
	if pattern.Name != nil {
		detail = pattern.Name.Text
	}
	*lines = append(*lines, fmt.Sprintf("%s%s %s %s", indent, pattern.Kind, detail, formatSpan(pattern.Span)))
	for _, argument := range pattern.Arguments {
		formatPattern(lines, argument, indent+"  ")
	}
}

func formatSpan(span Span) string {
	return fmt.Sprintf("@%d..%d", span.Start, span.End)
}

func SemanticNodeCount(result Result) int {
	count := 1
	var countType func(Type)
	var countExpr func(Expr)
	countType = func(value Type) {
		count++
		for _, input := range value.Inputs {
			countType(input)
		}
		if value.Success != nil {
			countType(*value.Success)
		}
		if value.Errors != nil {
			countType(*value.Errors)
		}
		for _, argument := range value.Arguments {
			countType(argument)
		}
		if value.Requirements != nil {
			count++
			for _, term := range value.Requirements.Terms {
				count++
				for _, field := range term.Fields {
					countType(field.Type)
				}
			}
		}
	}
	countExpr = func(expression Expr) {
		count++
		if expression.Receiver != nil {
			countExpr(*expression.Receiver)
		}
		if expression.Callee != nil {
			countExpr(*expression.Callee)
		}
		for _, argument := range expression.Arguments {
			countExpr(argument.Value)
		}
		for _, parameter := range expression.Parameters {
			count++
			if parameter.Type != nil {
				countType(*parameter.Type)
			}
		}
		if expression.Return != nil {
			countType(*expression.Return)
		}
		if expression.Body != nil {
			countExpr(*expression.Body)
		}
		if expression.Left != nil {
			countExpr(*expression.Left)
		}
		if expression.Right != nil {
			countExpr(*expression.Right)
		}
		for _, element := range expression.Elements {
			countExpr(element)
		}
		for _, field := range expression.Fields {
			count++
			countExpr(field.Value)
		}
		for _, subject := range expression.Subjects {
			countExpr(subject)
		}
		for _, arm := range expression.Arms {
			count++
			if arm.Guard != nil {
				countExpr(*arm.Guard)
			}
			countExpr(arm.Body)
		}
		for _, item := range expression.Steps {
			if item.Binding != nil {
				count++
				countExpr(item.Binding.Value)
			}
			if item.Expression != nil {
				countExpr(*item.Expression)
			}
		}
		if expression.Result != nil {
			countExpr(*expression.Result)
		}
	}
	for _, item := range result.File.Items {
		count++
		if item.Data != nil {
			count += len(item.Data.Parameters)
			for _, field := range item.Data.Fields {
				count++
				countType(field.Type)
			}
			for _, variant := range item.Data.Variants {
				count++
				for _, field := range variant.Fields {
					count++
					countType(field.Type)
				}
			}
		}
		if item.Function != nil {
			count += len(item.Function.GenericParameters)
			for _, parameter := range item.Function.Parameters {
				count++
				if parameter.Type != nil {
					countType(*parameter.Type)
				}
			}
			if item.Function.Return != nil {
				countType(*item.Function.Return)
			}
			countExpr(item.Function.Body)
		}
		if item.Binding != nil {
			countExpr(item.Binding.Value)
		}
		if item.Expression != nil {
			countExpr(*item.Expression)
		}
	}
	return count
}

func Summary(result Result) string {
	return strings.Join([]string{
		"Formatting tokens and comments are absent from the semantic representation.",
		"Each semantic node retains a byte span into the original source.",
		"Function-type errors and requirements remain channels on FunctionType.",
		"Omitted annotations remain omitted/infer; this stage does not infer types.",
		"Blocks separate intermediate steps from their final value expression.",
		"Pipelines normalize to calls but retain origin=pipeline for diagnostics.",
		"Using expressions remain explicit typed provision boundaries.",
		"Pattern names remain unresolved until name resolution identifies constructors and bindings.",
		"Calls and member selection are structural until later name resolution.",
	}, "\n")
}

var supportedCSTKinds = map[string]bool{
	"FILE": true, "IMPORT_DECL": true, "IMPORT_ALIAS": true,
	"IMPORT_MEMBERS": true, "IMPORT_MEMBER": true, "DECLARATION": true,
	"STRUCT_DECL": true, "ENUM_DECL": true, "INTERFACE_DECL": true,
	"FIELD": true, "VARIANT": true, "GENERIC_PARAMS": true, "GENERIC_PARAM": true,
	"NAMED_TYPE_FIELD": true, "FUNCTION_DECL": true, "VALUE_PARAMS": true,
	"VALUE_PARAM": true, "IMPLEMENTATION_CONTRACT": true, "TYPE": true,
	"FUNCTION_TYPE": true, "TYPE_ARGS": true, "REQUIREMENT_CLAUSE": true,
	"REQUIREMENT_RECORD": true, "LET_BINDING": true, "USING_EXPR": true, "PIPELINE_EXPR": true,
	"COMPARISON_EXPR": true, "ADDITIVE_EXPR": true, "MULTIPLICATIVE_EXPR": true,
	"UNARY_EXPR": true, "POSTFIX_EXPR": true, "MEMBER_ACCESS": true,
	"CALL_SUFFIX": true, "ARGUMENT": true, "LITERAL": true, "NAME_REF": true,
	"GROUPED_EXPR": true, "LIST_EXPR": true, "RECORD_EXPR": true,
	"RECORD_FIELD": true, "BLOCK_EXPR": true, "BLOCK_ITEM": true,
	"CASE_EXPR": true, "CASE_ARM": true, "PATTERN": true,
	"FUNCTION_EXPR": true, "EMPTY_BRACE_EXPR": true, "ERROR_IF_EXPR": true,
	"ERROR": true,
}

func UnsupportedCSTKinds(root *core.GreenNode) []string {
	unknown := map[string]bool{}
	var walk func(*core.GreenNode)
	walk = func(node *core.GreenNode) {
		if !supportedCSTKinds[node.Kind] {
			unknown[node.Kind] = true
		}
		for _, child := range node.Children {
			if child.Node != nil {
				walk(child.Node)
			}
		}
	}
	walk(root)
	result := []string{}
	for kind := range unknown {
		result = append(result, kind)
	}
	sort.Strings(result)
	return result
}

func TopLevelSyntaxItemCount(root *core.GreenNode) int {
	count := 0
	for _, child := range root.Children {
		if child.Node == nil {
			continue
		}
		switch child.Node.Kind {
		case "IMPORT_DECL", "DECLARATION", "LET_BINDING":
			count++
		default:
			if expressionKinds[child.Node.Kind] {
				count++
			}
		}
	}
	return count
}

func InvalidSpans(result Result, sourceLength int) []string {
	spanType := reflect.TypeOf(Span{})
	invalid := []string{}
	var walk func(reflect.Value, string)
	walk = func(value reflect.Value, path string) {
		if !value.IsValid() {
			return
		}
		if value.Kind() == reflect.Pointer {
			if value.IsNil() {
				return
			}
			walk(value.Elem(), path)
			return
		}
		if value.Type() == spanType {
			span := value.Interface().(Span)
			if span.Start < 0 || span.End < span.Start || span.End > sourceLength {
				invalid = append(invalid, fmt.Sprintf("%s=%s", path, formatSpan(span)))
			}
			return
		}
		switch value.Kind() {
		case reflect.Struct:
			for index := 0; index < value.NumField(); index++ {
				walk(value.Field(index), path+"."+value.Type().Field(index).Name)
			}
		case reflect.Slice:
			for index := 0; index < value.Len(); index++ {
				walk(value.Index(index), fmt.Sprintf("%s[%d]", path, index))
			}
		}
	}
	walk(reflect.ValueOf(result), "result")
	return invalid
}
