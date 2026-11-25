//! The standard library's own tests (`compiler/chimi/std-tests`).
//!
//! Most `<module>/<name>.chimi` files hold `test` items: `chimi test --json`
//! runs the whole directory on both backends, and every test must pass on
//! each. The rest are expected-output programs, kept where the point is the
//! runner's own report, such as an outcome record or a failure escaping
//! `main`: each has `pub fn main()` and a sibling `<name>.expected` holding the
//! runner's terminal outcome record. Every such program runs on the reference
//! interpreter and as a native executable: the two records must be identical,
//! and each must equal the expected file once the Core node and origin ids,
//! which move whenever std source changes, are removed. Every registered
//! native operation must be reached by some test or program, so each
//! primitive has differential coverage.

#[path = "support/interpreter.rs"]
mod interpreter;

use chimi::project::{LoadedProject, StandardTree};
use std::ffi::OsStr;
use chimi::{Compiler, ProjectSource};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Every `.chimi` file under `std-tests`, in path order.
fn sources() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("std-tests");
    let mut sources = Vec::new();
    for module in std::fs::read_dir(&root).expect("read std-tests") {
        let module = module.unwrap().path();
        for entry in std::fs::read_dir(&module).unwrap() {
            let path = entry.unwrap().path();
            if path
                .extension()
                .is_some_and(|extension| extension == "chimi")
            {
                sources.push(path);
            }
        }
    }
    sources.sort();
    sources
}

/// The expected-output programs: the sources with a sibling `.expected`.
fn programs() -> Vec<PathBuf> {
    let programs: Vec<PathBuf> = sources()
        .into_iter()
        .filter(|path| path.with_extension("expected").exists())
        .collect();
    assert!(!programs.is_empty(), "std-tests holds no programs");
    programs
}

/// The sources that hold `test` items: those without an `.expected` file.
fn test_modules() -> Vec<PathBuf> {
    let modules: Vec<PathBuf> = sources()
        .into_iter()
        .filter(|path| !path.with_extension("expected").exists())
        .collect();
    assert!(!modules.is_empty(), "std-tests holds no test modules");
    modules
}

/// `(test identity, outcome)` for each `test_finished` record of `chimi test
/// --json` on `std-tests` under `backend`, in report order.
fn test_outcomes(backend: &str) -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("std-tests");
    runner_outcomes("std-tests", &[OsStr::new("test"), root.as_os_str()], backend)
}

/// `chimi <command> --backend=<backend> --json`, run from a scratch directory:
/// each test's identity and `passed`, or `failed: <record>`.
fn runner_outcomes(name: &str, command: &[&OsStr], backend: &str) -> Vec<(String, String)> {
    let scratch = std::env::temp_dir().join(format!(
        "chimi-{name}-{}-runner-{backend}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(command)
        .arg(format!("--backend={backend}"))
        .arg("--json")
        .output()
        .expect("run chimi test");
    std::fs::remove_dir_all(&scratch).unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.code().is_some_and(|code| code <= 1),
        "{backend}: chimi test could not run: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let field = |line: &str, name: &str| {
        let start = line.find(&format!("\"{name}\":\"")).map(|start| start + name.len() + 4)?;
        let end = line[start..].find('"')? + start;
        Some(line[start..end].to_owned())
    };
    stdout
        .lines()
        .filter(|line| line.contains("\"test_finished\""))
        .map(|line| {
            let identity = format!(
                "{} {}",
                field(line, "module").unwrap_or_default(),
                field(line, "name").unwrap_or_default()
            );
            let outcome = match field(line, "outcome").as_deref() {
                Some("passed") => "passed".to_owned(),
                _ => format!("failed: {line}"),
            };
            (identity, outcome)
        })
        .collect()
}

fn scratch_directory(program: &Path) -> PathBuf {
    let name = program
        .strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")).join("std-tests"))
        .unwrap()
        .with_extension("")
        .to_string_lossy()
        .replace('/', "-");
    let path = std::env::temp_dir().join(format!("chimi-std-tests-{}-{name}", std::process::id()));
    if path.exists() {
        std::fs::remove_dir_all(&path).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn interpreter_record(program: &Path) -> String {
    let output = interpreter::traced(
        Command::new(env!("CARGO_BIN_EXE_chimi"))
            .arg("run")
            .arg(program),
    )
    .expect("run the reference interpreter");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .last()
        .unwrap_or_else(|| {
            panic!(
                "{}: interpreter did not complete: {}",
                program.display(),
                String::from_utf8_lossy(&output.stderr)
            )
        })
        .to_owned()
}

fn native_record(scratch: &Path, program: &Path) -> String {
    let executable = scratch.join("program");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(scratch)
        .arg("build")
        .arg(program)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("build the native executable");
    assert!(
        build.status.success(),
        "{}: native build failed: {}",
        program.display(),
        String::from_utf8_lossy(&build.stderr)
    );
    let outcome = scratch.join("outcome.jsonl");
    let run = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!("--outcome-file={}", outcome.display()))
        .arg("--")
        .output()
        .expect("run the native executable");
    assert!(
        run.status.code().is_some(),
        "{}: native runtime crashed: {run:?}",
        program.display()
    );
    std::fs::read_to_string(outcome)
        .expect("native outcome file")
        .trim_end()
        .to_owned()
}

/// The record without `"core_node_id"` and `"origin_id"`, which name positions
/// in the lowered program rather than behavior.
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
            let start = if end == digits && text[..start].ends_with(',') {
                start - 1
            } else {
                start
            };
            text.replace_range(start..end, "");
        }
    }
    text
}

#[test]
fn standard_programs_match_their_expected_outcomes_on_both_backends() {
    let mut mismatches = Vec::new();
    for program in programs() {
        let scratch = scratch_directory(&program);
        let interpreted = interpreter_record(&program);
        let native = native_record(&scratch, &program);
        let expected_path = program.with_extension("expected");
        let expected = std::fs::read_to_string(&expected_path)
            .unwrap_or_else(|_| panic!("{} has no expected file", program.display()));
        if interpreted != native {
            mismatches.push(format!(
                "{}: backends differ\n  interpreter: {interpreted}\n  native:      {native}",
                program.display()
            ));
        } else if without_node_ids(&interpreted) != expected.trim_end() {
            mismatches.push(format!(
                "{}: outcome differs from {}\n  got:      {}\n  expected: {}",
                program.display(),
                expected_path.display(),
                without_node_ids(&interpreted),
                expected.trim_end()
            ));
        }
        std::fs::remove_dir_all(scratch).unwrap();
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn standard_tests_pass_on_both_backends() {
    let native = test_outcomes("native");
    let interpreted = test_outcomes("interpreter");
    assert_eq!(native.len(), test_modules().len(), "one test per module: {native:?}");
    let failures: Vec<String> = native
        .iter()
        .chain(&interpreted)
        .filter(|(_, outcome)| outcome != "passed")
        .map(|(identity, outcome)| format!("{identity}: {outcome}"))
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(
        native.iter().map(|(identity, _)| identity).collect::<Vec<_>>(),
        interpreted.iter().map(|(identity, _)| identity).collect::<Vec<_>>(),
        "the backends ran different tests"
    );
}

/// The doc-comment examples in `std/` (`chimi std test`) pass on both backends.
#[test]
fn standard_doc_examples_pass_on_both_backends() {
    let command = [OsStr::new("std"), OsStr::new("test")];
    let native = runner_outcomes("std-examples", &command, "native");
    let interpreted = runner_outcomes("std-examples", &command, "interpreter");
    assert!(native.len() >= 16, "the std/ examples ran: {native:?}");
    let failures: Vec<String> = native
        .iter()
        .chain(&interpreted)
        .filter(|(_, outcome)| outcome != "passed")
        .map(|(identity, outcome)| format!("{identity}: {outcome}"))
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(native, interpreted, "the backends ran different examples");
}

#[test]
fn every_registered_operation_is_reached_by_a_standard_test_or_program() {
    let mut reached = BTreeSet::new();
    for module in test_modules() {
        let sources: Vec<ProjectSource> =
            LoadedProject::single_file(&module, &StandardTree::embedded())
                .expect("load test module")
                .sources();
        let operations = Compiler::standard_operations_reached_by_tests(sources)
            .unwrap_or_else(|diagnostics| panic!("{}: {diagnostics:?}", module.display()));
        reached.extend(operations);
    }
    for program in programs() {
        let sources: Vec<ProjectSource> =
            LoadedProject::single_file(&program, &StandardTree::embedded())
                .expect("load program")
                .sources();
        let operations = Compiler::standard_operations_reached(sources)
            .unwrap_or_else(|diagnostics| panic!("{}: {diagnostics:?}", program.display()));
        reached.extend(operations);
    }
    let unreached = Compiler::standard_operation_ids()
        .into_iter()
        .filter(|id| !reached.contains(*id))
        .collect::<Vec<_>>();
    assert!(
        unreached.is_empty(),
        "no std-tests test or program reaches {unreached:?}"
    );
}

fn diagnostic_codes(sources: Vec<ProjectSource>) -> Vec<String> {
    let report = Compiler::type_project(sources);
    let mut codes = report
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code().to_owned())
        .collect::<Vec<_>>();
    codes.sort();
    codes
}

fn module(path: &str, text: &str) -> ProjectSource {
    ProjectSource::new(
        path,
        chimi::source::SourceSnapshot::from_text(format!("{path}.chimi"), text),
    )
    .unwrap()
}

// Conformance: the former built-in qualifiers are modules a program imports.
#[test]
fn list_text_and_int_are_unresolved_without_an_import() {
    for call in [
        "List.length([1])",
        "Text.append(\"a\", \"b\")",
        "Int.to_text(1)",
    ] {
        let source = format!("pub fn main() {{ {call} }}\n");
        assert_eq!(
            diagnostic_codes(vec![module("main", &source)]),
            ["E_UNRESOLVED_VALUE"],
            "{call}"
        );
    }
}

// Conformance: `std/` belongs to the compiler, so a workspace module named
// `std/list` is rejected instead of shadowing or joining the embedded tree.
#[test]
fn a_workspace_module_under_std_is_rejected() {
    let codes = diagnostic_codes(vec![
        module(
            "main",
            "import std/list as List\npub fn main() -> Int { List.length([1]) }\n",
        ),
        module("std/list", "pub fn length(list: List<Int>) -> Int { 0 }\n"),
    ]);
    assert_eq!(codes, ["N3102"]);
}

// Conformance: only the embedded standard tree declares native items.
#[test]
fn a_native_fn_outside_the_standard_tree_is_rejected() {
    assert_eq!(
        diagnostic_codes(vec![module(
            "app/clock",
            "native fn now() -> Int\npub fn main() -> Int { 0 }\n"
        )]),
        ["N3101"]
    );
    assert_eq!(
        diagnostic_codes(vec![module(
            "app/list",
            "pub native fn length<T>(list: List<T>) -> Int { 0 }\npub fn main() -> Int { 0 }\n"
        )]),
        ["N3101", "S1102"]
    );
}

// Conformance: a `native type` is declared only in the embedded standard tree,
// and has no fields or constructors anywhere.
#[test]
fn a_native_type_outside_the_standard_tree_is_rejected() {
    assert_eq!(
        diagnostic_codes(vec![module(
            "app/handle",
            "pub native type Handle<T>\npub fn main() -> Int { 0 }\n"
        )]),
        ["N3101"]
    );
    assert_eq!(
        diagnostic_codes(vec![module(
            "app/handle",
            "native type Handle { raw: Int }\npub fn main() -> Int { 0 }\n"
        )]),
        ["N3101", "S1102"]
    );
}

// `type` after `native` is read as a keyword only there.
#[test]
fn type_is_an_ordinary_name_elsewhere() {
    assert_eq!(
        diagnostic_codes(vec![module(
            "main",
            "fn native(type: Int) -> Int { type }\npub fn main() -> Int { native(1) }\n"
        )]),
        Vec::<String>::new()
    );
}

// A module may still name a value `native`: the word is read only before `fn`.
#[test]
fn native_is_an_ordinary_name_elsewhere() {
    assert_eq!(
        diagnostic_codes(vec![module(
            "main",
            "fn native(value: Int) -> Int { value }\npub fn main() -> Int { let native = 1\n native }\n"
        )]),
        Vec::<String>::new()
    );
}

// A property that a standard function written in Chimi needs is reported at the
// caller's call, naming the function as the caller wrote it, never at the
// operator inside the standard source.
#[test]
fn a_standard_function_requirement_is_reported_at_the_call() {
    let report = Compiler::type_project(vec![module(
        "main",
        "import std/list as List\npub fn main() -> List<{ a: Int }> { List.sort([{ a: 1 }]) }\n",
    )]);
    let diagnostics = report.diagnostics();
    assert_eq!(diagnostics.len(), 1, "{}", report.render_diagnostics());
    assert_eq!(diagnostics[0].code(), "T4301");
    assert!(
        diagnostics[0]
            .message()
            .starts_with("`List.sort` needs `T` to be ordered"),
        "{}",
        diagnostics[0].message()
    );
    assert!(diagnostics[0].labels().is_empty());
}

// A program names only the modules it uses; the standard modules those import
// (std/text needs std/list, which needs std/order) are loaded with them.
#[test]
fn transitive_standard_imports_are_loaded() {
    for source in [
        "import std/text as Text\npub fn main() -> List<Text> { Text.lines(\"a\\nb\") }\n",
        "import std/list as List\npub fn main() -> List<Int> { List.sort([2, 1]) }\n",
    ] {
        assert_eq!(
            diagnostic_codes(vec![module("main", source)]),
            Vec::<String>::new(),
            "{source}"
        );
    }
}

// Conformance: `Map` keys and `Set` items must be equatable, and enumerating
// either needs them ordered, because enumeration shows the ascending order
// (standard library conventions, open question 2). Building and querying a map
// keyed by a record, which is equatable but not ordered, is accepted; listing
// its keys is not. Each lack is reported at the caller's call.
#[test]
fn map_and_set_state_their_key_requirements_at_the_call() {
    let ordered = |function: &str, generic: &str| {
        Some((
            "T4301",
            format!("`{function}` needs `{generic}` to be ordered"),
        ))
    };
    let equatable = |function: &str, generic: &str| {
        Some((
            "T4300",
            format!("`{function}` needs `{generic}` to be equatable"),
        ))
    };
    let cases = [
        (
            "Map.keys(Map.put(Map.empty(), { a: 1 }, 2))",
            ordered("Map.keys", "K"),
        ),
        (
            "Map.reduce(Map.put(Map.empty(), { a: 1 }, 2), 0, fn(total, _key, value) { total + value })",
            ordered("Map.reduce", "K"),
        ),
        (
            "Map.size(Map.merge(Map.put(Map.empty(), { a: 1 }, 2), Map.from([({ a: 2 }, 3)])))",
            None,
        ),
        (
            "Map.has_key(Map.remove(Map.put(Map.empty(), { a: 1 }, 2), { a: 1 }), { a: 1 })",
            None,
        ),
        ("Map.from([(fn() { 1 }, 2)])", equatable("Map.from", "K")),
        (
            "Map.values(Map.put(Map.empty(), { a: 1 }, 2))",
            ordered("Map.values", "K"),
        ),
        (
            "Map.entries(Map.put(Map.empty(), { a: 1 }, 2))",
            ordered("Map.entries", "K"),
        ),
        (
            "Map.map_values(Map.put(Map.empty(), { a: 1 }, 2), fn(value) { value + 1 })",
            ordered("Map.map_values", "K"),
        ),
        (
            "Map.filter(Map.put(Map.empty(), { a: 1 }, 2), fn(_key, value) { value > 1 })",
            ordered("Map.filter", "K"),
        ),
        (
            "Map.each(Map.put(Map.empty(), { a: 1 }, 2), fn(_key, _value) { Void })",
            ordered("Map.each", "K"),
        ),
        (
            "Set.to_list(Set.add(Set.empty(), { a: 1 }))",
            ordered("Set.to_list", "T"),
        ),
        (
            "Set.filter(Set.add(Set.empty(), { a: 1 }), fn(_item) { True })",
            ordered("Set.filter", "T"),
        ),
        (
            "Set.reduce(Set.add(Set.empty(), { a: 1 }), 0, fn(total, _item) { total + 1 })",
            ordered("Set.reduce", "T"),
        ),
        (
            "Set.each(Set.add(Set.empty(), { a: 1 }), fn(_item) { Void })",
            ordered("Set.each", "T"),
        ),
        ("Set.contains(Set.from([{ a: 1 }]), { a: 1 })", None),
        ("Set.from([fn() { 1 }])", equatable("Set.from", "T")),
    ];
    for (call, expected) in cases {
        let source =
            format!("import std/map as Map\nimport std/set as Set\npub fn main() {{ {call} }}\n");
        let report = Compiler::type_project(vec![module("main", &source)]);
        let found = report
            .diagnostics()
            .iter()
            .map(|diagnostic| {
                (
                    diagnostic.code().to_owned(),
                    diagnostic.message().to_owned(),
                )
            })
            .collect::<Vec<_>>();
        match expected {
            None => assert!(found.is_empty(), "{call}: {found:?}"),
            Some((code, message)) => assert!(
                found.len() == 1 && found[0].0 == code && found[0].1.starts_with(&message),
                "{call}: {found:?}"
            ),
        }
    }
}
