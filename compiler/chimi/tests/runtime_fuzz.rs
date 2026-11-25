//! Replays the fuzz corpora of the native runtime and the binding packages
//! (`tests/fuzz/`) under AddressSanitizer and UndefinedBehaviorSanitizer, and
//! checks that every input still gives the output recorded beside it.
//!
//! The recorded outputs pin the runtime's JSON, text, decimal and duration
//! behaviour, and the binding shims' TLS, SCRAM and SQLite behaviour, on
//! inputs a fuzzer found interesting, so a change to that code, or a Rust
//! replacement of it, shows up as a diff. After an intended change, rerun
//! with `CHIMI_FUZZ_BLESS=1` to rewrite the corpus files, and review the diff.
//! `scripts/fuzz-runtime.sh` runs libFuzzer over the same targets.

use std::path::{Path, PathBuf};
use std::process::Command;

#[allow(dead_code)]
#[path = "../src/backend/runtime_archive_key.rs"]
mod runtime_archive_key;

const RUNTIME_TARGETS: [&str; 4] = ["json", "text", "decimal", "duration"];
const BINDING_TARGETS: [&str; 6] = [
    "tls-client",
    "tls-server",
    "tls-records",
    "tls-roots",
    "scram",
    "sqlite",
];
/// The binding packages the binding harness links.
const BINDING_PACKAGES: [&str; 2] = ["tls", "sqlite"];

const SANITIZE: [&str; 3] = [
    "-fsanitize=address,undefined",
    "-fno-sanitize-recover=all",
    "-fno-omit-frame-pointer",
];

fn manifest_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// Runs `compile` with the sanitizers, and on failure without them where
/// that is allowed. Linux must have the sanitizers, so CI always replays
/// under them; elsewhere a compiler without them gets a plain build.
/// Returns the flags that worked.
fn sanitized_or_plain(what: &str, compile: &dyn Fn(&[&str]) -> std::process::Output) -> &'static [&'static str] {
    let sanitized = compile(&SANITIZE);
    if sanitized.status.success() {
        return &SANITIZE;
    }
    if cfg!(target_os = "linux") {
        panic!(
            "AddressSanitizer and UndefinedBehaviorSanitizer are required on Linux, but the {what} build failed:\n{}",
            String::from_utf8_lossy(&sanitized.stderr)
        );
    }
    eprintln!("{what}: sanitizers unavailable, replaying without them");
    let plain = compile(&[]);
    assert!(
        plain.status.success(),
        "{}",
        String::from_utf8_lossy(&plain.stderr)
    );
    &[]
}

fn build_runtime_driver(executable: &Path) {
    sanitized_or_plain("runtime fuzz", &|flags| {
        Command::new("cc")
            .args(["-std=c11", "-O1", "-g", "-w"])
            .args(flags)
            .arg("-I")
            .arg(manifest_path("runtime/native"))
            .arg("-I")
            .arg(manifest_path("../../runtime/chimi-fiber/include"))
            .arg(manifest_path("tests/fuzz/runtime-fuzz.c"))
            .arg("-o")
            .arg(executable)
            .arg("-lm")
            .output()
            .expect("run cc for the runtime fuzz driver")
    });
}

fn package_directory(package: &str) -> PathBuf {
    manifest_path(&format!("runtime/native/bindings/{package}"))
}

/// The package's own flags, with its `-I` directories made absolute, as
/// `compile_binding_archive` makes them.
fn package_flags(package: &str) -> Vec<String> {
    let directory = package_directory(package);
    runtime_archive_key::binding_package_flags(package)
        .iter()
        .map(|flag| match flag.strip_prefix("-I") {
            Some(relative) => format!("-I{}", directory.join(relative).display()),
            None => (*flag).to_owned(),
        })
        .collect()
}

/// The files under `directory`, at any depth, with the extension `extension`.
fn files_with(directory: &Path, extension: &str, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_with(&path, extension, files);
        } else if path.extension().is_some_and(|found| found == extension) {
            files.push(path);
        }
    }
}

fn c_files(directory: &Path, files: &mut Vec<PathBuf>) {
    files_with(directory, "c", files);
}

fn header_files(directory: &Path, files: &mut Vec<PathBuf>) {
    files_with(directory, "h", files);
}

/// Compiles every C file of `package` with `flags` into `objects`, several at
/// once. An object is kept between runs under a name that hashes its source,
/// its command, the compiler's identity and every header it may include (the
/// package's and `chimi_binding.h`), since SQLite alone takes over a minute
/// under ASan.
fn compile_package(package: &str, flags: &[&str], objects: &Path) -> Vec<PathBuf> {
    let directory = package_directory(package);
    let mut sources = Vec::new();
    c_files(&directory, &mut sources);
    sources.sort();
    let mut arguments: Vec<String> = ["-std=c11", "-O1", "-g", "-w"]
        .iter()
        .chain(flags)
        .map(|flag| (*flag).to_owned())
        .collect();
    arguments.push(format!("-I{}", directory.display()));
    arguments.push(format!("-I{}", manifest_path("runtime/native").display()));
    arguments.extend(package_flags(package));
    let mut headers = Vec::new();
    header_files(&directory, &mut headers);
    headers.push(manifest_path("runtime/native/chimi_binding.h"));
    headers.sort();
    let mut shared = blake3::Hasher::new();
    let identity = Command::new("cc").arg("--version").output().expect("run cc --version");
    shared.update(&identity.stdout);
    for header in &headers {
        shared.update(header.to_string_lossy().as_bytes());
        shared.update(&std::fs::read(header).unwrap());
    }
    let shared = shared.finalize();
    let units: Vec<(PathBuf, PathBuf)> = sources
        .into_iter()
        .map(|source| {
            let mut hasher = blake3::Hasher::new();
            hasher.update(shared.as_bytes());
            hasher.update(&std::fs::read(&source).unwrap());
            hasher.update(arguments.join("\0").as_bytes());
            hasher.update(source.to_string_lossy().as_bytes());
            let object = objects.join(format!("{package}-{}.o", &hasher.finalize().to_hex()[..24]));
            (source, object)
        })
        .collect();
    let workers = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some((source, object)) = units.get(index) else {
                    break;
                };
                if object.exists() {
                    continue;
                }
                let partial = object.with_extension(format!("{}.part", std::process::id()));
                let built = Command::new("cc")
                    .args(&arguments)
                    .arg("-c")
                    .arg(source)
                    .arg("-o")
                    .arg(&partial)
                    .output()
                    .expect("run cc for a binding source");
                assert!(
                    built.status.success(),
                    "compile {}: {}",
                    source.display(),
                    String::from_utf8_lossy(&built.stderr)
                );
                std::fs::rename(&partial, object).unwrap();
            });
        }
    });
    units.into_iter().map(|(_, object)| object).collect()
}

/// Builds the binding harness against the packages' code, sanitized where
/// the compiler has the sanitizers.
fn build_binding_driver(executable: &Path) {
    let objects = Path::new(env!("CARGO_TARGET_TMPDIR")).join("binding-fuzz-objects");
    std::fs::create_dir_all(&objects).unwrap();
    let probe_source = objects.join("sanitizer-probe.c");
    std::fs::write(&probe_source, "int main(void) { return 0; }\n").unwrap();
    let probe_program = objects.join(format!("sanitizer-probe-{}", std::process::id()));
    let flags = sanitized_or_plain("binding fuzz", &|flags| {
        Command::new("cc")
            .args(flags)
            .arg(&probe_source)
            .arg("-o")
            .arg(&probe_program)
            .output()
            .expect("run cc for the sanitizer probe")
    });
    let _ = std::fs::remove_file(&probe_program);
    let mut link = Command::new("cc");
    link.args(["-std=c11", "-O1", "-g", "-w"])
        .args(flags)
        .arg(format!("-I{}", manifest_path("runtime/native").display()));
    for package in BINDING_PACKAGES {
        link.arg(format!("-I{}", package_directory(package).display()))
            .args(package_flags(package));
    }
    link.arg(format!(
        "-I{}",
        package_directory("sqlite").join("sqlite").display()
    ))
    .arg(manifest_path("tests/fuzz/binding-fuzz.c"));
    for package in BINDING_PACKAGES {
        link.args(compile_package(package, flags, &objects));
    }
    let built = link
        .arg("-o")
        .arg(executable)
        .args(["-lpthread", "-lm"])
        .output()
        .expect("run cc for the binding fuzz driver");
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
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

/// Replays each target's corpus through `driver`; with `CHIMI_FUZZ_BLESS` set,
/// writes the outputs back instead. `minimum` is how many inputs a corpus
/// must hold at least.
fn replay(driver: &Path, targets: &[&str], minimum: usize) {
    let bless = std::env::var_os("CHIMI_FUZZ_BLESS").is_some();
    let mut failures = String::new();
    for target in targets {
        let corpus = manifest_path(&format!("tests/fuzz/corpus/{target}.txt"));
        let recorded = std::fs::read_to_string(&corpus)
            .unwrap_or_else(|error| panic!("read {}: {error}", corpus.display()));
        assert!(
            recorded.lines().count() >= minimum,
            "{target}: the corpus should hold the seeds and the fuzzer's finds"
        );
        let run = Command::new(driver)
            .arg(target)
            .arg(&corpus)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "print_stacktrace=1")
            .env("CHIMI_FUZZ_TLS_FIXTURES", manifest_path("tests/fixtures/tls"))
            .output()
            .expect("run the fuzz driver");
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
    assert!(failures.is_empty(), "{failures}");
}

fn scratch(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("chimi-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

// Guards: every runtime corpus input runs without a sanitizer report or a
// broken invariant (both abort the driver), and prints the output recorded
// for it.
#[test]
fn the_runtime_fuzz_corpus_replays_to_its_recorded_outputs() {
    let directory = scratch("runtime-fuzz");
    let driver = directory.join("runtime-fuzz");
    build_runtime_driver(&driver);
    replay(&driver, &RUNTIME_TARGETS, 100);
    std::fs::remove_dir_all(&directory).unwrap();
}

// Guards: the same for the binding shims' corpora, run against the code of
// the tls and sqlite packages.
#[test]
fn the_binding_fuzz_corpus_replays_to_its_recorded_outputs() {
    let directory = scratch("binding-fuzz");
    let driver = directory.join("binding-fuzz");
    build_binding_driver(&driver);
    replay(&driver, &BINDING_TARGETS, 8);
    std::fs::remove_dir_all(&directory).unwrap();
}

// Guards: scripts/fuzz-runtime.sh compiles the packages with the flags
// `chimi build` gives them, so libFuzzer runs the code that ships.
#[test]
fn the_fuzz_script_compiles_the_packages_with_their_own_flags() {
    let script = std::fs::read_to_string(manifest_path("../../scripts/fuzz-runtime.sh")).unwrap();
    for package in BINDING_PACKAGES {
        for flag in runtime_archive_key::binding_package_flags(package) {
            // The script spells an -I directory from the package's root.
            let spelled = match flag.strip_prefix("-I") {
                Some(relative) => format!("-I$bindings/{package}/{relative}"),
                None => (*flag).to_owned(),
            };
            assert!(
                script.contains(&spelled),
                "scripts/fuzz-runtime.sh lacks {package}'s flag {flag}"
            );
        }
    }
    for target in RUNTIME_TARGETS.iter().chain(&BINDING_TARGETS) {
        assert!(script.contains(target), "scripts/fuzz-runtime.sh lacks the target {target}");
    }
}
