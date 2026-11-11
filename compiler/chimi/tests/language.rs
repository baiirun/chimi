//! Language-feature conformance through both execution paths.
//!
//! Every program here runs on the reference interpreter (`chimi run`) and as a
//! native AOT executable (`chimi build`). Both outcomes must be identical and
//! must match the expected outcome. Each program also round-trips through the
//! formatter and survives inspection at every byte offset.

use chimi::source::SourceSnapshot;
use chimi::{Compiler, ProjectSource};
use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch_directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "chimi-language-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    if path.exists() {
        std::fs::remove_dir_all(&path).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn interpreter_outcome(source: &Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .arg("run")
        .arg(source)
        .output()
        .expect("run the reference interpreter");
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .last()
        .unwrap_or_else(|| {
            panic!(
                "no interpreter outcome: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        })
        .to_owned()
}

fn native_outcome(scratch: &Path, source: &Path) -> String {
    let executable = scratch.join("program");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(scratch)
        .arg("build")
        .arg(source)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("build the native executable");
    assert!(
        build.status.success(),
        "native build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let outcome_path = scratch.join("outcome.jsonl");
    Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!(
            "--telemetry-file={}",
            scratch.join("telemetry.jsonl").display()
        ))
        .arg(format!("--outcome-file={}", outcome_path.display()))
        .arg("--execution-id=language")
        .arg("--")
        .output()
        .expect("execute the native program");
    std::fs::read_to_string(outcome_path)
        .expect("native outcome file")
        .trim_end()
        .to_owned()
}

fn assert_formatter_round_trips(source: &str) {
    let first = Compiler::format(SourceSnapshot::from_text("main.chimi", source));
    assert!(!first.is_blocked(), "{}", first.render_diagnostics());
    let second = Compiler::format(SourceSnapshot::from_text("main.chimi", first.text()));
    assert_eq!(second.text(), first.text(), "formatting is not idempotent");
}

fn assert_inspection_survives_every_offset(source: &str) {
    for offset in 0..=source.len() {
        if !source.is_char_boundary(offset) {
            continue;
        }
        let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
            .expect("valid module path");
        let _ = Compiler::inspect_project(vec![module], "main", offset);
    }
}

/// Runs `source` on both execution paths and returns their shared outcome value.
///
/// The returned text is the outcome line without its schema prefix, such as
/// `"outcome":"success","value":42`.
fn run_both(name: &str, source: &str) -> String {
    let scratch = scratch_directory(name);
    let path = scratch.join("main.chimi");
    std::fs::write(&path, source).unwrap();
    let interpreted = interpreter_outcome(&path);
    let native = native_outcome(&scratch, &path);
    assert_eq!(interpreted, native, "execution paths disagree for {name}");
    assert_formatter_round_trips(source);
    assert_inspection_survives_every_offset(source);
    std::fs::remove_dir_all(scratch).unwrap();
    interpreted
        .strip_prefix("{\"schema\":\"chimi.runtime.v1\",")
        .and_then(|rest| rest.strip_suffix('}'))
        .unwrap_or_else(|| panic!("unexpected outcome line: {interpreted}"))
        .to_owned()
}

fn check_codes(source: &str) -> Vec<String> {
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    Compiler::type_project(vec![module])
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code().to_owned())
        .collect()
}

#[test]
fn pipeline_inserts_the_subject_as_the_first_argument() {
    let outcome = run_both(
        "pipe-first-argument",
        concat!(
            "fn subtract(left: Int, right: Int) -> Int { left - right }\n",
            "fn scale(value: Int, factor: Int, offset: Int) -> Int { value * factor + offset }\n",
            "fn increment(value: Int) -> Int { value + 1 }\n",
            "pub fn main() -> Int {\n",
            "  10\n",
            "  |> subtract(3)\n",
            "  |> scale(10, 2)\n",
            "  |> increment\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":73");
}

#[test]
fn pipeline_applies_a_non_call_right_side_as_a_unary_function() {
    let outcome = run_both(
        "pipe-unary-forms",
        concat!(
            "fn adder(amount: Int) -> fn(Int) -> Int { fn(value: Int) { value + amount } }\n",
            "pub fn main() -> Int {\n",
            "  let add_two = adder(2)\n",
            "  1 |> add_two |> (adder(5)) |> (fn(value: Int) { value * 3 })\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":24");
}

#[test]
fn pipeline_into_a_call_that_returns_a_function_does_not_apply_the_result() {
    // `1 |> adder(2)` is `adder(1, 2)`, which has too many arguments; the
    // pipeline never falls back to calling the returned function.
    let codes = check_codes(concat!(
        "fn adder(amount: Int) -> fn(Int) -> Int { fn(value: Int) { value + amount } }\n",
        "pub fn main() -> Int { 1 |> adder(2) }\n",
    ));
    assert_eq!(
        codes.first().map(String::as_str),
        Some("T4202"),
        "{codes:?}"
    );
}

// B1 (and the operators' truth tables): every combination of operands on both paths.
#[test]
fn boolean_operators_follow_their_truth_tables() {
    let outcome = run_both(
        "boolean-truth-tables",
        concat!(
            "fn bit(value: Bool, weight: Int) -> Int {\n",
            "  case value {\n",
            "    True -> weight\n",
            "    False -> 0\n",
            "  }\n",
            "}\n",
            "fn is_success(status: Int) -> Bool {\n",
            "  status >= 200 and status < 300\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  bit(True and True, 1) + bit(True and False, 2) + bit(False and True, 4)\n",
            "  + bit(False or False, 8) + bit(False or True, 16) + bit(True or False, 32)\n",
            "  + bit(not False, 64) + bit(not True, 128)\n",
            "  + bit(is_success(250), 256) + bit(is_success(300), 512)\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":369");
}

// B2: `not` binds more loosely than comparison; `(not 3) < 2` would not type-check.
#[test]
fn not_applies_to_a_whole_comparison() {
    let outcome = run_both(
        "boolean-not-precedence",
        "pub fn main() -> Bool { not 3 < 2 }\n",
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":true");
}

// B3: `and` binds more tightly than `or`; `(True or True) and False` would be False.
#[test]
fn and_binds_more_tightly_than_or() {
    let outcome = run_both(
        "boolean-and-or-precedence",
        "pub fn main() -> Bool { True or True and False }\n",
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":true");
}

// B4: a pipeline result combines directly; `xs |> (is_empty or True)` would not type-check.
#[test]
fn boolean_operators_bind_more_loosely_than_pipelines() {
    let outcome = run_both(
        "boolean-pipeline-precedence",
        concat!(
            "fn is_empty(values: List<Int>) -> Bool {\n",
            "  List.length(values) < 1\n",
            "}\n",
            "pub fn main() -> Bool {\n",
            "  let draining = False\n",
            "  [1, 2] |> is_empty or not draining\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":true");
}

// B5 (defect form): the skipped operand would defect if it ran.
#[test]
fn skipped_operands_do_not_run() {
    let outcome = run_both(
        "boolean-short-circuit-defect",
        concat!(
            "fn explode() -> Bool { 1 / 0 > 0 }\n",
            "pub fn main() -> Bool {\n",
            "  let zero = 0\n",
            "  (False and explode()) or not (True or 1 / zero > 0)\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":false");
}

const FAILING_OPERAND: &str = concat!(
    "enum Problem {\n",
    "  Broken(code: Int)\n",
    "}\n",
    "fn fail_now() -> Bool { Failure(Broken(7)) }\n",
);

// B6: the right operand is skipped at run time.
#[test]
fn or_skips_a_failing_right_operand() {
    let source = format!(
        "{FAILING_OPERAND}{}",
        "pub fn main() -> Bool { True or fail_now() }\n"
    );
    let outcome = run_both("boolean-or-skips-failure", &source);
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":true");
}

// B6: the static failure row still contains the skipped operand's failure.
#[test]
fn skipped_operand_failures_stay_in_the_failure_row() {
    let callable = |parameter: &str| {
        format!(
            "{FAILING_OPERAND}{}{parameter}{}",
            "fn short() -> Bool { True or fail_now() }\nfn call(operation: ",
            ") -> Bool { operation() }\npub fn main() -> Bool { call(short) }\n",
        )
    };
    assert_eq!(check_codes(&callable("fn() -> Bool")), ["T4203"]);
    assert!(check_codes(&callable("fn() -> Bool errors Problem")).is_empty());
}

// B7: an evaluated failing operand fails the whole expression.
#[test]
fn and_propagates_a_failing_right_operand() {
    let source = format!(
        "{FAILING_OPERAND}{}",
        "pub fn main() -> Bool { True and fail_now() }\n"
    );
    let outcome = run_both("boolean-and-propagates-failure", &source);
    assert_eq!(
        outcome,
        concat!(
            "\"outcome\":\"expected_failure\",",
            "\"error\":{\"type\":\"Problem\",\"variant\":\"Broken\",\"fields\":{\"code\":7}}",
        )
    );
}

// B9: the needs row is the union of both operands, even when the right one is skipped.
#[test]
fn skipped_operand_needs_stay_in_the_needs_row() {
    let callable = |parameter: &str| {
        format!(
            concat!(
                "interface Payment {{\n",
                "  approve: fn(Int) -> Bool\n",
                "}}\n",
                "fn call(operation: {}) -> Bool {{ operation() }}\n",
                "fn checkout(ready: Bool) -> Bool needs {{ payment: Payment }} {{\n",
                "  call(fn() {{ ready and payment.approve(5) }})\n",
                "}}\n",
                "let approving = {{ approve: fn(_amount) {{ True }} }}\n",
                "pub fn main() -> Bool {{ checkout(False) using {{ payment: approving }} }}\n",
            ),
            parameter
        )
    };
    assert_eq!(check_codes(&callable("fn() -> Bool")), ["T4203"]);
    assert!(check_codes(&callable("fn() -> Bool needs { payment: Payment }")).is_empty());
}

// B10
#[test]
fn boolean_operands_must_be_bool() {
    assert_eq!(
        check_codes("pub fn main() -> Bool { 1 and True }\n"),
        ["T4304"]
    );
    assert_eq!(
        check_codes("pub fn main() -> Bool { False or \"yes\" }\n"),
        ["T4304"]
    );
    assert_eq!(check_codes("pub fn main() -> Bool { not 0 }\n"), ["T4304"]);
}

// B11: the symbolic spellings are rejected, and applying the offered edits fixes the program.
#[test]
fn symbolic_boolean_operators_are_rejected_with_keyword_edits() {
    let source = concat!(
        "pub fn main() -> Bool {\n",
        "  let a = True\n",
        "  let b = a&&!a || a\n",
        "  let c = a and!a\n",
        "  case b or c {\n",
        "    v if!v -> v\n",
        "    v -> v\n",
        "  }\n",
        "}\n",
    );
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    let report = Compiler::type_project(vec![module]);
    let diagnostics = report.diagnostics();
    let codes = diagnostics.iter().map(|d| d.code()).collect::<Vec<_>>();
    assert_eq!(codes, ["S1004", "S1004", "S1004", "S1004", "S1004"]);

    let mut fixed = source.to_owned();
    let mut edits = diagnostics
        .iter()
        .flat_map(|diagnostic| diagnostic.fixes())
        .collect::<Vec<_>>();
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.range().start()));
    for edit in edits {
        let range = edit.range().start() as usize..edit.range().end() as usize;
        fixed.replace_range(range, edit.replacement());
    }
    assert_eq!(
        fixed,
        concat!(
            "pub fn main() -> Bool {\n",
            "  let a = True\n",
            "  let b = a and not a or a\n",
            "  let c = a and not a\n",
            "  case b or c {\n",
            "    v if not v -> v\n",
            "    v -> v\n",
            "  }\n",
            "}\n",
        )
    );
    assert!(check_codes(&fixed).is_empty());
}

// B11: `!=` is not the symbolic `not`, so it never reports S1004.
#[test]
fn bang_before_equal_is_not_reported_as_symbolic_not() {
    let codes = check_codes("pub fn main() -> Bool { 1 != 2 }\n");
    assert!(!codes.iter().any(|code| code == "S1004"), "{codes:?}");
}

// B12: a guard operand that calls a provider makes the guard impure.
#[test]
fn guard_operands_must_be_pure() {
    let codes = check_codes(concat!(
        "interface Validator {\n",
        "  valid: fn(Int) -> Bool\n",
        "}\n",
        "fn classify(value: Int) -> Int needs { validator: Validator } {\n",
        "  case value {\n",
        "    x if x > 0 and validator.valid(x) -> 1\n",
        "    _ -> 0\n",
        "  }\n",
        "}\n",
        "pub fn main() -> Int { 0 }\n",
    ));
    assert_eq!(codes, ["T4234"]);
}

// B13: a transfer in the right operand is a possible transfer, so the loop rule applies.
#[test]
fn loop_rule_applies_to_a_transfer_in_a_skippable_operand() {
    let codes = check_codes(concat!(
        "import std/channel as Channel\n",
        "import std/channel.{Receiver, Sender}\n",
        "fn sent(outcome: Void) -> Bool { True }\n",
        "fn hand_off(outbox: Sender<Receiver<Int>>, captured: Receiver<Int>) -> Int {\n",
        "  loop count = 0 {\n",
        "    let finished = count > 2 or sent(Channel.send(outbox, captured))\n",
        "    case finished {\n",
        "      True -> break count\n",
        "      False -> count + 1\n",
        "    }\n",
        "  }\n",
        "}\n",
        "pub fn main() -> Int { 1 }\n",
    ));
    assert_eq!(codes, ["T4250"]);
}

// Formatter: canonical keyword spacing, kept parentheses, and line breaks before operators.
#[test]
fn boolean_operators_format_canonically() {
    let source = concat!(
        "fn f(a: Bool, b: Bool) -> Bool {\n",
        "  not(a)or b  and not   not b\n",
        "  or (a and b)\n",
        "}\n",
    );
    let formatted = Compiler::format(SourceSnapshot::from_text("main.chimi", source));
    assert!(
        !formatted.is_blocked(),
        "{}",
        formatted.render_diagnostics()
    );
    assert_eq!(
        formatted.text(),
        concat!(
            "fn f(a: Bool, b: Bool) -> Bool {\n",
            "  not (a) or b and not not b\n",
            "  or (a and b)\n",
            "}\n",
        )
    );
}

// ts-8cf7db: a body whose failure row carries a type nothing pins used to
// grow its contract on every inference pass until the checker panicked.
#[test]
fn unpinned_channel_element_in_a_failure_row_reports_diagnostics() {
    let codes = check_codes(concat!(
        "import std/channel as Channel\n",
        "pub fn main() -> Int {\n",
        "  let jobs = Channel.bounded(1)\n",
        "  let outbox = Channel.bounded(1)\n",
        "  let finished = case Channel.send(outbox.sender, jobs.receiver) {\n",
        "    _ -> True\n",
        "  }\n",
        "  1\n",
        "}\n",
    ));
    assert_eq!(codes, ["T4222"]);
}

#[test]
fn unpinned_channel_element_in_a_propagated_failure_is_an_incomplete_type() {
    let scratch = scratch_directory("unpinned-propagated-failure");
    let path = scratch.join("main.chimi");
    std::fs::write(
        &path,
        concat!(
            "import std/channel as Channel\n",
            "pub fn main() -> Int {\n",
            "  let jobs = Channel.bounded(1)\n",
            "  let outbox = Channel.bounded(1)\n",
            "  Channel.send(outbox.sender, jobs.receiver)\n",
            "  1\n",
            "}\n",
        ),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .arg("check")
        .arg(&path)
        .output()
        .expect("run chimi check");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("error[E5006]"), "{stderr}");
    std::fs::remove_dir_all(scratch).unwrap();
}

// ts-270496: an expression is not a module item.
#[test]
fn an_expression_at_file_scope_is_a_syntax_error() {
    let codes = check_codes(concat!(
        "undefined_thing(1, 2)\n",
        "pub fn main() -> Int { 1 }\n",
    ));
    assert_eq!(codes, ["S1001"]);
}

// ts-5b02a8: a line break after a field's `errors` row separates fields.
#[test]
fn line_breaks_separate_interface_fields_with_failure_rows() {
    let outcome = run_both(
        "interface-failure-row-fields",
        concat!(
            "enum FileFault {\n",
            "  Missing(path: Text)\n",
            "}\n",
            "interface Files {\n",
            "  read: fn(Text) -> Int errors FileFault\n",
            "  write: fn(Text, Int) -> Int errors FileFault\n",
            "  size: fn(Text) -> Int\n",
            "    errors FileFault\n",
            "}\n",
            "fn copy(from: Text, to: Text) -> Int needs { files: Files } {\n",
            "  files.write(to, files.read(from) + files.size(from))\n",
            "}\n",
            "let fake = {\n",
            "  read: fn(_path) { 40 },\n",
            "  write: fn(_path, value) { value },\n",
            "  size: fn(_path) { 2 },\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  copy(\"a\", \"b\") using { files: fake }\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":42");
}

// ts-cdc536: standard failure enums denote their variants in failure rows.
#[test]
fn a_fake_of_a_standard_capability_states_its_standard_failure_row() {
    let outcome = run_both(
        "standard-failure-row",
        concat!(
            "import std/io.{FileError}\n",
            "interface Files {\n",
            "  read: fn(Text) -> Int errors FileError\n",
            "}\n",
            "fn load(path: Text) -> Int needs { files: Files } {\n",
            "  files.read(path)\n",
            "}\n",
            "let missing = {\n",
            "  read: fn(path) { Failure(FileError.Read(path, \"missing\")) },\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  load(\"a.txt\") using { files: missing }\n",
            "}\n",
        ),
    );
    assert_eq!(
        outcome,
        concat!(
            "\"outcome\":\"expected_failure\",",
            "\"error\":{\"type\":\"FileError\",\"variant\":\"Read\",",
            "\"fields\":{\"path\":\"a.txt\",\"message\":\"missing\"}}",
        )
    );
}

#[test]
fn a_standard_failure_row_admits_only_its_own_variants() {
    let program = |row: &str, failure: &str| {
        format!(
            concat!(
                "import std/io.{{ConsoleError, FileError, Utf8Error}}\n",
                "fn call(operation: fn() -> Int errors {}) -> Int {{ operation() }}\n",
                "fn broken() -> Int {{ Failure({}) }}\n",
                "pub fn main() -> Int {{ call(broken) }}\n",
            ),
            row, failure
        )
    };
    // Both enums have one variant, so a body failing with it has exactly the row.
    let write = "ConsoleError.WriteFailed(\"out\", \"gone\")";
    let invalid = "Utf8Error.InvalidUtf8(\"a\", 1)";
    assert!(check_codes(&program("ConsoleError", write)).is_empty());
    assert!(check_codes(&program("Utf8Error", invalid)).is_empty());
    assert_eq!(check_codes(&program("ConsoleError", invalid)), ["T4203"]);
}

// A grouping parenthesis keeps the space after an operator or keyword; only a
// call, index, or parameter list attaches to the token before it.
#[test]
fn formatting_keeps_the_space_before_a_grouping_parenthesis() {
    let source = concat!(
        "fn double(value: Int) -> Int { value * 2 }\n",
        "pub fn main() -> Int {\n",
        "  let x = (1 + 2)\n",
        "  case (x > (2)) {\n",
        "    True -> double (x) * (x)\n",
        "    False -> 0\n",
        "  }\n",
        "}\n",
    );
    let formatted = Compiler::format(SourceSnapshot::from_text("main.chimi", source));
    assert!(
        !formatted.is_blocked(),
        "{}",
        formatted.render_diagnostics()
    );
    let text = formatted.text();
    assert!(text.contains("let x = (1 + 2)\n"), "{text}");
    assert!(text.contains("case (x > (2)) {\n"), "{text}");
    assert!(text.contains("True -> double(x) * (x)\n"), "{text}");
    let original = run_both("grouping-before-format", source);
    let reformatted = run_both("grouping-after-format", text);
    assert_eq!(original, "\"outcome\":\"success\",\"value\":18");
    assert_eq!(reformatted, original);
}
