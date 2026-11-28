//! The run itself, once the runner has read its settings, opened its report
//! files and found the entry function (CHI-305 step 5, phase 8): the entry's
//! environment and application arguments, the computation's telemetry
//! events, the benchmark repeats, the reports written when the program
//! ends, and its exit status. runtime.c's `chimi_rt_main` calls
//! [`chimi_runtime_run_program_v1`] last, and still builds the scheduler's
//! configuration for each execution of the entry.

use core::ffi::{c_char, c_void};
use core::sync::atomic::Ordering::Relaxed;

use crate::eval::slot;
use crate::heap::allocator::carrier_count;
use crate::heap::layout::{Carrier, Control, Environment, Function, Runtime, RunnerOptions, Value};
use crate::heap::list;
use crate::heap::values::{new_environment, new_value_slots, text_value};
use crate::intrinsics::binding_calls::chimi_runtime_release_binding_handles_v1;
use crate::intrinsics::console_io::{chimi_runtime_console_drain_v1, chimi_runtime_console_set_v1};
use crate::intrinsics::metrics_host::{chimi_runtime_metrics_document_v1, chimi_runtime_metrics_started_v1};
use crate::outcome::chimi_runtime_print_outcome_v1;
use crate::reports::{chimi_runtime_write_memory_report_v1, chimi_runtime_write_profile_v1, close_output};
use crate::startup::{NodeFn, chimi_runtime_exit_status_v1};
use crate::telemetry::{chimi_trace_event_v1, unsigned_spelling};
use crate::text_abi::chimi_text_release_v1;

/// Runs `entry` once in `environment`, under the scheduler when the program
/// needs one (runtime.c's `execute_program_entry`).
type ExecuteEntry = unsafe extern "C" fn(*mut Runtime, NodeFn, *mut Environment) -> *mut Control;

unsafe extern "C" {
    static CHIMI_IDS: [u32; 0];
    fn chimi_runtime_carrier_at(index: usize) -> *mut Carrier;
}

/// The exit status of a report that could not be written (EX_IOERR).
const IO_ERROR: i32 = 74;

/// The exit status of a run the runtime itself cannot carry out
/// (EX_SOFTWARE).
const SOFTWARE: i32 = 70;

/// Runs the program from `function`, whose body compiles to `entry`, and
/// gives its exit status. With `--benchmark-repeats`, it runs the entry that
/// many times and prints the mean wall time of one run in nanoseconds on
/// stderr; otherwise it runs it once, writes the outcome record, and exits
/// as the outcome says. Either way it then writes the memory report, the
/// profile and the metrics, and closes the telemetry and outcome files; a
/// failed write exits 74.
///
/// # Safety
/// Runs on the host thread with no task running, after
/// `chimi_runtime_open_reports_v1`; `function` is the entry function
/// `chimi_runtime_program_entry_v1` found, `argv` holds `argc` C strings,
/// a single run's stop signals are installed, and `stdout` and `stderr` are
/// the C streams.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn chimi_runtime_run_program_v1(
    runtime: *mut Runtime,
    entry: NodeFn,
    function: *const Function,
    options: *const RunnerOptions,
    argc: i32,
    argv: *const *const c_char,
    process_kernel: bool,
    execute: ExecuteEntry,
    stdout: *mut libc::FILE,
    stderr: *mut libc::FILE,
) -> i32 {
    // SAFETY: the caller's contract.
    unsafe {
        let options = &*options;
        let function = &*function;
        let arguments = Arguments { count: argc - options.first_argument, values: argv.add(options.first_argument as usize) };
        if options.benchmark_repeats > 0 {
            return benchmark(runtime, entry, function, options, arguments, process_kernel, execute, stderr);
        }
        (*runtime).start_unix_ns = start_unix_nanos(runtime);
        chimi_runtime_metrics_started_v1((*runtime).start_unix_ns);
        let control = run_entry(runtime, entry, function, arguments, execute);
        // A log line already went out whole; a failed close loses nothing
        // the program could act on.
        if !(*runtime).log_file.is_null() {
            chimi_runtime_console_set_v1(2, -1, false);
            libc::fclose((*runtime).log_file.cast());
        }
        computation_event(runtime, c"computation_completed", function, control);
        if !(*runtime).outcome.is_null() {
            chimi_runtime_print_outcome_v1((*runtime).outcome.cast(), control);
        }
        let mut write_failed = chimi_runtime_write_memory_report_v1(options.memory_report_path, runtime, process_kernel);
        write_failed = chimi_runtime_write_profile_v1(options.profile_path) || write_failed;
        write_failed = write_metrics(runtime, (*runtime).start_unix_ns, (*runtime).metrics_file) || write_failed;
        write_failed = close_report((*runtime).telemetry) || write_failed;
        write_failed = close_report((*runtime).outcome) || write_failed;
        if write_failed {
            return IO_ERROR;
        }
        chimi_runtime_exit_status_v1(control, (*runtime).injected_signal, stdout, stderr)
    }
}

/// The application arguments after the runner's `--`.
#[derive(Clone, Copy)]
struct Arguments {
    count: i32,
    values: *const *const c_char,
}

/// The benchmark repeats of [`chimi_runtime_run_program_v1`]. Each repeat
/// starts the spans, sequence numbers and clock origin afresh; the metrics
/// aggregate every repeat, so they start with the first.
///
/// # Safety
/// As for [`chimi_runtime_run_program_v1`].
#[allow(clippy::too_many_arguments)]
unsafe fn benchmark(
    runtime: *mut Runtime,
    entry: NodeFn,
    function: &Function,
    options: &RunnerOptions,
    arguments: Arguments,
    process_kernel: bool,
    execute: ExecuteEntry,
    stderr: *mut libc::FILE,
) -> i32 {
    // SAFETY: the caller's contract.
    unsafe {
        let started = monotonic_nanos();
        let mut metrics_started = 0;
        for repeat in 0..options.benchmark_repeats {
            (*runtime).next_sequence = 0;
            (*runtime).next_span.store(2, Relaxed);
            for index in 0..carrier_count() {
                let carrier = chimi_runtime_carrier_at(index);
                (*carrier).next_span = 0;
                (*carrier).end_span = 0;
            }
            (*runtime).start_unix_ns = start_unix_nanos(runtime);
            if repeat == 0 {
                metrics_started = (*runtime).start_unix_ns;
                chimi_runtime_metrics_started_v1(metrics_started);
            }
            let control = run_entry(runtime, entry, function, arguments, execute);
            if control.is_null() {
                return SOFTWARE;
            }
            computation_event(runtime, c"computation_completed", function, control);
        }
        let mut write_failed = chimi_runtime_write_memory_report_v1(options.memory_report_path, runtime, process_kernel);
        write_failed = chimi_runtime_write_profile_v1(options.profile_path) || write_failed;
        write_failed = write_metrics(runtime, metrics_started, (*runtime).metrics_file) || write_failed;
        (*runtime).metrics_file = core::ptr::null_mut();
        write_failed = close_report((*runtime).telemetry) || write_failed;
        (*runtime).telemetry = core::ptr::null_mut();
        write_failed = close_report((*runtime).outcome) || write_failed;
        (*runtime).outcome = core::ptr::null_mut();
        let elapsed = monotonic_nanos() - started;
        let mut digits = [0; 20];
        let mean = unsigned_spelling((elapsed / i64::from(options.benchmark_repeats)) as u64, &mut digits);
        let mut line = [b'\n'; 21];
        line[..mean.len()].copy_from_slice(mean);
        libc::fwrite(line.as_ptr().cast(), 1, mean.len() + 1, stderr);
        if write_failed { IO_ERROR } else { 0 }
    }
}

/// One execution of the entry, from the computation's start event to the
/// console lines and binding handles it left: the program's control, or
/// null when the scheduler produced none.
///
/// # Safety
/// As for [`chimi_runtime_run_program_v1`]; the clock origin is set.
#[inline(never)]
unsafe fn run_entry(
    runtime: *mut Runtime,
    entry: NodeFn,
    function: &Function,
    arguments: Arguments,
    execute: ExecuteEntry,
) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        computation_event(runtime, c"computation_started", function, core::ptr::null());
        let environment = new_environment(function.local_count);
        if function.parameters_length == 1 {
            let parameter = *(&raw const CHIMI_IDS).cast::<u32>().add(function.parameters_offset as usize);
            *(*environment).locals.add(slot(parameter)) = application_arguments(arguments);
        }
        let control = if (*runtime).cancel != 0 {
            crate::eval::cancelled()
        } else {
            execute(runtime, entry, environment)
        };
        // Lines whose writers were cancelled while they waited (CHI-266).
        chimi_runtime_console_drain_v1();
        chimi_runtime_release_binding_handles_v1();
        control
    }
}

/// The application arguments as the entry's `List<Text>`.
///
/// # Safety
/// Runs in the main heap; `arguments` holds C strings.
unsafe fn application_arguments(arguments: Arguments) -> *mut Value {
    let count = arguments.count as u32;
    // SAFETY: the caller's contract.
    unsafe {
        let items = new_value_slots(count);
        for index in 0..count as usize {
            let text = *arguments.values.add(index);
            *items.add(index) = text_value(text, libc::strlen(text));
        }
        list::list_value(list::from_values(items, count), 0, count)
    }
}

/// The computation's `event` (its start or its end with `control`), unless
/// the run is silent.
///
/// # Safety
/// `runtime` is live; `control` is null or live.
unsafe fn computation_event(runtime: *mut Runtime, event: &core::ffi::CStr, function: &Function, control: *const Control) {
    // SAFETY: the caller's contract.
    unsafe {
        if !(*runtime).silent {
            chimi_trace_event_v1(runtime, event.as_ptr(), function.body, 1, -1, c"computation".as_ptr(), control);
        }
    }
}

/// Writes the metrics document, whose counts began at `started`, to
/// `output` and closes it; whether that failed. No file, no failure.
///
/// # Safety
/// `runtime` is live; `output` is null or an open stream.
unsafe fn write_metrics(runtime: *const Runtime, started: i64, output: *mut c_void) -> bool {
    if output.is_null() {
        return false;
    }
    // SAFETY: the caller's contract.
    unsafe {
        let document = chimi_runtime_metrics_document_v1(runtime, started);
        libc::fwrite(document.bytes.cast(), 1, document.length, output.cast());
        chimi_text_release_v1(document);
        close_output(output.cast())
    }
}

/// [`close_output`] for a report file that may not have been asked for.
///
/// # Safety
/// `output` is null or an open stream.
unsafe fn close_report(output: *mut c_void) -> bool {
    // SAFETY: the caller's contract.
    !output.is_null() && unsafe { close_output(output.cast()) }
}

/// The Unix time at the execution clock's origin: `--clock-start` under the
/// virtual clock, and under the real clock the host's wall time when the
/// computation starts, just before the scheduler takes its origin.
///
/// # Safety
/// `runtime` is live.
unsafe fn start_unix_nanos(runtime: *const Runtime) -> i64 {
    // SAFETY: the caller's contract.
    if unsafe { (*runtime).virtual_clock } {
        return unsafe { (*runtime).clock_start };
    }
    clock_nanos(libc::CLOCK_REALTIME)
}

fn monotonic_nanos() -> i64 {
    clock_nanos(libc::CLOCK_MONOTONIC)
}

fn clock_nanos(clock: libc::clockid_t) -> i64 {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `time` is a live timespec.
    unsafe { libc::clock_gettime(clock, &mut time) };
    time.tv_sec * 1_000_000_000 + time.tv_nsec
}
