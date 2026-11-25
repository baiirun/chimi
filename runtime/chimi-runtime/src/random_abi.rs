//! The C view of [`crate::random`]. A generator crosses as its four-word
//! state, which `runtime.c` keeps and guards with its own lock.

use crate::random::Generator;

/// The generator a 64-bit seed names.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_random_seeded_v1(seed: u64) -> Generator {
    Generator::seeded(seed)
}

/// The next 64 bits from `generator`, which advances.
///
/// # Safety
///
/// `generator` must point to a generator that `chimi_random_seeded_v1` made,
/// and no other thread may use it during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_random_next_v1(generator: *mut Generator) -> u64 {
    // SAFETY: the caller guarantees a valid, exclusively held generator.
    unsafe { (*generator).next() }
}
