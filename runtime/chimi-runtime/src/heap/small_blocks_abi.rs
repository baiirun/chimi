//! The C view of [`super::small_blocks`]: `runtime.c`'s `small_block_take`
//! and `small_block_give` call these with the running carrier's cache.

use super::small_blocks::SmallBlockCache;

/// [`SmallBlockCache::take`]; null when the system is out of memory.
///
/// # Safety
/// `cache` and `cached_bytes` are the running carrier's, as for `take`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_small_block_take_v1(
    cache: *mut SmallBlockCache,
    cached_bytes: *mut i64,
    bytes: usize,
    enabled: bool,
) -> *mut u8 {
    // SAFETY: the caller's contract.
    unsafe { (*cache).take(&mut *cached_bytes, bytes, enabled) }
}

/// [`SmallBlockCache::give`].
///
/// # Safety
/// As for `give`, with the running carrier's cache and count.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_small_block_give_v1(
    cache: *mut SmallBlockCache,
    cached_bytes: *mut i64,
    block: *mut u8,
    bytes: usize,
    enabled: bool,
) {
    // SAFETY: the caller's contract.
    unsafe { (*cache).give(&mut *cached_bytes, block, bytes, enabled) }
}
