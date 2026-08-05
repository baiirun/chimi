package main

import (
	"bufio"
	"flag"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"time"

	"chimi-lossless-parser-prototype/core"
	"chimi-name-resolution-prototype/resolve"
	"chimi-semantic-lowering-prototype/lower"
)

const (
	bold  = "\x1b[1m"
	dim   = "\x1b[2m"
	reset = "\x1b[0m"
)

type scenario struct {
	name          string
	description   string
	modules       []resolve.ModuleInput
	expectedCodes []string
	requiredRefs  []string
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

func loadModule(root, modulePath, sourcePath string) resolve.ModuleInput {
	bytes, err := os.ReadFile(filepath.Join(root, sourcePath))
	if err != nil {
		panic(err)
	}
	parsed := core.Parse(string(bytes))
	if len(parsed.Diagnostics) > 0 {
		panic("name-resolution fixture has syntax diagnostics: " + sourcePath)
	}
	semantic := lower.Lower(parsed.Root)
	if len(semantic.Diagnostics) > 0 {
		panic("name-resolution fixture has lowering diagnostics: " + sourcePath)
	}
	return resolve.ModuleInput{Path: modulePath, File: semantic.File}
}

func scenarios(root string) []scenario {
	money := loadModule(root, "modules/money", "prototypes/golden/valid/modules/money.chimi")
	useMoney := loadModule(root, "modules/use_money", "prototypes/golden/valid/modules/use_money.chimi")
	privateConstructor := loadModule(root, "cases/private_constructor", "prototypes/golden/invalid/private_struct_construction.chimi")
	publicEnumConstructor := loadModule(root, "cases/public_enum_constructor", "prototypes/name-resolution/cases/public_enum_constructor.chimi")
	explicitConstructorImport := loadModule(root, "cases/explicit_constructor_import", "prototypes/name-resolution/cases/explicit_constructor_import.chimi")
	unqualifiedFunctionImport := loadModule(root, "cases/unqualified_function_import", "prototypes/name-resolution/cases/unqualified_function_import.chimi")
	unqualified := loadModule(root, "cases/unqualified_external", "prototypes/name-resolution/cases/unqualified_external.chimi")
	billingMoney := loadModule(root, "billing/money", "prototypes/name-resolution/modules/billing/money.chimi")
	ledgerMoney := loadModule(root, "ledger/money", "prototypes/name-resolution/modules/ledger/money.chimi")
	ambiguous := loadModule(root, "cases/ambiguous_import", "prototypes/name-resolution/cases/ambiguous_import.chimi")
	aliased := loadModule(root, "cases/aliased_modules", "prototypes/name-resolution/cases/aliased_modules.chimi")
	explicitGenerics := loadModule(root, "cases/explicit_generics", "prototypes/name-resolution/cases/explicit_generics.chimi")
	undeclaredGeneric := loadModule(root, "cases/undeclared_generic", "prototypes/name-resolution/cases/undeclared_generic.chimi")
	return []scenario{
		{
			name:        "qualified module success",
			description: "UseMoney explicitly binds the imported module as Money; external values and types retain that visible origin.",
			modules:     []resolve.ModuleInput{money, useMoney},
			requiredRefs: []string{
				"module::modules/money",
				"modules/money::type::Money",
				"modules/money::value::from_cents",
				"modules/money::value::cents",
			},
		},
		{
			name:          "private representation",
			description:   "Publishing Money exposes its type but not the implicit struct constructor.",
			modules:       []resolve.ModuleInput{money, privateConstructor},
			expectedCodes: []string{"E_PRIVATE_STRUCT_CONSTRUCTOR"},
		},
		{
			name:        "public enum constructor",
			description: "Publishing an enum publishes its constructors through the defining module origin.",
			modules:     []resolve.ModuleInput{money, publicEnumConstructor},
			requiredRefs: []string{
				"modules/money::constructor::NegativeAmount",
			},
		},
		{
			name:        "explicit constructor import",
			description: "A public constructor enters the value namespace only when the import names it explicitly.",
			modules:     []resolve.ModuleInput{money, explicitConstructorImport},
			requiredRefs: []string{
				"resolved explicit import", "modules/money::constructor::NegativeAmount",
			},
		},
		{
			name:          "function member import rejected",
			description:   "Even an explicit member list cannot erase a function's module origin at call sites.",
			modules:       []resolve.ModuleInput{money, unqualifiedFunctionImport},
			expectedCodes: []string{"E_UNQUALIFIED_FUNCTION_IMPORT"},
		},
		{
			name:          "no member injection",
			description:   "Binding a module as Money does not make from_cents available as an unqualified local value.",
			modules:       []resolve.ModuleInput{money, unqualified},
			expectedCodes: []string{"E_UNRESOLVED_VALUE"},
		},
		{
			name:          "import binding collision",
			description:   "Two imports explicitly choosing the same module alias are rejected.",
			modules:       []resolve.ModuleInput{billingMoney, ledgerMoney, ambiguous},
			expectedCodes: []string{"E_IMPORT_BINDING_CONFLICT"},
		},
		{
			name:        "explicit module aliases",
			description: "Explicit aliases let same-basename modules coexist while every call remains qualified.",
			modules:     []resolve.ModuleInput{billingMoney, ledgerMoney, aliased},
			requiredRefs: []string{
				"module::billing/money", "module::ledger/money",
			},
		},
		{
			name:        "explicit generic parameters",
			description: "Declared type, failure-row, and needs-row parameters resolve in the function contract.",
			modules:     []resolve.ModuleInput{explicitGenerics},
			requiredRefs: []string{
				"resolved generic", "resolved generic row",
			},
		},
		{
			name:          "undeclared generic rejected",
			description:   "An unresolved uppercase type name is not silently promoted to an implicit generic.",
			modules:       []resolve.ModuleInput{undeclaredGeneric},
			expectedCodes: []string{"E_UNRESOLVED_TYPE", "E_UNRESOLVED_TYPE"},
		},
	}
}

func scenarioResult(item scenario) (resolve.Result, []string) {
	result := resolve.Resolve(item.modules)
	actualCodes := resolve.DiagnosticCodes(result)
	expectedCodes := append([]string{}, item.expectedCodes...)
	sort.Strings(expectedCodes)
	problems := []string{}
	if strings.Join(actualCodes, ",") != strings.Join(expectedCodes, ",") {
		problems = append(problems, fmt.Sprintf("diagnostics actual=%v expected=%v", actualCodes, expectedCodes))
	}
	formatted := strings.Join(resolve.Format(result), "\n")
	for _, reference := range item.requiredRefs {
		if !strings.Contains(formatted, reference) {
			problems = append(problems, "missing reference "+reference)
		}
	}
	return result, problems
}

func check(all []scenario) bool {
	passed := 0
	for _, item := range all {
		result, problems := scenarioResult(item)
		status := "PASS"
		if len(problems) == 0 {
			passed++
		} else {
			status = "MISMATCH"
		}
		fmt.Printf("%-8s %-30s diagnostics=%v\n", status, item.name, resolve.DiagnosticCodes(result))
		for _, problem := range problems {
			fmt.Println("         " + problem)
		}
	}
	fmt.Printf("\n%d/%d scenarios satisfy name-resolution expectations\n", passed, len(all))
	return passed == len(all)
}

func render(all []scenario, index, scroll int) {
	item := all[index]
	result, problems := scenarioResult(item)
	lines := resolve.Format(result)
	fmt.Print("\x1b[2J\x1b[H")
	fmt.Printf("%sChimi name/module resolution prototype%s  %s%d/%d%s\n\n", bold, reset, dim, index+1, len(all), reset)
	fmt.Printf("%sscenario%s:    %s\n", bold, reset, item.name)
	fmt.Printf("%squestion%s:    %s\n", bold, reset, item.description)
	fmt.Printf("%sexpected%s:    %v\n", bold, reset, item.expectedCodes)
	fmt.Printf("%sactual%s:      %v\n", bold, reset, resolve.DiagnosticCodes(result))
	fmt.Printf("%scontract%s:    %v\n\n", bold, reset, len(problems) == 0)
	if scroll < 0 {
		scroll = 0
	}
	if scroll >= len(lines) && len(lines) > 0 {
		scroll = len(lines) - 1
	}
	end := scroll + 24
	if end > len(lines) {
		end = len(lines)
	}
	for _, line := range lines[scroll:end] {
		fmt.Println(line)
	}
	if end < len(lines) {
		fmt.Printf("%s... %d more lines%s\n", dim, len(lines)-end, reset)
	}
	fmt.Println()
	fmt.Printf("%s[n/p]%s scenario  %s[j/k]%s scroll  %s[c]%s check all  %s[q]%s quit\n", bold, reset, bold, reset, bold, reset, bold, reset)
}

func interactive(all []scenario) {
	reader := bufio.NewReader(os.Stdin)
	index := 0
	scroll := 0
	for {
		render(all, index, scroll)
		fmt.Print("> ")
		line, err := reader.ReadString('\n')
		if err != nil {
			return
		}
		switch strings.TrimSpace(line) {
		case "q":
			return
		case "n":
			index = (index + 1) % len(all)
			scroll = 0
		case "p":
			index = (index - 1 + len(all)) % len(all)
			scroll = 0
		case "j":
			scroll += 18
		case "k":
			scroll -= 18
		case "c":
			fmt.Print("\x1b[2J\x1b[H")
			check(all)
			fmt.Print("\npress enter to return ")
			reader.ReadString('\n')
		}
	}
}

func benchmarkModule(declarations int) resolve.ModuleInput {
	file := lower.File{}
	for index := 0; index < declarations; index++ {
		name := fmt.Sprintf("function_%05d", index)
		parameterName := fmt.Sprintf("value_%05d", index)
		span := lower.Span{Start: index * 20, End: index*20 + 10}
		parameter := lower.Parameter{
			Name: lower.LocatedName{Text: parameterName, Span: span},
			Type: &lower.Type{Kind: "NamedType", Name: &lower.LocatedName{Text: "Int", Span: span}, Path: []lower.LocatedName{{Text: "Int", Span: span}}, Span: span},
			Span: span,
		}
		resultExpr := lower.Expr{Kind: "Name", Name: &parameter.Name, Span: span}
		body := lower.Expr{Kind: "Block", Result: &resultExpr, Span: span}
		function := lower.Function{Name: lower.LocatedName{Text: name, Span: span}, Public: true, Parameters: []lower.Parameter{parameter}, Body: body, Span: span}
		file.Items = append(file.Items, lower.Item{Function: &function})
	}
	return resolve.ModuleInput{Path: "benchmark/generated", File: file}
}

func percentile(durations []time.Duration, percentile float64) time.Duration {
	ordered := append([]time.Duration{}, durations...)
	sort.Slice(ordered, func(i, j int) bool { return ordered[i] < ordered[j] })
	index := int(math.Ceil(float64(len(ordered))*percentile)) - 1
	return ordered[index]
}

func benchmark() {
	const declarations = 10_000
	input := benchmarkModule(declarations)
	for warmup := 0; warmup < 3; warmup++ {
		resolve.Resolve([]resolve.ModuleInput{input})
	}
	runtime.GC()
	var before runtime.MemStats
	var after runtime.MemStats
	runtime.ReadMemStats(&before)
	durations := make([]time.Duration, 0, 40)
	var result resolve.Result
	for iteration := 0; iteration < 40; iteration++ {
		started := time.Now()
		result = resolve.Resolve([]resolve.ModuleInput{input})
		durations = append(durations, time.Since(started))
	}
	runtime.ReadMemStats(&after)
	allocated := after.TotalAlloc - before.TotalAlloc
	fmt.Println("Chimi name-resolution prototype benchmark")
	fmt.Printf("host=%s/%s go=%s\n", runtime.GOOS, runtime.GOARCH, runtime.Version())
	fmt.Printf("declarations=%d diagnostics=%d\n", declarations, len(resolve.DiagnosticCodes(result)))
	fmt.Printf("resolve p50=%s p95=%s allocation/declaration=%.0f bytes\n",
		percentile(durations, 0.50).Round(time.Microsecond),
		percentile(durations, 0.95).Round(time.Microsecond),
		float64(allocated)/float64(40*declarations),
	)
}

func main() {
	checkFlag := flag.Bool("check", false, "check all name-resolution scenarios")
	benchFlag := flag.Bool("bench", false, "benchmark name resolution over a generated module")
	flag.Parse()
	all := scenarios(repositoryRoot())
	if *benchFlag {
		benchmark()
		return
	}
	if *checkFlag {
		if !check(all) {
			os.Exit(1)
		}
		return
	}
	interactive(all)
}
