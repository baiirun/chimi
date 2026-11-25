//! The C view of [`crate::metrics`]. The runtime keeps one aggregate behind
//! its own lock; names and labels are `Text` bytes, which the runtime keeps
//! valid UTF-8, so bytes that are not are a runtime bug and abort.

use crate::metrics::Aggregate;
use crate::text_abi::{ChimiOwnedText, owned_text, utf8};
use alloc::boxed::Box;

/// A new, empty aggregate, for [`chimi_metrics_record_v1`] and
/// [`chimi_metrics_document_v1`]. The runtime's lives as long as the program;
/// the fuzz target frees each of its own with [`chimi_metrics_free_v1`].
#[unsafe(no_mangle)]
pub extern "C" fn chimi_metrics_new_v1() -> *mut Aggregate {
    Box::into_raw(Box::default())
}

/// Adds one measurement (`kind` 0 count, 1 gauge, 2 observe).
///
/// # Safety
///
/// `aggregate` must come from [`chimi_metrics_new_v1`] and no other thread may
/// use it during the call; `name` and `labels` must point to `name_length` and
/// `labels_length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_metrics_record_v1(
    aggregate: *mut Aggregate,
    kind: i64,
    name: *const u8,
    name_length: usize,
    value: f64,
    labels: *const u8,
    labels_length: usize,
) {
    // SAFETY: forwarded from the caller.
    let (aggregate, name, labels) = unsafe { (&mut *aggregate, utf8(name, name_length), utf8(labels, labels_length)) };
    aggregate.record(kind, name, value, labels);
}

/// The metrics document as it stands, for the C runtime to copy and then
/// release with `chimi_text_release_v1`.
///
/// # Safety
///
/// `aggregate` must come from [`chimi_metrics_new_v1`] and no other thread may
/// use it during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_metrics_document_v1(
    aggregate: *const Aggregate,
    start_unix_ns: i64,
    time_unix_ns: i64,
) -> ChimiOwnedText {
    // SAFETY: the caller guarantees a live aggregate.
    owned_text(unsafe { &*aggregate }.document(start_unix_ns, time_unix_ns))
}

/// Frees an aggregate.
///
/// # Safety
///
/// `aggregate` must come from [`chimi_metrics_new_v1`], not be in use, and
/// not have been freed already.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_metrics_free_v1(aggregate: *mut Aggregate) {
    // SAFETY: the caller passes back the box `chimi_metrics_new_v1` made.
    drop(unsafe { Box::from_raw(aggregate) });
}
