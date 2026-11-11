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
    // `chimi run` reports every outcome, defects included, on stdout and exits
    // 0; anything else means the compiler or interpreter itself failed.
    assert_eq!(
        output.status.code(),
        Some(0),
        "interpreter did not complete: {}",
        String::from_utf8_lossy(&output.stderr)
    );
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
    let execution = Command::new(&executable)
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
    let outcome = std::fs::read_to_string(outcome_path)
        .expect("native outcome file")
        .trim_end()
        .to_owned();
    // The runner exits 0 on success, 1 on an expected failure, and 70 on a
    // defect. A signal or any other code means the runtime itself crashed.
    let expected_code = if outcome.contains("\"outcome\":\"success\"") {
        0
    } else if outcome.contains("\"outcome\":\"expected_failure\"") {
        1
    } else {
        70
    };
    assert_eq!(
        execution.status.code(),
        Some(expected_code),
        "native runner exit does not match {outcome}: {}",
        String::from_utf8_lossy(&execution.stderr)
    );
    outcome
}

fn assert_formatter_round_trips(source: &str) {
    let first = Compiler::format(SourceSnapshot::from_text("main.chimi", source));
    assert!(!first.is_blocked(), "{}", first.render_diagnostics());
    let second = Compiler::format(SourceSnapshot::from_text("main.chimi", first.text()));
    assert_eq!(second.text(), first.text(), "formatting is not idempotent");
}

/// The formatted program must mean the same thing: it runs to the same outcome
/// on the interpreter. The native path is not rebuilt for the formatted text.
fn assert_formatting_preserves_the_outcome(path: &Path, source: &str, expected: &str) {
    let formatted = Compiler::format(SourceSnapshot::from_text("main.chimi", source));
    assert!(
        !formatted.is_blocked(),
        "{}",
        formatted.render_diagnostics()
    );
    if formatted.text() == source {
        return;
    }
    std::fs::write(path, formatted.text()).unwrap();
    assert_eq!(
        interpreter_outcome(path),
        expected,
        "formatting changed the outcome; formatted source:\n{}",
        formatted.text()
    );
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
    assert_formatting_preserves_the_outcome(&path, source, &interpreted);
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

// B1: the rows the truth-table test above leaves out. An exclusive `or`
// passes that test but fails `True or True`; an `and` that is really Bool
// equality fails `False and False`. `True or False and False` is False if
// the operators group left to right.
#[test]
fn boolean_operators_cover_the_remaining_truth_table_rows() {
    let outcome = run_both(
        "boolean-remaining-rows",
        concat!(
            "pub fn main() -> List<Bool> {\n",
            "  [True or True, False and False, True or False and False]\n",
            "}\n",
        ),
    );
    assert_eq!(
        outcome,
        "\"outcome\":\"success\",\"value\":[true,false,true]"
    );
}

// B2: `not` binds more tightly than `and`; `not (False and False)` would be True.
#[test]
fn not_binds_more_tightly_than_and() {
    let outcome = run_both(
        "boolean-not-and-precedence",
        "pub fn main() -> Bool { not False and False }\n",
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":false");
}

// Operator precedence table, levels 2-4 and 7: each value differs under the
// wrong grouping: `(1 + 2) * 3`, `10 - (3 - 2)`, `-(2 + 3)`, `20 / (2 / 5)`,
// `2 * (3 + 4) * 2`, or `1 + (3 |> double)`. The source is deliberately not
// in canonical form, so run_both also runs the formatter's output.
#[test]
fn arithmetic_follows_the_precedence_table() {
    let outcome = run_both(
        "arithmetic-precedence",
        concat!(
            "fn double(value: Int) -> Int { value * 2 }\n",
            "pub fn main() -> List<Int> {\n",
            "  [1+2*3, 10 - 3-2, -2*3, -2+3, 20 / 2 / 5, 2*3+4*2,\n",
            "   1+3\n",
            "   |> double]\n",
            "}\n",
        ),
    );
    assert_eq!(
        outcome,
        "\"outcome\":\"success\",\"value\":[7,5,-6,1,2,14,8]"
    );
}

// Precedence table, levels 4, 6, 7, 8, and 10: comparison takes whole sums,
// `not` takes a whole pipeline, and `or`/`and` take whole pipelines. The
// wrong groupings `1 + (1 < 3)`, `(not 0) |> positive`, and
// `0 |> (positive or True)` do not type-check.
#[test]
fn comparisons_and_pipelines_bind_inside_boolean_operators() {
    let outcome = run_both(
        "boolean-comparison-pipeline-precedence",
        concat!(
            "fn positive(value: Int) -> Bool { value > 0 }\n",
            "pub fn main() -> List<Bool> {\n",
            "  [1 + 1 < 3, 2 * 3 > 5 and 1 > 2, not 0 |> positive, 0 |> positive or True]\n",
            "}\n",
        ),
    );
    assert_eq!(
        outcome,
        "\"outcome\":\"success\",\"value\":[true,false,true,true]"
    );
}

// `and`, `or`, and `not` are reserved words, so they cannot name a binding.
#[test]
fn boolean_keywords_cannot_name_bindings() {
    for keyword in ["and", "or", "not"] {
        let codes = check_codes(&format!(
            "pub fn main() -> Int {{\n  let {keyword} = 1\n  1\n}}\n"
        ));
        assert_eq!(
            codes.first().map(String::as_str),
            Some("S1001"),
            "`let {keyword} = 1`: {codes:?}"
        );
    }
}

// Native code evaluates a pure integer expression directly and leaves any
// expression with division to the runtime. Both forms must agree with the
// interpreter, including the defects.
#[test]
fn pure_integer_arithmetic_matches_the_interpreter() {
    let outcome = run_both(
        "integer-direct",
        "pub fn main() -> Int { -(1 + 2) * 3 - 4 }\n",
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":-13");
    let outcome = run_both(
        "integer-division",
        "pub fn main() -> Int { (8 / 2) - 7 / 2 - -7 / 2 }\n",
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":4");
}

// A division defect inside an otherwise direct expression is the same on
// both paths. Overflow parity is covered by
// `integer_overflow_and_division_by_zero_defect_with_the_operator_and_operands`.
#[test]
fn integer_division_by_zero_matches_the_interpreter() {
    let outcome = run_both(
        "integer-divide-by-zero",
        "pub fn main() -> Int { 1 + 2 / 0 }\n",
    );
    assert!(
        outcome.starts_with(
            "\"outcome\":\"defect\",\"message\":\"integer division by zero in `/`: 2 / 0\""
        ),
        "{outcome}"
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

const BIG: &str = "9223372036854775807";

// Kernel "Pipelines": `x |> f` evaluates exactly as `f(x)`, so the callee
// runs before the left operand. Both operands defect here; the callee's
// division wins.
#[test]
fn unary_pipeline_evaluates_the_callee_before_the_subject() {
    let outcome = run_both(
        "pipe-unary-callee-first",
        &format!(
            concat!(
                "fn make(offset: Int) -> fn(Int) -> Int {{ fn(value) {{ value + offset }} }}\n",
                "pub fn main() -> Int {{\n",
                "  let big = {}\n",
                "  (big + 1) |> (make(1 / 0))\n",
                "}}\n",
            ),
            BIG
        ),
    );
    assert!(outcome.contains("integer division by zero"), "{outcome}");
}

#[test]
fn call_pipeline_evaluates_the_callee_before_the_subject() {
    let outcome = run_both(
        "pipe-call-callee-first",
        &format!(
            concat!(
                "fn make(offset: Int) -> fn(Int, Int) -> Int {{\n",
                "  fn(value, extra) {{ value + extra + offset }}\n",
                "}}\n",
                "pub fn main() -> Int {{\n",
                "  let big = {}\n",
                "  (big + 1) |> make(1 / 0)(5)\n",
                "}}\n",
            ),
            BIG
        ),
    );
    assert!(outcome.contains("integer division by zero"), "{outcome}");
}

#[test]
fn pipeline_into_a_curried_call_inserts_the_subject_into_the_last_call() {
    // `x |> f(a)(b)` is `f(a)(x, b)`.
    let outcome = run_both(
        "pipe-curried",
        concat!(
            "fn make(offset: Int) -> fn(Int, Int) -> Int {\n",
            "  fn(value, extra) { value * 100 + extra * 10 + offset }\n",
            "}\n",
            "pub fn main() -> Int { 1 |> make(3)(2) }\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":123");
}

#[test]
fn pipeline_into_member_constructor_and_empty_calls() {
    let outcome = run_both(
        "pipe-member-constructor-empty",
        concat!(
            "struct Pair {\n",
            "  left: Int\n",
            "  right: Int\n",
            "}\n",
            "fn length(text: Text) -> Int {\n",
            "  case text {\n",
            "    \"xy\" -> 2\n",
            "    _ -> 0\n",
            "  }\n",
            "}\n",
            "fn tenfold(value: Int) -> Int { value * 10 }\n",
            "pub fn main() -> Int {\n",
            "  let pair = 3 |> Pair(4)\n",
            "  let joined = \"x\" |> Text.append(\"y\")\n",
            "  (joined |> length()) + pair.left * 100 + (pair.right |> tenfold())\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":342");
}

#[test]
fn pipeline_stages_run_under_an_enclosing_using() {
    let outcome = run_both(
        "pipe-under-using",
        concat!(
            "interface Scale {\n",
            "  apply: fn(Int) -> Int\n",
            "}\n",
            "fn scaled(value: Int) -> Int needs { scale: Scale } { scale.apply(value) }\n",
            "fn add(value: Int, extra: Int) -> Int { value + extra }\n",
            "let triple = { apply: fn(value) { value * 3 } }\n",
            "pub fn main() -> Int {\n",
            "  2 |> scaled |> add(1) using { scale: triple }\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":7");
}

fn type_diagnostics(source: &str) -> Vec<(String, String)> {
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    Compiler::type_project(vec![module])
        .diagnostics()
        .iter()
        .map(|diagnostic| {
            let range = diagnostic.range();
            (
                diagnostic.code().to_owned(),
                source[range.start() as usize..range.end() as usize].to_owned(),
            )
        })
        .collect()
}

#[test]
fn a_diagnostic_inside_a_chain_covers_only_its_stage() {
    let source = concat!(
        "fn one(value: Int) -> Int { value }\n",
        "fn two(value: Int, extra: Int) -> Int { value * extra }\n",
        "pub fn main() -> Int {\n",
        "  1\n",
        "  |> one(2)\n",
        "  |> two(3)\n",
        "  |> two(4)\n",
        "}\n",
    );
    assert_eq!(
        type_diagnostics(source),
        [("T4202".to_owned(), "1\n  |> one(2)".to_owned())]
    );
}

#[test]
fn hover_on_a_later_stage_reports_that_stage_type() {
    let source = concat!(
        "struct Label {\n",
        "  text: Text\n",
        "}\n",
        "struct Sized {\n",
        "  label: Label\n",
        "  size: Int\n",
        "}\n",
        "fn label(value: Int, text: Text) -> Label { Label(text) }\n",
        "fn size(label: Label, size: Int) -> Sized { Sized(label, size) }\n",
        "pub fn main() -> Sized { 1 |> label(\"a\") |> size(5) }\n",
    );
    let hover = |needle: &str| {
        let offset = source.find(needle).expect("needle is in the source");
        let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
            .expect("valid module path");
        Compiler::inspect_project(vec![module], "main", offset)
            .expect("inspection succeeds")
            .inferred_type()
            .map(str::to_owned)
    };
    assert_eq!(hover("(5)").as_deref(), Some("Sized"));
    assert_eq!(hover("(\"a\")").as_deref(), Some("Label"));
}

// A constructor named as a value: one with fields is a callable, a fieldless
// variant is the value itself.
#[test]
fn constructors_with_fields_are_callable_values() {
    let outcome = run_both(
        "constructor-callable-value",
        concat!(
            "struct Meters {\n",
            "  value: Int\n",
            "}\n",
            "fn apply(make: fn(Int) -> Meters, value: Int) -> Meters { make(value) }\n",
            "fn total(option: Option<Int>) -> Int {\n",
            "  case option {\n",
            "    Some(value) -> value\n",
            "    None -> 0\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let wrap = Some\n",
            "  total(1 |> Some) + total(wrap(20)) + apply(Meters, 300).value\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":321");
}

#[test]
fn a_fieldless_variant_named_as_a_value_is_that_value() {
    let outcome = run_both(
        "fieldless-variant-value",
        concat!(
            "enum Problem {\n",
            "  Broken\n",
            "  Worn(level: Int)\n",
            "}\n",
            "fn count(option: Option<Int>) -> Int {\n",
            "  case option {\n",
            "    Some(value) -> value\n",
            "    None -> 0\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let nothing = None\n",
            "  let explicit = None()\n",
            "  let total = count(nothing) + count(explicit) + 1\n",
            "  case total {\n",
            "    1 -> Failure(Broken)\n",
            "    _ -> total\n",
            "  }\n",
            "}\n",
        ),
    );
    assert_eq!(
        outcome,
        concat!(
            "\"outcome\":\"expected_failure\",",
            "\"error\":{\"type\":\"Problem\",\"variant\":\"Broken\",\"fields\":{}}",
        )
    );
}

fn warning_codes(source: &str) -> Vec<String> {
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    Compiler::type_project(vec![module])
        .warnings()
        .iter()
        .map(|diagnostic| diagnostic.code().to_owned())
        .collect()
}

fn check_messages(source: &str) -> Vec<String> {
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    Compiler::type_project(vec![module])
        .diagnostics()
        .iter()
        .map(|diagnostic| format!("{} {}", diagnostic.code(), diagnostic.message()))
        .collect()
}

fn defect_message(outcome: &str) -> &str {
    outcome
        .strip_prefix("\"outcome\":\"defect\",\"message\":\"")
        .and_then(|rest| rest.split_once('"'))
        .map(|(message, _)| message)
        .unwrap_or_else(|| panic!("not a defect: {outcome}"))
}

// N1
#[test]
fn integer_division_truncates_and_the_remainder_follows_the_dividend() {
    let outcome = run_both(
        "int-truncation",
        "pub fn main() -> List<Int> { [-7 / 2, -7 % 2, 7 % -2, 7 % 2, -7 % -2] }\n",
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":[-3,-1,1,1,-1]");
}

// N2-N6 and the remaining `Int` edges: operands arrive through calls, so nothing
// folds, and both paths must report the same defect with the operator and operands.
#[test]
fn integer_overflow_and_division_by_zero_defect_with_the_operator_and_operands() {
    let min = "-9223372036854775808";
    let max = "9223372036854775807";
    for (expression, message) in [
        (
            format!("divide({min}, -1)"),
            format!("integer overflow in `/`: {min} / -1"),
        ),
        (
            "divide(5, 0)".to_owned(),
            "integer division by zero in `/`: 5 / 0".to_owned(),
        ),
        (
            "remainder(5, 0)".to_owned(),
            "integer division by zero in `%`: 5 % 0".to_owned(),
        ),
        (
            format!("negate({min})"),
            format!("integer overflow in unary `-`: -({min})"),
        ),
        (
            format!("add({max}, 1)"),
            format!("integer overflow in `+`: {max} + 1"),
        ),
        (
            format!("subtract({min}, 1)"),
            format!("integer overflow in `-`: {min} - 1"),
        ),
        (
            format!("multiply({max}, 2)"),
            format!("integer overflow in `*`: {max} * 2"),
        ),
    ] {
        let outcome = run_both(
            "int-edge-defects",
            &format!(
                concat!(
                    "fn add(a: Int, b: Int) -> Int {{ a + b }}\n",
                    "fn subtract(a: Int, b: Int) -> Int {{ a - b }}\n",
                    "fn multiply(a: Int, b: Int) -> Int {{ a * b }}\n",
                    "fn divide(a: Int, b: Int) -> Int {{ a / b }}\n",
                    "fn remainder(a: Int, b: Int) -> Int {{ a % b }}\n",
                    "fn negate(a: Int) -> Int {{ -a }}\n",
                    "pub fn main() -> Int {{ {} }}\n",
                ),
                expression
            ),
        );
        assert_eq!(defect_message(&outcome), message, "{expression}");
    }
}

// N6: the division overflows, but the remainder is exact.
#[test]
fn int_min_remainder_minus_one_is_zero() {
    let outcome = run_both(
        "int-min-remainder",
        concat!(
            "fn remainder(a: Int, b: Int) -> Int { a % b }\n",
            "pub fn main() -> List<Int> {\n",
            "  let min = -9223372036854775808\n",
            "  [min % -1, remainder(min, -1)]\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":[0,0]");
}

// N2
#[test]
fn constant_overflow_warns_and_still_defects_at_run_time() {
    let source = "pub fn main() -> Int { 9223372036854775807 + 1 }\n";
    assert!(check_codes(source).is_empty());
    assert_eq!(warning_codes(source), ["T4312"]);
    let outcome = run_both("constant-overflow", source);
    assert_eq!(
        defect_message(&outcome),
        "integer overflow in `+`: 9223372036854775807 + 1"
    );
}

#[test]
fn only_constant_operations_that_always_defect_warn() {
    for source in [
        "pub fn main() -> Int { 1 / 0 }\n",
        "pub fn main() -> Int { 1 % (2 - 2) }\n",
        "pub fn main() -> Int { -(-9223372036854775808) }\n",
        "pub fn main() -> Int { (9223372036854775807 - 1) * 2 }\n",
    ] {
        assert_eq!(warning_codes(source), ["T4312"], "{source}");
    }
    for source in [
        "pub fn main() -> Int { -9223372036854775808 % -1 }\n",
        "pub fn main() -> Int { 9223372036854775806 + 1 }\n",
        "fn zero() -> Int { 0 }\npub fn main() -> Int { 1 / zero() }\n",
    ] {
        assert!(warning_codes(source).is_empty(), "{source}");
    }
}

// N7, N8
#[test]
fn integer_literals_are_range_checked_after_folding_unary_minus() {
    assert!(check_codes("pub fn main() -> Int { -9223372036854775808 }\n").is_empty());
    for source in [
        "pub fn main() -> Int { 9223372036854775808 }\n",
        "pub fn main() -> Int { 0xFFFF_FFFF_FFFF_FFFF }\n",
        "pub fn main() -> Int { -(9223372036854775808) }\n",
        "pub fn main() -> Int { case 1 { 99999999999999999999 -> 1\n _ -> 0 } }\n",
    ] {
        assert_eq!(check_codes(source), ["S1002"], "{source}");
    }
}

// N9
#[test]
fn separated_hexadecimal_and_binary_literals_have_their_values() {
    let outcome = run_both(
        "integer-literal-forms",
        concat!(
            "pub fn main() -> List<Int> {\n",
            "  [1_000, 0x1F, 0b101, 0xff_FF, -0x8000_0000_0000_0000]\n",
            "}\n",
        ),
    );
    assert_eq!(
        outcome,
        "\"outcome\":\"success\",\"value\":[1000,31,5,65535,-9223372036854775808]"
    );
}

// N10
#[test]
fn malformed_number_literals_are_syntax_errors() {
    for literal in [
        "1.", ".5", "1__0", "_1", "1_", "0x", "0b102", "0xG", "017", "1.5_",
    ] {
        let codes = check_codes(&format!("pub fn main() -> Int {{ {literal} }}\n"));
        assert!(
            codes.iter().any(|code| code.starts_with('S')),
            "{literal}: {codes:?}"
        );
    }
}

// N12
#[test]
fn a_float_literal_that_rounds_to_infinity_is_rejected() {
    assert_eq!(check_codes("pub fn main() -> Float { 1e309 }\n"), ["S1003"]);
    assert!(check_codes("pub fn main() -> Float { 1e-400 }\n").is_empty());
}

#[test]
fn float_literals_and_arithmetic_render_canonically_on_both_paths() {
    let outcome = run_both(
        "float-arithmetic",
        concat!(
            "pub fn main() -> List<Float> {\n",
            "  [0.1 + 0.2, 1_000.25, 2.5e-3, 6.02E23, 1e16, 1e15, 0.00001, 0.000001,\n",
            "   0.0 - 0.0, -0.0, -7.5 % 2.0, 7.0 / 2.0, -(1.5), 1e300 * 1e-300]\n",
            "}\n",
        ),
    );
    assert_eq!(
        outcome,
        concat!(
            "\"outcome\":\"success\",\"value\":[0.30000000000000004,1000.25,0.0025,6.02e23,",
            "1e16,1000000000000000.0,0.00001,1e-6,0.0,-0.0,-1.5,3.5,-1.5,1.0]"
        )
    );
}

#[test]
fn float_comparisons_are_numeric() {
    let outcome = run_both(
        "float-comparison",
        concat!(
            "pub fn main() -> List<Bool> {\n",
            "  [0.0 <= -0.0, -0.0 < 0.0, 1.5 > 1.25, -1e300 < 5e-324, 2.0 >= 3.0]\n",
            "}\n",
        ),
    );
    assert_eq!(
        outcome,
        "\"outcome\":\"success\",\"value\":[true,false,true,true,false]"
    );
}

// N14, N15
#[test]
fn float_operations_without_a_finite_result_defect() {
    for (expression, message) in [
        (
            "divide(1.0, 0.0)",
            "float division by zero in `/`: 1.0 / 0.0",
        ),
        (
            "divide(0.0, -0.0)",
            "float division by zero in `/`: 0.0 / -0.0",
        ),
        (
            "remainder(1.0, 0.0)",
            "float division by zero in `%`: 1.0 % 0.0",
        ),
        (
            "square(1e200)",
            "float `*` has no finite result: 1e200 * 1e200",
        ),
        (
            "add(1.7976931348623157e308, 1e292)",
            "float `+` has no finite result: 1.7976931348623157e308 + 1e292",
        ),
    ] {
        let outcome = run_both(
            "float-defects",
            &format!(
                concat!(
                    "fn add(a: Float, b: Float) -> Float {{ a + b }}\n",
                    "fn divide(a: Float, b: Float) -> Float {{ a / b }}\n",
                    "fn remainder(a: Float, b: Float) -> Float {{ a % b }}\n",
                    "fn square(x: Float) -> Float {{ x * x }}\n",
                    "pub fn main() -> Float {{ {} }}\n",
                ),
                expression
            ),
        );
        assert_eq!(defect_message(&outcome), message, "{expression}");
    }
}

// N17, E11-adjacent: no mixed arithmetic or comparison.
#[test]
fn mixed_int_and_float_operands_are_rejected_with_a_conversion_hint() {
    for source in [
        "pub fn main() -> Float { 1 + 2.0 }\n",
        "pub fn main() -> Float { 2.0 * 3 }\n",
        "pub fn main() -> Bool { 1 < 1.5 }\n",
        "fn half(x: Float) -> Float { x / 2 }\npub fn main() -> Float { half(1.0) }\n",
    ] {
        let messages = check_messages(source);
        assert_eq!(messages.len(), 1, "{source}: {messages:?}");
        assert!(
            messages[0].starts_with("T4302") && messages[0].contains("Float.from_int"),
            "{source}: {messages:?}"
        );
    }
}

// N18
#[test]
fn arithmetic_on_text_is_rejected_with_a_text_append_hint() {
    let messages = check_messages("pub fn main() -> Text { \"a\" + \"b\" }\n");
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].starts_with("T4303") && messages[0].contains("Text.append"),
        "{messages:?}"
    );
    assert_eq!(
        check_codes("pub fn main() -> Bool { True * False }\n"),
        ["T4303"]
    );
}

// N23
#[test]
fn float_literal_patterns_are_rejected() {
    let source = concat!(
        "fn describe(x: Float) -> Int {\n",
        "  case x {\n",
        "    1.5 -> 1\n",
        "    _ -> 0\n",
        "  }\n",
        "}\n",
        "pub fn main() -> Int { describe(1.5) }\n",
    );
    assert_eq!(check_codes(source), ["T4313"]);
}

fn std_outcome(name: &str, body: &str) -> String {
    run_both(
        name,
        &format!("import std/int as Int\nimport std/float as Float\n\n{body}\n"),
    )
}

fn failure(type_name: &str, variant: &str) -> String {
    format!(
        "\"outcome\":\"expected_failure\",\"error\":{{\"type\":\"{type_name}\",\"variant\":\"{variant}\",\"fields\":{{}}}}"
    )
}

// N19
#[test]
fn int_parse_accepts_decimal_text_and_names_each_failure() {
    assert_eq!(
        std_outcome(
            "int-parse",
            "pub fn main() -> List<Int> { [Int.parse(\"42\"), Int.parse(\"-9223372036854775808\")] }"
        ),
        "\"outcome\":\"success\",\"value\":[42,-9223372036854775808]"
    );
    for (text, variant) in [
        ("", "Empty"),
        ("4x", "InvalidDigit"),
        ("+4", "InvalidDigit"),
        (" 4", "InvalidDigit"),
        ("99999999999999999999", "OutOfRange"),
    ] {
        assert_eq!(
            std_outcome(
                "int-parse-failure",
                &format!("pub fn main() -> Int {{ Int.parse(\"{text}\") }}")
            ),
            failure("ParseError", variant),
            "{text:?}"
        );
    }
}

// N20
#[test]
fn float_parse_rejects_text_outside_the_finite_literal_grammar() {
    assert_eq!(
        std_outcome(
            "float-parse",
            "pub fn main() -> List<Float> { [Float.parse(\"-2.5e-3\"), Float.parse(\"7\")] }"
        ),
        "\"outcome\":\"success\",\"value\":[-0.0025,7.0]"
    );
    for (text, variant) in [
        ("", "Empty"),
        ("nan", "Invalid"),
        ("inf", "Invalid"),
        ("1.", "Invalid"),
        ("+1.0", "Invalid"),
        ("1e400", "OutOfRange"),
    ] {
        assert_eq!(
            std_outcome(
                "float-parse-failure",
                &format!("pub fn main() -> Float {{ Float.parse(\"{text}\") }}")
            ),
            failure("ParseError", variant),
            "{text:?}"
        );
    }
}

// N21
#[test]
fn float_to_int_requires_an_already_rounded_value_inside_int() {
    assert_eq!(
        std_outcome(
            "float-to-int",
            "pub fn main() -> Int { Float.to_int(Float.round(2.5)) }"
        ),
        "\"outcome\":\"success\",\"value\":3"
    );
    for (value, variant) in [("2.5", "NotInteger"), ("1e20", "OutOfRange")] {
        assert_eq!(
            std_outcome(
                "float-to-int-failure",
                &format!("pub fn main() -> Int {{ Float.to_int({value}) }}")
            ),
            failure("ConversionError", variant),
            "{value}"
        );
    }
}

#[test]
fn float_rounding_functions_follow_their_direction() {
    assert_eq!(
        std_outcome(
            "float-rounding",
            concat!(
                "pub fn main() -> List<Float> {\n",
                "  [Float.round(2.5), Float.round(-2.5), Float.round(-0.4), Float.truncate(-2.7),\n",
                "   Float.floor(-2.5), Float.ceiling(-2.5), Float.from_int(9007199254740993)]\n",
                "}",
            )
        ),
        "\"outcome\":\"success\",\"value\":[3.0,-3.0,-0.0,-2.0,-3.0,-2.0,9007199254740992.0]"
    );
}

// N16 (text half)
#[test]
fn float_text_is_the_canonical_shortest_form() {
    assert_eq!(
        std_outcome(
            "float-text",
            concat!(
                "pub fn main() -> List<Text> {\n",
                "  [Float.to_text(-0.0), Float.to_text(0.0 - 0.0), Float.to_text(5e-324),\n",
                "   Float.to_text(1.7976931348623157e308), Float.to_text(100.0), Float.to_text(0.1)]\n",
                "}",
            )
        ),
        concat!(
            "\"outcome\":\"success\",\"value\":[\"-0.0\",\"0.0\",\"5e-324\",",
            "\"1.7976931348623157e308\",\"100.0\",\"0.1\"]"
        )
    );
}

// N22: walks hundreds of values across the exponent range. Each must parse back to
// itself (`same` uses ordering, since `==` arrives later), and both paths must
// render every one identically.
#[test]
fn float_text_round_trips_through_parse_for_sampled_values() {
    let outcome = std_outcome(
        "float-round-trip",
        concat!(
            "fn same(a: Float, b: Float) -> Bool { not (a < b) and not (a > b) }\n",
            "struct Walk { x: Float, steps: Int, rendered: Text, misses: Int }\n",
            "fn walk(start: Float, factor: Float, steps: Int) -> Text {\n",
            "  loop state = Walk(start, steps, \"\", 0) {\n",
            "    case state.steps {\n",
            "      0 -> break Text.append(state.rendered, Int.to_text(state.misses))\n",
            "      _ -> {\n",
            "        let text = Float.to_text(state.x)\n",
            "        let back = Float.parse(text)\n",
            "        let miss = case same(back, state.x) {\n",
            "          True -> 0\n",
            "          False -> 1\n",
            "        }\n",
            "        let rendered = Text.append(Text.append(state.rendered, text), \",\")\n",
            "        Walk(state.x * factor, state.steps - 1, rendered, state.misses + miss)\n",
            "      }\n",
            "    }\n",
            "  }\n",
            "}\n",
            "pub fn main() -> List<Text> {\n",
            "  [walk(1.0, -3.7, 500), walk(1.0, -0.27, 500), walk(1.0, 0.5, 1080),\n",
            "   walk(1.0, 2.0, 1000), walk(1e-300, 1.1, 2000)]\n",
            "}",
        ),
    );
    let walks = outcome
        .strip_prefix("\"outcome\":\"success\",\"value\":[")
        .unwrap_or_else(|| panic!("{outcome}"));
    assert_eq!(walks.matches(",0\"").count(), 5, "{outcome}");
}

#[test]
fn checked_integer_functions_return_none_instead_of_defecting() {
    let outcome = std_outcome(
        "int-checked",
        concat!(
            "pub fn main() -> List<Option<Int>> {\n",
            "  [Int.add_checked(9223372036854775807, 1), Int.subtract_checked(1, 2),\n",
            "   Int.multiply_checked(4611686018427387904, 2), Int.divide_checked(1, 0),\n",
            "   Int.divide_checked(-9223372036854775808, -1),\n",
            "   Int.remainder_checked(-9223372036854775808, -1), Int.remainder_checked(1, 0)]\n",
            "}",
        ),
    );
    let none = "{\"type\":\"Option\",\"variant\":\"None\",\"fields\":{}}";
    let some = |value: i64| {
        format!("{{\"type\":\"Option\",\"variant\":\"Some\",\"fields\":{{\"value\":{value}}}}}")
    };
    assert_eq!(
        outcome,
        format!(
            "\"outcome\":\"success\",\"value\":[{none},{},{none},{none},{none},{},{none}]",
            some(-1),
            some(0)
        )
    );
}

#[test]
fn int_modulo_takes_the_sign_of_the_modulus() {
    let outcome = std_outcome(
        "int-modulo",
        concat!(
            "pub fn main() -> List<Int> {\n",
            "  [Int.modulo(-7, 3), Int.modulo(7, -3), Int.modulo(-6, 3),\n",
            "   Int.modulo(-9223372036854775808, -1), Int.to_text(-17) |> Int.parse]\n",
            "}",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":[2,-2,0,0,-17]");
    let outcome = std_outcome(
        "int-modulo-zero",
        "pub fn main() -> Int { Int.modulo(5, 0) }",
    );
    assert_eq!(
        defect_message(&outcome),
        "integer division by zero in `Int.modulo`: Int.modulo(5, 0)"
    );
}

// N13
#[test]
#[ignore = "needs == (ts-95bffa)"]
fn float_addition_rounds_so_point_one_plus_point_two_is_not_point_three() {
    let outcome = run_both(
        "float-rounding-equality",
        "pub fn main() -> Bool { 0.1 + 0.2 == 0.3 }\n",
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":false");
}

// N16 (equality half)
#[test]
#[ignore = "needs == (ts-95bffa)"]
fn negative_zero_equals_zero() {
    let outcome = run_both(
        "float-negative-zero-equality",
        "pub fn main() -> Bool { 0.0 - 0.0 == -0.0 }\n",
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":true");
}

// E11
#[test]
#[ignore = "needs == (ts-95bffa)"]
fn int_and_float_are_never_equal_operands() {
    assert_eq!(
        check_codes("pub fn main() -> Bool { 1 == 1.0 }\n"),
        ["T4302"]
    );
}

// E20
#[test]
#[ignore = "needs == (ts-95bffa)"]
fn a_map_keyed_by_zero_finds_negative_zero() {
    let outcome = run_both(
        "float-map-key",
        concat!(
            "import std/map as Map\n",
            "pub fn main() -> Option<Int> {\n",
            "  Map.empty() |> Map.put(0.0, 1) |> Map.get(-0.0)\n",
            "}\n",
        ),
    );
    assert_eq!(
        outcome,
        "\"outcome\":\"success\",\"value\":{\"type\":\"Option\",\"variant\":\"Some\",\"fields\":{\"value\":1}}"
    );
}

// ---- List patterns (G15), docs/specification/patterns-and-function-clauses.md ----

fn coded_messages(source: &str) -> Vec<(String, String)> {
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    Compiler::type_project(vec![module])
        .diagnostics()
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.code().to_owned(),
                diagnostic.message().to_owned(),
            )
        })
        .collect()
}

/// A `case` over a `List<Int>` parameter with the given arms, called from `main`.
fn list_case(arms: &[&str]) -> String {
    format!(
        "fn f(values: List<Int>) -> Int {{\n  case values {{\n{}  }}\n}}\npub fn main() -> Int {{ f([1]) }}\n",
        arms.iter()
            .map(|arm| format!("    {arm}\n"))
            .collect::<String>()
    )
}

// LP-01, LP-03, and the coverage examples: these arm sets are exhaustive and
// every arm is reachable.
#[test]
fn list_patterns_cover_every_length_class() {
    for arms in [
        &["[] -> 0", "[first, ..rest] -> first"][..],
        &["[] -> 0", "[_] -> 1", "[_, _, ..] -> 2"],
        &["[] -> 0", "[a] -> a", "[.., last] -> last"],
        &["[] -> 0", "[first, .., last] -> first + last", "[x] -> x"],
    ] {
        assert_eq!(
            check_codes(&list_case(arms)),
            Vec::<String>::new(),
            "{arms:?}"
        );
    }
}

// LP-02 and T4231 witness rendering: the message names a missing length class.
#[test]
fn a_non_exhaustive_list_case_names_a_missing_list() {
    for (arms, witness) in [
        (&["[_, ..] -> 0"][..], "`[]`"),
        (&["[] -> 0", "[x] -> x"], "`[_, _, ..]`"),
        (&["[] -> 0", "[first, .., last] -> first + last"], "`[_]`"),
        (
            &["[a, b] -> a + b", "[] -> 0", "[_, _, _, ..] -> 0"],
            "`[_]`",
        ),
    ] {
        let messages = coded_messages(&list_case(arms));
        assert_eq!(messages.len(), 1, "{arms:?}: {messages:?}");
        assert_eq!(messages[0].0, "T4231");
        assert!(messages[0].1.contains(witness), "{arms:?}: {messages:?}");
    }
}

#[test]
fn a_non_exhaustive_enum_case_names_a_missing_variant() {
    let messages = coded_messages(concat!(
        "fn f(value: Option<List<Int>>) -> Int {\n",
        "  case value {\n",
        "    Some([]) -> 0\n",
        "    None -> 1\n",
        "  }\n",
        "}\n",
        "pub fn main() -> Int { f(Some([1])) }\n",
    ));
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0].0, "T4231");
    assert!(messages[0].1.contains("`Some([_, ..])`"), "{messages:?}");
}

// `[a]` then `[.., last]` keeps both reachable; an arm after `[]` and `[_, ..]` is not.
#[test]
fn a_list_arm_after_full_coverage_is_unreachable() {
    assert_eq!(
        check_codes(&list_case(&["[] -> 0", "[_, ..] -> 1", "[_] -> 2"])),
        ["T4232"]
    );
}

// LP-04
#[test]
fn a_list_pattern_with_two_rests_is_rejected() {
    let codes = check_codes(&list_case(&["[a, ..x, ..y] -> a", "[] -> 0"]));
    assert_eq!(codes, ["L2021"]);
}

#[test]
fn a_list_pattern_against_a_non_list_is_a_type_error() {
    let codes = check_codes(concat!(
        "fn f(value: Int) -> Int {\n",
        "  case value {\n",
        "    [x] -> x\n",
        "    _ -> 0\n",
        "  }\n",
        "}\n",
        "pub fn main() -> Int { f(1) }\n",
    ));
    assert_eq!(codes, ["T4230"]);
}

// LP-05
#[test]
fn an_exact_list_pattern_binds_elements_by_position() {
    let outcome = run_both(
        "list-pattern-exact",
        concat!(
            "fn route(segments: List<Text>) -> List<Text> {\n",
            "  case segments {\n",
            "    [a, b] -> [b, a]\n",
            "    _ -> []\n",
            "  }\n",
            "}\n",
            "pub fn main() -> List<Text> { route([\"orders\", \"7\"]) }\n",
        ),
    );
    assert_eq!(
        outcome,
        "\"outcome\":\"success\",\"value\":[\"7\",\"orders\"]"
    );
}

// LP-06
#[test]
fn the_rest_of_a_one_element_list_is_empty() {
    let outcome = run_both(
        "list-pattern-empty-rest",
        concat!(
            "fn tail(values: List<Int>) -> List<Int> {\n",
            "  case values {\n",
            "    [] -> [-1]\n",
            "    [_, ..rest] -> rest\n",
            "  }\n",
            "}\n",
            "pub fn main() -> List<Int> { tail([5]) }\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":[]");
}

#[test]
fn prefix_rest_and_suffix_bind_their_parts() {
    let outcome = run_both(
        "list-pattern-parts",
        concat!(
            "fn parts(values: List<Int>) -> List<Int> {\n",
            "  case values {\n",
            "    [first, ..inner, last] -> [last, List.length(inner), first]\n",
            "    _ -> []\n",
            "  }\n",
            "}\n",
            "fn inner(values: List<Int>) -> List<Int> {\n",
            "  case values {\n",
            "    [_, ..inner, _] -> inner\n",
            "    _ -> []\n",
            "  }\n",
            "}\n",
            "fn last_two(values: List<Int>) -> List<Int> {\n",
            "  case values {\n",
            "    [.., a, b] -> [b, a]\n",
            "    _ -> []\n",
            "  }\n",
            "}\n",
            "pub fn main() -> List<List<Int>> {\n",
            "  [parts([1, 2, 3, 4]), inner([1, 2, 3, 4]), last_two([1, 2, 3]), parts([9])]\n",
            "}\n",
        ),
    );
    assert_eq!(
        outcome,
        "\"outcome\":\"success\",\"value\":[[4,2,1],[2,3],[3,2],[]]"
    );
}

#[test]
fn list_patterns_nest_inside_and_around_constructor_patterns() {
    let outcome = run_both(
        "list-pattern-nesting",
        concat!(
            "enum Slot { Full(value: Int), Hole(width: Int) }\n",
            "fn score(values: List<Slot>) -> Int {\n",
            "  case values {\n",
            "    [Full(first), .., Full(last)] -> first * 10 + last\n",
            "    [Hole(width), ..rest] -> 100 * width + List.length(rest)\n",
            "    _ -> 0\n",
            "  }\n",
            "}\n",
            "fn head(value: Option<List<Int>>) -> Int {\n",
            "  case value {\n",
            "    Some([first, ..]) -> first\n",
            "    Some([]) -> -1\n",
            "    None -> -2\n",
            "  }\n",
            "}\n",
            "fn rest_of(values: List<Int>) -> List<Int> {\n",
            "  case values {\n",
            "    [_, ..rest] -> rest\n",
            "    [] -> values\n",
            "  }\n",
            "}\n",
            "pub fn main() -> List<Int> {\n",
            "  [score([Full(1), Full(5), Full(2)]), score([Hole(3), Full(3), Full(4)]), score([Full(7)]),\n",
            "    head(Some([9, 8])), head(Some(rest_of([1])))]\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":[12,302,0,9,-1]");
}

#[test]
fn list_patterns_match_inside_outcome_patterns() {
    let outcome = run_both(
        "list-pattern-outcome",
        concat!(
            "import std/json as Json\n",
            "import std/json.{Value}\n",
            "fn count(source: Text) -> Int {\n",
            "  case Json.decode(source) {\n",
            "    Success(Value.Array([])) -> 0\n",
            "    Success(Value.Array([_, ..rest])) -> 1 + List.length(rest)\n",
            "    Success(_) -> -1\n",
            "    Failure(error) -> -2\n",
            "  }\n",
            "}\n",
            "pub fn main() -> List<Int> { [count(\"[]\"), count(\"[1, 2, 3]\"), count(\"7\"), count(\"[\")] }\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":[0,3,-1,-2]");
}

// The loops below bind their record state with `let` first: the formatter splits
// `loop state = { a: 1 } {` into a form that no longer parses (pre-existing).
//
// Window retention: draining by `[x, ..rest]` copies a window once it falls below
// a quarter of its vector. A position-weighted sum checks that every element is
// read once, in order, across those copies and across 32-element leaves.
#[test]
fn draining_a_list_reads_every_element_in_order_across_window_copies() {
    let length = 70;
    let elements = (1..=length)
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let source = format!(
        concat!(
            "fn weighted(values: List<Int>) -> Int {{\n",
            "  let start = {{ rest: values, position: 1, total: 0 }}\n",
            "  loop state = start {{\n",
            "    case state.rest {{\n",
            "      [] -> break state.total\n",
            "      [x, ..rest] -> {{ rest: rest, position: state.position + 1, total: state.total + state.position * x }}\n",
            "    }}\n",
            "  }}\n",
            "}}\n",
            "pub fn main() -> Int {{ weighted([{elements}]) }}\n",
        ),
        elements = elements
    );
    let expected: i64 = (1..=length).map(|value: i64| value * value).sum();
    assert_eq!(
        run_both("list-pattern-drain", &source),
        format!("\"outcome\":\"success\",\"value\":{expected}")
    );
}

/// A module that decodes `2^doublings` JSON ones into `values: List<Value>`.
/// Chimi has no list-building primitive yet, so large lists come from doubling a
/// text and decoding it, which is linear in the final length.
fn large_list_module(doublings: u32, rest: &str) -> String {
    format!(
        concat!(
            "import std/json as Json\n",
            "import std/json.{{Value}}\n",
            "fn ones(doublings: Int) -> Text {{\n",
            "  let start = {{ text: \"1\", left: doublings }}\n",
            "  loop state = start {{\n",
            "    case state.left {{\n",
            "      0 -> break state.text\n",
            "      _ -> {{ text: Text.append(Text.append(state.text, \",\"), state.text), left: state.left - 1 }}\n",
            "    }}\n",
            "  }}\n",
            "}}\n",
            "{rest}",
            "pub fn main() -> Int {{\n",
            "  let decoded = Json.decode(Text.append(Text.append(\"[\", ones({doublings})), \"]\"))\n",
            "  case decoded {{\n",
            "    Value.Array(values) -> run(values)\n",
            "    _ -> -1\n",
            "  }}\n",
            "}}\n",
        ),
        doublings = doublings,
        rest = rest
    )
}

/// LP-07's program: `[.., last]` read `repeats` times from one list.
fn last_element_program(doublings: u32, repeats: u32) -> String {
    large_list_module(
        doublings,
        &format!(
            concat!(
                "fn run(values: List<Value>) -> Int {{\n",
                "  let start = {{ left: {repeats}, found: 0 }}\n",
                "  loop state = start {{\n",
                "    case state.left {{\n",
                "      0 -> break state.found\n",
                "      _ -> case values {{\n",
                "        [.., Value.Number(_)] -> {{ left: state.left - 1, found: state.found + 1 }}\n",
                "        _ -> {{ left: state.left - 1, found: state.found }}\n",
                "      }}\n",
                "    }}\n",
                "  }}\n",
                "}}\n",
            ),
            repeats = repeats
        ),
    )
}

/// LP-08's program: the `loop` that sums a list by rebinding `[x, ..rest]`.
fn draining_sum_program(doublings: u32) -> String {
    large_list_module(
        doublings,
        concat!(
            "fn run(values: List<Value>) -> Int {\n",
            "  let start = { rest: values, total: 0 }\n",
            "  loop state = start {\n",
            "    case state.rest {\n",
            "      [] -> break state.total\n",
            "      [Value.Number(_), ..rest] -> { rest: rest, total: state.total + 1 }\n",
            "      [_, ..rest] -> { rest: rest, total: state.total }\n",
            "    }\n",
            "  }\n",
            "}\n",
        ),
    )
}

// LP-07 on both paths. The reference interpreter records a full trace of every
// operation, so this runs at 2^14 elements; `tests/cli.rs` checks the
// 1,000,000-element row natively, where allocation is measurable.
#[test]
fn the_last_element_pattern_reads_one_element_of_a_large_list() {
    let outcome = run_both("list-pattern-last", &last_element_program(14, 200));
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":200");
}

// LP-08 on both paths, at 2^14 elements for the same reason as LP-07.
#[test]
fn a_loop_drains_a_large_list_by_rebinding_the_rest() {
    let outcome = run_both("list-pattern-sum", &draining_sum_program(14));
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":16384");
}

#[test]
fn list_patterns_format_canonically() {
    let source = concat!(
        "fn f(values: List<Int>) -> Int {\n",
        "  case values {\n",
        "    [ ] -> 0\n",
        "    [first,..rest,last] -> first\n",
        "    [ .. , last ] -> last\n",
        "  }\n",
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
            "fn f(values: List<Int>) -> Int {\n",
            "  case values {\n",
            "    [] -> 0\n",
            "    [first, ..rest, last] -> first\n",
            "    [.., last] -> last\n",
            "  }\n",
            "}\n",
        )
    );
}

#[test]
fn hovering_a_list_pattern_binding_reports_its_type() {
    let source = concat!(
        "fn f(values: List<Int>) -> Int {\n",
        "  case values {\n",
        "    [] -> 0\n",
        "    [first, ..rest] -> first + List.length(rest)\n",
        "  }\n",
        "}\n",
        "pub fn main() -> Int { f([1]) }\n",
    );
    for (name, expected) in [("first,", "Int"), ("rest]", "List<Int>")] {
        let offset = source.find(name).unwrap();
        let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
            .expect("valid module path");
        let inspection = Compiler::inspect_project(vec![module], "main", offset).unwrap();
        assert_eq!(inspection.inferred_type(), Some(expected), "{name}");
    }
}
