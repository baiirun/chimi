"""Mutants of std/signal (CHI-249): the scheduler's latch and waiters, the
runner's handler and injection option, and the Chimi provider.

Not listed, as equivalent: waking every waiter for a signal instead of the
oldest, and moving a reissued wait to the back. A waiter's reissue always
finds the bit it was woken for, so neither changes what any task receives;
they only add wakes. Also equivalent, so not listed: withdrawing a waiter
its scope refused (its next request or its end withdraws it before any other
task can be woken), collecting the handler's bits after every resume as well
as before each drive (only how soon a running program sees a signal), and the
handler's pipe write (it covers a signal that lands just before the scheduler
blocks, a race no test can force; a signal during the block interrupts the
poll itself).

Run: scripts/mutation/mutate.py scripts/mutation/signal_mutations.py
"""

CRATE = "compiler/chimi"
CARGO_TEST_ARGS = ["--release"]

SIGNAL = "../../runtime/chimi-fiber/src/scheduler/signal.rs"
SCHEDULER = "../../runtime/chimi-fiber/src/scheduler.rs"
RUNTIME = "runtime/native/runtime.c"
STARTUP = "../../runtime/chimi-runtime/src/startup.rs"
SIGNALS = "../../runtime/chimi-runtime/src/intrinsics/signals.rs"
OPERATIONS = "src/operations.rs"
MODULE = "std/signal.chimi"


def signals(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["std_signal"])


def fiber(name, file, edits):
    return dict(
        name=name, file=file, edits=edits,
        command=["cargo", "test", "--quiet", "-p", "chimi-fiber", "--lib", "--",
                 "a_wait_lists_codes"],
        expect=["a_wait_lists_codes"],
    )


MUTATIONS = [
    # The scheduler's latch and waiters.
    signals("a taken signal stays pending", SIGNAL, [
        ("            self.signals.latch &= !(1 << (code - 1));\n", "")]),
    signals("the last listed pending signal wins", SIGNAL, [
        ("        if let Some(code) = codes\n            .iter()\n            .copied()\n            .find(",
         "        if let Some(code) = codes\n            .iter()\n            .copied()\n            .rev()\n            .find(")]),
    signals("the newest waiter is woken first", SIGNAL, [
        ("                .iter_mut()\n                .find(|waiter| waiter.woken_for.is_none() && waiter.mask & bit != 0)",
         "                .iter_mut()\n                .rev()\n                .find(|waiter| waiter.woken_for.is_none() && waiter.mask & bit != 0)")]),
    signals("a new wait takes a bit reserved for a woken waiter", SIGNAL, [
        ("        let available = self.signals.latch & !self.reserved_bits(Some(owner));",
         "        let available = self.signals.latch;")]),
    signals("a latched signal wakes nobody", SIGNAL, [
        ("        self.signals.latch |= bits;\n        self.dispatch_user_signals();", "        self.signals.latch |= bits;")]),
    signals("an ending waiter is never withdrawn", SCHEDULER, [
        ("        self.clear_io_wait(task);\n        self.withdraw_signal_wait(task);\n        self.clear_scopes(task);",
         "        self.clear_io_wait(task);\n        self.clear_scopes(task);")]),
    signals("a waiter does not keep a real-clock program waiting", SIGNAL, [
        ("|| (self.signals.user_word.is_some() && self.has_signal_waiters()))", "|| false)")]),
    signals("injections after main still fire", SIGNAL, [
        ("        for timer in std::mem::take(&mut self.signals.user_timers) {\n            self.timers.cancel(timer);\n        }\n", "")]),
    fiber("an empty wait parks", SIGNAL, [
        ("    if codes.is_empty() || rest.iter()", "    if rest.iter()")]),
    # The runner.
    signals("the handler ignores SIGHUP", SIGNALS, [
        ("const USER_SIGNAL_NUMBERS: [c_int; 3] = [libc::SIGHUP, libc::SIGUSR1, libc::SIGUSR2];",
         "const USER_SIGNAL_NUMBERS: [c_int; 3] = [libc::SIGWINCH, libc::SIGUSR1, libc::SIGUSR2];")]),
    signals("an injected HUP is dropped", STARTUP, [
        ("                        runtime.injected_user_signals[runtime.injected_user_signal_count] = InjectedSignal { code, at };\n                        runtime.injected_user_signal_count += 1;",
         "                        let _ = (code, at);")]),
    signals("an empty wait is not a defect", SIGNALS, [
        ("        if count == 0 {\n            return defect(", "        if false {\n            return defect(")]),
    signals("injected signals run under the real clock", STARTUP, [
        ("if (runtime.injected_signal != 0 || runtime.injected_user_signal_count != 0) && !runtime.virtual_clock {",
         "if runtime.injected_signal != 0 && !runtime.virtual_clock {")]),
    # The provider.
    signals("the provider installs no handlers", MODULE, [
        ("fn host() {\n  host_install()\n", "fn host() {\n")]),
    signals("User1 and User2 swap codes", MODULE, [
        ("    Signal.User1 -> 2\n    Signal.User2 -> 3\n", "    Signal.User1 -> 3\n    Signal.User2 -> 2\n")]),
    signals("the runner closes another need name for signals", OPERATIONS, [
        ('        need: "signals",', '        need: "signal",')]),
]
