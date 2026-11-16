//! The native runtime keeps what a carrier thread mutates in one `Carrier`
//! struct, and every other writable global is a named process-wide table
//! (docs/architecture/parallel-scheduler.md, "Runtime state by owner").
//!
//! A new file-scope or function-local static would be state that running
//! fibers on several threads has to account for, so it must be added to
//! `Carrier` or to the list below on purpose, with its owner decided.
//! Function-local statics are not allowed at all: their owner is invisible.
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
    // The carrier, which holds every per-thread field.
    "carrier_state",
    // Heaps and constants every fiber shares.
    "main_heap",
    "next_heap_owner",
    "next_mark_epoch",
    "static_area",
    "shared_values",
    "constant_values",
    "host_stack_limit",
    // Handle counts and their release queue (step 2 splits them).
    "channel_endpoints",
    "channel_endpoint_capacity",
    "channel_endpoint_count",
    "released_channels",
    "released_channel_count",
    "released_channel_capacity",
    // Tables whose ids travel inside values.
    "binding_handles",
    "binding_handle_count",
    "binding_handle_capacity",
    "binding_free_slot",
    "fs_files",
    "fs_file_count",
    "test_logs",
    "test_log_count",
    "process_defects",
    "process_defect_count",
    // The `random` provider's one seeded stream.
    "random_state",
    "random_seed",
    "random_seed_given",
    "random_read",
    // The operation profile's rows, allocated once at startup.
    "profile_rows",
    // Standard input and signals, owned by the first carrier.
    "standard_input",
    "standard_input_bytes",
    "received_signal",
    "signal_pipe",
    "signal_runtime",
    "user_signals",
    "user_signals_installed",
    "child_signal_installed",
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
        for expected in ["carrier_state", "main_heap"] {
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
