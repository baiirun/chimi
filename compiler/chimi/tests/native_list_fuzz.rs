//! Differential fuzz of the native runtime's persistent List.
//!
//! Builds `tests/fixtures/native-list-fuzz.c`, which includes the real
//! `runtime/native/runtime.c`, and runs random push, slice, rebuild, and
//! collect operations over many live versions against a model. The default
//! seed and operation count keep the test near ten seconds in debug builds.
//! Run longer or with another seed:
//!
//! ```sh
//! CHIMI_FUZZ_OPS=400000 CHIMI_FUZZ_SEED=7 cargo test -p chimi --test native_list_fuzz
//! ```
//!
//! The harness is built with AddressSanitizer and UndefinedBehaviorSanitizer
//! when the C compiler supports them, so a collection that frees structure a
//! live version still shares fails as a use-after-free even when the freed
//! memory still holds the right numbers.

use std::path::{Path, PathBuf};
use std::process::Command;

const DEFAULT_SEED: u64 = 88_172_645_463_325_252;
const DEFAULT_OPERATIONS: u64 = 40_000;
const SANITIZERS: &[&str] = &[
    "-fsanitize=address,undefined",
    "-fno-omit-frame-pointer",
    "-DCHIMI_POISON_YOUNG_CHUNKS",
];

fn repository_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn env_u64(name: &str, default: u64) -> u64 {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .unwrap_or_else(|_| panic!("{name} must be an unsigned integer, got {value:?}")),
        Err(_) => default,
    }
}

fn compile(executable: &Path, extra_flags: &[&str]) -> std::process::Output {
    Command::new("cc")
        .arg("-std=c11")
        .arg("-O1")
        .arg("-g")
        .args(extra_flags)
        .arg("-I")
        .arg(repository_path("compiler/chimi/runtime/native"))
        .arg("-I")
        .arg(repository_path("runtime/chimi-fiber/include"))
        .arg(format!(
            "{}/tests/fixtures/native-list-fuzz.c",
            env!("CARGO_MANIFEST_DIR")
        ))
        .arg("-o")
        .arg(executable)
        // glibc keeps floor, fmod, and the rest of libm out of libc.
        .arg("-lm")
        .output()
        .expect("invoke cc for the native list fuzz harness")
}

// Guards: every live List version, including ones that share vectors with
// pushed, sliced, rebuilt, and collected siblings, reads back exactly what a
// plain array model holds.
#[test]
fn native_list_matches_a_model_under_random_operations() {
    let seed = env_u64("CHIMI_FUZZ_SEED", DEFAULT_SEED);
    let operations = env_u64("CHIMI_FUZZ_OPS", DEFAULT_OPERATIONS);
    let scratch =
        std::env::temp_dir().join(format!("chimi-native-list-fuzz-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let executable = scratch.join("native-list-fuzz");

    let sanitized = compile(&executable, SANITIZERS);
    let sanitizers = if sanitized.status.success() {
        "address,undefined"
    } else {
        let plain = compile(&executable, &[]);
        assert!(
            plain.status.success(),
            "{}",
            String::from_utf8_lossy(&plain.stderr)
        );
        eprintln!("native list fuzz: sanitizers unavailable, running without them");
        "none"
    };

    let run = Command::new(&executable)
        .arg(seed.to_string())
        .arg(operations.to_string())
        .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=0")
        .env("UBSAN_OPTIONS", "halt_on_error=1:print_stacktrace=1")
        .output()
        .expect("run native list fuzz harness");
    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        run.status.success(),
        "seed {seed}, {operations} operations, sanitizers {sanitizers}\n{stdout}{stderr}"
    );
    assert!(
        stdout.starts_with(&format!("fuzz ok seed={seed} operations={operations} ")),
        "{stdout}"
    );
    eprintln!(
        "native list fuzz ({sanitizers} sanitizers): {}",
        stdout.trim_end()
    );
    std::fs::remove_dir_all(scratch).unwrap();
}
