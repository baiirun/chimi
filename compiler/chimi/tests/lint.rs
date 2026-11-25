//! `chimi lint`: convention findings, separate from the compiler's errors.

use chimi::Compiler;
use chimi::project::{LoadedProject, StandardTree};
use std::path::Path;
use std::process::{Command, Output};

fn chimi(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(arguments)
        .output()
        .expect("run chimi lint")
}

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/lint-project")
        .join(name)
        .display()
        .to_string()
}

fn scratch_file(name: &str, text: &str) -> String {
    let directory = std::env::temp_dir().join(format!("chimi-lint-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("main.chimi");
    std::fs::write(&path, text).unwrap();
    path.display().to_string()
}

#[test]
fn reports_unused_imports_private_items_and_bindings() {
    let path = fixture("app.chimi");
    let output = chimi(&["lint", &path]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        EXPECTED.replace("{path}", &path)
    );
}

const EXPECTED: &str = concat!(
    "{path}:2:26: warning[unused-import] lint: the imported name DecodeError is never used\n",
    "{path}:4:20: warning[unused-import] lint: the module alias Text is never used\n",
    "{path}:6:7: warning[unused-private] lint: the private constant unused_limit is never used; remove it or make it `pub`\n",
    "{path}:14:4: warning[unused-private] lint: the private function unused_helper is never used; remove it or make it `pub`\n",
    "{path}:18:8: warning[missing-doc] lint: the public fn exported has no doc comment; add a `///` line saying what it is for\n",
    "{path}:25:7: warning[unused-binding] lint: ignored is never used; remove it, or name it _ignored to keep it\n",
    "{path}:28:10: warning[unused-binding] lint: first is never used; remove it, or name it _first to keep it\n",
);

#[test]
fn clean_code_has_no_findings() {
    let path = scratch_file(
        "clean",
        "import std/text as Text\nimport std/clock.{Instant}\n\n\
         /// Doc.\npub fn shout(words: Text) -> Text {\n  let loud = Text.repeat(words, 2)\n  loud\n}\n\n\
         /// Doc.\npub fn same(at: Instant) -> Instant {\n  at\n}\n\n\
         /// A plain comment may sit between a doc comment and its item.\n// note\n\
         pub fn noted() -> Int {\n  1\n}\n",
    );
    let output = chimi(&["lint", &path]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        output.stdout.is_empty() && output.stderr.is_empty(),
        "{output:?}"
    );
}

// A combined import line is checked binding by binding: the members used, the
// alias not.
#[test]
fn a_combined_import_reports_its_unused_alias() {
    let path = scratch_file(
        "combined",
        "import std/map as Map.{Map}\n\n/// Doc.\npub fn sized(map: Map<Int, Int>) -> Int {\n  0\n}\n",
    );
    let output = chimi(&["lint", &path]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!("{path}:1:19: warning[unused-import] lint: the module alias Map is never used\n")
    );
}

#[test]
fn a_qualified_name_uses_its_import() {
    // `Order.by` uses the alias `Order`; `Shape.Circle` in a type position
    // uses the imported enum `Shape`.
    let path = scratch_file(
        "qualified",
        "import std/order as Order\nimport std/list as List\nimport std/bytes as Bytes\n\n\
         /// Doc.\npub fn sorted(values: List<Int>) -> List<Int> {\n  List.sort_by(values, Order.by(fn(value) { value }))\n}\n\n\
         /// Doc.\npub fn size(data: Bytes.Bytes) -> Int {\n  Bytes.length(data)\n}\n",
    );
    let output = chimi(&["lint", &path]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
}

#[test]
fn json_lists_the_findings_for_tools() {
    let output = chimi(&["lint", "--json", &fixture("app.chimi")]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let json = String::from_utf8(output.stdout).unwrap();
    for code in ["unused-import", "unused-private", "unused-binding"] {
        assert!(json.contains(&format!("\"code\":\"{code}\"")), "{json}");
    }
    assert!(json.contains("\"stage\":\"lint\""), "{json}");
    assert!(json.contains("\"severity\":\"warning\""), "{json}");
}

#[test]
fn code_that_does_not_type_check_reports_its_errors_instead() {
    let path = scratch_file(
        "invalid",
        "/// Doc.\npub fn broken() -> Int {\n  let unused = 1\n  \"text\"\n}\n",
    );
    let output = chimi(&["lint", &path]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error["), "{stderr}");
    assert!(!stderr.contains("unused-binding"), "{stderr}");
}

#[test]
fn the_standard_tree_is_lint_clean() {
    let tree = LoadedProject::standard_tree(&StandardTree::default()).unwrap();
    let report = Compiler::type_project(tree.sources());
    assert!(report.is_success());
    let findings = report.lint(true);
    assert!(
        findings.is_empty(),
        "{}",
        tree.render_diagnostics(&findings)
    );
}

#[test]
fn a_project_lints_its_own_modules_but_not_std() {
    // A standard tree with a finding of its own: the project stays clean.
    let std_dir = std::env::temp_dir().join(format!("chimi-lint-std-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&std_dir);
    std::fs::create_dir_all(&std_dir).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("std");
    for entry in std::fs::read_dir(&source).unwrap() {
        let path = entry.unwrap().path();
        std::fs::copy(&path, std_dir.join(path.file_name().unwrap())).unwrap();
    }
    let option = std_dir.join("option.chimi");
    let text = std::fs::read_to_string(&option).unwrap();
    std::fs::write(&option, format!("import std/list as Unused\n{text}")).unwrap();
    let path = scratch_file(
        "with-std",
        "import std/option as Option\n\n/// Doc.\npub fn zero() -> Int {\n  Option.unwrap_or(None, 0)\n}\n",
    );

    let output = chimi(&["--std-dir", std_dir.to_str().unwrap(), "lint", &path]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
}

#[test]
fn the_example_programs_are_lint_clean() {
    let programs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs");
    let mut linted = 0;
    for entry in std::fs::read_dir(&programs).unwrap() {
        let directory = entry.unwrap().path();
        if !directory.is_dir() {
            continue;
        }
        let output = chimi(&["lint", directory.to_str().unwrap()]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}: {output:?}",
            directory.display()
        );
        linted += 1;
    }
    assert!(linted >= 5, "found {linted} programs");
}

#[test]
fn a_module_alias_does_not_use_a_member_of_the_same_name() {
    let path = scratch_file(
        "same-name",
        "import std/bytes as Bytes\nimport std/bytes.{Bytes}\n\n\
         /// Doc.\npub fn empty() -> Int {\n  Bytes.length(Bytes.empty())\n}\n",
    );
    let output = chimi(&["lint", &path]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!(
            "{path}:2:19: warning[unused-import] lint: the imported name Bytes is never used\n"
        )
    );
}

#[test]
fn an_alias_used_only_in_a_constructor_pattern_is_used() {
    let path = scratch_file(
        "pattern-alias",
        "import std/order as Order\nimport std/order.{Ordering}\n\n\
         /// Doc.\npub fn label(ordering: Ordering) -> Int {\n  case ordering {\n    Order.Ordering.Less -> 1\n    _ -> 0\n  }\n}\n",
    );
    let output = chimi(&["lint", &path]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
}

#[test]
fn an_alias_qualified_type_does_not_use_a_member_of_the_same_name() {
    let path = scratch_file(
        "alias-type",
        "import std/clock as Clock\nimport std/clock.{Clock}\n\n\
         /// Doc.\npub fn same(at: Clock.Instant) -> Clock.Instant {\n  at\n}\n",
    );
    let output = chimi(&["lint", &path]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!(
            "{path}:2:19: warning[unused-import] lint: the imported name Clock is never used\n"
        )
    );
}

fn lint_stderr(name: &str, text: &str) -> (Option<i32>, String, String) {
    let path = scratch_file(name, text);
    let output = chimi(&["lint", &path]);
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).replace(&path, "{path}"),
        path,
    )
}

// A use counts only when a root reaches it: two private functions that call
// each other, one that calls only itself, and a constant only dead code reads
// are all reported, while a chain a public function starts, and what a `_`
// item uses, are not.
#[test]
fn a_private_item_only_dead_code_uses_is_reported() {
    let (code, stderr, _) = lint_stderr(
        "reachability",
        "const step: Int = 1\n\n\
         fn ping(count: Int) -> Int {\n  pong(count - step)\n}\n\n\
         fn pong(count: Int) -> Int {\n  ping(count)\n}\n\n\
         fn spin(count: Int) -> Int {\n  spin(count)\n}\n\n\
         fn inner() -> Int {\n  2\n}\n\n\
         fn outer() -> Int {\n  inner()\n}\n\n\
         fn kept() -> Int {\n  3\n}\n\n\
         fn _spare() -> Int {\n  kept()\n}\n\n\
         /// Doc.\npub fn value() -> Int {\n  outer()\n}\n",
    );

    assert_eq!(code, Some(1));
    assert_eq!(
        stderr,
        concat!(
            "{path}:1:7: warning[unused-private] lint: the private constant step is used only by code that is never used; remove it or make it `pub`\n",
            "{path}:3:4: warning[unused-private] lint: the private function ping is used only by code that is never used; remove it or make it `pub`\n",
            "{path}:7:4: warning[unused-private] lint: the private function pong is used only by code that is never used; remove it or make it `pub`\n",
            "{path}:11:4: warning[unused-private] lint: the private function spin is never used; remove it or make it `pub`\n",
        )
    );
}

#[test]
fn unused_private_types_and_names_are_reported() {
    let (code, stderr, _) = lint_stderr(
        "private-types",
        "struct Point {\n  x: Int,\n}\n\n\
         enum Mode {\n  Fast,\n  Slow,\n}\n\n\
         interface Greeter {\n  greet: fn() -> Int\n}\n\n\
         type Count = Int\n\n\
         process name worker: Int\n\n\
         needs Shop {\n  greeter: Greeter,\n}\n\n\
         struct Used {\n  y: Int,\n}\n\n\
         enum Matched {\n  One,\n  Two,\n}\n\n\
         type Total = Int\n\n\
         /// Doc.\npub fn make(total: Total) -> Int {\n  let used = Used { y: total }\n  used.y\n}\n\n\
         /// Doc.\npub fn which(value: Matched) -> Int {\n  case value {\n    One -> 1\n    _ -> 2\n  }\n}\n",
    );

    assert_eq!(code, Some(1));
    assert_eq!(
        stderr,
        concat!(
            "{path}:1:8: warning[unused-private] lint: the private struct Point is never used; remove it or make it `pub`\n",
            "{path}:5:6: warning[unused-private] lint: the private enum Mode is never used; remove it or make it `pub`\n",
            "{path}:10:11: warning[unused-private] lint: the private interface Greeter is used only by code that is never used; remove it or make it `pub`\n",
            "{path}:14:6: warning[unused-private] lint: the private type alias Count is never used; remove it or make it `pub`\n",
            "{path}:16:14: warning[unused-private] lint: the private process name worker is never used; remove it or make it `pub`\n",
            "{path}:18:7: warning[unused-private] lint: the private needs group Shop is never used; remove it or make it `pub`\n",
        )
    );
}

// A written needs entry the body never reaches, directly or through a call, is
// reported on a private function and on `main`; a public function's entries
// are its contract, so they stay.
#[test]
fn a_written_need_the_body_never_reaches_is_reported() {
    let (code, stderr, _) = lint_stderr(
        "needs",
        "import std/clock as Clock.{Clock}\n\n\
         fn idle() -> Int needs {\n  clock: Clock,\n} {\n  1\n}\n\n\
         fn ticks() -> Int needs {\n  clock: Clock,\n} {\n  let _at = Clock.now()\n  2\n}\n\n\
         fn through() -> Int needs {\n  clock: Clock,\n} {\n  ticks()\n}\n\n\
         /// Doc.\npub fn contract() -> Int needs {\n  clock: Clock,\n} {\n  3\n}\n\n\
         /// Doc.\npub fn total() -> Int needs {\n  clock: Clock,\n} {\n  idle() + through()\n}\n",
    );

    assert_eq!(code, Some(1));
    assert_eq!(
        stderr,
        "{path}:4:3: warning[unused-need] lint: idle never reaches the need clock; remove it from the needs clause\n"
    );
}

// Each fix, applied alone, leaves a module that still type-checks and no
// longer has that finding: an import line loses only the unused part, or goes
// when nothing is left, and a binding gets a leading `_`.
#[test]
fn each_fix_removes_its_finding_and_keeps_the_module_valid() {
    let text = "import std/map as Map.{Map}\nimport std/list as List\n\
                import std/clock.{Clock, Instant}\nimport std/order.{Ordering}\n\n\
                /// Doc.\npub fn sized(map: Map<Int, Int>, at: Instant) -> Int {\n  let unused = 1\n  0\n}\n";
    let lint = |text: &str| {
        let module = chimi::ProjectSource::new(
            "main",
            chimi::source::SourceSnapshot::from_text("main.chimi", text.to_owned()),
        )
        .unwrap();
        let report = Compiler::type_project(vec![module]);
        assert!(report.is_success(), "{text}");
        report.lint(false)
    };
    let findings = lint(text);
    let mut fixed = Vec::new();
    for finding in &findings {
        let [fix] = finding.fixes() else {
            panic!("{} has {} fixes", finding.message(), finding.fixes().len());
        };
        let (start, end) = (fix.range().start() as usize, fix.range().end() as usize);
        let after = format!("{}{}{}", &text[..start], fix.replacement(), &text[end..]);
        let remaining = lint(&after);
        assert_eq!(remaining.len(), findings.len() - 1, "{after}");
        fixed.push(after.lines().take(4).collect::<Vec<_>>().join("|"));
        if finding.code() == "unused-binding" {
            assert!(after.contains("  let _unused = 1\n"), "{after}");
        }
    }
    assert_eq!(
        fixed,
        [
            "import std/map.{Map}|import std/list as List|import std/clock.{Clock, Instant}|import std/order.{Ordering}",
            "import std/map as Map.{Map}|import std/clock.{Clock, Instant}|import std/order.{Ordering}|",
            "import std/map as Map.{Map}|import std/list as List|import std/clock.{Instant}|import std/order.{Ordering}",
            "import std/map as Map.{Map}|import std/list as List|import std/clock.{Clock, Instant}|",
            "import std/map as Map.{Map}|import std/list as List|import std/clock.{Clock, Instant}|import std/order.{Ordering}",
        ]
    );
}

#[test]
fn an_import_line_not_in_one_line_form_gets_no_fix() {
    let text = "import std/clock.{\n  Clock,\n  Instant,\n}\n\n/// Doc.\npub fn same(at: Instant) -> Instant {\n  at\n}\n";
    let module = chimi::ProjectSource::new(
        "main",
        chimi::source::SourceSnapshot::from_text("main.chimi", text.to_owned()),
    )
    .unwrap();
    let findings = Compiler::type_project(vec![module]).lint(false);
    assert_eq!(findings.len(), 1);
    assert!(findings[0].fixes().is_empty());
}

// A fix must not make the module invalid or leave part of the line behind.
#[test]
fn a_fix_that_would_break_the_module_is_not_offered() {
    let findings = |text: &str| {
        let module = chimi::ProjectSource::new(
            "main",
            chimi::source::SourceSnapshot::from_text("main.chimi", text.to_owned()),
        )
        .unwrap();
        Compiler::type_project(vec![module]).lint(false)
    };
    // `_x` is taken, so naming `x` `_x` would be a duplicate local.
    let taken = findings("/// Doc.\npub fn f() -> Int {\n  let x = 1\n  let _x = 2\n  _x\n}\n");
    assert_eq!(taken.len(), 1);
    assert!(taken[0].fixes().is_empty());
    // A trailing comment would be left on its own.
    let commented = findings("import std/list as List // keep\n\n/// Doc.\npub fn f() -> Int {\n  1\n}\n");
    assert_eq!(commented.len(), 1);
    assert!(commented[0].fixes().is_empty());
    // A CRLF line goes with its whole terminator.
    let text = "import std/list as List\r\n\r\n/// Doc.\r\npub fn f() -> Int {\r\n  1\r\n}\r\n";
    let crlf = findings(text);
    let fix = &crlf[0].fixes()[0];
    assert_eq!((fix.range().start(), fix.range().end()), (0, 25));
}
