use chimi::frontend_benchmark::{self, ParsedFrontend};
use chimi::source::SourceSnapshot;
use chimi::{Compiler, CompilerSession, IncrementalStatistics, ProjectSource, TypeArenaStats};
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

struct Arguments {
    warmups: usize,
    samples: usize,
    semantics_only: bool,
    semantic_files: usize,
    semantic_declarations: usize,
    semantic_case: String,
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
    let arguments = arguments();
    print_environment(arguments.warmups, arguments.samples);

    if !arguments.semantics_only {
        run_syntax_benchmarks(arguments.warmups, arguments.samples);
    }
    if matches!(arguments.semantic_case.as_str(), "all" | "clean") {
        let semantic = run_semantic_workload(
            arguments.warmups,
            arguments.samples,
            arguments.semantic_files,
            arguments.semantic_declarations,
        );
        print_semantic_result(&semantic);
    }
    if arguments.semantic_case != "clean" {
        let incremental = run_incremental_workloads(
            arguments.warmups,
            arguments.samples,
            arguments.semantic_files,
            arguments.semantic_declarations,
            &arguments.semantic_case,
        );
        for result in incremental {
            print_incremental_result(&result);
        }
    }
}

fn run_syntax_benchmarks(warmups: usize, samples: usize) {
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

fn arguments() -> Arguments {
    let mut parsed = Arguments {
        warmups: 5,
        samples: 40,
        semantics_only: false,
        semantic_files: 256,
        semantic_declarations: 20,
        semantic_case: "all".to_owned(),
    };
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--bench" => {}
            "--warmups" => {
                parsed.warmups = parse_count("--warmups", arguments.next());
            }
            "--samples" => {
                parsed.samples = parse_count("--samples", arguments.next());
            }
            "--semantics-only" => parsed.semantics_only = true,
            "--semantic-files" => {
                parsed.semantic_files = parse_count("--semantic-files", arguments.next());
            }
            "--semantic-declarations" => {
                parsed.semantic_declarations =
                    parse_count("--semantic-declarations", arguments.next());
            }
            "--semantic-case" => {
                parsed.semantic_case = arguments
                    .next()
                    .filter(|value| {
                        matches!(
                            value.as_str(),
                            "all" | "clean" | "no-op" | "local" | "fanout"
                        )
                    })
                    .unwrap_or_else(|| {
                        panic!("--semantic-case requires all, clean, no-op, local, or fanout")
                    });
            }
            _ => panic!("unknown argument: {argument}"),
        }
    }
    assert!(parsed.samples >= 2, "--samples must be at least 2");
    assert!(
        parsed.semantic_files > 0,
        "--semantic-files must be positive"
    );
    assert!(
        parsed.semantic_declarations > 0,
        "--semantic-declarations must be positive"
    );
    parsed
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

struct SemanticResult {
    files: usize,
    source_bytes: usize,
    samples: usize,
    resolution_p50: Duration,
    resolution_p95: Duration,
    p50: Duration,
    p95: Duration,
    memory: Measurement,
    statistics: TypeArenaStats,
}

struct IncrementalResult {
    id: &'static str,
    files: usize,
    source_bytes: usize,
    samples: usize,
    p50: Duration,
    p95: Duration,
    clean_p95: Duration,
    target_ns_p95: Option<u64>,
    memory: Measurement,
    statistics: IncrementalStatistics,
    diagnostics: usize,
}

fn run_semantic_workload(
    warmups: usize,
    samples: usize,
    files: usize,
    declarations: usize,
) -> SemanticResult {
    let (sources, source_bytes) = generate_semantic_project(files, declarations, None);
    for _ in 0..warmups {
        let report = Compiler::resolve_project(black_box(sources.clone()));
        assert!(report.is_success(), "{}", report.render_diagnostics());
        black_box(report);
    }
    let mut resolution_durations = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        let report = Compiler::resolve_project(black_box(sources.clone()));
        resolution_durations.push(start.elapsed());
        assert!(report.is_success(), "{}", report.render_diagnostics());
        black_box(report);
    }
    resolution_durations.sort_unstable();

    for _ in 0..warmups {
        let report = Compiler::type_project(black_box(sources.clone()));
        assert!(report.is_success(), "{}", report.render_diagnostics());
        black_box(report);
    }

    let mut durations = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        let report = Compiler::type_project(black_box(sources.clone()));
        durations.push(start.elapsed());
        assert!(report.is_success(), "{}", report.render_diagnostics());
        black_box(report);
    }
    durations.sort_unstable();

    let (report, memory) = measure_memory(|| Compiler::type_project(sources.clone()));
    assert!(report.is_success(), "{}", report.render_diagnostics());
    let statistics = report.arena_stats();
    drop(report);
    SemanticResult {
        files: sources.len(),
        source_bytes,
        samples,
        resolution_p50: percentile(&resolution_durations, 50),
        resolution_p95: percentile(&resolution_durations, 95),
        p50: percentile(&durations, 50),
        p95: percentile(&durations, 95),
        memory,
        statistics,
    }
}

fn run_incremental_workloads(
    warmups: usize,
    samples: usize,
    files: usize,
    declarations: usize,
    semantic_case: &str,
) -> Vec<IncrementalResult> {
    let (before, source_bytes) = generate_semantic_project(files, declarations, None);
    let (after, after_bytes) = generate_semantic_project(files, declarations, Some(files - 1));
    assert_eq!(source_bytes, after_bytes);
    let (fanout_before, _) = generate_fanout_project(files, declarations, false);
    let (fanout_after, fanout_bytes) = generate_fanout_project(files, declarations, true);
    let mut results = Vec::new();
    if matches!(semantic_case, "all" | "no-op") {
        results.push(run_incremental_workload(
            "semantics.no-op-reanalysis",
            &before,
            &before,
            source_bytes,
            warmups,
            samples,
            Some(100_000_000),
        ));
    }
    if matches!(semantic_case, "all" | "local") {
        results.push(run_incremental_workload(
            "semantics.local-edit",
            &before,
            &after,
            source_bytes,
            warmups,
            samples,
            Some(100_000_000),
        ));
    }
    if matches!(semantic_case, "all" | "fanout") {
        results.push(run_incremental_workload(
            "semantics.fanout-edit",
            &fanout_before,
            &fanout_after,
            fanout_bytes,
            warmups,
            samples,
            None,
        ));
    }
    results
}

fn run_incremental_workload(
    id: &'static str,
    before: &[ProjectSource],
    after: &[ProjectSource],
    source_bytes: usize,
    warmups: usize,
    samples: usize,
    target_ns_p95: Option<u64>,
) -> IncrementalResult {
    let baseline = Compiler::type_project(before.to_vec());
    assert!(baseline.is_success(), "{}", baseline.render_diagnostics());
    let clean = Compiler::type_project(after.to_vec());
    for _ in 0..warmups {
        black_box(Compiler::type_project(black_box(after.to_vec())));
    }
    let mut clean_durations = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        black_box(Compiler::type_project(black_box(after.to_vec())));
        clean_durations.push(start.elapsed());
    }
    clean_durations.sort_unstable();
    for _ in 0..warmups {
        let mut session = CompilerSession::new();
        drop(session.analyze_project(before.to_vec()));
        let report = session.analyze_project(after.to_vec());
        assert_eq!(report.diagnostics(), clean.diagnostics());
        black_box(report);
    }

    let mut durations = Vec::with_capacity(samples);
    for _ in 0..samples {
        let mut session = CompilerSession::new();
        drop(session.analyze_project(before.to_vec()));
        let start = Instant::now();
        let report = session.analyze_project(after.to_vec());
        durations.push(start.elapsed());
        assert_eq!(report.diagnostics(), clean.diagnostics());
        black_box(report);
    }
    durations.sort_unstable();

    let mut session = CompilerSession::new();
    drop(session.analyze_project(before.to_vec()));
    let (report, memory) = measure_memory(|| session.analyze_project(after.to_vec()));
    assert_eq!(report.diagnostics(), clean.diagnostics());
    assert_eq!(report.arena_stats(), clean.arena_stats());
    let statistics = report.statistics();
    drop(report);
    IncrementalResult {
        id,
        files: after.len(),
        source_bytes,
        samples,
        p50: percentile(&durations, 50),
        p95: percentile(&durations, 95),
        clean_p95: percentile(&clean_durations, 95),
        target_ns_p95,
        memory,
        statistics,
        diagnostics: clean.diagnostics().len(),
    }
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
            "struct Value_{suffix} {{\n  amount: Int\n}}\n\npub fn increment_{suffix}(value: Int) -> Int {{\n  value + 1\n}}\n\nconst configuration_{suffix} = {{\n  retry_limit: 3,\n  tracing: True,\n}}\n\nconst transformed_{suffix} = 1\n|> increment_{suffix}\n\n"
        ));
    }
    source
}

fn generate_semantic_project(
    files: usize,
    declarations: usize,
    edited_module: Option<usize>,
) -> (Vec<ProjectSource>, usize) {
    let mut source_bytes = 0;
    let sources = (0..files)
        .map(|module| {
            let module_path = format!("generated/module_{module:04}");
            let mut text = generate_module(module, declarations);
            if edited_module == Some(module) {
                let offset =
                    text.find("value + 1").expect("generated function body") + "value + ".len();
                text.replace_range(offset..offset + 1, "2");
            }
            source_bytes += text.len();
            let source = SourceSnapshot::from_text(format!("{module_path}.chimi"), text);
            ProjectSource::new(module_path, source).expect("generated module path is valid")
        })
        .collect();
    (sources, source_bytes)
}

fn generate_fanout_project(
    files: usize,
    declarations: usize,
    changed_contract: bool,
) -> (Vec<ProjectSource>, usize) {
    let mut source_bytes = 0;
    let sources = (0..files)
        .map(|module| {
            let module_path = format!("generated/module_{module:04}");
            let mut text = generate_module(module, declarations);
            if module == 0 {
                let input = if changed_contract { "Text" } else { "Int" };
                text.push_str(&format!(
                    "pub fn shared(value: {input}) -> Int {{\n  1\n}}\n"
                ));
            } else {
                text.insert_str(0, "import generated/module_0000 as shared\n\n");
                text.push_str(&format!(
                    "pub fn use_shared_{module:04}(value: Int) -> Int {{\n  shared.shared(value)\n}}\n"
                ));
            }
            source_bytes += text.len();
            let source = SourceSnapshot::from_text(format!("{module_path}.chimi"), text);
            ProjectSource::new(module_path, source).expect("generated module path is valid")
        })
        .collect();
    (sources, source_bytes)
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

fn print_semantic_result(result: &SemanticResult) {
    println!(
        "{{\"type\":\"workload\",\"id\":\"semantics.clean-project\",\"files\":{},\"source_bytes\":{},\"samples\":{},\"resolution_p50_ns\":{},\"resolution_p95_ns\":{},\"p50_ns\":{},\"p95_ns\":{},\"target_p95_ns\":null,\"allocated_bytes\":{},\"peak_live_bytes\":{},\"peak_rss_bytes\":null,\"retained_analysis_bytes\":{},\"types\":{},\"failure_rows\":{},\"needs_rows\":{},\"callable_contracts\":{},\"type_facts\":{},\"expression_facts\":{},\"declaration_types\":{},\"field_shapes\":{},\"invariants\":true}}",
        result.files,
        result.source_bytes,
        result.samples,
        result.resolution_p50.as_nanos(),
        result.resolution_p95.as_nanos(),
        result.p50.as_nanos(),
        result.p95.as_nanos(),
        result.memory.allocated_bytes,
        result.memory.peak_live_bytes,
        result.memory.retained_bytes,
        result.statistics.types,
        result.statistics.failure_rows,
        result.statistics.needs_rows,
        result.statistics.callable_contracts,
        result.statistics.type_facts,
        result.statistics.expression_facts,
        result.statistics.declaration_types,
        result.statistics.field_shapes,
    );
}

fn print_incremental_result(result: &IncrementalResult) {
    let target = result
        .target_ns_p95
        .map_or("null".to_owned(), |target| target.to_string());
    println!(
        "{{\"type\":\"workload\",\"id\":\"{}\",\"files\":{},\"source_bytes\":{},\"samples\":{},\"p50_ns\":{},\"p95_ns\":{},\"clean_p95_ns\":{},\"target_p95_ns\":{},\"allocated_bytes\":{},\"peak_live_bytes\":{},\"peak_rss_bytes\":null,\"retained_incremental_bytes\":{},\"reused_parsed_modules\":{},\"parsed_modules\":{},\"reused_semantic_modules\":{},\"lowered_modules\":{},\"invalidated_modules\":{},\"invalidated_bytes\":{},\"rechecked_modules\":{},\"reused_type_facts\":{},\"invalidated_type_facts\":{},\"diagnostics\":{},\"bounded_by_clean\":{},\"equivalent_to_clean\":true}}",
        result.id,
        result.files,
        result.source_bytes,
        result.samples,
        result.p50.as_nanos(),
        result.p95.as_nanos(),
        result.clean_p95.as_nanos(),
        target,
        result.memory.allocated_bytes,
        result.memory.peak_live_bytes,
        result.memory.retained_bytes,
        result.statistics.reused_parsed_modules,
        result.statistics.parsed_modules,
        result.statistics.reused_semantic_modules,
        result.statistics.lowered_modules,
        result.statistics.invalidated_modules,
        result.statistics.invalidated_bytes,
        result.statistics.rechecked_modules,
        result.statistics.reused_type_facts,
        result.statistics.invalidated_type_facts,
        result.diagnostics,
        result.p95 <= result.clean_p95,
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
