//! Runtime-private guarded fibers behind the native generated-code ABI.

use backtrace::Backtrace;
use corosensei::stack::DefaultStack;
use corosensei::{Coroutine, CoroutineResult, Yielder};
use std::cell::Cell;
use std::ffi::{c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

mod channel;
pub mod scheduler;
#[cfg(all(
    unix,
    any(target_os = "macos", target_os = "linux"),
    any(target_arch = "aarch64", target_arch = "x86_64")
))]
mod stack_overflow;

pub const ABI_VERSION: u32 = 31;
pub const MIN_STACK_BYTES: usize = 32 * 1024;
pub const MAX_STACK_BYTES: usize = 8 * 1024 * 1024;
const PANIC_DEFECT_CODE: i64 = -1;

#[cfg(test)]
pub(crate) static FIXTURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChimiFiberCommand {
    Resume = 1,
    Cancel = 2,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChimiFiberState {
    Parked = 1,
    Succeeded = 2,
    Cancelled = 3,
    Defected = 4,
    StackOverflow = 5,
}

#[repr(C)]
pub struct ChimiRootFrame {
    pub previous: *const ChimiRootFrame,
    pub slots: *mut usize,
    pub length: usize,
    pub function_name: *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ChimiFiberEvent {
    pub state: u32,
    pub value: i64,
    pub defect_code: i64,
    pub roots: *const ChimiRootFrame,
    pub request: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ChimiFiberResult {
    pub state: u32,
    pub value: i64,
    pub defect_code: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ChimiFiberConfig {
    pub stack_bytes: usize,
    pub capture_backtraces: u32,
}

pub type ChimiFiberEntry = unsafe extern "C" fn(*mut c_void) -> ChimiFiberResult;
type Entry = ChimiFiberEntry;
type RootVisitor = unsafe extern "C" fn(*mut c_void, *mut usize);
type Fiber = Coroutine<ChimiFiberCommand, ParkSignal, ChimiFiberResult, DefaultStack>;

struct ParkSignal {
    roots: *const ChimiRootFrame,
    request: *mut c_void,
    backtrace: String,
}

thread_local! {
    static ACTIVE_YIELDER: Cell<*const Yielder<ChimiFiberCommand, ParkSignal>> =
        const { Cell::new(ptr::null()) };
    static CAPTURE_BACKTRACE: Cell<bool> = const { Cell::new(false) };
}

#[repr(C)]
pub struct ChimiFiber {
    fiber: Option<Fiber>,
    #[cfg(all(
        unix,
        any(target_os = "macos", target_os = "linux"),
        any(target_arch = "aarch64", target_arch = "x86_64")
    ))]
    stack_overflow: stack_overflow::FiberStackOverflow,
    event: ChimiFiberEvent,
    backtrace: String,
    started: bool,
}

impl ChimiFiber {
    fn resume(&mut self, command: ChimiFiberCommand) -> ChimiFiberEvent {
        if self.event.state != 0 && self.event.state != ChimiFiberState::Parked as u32 {
            return self.event;
        }
        if !self.started && command == ChimiFiberCommand::Cancel {
            self.fiber.take();
            self.event = ChimiFiberEvent {
                state: ChimiFiberState::Cancelled as u32,
                value: 0,
                defect_code: 0,
                roots: ptr::null(),
                request: ptr::null_mut(),
            };
            return self.event;
        }
        self.started = true;
        #[cfg(all(
            unix,
            any(target_os = "macos", target_os = "linux"),
            any(target_arch = "aarch64", target_arch = "x86_64")
        ))]
        let Some(_stack_overflow) = self.stack_overflow.enter(
            self.fiber
                .as_ref()
                .expect("non-terminal fiber owns its coroutine")
                .trap_handler(),
        ) else {
            self.fiber.take();
            self.event = defect_event(PANIC_DEFECT_CODE);
            return self.event;
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            self.fiber
                .as_mut()
                .expect("non-terminal fiber owns its coroutine")
                .resume(command)
        }));
        self.event = match outcome {
            Ok(CoroutineResult::Yield(signal)) => {
                self.backtrace = signal.backtrace;
                ChimiFiberEvent {
                    state: ChimiFiberState::Parked as u32,
                    value: 0,
                    defect_code: 0,
                    roots: signal.roots,
                    request: signal.request,
                }
            }
            Ok(CoroutineResult::Return(result)) => {
                self.fiber.take();
                ChimiFiberEvent {
                    state: result.state,
                    value: result.value,
                    defect_code: result.defect_code,
                    roots: ptr::null(),
                    request: ptr::null_mut(),
                }
            }
            Err(_) => {
                self.fiber.take();
                defect_event(PANIC_DEFECT_CODE)
            }
        };
        self.event
    }
}

impl Drop for ChimiFiber {
    fn drop(&mut self) {
        if self.event.state == ChimiFiberState::Parked as u32 {
            let _ = self.resume(ChimiFiberCommand::Cancel);
        }
    }
}

fn defect_event(code: i64) -> ChimiFiberEvent {
    ChimiFiberEvent {
        state: ChimiFiberState::Defected as u32,
        value: 0,
        defect_code: code,
        roots: ptr::null(),
        request: ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn chimi_fiber_abi_version() -> u32 {
    ABI_VERSION
}

#[unsafe(no_mangle)]
/// Creates a guarded fiber around a generated C entry.
///
/// # Safety
///
/// `entry` must use the declared ABI, must not unwind, and must keep every root
/// frame published to `chimi_fiber_park` alive until that call returns.
pub unsafe extern "C" fn chimi_fiber_create(
    entry: Option<Entry>,
    context: *mut c_void,
    config: ChimiFiberConfig,
) -> *mut ChimiFiber {
    let Some(entry) = entry else {
        return ptr::null_mut();
    };
    if !(MIN_STACK_BYTES..=MAX_STACK_BYTES).contains(&config.stack_bytes) {
        return ptr::null_mut();
    }
    let Ok(stack) = DefaultStack::new(config.stack_bytes) else {
        return ptr::null_mut();
    };
    #[cfg(all(
        unix,
        any(target_os = "macos", target_os = "linux"),
        any(target_arch = "aarch64", target_arch = "x86_64")
    ))]
    let Some(stack_overflow) = stack_overflow::FiberStackOverflow::new(&stack) else {
        return ptr::null_mut();
    };
    let capture_backtraces = config.capture_backtraces != 0;
    let fiber = Coroutine::with_stack(stack, move |yielder, command| {
        debug_assert_eq!(command, ChimiFiberCommand::Resume);
        set_park_context(yielder, capture_backtraces);
        // SAFETY: create requires a generated entry with the declared C ABI.
        let result = unsafe { entry(context) };
        set_park_context(ptr::null(), false);
        result
    });
    Box::into_raw(Box::new(ChimiFiber {
        fiber: Some(fiber),
        #[cfg(all(
            unix,
            any(target_os = "macos", target_os = "linux"),
            any(target_arch = "aarch64", target_arch = "x86_64")
        ))]
        stack_overflow,
        event: ChimiFiberEvent {
            state: 0,
            value: 0,
            defect_code: 0,
            roots: ptr::null(),
            request: ptr::null_mut(),
        },
        backtrace: String::new(),
        started: false,
    }))
}

#[unsafe(no_mangle)]
/// Advances a fiber by one scheduler command.
///
/// # Safety
///
/// `fiber` must be null or an exclusively borrowed pointer returned by
/// `chimi_fiber_create` that has not been destroyed.
pub unsafe extern "C" fn chimi_fiber_resume(
    fiber: *mut ChimiFiber,
    command: u32,
) -> ChimiFiberEvent {
    let Some(fiber) = (unsafe { fiber.as_mut() }) else {
        return defect_event(PANIC_DEFECT_CODE);
    };
    let command = match command {
        value if value == ChimiFiberCommand::Resume as u32 => ChimiFiberCommand::Resume,
        value if value == ChimiFiberCommand::Cancel as u32 => ChimiFiberCommand::Cancel,
        _ => return defect_event(PANIC_DEFECT_CODE),
    };
    fiber.resume(command)
}

#[unsafe(no_mangle)]
/// Suspends the active generated call stack and publishes its exact roots.
///
/// # Safety
///
/// This may only be called by generated code running in the active Chimi
/// fiber. `roots` and its linked frames and slots must remain valid and
/// writable until the function returns.
pub unsafe extern "C" fn chimi_fiber_park(roots: *const ChimiRootFrame) -> ChimiFiberCommand {
    // Not a tail call, so a parked backtrace keeps this frame.
    std::hint::black_box(unsafe { chimi_fiber_request(ptr::null_mut(), roots) })
}

#[unsafe(no_mangle)]
/// Suspends the active generated call stack with a scheduler request.
///
/// # Safety
///
/// `request` must remain valid and exclusively accessible until this call
/// returns. Root-frame requirements are the same as `chimi_fiber_park`.
pub unsafe extern "C" fn chimi_fiber_request(
    request: *mut c_void,
    roots: *const ChimiRootFrame,
) -> ChimiFiberCommand {
    let (yielder, capture_backtraces) = park_context();
    assert!(
        !yielder.is_null(),
        "park called outside an active Chimi fiber"
    );
    let backtrace = if capture_backtraces {
        format!("{:?}", Backtrace::new())
    } else {
        String::new()
    };
    // SAFETY: the yielder is installed for exactly the active fiber call.
    let command = unsafe {
        (&*yielder).suspend(ParkSignal {
            roots,
            request,
            backtrace,
        })
    };
    // Another fiber may have run while this one was suspended and replaced
    // the thread-local execution context, and the fiber may now run on
    // another thread. Restore this fiber's context on the thread it resumed
    // on before generated code can park again.
    set_park_context(yielder, capture_backtraces);
    command
}

// The park context is read and written only through these two functions,
// never inlined, so each call finds the thread-locals of the thread it runs
// on. A fiber may suspend on one thread and resume on another, and a
// compiler may keep a thread-local's address from before a call that
// suspends (parallel-scheduler.md, "Runtime state by owner").
#[inline(never)]
fn park_context() -> (*const Yielder<ChimiFiberCommand, ParkSignal>, bool) {
    (ACTIVE_YIELDER.with(Cell::get), CAPTURE_BACKTRACE.with(Cell::get))
}

#[inline(never)]
fn set_park_context(yielder: *const Yielder<ChimiFiberCommand, ParkSignal>, capture_backtraces: bool) {
    ACTIVE_YIELDER.with(|slot| slot.set(yielder));
    CAPTURE_BACKTRACE.with(|slot| slot.set(capture_backtraces));
}

#[unsafe(no_mangle)]
/// Visits each exact live root slot in a parked fiber.
///
/// # Safety
///
/// `fiber` must be null or a live pointer returned by `chimi_fiber_create`.
/// The callback and context must remain valid for the call and the callback
/// must not resume or destroy the visited fiber.
pub unsafe extern "C" fn chimi_fiber_visit_roots(
    fiber: *const ChimiFiber,
    visitor: Option<RootVisitor>,
    context: *mut c_void,
) {
    let (Some(fiber), Some(visitor)) = (unsafe { fiber.as_ref() }, visitor) else {
        return;
    };
    if fiber.event.state != ChimiFiberState::Parked as u32 {
        return;
    }
    let mut frame = fiber.event.roots;
    while let Some(current) = unsafe { frame.as_ref() } {
        for index in 0..current.length {
            // SAFETY: generated root frames remain live for the entire park.
            unsafe { visitor(context, current.slots.add(index)) };
        }
        frame = current.previous;
    }
}

#[unsafe(no_mangle)]
/// Copies the most recently captured parked backtrace into caller storage.
///
/// # Safety
///
/// `fiber` must be null or live. When `capacity` is nonzero, `output` must
/// point to at least `capacity` writable bytes.
pub unsafe extern "C" fn chimi_fiber_backtrace(
    fiber: *const ChimiFiber,
    output: *mut c_char,
    capacity: usize,
) -> usize {
    let Some(fiber) = (unsafe { fiber.as_ref() }) else {
        return 0;
    };
    let bytes = fiber.backtrace.as_bytes();
    if capacity != 0 && !output.is_null() {
        let written = bytes.len().min(capacity - 1);
        // SAFETY: the caller supplies writable storage of `capacity` bytes.
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), output.cast(), written);
            *output.add(written) = 0;
        }
    }
    bytes.len()
}

#[unsafe(no_mangle)]
/// Cancels a parked fiber if necessary and releases it.
///
/// # Safety
///
/// `fiber` must be null or a pointer returned by `chimi_fiber_create` whose
/// ownership has not already been transferred to this function.
pub unsafe extern "C" fn chimi_fiber_destroy(fiber: *mut ChimiFiber) {
    if !fiber.is_null() {
        // SAFETY: ownership is transferred exactly once by the C ABI.
        drop(unsafe { Box::from_raw(fiber) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    unsafe extern "C" {
        fn chimi_fixture_generated_entry(context: *mut c_void) -> ChimiFiberResult;
        fn chimi_fixture_generated_defect(context: *mut c_void) -> ChimiFiberResult;
        fn chimi_fixture_generated_overflow(context: *mut c_void) -> ChimiFiberResult;
        fn chimi_fixture_reset_cleanup();
        fn chimi_fixture_cleanup_count() -> i32;
    }

    unsafe extern "C" fn collect_root(context: *mut c_void, slot: *mut usize) {
        let roots = unsafe { &mut *context.cast::<Vec<usize>>() };
        roots.push(unsafe { *slot });
    }

    unsafe extern "C" fn relocate_root(_context: *mut c_void, slot: *mut usize) {
        unsafe { *slot += 1 };
    }

    fn config(backtrace: bool) -> ChimiFiberConfig {
        ChimiFiberConfig {
            stack_bytes: 64 * 1024,
            capture_backtraces: u32::from(backtrace),
        }
    }

    #[test]
    fn generated_nested_calls_park_visit_roots_and_resume_with_locals_intact() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        unsafe {
            chimi_fixture_reset_cleanup();
            let fiber = chimi_fiber_create(
                Some(chimi_fixture_generated_entry),
                ptr::null_mut(),
                config(true),
            );
            assert!(!fiber.is_null());
            let parked = chimi_fiber_resume(fiber, ChimiFiberCommand::Resume as u32);
            assert_eq!(parked.state, ChimiFiberState::Parked as u32);

            let mut roots = Vec::new();
            chimi_fiber_visit_roots(
                fiber,
                Some(collect_root),
                (&mut roots as *mut Vec<usize>).cast(),
            );
            assert_eq!(roots, [42_usize]);
            chimi_fiber_visit_roots(fiber, Some(relocate_root), ptr::null_mut());

            let length = chimi_fiber_backtrace(fiber, ptr::null_mut(), 0);
            let mut backtrace = vec![0_i8; length + 1];
            chimi_fiber_backtrace(fiber, backtrace.as_mut_ptr(), backtrace.len());
            let backtrace = std::ffi::CStr::from_ptr(backtrace.as_ptr()).to_string_lossy();
            assert!(backtrace.contains("chimi_fixture_nested_call"));
            assert!(backtrace.contains("chimi_fixture_generated_entry"));
            assert!(backtrace.contains("chimi_fiber_park"));

            let completed = chimi_fiber_resume(fiber, ChimiFiberCommand::Resume as u32);
            assert_eq!(completed.state, ChimiFiberState::Succeeded as u32);
            assert_eq!(completed.value, 43);
            assert_eq!(chimi_fixture_cleanup_count(), 1);
            chimi_fiber_destroy(fiber);
        }
    }

    #[test]
    fn cancellation_and_defects_cross_generated_c_as_explicit_outcomes() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        unsafe {
            chimi_fixture_reset_cleanup();
            let fiber = chimi_fiber_create(
                Some(chimi_fixture_generated_entry),
                ptr::null_mut(),
                config(false),
            );
            assert_eq!(
                chimi_fiber_resume(fiber, ChimiFiberCommand::Resume as u32).state,
                ChimiFiberState::Parked as u32
            );
            assert_eq!(chimi_fiber_backtrace(fiber, ptr::null_mut(), 0), 0);
            let cancelled = chimi_fiber_resume(fiber, ChimiFiberCommand::Cancel as u32);
            assert_eq!(cancelled.state, ChimiFiberState::Cancelled as u32);
            assert_eq!(chimi_fixture_cleanup_count(), 1);
            chimi_fiber_destroy(fiber);

            let defect = chimi_fiber_create(
                Some(chimi_fixture_generated_defect),
                ptr::null_mut(),
                config(false),
            );
            let defected = chimi_fiber_resume(defect, ChimiFiberCommand::Resume as u32);
            assert_eq!(defected.state, ChimiFiberState::Defected as u32);
            assert_eq!(defected.defect_code, 77);
            chimi_fiber_destroy(defect);
        }
    }

    #[test]
    fn generated_stack_overflow_is_an_explicit_causal_defect() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        unsafe {
            let fiber = chimi_fiber_create(
                Some(chimi_fixture_generated_overflow),
                ptr::null_mut(),
                ChimiFiberConfig {
                    stack_bytes: MIN_STACK_BYTES,
                    capture_backtraces: 0,
                },
            );
            assert!(!fiber.is_null());
            let overflow = chimi_fiber_resume(fiber, ChimiFiberCommand::Resume as u32);
            assert_eq!(overflow.state, ChimiFiberState::StackOverflow as u32);
            assert_eq!(overflow.value, 0);
            assert_eq!(overflow.defect_code, 0);
            assert!(overflow.roots.is_null());
            chimi_fiber_destroy(fiber);
        }
    }

    /// Parks three times, then succeeds with 7.
    unsafe extern "C" fn parks_three_times(_context: *mut c_void) -> ChimiFiberResult {
        for _ in 0..3 {
            unsafe { chimi_fiber_request(ptr::null_mut(), ptr::null()) };
        }
        ChimiFiberResult {
            state: ChimiFiberState::Succeeded as u32,
            value: 7,
            defect_code: 0,
        }
    }

    struct SendFiber(*mut ChimiFiber);
    // SAFETY: the test hands the fiber to one thread at a time and joins it
    // before the next resume, as the kernel does between carriers.
    unsafe impl Send for SendFiber {}

    /// A thread that resumes each fiber it is handed and sends back its
    /// state. Two of them live at once, so their thread-locals are distinct.
    fn resumer() -> (
        std::sync::mpsc::Sender<SendFiber>,
        std::sync::mpsc::Receiver<(SendFiber, u32)>,
        std::thread::JoinHandle<()>,
    ) {
        let (hand, take) = std::sync::mpsc::channel::<SendFiber>();
        let (answer, answers) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            for fiber in take {
                let event = unsafe { chimi_fiber_resume(fiber.0, ChimiFiberCommand::Resume as u32) };
                answer.send((fiber, event.state)).expect("answer");
            }
        });
        (hand, answers, thread)
    }

    /// A parked fiber resumed on another live thread parks again and
    /// finishes there: what a carrier that steals a parked task relies on
    /// (parallel-scheduler.md, step 4 PR 4b).
    #[test]
    fn a_parked_fiber_moves_between_threads() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let threads = [resumer(), resumer()];
        let mut fiber = SendFiber(unsafe {
            chimi_fiber_create(Some(parks_three_times), ptr::null_mut(), config(false))
        });
        assert!(!fiber.0.is_null());
        let expected = [ChimiFiberState::Parked; 3].into_iter().chain([ChimiFiberState::Succeeded]);
        for (resume, expected) in expected.enumerate() {
            let (hand, answers, _) = &threads[resume % 2];
            hand.send(fiber).expect("hand the fiber over");
            let (moved, state) = answers.recv().expect("resumed");
            fiber = moved;
            assert_eq!(state, expected as u32, "resume {resume}, on thread {}", resume % 2);
        }
        unsafe { chimi_fiber_destroy(fiber.0) };
        for (hand, _, thread) in threads {
            drop(hand);
            thread.join().expect("resumer");
        }
    }

    #[test]
    fn abi_and_stack_bounds_are_explicit_and_creation_cost_is_bounded() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        assert_eq!(chimi_fiber_abi_version(), ABI_VERSION);
        assert_eq!(size_of::<ChimiFiberCommand>(), size_of::<u32>());
        assert_eq!(size_of::<ChimiFiberResult>(), 24);
        unsafe {
            assert!(chimi_fiber_create(None, ptr::null_mut(), config(false)).is_null());
            let invalid = ChimiFiberConfig {
                stack_bytes: MIN_STACK_BYTES - 1,
                capture_backtraces: 0,
            };
            assert!(
                chimi_fiber_create(
                    Some(chimi_fixture_generated_entry),
                    ptr::null_mut(),
                    invalid
                )
                .is_null()
            );
        }

        let started = Instant::now();
        for _ in 0..1_000 {
            unsafe {
                let fiber = chimi_fiber_create(
                    Some(chimi_fixture_generated_defect),
                    ptr::null_mut(),
                    config(false),
                );
                assert!(!fiber.is_null());
                chimi_fiber_resume(fiber, ChimiFiberCommand::Resume as u32);
                chimi_fiber_destroy(fiber);
            }
        }
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
