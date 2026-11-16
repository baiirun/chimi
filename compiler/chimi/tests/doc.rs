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

#[test]
fn a_clause_group_shows_parameter_types_without_generated_names() {
    let path = scratch_file(
        "clauses",
        "pub enum Shape {\n  Circle(radius: Int)\n  Square(side: Int)\n}\n\
         \n\
         /// The area.\n\
         pub fn area(Circle(radius)) -> Int { radius * radius * 3 }\n\
         fn area(Square(side)) { side * side }\n",
    );
    let output = chimi(&["doc", &path]);

    assert!(output.status.success(), "{output:?}");
    let text = stdout(&output);
    assert!(
        text.contains("\npub fn area(Shape) -> Int\n    The area.\n"),
        "{text}"
    );
}

#[test]
fn the_second_item_on_a_line_has_its_own_header_and_no_doc() {
    // The file's first doc run documents `same`, so it is no module summary.
    let path = scratch_file(
        "one-line",
        "/// Returns its argument.\npub fn same<T>(x: T) -> T { x } pub fn one() -> Int { 1 }\n",
    );
    let output = chimi(&["doc", &path]);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stdout(&output),
        "module main\n\npub fn same<T>(x: T) -> T\n    Returns its argument.\n\npub fn one() -> Int\n"
    );
}

#[test]
fn crlf_declarations_print_with_plain_newlines() {
    let path = scratch_file(
        "crlf",
        "/// A point.\r\npub struct Point {\r\n  x: Int\r\n}\r\n",
    );
    let output = chimi(&["doc", "--json", &path]);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stdout(&output),
        "{\"module\":\"main\",\"summary\":null,\"items\":[{\"kind\":\"struct\",\"name\":\"Point\",\"signature\":\"pub struct Point {\\n  x: Int\\n}\",\"doc\":\"A point.\"}]}\n"
    );
}

fn scratch_directory(name: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!("chimi-doc-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

#[test]
fn html_writes_a_page_per_project_module_and_an_index() {
    let root = scratch_directory("html-project");
    std::fs::create_dir_all(root.join("lib")).unwrap();
    std::fs::write(
        root.join("app.chimi"),
        "/// The app.\n\nimport lib/geo as Geo\nimport base/unit as Unit\nimport std/list as List\n\n/// Twice the area, `<doubled>`.\npub fn double(side: Int) -> Int {\n  Geo.square(side) * Unit.one() * List.length([1, 2])\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("lib/geo.chimi"),
        "/// Shapes & sizes.\n\n/// - squares\n/// - more\npub fn square(side: Int) -> Int {\n  side * side\n}\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("base")).unwrap();
    std::fs::write(
        root.join("base/unit.chimi"),
        "/// One.\npub fn one() -> Int {\n  1\n}\n",
    )
    .unwrap();
    let site = root.join("site");

    let output = chimi(&[
        "doc",
        "--html",
        site.to_str().unwrap(),
        "--root",
        root.to_str().unwrap(),
        "app",
    ]);

    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let mut pages: Vec<String> = walk(&site)
        .iter()
        .map(|path| path.strip_prefix(&site).unwrap().display().to_string())
        .collect();
    pages.sort();
    assert_eq!(
        pages,
        ["app.html", "base/unit.html", "index.html", "lib/geo.html"]
    );
    let index = std::fs::read_to_string(site.join("index.html")).unwrap();
    assert!(
        index.contains("<li><a href=\"lib/geo.html\"><code>lib/geo</code></a> <span>Shapes &amp; sizes.</span></li>"),
        "{index}"
    );
    let app = std::fs::read_to_string(site.join("app.html")).unwrap();
    assert!(
        app.contains("<pre><code>pub fn double(side: Int) -&gt; Int</code></pre>\n<p>Twice the area, <code>&lt;doubled&gt;</code>.</p>"),
        "{app}"
    );
    let geo = std::fs::read_to_string(site.join("lib/geo.html")).unwrap();
    assert!(geo.contains("<a href=\"../index.html\">Index</a>"), "{geo}");
    assert!(
        geo.contains("<ul>\n<li>squares</li>\n<li>more</li>\n</ul>"),
        "{geo}"
    );
    assert!(
        !site.join("std").exists(),
        "std modules are not the project's"
    );
}

#[test]
fn html_for_std_documents_the_standard_tree() {
    let site = scratch_directory("html-std").join("site");

    let output = chimi(&["doc", "--html", site.to_str().unwrap(), "std"]);

    assert!(output.status.success(), "{output:?}");
    let index = std::fs::read_to_string(site.join("index.html")).unwrap();
    assert!(index.contains("href=\"std/option.html\""), "{index}");
    let option = std::fs::read_to_string(site.join("std/option.html")).unwrap();
    assert!(option.contains("unwrap_or"), "{option}");
}

#[test]
fn html_writes_nothing_for_a_project_that_does_not_type_check() {
    let root = scratch_directory("html-invalid");
    std::fs::write(
        root.join("app.chimi"),
        "pub fn broken() -> Int {\n  \"text\"\n}\n",
    )
    .unwrap();
    let site = root.join("site");

    let output = chimi(&[
        "doc",
        "--html",
        site.to_str().unwrap(),
        root.join("app.chimi").to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("error["));
    assert!(!site.exists());
}

fn walk(directory: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[test]
fn html_for_one_std_module_is_a_usage_error() {
    let site = scratch_directory("html-std-one").join("site");

    let output = chimi(&["doc", "--html", site.to_str().unwrap(), "std/option"]);

    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("name it as std"),
        "{output:?}"
    );
    assert!(!site.exists());
}
