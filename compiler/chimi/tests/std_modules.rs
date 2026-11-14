//! The standard library's expected-output programs (`compiler/chimi/std-tests`).
//!
//! Each `<module>/<name>.chimi` is a program with `pub fn main()`; its sibling
//! `<name>.expected` holds the runner's terminal outcome record. Every program
//! runs on the reference interpreter and as a native executable: the two
//! records must be identical, and each must equal the expected file once the
//! Core node and origin ids, which move whenever std source changes, are
//! removed. Every registered native operation must be reached by some program,
//! so each primitive has differential coverage.

use chimi::project::LoadedProject;
use chimi::{Compiler, ProjectSource};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn programs() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("std-tests");
    let mut programs = Vec::new();
    for module in std::fs::read_dir(&root).expect("read std-tests") {
        let module = module.unwrap().path();
        for entry in std::fs::read_dir(&module).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|extension| extension == "chimi") {
                programs.push(path);
            }
        }
    }
    programs.sort();
    assert!(!programs.is_empty(), "std-tests holds no programs");
    programs
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
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .arg("run")
        .arg(program)
        .output()
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
fn every_registered_operation_is_reached_by_a_standard_program() {
    let mut reached = BTreeSet::new();
    for program in programs() {
        let sources: Vec<ProjectSource> = LoadedProject::single_file(&program)
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
        "no std-tests program reaches {unreached:?}"
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
    for call in ["List.length([1])", "Text.append(\"a\", \"b\")", "Int.to_text(1)"] {
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
        module("main", "import std/list as List\npub fn main() -> Int { List.length([1]) }\n"),
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
        diagnostics[0].message().starts_with("`List.sort` needs `T` to be ordered"),
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
