//! Performance gates PG01-PG10 (lab specification, "Performance gates").
//!
//! Each gate builds its workload with the compiler this lab was built with,
//! so a debug lab links the debug scheduler archive and uses 10 times every
//! release time limit. Memory, ratio, and count limits are the same in both.
//!
//! - Times are differential: a workload runs at sizes N1 and N2, each timing
//!   is the p95 of `SAMPLES` runs, and the gate value is
//!   `(t(N2) - t(N1)) / (N2 - N1)`, so process start and runner setup cancel
//!   out. Each executable runs once unmeasured first, because the first exec
//!   of a freshly linked file pays a one-time system check.
//! - Resident memory is the child's maximum resident size from `wait4`.
//! - Workloads run under `--clock=virtual`, except PG04 and PG05: they read
//!   phase times from `--trace=boundaries` telemetry, whose `time_ns` is real
//!   only under `--clock=real`. That workload never sleeps, so the clock mode
//!   changes no behaviour.
//! - Every measured run must exit 0 with its workload's expected outcome, so
//!   a broken run cannot pass a gate by being fast.
//!
//! Workloads live in `tests/process-lab/performance/app/`; PG01, PG02, PG07,
//! and PG10 reuse MF01's program, which is PG01's workload exactly, and PG09
//! times the FP01 executable. Gates sharing a workload measure it once per
//! lab process.

use super::catalogue::{self, Scenario};
use super::fixture::{self, Scratch};
use super::{GateResult, Measurement};
use crate::json::{JsonData, decode_json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Timed runs per size; the timing is their p95.
const SAMPLES: usize = 7;
/// The longest one measured run may take before it is killed.
const RUN_LIMIT: Duration = Duration::from_secs(120);

/// A release time limit scaled for the profile this lab was built in.
fn time_limit(release: f64) -> f64 {
    if cfg!(debug_assertions) {
        release * 10.0
    } else {
        release
    }
}

// ---------------------------------------------------------------------------
// Gates, one function per catalogue entry.

pub(super) fn pg01() -> Result<Measurement, String> {
    let evidence = messages()?;
    Ok(Measurement::gates(vec![GateResult::below(
        "pg01_ns_per_message",
        evidence.ns_per_message,
        time_limit(2_000.0),
    )]))
}

pub(super) fn pg02() -> Result<Measurement, String> {
    let evidence = messages()?;
    Ok(Measurement {
        // Full tracing is a debugging mode with no overhead gate (ts-8f201b):
        // it records every Core operation, so its ratio is reported only.
        gates: vec![GateResult::below(
            "pg02_boundaries_over_off",
            evidence.boundaries_ratio,
            1.25,
        )],
        metrics: vec![
            ("pg02_full_over_off".into(), evidence.full_ratio),
            ("pg02_off_ns_per_message".into(), evidence.trace_ns[0]),
            (
                "pg02_boundaries_ns_per_message".into(),
                evidence.trace_ns[1],
            ),
            ("pg02_full_ns_per_message".into(), evidence.trace_ns[2]),
        ],
    })
}

pub(super) fn pg03() -> Result<Measurement, String> {
    let evidence = restarts()?;
    Ok(Measurement::gates(vec![GateResult::below(
        "pg03_us_per_restart",
        evidence,
        time_limit(40.0),
    )]))
}

pub(super) fn pg04() -> Result<Measurement, String> {
    let evidence = members()?;
    Ok(Measurement::gates(vec![
        GateResult::below(
            "pg04_us_per_start",
            evidence.at_1000.start_us,
            time_limit(50.0),
        ),
        GateResult::below(
            "pg04_us_per_stop",
            evidence.at_1000.stop_us,
            time_limit(30.0),
        ),
    ]))
}

pub(super) fn pg05() -> Result<Measurement, String> {
    let evidence = members()?;
    let per_process = |phases: &Phases| phases.start_us + phases.stop_us;
    Ok(Measurement {
        gates: vec![GateResult::below(
            "pg05_cost_at_1000_over_100",
            per_process(&evidence.at_1000) / per_process(&evidence.at_100),
            2.0,
        )],
        metrics: vec![
            ("pg05_us_per_start_at_100".into(), evidence.at_100.start_us),
            ("pg05_us_per_stop_at_100".into(), evidence.at_100.stop_us),
        ],
    })
}

pub(super) fn pg06() -> Result<Measurement, String> {
    let evidence = payloads()?;
    Ok(Measurement::gates(vec![
        GateResult::below("pg06_ns_per_text_4k", evidence[0], time_limit(2_000.0)),
        GateResult::below("pg06_ns_per_list_1000", evidence[1], time_limit(70_000.0)),
        GateResult::below("pg06_ns_per_dag_21", evidence[2], time_limit(5_000.0)),
    ]))
}

pub(super) fn pg07() -> Result<Measurement, String> {
    let evidence = messages()?;
    Ok(Measurement {
        gates: vec![GateResult::below(
            "pg07_resident_growth_bytes",
            evidence.resident_growth,
            1_048_576.0,
        )],
        metrics: vec![("pg07_resident_bytes_at_n1".into(), evidence.resident_at_n1)],
    })
}

pub(super) fn pg08() -> Result<Measurement, String> {
    let evidence = members()?;
    Ok(Measurement::gates(vec![GateResult::below(
        "pg08_parked_resident_bytes",
        evidence.parked_resident,
        64.0 * 1_048_576.0,
    )]))
}

pub(super) fn pg09() -> Result<Measurement, String> {
    let evidence = cold_start()?;
    Ok(Measurement {
        gates: vec![
            GateResult::below("pg09_cold_start_p95_ms", evidence.p95_ms, time_limit(25.0)),
            // OB06's bound, which the decision ledger assigns to this run:
            // 2m35s of virtual time in under 2 s of wall time.
            GateResult::below(
                "ob06_virtual_run_wall_ms",
                evidence.virtual_run_ms,
                time_limit(2_000.0),
            ),
        ],
        metrics: vec![
            ("fp01_executable_bytes".into(), evidence.forcing_bytes),
            (
                "process_free_executable_bytes".into(),
                evidence.process_free_bytes,
            ),
        ],
    })
}

pub(super) fn pg10() -> Result<Measurement, String> {
    Ok(Measurement::gates(vec![GateResult::exactly(
        "pg10_trace_off_events",
        trace_off_events()?,
        0.0,
    )]))
}

// ---------------------------------------------------------------------------
// Workloads.

struct MessageEvidence {
    ns_per_message: f64,
    /// Trace-off, boundaries, and full ns per message at the PG02 sizes.
    trace_ns: [f64; 3],
    boundaries_ratio: f64,
    full_ratio: f64,
    resident_at_n1: f64,
    resident_growth: f64,
}

/// PG01, PG02, and PG07 on MF01's program. MF01 rounds N down to whole
/// batches of 64, so the outcome (and the message count) is that rounded N.
fn messages() -> Result<&'static MessageEvidence, String> {
    static EVIDENCE: OnceLock<Result<MessageEvidence, String>> = OnceLock::new();
    cached(&EVIDENCE, || {
        let mf01 = catalogue::find("MF01").ok_or("the catalogue has no MF01")?;
        let workload = Workload::build("PG-messages", &fixture::root(mf01), "app/main")?;
        let sizes = |n: u64| (n.to_string(), n / 64 * 64);
        let (n1, n2) = (sizes(10_000), sizes(1_000_000));
        let off = Run::new(Trace::Off);
        let ns_per_message = workload.differential(&off, &n1, &n2)?;
        // One run per size for memory.
        let at_n1 = workload.run(&off, &n1.0, n1.1)?;
        let at_n2 = workload.run(&off, &n2.0, n2.1)?;
        // Trace overhead is measured at smaller sizes, because full mode
        // records every Core operation: about 15 KB of telemetry per message,
        // so 1,000,000 messages would write 15 GB per run. Off and boundaries
        // run at 10,000 and 100,000; full at 640 and 6,400 (about 100 MB per
        // run). A differential cost does not depend on the sizes, so each
        // ratio divides by the trace-off cost at 10,000 and 100,000.
        let trace_sizes = [
            (sizes(10_000), sizes(100_000)),
            (sizes(10_000), sizes(100_000)),
            (sizes(640), sizes(6_400)),
        ];
        let mut trace_ns = [0.0; 3];
        for (slot, (trace, (small, large))) in [Trace::Off, Trace::Boundaries, Trace::Full]
            .into_iter()
            .zip(&trace_sizes)
            .enumerate()
        {
            let run = Run::new(trace).with_telemetry();
            trace_ns[slot] = workload.differential(&run, small, large)?;
        }
        Ok(MessageEvidence {
            ns_per_message,
            trace_ns,
            boundaries_ratio: ratio(trace_ns[1], trace_ns[0]),
            full_ratio: ratio(trace_ns[2], trace_ns[0]),
            resident_at_n1: at_n1.max_resident as f64,
            resident_growth: at_n2.max_resident.saturating_sub(at_n1.max_resident) as f64,
        })
    })
    .map_err(|error| format!("PG01 workload: {error}"))
}

/// PG10: the events the runner constructed in one `--trace=off` run of
/// PG01's workload at N2, plus the telemetry lines it wrote; both must be 0.
/// Its own build, so a runtime that traces under `off` fails this gate and
/// not only the timing runs.
fn trace_off_events() -> Result<f64, String> {
    static EVIDENCE: OnceLock<Result<f64, String>> = OnceLock::new();
    cached(&EVIDENCE, || {
        let mf01 = catalogue::find("MF01").ok_or("the catalogue has no MF01")?;
        let workload = Workload::build("PG-trace-off", &fixture::root(mf01), "app/main")?;
        let run = workload.run(&Run::new(Trace::Off).with_reports(), "1000000", 1_000_000)?;
        let constructed = report_field(&run.memory_report, "trace_events_constructed")?;
        Ok(constructed + run.telemetry.lines().count() as f64)
    })
    .copied()
    .map_err(|error| format!("PG10 workload: {error}"))
}

/// PG03: µs per restart cycle.
fn restarts() -> Result<f64, String> {
    static EVIDENCE: OnceLock<Result<f64, String>> = OnceLock::new();
    cached(&EVIDENCE, || {
        let workload = Workload::build("PG-restarts", &workloads_root(), "app/restarts")?;
        let ns = workload.differential(
            &Run::new(Trace::Off),
            &("1000".into(), 1_000),
            &("10000".into(), 10_000),
        )?;
        Ok(ns / 1_000.0)
    })
    .copied()
    .map_err(|error| format!("PG03 workload: {error}"))
}

/// Per-process start and stop costs at one size.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Phases {
    start_us: f64,
    stop_us: f64,
}

struct MemberEvidence {
    at_100: Phases,
    at_1000: Phases,
    parked_resident: f64,
}

/// PG04, PG05, PG08 on the `members` workload.
fn members() -> Result<&'static MemberEvidence, String> {
    static EVIDENCE: OnceLock<Result<MemberEvidence, String>> = OnceLock::new();
    cached(&EVIDENCE, || {
        let workload = Workload::build("PG-members", &workloads_root(), "app/members")?;
        let timed = Run::new(Trace::Boundaries).with_telemetry().real_clock();
        let phases_at = |count: u64| -> Result<Phases, String> {
            let argument = count.to_string();
            workload.run(&timed, &argument, count)?;
            let mut starts = Vec::with_capacity(SAMPLES);
            let mut stops = Vec::with_capacity(SAMPLES);
            for _ in 0..SAMPLES {
                let run = workload.run(&timed, &argument, count)?;
                let (start, stop) = phase_nanoseconds(&run.telemetry)?;
                starts.push(start);
                stops.push(stop);
            }
            Ok(Phases {
                start_us: p95(&mut starts) as f64 / count as f64 / 1_000.0,
                stop_us: p95(&mut stops) as f64 / count as f64 / 1_000.0,
            })
        };
        let at_100 = phases_at(100)?;
        let at_1000 = phases_at(1_000)?;
        let off = Run::new(Trace::Off);
        let empty = workload.run(&off, "0", 0)?;
        let parked = workload.run(&off, "1000", 1_000)?;
        Ok(MemberEvidence {
            at_100,
            at_1000,
            parked_resident: parked.max_resident.saturating_sub(empty.max_resident) as f64,
        })
    })
    .map_err(|error| format!("PG04 workload: {error}"))
}

/// PG06: ns per message for the Text, List, and DAG payloads.
fn payloads() -> Result<[f64; 3], String> {
    static EVIDENCE: OnceLock<Result<[f64; 3], String>> = OnceLock::new();
    cached(&EVIDENCE, || {
        let mut values = [0.0; 3];
        // The List payload costs about 70 times the others per message, so it
        // runs fewer; every size is whole batches of 64.
        for (slot, (entry, n2)) in [
            ("app/text_payload", 64_000),
            ("app/list_payload", 12_800),
            ("app/dag_payload", 64_000),
        ]
        .into_iter()
        .enumerate()
        {
            let name = format!("PG-{}", &entry[4..]);
            let workload = Workload::build(&name, &workloads_root(), entry)?;
            values[slot] = workload.differential(
                &Run::new(Trace::Off),
                &("640".into(), 640),
                &(n2.to_string(), n2),
            )?;
        }
        Ok(values)
    })
    .copied()
    .map_err(|error| format!("PG06 workload: {error}"))
}

struct ColdStart {
    p95_ms: f64,
    virtual_run_ms: f64,
    forcing_bytes: f64,
    process_free_bytes: f64,
}

/// PG09 on the FP01 executable, OB06's wall time, and the executable sizes
/// recorded without a gate.
fn cold_start() -> Result<&'static ColdStart, String> {
    static EVIDENCE: OnceLock<Result<ColdStart, String>> = OnceLock::new();
    cached(&EVIDENCE, || {
        let fp01 = catalogue::find("FP01").ok_or("the catalogue has no FP01")?;
        let workload = Workload::build("PG-cold-start", &fixture::root(fp01), "app/main")?;
        let off = Run::new(Trace::Off);
        workload.run_with(&off, None, fp01)?;
        let mut samples = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            samples.push(workload.run_with(&off, None, fp01)?.elapsed_ns);
        }
        let ob06 = catalogue::find("OB06").ok_or("the catalogue has no OB06")?;
        let virtual_run = Workload::build("PG-OB06", &fixture::root(ob06), "app/main")?;
        virtual_run.run_with(&off, None, ob06)?;
        let virtual_run_ns = virtual_run.run_with(&off, None, ob06)?.elapsed_ns;
        let process_free = Scratch::named("PG-process-free")?;
        let project = process_free.path().join("project");
        std::fs::create_dir_all(project.join("app"))
            .map_err(|error| format!("create {}: {error}", project.display()))?;
        std::fs::write(
            project.join("app/main.chimi"),
            "pub fn main() -> Int { 1 }\n",
        )
        .map_err(|error| format!("write the process-free program: {error}"))?;
        let plain = fixture::build_entry(&project, "app/main", process_free.path())?;
        Ok(ColdStart {
            p95_ms: p95(&mut samples) as f64 / 1_000_000.0,
            virtual_run_ms: virtual_run_ns as f64 / 1_000_000.0,
            forcing_bytes: file_size(&workload.executable)?,
            process_free_bytes: file_size(&plain)?,
        })
    })
    .map_err(|error| format!("PG09 workload: {error}"))
}

fn workloads_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/process-lab/performance")
}

/// Computes a workload's evidence once per lab process. A failure is cached
/// too, so every gate of the workload reports it.
fn cached<T>(
    cell: &'static OnceLock<Result<T, String>>,
    measure: impl FnOnce() -> Result<T, String>,
) -> Result<&'static T, String> {
    cell.get_or_init(measure).as_ref().map_err(Clone::clone)
}

fn file_size(path: &Path) -> Result<f64, String> {
    std::fs::metadata(path)
        .map(|metadata| metadata.len() as f64)
        .map_err(|error| format!("read {}: {error}", path.display()))
}

// ---------------------------------------------------------------------------
// Running a workload.

#[derive(Clone, Copy)]
enum Trace {
    Off,
    Boundaries,
    Full,
}

/// How one measured run is invoked.
#[derive(Clone, Copy)]
struct Run {
    trace: Trace,
    /// Write the telemetry file (always under boundaries and full).
    telemetry: bool,
    memory_report: bool,
    real_clock: bool,
}

impl Run {
    fn new(trace: Trace) -> Self {
        Self {
            trace,
            telemetry: !matches!(trace, Trace::Off),
            memory_report: false,
            real_clock: false,
        }
    }

    fn with_telemetry(mut self) -> Self {
        self.telemetry = true;
        self
    }

    fn with_reports(mut self) -> Self {
        self.telemetry = true;
        self.memory_report = true;
        self
    }

    fn real_clock(mut self) -> Self {
        self.real_clock = true;
        self
    }
}

struct Finished {
    elapsed_ns: u64,
    max_resident: u64,
    memory_report: String,
    telemetry: String,
}

/// A built workload executable in its own scratch directory.
struct Workload {
    scratch: Scratch,
    executable: PathBuf,
}

impl Workload {
    fn build(name: &str, root: &Path, entry: &str) -> Result<Self, String> {
        let scratch = Scratch::named(name)?;
        let executable = fixture::build_entry(root, entry, scratch.path())?;
        Ok(Self {
            scratch,
            executable,
        })
    }

    /// ns per unit between sizes `n1` and `n2`, each `(argument, count)`
    /// where `count` is also the expected outcome: the p95 of `SAMPLES`
    /// timed runs at each size, after one unmeasured run.
    fn differential(
        &self,
        run: &Run,
        n1: &(String, u64),
        n2: &(String, u64),
    ) -> Result<f64, String> {
        self.run(run, &n1.0, n1.1)?;
        let mut small = Vec::with_capacity(SAMPLES);
        let mut large = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            small.push(self.run(run, &n1.0, n1.1)?.elapsed_ns);
            large.push(self.run(run, &n2.0, n2.1)?.elapsed_ns);
        }
        Ok(differential(p95(&mut small), p95(&mut large), n1.1, n2.1))
    }

    /// One run whose outcome must be the success value `expected`.
    fn run(&self, run: &Run, argument: &str, expected: u64) -> Result<Finished, String> {
        let finished = self.invoke(run, Some(argument))?;
        let outcome = format!(
            "{{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":{expected}}}"
        );
        if finished.1.trim_end() != outcome {
            return Err(format!(
                "run `{argument}` ended with `{}`, expected `{outcome}`",
                finished.1.trim_end()
            ));
        }
        Ok(finished.0)
    }

    /// One run whose outcome must match the scenario's `expected.outcome`.
    fn run_with(
        &self,
        run: &Run,
        argument: Option<&str>,
        scenario: &Scenario,
    ) -> Result<Finished, String> {
        let expected = fixture::expected_lines(scenario, "expected.outcome")?;
        let (finished, outcome) = self.invoke(run, argument)?;
        let actual: Vec<String> = outcome.lines().map(str::to_owned).collect();
        fixture::compare_outcomes(&expected, &actual)?;
        Ok(finished)
    }

    /// Runs the executable once; it must exit 0. Returns what it wrote and
    /// its outcome line.
    fn invoke(&self, run: &Run, argument: Option<&str>) -> Result<(Finished, String), String> {
        let directory = self.scratch.path();
        let outcome_path = directory.join("outcome.jsonl");
        let memory_path = directory.join("memory.json");
        let telemetry_path = directory.join("telemetry.jsonl");
        for path in [&outcome_path, &memory_path, &telemetry_path] {
            let _ = std::fs::remove_file(path);
        }
        let mut command = Command::new(&self.executable);
        command
            .arg("--chimi-runner")
            .arg(if run.real_clock {
                "--clock=real"
            } else {
                "--clock=virtual"
            })
            .arg(match run.trace {
                Trace::Off => "--trace=off",
                Trace::Boundaries => "--trace=boundaries",
                Trace::Full => "--trace=full",
            })
            .arg(format!("--outcome-file={}", outcome_path.display()));
        if run.telemetry {
            command.arg(format!("--telemetry-file={}", telemetry_path.display()));
        }
        if run.memory_report {
            command.arg(format!("--memory-report-file={}", memory_path.display()));
        }
        command
            .arg("--")
            .args(argument)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let name = argument.unwrap_or("<no arguments>");
        let (status, elapsed_ns, max_resident) =
            run_measured(&mut command).map_err(|error| format!("run `{name}`: {error}"))?;
        if status != Exit::Code(0) {
            return Err(format!(
                "run `{name}` ended with {status:?}, expected exit 0"
            ));
        }
        let read = |path: &Path, wanted: bool| {
            if !wanted {
                return Ok(String::new());
            }
            std::fs::read_to_string(path)
                .map_err(|error| format!("run `{name}`: read {}: {error}", path.display()))
        };
        let outcome = read(&outcome_path, true)?;
        Ok((
            Finished {
                elapsed_ns,
                max_resident,
                memory_report: read(&memory_path, run.memory_report)?,
                telemetry: read(&telemetry_path, run.telemetry)?,
            },
            outcome,
        ))
    }
}

/// How a measured child ended.
#[derive(Debug, Eq, PartialEq)]
enum Exit {
    Code(i32),
    Signal(i32),
}

#[repr(C)]
struct ResourceUsage {
    user_time: [i64; 2],
    system_time: [i64; 2],
    max_resident: i64,
    other: [i64; 13],
}

unsafe extern "C" {
    fn wait4(pid: i32, status: *mut i32, options: i32, usage: *mut ResourceUsage) -> i32;
    fn kill(pid: i32, signal: i32) -> i32;
}

const SIGKILL: i32 = 9;

/// Spawns `command`, reaps it with `wait4`, and returns its exit, the wall
/// time from spawn to exit, and its maximum resident size in bytes. A child
/// still running after `RUN_LIMIT` is killed and reported as an error.
fn run_measured(command: &mut Command) -> Result<(Exit, u64, u64), String> {
    let started = Instant::now();
    let child = command.spawn().map_err(|error| error.to_string())?;
    let pid = i32::try_from(child.id()).map_err(|_| "child pid out of range")?;
    let (done, finished) = std::sync::mpsc::channel::<()>();
    let watchdog = std::thread::spawn(move || {
        let expired = finished.recv_timeout(RUN_LIMIT).is_err();
        if expired {
            // SAFETY: `pid` is our child, not yet reaped while `done` is unsent.
            unsafe { kill(pid, SIGKILL) };
        }
        expired
    });
    let mut status = 0;
    // SAFETY: an all-zero `ResourceUsage` is valid, and `wait4` writes only
    // within it and `status`.
    let mut usage: ResourceUsage = unsafe { std::mem::zeroed() };
    let reaped = loop {
        // SAFETY: both pointers are valid for the call.
        let result = unsafe { wait4(pid, &mut status, 0, &mut usage) };
        if result == pid {
            break Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            break Err(format!("wait4: {error}"));
        }
    };
    let elapsed = started.elapsed();
    let _ = done.send(());
    // `child` was reaped by `wait4`; dropping it neither waits nor kills.
    drop(child);
    let expired = watchdog.join().unwrap_or(false);
    reaped?;
    if expired {
        return Err(format!("exceeded the {} s run limit", RUN_LIMIT.as_secs()));
    }
    let exit = if status & 0x7f == 0 {
        Exit::Code((status >> 8) & 0xff)
    } else {
        Exit::Signal(status & 0x7f)
    };
    // macOS reports bytes, Linux KiB.
    let unit = if cfg!(target_os = "macos") { 1 } else { 1024 };
    let resident = u64::try_from(usage.max_resident).unwrap_or(0) * unit;
    let elapsed_ns = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
    Ok((exit, elapsed_ns, resident))
}

// ---------------------------------------------------------------------------
// Pure helpers.

/// The p95 of `samples` (sorted in place): the value at index
/// `len * 95 / 100`, which for 7 samples is the slowest.
fn p95(samples: &mut [u64]) -> u64 {
    samples.sort_unstable();
    samples[samples.len() * 95 / 100]
}

/// `(t2 - t1) / (n2 - n1)`; 0 when the larger size ran no slower.
fn differential(t1: u64, t2: u64, n1: u64, n2: u64) -> f64 {
    t2.saturating_sub(t1) as f64 / (n2 - n1) as f64
}

/// `measured / baseline`, never below 1: a mode faster than trace-off is
/// noise, not negative overhead.
fn ratio(measured: f64, baseline: f64) -> f64 {
    if baseline <= 0.0 {
        return f64::INFINITY;
    }
    (measured / baseline).max(1.0)
}

/// An integer field of a memory report.
fn report_field(report: &str, name: &str) -> Result<f64, String> {
    let Ok(JsonData::Object(fields)) = decode_json(report) else {
        return Err(format!("memory report is not a JSON object: {report}"));
    };
    fields
        .iter()
        .find(|(key, _)| key == name)
        .and_then(|(_, value)| match value {
            JsonData::Number(text) => text.parse().ok(),
            _ => None,
        })
        .ok_or_else(|| format!("memory report has no number `{name}`"))
}

/// The start and stop phases of one `members` run, in ns, from its
/// boundaries telemetry: start runs from `supervisor_started` to the first
/// `process_terminated` (every start, first park, and one report round trip),
/// stop from there to `supervisor_stopped`.
fn phase_nanoseconds(telemetry: &str) -> Result<(u64, u64), String> {
    let mut started = None;
    let mut first_terminated = None;
    let mut stopped = None;
    for line in telemetry.lines() {
        let Ok(JsonData::Object(fields)) = decode_json(line) else {
            return Err(format!("telemetry line is not an object: {line}"));
        };
        let field = |key: &str| {
            fields
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value)
        };
        let Some(JsonData::Text(event)) = field("event") else {
            continue;
        };
        let slot = match event.as_str() {
            "supervisor_started" => &mut started,
            "process_terminated" => &mut first_terminated,
            "supervisor_stopped" => &mut stopped,
            _ => continue,
        };
        if slot.is_some() {
            continue;
        }
        let time = match field("attributes") {
            Some(JsonData::Object(attributes)) => attributes
                .iter()
                .find(|(name, _)| name == "time_ns")
                .and_then(|(_, value)| match value {
                    JsonData::Number(text) => text.parse::<u64>().ok(),
                    _ => None,
                }),
            _ => None,
        };
        *slot = Some(time.ok_or_else(|| format!("{event} has no time_ns: {line}"))?);
    }
    match (started, first_terminated, stopped) {
        (Some(started), Some(terminated), Some(stopped))
            if started <= terminated && terminated <= stopped =>
        {
            Ok((terminated - started, stopped - terminated))
        }
        _ => Err(format!(
            "telemetry lacks ordered supervisor_started, process_terminated, and supervisor_stopped times ({started:?}, {first_terminated:?}, {stopped:?})"
        )),
    }
}

/// The warning lines for `report`'s gates whose value is more than 1.5 times
/// the same metric in `baseline`, a saved `performance` report line.
/// Metrics absent from the baseline are skipped.
pub fn baseline_warnings(gates: &[GateResult], baseline: &str) -> Result<Vec<String>, String> {
    let saved = baseline
        .lines()
        .filter_map(|line| match decode_json(line) {
            Ok(JsonData::Object(fields)) => Some(fields),
            _ => None,
        })
        .find(|fields| {
            fields
                .iter()
                .any(|(key, value)| key == "kind" && *value == JsonData::Text("performance".into()))
        })
        .ok_or("the baseline has no performance report line")?;
    let Some((_, JsonData::Array(entries))) = saved.iter().find(|(key, _)| key == "gates") else {
        return Err("the baseline's performance report has no gates list".into());
    };
    let mut values = Vec::new();
    for entry in entries {
        let JsonData::Object(fields) = entry else {
            continue;
        };
        let field = |key: &str| {
            fields
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value)
        };
        if let (Some(JsonData::Text(metric)), Some(JsonData::Number(value))) =
            (field("metric"), field("value"))
            && let Ok(value) = value.parse::<f64>()
        {
            values.push((metric.clone(), value));
        }
    }
    Ok(gates
        .iter()
        .filter_map(|gate| {
            let (_, saved) = values.iter().find(|(metric, _)| *metric == gate.metric)?;
            (gate.value > saved * 1.5).then(|| {
                format!(
                    "warning: {} is {}, more than 1.5 times its baseline {}",
                    gate.metric,
                    super::render_number(gate.value),
                    super::render_number(*saved)
                )
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Rejects a p95 that takes the median or the fastest sample, and a sort
    // that leaves the slowest of 7 out.
    #[test]
    fn p95_of_seven_samples_is_the_slowest() {
        assert_eq!(p95(&mut [5, 1, 7, 3, 2, 6, 4]), 7);
        assert_eq!(p95(&mut [9; 1]), 9);
        // With 20 samples the p95 skips only the slowest.
        let mut twenty: Vec<u64> = (1..=20).collect();
        assert_eq!(p95(&mut twenty), 20);
        let mut forty: Vec<u64> = (1..=40).collect();
        assert_eq!(p95(&mut forty), 39);
    }

    // Rejects a per-unit cost that divides the large run's time by its size
    // (so setup does not cancel), and a negative cost when the large size
    // happened to run faster.
    #[test]
    fn differential_cost_cancels_fixed_setup() {
        // 2 ms of setup, 300 ns per message.
        let t = |n: u64| 2_000_000 + 300 * n;
        assert_eq!(
            differential(t(10_000), t(1_000_000), 10_000, 1_000_000),
            300.0
        );
        assert_eq!(differential(500, 400, 1, 2), 0.0);
    }

    // Rejects overhead ratios below 1 and a zero baseline that divides into a
    // passing NaN.
    #[test]
    fn trace_ratios_are_at_least_one_and_fail_on_a_zero_baseline() {
        assert_eq!(ratio(150.0, 100.0), 1.5);
        assert_eq!(ratio(90.0, 100.0), 1.0);
        assert!(!GateResult::below("x", ratio(5.0, 0.0), 1.25).passed);
    }

    fn event(name: &str, time: u64) -> String {
        format!(
            "{{\"schema\":\"chimi.runtime.v1\",\"event\":\"{name}\",\"sequence\":1,\"attributes\":{{\"time_ns\":{time},\"cause\":null}}}}\n"
        )
    }

    // Rejects phases measured from the last start (which drops the start
    // cost), or to the last termination (which drops the supervisor's own
    // stop work).
    #[test]
    fn phases_run_from_the_root_start_to_the_first_termination_to_the_stop() {
        let telemetry = [
            event("supervisor_started", 1_000),
            event("process_started", 1_500),
            event("process_started", 4_000),
            "{\"event\":\"task_started\",\"sequence\":2}\n".to_owned(),
            event("process_terminated", 9_000),
            event("process_terminated", 9_500),
            event("supervisor_stopped", 12_000),
        ]
        .concat();
        assert_eq!(phase_nanoseconds(&telemetry), Ok((8_000, 3_000)));
        assert!(phase_nanoseconds(&event("supervisor_started", 1)).is_err());
        let backwards = [
            event("supervisor_started", 9),
            event("process_terminated", 5),
            event("supervisor_stopped", 10),
        ]
        .concat();
        assert!(phase_nanoseconds(&backwards).is_err());
    }

    // Rejects a gate that passes at its limit, an equality gate that accepts
    // a near value, and a baseline check that warns at exactly 1.5 times or
    // stays silent above it.
    #[test]
    fn gates_are_strict_and_baselines_warn_above_one_and_a_half_times() {
        assert!(GateResult::below("x", 1_999.0, 2_000.0).passed);
        assert!(!GateResult::below("x", 2_000.0, 2_000.0).passed);
        assert!(GateResult::exactly("x", 0.0, 0.0).passed);
        assert!(!GateResult::exactly("x", 1.0, 0.0).passed);
        let baseline = concat!(
            "{\"schema\":\"chimi.process-conformance.v1\",\"kind\":\"forcing\",\"gates\":[]}\n",
            "{\"schema\":\"chimi.process-conformance.v1\",\"kind\":\"performance\",\"gates\":[",
            "{\"metric\":\"a\",\"value\":100,\"limit\":2000,\"rule\":\"<\",\"status\":\"pass\"},",
            "{\"metric\":\"b\",\"value\":100,\"limit\":2000,\"rule\":\"<\",\"status\":\"pass\"}]}\n",
        );
        let gates = [
            GateResult::below("a", 150.0, 2_000.0),
            GateResult::below("b", 151.0, 2_000.0),
            GateResult::below("c", 1_000.0, 2_000.0),
        ];
        assert_eq!(
            baseline_warnings(&gates, baseline),
            Ok(vec![
                "warning: b is 151, more than 1.5 times its baseline 100".to_owned()
            ])
        );
        assert!(baseline_warnings(&gates, "{\"kind\":\"forcing\"}\n").is_err());
    }
}
