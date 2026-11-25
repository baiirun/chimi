//! Heap scenarios: copy-boundary safety (`CB`) and memory flatness (`MF`).
//!
//! A C harness scenario compiles `tests/process-lab/c/<file>`, which includes
//! `runtime.c`, and runs it with the scenario ID as its only argument. It
//! passes when the harness exits 0. AddressSanitizer is used when the C
//! compiler supports it, so a freed value that is still reachable fails the
//! scenario even when its bytes look right. When it is not, the harness runs
//! unsanitized and the pass carries a note saying so.
//!
//! A fixture scenario runs natively once per argument and checks its
//! `expected.memory` against the runs' memory reports (`fixture::check`).
//!
//! MF06 (`pay_for_play_linking`) builds three programs and reads their symbol
//! tables.

use super::Options;
use super::catalogue::{Form, Scenario};
use super::fixture::{self, Scratch};
use crate::Compiler;
use crate::backend::native::build_application;
use crate::project::{LoadedProject, StandardTree};
use std::path::{Path, PathBuf};
use std::process::Command;

/// `Ok(Some(note))` is a pass with a caveat the report shows as its detail.
pub(super) fn check(scenario: &Scenario, options: &Options) -> Result<Option<String>, String> {
    match scenario.form {
        Form::CHarness(file) => harness(scenario, file),
        Form::Fixture => fixture::check(scenario, options).map(|_| None),
        form => Err(format!("the heap runner does not run {form:?} scenarios")),
    }
}

/// A symbol only the scheduler archive `libchimi_fiber.a` defines.
const FIBER_SYMBOL: &str = "chimi_fiber_abi_version";
/// The entry of every incarnation, which only the process-kernel runtime defines.
const PROCESS_KERNEL_SYMBOL: &str = "chimi_rt_process_entry";

/// MF06: a program without concurrency links neither the scheduler nor the
/// process kernel, a tasks-only program links the scheduler alone, and a
/// process program links both.
pub(super) fn pay_for_play_linking() -> Result<(), String> {
    let programs = [
        (
            "no concurrency",
            "pub fn main() -> Int { 1 }\n",
            false,
            false,
        ),
        (
            "tasks only",
            "import std/task as Task\npub fn main() -> Int {\n  Task.result(Task.start(fn() -> Int { 1 }))\n}\n",
            true,
            false,
        ),
        (
            "processes",
            concat!(
                "import std/duration as Duration\n",
                "import std/process as Process\n",
                "import std/process.{Inbox}\n",
                "import std/supervisor as Supervisor\n",
                "fn idle(inbox: Inbox<Int>) -> Void {\n  let message = Process.receive(inbox)\n  Void\n}\n",
                "pub fn main() -> Void {\n",
                "  let member = Supervisor.process(Process.spec(idle, 1, Duration.zero), Supervisor.Restart.Never)\n",
                "  let root = Supervisor.start(Supervisor.policy([member], 1, Supervisor.restarts(0, Duration.seconds(1))))\n",
                "  Supervisor.stop(root)\n",
                "}\n",
            ),
            true,
            true,
        ),
    ];
    let scratch =
        std::env::temp_dir().join(format!("chimi-process-lab-{}-MF06", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let result = programs
        .iter()
        .try_for_each(|(name, source, fiber, kernel)| {
            let symbols = defined_symbols(&scratch.join(name.replace(' ', "-")), source)
                .map_err(|error| format!("{name}: {error}"))?;
            for (symbol, expected) in [(FIBER_SYMBOL, fiber), (PROCESS_KERNEL_SYMBOL, kernel)] {
                if symbols.iter().any(|defined| defined == symbol) != *expected {
                    return Err(format!(
                        "the {name} program {} `{symbol}`",
                        if *expected {
                            "does not define"
                        } else {
                            "defines"
                        }
                    ));
                }
            }
            Ok(())
        });
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

/// Builds `source` as `app/main` in `directory` and lists the external symbols
/// its executable defines, without the platform's leading underscore.
fn defined_symbols(directory: &Path, source: &str) -> Result<Vec<String>, String> {
    std::fs::create_dir_all(directory.join("app"))
        .map_err(|error| format!("create {}: {error}", directory.display()))?;
    std::fs::write(directory.join("app/main.chimi"), source)
        .map_err(|error| format!("write the program: {error}"))?;
    let project = LoadedProject::load(directory, "app/main", &StandardTree::embedded())
        .map_err(|error| format!("load the program: {error}"))?;
    let program = Compiler::prepare_backend(project.sources()).map_err(|diagnostics| {
        format!(
            "does not compile: {}",
            project.render_diagnostics(&diagnostics).trim_end()
        )
    })?;
    let executable = directory.join("program");
    build_application(
        &program,
        &StandardTree::embedded(),
        &executable,
        &directory.join("artifacts"),
        Vec::new(),
    )
    .map_err(|error| format!("native build failed: {error}"))?;
    let output = Command::new("nm")
        .arg("-g")
        .arg("-U")
        .arg(&executable)
        .output()
        .map_err(|error| format!("run nm: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "nm failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .map(|symbol| symbol.strip_prefix('_').unwrap_or(symbol).to_owned())
        .collect())
}

fn manifest_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// A runtime-kind C harness (`kernel_faults.c`, RA06 and SD04): it drives the
/// scheduler's C ABI directly, so it links the scheduler archive the compiler
/// bundles.
pub(super) fn kernel_harness(scenario: &Scenario, file: &str) -> Result<Option<String>, String> {
    let archive = crate::backend::native::bundled_fiber_archive()?;
    let mut libraries = vec![archive.into_os_string()];
    if cfg!(target_os = "linux") {
        libraries.extend(["-ldl".into(), "-lpthread".into()]);
    }
    run_harness(scenario, file, &libraries)
}

/// A harness that includes `runtime.c` without the scheduler links the Rust
/// runtime archive a program without the fiber runtime links.
fn harness(scenario: &Scenario, file: &str) -> Result<Option<String>, String> {
    let archive = crate::backend::native::bundled_standalone_runtime_archive()?;
    run_harness(scenario, file, &[archive.into_os_string()])
}

/// Compiles the harness `file` (with AddressSanitizer when the compiler
/// supports it), links `libraries` and libm after it, and runs it for the
/// scenario. glibc keeps floor, fmod, and the rest of libm out of libc.
fn run_harness(
    scenario: &Scenario,
    file: &str,
    libraries: &[std::ffi::OsString],
) -> Result<Option<String>, String> {
    let scratch = Scratch::new(scenario)?;
    let executable = scratch.path().join("harness");
    let source = manifest_path("tests/process-lab/c").join(file);
    let compile = |flags: &[&str]| {
        Command::new("cc")
            .args(["-std=c11", "-O1", "-g"])
            .args(flags)
            .arg("-I")
            .arg(manifest_path("runtime/native"))
            .arg("-I")
            .arg(manifest_path("../../runtime/chimi-fiber/include"))
            .arg(&source)
            .args(libraries)
            .arg("-lm")
            .arg("-o")
            .arg(&executable)
            .output()
            .map_err(|error| format!("run cc: {error}"))
    };
    let sanitized = compile(&[
        "-fsanitize=address,undefined",
        "-fno-omit-frame-pointer",
        "-DCHIMI_POISON_YOUNG_CHUNKS",
    ])?;
    let mut note = None;
    if !sanitized.status.success() {
        note = Some(format!(
            "ran without AddressSanitizer: the sanitized build failed ({})",
            compile_error_line(&sanitized.stderr)
        ));
        let plain = compile(&[])?;
        if !plain.status.success() {
            return Err(format!(
                "compile {file}: {}",
                compile_error_line(&plain.stderr)
            ));
        }
    }
    // Output goes to files, not pipes, so a chatty harness cannot block on a
    // full pipe while the run limit is being polled.
    let stdout_path = scratch.path().join("stdout.txt");
    let stderr_path = scratch.path().join("stderr.txt");
    let create = |path: &Path| {
        std::fs::File::create(path).map_err(|error| format!("create {}: {error}", path.display()))
    };
    let mut child = Command::new(&executable)
        .arg(scenario.id)
        .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=0")
        .env("UBSAN_OPTIONS", "halt_on_error=1:print_stacktrace=1")
        .stdout(create(&stdout_path)?)
        .stderr(create(&stderr_path)?)
        .spawn()
        .map_err(|error| format!("run {file}: {error}"))?;
    let status = fixture::wait_within_run_limit(&mut child, &format!("{file} {}", scenario.id))?;
    let read = |path: &Path| {
        std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))
    };
    if !status.success() {
        return Err(format!(
            "{file} {} exited with {status}: {}",
            scenario.id,
            first_error_line(&read(&stderr_path)?)
        ));
    }
    let stdout = read(&stdout_path)?;
    if stdout != b"ok\n" {
        return Err(format!(
            "{file} {} printed `{}`, expected `ok`",
            scenario.id,
            String::from_utf8_lossy(&stdout).trim_end()
        ));
    }
    Ok(note)
}

fn compile_error_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .rev()
        .find(|line| line.contains("error"))
        .or_else(|| text.lines().last())
        .unwrap_or("")
        .to_owned()
}

/// The harness's own failure line, or the sanitizer's summary.
fn first_error_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .find(|line| line.contains("check failed") || line.contains("ERROR:"))
        .or_else(|| text.lines().next())
        .unwrap_or("")
        .to_owned()
}
