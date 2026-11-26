//! The native runtime keeps what a carrier thread mutates in one `Carrier`
//! struct, and every other writable global is a named process-wide table
//! (docs/architecture/parallel-scheduler.md, "Runtime state by owner").
//!
//! A new file-scope or function-local static would be state that running
//! fibers on several threads has to account for, so it must be added to
//! `Carrier` or to the list below on purpose, with its owner decided.
//! Function-local statics are not allowed at all: their owner is invisible.
//!
//! The shared handle table also gets a two-thread stress run here, since it
//! is the one shared table later steps touch from several carriers.
//!
//! The check reads the object's symbols, and only ELF reports one per
//! variable: on Mach-O, clang merges internal globals into `__MergedGlobals`
//! and labels sections `ltmpN`. The source is the same on every host, so
//! Linux guards it.
#![cfg(target_os = "linux")]

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

/// Writable process-wide globals, each owned as the design note says.
const SHARED: &[&str] = &[
    // The carriers, which hold every per-thread field: carrier 0, the others
    // carriers_create allocates and their count, and each thread's pointer to
    // its own (a thread-local, one per thread by construction).
    "carrier_first",
    "carrier_others",
    "carrier_count",
    "carrier_this_thread",
    // Heaps and constants every fiber shares.
    "main_heap",
    "next_heap_owner",
    "next_mark_epoch",
    "static_area",
    "shared_values",
    "constant_values",
    // Whether this execution counted any native handle (an atomic flag, set
    // once and cleared before the next run); the counts and their lock live
    // in Rust (chimi-runtime handle_counts_abi.rs).
    "handles_counted",
    // Tables whose ids travel inside values, each behind its lock.
    "binding_handle_lock",
    "binding_handle_free",
    // Guards the Rust runtime's open-file table (chimi-runtime files_abi.rs).
    "fs_file_lock",
    "test_logs",
    "test_log_count",
    "test_log_lock",
    // The `random` provider's one seeded stream, behind its lock.
    "random_lock",
    "random_generator",
    "random_seed",
    "random_seed_given",
    "random_read",
    // Held while one telemetry event is written.
    "trace_lock",
    // The operation profile's rows, allocated once at startup and counted
    // with atomic adds.
    "profile_rows",
    // Standard input, behind its lock.
    "standard_input",
    "standard_input_bytes",
    "standard_input_lock",
    // Console output streams (CHI-266; their queues are in the Rust
    // runtime), behind one lock, and whether this thread holds it (a
    // thread-local).
    "console_streams",
    "console_lock",
    "console_lock_held",
    // Signals: the handlers' fields, and the installers' once flags.
    "received_signal",
    "signal_pipe",
    "signal_runtime",
    "user_signals",
    "user_signals_installed",
    "child_signal_installed",
    // Set by the first fatal exit, so only one thread runs exit.
    "runtime_exiting",
    // The thread-local carrier's offset from the thread pointer, the same
    // on every thread; set once by a constructor before main.
    "carrier_tls_offset",
    // Process-wide footprints and peaks, folded in by every carrier.
    "process_footprint_bytes",
    "process_peak_footprint_bytes",
    "process_fragment_footprint_bytes",
    "process_peak_fragment_footprint_bytes",
    "metrics_lock",
    "metrics_aggregate",
    // Atomic: set once by the first Metrics.runner_document.
    "metrics_reading",
    // Set before main starts; read under metrics_lock.
    "metrics_started_unix_ns",
];

fn writable_symbols(process_kernel: bool) -> BTreeSet<String> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let native = manifest.join("runtime/native");
    let object = std::env::temp_dir().join(format!(
        "chimi-runtime-state-{}-{process_kernel}.o",
        std::process::id()
    ));
    let mut compile = Command::new("cc");
    // The shipping flags (RUNTIME_C_FLAGS), so constants they fold away are not
    // counted. Without position-independent code a table of constant pointers
    // goes to read-only data instead of the relocated data nm also shows as `d`.
    compile
        .args(["-std=c11", "-O2", "-w", "-fno-pic", "-fno-pie", "-c", "-I"])
        .arg(&native);
    compile
        .arg("-I")
        .arg(manifest.join("../../runtime/chimi-fiber/include"));
    if process_kernel {
        compile.arg("-DCHIMI_PROCESS_KERNEL=1");
    }
    let output = compile
        .arg(native.join("runtime.c"))
        .arg("-o")
        .arg(&object)
        .output()
        .expect("run cc");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let symbols = Command::new("nm").arg(&object).output().expect("run nm");
    let _ = std::fs::remove_file(&object);
    assert!(symbols.status.success());
    String::from_utf8(symbols.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| {
            let mut columns = line.split_whitespace().rev();
            let name = columns.next()?;
            let kind = columns.next()?;
            matches!(kind, "b" | "B" | "d" | "D" | "C").then(|| name.to_owned())
        })
        .collect()
}

/// Whether `symbol` is a listed global. An optimiser may split an array into
/// `name.0`, `name.1` and so on.
/// Every function-local static is refused: GCC names one `var.N` and Clang
/// `function.var`, so the latter never matches, and the former matches only
/// a local that shares a listed global's name.
fn allowed(symbol: &str) -> bool {
    let mut parts = symbol
        .split('.')
        .filter(|part| !part.chars().all(|character| character.is_ascii_digit()));
    match (parts.next(), parts.next()) {
        (Some(base), None) => SHARED.contains(&base),
        _ => false,
    }
}

#[test]
fn runtime_globals_are_the_carrier_or_listed_shared_state() {
    for process_kernel in [false, true] {
        let symbols = writable_symbols(process_kernel);
        // An nm format this test cannot read would otherwise pass vacuously.
        for expected in ["carrier_first", "main_heap"] {
            assert!(
                symbols.iter().any(|symbol| symbol.trim_start_matches('_') == expected),
                "{expected} missing from {symbols:?}"
            );
        }
        let unlisted: Vec<String> = symbols
            .into_iter()
            .filter(|symbol| !allowed(symbol))
            .collect();
        assert!(
            unlisted.is_empty(),
            "runtime.c defines writable statics that are neither in Carrier nor listed \
             as shared (process kernel {process_kernel}): {unlisted:?}"
        );
    }
}

/// Builds `tests/fixtures/{fixture}.c` with ThreadSanitizer, runs it, and
/// checks that it prints "ok". Under ThreadSanitizer any access to shared
/// state outside its lock fails the run. Linux must have ThreadSanitizer, so
/// CI always runs these fixtures under it; elsewhere a compiler without it
/// gets a plain build.
fn run_thread_fixture(fixture: &str) {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let executable = std::env::temp_dir().join(format!("chimi-{fixture}-{}", std::process::id()));
    let compile = |flags: &[&str]| {
        Command::new("cc")
            .args(["-std=c11", "-O1", "-g", "-w"])
            .args(flags)
            .arg("-I")
            .arg(manifest.join("runtime/native"))
            .arg("-I")
            .arg(manifest.join("../../runtime/chimi-fiber/include"))
            .arg(manifest.join(format!("tests/fixtures/{fixture}.c")))
            .arg(env!("CHIMI_BUILT_STANDALONE_RUNTIME_ARCHIVE"))
            .arg("-o")
            .arg(&executable)
            .args(["-lm", "-lpthread"])
            .output()
            .expect("run cc")
    };
    let sanitized = compile(&["-fsanitize=thread"]);
    if sanitized.status.success() {
        eprintln!("{fixture}: built with ThreadSanitizer");
    } else if cfg!(target_os = "linux") {
        panic!(
            "{fixture}: ThreadSanitizer is required on Linux, but the build failed:\n{}",
            String::from_utf8_lossy(&sanitized.stderr)
        );
    } else {
        eprintln!("{fixture}: ThreadSanitizer unavailable, plain build");
        let plain = compile(&[]);
        assert!(plain.status.success(), "{}", String::from_utf8_lossy(&plain.stderr));
    }
    let run = Command::new(&executable)
        .env("TSAN_OPTIONS", "halt_on_error=1")
        .output()
        .expect("run the fixture");
    let _ = std::fs::remove_file(&executable);
    assert!(
        run.status.success() && run.stdout == b"ok\n",
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// The shared handle table under two threads (`fixtures/handle-table-stress.c`):
/// a reference retained on one thread and dropped on another never reaches zero
/// early.
#[test]
fn the_handle_table_counts_across_threads() {
    run_thread_fixture("handle-table-stress");
}

/// Four carrier threads at once (`fixtures/carrier-threads-stress.c`): memory
/// counters freed on another carrier sum back to zero, span ids and mark
/// epochs never repeat (and on one carrier stay consecutive), telemetry
/// sequence numbers, the profile, the `random` stream and a shared test
/// Recorder stay whole, and a file closed during a read is closed only once
/// the read gives it back.
#[test]
fn the_runtime_state_holds_across_carrier_threads() {
    run_thread_fixture("carrier-threads-stress");
}

/// The std/metrics aggregate under four threads (`fixtures/metrics-stress.c`):
/// every measurement lands while the tables grow, and each thread's series
/// holds exactly its own total.
#[test]
fn the_metrics_aggregate_counts_across_threads() {
    run_thread_fixture("metrics-stress");
}
