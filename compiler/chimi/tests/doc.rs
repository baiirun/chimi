//! `chimi doc`: the public surface of a module, from the compiler.

use std::path::Path;
use std::process::{Command, Output};

fn chimi(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(arguments)
        .output()
        .expect("run chimi doc")
}

fn fixture_project() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/doc-project")
        .display()
        .to_string()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("UTF-8 output")
}

fn scratch_file(name: &str, text: &str) -> String {
    let directory = std::env::temp_dir().join(format!("chimi-doc-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("main.chimi");
    std::fs::write(&path, text).unwrap();
    path.display().to_string()
}

#[test]
fn documents_public_items_with_inferred_contracts_and_doc_comments() {
    let output = chimi(&["doc", "--root", &fixture_project(), "shapes"]);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stdout(&output),
        "module shapes\n\
         \n\
         Plane shapes and what they measure.\n\
         \n\
         Sizes are whole units.\n\
         \n\
         pub enum Shape {\n  Circle(radius: Int)\n  Square(side: Int)\n}\n    A shape on the plane.\n\
         \n\
         pub enum ShapeError {\n  Negative(size: Int)\n}\n\
         \n\
         pub const square_sides: Int = 4\n    How many sides a square has.\n\
         \n\
         pub const examples: List<Shape>\n    The shapes the examples use.\n\
         \n\
         pub fn sides(shape: Shape) -> Int\n\
         \n\
         pub fn area(shape: Shape) -> Int errors ShapeError\n    The area, rounded down, or `Negative` for a negative size.\n"
    );
}

#[test]
fn json_carries_the_same_items_for_tools() {
    let output = chimi(&["doc", "--json", "--root", &fixture_project(), "shapes"]);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stdout(&output),
        concat!(
            r#"{"module":"shapes","summary":"Plane shapes and what they measure.\n\nSizes are whole units.","items":["#,
            r#"{"kind":"enum","name":"Shape","signature":"pub enum Shape {\n  Circle(radius: Int)\n  Square(side: Int)\n}","doc":"A shape on the plane."},"#,
            r#"{"kind":"enum","name":"ShapeError","signature":"pub enum ShapeError {\n  Negative(size: Int)\n}","doc":null},"#,
            r#"{"kind":"const","name":"square_sides","signature":"pub const square_sides: Int = 4","doc":"How many sides a square has."},"#,
            r#"{"kind":"const","name":"examples","signature":"pub const examples: List<Shape>","doc":"The shapes the examples use."},"#,
            r#"{"kind":"fn","name":"sides","signature":"pub fn sides(shape: Shape) -> Int","doc":null},"#,
            r#"{"kind":"fn","name":"area","signature":"pub fn area(shape: Shape) -> Int errors ShapeError","doc":"The area, rounded down, or `Negative` for a negative size."}"#,
            "]}\n"
        )
    );
}

#[test]
fn shows_inferred_needs_a_source_signature_leaves_out() {
    let path = scratch_file(
        "needs",
        "import std/clock as Clock\n\
         import std/clock.{Instant}\n\
         \n\
         /// The current time.\n\
         pub fn now() -> Instant {\n  Clock.now()\n}\n",
    );
    let output = chimi(&["doc", &path]);

    assert!(output.status.success(), "{output:?}");
    let text = stdout(&output);
    assert!(
        text.contains("pub fn now() -> Instant needs { clock: Clock }\n    The current time.\n"),
        "{text}"
    );
}

#[test]
fn documents_a_standard_module_by_its_path() {
    let output = chimi(&["doc", "std/option"]);

    assert!(output.status.success(), "{output:?}");
    let text = stdout(&output);
    assert!(
        text.starts_with("module std/option\n\nA value that may be absent."),
        "{text}"
    );
    assert!(
        text.contains("\npub fn unwrap_or<T>(option: Option<T>, fallback: T) -> T\n    Returns the value of `option`, or `fallback` when it is `None`.\n"),
        "{text}"
    );
}

#[test]
fn a_module_that_does_not_type_check_reports_its_diagnostics() {
    let path = scratch_file("invalid", "pub fn broken() -> Int {\n  \"text\"\n}\n");
    let output = chimi(&["doc", &path]);

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("error["),
        "{output:?}"
    );
}

#[test]
fn an_unknown_standard_module_fails() {
    let output = chimi(&["doc", "std/no_such_module"]);

    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "chimi: no module `std/no_such_module`\n"
    );
}
