package main

import (
	"bufio"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"chimi-lossless-parser-prototype/core"
	"chimi-semantic-lowering-prototype/lower"
	"chimi-tooling-seams-prototype/tooling"
)

const (
	bold  = "\x1b[1m"
	dim   = "\x1b[2m"
	reset = "\x1b[0m"
)

type scenario struct {
	Name        string
	Question    string
	Source      string
	Parsed      core.Result
	Lowered     lower.Result
	Formatted   tooling.FormatResult
	Diagnostics []tooling.Diagnostic
	Inspection  tooling.Inspection
}

func repositoryRoot() string {
	directory, err := os.Getwd()
	if err != nil {
		panic(err)
	}
	for {
		if _, err := os.Stat(filepath.Join(directory, "CONTEXT.md")); err == nil {
			return directory
		}
		parent := filepath.Dir(directory)
		if parent == directory {
			panic("cannot locate repository root")
		}
		directory = parent
	}
}

func read(root, relative string) string {
	bytes, err := os.ReadFile(filepath.Join(root, relative))
	if err != nil {
		panic(err)
	}
	return string(bytes)
}

func analyze(name, question, source, marker string, additional []tooling.Diagnostic) scenario {
	parsed := core.Parse(source)
	lowered := lower.Lower(parsed.Root)
	formatted := tooling.Format(parsed)
	diagnostics := tooling.MergeDiagnostics(
		tooling.SyntaxDiagnostics(parsed),
		tooling.LoweringDiagnostics(lowered),
		additional,
	)
	offset := 0
	if marker != "" {
		offset = strings.Index(source, marker)
		if offset < 0 {
			panic("inspection marker not found: " + marker)
		}
	}
	return scenario{
		Name: name, Question: question, Source: source, Parsed: parsed,
		Lowered: lowered, Formatted: formatted, Diagnostics: diagnostics,
		Inspection: tooling.Inspect(parsed, lowered, offset),
	}
}

func scenarios(root string) []scenario {
	messy := `// comments survive canonical formatting
struct   Money{cents:Int}
fn add(left:Money,right:Money)->Money{Money(cents:left.cents+right.cents)}
let total=add(Money(cents:1),Money(cents:2))
`
	invalid := read(root, "prototypes/lossless-parser/cases/missing_record_colon.chimi")
	nonExhaustive := read(root, "prototypes/golden/invalid/non_exhaustive_case.chimi")
	spanStart := strings.Index(nonExhaustive, "case state")
	typeDiagnostic := tooling.Diagnostic{
		Stage: "type", Code: "E_CASE_NOT_EXHAUSTIVE", Severity: "error",
		Message: "case does not handle CheckoutState.Submitted",
		Primary: tooling.Span{Start: spanStart, End: spanStart + len("case state")},
		Labels: []tooling.Label{{
			Span:    tooling.Span{Start: spanStart, End: spanStart + len("case state")},
			Message: "missing pattern Submitted(_)",
		}},
	}
	inspection := read(root, "prototypes/golden/valid/modules/use_money.chimi")
	return []scenario{
		analyze("canonical formatting", "Can CST tokens produce deterministic formatting while preserving comments?", messy, "left.cents", nil),
		analyze("malformed source", "Does formatting refuse malformed source while diagnostics and inspection remain available?", invalid, "retry_limit", nil),
		analyze("staged diagnostic", "Can a type diagnostic use the same source-mapped envelope without being mistaken for syntax?", nonExhaustive, "case state", []tooling.Diagnostic{typeDiagnostic}),
		analyze("syntax plus semantics", "Can one cursor query return both the lossless syntax path and lowered semantic owner?", inspection, "Money.cents", nil),
	}
}

func check(items []scenario) bool {
	passed := 0
	for _, item := range items {
		problems := []string{}
		switch item.Name {
		case "canonical formatting":
			second := tooling.Format(core.Parse(item.Formatted.Source))
			if item.Formatted.Blocked || !item.Formatted.Changed {
				problems = append(problems, "valid messy source was not formatted")
			}
			if second.Source != item.Formatted.Source {
				problems = append(problems, "formatter is not idempotent")
			}
			if !strings.Contains(item.Formatted.Source, "// comments survive") {
				problems = append(problems, "comment was lost")
			}
		case "malformed source":
			if !item.Formatted.Blocked || item.Formatted.Source != item.Source {
				problems = append(problems, "malformed source was rewritten")
			}
			if len(item.Diagnostics) == 0 || len(item.Inspection.Syntax) == 0 {
				problems = append(problems, "malformed source lost diagnostics or inspection")
			}
		case "staged diagnostic":
			if len(item.Parsed.Diagnostics) != 0 || !hasStage(item.Diagnostics, "type") {
				problems = append(problems, "type diagnostic was confused with syntax")
			}
		case "syntax plus semantics":
			if len(item.Inspection.Syntax) < 2 || len(item.Inspection.Semantic) < 2 {
				problems = append(problems, "inspection did not compose both representations")
			}
		}
		status := "PASS"
		if len(problems) > 0 {
			status = "FAIL: " + strings.Join(problems, "; ")
		} else {
			passed++
		}
		fmt.Printf("%-24s %s\n", item.Name, status)
	}
	fmt.Printf("\n%d/%d tooling scenarios satisfy the proposed seams\n", passed, len(items))
	return passed == len(items)
}

func hasStage(diagnostics []tooling.Diagnostic, stage string) bool {
	for _, diagnostic := range diagnostics {
		if diagnostic.Stage == stage {
			return true
		}
	}
	return false
}

func render(item scenario, index, total int, mode string) {
	fmt.Print("\x1b[2J\x1b[H")
	fmt.Printf("%sChimi tooling seams%s  %s%d/%d%s\n\n", bold, reset, dim, index+1, total, reset)
	fmt.Printf("%sscenario%s: %s\n", bold, reset, item.Name)
	fmt.Printf("%squestion%s: %s\n", bold, reset, item.Question)
	fmt.Printf("%sformat%s:   changed=%v blocked=%v\n", bold, reset, item.Formatted.Changed, item.Formatted.Blocked)
	fmt.Printf("%sdiagnostics%s: %d\n", bold, reset, len(item.Diagnostics))
	fmt.Printf("%sinspect%s:  %d:%d syntax-depth=%d semantic-facts=%d\n\n", bold, reset,
		item.Inspection.Line, item.Inspection.Column, len(item.Inspection.Syntax), len(item.Inspection.Semantic))

	switch mode {
	case "source":
		fmt.Printf("%sOriginal source%s\n%s", bold, reset, item.Source)
	case "formatted":
		fmt.Printf("%sFormatter result%s\n%s", bold, reset, item.Formatted.Source)
	case "diagnostics":
		fmt.Printf("%sUnified diagnostics%s\n", bold, reset)
		if len(item.Diagnostics) == 0 {
			fmt.Println("No diagnostics.")
		}
		for _, diagnostic := range item.Diagnostics {
			line, column := core.LineColumn(item.Source, diagnostic.Primary.Start)
			fmt.Printf("[%s] %s %d:%d %s\n", diagnostic.Stage, diagnostic.Code, line, column, diagnostic.Message)
		}
	case "inspection":
		fmt.Printf("%sInspection at %d:%d%s\n", bold, item.Inspection.Line, item.Inspection.Column, reset)
		fmt.Printf("token: %s\nsyntax path:\n", item.Inspection.Token)
		for _, node := range item.Inspection.Syntax {
			fmt.Printf("  %s %d..%d\n", node.Kind, node.Span.Start, node.Span.End)
		}
		fmt.Println("semantic owners:")
		for _, fact := range item.Inspection.Semantic {
			fmt.Printf("  %s %s %d..%d\n", fact.Kind, fact.Name, fact.Span.Start, fact.Span.End)
		}
	default:
		fmt.Println("Use the views below to judge the proposed boundary.")
	}
	fmt.Printf("\n%s[n]%s next  %s[p]%s previous  %s[s]%s source  %s[f]%s formatted  %s[d]%s diagnostics  %s[i]%s inspect  %s[q]%s quit\n",
		bold, reset, bold, reset, bold, reset, bold, reset, bold, reset, bold, reset, bold, reset)
}

func interactive(items []scenario) {
	reader := bufio.NewReader(os.Stdin)
	index, mode := 0, "summary"
	for {
		render(items[index], index, len(items), mode)
		fmt.Print("> ")
		line, err := reader.ReadString('\n')
		if err != nil {
			return
		}
		switch strings.TrimSpace(strings.ToLower(line)) {
		case "q":
			return
		case "", "n":
			index, mode = (index+1)%len(items), "summary"
		case "p":
			index, mode = (index-1+len(items))%len(items), "summary"
		case "s":
			mode = "source"
		case "f":
			mode = "formatted"
		case "d":
			mode = "diagnostics"
		case "i":
			mode = "inspection"
		}
	}
}

func main() {
	checkFlag := flag.Bool("check", false, "check all tooling seam scenarios")
	flag.Parse()
	items := scenarios(repositoryRoot())
	if *checkFlag {
		if !check(items) {
			os.Exit(1)
		}
		return
	}
	interactive(items)
}
