//! The C view of [`crate::handle_counts`]: how many live values name each
//! native handle. A copy made on one carrier can be freed on another, so
//! every call takes the counts' lock, which used to be `runtime.c`'s
//! `handle_count_lock`. It is taken after the scheduler's kernel lock and the
//! test log lock, and nothing under it retains, drops, parks or runs a hook.
//! A call that runs out of memory returns first and leaves the exit to C, so
//! the lock is never held while the program ends.

use crate::handle_counts::HandleCounts;
use crate::locked::Locked;

static COUNTS: Locked<HandleCounts> = Locked::new(HandleCounts::new());

/// Counts one more live value naming `key`; false, counting nothing, when
/// there is no memory to grow the map.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_handle_counts_retain_v1(key: u64) -> bool {
    COUNTS.with(|counts| counts.retain(key))
}

/// Counts one live value naming `key` fewer: true exactly when it was the
/// last, so the caller owns the handle's close.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_handle_counts_drop_v1(key: u64) -> bool {
    COUNTS.with(|counts| counts.drop(key))
}

/// Forgets every count before an execution starts.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_handle_counts_reset_v1() {
    COUNTS.with(HandleCounts::reset)
}

/// Keys with at least one reference, for the stress fixture.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_handle_counts_len_v1() -> usize {
    COUNTS.with(|counts| counts.len())
}

/// Entries the map has room for, for the stress fixture.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_handle_counts_capacity_v1() -> usize {
    COUNTS.with(|counts| counts.capacity())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The one test of the shared map, so no other test races it.
    #[test]
    fn the_shared_map_counts_and_resets() {
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
