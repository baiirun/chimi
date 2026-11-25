//! Replays the fuzz corpus of the native runtime (`tests/fuzz/`) under
//! AddressSanitizer and UndefinedBehaviorSanitizer, and checks that every
//! input still gives the output recorded beside it.
//!
//! The recorded outputs pin the runtime's JSON, text, decimal and duration
//! behaviour on inputs a fuzzer found interesting, so a change to that code,
//! or a Rust replacement of it, shows up as a diff. After an intended change,
//! rerun with `CHIMI_FUZZ_BLESS=1` to rewrite the corpus files, and review
//! the diff. `scripts/fuzz-runtime.sh` runs libFuzzer over the same targets.

use std::path::{Path, PathBuf};
use std::process::Command;

const TARGETS: [&str; 4] = ["json", "text", "decimal", "duration"];

fn manifest_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// Builds the replay driver, sanitized. Linux must have the sanitizers, so CI
/// always replays under them; elsewhere a compiler without them gets a plain
/// build.
fn build_driver(executable: &Path) {
    let compile = |flags: &[&str]| {
        Command::new("cc")
            .args(["-std=c11", "-O1", "-g", "-w"])
            .args(flags)
            .arg("-I")
            .arg(manifest_path("runtime/native"))
            .arg("-I")
            .arg(manifest_path("../../runtime/chimi-fiber/include"))
            .arg(manifest_path("tests/fuzz/runtime-fuzz.c"))
            .arg(env!("CHIMI_BUILT_STANDALONE_RUNTIME_ARCHIVE"))
            .arg("-o")
            .arg(executable)
            .arg("-lm")
            .output()
            .expect("run cc for the runtime fuzz driver")
    };
    let sanitized = compile(&[
        "-fsanitize=address,undefined",
        "-fno-sanitize-recover=all",
        "-fno-omit-frame-pointer",
    ]);
    if sanitized.status.success() {
        return;
    }
    if cfg!(target_os = "linux") {
        panic!(
            "AddressSanitizer and UndefinedBehaviorSanitizer are required on Linux, but the build failed:\n{}",
            String::from_utf8_lossy(&sanitized.stderr)
        );
    }
    eprintln!("runtime fuzz: sanitizers unavailable, replaying without them");
    let plain = compile(&[]);
    assert!(
        plain.status.success(),
        "{}",
        String::from_utf8_lossy(&plain.stderr)
    );
}

/// The lines of `expected` that `actual` does not repeat at the same position,
/// as `line N` followed by both versions, for a readable failure.
fn differences(expected: &str, actual: &str) -> String {
    let mut report = String::new();
    let (expected_lines, actual_lines): (Vec<_>, Vec<_>) =
        (expected.lines().collect(), actual.lines().collect());
    for index in 0..expected_lines.len().max(actual_lines.len()) {
        let (before, after) = (expected_lines.get(index), actual_lines.get(index));
        if before != after {
            report.push_str(&format!(
                "line {}:\n  recorded {}\n  now      {}\n",
                index + 1,
                before.unwrap_or(&"(none)"),
                after.unwrap_or(&"(none)")
            ));
        }
    }
    report
}

// Guards: every corpus input runs without a sanitizer report or a broken
// invariant (both abort the driver), and prints the output recorded for it.
#[test]
fn the_runtime_fuzz_corpus_replays_to_its_recorded_outputs() {
    let scratch = std::env::temp_dir().join(format!("chimi-runtime-fuzz-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let driver = scratch.join("runtime-fuzz");
    build_driver(&driver);
    let bless = std::env::var_os("CHIMI_FUZZ_BLESS").is_some();
    let mut failures = String::new();
    for target in TARGETS {
        let corpus = manifest_path(&format!("tests/fuzz/corpus/{target}.txt"));
        let recorded = std::fs::read_to_string(&corpus)
            .unwrap_or_else(|error| panic!("read {}: {error}", corpus.display()));
        assert!(
            recorded.lines().count() > 100,
            "{target}: the corpus should hold the seeds and the fuzzer's finds"
        );
        let run = Command::new(&driver)
            .arg(target)
            .arg(&corpus)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "print_stacktrace=1")
            .output()
            .expect("run the runtime fuzz driver");
        let replayed = String::from_utf8(run.stdout).expect("driver output is ASCII");
        if !run.status.success() {
            failures.push_str(&format!(
                "{target}: the driver failed ({}):\n{}\n",
                run.status,
                String::from_utf8_lossy(&run.stderr)
            ));
        } else if bless {
            std::fs::write(&corpus, &replayed).unwrap();
        } else if replayed != recorded {
            failures.push_str(&format!(
                "{target}: outputs differ from {} (rerun with CHIMI_FUZZ_BLESS=1 if the change is intended):\n{}",
                corpus.display(),
                differences(&recorded, &replayed)
            ));
        }
    }
    std::fs::remove_dir_all(&scratch).unwrap();
    assert!(failures.is_empty(), "{failures}");
}
