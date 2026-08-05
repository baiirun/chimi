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
	"strconv"
	"strings"
	"time"

	"chimi-lossless-parser-prototype/core"
	"chimi-semantic-lowering-prototype/lower"
)

type fixture struct {
	path   string
	source string
}

const (
	bold  = "\x1b[1m"
	dim   = "\x1b[2m"
	reset = "\x1b[0m"
)

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

func numberedSource(source string) []string {
	parts := strings.Split(source, "\n")
	lines := make([]string, len(parts))
	width := len(strconv.Itoa(len(parts)))
	for index, line := range parts {
		lines[index] = fmt.Sprintf("%*d  %s", width, index+1, line)
	}
	return lines
}

func loadFixtures(root string) []fixture {
	fixtures := []fixture{}
	locations := []struct {
		root   string
		prefix string
	}{
		{filepath.Join(root, "prototypes", "golden"), "golden"},
		{filepath.Join(root, "prototypes", "lossless-parser", "cases"), "recovery"},
	}
	for _, location := range locations {
		err := filepath.Walk(location.root, func(path string, info os.FileInfo, err error) error {
			if err != nil {
				return err
			}
			if info.IsDir() || !strings.HasSuffix(path, ".chimi") {
				return nil
			}
			bytes, err := os.ReadFile(path)
			if err != nil {
				return err
			}
			relative, _ := filepath.Rel(location.root, path)
			fixtures = append(fixtures, fixture{path: location.prefix + "/" + filepath.ToSlash(relative), source: string(bytes)})
			return nil
		})
		if err != nil {
			panic(err)
		}
	}
	sort.Slice(fixtures, func(i, j int) bool { return fixtures[i].path < fixtures[j].path })
	return fixtures
}

func check(root string) bool {
	passed := 0
	fixtures := loadFixtures(root)
	for _, fixture := range fixtures {
		parsed := core.Parse(fixture.source)
		semantic := lower.Lower(parsed.Root)
		repeated := lower.Lower(parsed.Root)
		semanticText := strings.Join(lower.Format(semantic), "\n")
		expectsSyntaxFailure := strings.Contains(fixture.source, "// EXPECT-SYNTAX: failure")
		syntaxMatches := (len(parsed.Diagnostics) > 0) == expectsSyntaxFailure
		loweringMatches := expectsSyntaxFailure || len(semantic.Diagnostics) == 0
		deterministic := semanticText == strings.Join(lower.Format(repeated), "\n")
		nodes, tokens, _ := parsed.Counts()
		compact := lower.SemanticNodeCount(semantic) < nodes+tokens
		unsupported := lower.UnsupportedCSTKinds(parsed.Root)
		invalidSpans := lower.InvalidSpans(semantic, len(fixture.source))
		topLevelComplete := len(semantic.File.Items) == lower.TopLevelSyntaxItemCount(parsed.Root)
		contractMissing := missingContractMarkers(fixture.path, semanticText)
		ok := syntaxMatches && loweringMatches && deterministic && compact && len(unsupported) == 0 && len(invalidSpans) == 0 && topLevelComplete && len(contractMissing) == 0
		status := "PASS"
		if ok {
			passed++
		} else {
			status = "MISMATCH"
		}
		fmt.Printf("%-8s %-48s syntax=%d lowering=%d semantic=%d cst+tokens=%d deterministic=%v\n",
			status, fixture.path, len(parsed.Diagnostics), len(semantic.Diagnostics), lower.SemanticNodeCount(semantic), nodes+tokens, deterministic)
		if !loweringMatches {
			for _, diagnostic := range semantic.Diagnostics {
				fmt.Printf("         lowering: %s @%d..%d\n", diagnostic.Message, diagnostic.Span.Start, diagnostic.Span.End)
			}
		}
		if len(unsupported) > 0 {
			fmt.Printf("         unsupported CST kinds: %s\n", strings.Join(unsupported, ", "))
		}
		if len(invalidSpans) > 0 {
			fmt.Printf("         invalid spans: %s\n", strings.Join(invalidSpans, ", "))
		}
		if !topLevelComplete {
			fmt.Printf("         top-level items: semantic=%d syntax=%d\n", len(semantic.File.Items), lower.TopLevelSyntaxItemCount(parsed.Root))
		}
		if len(contractMissing) > 0 {
			fmt.Printf("         missing semantic contract markers: %s\n", strings.Join(contractMissing, ", "))
		}
	}
	fmt.Printf("\n%d/%d fixtures satisfy lowering-stage expectations\n", passed, len(fixtures))
	return passed == len(fixtures)
}

func missingContractMarkers(path, semantic string) []string {
	required := map[string][]string{
		"golden/invalid/ignored_value.chimi": {
			"step\n", "result\n",
		},
		"golden/valid/composition.chimi": {
			"errors channel: NamedType E", "requirements channel: RequirementRow", "RowVariable R", "omitted/infer",
		},
		"golden/valid/expressions_and_matching.chimi": {
			"Case @", "AppliedPattern", "guard\n",
		},
		"golden/valid/worker_forcing_example.chimi": {
			"Call origin=pipeline", "RowVariable R", "Field clock",
		},
		"golden/valid/dependencies_and_execution.chimi": {
			"Using @", "provider payment",
		},
		"recovery/incomplete_pipeline.chimi": {
			"Invalid origin=pipeline", "preserved input",
		},
	}
	missing := []string{}
	for _, marker := range required[path] {
		if !strings.Contains(semantic, marker) {
			missing = append(missing, strconv.Quote(marker))
		}
	}
	return missing
}

func percentile(durations []time.Duration, percentile float64) time.Duration {
	ordered := append([]time.Duration{}, durations...)
	sort.Slice(ordered, func(i, j int) bool { return ordered[i] < ordered[j] })
	index := int(math.Ceil(float64(len(ordered))*percentile)) - 1
	if index >= len(ordered) {
		index = len(ordered) - 1
	}
	return ordered[index]
}

func benchmark(root string) {
	path := filepath.Join(root, "prototypes", "golden", "valid", "composition.chimi")
	bytes, err := os.ReadFile(path)
	if err != nil {
		panic(err)
	}
	source := strings.Repeat(string(bytes)+"\n", 800)
	parsed := core.Parse(source)
	if len(parsed.Diagnostics) != 0 {
		panic("generated lowering workload has syntax diagnostics")
	}
	for warmup := 0; warmup < 3; warmup++ {
		lower.Lower(parsed.Root)
	}
	runtime.GC()
	var before runtime.MemStats
	var after runtime.MemStats
	runtime.ReadMemStats(&before)
	durations := make([]time.Duration, 0, 50)
	var result lower.Result
	for iteration := 0; iteration < 50; iteration++ {
		started := time.Now()
		result = lower.Lower(parsed.Root)
		durations = append(durations, time.Since(started))
	}
	runtime.ReadMemStats(&after)
	allocated := after.TotalAlloc - before.TotalAlloc

	runtime.GC()
	var retainedBefore runtime.MemStats
	var retainedAfter runtime.MemStats
	runtime.ReadMemStats(&retainedBefore)
	retained := make([]lower.Result, 0, 16)
	for index := 0; index < 16; index++ {
		retained = append(retained, lower.Lower(parsed.Root))
	}
	runtime.GC()
	runtime.ReadMemStats(&retainedAfter)
	runtime.KeepAlive(retained)
	retainedBytes := uint64(0)
	if retainedAfter.HeapAlloc > retainedBefore.HeapAlloc {
		retainedBytes = retainedAfter.HeapAlloc - retainedBefore.HeapAlloc
	}

	fmt.Println("Chimi semantic-lowering prototype benchmark")
	fmt.Printf("host=%s/%s go=%s\n", runtime.GOOS, runtime.GOARCH, runtime.Version())
	fmt.Printf("source=%d bytes CST=%d semantic=%d\n", len(source), func() int { nodes, _, _ := parsed.Counts(); return nodes }(), lower.SemanticNodeCount(result))
	fmt.Printf("lower p50=%s p95=%s alloc/source=%.1fx retained/source=%.1fx\n",
		percentile(durations, 0.50).Round(time.Microsecond),
		percentile(durations, 0.95).Round(time.Microsecond),
		float64(allocated)/float64(50*len(source)),
		float64(retainedBytes)/float64(16*len(source)),
	)
	fmt.Println("The retained ratio excludes the already-retained source and CST.")
}

func render(mode string, scroll int, source string, parsed core.Result, semantic lower.Result) {
	fmt.Print("\x1b[2J\x1b[H")
	nodes, tokens, missing := parsed.Counts()
	fmt.Printf("%sChimi CST → semantic lowering prototype%s\n\n", bold, reset)
	fmt.Printf("%sfixture%s:              golden/valid/composition.chimi\n", bold, reset)
	fmt.Printf("%ssyntax CST%s:           %d nodes, %d tokens, %d missing\n", bold, reset, nodes, tokens, missing)
	fmt.Printf("%ssemantic representation%s: %d nodes\n", bold, reset, lower.SemanticNodeCount(semantic))
	fmt.Printf("%ssyntax diagnostics%s:   %d\n", bold, reset, len(parsed.Diagnostics))
	fmt.Printf("%slowering diagnostics%s: %d\n", bold, reset, len(semantic.Diagnostics))
	fmt.Println()

	var title string
	var lines []string
	switch mode {
	case "source":
		title = "Source"
		lines = numberedSource(source)
	case "cst":
		title = "Lossless CST"
		lines = core.TreeLines(parsed.Root)
	case "decisions":
		title = "What this lowering does"
		lines = strings.Split(lower.Summary(semantic), "\n")
	default:
		title = "Compact semantic representation"
		lines = lower.Format(semantic)
	}
	fmt.Printf("%s%s%s  %sbyte spans point into the source%s\n", bold, title, reset, dim, reset)
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
	fmt.Printf("%s[s]%s source  %s[c]%s CST  %s[m]%s semantic  %s[d]%s decisions  %s[j/k]%s scroll  %s[q]%s quit\n",
		bold, reset, bold, reset, bold, reset, bold, reset, bold, reset, bold, reset)
}

func main() {
	checkFlag := flag.Bool("check", false, "check lowering over the full golden and recovery corpus")
	benchFlag := flag.Bool("bench", false, "benchmark CST-to-semantic lowering")
	flag.Parse()
	root := repositoryRoot()
	if *checkFlag {
		if !check(root) {
			os.Exit(1)
		}
		return
	}
	if *benchFlag {
		benchmark(root)
		return
	}
	path := filepath.Join(root, "prototypes", "golden", "valid", "composition.chimi")
	bytes, err := os.ReadFile(path)
	if err != nil {
		panic(err)
	}
	source := string(bytes)
	parsed := core.Parse(source)
	semantic := lower.Lower(parsed.Root)
	mode := "semantic"
	scroll := 0
	reader := bufio.NewReader(os.Stdin)
	for {
		render(mode, scroll, source, parsed, semantic)
		fmt.Print("> ")
		line, err := reader.ReadString('\n')
		if err != nil {
			return
		}
		switch strings.TrimSpace(line) {
		case "q":
			return
		case "s":
			mode, scroll = "source", 0
		case "c":
			mode, scroll = "cst", 0
		case "m":
			mode, scroll = "semantic", 0
		case "d":
			mode, scroll = "decisions", 0
		case "j":
			scroll += 18
		case "k":
			scroll -= 18
			if scroll < 0 {
				scroll = 0
			}
		}
	}
}
