use std::ffi::OsStr;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}.chimi", env!("CARGO_MANIFEST_DIR"))
}

fn repository_path(relative: impl AsRef<Path>) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn scratch_directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "chimi-cli-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    if path.exists() {
        std::fs::remove_dir_all(&path).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    path
}

struct NativeExecution {
    process: Output,
    telemetry: String,
    outcome: String,
}

fn execute_native(executable: &Path, scratch: &Path, name: &str, trace: &str) -> NativeExecution {
    execute_native_with(executable, scratch, name, trace, &[])
}

/// `execute_native` with extra runner arguments, such as `--clock=virtual`.
fn execute_native_with(
    executable: &Path,
    scratch: &Path,
    name: &str,
    trace: &str,
    runner: &[&str],
) -> NativeExecution {
    let telemetry_path = scratch.join(format!("{name}-telemetry.jsonl"));
    let outcome_path = scratch.join(format!("{name}-outcome.jsonl"));
    let process = Command::new(executable)
        .arg("--chimi-runner")
        .args(runner)
        .arg(format!("--trace={trace}"))
        .arg(format!("--telemetry-file={}", telemetry_path.display()))
        .arg(format!("--outcome-file={}", outcome_path.display()))
        .arg(format!("--execution-id={name}"))
        .arg("--")
        .output()
        .expect("execute native Chimi program");
    NativeExecution {
        process,
        telemetry: std::fs::read_to_string(telemetry_path).unwrap(),
        outcome: std::fs::read_to_string(outcome_path).unwrap(),
    }
}

#[test]
fn version_reports_the_bootstrap_compiler() {
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .arg("--version")
        .output()
        .expect("run chimi --version");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "chimi 0.0.0-bootstrap\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn concurrency_lab_runs_each_evidence_kind_independently() {
    for kind in ["static", "runtime", "observability", "smoke"] {
        assert_concurrency_lab_kind_reports(kind);
    }
}

/// The `performance` kind gates wall-clock latency, throughput ratios, and RSS
/// deltas, which a loaded machine exceeds, so it runs only on request:
/// `cargo test --release -p chimi --test cli -- --ignored concurrency_lab_performance`
/// or `cargo run --release --bin chimi-concurrency-lab -- performance`.
#[test]
#[ignore = "wall-clock performance gates; run on an idle machine"]
fn concurrency_lab_performance_gates_hold() {
    assert_concurrency_lab_kind_reports("performance");
}

fn assert_concurrency_lab_kind_reports(kind: &str) {
    let output = Command::new(env!("CARGO_BIN_EXE_chimi-concurrency-lab"))
        .arg(kind)
        .output()
        .expect("run concurrency conformance lab");
    assert!(
        output.status.success(),
        "{kind}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty(), "{kind}");
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(report.starts_with("{\"schema\":\"chimi.concurrency-conformance.v1\",\"kind\":\""));
    assert!(report.contains(&format!("\"kind\":\"{kind}\"")));
    assert!(report.contains("\"checks\":["));
    assert!(report.contains("\"metrics\":{"));
    assert_eq!(report.lines().count(), 1);
}

#[test]
fn check_accepts_a_single_module_project() {
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["check", &fixture("success")])
        .output()
        .expect("run chimi check");

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn format_prints_canonical_source_without_modifying_the_file() {
    let path = fixture("format_messy");
    let before = std::fs::read_to_string(&path).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["format", &path])
        .output()
        .expect("format valid Chimi source");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let formatted = String::from_utf8(output.stdout).unwrap();
    assert!(formatted.contains("// comments survive"));
    assert_ne!(formatted, before);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
}

#[test]
fn format_returns_malformed_source_unchanged_with_syntax_diagnostics() {
    let path = fixture("missing_closing_brace");
    let source = std::fs::read(&path).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["format", &path])
        .output()
        .expect("format malformed Chimi source");

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, source);
    assert!(String::from_utf8_lossy(&output.stderr).contains("error[S1001] syntax"));
}

/// Human diagnostics point at `path:line:column` (one-based, the column in
/// characters); `--json` keeps byte offsets.
#[test]
fn check_prints_line_and_column_for_humans_and_byte_offsets_for_json() {
    let scratch = scratch_directory("check-line-column");
    let source = scratch.join("main.chimi");
    // `é` is two bytes and one character, so the column is not a byte count.
    let text = concat!(
        "fn take(a: Text, b: Int) -> Int {\n",
        "  b\n",
        "}\n",
        "pub fn main() -> Int {\n",
        "  take(\"é\", \"x\")\n",
        "}\n",
    );
    std::fs::write(&source, text).unwrap();
    let human = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .arg("check")
        .arg(&source)
        .output()
        .expect("check the program");
    let stderr = String::from_utf8_lossy(&human.stderr);
    let expected = format!("{}:5:13: error[T4203]", source.display());
    assert!(stderr.starts_with(&expected), "{stderr}");
    let json = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .arg("check")
        .arg("--json")
        .arg(&source)
        .output()
        .expect("check the program as JSON");
    let report = String::from_utf8_lossy(&json.stdout);
    let start = text.find("\"x\"").unwrap();
    assert!(report.contains(&format!("\"start\":{start},")), "{report}");
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn run_executes_source_and_prints_structured_events_and_outcome() {
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["run", &fixture("success")])
        .output()
        .expect("run valid Chimi source");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert!(lines.len() > 3);
    assert!(lines[0].contains("\"event\":\"computation_started\""));
    assert!(lines[0].contains("\"computation\":\"main\""));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("\"operation\":\"construct\""))
    );
    assert!(lines[lines.len() - 2].contains("\"event\":\"computation_completed\""));
    assert_eq!(
        lines[lines.len() - 1],
        concat!(
            "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",",
            "\"value\":{\"type\":\"Answer\",\"fields\":{\"value\":42}}}"
        )
    );
}

#[test]
fn build_links_a_runnable_executable_and_reports_its_artifacts() {
    let scratch = scratch_directory("build");
    let executable = scratch.join("answer");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", &fixture("success"), "-o"])
        .arg(&executable)
        .output()
        .expect("build valid Chimi source");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(report.starts_with("{\"schema\":\"chimi.build.v1\""));
    assert!(report.contains("\"backend\":\"native-aot\""));
    assert!(
        report.contains("\"standard_library\":{\"source\":\"embedded\"}"),
        "{report}"
    );
    // G7: the build report records the constant evaluation budgets the spec states.
    assert!(
        report.contains(concat!(
            "\"constant_limits\":{\"operations\":50000000,",
            "\"call_depth\":10000,\"value_bytes\":16777216}"
        )),
        "{report}"
    );
    assert!(report.contains("\"application_objects\":["));
    assert!(report.contains("\"metadata_object\":"));
    assert!(report.contains("\"runtime_archive\":"));
    assert!(report.contains("\"fiber_runtime_archive\":null"));
    assert!(report.contains("\"dynamic_dependencies\":["));
    assert!(executable.is_file());
    assert!(
        std::fs::metadata(&executable).unwrap().len() < 1024 * 1024,
        "an ordinary binary must not retain the unused fiber runtime"
    );

    let manifest = scratch.join("target/chimi/answer/build.json");
    assert_eq!(std::fs::read_to_string(manifest).unwrap(), report);
    let execution = execute_native(&executable, &scratch, "cli-build", "boundaries");
    assert!(execution.process.status.success());
    assert!(execution.process.stdout.is_empty());
    assert!(execution.process.stderr.is_empty());
    assert!(
        execution
            .telemetry
            .lines()
            .next()
            .unwrap()
            .contains("\"execution_id\":\"cli-build\"")
    );
    assert_eq!(
        execution.outcome,
        concat!(
            "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",",
            "\"value\":{\"type\":\"Answer\",\"fields\":{\"value\":42}}}\n"
        )
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

/// A copy of the embedded standard tree in `directory`, for `--std-dir`.
fn copy_standard_tree(directory: &Path) {
    std::fs::create_dir_all(directory).unwrap();
    let tree = Path::new(env!("CARGO_MANIFEST_DIR")).join("std");
    for entry in std::fs::read_dir(tree).unwrap() {
        let path = entry.unwrap().path();
        std::fs::copy(&path, directory.join(path.file_name().unwrap())).unwrap();
    }
}

// `--std-dir` reads `std/` modules from disk instead of the embedded tree, and
// the build report says that it did.
#[test]
fn build_reads_the_standard_tree_from_a_std_dir_override() {
    let scratch = scratch_directory("std-dir-build");
    let standard = scratch.join("std");
    copy_standard_tree(&standard);
    let list = standard.join("list.chimi");
    let mut text = std::fs::read_to_string(&list).unwrap();
    text.push_str("\npub fn answer() -> Int {\n  42\n}\n");
    std::fs::write(&list, text).unwrap();
    let source = scratch.join("main.chimi");
    std::fs::write(
        &source,
        "import std/list as List\n\npub fn main() -> Int {\n  List.answer() + List.length([1])\n}\n",
    )
    .unwrap();
    let executable = scratch.join("answer");

    let embedded = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .arg("check")
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        !embedded.status.success(),
        "the embedded std/list has no answer"
    );

    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .arg("--std-dir")
        .arg(&standard)
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8(output.stdout).unwrap();
    let directory = standard.canonicalize().unwrap();
    assert!(
        report.contains(&format!(
            "\"standard_library\":{{\"source\":\"override\",\"directory\":\"{}\"}}",
            directory.display()
        )),
        "{report}"
    );
    let execution = execute_native(&executable, &scratch, "std-dir", "off");
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":43}\n"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

fn std_check(std_dir: Option<&Path>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_chimi"));
    if let Some(directory) = std_dir {
        command.arg("--std-dir").arg(directory);
    }
    command.args(["std", "check"]).output().unwrap()
}

// `chimi std check` verifies a whole standard tree against the operation
// registry without building a program: the embedded tree and a faithful copy
// pass; a copy that drops a registered `native fn` or adds an unregistered one
// fails and names the operation.
#[test]
fn std_check_verifies_a_standard_tree_against_the_registry() {
    let embedded = std_check(None);
    assert!(
        embedded.status.success(),
        "{}",
        String::from_utf8_lossy(&embedded.stderr)
    );

    let scratch = scratch_directory("std-check");
    let copy = scratch.join("copy");
    copy_standard_tree(&copy);
    let faithful = std_check(Some(&copy));
    assert!(
        faithful.status.success(),
        "{}",
        String::from_utf8_lossy(&faithful.stderr)
    );

    let removed = scratch.join("removed");
    copy_standard_tree(&removed);
    let debug = removed.join("debug.chimi");
    let text = std::fs::read_to_string(&debug).unwrap();
    let declaration = "pub native fn render_indented<T>(value: T) -> Text\n";
    assert!(text.contains(declaration));
    std::fs::write(&debug, text.replace(declaration, "")).unwrap();
    let output = std_check(Some(&removed));
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(
            "registered operation std/debug.render_indented has no `native fn` declaration"
        ),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let added = scratch.join("added");
    copy_standard_tree(&added);
    let debug = added.join("debug.chimi");
    let mut text = std::fs::read_to_string(&debug).unwrap();
    text.push_str("\npub native fn inspect<T>(value: T) -> Text\n");
    std::fs::write(&debug, text).unwrap();
    let output = std_check(Some(&added));
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("N3104")
            && stderr.contains("no registered operation is named std/debug.inspect"),
        "{stderr}"
    );

    // A copy is the whole tree: a module it lacks is not taken from the
    // embedded one.
    let partial = scratch.join("partial");
    copy_standard_tree(&partial);
    std::fs::remove_file(partial.join("order.chimi")).unwrap();
    let output = std_check(Some(&partial));
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot read module std/order"), "{stderr}");

    std::fs::remove_dir_all(scratch).unwrap();
}

/// A copy of the embedded standard tree in `scratch/<name>` with `change`
/// applied to `file`'s text.
fn changed_standard_tree(
    scratch: &Path,
    name: &str,
    file: &str,
    change: impl FnOnce(String) -> String,
) -> PathBuf {
    let directory = scratch.join(name);
    copy_standard_tree(&directory);
    let path = directory.join(file);
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, change(text)).unwrap();
    directory
}

/// The `chimi:` lines of std check's stderr, without the diagnostics, whose
/// `error[E...]` text the mutation runner reads as a build failure.
fn registry_lines(stderr: &str) -> String {
    stderr
        .lines()
        .filter(|line| line.starts_with("chimi:"))
        .collect::<Vec<_>>()
        .join("\n")
}

// A `native fn` header the registry cannot check is T4320 and binds no
// operation: a parameter count that differs from the operation's arity, or a
// parameter without a type.
#[test]
fn std_check_rejects_a_native_fn_header_that_disagrees_with_the_registry() {
    let scratch = scratch_directory("std-check-headers");
    let declaration = "pub native fn render<T>(value: T) -> Text\n";
    for (name, header, message) in [
        (
            "arity",
            "pub native fn render<T>(value: T, extra: Int) -> Text\n",
            "`native fn render` takes 2 parameters, but operation std/debug.render takes 1",
        ),
        (
            "unannotated",
            "pub native fn render(value) -> Text\n",
            "`native fn render` must annotate every parameter and its success type",
        ),
    ] {
        let tree = changed_standard_tree(&scratch, name, "debug.chimi", |text| {
            assert!(text.contains(declaration));
            text.replace(declaration, header)
        });
        let output = std_check(Some(&tree));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{name}: {stderr}");
        assert!(stderr.contains("T4320") && stderr.contains(message), "{name}: {stderr}");
        assert!(
            stderr.contains("registered operation std/debug.render has no `native fn` declaration"),
            "{name}: the header must not bind the operation: {stderr}"
        );
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

// Every runtime-known declaration of a module the tree holds must be in its
// source. A module the tree does not hold at all, such as std/json, which is
// still built in, keeps its synthesized declarations: the faithful copy in
// `std_check_verifies_a_standard_tree_against_the_registry` has no json.chimi
// and passes.
#[test]
fn std_check_names_a_runtime_known_declaration_missing_from_source() {
    let scratch = scratch_directory("std-check-runtime-known");
    let enum_declaration = "pub enum Ordering {\n  Less\n  Equal\n  Greater\n}\n";
    let tree = changed_standard_tree(&scratch, "no-ordering", "order.chimi", |text| {
        assert!(text.contains(enum_declaration));
        text.replace(enum_declaration, "")
    });
    let output = std_check(Some(&tree));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("chimi: runtime-known std/order.Ordering is not declared in source"),
        "missing the registry line; got {}",
        registry_lines(&stderr)
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// `--std-dir` applies to a `--root` project as to a single file: check and
// build read the override, and the build report records it.
#[test]
fn std_dir_override_applies_to_a_root_project() {
    let scratch = scratch_directory("std-dir-root");
    let standard = changed_standard_tree(&scratch, "std", "list.chimi", |mut text| {
        text.push_str("\npub fn answer() -> Int {\n  42\n}\n");
        text
    });
    let project = scratch.join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join("main.chimi"),
        "import std/list as List\n\npub fn main() -> Int {\n  List.answer()\n}\n",
    )
    .unwrap();
    let chimi = |arguments: &[&OsStr]| {
        Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&scratch)
            .args(arguments)
            .output()
            .unwrap()
    };
    let root = project.as_os_str();
    let override_dir = standard.as_os_str();

    let embedded = chimi(&["check".as_ref(), "--root".as_ref(), root, "main".as_ref()]);
    assert!(!embedded.status.success(), "the embedded std/list has no answer");
    let checked = chimi(&[
        "--std-dir".as_ref(),
        override_dir,
        "check".as_ref(),
        "--root".as_ref(),
        root,
        "main".as_ref(),
    ]);
    assert!(
        checked.status.success(),
        "the override's List.answer did not resolve ({} diagnostics)",
        String::from_utf8_lossy(&checked.stderr).lines().count()
    );

    let executable = scratch.join("answer");
    let built = chimi(&[
        "--std-dir".as_ref(),
        override_dir,
        "build".as_ref(),
        "--root".as_ref(),
        root,
        "main".as_ref(),
        "-o".as_ref(),
        executable.as_os_str(),
    ]);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let report = String::from_utf8(built.stdout).unwrap();
    assert!(
        report.contains(&format!(
            "\"standard_library\":{{\"source\":\"override\",\"directory\":\"{}\"}}",
            standard.canonicalize().unwrap().display()
        )),
        "{report}"
    );
    let execution = execute_native(&executable, &scratch, "std-dir-root", "off");
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_source_executes_task_start_and_result() {
    let scratch = scratch_directory("native-task-start-result");
    let source = scratch.join("main.chimi");
    let executable = scratch.join("task-program");
    std::fs::write(
        &source,
        concat!(
            "import std/task as Task\n",
            "enum WorkError { Stopped }\n",
            "fn work() -> Int {\n",
            "  case 1 {\n",
            "    0 -> Failure(Stopped())\n",
            "    _ -> 42\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let child = Task.start(work)\n",
            "  Task.result(child)\n",
            "}\n",
        ),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(&source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .expect("build native Task program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let execution = execute_native(&executable, &scratch, "native-task", "off");
    assert!(
        execution.process.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.process.stderr)
    );
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// A task whose body cannot fail has an empty, concrete failure set; it used to
// leak `Task.result`'s failure parameter into `main` and give E5005
// (ts-e2509a). Native only: the interpreter has no Task runtime.
#[test]
fn native_task_with_a_body_that_cannot_fail_builds_and_runs() {
    let scratch = scratch_directory("native-task-infallible");
    let source = scratch.join("main.chimi");
    let executable = scratch.join("task-program");
    std::fs::write(
        &source,
        concat!(
            "import std/task as Task\n",
            "fn work() -> Int { 42 }\n",
            "pub fn main() -> Int {\n",
            "  let named = Task.start(work)\n",
            "  let inline = Task.start(fn() -> Int { 1 })\n",
            "  Task.result(named) + Task.result(inline)\n",
            "}\n",
        ),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(&source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .expect("build native Task program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let execution = execute_native(&executable, &scratch, "native-task-infallible", "off");
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":43}\n"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_source_executes_bounded_channel_send_close_and_receive() {
    let scratch = scratch_directory("native-channel-round-trip");
    let source = scratch.join("main.chimi");
    let executable = scratch.join("channel-program");
    std::fs::write(
        &source,
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Receive}\n",
            "pub fn main() -> Int {\n",
            "  let channel = Channel.bounded(1)\n",
            "  Channel.send(channel.sender, 42)\n",
            "  Channel.close(channel.completion)\n",
            "  let first = Channel.receive(channel.receiver)\n",
            "  let second = Channel.receive(channel.receiver)\n",
            "  case first {\n",
            "    Receive.Received(value) -> case second {\n",
            "      Receive.Received(_) -> 0\n",
            "      Receive.Drained -> value\n",
            "    }\n",
            "    Receive.Drained -> 0\n",
            "  }\n",
            "}\n",
        ),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(&source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .expect("build native Channel program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let execution = execute_native(&executable, &scratch, "native-channel", "off");
    assert!(
        execution.process.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.process.stderr)
    );
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_source_suspends_a_task_on_a_rendezvous_channel() {
    let scratch = scratch_directory("native-task-channel-rendezvous");
    let source = scratch.join("main.chimi");
    let executable = scratch.join("rendezvous-program");
    std::fs::write(
        &source,
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Receive}\n",
            "import std/task as Task\n",
            "pub fn main() -> Int {\n",
            "  let channel = Channel.bounded(0)\n",
            "  let sender = channel.sender\n",
            "  let receiver = channel.receiver\n",
            "  let completion = channel.completion\n",
            "  let child = Task.start(fn() -> Int {\n",
            "    Channel.send(sender, 42)\n",
            "    7\n",
            "  })\n",
            "  let received = Channel.receive(receiver)\n",
            "  let child_value = Task.result(child)\n",
            "  Channel.close(completion)\n",
            "  case received {\n",
            "    Receive.Received(value) -> value + child_value\n",
            "    Receive.Drained -> 0\n",
            "  }\n",
            "}\n",
        ),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(&source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .expect("build native Task and Channel rendezvous program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let execution = execute_native(&executable, &scratch, "native-rendezvous", "off");
    assert!(
        execution.process.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.process.stderr)
    );
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":49}\n"
    );
    assert!(execution.telemetry.is_empty());

    let boundaries = execute_native(
        &executable,
        &scratch,
        "native-rendezvous-boundaries",
        "boundaries",
    );
    assert!(boundaries.process.status.success());
    assert!(boundaries.telemetry.contains("\"event\":\"task_started\""));
    assert!(
        boundaries
            .telemetry
            .contains("\"event\":\"channel_created\"")
    );
    assert!(!boundaries.telemetry.contains("\"event\":\"task_parked\""));

    let full = execute_native(&executable, &scratch, "native-rendezvous-full", "full");
    assert!(full.process.status.success());
    assert!(full.telemetry.contains("\"event\":\"task_parked\""));
    assert!(
        full.telemetry
            .contains("\"event\":\"channel_receive_parked\"")
    );
    assert!(
        full.telemetry
            .contains("\"event\":\"channel_receive_delivered\"")
    );
    let sequences = full
        .telemetry
        .lines()
        .map(|line| json_u64_field(line, "sequence"))
        .collect::<Vec<_>>();
    assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]));
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_source_transfers_an_exclusive_receiver_to_a_child() {
    let scratch = scratch_directory("native-channel-receiver-transfer");
    let source = scratch.join("main.chimi");
    let executable = scratch.join("receiver-transfer-program");
    std::fs::write(
        &source,
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Receive}\n",
            "import std/task as Task\n",
            "pub fn main() -> Int {\n",
            "  let channel = Channel.bounded(0)\n",
            "  let sender = channel.sender\n",
            "  let receiver = channel.receiver\n",
            "  let completion = channel.completion\n",
            "  let child = Task.start(fn() -> Int {\n",
            "    let received = Channel.receive(receiver)\n",
            "    case received {\n",
            "      Receive.Received(value) -> value\n",
            "      Receive.Drained -> 0\n",
            "    }\n",
            "  })\n",
            "  Channel.send(sender, 42)\n",
            "  let value = Task.result(child)\n",
            "  Channel.close(completion)\n",
            "  value\n",
            "}\n",
        ),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(&source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .expect("build native receiver-transfer program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let execution = execute_native(&executable, &scratch, "native-transfer", "off");
    assert!(
        execution.process.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.process.stderr)
    );
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_channel_close_recovers_the_unaccepted_value() {
    let scratch = scratch_directory("native-channel-close-recovery");
    let source = scratch.join("main.chimi");
    let executable = scratch.join("close-recovery-program");
    std::fs::write(
        &source,
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{SendError}\n",
            "pub fn main() -> Int {\n",
            "  let channel = Channel.bounded(1)\n",
            "  Channel.close(channel.completion)\n",
            "  case Channel.send(channel.sender, 42) {\n",
            "    Success(_) -> 0\n",
            "    Failure(error) -> case error {\n",
            "      SendError.Closed(value) -> value\n",
            "      SendError.Aborted(value) -> value\n",
            "    }\n",
            "  }\n",
            "}\n",
        ),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(&source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .expect("build native close-recovery program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let execution = execute_native(&executable, &scratch, "native-close-recovery", "off");
    assert!(execution.process.status.success());
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Task-and-channel "Value transfer": a `Failure(SendError.Closed(value))` arm
/// recovers the unaccepted value at the sender's element type, so a struct
/// payload keeps its fields.
#[test]
fn native_channel_send_recovers_a_struct_through_a_variant_pattern() {
    let scratch = scratch_directory("native-channel-send-variant-recovery");
    let source = scratch.join("main.chimi");
    let executable = scratch.join("send-recovery-program");
    std::fs::write(
        &source,
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{SendError, Sender}\n",
            "struct Order {\n",
            "  id: Int,\n",
            "}\n",
            "fn push(sender: Sender<Order>, order: Order) -> Int {\n",
            "  case Channel.send(sender, order) {\n",
            "    Success(_) -> 0\n",
            "    Failure(SendError.Closed(back)) -> back.id\n",
            "    Failure(SendError.Aborted(back)) -> back.id + 100\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let channel = Channel.bounded(1)\n",
            "  Channel.close(channel.completion)\n",
            "  push(channel.sender, Order(7))\n",
            "}\n",
        ),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(&source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .expect("build native send-recovery program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let execution = execute_native(&executable, &scratch, "native-send-recovery", "off");
    assert!(execution.process.status.success());
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":7}\n"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_task_cancel_cleans_up_a_parked_child() {
    let scratch = scratch_directory("native-task-cancel");
    let source = scratch.join("main.chimi");
    let executable = scratch.join("cancel-program");
    std::fs::write(
        &source,
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Receive}\n",
            "import std/task as Task\n",
            "pub fn main() -> Int {\n",
            "  let channel = Channel.bounded(0)\n",
            "  let receiver = channel.receiver\n",
            "  let child = Task.start(fn() -> Int {\n",
            "    let received = Channel.receive(receiver)\n",
            "    case received {\n",
            "      Receive.Received(value) -> value\n",
            "      Receive.Drained -> 0\n",
            "    }\n",
            "  })\n",
            "  Task.cancel(child)\n",
            "  42\n",
            "}\n",
        ),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(&source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .expect("build native cancellation program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let execution = execute_native(&executable, &scratch, "native-cancel", "off");
    assert!(execution.process.status.success());
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_channel_abort_discards_values_and_fails_receive_explicitly() {
    let scratch = scratch_directory("native-channel-abort");
    let source = scratch.join("main.chimi");
    let executable = scratch.join("abort-program");
    std::fs::write(
        &source,
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{ChannelError}\n",
            "pub fn main() -> Int {\n",
            "  let channel = Channel.bounded(1)\n",
            "  Channel.send(channel.sender, 10)\n",
            "  Channel.abort(channel.receiver)\n",
            "  case Channel.receive(channel.receiver) {\n",
            "    Success(_) -> 0\n",
            "    Failure(ChannelError.Aborted) -> 42\n",
            "  }\n",
            "}\n",
        ),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(&source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .expect("build native abort program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let execution = execute_native(&executable, &scratch, "native-abort", "full");
    assert!(execution.process.status.success());
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"
    );
    assert!(
        execution
            .telemetry
            .contains("\"event\":\"channel_aborted\"")
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_child_defect_propagates_through_task_result() {
    let scratch = scratch_directory("native-task-defect");
    let source = scratch.join("main.chimi");
    let executable = scratch.join("defect-program");
    std::fs::write(
        &source,
        concat!(
            "import std/task as Task\n",
            "enum WorkError { Stopped }\n",
            "fn broken() -> Int {\n",
            "  case 1 {\n",
            "    0 -> Failure(Stopped())\n",
            "    _ -> 1 / 0\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let child = Task.start(broken)\n",
            "  Task.result(child)\n",
            "}\n",
        ),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(&source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .expect("build native child-defect program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let execution = execute_native(&executable, &scratch, "native-defect", "full");
    assert_eq!(execution.process.status.code(), Some(70));
    assert!(execution.outcome.contains("\"outcome\":\"defect\""));
    assert!(execution.outcome.contains("integer division by zero"));
    assert!(execution.telemetry.contains("\"event\":\"task_terminal\""));
    std::fs::remove_dir_all(scratch).unwrap();
}

// Guards: a scheduler-made defect code reached the root as a control pointer and crashed.
#[test]
fn native_deadlocked_tasks_defect_the_root() {
    let scratch = scratch_directory("native-task-deadlock");
    let executable = build_native(
        &scratch,
        "deadlock",
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Receive}\n",
            "import std/task as Task\n",
            "pub fn main() -> Int {\n",
            "  let ch = Channel.bounded(1)\n",
            "  let s = ch.sender\n",
            "  let r = ch.receiver\n",
            "  let child = Task.start(fn() -> Int {\n",
            "    Channel.send(s, 1)\n",
            "    Channel.send(s, 2)\n",
            "    0\n",
            "  })\n",
            "  let waited = Task.result(child)\n",
            "  let got = Channel.receive(r)\n",
            "  case got {\n",
            "    Receive.Received(v) -> v + waited\n",
            "    Receive.Drained -> waited\n",
            "  }\n",
            "}\n",
        ),
    );
    let execution = execute_native(&executable, &scratch, "deadlock", "off");
    assert_eq!(
        execution.process.status.code(),
        Some(70),
        "{}",
        String::from_utf8_lossy(&execution.process.stderr)
    );
    assert!(
        execution.outcome.contains("\"outcome\":\"defect\"")
            && execution.outcome.contains("deadlock"),
        "{}",
        execution.outcome
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// ----- Clock.sleep and the virtual clock (ts-6ccbc9 decision 6) ----------

const CLOCK_IMPORTS: &str = concat!(
    "import std/channel as Channel\n",
    "import std/channel.{Receive}\n",
    "import std/clock as Clock\n",
    "import std/duration as Duration\n",
    "import std/task as Task\n",
);

/// The virtual times, in nanoseconds, that the telemetry's `clock_advanced`
/// events move the clock to, in order.
fn clock_advances(telemetry: &str) -> Vec<i64> {
    telemetry
        .lines()
        .filter(|line| line.contains("\"event\":\"clock_advanced\""))
        .map(|line| {
            let start = line.find("\"time_ns\":").expect("an advance has time_ns") + 10;
            let digits: String = line[start..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            digits.parse().expect("time_ns is a count")
        })
        .collect()
}

/// The lab projector's view of the run, with task terminals kept, so each
/// line carries the virtual time at which a task ended. The projector also
/// checks the clock events' key sets and that time never moves back.
fn terminals(telemetry: &str) -> Vec<String> {
    chimi::process_lab::transcript::project(telemetry, &["task_terminal"])
        .unwrap_or_else(|error| panic!("{error}\n{telemetry}"))
}

// Rejects: a clock that advances by anything but the requested duration, a
// non-positive duration that parks or moves time, and wall-clock waiting
// (thirty seconds of it would time the suite out).
#[test]
fn native_virtual_sleep_advances_exactly_the_requested_duration() {
    let scratch = scratch_directory("native-clock-virtual-sleep");
    let executable = build_native(
        &scratch,
        "virtual-sleep",
        &format!(
            "{CLOCK_IMPORTS}pub fn main() -> Int {{\n  \
             Clock.sleep(Duration.seconds(-5))\n  \
             Clock.sleep(Duration.zero)\n  \
             Clock.sleep(Duration.seconds(30))\n  \
             7\n}}\n"
        ),
    );
    for trace in ["boundaries", "full"] {
        let execution = execute_native_with(
            &executable,
            &scratch,
            &format!("virtual-sleep-{trace}"),
            trace,
            &["--clock=virtual"],
        );
        assert_eq!(
            execution.process.status.code(),
            Some(0),
            "{}",
            execution.outcome
        );
        assert_eq!(
            execution.outcome,
            "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":7}\n"
        );
        assert_eq!(clock_advances(&execution.telemetry), [30_000_000_000]);
        assert_eq!(
            terminals(&execution.telemetry),
            ["@30s task_terminal task=T1 parent=- cause=-"]
        );
        let timer_events = execution
            .telemetry
            .matches("\"event\":\"clock_timer_")
            .count();
        // Timer start and fire are motion events: full mode only.
        assert_eq!(timer_events, if trace == "full" { 2 } else { 0 }, "{trace}");
    }
    let off = execute_native_with(
        &executable,
        &scratch,
        "virtual-sleep-off",
        "off",
        &["--clock=virtual"],
    );
    assert_eq!(off.process.status.code(), Some(0));
    assert!(off.telemetry.is_empty(), "{}", off.telemetry);
    std::fs::remove_dir_all(scratch).unwrap();
}

// Rejects timers that fire in registration or start order instead of deadline
// order: the 2 ms sleeper starts first, so FIFO wakeup would deliver 2 first.
#[test]
fn native_virtual_sleepers_wake_in_deadline_order() {
    let scratch = scratch_directory("native-clock-deadline-order");
    let executable = build_native(
        &scratch,
        "deadline-order",
        &format!(
            "{CLOCK_IMPORTS}fn take(receiver: Channel.Receiver<Int>) -> Int {{\n  \
             let got = Channel.receive(receiver)\n  \
             case got {{\n    \
             Receive.Received(value) -> value\n    \
             Receive.Drained -> 0\n  }}\n}}\n\
             pub fn main() -> Int {{\n  \
             let channel = Channel.bounded(2)\n  \
             let sender = channel.sender\n  \
             let slow = Task.start(fn() -> Int {{\n    \
             Clock.sleep(Duration.milliseconds(2))\n    \
             Channel.send(sender, 2)\n    \
             0\n  }})\n  \
             let fast = Task.start(fn() -> Int {{\n    \
             Clock.sleep(Duration.milliseconds(1))\n    \
             Channel.send(sender, 1)\n    \
             0\n  }})\n  \
             let first = take(channel.receiver)\n  \
             let second = take(channel.receiver)\n  \
             Task.result(slow) + Task.result(fast) + first * 10 + second\n}}\n"
        ),
    );
    let execution = execute_native_with(
        &executable,
        &scratch,
        "deadline-order",
        "boundaries",
        &["--clock=virtual"],
    );
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":12}\n"
    );
    assert_eq!(clock_advances(&execution.telemetry), [1_000_000, 2_000_000]);
    assert_eq!(
        terminals(&execution.telemetry),
        [
            "@1ms task_terminal task=T3 parent=T1 cause=-",
            "@2ms task_terminal task=T2 parent=T1 cause=-",
            "@2ms task_terminal task=T1 parent=- cause=-",
        ]
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// The lab's `settle` barrier (supervised-process-conformance.md, "Virtual time
// in fixtures"): when it returns, every task woken before the call has run
// until it parked, and time has moved exactly 1 ms. Rejects a clock that
// advances while a task is runnable: the relay chain below would then end
// after the advance.
#[test]
fn native_settle_runs_woken_work_before_time_moves() {
    let scratch = scratch_directory("native-clock-settle");
    let executable = build_native(
        &scratch,
        "settle",
        &format!(
            "{CLOCK_IMPORTS}fn settle() -> Void needs {{ clock: Clock.Clock }} {{\n  \
             Clock.sleep(Duration.milliseconds(1))\n}}\n\
             pub fn main() -> Int {{\n  \
             let first = Channel.bounded(1)\n  \
             let second = Channel.bounded(1)\n  \
             let forward = second.sender\n  \
             let inbox = first.receiver\n  \
             let relay = Task.start(fn() -> Int {{\n    \
             let got = Channel.receive(inbox)\n    \
             case got {{\n      \
             Receive.Received(value) -> {{\n        \
             Channel.send(forward, value + 1)\n        \
             0\n      }}\n      \
             Receive.Drained -> 0\n    }}\n  }})\n  \
             let ping = first.sender\n  \
             let pinger = Task.start(fn() -> Int {{\n    \
             Channel.send(ping, 41)\n    \
             0\n  }})\n  \
             settle()\n  \
             let reply = Channel.receive(second.receiver)\n  \
             let relayed = case reply {{\n    \
             Receive.Received(value) -> value\n    \
             Receive.Drained -> 0\n  }}\n  \
             Task.result(relay) + Task.result(pinger) + relayed\n}}\n"
        ),
    );
    let execution = execute_native_with(
        &executable,
        &scratch,
        "settle",
        "boundaries",
        &["--clock=virtual"],
    );
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"
    );
    assert_eq!(clock_advances(&execution.telemetry), [1_000_000]);
    assert_eq!(
        terminals(&execution.telemetry),
        [
            "@0s task_terminal task=T3 parent=T1 cause=-",
            "@0s task_terminal task=T2 parent=T1 cause=-",
            "@1ms task_terminal task=T1 parent=- cause=-",
        ]
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// time-and-outcomes.md, "Virtual time": with every task parked and no timer
// pending, the root defects with a cause naming each parked task and what it
// waits on. Rejects deadlock detection that is off (the run would hang or
// report the real clock's generic deadlock) and a cause without the waiters.
#[test]
fn native_virtual_deadlock_defects_the_root_with_its_waiters() {
    let scratch = scratch_directory("native-clock-virtual-deadlock");
    let executable = build_native(
        &scratch,
        "virtual-deadlock",
        &format!(
            "{CLOCK_IMPORTS}pub fn main() -> Int {{\n  \
             let channel = Channel.bounded(1)\n  \
             let keep = channel.sender\n  \
             Clock.sleep(Duration.seconds(1))\n  \
             let got = Channel.receive(channel.receiver)\n  \
             case got {{\n    \
             Receive.Received(value) -> value\n    \
             Receive.Drained -> 0\n  }}\n}}\n"
        ),
    );
    let execution = execute_native_with(
        &executable,
        &scratch,
        "virtual-deadlock",
        "boundaries",
        &["--clock=virtual"],
    );
    assert_eq!(execution.process.status.code(), Some(70));
    assert!(
        execution.outcome.contains("\"outcome\":\"defect\"")
            && execution.outcome.contains(
                "virtual-time deadlock: every task is parked and no timer is pending: \
                 task 1 waits on Channel.receive"
            ),
        "{}",
        execution.outcome
    );
    let waiters: Vec<_> = execution
        .telemetry
        .lines()
        .filter(|line| line.contains("\"event\":\"clock_deadlock_waiter\""))
        .collect();
    assert_eq!(waiters.len(), 1, "{}", execution.telemetry);
    // The projector rejects a clock event that carries its request kind in
    // `cause_task_id`.
    assert_eq!(
        terminals(&execution.telemetry),
        ["@1s task_terminal task=T1 parent=- cause=T1"]
    );
    assert!(
        waiters[0].contains("\"task_id\":1,")
            && waiters[0].contains("\"time_ns\":1000000000,\"request_kind\":6}"),
        "{}",
        waiters[0]
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// An Instant past the range defects instead of wrapping to a deadline in the
// past. Rejects an unchecked `now + d`: the 1 s sleep first makes now positive,
// so the maximum Duration overflows, and a wrapped deadline would return at once.
#[test]
fn native_sleep_past_the_instant_range_defects() {
    let scratch = scratch_directory("native-clock-overflow");
    let executable = build_native(
        &scratch,
        "clock-overflow",
        &format!(
            "{CLOCK_IMPORTS}pub fn main() -> Int {{\n  \
             Clock.sleep(Duration.seconds(1))\n  \
             Clock.sleep(Duration.nanoseconds(9223372036854775807))\n  \
             0\n}}\n"
        ),
    );
    let execution = execute_native_with(
        &executable,
        &scratch,
        "clock-overflow",
        "off",
        &["--clock=virtual"],
    );
    assert_eq!(execution.process.status.code(), Some(70));
    assert!(
        execution.outcome.contains("\"outcome\":\"defect\"")
            && execution
                .outcome
                .contains("`Clock.sleep(2562047h47m16.854775807s)` overflows the Instant range"),
        "{}",
        execution.outcome
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// time-and-outcomes.md, "The Clock capability": cancelling a sleeping task
// removes its timer and propagates cancellation. Rejects a sleep that ignores
// the cancel and parks again: time would then advance to the child's 100 s
// deadline, and the timer would never be cancelled.
#[test]
fn native_cancelling_a_sleeping_task_removes_its_timer() {
    let scratch = scratch_directory("native-clock-cancel-sleep");
    let executable = build_native(
        &scratch,
        "cancel-sleep",
        &format!(
            "{CLOCK_IMPORTS}pub fn main() -> Int {{\n  \
             let child = Task.start(fn() -> Int {{\n    \
             Clock.sleep(Duration.seconds(100))\n    \
             1\n  }})\n  \
             Clock.sleep(Duration.seconds(1))\n  \
             Task.cancel(child)\n  \
             Clock.sleep(Duration.seconds(1))\n  \
             0\n}}\n"
        ),
    );
    let execution = execute_native_with(
        &executable,
        &scratch,
        "cancel-sleep",
        "full",
        &["--clock=virtual"],
    );
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":0}\n"
    );
    assert_eq!(
        clock_advances(&execution.telemetry),
        [1_000_000_000, 2_000_000_000]
    );
    let cancelled: Vec<_> = execution
        .telemetry
        .lines()
        .filter(|line| line.contains("\"event\":\"clock_timer_cancelled\""))
        .collect();
    assert_eq!(cancelled.len(), 1, "{}", execution.telemetry);
    assert!(
        cancelled[0].contains("\"task_id\":2,") && cancelled[0].contains("\"time_ns\":1000000000,"),
        "{}",
        cancelled[0]
    );
    terminals(&execution.telemetry);
    std::fs::remove_dir_all(scratch).unwrap();
}

// Real-clock sanity check, kept out of the default suite because it waits on
// the wall clock. Run it with
// `cargo test -p chimi --test cli native_real_sleep_waits -- --ignored`.
#[test]
#[ignore = "waits on the wall clock"]
fn native_real_sleep_waits_on_the_wall_clock() {
    let scratch = scratch_directory("native-clock-real-sleep");
    let executable = build_native(
        &scratch,
        "real-sleep",
        &format!(
            "{CLOCK_IMPORTS}pub fn main() -> Int {{\n  Clock.sleep(Duration.milliseconds(200))\n  1\n}}\n"
        ),
    );
    let started = std::time::Instant::now();
    let execution = execute_native(&executable, &scratch, "real-sleep", "boundaries");
    let elapsed = started.elapsed();
    assert_eq!(execution.process.status.code(), Some(0));
    assert!(
        elapsed >= std::time::Duration::from_millis(200),
        "{elapsed:?}"
    );
    assert!(clock_advances(&execution.telemetry).is_empty());
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_task_result_preserves_expected_failure_values() {
    let scratch = scratch_directory("native-task-expected-failure");
    let source = scratch.join("main.chimi");
    let executable = scratch.join("expected-failure-program");
    std::fs::write(
        &source,
        concat!(
            "import std/task as Task\n",
            "enum WorkError { Stopped }\n",
            "fn fail() -> Int { Failure(Stopped()) }\n",
            "pub fn main() -> Int {\n",
            "  let child = Task.start(fail)\n",
            "  Task.result(child)\n",
            "}\n",
        ),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(&source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .expect("build native expected-failure task program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let execution = execute_native(&executable, &scratch, "native-task-failure", "full");
    assert_eq!(execution.process.status.code(), Some(1));
    assert!(
        execution
            .outcome
            .contains("\"outcome\":\"expected_failure\"")
    );
    assert!(execution.outcome.contains("\"type\":\"WorkError\""));
    assert!(execution.outcome.contains("\"variant\":\"Stopped\""));
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_runner_separates_versioned_telemetry_outcome_and_application_streams() {
    let scratch = scratch_directory("runner-channels");
    let executable = scratch.join("answer");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", &fixture("success"), "-o"])
        .arg(&executable)
        .output()
        .expect("build runner-channel fixture");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let default_execution = Command::new(&executable)
        .output()
        .expect("run with the default trace-off policy");
    assert!(default_execution.status.success());
    assert!(default_execution.stdout.is_empty());
    assert!(default_execution.stderr.is_empty());

    let telemetry = scratch.join("telemetry.jsonl");
    let outcome = scratch.join("outcome.jsonl");
    let execution = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=full")
        .arg(format!("--telemetry-file={}", telemetry.display()))
        .arg(format!("--outcome-file={}", outcome.display()))
        .arg("--execution-id=separated-run")
        .arg("--")
        .output()
        .expect("run with separate runner sinks");
    assert!(execution.status.success());
    assert!(execution.stdout.is_empty());
    assert!(execution.stderr.is_empty());

    let telemetry = std::fs::read_to_string(&telemetry).unwrap();
    assert!(telemetry.lines().count() > 3);
    assert!(
        telemetry
            .lines()
            .all(|line| line.starts_with("{\"schema\":\"chimi.runtime.v1\",\"event\":"))
    );
    assert!(telemetry.contains("\"execution_id\":\"separated-run\""));
    assert_eq!(
        std::fs::read_to_string(&outcome).unwrap(),
        concat!(
            "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",",
            "\"value\":{\"type\":\"Answer\",\"fields\":{\"value\":42}}}\n",
        )
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_runner_options_require_the_explicit_configuration_boundary() {
    let scratch = scratch_directory("runner-boundary");
    let executable = scratch.join("answer");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", &fixture("success"), "-o"])
        .arg(&executable)
        .output()
        .expect("build runner-boundary fixture");
    assert!(build.status.success());

    let execution = Command::new(&executable)
        .arg("--trace=full")
        .output()
        .expect("keep unbounded arguments application-owned");
    assert_eq!(execution.status.code(), Some(64));
    assert!(execution.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&execution.stderr)
            .contains("program entry does not accept application arguments")
    );

    let missing_telemetry = Command::new(&executable)
        .args(["--chimi-runner", "--trace=full", "--"])
        .output()
        .expect("reject tracing without a selected sink");
    assert_eq!(missing_telemetry.status.code(), Some(64));
    assert!(
        String::from_utf8_lossy(&missing_telemetry.stderr).contains("requires --telemetry-file")
    );

    let shared_path = scratch.join("shared.jsonl");
    let shared_sinks = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!("--telemetry-file={}", shared_path.display()))
        .arg(format!("--outcome-file={}", shared_path.display()))
        .arg("--")
        .output()
        .expect("reject one file for both runner protocols");
    assert_eq!(shared_sinks.status.code(), Some(64));
    assert!(!shared_path.exists());

    let shared_memory_sink = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!("--outcome-file={}", shared_path.display()))
        .arg(format!("--memory-report-file={}", shared_path.display()))
        .arg("--")
        .output()
        .expect("reject one file for distinct outcome and memory protocols");
    assert_eq!(shared_memory_sink.status.code(), Some(64));
    assert!(!shared_path.exists());

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_main_receives_arguments_and_uses_real_scoped_host_io() {
    let scratch = scratch_directory("host-io");
    let executable = scratch.join("host-io");
    let project = repository_path("compiler/chimi/tests/fixtures/project-host-io");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", "--root"])
        .arg(&project)
        .args(["app/main", "-o"])
        .arg(&executable)
        .output()
        .expect("build host-I/O fixture");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let input = scratch.join("input.txt");
    std::fs::write(&input, "\"alpha\"\r\n\n\"β\"").unwrap();
    let telemetry = scratch.join("telemetry.jsonl");
    let outcome = scratch.join("outcome.jsonl");
    let execution = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=boundaries")
        .arg(format!("--telemetry-file={}", telemetry.display()))
        .arg(format!("--outcome-file={}", outcome.display()))
        .arg("--execution-id=host-io")
        .arg("--")
        .arg(&input)
        .output()
        .expect("execute real host-I/O fixture");

    assert!(execution.status.success());
    assert_eq!(
        String::from_utf8(execution.stdout).unwrap(),
        "\"alpha\"\n\"β\"\n"
    );
    assert_eq!(
        String::from_utf8(execution.stderr).unwrap(),
        "processed input\n"
    );
    assert_eq!(
        std::fs::read_to_string(outcome).unwrap(),
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":{\"type\":\"Void\"}}\n"
    );
    let telemetry = std::fs::read_to_string(telemetry).unwrap();
    assert!(telemetry.contains("\"method\":\"process_utf8_lines\""));
    assert_eq!(telemetry.matches("\"method\":\"write_line\"").count(), 2);
    assert!(telemetry.contains("\"method\":\"write_error\""));

    let stdin_outcome = scratch.join("stdin-outcome.jsonl");
    let mut stdin_execution = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!("--outcome-file={}", stdin_outcome.display()))
        .arg("--")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start stdin host-I/O fixture");
    stdin_execution
        .stdin
        .take()
        .unwrap()
        .write_all("\"from stdin\"\n".as_bytes())
        .unwrap();
    let stdin_execution = stdin_execution.wait_with_output().unwrap();
    assert!(stdin_execution.status.success());
    assert_eq!(stdin_execution.stdout, b"\"from stdin\"\n");

    let missing_outcome = scratch.join("missing-outcome.jsonl");
    let missing = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!("--outcome-file={}", missing_outcome.display()))
        .arg("--")
        .output()
        .expect("run without required application data");
    assert_eq!(missing.status.code(), Some(1));
    assert!(
        std::fs::read_to_string(missing_outcome)
            .unwrap()
            .contains("\"type\":\"UsageError\",\"variant\":\"MissingPath\"")
    );

    let invalid = scratch.join("invalid.txt");
    std::fs::write(&invalid, [0xff]).unwrap();
    let invalid_outcome = scratch.join("invalid-outcome.jsonl");
    let invalid = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!("--outcome-file={}", invalid_outcome.display()))
        .arg("--")
        .arg(&invalid)
        .output()
        .expect("reject invalid UTF-8");
    assert_eq!(invalid.status.code(), Some(1));
    assert!(invalid.stdout.is_empty());
    assert!(
        std::fs::read_to_string(invalid_outcome)
            .unwrap()
            .contains("\"type\":\"Utf8Error\",\"variant\":\"InvalidUtf8\"")
    );

    let malformed = scratch.join("malformed.jsonl");
    std::fs::write(&malformed, "{").unwrap();
    let malformed_outcome = scratch.join("malformed-outcome.jsonl");
    let malformed = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!("--outcome-file={}", malformed_outcome.display()))
        .arg("--")
        .arg(&malformed)
        .output()
        .expect("retain callback failure across allocation cleanup");
    assert_eq!(malformed.status.code(), Some(1));
    assert!(malformed.stdout.is_empty());
    assert!(
        std::fs::read_to_string(malformed_outcome)
            .unwrap()
            .contains("\"type\":\"DecodeError\",\"variant\":\"InvalidJson\"")
    );

    let oversized = scratch.join("oversized.txt");
    std::fs::write(&oversized, "four").unwrap();
    let oversized_outcome = scratch.join("oversized-outcome.jsonl");
    let oversized = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg("--max-line-bytes=3")
        .arg(format!("--outcome-file={}", oversized_outcome.display()))
        .arg("--")
        .arg(&oversized)
        .output()
        .expect("enforce the configured line bound");
    assert_eq!(oversized.status.code(), Some(1));
    assert!(
        std::fs::read_to_string(oversized_outcome)
            .unwrap()
            .contains("\"type\":\"FileError\",\"variant\":\"LineTooLong\"")
    );

    let measure_stream = |name: &str, lines: usize| {
        let input = scratch.join(format!("{name}.jsonl"));
        std::fs::write(&input, "{\"value\":1}\n".repeat(lines)).unwrap();
        let report = scratch.join(format!("{name}-memory.json"));
        let execution = Command::new(&executable)
            .arg("--chimi-runner")
            .arg("--trace=off")
            .arg(format!("--memory-report-file={}", report.display()))
            .arg("--")
            .arg(input)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("measure bounded streaming allocation");
        assert!(execution.success());
        let report = std::fs::read_to_string(report).unwrap();
        (
            json_u64_field(&report, "scoped_total_allocated_bytes"),
            json_u64_field(&report, "scoped_peak_live_bytes"),
            json_u64_field(&report, "scoped_retained_bytes"),
            json_u64_field(&report, "scoped_collections"),
        )
    };
    let small = measure_stream("small", 64);
    let large = measure_stream("large", 16_384);
    assert!(large.0 > small.0 * 100, "{small:?} {large:?}");
    assert_eq!(large.1, small.1, "{small:?} {large:?}");
    assert_eq!(large.2, small.2, "{small:?} {large:?}");
    assert!(large.3 > small.3 * 100, "{small:?} {large:?}");

    std::fs::remove_dir_all(scratch).unwrap();
}

fn json_u64_field(document: &str, name: &str) -> u64 {
    let marker = format!("\"{name}\":");
    document
        .split_once(&marker)
        .unwrap()
        .1
        .split(|character: char| !character.is_ascii_digit())
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn build_stops_before_artifact_emission_when_compilation_fails() {
    let scratch = scratch_directory("invalid-build");
    let executable = scratch.join("missing-main");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", &fixture("missing_main"), "-o"])
        .arg(&executable)
        .output()
        .expect("reject invalid Chimi build");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("error[E5001] execution-setup"));
    assert!(!executable.exists());
    assert!(!scratch.join("target/chimi/missing-main").exists());

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn build_loads_and_links_an_explicit_multi_module_project() {
    let scratch = scratch_directory("project-build");
    let executable = scratch.join("project");
    let root = repository_path("compiler/chimi/tests/fixtures/project");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", "--root"])
        .arg(&root)
        .args(["app/main", "-o"])
        .arg(&executable)
        .output()
        .expect("build multi-module Chimi project");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(report.contains("module-0.o"));
    assert!(report.contains("module-1.o"));
    assert!(report.contains("entry.o"));
    let execution = execute_native(&executable, &scratch, "project-build", "off");
    assert!(execution.process.status.success());
    assert!(execution.telemetry.is_empty());
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn build_executes_virtual_json_and_map_standard_modules() {
    let scratch = scratch_directory("stdlib-project-build");
    let executable = scratch.join("stdlib-project");
    let root = repository_path("compiler/chimi/tests/fixtures/project-stdlib");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", "--root"])
        .arg(&root)
        .args(["app/main", "-o"])
        .arg(&executable)
        .output()
        .expect("build project using virtual standard modules");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let execution = execute_native(&executable, &scratch, "stdlib", "off");
    assert!(execution.process.status.success());
    assert!(execution.process.stderr.is_empty());
    assert_eq!(
        execution.outcome,
        concat!(
            "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",",
            "\"value\":\"{\\\"alpha\\\":1,\\\"zeta\\\":2}\"}\n"
        )
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_concurrency_pipeline_forces_the_connected_runtime() {
    let scratch = scratch_directory("native-concurrency-pipeline");
    let executable = scratch.join("concurrency-pipeline");
    let root = repository_path("programs/concurrency-pipeline");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", "--root"])
        .arg(&root)
        .args(["app/main", "-o"])
        .arg(&executable)
        .output()
        .expect("build the source-level concurrency forcing program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let execution = execute_native(&executable, &scratch, "native-concurrency-pipeline", "full");
    assert!(execution.process.status.success());
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":25}\n"
    );
    for event in [
        "channel_send_parked",
        "channel_receiver_transferred",
        "channel_completion_transferred",
        "task_terminal",
        "dependency_called",
    ] {
        assert!(
            execution
                .telemetry
                .contains(&format!("\"event\":\"{event}\"")),
            "missing {event} in {}",
            execution.telemetry
        );
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_json_decode_reports_malformed_input_as_an_expected_failure() {
    let scratch = scratch_directory("stdlib-malformed-json");
    let project = scratch.join("project");
    std::fs::create_dir_all(project.join("app")).unwrap();
    std::fs::write(
        project.join("app/main.chimi"),
        concat!(
            "import std/json as Json\n",
            "import std/json.{Value}\n\n",
            "pub fn main() -> Value { Json.decode(\"{\") }\n",
        ),
    )
    .unwrap();
    let executable = scratch.join("malformed-json");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", "--root"])
        .arg(&project)
        .args(["app/main", "-o"])
        .arg(&executable)
        .output()
        .expect("build malformed JSON program");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let execution = execute_native(&executable, &scratch, "malformed-json", "off");
    assert_eq!(execution.process.status.code(), Some(1));
    assert!(execution.outcome.starts_with(concat!(
        "{\"schema\":\"chimi.runtime.v1\",",
        "\"outcome\":\"expected_failure\",\"error\":",
        "{\"type\":\"DecodeError\",\"variant\":\"InvalidJson\""
    )));

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_function_values_retain_provisioned_requirements() {
    let scratch = scratch_directory("native-latent-using");
    let project = scratch.join("project");
    std::fs::create_dir_all(project.join("app")).unwrap();
    std::fs::write(
        project.join("app/main.chimi"),
        concat!(
            "interface Source { read: fn() -> Int }\n",
            "const provider = { read: fn() -> Int { 41 } }\n",
            "fn read() -> Int needs { source: Source } { source.read() }\n",
            "fn configured() -> fn(Int) -> Int {\n",
            "  let operation = read using { source: provider }\n",
            "  let offset = 1\n",
            "  fn(value: Int) -> Int { operation() + offset + value }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let operation = configured()\n",
            "  operation(0)\n",
            "}\n",
        ),
    )
    .unwrap();
    let executable = scratch.join("latent-using");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", "--root"])
        .arg(&project)
        .args(["app/main", "-o"])
        .arg(&executable)
        .output()
        .expect("build latent using program");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let execution = execute_native(&executable, &scratch, "latent-using", "off");
    assert!(execution.process.status.success());
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_chimi_code_classifies_and_aggregates_trace_records() {
    let scratch = scratch_directory("trace-summary");
    let executable = scratch.join("chimi-trace-summary");
    let root = repository_path("programs/chimi-trace-summary");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", "--root"])
        .arg(&root)
        .args(["app/main", "-o"])
        .arg(&executable)
        .output()
        .expect("build chimi-trace-summary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let producer = scratch.join("producer");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", &fixture("success"), "-o"])
        .arg(&producer)
        .output()
        .expect("build trace producer");
    assert!(build.status.success());

    let capture = |producer: &Path, name: &str, trace: &str, cancel: bool| {
        let telemetry = scratch.join(format!("{name}-source-telemetry.jsonl"));
        let outcome = scratch.join(format!("{name}-source-outcome.jsonl"));
        let mut command = Command::new(producer);
        command
            .arg("--chimi-runner")
            .arg(format!("--trace={trace}"))
            .arg(format!("--telemetry-file={}", telemetry.display()))
            .arg(format!("--outcome-file={}", outcome.display()))
            .arg(format!("--execution-id=source-{name}"));
        if cancel {
            command.arg("--cancel");
        }
        let output = command.arg("--").output().expect("capture native trace");
        assert!(output.status.code().is_some());
        let input = scratch.join(format!("{name}-input.jsonl"));
        let records = format!(
            "{}{}",
            std::fs::read_to_string(telemetry).unwrap(),
            std::fs::read_to_string(outcome).unwrap()
        );
        std::fs::write(&input, records).unwrap();
        input
    };

    let summarize = |input: &Path, name: &str, trace: &str, max_line_bytes: Option<usize>| {
        let telemetry = scratch.join(format!("{name}-summary-telemetry.jsonl"));
        let outcome = scratch.join(format!("{name}-summary-outcome.jsonl"));
        let mut command = Command::new(&executable);
        command
            .arg("--chimi-runner")
            .arg(format!("--trace={trace}"))
            .arg(format!("--telemetry-file={}", telemetry.display()))
            .arg(format!("--outcome-file={}", outcome.display()))
            .arg(format!("--execution-id=summary-{name}"));
        if let Some(limit) = max_line_bytes {
            command.arg(format!("--max-line-bytes={limit}"));
        }
        let process = command
            .arg("--")
            .arg(input)
            .output()
            .expect("execute chimi-trace-summary");
        NativeExecution {
            process,
            telemetry: std::fs::read_to_string(telemetry).unwrap(),
            outcome: std::fs::read_to_string(outcome).unwrap(),
        }
    };

    let modes = [
        (
            "off",
            concat!(
                "{\"dependency_calls\":{},\"events\":0,\"operations\":{},",
                "\"records\":1,\"schema\":\"chimi.trace-summary.v1\",",
                "\"terminal_outcomes\":{\"success\":1}}\n",
            ),
        ),
        (
            "boundaries",
            concat!(
                "{\"dependency_calls\":{},\"events\":2,\"operations\":{},",
                "\"records\":3,\"schema\":\"chimi.trace-summary.v1\",",
                "\"terminal_outcomes\":{\"success\":1}}\n",
            ),
        ),
        (
            "full",
            concat!(
                "{\"dependency_calls\":{},\"events\":8,",
                "\"operations\":{\"constant\":1,\"construct\":1,\"sequence\":1},",
                "\"records\":9,\"schema\":\"chimi.trace-summary.v1\",",
                "\"terminal_outcomes\":{\"success\":1}}\n",
            ),
        ),
    ];
    for (mode, expected) in modes {
        let input = capture(&producer, mode, mode, false);
        let summary_modes: &[&str] = if mode == "boundaries" {
            &["off", "boundaries", "full"]
        } else {
            &[mode]
        };
        for summary_mode in summary_modes {
            let name = format!("{mode}-under-{summary_mode}");
            let execution = summarize(&input, &name, summary_mode, None);
            assert!(execution.process.status.success(), "{name}");
            assert_eq!(
                String::from_utf8(execution.process.stdout).unwrap(),
                expected
            );
            assert!(execution.process.stderr.is_empty());
            assert!(execution.outcome.contains("\"outcome\":\"success\""));
            if *summary_mode == "off" {
                assert!(execution.telemetry.is_empty());
            } else {
                assert!(!execution.telemetry.is_empty());
            }
        }
    }

    let dependency = scratch.join("dependency-producer");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(repository_path(
            "prototypes/golden/valid/dependencies_and_execution.chimi",
        ))
        .arg("-o")
        .arg(&dependency)
        .output()
        .expect("build dependency trace producer");
    assert!(build.status.success());
    let input = capture(&dependency, "dependency", "boundaries", false);
    let execution = summarize(&input, "dependency", "off", None);
    assert!(execution.process.status.success());
    assert_eq!(
        String::from_utf8(execution.process.stdout).unwrap(),
        concat!(
            "{\"dependency_calls\":{\"inventory.reserve\":1,\"payment.charge\":1},",
            "\"events\":4,\"operations\":{},\"records\":5,",
            "\"schema\":\"chimi.trace-summary.v1\",",
            "\"terminal_outcomes\":{\"success\":1}}\n",
        )
    );

    let defect = scratch.join("defect-producer");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(repository_path(
            "prototypes/backend-comparison/defect.chimi",
        ))
        .arg("-o")
        .arg(&defect)
        .output()
        .expect("build defect trace producer");
    assert!(build.status.success());

    let failure_project = scratch.join("failure-project");
    std::fs::create_dir_all(failure_project.join("app")).unwrap();
    std::fs::write(
        failure_project.join("app/main.chimi"),
        concat!(
            "import std/json as Json\n",
            "import std/json.{Value}\n\n",
            "pub fn main() -> Value { Json.decode(\"{\") }\n",
        ),
    )
    .unwrap();
    let failure = scratch.join("failure-producer");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", "--root"])
        .arg(&failure_project)
        .args(["app/main", "-o"])
        .arg(&failure)
        .output()
        .expect("build expected-failure trace producer");
    assert!(build.status.success());

    let terminal_cases = [
        (&failure, "expected-failure", false, "expected_failure"),
        (&defect, "defect", false, "defect"),
        (&producer, "cancellation", true, "cancellation"),
    ];
    for (producer, name, cancel, outcome) in terminal_cases {
        let input = capture(producer, name, "off", cancel);
        let execution = summarize(&input, name, "off", None);
        assert!(execution.process.status.success(), "{name}");
        assert_eq!(
            String::from_utf8(execution.process.stdout).unwrap(),
            format!(
                concat!(
                    "{{\"dependency_calls\":{{}},\"events\":0,\"operations\":{{}},",
                    "\"records\":1,\"schema\":\"chimi.trace-summary.v1\",",
                    "\"terminal_outcomes\":{{\"{}\":1}}}}\n",
                ),
                outcome
            )
        );
    }

    let allocation_records = 1_024_u64;
    let allocation_input = scratch.join("allocation-input.jsonl");
    let mut allocation_source = String::new();
    for sequence in 0..allocation_records {
        allocation_source.push_str(&format!(
            concat!(
                "{{\"schema\":\"chimi.runtime.v1\",",
                "\"event\":\"operation_completed\",\"sequence\":{},",
                "\"execution_id\":\"allocation\",\"computation\":\"main\",",
                "\"operation\":\"call\"}}\n"
            ),
            sequence
        ));
    }
    allocation_source
        .push_str("{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n");
    std::fs::write(&allocation_input, allocation_source).unwrap();
    let memory_report = scratch.join("allocation-memory.json");
    let allocation = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!("--memory-report-file={}", memory_report.display()))
        .arg("--")
        .arg(&allocation_input)
        .output()
        .expect("measure trace-summary callback allocation");
    assert!(allocation.status.success());
    assert_eq!(
        String::from_utf8(allocation.stdout).unwrap(),
        concat!(
            "{\"dependency_calls\":{},\"events\":1024,",
            "\"operations\":{\"call\":1024},\"records\":1025,",
            "\"schema\":\"chimi.trace-summary.v1\",",
            "\"terminal_outcomes\":{\"success\":1}}\n"
        )
    );
    let memory_report = std::fs::read_to_string(memory_report).unwrap();
    let total_allocated = json_u64_field(&memory_report, "scoped_total_allocated_bytes");
    let accounted_allocated: u64 = [
        "bytes",
        "values",
        "controls",
        "environments",
        "value_pointers",
        "fields",
        "callables",
        "map_nodes",
        "list_nodes",
    ]
    .into_iter()
    .map(|kind| json_u64_field(&memory_report, kind))
    .sum();
    assert_eq!(accounted_allocated, total_allocated);
    let bytes_per_record = total_allocated / (allocation_records + 1);
    assert!(
        bytes_per_record < 45_000,
        "trace summary allocated {bytes_per_record} bytes per record"
    );
    assert!(
        json_u64_field(&memory_report, "scoped_peak_live_bytes") < 50_000,
        "trace summary callback exceeded its live-memory budget"
    );
    assert!(
        json_u64_field(&memory_report, "scoped_retained_bytes") < 2_048,
        "trace summary retained too much callback state"
    );

    let invalid_cases = [
        (
            "malformed",
            "{\n",
            "\"type\":\"TraceError\",\"variant\":\"InvalidJson\",\"fields\":{\"line\":1}",
        ),
        (
            "blank",
            "\n",
            "\"type\":\"TraceError\",\"variant\":\"BlankLine\",\"fields\":{\"line\":1}",
        ),
        (
            "schema",
            "{\"schema\":\"other.v1\",\"outcome\":\"success\"}\n",
            "\"type\":\"TraceError\",\"variant\":\"InvalidSchema\",\"fields\":{\"line\":1}",
        ),
        (
            "event",
            concat!(
                "{\"schema\":\"chimi.runtime.v1\",\"event\":\"unknown\",",
                "\"sequence\":0,\"execution_id\":\"one\",\"computation\":\"main\"}\n",
            ),
            "\"type\":\"TraceError\",\"variant\":\"InvalidEvent\",\"fields\":{\"line\":1}",
        ),
        (
            "missing-outcome",
            concat!(
                "{\"schema\":\"chimi.runtime.v1\",\"event\":\"computation_started\",",
                "\"sequence\":0,\"execution_id\":\"one\",\"computation\":\"main\"}\n",
            ),
            "\"type\":\"TraceError\",\"variant\":\"MissingOutcome\",\"fields\":{\"line\":2}",
        ),
        (
            "mixed-execution",
            concat!(
                "{\"schema\":\"chimi.runtime.v1\",\"event\":\"computation_started\",",
                "\"sequence\":0,\"execution_id\":\"one\",\"computation\":\"main\"}\n",
                "{\"schema\":\"chimi.runtime.v1\",\"event\":\"computation_completed\",",
                "\"sequence\":1,\"execution_id\":\"two\",\"computation\":\"main\"}\n",
            ),
            "\"type\":\"TraceError\",\"variant\":\"MixedExecution\",\"fields\":{\"line\":2}",
        ),
        (
            "sequence",
            concat!(
                "{\"schema\":\"chimi.runtime.v1\",\"event\":\"computation_started\",",
                "\"sequence\":1,\"execution_id\":\"one\",\"computation\":\"main\"}\n",
                "{\"schema\":\"chimi.runtime.v1\",\"event\":\"computation_completed\",",
                "\"sequence\":1,\"execution_id\":\"one\",\"computation\":\"main\"}\n",
            ),
            "\"type\":\"TraceError\",\"variant\":\"InvalidSequence\",\"fields\":{\"line\":2}",
        ),
        (
            "mixed-computation",
            concat!(
                "{\"schema\":\"chimi.runtime.v1\",\"event\":\"computation_started\",",
                "\"sequence\":0,\"execution_id\":\"one\",\"computation\":\"main\"}\n",
                "{\"schema\":\"chimi.runtime.v1\",\"event\":\"computation_completed\",",
                "\"sequence\":1,\"execution_id\":\"one\",\"computation\":\"other\"}\n",
            ),
            "\"type\":\"TraceError\",\"variant\":\"MixedComputation\",\"fields\":{\"line\":2}",
        ),
        (
            "after-outcome",
            concat!(
                "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":1}\n",
                "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":1}\n",
            ),
            "\"type\":\"TraceError\",\"variant\":\"RecordAfterOutcome\",\"fields\":{\"line\":2}",
        ),
    ];
    for (name, source, expected_error) in invalid_cases {
        let input = scratch.join(format!("invalid-{name}.jsonl"));
        std::fs::write(&input, source).unwrap();
        let execution = summarize(&input, &format!("invalid-{name}"), "off", None);
        assert_eq!(execution.process.status.code(), Some(1), "{name}");
        assert!(execution.process.stdout.is_empty(), "{name}");
        assert_eq!(
            execution.process.stderr, b"trace summary failed\n",
            "{name}"
        );
        assert!(execution.outcome.contains(expected_error), "{name}");
    }

    let oversized = scratch.join("oversized-summary-input.jsonl");
    std::fs::write(&oversized, "{\"schema\":\"chimi.runtime.v1\"}\n").unwrap();
    let execution = summarize(&oversized, "oversized", "off", Some(8));
    assert_eq!(execution.process.status.code(), Some(1));
    assert!(execution.process.stdout.is_empty());
    assert_eq!(
        execution.process.stderr,
        b"trace summary input record is too large\n"
    );
    assert!(execution.outcome.contains("\"variant\":\"LineTooLong\""));

    let invalid_utf8 = scratch.join("invalid-utf8-summary-input.jsonl");
    std::fs::write(&invalid_utf8, [0xff]).unwrap();
    let execution = summarize(&invalid_utf8, "invalid-utf8", "off", None);
    assert_eq!(execution.process.status.code(), Some(1));
    assert!(execution.process.stdout.is_empty());
    assert_eq!(
        execution.process.stderr,
        b"trace summary input is not UTF-8\n"
    );
    assert!(execution.outcome.contains("\"variant\":\"InvalidUtf8\""));

    let missing = summarize(
        &scratch.join("does-not-exist.jsonl"),
        "missing-file",
        "off",
        None,
    );
    assert_eq!(missing.process.status.code(), Some(1));
    assert!(missing.process.stdout.is_empty());
    assert_eq!(missing.process.stderr, b"trace summary file IO failed\n");
    assert!(missing.outcome.contains("\"variant\":\"Open\""));

    let usage = Command::new(&executable)
        .output()
        .expect("reject missing application argument");
    assert_eq!(usage.status.code(), Some(1));
    assert!(usage.stdout.is_empty());
    assert_eq!(
        String::from_utf8(usage.stderr).unwrap(),
        "usage: chimi-trace-summary <path-or-dash>\n"
    );

    let extra = Command::new(&executable)
        .args(["one", "two"])
        .output()
        .expect("reject extra application arguments");
    assert_eq!(extra.status.code(), Some(1));
    assert!(extra.stdout.is_empty());
    assert_eq!(
        String::from_utf8(extra.stderr).unwrap(),
        "usage: chimi-trace-summary <path-or-dash>\n"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

/// Writes the two-module build-cache fixture under `scratch/project`.
fn write_two_module_project(scratch: &Path) -> PathBuf {
    let project = scratch.join("project");
    std::fs::create_dir_all(project.join("app")).unwrap();
    std::fs::create_dir_all(project.join("lib")).unwrap();
    std::fs::write(
        project.join("app/main.chimi"),
        "import lib/answer as Answer\n\npub fn main() -> Int {\n  Answer.value()\n}\n",
    )
    .unwrap();
    std::fs::write(
        project.join("lib/answer.chimi"),
        "pub fn value() -> Int {\n  42\n}\n",
    )
    .unwrap();
    project
}

/// Builds `project` from `scratch` (so the project cache is `scratch/target`)
/// with `path` as `PATH`, and returns the build report's `cache` object.
fn build_cache_report(scratch: &Path, project: &Path, executable: &Path, path: &OsStr) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(scratch)
        .env("PATH", path)
        .args(["build", "--root"])
        .arg(project)
        .args(["app/main", "-o"])
        .arg(executable)
        .output()
        .expect("build project");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8(output.stdout).unwrap();
    report.split("\"cache\":").nth(1).unwrap().to_owned()
}

/// The `[...]` list named `field` in a build report's `cache` object.
fn cache_list<'a>(cache: &'a str, field: &str) -> &'a str {
    let start = cache.find(&format!("\"{field}\":[")).unwrap() + field.len() + 4;
    &cache[start..start + cache[start..].find(']').unwrap()]
}

/// The archives in the project's native artifact cache under `scratch`.
fn cached_archives(scratch: &Path) -> Vec<PathBuf> {
    let cache = scratch.join("target/chimi/.cache/native-v1");
    std::fs::read_dir(&cache)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "a"))
        .collect()
}

#[test]
fn a_fresh_project_links_the_compilers_prebuilt_runtime_without_compiling_it() {
    let scratch = scratch_directory("prebuilt-runtime");
    let project = write_two_module_project(&scratch);
    let executable = scratch.join("app");

    let cache = build_cache_report(
        &scratch,
        &project,
        &executable,
        &std::env::var_os("PATH").unwrap(),
    );

    assert_eq!(
        cache_list(&cache, "prebuilt"),
        "\"libchimi_rt.a\"",
        "precondition: build.rs prebuilt the runtime (no `native runtime not prebuilt` \
         cargo warning) and the test-time `cc --version` matches the one build.rs saw \
         (rebuild the chimi crate after a toolchain change); cache report: {cache}"
    );
    assert!(
        !cache_list(&cache, "misses").contains("libchimi_rt"),
        "{cache}"
    );
    assert_eq!(cached_archives(&scratch), Vec::<PathBuf>::new());
    assert!(
        !scratch.join("target/chimi/app/runtime.c").exists(),
        "the runtime source is only written when it is compiled"
    );
    let execution = execute_native(&executable, &scratch, "prebuilt", "off");
    assert!(execution.process.status.success());
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[cfg(unix)]
#[test]
fn a_c_compiler_unlike_the_one_that_prebuilt_the_runtime_compiles_it_per_project() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = scratch_directory("prebuilt-runtime-mismatch");
    let project = write_two_module_project(&scratch);
    let executable = scratch.join("app");
    // A `cc` that reports a different identity and otherwise is the real one:
    // the prebuilt archive's key no longer matches, as after a toolchain update.
    let real_cc = Command::new("sh")
        .args(["-c", "command -v cc"])
        .output()
        .expect("locate cc");
    let real_cc = String::from_utf8(real_cc.stdout).unwrap();
    let bin = scratch.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let wrapper = bin.join("cc");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'updated cc 99'; fi\nexec {} \"$@\"\n",
            real_cc.trim()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut path = bin.into_os_string();
    path.push(":");
    path.push(std::env::var_os("PATH").unwrap());

    let cold = build_cache_report(&scratch, &project, &executable, &path);
    assert_eq!(cache_list(&cold, "prebuilt"), "", "{cold}");
    assert!(
        cache_list(&cold, "misses").contains("libchimi_rt.a"),
        "{cold}"
    );
    assert_eq!(cached_archives(&scratch).len(), 1);
    let execution = execute_native(&executable, &scratch, "fallback", "off");
    assert!(execution.process.status.success());
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"
    );

    let warm = build_cache_report(&scratch, &project, &executable, &path);
    assert!(
        cache_list(&warm, "hits").contains("libchimi_rt.a"),
        "{warm}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn build_reuses_unchanged_artifacts_and_invalidates_only_changed_core_inputs() {
    let scratch = scratch_directory("build-cache");
    let project = scratch.join("project");
    std::fs::create_dir_all(project.join("app")).unwrap();
    std::fs::create_dir_all(project.join("lib")).unwrap();
    std::fs::write(
        project.join("app/main.chimi"),
        "import lib/answer as Answer\n\npub fn main() -> Int {\n  Answer.value()\n}\n",
    )
    .unwrap();
    let answer = project.join("lib/answer.chimi");
    std::fs::write(&answer, "pub fn value() -> Int {\n  42\n}\n").unwrap();
    let executable = scratch.join("cached-app");

    let build = || {
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&scratch)
            .args(["build", "--root"])
            .arg(&project)
            .args(["app/main", "-o"])
            .arg(&executable)
            .output()
            .expect("build cache test project");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };

    let cold = build();
    let cold_cache = cold.split("\"cache\":").nth(1).unwrap();
    assert!(cold_cache.starts_with("{\"hits\":[],\"misses\":["));
    assert_eq!(cold_cache.matches("module-").count(), 2);
    assert!(cold_cache.contains("entry.o"));
    assert!(cold_cache.contains("program_data.o"));

    let unchanged = build();
    let unchanged_cache = unchanged.split("\"cache\":").nth(1).unwrap();
    assert!(unchanged_cache.contains("\"misses\":[],"));
    assert_eq!(unchanged_cache.matches("module-").count(), 2);
    assert!(unchanged_cache.contains("entry.o"));
    assert!(unchanged_cache.contains("program_data.o"));

    std::fs::write(&answer, "pub fn value() -> Int {\n  43\n}\n").unwrap();
    let changed = build();
    let changed_cache = changed.split("\"cache\":").nth(1).unwrap();
    let (hits, misses) = changed_cache.split_once("],\"misses\":[").unwrap();
    assert_eq!(hits.matches("module-").count(), 1);
    assert!(hits.contains("entry.o"));
    assert_eq!(misses.matches("module-").count(), 1);
    assert!(misses.contains("program_data.o"));
    let execution = execute_native(&executable, &scratch, "cache", "off");
    assert!(execution.process.status.success());
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":43}\n"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn direct_integer_build_preserves_off_boundary_and_full_trace_behavior() {
    let scratch = scratch_directory("direct-integer");
    let executable = scratch.join("integer");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .arg("build")
        .arg(repository_path(
            "prototypes/native-direct-lowering/integer-arithmetic.chimi",
        ))
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("build direct integer workload");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let off = execute_native(&executable, &scratch, "direct-off", "off");
    assert!(off.process.status.success());
    assert!(off.telemetry.is_empty());
    assert_eq!(
        off.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":-529}\n"
    );
    let boundaries = execute_native(&executable, &scratch, "direct-boundaries", "boundaries");
    assert_eq!(boundaries.telemetry.lines().count(), 2);
    assert!(
        boundaries
            .telemetry
            .contains("\"event\":\"computation_started\"")
    );
    let full = execute_native(&executable, &scratch, "direct-full", "full");
    assert!(full.telemetry.lines().count() > 200);
    assert!(full.telemetry.contains("\"operation\":\"constant\""));
    assert!(full.telemetry.contains("\"operation\":\"binary\""));
    assert!(full.telemetry.contains("\"operation\":\"unary\""));
    assert_eq!(full.outcome, off.outcome);

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn direct_integer_overflow_returns_the_existing_runtime_defect() {
    let scratch = scratch_directory("direct-integer-overflow");
    let executable = scratch.join("overflow");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .arg("build")
        .arg(repository_path(
            "prototypes/native-direct-lowering/integer-overflow.chimi",
        ))
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("build direct integer overflow workload");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let execution = execute_native(&executable, &scratch, "overflow", "off");
    assert_eq!(execution.process.status.code(), Some(70));
    assert!(execution.outcome.contains("\"outcome\":\"defect\""));
    assert!(
        execution
            .outcome
            .contains("\"message\":\"integer overflow in `+`: 9223372036854775807 + 1\"")
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn run_executes_the_checkout_forcing_program_with_dependency_traces() {
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args([
            "run",
            repository_path("prototypes/golden/valid/dependencies_and_execution.chimi")
                .to_str()
                .unwrap(),
        ])
        .output()
        .expect("run the checkout forcing program");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.matches("\"event\":\"dependency_called\"").count(), 2);
    assert_eq!(
        stdout.lines().last().unwrap(),
        concat!(
            "{\"schema\":\"chimi.runtime.v1\",",
            "\"outcome\":\"success\",\"value\":{\"type\":\"Order\",",
            "\"fields\":{\"id\":{\"type\":\"OrderId\",",
            "\"fields\":{\"value\":\"order-1\"}}}}}"
        )
    );
}

#[test]
fn invalid_programs_stop_at_the_owning_stage() {
    let cases = [
        ("missing_closing_brace", "error[S1001] syntax"),
        // ts-1bba2b: the recovered `[out, 1]` asks for `T = List<T>`, which used
        // to overflow the stack while formatting the mismatch.
        ("list_spread_in_loop", "error[S1001] syntax"),
        ("missing_main", "error[E5001] execution-setup"),
        ("unresolved_constructor", "error[E_UNRESOLVED_VALUE] name"),
        ("invalid_field_type", "error[T4203] type"),
    ];
    for (name, expected) in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .args(["check", &fixture(name)])
            .output()
            .expect("run invalid Chimi source");

        assert_eq!(output.status.code(), Some(1), "fixture {name}");
        assert!(output.stdout.is_empty(), "fixture {name}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "fixture {name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

// N2: `T4312` is a warning, so the check reports it and still succeeds.
#[test]
fn check_reports_warnings_without_failing() {
    let scratch = scratch_directory("check-warning");
    let path = scratch.join("main.chimi");
    std::fs::write(&path, "pub fn main() -> Int { 9223372036854775807 + 1 }\n").unwrap();
    let human = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .arg("check")
        .arg(&path)
        .output()
        .expect("run check");
    assert!(human.status.success());
    let stderr = String::from_utf8(human.stderr).unwrap();
    assert!(stderr.contains("warning[T4312]"), "{stderr}");

    let json = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .arg("check")
        .arg("--json")
        .arg(&path)
        .output()
        .expect("run structured check");
    assert!(json.status.success());
    let document = String::from_utf8(json.stdout).unwrap();
    assert!(document.contains("\"code\":\"T4312\""), "{document}");
    assert!(document.contains("\"severity\":\"warning\""), "{document}");
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn structured_check_keeps_compiler_stages_and_source_ownership_explicit() {
    let cases = [
        ("missing_closing_brace", "syntax", "S1001"),
        ("unresolved_constructor", "name", "E_UNRESOLVED_VALUE"),
        ("invalid_field_type", "type", "T4203"),
    ];
    for (fixture_name, stage, code) in cases {
        let path = fixture(fixture_name);
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .args(["check", "--json", &path])
            .output()
            .expect("run structured check");

        assert_eq!(output.status.code(), Some(1), "fixture {fixture_name}");
        assert!(output.stderr.is_empty(), "fixture {fixture_name}");
        let document = String::from_utf8(output.stdout).unwrap();
        assert!(
            document.starts_with("{\"schema\":\"chimi.diagnostics.v1\","),
            "{document}"
        );
        assert!(document.contains(&format!("\"stage\":\"{stage}\"")));
        assert!(document.contains(&format!("\"code\":\"{code}\"")));
        assert!(document.contains("\"severity\":\"error\""));
        assert!(document.contains("\"primary\":{\"module\":\"main\""));
        assert!(document.contains("\"start\":"));
        assert!(document.contains("\"end\":"));
        assert!(document.contains("\"labels\":[]"));
        assert!(document.contains("\"fixes\":[]"));
        assert!(document.ends_with("\n"));
        if fixture_name == "missing_closing_brace" {
            let end = std::fs::read_to_string(&path).unwrap().len();
            assert_eq!(
                document,
                format!(
                    concat!(
                        "{{\"schema\":\"chimi.diagnostics.v1\",\"diagnostics\":[{{",
                        "\"stage\":\"syntax\",\"code\":\"S1001\",",
                        "\"severity\":\"error\",\"message\":\"expected `}}`\",",
                        "\"primary\":{{\"module\":\"main\",\"path\":\"{}\",",
                        "\"start\":{},\"end\":{}}},\"labels\":[],\"fixes\":[]}}]}}\n"
                    ),
                    path, end, end
                )
            );
        }
    }

    let success = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["check", "--json", &fixture("success")])
        .output()
        .expect("run successful structured check");
    assert!(success.status.success());
    assert!(success.stderr.is_empty());
    assert_eq!(
        String::from_utf8(success.stdout).unwrap(),
        "{\"schema\":\"chimi.diagnostics.v1\",\"diagnostics\":[]}\n"
    );
}

#[test]
fn structured_project_diagnostic_names_the_owning_module() {
    let root = repository_path("compiler/chimi/tests/fixtures/project-invalid");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args([
            "check",
            "--json",
            "--root",
            root.to_str().unwrap(),
            "app/main",
        ])
        .output()
        .expect("check an invalid multi-module project as JSON");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let document = String::from_utf8(output.stdout).unwrap();
    assert!(document.contains("\"module\":\"lib/broken\""));
    assert!(document.contains("lib/broken.chimi"));
}

#[test]
fn inspect_explains_resolved_callable_channels_and_executable_origin() {
    let path = repository_path("prototypes/golden/valid/dependencies_and_execution.chimi");
    let source = std::fs::read_to_string(&path).unwrap();
    let offset = source.rfind("submit(").unwrap().to_string();
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["inspect", path.to_str().unwrap(), &offset])
        .output()
        .expect("inspect the reachable submit call");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let document = String::from_utf8(output.stdout).unwrap();
    assert!(document.starts_with("{\"schema\":\"chimi.inspection.v1\""));
    assert!(document.contains("\"kind\":\"name_reference\""));
    assert!(document.contains("\"text\":\"submit\""));
    assert!(document.contains("\"status\":\"resolved\""));
    assert!(document.contains("\"kind\":\"function\""));
    assert!(document.contains("\"module\":\"main\""));
    assert!(document.contains("\"display\":\"fn(Cart) -> Order errors CheckoutError needs"));
    assert!(document.contains("\"failures\":[\"PaymentFailed\",\"InventoryFailed\"]"));
    assert!(document.contains("\"name\":\"inventory\",\"interface\":\"Inventory\""));
    assert!(!document.contains("\"executable_origin\":null"));
    assert!(document.ends_with("\"diagnostic_count\":0}\n"));
}

#[test]
fn invalid_cli_input_is_rejected_at_the_boundary() {
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["check", "one.chimi", "two.chimi"])
        .output()
        .expect("run invalid chimi check");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        concat!(
            "chimi: check and run received unexpected arguments\n",
            "usage: chimi --version | chimi format <path> | ",
            "chimi inspect <path> <byte-offset> | ",
            "chimi inspect --root <project-dir> <entry-module> <module> <byte-offset> | ",
            "chimi check [--json] <path> | chimi run <path> [-- <argument>...] | ",
            "chimi build <path> [-o <executable>] | ",
            "chimi check [--json] --root <project-dir> <entry-module> | ",
            "chimi run --root <project-dir> <entry-module> [-- <argument>...] | ",
            "chimi build --root <project-dir> <entry-module> ",
            "[-o <executable>] | chimi std check; a compiler developer may put ",
            "--std-dir <directory> before inspect, check, run, build, or std check\n"
        )
    );
}

/// Runs `chimi run <args>` from `scratch` and returns the process output and
/// its last stdout line (the outcome record when the program ran).
fn chimi_run(scratch: &Path, args: &[&str]) -> (Output, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(scratch)
        .arg("run")
        .args(args)
        .output()
        .expect("run chimi run");
    let outcome = String::from_utf8_lossy(&output.stdout)
        .lines()
        .last()
        .map(|line| format!("{line}\n"))
        .unwrap_or_default();
    (output, outcome)
}

#[test]
fn run_passes_arguments_after_the_separator_to_main_like_a_native_build() {
    let scratch = scratch_directory("run-arguments");
    std::fs::write(
        scratch.join("echo.chimi"),
        "pub fn main(arguments: List<Text>) -> List<Text> {\n  arguments\n}\n",
    )
    .unwrap();
    // A second `--` and option-shaped words are application data, not flags.
    let application = ["first", "two words", "--", "-o", ""];
    let mut run_args = vec!["echo.chimi", "--"];
    run_args.extend(application);
    let (interpreted, outcome) = chimi_run(&scratch, &run_args);
    assert!(
        interpreted.status.success(),
        "{}",
        String::from_utf8_lossy(&interpreted.stderr)
    );
    assert_eq!(
        outcome,
        concat!(
            "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",",
            "\"value\":[\"first\",\"two words\",\"--\",\"-o\",\"\"]}\n"
        )
    );

    let executable = scratch.join("echo");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", "echo.chimi", "-o"])
        .arg(&executable)
        .output()
        .expect("build echo");
    assert!(build.status.success());
    let native_outcome = scratch.join("native-outcome.jsonl");
    let native = Command::new(&executable)
        .arg("--chimi-runner")
        .arg(format!("--outcome-file={}", native_outcome.display()))
        .arg("--")
        .args(application)
        .output()
        .expect("run native echo");
    assert!(native.status.success());
    assert_eq!(std::fs::read_to_string(native_outcome).unwrap(), outcome);

    let (empty, empty_outcome) = chimi_run(&scratch, &["echo.chimi"]);
    assert!(empty.status.success());
    assert!(
        empty_outcome.ends_with("\"value\":[]}\n"),
        "{empty_outcome}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn run_rejects_arguments_a_zero_input_main_cannot_take_and_unseparated_arguments() {
    let scratch = scratch_directory("run-argument-errors");
    std::fs::write(
        scratch.join("answer.chimi"),
        "pub fn main() -> Int {\n  42\n}\n",
    )
    .unwrap();

    let (plain, outcome) = chimi_run(&scratch, &["answer.chimi"]);
    assert!(plain.status.success());
    assert_eq!(
        outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"
    );
    let (separated, _) = chimi_run(&scratch, &["answer.chimi", "--"]);
    assert!(separated.status.success());

    // Same status and message as the native runner, and no outcome record.
    let (rejected, _) = chimi_run(&scratch, &["answer.chimi", "--", "extra"]);
    assert_eq!(rejected.status.code(), Some(64));
    assert!(rejected.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&rejected.stderr),
        "chimi: program entry does not accept application arguments\n"
    );

    let (unseparated, _) = chimi_run(&scratch, &["answer.chimi", "extra"]);
    assert_eq!(unseparated.status.code(), Some(2));
    assert!(unseparated.stdout.is_empty());
    assert!(String::from_utf8_lossy(&unseparated.stderr).starts_with(
        "chimi: run received unexpected arguments; pass application arguments after --\n"
    ));

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn explicit_project_root_loads_and_executes_imported_modules() {
    let root = repository_path("compiler/chimi/tests/fixtures/project");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["run", "--root", root.to_str().unwrap(), "app/main"])
        .output()
        .expect("run a multi-module Chimi project");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .last()
            .unwrap(),
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}"
    );
}

#[test]
fn imported_module_diagnostics_render_against_the_owning_source() {
    let root = repository_path("compiler/chimi/tests/fixtures/project-invalid");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["check", "--root", root.to_str().unwrap(), "app/main"])
        .output()
        .expect("check an invalid multi-module Chimi project");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("lib/broken.chimi"), "{stderr}");
    assert!(stderr.contains("error[T"), "{stderr}");
}

#[test]
fn golden_and_recovery_corpus_reaches_the_real_check_path() {
    let cases = [
        ("prototypes/golden/valid/composition.chimi", false),
        (
            "prototypes/golden/valid/dependencies_and_execution.chimi",
            false,
        ),
        (
            "prototypes/golden/valid/expressions_and_matching.chimi",
            false,
        ),
        ("prototypes/golden/valid/functions_as_values.chimi", false),
        ("prototypes/golden/valid/modules/money.chimi", false),
        ("prototypes/golden/valid/modules/use_money.chimi", false),
        ("prototypes/golden/valid/values_and_variants.chimi", false),
        (
            "prototypes/golden/valid/worker_forcing_example.chimi",
            false,
        ),
        ("prototypes/golden/invalid/ignored_value.chimi", false),
        (
            "prototypes/golden/invalid/interface_shape_mismatch.chimi",
            false,
        ),
        ("prototypes/golden/invalid/missing_dependency.chimi", false),
        ("prototypes/golden/invalid/non_exhaustive_case.chimi", false),
        (
            "prototypes/golden/invalid/non_exhaustive_outcome_case.chimi",
            false,
        ),
        (
            "prototypes/golden/invalid/observable_case_guard.chimi",
            false,
        ),
        (
            "prototypes/golden/invalid/private_struct_construction.chimi",
            false,
        ),
        (
            "prototypes/golden/invalid/private_struct_field_access.chimi",
            false,
        ),
        ("prototypes/golden/invalid/empty_braces.chimi", true),
        (
            "prototypes/golden/invalid/implicit_module_alias.chimi",
            true,
        ),
        ("prototypes/golden/invalid/standalone_if.chimi", true),
        ("prototypes/golden/invalid/empty_tuple.chimi", true),
        (
            "prototypes/lossless-parser/cases/incomplete_pipeline.chimi",
            true,
        ),
        (
            "prototypes/lossless-parser/cases/missing_closing_brace.chimi",
            true,
        ),
        (
            "prototypes/lossless-parser/cases/missing_record_colon.chimi",
            true,
        ),
        (
            "prototypes/lossless-parser/cases/missing_record_value.chimi",
            true,
        ),
    ];

    for (relative_path, expects_syntax_error) in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .args(["check", repository_path(relative_path).to_str().unwrap()])
            .output()
            .expect("run corpus source through chimi check");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            stderr.contains("error[S"),
            expects_syntax_error,
            "fixture {relative_path}: {stderr}"
        );
    }
}

/// Each error `chimi check --json` reports, as (stage, code, the source text of
/// its primary span). The human output names only a line and column.
fn check_errors(root: &Path, entry: &str) -> Vec<(String, String, String)> {
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["check", "--json", "--root"])
        .arg(root)
        .arg(entry)
        .output()
        .expect("run chimi check");
    assert!(!output.status.success(), "{entry} was accepted");
    let source = std::fs::read_to_string(root.join(format!("{entry}.chimi"))).unwrap();
    let report = String::from_utf8_lossy(&output.stdout).into_owned();
    // Fields of the first `"key":value` after `from`: a string's text or a number.
    fn field<'report>(report: &'report str, from: usize, key: &str) -> (&'report str, usize) {
        let pattern = format!("\"{key}\":");
        let at = from + report[from..].find(&pattern).unwrap() + pattern.len();
        let rest = &report[at..];
        let value = match rest.strip_prefix('"') {
            Some(text) => &text[..text.find('"').unwrap()],
            None => &rest[..rest.find([',', '}']).unwrap()],
        };
        (value, at)
    }
    let mut errors = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = report[cursor..].find("{\"stage\":") {
        let start_of = cursor + offset;
        let (stage, _) = field(&report, start_of, "stage");
        let (code, _) = field(&report, start_of, "code");
        let (severity, _) = field(&report, start_of, "severity");
        let (_, primary) = field(&report, start_of, "primary");
        let (path, _) = field(&report, primary, "path");
        let (start, _) = field(&report, primary, "start");
        let (end, after) = field(&report, primary, "end");
        cursor = after;
        if severity != "error" {
            continue;
        }
        assert!(
            path.ends_with(&format!("{entry}.chimi")),
            "{entry}: diagnostic outside the fixture: {report}"
        );
        let text =
            source[start.parse::<usize>().unwrap()..end.parse::<usize>().unwrap()].to_owned();
        errors.push((stage.to_owned(), code.to_owned(), text));
    }
    errors
}

// Each negative golden fixture is rejected for the reason it exists, at the
// construct it names, and for nothing else.
//
// Not listed, because the fixture does not reach its EXPECT-DIAGNOSTIC:
// - ignored_value: Chimi has no discarded-value rule yet, and the fixture
//   names the type `Money` without importing it.
// - missing_dependency, interface_shape_mismatch: `Checkout`, `cart`,
//   `Stripe`, `Warehouse`, and `Receipt` are undefined, so name resolution
//   fails first.
#[test]
fn negative_golden_fixtures_are_rejected_with_their_diagnostic() {
    let cases: [(&str, &str, &str, &str); 5] = [
        (
            "non_exhaustive_case",
            "type",
            "T4231",
            "case state {\n    Empty -> True\n    Ready -> True\n  }",
        ),
        (
            "non_exhaustive_outcome_case",
            "type",
            "T4223",
            "case payment.charge(total) {\n    Success(receipt) -> receipt\n  }",
        ),
        (
            "observable_case_guard",
            "type",
            "T4234",
            "risk.check(candidate)",
        ),
        (
            "private_struct_construction",
            "name",
            "E_PRIVATE_STRUCT_CONSTRUCTOR",
            "Money.Money(-100)",
        ),
        (
            "private_struct_field_access",
            "type",
            "E_PRIVATE_STRUCT_FIELD",
            "money.cents",
        ),
    ];
    for (fixture, stage, code, span) in cases {
        let root = scratch_directory(&format!("golden-{fixture}"));
        std::fs::create_dir_all(root.join("modules")).unwrap();
        std::fs::copy(
            repository_path("prototypes/golden/valid/modules/money.chimi"),
            root.join("modules/money.chimi"),
        )
        .unwrap();
        std::fs::copy(
            repository_path(format!("prototypes/golden/invalid/{fixture}.chimi")),
            root.join(format!("{fixture}.chimi")),
        )
        .unwrap();
        assert_eq!(
            check_errors(&root, fixture),
            [(stage.to_owned(), code.to_owned(), span.to_owned())],
            "fixture {fixture}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

fn build_native(scratch: &Path, name: &str, source_text: &str) -> PathBuf {
    let source = scratch.join(format!("{name}.chimi"));
    let executable = scratch.join(name);
    std::fs::write(&source, source_text).unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(scratch)
        .args(["build"])
        .arg(&source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .expect("build native loop program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    executable
}

// Pay-for-play linking, the no-concurrency and tasks-only half of lab MF06 (MF06
// itself waits for process programs, U5): only a program that uses tasks links
// libchimi_fiber.a. `chimi_fiber_create` is defined only in that archive; runtime.c
// has weak fallbacks for the two entry points it calls, not for this one.
#[test]
fn native_only_a_task_program_links_the_fiber_runtime() {
    let scratch = scratch_directory("native-pay-for-play-linking");
    let links_fiber_runtime = |name: &str, source: &str| {
        let source_path = scratch.join(format!("{name}.chimi"));
        let executable = scratch.join(name);
        std::fs::write(&source_path, source).unwrap();
        let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&scratch)
            .arg("build")
            .arg(&source_path)
            .arg("-o")
            .arg(&executable)
            .output()
            .expect("build native program");
        assert!(
            build.status.success(),
            "{}",
            String::from_utf8_lossy(&build.stderr)
        );
        let report = String::from_utf8(build.stdout).unwrap();
        let symbols = Command::new("nm")
            .arg(&executable)
            .output()
            .expect("run nm");
        assert!(symbols.status.success());
        let defined = String::from_utf8(symbols.stdout)
            .unwrap()
            .lines()
            .any(|line| {
                let mut columns = line.split_whitespace().rev();
                matches!(
                    columns.next(),
                    Some("chimi_fiber_create" | "_chimi_fiber_create")
                ) && matches!(columns.next(), Some("T" | "t"))
            });
        let archived = !report.contains("\"fiber_runtime_archive\":null");
        assert_eq!(archived, defined, "{name}: {report}");
        defined
    };
    assert!(!links_fiber_runtime(
        "plain",
        "import std/list as List\npub fn main() -> Int {\n  List.length([1, 2, 3]) - 1\n}\n"
    ));
    assert!(links_fiber_runtime(
        "tasks",
        concat!(
            "import std/task as Task\n",
            "pub fn main() -> Int {\n",
            "  let child = Task.start(fn() -> Int { 41 })\n",
            "  Task.result(child) + 1\n",
            "}\n",
        )
    ));
    std::fs::remove_dir_all(scratch).unwrap();
}

// Guards: native loops run passes without growing the C stack (recursion overflowed here).
#[test]
fn native_loop_runs_a_million_passes() {
    let scratch = scratch_directory("native-loop-million");
    let executable = build_native(
        &scratch,
        "loop-million",
        concat!(
            "pub fn main() -> Int {\n",
            "  loop count = 0 {\n",
            "    case count {\n",
            "      1000000 -> break count\n",
            "      _ -> count + 1\n",
            "    }\n",
            "  }\n",
            "}\n",
        ),
    );
    let execution = execute_native(&executable, &scratch, "loop-million", "off");
    assert!(
        execution.process.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.process.stderr)
    );
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":1000000}\n"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// Guards: a loop drives a real channel consumer until Drained, across task parking.
#[test]
fn native_loop_consumes_a_channel_until_drained() {
    let scratch = scratch_directory("native-loop-channel");
    let executable = build_native(
        &scratch,
        "loop-channel",
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Receiver, Receive}\n",
            "import std/task as Task\n",
            "fn total(jobs: Receiver<Int>) -> Int {\n",
            "  loop sum = 0 {\n",
            "    let next = Channel.receive(jobs)\n",
            "    case next {\n",
            "      Receive.Received(value) -> sum + value\n",
            "      Receive.Drained -> break sum\n",
            "    }\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let jobs = Channel.bounded(1)\n",
            "  let receiver = jobs.receiver\n",
            "  let sender = jobs.sender\n",
            "  let completion = jobs.completion\n",
            "  let child = Task.start(fn() -> Int { total(receiver) })\n",
            "  Channel.send(sender, 1)\n",
            "  Channel.send(sender, 2)\n",
            "  Channel.send(sender, 3)\n",
            "  Channel.send(sender, 4)\n",
            "  Channel.send(sender, 5)\n",
            "  Channel.close(completion)\n",
            "  Task.result(child)\n",
            "}\n",
        ),
    );
    let execution = execute_native(&executable, &scratch, "loop-channel", "off");
    assert!(
        execution.process.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.process.stderr)
    );
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":15}\n"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// Guards: without a scheduler, each pass's garbage is reclaimed, so live memory stays
// bounded by one pass instead of growing with the pass count.
#[test]
fn native_scheduler_free_loop_reclaims_each_pass() {
    let scratch = scratch_directory("native-loop-reclaim");
    let passes = 100_000_u64;
    let executable = build_native(
        &scratch,
        "loop-reclaim",
        &format!(
            concat!(
                "pub fn main() -> Int {{\n",
                // The binding keeps this loop on the runtime path: an integer loop
                // with no binding runs as direct machine passes, which allocate nothing.
                "  loop count = 0 {{\n",
                "    let next = count + 1\n",
                "    case count {{\n",
                "      {passes} -> break count\n",
                "      _ -> next\n",
                "    }}\n",
                "  }}\n",
                "}}\n",
            ),
            passes = passes
        ),
    );
    let memory_report = scratch.join("loop-memory.json");
    let execution = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!("--memory-report-file={}", memory_report.display()))
        .arg("--")
        .output()
        .expect("run native loop with a memory report");
    assert!(
        execution.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.stderr)
    );
    let memory_report = std::fs::read_to_string(memory_report).unwrap();
    assert!(json_u64_field(&memory_report, "scoped_collections") >= passes);
    let peak_live = json_u64_field(&memory_report, "scoped_peak_live_bytes");
    assert!(
        peak_live < 16_384,
        "loop kept {peak_live} live bytes across {passes} passes"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// Guards: an integer loop with no binding runs as direct machine passes (CHI-90): it
// opens no allocation region, so its passes leave no collections in the memory report,
// and it still returns the runtime loop's value.
#[test]
fn native_integer_loop_runs_as_direct_passes() {
    let scratch = scratch_directory("native-direct-loop");
    let executable = build_native(
        &scratch,
        "direct-loop",
        concat!(
            "pub fn main() -> Int {\n",
            "  loop count = 0 {\n",
            "    case count >= 100000 {\n",
            "      True -> break count\n",
            "      False -> count + 1\n",
            "    }\n",
            "  }\n",
            "}\n",
        ),
    );
    let memory_report = scratch.join("direct-loop-memory.json");
    let outcome = scratch.join("direct-loop-outcome.json");
    let execution = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!("--memory-report-file={}", memory_report.display()))
        .arg(format!("--outcome-file={}", outcome.display()))
        .arg("--")
        .output()
        .expect("run native direct loop");
    assert!(
        execution.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(outcome).unwrap(),
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":100000}\n"
    );
    let memory_report = std::fs::read_to_string(memory_report).unwrap();
    assert_eq!(json_u64_field(&memory_report, "scoped_collections"), 0);
    std::fs::remove_dir_all(scratch).unwrap();
}

// Guards: tracing keeps an integer loop on the runtime path, so a full trace still
// shows every pass's operations even though the loop could run as direct passes.
#[test]
fn native_traced_integer_loop_reports_every_pass() {
    let scratch = scratch_directory("native-direct-loop-trace");
    let executable = build_native(
        &scratch,
        "direct-loop-trace",
        concat!(
            "pub fn main() -> Int {\n",
            "  loop count = 0 {\n",
            "    case count >= 5 {\n",
            "      True -> break count\n",
            "      False -> count + 1\n",
            "    }\n",
            "  }\n",
            "}\n",
        ),
    );
    let execution = execute_native(&executable, &scratch, "direct-loop-trace", "full");
    assert!(
        execution.process.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.process.stderr)
    );
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":5}\n"
    );
    let passes = execution
        .telemetry
        .lines()
        .filter(|line| {
            line.contains("\"event\":\"operation_started\"")
                && line.contains("\"operation\":\"match\"")
        })
        .count();
    assert_eq!(passes, 6, "{}", execution.telemetry);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_list_is_a_persistent_vector() {
    let scratch = scratch_directory("native-list-runtime");
    let executable = scratch.join("native-list-runtime");
    let compilation = Command::new("cc")
        .arg("-std=c11")
        .arg("-O2")
        .arg("-I")
        .arg(repository_path("compiler/chimi/runtime/native"))
        .arg("-I")
        .arg(repository_path("runtime/chimi-fiber/include"))
        .arg(format!(
            "{}/tests/fixtures/native-list-runtime.c",
            env!("CARGO_MANIFEST_DIR")
        ))
        .arg("-o")
        .arg(&executable)
        // glibc keeps floor, fmod, and the rest of libm out of libc.
        .arg("-lm")
        .output()
        .expect("compile native list runtime test");
    assert!(
        compilation.status.success(),
        "{}",
        String::from_utf8_lossy(&compilation.stderr)
    );
    let run = Command::new(&executable)
        .output()
        .expect("run native list runtime test");
    let stdout = String::from_utf8(run.stdout).unwrap();
    assert!(
        run.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(stdout.ends_with("ok\n"), "{stdout}");
    std::fs::remove_dir_all(scratch).unwrap();
}

// runtime.c's Map tree, which Set shares, keeps the AVL height bound when
// removals empty most of it (the interpreter's twin is execution::tests).
#[test]
fn native_map_removal_keeps_the_tree_balanced() {
    let scratch = scratch_directory("native-map-runtime");
    let executable = scratch.join("native-map-runtime");
    let compilation = Command::new("cc")
        .arg("-std=c11")
        .arg("-O2")
        .arg("-I")
        .arg(repository_path("compiler/chimi/runtime/native"))
        .arg("-I")
        .arg(repository_path("runtime/chimi-fiber/include"))
        .arg(format!(
            "{}/tests/fixtures/native-map-runtime.c",
            env!("CARGO_MANIFEST_DIR")
        ))
        .arg("-o")
        .arg(&executable)
        // glibc keeps floor, fmod, and the rest of libm out of libc.
        .arg("-lm")
        .output()
        .expect("compile native map runtime test");
    assert!(
        compilation.status.success(),
        "{}",
        String::from_utf8_lossy(&compilation.stderr)
    );
    let run = Command::new(&executable)
        .output()
        .expect("run native map runtime test");
    let stdout = String::from_utf8(run.stdout).unwrap();
    assert!(
        run.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(stdout.ends_with("ok\n"), "{stdout}");
    std::fs::remove_dir_all(scratch).unwrap();
}

// The prototype's heap scenarios against runtime.c: pass cost independent of old
// survivors, flat memory over a million passes with bounded full passes, regions
// and adoption composing, sharing-preserving and window-compacting copies,
// non-consuming copies for Task.result, foreign pointers reported, small queued
// messages, and teardown returning every allocation. AddressSanitizer turns a
// freed-but-reachable value into a failure even when its bytes still look right.
#[test]
fn native_per_owner_heaps_meet_the_prototype_heap_contracts() {
    let scratch = scratch_directory("native-heap");
    let executable = scratch.join("native-heap");
    let compile = |flags: &[&str]| {
        Command::new("cc")
            .arg("-std=c11")
            .arg("-O1")
            .arg("-g")
            .args(flags)
            .arg("-I")
            .arg(repository_path("compiler/chimi/runtime/native"))
            .arg("-I")
            .arg(repository_path("runtime/chimi-fiber/include"))
            .arg(format!(
                "{}/tests/fixtures/native-heap.c",
                env!("CARGO_MANIFEST_DIR")
            ))
            .arg("-o")
            .arg(&executable)
            // glibc keeps floor, fmod, and the rest of libm out of libc.
            .arg("-lm")
            .output()
            .expect("compile native heap test")
    };
    let sanitized = compile(&["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
    if !sanitized.status.success() {
        let plain = compile(&[]);
        assert!(
            plain.status.success(),
            "{}",
            String::from_utf8_lossy(&plain.stderr)
        );
    }
    let run = Command::new(&executable)
        .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=0")
        .env("UBSAN_OPTIONS", "halt_on_error=1:print_stacktrace=1")
        .output()
        .expect("run native heap test");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        run.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(stdout.ends_with("ok\n"), "{stdout}");
    std::fs::remove_dir_all(scratch).unwrap();
}

// E18 on the native path: structural comparison of a list nested a million levels
// deep keeps its work stack on the heap. Chimi programs cannot build such a value
// in reasonable time yet, so this drives chimi_value_compare directly.
#[test]
fn native_structural_comparison_handles_deep_nesting_and_exact_json_numbers() {
    let scratch = scratch_directory("native-structural-compare");
    let executable = scratch.join("native-structural-compare");
    let compilation = Command::new("cc")
        .arg("-std=c11")
        .arg("-O2")
        .arg("-I")
        .arg(repository_path("compiler/chimi/runtime/native"))
        .arg("-I")
        .arg(repository_path("runtime/chimi-fiber/include"))
        .arg(format!(
            "{}/tests/fixtures/native-structural-compare.c",
            env!("CARGO_MANIFEST_DIR")
        ))
        .arg("-o")
        .arg(&executable)
        // glibc keeps floor, fmod, and the rest of libm out of libc.
        .arg("-lm")
        .output()
        .expect("compile native structural compare test");
    assert!(
        compilation.status.success(),
        "{}",
        String::from_utf8_lossy(&compilation.stderr)
    );
    let run = Command::new(&executable)
        .output()
        .expect("run native structural compare test");
    let stdout = String::from_utf8(run.stdout).unwrap();
    assert!(
        run.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(stdout, "ok\n");
    std::fs::remove_dir_all(scratch).unwrap();
}

// Guards: native list literals, decoded JSON arrays, and List.first/length agree
// past the 32-element boundary where the persistent vector leaves its tail.
#[test]
fn native_lists_longer_than_one_vector_leaf_round_trip_through_json() {
    let scratch = scratch_directory("native-long-list");
    let elements = (0..40)
        .map(|index| format!("Json.int({index})"))
        .collect::<Vec<_>>()
        .join(", ");
    let source = format!(
        concat!(
            "import std/json as Json\n",
            "import std/json.{{Value}}\n",
            "import std/list as List\n",
            "\n",
            "fn summarize(values: List<Value>) -> Text {{\n",
            "  case List.first(values) {{\n",
            "    Some(first) -> Json.encode(Json.array([Json.array(values), first, Json.int(List.length(values))]))\n",
            "    None -> \"empty\"\n",
            "  }}\n",
            "}}\n",
            "\n",
            "pub fn main() -> Text {{\n",
            "  let decoded = Json.decode(Json.encode(Json.array([{elements}])))\n",
            "  case decoded {{\n",
            "    Value.Array(values) -> summarize(values)\n",
            "    _ -> \"unexpected\"\n",
            "  }}\n",
            "}}\n",
        ),
        elements = elements
    );
    let executable = build_native(&scratch, "long-list", &source);
    let execution = execute_native(&executable, &scratch, "long-list", "off");
    assert!(
        execution.process.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.process.stderr)
    );
    let numbers = (0..40).map(|index| index.to_string()).collect::<Vec<_>>();
    assert_eq!(
        execution.outcome,
        format!(
            "{{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":\"[[{}],0,40]\"}}\n",
            numbers.join(",")
        )
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// Guards B5: a skipped `and` operand emits no channel events. Native-only: the
// reference interpreter has no Channel runtime.
#[test]
fn native_skipped_boolean_operand_sends_nothing() {
    let scratch = scratch_directory("native-boolean-short-circuit");
    let executable = build_native(
        &scratch,
        "boolean-short-circuit",
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Sender}\n",
            "fn consume(sender: Sender<Int>) -> Bool {\n",
            "  Channel.send(sender, 1)\n",
            "  True\n",
            "}\n",
            "pub fn main() -> Bool {\n",
            "  let channel = Channel.bounded(2)\n",
            "  let skipped = False and consume(channel.sender)\n",
            "  let taken = True and consume(channel.sender)\n",
            "  skipped or not taken\n",
            "}\n",
        ),
    );
    let execution = execute_native(&executable, &scratch, "boolean-short-circuit", "full");
    assert!(
        execution.process.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.process.stderr)
    );
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":false}\n"
    );
    let sends = execution
        .telemetry
        .matches("\"event\":\"channel_send_accepted\"")
        .count();
    assert_eq!(sends, 1, "{}", execution.telemetry);
    std::fs::remove_dir_all(scratch).unwrap();
}

// Guards B8: an operand that parks on Channel.receive waits like a `case` subject,
// then the expression completes. Native-only: the interpreter has no Channel runtime.
#[test]
fn native_boolean_operand_parks_on_receive_then_succeeds() {
    let scratch = scratch_directory("native-boolean-parks");
    let executable = build_native(
        &scratch,
        "boolean-parks",
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Receive, Receiver}\n",
            "import std/task as Task\n",
            "fn got_one(receiver: Receiver<Int>) -> Bool {\n",
            "  let next = Channel.receive(receiver)\n",
            "  case next {\n",
            "    Receive.Received(value) -> value < 2 and value > 0\n",
            "    Receive.Drained -> False\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Bool {\n",
            "  let channel = Channel.bounded(0)\n",
            "  let sender = channel.sender\n",
            "  let receiver = channel.receiver\n",
            "  let completion = channel.completion\n",
            "  let child = Task.start(fn() -> Int {\n",
            "    Channel.send(sender, 1)\n",
            "    7\n",
            "  })\n",
            "  let ready = True\n",
            "  let received = ready and got_one(receiver)\n",
            "  let child_value = Task.result(child)\n",
            "  Channel.close(completion)\n",
            "  received and child_value > 0\n",
            "}\n",
        ),
    );
    let execution = execute_native(&executable, &scratch, "boolean-parks", "full");
    assert!(
        execution.process.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.process.stderr)
    );
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":true}\n"
    );
    assert!(
        execution
            .telemetry
            .contains("\"event\":\"channel_receive_parked\"")
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

/// A program that decodes 2^20 = 1,048,576 JSON ones into `values` and passes them to
/// `run`. Chimi has no list-building primitive yet, so a large list comes from
/// doubling a text and decoding it, which is linear in the final length.
fn million_element_list_program(run: &str) -> String {
    format!(
        concat!(
            "import std/json as Json\n",
            "import std/json.{{Value}}\n",
            "import std/text as Text\n",
            "fn ones(doublings: Int) -> Text {{\n",
            "  loop state = {{ text: \"1\", left: doublings }} {{\n",
            "    case state.left {{\n",
            "      0 -> break state.text\n",
            "      _ -> {{ text: Text.append(Text.append(state.text, \",\"), state.text), left: state.left - 1 }}\n",
            "    }}\n",
            "  }}\n",
            "}}\n",
            "{run}",
            "pub fn main() -> Int {{\n",
            "  let decoded = Json.decode(Text.append(Text.append(\"[\", ones(20)), \"]\"))\n",
            "  case decoded {{\n",
            "    Value.Array(values) -> run(values)\n",
            "    _ -> -1\n",
            "  }}\n",
            "}}\n",
        ),
        run = run
    )
}

/// Runs `executable` and returns its outcome line and memory report.
fn execute_with_memory_report(executable: &Path, scratch: &Path) -> (String, String) {
    let outcome = scratch.join("outcome.jsonl");
    let memory_report = scratch.join("memory.json");
    let execution = Command::new(executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!("--outcome-file={}", outcome.display()))
        .arg(format!("--memory-report-file={}", memory_report.display()))
        .arg("--")
        .output()
        .expect("run native program with a memory report");
    assert!(
        execution.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.stderr)
    );
    (
        std::fs::read_to_string(outcome).unwrap(),
        std::fs::read_to_string(memory_report).unwrap(),
    )
}

/// The concurrency-pipeline forcing program with `jobs` jobs: the root sends each
/// job and receives its result in a loop while a worker task loops over the jobs.
fn pipeline_program(jobs: u64) -> String {
    format!(
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{{Completion, Receiver, Receive, Sender}}\n",
            "import std/task as Task\n",
            "fn worker(jobs: Receiver<Int>, results: Sender<Int>, completion: Completion<Int>) -> Int {{\n",
            "  loop processed = 0 {{\n",
            "    let next = Channel.receive(jobs)\n",
            "    case next {{\n",
            "      Receive.Received(value) -> {{\n",
            "        Channel.send(results, value + 10)\n",
            "        processed + 1\n",
            "      }}\n",
            "      Receive.Drained -> {{\n",
            "        Channel.close(completion)\n",
            "        break processed\n",
            "      }}\n",
            "    }}\n",
            "  }}\n",
            "}}\n",
            "pub fn main() -> Int {{\n",
            "  let jobs = Channel.bounded(1)\n",
            "  let results = Channel.bounded(1)\n",
            "  let job_sender = jobs.sender\n",
            "  let job_receiver = jobs.receiver\n",
            "  let job_completion = jobs.completion\n",
            "  let result_sender = results.sender\n",
            "  let result_receiver = results.receiver\n",
            "  let result_completion = results.completion\n",
            "  let child = Task.start(fn() -> Int {{ worker(job_receiver, result_sender, result_completion) }})\n",
            "  let total = loop state = {{ next: 0, sum: 0 }} {{\n",
            "    case state.next {{\n",
            "      {jobs} -> break state.sum\n",
            "      _ -> {{\n",
            "        Channel.send(job_sender, state.next)\n",
            "        let reply = Channel.receive(result_receiver)\n",
            "        case reply {{\n",
            "          Receive.Received(value) -> {{ next: state.next + 1, sum: state.sum + value }}\n",
            "          Receive.Drained -> break state.sum\n",
            "        }}\n",
            "      }}\n",
            "    }}\n",
            "  }}\n",
            "  Channel.close(job_completion)\n",
            "  total + Task.result(child)\n",
            "}}\n",
        ),
        jobs = jobs
    )
}

// ts-9b1ec1: a loop that nests its state one level deeper on every pass marks only
// what the pass allocated, plus full passes at geometric growth points, so marking
// work is linear in the depth. Marking the whole state on every pass, as before
// generational regions, scans about depth^2 / 2 allocations: 16 times as many for
// four times the depth.
#[test]
fn native_marking_work_is_linear_in_the_nesting_depth_a_loop_builds() {
    let measure = |depth: u64| {
        let scratch = scratch_directory(&format!("native-deep-marking-{depth}"));
        let source = format!(
            concat!(
                "enum Nest {{\n  Leaf\n  Wrap(children: List<Nest>)\n}}\n",
                "struct Build {{ depth: Int, nest: Nest }}\n",
                "pub fn main() -> Bool {{\n",
                "  let built = loop state = Build({depth}, Leaf) {{\n",
                "    case state.depth {{\n",
                "      0 -> break state.nest\n",
                "      _ -> Build(state.depth - 1, Wrap([state.nest]))\n",
                "    }}\n",
                "  }}\n",
                "  built != Leaf\n",
                "}}\n",
            ),
            depth = depth
        );
        let executable = build_native(&scratch, "deep", &source);
        let (outcome, report) = execute_with_memory_report(&executable, &scratch);
        assert!(outcome.contains("\"value\":true"), "{outcome}");
        std::fs::remove_dir_all(scratch).unwrap();
        json_u64_field(&report, "scoped_scanned_allocations")
    };
    let shallow = measure(5_000);
    let deep = measure(20_000);
    assert!(
        deep <= 8 * shallow,
        "20,000 levels scanned {deep} allocations, 5,000 levels {shallow}"
    );
}

// ts-51bbf7 (CHI-176): a native environment holds the locals of one body, so a
// call's environment costs the same however many locals other functions declare.
// Before, every environment had one slot per local in the whole program, so
// adding a reachable function with 400 locals made each of the loop's 2,000 calls
// allocate 3.2 KB more.
#[test]
fn native_call_environments_do_not_grow_with_other_functions_locals() {
    let program = |wide_locals: usize| {
        let mut wide = String::from("fn wide(x: Int) -> Int {\n  let a0 = x\n");
        for index in 1..wide_locals {
            wide.push_str(&format!("  let a{index} = a{} + 1\n", index - 1));
        }
        wide.push_str(&format!("  a{}\n}}\n", wide_locals - 1));
        format!(
            concat!(
                "{}",
                "fn step(i: Int) -> Int {{ i + 1 }}\n",
                "pub fn main() -> Int {{\n",
                "  let start = wide(0)\n",
                "  loop i = start {{\n",
                "    case i > 2000 {{\n",
                "      True -> break i\n",
                "      False -> step(i)\n",
                "    }}\n",
                "  }}\n",
                "}}\n",
            ),
            wide
        )
    };
    let measure = |wide_locals: usize| {
        let scratch = scratch_directory(&format!("native-environment-size-{wide_locals}"));
        let executable = build_native(&scratch, "environments", &program(wide_locals));
        let (outcome, report) = execute_with_memory_report(&executable, &scratch);
        assert!(outcome.contains("\"value\":2001"), "{outcome}");
        std::fs::remove_dir_all(scratch).unwrap();
        json_u64_field(&report, "scoped_total_allocated_bytes")
    };
    let narrow = measure(2);
    let wide = measure(400);
    // The wide function runs once; its own 400 locals cost a few KB, not 2,000
    // calls' worth.
    assert!(
        wide < narrow + 64 * 1024,
        "2 locals: {narrow} bytes; 400 locals: {wide} bytes"
    );
}

// ts-6ccbc9 / ts-df67fb: with per-owner heaps, loops reclaim under the scheduler, so
// the pipeline's memory does not grow with the number of jobs. Before, every pass's
// allocations stayed live until exit once a program used tasks or channels.
#[test]
fn native_concurrency_pipeline_runs_in_flat_memory() {
    let measure = |jobs: u64| {
        let scratch = scratch_directory(&format!("native-pipeline-memory-{jobs}"));
        let executable = build_native(&scratch, "pipeline", &pipeline_program(jobs));
        let (outcome, report) = execute_with_memory_report(&executable, &scratch);
        let expected = jobs * (jobs - 1) / 2 + 11 * jobs;
        assert!(
            outcome.contains(&format!("\"value\":{expected}")),
            "{outcome}"
        );
        std::fs::remove_dir_all(scratch).unwrap();
        report
    };
    let small = measure(50);
    let large = measure(5_000);
    // Nothing a pass allocates outlives the program's loops.
    assert_eq!(
        json_u64_field(&large, "unscoped_allocated_bytes"),
        json_u64_field(&small, "unscoped_allocated_bytes"),
        "small {small}\nlarge {large}"
    );
    assert_eq!(
        json_u64_field(&large, "scoped_retained_bytes"),
        json_u64_field(&small, "scoped_retained_bytes"),
        "small {small}\nlarge {large}"
    );
    assert!(json_u64_field(&large, "scoped_collections") >= 2 * 5_000);
    // Every heap and every in-transit message together, headers included.
    let small_peak = json_u64_field(&small, "peak_footprint_bytes");
    let large_peak = json_u64_field(&large, "peak_footprint_bytes");
    assert!(
        large_peak <= small_peak + 4_096,
        "peak footprint grew from {small_peak} to {large_peak} bytes"
    );
    assert!(
        json_u64_field(&large, "peak_fragment_footprint_bytes")
            <= json_u64_field(&small, "peak_fragment_footprint_bytes"),
        "small {small}\nlarge {large}"
    );
}

/// A Chimi function `payload()` returning a Text of about 16 KiB, so a message
/// that leaks shows up in the fragment footprint.
const PAYLOAD_FUNCTION: &str = concat!(
    "import std/text as Text\n",
    "fn payload() -> Text {\n",
    "  loop state = { text: \"1\", left: 13 } {\n",
    "    case state.left {\n",
    "      0 -> break state.text\n",
    "      _ -> { text: Text.append(Text.append(state.text, \",\"), state.text), left: state.left - 1 }\n",
    "    }\n",
    "  }\n",
    "}\n",
);

/// Builds `source` and returns the memory report of one run with `runner` options
/// and `rounds` application arguments.
fn memory_report_over_rounds(name: &str, source: &str, runner: &[&str], rounds: usize) -> String {
    let scratch = scratch_directory(&format!("{name}-{rounds}"));
    let executable = build_native(&scratch, name, source);
    let memory_report = scratch.join("memory.json");
    let execution = Command::new(&executable)
        .arg("--chimi-runner")
        .args(runner)
        .arg(format!("--memory-report-file={}", memory_report.display()))
        .arg("--")
        .args(std::iter::repeat_n("round", rounds))
        .output()
        .expect("run native program with a memory report");
    assert!(
        execution.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.stderr)
    );
    let report = std::fs::read_to_string(memory_report).unwrap();
    std::fs::remove_dir_all(scratch).unwrap();
    report
}

fn assert_flat_fragment_peak(small: &str, large: &str) {
    assert_eq!(
        json_u64_field(large, "peak_fragment_footprint_bytes"),
        json_u64_field(small, "peak_fragment_footprint_bytes"),
        "small {small}\nlarge {large}"
    );
}

// Guards: a child's result fragment was never freed, so every execution kept its
// children's results until the process exited. The scheduler now releases them
// when the execution ends, and the benchmark runner's repeats stay flat.
#[test]
fn native_task_results_are_freed_when_the_execution_ends() {
    let source = format!(
        concat!(
            "import std/task as Task\n",
            "{payload}",
            "pub fn main() -> Int {{\n",
            "  let child = Task.start(fn() -> Text {{ payload() }})\n",
            "  let first = Task.result(child)\n",
            "  let second = Task.result(child)\n",
            "  2\n",
            "}}\n",
        ),
        payload = PAYLOAD_FUNCTION
    );
    let measure = |repeats: usize| {
        let option = format!("--benchmark-repeats={repeats}");
        memory_report_over_rounds("task-result-release", &source, &[option.as_str()], 0)
    };
    assert_flat_fragment_peak(&measure(2), &measure(20));
}

// Guards: a message parked by a send whose task is cancelled stayed allocated until
// exit, so a loop that cancels a blocked producer each round grew without bound.
#[test]
fn native_cancelled_sends_free_their_messages() {
    let source = format!(
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{{Receive, Sender}}\n",
            "import std/task as Task\n",
            "import std/list as List\n",
            "{payload}",
            "fn spam(out: Sender<Text>, message: Text) -> Int {{\n",
            "  loop sent = 0 {{\n",
            "    Channel.send(out, message)\n",
            "    sent + 1\n",
            "  }}\n",
            "}}\n",
            "pub fn main(arguments: List<Text>) -> Int {{\n",
            "  let message = payload()\n",
            "  let rounds = List.length(arguments)\n",
            "  loop state = {{ round: 0, received: 0 }} {{\n",
            "    case state.round < rounds {{\n",
            "      False -> break state.received\n",
            "      True -> {{\n",
            "        let channel = Channel.bounded(0)\n",
            "        let sender = channel.sender\n",
            "        let receiver = channel.receiver\n",
            "        let producer = Task.start(fn() -> Int {{ spam(sender, message) }})\n",
            "        let got = Channel.receive(receiver)\n",
            "        let received = case got {{\n",
            "          Receive.Received(_) -> 1\n",
            "          Receive.Drained -> 0\n",
            "        }}\n",
            "        Task.cancel(producer)\n",
            "        {{ round: state.round + 1, received: state.received + received }}\n",
            "      }}\n",
            "    }}\n",
            "  }}\n",
            "}}\n",
        ),
        payload = PAYLOAD_FUNCTION
    );
    let measure = |rounds| memory_report_over_rounds("cancelled-send", &source, &[], rounds);
    assert_flat_fragment_peak(&measure(2), &measure(40));
}

// Guards: messages queued in a channel that is aborted stayed allocated until exit.
#[test]
fn native_aborting_a_channel_frees_its_queued_messages() {
    let source = format!(
        concat!(
            "import std/channel as Channel\n",
            "import std/list as List\n",
            "{payload}",
            "pub fn main(arguments: List<Text>) -> Int {{\n",
            "  let message = payload()\n",
            "  let rounds = List.length(arguments)\n",
            "  loop round = 0 {{\n",
            "    case round < rounds {{\n",
            "      False -> break round\n",
            "      True -> {{\n",
            "        let channel = Channel.bounded(4)\n",
            "        Channel.send(channel.sender, message)\n",
            "        Channel.send(channel.sender, message)\n",
            "        Channel.send(channel.sender, message)\n",
            "        Channel.abort(channel.receiver)\n",
            "        round + 1\n",
            "      }}\n",
            "    }}\n",
            "  }}\n",
            "}}\n",
        ),
        payload = PAYLOAD_FUNCTION
    );
    let measure = |rounds| memory_report_over_rounds("aborted-channel", &source, &[], rounds);
    let (small, large) = (measure(2), measure(40));
    assert_flat_fragment_peak(&small, &large);
    let small_peak = json_u64_field(&small, "peak_footprint_bytes");
    let large_peak = json_u64_field(&large, "peak_footprint_bytes");
    assert!(
        large_peak <= small_peak + 16_384,
        "peak footprint grew from {small_peak} to {large_peak} bytes"
    );
}

// Guards: a child cancelled before it first ran never adopted its captures, so the
// copy of what it captured stayed allocated until exit, one per round.
#[test]
fn native_cancelling_a_child_before_it_runs_frees_its_captures() {
    let source = format!(
        concat!(
            "import std/task as Task\n",
            "import std/list as List\n",
            "{payload}",
            "pub fn main(arguments: List<Text>) -> Int {{\n",
            "  let message = payload()\n",
            "  let rounds = List.length(arguments)\n",
            "  loop round = 0 {{\n",
            "    case round < rounds {{\n",
            "      False -> break round\n",
            "      True -> {{\n",
            "        let child = Task.start(fn() -> Text {{ message }})\n",
            "        Task.cancel(child)\n",
            "        round + 1\n",
            "      }}\n",
            "    }}\n",
            "  }}\n",
            "}}\n",
        ),
        payload = PAYLOAD_FUNCTION
    );
    let measure = |rounds| memory_report_over_rounds("unrun-child", &source, &[], rounds);
    let (small, large) = (measure(2), measure(40));
    assert_flat_fragment_peak(&small, &large);
    let small_peak = json_u64_field(&small, "peak_footprint_bytes");
    let large_peak = json_u64_field(&large, "peak_footprint_bytes");
    assert!(
        large_peak <= small_peak + 16_384,
        "peak footprint grew from {small_peak} to {large_peak} bytes"
    );
}

/// Each round drops a channel of capacity 1 or 2 that still holds queued messages,
/// after cancelling two producers parked on it. Nothing aborts or closes it.
fn abandoned_channels_program() -> String {
    format!(
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{{Receive, Sender}}\n",
            "import std/task as Task\n",
            "import std/list as List\n",
            "{payload}",
            "fn spam(out: Sender<Text>, message: Text) -> Int {{\n",
            "  loop sent = 0 {{\n",
            "    Channel.send(out, message)\n",
            "    sent + 1\n",
            "  }}\n",
            "}}\n",
            "pub fn main(arguments: List<Text>) -> Int {{\n",
            "  let message = payload()\n",
            "  let rounds = List.length(arguments)\n",
            "  loop state = {{ round: 0, received: 0 }} {{\n",
            "    case state.round < rounds {{\n",
            "      False -> break state.received\n",
            "      True -> {{\n",
            "        let channel = Channel.bounded(state.round - state.round / 2 * 2 + 1)\n",
            "        let sender = channel.sender\n",
            "        let receiver = channel.receiver\n",
            "        let first = Task.start(fn() -> Int {{ spam(sender, message) }})\n",
            "        let second = Task.start(fn() -> Int {{ spam(sender, message) }})\n",
            "        let got = Channel.receive(receiver)\n",
            "        let received = case got {{\n",
            "          Receive.Received(_) -> 1\n",
            "          Receive.Drained -> 0\n",
            "        }}\n",
            "        Task.cancel(first)\n",
            "        Task.cancel(second)\n",
            "        {{ round: state.round + 1, received: state.received + received }}\n",
            "      }}\n",
            "    }}\n",
            "  }}\n",
            "}}\n",
        ),
        payload = PAYLOAD_FUNCTION
    )
}

// Guards: messages queued in a channel the program dropped stayed allocated until
// exit (a 5.5 MB fragment peak over 80 rounds of review-heap s5). Once no endpoint
// value of a channel is left in any heap or fragment, the runtime releases it, and
// the scheduler frees what it still queues.
#[test]
fn native_dropping_a_channel_frees_its_queued_messages() {
    let source = abandoned_channels_program();
    let measure = |rounds| memory_report_over_rounds("abandoned-channel", &source, &[], rounds);
    let (small, large) = (measure(2), measure(40));
    assert_flat_fragment_peak(&small, &large);
    let small_peak = json_u64_field(&small, "peak_footprint_bytes");
    let large_peak = json_u64_field(&large, "peak_footprint_bytes");
    assert!(
        large_peak <= small_peak + 16_384,
        "peak footprint grew from {small_peak} to {large_peak} bytes"
    );
}

// The trace names the release: scheduler channel event kind 15.
#[test]
fn native_trace_reports_a_released_channel() {
    let scratch = scratch_directory("native-channel-released-trace");
    let executable = build_native(
        &scratch,
        "released",
        concat!(
            "import std/channel as Channel\n",
            "pub fn main() -> Int {\n",
            "  loop round = 0 {\n",
            "    case round {\n",
            "      2 -> break round\n",
            "      _ -> {\n",
            "        let channel = Channel.bounded(1)\n",
            "        Channel.send(channel.sender, round)\n",
            "        round + 1\n",
            "      }\n",
            "    }\n",
            "  }\n",
            "}\n",
        ),
    );
    let execution = execute_native(&executable, &scratch, "released", "full");
    assert!(
        execution.process.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.process.stderr)
    );
    assert!(
        execution
            .telemetry
            .contains("\"event\":\"channel_released\""),
        "{}",
        execution.telemetry
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// Guards the release rule against releasing too early: copies of a channel's
// endpoints that children captured keep it alive after the creator's loop pass
// reclaims its own, so the children still send and receive on it. An early release
// would defect the next request that names the channel.
#[test]
fn native_children_keep_using_a_channel_whose_creator_dropped_its_endpoints() {
    let scratch = scratch_directory("native-channel-held-by-children");
    let executable = build_native(
        &scratch,
        "held",
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Completion, Receiver, Receive, Sender}\n",
            "import std/task as Task\n",
            "fn produce(out: Sender<Int>, done: Completion<Int>) -> Int {\n",
            "  loop sent = 0 {\n",
            "    case sent {\n",
            "      50 -> {\n",
            "        Channel.close(done)\n",
            "        break sent\n",
            "      }\n",
            "      _ -> {\n",
            "        Channel.send(out, sent)\n",
            "        sent + 1\n",
            "      }\n",
            "    }\n",
            "  }\n",
            "}\n",
            "fn consume(input: Receiver<Int>) -> Int {\n",
            "  loop total = 0 {\n",
            "    let next = Channel.receive(input)\n",
            "    case next {\n",
            "      Receive.Received(value) -> total + value\n",
            "      Receive.Drained -> break total\n",
            "    }\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let consumer = loop pass = 0 {\n",
            "    let channel = Channel.bounded(0)\n",
            "    let sender = channel.sender\n",
            "    let completion = channel.completion\n",
            "    let receiver = channel.receiver\n",
            "    let producer = Task.start(fn() -> Int { produce(sender, completion) })\n",
            "    break Task.start(fn() -> Int { consume(receiver) })\n",
            "  }\n",
            "  Task.result(consumer)\n",
            "}\n",
        ),
    );
    let execution = execute_native(&executable, &scratch, "held", "off");
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":1225}\n"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// Guards the release rule against releasing too early: one endpoint value is still
// a reference. The pass that made the channel reclaims its sender and completion
// but returns its receiver, which then receives the message still queued.
#[test]
fn native_a_receiver_that_outlives_its_sender_still_receives_the_queued_message() {
    let scratch = scratch_directory("native-channel-held-by-receiver");
    let executable = build_native(
        &scratch,
        "receiver-only",
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Receive}\n",
            "pub fn main() -> Int {\n",
            "  let receiver = loop pass = 0 {\n",
            "    let channel = Channel.bounded(1)\n",
            "    Channel.send(channel.sender, 42)\n",
            "    break channel.receiver\n",
            "  }\n",
            "  let got = Channel.receive(receiver)\n",
            "  case got {\n",
            "    Receive.Received(value) -> value\n",
            "    Receive.Drained -> 0\n",
            "  }\n",
            "}\n",
        ),
    );
    let execution = execute_native(&executable, &scratch, "receiver-only", "off");
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// Port requirement (Task.result adoption): a task handle can be read many times, and
// each read inside a loop pass gets its own copy of the child's result, so a later
// pass that reclaims one copy leaves the next read intact.
#[test]
fn native_task_result_can_be_read_on_every_pass_of_a_loop() {
    let scratch = scratch_directory("native-task-result-every-pass");
    let executable = build_native(
        &scratch,
        "task-result",
        concat!(
            "import std/task as Task\n",
            "struct Answer {\n",
            "  a: Int,\n",
            "  b: Text,\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let child = Task.start(fn() -> Answer { Answer(40, \"two\") })\n",
            "  loop state = { pass: 0, sum: 0 } {\n",
            "    case state.pass {\n",
            "      1000 -> break state.sum\n",
            "      _ -> {\n",
            "        let result = Task.result(child)\n",
            "        let bonus = case result.b {\n",
            "          \"two\" -> 2\n",
            "          _ -> 0\n",
            "        }\n",
            "        { pass: state.pass + 1, sum: state.sum + result.a + bonus }\n",
            "      }\n",
            "    }\n",
            "  }\n",
            "}\n",
        ),
    );
    let (outcome, _) = execute_with_memory_report(&executable, &scratch);
    assert!(outcome.contains("\"value\":42000"), "{outcome}");
    std::fs::remove_dir_all(scratch).unwrap();
}

// Lab CB04's task analogue (CB04 itself needs `remove_process`, U7): a child puts a
// value it built in its own heap on a buffered channel and finishes, so its heap is
// destroyed. Main then reuses the freed blocks with garbage of the same sizes before
// it receives. A message that pointed into the child's heap would read reused,
// zeroed blocks instead of the label and the list.
#[test]
fn native_a_value_a_finished_task_put_on_a_channel_survives_its_heap() {
    let scratch = scratch_directory("native-channel-value-outlives-task-heap");
    let executable = build_native(
        &scratch,
        "outlives-heap",
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Receive}\n",
            "import std/task as Task\n",
            "import std/list as List\n",
            "import std/text as Text\n",
            "struct Reply {\n",
            "  label: Text,\n",
            "  values: List<Int>,\n",
            "}\n",
            "fn churn(passes: Int) -> Int {\n",
            "  loop state = { left: passes, made: 0 } {\n",
            "    case state.left {\n",
            "      0 -> break state.made\n",
            "      _ -> {\n",
            "        let garbage = Reply(Text.append(\"xx\", \"xxx\"), [state.left, state.left, state.left])\n",
            "        { left: state.left - 1, made: state.made + List.length(garbage.values) }\n",
            "      }\n",
            "    }\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let channel = Channel.bounded(1)\n",
            "  let sender = channel.sender\n",
            "  let base = 40\n",
            "  let child = Task.start(fn() -> Int {\n",
            "    Channel.send(sender, Reply(Text.append(\"re\", \"ply\"), [base, base + 1, base + 2]))\n",
            "    1\n",
            "  })\n",
            "  let finished = Task.result(child)\n",
            "  let churned = churn(1000)\n",
            "  let got = Channel.receive(channel.receiver)\n",
            "  case got {\n",
            "    Receive.Received(reply) -> case reply.label {\n",
            "      \"reply\" -> case List.first(reply.values) {\n",
            "        Some(first) -> first * 100 + List.length(reply.values) * 10 + finished + churned\n",
            "        None -> 0 - 3\n",
            "      }\n",
            "      _ -> 0 - 1\n",
            "    }\n",
            "    Receive.Drained -> 0 - 2\n",
            "  }\n",
            "}\n",
        ),
    );
    let (outcome, _) = execute_with_memory_report(&executable, &scratch);
    // First element 40 * 100, three elements * 10, the child's 1, and the
    // 3 * 1,000 garbage elements the churn made.
    assert_eq!(
        outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":7031}\n"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// Port requirement (Task.start captures): a child runs in its own heap on a copy of
// what it captured, so a value built in one loop pass and captured by a task stays
// intact after that pass ends and its own allocations are reclaimed.
#[test]
fn native_a_task_keeps_what_it_captured_after_the_parents_pass_ends() {
    let scratch = scratch_directory("native-task-captures-pass-values");
    let executable = build_native(
        &scratch,
        "task-captures",
        concat!(
            "import std/task as Task\n",
            "pub fn main() -> Int {\n",
            "  let first = Task.start(fn() -> Int { 0 })\n",
            "  loop state = { pass: 0, sum: 0, task: first } {\n",
            "    let previous = Task.result(state.task)\n",
            "    case state.pass {\n",
            "      200 -> break state.sum + previous\n",
            "      _ -> {\n",
            "        let captured = { value: state.pass + 1, label: \"pass\" }\n",
            "        let next = Task.start(fn() -> Int {\n",
            "          case captured.label {\n",
            "            \"pass\" -> captured.value\n",
            "            _ -> 1000000\n",
            "          }\n",
            "        })\n",
            "        { pass: state.pass + 1, sum: state.sum + previous, task: next }\n",
            "      }\n",
            "    }\n",
            "  }\n",
            "}\n",
        ),
    );
    let (outcome, _) = execute_with_memory_report(&executable, &scratch);
    // 1 + 2 + ... + 200: each task returns the value its pass captured.
    assert!(outcome.contains("\"value\":20100"), "{outcome}");
    std::fs::remove_dir_all(scratch).unwrap();
}

/// A helper that starts a task reading from `r`: the task, not the caller, owns
/// the Receiver once `worker` returns.
const WORKER_PROGRAM: &str = concat!(
    "import std/channel as Channel\n",
    "import std/channel.{Receive, Receiver}\n",
    "import std/task as Task\n",
    "fn worker(r: Receiver<Int>) -> Int {\n",
    "  let child = Task.start(fn() -> Int {\n",
    "    let got = Channel.receive(r)\n",
    "    case got {\n",
    "      Receive.Received(v) -> v\n",
    "      Receive.Drained -> 0\n",
    "    }\n",
    "  })\n",
    "  1\n",
    "}\n",
);

/// Builds a program the checker accepts, runs it natively, and returns the
/// defect message of its outcome. Panics unless the run defects (exit 70).
fn native_capability_defect(name: &str, main: &str) -> String {
    let scratch = scratch_directory(name);
    let executable = build_native(&scratch, name, &format!("{WORKER_PROGRAM}{main}"));
    let execution = execute_native(&executable, &scratch, name, "boundaries");
    assert_eq!(
        execution.process.status.code(),
        Some(70),
        "{}",
        execution.outcome
    );
    let outcome = execution.outcome.trim_end().to_owned();
    let prefix = "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"defect\",\"message\":\"";
    let message = outcome
        .strip_prefix(prefix)
        .and_then(|rest| rest.split_once('"'))
        .map(|(message, _)| message.to_owned())
        .unwrap_or_else(|| panic!("not a defect outcome: {outcome}"));
    std::fs::remove_dir_all(scratch).unwrap();
    message
}

// task-and-channel-capabilities.md, "Checked at run time": the exclusive-move rule
// does not follow a capability into a called function, so a callee that hands its
// parameter to a task leaves the caller's argument usable to the checker. The
// owner token catches the caller's next use. Task 1 (main) passes `r` to `worker`,
// whose child, task 2, now owns it; main's own `Channel.receive(r)` must defect
// naming the Receiver, main, and task 2. A child that does the same to its own
// grandchild defects the same way and takes the program down with it. Rejects a
// runtime that lets the old owner read (main would print 5 or block), and a
// defect that does not say which capability and tasks are involved.
#[test]
fn native_backstop_a_callee_that_hands_its_parameter_to_a_task_defects_the_caller_on_reuse() {
    let message = native_capability_defect(
        "callee-hands-off",
        concat!(
            "pub fn main() -> Int {\n",
            "  let c = Channel.bounded(1)\n",
            "  let r = c.receiver\n",
            "  worker(r)\n",
            "  Channel.send(c.sender, 5)\n",
            "  let got = Channel.receive(r)\n",
            "  case got {\n",
            "    Receive.Received(v) -> v\n",
            "    Receive.Drained -> 0\n",
            "  }\n",
            "}\n",
        ),
    );
    assert_eq!(
        message,
        "channel capability violation: task 1 cannot use the Receiver of channel 1 \
         in Channel.receive: task 2 owns it"
    );
    let message = native_capability_defect(
        "child-callee-hands-off",
        concat!(
            "pub fn main() -> Int {\n",
            "  let c = Channel.bounded(1)\n",
            "  let r = c.receiver\n",
            "  let s = c.sender\n",
            "  let child = Task.start(fn() -> Int {\n",
            "    worker(r)\n",
            "    Channel.send(s, 5)\n",
            "    let got = Channel.receive(r)\n",
            "    case got {\n",
            "      Receive.Received(v) -> v\n",
            "      Receive.Drained -> 0\n",
            "    }\n",
            "  })\n",
            "  Task.result(child)\n",
            "}\n",
        ),
    );
    assert_eq!(
        message,
        "channel capability violation: task 2 cannot use the Receiver of channel 1 \
         in Channel.receive: task 3 owns it"
    );
}

// task-and-channel-capabilities.md, "Checked at run time": a generic function
// reached through a function value is not followed either, so `dup` called as
// `f(c.receiver)` or a higher-order `twice(x, f)` can give two tasks one Receiver.
// The first `Task.start` hands it to task 2; starting task 3 with the stale copy
// must defect. Rejects a runtime that lets both children read one channel.
#[test]
fn native_backstop_a_generic_function_value_that_duplicates_a_receiver_defects_the_second_start() {
    let expected = "channel capability violation: task 1 cannot hand the Receiver of \
                    channel 1 to task 3: task 2 owns it";
    let message = native_capability_defect(
        "dup-through-function-value",
        concat!(
            "fn dup<T>(x: T) -> (T, T) {\n",
            "  (x, x)\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let c = Channel.bounded(1)\n",
            "  let f = dup\n",
            "  let p = f(c.receiver)\n",
            "  case p {\n",
            "    (a, b) -> worker(a) + worker(b)\n",
            "  }\n",
            "}\n",
        ),
    );
    assert_eq!(message, expected);
    let message = native_capability_defect(
        "higher-order-twice",
        concat!(
            "fn twice<T>(x: T, f: fn(T) -> Int) -> Int {\n",
            "  f(x) + f(x)\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let c = Channel.bounded(1)\n",
            "  twice(c.receiver, worker)\n",
            "}\n",
        ),
    );
    assert_eq!(message, expected);
}

// task-and-channel-capabilities.md, "Checked at run time": reading a field of a
// binding moves nothing, so `c.receiver` read twice gives two bindings the checker
// treats as independent. Handing both to tasks must defect at the second hand-off.
// Rejects a runtime that lets two children own one Receiver.
#[test]
fn native_backstop_reading_one_receiver_field_into_two_tasks_defects_the_second_start() {
    let message = native_capability_defect(
        "field-read-twice",
        concat!(
            "pub fn main() -> Int {\n",
            "  let c = Channel.bounded(1)\n",
            "  let a = c.receiver\n",
            "  let b = c.receiver\n",
            "  worker(a) + worker(b)\n",
            "}\n",
        ),
    );
    assert_eq!(
        message,
        "channel capability violation: task 1 cannot hand the Receiver of channel 1 \
         to task 3: task 2 owns it"
    );
}

// task-and-channel-capabilities.md, "Checked at run time": a stale copy sent toward
// the task that owns the capability. `dup` through a function value gives main two
// copies of one Receiver; the child captures `a`, so it owns the Receiver, and main
// then sends the stale `b` to that same child over a side channel. A sent Receiver
// travels with its message (ts-f4e749), so the send itself must defect main, naming
// the child as owner: main is the task that broke the contract. Rejects a runtime
// that lets the stale copy through (the child would read 5 and main return it), and
// one that defects only the child that received it.
#[test]
fn native_backstop_a_stale_copy_sent_to_the_owner_defects() {
    let message = native_capability_defect(
        "stale-copy-to-owner",
        concat!(
            "fn dup<T>(x: T) -> (T, T) {\n",
            "  (x, x)\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let c = Channel.bounded(1)\n",
            "  let side = Channel.bounded(1)\n",
            "  let f = dup\n",
            "  let p = f(c.receiver)\n",
            "  case p {\n",
            "    (a, b) -> {\n",
            "      let sr = side.receiver\n",
            "      let child = Task.start(fn() -> Int {\n",
            "        let held = a\n",
            "        let got = Channel.receive(sr)\n",
            "        case got {\n",
            "          Receive.Received(r) -> {\n",
            "            let x = Channel.receive(r)\n",
            "            case x {\n",
            "              Receive.Received(v) -> v\n",
            "              Receive.Drained -> 0\n",
            "            }\n",
            "          }\n",
            "          Receive.Drained -> 0\n",
            "        }\n",
            "      })\n",
            "      Channel.send(c.sender, 5)\n",
            "      Channel.send(side.sender, b)\n",
            "      Task.result(child)\n",
            "    }\n",
            "  }\n",
            "}\n",
        ),
    );
    assert_eq!(
        message,
        "channel capability violation: task 1 cannot use the Receiver of channel 1 \
         in Channel.send: task 2 owns it"
    );
}

// task-and-channel-capabilities.md, "Checked at run time": a stale copy used by the
// task that owns the capability. Main duplicates a Receiver through a function
// value, hands one copy to a child, and gets it back as the child's result, so main
// owns the Receiver again; its other copy predates both moves. Using that copy must
// defect, and since main owns the capability, the message must say the copy is
// stale rather than name another owner. Rejects a runtime that accepts a copy
// because its user owns the channel (main would read 5).
#[test]
fn native_backstop_a_stale_copy_at_its_owner_defects() {
    let message = native_capability_defect(
        "stale-copy-at-owner",
        concat!(
            "fn dup<T>(x: T) -> (T, T) {\n",
            "  (x, x)\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let c = Channel.bounded(1)\n",
            "  let f = dup\n",
            "  let p = f(c.receiver)\n",
            "  case p {\n",
            "    (a, b) -> {\n",
            "      let child = Task.start(fn() -> Receiver<Int> {\n",
            "        a\n",
            "      })\n",
            "      let back = Task.result(child)\n",
            "      Channel.send(c.sender, 5)\n",
            "      let got = Channel.receive(b)\n",
            "      case got {\n",
            "        Receive.Received(v) -> v\n",
            "        Receive.Drained -> 0\n",
            "      }\n",
            "    }\n",
            "  }\n",
            "}\n",
        ),
    );
    assert_eq!(
        message,
        "channel capability violation: task 1 cannot use the Receiver of channel 1 \
         in Channel.receive: this copy is stale; task 1 owns the capability"
    );
}

/// A helper whose child reads one value from `r` and ends, returning its handle so
/// the caller can wait for the end.
const HAND_AND_JOIN: &str = concat!(
    "import std/channel.{ChannelError}\n",
    "import std/task.{Task}\n",
    "fn hand(r: Receiver<Int>) -> Task<Int, ChannelError> {\n",
    "  Task.start(fn() -> Int {\n",
    "    let got = Channel.receive(r)\n",
    "    case got {\n",
    "      Receive.Received(v) -> v\n",
    "      Receive.Drained -> 0\n",
    "    }\n",
    "  })\n",
    "}\n",
);

// task-and-channel-capabilities.md, "Checked at run time": the callee route when
// the task the callee handed the Receiver to has already ended. Its cleanup took
// the capability with it, so the caller's stale argument must still defect, both
// when handed to a new task and when used directly, and the message must say the
// owner ended rather than name a live owner. Rejects a runtime that frees a
// capability for reuse once its owner ends (the second child would read 6 and
// main return 56, or main would read from the dead owner's channel).
#[test]
fn native_backstop_a_stale_capability_whose_owner_has_ended_defects() {
    let message = native_capability_defect(
        "ended-owner-hand-off",
        &format!(
            "{HAND_AND_JOIN}{}",
            concat!(
                "pub fn main() -> Int {\n",
                "  let c = Channel.bounded(2)\n",
                "  let r = c.receiver\n",
                "  Channel.send(c.sender, 5)\n",
                "  Channel.send(c.sender, 6)\n",
                "  let v = Task.result(hand(r))\n",
                "  v * 10 + Task.result(hand(r))\n",
                "}\n",
            )
        ),
    );
    assert_eq!(
        message,
        "channel capability violation: task 1 cannot hand the Receiver of channel 1 \
         to task 3: the task it was handed to has ended"
    );
    let message = native_capability_defect(
        "ended-owner-use",
        &format!(
            "{HAND_AND_JOIN}{}",
            concat!(
                "pub fn main() -> Int {\n",
                "  let c = Channel.bounded(2)\n",
                "  let r = c.receiver\n",
                "  Channel.send(c.sender, 5)\n",
                "  let v = Task.result(hand(r))\n",
                "  let got = Channel.receive(r)\n",
                "  case got {\n",
                "    Receive.Received(w) -> v * 10 + w\n",
                "    Receive.Drained -> v\n",
                "  }\n",
                "}\n",
            )
        ),
    );
    assert_eq!(
        message,
        "channel capability violation: task 1 cannot use the Receiver of channel 1 \
         in Channel.receive: the task it was handed to has ended"
    );
}

// E18 across tasks and channels on native, where each task runs on a 64 KiB fiber
// stack. A child task captures a value 100,000 levels deep, renders it with
// `Debug.render`, encodes a Json.Value as deep with `Json.encode`, and sends a
// third deep value back over a Channel, which the parent compares with its own.
// A second child captures a Receiver held 100,000 levels down and reads from it,
// so `Task.start`'s capability transfer must reach the bottom of the capture.
// Before, the transfer recursed per level and the capture defected with "Task
// stack overflow"; a walk that stopped early would leave the Receiver with the
// parent and the read would fail.
#[test]
fn native_tasks_capture_render_encode_and_send_deeply_nested_values() {
    let scratch = scratch_directory("native-deep-task-values");
    let executable = build_native(
        &scratch,
        "deep-tasks",
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Receive, Receiver}\n",
            "import std/debug as Debug\n",
            "import std/json as Json\n",
            "import std/task as Task\n",
            "enum Nest {\n",
            "  Leaf\n",
            "  Wrap(children: List<Nest>)\n",
            "}\n",
            "enum Holder {\n",
            "  Bottom(receiver: Receiver<Int>)\n",
            "  Around(inner: List<Holder>)\n",
            "}\n",
            "struct Build { depth: Int, nest: Nest }\n",
            "struct Hold { depth: Int, holder: Holder }\n",
            "struct JsonBuild { depth: Int, value: Json.Value }\n",
            "fn nested(depth: Int) -> Nest {\n",
            "  loop state = Build(depth, Leaf) {\n",
            "    case state.depth {\n",
            "      0 -> break state.nest\n",
            "      _ -> Build(state.depth - 1, Wrap([state.nest]))\n",
            "    }\n",
            "  }\n",
            "}\n",
            "fn json_nested(depth: Int) -> Json.Value {\n",
            "  loop state = JsonBuild(depth, Json.Value.Null) {\n",
            "    case state.depth {\n",
            "      0 -> break state.value\n",
            "      _ -> JsonBuild(state.depth - 1, Json.Value.Array([state.value]))\n",
            "    }\n",
            "  }\n",
            "}\n",
            "fn innermost(holder: Holder) -> Int {\n",
            "  loop current = holder {\n",
            "    case current {\n",
            "      Bottom(receiver) -> {\n",
            "        let next = Channel.receive(receiver)\n",
            "        break case next {\n",
            "          Receive.Received(value) -> value\n",
            "          Receive.Drained -> 0\n",
            "        }\n",
            "      }\n",
            "      Around([inner]) -> inner\n",
            "      Around(_) -> break -1\n",
            "    }\n",
            "  }\n",
            "}\n",
            "pub fn main() -> (Bool, (Text, Text), Int) {\n",
            "  let deep = nested(100000)\n",
            "  let replies = Channel.bounded(1)\n",
            "  let reply_sender = replies.sender\n",
            "  let reply_receiver = replies.receiver\n",
            "  let reply_completion = replies.completion\n",
            "  let child = Task.start(fn() -> (Text, Text) {\n",
            "    Channel.send(reply_sender, nested(100000))\n",
            "    (Debug.render(deep), Json.encode(json_nested(100000)))\n",
            "  })\n",
            "  let reply = Channel.receive(reply_receiver)\n",
            "  let received = case reply {\n",
            "    Receive.Received(value) -> value == deep\n",
            "    Receive.Drained -> False\n",
            "  }\n",
            "  Channel.close(reply_completion)\n",
            "  let numbers = Channel.bounded(1)\n",
            "  let number_sender = numbers.sender\n",
            "  let held = loop state = Hold(100000, Bottom(numbers.receiver)) {\n",
            "    case state.depth {\n",
            "      0 -> break state.holder\n",
            "      _ -> Hold(state.depth - 1, Around([state.holder]))\n",
            "    }\n",
            "  }\n",
            "  let reader = Task.start(fn() -> Int { innermost(held) })\n",
            "  Channel.send(number_sender, 42)\n",
            "  Channel.close(numbers.completion)\n",
            "  (received, Task.result(child), Task.result(reader))\n",
            "}\n",
        ),
    );
    let (outcome, _) = execute_with_memory_report(&executable, &scratch);
    let levels = 100_000;
    let rendered = format!(
        "{}Nest.Leaf{}",
        "Nest.Wrap([".repeat(levels),
        "])".repeat(levels)
    );
    let encoded = format!("{}null{}", "[".repeat(levels), "]".repeat(levels));
    assert_eq!(
        outcome.trim_end(),
        format!(
            "{{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":{{\"tuple\":[true,{{\"tuple\":[\"{rendered}\",\"{encoded}\"]}},42]}}}}"
        )
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// `Task.start`'s capability transfer visits each captured part once through a
// hashed visited set, so its work is linear in the capture: sixteen times the
// tree costs about sixteen times the probes. The earlier list scan compared each
// part with every part before it, about 256 times as many probes here.
#[test]
fn native_capture_transfer_work_is_linear_in_the_captured_parts() {
    let measure = |depth: u64| {
        let scratch = scratch_directory(&format!("native-capture-transfer-{depth}"));
        let source = format!(
            concat!(
                "import std/task as Task\n",
                "enum Tree {{\n  Leaf(value: Int)\n  Node(left: Tree, right: Tree)\n}}\n",
                "fn build(depth: Int) -> Tree {{\n",
                "  case depth {{\n",
                "    0 -> Leaf(1)\n",
                "    _ -> Node(build(depth - 1), build(depth - 1))\n",
                "  }}\n",
                "}}\n",
                "fn size(tree: Tree) -> Int {{\n",
                "  case tree {{\n",
                "    Leaf(value) -> value\n",
                "    Node(left, right) -> size(left) + size(right)\n",
                "  }}\n",
                "}}\n",
                "pub fn main() -> Int {{\n",
                "  let tree = build({depth})\n",
                "  let child = Task.start(fn() -> Int {{ size(tree) }})\n",
                "  Task.result(child)\n",
                "}}\n",
            ),
            depth = depth
        );
        let executable = build_native(&scratch, "capture", &source);
        let (outcome, report) = execute_with_memory_report(&executable, &scratch);
        assert!(
            outcome.contains(&format!("\"value\":{}", 1u64 << depth)),
            "{outcome}"
        );
        std::fs::remove_dir_all(scratch).unwrap();
        json_u64_field(&report, "capture_transfer_probes")
    };
    let small = measure(8);
    let large = measure(12);
    assert!(
        small > 0 && large <= 64 * small,
        "2^12 leaves took {large} probes, 2^8 leaves {small}"
    );
}

// A message crosses to the receiver as a copy, so the sender's later passes may
// reclaim the original while the message still waits in the channel. Here every
// message is sent from its own pass and read only after the sender has run all
// of its passes.
#[test]
fn native_a_queued_message_survives_the_senders_later_passes() {
    let scratch = scratch_directory("native-queued-message-survives");
    let executable = build_native(
        &scratch,
        "queued-message",
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Completion, Receiver, Receive, Sender}\n",
            "import std/task as Task\n",
            "struct Message {\n",
            "  value: Int,\n",
            "  label: Text,\n",
            "}\n",
            "fn drain(messages: Receiver<Message>) -> Int {\n",
            "  loop sum = 0 {\n",
            "    let next = Channel.receive(messages)\n",
            "    case next {\n",
            "      Receive.Received(message) -> case message.label {\n",
            "        \"job\" -> sum + message.value\n",
            "        _ -> sum + 1000000\n",
            "      }\n",
            "      Receive.Drained -> break sum\n",
            "    }\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let channel = Channel.bounded(100)\n",
            "  let sender = channel.sender\n",
            "  let receiver = channel.receiver\n",
            "  let completion = channel.completion\n",
            "  let sent = loop count = 0 {\n",
            "    case count {\n",
            "      100 -> break count\n",
            "      _ -> {\n",
            "        Channel.send(sender, Message(count + 1, \"job\"))\n",
            "        count + 1\n",
            "      }\n",
            "    }\n",
            "  }\n",
            "  Channel.close(completion)\n",
            "  let reader = Task.start(fn() -> Int { drain(receiver) })\n",
            "  Task.result(reader) + sent - sent\n",
            "}\n",
        ),
    );
    let (outcome, _) = execute_with_memory_report(&executable, &scratch);
    // 1 + 2 + ... + 100, and no message read back as anything but a job.
    assert!(outcome.contains("\"value\":5050"), "{outcome}");
    std::fs::remove_dir_all(scratch).unwrap();
}

// LP-08: draining a list that was built outside every loop frees each drain's window
// copies, so draining it 8 times peaks at the same memory as draining it twice and
// leaves nothing more behind. Copies that outlived the loop that made them used to
// accumulate until exit, growing the process by about 190 KB per drain here.
#[test]
fn native_repeated_drains_of_a_list_built_outside_loops_do_not_accumulate_copies() {
    let scratch = scratch_directory("native-list-repeated-drains");
    let measure = |drains: u32| {
        let executable = build_native(
            &scratch,
            &format!("list-drains-{drains}"),
            &format!(
                concat!(
                    "import std/json as Json\n",
                    "import std/json.{{Value}}\n",
                    "import std/text as Text\n",
                    "fn ones(doublings: Int) -> Text {{\n",
                    "  loop state = {{ text: \"1\", left: doublings }} {{\n",
                    "    case state.left {{\n",
                    "      0 -> break state.text\n",
                    "      _ -> {{ text: Text.append(Text.append(state.text, \",\"), state.text), left: state.left - 1 }}\n",
                    "    }}\n",
                    "  }}\n",
                    "}}\n",
                    "fn drain(values: List<Value>) -> Int {{\n",
                    "  loop state = {{ rest: values, total: 0 }} {{\n",
                    "    case state.rest {{\n",
                    "      [] -> break state.total\n",
                    "      [_, ..rest] -> {{ rest: rest, total: state.total + 1 }}\n",
                    "    }}\n",
                    "  }}\n",
                    "}}\n",
                    "fn run(values: List<Value>) -> Int {{\n",
                    "  loop state = {{ left: {drains}, total: 0 }} {{\n",
                    "    case state.left {{\n",
                    "      0 -> break state.total\n",
                    "      _ -> {{ left: state.left - 1, total: state.total + drain(values) }}\n",
                    "    }}\n",
                    "  }}\n",
                    "}}\n",
                    "pub fn main() -> Int {{\n",
                    "  let decoded = Json.decode(Text.append(Text.append(\"[\", ones(16)), \"]\"))\n",
                    "  case decoded {{\n",
                    "    Value.Array(values) -> run(values)\n",
                    "    _ -> -1\n",
                    "  }}\n",
                    "}}\n",
                ),
                drains = drains
            ),
        );
        let (outcome, report) = execute_with_memory_report(&executable, &scratch);
        assert_eq!(
            outcome,
            format!(
                "{{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":{}}}\n",
                u64::from(drains) << 16
            )
        );
        std::fs::remove_file(scratch.join("outcome.jsonl")).unwrap();
        std::fs::remove_file(scratch.join("memory.json")).unwrap();
        report
    };
    let twice = measure(2);
    let eight_times = measure(8);
    for field in [
        "unscoped_allocated_bytes",
        "scoped_peak_live_bytes",
        "scoped_retained_bytes",
    ] {
        assert_eq!(
            json_u64_field(&twice, field),
            json_u64_field(&eight_times, field),
            "{field}: {twice} {eight_times}"
        );
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

// LP-10: `w` holds exactly a quarter of its vector, so each `rest` view of it is
// shorter than a quarter and must not retain the vector. `w` adopts a compact vector
// on the first match and every later view shares it, so 18,000 more matches add a
// constant amount per match. A copy per match would add 16,383 elements (about
// 130 KB of list nodes) each time.
#[test]
fn native_matching_one_boundary_window_repeatedly_copies_it_once() {
    let scratch = scratch_directory("native-list-boundary-window");
    let measure = |matches: u32| {
        let executable = build_native(
            &scratch,
            &format!("list-boundary-{matches}"),
            &format!(
                concat!(
                    "import std/json as Json\n",
                    "import std/json.{{Value}}\n",
                    "import std/list as List\n",
                    "import std/text as Text\n",
                    "fn ones(doublings: Int) -> Text {{\n",
                    "  loop state = {{ text: \"1\", left: doublings }} {{\n",
                    "    case state.left {{\n",
                    "      0 -> break state.text\n",
                    "      _ -> {{ text: Text.append(Text.append(state.text, \",\"), state.text), left: state.left - 1 }}\n",
                    "    }}\n",
                    "  }}\n",
                    "}}\n",
                    "fn skip(values: List<Value>, count: Int) -> List<Value> {{\n",
                    "  loop state = {{ rest: values, left: count }} {{\n",
                    "    case state.left {{\n",
                    "      0 -> break state.rest\n",
                    "      _ -> case state.rest {{\n",
                    "        [] -> break state.rest\n",
                    "        [_, ..rest] -> {{ rest: rest, left: state.left - 1 }}\n",
                    "      }}\n",
                    "    }}\n",
                    "  }}\n",
                    "}}\n",
                    "fn peek(w: List<Value>) -> Int {{\n",
                    "  loop state = {{ left: {matches}, total: 0 }} {{\n",
                    "    case state.left {{\n",
                    "      0 -> break state.total\n",
                    "      _ -> case w {{\n",
                    "        [_, ..rest] -> {{ left: state.left - 1, total: state.total + List.length(rest) }}\n",
                    "        [] -> break 0\n",
                    "      }}\n",
                    "    }}\n",
                    "  }}\n",
                    "}}\n",
                    "pub fn main() -> Int {{\n",
                    "  let decoded = Json.decode(Text.append(Text.append(\"[\", ones(16)), \"]\"))\n",
                    "  case decoded {{\n",
                    "    Value.Array(values) -> peek(skip(values, 49152))\n",
                    "    _ -> -1\n",
                    "  }}\n",
                    "}}\n",
                ),
                matches = matches
            ),
        );
        let (outcome, report) = execute_with_memory_report(&executable, &scratch);
        assert_eq!(
            outcome,
            format!(
                "{{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":{}}}\n",
                u64::from(matches) * 16_383
            )
        );
        std::fs::remove_file(scratch.join("outcome.jsonl")).unwrap();
        std::fs::remove_file(scratch.join("memory.json")).unwrap();
        json_u64_field(&report, "scoped_total_allocated_bytes")
            + json_u64_field(&report, "unscoped_allocated_bytes")
    };
    let few = measure(2_000);
    let many = measure(20_000);
    let per_match = (many - few) / 18_000;
    assert!(per_match < 4096, "{per_match} bytes per match");
    std::fs::remove_dir_all(scratch).unwrap();
}

// LP-07: `[.., last]` checks the length and reads one element, so matching a
// 1,048,576-element list 10,000 times allocates no list storage. A copy per match
// would allocate about 8 MB of list nodes each time.
#[test]
fn native_last_element_pattern_does_not_copy_a_large_list() {
    let scratch = scratch_directory("native-list-pattern-last");
    let executable = build_native(
        &scratch,
        "list-pattern-last",
        &million_element_list_program(concat!(
            "fn run(values: List<Value>) -> Int {\n",
            "  loop state = { left: 10000, found: 0 } {\n",
            "    case state.left {\n",
            "      0 -> break state.found\n",
            "      _ -> case values {\n",
            "        [.., Value.Number(_)] -> { left: state.left - 1, found: state.found + 1 }\n",
            "        _ -> { left: state.left - 1, found: state.found }\n",
            "      }\n",
            "    }\n",
            "  }\n",
            "}\n",
        )),
    );
    let (outcome, memory_report) = execute_with_memory_report(&executable, &scratch);
    assert_eq!(
        outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":10000}\n"
    );
    assert_eq!(
        json_u64_field(&memory_report, "list_nodes"),
        0,
        "{memory_report}"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// LP-08: a `loop` that sums a list by rebinding `[x, ..rest]` runs its passes in
// constant stack and allocates O(1) per `rest` view. The spec row says 10,000,000
// elements; this uses 2^20 because each decoded JSON element costs about 100 bytes
// of setup, and the per-element bound below already separates a constant cost per
// view (hundreds of bytes) from a copy per view (megabytes per pass at this size).
#[test]
fn native_draining_loop_allocates_a_constant_amount_per_rest_view() {
    let scratch = scratch_directory("native-list-pattern-drain");
    let executable = build_native(
        &scratch,
        "list-pattern-drain",
        &million_element_list_program(concat!(
            "fn run(values: List<Value>) -> Int {\n",
            "  loop state = { rest: values, total: 0 } {\n",
            "    case state.rest {\n",
            "      [] -> break state.total\n",
            "      [Value.Number(_), ..rest] -> { rest: rest, total: state.total + 1 }\n",
            "      [_, ..rest] -> { rest: rest, total: state.total }\n",
            "    }\n",
            "  }\n",
            "}\n",
        )),
    );
    let started = std::time::Instant::now();
    let (outcome, memory_report) = execute_with_memory_report(&executable, &scratch);
    let elapsed = started.elapsed();
    let elements = 1_u64 << 20;
    assert_eq!(
        outcome,
        format!(
            "{{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":{elements}}}\n"
        )
    );
    // Unscoped bytes count too: a copy per view must fail this bound wherever the
    // runtime places the copy, including outside every reclaiming scope.
    let allocated = json_u64_field(&memory_report, "scoped_total_allocated_bytes")
        + json_u64_field(&memory_report, "unscoped_allocated_bytes");
    let per_element = allocated / elements;
    assert!(
        per_element < 2048,
        "{per_element} bytes per pass: {memory_report}"
    );
    // Re-marking the whole list on each pass's collection took minutes at this size.
    assert!(elapsed.as_secs() < 60, "draining took {elapsed:?}");
    std::fs::remove_dir_all(scratch).unwrap();
}

// G10: a capability renders as its type's simple name and never shows its contents.
// Channels exist only on the native runtime, so this row is native-only.
#[test]
fn native_debug_render_names_capabilities_without_their_contents() {
    let scratch = scratch_directory("debug-render-capabilities");
    let executable = build_native(
        &scratch,
        "debug-capabilities",
        concat!(
            "import std/channel as Channel\n",
            "import std/debug as Debug\n",
            "pub fn main() -> Text {\n",
            "  let channel = Channel.bounded(1)\n",
            "  Channel.send(channel.sender, 1)\n",
            "  Channel.close(channel.completion)\n",
            "  Debug.render(channel)\n",
            "}\n",
        ),
    );
    let execution = execute_native(&executable, &scratch, "debug-capabilities", "off");
    assert!(
        execution.process.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.process.stderr)
    );
    assert_eq!(
        execution.outcome,
        concat!(
            "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":",
            "\"{ completion: <Completion>, receiver: <Receiver>, sender: <Sender> }\"}\n"
        )
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Counts events into a `Tally` with `step`, which the caller defines either as
/// function clauses or as one `case`.
fn tally_program(step: &str) -> String {
    format!(
        concat!(
            "struct Tally {{\n",
            "  a: Int,\n",
            "  b: Int,\n",
            "  other: Int,\n",
            "}}\n",
            "{step}\n",
            "pub fn main() -> Int {{\n",
            "  loop state = (0, Tally(0, 0, 0)) {{\n",
            "    let (i, tally) = state\n",
            "    case i {{\n",
            "      2000 -> break tally.a + 10 * tally.b + 100 * tally.other\n",
            "      _ -> (i + 1, step(step(step(tally, \"a\"), \"b\"), \"c\"))\n",
            "    }}\n",
            "  }}\n",
            "}}\n",
        ),
        step = step
    )
}

// ts-6c5e9e: a clause group lowers to the Match of the `case` written over the
// parameters it inspects. Before, each clause rebound every plain parameter, so
// each call cloned its environment, matched one more subject, and every
// environment carried a slot per clause binding.
#[test]
fn native_function_clauses_allocate_what_the_equivalent_case_allocates() {
    let measure = |name: &str, step: &str| {
        let scratch = scratch_directory(&format!("native-clause-allocation-{name}"));
        let executable = build_native(&scratch, "tally", &tally_program(step));
        let (outcome, report) = execute_with_memory_report(&executable, &scratch);
        assert!(outcome.contains("\"value\":222000"), "{name}: {outcome}");
        std::fs::remove_dir_all(scratch).unwrap();
        json_u64_field(&report, "scoped_total_allocated_bytes")
    };
    let clauses = measure(
        "clauses",
        concat!(
            "fn step(tally: Tally, \"a\") -> Tally { { ..tally, a: tally.a + 1 } }\n",
            "fn step(tally, \"b\") { { ..tally, b: tally.b + 1 } }\n",
            "fn step(tally, _) { { ..tally, other: tally.other + 1 } }\n",
        ),
    );
    let case = measure(
        "case",
        concat!(
            "fn step(tally: Tally, event: Text) -> Tally {\n",
            "  case event {\n",
            "    \"a\" -> { ..tally, a: tally.a + 1 }\n",
            "    \"b\" -> { ..tally, b: tally.b + 1 }\n",
            "    _ -> { ..tally, other: tally.other + 1 }\n",
            "  }\n",
            "}\n",
        ),
    );
    assert_eq!(clauses, case, "clauses {clauses} bytes, case {case} bytes");
}

// RU-11 (ts-8250de): an update of a local base allocates what the constructor
// written out allocates. Before, it bound the base and each value in a sequence,
// which cloned the environment, and every environment carried a slot per
// update temporary.
#[test]
fn native_record_update_allocates_what_the_written_out_constructor_allocates() {
    let measure = |name: &str, bump: &str| {
        let scratch = scratch_directory(&format!("native-update-allocation-{name}"));
        let step = format!(
            concat!(
                "fn bump(tally: Tally, event: Text) -> Tally {{\n",
                "  case event {{\n",
                "    \"b\" -> {}\n",
                "    _ -> tally\n",
                "  }}\n",
                "}}\n",
                "fn step(tally: Tally, event: Text) -> Tally {{ bump(tally, event) }}\n",
            ),
            bump
        );
        let executable = build_native(&scratch, "tally", &tally_program(&step));
        let (outcome, report) = execute_with_memory_report(&executable, &scratch);
        assert!(outcome.contains("\"value\":20000"), "{name}: {outcome}");
        std::fs::remove_dir_all(scratch).unwrap();
        json_u64_field(&report, "scoped_total_allocated_bytes")
    };
    let update = measure("update", "{ ..tally, b: tally.b + 1 }");
    let constructor = measure("constructor", "Tally(tally.a, tally.b + 1, tally.other)");
    assert_eq!(
        update, constructor,
        "update {update} bytes, constructor {constructor} bytes"
    );
}

// The native build runs process programs (ts-6ccbc9 U5). Covers both reasons a
// program links the process kernel: a call into std/process, and a program whose
// only process use is a `process name` reference. A send to a name no process has
// registered returns `Unavailable` rather than being accepted. Rejects a build that
// still refuses process programs, and a named address that routes without a
// registration.
#[test]
fn native_process_programs_build_and_an_unregistered_name_is_unavailable() {
    let prelude = concat!(
        "import std/process as Process\n",
        "import std/process.{SendError}\n",
        "enum Message {\n  Ping\n}\n",
        "process name pinger: Message\n",
    );
    for (name, main, value) in [
        (
            "process-send",
            "pub fn main() -> Int {\n  case Process.send(pinger, Message.Ping) {\n    \
             Success(_) -> 0\n    Failure(error) -> case error {\n      \
             SendError.Full(_) -> 1\n      SendError.Unavailable(_) -> 2\n    }\n  }\n}\n",
            "2",
        ),
        (
            "process-name-only",
            "fn keep(address: Process.Address<Message>) -> Int {\n  0\n}\n\
             pub fn main() -> Int {\n  keep(pinger)\n}\n",
            "0",
        ),
    ] {
        let scratch = scratch_directory(name);
        let source = scratch.join("main.chimi");
        let executable = scratch.join("program");
        std::fs::write(&source, format!("{prelude}{main}")).unwrap();
        let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&scratch)
            .arg("build")
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            build.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let outcome = scratch.join("outcome.jsonl");
        let run = Command::new(&executable)
            .arg("--chimi-runner")
            .arg(format!("--outcome-file={}", outcome.display()))
            .arg("--")
            .output()
            .unwrap();
        assert!(run.status.success(), "{name}: {run:?}");
        assert_eq!(
            std::fs::read_to_string(&outcome).unwrap(),
            format!(
                "{{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":{value}}}\n"
            ),
            "{name}"
        );
        std::fs::remove_dir_all(scratch).unwrap();
    }
}

// Scheduler fibers get a 1 MiB guarded stack, and an incarnation's defect is named
// in its `process_terminated` telemetry. Each run sends one request to `worker` and
// then asks for its result: a worker that ended answers `Unavailable` (-1), so main
// never waits on a dead process.
// - Recursion 1,000 calls deep succeeds in the body and in main, which runs as a
//   scheduler task. Rejects the old 64 KiB stack, which overflowed near 150 calls.
// - Recursion 100,000 calls deep overflows the body's stack. The incarnation ends
//   `defected` with defect "Process stack overflow", and main still finishes.
// - A body that divides by zero ends `defected` with its own defect message.
#[test]
fn native_process_stack_depth_and_named_defects() {
    let scratch = scratch_directory("native-process-stack");
    let source = scratch.join("main.chimi");
    let executable = scratch.join("program");
    std::fs::write(
        &source,
        concat!(
            "import std/int as Int\n",
            "import std/channel as Channel\n",
            "import std/channel.{Receive, Sender}\n",
            "import std/clock as Clock\n",
            "import std/clock.{Clock}\n",
            "import std/duration as Duration\n",
            "import std/process as Process\n",
            "import std/supervisor as Supervisor\n",
            "import std/process.{Inbox}\n",
            "import std/list as List\n",
            "pub enum Message {\n  Deep(depth: Int)\n  Report(reply: Sender<Int>)\n}\n",
            "pub process name worker: Message\n",
            "fn deep(n: Int) -> Int {\n  case n {\n    0 -> 0\n    _ -> 1 + deep(n - 1)\n  }\n}\n",
            "fn work(inbox: Inbox<Message>) -> Void {\n",
            "  loop last = 0 {\n",
            "    case Process.receive(inbox) {\n",
            "      Message.Deep(depth) -> case depth {\n        0 -> 1 / depth\n        _ -> deep(depth)\n      }\n",
            "      Message.Report(reply) -> {\n        Channel.send(reply, last)\n        last\n      }\n",
            "    }\n  }\n}\n",
            "pub fn main(arguments: List<Text>) -> Int needs { clock: Clock } {\n",
            "  let depth = case List.first(arguments) {\n    Some(text) -> Int.parse(text)\n    None -> 0\n  }\n",
            "  let member = Process.spec(work, 2, Duration.seconds(1))\n",
            "    |> Process.register(worker)\n",
            "    |> Supervisor.process(Supervisor.Restart.Never)\n",
            "  let root = Supervisor.start(Supervisor.policy([member], 1, Supervisor.restarts(0, Duration.seconds(30))))\n",
            "  Process.send(worker, Message.Deep(depth))\n",
            "  Clock.sleep(Duration.milliseconds(1))\n",
            "  let replies = Channel.bounded(1)\n",
            "  let answered = case Process.send(worker, Message.Report(replies.sender)) {\n",
            "    Success(_) -> {\n      let received = Channel.receive(replies.receiver)\n",
            "      case received {\n        Receive.Received(value) -> value\n        Receive.Drained -> -2\n      }\n    }\n",
            "    Failure(_) -> -1\n  }\n",
            "  Supervisor.stop(root)\n",
            "  answered + deep(1000) - 1000\n",
            "}\n",
        ),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    for (depth, value, defect) in [
        ("1000", "1000", None),
        (
            "100000",
            "-1",
            Some("\"defect\":\"Process stack overflow\""),
        ),
        ("0", "-1", Some("\"defect\":\"integer division by zero")),
    ] {
        let telemetry = scratch.join(format!("telemetry-{depth}.jsonl"));
        let outcome = scratch.join(format!("outcome-{depth}.jsonl"));
        let run = Command::new(&executable)
            .arg("--chimi-runner")
            .arg("--clock=virtual")
            .arg("--trace=boundaries")
            .arg(format!("--telemetry-file={}", telemetry.display()))
            .arg(format!("--outcome-file={}", outcome.display()))
            .arg("--")
            .arg(depth)
            .output()
            .unwrap();
        assert!(run.status.success(), "depth {depth}: {run:?}");
        assert_eq!(
            std::fs::read_to_string(&outcome).unwrap(),
            format!(
                "{{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":{value}}}\n"
            ),
            "depth {depth}"
        );
        let telemetry = std::fs::read_to_string(&telemetry).unwrap();
        let terminated = telemetry
            .lines()
            .find(|line| line.contains("\"event\":\"process_terminated\""))
            .unwrap_or_else(|| panic!("depth {depth}: no process_terminated"));
        match defect {
            Some(defect) => {
                assert!(
                    terminated.contains("\"cause\":\"defected\""),
                    "{terminated}"
                );
                assert!(terminated.contains(defect), "{terminated}");
            }
            None => {
                assert!(
                    terminated.contains("\"cause\":\"cancelled\""),
                    "{terminated}"
                );
                assert!(terminated.contains("\"defect\":null"), "{terminated}");
            }
        }
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

// ts-ce73fb: Console writes every byte of a Text holding U+0000, and a path holding
// it names no file on either backend, with the interpreter's message.
#[test]
fn native_host_io_keeps_text_bytes_after_nul() {
    let scratch = scratch_directory("native-text-nul");
    let source = concat!(
        "import std/io as IO\n",
        "import std/io.{Console}\n",
        "import std/io.{FileSystem}\n",
        "pub fn main() -> Int\n",
        "  needs {\n",
        "    files: FileSystem,\n",
        "    console: Console,\n",
        "  }\n",
        "{\n",
        "  console.write_line(\"a\\u{0}b\")\n",
        "  files.process_utf8_lines(IO.path(\"missing\\u{0}name\"), 0, fn(count: Int, line: Text) { count + 1 })\n",
        "}\n",
    );
    let executable = build_native(&scratch, "text-nul", source);
    let native = execute_native(&executable, &scratch, "text-nul", "off");
    assert_eq!(native.process.stdout, b"a\0b\n");
    let expected = concat!(
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"expected_failure\",\"error\":",
        "{\"type\":\"FileError\",\"variant\":\"Open\",\"fields\":{\"path\":\"missing\\u0000name\",",
        "\"message\":\"file name contained an unexpected NUL byte\"}}}"
    );
    assert_eq!(native.outcome.trim_end(), expected);
    let interpreted = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .arg("run")
        .arg(scratch.join("text-nul.chimi"))
        .output()
        .expect("run the reference interpreter");
    let stdout = String::from_utf8_lossy(&interpreted.stdout);
    assert_eq!(stdout.lines().last(), Some(expected));
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Builds a program, runs it natively under `--trace=boundaries`, and returns its
/// outcome line.
fn native_hand_back_outcome(name: &str, source: &str) -> String {
    let scratch = scratch_directory(name);
    let executable = build_native(&scratch, name, source);
    let execution = execute_native(&executable, &scratch, name, "boundaries");
    let outcome = execution.outcome.trim_end().to_owned();
    std::fs::remove_dir_all(scratch).unwrap();
    outcome
}

const HAND_BACK_PRELUDE: &str = concat!(
    "import std/channel as Channel\n",
    "import std/channel.{Receive, Receiver, SendError}\n",
    "import std/task as Task\n",
    "fn take(r: Receiver<Int>) -> Int {\n",
    "  let got = Channel.receive(r)\n",
    "  case got {\n",
    "    Receive.Received(v) -> v\n",
    "    Receive.Drained -> 0\n",
    "  }\n",
    "}\n",
);

// task-and-channel-capabilities.md, "Owner exit and automatic cleanup": handing a
// Receiver on transfers its cleanup responsibility (ts-f4e749). A child that sends
// the Receiver it captured back to its parent over a channel, or returns it as its
// `Task.result`, and then ends must leave it open and usable by the parent.
// Before, ownership moved only at `Task.start`, so the ended child's cleanup
// aborted the channel and the parent's send failed with `Aborted` (-1 here),
// or its receive defected while the child still lived.
#[test]
fn native_a_receiver_handed_back_to_the_parent_stays_open_after_the_child_ends() {
    let over_a_channel = native_hand_back_outcome(
        "hand-back-channel",
        &format!(
            "{HAND_BACK_PRELUDE}{}",
            concat!(
                "pub fn main() -> Int {\n",
                "  let data = Channel.bounded(1)\n",
                "  let back = Channel.bounded(1)\n",
                "  let r = data.receiver\n",
                "  let back_sender = back.sender\n",
                "  let back_receiver = back.receiver\n",
                "  let child = Task.start(fn() -> Int {\n",
                "    Channel.send(back_sender, r)\n",
                "    1\n",
                "  })\n",
                "  let returned = Channel.receive(back_receiver)\n",
                "  let ended = Task.result(child)\n",
                "  case returned {\n",
                "    Receive.Received(mine) -> case Channel.send(data.sender, 5) {\n",
                "      Success(_) -> take(mine) + ended - 1\n",
                "      Failure(_) -> -1\n",
                "    }\n",
                "    Receive.Drained -> -2\n",
                "  }\n",
                "}\n",
            )
        ),
    );
    assert_eq!(
        over_a_channel,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":5}"
    );
    let as_its_result = native_hand_back_outcome(
        "hand-back-result",
        &format!(
            "{HAND_BACK_PRELUDE}{}",
            concat!(
                "pub fn main() -> Int {\n",
                "  let data = Channel.bounded(1)\n",
                "  let r = data.receiver\n",
                "  let child = Task.start(fn() -> Receiver<Int> {\n",
                "    r\n",
                "  })\n",
                "  let mine = Task.result(child)\n",
                "  case Channel.send(data.sender, 5) {\n",
                "    Success(_) -> take(mine)\n",
                "    Failure(_) -> -1\n",
                "  }\n",
                "}\n",
            )
        ),
    );
    assert_eq!(
        as_its_result,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":5}"
    );
}

// ts-f4e749: what happens to a Receiver sent in a message that no task receives.
// - A refused send hands it back to the sender, whose own copy works again: main
//   reads 5 through the Receiver it got back from `SendError.Closed`.
// - A message discarded unread (its channel aborted while it was queued) takes
//   the Receiver out of the program, so its channel aborts as if its owner had
//   ended, and a later send on it fails (7). A runtime that left it open with no
//   owner would accept the send and return 0, and any task waiting on that
//   channel would wait forever.
// - The sender's own copy is stale while the message is in flight: using it
//   defects the sender and says why.
#[test]
fn native_a_receiver_in_an_unreceived_message_is_returned_or_ends() {
    let refused = native_hand_back_outcome(
        "hand-back-refused",
        &format!(
            "{HAND_BACK_PRELUDE}{}",
            concat!(
                "pub fn main() -> Int {\n",
                "  let data = Channel.bounded(1)\n",
                "  let parcels = Channel.bounded(1)\n",
                "  Channel.close(parcels.completion)\n",
                "  case Channel.send(parcels.sender, data.receiver) {\n",
                "    Success(_) -> -1\n",
                "    Failure(SendError.Closed(back)) -> {\n",
                "      Channel.send(data.sender, 5)\n",
                "      take(back)\n",
                "    }\n",
                "    Failure(SendError.Aborted(back)) -> -2\n",
                "  }\n",
                "}\n",
            )
        ),
    );
    assert_eq!(
        refused,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":5}"
    );
    let discarded = native_hand_back_outcome(
        "hand-back-discarded",
        &format!(
            "{HAND_BACK_PRELUDE}{}",
            concat!(
                "pub fn main() -> Int {\n",
                "  let data = Channel.bounded(1)\n",
                "  let parcels = Channel.bounded(1)\n",
                "  Channel.send(parcels.sender, data.receiver)\n",
                "  Channel.abort(parcels.receiver)\n",
                "  case Channel.send(data.sender, 5) {\n",
                "    Success(_) -> 0\n",
                "    Failure(SendError.Aborted(_)) -> 7\n",
                "    Failure(SendError.Closed(_)) -> -1\n",
                "  }\n",
                "}\n",
            )
        ),
    );
    assert_eq!(
        discarded,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":7}"
    );
    let message = native_capability_defect(
        "hand-back-in-flight",
        concat!(
            "fn dup<T>(x: T) -> (T, T) {\n",
            "  (x, x)\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let data = Channel.bounded(1)\n",
            "  let parcels = Channel.bounded(1)\n",
            "  let f = dup\n",
            "  let p = f(data.receiver)\n",
            "  case p {\n",
            "    (a, b) -> {\n",
            "      Channel.send(parcels.sender, a)\n",
            "      let got = Channel.receive(b)\n",
            "      0\n",
            "    }\n",
            "  }\n",
            "}\n",
        ),
    );
    assert_eq!(
        message,
        "channel capability violation: task 1 cannot use the Receiver of channel 1 \
         in Channel.receive: this copy is stale; the capability was sent in a message \
         no task has received yet"
    );
}
