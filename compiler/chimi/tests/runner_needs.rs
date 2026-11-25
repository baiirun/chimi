//! Runner-supplied needs (CHI-319): what `chimi inspect` says about a host
//! need matches what the checker does with it. For each host interface, a
//! `main` and a test that need it, under the runner's name and under another
//! name, are checked; `E5002` (main left a need open) and `T4400` (a test
//! left a need open) must appear exactly when the inspection says the runner
//! or the test plan does not supply it. The table is also the list in
//! docs/runner-supplied-needs.md.

use std::path::PathBuf;
use std::process::Command;

/// A host interface, where to import it from, the need name the runner
/// closes (`None` for any name), and what a test gets.
const HOST_INTERFACES: &[(&str, &str, Option<&str>, &str)] = &[
    ("Console", "std/io", None, "captured_console"),
    ("FileSystem", "std/io", None, "using"),
    ("Clock", "std/clock", Some("clock"), "virtual_clock"),
    ("Random", "std/random", Some("random"), "seeded_random"),
    ("Environment", "std/env", Some("environment"), "using"),
    ("Files", "std/fs", Some("files"), "using"),
    ("Signals", "std/signal", Some("signals"), "using"),
    ("Commands", "std/exec", Some("commands"), "using"),
    ("Terminal", "std/terminal", Some("terminal"), "using"),
    ("Network", "std/net", Some("network"), "using"),
    ("Log", "std/log", Some("log"), "using"),
    ("Metrics", "std/metrics", Some("metrics"), "using"),
];

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "chimi-runner-needs-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        if path.exists() {
            std::fs::remove_dir_all(&path).unwrap();
        }
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn chimi(&self, arguments: &[&str]) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&self.0)
            .args(arguments)
            .output()
            .expect("run chimi");
        String::from_utf8(output.stdout).unwrap()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn program(interface: &str, module: &str, need: &str) -> String {
    format!(
        "import {module}.{{{interface}}}\n\n\
         fn reach() -> Void needs {{ {need}: {interface} }} {{\n  Void\n}}\n\n\
         pub fn main() -> Void needs {{ {need}: {interface} }} {{\n  reach()\n}}\n\n\
         test \"reaches\" {{\n  reach()\n}}\n"
    )
}

// For every host interface and both a right and a wrong need name, the
// inspection's `supplied_on_main` and `in_tests` agree with the checker.
// Rejects a table entry with the wrong fixed name or test supply, a host
// interface missing from the table, or inspection that ignores the name.
#[test]
fn inspection_of_a_host_need_matches_what_the_checker_closes() {
    let scratch = Scratch::new("table");
    for &(interface, module, fixed, in_tests) in HOST_INTERFACES {
        let names = match fixed {
            Some(fixed) => vec![(fixed, true), ("other_name", false)],
            None => vec![("other_name", true)],
        };
        for (need, on_main) in names {
            let source = program(interface, module, need);
            std::fs::write(scratch.0.join("main.chimi"), &source).unwrap();
            let check = scratch.chimi(&["check", "--json", "main.chimi"]);
            let case = format!("{interface} as `{need}`: {check}");
            assert_eq!(check.contains("\"E5002\""), !on_main, "{case}");
            let test_open = !on_main || in_tests == "using";
            assert_eq!(check.contains("\"T4400\""), test_open, "{case}");
            let errors: Vec<&str> = check
                .split("\"code\":\"")
                .skip(1)
                .filter_map(|rest| {
                    let code = rest.split('"').next()?;
                    rest[code.len()..]
                        .starts_with("\",\"severity\":\"error\"")
                        .then_some(code)
                })
                .filter(|code| !matches!(*code, "E5002" | "T4400"))
                .collect();
            assert!(errors.is_empty(), "{case}");

            let main = source.find("main()").unwrap().to_string();
            let inspection = scratch.chimi(&["inspect", "main.chimi", &main]);
            let fixed_json = fixed.map_or("null".to_owned(), |name| format!("\"{name}\""));
            let runner = format!(
                "\"runner\":{{\"fixed_name\":{fixed_json},\"in_tests\":\"{in_tests}\",\"supplied_on_main\":{on_main}}}"
            );
            assert!(inspection.contains(&runner), "{case}\n{inspection}");
        }
    }
}

// An ordinary interface's need has no runner entry. Rejects a table that
// marks every interface as host-supplied.
#[test]
fn an_ordinary_need_has_no_runner_entry() {
    let scratch = Scratch::new("ordinary");
    let source = "pub interface Mailer {\n  send: fn(Text) -> Void\n}\n\n\
                  pub fn main() -> Void needs { mailer: Mailer } {\n  mailer.send(\"hi\")\n}\n";
    std::fs::write(scratch.0.join("main.chimi"), source).unwrap();
    let main = source.find("main()").unwrap().to_string();
    let inspection = scratch.chimi(&["inspect", "main.chimi", &main]);
    assert!(
        inspection.contains("{\"name\":\"mailer\",\"interface\":\"Mailer\",\"runner\":null}"),
        "{inspection}"
    );
}

// A reference to a need names it with its runner entry, so hovering
// `console` in `console.write_line(...)` can say where it comes from.
#[test]
fn a_reference_to_a_need_reports_the_need() {
    let scratch = Scratch::new("reference");
    let source = "import std/io.{Console}\n\n\
                  pub fn main() -> Void needs { console: Console } {\n  console.write_line(\"hi\")\n}\n";
    std::fs::write(scratch.0.join("main.chimi"), source).unwrap();
    let at = source.find("console.write_line").unwrap().to_string();
    let inspection = scratch.chimi(&["inspect", "main.chimi", &at]);
    assert!(
        inspection.contains(
            "\"need\":{\"name\":\"console\",\"interface\":\"Console\",\"runner\":{\"fixed_name\":null,\"in_tests\":\"captured_console\",\"supplied_on_main\":true}}"
        ),
        "{inspection}"
    );
}
