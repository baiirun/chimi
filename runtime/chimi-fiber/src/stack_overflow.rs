use crate::{ChimiFiberResult, ChimiFiberState};
use corosensei::stack::{DefaultStack, Stack};
use corosensei::trap::{CoroutineTrapHandler, TrapHandlerRegs};
use std::cell::Cell;
use std::ffi::c_void;
use std::mem;
use std::ptr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

const SIGNAL_STACK_BYTES_MIN: usize = 64 * 1024;

#[derive(Clone, Copy)]
struct ActiveFiber {
    handler: CoroutineTrapHandler<ChimiFiberResult>,
    guard_start: usize,
    guard_end: usize,
}

thread_local! {
    static ACTIVE_FIBER: Cell<Option<ActiveFiber>> = const { Cell::new(None) };
    static SIGNAL_STACK: ThreadSignalStack = ThreadSignalStack::install();
}

struct PreviousHandler {
    disposition: AtomicUsize,
    flags: AtomicI32,
}

impl PreviousHandler {
    const fn new() -> Self {
        Self {
            disposition: AtomicUsize::new(libc::SIG_DFL),
            flags: AtomicI32::new(0),
        }
    }

    fn store(&self, action: &libc::sigaction) {
        self.disposition
            .store(action.sa_sigaction, Ordering::Relaxed);
        self.flags.store(action.sa_flags, Ordering::Relaxed);
    }
}

static PREVIOUS_SEGMENTATION_VIOLATION: PreviousHandler = PreviousHandler::new();
static PREVIOUS_BUS_ERROR: PreviousHandler = PreviousHandler::new();
static PROCESS_HANDLERS_INSTALLED: OnceLock<bool> = OnceLock::new();

pub(super) struct FiberStackOverflow {
    guard_start: usize,
    guard_end: usize,
}

impl FiberStackOverflow {
    pub(super) fn new(stack: &DefaultStack) -> Option<Self> {
        if !*PROCESS_HANDLERS_INSTALLED.get_or_init(install_process_handlers) {
            return None;
        }
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if page_size <= 0 {
            return None;
        }
        let guard_start = stack.limit().get();
        let guard_end = guard_start.checked_add(page_size as usize)?;
        Some(Self {
            guard_start,
            guard_end,
        })
    }

    pub(super) fn enter(
        &self,
        handler: CoroutineTrapHandler<ChimiFiberResult>,
    ) -> Option<ActiveFiberGuard> {
        let signal_stack_ready = SIGNAL_STACK.with(|stack| stack.ready);
        if !signal_stack_ready {
            return None;
        }
        let previous = ACTIVE_FIBER.with(|active| {
            active.replace(Some(ActiveFiber {
                handler,
                guard_start: self.guard_start,
                guard_end: self.guard_end,
            }))
        });
        Some(ActiveFiberGuard { previous })
    }
}

pub(super) struct ActiveFiberGuard {
    previous: Option<ActiveFiber>,
}

impl Drop for ActiveFiberGuard {
    fn drop(&mut self) {
        ACTIVE_FIBER.with(|active| active.set(self.previous));
    }
}

struct ThreadSignalStack {
    mapping: *mut c_void,
    length: usize,
    ready: bool,
}

impl ThreadSignalStack {
    fn install() -> Self {
        let mut current = unsafe { mem::zeroed::<libc::stack_t>() };
        if unsafe { libc::sigaltstack(ptr::null(), &mut current) } != 0 {
            return Self::unavailable();
        }
        if current.ss_flags & libc::SS_DISABLE == 0 {
            return Self {
                mapping: ptr::null_mut(),
                length: 0,
                ready: true,
            };
        }

        let length = libc::SIGSTKSZ.max(SIGNAL_STACK_BYTES_MIN);
        let mapping = unsafe {
            libc::mmap(
                ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        if mapping == libc::MAP_FAILED {
            return Self::unavailable();
        }

        let alternate = libc::stack_t {
            ss_sp: mapping,
            ss_size: length,
            ss_flags: 0,
        };
        if unsafe { libc::sigaltstack(&alternate, ptr::null_mut()) } != 0 {
            unsafe {
                libc::munmap(mapping, length);
            }
            return Self::unavailable();
        }
        Self {
            mapping,
            length,
            ready: true,
        }
    }

    const fn unavailable() -> Self {
        Self {
            mapping: ptr::null_mut(),
            length: 0,
            ready: false,
        }
    }
}

impl Drop for ThreadSignalStack {
    fn drop(&mut self) {
        if self.mapping.is_null() {
            return;
        }
        let mut current = unsafe { mem::zeroed::<libc::stack_t>() };
        if unsafe { libc::sigaltstack(ptr::null(), &mut current) } == 0
            && current.ss_sp == self.mapping
        {
            let disabled = libc::stack_t {
                ss_sp: ptr::null_mut(),
                ss_size: 0,
                ss_flags: libc::SS_DISABLE,
            };
            unsafe {
                libc::sigaltstack(&disabled, ptr::null_mut());
            }
        }
        unsafe {
            libc::munmap(self.mapping, self.length);
        }
    }
}

fn install_process_handlers() -> bool {
    let Some(previous_segv) =
        install_process_handler(libc::SIGSEGV, &PREVIOUS_SEGMENTATION_VIOLATION)
    else {
        return false;
    };
    if install_process_handler(libc::SIGBUS, &PREVIOUS_BUS_ERROR).is_none() {
        unsafe {
            libc::sigaction(libc::SIGSEGV, &previous_segv, ptr::null_mut());
        }
        return false;
    }
    true
}

fn install_process_handler(
    signal: libc::c_int,
    previous: &PreviousHandler,
) -> Option<libc::sigaction> {
    let mut old = unsafe { mem::zeroed::<libc::sigaction>() };
    if unsafe { libc::sigaction(signal, ptr::null(), &mut old) } != 0 {
        return None;
    }
    previous.store(&old);

    let mut action = unsafe { mem::zeroed::<libc::sigaction>() };
    action.sa_sigaction = signal_handler as usize;
    action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
    unsafe {
        libc::sigfillset(&mut action.sa_mask);
    }
    if unsafe { libc::sigaction(signal, &action, ptr::null_mut()) } != 0 {
        return None;
    }
    Some(old)
}

unsafe extern "C" fn signal_handler(
    signal: libc::c_int,
    information: *mut libc::siginfo_t,
    context: *mut c_void,
) {
    let handled = if information.is_null() || context.is_null() {
        false
    } else {
        ACTIVE_FIBER.with(|active| {
            active.get().is_some_and(|fiber| unsafe {
                handle_fiber_guard_fault(fiber, information, context)
            })
        })
    };
    if !handled {
        unsafe {
            chain_previous_handler(signal, information, context);
        }
    }
}

unsafe fn handle_fiber_guard_fault(
    fiber: ActiveFiber,
    information: *mut libc::siginfo_t,
    context: *mut c_void,
) -> bool {
    let stack_pointer = unsafe { interrupted_stack_pointer(context) };
    let fault_address = unsafe { (*information).si_addr() as usize };
    // A fiber may fault for ordinary invalid memory access while its stack
    // pointer is in bounds. Requiring the protected page as well ensures only
    // actual stack exhaustion is converted into Chimi's overflow cause.
    if !fiber.handler.stack_ptr_in_bounds(stack_pointer)
        || !guard_contains(fiber.guard_start, fiber.guard_end, fault_address)
    {
        return false;
    }

    let registers = unsafe {
        fiber.handler.setup_trap_handler(|| ChimiFiberResult {
            state: ChimiFiberState::StackOverflow as u32,
            value: 0,
            defect_code: 0,
        })
    };
    unsafe {
        install_trap_registers(context, registers);
    }
    true
}

fn guard_contains(start: usize, end: usize, address: usize) -> bool {
    start <= address && address < end
}

unsafe fn chain_previous_handler(
    signal: libc::c_int,
    information: *mut libc::siginfo_t,
    context: *mut c_void,
) {
    let previous = if signal == libc::SIGBUS {
        &PREVIOUS_BUS_ERROR
    } else {
        &PREVIOUS_SEGMENTATION_VIOLATION
    };
    let disposition = previous.disposition.load(Ordering::Relaxed);
    // The handlers are process-global, so faults outside an active Chimi guard
    // must preserve the host disposition rather than becoming runtime defects.
    if disposition == libc::SIG_IGN {
        return;
    }
    if disposition == libc::SIG_DFL {
        let mut action = unsafe { mem::zeroed::<libc::sigaction>() };
        action.sa_sigaction = libc::SIG_DFL;
        unsafe {
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signal, &action, ptr::null_mut());
            libc::raise(signal);
            libc::_exit(128 + signal);
        }
    }

    if previous.flags.load(Ordering::Relaxed) & libc::SA_SIGINFO != 0 {
        let handler: unsafe extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut c_void) =
            unsafe { mem::transmute(disposition) };
        unsafe {
            handler(signal, information, context);
        }
    } else {
        let handler: unsafe extern "C" fn(libc::c_int) = unsafe { mem::transmute(disposition) };
        unsafe {
            handler(signal);
        }
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
unsafe fn interrupted_stack_pointer(context: *mut c_void) -> usize {
    let context = unsafe { &mut *context.cast::<libc::ucontext_t>() };
    unsafe { (*context.uc_mcontext).__ss.__sp as usize }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
unsafe fn install_trap_registers(context: *mut c_void, registers: TrapHandlerRegs) {
    let context = unsafe { &mut *context.cast::<libc::ucontext_t>() };
    let TrapHandlerRegs {
        pc,
        sp,
        x0,
        x1,
        x29,
        lr,
    } = registers;
    unsafe {
        (*context.uc_mcontext).__ss.__pc = pc;
        (*context.uc_mcontext).__ss.__sp = sp;
        (*context.uc_mcontext).__ss.__x[0] = x0;
        (*context.uc_mcontext).__ss.__x[1] = x1;
        (*context.uc_mcontext).__ss.__fp = x29;
        (*context.uc_mcontext).__ss.__lr = lr;
    }
}

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
unsafe fn interrupted_stack_pointer(context: *mut c_void) -> usize {
    let context = unsafe { &mut *context.cast::<libc::ucontext_t>() };
    unsafe { (*context.uc_mcontext).__ss.__rsp as usize }
}

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
unsafe fn install_trap_registers(context: *mut c_void, registers: TrapHandlerRegs) {
    let context = unsafe { &mut *context.cast::<libc::ucontext_t>() };
    let TrapHandlerRegs {
        rip,
        rsp,
        rbp,
        rdi,
        rsi,
    } = registers;
    unsafe {
        (*context.uc_mcontext).__ss.__rip = rip;
        (*context.uc_mcontext).__ss.__rsp = rsp;
        (*context.uc_mcontext).__ss.__rbp = rbp;
        (*context.uc_mcontext).__ss.__rdi = rdi;
        (*context.uc_mcontext).__ss.__rsi = rsi;
    }
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
unsafe fn interrupted_stack_pointer(context: *mut c_void) -> usize {
    let context = unsafe { &mut *context.cast::<libc::ucontext_t>() };
    context.uc_mcontext.sp as usize
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
unsafe fn install_trap_registers(context: *mut c_void, registers: TrapHandlerRegs) {
    let context = unsafe { &mut *context.cast::<libc::ucontext_t>() };
    let TrapHandlerRegs {
        pc,
        sp,
        x0,
        x1,
        x29,
        lr,
    } = registers;
    context.uc_mcontext.pc = pc;
    context.uc_mcontext.sp = sp;
    context.uc_mcontext.regs[0] = x0;
    context.uc_mcontext.regs[1] = x1;
    context.uc_mcontext.regs[29] = x29;
    context.uc_mcontext.regs[30] = lr;
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
unsafe fn interrupted_stack_pointer(context: *mut c_void) -> usize {
    let context = unsafe { &mut *context.cast::<libc::ucontext_t>() };
    context.uc_mcontext.gregs[libc::REG_RSP as usize] as usize
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
unsafe fn install_trap_registers(context: *mut c_void, registers: TrapHandlerRegs) {
    let context = unsafe { &mut *context.cast::<libc::ucontext_t>() };
    let TrapHandlerRegs {
        rip,
        rsp,
        rbp,
        rdi,
        rsi,
    } = registers;
    context.uc_mcontext.gregs[libc::REG_RIP as usize] = rip as libc::greg_t;
    context.uc_mcontext.gregs[libc::REG_RSP as usize] = rsp as libc::greg_t;
    context.uc_mcontext.gregs[libc::REG_RBP as usize] = rbp as libc::greg_t;
    context.uc_mcontext.gregs[libc::REG_RDI as usize] = rdi as libc::greg_t;
    context.uc_mcontext.gregs[libc::REG_RSI as usize] = rsi as libc::greg_t;
}

#[cfg(test)]
mod tests {
    use super::guard_contains;

    #[test]
    fn guard_bounds_reject_unrelated_fault_addresses() {
        assert!(!guard_contains(4096, 8192, 4095));
        assert!(guard_contains(4096, 8192, 4096));
        assert!(guard_contains(4096, 8192, 8191));
        assert!(!guard_contains(4096, 8192, 8192));
    }
}
