//! Conformance for how a program ends with `std/cli.Exit` (CHI-252): the
//! runner writes `Help` to standard output with status 0 and `Usage` to
//! standard error with status 2, exits `Code(n)` with n, and does each only
//! after `main`'s releases have run. Parsing itself is tested in
//! `std-tests/cli`.
//!
//! Each test writes `cases.chimi` to a scratch directory, builds it natively
//! and runs it, and runs it with `chimi run`, the reference interpreter.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str, source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "chimi-cli-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        if path.exists() {
            std::fs::remove_dir_all(&path).unwrap();
        }
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("cases.chimi"), source).unwrap();
        let scratch = Self(path);
        let built = scratch.run(
            env!("CARGO_BIN_EXE_chimi"),
            &["build", "cases.chimi", "-o", "program"],
        );
        assert_eq!(built.status, Some(0), "{}{}", built.stdout, built.stderr);
        scratch
    }

    fn run(&self, program: &str, arguments: &[&str]) -> Run {
        let output = Command::new(program)
            .current_dir(&self.0)
            .args(arguments)
            .output()
            .expect("run program");
        Run {
            status: output.status.code(),
            stdout: String::from_utf8(output.stdout).unwrap(),
            stderr: String::from_utf8(output.stderr).unwrap(),
        }
    }

    fn native(&self, arguments: &[&str]) -> Run {
        self.run(self.0.join("program").to_str().unwrap(), arguments)
    }

    /// `chimi run`, whose standard output and error are what the program and
    /// the runner wrote, as a native executable's are.
    fn interpreted(&self, arguments: &[&str]) -> Run {
        let mut command = vec!["run", "cases.chimi"];
        if !arguments.is_empty() {
            command.push("--");
            command.extend_from_slice(arguments);
        }
        self.run(env!("CARGO_BIN_EXE_chimi"), &command)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug)]
struct Run {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

const GREP: &str = r#"import std/cli as Cli
import std/cli.{Exit}
import std/debug as Debug
import std/io.{Console}

const ignore_case = Cli.flag("ignore-case", "i", "Match without regard to case")
const max = Cli.option("max", "m", "Stop after N matches", Cli.int)
const pattern = Cli.argument("pattern", "What to search for", Cli.text)
const grep = Cli.version(Cli.command("grep", "Search files", [Cli.arg(ignore_case), Cli.arg(max), Cli.arg(pattern)]), "1.0.0")

pub fn main(arguments: List<Text>) -> Exit needs { console: Console } {
  let parsed = Cli.parse(grep, arguments)
  console.write_line("${Cli.get(parsed, pattern)} ${Debug.render(Cli.get(parsed, max))} ${Debug.render(Cli.get(parsed, ignore_case))}")
  Exit.Code(0)
}
"#;

const USAGE: &str = "Search files

usage: grep [options] <pattern>

Arguments:
  <pattern>  What to search for

Options:
  -i, --ignore-case  Match without regard to case
  -m, --max <int>    Stop after N matches
  -h, --help         Print this help
      --version      Print the version
";

const UNKNOWN: &str = "grep: unknown option --colour
usage: grep [options] <pattern>
Try 'grep --help' for more information.
";

/// Checks one run on both backends: its status, standard error, and
/// standard output, which is what the program wrote (`console`) and then
/// what the runner wrote (`shown`), the same on both.
fn check(
    scratch: &Scratch,
    arguments: &[&str],
    status: i32,
    console: &str,
    shown: &str,
    stderr: &str,
) {
    let native = scratch.native(arguments);
    assert_eq!(
        (native.status, native.stdout.clone(), native.stderr.as_str()),
        (Some(status), format!("{console}{shown}"), stderr),
        "native {arguments:?}"
    );
    let interpreted = scratch.interpreted(arguments);
    assert_eq!(
        (
            interpreted.status,
            interpreted.stdout,
            interpreted.stderr.as_str()
        ),
        (Some(status), format!("{console}{shown}"), stderr),
        "interpreted {arguments:?}"
    );
}

#[test]
fn parsed_arguments_reach_main_and_code_zero_exits_zero() {
    let scratch = Scratch::new("parsed", GREP);
    check(
        &scratch,
        &["-i", "--max=3", "needle"],
        0,
        "needle Some(3) True\n",
        "",
        "",
    );
}

#[test]
fn help_and_version_go_to_standard_output_with_status_zero() {
    let scratch = Scratch::new("help", GREP);
    check(&scratch, &["needle", "--help"], 0, "", USAGE, "");
    check(&scratch, &["-h"], 0, "", USAGE, "");
    check(&scratch, &["--version"], 0, "", "grep 1.0.0\n", "");
}

#[test]
fn a_usage_error_goes_to_standard_error_with_status_two() {
    let scratch = Scratch::new("usage", GREP);
    check(&scratch, &["--colour", "needle"], 2, "", "", UNKNOWN);
    check(
        &scratch,
        &[],
        2,
        "",
        "",
        "grep: missing argument <pattern>\nusage: grep [options] <pattern>\nTry 'grep --help' for more information.\n",
    );
}

#[test]
fn main_returns_the_code_it_exits_with() {
    let scratch = Scratch::new(
        "code",
        "import std/cli.{Exit}\n\npub fn main() -> Exit {\n  Exit.Code(3)\n}\n",
    );
    check(&scratch, &[], 3, "", "", "");
}

#[test]
fn a_code_outside_a_byte_is_a_defect() {
    let scratch = Scratch::new(
        "range",
        "import std/cli.{Exit}\n\npub fn main() -> Exit {\n  Exit.Code(256)\n}\n",
    );
    check(
        &scratch,
        &[],
        70,
        "",
        "",
        "Chimi runtime: Exit.Code(256) is outside 0 to 255\n",
    );
    let negative = Scratch::new(
        "negative",
        "import std/cli.{Exit}\n\npub fn main() -> Exit {\n  Exit.Code(-1)\n}\n",
    );
    check(
        &negative,
        &[],
        70,
        "",
        "",
        "Chimi runtime: Exit.Code(-1) is outside 0 to 255\n",
    );
}

#[test]
fn main_may_return_help_text_of_its_own() {
    let scratch = Scratch::new(
        "own-help",
        "import std/cli.{Exit}\n\npub fn main() -> Exit {\n  Exit.Help(\"see the manual\")\n}\n",
    );
    check(&scratch, &[], 0, "", "see the manual\n", "");
}

// The exit is applied after `main` ends, so a release in `main` runs first,
// whether the Exit was returned or failed with.
#[test]
fn releases_run_before_the_exit() {
    let scratch = Scratch::new(
        "release",
        r#"import std/cli as Cli
import std/cli.{Exit}
import std/io.{Console}
import std/resource as Resource

const name = Cli.argument("name", "Who", Cli.text)
const tool = Cli.command("tool", "A tool", [Cli.arg(name)])

pub fn main(arguments: List<Text>) -> Exit needs { console: Console } {
  Resource.scoped(
    fn() { arguments },
    fn(_) { console.write_line("released") },
    fn(held) {
      let parsed = Cli.parse(tool, held)
      console.write_line(Cli.get(parsed, name))
      Exit.Code(4)
    },
  )
}
"#,
    );
    check(&scratch, &["x"], 4, "x\nreleased\n", "", "");
    check(
        &scratch,
        &["--help"],
        0,
        "released\n",
        "A tool\n\nusage: tool [options] <name>\n\nArguments:\n  <name>  Who\n\nOptions:\n  -h, --help  Print this help\n",
        "",
    );
    check(
        &scratch,
        &["x", "y"],
        2,
        "released\n",
        "",
        "tool: unexpected argument \"y\"\nusage: tool [options] <name>\nTry 'tool --help' for more information.\n",
    );
}

// Only `std/cli`'s `Exit` ends the program: a program's own type of that name
// is an ordinary value, so returning it exits 0.
#[test]
fn a_type_of_the_same_name_is_an_ordinary_value() {
    let scratch = Scratch::new(
        "own-type",
        "pub enum Exit {\n  Code(code: Int)\n}\n\npub fn main() -> Exit {\n  Exit.Code(3)\n}\n",
    );
    check(&scratch, &[], 0, "", "", "");
}
