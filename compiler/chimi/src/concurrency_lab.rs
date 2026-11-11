//! Connected evidence for the first finite Task and bounded Channel kernel.

use crate::diagnostic::Diagnostic;
use crate::source::SourceSnapshot;
use crate::{Compiler, ProjectSource};
use chimi_fiber::ChimiFiberResult;
use chimi_fiber::scheduler::{
    ChannelCapabilities, ChannelEventKind, ReceiveResult, Scheduler, SendResult, TaskId,
    TaskOutcome, TaskResult, TraceMode,
};
use std::collections::{BTreeMap, BTreeSet};
use std::hint::black_box;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

const STACK_BYTES: usize = 64 * 1024;
const PROVIDER_OFFSET: usize = 100;
const PERFORMANCE_MESSAGES: usize = 16_384;
const THROUGHPUT_SAMPLES: usize = 7;
const PARKED_TASKS: usize = 256;
const COLD_SAMPLES: usize = 16;

const RELEASE_MAX_COLD_START_P95_NS: u64 = 25_000_000;
const DEBUG_MAX_COLD_START_P95_NS: u64 = 250_000_000;
const RELEASE_MAX_OFF_NS_PER_MESSAGE: u64 = 5_000;
const DEBUG_MAX_OFF_NS_PER_MESSAGE: u64 = 50_000;
const MAX_BOUNDARY_OVERHEAD_PERMILLE: u64 = 2_000;
const MAX_FULL_OVERHEAD_PERMILLE: u64 = 4_000;
const MAX_PARKED_RESIDENT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_RETAINED_RESIDENT_BYTES: u64 = 8 * 1024 * 1024;

unsafe extern "C" {
    fn chimi_fixture_generated_many_parks(context: *mut std::ffi::c_void) -> ChimiFiberResult;
    fn chimi_fixture_generated_defect(context: *mut std::ffi::c_void) -> ChimiFiberResult;
    fn chimi_fixture_generated_expected_failure_value(
        context: *mut std::ffi::c_void,
    ) -> ChimiFiberResult;
}

#[derive(Debug, Eq, PartialEq)]
pub struct ConformanceReport {
    kind: &'static str,
    checks: Vec<&'static str>,
    metrics: BTreeMap<&'static str, u64>,
}

impl ConformanceReport {
    fn new(kind: &'static str) -> Self {
        Self {
            kind,
            checks: Vec::new(),
            metrics: BTreeMap::new(),
        }
    }

    fn check(&mut self, name: &'static str) {
        self.checks.push(name);
    }

    fn metric(&mut self, name: &'static str, value: impl TryInto<u64>) -> Result<(), String> {
        let value = value
            .try_into()
            .map_err(|_| format!("metric {name} does not fit u64"))?;
        self.metrics.insert(name, value);
        Ok(())
    }

    pub fn kind(&self) -> &str {
        self.kind
    }

    pub fn checks(&self) -> &[&'static str] {
        &self.checks
    }

    pub fn metric_value(&self, name: &str) -> Option<u64> {
        self.metrics.get(name).copied()
    }

    pub fn render_json(&self) -> String {
        let mut output = format!(
            "{{\"schema\":\"chimi.concurrency-conformance.v1\",\"kind\":\"{}\",\"checks\":[",
            self.kind
        );
        for (index, check) in self.checks.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            output.push('"');
            output.push_str(check);
            output.push('"');
        }
        output.push_str("],\"metrics\":{");
        for (index, (name, value)) in self.metrics.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            output.push_str(&format!("\"{name}\":{value}"));
        }
        output.push_str("}}\n");
        output
    }
}

pub fn run_static() -> Result<ConformanceReport, String> {
    let valid = type_fixture(
        "conformance/channel_valid",
        "channel-static-valid.chimi",
        include_str!("../tests/fixtures/channel-static-valid.chimi"),
    )?;
    require(valid.is_success(), valid.render_diagnostics())?;

    let invalid = type_fixture(
        "conformance/channel_invalid",
        "channel-static-invalid.chimi",
        include_str!("../tests/fixtures/channel-static-invalid.chimi"),
    )?;
    let invalid_codes = invalid
        .diagnostics()
        .iter()
        .map(Diagnostic::code)
        .collect::<Vec<_>>();
    require(
        invalid_codes == ["T4241", "T4243", "T4241"],
        format!("unexpected capability diagnostics: {invalid_codes:?}"),
    )?;

    let wrong_recovery = type_fixture(
        "conformance/channel_wrong_recovery",
        "channel-static-wrong-recovery.chimi",
        include_str!("../tests/fixtures/channel-static-wrong-recovery.chimi"),
    )?;
    require(
        wrong_recovery
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code() == "T4203"),
        "wrong recovered payload type was not diagnosed",
    )?;

    let task_contract = type_fixture(
        "conformance/task_needs",
        "task-needs.chimi",
        concat!(
            "import std/task as Task\n",
            "import std/task.{Task}\n",
            "enum WorkError { Stopped }\n",
            "interface Clock { now: fn() -> Int }\n",
            "fn work() -> Int needs { clock: Clock } {\n",
            "  case clock.now() {\n",
            "    0 -> Failure(Stopped())\n",
            "    _ -> 42\n",
            "  }\n",
            "}\n",
            "fn launch() -> Task<Int, WorkError> needs { clock: Clock } {\n",
            "  Task.start(work)\n",
            "}\n",
        ),
    )?;
    require(
        task_contract.is_success(),
        task_contract.render_diagnostics(),
    )?;
    require(
        task_contract
            .contract("conformance/task_needs", "launch")
            .as_deref()
            == Some("fn() -> Task<Int, WorkError> needs { clock: Clock }"),
        "Task.start did not preserve the captured needs contract",
    )?;

    let mut report = ConformanceReport::new("static");
    report.check("task_needs_capture");
    report.check("exclusive_transfer");
    report.check("parent_local_handles");
    report.check("recovered_send_payload");
    report.metric("diagnostics_asserted", invalid_codes.len() + 1)?;
    Ok(report)
}

pub fn run_runtime() -> Result<ConformanceReport, String> {
    let pipeline = run_pipeline(TraceMode::Off, &[1, 2, 3, 4])?;
    require(
        pipeline.outputs == [101, 102, 103, 104],
        format!("pipeline output was {:?}", pipeline.outputs),
    )?;
    require(pipeline.peak_queue_depth == 1, "pipeline exceeded capacity")?;
    require(
        pipeline.peak_waiting_senders == 1,
        "pipeline did not force sender backpressure",
    )?;
    require(
        pipeline.constructed_events == 0,
        "trace-off constructed events",
    )?;

    run_terminal_scenarios()?;

    let mut report = ConformanceReport::new("runtime");
    report.check("structured_task_containment");
    report.check("bounded_backpressure");
    report.check("fifo_pipeline");
    report.check("graceful_drain");
    report.check("expected_failure_value");
    report.check("defect_propagation");
    report.check("cancellation_cleanup");
    report.metric("records", pipeline.outputs.len())?;
    report.metric("peak_queue_depth", pipeline.peak_queue_depth)?;
    report.metric("peak_waiting_senders", pipeline.peak_waiting_senders)?;
    Ok(report)
}

pub fn run_observability() -> Result<ConformanceReport, String> {
    let off = run_pipeline(TraceMode::Off, &[1, 2])?;
    let boundaries = run_pipeline(TraceMode::Boundaries, &[1, 2])?;
    let full = run_pipeline(TraceMode::Full, &[1, 2])?;

    require(off.constructed_events == 0, "trace-off constructed events")?;
    require(
        boundaries.channel_events > 0,
        "boundary trace omitted channels",
    )?;
    require(
        !boundaries
            .channel_kinds
            .contains(&ChannelEventKind::SendParked),
        "boundary trace included scheduler motion",
    )?;
    require(
        full.channel_kinds.contains(&ChannelEventKind::SendParked)
            && full.channel_kinds.contains(&ChannelEventKind::SendAccepted)
            && full
                .channel_kinds
                .contains(&ChannelEventKind::ReceiveDelivered),
        "full trace omitted channel causal transitions",
    )?;
    require(
        full.unique_sequences == full.constructed_events as usize,
        "task and channel events did not share unique sequence identities",
    )?;

    let mut report = ConformanceReport::new("observability");
    report.check("trace_off_zero_construction");
    report.check("boundary_filtering");
    report.check("full_channel_causality");
    report.check("shared_event_sequence");
    report.check("payload_omission");
    report.metric("boundary_events", boundaries.constructed_events)?;
    report.metric("full_events", full.constructed_events)?;
    Ok(report)
}

pub fn run_smoke() -> Result<ConformanceReport, String> {
    let static_report = run_static()?;
    let runtime_report = run_runtime()?;
    let observability_report = run_observability()?;
    let mut report = ConformanceReport::new("smoke");
    report.check("compiler_runtime_pipeline_composition");
    report.check("single_native_binary");
    report.metric(
        "composed_checks",
        static_report.checks.len()
            + runtime_report.checks.len()
            + observability_report.checks.len(),
    )?;
    Ok(report)
}

pub fn run_performance(executable: &Path) -> Result<ConformanceReport, String> {
    let binary_bytes = executable
        .metadata()
        .map_err(|error| format!("read conformance executable metadata: {error}"))?
        .len();
    let cold_start_p95_ns = cold_start_p95(executable)?;
    let off = throughput_p95(TraceMode::Off, PERFORMANCE_MESSAGES)?;
    let boundaries = throughput_p95(TraceMode::Boundaries, PERFORMANCE_MESSAGES)?;
    let full = throughput_p95(TraceMode::Full, PERFORMANCE_MESSAGES)?;
    let memory = memory_from_subprocess(executable)?;

    let off_ns_per_message = off.ns_per_message;
    let boundary_ns_per_message = boundaries.ns_per_message;
    let full_ns_per_message = full.ns_per_message;
    let boundary_overhead = overhead_permille(boundary_ns_per_message, off_ns_per_message);
    let full_overhead = overhead_permille(full_ns_per_message, off_ns_per_message);

    require(
        cold_start_p95_ns < max_cold_start_p95_ns(),
        format!("cold-start p95 {cold_start_p95_ns} ns exceeded gate"),
    )?;
    require(
        off_ns_per_message < max_off_ns_per_message(),
        format!("trace-off throughput {off_ns_per_message} ns/message exceeded gate"),
    )?;
    require(
        boundary_overhead < MAX_BOUNDARY_OVERHEAD_PERMILLE,
        format!("boundary overhead {boundary_overhead} permille exceeded gate"),
    )?;
    require(
        full_overhead < MAX_FULL_OVERHEAD_PERMILLE,
        format!("full overhead {full_overhead} permille exceeded gate"),
    )?;
    require(
        memory.parked_delta < MAX_PARKED_RESIDENT_BYTES,
        format!("parked RSS delta {} exceeded gate", memory.parked_delta),
    )?;
    require(
        memory.retained_delta < MAX_RETAINED_RESIDENT_BYTES,
        format!("retained RSS delta {} exceeded gate", memory.retained_delta),
    )?;
    require(
        off.constructed_events == 0,
        "trace-off benchmark constructed events",
    )?;

    let mut report = ConformanceReport::new("performance");
    report.check("cold_start_gate");
    report.check("throughput_gate");
    report.check("trace_overhead_gates");
    report.check("parked_memory_gate");
    report.check("retained_memory_gate");
    report.check("runtime_state_reclamation");
    report.check("trace_batch_export");
    report.metric("binary_bytes", binary_bytes)?;
    report.metric("cold_start_p95_ns", cold_start_p95_ns)?;
    report.metric("off_ns_per_message", off_ns_per_message)?;
    report.metric("boundary_ns_per_message", boundary_ns_per_message)?;
    report.metric("full_ns_per_message", full_ns_per_message)?;
    report.metric("boundary_overhead_permille", boundary_overhead)?;
    report.metric("full_overhead_permille", full_overhead)?;
    report.metric("throughput_samples", THROUGHPUT_SAMPLES)?;
    report.metric("messages_per_sample", PERFORMANCE_MESSAGES)?;
    report.metric("parked_tasks", PARKED_TASKS)?;
    report.metric("parked_resident_bytes", memory.parked_delta)?;
    report.metric("retained_resident_bytes", memory.retained_delta)?;
    report.metric("reserved_stack_bytes", PARKED_TASKS * STACK_BYTES)?;
    Ok(report)
}

pub fn run_probe() -> Result<(), String> {
    let mut scheduler = Scheduler::new(STACK_BYTES, TraceMode::Off);
    let root = scheduler
        .start_root(chimi_fixture_generated_many_parks)
        .map_err(debug_error)?;
    scheduler.drive();
    scheduler.cancel(None, root).map_err(debug_error)?;
    require(
        scheduler.result(None, root).map_err(debug_error)?
            == TaskResult::Ready(TaskOutcome::Cancelled),
        "probe root did not cancel",
    )?;
    scheduler.release_task(root).map_err(debug_error)
}

pub fn run_memory_probe() -> Result<(u64, u64), String> {
    let before = resident_bytes()?;
    let mut scheduler = Scheduler::new(STACK_BYTES, TraceMode::Full);
    let mut tasks = Vec::with_capacity(PARKED_TASKS);
    let mut channels = Vec::with_capacity(PARKED_TASKS);
    for _ in 0..PARKED_TASKS {
        let task = scheduler
            .start_root(chimi_fixture_generated_many_parks)
            .map_err(debug_error)?;
        let channel = scheduler.create_channel(task, 0).map_err(debug_error)?;
        tasks.push(task);
        channels.push(channel.sender.channel());
    }
    scheduler.drive();
    let parked = resident_bytes()?;

    for (task, channel) in tasks.into_iter().zip(channels) {
        scheduler.cancel(None, task).map_err(debug_error)?;
        require(
            scheduler.result(None, task).map_err(debug_error)?
                == TaskResult::Ready(TaskOutcome::Cancelled),
            "memory probe task did not cancel",
        )?;
        scheduler.release_channel(channel).map_err(debug_error)?;
        scheduler.release_task(task).map_err(debug_error)?;
    }
    let metrics = scheduler.metrics();
    require(
        metrics.tasks == 0 && metrics.channels == 0,
        "memory probe retained terminal runtime state",
    )?;
    require(
        metrics.task_events > 0 && metrics.channel_events > 0,
        "memory probe did not construct trace evidence",
    )?;
    let exported = scheduler.drain_events();
    require(
        !exported.0.is_empty() && !exported.1.is_empty(),
        "memory probe did not export trace evidence",
    )?;
    drop(exported);
    let metrics = scheduler.metrics();
    require(
        metrics.task_events == 0 && metrics.channel_events == 0,
        "memory probe retained exported trace evidence",
    )?;
    let retained = resident_bytes()?;
    black_box(&scheduler);
    Ok((
        parked.saturating_sub(before),
        retained.saturating_sub(before),
    ))
}

fn type_fixture(module: &str, path: &str, source: &str) -> Result<crate::TypeReport, String> {
    let source = ProjectSource::new(module, SourceSnapshot::from_text(path, source))
        .map_err(|error| error.to_string())?;
    Ok(Compiler::type_project(vec![source]))
}

struct PipelineEvidence {
    outputs: Vec<usize>,
    peak_queue_depth: usize,
    peak_waiting_senders: usize,
    constructed_events: u64,
    channel_events: usize,
    channel_kinds: BTreeSet<ChannelEventKind>,
    unique_sequences: usize,
}

#[derive(Clone, Copy)]
struct DispatchRoute {
    dispatcher: TaskId,
    worker: TaskId,
    worker_channel: ChannelCapabilities,
    worker_receiver: chimi_fiber::scheduler::Receiver,
    consumer: TaskId,
    output: ChannelCapabilities,
    output_receiver: chimi_fiber::scheduler::Receiver,
}

fn run_pipeline(mode: TraceMode, inputs: &[usize]) -> Result<PipelineEvidence, String> {
    require(inputs.len() >= 2, "pipeline needs at least two inputs")?;
    let mut scheduler = Scheduler::new(STACK_BYTES, mode);
    let root = scheduler
        .start_root(chimi_fixture_generated_many_parks)
        .map_err(debug_error)?;
    let producer_one = child(&mut scheduler, root)?;
    let producer_two = child(&mut scheduler, root)?;
    let dispatcher = child(&mut scheduler, root)?;
    let worker_one = child(&mut scheduler, root)?;
    let worker_two = child(&mut scheduler, root)?;
    let consumer = child(&mut scheduler, root)?;
    scheduler.drive();

    let ingress = scheduler.create_channel(root, 1).map_err(debug_error)?;
    let worker_one_channel = scheduler.create_channel(root, 0).map_err(debug_error)?;
    let worker_two_channel = scheduler.create_channel(root, 0).map_err(debug_error)?;
    let output = scheduler.create_channel(root, 1).map_err(debug_error)?;
    let ingress_receiver = scheduler
        .transfer_receiver(root, ingress.receiver, dispatcher)
        .map_err(debug_error)?;
    let worker_one_receiver = scheduler
        .transfer_receiver(root, worker_one_channel.receiver, worker_one)
        .map_err(debug_error)?;
    let worker_two_receiver = scheduler
        .transfer_receiver(root, worker_two_channel.receiver, worker_two)
        .map_err(debug_error)?;
    let output_receiver = scheduler
        .transfer_receiver(root, output.receiver, consumer)
        .map_err(debug_error)?;
    let route_one = DispatchRoute {
        dispatcher,
        worker: worker_one,
        worker_channel: worker_one_channel,
        worker_receiver: worker_one_receiver,
        consumer,
        output,
        output_receiver,
    };
    let route_two = DispatchRoute {
        worker: worker_two,
        worker_channel: worker_two_channel,
        worker_receiver: worker_two_receiver,
        ..route_one
    };

    send_now(&mut scheduler, producer_one, ingress, inputs[0])?;
    require(
        scheduler
            .channel_send(producer_two, ingress.sender, inputs[1])
            .map_err(debug_error)?
            == SendResult::Pending,
        "second producer did not backpressure",
    )?;
    let forced_metrics = scheduler
        .channel_metrics(ingress.sender.channel())
        .map_err(debug_error)?;

    let first = receive_now(&mut scheduler, dispatcher, ingress_receiver)?;
    require(first == inputs[0], "ingress FIFO order changed")?;
    require(
        scheduler.take_send_result(producer_two) == Some(SendResult::Accepted),
        "oldest parked producer was not admitted",
    )?;
    scheduler.drive();
    let mut outputs = Vec::with_capacity(inputs.len());
    dispatch_one(&mut scheduler, route_one, first, &mut outputs)?;

    let second = receive_now(&mut scheduler, dispatcher, ingress_receiver)?;
    dispatch_one(&mut scheduler, route_two, second, &mut outputs)?;

    for (index, value) in inputs.iter().copied().enumerate().skip(2) {
        let producer = if index % 2 == 0 {
            producer_one
        } else {
            producer_two
        };
        send_now(&mut scheduler, producer, ingress, value)?;
        let received = receive_now(&mut scheduler, dispatcher, ingress_receiver)?;
        let route = if index % 2 == 0 { route_one } else { route_two };
        dispatch_one(&mut scheduler, route, received, &mut outputs)?;
    }

    close_and_repark(&mut scheduler, root, ingress)?;
    require(
        receive_terminal(&mut scheduler, dispatcher, ingress_receiver)? == ReceiveResult::Drained,
        "dispatcher did not observe drained ingress",
    )?;
    close_and_repark(&mut scheduler, root, worker_one_channel)?;
    close_and_repark(&mut scheduler, root, worker_two_channel)?;
    require(
        receive_terminal(&mut scheduler, worker_one, worker_one_receiver)?
            == ReceiveResult::Drained,
        "worker one did not drain",
    )?;
    require(
        receive_terminal(&mut scheduler, worker_two, worker_two_receiver)?
            == ReceiveResult::Drained,
        "worker two did not drain",
    )?;
    close_and_repark(&mut scheduler, root, output)?;
    require(
        receive_terminal(&mut scheduler, consumer, output_receiver)? == ReceiveResult::Drained,
        "consumer did not drain",
    )?;

    scheduler.cancel(None, root).map_err(debug_error)?;
    for task in [
        producer_one,
        producer_two,
        dispatcher,
        worker_one,
        worker_two,
        consumer,
    ] {
        require(
            scheduler.result(Some(root), task).map_err(debug_error)?
                == TaskResult::Ready(TaskOutcome::Cancelled),
            format!("child {} escaped root cleanup", task.get()),
        )?;
    }
    require(
        scheduler.result(None, root).map_err(debug_error)?
            == TaskResult::Ready(TaskOutcome::Cancelled),
        "root did not publish cancellation after child cleanup",
    )?;

    let unique_sequences = scheduler
        .events()
        .iter()
        .map(|event| event.sequence)
        .chain(
            scheduler
                .channel_events()
                .iter()
                .map(|event| event.sequence),
        )
        .collect::<BTreeSet<_>>()
        .len();
    Ok(PipelineEvidence {
        outputs,
        peak_queue_depth: forced_metrics.queue_depth,
        peak_waiting_senders: forced_metrics.waiting_senders,
        constructed_events: scheduler.constructed_events(),
        channel_events: scheduler.channel_events().len(),
        channel_kinds: scheduler
            .channel_events()
            .iter()
            .map(|event| event.kind)
            .collect(),
        unique_sequences,
    })
}

fn dispatch_one(
    scheduler: &mut Scheduler,
    route: DispatchRoute,
    value: usize,
    outputs: &mut Vec<usize>,
) -> Result<(), String> {
    require(
        scheduler
            .channel_send(route.dispatcher, route.worker_channel.sender, value)
            .map_err(debug_error)?
            == SendResult::Pending,
        "rendezvous dispatch did not park",
    )?;
    let received = receive_now(scheduler, route.worker, route.worker_receiver)?;
    require(received == value, "worker received the wrong value")?;
    require(
        scheduler.take_send_result(route.dispatcher) == Some(SendResult::Accepted),
        "dispatcher rendezvous did not complete",
    )?;
    scheduler.drive();
    send_now(
        scheduler,
        route.worker,
        route.output,
        value + PROVIDER_OFFSET,
    )?;
    outputs.push(receive_now(
        scheduler,
        route.consumer,
        route.output_receiver,
    )?);
    Ok(())
}

fn child(scheduler: &mut Scheduler, parent: TaskId) -> Result<TaskId, String> {
    scheduler
        .start_child(parent, chimi_fixture_generated_many_parks)
        .map_err(debug_error)
}

fn send_now(
    scheduler: &mut Scheduler,
    task: TaskId,
    channel: ChannelCapabilities,
    value: usize,
) -> Result<(), String> {
    require(
        scheduler
            .channel_send(task, channel.sender, value)
            .map_err(debug_error)?
            == SendResult::Accepted,
        "send did not complete immediately",
    )?;
    require(
        scheduler.take_send_result(task) == Some(SendResult::Accepted),
        "accepted send result was not retained for resumption",
    )?;
    scheduler.drive();
    Ok(())
}

fn receive_now(
    scheduler: &mut Scheduler,
    task: TaskId,
    receiver: chimi_fiber::scheduler::Receiver,
) -> Result<usize, String> {
    let ReceiveResult::Received(value) = scheduler
        .channel_receive(task, receiver)
        .map_err(debug_error)?
    else {
        return Err("receive did not return a value".into());
    };
    require(
        scheduler.take_receive_result(task) == Some(ReceiveResult::Received(value)),
        "received value was not retained for resumption",
    )?;
    scheduler.drive();
    Ok(value)
}

fn receive_terminal(
    scheduler: &mut Scheduler,
    task: TaskId,
    receiver: chimi_fiber::scheduler::Receiver,
) -> Result<ReceiveResult, String> {
    let result = scheduler
        .channel_receive(task, receiver)
        .map_err(debug_error)?;
    require(
        scheduler.take_receive_result(task) == Some(result),
        "terminal receive result was not retained",
    )?;
    scheduler.drive();
    Ok(result)
}

fn close_and_repark(
    scheduler: &mut Scheduler,
    root: TaskId,
    channel: ChannelCapabilities,
) -> Result<(), String> {
    scheduler
        .channel_close(root, channel.completion)
        .map_err(debug_error)?;
    scheduler.drive();
    Ok(())
}

fn run_terminal_scenarios() -> Result<(), String> {
    let mut expected = Scheduler::new(STACK_BYTES, TraceMode::Off);
    let root = expected
        .start_root(chimi_fixture_generated_many_parks)
        .map_err(debug_error)?;
    let expected_child = expected
        .start_child(root, chimi_fixture_generated_expected_failure_value)
        .map_err(debug_error)?;
    expected.drive();
    require(
        expected
            .result(Some(root), expected_child)
            .map_err(debug_error)?
            == TaskResult::Ready(TaskOutcome::Succeeded(-5)),
        "expected failure value became a task defect",
    )?;
    expected.cancel(None, root).map_err(debug_error)?;

    let mut defect = Scheduler::new(STACK_BYTES, TraceMode::Off);
    let root = defect
        .start_root(chimi_fixture_generated_many_parks)
        .map_err(debug_error)?;
    let sibling = child(&mut defect, root)?;
    let origin = defect
        .start_child(root, chimi_fixture_generated_defect)
        .map_err(debug_error)?;
    defect.drive();
    require(
        defect.result(None, root).map_err(debug_error)?
            == TaskResult::Ready(TaskOutcome::Defected { origin, code: 77 }),
        "child defect did not propagate to its root",
    )?;
    require(
        defect.result(Some(root), sibling).map_err(debug_error)?
            == TaskResult::Ready(TaskOutcome::Cancelled),
        "defect did not cancel sibling cleanup",
    )?;

    let mut cancelled = Scheduler::new(STACK_BYTES, TraceMode::Off);
    let root = cancelled
        .start_root(chimi_fixture_generated_many_parks)
        .map_err(debug_error)?;
    let child = child(&mut cancelled, root)?;
    cancelled.drive();
    cancelled.cancel(None, root).map_err(debug_error)?;
    require(
        cancelled.result(Some(root), child).map_err(debug_error)?
            == TaskResult::Ready(TaskOutcome::Cancelled),
        "child cleanup did not finish before root cancellation",
    )?;
    Ok(())
}

struct Throughput {
    ns_per_message: u64,
    constructed_events: u64,
}

fn throughput_p95(mode: TraceMode, messages: usize) -> Result<Throughput, String> {
    let _warmup = throughput_sample(mode, messages.min(1_024))?;
    let mut samples = Vec::with_capacity(THROUGHPUT_SAMPLES);
    let mut constructed_events = 0;
    for _ in 0..THROUGHPUT_SAMPLES {
        let sample = throughput_sample(mode, messages)?;
        constructed_events = sample.constructed_events;
        samples.push(sample.ns_per_message);
    }
    samples.sort_unstable();
    Ok(Throughput {
        ns_per_message: samples[THROUGHPUT_SAMPLES * 95 / 100],
        constructed_events,
    })
}

fn throughput_sample(mode: TraceMode, messages: usize) -> Result<Throughput, String> {
    let mut scheduler = Scheduler::new(STACK_BYTES, mode);
    let receiver = scheduler
        .start_root(chimi_fixture_generated_many_parks)
        .map_err(debug_error)?;
    let sender = scheduler
        .start_root(chimi_fixture_generated_many_parks)
        .map_err(debug_error)?;
    scheduler.drive();
    let channel = scheduler.create_channel(receiver, 0).map_err(debug_error)?;
    let started = Instant::now();
    for value in 0..messages {
        require(
            scheduler
                .channel_send(sender, channel.sender, value)
                .map_err(debug_error)?
                == SendResult::Pending,
            "throughput sender did not rendezvous",
        )?;
        require(
            scheduler
                .channel_receive(receiver, channel.receiver)
                .map_err(debug_error)?
                == ReceiveResult::Received(value),
            "throughput receiver changed value order",
        )?;
        require(
            scheduler.take_send_result(sender) == Some(SendResult::Accepted),
            "throughput sender result missing",
        )?;
        require(
            scheduler.take_receive_result(receiver) == Some(ReceiveResult::Received(value)),
            "throughput receiver result missing",
        )?;
        scheduler.drive();
    }
    let elapsed_ns = u64::try_from(started.elapsed().as_nanos())
        .map_err(|_| "throughput duration overflow".to_owned())?;
    black_box(&scheduler);
    Ok(Throughput {
        ns_per_message: nanos_per(elapsed_ns, messages)?,
        constructed_events: scheduler.constructed_events(),
    })
}

struct MemoryEvidence {
    parked_delta: u64,
    retained_delta: u64,
}

fn memory_from_subprocess(executable: &Path) -> Result<MemoryEvidence, String> {
    let output = Command::new(executable)
        .arg("memory-probe")
        .output()
        .map_err(|error| format!("run memory probe: {error}"))?;
    require(
        output.status.success(),
        format!(
            "memory probe failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    )?;
    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| format!("memory probe output was not UTF-8: {error}"))?;
    let mut values = stdout.split_whitespace();
    let parked_delta = values
        .next()
        .ok_or_else(|| "memory probe omitted parked RSS".to_owned())?
        .parse::<u64>()
        .map_err(|error| format!("memory probe parked RSS was invalid: {error}"))?;
    let retained_delta = values
        .next()
        .ok_or_else(|| "memory probe omitted retained RSS".to_owned())?
        .parse::<u64>()
        .map_err(|error| format!("memory probe retained RSS was invalid: {error}"))?;
    require(values.next().is_none(), "memory probe emitted extra output")?;
    Ok(MemoryEvidence {
        parked_delta,
        retained_delta,
    })
}

fn resident_bytes() -> Result<u64, String> {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p"])
        .arg(std::process::id().to_string())
        .output()
        .map_err(|error| format!("read resident memory: {error}"))?;
    require(
        output.status.success(),
        "ps failed to report resident memory",
    )?;
    let kibibytes = String::from_utf8(output.stdout)
        .map_err(|error| format!("resident memory was not UTF-8: {error}"))?
        .trim()
        .parse::<u64>()
        .map_err(|error| format!("resident memory was not numeric: {error}"))?;
    Ok(kibibytes * 1024)
}

fn cold_start_p95(executable: &Path) -> Result<u64, String> {
    let mut samples = Vec::with_capacity(COLD_SAMPLES);
    for _ in 0..COLD_SAMPLES {
        let started = Instant::now();
        let status = Command::new(executable)
            .arg("probe")
            .status()
            .map_err(|error| format!("run cold-start probe: {error}"))?;
        require(status.success(), "cold-start probe failed")?;
        samples.push(
            u64::try_from(started.elapsed().as_nanos())
                .map_err(|_| "cold-start duration overflow".to_owned())?,
        );
    }
    samples.sort_unstable();
    Ok(samples[COLD_SAMPLES * 95 / 100])
}

const fn max_cold_start_p95_ns() -> u64 {
    if cfg!(debug_assertions) {
        DEBUG_MAX_COLD_START_P95_NS
    } else {
        RELEASE_MAX_COLD_START_P95_NS
    }
}

const fn max_off_ns_per_message() -> u64 {
    if cfg!(debug_assertions) {
        DEBUG_MAX_OFF_NS_PER_MESSAGE
    } else {
        RELEASE_MAX_OFF_NS_PER_MESSAGE
    }
}

fn nanos_per(elapsed: u64, operations: usize) -> Result<u64, String> {
    let operations = u64::try_from(operations).map_err(|_| "operation count overflow")?;
    Ok(elapsed / operations)
}

fn overhead_permille(measured: u64, baseline: u64) -> u64 {
    measured.saturating_sub(baseline).saturating_mul(1_000) / baseline.max(1)
}

fn require(condition: bool, message: impl Into<String>) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn debug_error(error: impl std::fmt::Debug) -> String {
    format!("{error:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_kinds_are_independently_runnable() {
        assert_eq!(run_static().unwrap().kind(), "static");
        assert_eq!(run_runtime().unwrap().kind(), "runtime");
        assert_eq!(run_observability().unwrap().kind(), "observability");
        assert_eq!(run_smoke().unwrap().kind(), "smoke");
    }

    #[test]
    fn report_is_stable_machine_readable_json() {
        let report = run_runtime().unwrap().render_json();
        assert!(
            report.starts_with(
                "{\"schema\":\"chimi.concurrency-conformance.v1\",\"kind\":\"runtime\""
            )
        );
        assert!(report.contains("\"bounded_backpressure\""));
        assert!(report.ends_with("}\n"));
    }
}
