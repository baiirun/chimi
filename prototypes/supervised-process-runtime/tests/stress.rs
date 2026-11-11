//! Heap stress: nested regions over outer young values, large values, text,
//! fragments with large values adopted into a region, and the heap's own mix of
//! minor and full passes, ending with a chunk-leak check. `c/stress.c` holds the
//! checks; this test builds it as its own process (with AddressSanitizer and
//! UndefinedBehaviorSanitizer when `cc` supports them) so per-thread chunk
//! accounting sees only this run.
//!
//! `CHIMI_STRESS_PASSES=3000 cargo test --release --test stress` runs longer.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SANITIZERS: &[&str] = &["-fsanitize=address,undefined", "-fno-omit-frame-pointer"];

fn source(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn compile(executable: &Path, extra_flags: &[&str]) -> Output {
    Command::new("cc")
        .args(["-std=c11", "-O1", "-g"])
        .args(extra_flags)
        .arg("-I")
        .arg(source("c"))
        .arg(source("c/stress.c"))
        .arg(source("c/heap.c"))
        .arg("-o")
        .arg(executable)
        .output()
        .expect("invoke cc for the heap stress harness")
}

// Guards: regions, adoption, and minor/full collection compose without moving
// outer values, corrupting adopted copies, or leaking chunks.
#[test]
fn heap_survives_mixed_regions_fragments_and_collections() {
    let passes: u32 = std::env::var("CHIMI_STRESS_PASSES")
        .map(|value| value.parse().expect("CHIMI_STRESS_PASSES must be a number"))
        .unwrap_or(300);
    let scratch = std::env::temp_dir().join(format!("chimi-heap-stress-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let executable = scratch.join("stress");

    let sanitizers = if compile(&executable, SANITIZERS).status.success() {
        "address,undefined"
    } else {
        let plain = compile(&executable, &[]);
        assert!(
            plain.status.success(),
            "{}",
            String::from_utf8_lossy(&plain.stderr)
        );
        "none"
    };
    let run = Command::new(&executable)
        .arg(passes.to_string())
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .env("UBSAN_OPTIONS", "halt_on_error=1:print_stacktrace=1")
        .output()
        .expect("run heap stress harness");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        run.status.success(),
        "sanitizers {sanitizers}\n{stdout}{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(stdout.ends_with("ok\n"), "{stdout}");
    eprintln!(
        "heap stress ({sanitizers} sanitizers): {}",
        stdout.lines().next().unwrap_or("")
    );
    std::fs::remove_dir_all(scratch).unwrap();
}
