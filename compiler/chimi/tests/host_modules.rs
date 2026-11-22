//! The system modules that reach the operating system (`std/env`), run as
//! programs on the reference interpreter and as native executables with a
//! controlled environment and working directory. Each test compares the two
//! backends' outcome records with the expected one.

use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch_directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("chimi-host-{name}-{}", std::process::id()));
    if path.exists() {
        std::fs::remove_dir_all(&path).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    path.canonicalize().unwrap()
}

/// A command with an empty environment except `variables`, run in `directory`,
/// plus two variables that are not UTF-8 (a value and a name), which both
/// backends leave out.
fn command(program: &Path, directory: &Path, variables: &[(&str, &str)]) -> Command {
    let mut command = Command::new(program);
    command
        .current_dir(directory)
        .env_clear()
        .envs(variables.iter().copied())
        .env("BROKEN", std::ffi::OsStr::from_bytes(b"\xff"))
        .env(std::ffi::OsStr::from_bytes(b"\xfeNAME"), "value");
    command
}

/// The terminal outcome record of `source` on both backends, which must agree.
fn run_both(name: &str, source: &str, variables: &[(&str, &str)]) -> String {
    let scratch = scratch_directory(name);
    let program = scratch.join("main.chimi");
    std::fs::write(&program, source).unwrap();

    // Each backend starts without the `work` directory a program may make.
    let fresh_work = || drop(std::fs::remove_dir_all(scratch.join("work")));
    fresh_work();
    let interpreted = command(Path::new(env!("CARGO_BIN_EXE_chimi")), &scratch, variables)
        .arg("run")
        .arg(&program)
        .output()
        .unwrap();
    let interpreted = String::from_utf8_lossy(&interpreted.stdout)
        .lines()
        .last()
        .unwrap_or_else(|| panic!("interpreter gave no outcome: {interpreted:?}"))
        .to_owned();

    let executable = scratch.join("program");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .arg("build")
        .arg(&program)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    fresh_work();
    let outcome = scratch.join("outcome.jsonl");
    let run = command(&executable, &scratch, variables)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!("--outcome-file={}", outcome.display()))
        .arg("--")
        .output()
        .unwrap();
    assert!(
        run.status.code().is_some(),
        "native runtime crashed: {run:?}"
    );
    let native = std::fs::read_to_string(&outcome)
        .unwrap()
        .trim_end()
        .to_owned();
    assert_eq!(interpreted, native, "the backends disagree");
    std::fs::remove_dir_all(scratch).unwrap();
    without_node_ids(&native)
}

fn without_node_ids(record: &str) -> String {
    let mut text = record.to_owned();
    for field in ["\"core_node_id\":", "\"origin_id\":"] {
        while let Some(start) = text.find(field) {
            let digits = text[start + field.len()..]
                .find(|character: char| !character.is_ascii_digit())
                .map_or(text.len(), |offset| start + field.len() + offset);
            let end = if text[digits..].starts_with(',') {
                digits + 1
            } else {
                digits
            };
            text.replace_range(start..end, "");
        }
    }
    text
}

fn value(record: &str) -> &str {
    record
        .split_once("\"value\":")
        .map(|(_, value)| value.trim_end_matches('}'))
        .unwrap_or_else(|| panic!("no value in {record}"))
}

// The runner closes `main`'s `environment` need with the host's variables and
// working directory, on both backends.
#[test]
fn the_runner_provides_the_environment_to_main() {
    // `run_both` runs in this same directory, recreated.
    let directory = scratch_directory("variables");
    let record = run_both(
        "variables",
        r#"
import std/debug as Debug
import std/env as Env
import std/env.{Environment}
import std/map as Map
import std/path as Path

pub fn main() -> List<Text> needs { environment: Environment } {
  [
    Debug.render(Env.get("GREETING")),
    Debug.render(Env.get("greeting")),
    Debug.render(Env.get("MISSING")),
    Debug.render(Env.get("EMPTY")),
    Debug.render([Env.get("=odd"), Env.get(""), Env.get("BROKEN")]),
    Debug.render(Map.size(Env.all())),
    Path.to_text(Env.current_directory()),
  ]
}
"#,
        &[("GREETING", "héllo wörld"), ("EMPTY", ""), ("=odd", "a=b")],
    );
    assert_eq!(
        value(&record),
        format!(
            r#"["Some(\"héllo wörld\")","None","None","Some(\"\")","[Some(\"a=b\"), None, None]","3","{}"]"#,
            directory.display()
        )
    );
}

// A function that reads the environment says so with `needs`, and a test that
// calls it must supply a fake: the test provider plan never closes it, so no
// test reads the machine's environment by accident.
#[test]
fn a_test_supplies_the_environment_as_a_fake() {
    let scratch = scratch_directory("fake");
    let program = scratch.join("main.chimi");
    let source = r#"
import std/env as Env
import std/env.{Environment, EnvError}
import std/map as Map
import std/option as Option
import std/result as Result
import std/test as Test

fn home() -> Text needs { environment: Environment } {
  Option.unwrap_or(Env.get("HOME"), "/")
}

test "the first value of a name wins" {
  let fake = {
    variables: fn() { [("HOME", "/home/a"), ("HOME", "/home/b")] },
    current_directory: fn() { None },
  }
  Test.assert_equal(home() using { environment: fake }, "/home/a")
  Test.assert_equal(Map.get(Env.all(), "HOME") using { environment: fake }, Some("/home/a"))
  Test.assert_equal(
    Result.capture(Env.current_directory) using { environment: fake },
    Result.Err(EnvError.NoCurrentDirectory),
  )
}
"#;
    std::fs::write(&program, source).unwrap();
    for backend in ["native", "interpreter"] {
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&scratch)
            .arg("test")
            .arg(format!("--backend={backend}"))
            .arg(&program)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{backend}: {stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(stdout.contains("1 passed, 0 failed"), "{backend}: {stdout}");
    }

    let unsupplied = "import std/env as Env\nimport std/test as Test\n\ntest \"reads the machine\" {\n  Test.assert_equal(Env.get(\"HOME\"), None)\n}\n";
    std::fs::write(&program, unsupplied).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .arg("test")
        .arg(&program)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("T4400"), "{stderr}");
    std::fs::remove_dir_all(scratch).unwrap();
}

// The need name is fixed: an `Environment` need under another name is not one
// the runner closes, so it cannot be a second way to name the same authority.
#[test]
fn only_the_environment_need_name_is_closed_by_the_runner() {
    let scratch = scratch_directory("need-name");
    let program = scratch.join("main.chimi");
    std::fs::write(
        &program,
        "import std/env.{Environment}\n\npub fn main() -> Void needs { env: Environment } {\n  Void\n}\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .arg("check")
        .arg(&program)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("E5002") && stderr.contains("main needs env"),
        "{stderr}"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// ----- std/fs ---------------------------------------------------------------

// The runner closes `main`'s `files` need with the host's file system, and
// both backends read, write, list, and fail the same way. Metadata is checked
// on the native backend alone, since the interpreter has no clock to build a
// `Timestamp` with.
#[test]
fn the_runner_provides_the_file_system_to_main() {
    let record = run_both(
        "files",
        r#"
import std/bytes as Bytes
import std/debug as Debug
import std/fs as Fs
import std/fs.{Files, WriteMode}
import std/list as List
import std/path as Path
import std/path.{Path}
import std/resource as Resource
import std/result as Result
import std/stream as Stream
import std/text as Text

fn at(name: Text) -> Path {
  Path.join(Path.from("work"), name)
}

fn outcome<T, E>(operation: fn() -> T errors E needs { files: Files }) -> Text needs { files: Files } {
  Debug.render(Result.capture(operation))
}

pub fn main() -> List<Text> needs { files: Files } {
  Fs.create_directory(Path.from("work"))
  Fs.create_directories(at("deep/er/est"))
  Fs.write_text(at("notes.txt"), "first line\n")
  Fs.append(at("notes.txt"), Bytes.from_text("second line\n"))
  let large = Text.repeat("0123456789abcdef", 8192)
  Fs.write_text(at("large.txt"), large)
  let writer = Fs.create(at("copy.txt"), WriteMode.New)
  let copied = Resource.scoped(fn() { Fs.open(at("large.txt")) }, Stream.stop, fn(reader) {
    Stream.copy(reader, writer)
  })
  Stream.close(writer)
  Fs.write(at("broken.txt"), Bytes.from([255, 254]))
  Fs.write_text(at("short.txt"), "a longer first text")
  Fs.write_text(at("short.txt"), "short")
  let first = Fs.create(at("first.txt"), WriteMode.Replace)
  Stream.close(first)
  let second = Fs.create(at("second.txt"), WriteMode.Replace)
  let late = Result.capture(fn() { Stream.write(first, Bytes.from_text("late")) })
  Stream.close(second)
  Fs.rename(at("notes.txt"), at("renamed.txt"))
  [
    Fs.read_text(at("renamed.txt")),
    Debug.render(Fs.read_text(at("copy.txt")) == large),
    Debug.render(copied),
    Fs.read_text(at("short.txt")),
    Debug.render(late),
    Debug.render(Bytes.length(Fs.read(at("second.txt")))),
    Debug.render(Fs.list(Path.from("work"))),
    Debug.render(Fs.list(at("deep"))),
    outcome(fn() { Fs.read_text(at("broken.txt")) }),
    outcome(fn() { Fs.read(at("notes.txt")) }),
    outcome(fn() { Fs.read(at("renamed.txt/inside")) }),
    outcome(fn() { Fs.read(at("deep")) }),
    outcome(fn() { Fs.list(at("renamed.txt")) }),
    outcome(fn() { Fs.create_directory(at("deep")) }),
    outcome(fn() { Fs.create(at("copy.txt"), WriteMode.New) }),
    outcome(fn() { Fs.remove(at("deep")) }),
    outcome(fn() { Fs.remove(at("deep/er/est")) }),
    Debug.render(Fs.list(at("deep/er"))),
    outcome(fn() { Fs.read(at(Bytes.to_text(Bytes.from([0])))) }),
  ]
}
"#,
        &[],
    );
    assert_eq!(
        value(&record),
        concat!(
            r#"["first line\nsecond line\n","True","131072","#,
            r#""short","Result.Err(FsError.Closed(\"work/first.txt\"))","0","#,
            r#""[\"broken.txt\", \"copy.txt\", \"deep\", \"first.txt\", \"large.txt\", \"renamed.txt\", \"second.txt\", \"short.txt\"]","#,
            r#""[\"er\"]","#,
            r#""Result.Err(DecodeError.InvalidUtf8(0))","#,
            r#""Result.Err(FsError.NotFound(\"work/notes.txt\"))","#,
            r#""Result.Err(FsError.NotADirectory(\"work/renamed.txt/inside\"))","#,
            r#""Result.Err(FsError.IsADirectory(\"work/deep\"))","#,
            r#""Result.Err(FsError.NotADirectory(\"work/renamed.txt\"))","#,
            r#""Result.Err(FsError.AlreadyExists(\"work/deep\"))","#,
            r#""Result.Err(FsError.AlreadyExists(\"work/copy.txt\"))","#,
            r#""Result.Err(FsError.NotEmpty(\"work/deep\"))","#,
            r#""Result.Ok(Void)","#,
            r#""[]","#,
            r#""Result.Err(FsError.Other(path: \"work/\\u{0}\", message: \"path contains a NUL byte\"))"]"#,
        )
    );
}

// Metadata follows symbolic links, and `create_directories` accepts the
// directories that exist; native only (the interpreter has no clock).
#[test]
fn metadata_reads_kind_size_and_modification_time() {
    let scratch = scratch_directory("metadata");
    let program = scratch.join("main.chimi");
    std::fs::create_dir(scratch.join("work")).unwrap();
    std::fs::write(scratch.join("work/file.txt"), "12345").unwrap();
    std::os::unix::fs::symlink("file.txt", scratch.join("work/link")).unwrap();
    std::fs::write(
        &program,
        r#"
import std/clock as Clock
import std/debug as Debug
import std/fs as Fs
import std/fs.{Files}
import std/path as Path
import std/result as Result

fn described(name: Text) -> Text needs { files: Files } {
  let metadata = Fs.metadata(Path.from(name))
  Debug.render((Fs.kind(metadata), Clock.unix_nanoseconds(Fs.modified(metadata)) > 1700000000000000000))
}

pub fn main() -> List<Text> needs { files: Files } {
  Fs.create_directories(Path.from("work/new/one"))
  Fs.create_directories(Path.from("work/new/one"))
  [
    described("work/file.txt"),
    described("work/link"),
    described("work"),
    described("/dev/null"),
    Debug.render(Fs.size(Fs.metadata(Path.from("work/file.txt")))),
    Debug.render(Fs.list(Path.from("work/new"))),
    Debug.render(Result.capture(fn() { Fs.create_directories(Path.from("work/file.txt/below")) })),
    Debug.render(Result.capture(fn() { Fs.create_directories(Path.from("work/file.txt")) })),
    Debug.render(Result.capture(fn() { Fs.metadata(Path.from("work/missing")) })),
  ]
}
"#,
    )
    .unwrap();
    let executable = scratch.join("program");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .arg("build")
        .arg(&program)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let outcome = scratch.join("outcome.jsonl");
    command(&executable, &scratch, &[])
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!("--outcome-file={}", outcome.display()))
        .arg("--")
        .output()
        .unwrap();
    let record = std::fs::read_to_string(&outcome).unwrap();
    assert_eq!(
        value(record.trim_end()),
        concat!(
            r#"["(Kind.File, True)","(Kind.File, True)","(Kind.Directory, True)","(Kind.Special, True)","#,
            r#""5","[\"one\"]","#,
            r#""Result.Err(FsError.NotADirectory(\"work/file.txt/below\"))","#,
            r#""Result.Err(FsError.AlreadyExists(\"work/file.txt\"))","#,
            r#""Result.Err(FsError.NotFound(\"work/missing\"))"]"#,
        ),
        "{record}"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// A test reaches real files only through `Fs.temporary_directory` and
// `Fs.files_within`: paths outside the directory, and relative paths, are
// refused, and the directory is gone when the test ends. A fake `Files` is a
// record of functions. Both run on both test backends.
#[test]
fn a_test_uses_real_files_in_a_temporary_directory_or_a_fake() {
    let scratch = scratch_directory("fs-tests");
    let temporary = scratch.join("tmp");
    std::fs::create_dir(&temporary).unwrap();
    let program = scratch.join("main.chimi");
    let source = r#"
import std/fs as Fs
import std/fs.{Files, FsError, Kind, WriteMode}
import std/list as List
import std/path as Path
import std/path.{Path}
import std/result as Result
import std/test as Test
import std/text as Text

fn backup(name: Path) -> Void needs { files: Files } {
  Fs.write(Path.from(Text.append(Path.to_text(name), ".bak")), Fs.read(name))
}

test "a backup copies the file" {
  Fs.temporary_directory(fn(directory) {
    let files = Fs.files_within(directory)
    let notes = Path.join(directory, "notes.txt")
    Fs.write_text(notes, "keep me") using { files: files }
    backup(notes) using { files: files }
    Test.assert_equal(Fs.read_text(Path.join(directory, "notes.txt.bak")) using { files: files }, "keep me")
    Test.assert_equal(Fs.list(directory) using { files: files }, ["notes.txt", "notes.txt.bak"])
  })
}

test "paths outside the directory are refused" {
  Fs.temporary_directory(fn(directory) {
    let files = Fs.files_within(directory)
    let escape = Path.join(directory, "../escape.txt")
    Test.assert_equal(
      Result.capture(fn() { Fs.write_text(escape, "no") }) using { files: files },
      Result.Err(FsError.PermissionDenied(escape)),
    )
    Test.assert_equal(
      Result.capture(fn() { Fs.read(Path.from("relative.txt")) }) using { files: files },
      Result.Err(FsError.PermissionDenied(Path.from("relative.txt"))),
    )
    let sibling = Path.from(Text.append(Path.to_text(directory), "-sibling/x"))
    Test.assert_equal(
      Result.capture(fn() { Fs.read(sibling) }) using { files: files },
      Result.Err(FsError.PermissionDenied(sibling)),
    )
    Fs.create_directories(Path.join(directory, "a/b")) using { files: files }
    let everything = Fs.files_within(Path.from("/"))
    Test.assert(List.contains(Fs.list(Path.from("/")) using { files: everything }, "tmp"), "the root lists")
    Test.assert_equal(Fs.list(directory) using { files: everything }, ["a"])
    Fs.write_text(Path.join(directory, "a/b/c.txt"), "deep") using { files: files }
  })
}

test "a fake supplies the files" {
  let fake = {
    open: fn(path: Path) { Failure(FsError.NotFound(path)) },
    create: fn(path: Path, mode: WriteMode) { Failure(FsError.PermissionDenied(path)) },
    metadata: fn(path: Path) { Failure(FsError.NotFound(path)) },
    list: fn(path: Path) { ["b", "a"] },
    create_directory: fn(path: Path) { Void },
    remove: fn(path: Path) { Void },
    rename: fn(from: Path, to: Path) { Void },
  }
  Test.assert_equal(Fs.list(Path.from("/any")) using { files: fake }, ["b", "a"])
  Test.assert_equal(
    Result.capture(fn() { backup(Path.from("/x")) }) using { files: fake },
    Result.Err(FsError.NotFound(Path.from("/x"))),
  )
}
"#;
    std::fs::write(&program, source).unwrap();
    for backend in ["native", "interpreter"] {
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&scratch)
            .env("TMPDIR", &temporary)
            .arg("test")
            .arg(format!("--backend={backend}"))
            .arg(&program)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{backend}: {stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(stdout.contains("3 passed, 0 failed"), "{backend}: {stdout}");
        let left: Vec<_> = std::fs::read_dir(&temporary).unwrap().collect();
        assert!(left.is_empty(), "{backend} left {left:?}");
    }

    let unsupplied = "import std/fs as Fs\nimport std/path as Path\nimport std/test as Test\n\ntest \"reads the machine\" {\n  Test.assert_equal(Fs.list(Path.from(\"/\")), [])\n}\n";
    std::fs::write(&program, unsupplied).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .arg("test")
        .arg(&program)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("T4400"), "{stderr}");

    let from_main = "import std/fs as Fs\n\npub fn main() -> Void {\n  Fs.temporary_directory(fn(directory) { Void })\n}\n";
    std::fs::write(&program, from_main).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .arg("check")
        .arg(&program)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("T4401"), "{stderr}");
    std::fs::remove_dir_all(scratch).unwrap();
}
