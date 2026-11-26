//! The small-block cache (CHI-305 step 5, phase 1): freed blocks of up to
//! `SMALL_BLOCK_LIMIT` bytes kept on per-size free lists and reused, because
//! a message or a pass allocates and frees many small blocks and the system
//! allocator dominated their cost. Each list keeps at most
//! `SMALL_BLOCK_CACHE` blocks, so the cache is bounded. Each carrier has one,
//! in its `Carrier` record in `runtime.c`; the blocks themselves come from
//! and go back to the C library, so a sanitizer still sees every one.

use core::ptr;

pub use super::layout::SmallBlockCache;
use super::layout::{SMALL_BLOCK_CACHE, SMALL_BLOCK_GRAIN, SMALL_BLOCK_LIMIT, SmallBlock};

/// `bytes` rounded up to the grain, and its size class when the cache keeps
/// blocks of that size.
fn class_of(bytes: usize) -> (usize, Option<usize>) {
    let rounded = bytes.div_ceil(SMALL_BLOCK_GRAIN) * SMALL_BLOCK_GRAIN;
    let class = (1..=SMALL_BLOCK_LIMIT).contains(&rounded).then(|| rounded / SMALL_BLOCK_GRAIN - 1);
    (rounded, class)
}

impl SmallBlockCache {
    /// A zeroed block of at least `bytes` bytes, to be returned with
    /// [`SmallBlockCache::give`], or null when the system is out of memory.
    /// `cached_bytes` is the carrier's count of bytes the cache holds; a
    /// block taken from it leaves it. With `enabled` false (sanitizer
    /// builds) every block comes from the C library.
    ///
    /// # Safety
    /// The lists hold only blocks that `give` put there.
    pub unsafe fn take(&mut self, cached_bytes: &mut i64, bytes: usize, enabled: bool) -> *mut u8 {
        let (rounded, class) = class_of(bytes);
        if enabled && let Some(class) = class {
            let block = self.lists[class];
            if !block.is_null() {
                // SAFETY: the caller's contract: a cached block is `rounded`
                // bytes the cache owns.
                unsafe {
                    self.lists[class] = (*block).next;
                    self.counts[class] -= 1;
                    *cached_bytes -= rounded as i64;
                    ptr::write_bytes(block.cast::<u8>(), 0, rounded);
                }
                return block.cast();
            }
        }
        // SAFETY: calloc has no precondition; a null result is passed on.
        unsafe { libc::calloc(1, rounded) }.cast()
    }

    /// Takes back a block of `bytes` bytes that [`SmallBlockCache::take`]
    /// returned: onto its free list while that list has room, else to the C
    /// library.
    ///
    /// # Safety
    /// `block` came from `take` with the same `bytes`, on any carrier, and
    /// nothing uses it any more.
    pub unsafe fn give(&mut self, cached_bytes: &mut i64, block: *mut u8, bytes: usize, enabled: bool) {
        let (rounded, class) = class_of(bytes);
        if enabled
            && let Some(class) = class
            && (self.counts[class] as usize) < SMALL_BLOCK_CACHE
        {
            let block = block.cast::<SmallBlock>();
            // SAFETY: the caller's contract: the block is at least one
            // grain, which holds the link.
            unsafe { (*block).next = self.lists[class] };
            self.lists[class] = block;
            self.counts[class] += 1;
            *cached_bytes += rounded as i64;
            return;
        }
        // SAFETY: the caller's contract: the block came from calloc.
        unsafe { libc::free(block.cast()) };
    }
}

#[cfg(test)]
mod tests {
    use super::super::layout::SMALL_BLOCK_CLASSES as CLASSES;
    use super::*;

    fn empty() -> SmallBlockCache {
        SmallBlockCache { lists: [ptr::null_mut(); CLASSES], counts: [0; CLASSES] }
    }

    #[test]
    fn classes_round_up_to_the_grain_and_stop_at_the_limit() {
        assert_eq!(class_of(1), (16, Some(0)));
        assert_eq!(class_of(16), (16, Some(0)));
        assert_eq!(class_of(17), (32, Some(1)));
        assert_eq!(class_of(256), (256, Some(CLASSES - 1)));
        assert_eq!(class_of(257), (272, None));
    }

    #[test]
    fn a_given_block_comes_back_zeroed_and_the_cached_bytes_balance() {
        let mut cache = empty();
        let mut cached = 0;
        unsafe {
            let block = cache.take(&mut cached, 40, true);
            assert!(!block.is_null());
            ptr::write_bytes(block, 0xab, 48);
            cache.give(&mut cached, block, 40, true);
            assert_eq!((cached, cache.counts[2]), (48, 1));
            let again = cache.take(&mut cached, 33, true);
            assert_eq!(again, block);
            assert!(core::slice::from_raw_parts(again, 48).iter().all(|byte| *byte == 0));
            assert_eq!((cached, cache.counts[2]), (0, 0));
            cache.give(&mut cached, again, 33, false);
            assert_eq!((cached, cache.counts[2]), (0, 0));
        }
    }

    #[test]
    fn with_the_cache_off_a_cached_block_stays_cached() {
        let mut cache = empty();
        let mut cached = 0;
        unsafe {
            let block = cache.take(&mut cached, 16, true);
            cache.give(&mut cached, block, 16, true);
            let fresh = cache.take(&mut cached, 16, false);
            assert_ne!(fresh, block);
            assert_eq!((cached, cache.counts[0]), (16, 1));
            cache.give(&mut cached, fresh, 16, false);
            let again = cache.take(&mut cached, 16, true);
            assert_eq!(again, block);
            cache.give(&mut cached, again, 16, false);
        }
    }

    #[test]
    fn a_full_list_and_a_large_block_go_back_to_the_c_library() {
        let mut cache = empty();
        let mut cached = 0;
        unsafe {
            let large = cache.take(&mut cached, 300, true);
            cache.give(&mut cached, large, 300, true);
            assert_eq!(cached, 0);
            cache.counts[0] = SMALL_BLOCK_CACHE as u32;
            let small = cache.take(&mut cached, 8, true);
            cache.give(&mut cached, small, 8, true);
            assert!(cache.lists[0].is_null());
            assert_eq!(cached, 0);
        }
    }
}
