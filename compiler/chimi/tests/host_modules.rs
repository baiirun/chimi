//! The system modules that reach the operating system (`std/env`), run as
//! programs on the reference interpreter and as native executables with a
//! controlled environment and working directory. Each test compares the two
//! backends' outcome records with the expected one.

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

/// A command with an empty environment except `variables`, run in `directory`.
fn command(program: &Path, directory: &Path, variables: &[(&str, &str)]) -> Command {
    let mut command = Command::new(program);
    command
        .current_dir(directory)
        .env_clear()
        .envs(variables.iter().copied());
    command
}

/// The terminal outcome record of `source` on both backends, which must agree.
fn run_both(name: &str, source: &str, variables: &[(&str, &str)]) -> String {
    let scratch = scratch_directory(name);
    let program = scratch.join("main.chimi");
    std::fs::write(&program, source).unwrap();

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
