use backtrace::Backtrace;
use corosensei::stack::DefaultStack;
use corosensei::{Coroutine, CoroutineResult, Yielder};
use std::cell::Cell;
use std::hint::black_box;
use std::io::{self, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const STACK_BYTES: usize = 64 * 1024;
const CANCELLED_CODE: i64 = i64::MIN;
const DEFECT_CODE: i64 = i64::MIN + 1;

#[repr(C)]
struct RootFrame {
    slots: *const i64,
    length: usize,
}

#[derive(Clone, Copy, Debug)]
enum BridgeCommand {
    Start,
    Wake,
    Cancel,
}

#[derive(Debug)]
enum BridgeSignal {
    Parked { frame: usize, backtrace: String },
}

#[derive(Debug)]
enum BridgeOutcome {
    Succeeded(i64),
    Cancelled,
    Defected(i64),
}

impl BridgeOutcome {
    fn label(&self) -> String {
        match self {
            Self::Succeeded(value) => format!("Succeeded({value})"),
            Self::Cancelled => "Cancelled".into(),
            Self::Defected(code) => format!("Defected({code})"),
        }
    }
}

thread_local! {
    static BRIDGE_YIELDER: Cell<*const Yielder<BridgeCommand, BridgeSignal>> =
        const { Cell::new(std::ptr::null()) };
}

unsafe extern "C" {
    fn chimi_prototype_generated_child() -> i64;
    fn chimi_prototype_generated_defect() -> i64;
}

#[unsafe(no_mangle)]
extern "C" fn chimi_prototype_park(frame: *const RootFrame) -> i64 {
    let backtrace = format!("{:?}", Backtrace::new());
    let command = BRIDGE_YIELDER.with(|slot| {
        let yielder = slot.get();
        assert!(!yielder.is_null(), "generated child parked outside a fiber");
        unsafe {
            (&*yielder).suspend(BridgeSignal::Parked {
                frame: frame as usize,
                backtrace,
            })
        }
    });
    match command {
        BridgeCommand::Wake => 0,
        BridgeCommand::Cancel => CANCELLED_CODE,
        BridgeCommand::Start => unreachable!("start is only the first resume command"),
    }
}

#[derive(Clone, Copy, Debug)]
enum Signal {
    Parked(&'static str),
}

#[derive(Clone, Debug)]
enum Status {
    NotStarted,
    Runnable,
    Parked(&'static str),
    Succeeded(i64),
    Cancelled,
    Defected(String),
}

struct CleanupFlag(Arc<AtomicBool>);

impl Drop for CleanupFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

type Fiber = Coroutine<(), Signal, i64, DefaultStack>;

struct Runtime {
    status: Status,
    fiber: Option<Fiber>,
    roots: Arc<Mutex<Vec<i64>>>,
    cleaned: Arc<AtomicBool>,
    events: Vec<String>,
}

impl Runtime {
    fn new() -> Self {
        Self {
            status: Status::NotStarted,
            fiber: None,
            roots: Arc::new(Mutex::new(Vec::new())),
            cleaned: Arc::new(AtomicBool::new(false)),
            events: vec!["runtime initialized".into()],
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    fn start_or_wake(&mut self) {
        if matches!(self.status, Status::NotStarted) {
            self.install_normal_child();
            self.status = Status::Runnable;
            self.events
                .push("Task.start transferred child thunk".into());
        }
        if !matches!(self.status, Status::Runnable | Status::Parked(_)) {
            self.events.push("terminal child cannot resume".into());
            return;
        }
        self.status = Status::Runnable;
        self.events.push("scheduler resumed child fiber".into());
        self.resume_child();
    }

    fn cancel(&mut self) {
        if matches!(self.status, Status::NotStarted) {
            self.start_or_wake();
        }
        if matches!(self.status, Status::Parked(_)) {
            self.events.push("Task.cancel began child unwind".into());
            self.fiber.take();
            self.roots.lock().expect("root registry").clear();
            self.status = Status::Cancelled;
            self.events
                .push("child cleanup drained before cancel returned".into());
        }
    }

    fn defect(&mut self) {
        self.reset();
        let cleaned = self.cleaned.clone();
        let stack = DefaultStack::new(STACK_BYTES).expect("guarded fiber stack");
        self.fiber = Some(Coroutine::with_stack(stack, move |_, ()| {
            let _cleanup = CleanupFlag(cleaned);
            panic!("prototype child defect")
        }));
        self.status = Status::Runnable;
        self.events.push("defecting child started".into());

        let old_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            self.fiber.as_mut().expect("installed fiber").resume(())
        }));
        std::panic::set_hook(old_hook);
        match outcome {
            Err(payload) => {
                let cause = payload
                    .downcast_ref::<&str>()
                    .copied()
                    .unwrap_or("unknown defect");
                self.fiber.take();
                self.status = Status::Defected(cause.into());
                self.events
                    .push("defect unwound the fiber and propagated to its owner".into());
            }
            Ok(_) => unreachable!("defecting child must unwind"),
        }
    }

    fn install_normal_child(&mut self) {
        let roots = self.roots.clone();
        let cleaned = self.cleaned.clone();
        let stack = DefaultStack::new(STACK_BYTES).expect("guarded fiber stack");
        self.fiber = Some(Coroutine::with_stack(stack, move |yielder, ()| {
            let _cleanup = CleanupFlag(cleaned);
            nested_call(yielder, roots, 40)
        }));
    }

    fn resume_child(&mut self) {
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            self.fiber.as_mut().expect("installed child").resume(())
        }));
        match outcome {
            Ok(CoroutineResult::Yield(Signal::Parked(reason))) => {
                self.status = Status::Parked(reason);
                self.events.push(format!(
                    "nested call parked; scheduler sees {} shadow root(s)",
                    self.roots.lock().expect("root registry").len()
                ));
            }
            Ok(CoroutineResult::Return(value)) => {
                self.fiber.take();
                self.status = Status::Succeeded(value);
                self.events
                    .push("nested call resumed with locals intact and returned 42".into());
            }
            Err(_) => {
                self.fiber.take();
                self.status = Status::Defected("unexpected unwind".into());
            }
        }
    }
}

fn nested_call(yielder: &Yielder<(), Signal>, roots: Arc<Mutex<Vec<i64>>>, value: i64) -> i64 {
    let local = value + 2;
    roots.lock().expect("root registry").push(local);
    yielder.suspend(Signal::Parked("Channel.receive"));
    roots
        .lock()
        .expect("root registry")
        .pop()
        .expect("live root")
}

fn render(runtime: &Runtime) {
    print!("\x1b[2J\x1b[H");
    println!("\x1b[1mChimi stackful Task runtime prototype\x1b[0m");
    println!("\x1b[2mguarded stack: {STACK_BYTES} bytes; fiber is runtime-private\x1b[0m\n");
    println!("\x1b[1mTask\x1b[0m");
    match &runtime.status {
        Status::Parked(reason) => println!("  status: parked on {reason}"),
        Status::Succeeded(value) => println!("  status: succeeded({value})"),
        Status::Defected(cause) => println!("  status: defected({cause})"),
        other => println!("  status: {}", format!("{other:?}").to_lowercase()),
    }
    println!(
        "  cleanup completed: {}",
        runtime.cleaned.load(Ordering::SeqCst)
    );
    println!(
        "  shadow roots: {:?}",
        runtime.roots.lock().expect("root registry")
    );
    println!("\n\x1b[1mEvents\x1b[0m");
    for event in &runtime.events {
        println!("  {event}");
    }
    println!(
        "\n[n] start/wake  [c] cancel  [d] defect  [f] generated C bridge  [b] benchmark  [r] reset  [q] quit"
    );
}

fn run_all() {
    let mut runtime = Runtime::new();
    for label in ["start", "wake"] {
        runtime.start_or_wake();
        println!(
            "{label}: status={:?}, roots={:?}, cleaned={}",
            runtime.status,
            runtime.roots.lock().expect("root registry"),
            runtime.cleaned.load(Ordering::SeqCst)
        );
    }
    runtime.reset();
    runtime.start_or_wake();
    runtime.cancel();
    println!(
        "cancel: status={:?}, roots={:?}, cleaned={}",
        runtime.status,
        runtime.roots.lock().expect("root registry"),
        runtime.cleaned.load(Ordering::SeqCst)
    );
    runtime.defect();
    println!(
        "defect: status={:?}, roots={:?}, cleaned={}",
        runtime.status,
        runtime.roots.lock().expect("root registry"),
        runtime.cleaned.load(Ordering::SeqCst)
    );
    for line in generated_c_bridge_probe() {
        println!("c-bridge: {line}");
    }
    benchmarks();
}

type BridgeFiber = Coroutine<BridgeCommand, BridgeSignal, BridgeOutcome, DefaultStack>;

fn generated_c_bridge_probe() -> Vec<String> {
    let mut evidence = Vec::new();

    let cleaned = Arc::new(AtomicBool::new(false));
    let mut child = generated_bridge_fiber(chimi_prototype_generated_child, cleaned.clone());
    let CoroutineResult::Yield(BridgeSignal::Parked { frame, backtrace }) =
        child.resume(BridgeCommand::Start)
    else {
        panic!("generated child must park");
    };
    let roots = roots_from_parked_frame(frame);
    let nested_frame = backtrace.contains("chimi_prototype_nested_call");
    let entry_frame = backtrace.contains("chimi_prototype_generated_child");
    assert_eq!(
        roots,
        [42],
        "generated root frame must expose the live local"
    );
    assert!(
        nested_frame,
        "backtrace must retain the nested generated frame"
    );
    assert!(
        entry_frame,
        "backtrace must retain the generated entry frame"
    );
    evidence.push(format!(
        "parked generated C frames expose roots={roots:?}; backtrace nested={nested_frame}, entry={entry_frame}"
    ));
    let CoroutineResult::Return(outcome) = child.resume(BridgeCommand::Wake) else {
        panic!("generated child must complete after wake");
    };
    assert!(matches!(outcome, BridgeOutcome::Succeeded(42)));
    assert!(cleaned.load(Ordering::SeqCst));
    evidence.push(format!(
        "wake returned {}; cleanup={}",
        outcome.label(),
        cleaned.load(Ordering::SeqCst)
    ));

    let cancelled = Arc::new(AtomicBool::new(false));
    let mut child = generated_bridge_fiber(chimi_prototype_generated_child, cancelled.clone());
    assert!(matches!(
        child.resume(BridgeCommand::Start),
        CoroutineResult::Yield(BridgeSignal::Parked { .. })
    ));
    let CoroutineResult::Return(outcome) = child.resume(BridgeCommand::Cancel) else {
        panic!("generated child must return after cooperative cancellation");
    };
    assert!(matches!(outcome, BridgeOutcome::Cancelled));
    assert!(cancelled.load(Ordering::SeqCst));
    evidence.push(format!(
        "cancel returned {} through C normally; cleanup={}",
        outcome.label(),
        cancelled.load(Ordering::SeqCst)
    ));

    let defected = Arc::new(AtomicBool::new(false));
    let mut child = generated_bridge_fiber(chimi_prototype_generated_defect, defected.clone());
    let CoroutineResult::Return(outcome) = child.resume(BridgeCommand::Start) else {
        panic!("generated defect must return a cause");
    };
    assert!(matches!(outcome, BridgeOutcome::Defected(DEFECT_CODE)));
    assert!(defected.load(Ordering::SeqCst));
    evidence.push(format!(
        "defect returned {} as runtime cause; cleanup={}",
        outcome.label(),
        defected.load(Ordering::SeqCst)
    ));

    evidence
}

fn generated_bridge_fiber(
    entry: unsafe extern "C" fn() -> i64,
    cleaned: Arc<AtomicBool>,
) -> BridgeFiber {
    let stack = DefaultStack::new(STACK_BYTES).expect("guarded fiber stack");
    Coroutine::with_stack(stack, move |yielder, command| {
        assert!(matches!(command, BridgeCommand::Start));
        let _cleanup = CleanupFlag(cleaned);
        BRIDGE_YIELDER.with(|slot| slot.set(yielder));
        let result = unsafe { entry() };
        BRIDGE_YIELDER.with(|slot| slot.set(std::ptr::null()));
        match result {
            CANCELLED_CODE => BridgeOutcome::Cancelled,
            DEFECT_CODE => BridgeOutcome::Defected(result),
            value => BridgeOutcome::Succeeded(value),
        }
    })
}

fn roots_from_parked_frame(frame: usize) -> Vec<i64> {
    let frame = unsafe { &*(frame as *const RootFrame) };
    unsafe { std::slice::from_raw_parts(frame.slots, frame.length) }.to_vec()
}

fn benchmarks() {
    const COLD_SAMPLES: usize = 1_000;
    const SPAWN_TASKS: usize = 10_000;
    const SWITCHES: usize = 1_000_000;
    const PARKED: usize = 2_000;

    let mut cold = Vec::with_capacity(COLD_SAMPLES);
    for _ in 0..COLD_SAMPLES {
        let started = Instant::now();
        let stack = DefaultStack::new(STACK_BYTES).expect("guarded stack");
        let mut fiber = Coroutine::with_stack(stack, |_: &Yielder<(), ()>, ()| 1_u8);
        black_box(fiber.resume(()));
        cold.push(started.elapsed());
    }
    cold.sort_unstable();
    let cold_p95 = cold[COLD_SAMPLES * 95 / 100];

    let spawn_started = Instant::now();
    for _ in 0..SPAWN_TASKS {
        let stack = DefaultStack::new(STACK_BYTES).expect("guarded stack");
        let mut fiber = Coroutine::with_stack(stack, |_: &Yielder<(), ()>, ()| 1_u8);
        black_box(fiber.resume(()));
    }
    let spawn_complete = spawn_started.elapsed();

    let parked_before = resident_high_water_bytes();
    let mut parked = Vec::with_capacity(PARKED);
    for _ in 0..PARKED {
        let stack = DefaultStack::new(STACK_BYTES).expect("guarded stack");
        let mut fiber = Coroutine::with_stack(stack, |yielder, ()| {
            touch_stack_then_park(yielder, 1);
        });
        black_box(fiber.resume(()));
        parked.push(fiber);
    }
    let parked_after = resident_high_water_bytes();

    let off = switch_benchmark(SWITCHES, false);
    let full = switch_benchmark(SWITCHES, true);
    black_box(&parked);

    println!("\nmeasurements (release, stack={STACK_BYTES} bytes):");
    println!(
        "  cold create+complete p95: {} ns ({COLD_SAMPLES} samples)",
        cold_p95.as_nanos()
    );
    println!(
        "  spawn+complete throughput: {:.1} ns/task ({SPAWN_TASKS} tasks)",
        nanos_per(spawn_complete, SPAWN_TASKS)
    );
    println!(
        "  resume→yield trace-off:    {:.1} ns/round-trip ({SWITCHES} yields)",
        nanos_per(off, SWITCHES)
    );
    println!(
        "  resume→yield traced:       {:.1} ns/round-trip ({SWITCHES} yields)",
        nanos_per(full, SWITCHES)
    );
    println!(
        "  parked high-water delta:  {} bytes ({PARKED} tasks, two 4 KiB frames touched)",
        parked_after.saturating_sub(parked_before)
    );
    println!(
        "  reserved stack address space: {} bytes",
        PARKED * STACK_BYTES
    );
}

fn switch_benchmark(switches: usize, traced: bool) -> Duration {
    let stack = DefaultStack::new(STACK_BYTES).expect("guarded stack");
    let mut fiber = Coroutine::with_stack(stack, move |yielder, ()| {
        for _ in 0..switches {
            yielder.suspend(());
        }
    });
    let mut trace = Vec::with_capacity(if traced { switches } else { 0 });
    let started = Instant::now();
    for _ in 0..switches {
        black_box(fiber.resume(()));
        if traced {
            trace.push((1_u32, trace.len() as u64, 1_u8));
        }
    }
    let elapsed = started.elapsed();
    black_box(trace);
    elapsed
}

#[inline(never)]
fn touch_stack_then_park(yielder: &Yielder<(), ()>, depth: usize) {
    let mut page = [0_u8; 4096];
    page[0] = depth as u8;
    black_box(&mut page);
    if depth == 0 {
        yielder.suspend(());
    } else {
        touch_stack_then_park(yielder, depth - 1);
    }
    black_box(page[0]);
}

fn nanos_per(duration: Duration, operations: usize) -> f64 {
    duration.as_nanos() as f64 / operations as f64
}

fn resident_high_water_bytes() -> usize {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    assert_eq!(result, 0, "getrusage failed");
    let rss = unsafe { usage.assume_init().ru_maxrss as usize };
    if cfg!(target_os = "macos") {
        rss
    } else {
        rss * 1024
    }
}

fn main() {
    if std::env::args().any(|argument| argument == "--all") {
        run_all();
        return;
    }

    let mut runtime = Runtime::new();
    loop {
        render(&runtime);
        print!("> ");
        io::stdout().flush().expect("flush prompt");
        let mut input = String::new();
        if io::stdin().read_line(&mut input).expect("read command") == 0 {
            break;
        }
        match input.trim() {
            "n" | "" => runtime.start_or_wake(),
            "c" => runtime.cancel(),
            "d" => runtime.defect(),
            "f" => runtime.events.extend(generated_c_bridge_probe()),
            "b" => benchmarks(),
            "r" => runtime.reset(),
            "q" => break,
            _ => runtime.events.push("unknown command".into()),
        }
    }
}
