//! Measurements for docs/architecture/supervised-process-runtime.md.
//!
//! Run: cargo run --release --example measure
//! Times are wall-clock on one thread and vary by machine; byte counts are exact.

use chimi_supervised_process_runtime_prototype::heap::*;
use chimi_supervised_process_runtime_prototype::kernel::*;
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

fn int(value: i64) -> *mut Value {
    unsafe { chimi_value_int(value) }
}

fn items(values: &[*mut Value]) -> *mut Value {
    unsafe { chimi_value_items(values.as_ptr(), values.len() as u32) }
}

fn per_pass_cost() {
    println!("## Per-pass cost vs retained state (3 garbage values per pass)");
    println!("| retained ints | first two passes (promote + first full) | steady ns/pass over 100,000 passes | copied bytes total | full collections |");
    println!("| ---: | ---: | ---: | ---: | ---: |");
    for size in [10_usize, 1_000, 100_000, 1_000_000] {
        let heap = Heap::new(1);
        let _current = heap.enter();
        unsafe { chimi_region_enter() };
        let mut state = items(&(0..size as i64).map(int).collect::<Vec<_>>());
        let started = Instant::now();
        for _ in 0..2 {
            unsafe { chimi_region_pass(&mut state) };
        }
        let warmup = started.elapsed();
        let passes = 100_000;
        let started = Instant::now();
        for pass in 0..passes {
            let _garbage = items(&[int(pass), int(pass)]);
            unsafe { chimi_region_pass(&mut state) };
        }
        let elapsed = started.elapsed();
        unsafe { chimi_region_exit() };
        let stats = heap.stats();
        println!(
            "| {size} | {:.1} us | {:.0} | {} | {} |",
            warmup.as_secs_f64() * 1e6,
            elapsed.as_nanos() as f64 / passes as f64,
            stats.copied_bytes,
            stats.full_collections
        );
    }
    println!();
}

fn million_passes() {
    println!("## 1,000,000 passes replacing an Int state");
    let heap = Heap::new(1);
    let _current = heap.enter();
    unsafe { chimi_region_enter() };
    let mut state = int(0);
    let mut peak_chunks = 0;
    let started = Instant::now();
    for pass in 0..1_000_000_i64 {
        let next = unsafe { (*state).as_.integer } + 1;
        let _garbage = items(&[int(pass), int(pass)]);
        state = int(next);
        unsafe { chimi_region_pass(&mut state) };
        peak_chunks = peak_chunks.max(chunk_stats().live_chunk_bytes);
    }
    let elapsed = started.elapsed();
    unsafe { chimi_region_exit() };
    let stats = heap.stats();
    println!(
        "- {:.0} ns/pass; peak value bytes {}; peak chunk bytes {}; minor {}, full {}",
        elapsed.as_nanos() as f64 / 1e6,
        stats.peak_live_bytes,
        peak_chunks,
        stats.minor_collections,
        stats.full_collections
    );
    println!();
}

fn copy_cost() {
    println!("## Copy cost per message (copy into fragment + adopt + receiver pass)");
    println!("| message | value bytes | ns/message | ns/KiB |");
    println!("| --- | ---: | ---: | ---: |");
    let cases: Vec<(&str, Box<dyn Fn() -> *mut Value>)> = vec![
        ("Int", Box::new(|| int(7))),
        ("Text 64 B", Box::new(|| unsafe { chimi_value_text([b'x'; 64].as_ptr(), 64) })),
        ("Text 4 KiB", Box::new(|| unsafe { chimi_value_text([b'x'; 4096].as_ptr(), 4096) })),
        ("List of 16 Ints", Box::new(|| items(&(0..16).map(int).collect::<Vec<_>>()))),
        ("List of 1,000 Ints", Box::new(|| items(&(0..1_000).map(int).collect::<Vec<_>>()))),
        ("List of 100,000 Ints", Box::new(|| items(&(0..100_000).map(int).collect::<Vec<_>>()))),
        ("Shared DAG, 2^20 paths", Box::new(|| {
            let mut value = int(1);
            for _ in 0..20 {
                value = items(&[value, value]);
            }
            value
        })),
    ];
    for (label, build) in cases {
        let sender = Heap::new(1);
        let receiver = Heap::new(2);
        let root = {
            let _current = sender.enter();
            build()
        };
        {
            let _current = receiver.enter();
            unsafe { chimi_region_enter() };
        }
        let mut anchor = {
            let _current = receiver.enter();
            int(0)
        };
        let rounds = if label.contains("100,000") { 200 } else { 20_000 };
        let mut bytes = 0;
        let started = Instant::now();
        for _ in 0..rounds {
            let fragment = {
                let _current = sender.enter();
                unsafe { Fragment::copy(root) }.expect("owned graph")
            };
            let _current = receiver.enter();
            let before = receiver.stats().live_bytes;
            let _adopted = fragment.adopt();
            bytes = receiver.stats().live_bytes - before;
            unsafe { chimi_region_pass(&mut anchor) };
        }
        let ns = started.elapsed().as_nanos() as f64 / rounds as f64;
        println!("| {label} | {bytes} | {ns:.0} | {:.0} |", ns / (bytes as f64 / 1024.0));
    }
    println!();
}

fn counter(context: &mut ProcessContext) -> BodyEnd {
    unsafe { chimi_region_enter() };
    let mut state = int(0);
    let end = loop {
        match context.receive() {
            Received::Cancelled => break BodyEnd::Returned,
            Received::Message(message) => {
                let amount = unsafe { (*message).as_.integer };
                if amount == 0 {
                    break BodyEnd::Defected;
                }
                state = int(unsafe { (*state).as_.integer } + amount);
                unsafe { chimi_region_pass(&mut state) };
            }
        }
    };
    unsafe { chimi_region_exit() };
    end
}

fn spec(name: Option<NameId>) -> ProcessSpec {
    ProcessSpec {
        body: counter,
        mailbox_capacity: NonZeroU32::new(64).unwrap(),
        shutdown_within: Duration::from_secs(5),
        name,
    }
}

fn policy(members: Vec<Member>, capacity: u32, limit: u32) -> Policy {
    Policy {
        members,
        capacity: NonZeroU32::new(capacity).unwrap(),
        restarts: Restarts { limit, within: Duration::from_secs(30) },
    }
}

struct Host(Heap);

impl Host {
    fn new() -> Self {
        let host = Self(Heap::new(u64::MAX));
        let _current = host.0.enter();
        unsafe { chimi_region_enter() };
        drop(_current);
        host
    }

    fn message(&self, value: i64) -> Fragment {
        let _current = self.0.enter();
        let mut scratch = int(value);
        let fragment = unsafe { Fragment::copy(scratch) }.expect("host value");
        unsafe { chimi_region_pass(&mut scratch) };
        fragment
    }
}

fn million_messages() {
    println!("## 1,000,000 messages through one process (batches of 64)");
    let name = NameId(1);
    let host = Host::new();
    let mut kernel = Kernel::new(64 * 1024);
    kernel
        .start(policy(vec![Member::Process(spec(Some(name)), Restart::Never)], 1, 0))
        .expect("started");
    let mut peak_chunks = 0;
    let started = Instant::now();
    for _ in 0..15_625 {
        for _ in 0..64 {
            kernel.send(Address::Named(name), host.message(1)).expect("accepted");
        }
        peak_chunks = peak_chunks.max(chunk_stats().live_chunk_bytes);
        kernel.run_until_idle();
    }
    let elapsed = started.elapsed();
    let stats = kernel.heap_stats(Address::Named(name)).expect("running");
    println!(
        "- {:.0} ns/message (host copy, send, fiber switch, adopt, pass); process peak value bytes {}; thread peak chunk bytes {} (includes 64 queued fragments)",
        elapsed.as_nanos() as f64 / 1e6,
        stats.peak_live_bytes,
        peak_chunks
    );
    println!();
}

fn restart_and_teardown() {
    println!("## Restart and teardown");
    let name = NameId(1);
    let host = Host::new();
    let mut kernel = Kernel::new(64 * 1024);
    kernel
        .start(policy(vec![Member::Process(spec(Some(name)), Restart::OnDefect)], 1, u32::MAX))
        .expect("started");
    kernel.run_until_idle();
    let restarts = 10_000;
    let started = Instant::now();
    for _ in 0..restarts {
        kernel.send(Address::Named(name), host.message(0)).expect("accepted");
        kernel.run_until_idle();
    }
    println!(
        "- defect -> terminate -> restart -> first park: {:.1} us/cycle",
        started.elapsed().as_secs_f64() * 1e6 / restarts as f64
    );
    drop(kernel);

    for processes in [100_u32, 1_000] {
        let mut kernel = Kernel::new(64 * 1024);
        let root = kernel.start(policy(vec![], processes, 0)).expect("started");
        let started = Instant::now();
        for _ in 0..processes {
            kernel.start_process(root, spec(None), Restart::Never).expect("admitted");
        }
        kernel.run_until_idle();
        let spawn = started.elapsed();
        let started = Instant::now();
        kernel.stop(root);
        let stop = started.elapsed();
        println!(
            "- {processes} processes: start+first park {:.1} us/process, stop {:.1} us/process",
            spawn.as_secs_f64() * 1e6 / processes as f64,
            stop.as_secs_f64() * 1e6 / processes as f64
        );
    }

    for retained in [1_000_usize, 1_000_000] {
        let heap = Heap::new(1);
        {
            let _current = heap.enter();
            let _kept = items(&(0..retained as i64).map(int).collect::<Vec<_>>());
        }
        let chunks = chunk_stats().live_chunks;
        let started = Instant::now();
        drop(heap);
        println!(
            "- destroy heap holding {retained} values ({} chunks): {:.1} us",
            chunks - chunk_stats().live_chunks,
            started.elapsed().as_secs_f64() * 1e6
        );
    }
    println!();
}

fn peak_rss_bytes() -> i64 {
    let mut usage = std::mem::MaybeUninit::<Rusage>::zeroed();
    unsafe { getrusage(0, usage.as_mut_ptr()) };
    let maxrss = unsafe { usage.assume_init() }.maxrss;
    // macOS reports bytes, Linux kibibytes.
    if cfg!(target_os = "macos") { maxrss } else { maxrss * 1024 }
}

#[repr(C)]
struct Rusage {
    utime: [i64; 2],
    stime: [i64; 2],
    maxrss: i64,
    rest: [i64; 13],
}

unsafe extern "C" {
    fn getrusage(who: i32, usage: *mut Rusage) -> i32;
}

fn main() {
    per_pass_cost();
    million_passes();
    copy_cost();
    million_messages();
    restart_and_teardown();
    println!("Process peak RSS: {} MiB", peak_rss_bytes() / (1024 * 1024));
}
