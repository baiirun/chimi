package main

import (
	"bufio"
	"flag"
	"fmt"
	"os"
	"sort"
	"strings"

	"chimi-type-contracts-prototype/contracts"
)

const (
	bold  = "\x1b[1m"
	dim   = "\x1b[2m"
	reset = "\x1b[0m"
)

func result(item contracts.Scenario, catalog contracts.Catalog) (contracts.Inference, []string) {
	inferred := item.Infer()
	problems := []string{}
	actual := contracts.FormatContract(inferred.Contract, catalog)
	if strings.HasPrefix(item.Expected, "fn(") {
		end := strings.Index(item.Expected, ") -> ")
		if end >= 0 {
			inputsText := strings.TrimPrefix(item.Expected[:end+1], "fn(")
			inputsText = strings.TrimSuffix(inputsText, ")")
			inputs := []contracts.Type{}
			if inputsText != "" {
				for _, name := range strings.Split(inputsText, ", ") {
					inputs = append(inputs, contracts.Named(name))
				}
			}
			actual = contracts.FormatFunction(inputs, inferred.Contract, catalog)
		}
	}
	if actual != item.Expected {
		problems = append(problems, fmt.Sprintf("contract actual=%q expected=%q", actual, item.Expected))
	}
	actualIDs := contracts.DiagnosticIDs(inferred)
	expectedIDs := append([]string{}, item.ExpectedIDs...)
	sort.Strings(expectedIDs)
	if strings.Join(actualIDs, ",") != strings.Join(expectedIDs, ",") {
		problems = append(problems, fmt.Sprintf("diagnostics actual=%v expected=%v", actualIDs, expectedIDs))
	}
	return inferred, problems
}

func check(scenarios []contracts.Scenario, catalog contracts.Catalog) bool {
	passed := 0
	for _, item := range scenarios {
		inferred, problems := result(item, catalog)
		status := "PASS"
		if len(problems) == 0 {
			passed++
		} else {
			status = "MISMATCH"
		}
		fmt.Printf("%-8s %-30s contract=%s diagnostics=%v\n", status, item.Name, contracts.FormatContract(inferred.Contract, catalog), contracts.DiagnosticIDs(inferred))
		for _, problem := range problems {
			fmt.Println("         " + problem)
		}
	}
	fmt.Printf("\n%d/%d scenarios satisfy type-contract expectations\n", passed, len(scenarios))
	return passed == len(scenarios)
}

func render(item contracts.Scenario, catalog contracts.Catalog, index, total int) {
	inferred, problems := result(item, catalog)
	fmt.Print("\x1b[2J\x1b[H")
	fmt.Printf("%sChimi type/contract prototype%s  %s%d/%d%s\n\n", bold, reset, dim, index+1, total, reset)
	fmt.Printf("%sscenario%s:    %s\n", bold, reset, item.Name)
	fmt.Printf("%squestion%s:    %s\n", bold, reset, item.Question)
	fmt.Printf("%scontract%s:    %s\n", bold, reset, contracts.FormatContract(inferred.Contract, catalog))
	fmt.Printf("%sdiagnostics%s: %v\n", bold, reset, contracts.DiagnosticIDs(inferred))
	fmt.Printf("%sexpectation%s: %v\n\n", bold, reset, len(problems) == 0)
	fmt.Printf("%sSource scenario%s\n%s\n\n", bold, reset, item.Source)
	fmt.Printf("%sInference decisions%s\n", bold, reset)
	for _, line := range inferred.Trace {
		fmt.Println("- " + line)
	}
	if len(inferred.Diagnostics) > 0 {
		fmt.Printf("\n%sDiagnostics%s\n", bold, reset)
		for _, diagnostic := range inferred.Diagnostics {
			fmt.Printf("- %s: %s\n", diagnostic.Code, diagnostic.Message)
		}
	}
	fmt.Printf("\n%s[n]%s next  %s[p]%s previous  %s[q]%s quit\n", bold, reset, bold, reset, bold, reset)
}

func interactive(scenarios []contracts.Scenario, catalog contracts.Catalog) {
	reader := bufio.NewReader(os.Stdin)
	index := 0
	for {
		render(scenarios[index], catalog, index, len(scenarios))
		line, err := reader.ReadString('\n')
		if err != nil {
			return
		}
		switch strings.TrimSpace(strings.ToLower(line)) {
		case "q":
			return
		case "p":
			index = (index - 1 + len(scenarios)) % len(scenarios)
		default:
			index = (index + 1) % len(scenarios)
		}
	}
}

func main() {
	checkAll := flag.Bool("check", false, "check every scenario and exit")
	flag.Parse()
	scenarios, catalog := contracts.Scenarios()
	if *checkAll {
		if !check(scenarios, catalog) {
			os.Exit(1)
		}
		return
	}
	interactive(scenarios, catalog)
}
