use chimi::frontend_benchmark::{self, ParsedFrontend};
use chimi::source::SourceSnapshot;
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

struct TrackingAllocator;

static TRACKING: AtomicBool = AtomicBool::new(false);
static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let resized = unsafe { System.realloc(pointer, layout, new_size) };
        if !resized.is_null() {
            if new_size >= layout.size() {
                LIVE_BYTES.fetch_add(new_size - layout.size(), Ordering::Relaxed);
            } else {
                LIVE_BYTES.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
            if TRACKING.load(Ordering::Relaxed) {
                ALLOCATED_BYTES.fetch_add(new_size, Ordering::Relaxed);
                update_peak();
            }
        }
        resized
    }
}

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

fn record_allocation(size: usize) {
    LIVE_BYTES.fetch_add(size, Ordering::Relaxed);
    if TRACKING.load(Ordering::Relaxed) {
        ALLOCATED_BYTES.fetch_add(size, Ordering::Relaxed);
        update_peak();
    }
}

fn update_peak() {
    PEAK_LIVE_BYTES.fetch_max(LIVE_BYTES.load(Ordering::Relaxed), Ordering::Relaxed);
}

#[derive(Clone, Copy)]
struct Measurement {
    allocated_bytes: usize,
    peak_live_bytes: usize,
    retained_bytes: usize,
}

fn measure_memory<T>(operation: impl FnOnce() -> T) -> (T, Measurement) {
    let baseline_live = LIVE_BYTES.load(Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
    PEAK_LIVE_BYTES.store(baseline_live, Ordering::Relaxed);
    TRACKING.store(true, Ordering::Relaxed);
    let value = operation();
    TRACKING.store(false, Ordering::Relaxed);
    let live = LIVE_BYTES.load(Ordering::Relaxed);
    (
        value,
        Measurement {
            allocated_bytes: ALLOCATED_BYTES.load(Ordering::Relaxed),
            peak_live_bytes: PEAK_LIVE_BYTES
                .load(Ordering::Relaxed)
                .saturating_sub(baseline_live),
            retained_bytes: live.saturating_sub(baseline_live),
        },
    )
}

struct Workload {
    id: &'static str,
    source: String,
    target_ns_p95: Option<u64>,
}

struct ResultRow {
    id: &'static str,
    source_bytes: usize,
    samples: usize,
    p50: Duration,
    p95: Duration,
    memory: Measurement,
    diagnostics: usize,
    nodes: usize,
    tokens: usize,
    missing_tokens: usize,
    target_ns_p95: Option<u64>,
}

fn main() {
    let (warmups, samples) = arguments();
    print_environment(warmups, samples);

    let valid_500 = generate_module(0, 500);
    let workloads = [
        Workload {
            id: "syntax.typical-valid",
            source: generate_module(0, 250),
            target_ns_p95: Some(16_000_000),
        },
        Workload {
            id: "syntax.valid-500",
            source: valid_500.clone(),
            target_ns_p95: None,
        },
        Workload {
            id: "syntax.large-valid",
            source: generate_module(0, 5_000),
            target_ns_p95: None,
        },
        Workload {
            id: "syntax.malformed-brace",
            source: remove_record_colon(&valid_500, false),
            target_ns_p95: None,
        },
        Workload {
            id: "syntax.malformed-pipeline",
            source: remove_pipeline_transform(&valid_500),
            target_ns_p95: None,
        },
        Workload {
            id: "editor.prefix-edit",
            source: format!(" {valid_500}"),
            target_ns_p95: Some(16_000_000),
        },
        Workload {
            id: "editor.middle-brace-edit",
            source: remove_record_colon(&valid_500, true),
            target_ns_p95: Some(16_000_000),
        },
    ];

    let mut results = Vec::new();
    for workload in workloads {
        let result = run_workload(workload, warmups, samples);
        print_result(&result);
        results.push(result);
    }
    let storage = run_storage_workload();
    print_storage_result(&storage);
    print_gate_summary(&results);
}

fn arguments() -> (usize, usize) {
    let mut warmups = 5;
    let mut samples = 40;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--bench" => {}
            "--warmups" => {
                warmups = parse_count("--warmups", arguments.next());
            }
            "--samples" => {
                samples = parse_count("--samples", arguments.next());
            }
            _ => panic!("unknown argument: {argument}"),
        }
    }
    assert!(samples >= 2, "--samples must be at least 2");
    (warmups, samples)
}

fn parse_count(name: &str, value: Option<String>) -> usize {
    value
        .unwrap_or_else(|| panic!("{name} requires an integer"))
        .parse()
        .unwrap_or_else(|_| panic!("{name} requires an integer"))
}

fn run_workload(workload: Workload, warmups: usize, samples: usize) -> ResultRow {
    let snapshot = SourceSnapshot::from_text(format!("{}.chimi", workload.id), workload.source);
    for _ in 0..warmups {
        black_box(frontend_benchmark::parse(black_box(&snapshot)));
    }

    let mut durations = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        let parsed = frontend_benchmark::parse(black_box(&snapshot));
        durations.push(start.elapsed());
        black_box(parsed);
    }
    durations.sort_unstable();

    let (parsed, memory) = measure_memory(|| frontend_benchmark::parse(&snapshot));
    let statistics = parsed
        .validate(&snapshot)
        .unwrap_or_else(|error| panic!("{} invariant failed: {error}", workload.id));
    drop(parsed);

    ResultRow {
        id: workload.id,
        source_bytes: snapshot.text().len(),
        samples,
        p50: percentile(&durations, 50),
        p95: percentile(&durations, 95),
        memory,
        diagnostics: statistics.diagnostic_count,
        nodes: statistics.node_count,
        tokens: statistics.token_count,
        missing_tokens: statistics.missing_token_count,
        target_ns_p95: workload.target_ns_p95,
    }
}

fn percentile(samples: &[Duration], percentile: usize) -> Duration {
    let index = (samples.len() * percentile).div_ceil(100).saturating_sub(1);
    samples[index]
}

struct StorageResult {
    files: usize,
    source_bytes: usize,
    elapsed: Duration,
    memory: Measurement,
    nodes: usize,
    tokens: usize,
}

fn run_storage_workload() -> StorageResult {
    let snapshots = (0..64)
        .map(|index| {
            SourceSnapshot::from_text(
                format!("generated/module_{index:04}.chimi"),
                generate_module(index, 150),
            )
        })
        .collect::<Vec<_>>();
    let source_bytes = snapshots.iter().map(|source| source.text().len()).sum();
    let start = Instant::now();
    let (parsed, memory): (Vec<ParsedFrontend>, Measurement) =
        measure_memory(|| snapshots.iter().map(frontend_benchmark::parse).collect());
    let elapsed = start.elapsed();
    let mut nodes = 0;
    let mut tokens = 0;
    for (result, source) in parsed.iter().zip(&snapshots) {
        let statistics = result
            .validate(source)
            .unwrap_or_else(|error| panic!("cst.project-storage invariant failed: {error}"));
        nodes += statistics.node_count;
        tokens += statistics.token_count;
    }
    drop(parsed);
    StorageResult {
        files: snapshots.len(),
        source_bytes,
        elapsed,
        memory,
        nodes,
        tokens,
    }
}

fn generate_module(module: usize, declarations: usize) -> String {
    let mut source = String::from("// GENERATED FRONTEND WORKLOAD — DO NOT EDIT\n");
    for item in 0..declarations {
        let suffix = format!("{module:04}_{item:04}");
        source.push_str(&format!(
            "struct Value_{suffix} {{\n  amount: Int\n}}\n\npub fn increment_{suffix}(value: Int) -> Int {{\n  value + 1\n}}\n\nlet configuration_{suffix} = {{\n  retry_limit: 3,\n  tracing: True,\n}}\n\nlet transformed_{suffix} = 1\n|> increment_{suffix}\n\n"
        ));
    }
    source
}

fn remove_record_colon(source: &str, middle: bool) -> String {
    let needle = "retry_limit:";
    let offset = if middle {
        let offsets = source
            .match_indices(needle)
            .map(|(offset, _)| offset)
            .collect::<Vec<_>>();
        offsets[offsets.len() / 2] + "retry_limit".len()
    } else {
        source.find(needle).expect("generated record") + "retry_limit".len()
    };
    let mut changed = source.to_owned();
    changed.remove(offset);
    changed
}

fn remove_pipeline_transform(source: &str) -> String {
    let marker = "|> increment_";
    let start = source.find(marker).expect("generated pipeline") + "|> ".len();
    let end = source[start..].find('\n').expect("pipeline line") + start;
    let mut changed = source.to_owned();
    changed.replace_range(start..end, "");
    changed
}

fn print_environment(warmups: usize, samples: usize) {
    let revision = command_output("git", &["rev-parse", "--short", "HEAD"]);
    let rustc = command_output("rustc", &["--version"]);
    let cpu = if cfg!(target_os = "macos") {
        command_output("sysctl", &["-n", "machdep.cpu.brand_string"])
    } else {
        "unknown".to_owned()
    };
    let cores = std::thread::available_parallelism().map_or(0, usize::from);
    println!(
        "{{\"type\":\"environment\",\"revision\":\"{}\",\"mode\":\"release\",\"os\":\"{}\",\"architecture\":\"{}\",\"cpu\":\"{}\",\"logical_cores\":{},\"memory_bytes\":{},\"rustc\":\"{}\",\"warmups\":{},\"samples\":{}}}",
        json(&revision),
        std::env::consts::OS,
        std::env::consts::ARCH,
        json(&cpu),
        cores,
        machine_memory_bytes().map_or("null".to_owned(), |bytes| bytes.to_string()),
        json(&rustc),
        warmups,
        samples,
    );
}

fn print_result(result: &ResultRow) {
    let target = result
        .target_ns_p95
        .map_or("null".to_owned(), |value| value.to_string());
    println!(
        "{{\"type\":\"workload\",\"id\":\"{}\",\"source_bytes\":{},\"samples\":{},\"p50_ns\":{},\"p95_ns\":{},\"target_p95_ns\":{},\"allocated_bytes\":{},\"peak_live_bytes\":{},\"peak_rss_bytes\":null,\"retained_cst_bytes\":{},\"diagnostics\":{},\"nodes\":{},\"tokens\":{},\"missing_tokens\":{},\"invariants\":true}}",
        result.id,
        result.source_bytes,
        result.samples,
        result.p50.as_nanos(),
        result.p95.as_nanos(),
        target,
        result.memory.allocated_bytes,
        result.memory.peak_live_bytes,
        result.memory.retained_bytes,
        result.diagnostics,
        result.nodes,
        result.tokens,
        result.missing_tokens,
    );
}

fn print_storage_result(result: &StorageResult) {
    println!(
        "{{\"type\":\"workload\",\"id\":\"cst.project-storage\",\"files\":{},\"source_bytes\":{},\"wall_time_ns\":{},\"allocated_bytes\":{},\"peak_live_bytes\":{},\"peak_rss_bytes\":null,\"retained_cst_bytes\":{},\"retained_bytes_per_source_byte\":{:.3},\"nodes\":{},\"tokens\":{},\"invariants\":true}}",
        result.files,
        result.source_bytes,
        result.elapsed.as_nanos(),
        result.memory.allocated_bytes,
        result.memory.peak_live_bytes,
        result.memory.retained_bytes,
        result.memory.retained_bytes as f64 / result.source_bytes as f64,
        result.nodes,
        result.tokens,
    );
}

fn print_gate_summary(results: &[ResultRow]) {
    let valid_500 = results
        .iter()
        .find(|result| result.id == "syntax.valid-500")
        .expect("valid comparison workload");
    let malformed_pass = results
        .iter()
        .filter(|result| result.id.starts_with("syntax.malformed"))
        .all(|result| result.p95 <= valid_500.p95.saturating_mul(2));
    let latency_pass = results
        .iter()
        .filter_map(|result| {
            result
                .target_ns_p95
                .map(|target| result.p95.as_nanos() <= u128::from(target))
        })
        .all(|passed| passed);
    println!(
        "{{\"type\":\"gate\",\"latency_targets_pass\":{},\"malformed_ratio_pass\":{},\"pass\":{}}}",
        latency_pass,
        malformed_pass,
        latency_pass && malformed_pass,
    );
}

fn command_output(program: &str, arguments: &[&str]) -> String {
    Command::new(program)
        .args(arguments)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|| "unknown".to_owned())
}

fn machine_memory_bytes() -> Option<u64> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    command_output("sysctl", &["-n", "hw.memsize"]).parse().ok()
}

fn json(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
