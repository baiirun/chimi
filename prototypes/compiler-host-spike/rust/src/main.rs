// PROTOTYPE: disposable compiler-host workload, not Chimi compiler code.

use std::alloc::{GlobalAlloc, Layout, System};
use std::env;
use std::process;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

struct CountingAllocator;

static CURRENT_ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static TOTAL_ALLOCATED: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            CURRENT_ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
            TOTAL_ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        CURRENT_ALLOCATED.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, old: Layout, new_size: usize) -> *mut u8 {
        let replacement = unsafe { System.realloc(pointer, old, new_size) };
        if !replacement.is_null() {
            if new_size >= old.size() {
                CURRENT_ALLOCATED.fetch_add(new_size - old.size(), Ordering::Relaxed);
            } else {
                CURRENT_ALLOCATED.fetch_sub(old.size() - new_size, Ordering::Relaxed);
            }
            TOTAL_ALLOCATED.fetch_add(new_size, Ordering::Relaxed);
        }
        replacement
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[repr(C)]
#[derive(Clone, Copy)]
struct Token {
    kind: u8,
    start: u32,
    length: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Node {
    kind: u8,
    first_token: u32,
    token_count: u32,
    text_length: u32,
}

struct Syntax {
    tokens: Vec<Token>,
    nodes: Vec<Node>,
}

const KIND_WHITESPACE: u8 = 0;
const KIND_NEWLINE: u8 = 1;
const KIND_COMMENT: u8 = 2;
const KIND_IDENTIFIER: u8 = 3;
const KIND_INTEGER: u8 = 4;
const KIND_SYMBOL: u8 = 5;
const KIND_MULTI: u8 = 6;

fn generate_source(declarations: usize) -> String {
    use std::fmt::Write;

    let mut source = String::with_capacity(declarations * 240);
    source.push_str("// PROTOTYPE GENERATED WORKLOAD — DO NOT EDIT\n");
    for index in 0..declarations {
        let suffix = format!("{index:06}");
        writeln!(source, "struct Value_{suffix} {{\n  amount: Int\n}}\n").unwrap();
        writeln!(
            source,
            "pub fn increment_{suffix}(value: Int) -> Int {{\n  value + 1\n}}\n"
        )
        .unwrap();
        writeln!(
            source,
            "let configuration_{suffix} = {{\n  retry_limit: 3,\n  tracing: True,\n}}\n"
        )
        .unwrap();
        writeln!(
            source,
            "let transformed_{suffix} = 1\n|> increment_{suffix}\n"
        )
        .unwrap();
    }
    source
}

fn lex(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::with_capacity(bytes.len() / 2);
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        let character = bytes[index];
        let kind;
        if character == b' ' || character == b'\t' {
            index += 1;
            while index < bytes.len() && (bytes[index] == b' ' || bytes[index] == b'\t') {
                index += 1;
            }
            kind = KIND_WHITESPACE;
        } else if character == b'\n' {
            index += 1;
            kind = KIND_NEWLINE;
        } else if character == b'/' && index + 1 < bytes.len() && bytes[index + 1] == b'/' {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            kind = KIND_COMMENT;
        } else if index + 1 < bytes.len()
            && matches!(character, b'-' | b'|')
            && bytes[index + 1] == b'>'
        {
            index += 2;
            kind = KIND_MULTI;
        } else if is_identifier_start(character) {
            index += 1;
            while index < bytes.len() && is_identifier_continue(bytes[index]) {
                index += 1;
            }
            kind = KIND_IDENTIFIER;
        } else if character.is_ascii_digit() {
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
            kind = KIND_INTEGER;
        } else {
            index += 1;
            kind = KIND_SYMBOL;
        }
        tokens.push(Token {
            kind,
            start: start as u32,
            length: (index - start) as u32,
        });
    }
    tokens
}

fn is_identifier_start(character: u8) -> bool {
    character == b'_' || character.is_ascii_lowercase() || character.is_ascii_uppercase()
}

fn is_identifier_continue(character: u8) -> bool {
    is_identifier_start(character) || character.is_ascii_digit()
}

fn build(source: &str) -> Syntax {
    let tokens = lex(source);
    let mut nodes = Vec::with_capacity(source.len() / 10);
    nodes.push(Node {
        kind: 0,
        first_token: 0,
        token_count: tokens.len() as u32,
        text_length: source.len() as u32,
    });
    let mut stack = Vec::with_capacity(32);
    let mut line_first_token = 0;
    let mut line_start_byte = 0;
    for (index, token) in tokens.iter().enumerate() {
        let text = &source[token.start as usize..(token.start + token.length) as usize];
        if token.kind == KIND_SYMBOL && text == "{" {
            nodes.push(Node {
                kind: 2,
                first_token: index as u32,
                token_count: 0,
                text_length: token.length,
            });
            stack.push(nodes.len() - 1);
        } else if token.kind == KIND_SYMBOL
            && text == "}"
            && let Some(node_index) = stack.pop()
        {
            let first_token = nodes[node_index].first_token;
            nodes[node_index].token_count = index as u32 + 1 - first_token;
            nodes[node_index].text_length =
                token.start + token.length - tokens[first_token as usize].start;
        }
        if token.kind == KIND_NEWLINE {
            nodes.push(Node {
                kind: 1,
                first_token: line_first_token as u32,
                token_count: (index + 1 - line_first_token) as u32,
                text_length: token.start + token.length - line_start_byte as u32,
            });
            line_first_token = index + 1;
            line_start_byte = (token.start + token.length) as usize;
        }
    }
    if line_first_token < tokens.len() {
        nodes.push(Node {
            kind: 1,
            first_token: line_first_token as u32,
            token_count: (tokens.len() - line_first_token) as u32,
            text_length: (source.len() - line_start_byte) as u32,
        });
    }
    Syntax { tokens, nodes }
}

fn inspect(source: &str, tree: &Syntax) -> (bool, u64) {
    let mut expected_start = 0_u32;
    let mut checksum = 1_469_598_103_934_665_603_u64;
    for token in &tree.tokens {
        if token.start != expected_start {
            return (false, 0);
        }
        expected_start += token.length;
        checksum = mix(checksum, token.kind as u64);
        checksum = mix(checksum, token.start as u64);
        checksum = mix(checksum, token.length as u64);
    }
    for node in &tree.nodes {
        checksum = mix(checksum, node.kind as u64);
        checksum = mix(checksum, node.first_token as u64);
        checksum = mix(checksum, node.token_count as u64);
        checksum = mix(checksum, node.text_length as u64);
    }
    (expected_start as usize == source.len(), checksum)
}

fn mix(checksum: u64, value: u64) -> u64 {
    (checksum ^ value).wrapping_mul(1_099_511_628_211)
}

fn percentile(samples: &mut [Duration], fraction: f64) -> Duration {
    samples.sort_unstable();
    let index = (((samples.len() - 1) as f64 * fraction) + 0.5) as usize;
    samples[index]
}

fn argument(name: &str, default: usize) -> usize {
    let arguments: Vec<String> = env::args().collect();
    arguments
        .windows(2)
        .find(|pair| pair[0] == name)
        .and_then(|pair| pair[1].parse().ok())
        .unwrap_or(default)
}

fn main() {
    let declarations = argument("--declarations", 10_000);
    let iterations = argument("--iterations", 30);
    if declarations == 0 || iterations == 0 {
        eprintln!("declarations and iterations must be positive");
        process::exit(2);
    }

    let source = generate_source(declarations);
    for _ in 0..3 {
        let tree = build(&source);
        std::hint::black_box(inspect(&source, &tree));
    }

    let allocated_before = TOTAL_ALLOCATED.load(Ordering::Relaxed);
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        let tree = build(&source);
        std::hint::black_box(inspect(&source, &tree));
        samples.push(started.elapsed());
    }
    let allocated_after = TOTAL_ALLOCATED.load(Ordering::Relaxed);

    let retained_before = CURRENT_ALLOCATED.load(Ordering::Relaxed);
    let retained = build(&source);
    let retained_after = CURRENT_ALLOCATED.load(Ordering::Relaxed);
    let (round_trip, checksum) = inspect(&source, &retained);
    let p50 = percentile(&mut samples.clone(), 0.50);
    let p95 = percentile(&mut samples, 0.95);
    let throughput = source.len() as f64 / (1024.0 * 1024.0) / p50.as_secs_f64();
    let payload = source.len()
        + retained.tokens.capacity() * std::mem::size_of::<Token>()
        + retained.nodes.capacity() * std::mem::size_of::<Node>();

    println!(
        concat!(
            "{{\"schema_version\":1,",
            "\"implementation\":\"rust\",",
            "\"toolchain\":\"rustc\",",
            "\"mode\":\"release\",",
            "\"declarations\":{},",
            "\"iterations\":{},",
            "\"input_bytes\":{},",
            "\"token_count\":{},",
            "\"node_count\":{},",
            "\"round_trip\":{},",
            "\"checksum\":{},",
            "\"p50_microseconds\":{},",
            "\"p95_microseconds\":{},",
            "\"throughput_mib_per_second\":{},",
            "\"allocated_bytes_per_run\":{},",
            "\"retained_heap_bytes\":{},",
            "\"represented_payload_bytes\":{} }}"
        ),
        declarations,
        iterations,
        source.len(),
        retained.tokens.len(),
        retained.nodes.len(),
        round_trip,
        checksum,
        p50.as_secs_f64() * 1_000_000.0,
        p95.as_secs_f64() * 1_000_000.0,
        throughput,
        (allocated_after - allocated_before) / iterations,
        retained_after - retained_before,
        payload,
    );
}
