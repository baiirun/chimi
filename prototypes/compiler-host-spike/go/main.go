// PROTOTYPE: disposable compiler-host workload, not Chimi compiler code.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"runtime"
	"sort"
	"strings"
	"time"
)

type token struct {
	kind   uint8
	start  uint32
	length uint32
}

type node struct {
	kind       uint8
	firstToken uint32
	tokenCount uint32
	textLength uint32
}

type syntax struct {
	tokens []token
	nodes  []node
}

type result struct {
	SchemaVersion           int     `json:"schema_version"`
	Implementation          string  `json:"implementation"`
	Toolchain               string  `json:"toolchain"`
	Mode                    string  `json:"mode"`
	Declarations            int     `json:"declarations"`
	Iterations              int     `json:"iterations"`
	InputBytes              int     `json:"input_bytes"`
	TokenCount              int     `json:"token_count"`
	NodeCount               int     `json:"node_count"`
	RoundTrip               bool    `json:"round_trip"`
	Checksum                uint64  `json:"checksum"`
	P50Microseconds         float64 `json:"p50_microseconds"`
	P95Microseconds         float64 `json:"p95_microseconds"`
	ThroughputMiBPerSecond  float64 `json:"throughput_mib_per_second"`
	AllocatedBytesPerRun    uint64  `json:"allocated_bytes_per_run"`
	RetainedHeapBytes       int64   `json:"retained_heap_bytes"`
	RepresentedPayloadBytes uint64  `json:"represented_payload_bytes"`
}

const (
	kindWhitespace uint8 = iota
	kindNewline
	kindComment
	kindIdentifier
	kindInteger
	kindSymbol
	kindMulti
)

func generateSource(declarations int) string {
	var source strings.Builder
	source.Grow(declarations * 240)
	source.WriteString("// PROTOTYPE GENERATED WORKLOAD — DO NOT EDIT\n")
	for index := 0; index < declarations; index++ {
		suffix := fmt.Sprintf("%06d", index)
		fmt.Fprintf(&source, "struct Value_%s {\n  amount: Int\n}\n\n", suffix)
		fmt.Fprintf(&source, "pub fn increment_%s(value: Int) -> Int {\n  value + 1\n}\n\n", suffix)
		fmt.Fprintf(&source, "let configuration_%s = {\n  retry_limit: 3,\n  tracing: True,\n}\n\n", suffix)
		fmt.Fprintf(&source, "let transformed_%s = 1\n|> increment_%s\n\n", suffix, suffix)
	}
	return source.String()
}

func lex(source string) []token {
	tokens := make([]token, 0, len(source)/2)
	emit := func(kind uint8, start, end int) {
		tokens = append(tokens, token{kind: kind, start: uint32(start), length: uint32(end - start)})
	}
	for index := 0; index < len(source); {
		start := index
		character := source[index]
		switch {
		case character == ' ' || character == '\t':
			for index < len(source) && (source[index] == ' ' || source[index] == '\t') {
				index++
			}
			emit(kindWhitespace, start, index)
		case character == '\n':
			index++
			emit(kindNewline, start, index)
		case character == '/' && index+1 < len(source) && source[index+1] == '/':
			index += 2
			for index < len(source) && source[index] != '\n' {
				index++
			}
			emit(kindComment, start, index)
		case index+1 < len(source) && ((character == '-' && source[index+1] == '>') || (character == '|' && source[index+1] == '>')):
			index += 2
			emit(kindMulti, start, index)
		case isIdentifierStart(character):
			index++
			for index < len(source) && isIdentifierContinue(source[index]) {
				index++
			}
			emit(kindIdentifier, start, index)
		case character >= '0' && character <= '9':
			index++
			for index < len(source) && source[index] >= '0' && source[index] <= '9' {
				index++
			}
			emit(kindInteger, start, index)
		default:
			index++
			emit(kindSymbol, start, index)
		}
	}
	return tokens
}

func isIdentifierStart(character byte) bool {
	return character == '_' || character >= 'a' && character <= 'z' || character >= 'A' && character <= 'Z'
}

func isIdentifierContinue(character byte) bool {
	return isIdentifierStart(character) || character >= '0' && character <= '9'
}

func build(source string) syntax {
	tokens := lex(source)
	nodes := make([]node, 1, len(source)/10)
	nodes[0] = node{kind: 0, tokenCount: uint32(len(tokens)), textLength: uint32(len(source))}
	stack := make([]int, 0, 32)
	lineFirstToken := 0
	lineStartByte := 0
	for index, item := range tokens {
		text := source[int(item.start):int(item.start+item.length)]
		if item.kind == kindSymbol && text == "{" {
			nodes = append(nodes, node{kind: 2, firstToken: uint32(index), textLength: item.length})
			stack = append(stack, len(nodes)-1)
		} else if item.kind == kindSymbol && text == "}" && len(stack) > 0 {
			nodeIndex := stack[len(stack)-1]
			stack = stack[:len(stack)-1]
			nodes[nodeIndex].tokenCount = uint32(index + 1 - int(nodes[nodeIndex].firstToken))
			nodes[nodeIndex].textLength = item.start + item.length - tokens[nodes[nodeIndex].firstToken].start
		}
		if item.kind == kindNewline {
			nodes = append(nodes, node{
				kind:       1,
				firstToken: uint32(lineFirstToken),
				tokenCount: uint32(index + 1 - lineFirstToken),
				textLength: item.start + item.length - uint32(lineStartByte),
			})
			lineFirstToken = index + 1
			lineStartByte = int(item.start + item.length)
		}
	}
	if lineFirstToken < len(tokens) {
		nodes = append(nodes, node{
			kind:       1,
			firstToken: uint32(lineFirstToken),
			tokenCount: uint32(len(tokens) - lineFirstToken),
			textLength: uint32(len(source) - lineStartByte),
		})
	}
	return syntax{tokens: tokens, nodes: nodes}
}

func inspect(source string, tree syntax) (bool, uint64) {
	expectedStart := uint32(0)
	checksum := uint64(1469598103934665603)
	for _, item := range tree.tokens {
		if item.start != expectedStart {
			return false, 0
		}
		expectedStart += item.length
		checksum = mix(checksum, uint64(item.kind))
		checksum = mix(checksum, uint64(item.start))
		checksum = mix(checksum, uint64(item.length))
	}
	for _, item := range tree.nodes {
		checksum = mix(checksum, uint64(item.kind))
		checksum = mix(checksum, uint64(item.firstToken))
		checksum = mix(checksum, uint64(item.tokenCount))
		checksum = mix(checksum, uint64(item.textLength))
	}
	return int(expectedStart) == len(source), checksum
}

func mix(checksum, value uint64) uint64 {
	checksum ^= value
	return checksum * 1099511628211
}

func percentile(samples []time.Duration, fraction float64) time.Duration {
	sorted := append([]time.Duration(nil), samples...)
	sort.Slice(sorted, func(i, j int) bool { return sorted[i] < sorted[j] })
	index := int(float64(len(sorted)-1)*fraction + 0.5)
	return sorted[index]
}

func main() {
	declarations := flag.Int("declarations", 10000, "generated declarations")
	iterations := flag.Int("iterations", 30, "measured iterations")
	flag.Parse()
	if *declarations < 1 || *iterations < 1 {
		fmt.Fprintln(os.Stderr, "declarations and iterations must be positive")
		os.Exit(2)
	}

	source := generateSource(*declarations)
	for index := 0; index < 3; index++ {
		tree := build(source)
		inspect(source, tree)
		runtime.KeepAlive(tree)
	}

	var benchmarkBefore, benchmarkAfter runtime.MemStats
	runtime.ReadMemStats(&benchmarkBefore)
	samples := make([]time.Duration, 0, *iterations)
	var finalChecksum uint64
	for index := 0; index < *iterations; index++ {
		started := time.Now()
		tree := build(source)
		_, finalChecksum = inspect(source, tree)
		samples = append(samples, time.Since(started))
		runtime.KeepAlive(tree)
	}
	runtime.ReadMemStats(&benchmarkAfter)
	allocatedBytesPerRun := (benchmarkAfter.TotalAlloc - benchmarkBefore.TotalAlloc) / uint64(*iterations)

	var retainedBefore, retainedAfter runtime.MemStats
	runtime.GC()
	runtime.ReadMemStats(&retainedBefore)
	retained := build(source)
	roundTrip, finalChecksum := inspect(source, retained)
	runtime.GC()
	runtime.KeepAlive(source)
	runtime.KeepAlive(retained)
	runtime.ReadMemStats(&retainedAfter)
	retainedBytes := int64(retainedAfter.HeapAlloc) - int64(retainedBefore.HeapAlloc)

	p50 := percentile(samples, 0.50)
	p95 := percentile(samples, 0.95)
	throughput := float64(len(source)) / (1024 * 1024) / p50.Seconds()
	payload := uint64(len(source) + cap(retained.tokens)*12 + cap(retained.nodes)*16)
	output := result{
		SchemaVersion:           1,
		Implementation:          "go",
		Toolchain:               runtime.Version(),
		Mode:                    "release",
		Declarations:            *declarations,
		Iterations:              *iterations,
		InputBytes:              len(source),
		TokenCount:              len(retained.tokens),
		NodeCount:               len(retained.nodes),
		RoundTrip:               roundTrip,
		Checksum:                finalChecksum,
		P50Microseconds:         float64(p50.Nanoseconds()) / 1000,
		P95Microseconds:         float64(p95.Nanoseconds()) / 1000,
		ThroughputMiBPerSecond:  throughput,
		AllocatedBytesPerRun:    allocatedBytesPerRun,
		RetainedHeapBytes:       retainedBytes,
		RepresentedPayloadBytes: payload,
	}
	if err := json.NewEncoder(os.Stdout).Encode(output); err != nil {
		panic(err)
	}
}
