package main

import (
	"bufio"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"sort"
	"strings"

	chimirt "chimi-runner-prototype/runtime"
)

const (
	bold  = "\x1b[1m"
	dim   = "\x1b[2m"
	reset = "\x1b[0m"
)

func diagnosticCodes(report chimirt.Report) []string {
	result := make([]string, len(report.Diagnostics))
	for index, diagnostic := range report.Diagnostics {
		result[index] = diagnostic.Code
	}
	sort.Strings(result)
	return result
}

func scenarioResult(scenario chimirt.Scenario) (chimirt.Report, []string) {
	report := chimirt.Run(scenario.Program, scenario.Config)
	problems := []string{}
	actualOutcome := ""
	if len(report.Diagnostics) == 0 {
		actualOutcome = chimirt.FormatOutcome(report.Outcome)
	}
	if actualOutcome != scenario.Expected {
		problems = append(problems, fmt.Sprintf("outcome actual=%q expected=%q", actualOutcome, scenario.Expected))
	}
	actualCodes := diagnosticCodes(report)
	expectedCodes := append([]string{}, scenario.ExpectedCodes...)
	sort.Strings(expectedCodes)
	if strings.Join(actualCodes, ",") != strings.Join(expectedCodes, ",") {
		problems = append(problems, fmt.Sprintf("diagnostics actual=%v expected=%v", actualCodes, expectedCodes))
	}
	events := chimirt.EventTypes(report)
	for _, event := range report.Events {
		if event.Computation != scenario.Program.Name {
			problems = append(problems, fmt.Sprintf("event %d computation=%q expected=%q", event.Sequence, event.Computation, scenario.Program.Name))
		}
	}
	position := 0
	for _, required := range scenario.RequiredEvents {
		found := false
		for position < len(events) {
			if events[position] == required {
				position++
				found = true
				break
			}
			position++
		}
		if !found {
			problems = append(problems, "missing ordered event "+required)
			break
		}
	}
	pairs := strings.Join(chimirt.EventNodePairs(report), "\n")
	for _, forbidden := range scenario.ForbiddenPairs {
		if strings.Contains(pairs, forbidden) {
			problems = append(problems, "forbidden event "+forbidden)
		}
	}
	return report, problems
}

func check(scenarios []chimirt.Scenario) bool {
	passed := 0
	for _, scenario := range scenarios {
		report, problems := scenarioResult(scenario)
		status := "PASS"
		if len(problems) == 0 {
			passed++
		} else {
			status = "MISMATCH"
		}
		outcome := "setup rejected"
		if len(report.Diagnostics) == 0 {
			outcome = chimirt.FormatOutcome(report.Outcome)
		}
		fmt.Printf("%-8s %-34s outcome=%s events=%d diagnostics=%v\n", status, scenario.Name, outcome, len(report.Events), diagnosticCodes(report))
		for _, problem := range problems {
			fmt.Println("         " + problem)
		}
	}
	fmt.Printf("\n%d/%d scenarios satisfy executable semantics\n", passed, len(scenarios))
	return passed == len(scenarios)
}

func render(scenario chimirt.Scenario, index, total int) {
	report, problems := scenarioResult(scenario)
	fmt.Print("\x1b[2J\x1b[H")
	fmt.Printf("%sChimi observable runner prototype%s  %s%d/%d%s\n\n", bold, reset, dim, index+1, total, reset)
	fmt.Printf("%sscenario%s:    %s\n", bold, reset, scenario.Name)
	fmt.Printf("%squestion%s:    %s\n", bold, reset, scenario.Question)
	if len(report.Diagnostics) == 0 {
		fmt.Printf("%soutcome%s:     %s\n", bold, reset, chimirt.FormatOutcome(report.Outcome))
	} else {
		fmt.Printf("%soutcome%s:     execution did not start\n", bold, reset)
	}
	fmt.Printf("%scontract%s:    %v\n\n", bold, reset, len(problems) == 0)
	fmt.Printf("%sSource scenario%s\n%s\n\n", bold, reset, scenario.Source)
	if len(report.Diagnostics) > 0 {
		fmt.Printf("%sSetup diagnostics%s\n", bold, reset)
		for _, diagnostic := range report.Diagnostics {
			fmt.Printf("- %s: %s\n", diagnostic.Code, diagnostic.Message)
		}
	} else {
		fmt.Printf("%sStructured execution events%s\n", bold, reset)
		for _, event := range report.Events {
			fmt.Println(chimirt.FormatEvent(event))
		}
	}
	fmt.Printf("\n%s[n]%s next  %s[p]%s previous  %s[q]%s quit\n", bold, reset, bold, reset, bold, reset)
}

func interactive(scenarios []chimirt.Scenario) {
	reader := bufio.NewReader(os.Stdin)
	index := 0
	for {
		render(scenarios[index], index, len(scenarios))
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

func exportJSON(scenarios []chimirt.Scenario, name string) bool {
	for _, scenario := range scenarios {
		if scenario.Name != name {
			continue
		}
		report := chimirt.Run(scenario.Program, scenario.Config)
		encoder := json.NewEncoder(os.Stdout)
		for _, event := range report.Events {
			if err := encoder.Encode(event); err != nil {
				panic(err)
			}
		}
		return true
	}
	return false
}

func main() {
	checkAll := flag.Bool("check", false, "check every runner scenario and exit")
	jsonScenario := flag.String("json", "", "export one scenario's events as JSONL")
	flag.Parse()
	scenarios := chimirt.Scenarios()
	if *checkAll {
		if !check(scenarios) {
			os.Exit(1)
		}
		return
	}
	if *jsonScenario != "" {
		if !exportJSON(scenarios, *jsonScenario) {
			fmt.Fprintln(os.Stderr, "unknown scenario: "+*jsonScenario)
			os.Exit(2)
		}
		return
	}
	interactive(scenarios)
}
