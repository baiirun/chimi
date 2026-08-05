// Package tooling contains pure formatting, diagnostic, and inspection seams
// for the disposable Chimi tooling prototype.
package tooling

import (
	"fmt"
	"sort"
	"strings"

	"chimi-lossless-parser-prototype/core"
	"chimi-semantic-lowering-prototype/lower"
)

type Span struct {
	Start int `json:"start"`
	End   int `json:"end"`
}

type Label struct {
	Span    Span   `json:"span"`
	Message string `json:"message"`
}

type Edit struct {
	Span        Span   `json:"span"`
	Replacement string `json:"replacement"`
}

type Diagnostic struct {
	Stage    string  `json:"stage"`
	Code     string  `json:"code"`
	Severity string  `json:"severity"`
	Message  string  `json:"message"`
	Primary  Span    `json:"primary"`
	Labels   []Label `json:"labels,omitempty"`
	Fixes    []Edit  `json:"fixes,omitempty"`
}

type FormatResult struct {
	Source      string
	Changed     bool
	Blocked     bool
	Diagnostics []Diagnostic
}

type NodeView struct {
	Kind string
	Span Span
}

type SemanticFact struct {
	Kind string
	Name string
	Span Span
}

type Inspection struct {
	Offset   int
	Line     int
	Column   int
	Token    string
	Syntax   []NodeView
	Semantic []SemanticFact
}

func SyntaxDiagnostics(parsed core.Result) []Diagnostic {
	result := make([]Diagnostic, 0, len(parsed.Diagnostics))
	for _, item := range parsed.Diagnostics {
		end := item.Offset
		if end < len(parsed.Source) {
			end++
		}
		result = append(result, Diagnostic{
			Stage: "syntax", Code: syntaxCode(item.Message), Severity: "error",
			Message: item.Message, Primary: Span{item.Offset, end},
		})
	}
	return result
}

func LoweringDiagnostics(lowered lower.Result) []Diagnostic {
	result := make([]Diagnostic, 0, len(lowered.Diagnostics))
	for _, item := range lowered.Diagnostics {
		result = append(result, Diagnostic{
			Stage: "lowering", Code: "E_INVALID_SEMANTIC_FORM", Severity: "error",
			Message: item.Message, Primary: Span{item.Span.Start, item.Span.End},
		})
	}
	return result
}

func MergeDiagnostics(groups ...[]Diagnostic) []Diagnostic {
	result := []Diagnostic{}
	for _, group := range groups {
		result = append(result, group...)
	}
	sort.SliceStable(result, func(i, j int) bool {
		if result[i].Primary.Start != result[j].Primary.Start {
			return result[i].Primary.Start < result[j].Primary.Start
		}
		return stageOrder(result[i].Stage) < stageOrder(result[j].Stage)
	})
	return result
}

func stageOrder(stage string) int {
	switch stage {
	case "syntax":
		return 0
	case "lowering":
		return 1
	case "name":
		return 2
	case "type":
		return 3
	default:
		return 4
	}
}

func syntaxCode(message string) string {
	switch {
	case strings.Contains(message, "explicit alias"):
		return "E_MISSING_MODULE_ALIAS"
	case strings.Contains(message, "standalone if"):
		return "E_STANDALONE_IF"
	case strings.Contains(message, "Void"):
		return "E_VOID_VALUE"
	case strings.Contains(message, "empty"):
		return "E_EMPTY_FORM"
	case strings.HasPrefix(message, "expected"):
		return "E_EXPECTED_TOKEN"
	default:
		return "E_INVALID_SYNTAX"
	}
}

// Format returns the original source unchanged when syntax diagnostics exist.
// The prototype deliberately avoids guessing how malformed syntax should look.
func Format(parsed core.Result) FormatResult {
	diagnostics := SyntaxDiagnostics(parsed)
	if len(diagnostics) > 0 {
		return FormatResult{Source: parsed.Source, Blocked: true, Diagnostics: diagnostics}
	}
	tokens, lexical := core.Lex(parsed.Source)
	if len(lexical) > 0 {
		return FormatResult{Source: parsed.Source, Blocked: true, Diagnostics: SyntaxDiagnostics(parsed)}
	}
	formatted := formatTokens(tokens)
	return FormatResult{Source: formatted, Changed: formatted != parsed.Source}
}

type writer struct {
	output         strings.Builder
	line           strings.Builder
	indent         int
	nextLineIndent int
	braceDepth     int
}

func (w *writer) text() string {
	w.flush()
	return strings.TrimRight(w.output.String(), "\n") + "\n"
}

func (w *writer) write(text string) { w.line.WriteString(text) }

func (w *writer) trimSpace() {
	value := strings.TrimRight(w.line.String(), " ")
	w.line.Reset()
	w.line.WriteString(value)
}

func (w *writer) space() {
	if w.line.Len() > 0 && !strings.HasSuffix(w.line.String(), " ") {
		w.line.WriteByte(' ')
	}
}

func (w *writer) flush() {
	value := strings.TrimSpace(w.line.String())
	if value == "" {
		w.line.Reset()
		return
	}
	w.output.WriteString(strings.Repeat("  ", w.indent+w.nextLineIndent))
	w.output.WriteString(value)
	w.output.WriteByte('\n')
	w.line.Reset()
	w.nextLineIndent = 0
}

func formatTokens(tokens []core.Token) string {
	w := &writer{}
	previous := ""
	for index, token := range tokens {
		if token.Kind == "EOF" || token.Kind == "WHITESPACE" || token.Kind == "NEWLINE" {
			continue
		}
		next := nextSignificant(tokens, index+1)
		switch token.Kind {
		case "COMMENT":
			if w.line.Len() > 0 {
				w.space()
			}
			w.write(token.Text)
			w.flush()
		case "{":
			w.space()
			w.write("{")
			w.flush()
			w.indent++
			w.braceDepth++
		case "}":
			w.flush()
			if w.indent > 0 {
				w.indent--
			}
			if w.braceDepth > 0 {
				w.braceDepth--
			}
			w.write("}")
			if next != "," && next != ")" && next != "KW_USING" {
				w.flush()
				if w.braceDepth == 0 {
					w.output.WriteByte('\n')
				}
			}
		case ",":
			w.trimSpace()
			w.write(",")
			w.flush()
			w.nextLineIndent = 1
		case ":":
			w.trimSpace()
			w.write(": ")
		case ".":
			w.trimSpace()
			w.write(".")
		case "(":
			w.trimSpace()
			w.write("(")
		case ")":
			w.trimSpace()
			w.write(")")
		case "[":
			w.trimSpace()
			w.write("[")
		case "]":
			w.trimSpace()
			w.write("]")
		case "PIPE":
			w.flush()
			w.write("|> ")
		case "ARROW", "=", "+", "-", "*", "/", "GTE", "LTE", ">", "<":
			w.space()
			w.write(token.Text)
			w.write(" ")
		default:
			if needsSpace(previous, token.Kind, w.line.String()) {
				w.space()
			}
			w.write(token.Text)
		}
		previous = token.Kind
	}
	return w.text()
}

func needsSpace(previous, current, line string) bool {
	if line == "" || strings.HasSuffix(line, " ") {
		return false
	}
	if previous == "." || previous == "(" || previous == "[" {
		return false
	}
	return wordKind(previous) && wordKind(current) || previous == ")" || previous == "]"
}

func wordKind(kind string) bool {
	return kind == "IDENT" || kind == "INT" || kind == "TEXT" || kind == "TRUE" || kind == "FALSE" || kind == "VOID" || strings.HasPrefix(kind, "KW_")
}

func nextSignificant(tokens []core.Token, start int) string {
	for _, token := range tokens[start:] {
		if token.Kind != "WHITESPACE" && token.Kind != "NEWLINE" {
			return token.Kind
		}
	}
	return "EOF"
}

func Inspect(parsed core.Result, lowered lower.Result, offset int) Inspection {
	if offset < 0 {
		offset = 0
	}
	if offset > len(parsed.Source) {
		offset = len(parsed.Source)
	}
	line, column := core.LineColumn(parsed.Source, offset)
	result := Inspection{Offset: offset, Line: line, Column: column}
	result.Syntax, result.Token = syntaxPath(parsed.Root, offset)
	result.Semantic = semanticFacts(lowered, offset)
	return result
}

func syntaxPath(root *core.GreenNode, offset int) ([]NodeView, string) {
	path := []NodeView{}
	token := ""
	var walk func(*core.GreenNode, int)
	walk = func(node *core.GreenNode, start int) {
		end := start + node.TextLen
		if offset < start || offset > end {
			return
		}
		path = append(path, NodeView{Kind: node.Kind, Span: Span{start, end}})
		current := start
		for _, child := range node.Children {
			length := 0
			if child.Token != nil {
				length = len(child.Token.Text)
			}
			if child.Node != nil {
				length = child.Node.TextLen
			}
			if offset >= current && (offset < current+length || length == 0 && offset == current) {
				if child.Node != nil {
					walk(child.Node, current)
				} else {
					token = fmt.Sprintf("%s %q %d..%d", child.Token.Kind, child.Token.Text, current, current+length)
				}
				return
			}
			current += length
		}
	}
	walk(root, 0)
	return path, token
}

func semanticFacts(result lower.Result, offset int) []SemanticFact {
	facts := []SemanticFact{{Kind: "file", Span: Span{result.File.Span.Start, result.File.Span.End}}}
	for _, item := range result.File.Items {
		switch {
		case item.Import != nil:
			facts = appendIfContains(facts, "import", strings.Join(names(item.Import.Path), "/"), item.Import.Span, offset)
		case item.Data != nil:
			facts = appendIfContains(facts, item.Data.Kind, item.Data.Name.Text, item.Data.Span, offset)
		case item.Function != nil:
			facts = appendIfContains(facts, "function", item.Function.Name.Text, item.Function.Span, offset)
		case item.Binding != nil:
			facts = appendIfContains(facts, "binding", item.Binding.Name.Text, item.Binding.Span, offset)
		case item.Expression != nil:
			facts = appendIfContains(facts, "expression", item.Expression.Kind, item.Expression.Span, offset)
		}
	}
	filtered := facts[:0]
	for _, fact := range facts {
		if contains(fact.Span, offset) {
			filtered = append(filtered, fact)
		}
	}
	return filtered
}

func appendIfContains(facts []SemanticFact, kind, name string, span lower.Span, offset int) []SemanticFact {
	converted := Span{span.Start, span.End}
	if contains(converted, offset) {
		facts = append(facts, SemanticFact{Kind: kind, Name: name, Span: converted})
	}
	return facts
}

func contains(span Span, offset int) bool { return offset >= span.Start && offset <= span.End }

func names(values []lower.LocatedName) []string {
	result := make([]string, len(values))
	for index, value := range values {
		result[index] = value.Text
	}
	return result
}
