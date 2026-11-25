//! Conformance for `std/terminal` (CHI-251): the escape codes `style` writes
//! and `strip` removes, the `uses_color` rule (a terminal, `NO_COLOR` unset or
//! empty, `TERM` not `dumb`), fakes through `using`, and the runner's
//! provider on piped streams and on a pseudo-terminal.
//!
//! Each test writes `cases.chimi` to a scratch directory and runs the CLI there.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str, source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "chimi-terminal-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        if path.exists() {
            std::fs::remove_dir_all(&path).unwrap();
        }
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("cases.chimi"), source).unwrap();
        Self(path)
    }

    fn chimi(&self, arguments: &[&str]) -> Run {
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&self.0)
            .args(arguments)
            .output()
            .expect("run chimi");
        Run {
            status: output.status.code(),
            stdout: String::from_utf8(output.stdout).unwrap(),
            stderr: String::from_utf8(output.stderr).unwrap(),
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Run {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

const IMPORTS: &str = r#"import std/debug as Debug
import std/env.{Environment}
import std/io.{Console}
import std/path.{Path}
import std/terminal as Terminal
import std/terminal.{Color, Standard, Style}
import std/test as Test

// A fake terminal: every stream is a terminal when `attached` is true.
fn screen(attached: Bool) {
  {
    is_terminal: fn(_stream) { attached },
    size: fn(stream) {
      case (attached, stream) {
        (True, Standard.Output) -> Some({ columns: 100, rows: 30 })
        _ -> None
      }
    },
  }
}

fn variables(pairs: List<(Text, Text)>) {
  { variables: fn() { pairs }, current_directory: fn() -> Option<Path> { None } }
}
"#;

fn run_tests(name: &str, tests: &str, backend: &[&str]) -> Run {
    let mut arguments = vec!["test", "cases.chimi"];
    arguments.extend_from_slice(backend);
    Scratch::new(name, &format!("{IMPORTS}\n{tests}")).chimi(&arguments)
}

fn assert_passed(run: &Run, count: usize) {
    assert_eq!(run.status, Some(0), "{}{}", run.stdout, run.stderr);
    assert!(
        run.stdout.contains(&format!("{count} passed, 0 failed")),
        "{}",
        run.stdout
    );
}

// Rejects wrong SGR codes, a style list that does not compose into one
// sequence, a blanket reset (ESC[0m) instead of each style's own close, a
// close repeated for styles that share it, styling of empty input, and a
// `strip` that leaves a sequence or eats text that is not one. Pure, so both
// backends run it.
#[test]
fn styles_write_their_codes_and_strip_removes_them() {
    let tests = r#"
test "each style's codes" {
  Debug.render(Terminal.style("hi", [Style.Bold])) |> Test.assert_equal("\"\\u{1b}[1mhi\\u{1b}[22m\"")
  Debug.render(Terminal.style("x", [Style.Dim, Style.Italic, Style.Underline, Style.Inverse, Style.Strikethrough]))
    |> Test.assert_equal("\"\\u{1b}[2;3;4;7;9mx\\u{1b}[22;23;24;27;29m\"")
  Debug.render(Terminal.style("x", [Style.Foreground(Color.Black), Style.Background(Color.White)]))
    |> Test.assert_equal("\"\\u{1b}[30;47mx\\u{1b}[39;49m\"")
  Debug.render(Terminal.style("x", [Style.Foreground(Color.BrightRed), Style.Background(Color.BrightCyan)]))
    |> Test.assert_equal("\"\\u{1b}[91;106mx\\u{1b}[39;49m\"")
  Debug.render(Terminal.style("x", [Style.Foreground(Color.Magenta), Style.Background(Color.BrightBlack)]))
    |> Test.assert_equal("\"\\u{1b}[35;100mx\\u{1b}[39;49m\"")
}

test "styles that share a close write it once" {
  Debug.render(Terminal.style("x", [Style.Bold, Style.Dim])) |> Test.assert_equal("\"\\u{1b}[1;2mx\\u{1b}[22m\"")
}

test "nothing to style is left alone" {
  Terminal.style("plain", []) |> Test.assert_equal("plain")
  Terminal.style("", [Style.Bold]) |> Test.assert_equal("")
}

test "strip removes sequences and keeps the rest" {
  let styled = Terminal.style("red", [Style.Bold, Style.Foreground(Color.Red)])
  Terminal.strip("a ${styled} b") |> Test.assert_equal("a red b")
  Terminal.strip("\u{1b}[2J\u{1b}[1;1Hgo") |> Test.assert_equal("go")
  Terminal.strip("no codes, ünïcode") |> Test.assert_equal("no codes, ünïcode")
  Terminal.strip("cut \u{1b}[") |> Test.assert_equal("cut \u{1b}[")
  Terminal.strip("odd \u{1b}[1\u{7}x") |> Test.assert_equal("odd \u{1b}[1\u{7}x")
  Terminal.strip("é\u{1b}[31mñ\u{1b}[0m日") |> Test.assert_equal("éñ日")
  Terminal.strip("\u{1b}[1m\u{1b}[4m\u{1b}[?25lx") |> Test.assert_equal("x")
  Terminal.strip("lone \u{1b}x \u{1b}") |> Test.assert_equal("lone \u{1b}x \u{1b}")
}

test "only an outer style of another attribute survives an inner close" {
  let inner = Terminal.style("u", [Style.Underline])
  Debug.render(Terminal.style("a${inner}b", [Style.Bold]))
    |> Test.assert_equal("\"\\u{1b}[1ma\\u{1b}[4mu\\u{1b}[24mb\\u{1b}[22m\"")
}
"#;
    assert_passed(&run_tests("styles", tests, &[]), 5);
    assert_passed(&run_tests("styles-native", tests, &["--backend=native"]), 5);
}

// Rejects a `uses_color` that colors piped output, ignores NO_COLOR or
// treats an empty NO_COLOR as set, ignores TERM=dumb, or asks about a stream
// other than the one named; and queries that do not reach the fake.
#[test]
fn color_follows_the_terminal_and_no_color() {
    let tests = r#"
test "a terminal with nothing set uses color" {
  let used = Terminal.uses_color(Standard.Output) using { terminal: screen(True), environment: variables([("TERM", "xterm")]) }
  used |> Test.assert_equal(True)
}

test "piped output does not" {
  let used = Terminal.uses_color(Standard.Output) using { terminal: screen(False), environment: variables([]) }
  used |> Test.assert_equal(False)
}

test "NO_COLOR turns it off unless empty" {
  let set = Terminal.uses_color(Standard.Error) using { terminal: screen(True), environment: variables([("NO_COLOR", "1")]) }
  let empty = Terminal.uses_color(Standard.Error) using { terminal: screen(True), environment: variables([("NO_COLOR", "")]) }
  (set, empty) |> Test.assert_equal((False, True))
}

test "a dumb terminal does not" {
  let used = Terminal.uses_color(Standard.Output) using { terminal: screen(True), environment: variables([("TERM", "dumb")]) }
  used |> Test.assert_equal(False)
}

test "the named stream is the one asked" {
  let only_error = {
    is_terminal: fn(stream) { stream == Standard.Error },
    size: fn(_stream) { None },
  }
  let asked = (Terminal.uses_color(Standard.Output), Terminal.uses_color(Standard.Error))
    using { terminal: only_error, environment: variables([]) }
  asked |> Test.assert_equal((False, True))
}

test "queries reach the fake" {
  let seen = (Terminal.is_terminal(Standard.Input), Terminal.size(Standard.Output), Terminal.size(Standard.Error))
    using { terminal: screen(True) }
  Debug.render(seen) |> Test.assert_equal("(True, Some({ columns: 100, rows: 30 }), None)")
}
"#;
    assert_passed(&run_tests("color", tests, &[]), 6);
    assert_passed(&run_tests("color-native", tests, &["--backend=native"]), 6);
}

const PROBE: &str = r#"
pub fn main() -> Void needs { console: Console, terminal: Terminal.Terminal } {
  let streams = [Standard.Input, Standard.Output, Standard.Error]
  console.write_line(Debug.render(List.map(streams, Terminal.is_terminal)))
  console.write_line(Debug.render(Terminal.size(Standard.Output)))
}
"#;

fn build_probe(name: &str) -> Scratch {
    let scratch = Scratch::new(name, &format!("{IMPORTS}import std/list as List\n{PROBE}"));
    let built = scratch.chimi(&["build", "cases.chimi", "-o", "probe"]);
    assert_eq!(built.status, Some(0), "{}{}", built.stdout, built.stderr);
    scratch
}

/// The global functions `executable` defines, from `nm`.
fn defined_functions(executable: &std::path::Path) -> Vec<String> {
    let output = Command::new("nm").arg("-g").arg("-U").arg(executable).output().expect("run nm");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .map(|name| name.strip_prefix('_').unwrap_or(name).to_owned())
        .collect()
}

// Rejects a link that keeps the Rust binding package whole: the provider is
// compiled into the Rust runtime archive every program links, so a program
// that does not reach it, with the scheduler or without, must not carry it,
// and one that does must.
#[test]
fn only_a_program_that_reaches_the_terminal_links_its_binding() {
    let probe = build_probe("linked");
    let functions = defined_functions(&probe.0.join("probe"));
    for symbol in ["chimi_terminal_is_terminal", "chimi_terminal_size"] {
        assert!(functions.iter().any(|name| name == symbol), "the probe lacks {symbol}");
    }
    let programs = [
        ("plain", "pub fn main() -> Int { 1 }\n"),
        (
            "tasks",
            "import std/task as Task\npub fn main() -> Int {\n  Task.result(Task.start(fn() -> Int { 1 }))\n}\n",
        ),
    ];
    for (name, source) in programs {
        let scratch = Scratch::new(name, source);
        let built = scratch.chimi(&["build", "cases.chimi", "-o", "program"]);
        assert_eq!(built.status, Some(0), "{}{}", built.stdout, built.stderr);
        assert!(built.stdout.contains("\"binding_packages\":[]"), "{}", built.stdout);
        let functions = defined_functions(&scratch.0.join("program"));
        assert!(
            !functions.iter().any(|name| name.starts_with("chimi_terminal_")),
            "the {name} program links the terminal binding"
        );
    }
}

// Rejects a build that links a Rust archive built without the binding
// packages: it must say so, not fail at link with undefined symbols.
#[test]
fn an_archive_without_the_bindings_is_refused_by_name() {
    let scratch = Scratch::new("no-bindings", &format!("{IMPORTS}import std/list as List\n{PROBE}"));
    let archive = scratch.0.join("empty.a");
    std::fs::write(&archive, b"!<arch>\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch.0)
        .args(["build", "cases.chimi", "-o", "probe"])
        .env("CHIMI_FIBER_RUNTIME_ARCHIVE", &archive)
        .output()
        .expect("run chimi");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_ne!(output.status.code(), Some(0));
    assert!(stderr.contains("built without the binding packages"), "{stderr}");
}

// Rejects a provider that reports a pipe as a terminal or gives a pipe a size.
#[test]
fn piped_streams_are_not_terminals() {
    let scratch = build_probe("piped");
    let output = Command::new(scratch.0.join("probe"))
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("run the probe");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "[False, False, False]\nNone\n");
}

// Rejects a provider that misses a terminal, reads another stream's
// descriptor, or swaps columns and rows: the probe runs on a pseudo-terminal
// sized 100 columns by 30 rows, with stdin a pipe and error output
// /dev/null, so only output is a terminal. python3's pty module makes the terminal on
// Linux and macOS alike.
#[test]
fn a_pseudo_terminal_is_one_and_has_its_size() {
    let scratch = build_probe("pty");
    assert_eq!(
        on_pty(&scratch, 30, 100),
        "[False, True, False]\nSome({ columns: 100, rows: 30 })\n"
    );
    assert_eq!(on_pty(&scratch, 30, 0), "[False, True, False]\nNone\n", "zero columns");
    assert_eq!(on_pty(&scratch, 0, 100), "[False, True, False]\nNone\n", "zero rows");
}

/// The probe's output on a pseudo-terminal of `rows` by `columns`.
fn on_pty(scratch: &Scratch, rows: u16, columns: u16) -> String {
    let script = r#"
import fcntl, os, pty, struct, sys, termios
read_end, write_end = os.pipe()
pid, fd = pty.fork()
if pid == 0:
    fcntl.ioctl(1, termios.TIOCSWINSZ, struct.pack("HHHH", int(sys.argv[2]), int(sys.argv[3]), 0, 0))
    os.dup2(read_end, 0)
    null = os.open(os.devnull, os.O_WRONLY)
    os.dup2(null, 2)
    os.execv(sys.argv[1], [sys.argv[1]])
os.close(write_end)
out = b""
while True:
    try:
        chunk = os.read(fd, 4096)
    except OSError:
        break
    if not chunk:
        break
    out += chunk
os.waitpid(pid, 0)
sys.stdout.write(out.decode().replace("\r\n", "\n"))
"#;
    let output = Command::new("python3")
        .args(["-c", script, scratch.0.join("probe").to_str().unwrap(), &rows.to_string(), &columns.to_string()])
        .output()
        .expect("run python3");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

// Rejects an interpreter that reaches the host terminal: the provider is a C
// binding, which only native code calls (E5204).
#[test]
fn the_interpreter_cannot_ask_the_host_terminal() {
    let scratch = Scratch::new("interpreter", &format!("{IMPORTS}import std/list as List\n{PROBE}"));
    let run = scratch.chimi(&["run", "cases.chimi"]);
    assert_ne!(run.status, Some(0));
    assert!(run.stdout.contains("E5204") || run.stderr.contains("E5204"), "{}{}", run.stdout, run.stderr);
}

// Rejects a test that reaches the terminal without supplying it: the need is
// closed only for `main` (T4400).
#[test]
fn a_test_must_supply_the_terminal() {
    let run = run_tests(
        "unsupplied",
        "test \"no terminal\" {\n  Terminal.is_terminal(Standard.Output) |> Test.assert_equal(False)\n}\n",
        &[],
    );
    assert_ne!(run.status, Some(0));
    assert!(run.stdout.contains("T4400") || run.stderr.contains("T4400"), "{}{}", run.stdout, run.stderr);
}
