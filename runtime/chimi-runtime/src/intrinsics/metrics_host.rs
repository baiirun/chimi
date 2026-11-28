//! std/metrics' host core: one aggregate per name and label set
//! ([`Aggregate`], which the interpreter keeps its measurements in too, so
//! both backends write the same document), written as one JSON document to
//! the `--metrics-file` file when the program ends. Every carrier records
//! into the one aggregate behind its lock.

use core::ffi::c_char;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use super::standard::{boolean_value, shape_defect, success, void_value};
use crate::heap::layout::{Control, ControlKind, Intrinsic, Runtime, Value, ValueKind};
use crate::heap::values::{new_control, text_value};
use crate::metrics::Aggregate;
use crate::pthread_locked::PthreadLocked;
use crate::text_abi::{ChimiOwnedText, owned_text, utf8};

unsafe extern "C" {
    /// The running fiber's clock, in nanoseconds since the run began
    /// (`runtime.c` defines a weak stand-in for programs without the
    /// scheduler).
    fn chimi_fiber_now() -> i64;
}

static AGGREGATE: PthreadLocked<Aggregate> = PthreadLocked::new(Aggregate::new());
/// Set by the first `Metrics.runner_document`: from then on the provider
/// records without `--metrics-file`.
static READING: AtomicBool = AtomicBool::new(false);
/// When the aggregate began: the run's start, or the first repeat's.
static STARTED_UNIX_NS: AtomicI64 = AtomicI64::new(0);

/// Notes when the aggregate began; `runtime.c` calls it before `main`
/// starts.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_runtime_metrics_started_v1(start_unix_ns: i64) {
    STARTED_UNIX_NS.store(start_unix_ns, Ordering::Relaxed);
}

/// Adds one measurement (`kind` 0 count, 1 gauge, 2 observe) from any
/// thread; the metrics stress harness calls it too.
///
/// # Safety
/// `name` and `labels` point to `name_length` and `labels_length` bytes of
/// UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_metrics_add_v1(
    kind: i64,
    name: *const u8,
    name_length: usize,
    value: f64,
    labels: *const u8,
    labels_length: usize,
) {
    // SAFETY: the caller's contract.
    let (name, labels) = unsafe { (utf8(name, name_length), utf8(labels, labels_length)) };
    AGGREGATE.with(|aggregate| aggregate.record(kind, name, value, labels));
}

/// The metrics document as it stands, whose counts began at
/// `start_unix_ns`, for `runtime.c` to write at exit and then release with
/// `chimi_text_release_v1`. At exit `main` has ended and the clock is where
/// its last fiber left it; mid-run, it is the reading fiber's time.
///
/// # Safety
/// `runtime` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_metrics_document_v1(
    runtime: *const Runtime,
    start_unix_ns: i64,
) -> ChimiOwnedText {
    // SAFETY: the caller's contract; the clock read has no precondition.
    let ended = unsafe { (*runtime).start_unix_ns.checked_add(chimi_fiber_now()) }.unwrap_or(i64::MAX);
    owned_text(AGGREGATE.with(|aggregate| aggregate.document(start_unix_ns, ended)))
}

/// Whether the provider records: with `--metrics-file`, or once the program
/// has read the aggregate.
///
/// # Safety
/// `runtime` is live.
unsafe fn recording(runtime: *const Runtime) -> bool {
    // SAFETY: the caller's contract.
    !unsafe { (*runtime).metrics_file }.is_null() || READING.load(Ordering::Acquire)
}

/// # Safety
/// `runtime` is live.
unsafe fn is_cancelled(runtime: *const Runtime) -> bool {
    // SAFETY: the caller's contract; another carrier may set the flag.
    unsafe { ptr::read_volatile(&raw const (*runtime).cancel) != 0 }
}

/// The running task was cancelled.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn cancelled() -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { new_control(ControlKind::CANCEL, ptr::null_mut(), ptr::null(), -1) }
}

/// The bytes of a Text value.
///
/// # Safety
/// `text` is a live Text.
unsafe fn text_parts(text: *const Value) -> (*const u8, usize) {
    // SAFETY: the caller's contract.
    unsafe { ((*text).payload.text.bytes.cast::<u8>(), (*text).payload.text.length) }
}

/// `Metrics.host_enabled`, `host_record` and `runner_document`.
///
/// # Safety
/// Runs on a fiber that may allocate; `runtime` and the arguments are live.
#[inline(never)]
pub unsafe fn metrics_operation(
    runtime: *mut Runtime,
    intrinsic: u32,
    arguments: &[*mut Value],
    call_node: u32,
) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        if intrinsic == Intrinsic::METRICS_HOST_ENABLED {
            if !arguments.is_empty() {
                return shape_defect(call_node);
            }
            return success(boolean_value(recording(runtime)));
        }
        if intrinsic == Intrinsic::METRICS_HOST_DOCUMENT {
            if !arguments.is_empty() {
                return shape_defect(call_node);
            }
            if is_cancelled(runtime) {
                return cancelled();
            }
            READING.store(true, Ordering::Release);
            let document = chimi_runtime_metrics_document_v1(runtime, STARTED_UNIX_NS.load(Ordering::Relaxed));
            let value = text_value(document.bytes.cast::<c_char>(), document.length);
            crate::text_abi::chimi_text_release_v1(document);
            return success(value);
        }
        let [kind, name, value, labels] = arguments else { return shape_defect(call_node) };
        if (**kind).kind != ValueKind::INTEGER
            || (**name).kind != ValueKind::TEXT
            || (**value).kind != ValueKind::FLOAT
            || (**labels).kind != ValueKind::TEXT
        {
            return shape_defect(call_node);
        }
        if is_cancelled(runtime) {
            return cancelled();
        }
        if recording(runtime) {
            let (name, name_length) = text_parts(*name);
            let (labels, labels_length) = text_parts(*labels);
            chimi_runtime_metrics_add_v1(
                (**kind).payload.integer,
                name,
                name_length,
                (**value).payload.floating,
                labels,
                labels_length,
            );
        }
        if is_cancelled(runtime) { cancelled() } else { success(void_value()) }
    }
}
