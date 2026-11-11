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

// B2: `not` binds more loosely than `==`; `(not 1) == 2` would not type-check.
#[test]
fn not_applies_to_a_whole_comparison() {
    let outcome = run_both(
        "boolean-not-precedence",
        "pub fn main() -> Bool { not 1 == 2 }\n",
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
                "const approving = {{ approve: fn(_amount: Int) {{ True }} }}\n",
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

// The element types of both channels are never constrained, so they are erased
// (ts-fff274) and the send succeeds. Before bcf1885 this program crashed the
// checker's fixed point; native-only because the interpreter has no channels.
#[test]
fn unpinned_channel_element_in_a_propagated_failure_runs() {
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
    let outcome = native_outcome(&scratch, &path);
    assert!(
        outcome.ends_with("\"outcome\":\"success\",\"value\":1}"),
        "{outcome}"
    );
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

// ts-5b02a8: a line break after a field's `errors` row separates fields. A
// `needs` row on the next line continues a function type, and a
// function-typed parameter ends at the comma of a multi-line parameter list.
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
            "fn twice(\n",
            "  step: fn(Int) -> Int errors FileFault\n",
            "    needs { files: Files },\n",
            "  value: Int,\n",
            ") -> Int needs { files: Files } {\n",
            "  step(step(value))\n",
            "}\n",
            "fn grow(value: Int) -> Int needs { files: Files } {\n",
            "  value + files.size(\"x\")\n",
            "}\n",
            "const fake = {\n",
            "  read: fn(_path: Text) { 40 },\n",
            "  write: fn(_path: Text, value: Int) { value },\n",
            "  size: fn(_path: Text) { 2 },\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  twice(grow, copy(\"a\", \"b\")) using { files: fake }\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":46");
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
            "const missing = {\n",
            "  read: fn(path: Text) -> Int { Failure(FileError.Read(path, \"missing\")) },\n",
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
            "const triple = { apply: fn(value) { value * 3 } }\n",
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

// A fieldless variant named as a value is that value, so inspection reports
// its value type and no constructor signature; called, it is a callable.
#[test]
fn inspecting_a_fieldless_variant_value_reports_no_callable() {
    let source = concat!(
        "fn count(option: Option<Int>) -> Int {\n",
        "  case option {\n",
        "    Some(value) -> value\n",
        "    None -> 0\n",
        "  }\n",
        "}\n",
        "pub fn main() -> Int { count(None) + count(None()) }\n",
    );
    let inspect = |needle: &str| {
        let offset = source.find(needle).expect("needle is in the source") + 1;
        let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
            .expect("valid module path");
        let report =
            Compiler::inspect_project(vec![module], "main", offset).expect("inspection succeeds");
        (
            report.inferred_type().map(str::to_owned),
            report
                .callable()
                .map(|callable| callable.display().to_owned()),
        )
    };
    assert_eq!(inspect("None) +"), (Some("Option<Int>".to_owned()), None));
    let (_, called) = inspect("None())");
    assert!(called.is_some(), "a called fieldless variant is a callable");
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
fn float_addition_rounds_so_point_one_plus_point_two_is_not_point_three() {
    let outcome = run_both(
        "float-rounding-equality",
        "pub fn main() -> Bool { 0.1 + 0.2 == 0.3 }\n",
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":false");
}

// N16 (equality half)
#[test]
fn negative_zero_equals_zero() {
    let outcome = run_both(
        "float-negative-zero-equality",
        "pub fn main() -> Bool { 0.0 - 0.0 == -0.0 }\n",
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":true");
}

// E11
#[test]
fn int_and_float_are_never_equal_operands() {
    assert_eq!(
        check_codes("pub fn main() -> Bool { 1 == 1.0 }\n"),
        ["T4302"]
    );
}

// E20
#[test]
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

// A generic function value instantiates where it meets a function type, so it
// can fill a monomorphic function parameter (ts-9e7bae).
#[test]
fn generic_function_values_fill_monomorphic_function_parameters() {
    let outcome = run_both(
        "generic-function-value-argument",
        concat!(
            "fn wrap<T>(value: T) -> Option<T> { Some(value) }\n",
            "fn apply(f: fn(Int) -> Option<Int>, value: Int) -> Option<Int> { f(value) }\n",
            "fn total(option: Option<Int>) -> Int {\n",
            "  case option {\n",
            "    Some(value) -> value\n",
            "    None -> 0\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  total(apply(wrap, 3)) * 10 + total(apply(Some, 4))\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":34");
}

// A generic constructor passed to a generic standard function must not leak
// the callee's failure and needs row parameters into the caller's contract;
// the leak kept the contract fixed point from settling (a checker panic).
#[test]
fn a_generic_constructor_maps_the_values_of_an_updated_map() {
    let outcome = run_both(
        "map-values-with-some",
        concat!(
            "import std/map as Map\n",
            "pub fn main() -> Int {\n",
            "  let counts = Map.update(Map.empty(), \"a\", fn(_current) { 2 })\n",
            "  let wrapped = Map.map_values(counts, Some)\n",
            "  case Map.get(wrapped, \"a\") {\n",
            "    Some(Some(value)) -> value\n",
            "    _ -> 0\n",
            "  }\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":2");
}

// Unrelated declarations change how many fixed-point passes the checker
// runs; with them the leak surfaced as bogus T4203 and E5002 diagnostics.
#[test]
fn map_values_with_a_constructor_checks_cleanly_beside_other_declarations() {
    let codes = check_codes(concat!(
        "import std/map as Map\n",
        "fn unused() -> Int { 1 }\n",
        "fn unused_too() -> Int { 1 }\n",
        "pub fn main() -> Int {\n",
        "  let counts = Map.update(Map.empty(), \"a\", fn(_current) { 2 })\n",
        "  let wrapped = Map.map_values(counts, Some)\n",
        "  1\n",
        "}\n",
    ));
    assert!(codes.is_empty(), "unexpected diagnostics: {codes:?}");
}

// A type parameter no argument mentions is fixed by the call's context, both
// for the enum constructor in the body and at the call site (ts-5a8e9a).
#[test]
fn a_type_parameter_only_in_the_result_is_inferred_from_context() {
    let outcome = run_both(
        "result-only-type-parameter",
        concat!(
            "enum Res<T, E> {\n",
            "  Good(value: T)\n",
            "  Bad(error: E)\n",
            "}\n",
            "fn fail_with<T, E>(error: E) -> Res<T, E> { Bad(error) }\n",
            "fn score(result: Res<Int, Int>) -> Int {\n",
            "  case result {\n",
            "    Good(value) -> value\n",
            "    Bad(error) -> error + 1\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  score(fail_with(3))\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":4");
}

// ts-fff274: a type parameter that no constraint ever reaches is erased. The
// only values of `Option<T>` that never pin `T` are values like `None`, so the
// program runs instead of failing execution setup with E5006.
#[test]
fn a_never_constrained_type_parameter_passed_to_a_generic_callee_runs() {
    let outcome = run_both(
        "never-constrained-argument",
        concat!(
            "fn nothing<T>() -> Option<T> { None }\n",
            "fn is_none<T>(option: Option<T>) -> Bool {\n",
            "  case option {\n",
            "    Some(_) -> False\n",
            "    None -> True\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Bool {\n",
            "  is_none(nothing())\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":true");
}

// ts-fff274: two unconstrained instantiations, one through a local binding.
#[test]
fn never_constrained_type_parameters_through_a_binding_run() {
    let outcome = run_both(
        "never-constrained-binding",
        concat!(
            "fn nothing<T>() -> Option<T> { None }\n",
            "fn count<T>(option: Option<T>) -> Int {\n",
            "  case option {\n",
            "    Some(_) -> 1\n",
            "    None -> 0\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let none = nothing()\n",
            "  count(none) + count(nothing()) + 2\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":2");
}

// ts-fff274: an unused empty map has never-constrained key and value types.
#[test]
fn an_unused_empty_map_runs() {
    let outcome = run_both(
        "unused-empty-map",
        concat!(
            "import std/map as Map\n",
            "pub fn main() -> Int {\n",
            "  let unused = Map.empty()\n",
            "  1\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":1");
}

// ts-fff274: a callee's needs row parameter left unbound by a type error is
// not reported again as an unresolved row on `main`.
#[test]
fn a_callee_row_parameter_left_by_a_type_error_is_not_reported_on_main() {
    let codes = check_codes(concat!(
        "import std/map as Map\n",
        "fn build() {\n",
        "  Map.update(Map.empty(), \"a\", 3)\n",
        "}\n",
        "pub fn main() -> Int {\n",
        "  let built = build()\n",
        "  1\n",
        "}\n",
    ));
    assert_eq!(codes, ["T4203"]);
}

// ts-cefed4: a local binding named like an enum shadows it in a selection, both
// for a member the enum lacks and for one that is also a variant name.
#[test]
fn a_local_binding_shadows_an_enum_of_the_same_name_in_a_selection() {
    let outcome = run_both(
        "local-shadows-enum",
        concat!(
            "enum Status {\n",
            "  Done\n",
            "  Pending\n",
            "}\n",
            "struct Box {\n",
            "  Done: Int\n",
            "  Other: Int\n",
            "}\n",
            "fn score(Status: Box) -> Int {\n",
            "  Status.Done\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let Status = Box(3, 4)\n",
            "  Status.Done * 10 + Status.Other + score(Box(100, 0))\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":134");
}

// ts-50dfc3: a caller declared before a callee whose return type is inferred
// sees the callee's settled contract; declaration order does not matter.
#[test]
fn a_caller_may_precede_a_callee_with_an_inferred_return_type() {
    let outcome = run_both(
        "caller-before-inferred-callee",
        concat!(
            "pub fn main() -> Int {\n",
            "  let constant = konst(3)\n",
            "  constant(True) + constant(\"x\")\n",
            "}\n",
            "fn konst<T, U>(value: T) {\n",
            "  fn(ignored: U) { value }\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":6");
}

// An unannotated lambda parameter matched against a generic callee's parameter
// takes the instantiated type. Before, it took the callee's own `T`, which a
// second fixed-point pass (from the inferred return type) then rejected.
#[test]
fn a_lambda_argument_to_a_generic_callee_checks_in_an_inferred_body() {
    let outcome = run_both(
        "lambda-into-generic-inferred-body",
        concat!(
            "import std/map as Map\n",
            "fn apply<T>(operation: fn(Option<T>) -> T) -> T { operation(None) }\n",
            "fn seven() {\n",
            "  apply(fn(_ignored) { 7 })\n",
            "}\n",
            "fn counts() {\n",
            "  Map.update(Map.empty(), \"a\", fn(_current) { 2 })\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  case Map.get(counts(), \"a\") {\n",
            "    Some(count) -> count + seven()\n",
            "    None -> 0\n",
            "  }\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":9");
}

// ts-dbf382: reading a generic struct field gives the receiver's type argument.
#[test]
fn a_generic_struct_field_has_the_instantiated_type() {
    let source = |argument: &str| {
        format!(
            concat!(
                "struct Box<T> {{\n",
                "  value: T\n",
                "}}\n",
                "fn unbox<T>(boxed: Box<T>) -> T {{ boxed.value }}\n",
                "pub fn main() -> Int {{\n",
                "  let nested = Box(Box({}))\n",
                "  nested.value.value + unbox(Box(1))\n",
                "}}\n",
            ),
            argument
        )
    };
    let outcome = run_both("generic-struct-field", &source("3"));
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":4");
    assert_eq!(check_codes(&source("\"x\"")), ["T4303"]);
}

// ts-d00929: an unannotated lambda parameter takes the callee's parameter type
// before the lambda body is checked, so the body can read its fields. Covers a
// monomorphic callee and a generic one whose `T` comes from an earlier argument.
#[test]
fn a_lambda_body_reads_a_field_of_a_parameter_typed_by_the_callee() {
    let outcome = run_both(
        "lambda-parameter-from-callee",
        concat!(
            "struct Money {\n",
            "  cents: Int\n",
            "}\n",
            "fn apply(money: Money, operation: fn(Money) -> Int) -> Int { operation(money) }\n",
            "fn with<T>(value: T, operation: fn(T) -> Int) -> Int { operation(value) }\n",
            "pub fn main() -> Int {\n",
            "  apply(Money(3), fn(x) { x.cents }) + with(Money(40), fn(y) { y.cents })\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":43");
}

// ts-702c4c: a task passed through a user generic wrapper keeps its failure
// set, both for an infallible and a failing task. Native only: the interpreter
// has no task runtime.
#[test]
fn a_task_result_through_a_generic_wrapper_runs() {
    for (index, (body, expected)) in [
        (
            concat!(
                "fn work() -> Int { 42 }\n",
                "fn wait<T, E>(task: Task<T, E>) { Task.result(task) }\n",
                "fn wait_declared<T, E>(task: Task<T, E>) -> T { Task.result(task) }\n",
                "pub fn main() -> Int {\n",
                "  let named = Task.start(work)\n",
                "  wait(named) + wait_declared(Task.start(work)) + 1\n",
                "}\n",
            ),
            85,
        ),
        (
            concat!(
                "enum Oops {\n",
                "  Bad\n",
                "}\n",
                "fn work() -> Int { Failure(Bad) }\n",
                "fn wait<T, E>(task: Task<T, E>) { Task.result(task) }\n",
                "pub fn main() -> Int {\n",
                "  case wait(Task.start(work)) {\n",
                "    Success(value) -> value\n",
                "    Failure(Bad) -> 3\n",
                "  }\n",
                "}\n",
            ),
            3,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let scratch = scratch_directory(&format!("task-generic-wrapper-{index}"));
        let path = scratch.join("main.chimi");
        std::fs::write(
            &path,
            format!("import std/task as Task\nimport std/task.{{Task}}\n{body}"),
        )
        .unwrap();
        let outcome = native_outcome(&scratch, &path);
        assert!(
            outcome.ends_with(&format!("\"outcome\":\"success\",\"value\":{expected}}}")),
            "{outcome}"
        );
        std::fs::remove_dir_all(scratch).unwrap();
    }
}

// ts-adec7a: a function value generic over a failure row takes the row of the
// slot it fills, from a generic callee bound by another argument and from a
// monomorphic slot. Before, its row was forced empty and the call gave T4203.
#[test]
fn a_row_polymorphic_function_value_takes_the_rows_of_its_slot() {
    let declarations = concat!(
        "enum Oops {\n",
        "  Bad\n",
        "}\n",
        "fn run<E>(operation: fn() -> Int errors E) -> Int { operation() }\n",
        "fn rethrow<E>(operation: fn() -> Int errors E) {\n",
        "  case operation() {\n",
        "    Success(value) -> value\n",
        "    Failure(error) -> Failure(error)\n",
        "  }\n",
        "}\n",
        "fn apply<F>(f: fn(fn() -> Int errors F) -> Int errors F, op: fn() -> Int errors F) -> Int {\n",
        "  f(op)\n",
        "}\n",
        "fn handle(f: fn(fn() -> Int errors Oops) -> Int errors Oops) -> Int {\n",
        "  case f(fn() { Failure(Oops.Bad) }) {\n",
        "    Success(value) -> value\n",
        "    Failure(Bad) -> 9\n",
        "  }\n",
        "}\n",
        "fn failing() -> Int { Failure(Oops.Bad) }\n",
        "fn five() -> Int { 5 }\n",
    );
    let failing = run_both(
        "row-polymorphic-value-generic-slot",
        &format!("{declarations}pub fn main() -> Int {{\n  apply(run, failing)\n}}\n"),
    );
    assert_eq!(
        failing,
        "\"outcome\":\"expected_failure\",\"error\":{\"type\":\"Oops\",\"variant\":\"Bad\",\"fields\":{}}"
    );
    let handled = run_both(
        "row-polymorphic-value-monomorphic-slot",
        &format!(
            "{declarations}pub fn main() -> Int {{\n  handle(rethrow) + apply(rethrow, five) + apply(run, five)\n}}\n"
        ),
    );
    assert_eq!(handled, "\"outcome\":\"success\",\"value\":19");
}

// A row-polymorphic function value that would fail is still rejected where an
// infallible function is expected.
#[test]
fn a_failing_instantiation_of_a_function_value_does_not_fit_an_infallible_slot() {
    let codes = check_codes(concat!(
        "enum Oops {\n",
        "  Bad\n",
        "}\n",
        "fn run<E>(operation: fn() -> Int errors E) -> Int { operation() }\n",
        "fn apply(f: fn(fn() -> Int errors Oops) -> Int) -> Int {\n",
        "  f(fn() { Failure(Oops.Bad) })\n",
        "}\n",
        "pub fn main() -> Int {\n",
        "  apply(run)\n",
        "}\n",
    ));
    assert_eq!(codes, ["T4203"]);
}

// ts-0ee7c8: a `Failure(...)` arm that catches every failure is unreachable when
// the subject's failure row is closed and empty; one over a fallible subject
// is not.
#[test]
fn a_failure_arm_over_an_infallible_subject_is_unreachable() {
    let source = |body: &str| {
        format!(
            concat!(
                "enum Oops {{\n",
                "  Bad\n",
                "}}\n",
                "fn five() -> Int {{ 5 }}\n",
                "fn failing() -> Int {{ Failure(Bad) }}\n",
                "pub fn main() -> Int {{\n",
                "  case {}() {{\n",
                "    Success(value) -> value\n",
                "    Failure(_error) -> 0\n",
                "  }}\n",
                "}}\n",
            ),
            body
        )
    };
    assert_eq!(check_codes(&source("five")), ["T4232"]);
    assert!(check_codes(&source("failing")).is_empty());
}

// A constructor value renders as its constructor's name on both paths, as a
// standard function value renders as its function's name.
#[test]
fn a_constructor_value_result_renders_its_name() {
    let outcome = run_both(
        "constructor-value-result",
        concat!(
            "enum Shape {\n",
            "  Circle(radius: Int)\n",
            "  Dot\n",
            "}\n",
            "pub fn main() -> fn(Int) -> Shape {\n",
            "  Circle\n",
            "}\n",
        ),
    );
    assert_eq!(
        outcome,
        "\"outcome\":\"success\",\"value\":{\"callable\":\"Circle\"}"
    );
}

// Source enum variants resolve qualified by their type, bare and called, in
// expressions and patterns, as standard ones do (ts-7d6bce).
#[test]
fn qualified_source_enum_variants_resolve_in_expressions_and_patterns() {
    let outcome = run_both(
        "qualified-source-variants",
        concat!(
            "enum Status {\n",
            "  Pending\n",
            "  Done\n",
            "}\n",
            "enum Shape {\n",
            "  Circle(radius: Int)\n",
            "  Dot\n",
            "}\n",
            "fn rank(status: Status) -> Int {\n",
            "  case status {\n",
            "    Status.Pending -> 1\n",
            "    Status.Done -> 2\n",
            "  }\n",
            "}\n",
            "fn size(shape: Shape) -> Int {\n",
            "  case shape {\n",
            "    Shape.Circle(radius) -> radius\n",
            "    Shape.Dot -> 0\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  rank(Done) * 1000 + rank(Status.Done) * 100 + rank(Status.Pending) * 10\n",
            "  + size(Shape.Circle(7)) + size(Shape.Dot)\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":2217");
}

#[test]
fn qualified_variants_of_an_imported_enum_resolve() {
    let shapes = ProjectSource::new(
        "shapes",
        SourceSnapshot::from_text(
            "shapes.chimi",
            "pub enum Shape {\n  Circle(radius: Int)\n  Dot\n}\n",
        ),
    )
    .expect("valid module path");
    let main = ProjectSource::new(
        "main",
        SourceSnapshot::from_text(
            "main.chimi",
            concat!(
                "import shapes.{Shape}\n",
                "fn size(shape: Shape) -> Int {\n",
                "  case shape {\n",
                "    Shape.Circle(radius) -> radius\n",
                "    Shape.Dot -> 0\n",
                "  }\n",
                "}\n",
                "pub fn main() -> Int { size(Shape.Circle(7)) + size(Shape.Dot) }\n",
            ),
        ),
    )
    .expect("valid module path");
    let codes = Compiler::type_project(vec![shapes, main])
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code().to_owned())
        .collect::<Vec<_>>();
    assert!(codes.is_empty(), "unexpected diagnostics: {codes:?}");
}

#[test]
fn an_unknown_qualified_variant_is_unresolved() {
    let codes = check_codes(concat!(
        "enum Status {\n",
        "  Pending\n",
        "}\n",
        "pub fn main() -> Status { Status.Done }\n",
    ));
    assert_eq!(codes, ["E_UNRESOLVED_MEMBER"]);
}

// Publishing a struct exposes its type, not its fields (ts-9c6abd): the
// declaring module reads them, another module gets E_PRIVATE_STRUCT_FIELD,
// pointed at the published accessor when one exists. A published function with
// the field's name that does not take the struct is not an accessor.
fn money_diagnostics(reader: &str) -> Vec<(String, String)> {
    let money = ProjectSource::new(
        "money",
        SourceSnapshot::from_text(
            "money.chimi",
            concat!(
                "pub struct Money {\n",
                "  cents: Int\n",
                "  currency: Text\n",
                "}\n",
                "pub fn make(cents: Int) -> Money { Money(cents, \"USD\") }\n",
                "pub fn cents(money: Money) -> Int { money.cents }\n",
                // Named like a field but not an accessor: it takes no Money.
                "pub fn currency(code: Text) -> Text { code }\n",
            ),
        ),
    )
    .expect("valid module path");
    let main = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", reader))
        .expect("valid module path");
    Compiler::type_project(vec![money, main])
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

#[test]
fn struct_fields_are_private_to_their_declaring_module() {
    let reader = |body: &str| {
        format!(
            "import money as Money\nimport money.{{Money}}\n\
             fn size(_text: Text) -> Int {{ 1 }}\n\
             pub fn main() -> Int {{ {body} }}\n"
        )
    };
    assert_eq!(money_diagnostics(&reader("Money.cents(Money.make(5))")), []);
    assert_eq!(
        money_diagnostics(&reader("Money.make(5).cents")),
        [(
            "E_PRIVATE_STRUCT_FIELD".to_owned(),
            "field cents of Money is module-private; use Money.cents(Money.make(5))".to_owned()
        )]
    );
    assert_eq!(
        money_diagnostics(&reader("size(Money.make(5).currency)")),
        [(
            "E_PRIVATE_STRUCT_FIELD".to_owned(),
            "field currency of Money is module-private".to_owned()
        )]
    );
}

/// Runs a multi-module project rooted at a scratch directory on both execution paths.
/// `modules` pairs a module path such as `app/main` with its source; the first is the entry.
fn run_both_project(name: &str, modules: &[(&str, &str)]) -> String {
    let scratch = scratch_directory(name);
    let root = scratch.join("project");
    for (module, source) in modules {
        let path = root.join(format!("{module}.chimi"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, source).unwrap();
        assert_formatter_round_trips(source);
    }
    let entry = modules[0].0;
    let interpreted = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .arg("run")
        .arg("--root")
        .arg(&root)
        .arg(entry)
        .output()
        .expect("run the reference interpreter");
    let interpreted = String::from_utf8_lossy(&interpreted.stdout)
        .lines()
        .last()
        .unwrap_or_default()
        .to_owned();
    let executable = scratch.join("program");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .arg("build")
        .arg("--root")
        .arg(&root)
        .arg(entry)
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
        .arg(format!("--outcome-file={}", outcome_path.display()))
        .arg("--")
        .output()
        .expect("execute the native program");
    let native = std::fs::read_to_string(outcome_path)
        .expect("native outcome file")
        .trim_end()
        .to_owned();
    assert_eq!(interpreted, native, "execution paths disagree for {name}");
    std::fs::remove_dir_all(scratch).unwrap();
    interpreted
        .strip_prefix("{\"schema\":\"chimi.runtime.v1\",")
        .and_then(|rest| rest.strip_suffix('}'))
        .unwrap_or_else(|| panic!("unexpected outcome line: {interpreted}"))
        .to_owned()
}

/// Diagnostic codes from checking a project, including build-time constant evaluation.
fn build_codes(modules: &[(&str, &str)]) -> Vec<String> {
    let sources = modules
        .iter()
        .map(|(module, source)| {
            ProjectSource::new(
                *module,
                SourceSnapshot::from_text(format!("{module}.chimi"), *source),
            )
            .expect("valid module path")
        })
        .collect();
    match Compiler::verify_project(sources) {
        Ok(_) => Vec::new(),
        Err(diagnostics) => diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code().to_owned())
            .collect(),
    }
}

fn build_messages(source: &str) -> Vec<String> {
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    match Compiler::verify_project(vec![module]) {
        Ok(_) => Vec::new(),
        Err(diagnostics) => diagnostics
            .iter()
            .map(|diagnostic| format!("{}: {}", diagnostic.code(), diagnostic.message()))
            .collect(),
    }
}

// G7: every constant-representable kind survives build-time evaluation and loads to
// the same value on both backends, including closures that capture computed values.
#[test]
fn constants_of_every_representable_kind_load_the_built_value() {
    let outcome = run_both(
        "constant-kinds",
        concat!(
            "import std/map as Map\n",
            "import std/json as Json\n",
            "struct Money {\n",
            "  cents: Int\n",
            "}\n",
            "enum Status {\n",
            "  Pending\n",
            "  Paid(amount: Money)\n",
            "}\n",
            "const base = 40\n",
            "pub const answer: Int = base + 2\n",
            "const ratio = 0.5\n",
            "const greeting = Text.append(\"hello \", \"world\")\n",
            "const ready = answer > base and True\n",
            "const prices = [Money(1), Money(base)]\n",
            "const status = Paid(Money(answer))\n",
            "const missing: Option<Int> = None\n",
            "const present = Some(answer)\n",
            "const table = Map.put(Map.put(Map.empty(), \"b\", 2), \"a\", 1)\n",
            "const number = Json.int(7)\n",
            "const add_answer = make_adder(answer)\n",
            "const double = { apply: fn(value: Int) { value * 2 } }\n",
            "const join = Text.append\n",
            "fn make_adder(amount: Int) -> fn(Int) -> Int {\n",
            "  fn(value: Int) { value + amount }\n",
            "}\n",
            "fn total(values: List<Money>) -> Int {\n",
            "  case values {\n",
            "    [] -> 0\n",
            "    [first, ..rest] -> first.cents + total(rest)\n",
            "  }\n",
            "}\n",
            "fn paid(value: Status) -> Int {\n",
            "  case value {\n",
            "    Pending -> 0\n",
            "    Paid(amount) -> amount.cents\n",
            "  }\n",
            "}\n",
            "fn or_zero(value: Option<Int>) -> Int {\n",
            "  case value {\n",
            "    Some(inner) -> inner\n",
            "    None -> 0\n",
            "  }\n",
            "}\n",
            "pub fn main() {\n",
            "  let looked = or_zero(Map.get(table, \"a\")) + or_zero(Map.get(table, \"b\"))\n",
            "  let result = {\n",
            "    a: add_answer(total(prices)) + double.apply(paid(status)) + or_zero(missing),\n",
            "    b: join(greeting, \"!\"),\n",
            "    c: ready,\n",
            "    d: ratio,\n",
            "    e: or_zero(present) + looked,\n",
            "    f: number,\n",
            "  }\n",
            "  result\n",
            "}\n",
        ),
    );
    assert_eq!(
        outcome,
        concat!(
            "\"outcome\":\"success\",\"value\":{\"a\":167,\"b\":\"hello world!\",",
            "\"c\":true,\"d\":0.5,\"e\":45,\"f\":{\"type\":\"Value\",\"variant\":\"Number\",",
            "\"fields\":{\"value\":\"7\"}}}"
        )
    );
}

// G7: a public constant is a value of its module, used qualified from another module.
#[test]
fn a_public_constant_is_used_qualified_from_another_module() {
    let outcome = run_both_project(
        "constant-across-modules",
        &[
            (
                "app/main",
                concat!(
                    "import lib/duration as Duration\n",
                    "pub fn main() -> Int {\n",
                    "  Duration.nanoseconds(Duration.zero) + Duration.nanoseconds(Duration.seconds(2))\n",
                    "}\n",
                ),
            ),
            (
                "lib/duration",
                concat!(
                    "pub struct Duration {\n",
                    "  nanoseconds: Int\n",
                    "}\n",
                    "pub const zero = Duration(0)\n",
                    "const scale = 1_000_000_000\n",
                    "pub fn seconds(count: Int) -> Duration {\n",
                    "  Duration(count * scale)\n",
                    "}\n",
                    "pub fn nanoseconds(duration: Duration) -> Int {\n",
                    "  duration.nanoseconds\n",
                    "}\n",
                ),
            ),
        ],
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":2000000000");
}

// G7 visibility: a private constant stays in its module, and a member import cannot
// bind a constant any more than it can bind a function.
#[test]
fn constants_are_private_by_default_and_never_member_imported() {
    let library = concat!("pub const zero = 0\n", "const secret = 1\n",);
    let codes = |main: &str| build_codes(&[("app/main", main), ("lib/values", library)]);
    assert_eq!(
        codes("import lib/values as Values\npub fn main() -> Int { Values.secret }\n"),
        ["E_PRIVATE_SYMBOL"]
    );
    assert_eq!(
        codes("import lib/values.{zero}\npub fn main() -> Int { 0 }\n"),
        ["E_UNQUALIFIED_CONSTANT_IMPORT"]
    );
    assert!(
        codes("import lib/values as Values\npub fn main() -> Int { Values.zero }\n").is_empty()
    );
}

// G7: `const a = b` and `const b = a` in two modules is one cycle, reported once.
#[test]
fn constants_that_name_each_other_across_modules_are_one_cycle() {
    let codes = build_codes(&[
        (
            "app/main",
            "import lib/other as Other\npub const a = Other.b\npub fn main() -> Int { 0 }\n",
        ),
        (
            "lib/other",
            "import app/main as Main\npub const b = Main.a\n",
        ),
    ]);
    assert_eq!(codes, ["E5205"]);
}

// G7 conformance rows for rejected constants. Each program is otherwise valid, so the
// listed code is the whole verdict, and constants nothing references still count.
#[test]
fn constants_that_break_a_build_time_rule_fail_the_build() {
    let cases: &[(&str, &str, &[&str])] = &[
        ("module let", "let limit = 32\n", &["L2101"]),
        (
            "const in a block",
            "fn f() -> Int {\n  const limit = 32\n  limit\n}\n",
            &["S1103"],
        ),
        (
            "failing initializer",
            "import std/int as Int\nconst parsed = Int.parse(\"5\")\n",
            &["T4321"],
        ),
        (
            "initializer with needs",
            concat!(
                "interface Clock {\n  now: fn() -> Int\n}\n",
                "fn read() -> Int needs { clock: Clock } { clock.now() }\n",
                "const now = read()\n",
            ),
            &["T4322"],
        ),
        (
            "capability",
            "import std/channel as Channel\nconst jobs = Channel.bounded(8)\n",
            &["T4323"],
        ),
        ("undetermined", "const nothing = None\n", &["T4324"]),
        (
            "runtime operation in a block",
            concat!(
                "import std/channel as Channel\n",
                "import std/channel.{Sender}\n",
                "fn keep(sender: Sender<Int>) -> Int { 5 }\n",
                "const total = {\n  let pair = Channel.bounded(1)\n  keep(pair.sender)\n}\n",
            ),
            &["E5201"],
        ),
        (
            "defect",
            concat!(
                "fn first_or_defect(values: List<Int>) -> Int {\n",
                "  case values {\n    [first, ..] -> first\n    [] -> 1 / 0\n  }\n}\n",
                "const first = first_or_defect([])\n",
            ),
            &["E5202"],
        ),
        ("cycle", "const a = b\nconst b = a + 1\n", &["E5205"]),
        (
            "only a dependency reports",
            "const broken = 1 / 0\nconst uses = broken + 1\n",
            &["E5202"],
        ),
    ];
    for (name, declarations, expected) in cases {
        let source = format!("{declarations}pub fn main() -> Int {{ 0 }}\n");
        assert_eq!(build_codes(&[("main", &source)]), *expected, "{name}");
    }
}

#[test]
fn constant_diagnostics_name_what_broke_the_rule() {
    let messages = build_messages(concat!(
        "import std/channel as Channel\n",
        "import std/channel.{Sender}\n",
        "fn keep(sender: Sender<Int>) -> Int { 5 }\n",
        "fn make() -> Int {\n  let pair = Channel.bounded(1)\n  keep(pair.sender)\n}\n",
        "const total = make()\n",
        "pub fn main() -> Int { 0 }\n",
    ));
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains("`std/channel.bounded`") && messages[0].contains("total -> make"),
        "{messages:?}"
    );
    let messages = build_messages(concat!(
        "import std/channel as Channel\n",
        "const jobs = Channel.bounded(8)\n",
        "pub fn main() -> Int { 0 }\n",
    ));
    assert!(messages[0].contains("`Sender`"), "{messages:?}");
    let messages = build_messages("const a = b\nconst b = a\npub fn main() -> Int { 0 }\n");
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("a -> b -> a"), "{messages:?}");
}

#[test]
fn a_module_let_carries_the_edit_to_const() {
    let module = ProjectSource::new(
        "main",
        SourceSnapshot::from_text(
            "main.chimi",
            "let limit = 32\npub fn main() -> Int { limit }\n",
        ),
    )
    .expect("valid module path");
    let report = Compiler::type_project(vec![module]);
    let [diagnostic] = report.diagnostics() else {
        panic!("{:?}", report.diagnostics());
    };
    assert_eq!(diagnostic.code(), "L2101");
    let [fix] = diagnostic.fixes() else {
        panic!("L2101 carries one edit");
    };
    assert_eq!(fix.replacement(), "const");
    assert_eq!((fix.range().start(), fix.range().end()), (0, 3));
}

// G7 "At run time": the initializer ran at build time, so referencing a constant
// produces only the reference's own events, on both backends' full traces.
#[test]
fn referencing_a_constant_runs_none_of_its_initializer() {
    let source = concat!(
        "const config = { limit: 41, step: fn(value: Int) { value + 1 } }\n",
        "pub fn main() -> Int {\n",
        "  config.step(config.limit)\n",
        "}\n",
    );
    let scratch = scratch_directory("constant-trace");
    let path = scratch.join("main.chimi");
    std::fs::write(&path, source).unwrap();
    let interpreted = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .arg("run")
        .arg(&path)
        .output()
        .expect("run the reference interpreter");
    let interpreted = String::from_utf8_lossy(&interpreted.stdout).into_owned();
    let executable = scratch.join("program");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .arg("build")
        .arg(&path)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("build the native executable");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let telemetry = scratch.join("telemetry.jsonl");
    Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=full")
        .arg(format!("--telemetry-file={}", telemetry.display()))
        .arg("--")
        .output()
        .expect("execute the native program");
    let native = std::fs::read_to_string(telemetry).unwrap();
    for (backend, trace) in [("interpreter", &interpreted), ("native", &native)] {
        assert!(
            trace.contains("\"operation\":\"reference\""),
            "{backend}: {trace}"
        );
        for initializer_operation in ["record", "closure"] {
            assert!(
                !trace.contains(&format!("\"operation\":\"{initializer_operation}\"")),
                "{backend} ran the initializer's {initializer_operation}: {trace}"
            );
        }
    }
    assert!(interpreted.ends_with("\"outcome\":\"success\",\"value\":42}\n"));
    std::fs::remove_dir_all(scratch).unwrap();
}

// G9 conformance: interpolation inserts Text unchanged and Int and Float in their
// canonical text, left to right; `\$` writes `$`, and `{` alone is never interpolation.
#[test]
fn interpolation_inserts_text_and_numbers_in_canonical_form() {
    let outcome = run_both(
        "interpolation",
        concat!(
            "fn blame_argument(line: Int) -> Text {\n",
            "  \"-L ${line},${line}\"\n",
            "}\n",
            "fn join(left: Text, right: Text) -> Text { Text.append(left, right) }\n",
            "pub fn main() -> Text {\n",
            "  let name = \"pay\"\n",
            "  let pieces = [\n",
            "    blame_argument(7),\n",
            "    \"|${name}|${-42}|${0.1 + 0.2}|${1e20}|\",\n",
            "    \"cost: \\${amount}\",\n",
            "    \"{\\\"a\\\": 1}\",\n",
            "    \"${join(\"a\", \"${name}\")}${1 + 2}\",\n",
            "  ]\n",
            "  join_all(pieces)\n",
            "}\n",
            "fn join_all(pieces: List<Text>) -> Text {\n",
            "  case pieces {\n",
            "    [] -> \"\"\n",
            "    [first, ..rest] -> \"${first};${join_all(rest)}\"\n",
            "  }\n",
            "}\n",
        ),
    );
    assert_eq!(
        outcome,
        concat!(
            "\"outcome\":\"success\",\"value\":",
            "\"-L 7,7;|pay|-42|0.30000000000000004|1e20|;cost: ${amount};",
            "{\\\\\\\"a\\\\\\\": 1};apay3;\""
        )
    );
}

// G9: an interpolated expression is an ordinary subexpression, so its failures join
// the enclosing function's failure row.
#[test]
fn an_interpolated_failure_joins_the_enclosing_row() {
    let outcome = run_both(
        "interpolation-failure",
        concat!(
            "enum LoadError {\n",
            "  Missing(path: Text)\n",
            "}\n",
            "fn load(path: Text) -> Text {\n",
            "  Failure(Missing(path))\n",
            "}\n",
            "pub fn main() -> Text {\n",
            "  \"loaded ${load(\"a.txt\")}\"\n",
            "}\n",
        ),
    );
    assert_eq!(
        outcome,
        concat!(
            "\"outcome\":\"expected_failure\",\"error\":{\"type\":\"LoadError\",",
            "\"variant\":\"Missing\",\"fields\":{\"path\":\"a.txt\"}}"
        )
    );
}

#[test]
fn interpolation_rejects_empty_unclosed_and_non_text_parts() {
    let codes = |body: &str| {
        check_codes(&format!(
            "enum State {{\n  Ready\n}}\npub fn main() -> Text {{\n  let state = Ready\n  {body}\n}}\n"
        ))
    };
    assert_eq!(codes("\"${}\""), ["S1101"]);
    assert!(codes("\"${state\"").contains(&"S1101".to_owned()));
    assert_eq!(codes("\"state: ${state}\""), ["T4325"]);
    assert_eq!(codes("\"flags: ${[1]}\""), ["T4325"]);
    assert!(codes("\"ok: ${\"${1}\"}\"").is_empty());
}

#[test]
fn interpolation_suggests_debug_rendering_for_other_types() {
    let source = "pub fn main() -> Text {\n  \"${True}\"\n}\n";
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    let report = Compiler::type_project(vec![module]);
    let [diagnostic] = report.diagnostics() else {
        panic!("{:?}", report.diagnostics());
    };
    assert_eq!(diagnostic.code(), "T4325");
    assert!(diagnostic.message().contains("Debug.render"));
}

#[test]
fn interpolations_format_tightly_and_keep_their_text() {
    let source = "pub fn main() -> Text {\n  \"a ${ 1+2 } b ${Text.append(\"x\",\"y\")}\"\n}\n";
    let formatted = Compiler::format(SourceSnapshot::from_text("main.chimi", source));
    assert!(
        !formatted.is_blocked(),
        "{}",
        formatted.render_diagnostics()
    );
    assert_eq!(
        formatted.text(),
        "pub fn main() -> Text {\n  \"a ${1 + 2} b ${Text.append(\"x\", \"y\")}\"\n}\n"
    );
}

fn json_text(text: &str) -> String {
    let mut quoted = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

// G10 Format table: every value kind that exists today renders like the declaration
// or literal that describes it, identically on both backends. Each row pairs the
// rendered expression with the text the table promises.
#[test]
fn debug_render_follows_the_format_table_on_both_backends() {
    let rows: &[(&str, &str)] = &[
        ("-42", "-42"),
        ("-0.5", "-0.5"),
        ("0.0 - 0.0", "0.0"),
        ("1e20", "1e20"),
        ("True", "True"),
        ("False", "False"),
        // A text holding real quotes, which no literal can write yet.
        (
            "Json.encode(Json.text(\"say \\${hi}\"))",
            r#""\"say \${hi}\"""#,
        ),
        ("Money(2500)", "Money { cents: 2500 }"),
        ("GatewayUnavailable", "PaymentError.GatewayUnavailable"),
        ("Declined(\"card\")", r#"PaymentError.Declined("card")"#),
        (
            "Open(Path(\"a.txt\"), \"denied\")",
            r#"FileError.Open(path: Path { text: "a.txt" }, message: "denied")"#,
        ),
        ("Some(3)", "Some(3)"),
        ("nothing()", "None"),
        ("{ name: \"a\", cents: 1 }", r#"{ cents: 1, name: "a" }"#),
        ("[1, 2, 3]", "[1, 2, 3]"),
        ("no_items()", "[]"),
        (
            "Map.put(Map.put(Map.empty(), \"b\", 2), \"a\", 1)",
            r#"Map { "a": 1, "b": 2 }"#,
        ),
        ("Json.int(1)", "Value.Number(Json.Number(1))"),
        (
            "{ charge: checkout, other: fn(x: Int) { x } }",
            "{ charge: <fn checkout>, other: <fn> }",
        ),
        ("Text.append", "<fn append>"),
        ("stored", "[Money { cents: 1 }, Money { cents: 2 }]"),
    ];
    let mut source = String::from(concat!(
        "import std/debug as Debug\n",
        "import std/map as Map\n",
        "import std/json as Json\n",
        "struct Money {\n  cents: Int\n}\n",
        "struct Path {\n  text: Text\n}\n",
        "enum PaymentError {\n  GatewayUnavailable\n  Declined(reason: Text)\n}\n",
        "enum FileError {\n  Open(path: Path, message: Text)\n}\n",
        "const stored = [Money(1), Money(2)]\n",
        "fn checkout(amount: Int) -> Int { amount }\n",
        "fn nothing() -> Option<Int> { None }\n",
        "fn no_items() -> List<Int> { [] }\n",
        "pub fn main() -> List<Text> {\n  [\n",
    ));
    for (expression, _) in rows {
        source.push_str(&format!("    Debug.render({expression}),\n"));
    }
    source.push_str("  ]\n}\n");
    let expected = rows
        .iter()
        .map(|(_, rendered)| json_text(rendered))
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(
        run_both("debug-render", &source),
        format!("\"outcome\":\"success\",\"value\":[{expected}]")
    );
}

// G10: `render_indented` puts each field, element, and entry on its own line.
#[test]
fn debug_render_indented_puts_each_part_on_its_own_line() {
    let outcome = run_both(
        "debug-render-indented",
        concat!(
            "import std/debug as Debug\n",
            "import std/map as Map\n",
            "struct Money {\n  cents: Int\n}\n",
            "enum Status {\n  Declined(reason: Text)\n}\n",
            "fn no_items() -> List<Int> { [] }\n",
            "pub fn main() -> Text {\n",
            "  Debug.render_indented({\n",
            "    items: [Money(1), Money(2)],\n",
            "    empty: no_items(),\n",
            "    status: Declined(\"x\"),\n",
            "    table: Map.put(Map.empty(), \"k\", 1),\n",
            "  })\n",
            "}\n",
        ),
    );
    let expected = concat!(
        "{\n",
        "  empty: [],\n",
        "  items: [\n",
        "    Money {\n",
        "      cents: 1,\n",
        "    },\n",
        "    Money {\n",
        "      cents: 2,\n",
        "    },\n",
        "  ],\n",
        "  status: Status.Declined(\n",
        "    \"x\",\n",
        "  ),\n",
        "  table: Map {\n",
        "    \"k\": 1,\n",
        "  },\n",
        "}",
    );
    assert_eq!(
        outcome,
        format!("\"outcome\":\"success\",\"value\":{}", json_text(expected))
    );
}

// G10: equal values render equally, so rendering cannot tell `1.0` from `1` in JSON
// or `-0.0` from `0.0`, and interpolating a rendering gives the G10 example's text.
#[test]
fn equal_values_render_to_the_same_text() {
    let outcome = run_both(
        "debug-render-equal",
        concat!(
            "import std/debug as Debug\n",
            "import std/json as Json\n",
            "struct Duration {\n  nanoseconds: Int\n}\n",
            "const job_timeout = Duration(30_000_000_000)\n",
            "fn number(text: Text) -> Text {\n",
            "  case Json.decode(text) {\n",
            "    Success(value) -> Debug.render(value)\n",
            "    Failure(_) -> \"invalid\"\n",
            "  }\n",
            "}\n",
            "pub fn main() -> List<Text> {\n",
            "  [\n",
            "    number(\"1.0\"),\n",
            "    number(\"1\"),\n",
            "    number(\"2.50\"),\n",
            "    Debug.render(-0.0),\n",
            "    \"payments returned ${503} after ${Debug.render(job_timeout)}\",\n",
            "  ]\n",
            "}\n",
        ),
    );
    let expected = [
        "Value.Number(Json.Number(1))",
        "Value.Number(Json.Number(1))",
        "Value.Number(Json.Number(2.5))",
        "0.0",
        "payments returned 503 after Duration { nanoseconds: 30000000000 }",
    ]
    .map(json_text)
    .join(",");
    assert_eq!(
        outcome,
        format!("\"outcome\":\"success\",\"value\":[{expected}]")
    );
}

// ---- Equality and ordering (G2/G3), docs/specification/core-data-and-operators.md ----

const MONEY_STATUS: &str =
    "struct Money { cents: Int }\nenum Status {\n  Pending\n  Paid(amount: Money)\n}\n";

// E1, E2, E3, E4, E19: equal payloads, different variants, separately built
// lists, and records written in another field order; `!=` negates each.
#[test]
fn equality_is_structural_and_inequality_is_its_negation() {
    let pairs = [
        ("Paid(Money(5))", "Paid(Money(5))", true),
        ("Paid(Money(5))", "Paid(Money(6))", false),
        ("Paid(Money(5))", "Status.Pending", false),
        ("pair()", "[1, 2]", true),
        ("pair()", "[1, 2, 3]", false),
        ("{ a: 1, b: \"x\" }", "{ b: \"x\", a: 1 }", true),
        ("{ a: 1, b: \"x\" }", "{ b: \"y\", a: 1 }", false),
    ];
    let equal = pairs
        .iter()
        .map(|(left, right, _)| format!("{left} == {right}"))
        .collect::<Vec<_>>()
        .join(", ");
    let not_equal = pairs
        .iter()
        .map(|(left, right, _)| format!("{left} != {right}"))
        .collect::<Vec<_>>()
        .join(", ");
    let source = format!(
        "{MONEY_STATUS}fn pair() -> List<Int> {{ [1, 2] }}\n\
         pub fn main() -> List<List<Bool>> {{ [[{equal}], [{not_equal}]] }}\n"
    );
    let expected = pairs
        .iter()
        .map(|(_, _, equal)| equal.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let negated = pairs
        .iter()
        .map(|(_, _, equal)| (!equal).to_string())
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(
        run_both("structural-equality", &source),
        format!("\"outcome\":\"success\",\"value\":[[{expected}],[{negated}]]")
    );
}

// E5: maps compare by entries, whatever their insertion order.
#[test]
fn maps_built_in_different_orders_are_equal() {
    let source = concat!(
        "import std/map as Map\n",
        "pub fn main() -> List<Bool> {\n",
        "  let forward = Map.empty() |> Map.put(\"a\", 1) |> Map.put(\"b\", 2) |> Map.put(\"c\", 3)\n",
        "  let backward = Map.empty() |> Map.put(\"c\", 3) |> Map.put(\"b\", 2) |> Map.put(\"a\", 1)\n",
        "  let changed = backward |> Map.put(\"b\", 4)\n",
        "  [forward == backward, forward == changed, forward != changed]\n",
        "}\n",
    );
    assert_eq!(
        run_both("map-equality", source),
        "\"outcome\":\"success\",\"value\":[true,false,true]"
    );
}

// Structured keys through the constants table and the debug format: a constant
// map keeps value keys on both backends, and `Debug.render` writes each key in
// the debug format, in one order on both paths whatever the insertion order.
#[test]
fn structured_map_keys_survive_constants_and_debug_rendering() {
    let source = concat!(
        "import std/debug as Debug\n",
        "import std/map as Map\n",
        "struct Point {\n  x: Int,\n  y: Int,\n}\n",
        "const scores = Map.empty() |> Map.put(Point(2, 1), 20) |> Map.put(Point(1, 9), 10)\n",
        "pub fn main() -> List<Text> {\n",
        "  let floats = Map.empty() |> Map.put(1.5, \"b\") |> Map.put(-0.0, \"a\")\n",
        "  [\n",
        "    Debug.render(scores |> Map.get(Point(2, 1))),\n",
        "    Debug.render(scores),\n",
        "    Debug.render(floats |> Map.put(0.0, \"c\")),\n",
        "  ]\n",
        "}\n",
    );
    let expected = [
        "Some(20)",
        "Map { Point { x: 1, y: 9 }: 10, Point { x: 2, y: 1 }: 20 }",
        "Map { 0.0: \"c\", 1.5: \"b\" }",
    ]
    .iter()
    .map(|rendered| json_text(rendered))
    .collect::<Vec<_>>()
    .join(",");
    assert_eq!(
        run_both("structured-map-keys-debug", source),
        format!("\"outcome\":\"success\",\"value\":[{expected}]")
    );
}

// E6 (with a struct and a list for the tuple, which lands with tuples): any
// equatable key finds its entry through an equal, separately built key, and
// equal keys replace each other.
#[test]
fn a_map_keyed_by_structured_values_finds_equal_keys() {
    let source = concat!(
        "import std/map as Map\n",
        "struct Point { x: Int, y: Int }\n",
        "enum Shape {\n  Dot\n  Line(points: List<Point>)\n}\n",
        "fn line() -> Shape { Line([Point(1, 2), Point(3, 4)]) }\n",
        "pub fn main() -> List<Option<Int>> {\n",
        "  let shapes = Map.empty() |> Map.put(line(), 1) |> Map.put(Dot, 2) |> Map.put(line(), 3)\n",
        "  let lists = Map.empty() |> Map.put([1, 2], 10) |> Map.put([1], 20)\n",
        "  [\n",
        "    shapes |> Map.get(Line([Point(1, 2), Point(3, 4)])),\n",
        "    shapes |> Map.get(Dot),\n",
        "    shapes |> Map.get(Line([Point(1, 2)])),\n",
        "    lists |> Map.get([1, 2]),\n",
        "    lists |> Map.get([1]),\n",
        "  ]\n",
        "}\n",
    );
    let some = |value: i64| {
        format!("{{\"type\":\"Option\",\"variant\":\"Some\",\"fields\":{{\"value\":{value}}}}}")
    };
    let none = "{\"type\":\"Option\",\"variant\":\"None\",\"fields\":{}}";
    assert_eq!(
        run_both("structured-map-keys", source),
        format!(
            "\"outcome\":\"success\",\"value\":[{},{},{none},{},{}]",
            some(3),
            some(2),
            some(10),
            some(20)
        )
    );
}

// E15, E16: JSON numbers compare by the exact number, not by spelling or `f64`.
// Text literals have no escapes yet, so E16 nests arrays instead of an object;
// object equality is map equality, covered above.
#[test]
fn json_values_compare_numbers_by_exact_value() {
    let source = concat!(
        "import std/json as Json\n",
        "pub fn main() -> List<Bool> {\n",
        "  [\n",
        "    Json.decode(\"1.0\") == Json.decode(\"1\"),\n",
        "    Json.decode(\"[1, [2, 30]]\") == Json.decode(\"[1.0, [2, 3e1]]\"),\n",
        "    Json.decode(\"0.1\") == Json.decode(\"0.10000000000000000001\"),\n",
        "  ]\n",
        "}\n",
    );
    assert_eq!(
        run_both("json-number-equality", source),
        "\"outcome\":\"success\",\"value\":[true,true,false]"
    );
}

// E17: text equality is by UTF-8 bytes, with no normalization.
#[test]
fn text_equality_does_not_normalize() {
    let outcome = run_both(
        "text-equality-bytes",
        "pub fn main() -> List<Bool> { [\"é\" == \"e\u{301}\", \"é\" == \"é\"] }\n",
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":[false,true]");
}

// E18 end to end: the deepest value Chimi builds in a few seconds today. Both
// paths are bounded by construction, not comparison: the interpreter's value
// `Drop` recurses near 10,000 levels, and native loop collection is quadratic in
// nesting depth. The million-level rows are the unit and C runtime tests.
#[test]
fn deeply_nested_values_compare_equal() {
    let source = concat!(
        "enum Nest {\n  Leaf\n  Wrap(children: List<Nest>)\n}\n",
        "struct Build { depth: Int, nest: Nest }\n",
        "fn nested(depth: Int) -> Nest {\n",
        "  let start = Build(depth, Leaf)\n",
        "  loop state = start {\n",
        "    case state.depth {\n",
        "      0 -> break state.nest\n",
        "      _ -> Build(state.depth - 1, Wrap([state.nest]))\n",
        "    }\n",
        "  }\n",
        "}\n",
        "pub fn main() -> List<Bool> { [nested(5000) == nested(5000), nested(5000) == nested(4999)] }\n",
    );
    assert_eq!(
        run_both("deep-equality", source),
        "\"outcome\":\"success\",\"value\":[true,false]"
    );
}

// O1, O3, O4, O5, O6, O7: declaration-order structs and enums, list prefixes,
// `None` before `Some`, `False` before `True`, and text by UTF-8 bytes. Each
// pair is also checked the other way round, which a constant answer would fail.
#[test]
fn ordering_is_structural_in_declaration_order() {
    let pairs = [
        ("Version(1, 9, 7)", "Version(1, 10, 0)"),
        ("[1, 2]", "[1, 2, 0]"),
        ("[1, 2, 0]", "[1, 3]"),
        ("None", "Some(0)"),
        ("Some(-1)", "Some(0)"),
        ("False", "True"),
        ("Level.Low", "Level.High"),
        ("Below(1)", "Above(0)"),
        ("\"Z\"", "\"a\""),
        ("\"a\"", "\"é\""),
        ("-0.5", "0.25"),
    ];
    let comparisons = pairs
        .iter()
        .map(|(low, high)| {
            format!("[{low} < {high}, {high} > {low}, {high} < {low}, {high} >= {high}]")
        })
        .collect::<Vec<_>>()
        .join(",\n    ");
    let source = format!(
        "struct Version {{ major: Int, minor: Int, patch: Int }}\n\
         enum Level {{\n  Low\n  High\n}}\n\
         enum Rank {{\n  Above(by: Int)\n  Below(by: Int)\n}}\n\
         pub fn main() -> List<List<Bool>> {{\n  [\n    {comparisons}\n  ]\n}}\n"
    );
    let outcome = run_both("structural-ordering", &source);
    // `Rank` declares `Above` first, so `Below(1) < Above(0)` is false.
    let rows = pairs
        .iter()
        .map(|(low, _)| {
            if *low == "Below(1)" {
                "[false,false,true,true]"
            } else {
                "[true,true,false,true]"
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(
        outcome,
        format!("\"outcome\":\"success\",\"value\":[{rows}]")
    );
}

// E8, E9, E10, O13, O14, O15: the diagnostic names the component that lacks
// the property.
#[test]
fn comparing_values_without_the_property_names_the_offending_component() {
    let cases = [
        (
            "struct Route { path: Text, handler: fn() -> Int }\n\
             fn same(a: Route, b: Route) -> Bool { a == b }\n",
            "T4300",
            "field `handler` of `Route` is a function",
        ),
        (
            "import std/channel.{Receiver}\n\
             fn same(a: Receiver<Int>, b: Receiver<Int>) -> Bool { a == b }\n",
            "T4300",
            "a channel capability",
        ),
        (
            "import std/map as Map\nimport std/map.{Map}\n\
             fn keyed() -> Map<fn() -> Int, Int> { Map.empty() }\n",
            "T4300",
            "`Map.empty` needs `K` to be equatable",
        ),
        (
            "fn less() -> Bool { { a: 1 } < { a: 2 } }\n",
            "T4301",
            "a record, which has no declaration order",
        ),
        (
            "import std/map.{Map}\n\
             fn less(a: Map<Text, Int>, b: Map<Text, Int>) -> Bool { a < b }\n",
            "T4301",
            "`Map`, which has no order",
        ),
        (
            "import std/json.{Value}\n\
             fn less(a: List<Value>, b: List<Value>) -> Bool { a < b }\n",
            "T4301",
            "element of `List`",
        ),
    ];
    for (declarations, code, fragment) in cases {
        let source = format!("{declarations}pub fn main() -> Int {{ 1 }}\n");
        let diagnostics = coded_messages(&source);
        assert_eq!(diagnostics.len(), 1, "{source}: {diagnostics:?}");
        assert_eq!(diagnostics[0].0, code, "{source}");
        assert!(
            diagnostics[0].1.contains(fragment),
            "{source}: {}",
            diagnostics[0].1
        );
    }
}

// E11, O16, E12: operands must share a type, and comparisons do not chain.
#[test]
fn mismatched_and_chained_comparisons_are_rejected() {
    assert_eq!(
        check_codes("pub fn main() -> Bool { 1 == \"1\" }\n"),
        ["T4302"]
    );
    assert_eq!(
        check_codes("pub fn main() -> Bool { \"a\" < 1 }\n"),
        ["T4302"]
    );
    for chained in ["1 < 2 < 3", "1 == 1 == True"] {
        let codes = check_codes(&format!("pub fn main() -> Bool {{ {chained} }}\n"));
        assert_eq!(
            codes.first().map(String::as_str),
            Some("S1006"),
            "{chained}"
        );
    }
}

// G5 sweep: a non-numeric arithmetic operand is T4303 on either side, and so is
// unary `-`, with no cascade.
#[test]
fn a_non_numeric_arithmetic_operand_is_t4303_on_either_side() {
    for expression in ["\"a\" + 1", "1 + \"a\"", "-\"a\""] {
        assert_eq!(
            check_codes(&format!("pub fn main() -> Int {{ {expression} }}\n")),
            ["T4303"],
            "{expression}"
        );
    }
}

fn type_report(source: &str) -> chimi::TypeReport {
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    Compiler::type_project(vec![module])
}

// Generic code: `==` on a parameter infers `T: equatable`, shown in the contract
// and carried through calls.
#[test]
fn equality_on_a_generic_parameter_becomes_part_of_the_contract() {
    let report = type_report(concat!(
        "fn same<T>(left: T, right: T) -> Bool { left == right }\n",
        "fn same_again<U>(left: U, right: U) -> Bool { same(left, right) }\n",
        "fn smaller<T>(left: T, right: T) -> Bool { left < right }\n",
        "fn first<T>(left: T, right: T) -> T { left }\n",
        "pub fn main() -> Bool { same(1, 1) }\n",
    ));
    assert!(report.is_success(), "{}", report.render_diagnostics());
    assert_eq!(
        report.contract("main", "same").unwrap(),
        "fn<T: equatable>(T, T) -> Bool"
    );
    assert_eq!(
        report.contract("main", "same_again").unwrap(),
        "fn<U: equatable>(U, U) -> Bool"
    );
    assert_eq!(
        report.contract("main", "smaller").unwrap(),
        "fn<T: ordered>(T, T) -> Bool"
    );
    assert_eq!(report.contract("main", "first").unwrap(), "fn(T, T) -> T");
}

// E13: the call that instantiates the requirement with a function is T4305; it
// labels the operator use in the callee and the parameter, even through calls.
#[test]
fn instantiating_a_requirement_with_a_function_is_t4305() {
    let source = concat!(
        "fn same<T>(left: T, right: T) -> Bool { left == right }\n",
        "fn same_again<U>(left: U, right: U) -> Bool { same(left, right) }\n",
        "fn one() -> Int { 1 }\n",
        "pub fn main() -> List<Bool> { [same(one, one), same_again(one, one)] }\n",
    );
    let report = type_report(source);
    let diagnostics = report.diagnostics();
    assert_eq!(diagnostics.len(), 2, "{}", report.render_diagnostics());
    for diagnostic in diagnostics {
        assert_eq!(diagnostic.code(), "T4305");
        assert!(
            diagnostic.message().contains("it is a function"),
            "{}",
            diagnostic.message()
        );
        let labelled = diagnostic
            .labels()
            .iter()
            .map(|label| {
                let range = label.range();
                &source[range.start() as usize..range.end() as usize]
            })
            .collect::<Vec<_>>();
        assert_eq!(labelled[0], "left == right", "{labelled:?}");
        assert!(["T", "U"].contains(&labelled[1]), "{labelled:?}");
    }
}

// E14: a generic struct is equatable per instantiation.
#[test]
fn a_generic_struct_instantiated_with_data_is_equatable() {
    let outcome = run_both(
        "generic-equality",
        concat!(
            "struct Box<T> { value: T }\n",
            "fn same<T>(left: T, right: T) -> Bool { left == right }\n",
            "pub fn main() -> List<Bool> { [same(Box(1), Box(1)), same(Box(1), Box(2))] }\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":[true,false]");
}

fn ordering(variant: &str) -> String {
    format!("{{\"type\":\"Ordering\",\"variant\":\"{variant}\",\"fields\":{{}}}}")
}

// O8 and comparators: `Order.compare` is the structural order as an `Ordering`,
// and it is an ordinary function value that fits a comparator parameter.
#[test]
fn order_compare_returns_the_structural_ordering() {
    let source = concat!(
        "import std/order as Order\n",
        "import std/order.{Equal, Ordering}\n",
        "struct User { name: Text, age: Int }\n",
        "fn by_age_then_name(left: User, right: User) -> Ordering {\n",
        "  case Order.compare(left.age, right.age) {\n",
        "    Equal -> Order.compare(left.name, right.name)\n",
        "    other -> other\n",
        "  }\n",
        "}\n",
        "fn apply(comparator: fn(Int, Int) -> Ordering) -> Ordering { comparator(1, 0) }\n",
        "pub fn main() -> List<Ordering> {\n",
        "  [\n",
        "    Order.compare(3, 3),\n",
        "    Order.compare([1], [1, 0]),\n",
        "    by_age_then_name(User(\"b\", 30), User(\"a\", 30)),\n",
        "    by_age_then_name(User(\"a\", 30), User(\"b\", 31)),\n",
        "    apply(Order.compare),\n",
        "  ]\n",
        "}\n",
    );
    assert_eq!(
        run_both("order-compare", source),
        format!(
            "\"outcome\":\"success\",\"value\":[{},{},{},{},{}]",
            ordering("Equal"),
            ordering("Less"),
            ordering("Greater"),
            ordering("Less"),
            ordering("Greater")
        )
    );
}

// O17: `Order.min` and `Order.max` return the left operand when the two are
// equal; `-0.0` and `0.0` are equal but render differently, so the pick shows.
#[test]
fn order_min_and_max_keep_the_left_operand_on_ties() {
    let outcome = run_both(
        "order-min-max",
        concat!(
            "import std/order as Order\n",
            "pub fn main() -> List<Float> {\n",
            "  [Order.min(-0.0, 0.0), Order.min(0.0, -0.0), Order.max(-0.0, 0.0),\n",
            "   Order.min(2.5, 1.5), Order.max(1.5, 2.5)]\n",
            "}\n",
        ),
    );
    assert_eq!(
        outcome,
        "\"outcome\":\"success\",\"value\":[-0.0,0.0,-0.0,1.5,2.5]"
    );
}

// T4301 for `Order.compare`: its argument type must be ordered.
#[test]
fn order_compare_rejects_an_unordered_argument() {
    let diagnostics = coded_messages(concat!(
        "import std/order as Order\n",
        "import std/order.{Ordering}\n",
        "fn compare_records() -> Ordering { Order.compare({ a: 1 }, { a: 2 }) }\n",
        "pub fn main() -> Int { 1 }\n",
    ));
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].0, "T4301");
    assert!(
        diagnostics[0]
            .1
            .contains("`Order.compare` needs `A` to be ordered"),
        "{}",
        diagnostics[0].1
    );
}
