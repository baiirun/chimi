use chimi::execution::{EventKind, ExecutionConfig, ExecutionOutcome, TraceMode};
use chimi::source::SourceSnapshot;
use chimi::{BackendProfile, BackendProgram, Compiler, ProjectSource};
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

struct CountingAllocator;

static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK_LIVE: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, old: Layout, new_size: usize) -> *mut u8 {
        let next = unsafe { System.realloc(pointer, old, new_size) };
        if !next.is_null() {
            if new_size >= old.size() {
                record_allocation(new_size - old.size());
            } else {
                LIVE.fetch_sub(old.size() - new_size, Ordering::Relaxed);
            }
        }
        next
    }
}

fn record_allocation(bytes: usize) {
    ALLOCATED.fetch_add(bytes, Ordering::Relaxed);
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    let mut peak = PEAK_LIVE.load(Ordering::Relaxed);
    while live > peak {
        match PEAK_LIVE.compare_exchange_weak(peak, live, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(current) => peak = current,
        }
    }
}

#[derive(Clone, Copy)]
struct Arguments {
    warmups: usize,
    samples: usize,
}

struct Workload {
    id: &'static str,
    sources: Vec<ProjectSource>,
    source_bytes: usize,
    expected_outcome: &'static str,
    initial_control: InitialControl,
}

#[derive(Clone, Copy)]
enum InitialControl {
    Run,
    Cancel,
}

#[derive(Clone, Copy)]
struct Memory {
    allocated: usize,
    peak_live: usize,
    retained: usize,
}

struct Measurement<T> {
    value: T,
    memory: Memory,
}

struct ResultRow {
    id: &'static str,
    files: usize,
    source_bytes: usize,
    profile: BackendProfile,
    prepare_p50: Duration,
    prepare_p95: Duration,
    prepare_memory: Memory,
    boundary_p50: Duration,
    boundary_p95: Duration,
    boundary_memory: Memory,
    boundary_events: usize,
    full_p50: Duration,
    full_p95: Duration,
    full_memory: Memory,
    full_events: usize,
    outcome: &'static str,
}

fn main() {
    let arguments = arguments();
    println!(
        "{{\"schema\":\"chimi.backend-comparison.v1\",\"kind\":\"environment\",\"os\":\"{}\",\"arch\":\"{}\",\"warmups\":{},\"samples\":{},\"startup_ns\":null,\"artifact_bytes\":null}}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        arguments.warmups,
        arguments.samples,
    );

    for workload in workloads() {
        print_result(&run_workload(workload, arguments));
    }
}

fn arguments() -> Arguments {
    let mut parsed = Arguments {
        warmups: 5,
        samples: 40,
    };
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--bench" => {}
            "--warmups" => parsed.warmups = parse_count("--warmups", arguments.next()),
            "--samples" => parsed.samples = parse_count("--samples", arguments.next()),
            _ => panic!("unknown argument: {argument}"),
        }
    }
    assert!(parsed.samples >= 2, "--samples must be at least 2");
    parsed
}

fn parse_count(name: &str, value: Option<String>) -> usize {
    value
        .unwrap_or_else(|| panic!("{name} requires a value"))
        .parse()
        .unwrap_or_else(|_| panic!("{name} requires a non-negative integer"))
}

fn workloads() -> Vec<Workload> {
    let checkout =
        include_str!("../../../prototypes/golden/valid/dependencies_and_execution.chimi");
    let retry = include_str!("../../../prototypes/retry-composition/lib/retry.chimi");
    let retry_main = include_str!("../../../prototypes/retry-composition/app/main.chimi");
    let rich = include_str!("../../../prototypes/backend-comparison/rich-success.chimi");
    let defect = include_str!("../../../prototypes/backend-comparison/defect.chimi");
    let cancellation = include_str!("../../../prototypes/backend-comparison/cancellation.chimi");
    vec![
        Workload {
            id: "rich-success",
            sources: vec![source(
                "comparison/rich",
                "prototypes/backend-comparison/rich-success.chimi",
                rich,
            )],
            source_bytes: rich.len(),
            expected_outcome: "success",
            initial_control: InitialControl::Run,
        },
        Workload {
            id: "checkout-success",
            sources: vec![source(
                "checkout/main",
                "dependencies_and_execution.chimi",
                checkout,
            )],
            source_bytes: checkout.len(),
            expected_outcome: "success",
            initial_control: InitialControl::Run,
        },
        Workload {
            id: "retry-exhaustion",
            sources: vec![
                source(
                    "lib/retry",
                    "prototypes/retry-composition/lib/retry.chimi",
                    retry,
                ),
                source(
                    "app/main",
                    "prototypes/retry-composition/app/main.chimi",
                    retry_main,
                ),
            ],
            source_bytes: retry.len() + retry_main.len(),
            expected_outcome: "expected_failure",
            initial_control: InitialControl::Run,
        },
        Workload {
            id: "defect",
            sources: vec![source(
                "comparison/defect",
                "prototypes/backend-comparison/defect.chimi",
                defect,
            )],
            source_bytes: defect.len(),
            expected_outcome: "defect",
            initial_control: InitialControl::Run,
        },
        Workload {
            id: "cancellation",
            sources: vec![source(
                "comparison/cancellation",
                "prototypes/backend-comparison/cancellation.chimi",
                cancellation,
            )],
            source_bytes: cancellation.len(),
            expected_outcome: "cancellation",
            initial_control: InitialControl::Cancel,
        },
    ]
}

fn source(module_path: &str, path: &str, text: &str) -> ProjectSource {
    ProjectSource::new(module_path, SourceSnapshot::from_text(path, text))
        .expect("benchmark module path is valid")
}

fn run_workload(workload: Workload, arguments: Arguments) -> ResultRow {
    for _ in 0..arguments.warmups {
        black_box(prepare(workload.sources.clone()));
    }
    let mut prepare_durations = Vec::with_capacity(arguments.samples);
    for _ in 0..arguments.samples {
        let start = Instant::now();
        let program = prepare(black_box(workload.sources.clone()));
        prepare_durations.push(start.elapsed());
        black_box(program);
    }
    prepare_durations.sort_unstable();
    let prepared = measure(|| prepare(workload.sources.clone()));
    let program = prepared.value;
    let profile = program.profile();
    assert_eq!(profile.operations.total(), profile.nodes);

    let boundary = measure_execution(
        &program,
        TraceMode::Boundaries,
        arguments,
        workload.expected_outcome,
        workload.initial_control,
    );
    let full = measure_execution(
        &program,
        TraceMode::Full,
        arguments,
        workload.expected_outcome,
        workload.initial_control,
    );
    assert_eq!(boundary.outcome, full.outcome);

    ResultRow {
        id: workload.id,
        files: workload.sources.len(),
        source_bytes: workload.source_bytes,
        profile,
        prepare_p50: percentile(&prepare_durations, 50),
        prepare_p95: percentile(&prepare_durations, 95),
        prepare_memory: prepared.memory,
        boundary_p50: boundary.p50,
        boundary_p95: boundary.p95,
        boundary_memory: boundary.memory,
        boundary_events: boundary.events,
        full_p50: full.p50,
        full_p95: full.p95,
        full_memory: full.memory,
        full_events: full.events,
        outcome: boundary.outcome,
    }
}

fn prepare(sources: Vec<ProjectSource>) -> BackendProgram {
    Compiler::prepare_backend(sources)
        .unwrap_or_else(|diagnostics| panic!("backend workload did not prepare: {diagnostics:#?}"))
}

struct ExecutionMeasurement {
    p50: Duration,
    p95: Duration,
    memory: Memory,
    events: usize,
    outcome: &'static str,
}

fn measure_execution(
    program: &BackendProgram,
    trace_mode: TraceMode,
    arguments: Arguments,
    expected_outcome: &'static str,
    initial_control: InitialControl,
) -> ExecutionMeasurement {
    for _ in 0..arguments.warmups {
        black_box(program.execute_oracle(config(trace_mode, initial_control)));
    }
    let mut durations = Vec::with_capacity(arguments.samples);
    for _ in 0..arguments.samples {
        let start = Instant::now();
        let report = program.execute_oracle(config(trace_mode, initial_control));
        durations.push(start.elapsed());
        assert_eq!(outcome_name(&report.outcome), expected_outcome);
        black_box(report);
    }
    durations.sort_unstable();
    let measured = measure(|| program.execute_oracle(config(trace_mode, initial_control)));
    let outcome = outcome_name(&measured.value.outcome);
    assert_eq!(outcome, expected_outcome);
    assert_eq!(
        measured.value.events.first().map(|event| event.kind),
        Some(EventKind::ComputationStarted)
    );
    assert_eq!(
        measured.value.events.last().map(|event| event.kind),
        Some(EventKind::ComputationCompleted)
    );
    ExecutionMeasurement {
        p50: percentile(&durations, 50),
        p95: percentile(&durations, 95),
        memory: measured.memory,
        events: measured.value.events.len(),
        outcome,
    }
}

fn config(trace_mode: TraceMode, initial_control: InitialControl) -> ExecutionConfig {
    let config = ExecutionConfig::new("backend-benchmark")
        .expect("benchmark execution ID is valid")
        .with_trace_mode(trace_mode);
    match initial_control {
        InitialControl::Run => config,
        InitialControl::Cancel => config.with_cancellation_requested(),
    }
}

fn outcome_name(outcome: &ExecutionOutcome) -> &'static str {
    match outcome {
        ExecutionOutcome::Success(_) => "success",
        ExecutionOutcome::ExpectedFailure(_) => "expected_failure",
        ExecutionOutcome::Defect(_) => "defect",
        ExecutionOutcome::Cancellation(_) => "cancellation",
    }
}

fn measure<T>(operation: impl FnOnce() -> T) -> Measurement<T> {
    let baseline_allocated = ALLOCATED.load(Ordering::Relaxed);
    let baseline_live = LIVE.load(Ordering::Relaxed);
    PEAK_LIVE.store(baseline_live, Ordering::Relaxed);
    let value = operation();
    Measurement {
        value,
        memory: Memory {
            allocated: ALLOCATED.load(Ordering::Relaxed) - baseline_allocated,
            peak_live: PEAK_LIVE.load(Ordering::Relaxed) - baseline_live,
            retained: LIVE.load(Ordering::Relaxed) - baseline_live,
        },
    }
}

fn percentile(durations: &[Duration], percentile: usize) -> Duration {
    durations[(durations.len() - 1) * percentile / 100]
}

fn print_result(result: &ResultRow) {
    let operations = result.profile.operations;
    let values = result.profile.values;
    println!(
        concat!(
            "{{\"schema\":\"chimi.backend-comparison.v1\",\"kind\":\"workload\",",
            "\"id\":\"{}\",\"files\":{},\"source_bytes\":{},\"outcome\":\"{}\",",
            "\"profile\":{{\"types\":{},\"symbols\":{},\"functions\":{},\"bindings\":{},",
            "\"nodes\":{},\"origins\":{},\"operations\":{{\"constants\":{},\"references\":{},",
            "\"selects\":{},\"dependency_methods\":{},\"constructions\":{},\"calls\":{},",
            "\"lists\":{},\"records\":{},\"unary\":{},\"binary\":{},\"identities\":{},",
            "\"provisions\":{},\"sequences\":{},\"closures\":{},\"matches\":{}}},",
            "\"values\":{{\"integer_literals\":{},\"text_literals\":{},\"text_literal_bytes\":{},",
            "\"boolean_literals\":{},\"list_elements\":{},\"record_fields\":{},",
            "\"provider_fields\":{},\"constructor_arguments\":{},\"call_arguments\":{},",
            "\"sequence_steps\":{},\"closure_parameters\":{},\"match_subjects\":{},",
            "\"match_arms\":{},\"pattern_nodes\":{},\"max_list_elements\":{},",
            "\"max_record_fields\":{},\"max_provider_fields\":{},\"max_constructor_arguments\":{},",
            "\"max_call_arguments\":{},\"max_sequence_steps\":{},\"max_closure_parameters\":{},",
            "\"max_match_subjects\":{},\"max_match_arms\":{},\"max_pattern_depth\":{}}}}},",
            "\"prepare\":{},\"boundary\":{},\"full\":{},\"startup_ns\":null,\"artifact_bytes\":null}}"
        ),
        result.id,
        result.files,
        result.source_bytes,
        result.outcome,
        result.profile.types,
        result.profile.symbols,
        result.profile.functions,
        result.profile.bindings,
        result.profile.nodes,
        result.profile.origins,
        operations.constants,
        operations.references,
        operations.selects,
        operations.dependency_methods,
        operations.constructions,
        operations.calls,
        operations.lists,
        operations.records,
        operations.unary,
        operations.binary,
        operations.identities,
        operations.provisions,
        operations.sequences,
        operations.closures,
        operations.matches,
        values.integer_literals,
        values.text_literals,
        values.text_literal_bytes,
        values.boolean_literals,
        values.list_elements,
        values.record_fields,
        values.provider_fields,
        values.constructor_arguments,
        values.call_arguments,
        values.sequence_steps,
        values.closure_parameters,
        values.match_subjects,
        values.match_arms,
        values.pattern_nodes,
        values.max_list_elements,
        values.max_record_fields,
        values.max_provider_fields,
        values.max_constructor_arguments,
        values.max_call_arguments,
        values.max_sequence_steps,
        values.max_closure_parameters,
        values.max_match_subjects,
        values.max_match_arms,
        values.max_pattern_depth,
        memory_json(
            result.prepare_p50,
            result.prepare_p95,
            result.prepare_memory,
            0
        ),
        memory_json(
            result.boundary_p50,
            result.boundary_p95,
            result.boundary_memory,
            result.boundary_events,
        ),
        memory_json(
            result.full_p50,
            result.full_p95,
            result.full_memory,
            result.full_events,
        ),
    );
}

fn memory_json(p50: Duration, p95: Duration, memory: Memory, events: usize) -> String {
    format!(
        "{{\"p50_ns\":{},\"p95_ns\":{},\"allocated_bytes\":{},\"peak_live_bytes\":{},\"retained_bytes\":{},\"events\":{}}}",
        p50.as_nanos(),
        p95.as_nanos(),
        memory.allocated,
        memory.peak_live,
        memory.retained,
        events,
    )
}
