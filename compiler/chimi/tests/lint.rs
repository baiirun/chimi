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
         /// Doc.\npub fn same(at: Instant) -> Instant {\n  at\n}\n",
    );
    let output = chimi(&["lint", &path]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        output.stdout.is_empty() && output.stderr.is_empty(),
        "{output:?}"
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
