//! The `random` provider (testing.md, Randomness): one stream every carrier
//! draws from, xoshiro256** seeded through SplitMix64 ([`Generator`], which
//! the interpreter uses too, so a seed draws the same numbers on both
//! backends). `--random-seed` fixes the seed (`chimi test` gives each test
//! its own); without it the first draw seeds from the operating system.

use core::ptr;

use super::standard::{defect, success};
use crate::heap::layout::{Control, ControlKind, Runtime, Value, ValueKind};
use crate::heap::values::{integer_value, new_control};
use crate::pthread_locked::PthreadLocked;
use crate::random::Generator;

struct Stream {
    /// The `--random-seed` seed, if one was given.
    seed: Option<u64>,
    /// Seeded by the first draw; `Some` means the program read the stream.
    generator: Option<Generator>,
}

static STREAM: PthreadLocked<Stream> = PthreadLocked::new(Stream { seed: None, generator: None });

/// Fixes the seed (`--random-seed`); the next draw starts the stream from it.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_runtime_random_seed_v1(seed: u64) {
    STREAM.with(|stream| {
        stream.seed = Some(seed);
        stream.generator = None;
    });
}

/// Whether the program drew a number: `chimi test` names the seed in the
/// report of a failing test that did.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_runtime_random_read_v1() -> bool {
    STREAM.with(|stream| stream.generator.is_some())
}

/// A seed from `/dev/urandom`, or from the clock and process id when it
/// cannot be read.
fn os_seed() -> u64 {
    let mut seed = [0u8; 8];
    // SAFETY: the path is NUL-terminated and `seed` is 8 writable bytes.
    let read = unsafe {
        let fd = libc::open(c"/dev/urandom".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
        let count = if fd >= 0 { libc::read(fd, seed.as_mut_ptr().cast(), 8) } else { -1 };
        if fd >= 0 {
            libc::close(fd);
        }
        count == 8
    };
    if read {
        return u64::from_ne_bytes(seed);
    }
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `now` is writable; getpid has no preconditions.
    let pid = unsafe {
        libc::clock_gettime(libc::CLOCK_REALTIME, &mut now);
        libc::getpid()
    };
    let nanos = (now.tv_sec as u64).wrapping_mul(1_000_000_000).wrapping_add(now.tv_nsec as u64);
    nanos ^ ((pid as u64) << 32)
}

/// The next number of the stream, from any thread; the carrier stress
/// harness calls it too.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_runtime_random_next_v1() -> u64 {
    STREAM.with(|stream| {
        let seed = stream.seed;
        stream.generator.get_or_insert_with(|| Generator::seeded(seed.unwrap_or_else(os_seed))).next()
    })
}

/// `random.next_int()`, the runner's provider's one host function.
///
/// # Safety
/// Runs on a fiber that may allocate; `runtime` is live.
#[inline(never)]
pub unsafe fn random_host(runtime: *mut Runtime, arguments: &[*mut Value], call_node: u32) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        if !arguments.is_empty() {
            return defect(c"verified host function has invalid runtime arguments", i64::from(call_node));
        }
        if ptr::read_volatile(&raw const (*runtime).cancel) != 0 {
            return new_control(ControlKind::CANCEL, ptr::null_mut(), ptr::null(), -1);
        }
        success(integer_value(ValueKind::INTEGER, chimi_runtime_random_next_v1() as i64))
    }
}
