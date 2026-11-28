//! How a run starts and ends (CHI-305 step 5, phase 8): the runner's
//! arguments (`--chimi-runner`, the settings up to `--`, and the checks
//! between them), opening the report files the run writes as it goes, and
//! the exit status. runtime.c's `chimi_rt_main` calls these around the
//! program's entry. Numbers are read with libc's `strtoull`, as runtime.c read
//! them with `strtoul` and `strtoull`, so every accepted spelling is the same
//! on the 64-bit targets Chimi builds for, where the two agree.

use alloc::vec::Vec;
use core::ffi::{CStr, c_char, c_void};

use crate::heap::layout::{
    CARRIERS_MAX, Constructor, Control, ControlKind, Environment, Function, InjectedSignal, MAX_INJECTED_SIGNALS, Runtime,
    RunnerOptions, ValueKind,
};
use crate::intrinsics::console_io::chimi_runtime_console_set_v1;
use crate::intrinsics::signals::{chimi_runtime_end_by_signal_v1, chimi_runtime_stop_signal_v1};
use crate::intrinsics::standard::{string, symbol};
use crate::reports::{open_report, report_open_failure};
use crate::telemetry::unsigned_spelling;

/// `ChimiNodeFn`: a node's compiled function.
type NodeFn = unsafe extern "C" fn(*mut Runtime, *mut Environment, u64) -> *mut Control;

unsafe extern "C" {
    static CHIMI_CONSTRUCTORS: [Constructor; 0];
    static CHIMI_TEST_COUNT: u32;
    static CHIMI_FUNCTIONS: [Function; 0];
    static CHIMI_FUNCTION_COUNT: u32;
    static CHIMI_NODE_FUNCTIONS: [Option<NodeFn>; 0];
    static CHIMI_TEST_FUNCTIONS: [u32; 0];
    static CHIMI_TEST_COMPUTATIONS: [*const c_char; 0];
    fn chimi_runtime_random_seed_v1(seed: u64);
    fn chimi_timestamp_parse_v1(text: *const u8, length: usize, nanoseconds: *mut i64) -> bool;
}

/// The exit status of a run refused for its arguments (EX_USAGE).
const USAGE: i32 = 64;

/// The exit status of a run the runtime itself cannot carry out
/// (EX_SOFTWARE).
const SOFTWARE: i32 = 70;

#[cfg(target_os = "linux")]
fn errno() -> *mut i32 {
    // SAFETY: the calling thread's errno.
    unsafe { libc::__errno_location() }
}

#[cfg(not(target_os = "linux"))]
fn errno() -> *mut i32 {
    // SAFETY: the calling thread's errno.
    unsafe { libc::__error() }
}

/// Writes `parts` and a newline to stderr as one line, and gives the usage
/// status.
fn refuse(parts: &[&[u8]]) -> i32 {
    let mut line = Vec::new();
    for part in parts {
        line.extend_from_slice(part);
    }
    line.push(b'\n');
    // SAFETY: the buffer holds `line.len()` bytes.
    unsafe { libc::write(2, line.as_ptr().cast(), line.len()) };
    USAGE
}

/// `text` after `prefix`, as a C string, if it starts with it.
fn after<'a>(text: &'a CStr, prefix: &[u8]) -> Option<&'a CStr> {
    let bytes = text.to_bytes_with_nul();
    // SAFETY: what follows the prefix ends at the same NUL.
    bytes.starts_with(prefix).then(|| unsafe { CStr::from_bytes_with_nul_unchecked(&bytes[prefix.len()..]) })
}

/// `strtoull(text, &end, 10)`, with whether it overflowed and the rest.
fn read_unsigned(text: &CStr) -> (u64, bool, &[u8]) {
    let mut end: *mut c_char = core::ptr::null_mut();
    // SAFETY: `text` is a C string and errno is this thread's.
    unsafe {
        *errno() = 0;
        let value = libc::strtoull(text.as_ptr(), &mut end, 10) as u64;
        let overflowed = *errno() == libc::ERANGE;
        let rest = CStr::from_ptr(end).to_bytes();
        (value, overflowed, rest)
    }
}

/// `INT@2s`: a signal the virtual clock delivers at that instant. The time
/// is an integer with a unit of ns, us, ms, s, m, or h. INT and TERM stop
/// the program; HUP, USR1 and USR2 are std/signal's, latched for its
/// waiters, and may be given more than once.
fn parse_injected_signal(text: &CStr) -> Option<(i32, i64)> {
    let bytes = text.to_bytes();
    let (number, prefix) = [
        (libc::SIGINT, &b"INT@"[..]),
        (libc::SIGTERM, b"TERM@"),
        (libc::SIGHUP, b"HUP@"),
        (libc::SIGUSR1, b"USR1@"),
        (libc::SIGUSR2, b"USR2@"),
    ]
    .into_iter()
    .find(|(_, prefix)| bytes.starts_with(prefix))?;
    if !bytes.get(prefix.len()).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    // SAFETY: the prefix is ASCII, so the rest is a C string.
    let rest = unsafe { CStr::from_bytes_with_nul_unchecked(&text.to_bytes_with_nul()[prefix.len()..]) };
    let (amount, overflowed, unit) = read_unsigned(rest);
    if overflowed {
        return None;
    }
    let nanoseconds: u64 = match unit {
        b"ns" => 1,
        b"us" => 1_000,
        b"ms" => 1_000_000,
        b"s" => 1_000_000_000,
        b"m" => 60_000_000_000,
        b"h" => 3_600_000_000_000,
        _ => return None,
    };
    if amount > i64::MAX as u64 / nanoseconds {
        return None;
    }
    Some((number, (amount * nanoseconds) as i64))
}

/// Reads `--chimi-runner` and its settings up to `--` into `runtime` and
/// `options`, which start with their defaults, and leaves the first
/// application argument's index in `options.first_argument`. Without
/// `--chimi-runner` there are no settings. Returns 0 to run, or the exit
/// status after a refusal on stderr. `simulation` is the carrier simulation
/// build, which has a single carrier.
///
/// # Safety
/// `runtime` and `options` are live; `argv` holds `argc` C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_parse_runner_v1(
    runtime: *mut Runtime,
    argc: i32,
    argv: *const *const c_char,
    simulation: bool,
    options: *mut RunnerOptions,
) -> i32 {
    // SAFETY: the caller's contract.
    unsafe {
        let runtime = &mut *runtime;
        let options = &mut *options;
        let argument = |index: i32| CStr::from_ptr(*argv.add(index as usize));
        let mut index = 1;
        if index < argc && argument(index).to_bytes() == b"--chimi-runner" {
            index += 1;
            let mut clock_start_given = false;
            let mut found_boundary = false;
            while index < argc {
                let text = argument(index);
                index += 1;
                if text.to_bytes() == b"--" {
                    found_boundary = true;
                    break;
                }
                let status = runner_setting(runtime, options, text, &mut clock_start_given);
                if status != 0 {
                    return status;
                }
            }
            if !found_boundary {
                return refuse(&[b"Chimi runner configuration must end with --"]);
            }
            if (runtime.injected_signal != 0 || runtime.injected_user_signal_count != 0) && !runtime.virtual_clock {
                return refuse(&[b"--signal requires --clock=virtual; under the real clock, send the signal"]);
            }
            if clock_start_given && !runtime.virtual_clock {
                return refuse(&[b"--clock-start requires --clock=virtual; the real clock reads the host's wall time"]);
            }
            if runtime.carriers > 1 && runtime.virtual_clock {
                return refuse(&[
                    b"--carriers above 1 requires --clock=real; virtual time moves only when every carrier is idle",
                ]);
            }
            if simulation && runtime.carriers > 1 {
                return refuse(&[b"--carriers above 1 is not available in the carrier simulation build"]);
            }
        }
        options.first_argument = index;
        0
    }
}

/// Applies one runner setting; 0, or the usage status after a refusal.
///
/// # Safety
/// The test count and the random seed are runtime.c's and Rust's.
unsafe fn runner_setting(
    runtime: &mut Runtime,
    options: &mut RunnerOptions,
    text: &CStr,
    clock_start_given: &mut bool,
) -> i32 {
    // SAFETY: the caller's contract.
    unsafe {
        let path = |prefix: &[u8]| after(text, prefix).map(CStr::as_ptr);
        match text.to_bytes() {
            b"--trace=full" => (runtime.full_trace, runtime.silent) = (true, false),
            b"--trace=boundaries" => (runtime.full_trace, runtime.silent) = (false, false),
            b"--trace=off" => (runtime.full_trace, runtime.silent) = (false, true),
            b"--cancel" => runtime.cancel = 1,
            b"--clock=real" => runtime.virtual_clock = false,
            b"--clock=virtual" => runtime.virtual_clock = true,
            _ => {
                if let Some(path) = path(b"--telemetry-file=") {
                    options.telemetry_path = path;
                } else if let Some(path) = path(b"--outcome-file=") {
                    options.outcome_path = path;
                } else if let Some(path) = path(b"--memory-report-file=") {
                    options.memory_report_path = path;
                } else if let Some(path) = path(b"--profile-file=") {
                    options.profile_path = path;
                } else if let Some(id) = path(b"--execution-id=") {
                    runtime.execution_id = id;
                } else if let Some(value) = after(text, b"--test=") {
                    let (index, overflowed, rest) = read_unsigned(value);
                    // A sign or a space before the digits is still a number to
                    // strtoul; only an empty one is not.
                    let read = rest.len() < value.to_bytes().len();
                    if overflowed || !read || !rest.is_empty() || index >= u64::from(CHIMI_TEST_COUNT) {
                        let mut digits = [0; 20];
                        return refuse(&[
                            b"--test requires a test index below ",
                            unsigned_spelling(u64::from(CHIMI_TEST_COUNT), &mut digits),
                        ]);
                    }
                    options.test_index = index as i64;
                } else if let Some(value) = after(text, b"--signal=") {
                    let Some((number, at)) = parse_injected_signal(value) else {
                        return refuse(&[
                            b"--signal requires INT, TERM, HUP, USR1 or USR2, @, and a time such as 2s or 150ms",
                        ]);
                    };
                    if number == libc::SIGINT || number == libc::SIGTERM {
                        runtime.injected_signal = number;
                        runtime.injected_signal_at = at;
                    } else if runtime.injected_user_signal_count == MAX_INJECTED_SIGNALS {
                        return refuse(&[b"--signal is given too many times; at most 64 HUP, USR1 and USR2 signals"]);
                    } else {
                        let code = match number {
                            libc::SIGHUP => 1,
                            libc::SIGUSR1 => 2,
                            _ => 3,
                        };
                        runtime.injected_user_signals[runtime.injected_user_signal_count] = InjectedSignal { code, at };
                        runtime.injected_user_signal_count += 1;
                    }
                } else if let Some(value) = after(text, b"--random-seed=") {
                    let (seed, overflowed, rest) = read_unsigned(value);
                    let read = rest.len() < value.to_bytes().len();
                    if overflowed || !read || !rest.is_empty() || value.to_bytes().first() == Some(&b'-') {
                        return refuse(&[b"--random-seed requires an integer from 0 through 18446744073709551615"]);
                    }
                    chimi_runtime_random_seed_v1(seed);
                } else if let Some(value) = after(text, b"--carriers=") {
                    // strtoul skips spaces and takes a sign, so the count must
                    // start with a digit; one that overflows reads as the
                    // largest value, which the range refuses.
                    let (count, rest) = if value.to_bytes().first().is_some_and(u8::is_ascii_digit) {
                        let (count, _, rest) = read_unsigned(value);
                        (count, rest)
                    } else {
                        (0, &b""[..])
                    };
                    if count == 0 || !rest.is_empty() || count > u64::from(CARRIERS_MAX) {
                        let mut digits = [0; 20];
                        return refuse(&[
                            b"--carriers requires an integer from 1 through ",
                            unsigned_spelling(u64::from(CARRIERS_MAX), &mut digits),
                        ]);
                    }
                    runtime.carriers = count as u32;
                    options.carriers_given = true;
                } else if let Some(value) = after(text, b"--clock-start=") {
                    if !chimi_timestamp_parse_v1(value.as_ptr().cast(), value.to_bytes().len(), &mut runtime.clock_start) {
                        return refuse(&[
                            b"--clock-start requires an RFC 3339 timestamp from 1678 through 2262, such as 2026-11-10T09:30:00Z",
                        ]);
                    }
                    *clock_start_given = true;
                } else if let Some(value) = after(text, b"--max-line-bytes=") {
                    let (bytes, overflowed, rest) = read_unsigned(value);
                    let read = rest.len() < value.to_bytes().len();
                    if overflowed || !read || !rest.is_empty() || bytes == 0 || bytes > u64::from(u32::MAX) {
                        return refuse(&[b"--max-line-bytes requires an integer from 1 through 4294967295"]);
                    }
                    runtime.max_line_bytes = bytes as u32;
                } else if let Some(level) = after(text, b"--log-level=") {
                    runtime.log_level = match level.to_bytes() {
                        b"debug" => 0,
                        b"info" => 1,
                        b"warn" => 2,
                        b"error" => 3,
                        _ => return refuse(&[b"--log-level requires debug, info, warn or error"]),
                    };
                } else if let Some(path) = path(b"--log-file=") {
                    options.log_path = path;
                } else if let Some(path) = path(b"--metrics-file=") {
                    options.metrics_path = path;
                } else if let Some(value) = after(text, b"--benchmark-repeats=") {
                    // As `(uint32_t)strtoul(value, NULL, 10)`: no checks.
                    options.benchmark_repeats = read_unsigned(value).0 as u32;
                } else {
                    return refuse(&[b"unknown Chimi runtime argument: ", text.to_bytes()]);
                }
            }
        }
        0
    }
}

/// Refuses report and log files that name the same path, a trace without a
/// telemetry file, and a profile of a benchmark. Returns 0, or the usage
/// status after a refusal on stderr. The same file under two spellings is
/// caught as the files are opened.
///
/// # Safety
/// `runtime` and `options` are live, and every path in `options` is null or
/// a C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_check_report_paths_v1(runtime: *const Runtime, options: *const RunnerOptions) -> i32 {
    // SAFETY: the caller's contract.
    unsafe {
        let options = &*options;
        let named = |path: *const c_char| (!path.is_null()).then(|| CStr::from_ptr(path));
        let same = |left: *const c_char, right: *const c_char| matches!((named(left), named(right)), (Some(l), Some(r)) if l == r);
        let telemetry = options.telemetry_path;
        let outcome = options.outcome_path;
        let memory = options.memory_report_path;
        let profile = options.profile_path;
        let log = options.log_path;
        let metrics = options.metrics_path;
        if !(*runtime).silent && telemetry.is_null() {
            return refuse(&[b"boundary or full tracing requires --telemetry-file"]);
        }
        if same(telemetry, outcome) {
            return refuse(&[b"telemetry and runner outcome require distinct files"]);
        }
        if same(memory, telemetry) || same(memory, outcome) {
            return refuse(&[b"memory report, telemetry, and runner outcome require distinct files"]);
        }
        if same(profile, telemetry) || same(profile, outcome) || same(profile, memory) {
            return refuse(&[b"profile, memory report, telemetry, and runner outcome require distinct files"]);
        }
        if [telemetry, outcome, memory, profile].into_iter().any(|other| same(log, other)) {
            return refuse(&[b"the log file must be distinct from the runner's report files"]);
        }
        if [telemetry, outcome, memory, profile, log].into_iter().any(|other| same(metrics, other)) {
            return refuse(&[b"the metrics file must be distinct from the runner's report files and the log file"]);
        }
        if !profile.is_null() && options.benchmark_repeats != 0 {
            return refuse(&[b"--profile-file cannot be combined with --benchmark-repeats"]);
        }
        0
    }
}

/// The exit status of a run that could not open a report file (EX_IOERR).
const IO_ERROR: i32 = 74;

/// Whether `left` and `right` are one file, unless it is a character device
/// such as /dev/null, which every report may share.
fn one_file(left: &libc::stat, right: &libc::stat) -> bool {
    left.st_dev == right.st_dev && left.st_ino == right.st_ino && (left.st_mode & libc::S_IFMT) != libc::S_IFCHR
}

/// The status of an open stream's file.
///
/// # Safety
/// `stream` is an open `FILE *`.
unsafe fn stream_status(stream: *mut c_void) -> Option<libc::stat> {
    // SAFETY: the caller's contract; `stat` is plain data.
    unsafe {
        let mut status: libc::stat = core::mem::zeroed();
        (libc::fstat(libc::fileno(stream.cast()), &mut status) == 0).then_some(status)
    }
}

/// The status of the file at `path`.
///
/// # Safety
/// `path` is a C string.
unsafe fn path_status(path: *const c_char) -> Option<libc::stat> {
    // SAFETY: the caller's contract; `stat` is plain data.
    unsafe {
        let mut status: libc::stat = core::mem::zeroed();
        (libc::stat(path, &mut status) == 0).then_some(status)
    }
}

/// Closes each open stream among `streams`, after a later report failed to
/// open.
///
/// # Safety
/// Each stream is null or an open `FILE *`.
unsafe fn close_all(streams: &[*mut c_void]) {
    for &stream in streams {
        if !stream.is_null() {
            // SAFETY: the caller's contract.
            unsafe { libc::fclose(stream.cast()) };
        }
    }
}

/// Opens the report files the run writes as it goes into `runtime`: the
/// telemetry (which `/dev/null` may be, shared), the runner outcome, the log
/// (appended to, since a log outlives one run, and given the log stream) and
/// the metrics. The same file under two spellings is refused, by comparing
/// the opened files; the memory report and profile are opened only at exit,
/// so the metrics file is compared with their paths. Returns 0, the usage
/// status after a clash, or the I/O status after a failed open, with the
/// reason on stderr.
///
/// # Safety
/// `runtime` and `options` are live; no fiber runs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_open_reports_v1(runtime: *mut Runtime, options: *const RunnerOptions) -> i32 {
    // SAFETY: the caller's contract; every path is null or a C string.
    unsafe {
        let options = &*options;
        if !options.telemetry_path.is_null() {
            let path = options.telemetry_path;
            let mode = if CStr::from_ptr(path).to_bytes() == b"/dev/null" { c"w" } else { c"wx" };
            (*runtime).telemetry = open_report(path, mode).cast();
            if (*runtime).telemetry.is_null() {
                report_open_failure(b"telemetry", path);
                return IO_ERROR;
            }
        }
        if !options.outcome_path.is_null() {
            (*runtime).outcome = open_report(options.outcome_path, c"wx").cast();
            if (*runtime).outcome.is_null() {
                report_open_failure(b"outcome", options.outcome_path);
                close_all(&[(*runtime).telemetry]);
                return IO_ERROR;
            }
        }
        if !options.log_path.is_null() {
            (*runtime).log_file = open_report(options.log_path, c"a").cast();
            if (*runtime).log_file.is_null() {
                report_open_failure(b"log", options.log_path);
                close_all(&[(*runtime).telemetry, (*runtime).outcome]);
                return IO_ERROR;
            }
            for report in [(*runtime).telemetry, (*runtime).outcome] {
                if report.is_null() {
                    continue;
                }
                if let (Some(log), Some(other)) = (stream_status((*runtime).log_file), stream_status(report))
                    && one_file(&log, &other)
                {
                    return refuse(&[b"the log file must be distinct from the runner's report files"]);
                }
            }
            chimi_runtime_console_set_v1(2, libc::fileno((*runtime).log_file.cast()), false);
        }
        if !options.metrics_path.is_null() {
            let clash = b"the metrics file must be distinct from the runner's report files and the log file";
            // Compared before opening, since opening empties the file.
            if let Some(metrics) = path_status(options.metrics_path)
                && (metrics.st_mode & libc::S_IFMT) != libc::S_IFCHR
            {
                for opened in [(*runtime).telemetry, (*runtime).outcome, (*runtime).log_file] {
                    if !opened.is_null()
                        && let Some(other) = stream_status(opened)
                        && metrics.st_dev == other.st_dev
                        && metrics.st_ino == other.st_ino
                    {
                        return refuse(&[clash]);
                    }
                }
            }
            (*runtime).metrics_file = open_report(options.metrics_path, c"w").cast();
            if (*runtime).metrics_file.is_null() {
                report_open_failure(b"metrics", options.metrics_path);
                close_all(&[(*runtime).telemetry, (*runtime).outcome, (*runtime).log_file]);
                return IO_ERROR;
            }
            for later in [options.memory_report_path, options.profile_path] {
                if !later.is_null()
                    && let Some(other) = path_status(later)
                    && let Some(opened) = stream_status((*runtime).metrics_file)
                    && one_file(&opened, &other)
                {
                    return refuse(&[clash]);
                }
            }
        }
        0
    }
}

/// Finds the function the run starts from: the selected test's, whose
/// computation the run then takes, or else the one whose body compiles to
/// `entry`. An entry without a parameter refuses application arguments.
/// Gives 0 with the function in `*function`, or the exit status.
///
/// # Safety
/// `runtime`, `options` and `function` are live, `options` as
/// [`chimi_runtime_parse_runner_v1`] left it for `argc` arguments, and the
/// program data is linked.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_program_entry_v1(
    runtime: *mut Runtime,
    entry: NodeFn,
    options: *const RunnerOptions,
    argc: i32,
    function: *mut *const Function,
) -> i32 {
    // SAFETY: the caller's contract; a selected test's index is below
    // CHIMI_TEST_COUNT, and every function's body is a node.
    unsafe {
        let options = &*options;
        let functions = (&raw const CHIMI_FUNCTIONS).cast::<Function>();
        let found = if options.test_index >= 0 {
            let index = options.test_index as usize;
            (*runtime).computation = *(&raw const CHIMI_TEST_COMPUTATIONS).cast::<*const c_char>().add(index);
            functions.add(*(&raw const CHIMI_TEST_FUNCTIONS).cast::<u32>().add(index) as usize)
        } else {
            let nodes = (&raw const CHIMI_NODE_FUNCTIONS).cast::<Option<NodeFn>>();
            (0..CHIMI_FUNCTION_COUNT as usize)
                .map(|index| functions.add(index))
                .find(|&candidate| {
                    (*nodes.add((*candidate).body as usize)).is_some_and(|body| core::ptr::fn_addr_eq(body, entry))
                })
                .unwrap_or(core::ptr::null())
        };
        if found.is_null() {
            return SOFTWARE;
        }
        if (*found).parameters_length == 0 && options.first_argument < argc {
            return refuse(&[b"program entry does not accept application arguments"]);
        }
        *function = found;
        0
    }
}

/// The exit status of a run that ended with `control`. A program-exit value
/// (std/cli's `Exit`, CHI-252) sets it: `Code(n)` exits with n, a defect (70)
/// outside 0 to 255; `Help(text)` writes the text to `stdout` and exits 0;
/// `Usage(_, message)` writes the message to `stderr` and exits 2; either
/// exits 74 when the write fails. Otherwise a
/// success exits 0, a failure 1, a defect 70, and a cancellation by a stop
/// signal re-raises that signal, or reports the shell's status for an
/// injected one; any other cancellation exits 130.
///
/// # Safety
/// `control` is the run's live result; `stdout` and `stderr` are the C
/// streams.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_exit_status_v1(
    control: *const Control,
    injected_signal: i32,
    stdout: *mut libc::FILE,
    stderr: *mut libc::FILE,
) -> i32 {
    // SAFETY: the caller's contract; a program-exit constructor's last field
    // is an Int (Code) or a Text (Help, Usage).
    unsafe {
        let control = &*control;
        if let Some(status) = program_exit(control, stdout, stderr) {
            return status;
        }
        match control.kind {
            ControlKind::SUCCESS => 0,
            ControlKind::FAILURE => 1,
            ControlKind::DEFECT => 70,
            ControlKind::CANCEL => {
                let received = core::ptr::read_volatile(chimi_runtime_stop_signal_v1());
                if received != 0 && injected_signal != 0 {
                    return 128 + received;
                }
                if received != 0 {
                    chimi_runtime_end_by_signal_v1(received);
                }
                130
            }
            _ => 70,
        }
    }
}

/// The status a program-exit value sets, or `None` for any other result.
///
/// # Safety
/// As [`chimi_runtime_exit_status_v1`].
unsafe fn program_exit(control: &Control, stdout: *mut libc::FILE, stderr: *mut libc::FILE) -> Option<i32> {
    // SAFETY: the caller's contract.
    unsafe {
        if control.kind != ControlKind::SUCCESS && control.kind != ControlKind::FAILURE {
            return None;
        }
        let value = control.value;
        if value.is_null() || (*value).kind != ValueKind::CONSTRUCTED {
            return None;
        }
        let constructed = &(*value).payload.constructed;
        let shape =
            &*(&raw const CHIMI_CONSTRUCTORS).cast::<Constructor>().add(symbol(constructed.constructor).constructor as usize);
        if shape.program_exit == 0 {
            return None;
        }
        let field = &*(*constructed.fields.add(constructed.length as usize - 1)).value;
        let variant = CStr::from_ptr(string(shape.variant_name)).to_bytes();
        if variant == b"Code" {
            let code = field.payload.integer;
            if !(0..=255).contains(&code) {
                let mut digits = [0; 20];
                let spelled = unsigned_spelling(code.unsigned_abs(), &mut digits);
                let sign: &[u8] = if code < 0 { b"-" } else { b"" };
                let mut line = Vec::new();
                for part in [&b"Chimi runtime: Exit.Code("[..], sign, spelled, b") is outside 0 to 255\n"] {
                    line.extend_from_slice(part);
                }
                libc::fwrite(line.as_ptr().cast(), 1, line.len(), stderr);
                return Some(70);
            }
            return Some(code as i32);
        }
        let help = variant == b"Help";
        let stream = if help { stdout } else { stderr };
        let text = &field.payload.text;
        let written = libc::fwrite(text.bytes.cast(), 1, text.length, stream) == text.length
            && libc::fputc(i32::from(b'\n'), stream) != libc::EOF
            && libc::fflush(stream) == 0;
        Some(if !written { 74 } else if help { 0 } else { 2 })
    }
}
