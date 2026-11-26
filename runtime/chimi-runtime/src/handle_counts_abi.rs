//! The C view of [`crate::handle_counts`]: how many live values name each
//! native handle. A copy made on one carrier can be freed on another, so
//! `runtime.c` makes every call under `handle_count_lock`. That lock is taken
//! after the scheduler's kernel lock, and nothing under it retains, drops,
//! parks or runs a hook; no call here does.

use crate::handle_counts::HandleCounts;
use crate::serialized::Serialized;

static COUNTS: Serialized<HandleCounts> = Serialized::new(HandleCounts::new());

/// Counts one more live value naming `key`; false, counting nothing, when
/// there is no memory to grow the map.
/// # Safety
/// The caller holds `handle_count_lock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_handle_counts_retain_v1(key: u64) -> bool {
    // SAFETY: the caller holds the lock.
    unsafe { COUNTS.with(|counts| counts.retain(key)) }
}

/// Counts one live value naming `key` fewer: true exactly when it was the
/// last, so the caller owns the handle's close.
/// # Safety
/// The caller holds `handle_count_lock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_handle_counts_drop_v1(key: u64) -> bool {
    // SAFETY: the caller holds the lock.
    unsafe { COUNTS.with(|counts| counts.drop(key)) }
}

/// Forgets every count before an execution starts.
/// # Safety
/// The caller holds `handle_count_lock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_handle_counts_reset_v1() {
    // SAFETY: the caller holds the lock.
    unsafe { COUNTS.with(HandleCounts::reset) }
}

/// Keys with at least one reference, for the stress fixture.
/// # Safety
/// The caller holds `handle_count_lock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_handle_counts_len_v1() -> usize {
    // SAFETY: the caller holds the lock.
    unsafe { COUNTS.with(|counts| counts.len()) }
}

/// Entries the map has room for, for the stress fixture.
/// # Safety
/// The caller holds `handle_count_lock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_handle_counts_capacity_v1() -> usize {
    // SAFETY: the caller holds the lock.
    unsafe { COUNTS.with(|counts| counts.capacity()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The one test of the shared map, so no other test races it.
    #[test]
    fn the_shared_map_counts_and_resets() {
        // SAFETY: the test is the only caller, so it stands in for the lock.
        unsafe {
            chimi_handle_counts_reset_v1();
            assert!(chimi_handle_counts_retain_v1(9));
            assert!(chimi_handle_counts_retain_v1(9));
            assert_eq!(chimi_handle_counts_len_v1(), 1);
            assert!(chimi_handle_counts_capacity_v1() >= 2);
            assert!(!chimi_handle_counts_drop_v1(9));
            assert!(chimi_handle_counts_drop_v1(9));
            assert!(chimi_handle_counts_retain_v1(4));
            chimi_handle_counts_reset_v1();
            assert_eq!(chimi_handle_counts_len_v1(), 0);
            assert!(!chimi_handle_counts_drop_v1(4));
        }
    }
}
