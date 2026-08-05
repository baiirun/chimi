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
)

const (
	bold  = "\x1b[1m"
	dim   = "\x1b[2m"
	reset = "\x1b[0m"
)

type analysis struct {
	Path     string
	Source   string
	Result   core.Result
	Parse    time.Duration
	Tree     []string
	SourceUI []string
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

func loadAnalyses(root string) []analysis {
	type sourcePath struct {
		absolute string
		display  string
	}
	sources := []sourcePath{}
	directories := []struct {
		path   string
		prefix string
	}{
		{filepath.Join(root, "prototypes", "golden"), "golden"},
		{filepath.Join(root, "prototypes", "lossless-parser", "cases"), "recovery"},
	}
	for _, directory := range directories {
		err := filepath.Walk(directory.path, func(path string, info os.FileInfo, err error) error {
			if err != nil {
				return err
			}
			if !info.IsDir() && strings.HasSuffix(path, ".chimi") {
				relative, _ := filepath.Rel(directory.path, path)
				sources = append(sources, sourcePath{
					absolute: path,
					display:  directory.prefix + "/" + filepath.ToSlash(relative),
				})
			}
			return nil
		})
		if err != nil {
			panic(err)
		}
	}
	sort.Slice(sources, func(i, j int) bool { return sources[i].display < sources[j].display })
	items := make([]analysis, 0, len(sources))
	for _, sourcePath := range sources {
		path := sourcePath.absolute
		bytes, err := os.ReadFile(path)
		if err != nil {
			panic(err)
		}
		source := string(bytes)
		started := time.Now()
		result := core.Parse(source)
		elapsed := time.Since(started)
		items = append(items, analysis{
			Path:     sourcePath.display,
			Source:   source,
			Result:   result,
			Parse:    elapsed,
			Tree:     core.TreeLines(result.Root),
			SourceUI: numberedSource(source),
		})
	}
	return items
}

func numberedSource(source string) []string {
	lines := strings.Split(source, "\n")
	result := make([]string, len(lines))
	width := len(strconv.Itoa(len(lines)))
	for index, line := range lines {
		result[index] = fmt.Sprintf("%*d  %s", width, index+1, line)
	}
	return result
}

func expectedSyntaxFailure(source string) bool {
	return strings.Contains(source, "// EXPECT-SYNTAX: failure")
}

func fixtureExpectation(path, source string) string {
	if expectedSyntaxFailure(source) {
		return "rejected by parser (invalid syntax)"
	}
	if strings.HasPrefix(path, "golden/invalid/") {
		return "accepted by parser; rejected by semantic/type checking"
	}
	return "accepted by parser and later compilation"
}

func cursorOffsetsValid(result core.Result) bool {
	var walk func(core.Cursor, int) bool
	walk = func(cursor core.Cursor, expected int) bool {
		if cursor.Offset() != expected {
			return false
		}
		node := cursor.Node()
		current := expected
		for index, child := range node.Children {
			if child.Node != nil {
				path := append(append([]int{}, cursor.Path...), index)
				childCursor := core.Cursor{Root: cursor.Root, Path: path}
				parent, ok := childCursor.Parent()
				if !ok || fmt.Sprint(parent.Path) != fmt.Sprint(cursor.Path) || !walk(childCursor, current) {
					return false
				}
				current += child.Node.TextLen
			} else {
				current += len(child.Token.Text)
			}
		}
		return current == expected+node.TextLen
	}
	return walk(core.RootCursor(result.Root), 0)
}

func check(all []analysis) bool {
	matched := 0
	for _, item := range all {
		roundTrip := item.Result.RoundTrip() == item.Source
		textLength := item.Result.Root.TextLen == len(item.Source)
		cursorOffsets := cursorOffsetsValid(item.Result)
		actualFailure := len(item.Result.Diagnostics) > 0
		expectedFailure := expectedSyntaxFailure(item.Source)
		ok := roundTrip && textLength && cursorOffsets && actualFailure == expectedFailure
		status := "PASS"
		if !ok {
			status = "MISMATCH"
		} else {
			matched++
		}
		fmt.Printf(
			"%-8s %-48s roundtrip=%-3v cursor=%-3v syntax-diagnostics=%-3d expected-syntax-error=%v\n",
			status,
			item.Path,
			roundTrip,
			cursorOffsets,
			len(item.Result.Diagnostics),
			expectedFailure,
		)
	}
	fmt.Printf("\n%d/%d fixtures satisfy lossless and syntax-diagnostic expectations\n", matched, len(all))
	return matched == len(all)
}

func renderIntro(total int) {
	fmt.Print("\x1b[2J\x1b[H")
	fmt.Printf("%sChimi lossless parser/CST prototype%s\n\n", bold, reset)
	fmt.Println("This is actual parser output over the golden Chimi corpus.")
	fmt.Println("Every file produces a CST and syntax diagnostics, including malformed source.")
	fmt.Println("Semantic and type-checking diagnostics belong to later compiler stages.")
	fmt.Println()
	fmt.Printf("The browser contains %d fixtures. For each one, inspect:\n", total)
	fmt.Println("  - whether CST leaves reproduce the source exactly;")
	fmt.Println("  - whether errors remain local and later structure survives;")
	fmt.Println("  - whether MISSING tokens describe absent syntax without inventing text;")
	fmt.Println("  - whether node ranges are useful for future tools.")
	fmt.Println()
	fmt.Printf("%s[n]%s first fixture  %s[c]%s check all  %s[b]%s benchmarks  %s[q]%s quit\n", bold, reset, bold, reset, bold, reset, bold, reset)
}

func render(item analysis, index, total int, mode string, scroll int) {
	fmt.Print("\x1b[2J\x1b[H")
	nodes, tokens, missing := item.Result.Counts()
	fmt.Printf("%sChimi lossless parser/CST%s  %s%d/%d%s\n\n", bold, reset, dim, index+1, total, reset)
	fmt.Printf("%sfixture%s:             %s\n", bold, reset, item.Path)
	fmt.Printf("%sexpectation%s:         %s\n", bold, reset, fixtureExpectation(item.Path, item.Source))
	fmt.Printf("%ssource%s:              %d bytes\n", bold, reset, len(item.Source))
	fmt.Printf("%sparse%s:               %s\n", bold, reset, item.Parse.Round(time.Microsecond))
	fmt.Printf("%sCST%s:                 %d nodes, %d tokens, %d missing\n", bold, reset, nodes, tokens, missing)
	fmt.Printf("%sround-trip%s:          %v\n", bold, reset, item.Result.RoundTrip() == item.Source)
	fmt.Printf("%ssyntax diagnostics%s:  %d\n", bold, reset, len(item.Result.Diagnostics))
	fmt.Println()

	var lines []string
	switch mode {
	case "tree":
		fmt.Printf("%sRecovered CST%s  %snode/token ranges are byte offsets%s\n", bold, reset, dim, reset)
		lines = item.Tree
	case "diagnostics":
		fmt.Printf("%sSyntax diagnostics%s\n", bold, reset)
		if len(item.Result.Diagnostics) == 0 {
			if strings.HasPrefix(item.Path, "golden/invalid/") {
				lines = []string{
					"No syntax diagnostics.",
					"This source is grammatically valid and is expected to be rejected",
					"later by semantic analysis or type checking.",
				}
			} else {
				lines = []string{"No syntax diagnostics."}
			}
		} else {
			for _, diagnostic := range item.Result.Diagnostics {
				line, column := core.LineColumn(item.Source, diagnostic.Offset)
				lines = append(lines, fmt.Sprintf("%d:%d  %s", line, column, diagnostic.Message))
			}
		}
	case "source":
		fmt.Printf("%sSource%s\n", bold, reset)
		lines = item.SourceUI
	default:
		fmt.Printf("%sWhat to review%s\n", bold, reset)
		if expectedSyntaxFailure(item.Source) {
			lines = []string{
				"This fixture is syntactically invalid by design.",
				"Open diagnostics and the recovered tree.",
				"Check that the damaged region is local and later source remains represented.",
			}
		} else {
			lines = []string{
				"This fixture should have no syntax diagnostics.",
				"Open the tree to inspect explicit trivia and source ranges.",
				"Round-trip must remain true even though semantic-invalid fixtures do not compile.",
			}
		}
	}

	if scroll < 0 {
		scroll = 0
	}
	if scroll >= len(lines) && len(lines) > 0 {
		scroll = len(lines) - 1
	}
	end := scroll + 20
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
	fmt.Printf("%s[n]%s next  %s[p]%s previous  %s[s]%s source  %s[t]%s tree  %s[d]%s syntax diagnostics  %s[j/k]%s scroll  %s[h]%s help  %s[q]%s quit\n",
		bold, reset, bold, reset, bold, reset, bold, reset, bold, reset, bold, reset, bold, reset, bold, reset)
}

func interactive(all []analysis) {
	reader := bufio.NewReader(os.Stdin)
	index := -1
	mode := "summary"
	scroll := 0
	for {
		if index < 0 {
			renderIntro(len(all))
		} else {
			render(all[index], index, len(all), mode, scroll)
		}
		fmt.Print("> ")
		line, err := reader.ReadString('\n')
		if err != nil {
			fmt.Println()
			return
		}
		command := strings.TrimSpace(strings.ToLower(line))
		switch command {
		case "q":
			return
		case "", "n":
			if index < 0 {
				index = 0
			} else {
				index = (index + 1) % len(all)
			}
			mode, scroll = "summary", 0
		case "p":
			if index < 0 {
				index = len(all) - 1
			} else {
				index = (index - 1 + len(all)) % len(all)
			}
			mode, scroll = "summary", 0
		case "s", "t", "d":
			if index >= 0 {
				modes := map[string]string{"s": "source", "t": "tree", "d": "diagnostics"}
				mode, scroll = modes[command], 0
			}
		case "j":
			scroll += 15
		case "k":
			scroll -= 15
			if scroll < 0 {
				scroll = 0
			}
		case "h":
			index, mode, scroll = -1, "summary", 0
		case "c":
			fmt.Print("\x1b[2J\x1b[H")
			check(all)
			fmt.Print("\npress enter to return ")
			reader.ReadString('\n')
		case "b":
			fmt.Print("\x1b[2J\x1b[H")
			runBenchmarks()
			fmt.Print("\npress enter to return ")
			reader.ReadString('\n')
		}
	}
}

func generatedModule(declarations int) string {
	var output strings.Builder
	output.WriteString("// PROTOTYPE GENERATED WORKLOAD — DO NOT EDIT\n")
	for item := 0; item < declarations; item++ {
		suffix := fmt.Sprintf("0000_%04d", item)
		fmt.Fprintf(&output, "struct Value_%s {\n  amount: Int\n}\n\n", suffix)
		fmt.Fprintf(&output, "pub fn increment_%s(value: Int) -> Int {\n  value + 1\n}\n\n", suffix)
		fmt.Fprintf(&output, "let configuration_%s = {\n  retry_limit: 3,\n  tracing: True,\n}\n\n", suffix)
		fmt.Fprintf(&output, "let transformed_%s = 1\n|> increment_%s\n\n", suffix, suffix)
	}
	return output.String()
}

type benchmark struct {
	name       string
	source     string
	iterations int
	target     time.Duration
}

func removeFirst(source, needle string, relative int, count int) string {
	index := strings.Index(source, needle)
	if index < 0 {
		panic("benchmark mutation marker not found")
	}
	offset := index + relative
	return source[:offset] + source[offset+count:]
}

func benchmarkCases() []benchmark {
	typical := generatedModule(250)
	large := generatedModule(5_000)
	editor := generatedModule(500)
	brace := removeFirst(editor, "retry_limit:", len("retry_limit"), 1)
	pipelineMarker := "|> increment_"
	pipelineStart := strings.Index(editor, pipelineMarker) + len("|> ")
	pipelineEnd := strings.Index(editor[pipelineStart:], "\n") + pipelineStart
	pipeline := editor[:pipelineStart] + editor[pipelineEnd:]
	middleNeedle := "retry_limit:"
	positions := []int{}
	for start := 0; ; {
		found := strings.Index(editor[start:], middleNeedle)
		if found < 0 {
			break
		}
		positions = append(positions, start+found)
		start += found + len(middleNeedle)
	}
	middleOffset := positions[len(positions)/2] + len("retry_limit")
	middle := editor[:middleOffset] + editor[middleOffset+1:]
	return []benchmark{
		{"typical valid full parse", typical, 60, 16 * time.Millisecond},
		{"large valid full parse", large, 12, 0},
		{"malformed brace full parse", brace, 50, 0},
		{"incomplete pipeline full parse", pipeline, 50, 0},
		{"prefix edit full reparse", " " + editor, 50, 16 * time.Millisecond},
		{"middle brace edit full reparse", middle, 50, 16 * time.Millisecond},
	}
}

func percentile(durations []time.Duration, percentile float64) time.Duration {
	ordered := append([]time.Duration{}, durations...)
	sort.Slice(ordered, func(i, j int) bool { return ordered[i] < ordered[j] })
	index := int(math.Ceil(float64(len(ordered))*percentile)) - 1
	return ordered[index]
}

func runBenchmarks() {
	fmt.Println("Chimi lossless parser prototype benchmarks")
	fmt.Printf("host=%s/%s go=%s\n\n", runtime.GOOS, runtime.GOARCH, runtime.Version())
	for _, item := range benchmarkCases() {
		for warmup := 0; warmup < 3; warmup++ {
			core.Parse(item.source)
		}
		runtime.GC()
		var memoryBefore runtime.MemStats
		var memoryAfter runtime.MemStats
		runtime.ReadMemStats(&memoryBefore)
		durations := make([]time.Duration, 0, item.iterations)
		var result core.Result
		for iteration := 0; iteration < item.iterations; iteration++ {
			started := time.Now()
			result = core.Parse(item.source)
			durations = append(durations, time.Since(started))
		}
		runtime.ReadMemStats(&memoryAfter)
		p50 := percentile(durations, 0.50)
		p95 := percentile(durations, 0.95)
		greenRatio := float64(result.EstimatedBytes()) / float64(len(item.source))
		allocated := memoryAfter.TotalAlloc - memoryBefore.TotalAlloc
		allocationRatio := float64(allocated) / float64(item.iterations*len(item.source))
		status := "record"
		if item.target > 0 {
			if p95 <= item.target {
				status = "within provisional target"
			} else {
				status = "OVER provisional target"
			}
		}
		fmt.Printf("%-34s bytes=%8d p50=%9s p95=%9s alloc/source=%5.1fx green-est/source=%5.1fx diagnostics=%d %s\n",
			item.name, len(item.source), p50.Round(time.Microsecond), p95.Round(time.Microsecond), allocationRatio, greenRatio, len(result.Diagnostics), status)
		if result.RoundTrip() != item.source {
			fmt.Println("  INVARIANT FAILURE: CST did not round-trip source")
		}
	}

	projectSource := generatedModule(150)
	runtime.GC()
	var retainedBefore runtime.MemStats
	var retainedAfter runtime.MemStats
	runtime.ReadMemStats(&retainedBefore)
	started := time.Now()
	totalEstimated := 0
	retained := make([]core.Result, 0, 64)
	for file := 0; file < 64; file++ {
		result := core.Parse(projectSource)
		totalEstimated += result.EstimatedBytes()
		retained = append(retained, result)
	}
	elapsed := time.Since(started)
	runtime.GC()
	runtime.ReadMemStats(&retainedAfter)
	runtime.KeepAlive(retained)
	totalSource := len(projectSource) * 64
	retainedBytes := uint64(0)
	if retainedAfter.HeapAlloc > retainedBefore.HeapAlloc {
		retainedBytes = retainedAfter.HeapAlloc - retainedBefore.HeapAlloc
	}
	fmt.Printf("%-34s bytes=%8d total=%9s retained/source=%5.1fx green-est/source=%5.1fx\n",
		"64-file retained parse results", totalSource, elapsed.Round(time.Microsecond), float64(retainedBytes)/float64(totalSource), float64(totalEstimated)/float64(totalSource))
	fmt.Println()
	fmt.Println("alloc/source is total parser allocation; retained/source is live heap after GC.")
	fmt.Println("green-est/source models green structs and child storage without allocator overhead.")
	fmt.Println("These are prototype-host measurements, not permanent Chimi compiler budgets.")
}

func main() {
	checkFlag := flag.Bool("check", false, "check the complete golden corpus")
	benchFlag := flag.Bool("bench", false, "run syntax performance workloads")
	flag.Parse()
	root := repositoryRoot()
	all := loadAnalyses(root)
	if *checkFlag {
		if !check(all) {
			os.Exit(1)
		}
		return
	}
	if *benchFlag {
		runBenchmarks()
		return
	}
	interactive(all)
}
