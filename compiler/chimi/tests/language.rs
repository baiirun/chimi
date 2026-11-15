//! Language-feature conformance through both execution paths.
//!
//! Every program here runs on the reference interpreter (`chimi run`) and as a
//! native AOT executable (`chimi build`). Both outcomes must be identical and
//! must match the expected outcome. Each program also round-trips through the
//! formatter and survives inspection at every byte offset.

use chimi::source::SourceSnapshot;
use chimi::{Compiler, CompilerSession, ProjectSource};
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
    let outcome = stdout
        .lines()
        .last()
        .unwrap_or_else(|| {
            panic!(
                "interpreter did not complete: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        })
        .to_owned();
    // `chimi run` reports every outcome on stdout and exits as a native runner
    // does for it; any other status means the compiler or interpreter failed.
    assert_eq!(
        output.status.code(),
        Some(outcome_exit_code(&outcome)),
        "interpreter exit does not match {outcome}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    outcome
}

/// The exit status runtime-output-channels.md assigns to an outcome record.
fn outcome_exit_code(outcome: &str) -> i32 {
    if outcome.contains("\"outcome\":\"success\"") {
        0
    } else if outcome.contains("\"outcome\":\"expected_failure\"") {
        1
    } else if outcome.contains("\"outcome\":\"cancellation\"") {
        130
    } else {
        70
    }
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
    // A signal or any code but the outcome's means the runtime itself crashed.
    assert_eq!(
        execution.status.code(),
        Some(outcome_exit_code(&outcome)),
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

/// Inspects the program at every character boundary. One analysis answers every
/// offset, as an editor session does: `IncrementalTypeReport::inspect` promises
/// the facts `Compiler::inspect_project` gives at that offset, without the
/// recheck that made one project check per byte the cost of this suite. The
/// sealing step it skips runs once, through `Compiler::inspect_project`.
fn assert_inspection_survives_every_offset(source: &str) {
    let module = || {
        ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
            .expect("valid module path")
    };
    let report = CompilerSession::new().analyze_project(vec![module()]);
    for offset in (0..=source.len()).filter(|&offset| source.is_char_boundary(offset)) {
        let _ = report.inspect("main", offset);
    }
    let _ = Compiler::inspect_project(vec![module()], "main", source.len());
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
            "import std/list as List\n",
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

// ts-4d9023: a function value argument may expose fewer failures and needs than
// its parameter's function type declares (executable-kernel, "Functions and
// Callable Contracts"), as a `using` provider may.
#[test]
fn a_function_argument_may_fail_with_fewer_variants_than_its_parameter_declares() {
    let outcome = run_both(
        "narrower-argument-row",
        concat!(
            "enum Oops {\n",
            "  Bad\n",
            "  Worse\n",
            "}\n",
            "fn run(f: fn() -> Int errors Oops) -> Int {\n",
            "  case f() {\n",
            "    Success(v) -> v\n",
            "    Failure(Bad) -> 100\n",
            "    Failure(Worse) -> 200\n",
            "  }\n",
            "}\n",
            "fn with_k<F>(k: fn(fn() -> Int errors F) -> Int errors F) -> Int { k(fn() { 4 }) }\n",
            "fn bad() -> Int { Failure(Oops.Bad) }\n",
            "pub fn main() -> Int {\n",
            "  run(fn() { 3 }) + run(bad) + with_k(fn(g) { g() + 1 })\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":108");
}

// A generic row parameter met by several function arguments is their union.
#[test]
fn a_generic_failure_row_bound_by_several_function_arguments_is_their_union() {
    let outcome = run_both(
        "union-argument-rows",
        concat!(
            "enum Oops {\n",
            "  Bad\n",
            "}\n",
            "fn zero() -> Int { 0 }\n",
            "fn failing() -> Int { Failure(Oops.Bad) }\n",
            "fn both<E>(f: fn() -> Int errors E, g: fn() -> Int errors E) -> Int { f() + g() }\n",
            "pub fn main() -> Int {\n",
            "  let first = case both(zero, failing) {\n",
            "    Success(v) -> v\n",
            "    Failure(Bad) -> 7\n",
            "  }\n",
            "  let second = case both(failing, zero) {\n",
            "    Success(v) -> v\n",
            "    Failure(Bad) -> 30\n",
            "  }\n",
            "  first + second\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":37");
}

#[test]
fn a_function_argument_may_need_fewer_dependencies_than_its_parameter_declares() {
    let outcome = run_both(
        "narrower-argument-needs",
        concat!(
            "interface Scale {\n",
            "  apply: fn(Int) -> Int\n",
            "}\n",
            "interface Offset {\n",
            "  add: fn(Int) -> Int\n",
            "}\n",
            "fn twice(step: fn(Int) -> Int needs { scale: Scale, offset: Offset }, value: Int) -> Int\n",
            "  needs { scale: Scale, offset: Offset } {\n",
            "  step(step(value))\n",
            "}\n",
            "fn scaled(value: Int) -> Int needs { scale: Scale } { scale.apply(value) }\n",
            "pub fn main() -> Int {\n",
            "  twice(scaled, 5) using {\n",
            "    scale: { apply: fn(value: Int) { value * 3 } },\n",
            "    offset: { add: fn(value: Int) { value + 1 } },\n",
            "  }\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":45");
}

// Soundness: a wider row is still rejected, whether the parameter's row is
// closed or a generic that another position fixes exactly.
#[test]
fn a_function_argument_with_a_wider_row_is_rejected() {
    let header = concat!(
        "enum Oops {\n",
        "  Bad\n",
        "}\n",
        "interface Scale {\n",
        "  apply: fn(Int) -> Int\n",
        "}\n",
        "fn failing() -> Int { Failure(Oops.Bad) }\n",
        "fn safe(g: fn() -> Int) -> Int { g() }\n",
        "fn scaled() -> Int needs { scale: Scale } { scale.apply(1) }\n",
    );
    let codes = |body: &str| check_codes(&format!("{header}{body}"));
    assert_eq!(
        codes("fn run(f: fn() -> Int) -> Int { f() }\npub fn main() -> Int { run(failing) }\n"),
        ["T4203"]
    );
    assert_eq!(
        codes("fn run(f: fn() -> Int) -> Int { f() }\npub fn main() -> Int { run(scaled) }\n"),
        ["T4203"]
    );
    // The handler fixes `E` as the empty row, so `failing` cannot widen it.
    assert_eq!(
        codes(concat!(
            "fn apply<E>(handler: fn(fn() -> Int errors E) -> Int, f: fn() -> Int errors E) -> Int {\n",
            "  handler(f)\n",
            "}\n",
            "pub fn main() -> Int { apply(safe, failing) }\n",
        )),
        ["T4203"]
    );
    assert_eq!(
        codes(concat!(
            "fn apply<E>(f: fn() -> Int errors E, handler: fn(fn() -> Int errors E) -> Int) -> Int {\n",
            "  handler(f)\n",
            "}\n",
            "pub fn main() -> Int { apply(failing, safe) }\n",
        )),
        ["T4203"]
    );
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
            "import std/text as Text\n",
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

// N10. `1.` and `.5` are not literals, so the parser reports what follows them: a
// field name after `1.`, an expression at `.5`. The rest are literal diagnostics.
#[test]
fn malformed_number_literals_are_syntax_errors() {
    const SEPARATOR: &str = "S1001 `_` may appear only between two digits of a number";
    const LEADING_ZERO: &str =
        "S1001 a decimal literal cannot start with `0`; Chimi has no octal literals";
    const HEXADECIMAL: &str = "S1001 a hexadecimal literal needs digits 0-9 and a-f after `0x`";
    for (literal, expected) in [
        ("1.", "S1001 expected identifier"),
        (".5", "S1001 expected expression"),
        ("1__0", SEPARATOR),
        ("_1", SEPARATOR),
        ("1_", SEPARATOR),
        ("1.5_", SEPARATOR),
        ("0x", HEXADECIMAL),
        ("0xG", HEXADECIMAL),
        (
            "0b102",
            "S1001 a binary literal needs digits 0 and 1 after `0b`",
        ),
        ("017", LEADING_ZERO),
        ("0_17", LEADING_ZERO),
        ("0_0_7", LEADING_ZERO),
        ("0_1", LEADING_ZERO),
    ] {
        let source = format!("pub fn main() -> Int {{ {literal} }}\n");
        let module = ProjectSource::new(
            "main",
            SourceSnapshot::from_text("main.chimi", source.as_str()),
        )
        .expect("valid module path");
        let diagnostics = Compiler::type_project(vec![module])
            .diagnostics()
            .iter()
            .map(|diagnostic| format!("{} {}", diagnostic.code(), diagnostic.message()))
            .collect::<Vec<_>>();
        assert_eq!(
            diagnostics.first().map(String::as_str),
            Some(expected),
            "{literal}: {diagnostics:?}"
        );
    }
}

// N12
#[test]
fn a_float_literal_that_rounds_to_infinity_is_rejected() {
    assert_eq!(check_codes("pub fn main() -> Float { 1e309 }\n"), ["S1003"]);
    assert!(check_codes("pub fn main() -> Float { 1e-400 }\n").is_empty());
    // The range check applies to pattern literals too, besides T4313 for any Float
    // pattern.
    let mut codes = check_codes(concat!(
        "pub fn main() -> Int {\n",
        "  case 1.5 {\n",
        "    1e309 -> 1\n",
        "    _ -> 0\n",
        "  }\n",
        "}\n",
    ));
    codes.sort();
    assert_eq!(codes, ["S1003", "T4313"]);
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

const DECIMAL_IMPORTS: &str = "import std/decimal as Decimal\nimport std/decimal.{Decimal}\n";

// N26: division needs a scale and a rounding mode, so `/` and `%` are not
// `Decimal` operators, whichever side the `Decimal` is on.
#[test]
fn decimal_division_operators_are_rejected_with_a_divide_hint() {
    for body in [
        "Decimal.of(1999, 2) / Decimal.of(4, 0)",
        "Decimal.of(1999, 2) % Decimal.of(4, 0)",
        "price(Decimal.of(1, 0)) / Decimal.of(4, 0)",
    ] {
        let source = format!(
            "{DECIMAL_IMPORTS}fn price(value: Decimal) -> Decimal {{ value }}\npub fn main() -> Decimal {{ {body} }}\n"
        );
        let messages = check_messages(&source);
        assert_eq!(messages.len(), 1, "{body}: {messages:?}");
        assert!(
            messages[0].starts_with("T4306") && messages[0].contains("Decimal.divide"),
            "{body}: {messages:?}"
        );
    }
}

// `+ - *`, unary `-`, and comparisons type on two `Decimal`s; an `Int` beside a
// `Decimal` is not converted, and the hint names `Decimal.of`, not `Float`.
#[test]
fn decimal_operators_take_two_decimals() {
    let source = format!(
        "{DECIMAL_IMPORTS}pub fn main() -> Bool {{\n  let total = -(Decimal.of(1, 0) + Decimal.of(2, 1) * Decimal.of(3, 0) - Decimal.of(4, 2))\n  total < Decimal.of(0, 0) and total != Decimal.of(5, 1)\n}}\n"
    );
    assert_eq!(check_messages(&source), Vec::<String>::new());
    let messages = check_messages(&format!(
        "{DECIMAL_IMPORTS}pub fn main() -> Decimal {{ Decimal.of(1, 0) + 1 }}\n"
    ));
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].starts_with("T4302") && messages[0].contains("Decimal.of(value, 0)"),
        "{messages:?}"
    );
    // A `Float` has no exact conversion, so the `Int` hint would mislead.
    for body in ["Decimal.of(1, 0) * 2.5", "2.5 - Decimal.of(1, 0)"] {
        let messages = check_messages(&format!(
            "{DECIMAL_IMPORTS}pub fn main() -> Decimal {{ {body} }}\n"
        ));
        assert_eq!(messages.len(), 1, "{body}: {messages:?}");
        assert!(
            messages[0].starts_with("T4302")
                && messages[0].contains("no conversion from `Float`")
                && !messages[0].contains("Decimal.of(value, 0)")
                && !messages[0].contains("Float.from_int"),
            "{body}: {messages:?}"
        );
    }
}

// A `Decimal` is plain data: a task's result crosses heaps with its sign,
// its magnitude past 64 bits, and its scale intact. Native only: the
// interpreter has no task runtime.
#[test]
fn a_decimal_task_result_crosses_heaps_intact() {
    let scratch = scratch_directory("decimal-task-result");
    let path = scratch.join("main.chimi");
    std::fs::write(
        &path,
        format!(
            "import std/task as Task\nimport std/task.{{Task}}\n{DECIMAL_IMPORTS}\
             fn work() -> Decimal {{ Decimal.of(9223372036854775807, 19) * Decimal.of(-9223372036854775807, 19) }}\n\
             pub fn main() -> Text {{ Decimal.to_text(Task.result(Task.start(work))) }}\n"
        ),
    )
    .unwrap();
    let outcome = native_outcome(&scratch, &path);
    assert!(
        outcome.ends_with(
            "\"outcome\":\"success\",\"value\":\"-0.85070591730234615847396907784232501249\"}"
        ),
        "{outcome}"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn a_bytes_task_result_crosses_heaps_intact() {
    let scratch = scratch_directory("bytes-task-result");
    let path = scratch.join("main.chimi");
    std::fs::write(
        &path,
        "import std/task as Task\nimport std/bytes as Bytes\nimport std/bytes.{Bytes}\n\
         fn work() -> Bytes { Bytes.concat([Bytes.from([0xca, 0x00, 0xfe]), Bytes.from_text(\"task\")]) }\n\
         pub fn main() -> Bytes { Task.result(Task.start(work)) }\n",
    )
    .unwrap();
    let outcome = native_outcome(&scratch, &path);
    assert!(
        outcome.ends_with("\"outcome\":\"success\",\"value\":{\"bytes\":\"ca00fe7461736b\"}}"),
        "{outcome}"
    );
    std::fs::remove_dir_all(scratch).unwrap();
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
            concat!(
                "pub fn main() -> List<Int> {\n",
                "  [Int.parse(\"42\"), Int.parse(\"-9223372036854775808\"),\n",
                "   Int.parse(\"007\"), Int.parse(\"-0017\")]\n",
                "}",
            )
        ),
        "\"outcome\":\"success\",\"value\":[42,-9223372036854775808,7,-17]"
    );
    for (text, variant) in [
        ("", "Empty"),
        ("-", "InvalidDigit"),
        ("4x", "InvalidDigit"),
        ("+4", "InvalidDigit"),
        (" 4", "InvalidDigit"),
        ("99999999999999999999", "OutOfRange"),
        // U+0000 is not a digit; a C string would have stopped before it.
        ("4\\u{0}", "InvalidDigit"),
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
            concat!(
                "pub fn main() -> List<Float> {\n",
                "  [Float.parse(\"-2.5e-3\"), Float.parse(\"7\"), Float.parse(\"007\"),\n",
                "   Float.parse(\"0017.25\"), Float.parse(\"-0017\")]\n",
                "}",
            )
        ),
        "\"outcome\":\"success\",\"value\":[-0.0025,7.0,7.0,17.25,-17.0]"
    );
    for (text, variant) in [
        ("", "Empty"),
        ("-", "Invalid"),
        ("nan", "Invalid"),
        ("inf", "Invalid"),
        ("1.", "Invalid"),
        ("+1.0", "Invalid"),
        ("1e400", "OutOfRange"),
        ("1.5\\u{0}", "Invalid"),
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

// ts-ce73fb: Text may hold U+0000. Native Text was a C string, so every operation
// stopped at the first NUL byte and disagreed with the interpreter.
#[test]
fn text_holding_nul_keeps_every_byte_on_both_backends() {
    let outcome = run_both(
        "text-nul",
        concat!(
            "import std/text as Text\n",
            "import std/list as List\n",
            "import std/debug as Debug\n",
            "import std/json as Json\n",
            "import std/json.{Value}\n",
            "import std/duration as Duration\n",
            "fn flag(value: Bool) -> Text {\n",
            "  case value {\n",
            "    True -> \"T\"\n",
            "    False -> \"F\"\n",
            "  }\n",
            "}\n",
            "fn tag(text: Text) -> Text {\n",
            "  case text {\n",
            "    \"a\\u{0}b\" -> \"exact\"\n",
            "    _ -> \"other\"\n",
            "  }\n",
            "}\n",
            "fn duration(text: Text) -> Text {\n",
            "  case Duration.parse(text) {\n",
            "    Success(_) -> \"parsed\"\n",
            "    Failure(_) -> \"rejected\"\n",
            "  }\n",
            "}\n",
            "fn decoded_length(source: Text) -> Int {\n",
            "  case Json.decode(source) {\n",
            "    Success(Value.Text(text)) -> Text.byte_length(text)\n",
            "    Success(_) -> -1\n",
            "    Failure(_) -> -2\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Text {\n",
            "  let text = \"a\\u{0}b\"\n",
            "  let joined = Text.concat([text, \"c\"])\n",
            "  let parts = Text.split(joined, \"\\u{0}\")\n",
            "  [\n",
            "    \"${Text.byte_length(text)}\",\n",
            "    \"${Text.byte_length(joined)}\",\n",
            "    \"${List.length(parts)}\",\n",
            "    Text.replace(joined, \"\\u{0}\", \"-\"),\n",
            "    Text.replace(\"a\\u{0}b\", \"a\", \"z\"),\n",
            "    Text.join(Text.split(\"x,\\u{0}y\", \",\"), \"+\"),\n",
            "    \"${Text.byte_length(Text.trim(\" a\\u{0} \"))}\",\n",
            "    flag(text == \"a\\u{0}c\"),\n",
            "    flag(\"a\" == \"a\\u{0}\"),\n",
            "    flag(Text.contains(text, \"b\")),\n",
            "    flag(Text.starts_with(text, \"a\\u{0}b\")),\n",
            "    flag(Text.ends_with(joined, \"bc\")),\n",
            "    tag(text),\n",
            "    tag(\"a\\u{0}c\"),\n",
            "    Debug.render(text),\n",
            "    duration(\"1s\\u{0}\"),\n",
            "    Json.encode(Json.text(text)),\n",
            "    \"${decoded_length(\"\\\"x\\\\u0000y\\\"\")}\",\n",
            "    joined,\n",
            "  ] |> Text.join(\"|\")\n",
            "}\n",
        ),
    );
    assert_eq!(
        outcome,
        concat!(
            "\"outcome\":\"success\",\"value\":",
            "\"3|4|2|a-bc|z\\u0000b|x+\\u0000y|2|F|F|T|T|T|exact|other|\\\"a\\\\u{0}b\\\"|rejected|",
            "\\\"a\\\\u0000b\\\"|3|a\\u0000bc\""
        )
    );
}

// The std/int and std/float failure types are module members: a program names
// them through the module alias in types, `errors` rows, expressions, and
// patterns, and they are closed enums for coverage.
#[test]
fn std_number_failure_types_are_named_through_their_module() {
    assert_eq!(
        std_outcome(
            "number-failure-types",
            concat!(
                "fn parse_with(parse: fn(Text) -> Float errors Float.ParseError, text: Text) -> Text {\n",
                "  case parse(text) {\n",
                "    Success(_) -> \"ok\"\n",
                "    Failure(Float.ParseError.OutOfRange) -> \"range\"\n",
                "    Failure(_) -> \"other\"\n",
                "  }\n",
                "}\n",
                "fn describe(error: Int.ParseError) -> Text {\n",
                "  case error {\n",
                "    Int.ParseError.Empty -> \"empty\"\n",
                "    Int.ParseError.InvalidDigit -> \"digit\"\n",
                "    Int.ParseError.OutOfRange -> \"range\"\n",
                "  }\n",
                "}\n",
                "fn to_int(value: Float) -> Text {\n",
                "  case Float.to_int(value) {\n",
                "    Success(_) -> \"int\"\n",
                "    Failure(Float.ConversionError.NotInteger) -> \"fraction\"\n",
                "    Failure(Float.ConversionError.OutOfRange) -> \"range\"\n",
                "  }\n",
                "}\n",
                "pub fn main() -> List<Text> {\n",
                "  let empty = Int.ParseError.Empty\n",
                "  [parse_with(Float.parse, \"1e400\"), parse_with(Float.parse, \"x\"),\n",
                "   describe(empty), describe(Int.ParseError.InvalidDigit), to_int(2.5)]\n",
                "}",
            )
        ),
        "\"outcome\":\"success\",\"value\":[\"range\",\"other\",\"empty\",\"digit\",\"fraction\"]"
    );
    assert_eq!(
        check_codes(concat!(
            "import std/float as Float\n",
            "pub fn main() -> Int {\n",
            "  let error = Float.ParseError.Missing\n",
            "  0\n",
            "}\n",
        )),
        ["E_UNRESOLVED_MEMBER"]
    );
}

// std/io failure types are closed enums for coverage: a case that names every
// variant is exhaustive, and one that omits a variant names it as the witness.
#[test]
fn std_io_failure_types_are_closed_for_coverage() {
    let module = |file_arms: &str| {
        format!(
            concat!(
                "import std/io as Io\n",
                "fn file(error: Io.FileError) -> Text {{\n",
                "  case error {{\n{file_arms}  }}\n",
                "}}\n",
                "fn utf8(error: Io.Utf8Error) -> Text {{\n",
                "  case error {{\n",
                "    Io.Utf8Error.InvalidUtf8(_, _) -> \"invalid\"\n",
                "  }}\n",
                "}}\n",
                "fn console(error: Io.ConsoleError) -> Text {{\n",
                "  case error {{\n",
                "    Io.ConsoleError.WriteFailed(_, _) -> \"write\"\n",
                "  }}\n",
                "}}\n",
                "pub fn main() -> Int {{ 0 }}\n",
            ),
            file_arms = file_arms
        )
    };
    let every_file_variant = concat!(
        "    Io.FileError.Open(_, _) -> \"open\"\n",
        "    Io.FileError.Read(_, _) -> \"read\"\n",
        "    Io.FileError.Close(_, _) -> \"close\"\n",
        "    Io.FileError.LineTooLong(_, _) -> \"long\"\n",
    );
    assert!(check_codes(&module(every_file_variant)).is_empty());
    let source =
        module(&every_file_variant.replace("    Io.FileError.Close(_, _) -> \"close\"\n", ""));
    let module = ProjectSource::new(
        "main",
        SourceSnapshot::from_text("main.chimi", source.as_str()),
    )
    .expect("valid module path");
    let diagnostics = Compiler::type_project(vec![module])
        .diagnostics()
        .iter()
        .map(|diagnostic| format!("{} {}", diagnostic.code(), diagnostic.message()))
        .collect::<Vec<_>>();
    assert_eq!(
        diagnostics,
        ["T4231 case is not exhaustive; `Close(_, _)` is not matched"]
    );
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
                "   Float.to_text(1.7976931348623157e308), Float.to_text(100.0), Float.to_text(0.1),\n",
                // Each value lies exactly halfway between its two shortest texts that
                // parse back; the spec picks the larger.
                "   Float.to_text(0.000221729278564453125), Float.to_text(9499534713890.8125)]\n",
                "}",
            )
        ),
        concat!(
            "\"outcome\":\"success\",\"value\":[\"-0.0\",\"0.0\",\"5e-324\",",
            "\"1.7976931348623157e308\",\"100.0\",\"0.1\",",
            "\"0.00022172927856445313\",\"9499534713890.813\"]"
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
            "import std/text as Text\n",
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

// LP-04. The rejected pattern does not count toward coverage, so a later
// catch-all is not unreachable and a case with no other arm is not reported
// as missing lists.
#[test]
fn a_list_pattern_with_two_rests_is_rejected() {
    for arms in [
        &["[a, ..x, ..y] -> a", "[] -> 0"][..],
        &["[.., ..] -> 1", "_ -> 0"],
        &["[.., ..] -> 1"],
    ] {
        assert_eq!(check_codes(&list_case(arms)), ["L2021"], "{arms:?}");
    }
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
            "import std/list as List\n",
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
            "import std/list as List\n",
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
            "import std/list as List\n",
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

// The formatter keeps a `loop` or `case` body brace on the line of a record header,
// so the formatted program still parses and computes the same value.
#[test]
fn record_headers_keep_their_body_brace_when_formatted() {
    let outcome = run_both(
        "record-loop-header",
        concat!(
            "fn three() -> Int { 3 }\n",
            "fn count() -> Int {\n",
            "  loop s = { a: 1 } {\n",
            "    case s.a > 3 {\n",
            "      True -> break s.a\n",
            "      False -> { a: s.a + 1 }\n",
            "    }\n",
            "  }\n",
            "}\n",
            "fn sum() -> Int {\n",
            "  loop s = { k: 2, h: three() } { break s.k + s.h }\n",
            "}\n",
            "fn first(n: Int) -> Int {\n",
            "  case { a: n, b: 2 } {\n",
            "    r -> r.a * 10 + r.b\n",
            "  }\n",
            "}\n",
            "pub fn main() -> List<Int> { [count(), sum(), first(1)] }\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":[4,5,12]");
}

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
            "import std/text as Text\n",
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

// Guards: each pass builds closures that capture the previous pass's closure
// through several locals. The interpreter copied captured environments by value,
// so the copy doubled with every pass: 8 passes took 33 s. 40 passes now finish
// in well under a second.
#[test]
fn closures_capturing_earlier_closures_stay_cheap_to_copy() {
    let started = std::time::Instant::now();
    let outcome = run_both(
        "closure-chain",
        concat!(
            "fn run(n: Int) -> Int {\n",
            "  let start = { left: n, f: fn(x: Int) { x } }\n",
            "  loop state = start {\n",
            "    case state.left {\n",
            "      0 -> break state.f(0)\n",
            "      _ -> {\n",
            "        let g = state.f\n",
            "        let first = { k: 2, h: g }\n",
            "        let inner = loop s = first {\n",
            "          case s.k {\n",
            "            0 -> break s\n",
            "            _ -> { k: s.k - 1, h: fn(y: Int) { y + s.k } }\n",
            "          }\n",
            "        }\n",
            "        { left: state.left - 1, f: inner.h }\n",
            "      }\n",
            "    }\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int { run(40) }\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":1");
    let elapsed = started.elapsed();
    assert!(elapsed.as_secs() < 60, "40 passes took {elapsed:?}");
}

// LP-07 on both paths. The reference interpreter records a full trace of every
// operation, so this runs at 2^14 elements; `tests/cli.rs` checks the
// 1,000,000-element row natively, where allocation is measurable.
#[test]
fn the_last_element_pattern_reads_one_element_of_a_large_list() {
    let outcome = run_both("list-pattern-last", &last_element_program(14, 200));
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":200");
}

// ts-1bba2b: an element type that must contain itself has no finite type. The
// mismatch is reported instead of building `T = List<T>` and recursing forever.
#[test]
fn a_type_that_must_contain_itself_is_a_mismatch() {
    assert_eq!(
        check_codes(concat!(
            "pub fn main() -> List<Int> {\n",
            "  loop out = [] {\n",
            "    case out {\n",
            "      [] -> [out]\n",
            "      _ -> break out\n",
            "    }\n",
            "  }\n",
            "}\n",
        )),
        ["T4203"]
    );
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

// Value properties: inspection shows whether the value at the cursor is
// equatable and ordered, in the report and in its JSON.
#[test]
fn inspection_shows_equatable_and_ordered_properties() {
    use chimi::ValuePropertyStatus::{Always, Never, ValueDependent};
    let source = concat!(
        "import std/map.{Map}\n",
        "fn one() -> Int { 1 }\n",
        "fn f<T>(table: Map<Text, Int>, values: List<Int>, function: fn() -> Int, item: T) -> Int {\n",
        "  let number = 1\n",
        "  let t = table\n",
        "  let r = values\n",
        "  let g = function\n",
        "  let i = item\n",
        "  number\n",
        "}\n",
        "pub fn main() -> Int { 1 }\n",
    );
    for (name, equatable, ordered) in [
        ("number\n}", Always, Always),
        ("= table", Always, Never),
        ("= values", Always, Always),
        ("= function", Never, Never),
        ("= item", ValueDependent, ValueDependent),
    ] {
        let offset = source.find(name).unwrap() + name.find(char::is_alphabetic).unwrap();
        let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
            .expect("valid module path");
        let inspection = Compiler::inspect_project(vec![module], "main", offset).unwrap();
        let properties = inspection
            .value_properties()
            .unwrap_or_else(|| panic!("{name}: no value properties"));
        assert_eq!(
            (properties.equatable, properties.ordered),
            (equatable, ordered),
            "{name}"
        );
        let json = inspection.render_json();
        let expected = |status| match status {
            Always => "always",
            Never => "never",
            ValueDependent => "value_dependent",
        };
        let fragment = format!(
            "\"value_properties\":{{\"equatable\":\"{}\",\"ordered\":\"{}\"}}",
            expected(equatable),
            expected(ordered)
        );
        assert!(json.contains(&fragment), "{name}: {json}");
    }
}

#[test]
fn hovering_a_list_pattern_binding_reports_its_type() {
    let source = concat!(
        "import std/list as List\n",
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

// ts-fd591d: a type an inferred contract never constrains is generic in that
// contract, so each caller instantiates it at its own type.
#[test]
fn an_inferred_contract_is_generic_in_the_types_its_body_leaves_open() {
    let outcome = run_both(
        "inferred-contract-generic",
        concat!(
            "fn nothing<T>() -> Option<T> { None }\n",
            "fn wrapper() { nothing() }\n",
            "fn pass(value) { value }\n",
            "fn count(o: Option<Int>) -> Int {\n",
            "  case o {\n",
            "    Some(v) -> v\n",
            "    None -> 0\n",
            "  }\n",
            "}\n",
            "fn flag(o: Option<Bool>) -> Int {\n",
            "  case o {\n",
            "    Some(_) -> 1\n",
            "    None -> 2\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  count(wrapper()) + flag(wrapper()) + count(pass(Some(40))) + flag(pass(Some(True)))\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":43");
}

// An open type inside a tuple is generalized like one inside a record.
#[test]
fn an_inferred_contract_is_generic_in_an_open_tuple_element() {
    let outcome = run_both(
        "inferred-contract-tuple",
        concat!(
            "fn pair(value) { (value, 1) }\n",
            "pub fn main() -> Int {\n",
            "  let (n, a) = pair(40)\n",
            "  let (flag, b) = pair(True)\n",
            "  case flag {\n",
            "    True -> n + a + b\n",
            "    False -> 0\n",
            "  }\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":42");
}

// Each caller's instantiation is still checked: one shared open variable
// keeps input and result tied together.
#[test]
fn an_inferred_generic_contract_keeps_its_input_and_result_tied() {
    assert_eq!(
        check_codes(concat!(
            "fn pass(value) { value }\n",
            "fn count(o: Option<Int>) -> Int {\n",
            "  case o {\n",
            "    Some(v) -> v\n",
            "    None -> 0\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int { count(pass(Some(True))) }\n",
        )),
        ["T4203"]
    );
    // A recursive call sees the result type its body settles on.
    assert_eq!(
        check_codes(concat!(
            "fn needs_text(t: Text) -> Int { 0 }\n",
            "fn f(b: Bool) {\n",
            "  case b {\n",
            "    True -> 1\n",
            "    False -> needs_text(f(True))\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int { f(False) }\n",
        )),
        ["T4203"]
    );
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

// ts-3af9ef: generic calls that the checker once rejected with T4203 or T4205.
// Each is well typed, so each must run to the same outcome on both backends.
#[test]
fn generic_calls_with_function_arguments_bind_their_parameters_in_any_order() {
    let declarations = concat!(
        "enum Oops {\n",
        "  Bad\n",
        "}\n",
        "struct Money {\n",
        "  cents: Int\n",
        "}\n",
        "fn run<E>(operation: fn() -> Int errors E) -> Int { operation() }\n",
        "fn failing() -> Int { Failure(Oops.Bad) }\n",
        // A self-recursive call passes its own `F` to a row-polymorphic value.
        "fn apply<F>(n: Int, f: fn(fn() -> Int errors F) -> Int errors F, op: fn() -> Int errors F) -> Int {\n",
        "  case n {\n",
        "    0 -> f(op)\n",
        "    _ -> apply(n - 1, run, op)\n",
        "  }\n",
        "}\n",
        "fn with_k<F>(k: fn(fn() -> Int errors F) -> Int errors F, op: fn() -> Int errors F) -> Int { k(op) }\n",
        "fn compose<A, B>(a: A, f: fn(A) -> B, g: fn(B) -> Int) -> Int { g(f(a)) }\n",
        "fn later<T>(op: fn(T) -> Int, make: fn() -> T) -> Int { op(make()) }\n",
    );
    let outcome = |name: &str, body: &str| {
        run_both(
            name,
            &format!("{declarations}pub fn main() -> Int {{\n  {body}\n}}\n"),
        )
    };
    let bad = "\"outcome\":\"expected_failure\",\"error\":{\"type\":\"Oops\",\"variant\":\"Bad\",\"fields\":{}}";
    // The recursive call's slot keeps the row bound by `op`.
    assert_eq!(outcome("recursive-row-slot", "apply(2, run, failing)"), bad);
    // A lambda's parameter takes the row a later lambda binds, never the callee's `F`.
    assert_eq!(
        outcome(
            "lambda-row-from-later-lambda",
            "with_k(fn(g) { g() + 1 }, fn() { Failure(Oops.Bad) })"
        ),
        bad
    );
    assert_eq!(
        outcome(
            "lambda-row-handled",
            "case with_k(fn(g) { g() + 1 }, fn() { Failure(Oops.Bad) }) {\n    Success(v) -> v\n    Failure(Bad) -> 9\n  }"
        ),
        "\"outcome\":\"success\",\"value\":9"
    );
    assert_eq!(
        outcome("lambda-empty-row", "with_k(fn(g) { g() + 1 }, fn() { 5 })"),
        "\"outcome\":\"success\",\"value\":6"
    );
    // One lambda's result types the next lambda's parameter, in either order.
    assert_eq!(
        outcome(
            "lambda-result-forward",
            "compose(3, fn(x) { Money(x) }, fn(m) { m.cents })"
        ),
        "\"outcome\":\"success\",\"value\":3"
    );
    assert_eq!(
        outcome(
            "lambda-result-backward",
            "later(fn(m) { m.cents }, fn() { Money(4) })"
        ),
        "\"outcome\":\"success\",\"value\":4"
    );
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
            "import std/text as Text\n",
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

// G7 row: `const a = f()` with `fn f() -> Int { a }` is one cycle through the
// function, reported once with its path.
#[test]
fn a_constant_reached_through_its_own_initializer_call_is_a_cycle() {
    let messages = build_messages(concat!(
        "const a = f()\n",
        "fn f() -> Int { a }\n",
        "pub fn main() -> Int { a }\n",
    ));
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].starts_with("E5205") && messages[0].contains("a -> f -> a"),
        "{messages:?}"
    );
}

// Modules that import each other are rejected by the compiler API with the path, as
// the project loader rejects them for the CLI. This is also why two modules cannot
// name each other's constants.
#[test]
fn modules_that_import_each_other_are_an_import_cycle() {
    let sources = [
        (
            "app/main",
            "import lib/other as Other\npub fn main() -> Int { Other.value() }\npub fn base() -> Int { 1 }\n",
        ),
        (
            "lib/other",
            "import app/main as Main\npub fn value() -> Int { Main.base() }\n",
        ),
    ]
    .into_iter()
    .map(|(module, source)| {
        ProjectSource::new(
            module,
            SourceSnapshot::from_text(format!("{module}.chimi"), source),
        )
        .expect("valid module path")
    })
    .collect();
    let diagnostics = Compiler::verify_project(sources).expect_err("an import cycle fails");
    let [diagnostic] = diagnostics.as_slice() else {
        panic!("{diagnostics:?}");
    };
    assert_eq!(diagnostic.code(), "E_IMPORT_CYCLE");
    assert_eq!(
        diagnostic.message(),
        "import cycle: app/main -> lib/other -> app/main"
    );
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

// G7: a build-time defect reports the span of the operation that failed, inside the
// called function, and names the constant being built.
#[test]
fn a_constant_defect_points_at_the_failing_operation() {
    let source = concat!(
        "fn first_or_defect(values: List<Int>) -> Int {\n",
        "  case values {\n    [first, ..] -> first\n    [] -> 1 / 0\n  }\n}\n",
        "const first = first_or_defect([])\n",
        "pub fn main() -> Int { 0 }\n",
    );
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    let diagnostics = Compiler::verify_project(vec![module]).expect_err("the constant defects");
    let [diagnostic] = diagnostics.as_slice() else {
        panic!("{diagnostics:?}");
    };
    assert_eq!(diagnostic.code(), "E5202");
    let range = diagnostic.range();
    assert_eq!(
        &source[range.start() as usize..range.end() as usize],
        "1 / 0"
    );
    assert!(
        diagnostic.message().contains("constant `first`"),
        "{}",
        diagnostic.message()
    );
}

fn counting_constant(depth: u32) -> String {
    format!(
        concat!(
            "fn count(n: Int) -> Int {{\n",
            "  case n {{\n    0 -> 0\n    _ -> 1 + count(n - 1)\n  }}\n",
            "}}\n",
            "const counted = count({})\n",
            "pub fn main() -> Int {{ counted }}\n",
        ),
        depth
    )
}

// G7 E5203 call depth: `count(9_999)` nests exactly the 10 000 calls a constant may
// nest, so it builds and loads its value on both backends; one more call is E5203.
#[test]
fn a_constant_may_nest_calls_up_to_the_call_depth_budget() {
    let outcome = run_both("constant-call-depth", &counting_constant(9_999));
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":9999");
    let messages = build_messages(&counting_constant(10_000));
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].starts_with("E5203") && messages[0].contains("10000 calls"),
        "{messages:?}"
    );
}

// G7 E5203: recursion that never ends stops at the call depth budget instead of
// overflowing the compiler's stack.
#[test]
fn unbounded_recursion_in_a_constant_is_e5203() {
    let codes = build_codes(&[(
        "main",
        concat!(
            "fn spin(n: Int) -> Int { spin(n + 1) }\n",
            "const spun = spin(0)\n",
            "pub fn main() -> Int { 0 }\n",
        ),
    )]);
    assert_eq!(codes, ["E5203"]);
}

// G7 E5203 size: a value that doubles each step by holding two copies of itself, or a
// text appended to itself, would reach terabytes; evaluation stops near the 16 MiB
// budget rather than building and measuring the whole value.
#[test]
fn a_constant_that_doubles_each_step_stops_at_the_size_budget() {
    let tree = concat!(
        "enum Tree {\n  Leaf\n  Node(left: Tree, right: Tree)\n}\n",
        "fn grow(limit: Int) -> Tree {\n",
        "  loop state = { n: 0, value: Leaf } {\n",
        "    case state.n < limit {\n",
        "      True -> { n: state.n + 1, value: Node(state.value, state.value) }\n",
        "      False -> break state.value\n",
        "    }\n",
        "  }\n",
        "}\n",
        "const big = grow(40)\n",
        "pub fn main() -> Int { 0 }\n",
    );
    let text = concat!(
        "import std/text as Text\n",
        "fn grow(limit: Int) -> Text {\n",
        "  loop state = { n: 0, value: \"a\" } {\n",
        "    case state.n < limit {\n",
        "      True -> { n: state.n + 1, value: Text.append(state.value, state.value) }\n",
        "      False -> break state.value\n",
        "    }\n",
        "  }\n",
        "}\n",
        "const big = grow(40)\n",
        "pub fn main() -> Int { 0 }\n",
    );
    for source in [tree, text] {
        // Bounded, so a regression fails instead of exhausting memory.
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || sender.send(build_messages(source)));
        let messages = receiver
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("evaluation stops within a minute");
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(
            messages[0].starts_with("E5203") && messages[0].contains("16 MiB"),
            "{messages:?}"
        );
    }
}

// G7 rule 3 and Migration: a function configured with `using` over constant
// providers is a constant, whether it is a declared function or a closure, and calls
// through the stored providers on both backends.
#[test]
fn a_function_configured_with_constant_providers_is_a_constant() {
    let outcome = run_both(
        "constant-configured-function",
        concat!(
            "interface Clock {\n  now: fn() -> Int\n}\n",
            "const fake = { now: fn() -> Int { 41 } }\n",
            "fn timestamp() -> Int needs { clock: Clock } { clock.now() }\n",
            "const configured = timestamp using { clock: fake }\n",
            "const later = (fn() -> Int needs { clock: Clock } { clock.now() + 1 }) using {\n",
            "  clock: fake,\n",
            "}\n",
            "const both = [configured, later]\n",
            "fn call_all(functions: List<fn() -> Int>) -> Int {\n",
            "  case functions {\n",
            "    [first, ..rest] -> first() + call_all(rest)\n",
            "    [] -> 0\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  configured() * 1000 + later() + call_all(both)\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":41125");
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
            "import std/text as Text\n",
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
            "{\\\"a\\\": 1};apay3;\""
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
    // An interpolation must close on its own line, even inside a delimited expression.
    assert!(codes("\"sum: ${Text.append(\"a\",\n\"b\")}\"").contains(&"S1101".to_owned()));
}

// G9: a text pattern is a literal and cannot interpolate; the whole pattern is one
// S1101, with no cascade of token or coverage errors.
#[test]
fn an_interpolated_text_pattern_is_one_diagnostic() {
    let source = concat!(
        "pub fn main() -> Int {\n",
        "  case \"a1\" {\n",
        "    \"a${1}b${2}\" -> 1\n",
        "    \"a\" -> 2\n",
        "    _ -> 0\n",
        "  }\n",
        "}\n",
    );
    assert_eq!(check_codes(source), ["S1101"]);
}

// G9: interpolated parts evaluate left to right, so the first failing part decides
// the failure.
#[test]
fn interpolated_parts_evaluate_left_to_right() {
    let outcome = run_both(
        "interpolation-order",
        concat!(
            "enum Problem {\n",
            "  First\n",
            "  Second\n",
            "}\n",
            "fn a() -> Text { Failure(First) }\n",
            "fn b() -> Text { Failure(Second) }\n",
            "pub fn main() -> Text {\n",
            "  \"${a()} ${b()}\"\n",
            "}\n",
        ),
    );
    assert_eq!(
        outcome,
        concat!(
            "\"outcome\":\"expected_failure\",",
            "\"error\":{\"type\":\"Problem\",\"variant\":\"First\",\"fields\":{}}"
        )
    );
}

// G9: an interpolated provider call adds its need to the enclosing function's row,
// so a lambda that interpolates one must declare the need.
#[test]
fn interpolated_needs_join_the_enclosing_row() {
    let callable = |parameter: &str| {
        format!(
            concat!(
                "interface Names {{\n",
                "  lookup: fn(Int) -> Text\n",
                "}}\n",
                "fn call(operation: {}) -> Text {{ operation() }}\n",
                "fn greet() -> Text needs {{ names: Names }} {{\n",
                "  call(fn() {{ \"hello ${{names.lookup(5)}}\" }})\n",
                "}}\n",
                "const naming = {{ lookup: fn(_id: Int) {{ \"a\" }} }}\n",
                "pub fn main() -> Text {{ greet() using {{ names: naming }} }}\n",
            ),
            parameter
        )
    };
    assert_eq!(check_codes(&callable("fn() -> Text")), ["T4203"]);
    assert!(check_codes(&callable("fn() -> Text needs { names: Names }")).is_empty());
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

// A text literal cannot span lines, so the formatter keeps record, closure, block,
// and pipeline expressions inside `${}` on one line; the formatted program parses,
// formats to itself, and computes the same text.
#[test]
fn braced_expressions_inside_interpolations_format_on_one_line() {
    let source = concat!(
        "import std/debug as Debug\n",
        "fn twice(n: Int) -> Int { n * 2 }\n",
        "pub fn main() -> Text {\n",
        "  let record = \"v: ${Debug.render({ a: 1 })}\"\n",
        "  let closure = \"x${(fn() { 7 })()}y\"\n",
        "  let field = \"${ { a: 1 }.a }\"\n",
        "  let piped = \"${3 |> twice}\"\n",
        "  let nested = \"${\"in ${ {b: 2}.b } side\"}\"\n",
        "  \"${record}|${closure}|${field}|${piped}|${nested}\"\n",
        "}\n",
    );
    let formatted = Compiler::format(SourceSnapshot::from_text("main.chimi", source));
    assert!(
        formatted.text().contains(concat!(
            "  let field = \"${{ a: 1 }.a}\"\n",
            "  let piped = \"${3 |> twice}\"\n",
            "  let nested = \"${\"in ${{ b: 2 }.b} side\"}\"\n",
        )),
        "{}",
        formatted.text()
    );
    assert_eq!(
        run_both("interpolation-braces", source),
        r#""outcome":"success","value":"v: { a: 1 }|x7y|1|6|in 2 side""#
    );
}

// Lexical model: text literal escapes denote their characters in plain literals,
// interpolated chunks, and text patterns, on both backends.
#[test]
fn text_literal_escapes_denote_their_characters() {
    let source = concat!(
        "fn kind(text: Text) -> Int {\n",
        "  case text {\n",
        "    \"a\\\"b\" -> 1\n",
        "    _ -> 2\n",
        "  }\n",
        "}\n",
        "pub fn main() -> List<Text> {\n",
        "  let quote = \"a\\\"b\"\n",
        "  [\n",
        "    \"q\\\" s\\\\ n\\n r\\r t\\t d\\$ u\\u{e9}\\u{1F600}\",\n",
        "    \"${kind(quote)}\\t\\\"${quote}\\\"\\n\",\n",
        "  ]\n",
        "}\n",
    );
    assert_eq!(
        run_both("text-escapes", source),
        r#""outcome":"success","value":["q\" s\\ n\n r\r t\t d$ ué😀","1\t\"a\"b\"\n"]"#
    );
}

#[test]
fn malformed_text_escapes_are_syntax_errors() {
    let codes =
        |literal: &str| check_codes(&format!("pub fn main() -> Text {{\n  {literal}\n}}\n"));
    for literal in [
        r#""\q""#,
        r#""\u{}""#,
        r#""\u{1234567}""#,
        r#""\u{d800}""#,
        r#""\u{110000}""#,
        r#""\u41""#,
        r#""\u{41""#,
    ] {
        assert_eq!(codes(literal), ["S1001"], "{literal}");
    }
    // A backslash before the line break cannot escape it: the literal is unterminated.
    assert!(codes("\"a\\\n\"").contains(&"S1001".to_owned()));
}

// Json.decode reports the same DecodeError message and offset on both backends for
// every kind of malformed input.
#[test]
fn malformed_json_reports_the_same_decode_error_on_both_backends() {
    let inputs = [
        "",
        "{1: 2}",
        "{\"a\" 1}",
        "{\"a\": 1 \"b\"}",
        "{\"a\": 1, \"a\": 2}",
        "[1 2]",
        "[1,",
        "01",
        "-",
        "1.",
        "1e ",
        "nul",
        "\"ab",
        "\"a\tb\"",
        "\"\\q\"",
        "\"\\",
        "\"\\u12\"",
        "\"\\u12zz\"",
        "\"\\ud800x\"",
        "\"\\ud800\\x\"",
        "\"\\ud800\\u0041\"",
        "\"\\udc00\"",
        "1 x",
    ];
    let literal = |input: &str| {
        let mut quoted = String::from("\"");
        for character in input.chars() {
            match character {
                '"' => quoted.push_str("\\\""),
                '\\' => quoted.push_str("\\\\"),
                '\t' => quoted.push_str("\\t"),
                other => quoted.push(other),
            }
        }
        quoted.push('"');
        quoted
    };
    let calls = inputs
        .iter()
        .map(|input| format!("    error_of({}),\n", literal(input)))
        .collect::<String>();
    let source = format!(
        concat!(
            "import std/json as Json\n",
            "import std/debug as Debug\n",
            "fn error_of(source: Text) -> Text {{\n",
            "  case Json.decode(source) {{\n",
            "    Success(_) -> \"decoded\"\n",
            "    Failure(error) -> Debug.render(error)\n",
            "  }}\n",
            "}}\n",
            "pub fn main() -> List<Text> {{\n  [\n{}  ]\n}}\n",
        ),
        calls
    );
    let outcome = run_both("json-decode-errors", &source);
    assert_eq!(
        outcome.matches("DecodeError.InvalidJson").count(),
        inputs.len(),
        "{outcome}"
    );
    assert!(
        outcome.contains(r#"message: \"expected a string key in JSON object\", offset: 1"#),
        "{outcome}"
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
        // Text renders as the literal that writes it, so each escape round trips.
        (
            r#""say \"hi\" \\ \n\r\t\u{1b} $x \${hi} é""#,
            r#""say \"hi\" \\ \n\r\t\u{1b} $x \${hi} é""#,
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
        "import std/text as Text\n",
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

// E2 across a failure set: enum values are equal only within one enum type, so
// variants of two enums at the same declaration position, with equal payloads,
// differ, and stay distinct as Map keys.
#[test]
fn failure_set_values_of_different_enums_are_never_equal() {
    let source = concat!(
        "import std/map as Map\n",
        "enum E {\n  A(n: Int)\n  B\n}\n",
        "enum F {\n  C(m: Int)\n  D\n}\n",
        "fn fail(x: Int) -> Int {\n",
        "  case x {\n",
        "    0 -> Failure(E.A(1))\n",
        "    1 -> Failure(F.C(1))\n",
        "    2 -> Failure(E.B)\n",
        "    _ -> Failure(F.D)\n",
        "  }\n",
        "}\n",
        "fn same(x: Int, y: Int) -> Bool {\n",
        "  case fail(x) {\n",
        "    Failure(e) -> case fail(y) {\n",
        "      Failure(f) -> e == f\n",
        "      Success(_) -> False\n",
        "    }\n",
        "    Success(_) -> False\n",
        "  }\n",
        "}\n",
        "fn keys() -> List<Bool> {\n",
        "  case fail(0) {\n",
        "    Failure(a) -> case fail(1) {\n",
        "      Failure(c) -> {\n",
        "        let table = Map.empty() |> Map.put(a, 1) |> Map.put(c, 2)\n",
        "        [Map.get(table, a) == Some(1), Map.get(table, c) == Some(2)]\n",
        "      }\n",
        "      Success(_) -> []\n",
        "    }\n",
        "    Success(_) -> []\n",
        "  }\n",
        "}\n",
        "pub fn main() -> List<List<Bool>> {\n",
        "  [\n",
        "    [same(0, 1), same(2, 3), same(2, 1), same(0, 3)],\n",
        "    [same(0, 0), same(1, 1), same(3, 3), same(0, 2)],\n",
        "    keys(),\n",
        "  ]\n",
        "}\n",
    );
    assert_eq!(
        run_both("failure-set-enum-identity", source),
        concat!(
            "\"outcome\":\"success\",\"value\":",
            "[[false,false,false,false],[true,true,true,false],[true,true]]"
        )
    );
}

// Diagnostics name a failure set by its members, and T4301 suggests nothing
// that does not exist yet (`List.sort_by` is not in `std/list`).
#[test]
fn comparison_diagnostics_render_failure_sets_and_no_missing_helpers() {
    let source = concat!(
        "enum E {\n  A\n}\n",
        "enum F {\n  C\n}\n",
        "fn fail(x: Int) -> Int {\n",
        "  case x {\n",
        "    0 -> Failure(E.A)\n",
        "    _ -> Failure(F.C)\n",
        "  }\n",
        "}\n",
        "fn check(x: Int) -> List<Bool> {\n",
        "  case fail(x) {\n",
        "    Failure(e) -> [e == 1, e < e]\n",
        "    Success(_) -> []\n",
        "  }\n",
        "}\n",
        "pub fn main() -> Bool { { a: 1 } < { a: 2 } }\n",
    );
    let diagnostics = coded_messages(source);
    assert_eq!(diagnostics.len(), 3, "{diagnostics:?}");
    assert_eq!(diagnostics[0].0, "T4302");
    assert!(
        diagnostics[0].1.contains("found `E | F` and `Int`"),
        "{diagnostics:?}"
    );
    assert_eq!(diagnostics[1].0, "T4301");
    assert!(diagnostics[1].1.contains("`E | F`"), "{diagnostics:?}");
    for (_, message) in &diagnostics {
        assert!(!message.contains("this type"), "{message}");
        assert!(!message.contains("sort_by"), "{message}");
    }
}

// A member value widens to its failure set for `==`, on either side.
#[test]
fn a_failure_set_value_compares_with_a_member_value() {
    let source = concat!(
        "enum E {\n  A\n  B(n: Int)\n}\n",
        "enum F {\n  C\n}\n",
        "fn fail(x: Int) -> Int {\n",
        "  case x {\n",
        "    0 -> Failure(E.A)\n",
        "    1 -> Failure(E.B(1))\n",
        "    _ -> Failure(F.C)\n",
        "  }\n",
        "}\n",
        "fn check(x: Int) -> List<Bool> {\n",
        "  case fail(x) {\n",
        "    Failure(e) -> [e == E.B(1), F.C == e, e != E.A]\n",
        "    Success(_) -> []\n",
        "  }\n",
        "}\n",
        "pub fn main() -> List<List<Bool>> { [check(0), check(1), check(2)] }\n",
    );
    assert_eq!(
        run_both("failure-set-member-equality", source),
        concat!(
            "\"outcome\":\"success\",\"value\":",
            "[[false,false,false],[true,false,true],[false,true,true]]"
        )
    );
    // Widening goes only to a set that holds the member's whole enum.
    let outside = concat!(
        "enum E {\n  A\n}\n",
        "enum F {\n  C\n}\n",
        "enum G {\n  D\n}\n",
        "fn fail(x: Int) -> Int {\n",
        "  case x {\n",
        "    0 -> Failure(E.A)\n",
        "    _ -> Failure(F.C)\n",
        "  }\n",
        "}\n",
        "fn check(x: Int) -> Bool {\n",
        "  case fail(x) {\n",
        "    Failure(e) -> e == G.D\n",
        "    Success(_) -> False\n",
        "  }\n",
        "}\n",
        "pub fn main() -> Bool { check(0) }\n",
    );
    assert_eq!(check_codes(outside), ["T4302"]);
    // A non-enum operand is a plain mismatch.
    assert_eq!(
        check_codes(&outside.replace("e == G.D", "e == 1")),
        ["T4302"]
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

// Collections and JSON: an `Int` key, which once failed natively with "invalid
// runtime shapes", stores and finds entries on both backends.
#[test]
fn int_keyed_maps_store_and_find_entries() {
    let source = concat!(
        "import std/map as Map\n",
        "import std/debug as Debug\n",
        "pub fn main() -> List<Text> {\n",
        "  let single = Map.put(Map.empty(), 10, \"x\")\n",
        "  let both = Map.put(single, 2, \"y\") |> Map.put(10, \"z\")\n",
        "  [Debug.render(Map.get(single, 10)), Debug.render(Map.get(both, 3)), Debug.render(both)]\n",
        "}\n",
    );
    assert_eq!(
        run_both("int-map-keys", source),
        r#""outcome":"success","value":["Some(\"x\")","None","Map { 2: \"y\", 10: \"z\" }"]"#
    );
}

// Runtime output channels: the native outcome record writes a map's entries as the
// interpreter does: a Text-keyed map is an object, an empty map is `{}`, and any
// other map is `{"map":[[key,value],...]}` in key order.
#[test]
fn map_outcomes_render_their_entries_on_both_backends() {
    let source = concat!(
        "import std/map as Map\n",
        "import std/map.{Map}\n",
        "pub fn main() -> List<Map<Int, Map<Text, Int>>> {\n",
        "  let inner = Map.put(Map.put(Map.empty(), \"b\", 2), \"a\", 1)\n",
        "  [Map.put(Map.put(Map.empty(), 10, inner), 2, Map.empty()), Map.empty()]\n",
        "}\n",
    );
    assert_eq!(
        run_both("map-outcome", source),
        r#""outcome":"success","value":[{"map":[[2,{}],[10,{"a":1,"b":2}]]},{}]"#
    );
}

// Structured keys through the constants table and the debug format: a constant
// map keeps value keys on both backends, and `Debug.render` writes each key in
// the debug format, the same on both paths whatever the insertion order.
#[test]
fn structured_map_keys_survive_constants_and_debug_rendering() {
    let source = concat!(
        "import std/debug as Debug\n",
        "import std/map as Map\n",
        "struct Point {\n  x: Int,\n  y: Int,\n}\n",
        "const scores = Map.empty() |> Map.put(Point(2, 1), 20) |> Map.put(Point(1, 9), 10)\n",
        "pub fn main() -> List<Text> {\n",
        "  let reordered = Map.empty() |> Map.put(Point(1, 9), 10) |> Map.put(Point(2, 1), 20)\n",
        "  let floats = Map.empty() |> Map.put(1.5, \"b\") |> Map.put(-0.0, \"a\")\n",
        "  let floats_reordered = Map.empty() |> Map.put(0.0, \"c\") |> Map.put(1.5, \"b\")\n",
        "  [\n",
        "    Debug.render(scores |> Map.get(Point(2, 1))),\n",
        "    Debug.render(Debug.render(scores) == Debug.render(reordered)),\n",
        "    Debug.render(Debug.render(floats |> Map.put(0.0, \"c\")) == Debug.render(floats_reordered)),\n",
        "    Debug.render(scores),\n",
        "    Debug.render(floats |> Map.put(0.0, \"c\")),\n",
        "  ]\n",
        "}\n",
    );
    // run_both asserts the two paths render identically; the key order itself
    // is internal, so only the entries and insertion independence are checked.
    let outcome = run_both("structured-map-keys-debug", source);
    let prefix = format!(
        "\"outcome\":\"success\",\"value\":[{},{},{},",
        json_text("Some(20)"),
        json_text("True"),
        json_text("True"),
    );
    assert!(outcome.starts_with(&prefix), "{outcome}");
    for entry in [
        "Point { x: 1, y: 9 }: 10",
        "Point { x: 2, y: 1 }: 20",
        "0.0: \"c\"",
        "1.5: \"b\"",
    ] {
        let quoted = json_text(entry);
        assert!(
            outcome.contains(&quoted[1..quoted.len() - 1]),
            "{entry}: {outcome}"
        );
    }
    assert!(!outcome.contains("-0.0"), "{outcome}");
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

// E15: exponents wider than any machine integer compare exactly on both paths:
// no overflow, no clamp, and Map keys that differ only there stay distinct.
#[test]
fn json_numbers_with_huge_exponents_compare_exactly() {
    let source = concat!(
        "import std/json as Json\n",
        "import std/map as Map\n",
        "pub fn main() -> List<Bool> {\n",
        "  let keys = Map.empty()\n",
        "    |> Map.put(Json.decode(\"1e1152921504606846977\"), 1)\n",
        "    |> Map.put(Json.decode(\"1e1152921504606846976\"), 2)\n",
        "    |> Map.put(Json.decode(\"10e1152921504606846976\"), 3)\n",
        "  [\n",
        "    Json.decode(\"1e9999999999999999999\") == Json.decode(\"1\"),\n",
        "    Json.decode(\"1e9999999999999999999\") == Json.decode(\"1e9999999999999999998\"),\n",
        "    Json.decode(\"1e1152921504606846977\") == Json.decode(\"1e1152921504606846976\"),\n",
        "    Json.decode(\"10e1152921504606846976\") == Json.decode(\"1e1152921504606846977\"),\n",
        "    Json.decode(\"1e-9999999999999999999\") == Json.decode(\"0.1e-9999999999999999998\"),\n",
        "    Map.get(keys, Json.decode(\"1e1152921504606846977\")) == Some(3),\n",
        "    Map.get(keys, Json.decode(\"1e1152921504606846976\")) == Some(2),\n",
        "  ]\n",
        "}\n",
    );
    assert_eq!(
        run_both("json-number-huge-exponents", source),
        "\"outcome\":\"success\",\"value\":[false,false,false,true,true,true,true]"
    );
}

// Operands evaluate left to right, so when both fail the left failure is the
// outcome of the comparison.
#[test]
fn a_comparison_propagates_the_left_operand_failure_first() {
    let source = concat!(
        "enum E {\n  A\n  B\n}\n",
        "fn a() -> Int { Failure(E.A) }\n",
        "fn b() -> Int { Failure(E.B) }\n",
        "pub fn main() -> List<Bool> { [a() == b(), b() < a()] }\n",
    );
    assert_eq!(
        run_both("comparison-evaluation-order", source),
        concat!(
            "\"outcome\":\"expected_failure\",",
            "\"error\":{\"type\":\"E\",\"variant\":\"A\",\"fields\":{}}"
        )
    );
    let reversed = source.replace("[a() == b(), b() < a()]", "[b() < a(), a() == b()]");
    assert_eq!(
        run_both("comparison-evaluation-order-reversed", &reversed),
        concat!(
            "\"outcome\":\"expected_failure\",",
            "\"error\":{\"type\":\"E\",\"variant\":\"B\",\"fields\":{}}"
        )
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

// E18 end to end, with ts-0e026f and ts-9b1ec1: a value nested 100,000 levels deep
// builds, compares, renders with `Debug.render`, is returned as the outcome, and is
// freed, on both backends, without overflowing a host or fiber stack. run_both
// compares the whole rendered text and outcome across backends. The million-level
// comparison rows are the unit and C runtime tests.
#[test]
fn deeply_nested_values_build_compare_render_and_free() {
    let source = concat!(
        "import std/debug as Debug\n",
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
        "pub fn main() -> (Bool, Bool, Text, Nest) {\n",
        "  let deep = nested(100000)\n",
        "  let copy = nested(100000)\n",
        "  (deep == copy, Wrap([deep]) == copy, Debug.render(deep), copy)\n",
        "}\n",
    );
    let outcome = run_both("deep-values", source);
    let levels = 100_000;
    let rendered = format!(
        "\"{}Nest.Leaf{}\"",
        "Nest.Wrap([".repeat(levels),
        "])".repeat(levels)
    );
    let value = format!(
        "{}{{\"type\":\"Nest\",\"variant\":\"Leaf\",\"fields\":{{}}}}{}",
        "{\"type\":\"Nest\",\"variant\":\"Wrap\",\"fields\":{\"children\":[".repeat(levels),
        "]}}".repeat(levels)
    );
    assert_eq!(
        outcome,
        format!("\"outcome\":\"success\",\"value\":{{\"tuple\":[true,false,{rendered},{value}]}}")
    );
}

// E18 for `Json.encode`: a Json.Value a program builds 100,000 levels deep, through
// both Array and Object, encodes on both backends. A native encoder that recursed
// per level overflowed the C stack; one whose step stack wrote an Object's entries
// or closing text out of order would disagree with the interpreter and with the
// expected text.
#[test]
fn json_encode_writes_a_deeply_nested_value() {
    let source = concat!(
        "import std/json as Json\n",
        "import std/map as Map\n",
        "struct Build { depth: Int, value: Json.Value }\n",
        "pub fn main() -> Text {\n",
        "  let built = loop state = Build(100000, Json.Value.Null) {\n",
        "    case state.depth {\n",
        "      0 -> break state.value\n",
        "      _ -> {\n",
        "        let fields = Map.put(Map.put(Map.empty(), \"b\", Json.Value.Text(\"q\\\"\")), \"a\", state.value)\n",
        "        Build(state.depth - 1, Json.Value.Array([Json.Value.Object(fields), Json.Value.Boolean(True)]))\n",
        "      }\n",
        "    }\n",
        "  }\n",
        "  Json.encode(built)\n",
        "}\n",
    );
    let levels = 100_000;
    let encoded = format!(
        "{}null{}",
        "[{\\\"a\\\":".repeat(levels),
        ",\\\"b\\\":\\\"q\\\\\\\"\\\"},true]".repeat(levels)
    );
    assert_eq!(
        run_both("deep-json-encode", source),
        format!("\"outcome\":\"success\",\"value\":\"{encoded}\"")
    );
}

// E18 through map keys: a value nested 100,000 levels deep where each level is a
// key of the next level's map renders with `Debug.render` on both backends. A key
// renders on one line even in the indented format, and the value after it is
// indented again, so the shallow indented render checks that the format is
// restored after a key. The outcome encoding of such maps is
// outcome_size_is_linear_in_map_key_nesting's.
#[test]
fn debug_render_nests_through_map_keys() {
    let source = concat!(
        "import std/debug as Debug\n",
        "import std/map as Map\n",
        "import std/map.{Map}\n",
        "enum N {\n  Leaf\n  W(m: Map<N, List<Int>>)\n}\n",
        "struct Build { depth: Int, n: N }\n",
        "fn nested(depth: Int) -> N {\n",
        "  loop state = Build(depth, Leaf) {\n",
        "    case state.depth {\n",
        "      0 -> break state.n\n",
        "      _ -> Build(state.depth - 1, W(Map.put(Map.empty(), state.n, [0])))\n",
        "    }\n",
        "  }\n",
        "}\n",
        "pub fn main() -> (Text, Text, N) {\n",
        "  let shallow = nested(2)\n",
        "  (Debug.render(nested(100000)), Debug.render_indented(shallow), shallow)\n",
        "}\n",
    );
    let outcome = run_both("deep-map-keys", source);
    let levels = 100_000;
    let rendered = format!(
        "\"{}N.Leaf{}\"",
        "N.W(Map { ".repeat(levels),
        ": [0] })".repeat(levels)
    );
    let indented =
        "\"N.W(\\n  Map {\\n    N.W(Map { N.Leaf: [0] }): [\\n      0,\\n    ],\\n  },\\n)\"";
    assert!(
        outcome.starts_with(&format!(
            "\"outcome\":\"success\",\"value\":{{\"tuple\":[{rendered},{indented},"
        )),
        "{}",
        &outcome[outcome.len().saturating_sub(400)..]
    );
}

// ts-0fa50c (CHI-177): an outcome writes each map key once, so its size is
// linear in the value's size however deep maps nest through keys. Before, a
// non-Text key's member name was the key's own JSON text, which escaped every
// enclosing key again and doubled the size per level, so this ran out of memory.
#[test]
fn outcome_size_is_linear_in_map_key_nesting() {
    let source = concat!(
        "import std/map as Map\n",
        "import std/map.{Map}\n",
        "enum N {\n  Leaf\n  W(m: Map<N, List<Int>>)\n}\n",
        "struct Build { depth: Int, n: N }\n",
        "fn nested(depth: Int) -> N {\n",
        "  loop state = Build(depth, Leaf) {\n",
        "    case state.depth {\n",
        "      0 -> break state.n\n",
        "      _ -> Build(state.depth - 1, W(Map.put(Map.empty(), state.n, [0])))\n",
        "    }\n",
        "  }\n",
        "}\n",
        "pub fn main() -> N { nested(100000) }\n",
    );
    let levels = 100_000;
    let leaf = "{\"type\":\"N\",\"variant\":\"Leaf\",\"fields\":{}}";
    let nested = format!(
        "{}{leaf}{}",
        "{\"type\":\"N\",\"variant\":\"W\",\"fields\":{\"m\":{\"map\":[[".repeat(levels),
        ",[0]]]}}}".repeat(levels)
    );
    assert_eq!(
        run_both("outcome-non-text-keys", source),
        format!("\"outcome\":\"success\",\"value\":{nested}")
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
            "import std/map.{Map}\n\
             fn keyed(table: Map<fn() -> Int, Int>) -> Int { 1 }\n",
            "T4300",
            "`Map` keys must be equatable, but `fn() -> Int` is not equatable: it is a function",
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
            "import std/set.{Set}\n\
             fn keyed(items: Set<fn() -> Int>) -> Int { 1 }\n",
            "T4300",
            "`Set` items must be equatable, but `fn() -> Int` is not equatable: it is a function",
        ),
        (
            "import std/set.{Set}\n\
             fn less(a: Set<Int>, b: Set<Int>) -> Bool { a < b }\n",
            "T4301",
            "`Set`, which has no order",
        ),
        (
            "import std/json.{Value}\n\
             fn less(a: List<Value>, b: List<Value>) -> Bool { a < b }\n",
            "T4301",
            "element of `List` is `Value`, which is not ordered because an `Object` holds a `Map`",
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

// E10: a written `Map` type whose key type is not equatable is rejected where it
// is written, as a parameter, a struct field, or a result, even with no call
// that builds the map.
#[test]
fn a_written_map_type_needs_an_equatable_key() {
    let source = concat!(
        "import std/map.{Map}\n",
        "struct Registry { routes: Map<fn() -> Int, Int> }\n",
        "fn count(table: Map<fn() -> Int, Int>) -> Int { 1 }\n",
        "fn nested(table: List<Map<List<fn() -> Int>, Int>>) -> Int { 1 }\n",
        "fn fine(table: Map<Text, fn() -> Int>) -> Int { 1 }\n",
        "fn generic<K>(table: Map<K, Int>) -> Int { 1 }\n",
        "fn early(table: Map<Later, Int>) -> Int { 1 }\n",
        "struct Later { run: fn() -> Int }\n",
        "pub fn main() -> Int { 1 }\n",
    );
    let diagnostics = coded_messages(source);
    assert_eq!(diagnostics.len(), 4, "{diagnostics:?}");
    assert!(
        diagnostics[3].1.contains("field `run` of `Later`"),
        "{diagnostics:?}"
    );
    for (code, message) in &diagnostics {
        assert_eq!(code, "T4300", "{message}");
        assert!(message.contains("`Map` key"), "{message}");
    }
    assert!(
        diagnostics[0].1.contains("`fn() -> Int`"),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics[2].1.contains("`List<fn() -> Int>`"),
        "{diagnostics:?}"
    );
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
    // The chain is the one error; the operand types it happens to pair are not.
    for chained in ["1 < 2 < 3", "1 == 1 == True", "1 == 2 == 3"] {
        let codes = check_codes(&format!("pub fn main() -> Bool {{ {chained} }}\n"));
        assert_eq!(codes, ["S1006"], "{chained}");
    }
    // Parentheses make it one comparison of a `Bool` with an `Int`.
    assert_eq!(
        check_codes("pub fn main() -> Bool { (1 < 2) == 3 }\n"),
        ["T4302"]
    );
}

// One missing property is one diagnostic: a pipeline of standard Map calls that
// all instantiate `K` with the same function type reports it once.
#[test]
fn a_map_pipeline_with_a_function_key_reports_once() {
    let source = concat!(
        "import std/map as Map\n",
        "fn one() -> Int { 1 }\n",
        "pub fn main() -> Option<Int> { Map.empty() |> Map.put(one, 1) |> Map.get(one) }\n",
    );
    let diagnostics = coded_messages(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].0, "T4300");
    assert!(
        diagnostics[0].1.contains("needs `K` to be equatable"),
        "{diagnostics:?}"
    );
    // Two separate maps are two errors.
    let separate = concat!(
        "import std/map as Map\n",
        "fn one() -> Int { 1 }\n",
        "pub fn main() -> Int {\n",
        "  let first = Map.empty() |> Map.put(one, 1)\n",
        "  let second = Map.empty() |> Map.put(one, 2)\n",
        "  1\n",
        "}\n",
    );
    assert_eq!(check_codes(separate), ["T4300", "T4300"]);
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

// `Ordering` is a closed enum: matching `Less`, `Equal`, and `Greater` covers it,
// imported bare or qualified, with no wildcard arm.
#[test]
fn a_case_over_every_ordering_variant_is_exhaustive() {
    let source = concat!(
        "import std/order as Order\n",
        "import std/order.{Less, Equal, Greater, Ordering}\n",
        "fn code(o: Ordering) -> Int {\n",
        "  case o {\n",
        "    Less -> 0\n",
        "    Equal -> 1\n",
        "    Greater -> 2\n",
        "  }\n",
        "}\n",
        "fn qualified(o: Ordering) -> Int {\n",
        "  case o {\n",
        "    Ordering.Less -> 10\n",
        "    Ordering.Equal -> 20\n",
        "    Ordering.Greater -> 30\n",
        "  }\n",
        "}\n",
        "pub fn main() -> List<Int> {\n",
        "  [code(Order.compare(1, 2)), code(Order.compare(2, 2)), qualified(Order.compare(3, 2))]\n",
        "}\n",
    );
    assert_eq!(
        run_both("ordering-exhaustive", source),
        "\"outcome\":\"success\",\"value\":[0,1,30]"
    );
    let missing = concat!(
        "import std/order.{Less, Equal, Ordering}\n",
        "fn code(o: Ordering) -> Int {\n",
        "  case o {\n",
        "    Less -> 0\n",
        "    Equal -> 1\n",
        "  }\n",
        "}\n",
        "pub fn main() -> Int { 1 }\n",
    );
    let diagnostics = coded_messages(missing);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].0, "T4231");
    assert!(diagnostics[0].1.contains("Greater"), "{diagnostics:?}");
}

// E13 when the requirement starts in a standard function: the caller of the user
// generic gets T4305, labelled at the standard call inside the callee.
#[test]
fn a_requirement_from_a_standard_function_labels_the_call_in_the_callee() {
    let source = concat!(
        "import std/order as Order\n",
        "import std/order.{Ordering}\n",
        "fn cmp<T>(a: T, b: T) -> Ordering { Order.compare(a, b) }\n",
        "pub fn main() -> Ordering { cmp({ a: 1 }, { a: 2 }) }\n",
    );
    let report = type_report(source);
    let diagnostics = report.diagnostics();
    assert_eq!(diagnostics.len(), 1, "{}", report.render_diagnostics());
    let diagnostic = &diagnostics[0];
    assert_eq!(
        diagnostic.code(),
        "T4305",
        "{}",
        report.render_diagnostics()
    );
    let range = diagnostic.range();
    assert_eq!(
        &source[range.start() as usize..range.end() as usize],
        "cmp({ a: 1 }, { a: 2 })"
    );
    let labels = diagnostic
        .labels()
        .iter()
        .map(|label| {
            let text = &source[label.range().start() as usize..label.range().end() as usize];
            (text, label.message())
        })
        .collect::<Vec<_>>();
    assert_eq!(labels[0].0, "Order.compare(a, b)", "{labels:?}");
    assert!(labels[0].1.contains("`Order.compare`"), "{labels:?}");
    assert_eq!(labels[1].0, "T", "{labels:?}");
}

// A call with no named target, such as calling the function another call
// returns, names the declaration that owns the instantiated parameter.
#[test]
fn an_unnamed_call_target_is_named_by_the_parameter_owner() {
    let source = concat!(
        "fn mk<T>() -> fn(T, T) -> Bool { fn(a, b) { a == b } }\n",
        "fn one() -> Int { 1 }\n",
        "fn two() -> Int { 2 }\n",
        "pub fn main() -> Bool { mk()(one, two) }\n",
    );
    let diagnostics = coded_messages(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].0, "T4305", "{diagnostics:?}");
    assert!(diagnostics[0].1.contains("`T` of `mk`"), "{diagnostics:?}");
}

// E13 for a generic function used as a value: passing it where a function type
// fixes its parameter is an instantiation too, so the requirement is checked
// there, for a user function and for a standard one.
#[test]
fn passing_a_generic_function_as_a_value_checks_its_requirement() {
    let source = concat!(
        "fn same<T>(left: T, right: T) -> Bool { left == right }\n",
        "fn one() -> Int { 1 }\n",
        "fn apply(f: fn(fn() -> Int, fn() -> Int) -> Bool) -> Bool { f(one, one) }\n",
        "pub fn main() -> Bool { apply(same) }\n",
    );
    let report = type_report(source);
    let diagnostics = report.diagnostics();
    assert_eq!(diagnostics.len(), 1, "{}", report.render_diagnostics());
    let diagnostic = &diagnostics[0];
    assert_eq!(
        diagnostic.code(),
        "T4305",
        "{}",
        report.render_diagnostics()
    );
    assert!(
        diagnostic.message().contains("`same`")
            && diagnostic.message().contains("it is a function"),
        "{}",
        diagnostic.message()
    );
    let range = diagnostic.range();
    assert_eq!(
        &source[range.start() as usize..range.end() as usize],
        "apply(same)"
    );
    let labelled = diagnostic
        .labels()
        .iter()
        .map(|label| &source[label.range().start() as usize..label.range().end() as usize])
        .collect::<Vec<_>>();
    assert_eq!(labelled, ["left == right", "T"]);

    let standard = concat!(
        "import std/order as Order\n",
        "import std/order.{Ordering}\n",
        "fn one() -> Int { 1 }\n",
        "fn two() -> Int { 2 }\n",
        "fn apply(f: fn(fn() -> Int, fn() -> Int) -> Ordering) -> Ordering { f(one, two) }\n",
        "pub fn main() -> Ordering { apply(Order.compare) }\n",
    );
    assert_eq!(check_codes(standard), ["T4301"]);
    // Data instantiations still pass.
    let data = concat!(
        "fn same<T>(left: T, right: T) -> Bool { left == right }\n",
        "fn apply(f: fn(Int, Int) -> Bool) -> Bool { f(1, 1) }\n",
        "pub fn main() -> Bool { apply(same) }\n",
    );
    assert_eq!(
        run_both("generic-value-data", data),
        "\"outcome\":\"success\",\"value\":true"
    );
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

// Multi-clause functions (patterns-and-function-clauses.md, "Multi-clause functions").

const SHAPES: &str = concat!(
    "enum Shape {\n",
    "  Circle(radius: Int)\n",
    "  Rectangle(width: Int, height: Int)\n",
    "}\n",
);

const DESCRIBE: &str = concat!(
    "import std/text as Text\n",
    "enum CountError {\n",
    "  Negative(count: Int)\n",
    "}\n",
    "fn describe(0) -> Text {\n",
    "  \"no items\"\n",
    "}\n",
    "fn describe(1) {\n",
    "  \"one item\"\n",
    "}\n",
    "fn describe(count) if count < 0 {\n",
    "  Failure(CountError.Negative(count))\n",
    "}\n",
    "fn describe(count) {\n",
    "  Text.append(Int.to_text(count), \" items\")\n",
    "}\n",
);

/// Inspects `source` at the start of the `occurrence`-th (from 0) match of `needle`.
fn inspect_at(source: &str, needle: &str, occurrence: usize) -> chimi::InspectionReport {
    let offset = source
        .match_indices(needle)
        .nth(occurrence)
        .unwrap_or_else(|| panic!("`{needle}` occurs fewer than {} times", occurrence + 1))
        .0;
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    Compiler::inspect_project(vec![module], "main", offset).expect("inspectable offset")
}

// PC-01
#[test]
fn clauses_dispatch_on_the_argument_shape() {
    let source = format!(
        concat!(
            "{}",
            "pub fn area(Circle(radius)) -> Int {{\n",
            "  3 * radius * radius\n",
            "}}\n",
            "fn area(Rectangle(width, height)) {{\n",
            "  width * height\n",
            "}}\n",
            "pub fn main() -> List<Int> {{ [area(Circle(2)), area(Rectangle(3, 4))] }}\n",
        ),
        SHAPES
    );
    assert_eq!(
        run_both("clauses-area", &source),
        "\"outcome\":\"success\",\"value\":[12,12]"
    );
    let callable = inspect_at(&source, "area(Rectangle(width", 0);
    assert_eq!(
        callable.callable().map(chimi::CallableInspection::display),
        Some("fn(Shape) -> Int")
    );
}

// ts-270757: a literal whose patterns include a plain name parameter used to
// capture that name before it became an alias of the generated parameter, and
// the native closure then read a local slot that does not exist.
#[test]
fn a_function_literal_with_pattern_parameters_runs_and_captures_on_both_backends() {
    let outcome = run_both(
        "literal-tuple-parameter",
        concat!(
            "fn apply(f: fn((Int, Int), Int) -> Int) -> Int { f((1, 2), 30) }\n",
            "pub fn main() -> Int {\n",
            "  let offset = 400\n",
            "  apply(fn((a, b), c) { a + b * 10 + c + offset })\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":451");
}

// PC-02
#[test]
fn a_clause_gap_names_the_missing_argument_shape() {
    let source = format!(
        concat!(
            "{}",
            "fn area(Circle(radius)) -> Int {{\n",
            "  radius\n",
            "}}\n",
            "pub fn main() -> Int {{ area(Circle(2)) }}\n",
        ),
        SHAPES
    );
    let messages = coded_messages(&source);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0].0, "T4231");
    assert!(
        messages[0].1.contains("`area(Rectangle(_, _))`"),
        "{messages:?}"
    );
}

// PC-03
#[test]
fn a_clause_after_a_covering_clause_never_runs() {
    let source = concat!(
        "fn describe(count) -> Int {\n",
        "  count\n",
        "}\n",
        "fn describe(0) {\n",
        "  1\n",
        "}\n",
        "pub fn main() -> Int { describe(0) }\n",
    );
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    let report = Compiler::type_project(vec![module]);
    let unreachable = report
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.code() == "T4232")
        .expect("the second clause is unreachable");
    let start = source.find("describe(0)").unwrap();
    assert_eq!(unreachable.range().start() as usize, start);
    let labelled = |source: &str, diagnostic: &chimi::diagnostic::Diagnostic| {
        diagnostic
            .labels()
            .iter()
            .map(|label| label.range().start() as usize)
            .map(|start| source[start..].lines().next().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(labelled(source, unreachable), ["describe(count) -> Int {"]);
    // Two clauses that cover it only together: the label is on the later one.
    let joint = concat!(
        "fn pick(True, _) -> Int {\n",
        "  1\n",
        "}\n",
        "fn pick(False, _) {\n",
        "  2\n",
        "}\n",
        "fn pick(_, 0) {\n",
        "  3\n",
        "}\n",
        "pub fn main() -> Int { pick(True, 0) }\n",
    );
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", joint))
        .expect("valid module path");
    let report = Compiler::type_project(vec![module]);
    let codes = report
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code())
        .collect::<Vec<_>>();
    assert_eq!(codes, ["T4232"]);
    assert_eq!(
        labelled(joint, &report.diagnostics()[0]),
        ["pick(False, _) {"]
    );
}

// PC-04
#[test]
fn a_catch_all_clause_must_be_last() {
    let codes = check_codes(concat!(
        "fn f(_) -> Int {\n",
        "  0\n",
        "}\n",
        "fn f(1) {\n",
        "  1\n",
        "}\n",
        "pub fn main() -> Int { f(1) }\n",
    ));
    assert_eq!(codes, ["T4233", "T4232"]);
}

// PC-05 and the guarded-clause example: a guard that fails selects the next clause,
// and a clause that returns `Failure` adds to the group's failure row.
#[test]
fn guarded_clauses_run_in_order_and_contribute_failures() {
    let source = format!(
        "import std/int as Int\n{DESCRIBE}{}",
        concat!(
            "pub fn main() -> List<Text> {\n",
            "  [describe(0), describe(1), describe(5), describe(-2)]\n",
            "}\n",
        )
    );
    assert_eq!(
        run_both("clauses-describe", &source),
        concat!(
            "\"outcome\":\"expected_failure\",",
            "\"error\":{\"type\":\"CountError\",\"variant\":\"Negative\",\"fields\":{\"count\":-2}}",
        )
    );
    let succeeding = source.replace("describe(-2)]", "describe(-0)]");
    assert_eq!(
        run_both("clauses-describe-success", &succeeding),
        "\"outcome\":\"success\",\"value\":[\"no items\",\"one item\",\"5 items\",\"no items\"]"
    );
    let callable = inspect_at(&source, "describe(count) if", 0);
    assert_eq!(
        callable.callable().map(chimi::CallableInspection::display),
        Some("fn(Int) -> Text errors CountError")
    );
}

// Range patterns: integer ranges are guards over clauses.
#[test]
fn integer_ranges_are_matched_with_guards() {
    let source = concat!(
        "fn grade(score) if score >= 90 {\n",
        "  \"A\"\n",
        "}\n",
        "fn grade(score) if score >= 80 {\n",
        "  \"B\"\n",
        "}\n",
        "fn grade(_) {\n",
        "  \"C\"\n",
        "}\n",
        "pub fn main() -> List<Text> { [grade(95), grade(80), grade(79)] }\n",
    );
    assert_eq!(
        run_both("clauses-grade", source),
        "\"outcome\":\"success\",\"value\":[\"A\",\"B\",\"C\"]"
    );
}

// PC-06
#[test]
fn guarded_clauses_do_not_count_toward_coverage() {
    let codes = check_codes(concat!(
        "fn sign(n) -> Int if n < 0 {\n",
        "  -1\n",
        "}\n",
        "fn sign(n) if n >= 0 {\n",
        "  1\n",
        "}\n",
        "pub fn main() -> Int { sign(3) }\n",
    ));
    assert_eq!(codes, ["T4231"]);
}

// PC-07
#[test]
fn a_clause_guard_must_be_pure() {
    let codes = check_codes(concat!(
        "interface Validator {\n",
        "  valid: fn(Int) -> Bool\n",
        "}\n",
        "fn classify(value) -> Int needs { validator: Validator } if validator.valid(value) {\n",
        "  1\n",
        "}\n",
        "fn classify(_) {\n",
        "  0\n",
        "}\n",
        "pub fn main() -> Int { 0 }\n",
    ));
    assert_eq!(codes, ["T4234"]);
}

// PC-08
#[test]
fn clauses_must_take_the_same_number_of_parameters() {
    let codes = check_codes(concat!(
        "fn f(0) -> Int {\n",
        "  0\n",
        "}\n",
        "fn f(a, b) {\n",
        "  a + b\n",
        "}\n",
        "fn f(_) {\n",
        "  1\n",
        "}\n",
        "pub fn main() -> Int { f(1) }\n",
    ));
    assert_eq!(codes, ["T4280"]);
}

// PC-09
#[test]
fn clauses_cannot_overload_by_type() {
    let messages = coded_messages(concat!(
        "fn f(0) -> Int {\n",
        "  0\n",
        "}\n",
        "fn f(\"zero\") {\n",
        "  1\n",
        "}\n",
        "fn f(_) {\n",
        "  2\n",
        "}\n",
        "pub fn main() -> Int { f(1) }\n",
    ));
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0].0, "T4281");
    assert!(messages[0].1.contains("does not overload"), "{messages:?}");
}

// PC-10
#[test]
fn only_the_first_clause_states_the_header() {
    let source = concat!(
        "pub fn f(0) -> Int {\n",
        "  0\n",
        "}\n",
        "pub fn f(_) -> Int {\n",
        "  1\n",
        "}\n",
        "pub fn main() -> Int { f(1) }\n",
    );
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    let report = Compiler::type_project(vec![module]);
    let repeated = report
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code() == "L2020")
        .collect::<Vec<_>>();
    assert_eq!(repeated.len(), 2, "{:?}", report.diagnostics());
    let second_clause = source.match_indices("pub fn f").nth(1).unwrap().0;
    let mut fixed = source.to_owned();
    for diagnostic in repeated.iter().rev() {
        let fix = &diagnostic.fixes()[0];
        assert_eq!(fix.replacement(), "");
        assert!(fix.range().start() as usize >= second_clause);
        fixed.replace_range(fix.range().start() as usize..fix.range().end() as usize, "");
    }
    assert!(check_codes(&fixed).is_empty(), "{fixed}");
}

// PC-11
#[test]
fn clauses_separated_by_another_declaration_are_rejected() {
    let source = concat!(
        "fn f(0) -> Int {\n",
        "  0\n",
        "}\n",
        "struct Money {\n",
        "  cents: Int\n",
        "}\n",
        "fn f(_) {\n",
        "  1\n",
        "}\n",
        "pub fn main() -> Int { f(1) }\n",
    );
    // The first group alone does not cover `Int`, but separated groups are not
    // checked for coverage.
    assert_eq!(check_codes(source), ["E_SEPARATED_FUNCTION_CLAUSES"]);
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    let report = Compiler::type_project(vec![module]);
    let separated = &report.diagnostics()[0];
    let at = |start: u32| start as usize;
    assert_eq!(at(separated.range().start()), source.rfind("f(_)").unwrap());
    let labels = separated
        .labels()
        .iter()
        .map(|label| at(label.range().start()))
        .collect::<Vec<_>>();
    assert_eq!(labels, [source.find("f(0)").unwrap()]);
}

#[test]
fn a_non_function_named_like_a_function_is_a_duplicate_symbol() {
    let codes = check_codes(concat!(
        "fn Money(0) -> Int {\n",
        "  0\n",
        "}\n",
        "struct Money {\n",
        "  cents: Int\n",
        "}\n",
        "pub fn main() -> Int { 0 }\n",
    ));
    assert!(
        codes.contains(&"E_DUPLICATE_SYMBOL".to_owned()),
        "{codes:?}"
    );
}

// PC-12
#[test]
fn needs_from_the_first_clause_are_in_scope_in_every_clause() {
    let source = concat!(
        "interface Database {\n",
        "  ping: fn() -> Bool\n",
        "}\n",
        "fn route(\"health\") -> Text needs { db: Database } {\n",
        "  case db.ping() {\n",
        "    True -> \"ready\"\n",
        "    False -> \"unavailable\"\n",
        "  }\n",
        "}\n",
        "fn route(_) {\n",
        "  case db.ping() {\n",
        "    True -> \"not found\"\n",
        "    False -> \"unavailable\"\n",
        "  }\n",
        "}\n",
        "const up = { ping: fn() { True } }\n",
        "pub fn main() -> List<Text> {\n",
        "  [route(\"health\"), route(\"orders\")] using { db: up }\n",
        "}\n",
    );
    assert_eq!(
        run_both("clauses-needs", source),
        "\"outcome\":\"success\",\"value\":[\"ready\",\"not found\"]"
    );
    let callable = inspect_at(source, "route(_)", 0);
    let needs = callable.callable().expect("the group contract").needs();
    assert_eq!(needs.len(), 1);
    assert_eq!(needs[0].name(), "db");
}

// PC-13
#[test]
fn a_clause_cannot_inspect_a_generic_parameter() {
    let codes = check_codes(concat!(
        "fn f<T>(value: T) -> Int {\n",
        "  1\n",
        "}\n",
        "fn f(0) {\n",
        "  0\n",
        "}\n",
        "pub fn main() -> Int { f(1) }\n",
    ));
    assert!(codes.contains(&"T4230".to_owned()), "{codes:?}");
    // In the reverse order the literal comes before the clause that names `T`;
    // it is still T4230 on the literal, not a cross-clause T4281.
    let reversed = concat!(
        "fn f<T>(0) -> Int {\n",
        "  1\n",
        "}\n",
        "fn f(_: T) {\n",
        "  0\n",
        "}\n",
        "pub fn main() -> Int { f(1) }\n",
    );
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", reversed))
        .expect("valid module path");
    let report = Compiler::type_project(vec![module]);
    let [literal] = report.diagnostics() else {
        panic!("{:?}", report.diagnostics());
    };
    assert_eq!(literal.code(), "T4230");
    assert_eq!(
        literal.range().start() as usize,
        reversed.find("(0)").unwrap() + 1
    );
}

#[test]
fn a_typed_binding_in_a_later_clause_may_name_a_generic_of_the_first() {
    let source = concat!(
        "fn last<T>([]) -> Option<T> {\n",
        "  None\n",
        "}\n",
        "fn last([.., item: T]) {\n",
        "  Some(item)\n",
        "}\n",
        "pub fn main() -> Option<Int> { last([1, 2, 3]) }\n",
    );
    assert_eq!(
        run_both("clauses-generic-typed-binding", source),
        "\"outcome\":\"success\",\"value\":{\"type\":\"Option\",\"variant\":\"Some\",\"fields\":{\"value\":3}}"
    );
}

// PC-14
#[test]
fn a_clause_parameter_is_never_an_outcome() {
    let messages = coded_messages(concat!(
        "fn f(Failure(e)) -> Int {\n",
        "  0\n",
        "}\n",
        "fn f(_) {\n",
        "  1\n",
        "}\n",
        "pub fn main() -> Int { f(1) }\n",
    ));
    assert!(
        messages
            .iter()
            .any(|(code, message)| code == "T4282" && message.contains("`case`")),
        "{messages:?}"
    );
    // The outcome pattern is rejected: no pattern-shape error, and it neither
    // counts as a catch-all nor covers the next clause.
    let codes = messages
        .iter()
        .map(|(code, _)| code.as_str())
        .collect::<Vec<_>>();
    assert_eq!(codes, ["T4282"]);
    for single in [
        "fn f(Failure(e)) -> Int {\n  0\n}\npub fn main() -> Int { f(1) }\n",
        "pub fn main() -> Int {\n  let g = fn(Failure(e)) { 0 }\n  g(1)\n}\n",
    ] {
        assert_eq!(check_codes(single), ["T4282"], "{single}");
    }
}

// PC-15
#[test]
fn a_clause_group_is_one_function_value() {
    let source = concat!(
        "fn grade(score) if score >= 90 {\n",
        "  \"A\"\n",
        "}\n",
        "fn grade(_) {\n",
        "  \"C\"\n",
        "}\n",
        "fn apply(transform: fn(Int) -> Text, value: Int) -> Text {\n",
        "  transform(value)\n",
        "}\n",
        "pub fn main() -> List<Text> {\n",
        "  let label = 95 |> grade\n",
        "  [label, apply(grade, 10)]\n",
        "}\n",
    );
    assert_eq!(
        run_both("clauses-function-value", source),
        "\"outcome\":\"success\",\"value\":[\"A\",\"C\"]"
    );
}

// PC-16
#[test]
fn break_in_a_clause_body_cannot_leave_a_callers_loop() {
    let codes = check_codes(concat!(
        "fn step(0) -> Int {\n",
        "  break 0\n",
        "}\n",
        "fn step(n) {\n",
        "  n - 1\n",
        "}\n",
        "pub fn main() -> Int {\n",
        "  loop n = 3 {\n",
        "    step(n)\n",
        "  }\n",
        "}\n",
    ));
    assert!(
        codes.contains(&"E_BREAK_OUTSIDE_LOOP".to_owned()),
        "{codes:?}"
    );
}

// PC-17
#[test]
fn a_clause_cannot_bind_one_name_twice() {
    let codes = check_codes(concat!(
        "fn same(x, x) -> Bool {\n",
        "  True\n",
        "}\n",
        "pub fn main() -> Bool { same(1, 1) }\n",
    ));
    assert!(codes.contains(&"E_DUPLICATE_LOCAL".to_owned()), "{codes:?}");
}

#[test]
fn a_single_function_with_a_refutable_parameter_pattern_is_not_exhaustive() {
    let messages = coded_messages(concat!(
        "fn unwrap(Some(value)) -> Int {\n",
        "  value\n",
        "}\n",
        "pub fn main() -> Int { unwrap(Some(1)) }\n",
    ));
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0].0, "T4231");
    assert!(messages[0].1.contains("`unwrap(None)`"), "{messages:?}");
}

// A function literal cannot gain a clause, so its gap points at the pattern and
// suggests `case` instead.
#[test]
fn a_function_literal_with_a_refutable_parameter_suggests_case() {
    let source = concat!(
        "pub fn main() -> Int {\n",
        "  let f = fn(Some(x)) { x }\n",
        "  f(Some(1))\n",
        "}\n",
    );
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    let report = Compiler::type_project(vec![module]);
    let [gap] = report.diagnostics() else {
        panic!("{:?}", report.diagnostics());
    };
    assert_eq!(gap.code(), "T4231");
    assert!(gap.message().contains("`fn(None)`"), "{}", gap.message());
    assert!(gap.message().contains("`case`"), "{}", gap.message());
    assert!(!gap.message().contains("Add a clause"), "{}", gap.message());
    let range = gap.range();
    assert_eq!(
        &source[range.start() as usize..range.end() as usize],
        "Some(x)"
    );
}

#[test]
fn recursion_through_clauses_refers_to_the_whole_group() {
    let source = concat!(
        "fn sum([]) -> Int {\n",
        "  0\n",
        "}\n",
        "fn sum([first, ..rest]) {\n",
        "  first + sum(rest)\n",
        "}\n",
        "pub fn main() -> Int { sum([1, 2, 3, 4]) }\n",
    );
    assert_eq!(
        run_both("clauses-recursion", source),
        "\"outcome\":\"success\",\"value\":10"
    );
}

// ts-8acfe5: an anonymous record type is written `{ name: Type, ... }` in
// parameter, return, field, and function types; it is the type of a record
// value with exactly those fields, in any order.
#[test]
fn record_types_are_written_in_parameters_returns_and_fields() {
    let source = concat!(
        "struct Holder {\n",
        "  point: { x: Int, y: Int }\n",
        "}\n",
        "fn sum(point: { y: Int, x: Int }) -> Int { point.x + point.y }\n",
        "fn origin() -> { x: Int, y: Int } { { x: 1, y: 2 } }\n",
        "fn apply(f: fn({ x: Int, y: Int }) -> Int, point: { x: Int, y: Int }) -> Int {\n",
        "  f(point)\n",
        "}\n",
        "pub fn main() -> Int {\n",
        "  let holder = Holder({ y: 20, x: 10 })\n",
        "  sum(origin()) + apply(sum, holder.point)\n",
        "}\n",
    );
    assert_eq!(
        run_both("record-types", source),
        "\"outcome\":\"success\",\"value\":33"
    );
}

#[test]
fn a_record_type_admits_only_records_with_its_fields() {
    let call = |argument: &str| {
        check_codes(&format!(
            "fn sum(point: {{ x: Int, y: Int }}) -> Int {{ point.x + point.y }}\npub fn main() -> Int {{ sum({argument}) }}\n"
        ))
    };
    assert!(call("{ x: 1, y: 2 }").is_empty());
    assert_eq!(call("{ x: 1 }"), ["T4203"]);
    assert_eq!(call("{ x: 1, y: 2, z: 3 }"), ["T4203"]);
    assert_eq!(call("{ x: 1, y: \"two\" }"), ["T4203"]);
    assert_eq!(
        check_codes(
            "fn f(point: { x: Int, x: Int }) -> Int { point.x }\npub fn main() -> Int { 0 }\n"
        ),
        ["T4211"]
    );
    assert_eq!(
        check_codes("fn f(point: { x: Missing }) -> Int { 0 }\npub fn main() -> Int { 0 }\n"),
        ["E_UNRESOLVED_TYPE"]
    );
}

// Tuple types `(A, B)` and record types `{ name: Type }` nest inside each other.
#[test]
fn tuple_and_record_types_nest_in_type_positions() {
    let source = concat!(
        "fn split(pair: ({ x: Int }, Int)) -> { both: (Int, Int) } {\n",
        "  let (point, n) = pair\n",
        "  { both: (point.x, n) }\n",
        "}\n",
        "pub fn main() -> Int {\n",
        "  let (a, b) = split(({ x: 30 }, 4)).both\n",
        "  a + b\n",
        "}\n",
    );
    assert_eq!(
        run_both("tuple-record-types", source),
        "\"outcome\":\"success\",\"value\":34"
    );
}

// Formatting a member import keeps it tight and keeps it a member import.
#[test]
fn a_formatted_member_import_stays_tight_and_still_checks() {
    let source = concat!(
        "import std/map.{Map}\n",
        "import std/io.{ FileError,\n  Utf8Error }\n",
        "fn size(map: Map<Int, Int>) -> Int { 0 }\n",
        "fn fail(error: FileError, other: Utf8Error) -> Int { 0 }\n",
        "pub fn main() -> Int { 0 }\n",
    );
    assert!(check_codes(source).is_empty(), "{:?}", check_codes(source));
    let formatted = Compiler::format(SourceSnapshot::from_text("main.chimi", source));
    assert!(
        !formatted.is_blocked(),
        "{}",
        formatted.render_diagnostics()
    );
    assert!(
        formatted
            .text()
            .starts_with("import std/map.{Map}\nimport std/io.{FileError, Utf8Error}\n"),
        "{}",
        formatted.text()
    );
    assert!(
        check_codes(formatted.text()).is_empty(),
        "{:?}",
        check_codes(formatted.text())
    );
}

// ts-65f575: `chimi run` holds as deep a recursion as a native build does.
#[test]
fn deep_runtime_recursion_completes_on_both_backends() {
    let source = concat!(
        "fn count(n: Int) -> Int {\n",
        "  case n == 0 {\n",
        "    True -> 0\n",
        "    False -> 1 + count(n - 1)\n",
        "  }\n",
        "}\n",
        "pub fn main() -> Int { count(5000) }\n",
    );
    assert_eq!(
        run_both("deep-recursion", source),
        "\"outcome\":\"success\",\"value\":5000"
    );
}

// Formatting: one clause group reads as one declaration and keeps its order.
#[test]
fn the_formatter_keeps_a_clause_group_together_in_source_order() {
    let source = concat!(
        "fn f(1) -> Int {\n",
        "  10\n",
        "}\n",
        "\n",
        "// the catch-all\n",
        "fn f(_) {\n",
        "  0\n",
        "}\n",
        "fn g() -> Int {\n",
        "  f(1)\n",
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
            "fn f(1) -> Int {\n",
            "  10\n",
            "}\n",
            "// the catch-all\n",
            "fn f(_) {\n",
            "  0\n",
            "}\n",
            "\n",
            "fn g() -> Int {\n",
            "  f(1)\n",
            "}\n",
        )
    );
}

// Inspection reports the group contract and the clause position for any clause.
#[test]
fn inspecting_any_clause_reports_the_group_and_the_clause_position() {
    let source =
        format!("import std/int as Int\n{DESCRIBE}pub fn main() -> Text {{ describe(1) }}\n");
    for (needle, index) in [
        ("describe(0)", 1),
        ("describe(1) {", 2),
        ("describe(count) if", 3),
        ("describe(count) {", 4),
    ] {
        let report = inspect_at(&source, needle, 0);
        let clause = report.clause().expect("a clause head");
        assert_eq!((clause.index(), clause.count()), (index, 4), "{needle}");
        assert_eq!(
            report.callable().map(chimi::CallableInspection::display),
            Some("fn(Int) -> Text errors CountError"),
            "{needle}"
        );
    }
}

// ts-061593 (CHI-158): the name in an ordinary function's declaration reports
// the function's contract, as a call site does; it is no clause, so there is no
// clause position. Before, only call sites and clause groups reported it.
#[test]
fn inspecting_a_single_clause_function_name_reports_its_contract() {
    let source = concat!(
        "enum Shortfall {\n  Short\n}\n",
        "fn total(a: Int, b: Int) -> Int errors Shortfall {\n",
        "  case a + b < 0 {\n    True -> Failure(Shortfall.Short)\n    False -> a + b\n  }\n",
        "}\n",
        "pub fn main() -> Int errors Shortfall { total(1, 2) }\n",
    );
    for occurrence in [0, 1] {
        let report = inspect_at(source, "total", occurrence);
        assert_eq!(
            report.callable().map(chimi::CallableInspection::display),
            Some("fn(Int, Int) -> Int errors Shortfall"),
            "occurrence {occurrence}"
        );
        assert!(report.clause().is_none(), "occurrence {occurrence}");
    }
}

// Struct, alias, and literal patterns (patterns-and-function-clauses.md, "Patterns").

#[test]
fn a_struct_pattern_destructures_fields_by_position() {
    let source = concat!(
        "struct Money {\n",
        "  cents: Int\n",
        "  currency: Text\n",
        "}\n",
        "fn cents(Money(amount, _)) -> Int {\n",
        "  amount\n",
        "}\n",
        "fn label(money: Money) -> Text {\n",
        "  case money {\n",
        "    Money(0, currency) -> currency\n",
        "    Money(_, _) -> \"some\"\n",
        "  }\n",
        "}\n",
        "pub fn main() -> List<Text> {\n",
        "  let total = cents(Money(250, \"EUR\")) + cents(Money(5, \"EUR\"))\n",
        "  let doubled = fn(Money(amount, _)) { amount * 2 }\n",
        "  [Int.to_text(total + doubled(Money(10, \"EUR\"))), label(Money(0, \"USD\")), label(Money(1, \"USD\"))]\n",
        "}\n",
    );
    let source = format!("import std/int as Int\n{source}");
    assert_eq!(
        run_both("struct-patterns", &source),
        "\"outcome\":\"success\",\"value\":[\"275\",\"USD\",\"some\"]"
    );
}

#[test]
fn a_struct_pattern_with_the_wrong_field_count_is_a_type_error() {
    let codes = check_codes(concat!(
        "struct Money {\n",
        "  cents: Int\n",
        "}\n",
        "fn cents(money: Money) -> Int {\n",
        "  case money {\n",
        "    Money(a, b) -> a\n",
        "  }\n",
        "}\n",
        "pub fn main() -> Int { cents(Money(1)) }\n",
    ));
    assert_eq!(codes, ["T4230"]);
}

#[test]
fn a_refutable_field_makes_a_struct_pattern_refutable() {
    let messages = coded_messages(concat!(
        "struct Money {\n",
        "  cents: Int\n",
        "}\n",
        "fn free(Money(0)) -> Bool {\n",
        "  True\n",
        "}\n",
        "pub fn main() -> Bool { free(Money(0)) }\n",
    ));
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0].0, "T4231");
    assert!(messages[0].1.contains("`free(Money(1))`"), "{messages:?}");
}

// TP-04
#[test]
fn a_struct_pattern_outside_its_module_is_private() {
    let money = ProjectSource::new(
        "money",
        SourceSnapshot::from_text(
            "money.chimi",
            concat!(
                "pub struct Money {\n",
                "  cents: Int\n",
                "}\n",
                "pub fn zero() -> Money { Money(0) }\n",
            ),
        ),
    )
    .expect("valid module path");
    for pattern in ["Money.Money(cents)", "Money(cents)"] {
        let main = ProjectSource::new(
            "main",
            SourceSnapshot::from_text(
                "main.chimi",
                format!(
                    concat!(
                        "import money as Money\n",
                        "import money.{{Money}}\n",
                        "fn cents(value: Money) -> Int {{\n",
                        "  case value {{\n",
                        "    {} -> cents\n",
                        "  }}\n",
                        "}}\n",
                        "pub fn main() -> Int {{ cents(Money.zero()) }}\n",
                    ),
                    pattern
                ),
            ),
        )
        .expect("valid module path");
        let codes = Compiler::type_project(vec![money.clone(), main])
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(codes, ["E_PRIVATE_STRUCT_CONSTRUCTOR"], "{pattern}");
    }
}

// TP-05
#[test]
fn a_labelled_pattern_is_a_syntax_error_about_positional_patterns() {
    let messages = coded_messages(concat!(
        "struct Money {\n",
        "  cents: Int\n",
        "}\n",
        "fn free(money: Money) -> Bool {\n",
        "  case money {\n",
        "    Money(cents: 2500) -> True\n",
        "    _ -> False\n",
        "  }\n",
        "}\n",
        "pub fn main() -> Bool { free(Money(0)) }\n",
    ));
    assert!(
        messages
            .iter()
            .any(|(code, message)| code == "S1001" && message.contains("positional")),
        "{messages:?}"
    );
}

// TP-06
#[test]
fn a_negative_literal_pattern_matches_negative_one() {
    let source = concat!(
        "fn sign(-1) -> Text {\n",
        "  \"minus one\"\n",
        "}\n",
        "fn sign(_) {\n",
        "  \"other\"\n",
        "}\n",
        "fn sign_case(value: Int) -> Text {\n",
        "  case value {\n",
        "    -1 -> \"minus one\"\n",
        "    _ -> \"other\"\n",
        "  }\n",
        "}\n",
        "pub fn main() -> List<Text> { [sign(-1), sign(1), sign_case(-1), sign_case(1)] }\n",
    );
    assert_eq!(
        run_both("negative-literal-pattern", source),
        "\"outcome\":\"success\",\"value\":[\"minus one\",\"other\",\"minus one\",\"other\"]"
    );
}

// TP-06 in a later arm: a line starting `-1 ->` begins a new arm, not
// `10 - 1`; a `-` line without `->` still continues a subtraction.
#[test]
fn a_negative_literal_pattern_may_follow_another_arm() {
    let source = concat!(
        "fn sign(value: Int) -> Int {\n",
        "  case value {\n",
        "    0 -> 10\n",
        "    -1 -> 20\n",
        "    -2 if value < 0 -> 30\n",
        "    _ -> 40\n",
        "      - 1\n",
        "  }\n",
        "}\n",
        "pub fn main() -> List<Int> { [sign(0), sign(-1), sign(-2), sign(5)] }\n",
    );
    assert_eq!(
        run_both("negative-literal-later-arm", source),
        "\"outcome\":\"success\",\"value\":[10,20,30,39]"
    );
}

// TP-07
#[test]
fn text_patterns_match_bytes_without_normalization() {
    let source = concat!(
        "fn accent(\"\u{e9}\") -> Bool {\n",
        "  True\n",
        "}\n",
        "fn accent(_) {\n",
        "  False\n",
        "}\n",
        "pub fn main() -> List<Bool> { [accent(\"e\u{301}\"), accent(\"\u{e9}\")] }\n",
    );
    assert_eq!(
        run_both("text-pattern-bytes", source),
        "\"outcome\":\"success\",\"value\":[false,true]"
    );
}

#[test]
fn an_alias_binds_the_whole_matched_value() {
    let source = concat!(
        "fn outer(Some([first, ..]) as whole) -> Option<List<Int>> {\n",
        "  case first > 0 {\n",
        "    True -> whole\n",
        "    False -> None\n",
        "  }\n",
        "}\n",
        "fn outer(_) {\n",
        "  None\n",
        "}\n",
        "pub fn main() -> List<Option<List<Int>>> {\n",
        "  [outer(Some([1, 2])), outer(Some([0])), outer(None)]\n",
        "}\n",
    );
    assert_eq!(
        run_both("alias-pattern", source),
        concat!(
            "\"outcome\":\"success\",\"value\":[",
            "{\"type\":\"Option\",\"variant\":\"Some\",\"fields\":{\"value\":[1,2]}},",
            "{\"type\":\"Option\",\"variant\":\"None\",\"fields\":{}},",
            "{\"type\":\"Option\",\"variant\":\"None\",\"fields\":{}}]"
        )
    );
}

// FP-08 (ordinary position): a typed binding is an annotation, never a type test.
#[test]
fn a_typed_binding_of_another_type_is_rejected() {
    let messages = coded_messages(concat!(
        "fn describe(value: Int) -> Int {\n",
        "  case value {\n",
        "    count: Text -> 0\n",
        "  }\n",
        "}\n",
        "pub fn main() -> Int { describe(1) }\n",
    ));
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0].0, "T4283");
    assert!(messages[0].1.contains("run time"), "{messages:?}");
}

// FP-08 (clause head): the annotation joins the cross-clause unification, so a
// conflict is between clauses (T4281), not a typed-binding mismatch (T4283).
#[test]
fn a_typed_binding_of_another_type_in_a_clause_head_conflicts_between_clauses() {
    let messages = coded_messages(concat!(
        "fn describe(0) -> Int {\n",
        "  0\n",
        "}\n",
        "fn describe(count: Text) {\n",
        "  1\n",
        "}\n",
        "pub fn main() -> Int { describe(1) }\n",
    ));
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0].0, "T4281");
}

// Execution: a guard runs only for a clause whose patterns matched, in clause
// order. `10 / n` would defect for `n == 0`, which the first clause takes.
#[test]
fn a_clause_guard_runs_only_after_its_patterns_match() {
    let source = concat!(
        "fn f(0) -> Int {\n",
        "  100\n",
        "}\n",
        "fn f(n) if 10 / n > 1 {\n",
        "  1\n",
        "}\n",
        "fn f(_) {\n",
        "  2\n",
        "}\n",
        "pub fn main() -> List<Int> { [f(0), f(3), f(20)] }\n",
    );
    assert_eq!(
        run_both("clause-guard-order", source),
        "\"outcome\":\"success\",\"value\":[100,1,2]"
    );
}

// Execution: a multi-clause function is one callable, so calling it traces one
// `call` and one `match`, on both backends' full traces.
#[test]
fn a_clause_call_traces_one_call_and_one_match() {
    let source = concat!(
        "enum Shape {\n",
        "  Circle(radius: Int)\n",
        "  Rectangle(width: Int, height: Int)\n",
        "}\n",
        "fn area(Circle(radius)) -> Int {\n",
        "  3 * radius * radius\n",
        "}\n",
        "fn area(Rectangle(width, height)) {\n",
        "  width * height\n",
        "}\n",
        "pub fn main() -> Int { area(Rectangle(3, 4)) }\n",
    );
    let scratch = scratch_directory("clause-trace");
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
        let started = |operation: &str| {
            trace
                .lines()
                .filter(|line| line.contains("\"event\":\"operation_started\""))
                .filter(|line| line.contains(&format!("\"operation\":\"{operation}\"")))
                .count()
        };
        assert_eq!(started("call"), 1, "{backend}: {trace}");
        assert_eq!(started("match"), 1, "{backend}: {trace}");
    }
    assert!(interpreted.ends_with("\"outcome\":\"success\",\"value\":12}\n"));
    std::fs::remove_dir_all(scratch).unwrap();
}

// PC-10: repeated generics and a repeated `needs` are L2020 too, each with its
// own range in the later clause.
#[test]
fn a_later_clause_cannot_repeat_generics_or_needs() {
    let source = concat!(
        "interface Db {\n",
        "  ping: fn() -> Bool\n",
        "}\n",
        "fn f<T>(Some(x: T)) -> Int needs { db: Db } {\n",
        "  0\n",
        "}\n",
        "fn f<T>(None) needs { db: Db } {\n",
        "  1\n",
        "}\n",
        "pub fn main() -> Int { 0 }\n",
    );
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    let report = Compiler::type_project(vec![module]);
    let repeated = report
        .diagnostics()
        .iter()
        .map(|diagnostic| {
            let range = diagnostic.range();
            (
                diagnostic.code(),
                &source[range.start() as usize..range.end() as usize],
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(repeated, [("L2020", "<T>"), ("L2020", "needs { db: Db }")]);
}

// The formatter keeps update fields in source order: order is evaluation order.
#[test]
fn formatting_keeps_update_fields_in_source_order() {
    let source = concat!(
        "struct Config {\n",
        "  name: Text\n",
        "  jobs: Int\n",
        "}\n",
        "pub fn main() -> Config {\n",
        "  let base = Config(\"a\", 1)\n",
        "  let updated = {   ..base,  name: \"b\",jobs: 2 }\n",
        "  updated\n",
        "}\n",
    );
    let formatted = Compiler::format(SourceSnapshot::from_text("main.chimi", source));
    assert!(
        formatted.text().contains("..base, name: \"b\", jobs: 2"),
        "{}",
        formatted.text()
    );
    let reordered = source.replace("name: \"b\",jobs: 2", "jobs: 2, name: \"b\"");
    let formatted = Compiler::format(SourceSnapshot::from_text("main.chimi", reordered.as_str()));
    assert!(
        formatted.text().contains("..base, jobs: 2, name: \"b\""),
        "{}",
        formatted.text()
    );
}

// Failure-type patterns (patterns-and-function-clauses.md, "Typed bindings and
// failure-type patterns").

const FAILURE_KINDS: &str = concat!(
    "enum FileProblem {\n",
    "  Missing(path: Text)\n",
    "  Denied(path: Text)\n",
    "  TooLong(path: Text, limit: Int)\n",
    "}\n",
    "enum ParseProblem {\n",
    "  Empty\n",
    "  Garbled(line: Int)\n",
    "}\n",
    "fn load(choice: Int) -> Text {\n",
    "  case choice {\n",
    "    0 -> Failure(FileProblem.Missing(\"a\"))\n",
    "    1 -> Failure(FileProblem.TooLong(\"b\", 9))\n",
    "    2 -> Failure(ParseProblem.Garbled(3))\n",
    "    3 -> Failure(ParseProblem.Empty)\n",
    "    _ -> \"loaded\"\n",
    "  }\n",
    "}\n",
);

fn type_contract(source: &str, function: &str) -> (Vec<String>, Option<String>) {
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    let report = Compiler::type_project(vec![module]);
    let codes = report
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code().to_owned())
        .collect();
    (codes, report.contract("main", function))
}

// FP-01: the trace summarizer's `main`, which selects failures with one arm per
// failure type, has the same contract as that `main` with one arm per variant.
#[test]
fn the_trace_summarizer_main_with_failure_type_arms_keeps_its_contract() {
    let original = include_str!("../../../programs/chimi-trace-summary/app/main.chimi");
    let start = original
        .find("      Failure(error: TraceError) -> report_trace(error)\n")
        .expect("the failure-type arms");
    let end = original.find("    _ -> {\n").expect("the usage arm");
    let mut arms = String::new();
    for variant in [
        "InvalidArgumentCount(actual)",
        "BlankLine(line)",
        "InvalidJson(line)",
        "InvalidSchema(line)",
        "InvalidRecord(line)",
        "InvalidEvent(line)",
        "InvalidSequence(line)",
        "MixedExecution(line)",
        "MixedComputation(line)",
        "RecordAfterOutcome(line)",
        "MissingOutcome(line)",
    ] {
        arms.push_str(&format!(
            "      Failure({variant}) -> report_trace({variant})\n"
        ));
    }
    for (variant, message) in [
        ("Open(path, message)", "trace summary file IO failed"),
        ("Read(path, message)", "trace summary file IO failed"),
        ("Close(path, message)", "trace summary file IO failed"),
        (
            "LineTooLong(path, limit)",
            "trace summary input record is too large",
        ),
        (
            "InvalidUtf8(path, line)",
            "trace summary input is not UTF-8",
        ),
    ] {
        arms.push_str(&format!(
            "      Failure({variant}) -> {{\n        console.write_error(\"{message}\")\n        Failure({variant})\n      }}\n"
        ));
    }
    arms.push_str("    }\n");
    let rewritten = format!(
        concat!(
            "import std/io.{{Close}}\nimport std/io.{{InvalidUtf8}}\n",
            "import std/io.{{Open}}\nimport std/io.{{Read}}\n{}{}{}"
        ),
        &original[..start],
        arms,
        &original[end..]
    );
    let contract = |source: &str| {
        let module = ProjectSource::new(
            "app/main",
            SourceSnapshot::from_text("app/main.chimi", source),
        )
        .expect("valid module path");
        let report = Compiler::type_project(vec![module]);
        assert!(report.is_success(), "{:?}", report.diagnostics());
        report
            .contract("app/main", "main")
            .expect("main has a contract")
    };
    assert_eq!(contract(&rewritten), contract(original));
}

// FP-02
#[test]
fn a_failure_type_absent_from_a_closed_row_is_rejected() {
    let (codes, _) = type_contract(
        &format!(
            "{FAILURE_KINDS}{}",
            concat!(
                "fn only_parse() -> Text {\n",
                "  Failure(ParseProblem.Empty)\n",
                "}\n",
                "pub fn main() -> Text {\n",
                "  case only_parse() {\n",
                "    Success(text) -> text\n",
                "    Failure(error: FileProblem) -> \"file\"\n",
                "    Failure(_) -> \"other\"\n",
                "  }\n",
                "}\n",
            )
        ),
        "main",
    );
    assert_eq!(codes, ["T4224"]);
}

// FP-03 and FP-04: a generic recovery subtracts the failure type from the open row,
// and only when the arm has no guard.
#[test]
fn a_generic_recovery_subtracts_the_failure_type_from_its_row() {
    let recover = |guard: &str| {
        format!(
            concat!(
                "{}",
                "fn recover<T, E>(operation: fn() -> T errors E, fallback: T) -> T {{\n",
                "  case operation() {{\n",
                "    Success(value) -> value\n",
                "    Failure(_error: FileProblem){} -> fallback\n",
                "  }}\n",
                "}}\n",
                "fn read() -> Text {{\n",
                "  recover(fn() {{ load(0) }}, \"fallback\")\n",
                "}}\n",
                "pub fn main() -> Text {{ read() }}\n",
            ),
            FAILURE_KINDS, guard
        )
    };
    let (codes, contract) = type_contract(&recover(""), "read");
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(
        contract.as_deref(),
        Some("fn() -> Text errors ParseProblem")
    );
    let (codes, contract) = type_contract(&recover(" if True"), "read");
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(
        contract.as_deref(),
        Some("fn() -> Text errors { FileProblem.Missing, FileProblem.TooLong, ParseProblem }")
    );
}

// FP-05, and the runtime selection: an earlier variant arm wins, the failure-type
// arm takes the remaining variants, and an alias re-raises exactly its variant.
#[test]
fn failure_type_arms_select_variants_in_order_on_both_paths() {
    let source = format!(
        "{FAILURE_KINDS}{}",
        concat!(
            "fn classify(choice: Int) -> Text {\n",
            "  case load(choice) {\n",
            "    Success(text) -> text\n",
            "    Failure(FileProblem.TooLong(_, _) as error) -> \"too long\"\n",
            "    Failure(error: FileProblem) -> \"file\"\n",
            "    Failure(_: ParseProblem) -> \"parse\"\n",
            "  }\n",
            "}\n",
            "fn reraise(choice: Int) -> Text {\n",
            "  case load(choice) {\n",
            "    Success(text) -> text\n",
            "    Failure(FileProblem.TooLong(_, _) as error) -> Failure(error)\n",
            "    Failure(_) -> \"handled\"\n",
            "  }\n",
            "}\n",
            "pub fn main() -> List<Text> {\n",
            "  [classify(0), classify(1), classify(2), classify(3), classify(4), reraise(0), reraise(1)]\n",
            "}\n",
        )
    );
    let (codes, contract) = type_contract(&source, "reraise");
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(
        contract.as_deref(),
        Some("fn(Int) -> Text errors FileProblem.TooLong")
    );
    assert_eq!(
        run_both("failure-type-arms", &source),
        concat!(
            "\"outcome\":\"expected_failure\",",
            "\"error\":{\"type\":\"FileProblem\",\"variant\":\"TooLong\",",
            "\"fields\":{\"path\":\"b\",\"limit\":9}}",
        )
    );
    let succeeding = source.replace(", reraise(1)]", "]");
    assert_eq!(
        run_both("failure-type-arms-success", &succeeding),
        "\"outcome\":\"success\",\"value\":[\"file\",\"too long\",\"parse\",\"parse\",\"loaded\",\"handled\"]"
    );
}

// FP-06
#[test]
fn a_variant_arm_after_its_failure_type_arm_never_runs() {
    let (codes, _) = type_contract(
        &format!(
            "{FAILURE_KINDS}{}",
            concat!(
                "pub fn main() -> Text {\n",
                "  case load(0) {\n",
                "    Success(text) -> text\n",
                "    Failure(error: FileProblem) -> \"file\"\n",
                "    Failure(FileProblem.TooLong(_, _)) -> \"too long\"\n",
                "    Failure(_) -> \"other\"\n",
                "  }\n",
                "}\n",
            )
        ),
        "main",
    );
    assert_eq!(codes, ["T4232"]);
}

// FP-07
#[test]
fn a_case_over_a_bound_failure_set_may_select_by_failure_type() {
    let source = |arms: &str| {
        format!(
            concat!(
                "{}",
                "fn describe(choice: Int) -> Text {{\n",
                "  case load(choice) {{\n",
                "    Success(text) -> text\n",
                "    Failure(error) -> case error {{\n",
                "{}",
                "    }}\n",
                "  }}\n",
                "}}\n",
                "pub fn main() -> List<Text> {{ [describe(1), describe(2)] }}\n",
            ),
            FAILURE_KINDS, arms
        )
    };
    let both = source(concat!(
        "      file_error: FileProblem -> \"file\"\n",
        "      parse_error: ParseProblem -> \"parse\"\n",
    ));
    assert_eq!(
        run_both("failure-type-nested-case", &both),
        "\"outcome\":\"success\",\"value\":[\"file\",\"parse\"]"
    );
    let (codes, _) = type_contract(
        &source("      file_error: FileProblem -> \"file\"\n"),
        "describe",
    );
    assert_eq!(codes, ["T4231"]);
}

// FP-09: a defect is not a failure, so no failure-type arm sees it.
#[test]
fn a_defect_bypasses_failure_type_arms() {
    let source = format!(
        "{FAILURE_KINDS}{}",
        concat!(
            "fn explode(divisor: Int) -> Text {\n",
            "  case 10 / divisor {\n",
            "    0 -> load(0)\n",
            "    _ -> \"never\"\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Text {\n",
            "  case explode(0) {\n",
            "    Success(text) -> text\n",
            "    Failure(_: FileProblem) -> \"file\"\n",
            "  }\n",
            "}\n",
        )
    );
    let outcome = run_both("failure-type-defect", &source);
    assert!(outcome.starts_with("\"outcome\":\"defect\""), "{outcome}");
}

// A value narrowed by `Failure(E.A as err)` joins a whole-enum value as the
// whole enum, so a case arm or list element cannot smuggle `E.B` out under a
// row that only lists `E.A`.
#[test]
fn joining_a_narrowed_failure_with_its_enum_widens_the_row() {
    let source = concat!(
        "enum E {\n",
        "  A\n",
        "  B\n",
        "}\n",
        "fn op() -> Int {\n",
        "  Failure(E.A)\n",
        "}\n",
        "fn by_case(flag: Bool) -> Int {\n",
        "  case op() {\n",
        "    Failure(E.A as err) -> {\n",
        "      let chosen = case flag {\n",
        "        True -> err\n",
        "        False -> E.B\n",
        "      }\n",
        "      Failure(chosen)\n",
        "    }\n",
        "    Success(v) -> v\n",
        "  }\n",
        "}\n",
        "fn by_list() -> Int {\n",
        "  case op() {\n",
        "    Failure(E.A as err) -> case [err, E.B] {\n",
        "      [_, second, ..] -> Failure(second)\n",
        "      _ -> 0\n",
        "    }\n",
        "    Success(v) -> v\n",
        "  }\n",
        "}\n",
        "fn label(run: fn() -> Int errors E) -> Int {\n",
        "  case run() {\n",
        "    Failure(E.A) -> 1\n",
        "    Failure(E.B) -> 2\n",
        "    Success(v) -> v\n",
        "  }\n",
        "}\n",
        "pub fn main() -> List<Int> {\n",
        "  [label(fn() { by_case(True) }), label(fn() { by_case(False) }), label(by_list)]\n",
        "}\n",
    );
    for function in ["by_case", "by_list"] {
        let (codes, contract) = type_contract(source, function);
        assert!(codes.is_empty(), "{codes:?}");
        let expected = if function == "by_case" {
            "fn(Bool) -> Int errors E"
        } else {
            "fn() -> Int errors E"
        };
        assert_eq!(contract.as_deref(), Some(expected), "{function}");
    }
    assert_eq!(
        run_both("narrowed-failure-join", source),
        "\"outcome\":\"success\",\"value\":[1,2,2]"
    );
}

// A `Never` arm adds nothing to a case join, and the arms after it still
// widen a narrowed failure value to its whole enum: `chosen` is `E`, so a
// case over it that names only `A` is not exhaustive.
#[test]
fn a_never_arm_keeps_the_narrowed_failure_join_widening() {
    let source = concat!(
        "enum E {\n",
        "  A\n",
        "  B\n",
        "}\n",
        "fn op() -> Int {\n",
        "  Failure(E.A)\n",
        "}\n",
        "fn pick(n: Int) -> Int {\n",
        "  case op() {\n",
        "    Failure(E.A as err) -> {\n",
        "      let chosen = case n {\n",
        "        0 -> Failure(E.A)\n",
        "        1 -> err\n",
        "        _ -> E.B\n",
        "      }\n",
        "      case chosen {\n",
        "        A -> 1\n",
        "      }\n",
        "    }\n",
        "    Success(v) -> v\n",
        "  }\n",
        "}\n",
        "pub fn main() -> Int { pick(2) }\n",
    );
    assert_eq!(check_codes(source), ["T4231"]);
    let covered = source.replace("        A -> 1\n", "        A -> 1\n        B -> 2\n");
    assert_eq!(check_codes(&covered), Vec::<String>::new());
}

// The narrowing is one-way: a whole-enum value is not accepted where only
// the narrowed variants are known to flow.
#[test]
fn a_whole_enum_value_does_not_narrow_to_a_failure_set() {
    let source = concat!(
        "enum E {\n",
        "  A\n",
        "  B\n",
        "}\n",
        "fn op() -> Int {\n",
        "  Failure(E.A)\n",
        "}\n",
        "fn only_a(flag: Bool) -> Int {\n",
        "  case op() {\n",
        "    Failure(E.A as err) -> {\n",
        "      let chosen = case flag {\n",
        "        True -> err\n",
        "        False -> E.B\n",
        "      }\n",
        "      case chosen {\n",
        "        A -> 1\n",
        "      }\n",
        "    }\n",
        "    Success(v) -> v\n",
        "  }\n",
        "}\n",
        "pub fn main() -> Int { only_a(False) }\n",
    );
    assert_eq!(check_codes(source), ["T4231"]);
    // An inference variable that met the narrowed value first widens to the enum.
    let through_parameter = source.replace(
        concat!(
            "      let chosen = case flag {\n",
            "        True -> err\n",
            "        False -> E.B\n",
            "      }\n",
        ),
        concat!(
            "      let pick = fn(x) { x }\n",
            "      let chosen = pick(err)\n",
            "      let other = pick(E.B)\n",
        ),
    );
    assert_eq!(check_codes(&through_parameter), ["T4231"]);
}

// A failure-type pattern inside another pattern cannot select variants (only a
// whole subject or a `Failure` payload expands into variant tests), so it is
// rejected instead of matching every failure. Its rewrite runs on both paths.
#[test]
fn a_nested_failure_type_pattern_is_rejected() {
    let source = |arms: &str| {
        format!(
            concat!(
                "{}",
                "fn file_label(problem: FileProblem) -> Text {{\n",
                "  case problem {{\n",
                "    Missing(path) -> path\n",
                "    Denied(path) -> path\n",
                "    TooLong(path, _) -> path\n",
                "  }}\n",
                "}}\n",
                "fn classify(choice: Int) -> Text {{\n",
                "  case load(choice) {{\n",
                "    Success(text) -> text\n",
                "    Failure(error) -> {}\n",
                "  }}\n",
                "}}\n",
                "pub fn main() -> List<Text> {{ [classify(0), classify(2)] }}\n",
            ),
            FAILURE_KINDS, arms
        )
    };
    for nested in [
        "case Some(error) {\n      Some(file: FileProblem) -> file_label(file)\n      _ -> \"other\"\n    }",
        "case [error] {\n      [file: FileProblem] -> file_label(file)\n      _ -> \"other\"\n    }",
    ] {
        assert_eq!(check_codes(&source(nested)), ["T4230"], "{nested}");
    }
    let rewritten = source(concat!(
        "case Some(error) {\n",
        "      Some(inner) -> case inner {\n",
        "        file: FileProblem -> file_label(file)\n",
        "        _ -> \"other\"\n",
        "      }\n",
        "      None -> \"none\"\n",
        "    }",
    ));
    assert_eq!(
        run_both("nested-failure-type-rewrite", &rewritten),
        "\"outcome\":\"success\",\"value\":[\"a\",\"other\"]"
    );
}

// A failure-type pattern can name only a closed enum today; naming one variant
// as a type teaches the variant pattern instead of a bare resolution error.
#[test]
fn a_variant_named_as_a_failure_type_teaches_the_variant_pattern() {
    let messages = check_messages(&format!(
        "{FAILURE_KINDS}{}",
        concat!(
            "pub fn main() -> Text {\n",
            "  case load(0) {\n",
            "    Success(text) -> text\n",
            "    Failure(error: FileProblem.TooLong) -> \"long\"\n",
            "    Failure(_) -> \"other\"\n",
            "  }\n",
            "}\n",
        )
    ));
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].starts_with("E_UNRESOLVED_TYPE "),
        "{messages:?}"
    );
    assert!(
        messages[0].contains("only a closed enum such as `FileProblem`")
            && messages[0].contains("`Failure(FileProblem.TooLong(_, _) as error)`"),
        "{messages:?}"
    );
}

// ts-8c488d: an unguarded `Failure(_)` arm handles every variant of the row.
#[test]
fn a_wildcard_failure_arm_handles_every_failure() {
    let (codes, contract) = type_contract(
        &format!(
            "{FAILURE_KINDS}{}",
            concat!(
                "fn safe(choice: Int) -> Text {\n",
                "  case load(choice) {\n",
                "    Success(text) -> text\n",
                "    Failure(_) -> \"failed\"\n",
                "  }\n",
                "}\n",
                "pub fn main() -> Text { safe(0) }\n",
            )
        ),
        "safe",
    );
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(contract.as_deref(), Some("fn(Int) -> Text"));
}

// Record update (patterns-and-function-clauses.md, "Record update").

const CONFIG: &str = concat!(
    "struct Config {\n",
    "  name: Text\n",
    "  jobs: Int\n",
    "  verbose: Bool\n",
    "}\n",
);

fn update_codes(update: &str) -> Vec<String> {
    check_codes(&format!(
        "{CONFIG}fn change(config: Config) -> Config {{\n  {update}\n}}\npub fn main() -> Int {{ change(Config(\"a\", 1, False)).jobs }}\n"
    ))
}

// RU-01, and a record base: the result has the base's type and keeps every
// unnamed field.
#[test]
fn an_update_copies_the_base_and_replaces_named_fields() {
    let source = format!(
        "{CONFIG}{}",
        concat!(
            "fn with_jobs(config: Config, jobs: Int) -> Config {\n",
            "  { ..config, jobs: jobs }\n",
            "}\n",
            "pub fn main() -> List<Text> {\n",
            "  let base = Config(\"build\", 2, False)\n",
            "  let updated = { ..with_jobs(base, 4), verbose: True }\n",
            "  let point = { x: 1, y: 2 }\n",
            "  let moved = { ..point, y: 5 }\n",
            "  [\n",
            "    Debug.render(updated),\n",
            "    Debug.render(base),\n",
            "    Debug.render(moved),\n",
            "  ]\n",
            "}\n",
        )
    );
    let source = format!("import std/debug as Debug\n{source}");
    let outcome = run_both("record-update", &source);
    assert!(
        outcome.contains("Config { name: \\\"build\\\", jobs: 4, verbose: True }"),
        "{outcome}"
    );
    assert!(
        outcome.contains("Config { name: \\\"build\\\", jobs: 2, verbose: False }"),
        "{outcome}"
    );
    assert!(outcome.contains("x: 1, y: 5"), "{outcome}");
}

// A line break ends a block item, so an update on the line after a call or a
// record is a new item; formatting must not join it onto the line before.
#[test]
fn an_update_on_the_line_after_a_call_or_record_is_its_own_item() {
    let source = format!(
        "{CONFIG}{}",
        concat!(
            "fn after_call() -> Config {\n",
            "  let base = Config(\"a\", 1, False)\n",
            "  { ..base, jobs: 2 }\n",
            "}\n",
            "fn after_record(base: Config) -> Config {\n",
            "  let changed = { ..base, jobs: 30 }\n",
            "  { ..changed, verbose: True }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let record = after_record(Config(\"b\", 1, False))\n",
            "  after_call().jobs + record.jobs\n",
            "}\n",
        )
    );
    // run_both also runs the formatted source and requires the same outcome.
    assert!(
        run_both("update-after-call", &source).contains("\"value\":32"),
        "update-after-call"
    );
}

// RU-02
#[test]
fn an_update_cannot_name_an_unknown_field() {
    assert_eq!(update_codes("{ ..config, job: 4 }"), ["T4205"]);
    assert_eq!(
        check_codes(concat!(
            "pub fn main() -> Int {\n",
            "  let point = { x: 1 }\n",
            "  { ..point, y: 2 }.x\n",
            "}\n",
        )),
        ["T4205"]
    );
}

// RU-03
#[test]
fn an_update_names_each_field_once() {
    assert_eq!(update_codes("{ ..config, jobs: 1, jobs: 2 }"), ["T4211"]);
}

// RU-04
#[test]
fn an_update_keeps_each_field_type() {
    assert_eq!(update_codes("{ ..config, jobs: \"four\" }"), ["T4203"]);
}

// RU-05
#[test]
fn an_update_without_fields_is_the_base() {
    let source = format!(
        "{CONFIG}fn change(config: Config) -> Config {{\n  {{ ..config }}\n}}\npub fn main() -> Int {{ change(Config(\"a\", 1, False)).jobs }}\n"
    );
    let module = ProjectSource::new(
        "main",
        SourceSnapshot::from_text("main.chimi", source.as_str()),
    )
    .expect("valid module path");
    let report = Compiler::type_project(vec![module]);
    let codes = report
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code())
        .collect::<Vec<_>>();
    assert_eq!(codes, ["L2022"]);
    assert_eq!(report.diagnostics()[0].fixes()[0].replacement(), "config");
}

// `let _: T = value` is an annotated `let` like `let name: T = value`: a mismatch
// is T4203 at the value, not the typed-binding pattern's T4283.
#[test]
fn a_typed_wildcard_let_checks_its_value_like_an_annotated_let() {
    assert_eq!(
        type_diagnostics("fn f() -> Int {\n  let _: Int = \"x\"\n  1\n}\n"),
        [("T4203".to_owned(), "\"x\"".to_owned())]
    );
    assert_eq!(
        run_both(
            "typed-wildcard-let",
            "pub fn main() -> Int {\n  let _: Int = 4\n  5\n}\n",
        ),
        "\"outcome\":\"success\",\"value\":5"
    );
}

// TP-08: a block's value is its final expression, so a block cannot end with a
// `let`. These once ran: the plain `let` returned its value and the
// destructuring one evaluated to `Void`.
#[test]
fn a_block_cannot_end_with_a_let() {
    for (source, label) in [
        ("fn g() -> Int {\n  let a = 1\n}\n", "let a = 1"),
        (
            "fn g() -> Void {\n  let (a, b) = (1, 2)\n}\n",
            "let (a, b) = (1, 2)",
        ),
        (
            "fn g() -> Int {\n  let v = {\n    let b: Int = 2\n  }\n  v\n}\n",
            "let b: Int = 2",
        ),
    ] {
        assert_eq!(
            type_diagnostics(source),
            [("L2023".to_owned(), label.to_owned())],
            "{source}"
        );
    }
    // A `let` followed by the block's value is the ordinary form.
    assert_eq!(
        run_both(
            "let-then-value",
            "pub fn main() -> Int {\n  let (a, b) = (1, 2)\n  let c = a + b\n  c\n}\n",
        ),
        "\"outcome\":\"success\",\"value\":3"
    );
}

// RU-06
#[test]
fn only_structs_and_records_can_be_updated() {
    assert_eq!(
        check_codes(concat!(
            "pub fn main() -> Option<Int> {\n",
            "  let value = Some(1)\n",
            "  { ..value, value: 2 }\n",
            "}\n",
        )),
        ["T4284"]
    );
}

// A function-typed field mismatch names both function types in full.
#[test]
fn an_update_type_error_spells_out_function_types() {
    let messages = check_messages(concat!(
        "struct Hooks {\n",
        "  run: fn() -> Int\n",
        "}\n",
        "pub fn main() -> Int {\n",
        "  let hooks = Hooks(fn() { 1 })\n",
        "  let changed = { ..hooks, run: fn() { \"x\" } }\n",
        "  changed.run()\n",
        "}\n",
    ));
    assert_eq!(
        messages,
        [
            "T4203 field `run` has type `fn() -> Int`, not `fn() -> Text`; an update keeps the base's type"
        ]
    );
}

// A base whose type is not solved yet (an unannotated clause parameter) gets
// advice to annotate it, not a description of the missing type.
#[test]
fn an_update_of_an_unsolved_base_asks_for_an_annotation() {
    let messages = check_messages(&format!(
        "{CONFIG}fn bump(0, config) -> Config {{\n  config\n}}\nfn bump(n, config) {{\n  {{ ..config, jobs: n }}\n}}\npub fn main() -> Int {{ bump(4, Config(\"a\", 1, False)).jobs }}\n"
    ));
    assert_eq!(
        messages,
        [
            "T4284 only a struct or record value can be updated, and this value's type is not known yet; annotate the parameter or `let` binding it comes from with its struct type"
        ]
    );
}

// RU-07
#[test]
fn an_update_of_another_modules_struct_is_private() {
    let config = ProjectSource::new(
        "config",
        SourceSnapshot::from_text(
            "config.chimi",
            concat!(
                "pub struct Config {\n",
                "  jobs: Int\n",
                "}\n",
                "pub fn default() -> Config { Config(1) }\n",
            ),
        ),
    )
    .expect("valid module path");
    let main = ProjectSource::new(
        "main",
        SourceSnapshot::from_text(
            "main.chimi",
            concat!(
                "import config as Config\n",
                "pub fn main() -> Int {\n",
                "  let changed = { ..Config.default(), jobs: 4 }\n",
                "  0\n",
                "}\n",
            ),
        ),
    )
    .expect("valid module path");
    let codes = Compiler::type_project(vec![config, main])
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(codes, ["E_PRIVATE_STRUCT_CONSTRUCTOR"]);
}

// RU-08 and a misplaced spread: both are syntax errors that teach the braced form.
#[test]
fn a_positional_or_misplaced_update_is_a_syntax_error() {
    let messages = coded_messages(&format!(
        "{CONFIG}fn change(base: Config, jobs: Int) -> Config {{\n  Config(..base, jobs)\n}}\npub fn main() -> Int {{ 0 }}\n"
    ));
    assert!(
        messages
            .iter()
            .any(|(code, message)| code == "S1001" && message.contains("`{ ..base, jobs: jobs }`")),
        "{messages:?}"
    );
    // Only a lowercase identifier can name a field; anything else gets a placeholder.
    let messages = coded_messages(&format!(
        "{CONFIG}fn change(base: Config) -> Config {{\n  Config(..base, 4, True)\n}}\npub fn main() -> Int {{ 0 }}\n"
    ));
    assert!(
        messages.iter().any(|(code, message)| code == "S1001"
            && message.contains("`{ ..base, field: 4, field: True }`")),
        "{messages:?}"
    );
    let messages = coded_messages(&format!(
        "{CONFIG}fn change(base: Config) -> Config {{\n  {{ jobs: 1, ..base }}\n}}\npub fn main() -> Int {{ 0 }}\n"
    ));
    assert!(
        messages
            .iter()
            .any(|(code, message)| code == "S1001" && message.contains("must come first")),
        "{messages:?}"
    );
    // Each error carries the fix that applies its advice, and adds no type error.
    for (wrong, fixed) in [
        ("Config(..base, jobs)", "{ ..base, jobs: jobs }"),
        ("{ jobs: 1, ..base }", "{ ..base, jobs: 1 }"),
    ] {
        let source = format!(
            "{CONFIG}fn change(base: Config, jobs: Int) -> Config {{\n  {wrong}\n}}\npub fn main() -> Int {{ 0 }}\n"
        );
        let module = ProjectSource::new(
            "main",
            SourceSnapshot::from_text("main.chimi", source.as_str()),
        )
        .expect("valid module path");
        let report = Compiler::type_project(vec![module]);
        let diagnostics = report.diagnostics();
        let codes = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code())
            .collect::<Vec<_>>();
        assert_eq!(codes, ["S1001"], "{wrong}");
        let [fix] = diagnostics[0].fixes() else {
            panic!("one fix for {wrong}: {:?}", diagnostics[0].fixes());
        };
        let range = fix.range();
        let mut applied = source.clone();
        applied.replace_range(
            range.start() as usize..range.end() as usize,
            fix.replacement(),
        );
        assert_eq!(applied, source.replace(wrong, fixed));
        assert!(check_codes(&applied).is_empty(), "{applied}");
    }
}

// RU-09: the base evaluates first, then the values left to right; a failure stops
// the update.
#[test]
fn an_update_evaluates_the_base_then_each_value_in_order() {
    let program = |base: &str, first: &str, second: &str| {
        format!(
            concat!(
                "{}",
                "enum Step {{\n",
                "  Base\n",
                "  First\n",
                "  Second\n",
                "}}\n",
                "fn base(fails: Bool) -> Config {{\n",
                "  case fails {{\n",
                "    True -> Failure(Step.Base)\n",
                "    False -> Config(\"a\", 1, False)\n",
                "  }}\n",
                "}}\n",
                "fn jobs(fails: Bool) -> Int {{\n",
                "  case fails {{\n",
                "    True -> Failure(Step.First)\n",
                "    False -> 2\n",
                "  }}\n",
                "}}\n",
                "fn verbose(fails: Bool) -> Bool {{\n",
                "  case fails {{\n",
                "    True -> Failure(Step.Second)\n",
                "    False -> True\n",
                "  }}\n",
                "}}\n",
                "pub fn main() -> Int {{\n",
                "  let updated = {{ ..base({}), verbose: verbose({}), jobs: jobs({}) }}\n",
                "  updated.jobs\n",
                "}}\n",
            ),
            CONFIG, base, second, first
        )
    };
    let failure = |variant: &str| {
        format!(
            "\"outcome\":\"expected_failure\",\"error\":{{\"type\":\"Step\",\"variant\":\"{variant}\",\"fields\":{{}}}}"
        )
    };
    assert_eq!(
        run_both("update-order-base", &program("True", "True", "True")),
        failure("Base")
    );
    assert_eq!(
        run_both("update-order-values", &program("False", "True", "True")),
        failure("Second")
    );
    assert_eq!(
        run_both("update-order-success", &program("False", "False", "False")),
        "\"outcome\":\"success\",\"value\":2"
    );
}

// RU-10, the spec example: a method whose interface declares `errors` takes a
// value that fails with fewer variants (or none), as a `using` provider does,
// and an ignored `_` parameter lowers on both paths.
#[test]
fn an_update_of_a_fallible_provider_method_accepts_a_narrower_row() {
    let source = concat!(
        "enum PaymentError {\n",
        "  Offline\n",
        "  Declined\n",
        "}\n",
        "interface Payment {\n",
        "  charge: fn(Int) -> Bool errors PaymentError\n",
        "}\n",
        "fn attempt(payment: Payment) -> Text {\n",
        "  case payment.charge(3) {\n",
        "    Success(True) -> \"paid\"\n",
        "    Success(False) -> \"refused\"\n",
        "    Failure(Offline) -> \"offline\"\n",
        "    Failure(Declined) -> \"declined\"\n",
        "  }\n",
        "}\n",
        "fn declined() -> List<Text> needs { payment: Payment } {\n",
        "  let failing = { ..payment, charge: fn(_) { Failure(PaymentError.Offline) } }\n",
        "  let refusing = { ..payment, charge: fn(_amount: Int) { False } }\n",
        "  [attempt(payment), attempt(failing), attempt(refusing)]\n",
        "}\n",
        "pub fn main() -> List<Text> {\n",
        "  declined() using { payment: { charge: fn(_amount: Int) { True } } }\n",
        "}\n",
    );
    assert_eq!(
        run_both("update-fallible-provider", source),
        "\"outcome\":\"success\",\"value\":[\"paid\",\"offline\",\"refused\"]"
    );
}

// Struct construction and update use the same rule for a function field.
#[test]
fn a_function_field_accepts_a_value_with_fewer_failures() {
    let source = concat!(
        "enum E {\n",
        "  A\n",
        "}\n",
        "struct Hooks {\n",
        "  run: fn() -> Int errors E\n",
        "  n: Int\n",
        "}\n",
        "fn total(hooks: Hooks) -> Int {\n",
        "  case hooks.run() {\n",
        "    Success(v) -> v + hooks.n\n",
        "    Failure(_) -> 0\n",
        "  }\n",
        "}\n",
        "pub fn main() -> List<Int> {\n",
        "  let quiet = Hooks(fn() { 1 }, 2)\n",
        "  let failing = Hooks(fn() { Failure(E.A) }, 2)\n",
        "  [total(quiet), total(failing), total({ ..quiet, run: fn() { 5 } })]\n",
        "}\n",
    );
    assert_eq!(
        run_both("function-field-narrower-row", source),
        "\"outcome\":\"success\",\"value\":[3,0,7]"
    );
    let widened = source.replace("fn() { 5 }", "fn() { Failure(Unexpected.Value) }");
    let widened = format!("enum Unexpected {{\n  Value\n}}\n{widened}");
    assert_eq!(check_codes(&widened), ["T4203"]);
}

// RU-10: updating one method of a provider value keeps the interface type.
#[test]
fn an_update_of_a_provider_keeps_the_interface_type() {
    let source = concat!(
        "interface Payment {\n",
        "  charge: fn(Int) -> Bool\n",
        "}\n",
        "fn charge_with(payment: Payment, amount: Int) -> Bool {\n",
        "  payment.charge(amount)\n",
        "}\n",
        "fn declined() -> Bool needs { payment: Payment } {\n",
        "  let failing = { ..payment, charge: fn(_amount: Int) { False } }\n",
        "  charge_with(failing, 3)\n",
        "}\n",
        "const approving = { charge: fn(_amount: Int) { True } }\n",
        "pub fn main() -> List<Bool> {\n",
        "  [declined() using { payment: approving }, approving.charge(1)]\n",
        "}\n",
    );
    assert_eq!(
        run_both("update-provider", source),
        "\"outcome\":\"success\",\"value\":[false,true]"
    );
}

// V1, V2, V3, and the `Void` pattern: `Void` is a value in expression position, a
// bare `break` leaves with it, and a `Void` pattern matches it.
#[test]
fn void_is_a_value_and_a_pattern() {
    let source = concat!(
        "interface Console {\n",
        "  write_line: fn(Text) -> Void\n",
        "}\n",
        "fn greet() -> Void needs { console: Console } {\n",
        "  console.write_line(\"hi\")\n",
        "}\n",
        "fn counted(value: Void) -> Int {\n",
        "  case value {\n",
        "    Void -> 1\n",
        "  }\n",
        "}\n",
        "pub fn main() -> List<Bool> {\n",
        "  let quiet = {\n",
        "    write_line: fn(_line) { Void },\n",
        "  }\n",
        "  let greeted = greet() using { console: quiet }\n",
        "  let stopped = loop count = 0 {\n",
        "    case count {\n",
        "      3 -> break\n",
        "      n -> n + 1\n",
        "    }\n",
        "  }\n",
        "  let idle = loop _ = Void {\n",
        "    break Void\n",
        "  }\n",
        "  [greeted == Void, stopped == Void, idle == Void, counted(Void) == 1]\n",
        "}\n",
    );
    assert_eq!(
        run_both("void-value", source),
        "\"outcome\":\"success\",\"value\":[true,true,true,true]"
    );
}

// `Void` is ordered (it equals itself), `_` matches it, and V2: a `loop` over
// `Void` state whose body yields `Void` to continue type-checks and runs.
#[test]
fn void_is_ordered_matches_a_wildcard_and_can_be_loop_state() {
    let source = concat!(
        "fn ignore(value: Void) -> Int {\n",
        "  case value {\n",
        "    _ -> 7\n",
        "  }\n",
        "}\n",
        "fn settle(limit: Int) -> Int {\n",
        "  loop _ = Void {\n",
        "    case limit > 0 {\n",
        "      True -> break limit\n",
        "      False -> Void\n",
        "    }\n",
        "  }\n",
        "}\n",
        "pub fn main() -> List<Bool> {\n",
        "  [Void <= Void, Void >= Void, Void < Void, ignore(Void) == 7, settle(3) == 3]\n",
        "}\n",
    );
    assert_eq!(
        run_both("void-order-and-state", source),
        "\"outcome\":\"success\",\"value\":[true,true,false,true,true]"
    );
}

// V3: `Void` is the outcome value itself, rendered as the `Void` record.
#[test]
fn void_is_a_success_value() {
    assert_eq!(
        run_both("void-outcome", "pub fn main() -> Void {\n  Void\n}\n"),
        "\"outcome\":\"success\",\"value\":{\"type\":\"Void\"}"
    );
}

// V4: an empty block still does not stand in for `Void`, and a `Void` pattern
// rejects a subject of another type.
#[test]
fn void_does_not_replace_an_empty_block_or_match_other_types() {
    assert_eq!(check_codes("pub fn main() -> Void {}\n"), ["S1001"]);
    assert_eq!(
        check_codes("pub fn main() -> Int {\n  case 1 {\n    Void -> 0\n  }\n}\n"),
        ["T4230"]
    );
}

// T1, T2, TP-01, and LP-08's `sum`: a tuple is built and read back with `let`, a
// `loop` state, a `case` subject, and a tuple parameter, on both paths.
#[test]
fn tuples_destructure_in_let_loop_case_and_parameters() {
    let source = concat!(
        "fn sum(numbers: List<Int>) -> Int {\n",
        "  loop (remaining, total) = (numbers, 0) {\n",
        "    case remaining {\n",
        "      [] -> break total\n",
        "      [first, ..rest] -> (rest, total + first)\n",
        "    }\n",
        "  }\n",
        "}\n",
        "fn min_and_max(values: List<Int>) -> (Int, Int) {\n",
        "  loop (remaining, low, high) = (values, 100, 0) {\n",
        "    case remaining {\n",
        "      [] -> break (low, high)\n",
        "      [next, ..rest] -> (rest, Order.min(next, low), Order.max(next, high))\n",
        "    }\n",
        "  }\n",
        "}\n",
        "fn describe(pair: (Int, Text)) -> Text {\n",
        "  case pair {\n",
        "    (0, name) -> \"zero ${name}\"\n",
        "    (_, name) -> name\n",
        "  }\n",
        "}\n",
        "fn swap((left, right)) {\n",
        "  (right, left)\n",
        "}\n",
        "pub fn main() -> List<(Text, Int)> {\n",
        "  let (low, high) = min_and_max([3, 1, 4])\n",
        "  let (label, count) = swap((sum([1, 2, 3, 4]), describe((0, \"x\"))))\n",
        "  [(describe((low, \"low\")), low), (describe((high - high, \"y\")), high), (label, count)]\n",
        "}\n",
    );
    let source = format!("import std/order as Order\n{source}");
    assert_eq!(
        run_both("tuple-destructuring", &source),
        concat!(
            "\"outcome\":\"success\",\"value\":[",
            "{\"tuple\":[\"low\",1]},{\"tuple\":[\"zero y\",4]},{\"tuple\":[\"zero x\",10]}]",
        )
    );
}

// E6, O2, and T9: tuples are equatable and ordered element by element, so a
// separately built equal tuple finds a `Map` entry, and `Map.from` keeps the last
// value for a repeated key.
#[test]
fn tuples_compare_structurally_and_key_maps() {
    let source = concat!(
        "import std/map as Map\n",
        "import std/map.{Map}\n",
        "fn key(id: Int, name: Text) -> (Int, Text) {\n",
        "  (id, name)\n",
        "}\n",
        "pub fn main() -> List<Bool> {\n",
        "  let keyed: Map<(Int, Text), Int> = Map.put(Map.empty(), (7, \"seven\"), 70)\n",
        "  let counts = Map.from([(\"ok\", 3), (\"failed\", 1), (\"ok\", 4)])\n",
        "  [\n",
        "    Map.get(keyed, key(7, \"seven\")) == Some(70),\n",
        "    Map.get(keyed, key(7, \"eight\")) == None,\n",
        "    (2, \"b\") < (2, \"c\"),\n",
        "    (1, \"z\") < (2, \"a\"),\n",
        "    (2, \"c\") > (2, \"b\"),\n",
        "    (2, \"b\") != (2, \"c\"),\n",
        "    Map.get(counts, \"ok\") == Some(4),\n",
        "    Map.get(counts, \"failed\") == Some(1),\n",
        "  ]\n",
        "}\n",
    );
    assert_eq!(
        run_both("tuple-structural", source),
        "\"outcome\":\"success\",\"value\":[true,true,true,true,true,true,true,true]"
    );
}

// Tuples render as `(a, b)` in the debug format, also from a module constant.
#[test]
fn tuples_render_in_parentheses() {
    let source = concat!(
        "import std/debug as Debug\n",
        "const pair = (1, \"one\")\n",
        "pub fn main() -> List<Text> {\n",
        "  [Debug.render((1, (\"a\", [True]))), Debug.render(pair)]\n",
        "}\n",
    );
    assert_eq!(
        run_both("tuple-render", source),
        "\"outcome\":\"success\",\"value\":[\"(1, (\\\"a\\\", [True]))\",\"(1, \\\"one\\\")\"]"
    );
}

// T3 to T7, TP-02, TP-03, and the irrefutable positions: each written program gets
// exactly the listed diagnostics.
#[test]
fn tuple_and_binding_patterns_report_their_diagnostics() {
    let cases: [(&str, &[&str]); 14] = [
        // T3
        (
            "fn f(pair: (Int, Text)) -> Text {\n  case pair {\n    (0, name) -> name\n  }\n}\n",
            &["T4231"],
        ),
        // T4: the arity mismatch is the only error; it adds no coverage follow-on.
        (
            "fn f() -> Int {\n  let (a, b, c) = (1, 2)\n  a\n}\n",
            &["T4230"],
        ),
        // T5
        (
            "fn f(pair: (Int, Int)) -> Int {\n  let (0, x) = pair\n  x\n}\n",
            &["T4231"],
        ),
        // T6
        ("fn f() -> Int {\n  let x = (1,)\n  1\n}\n", &["S1005"]),
        ("fn f() -> Int {\n  let x = ()\n  1\n}\n", &["S1005"]),
        ("fn f(x: (Int)) -> Int {\n  1\n}\n", &["S1005"]),
        // A rejected `()` or `(p,)` pattern adds no tuple-shape follow-on (T4230).
        (
            "fn f(x: Void) -> Int {\n  case x {\n    () -> 1\n  }\n}\n",
            &["S1005"],
        ),
        (
            "fn f(x: Int) -> Int {\n  case x {\n    (y,) -> y\n  }\n}\n",
            &["S1005"],
        ),
        // T7: `(e)` is a group, not a tuple.
        ("fn f() -> Int {\n  (1)\n}\n", &[]),
        // TP-02
        (
            "fn lookup(key: Text) -> Option<Int> {\n  None\n}\nfn f() -> Int {\n  let Some(value) = lookup(\"a\")\n  value\n}\n",
            &["T4231"],
        ),
        // TP-03
        (
            "fn f(pair: (Int, Int)) -> Int {\n  case pair {\n    (a, _, _) -> a\n  }\n}\n",
            &["T4230"],
        ),
        // A rest-only list pattern is irrefutable; one fixed element is not.
        (
            "fn f(values: List<Int>) -> List<Int> {\n  let [..rest] = values\n  rest\n}\n",
            &[],
        ),
        (
            "fn f() -> Int {\n  loop (0, x) = (0, 1) {\n    break x\n  }\n}\n",
            &["T4231"],
        ),
        (
            // T4282 alone: an outcome pattern is matched as `_`, as for a clause
            // parameter, so it raises no pattern-type follow-on.
            "fn f(pair: (Int, Int)) -> Int {\n  let Failure(e) = pair\n  1\n}\n",
            &["T4282"],
        ),
    ];
    for (source, expected) in cases {
        assert_eq!(check_codes(source), expected, "{source}");
    }
}

// T6: `()` is rejected with a message that points to `Void`, in an expression
// and in a pattern.
#[test]
fn unit_parentheses_suggest_void() {
    for source in [
        "fn f() -> Int {\n  let x = ()\n  1\n}\n",
        "fn f(x: Void) -> Int {\n  case x {\n    () -> 1\n  }\n}\n",
    ] {
        let diagnostics = coded_messages(source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let (code, message) = &diagnostics[0];
        assert_eq!(code, "S1005");
        assert!(message.contains("`Void`"), "{message}");
    }
}

// The `T4231` witness names what a refutable `let` misses.
#[test]
fn a_refutable_let_names_its_witness() {
    let diagnostics =
        coded_messages("fn f(values: List<Int>) -> Int {\n  let [only] = values\n  only\n}\n");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    let (code, message) = &diagnostics[0];
    assert_eq!(code, "T4231");
    assert!(message.contains("`[]` is not matched"), "{message}");
    assert!(message.contains("`case`"), "{message}");
    // T5: an integer the pattern misses, not `(_, _)`, which the pattern partly matches.
    let diagnostics =
        coded_messages("fn f(pair: (Int, Int)) -> Int {\n  let (0, x) = pair\n  x\n}\n");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert!(
        diagnostics[0].1.contains("`(1, _)` is not matched"),
        "{diagnostics:?}"
    );
}

// ts-494159: `let name: Type = value` states the binding's type, which drives
// inference of the value and rejects a value of another type.
#[test]
fn an_annotated_let_states_the_binding_type() {
    assert_eq!(
        run_both(
            "typed-let",
            concat!(
                "pub fn main() -> List<Option<Int>> {\n",
                "  let nothing: Option<Int> = None\n",
                "  let count: Int = 2\n",
                "  [nothing, Some(count)]\n",
                "}\n",
            ),
        ),
        "\"outcome\":\"success\",\"value\":[{\"type\":\"Option\",\"variant\":\"None\",\"fields\":{}},{\"type\":\"Option\",\"variant\":\"Some\",\"fields\":{\"value\":2}}]"
    );
    assert_eq!(
        check_codes("fn f() -> Int {\n  let count: Text = 1\n  2\n}\n"),
        ["T4203"]
    );
}

// T8: a tuple of shareable parts is itself shareable, so it can cross a channel.
#[test]
fn a_tuple_of_shareable_parts_is_shareable() {
    assert!(
        check_codes(concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Sender}\n",
            "struct Job {\n  name: Text\n}\n",
            "fn forward(outbox: Sender<(Int, Job)>, item: (Int, Job)) -> Int {\n",
            "  Channel.send(outbox, item)\n",
            "  1\n",
            "}\n",
        ))
        .is_empty()
    );
}

// T8, negative: a tuple is shareable only when every part is, so a tuple
// holding a parent-local `Task` cannot cross a channel.
#[test]
fn a_tuple_holding_a_task_is_not_shareable() {
    assert_eq!(
        check_codes(concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Sender}\n",
            "import std/task.{Task}\n",
            "enum WorkError {\n  Bad\n}\n",
            "fn forward(sender: Sender<(Int, Task<Int, WorkError>)>, child: Task<Int, WorkError>) -> Void {\n",
            "  Channel.send(sender, (1, child))\n",
            "}\n",
        )),
        ["T4243"]
    );
}

// LP-09: tuple patterns nest inside list and enum patterns, and the nested
// match binds `id`.
#[test]
fn tuple_patterns_nest_inside_list_and_variant_patterns() {
    let source = concat!(
        "import std/json as Json\n",
        "import std/json.{Value}\n",
        "fn id(fields: Option<List<(Text, Value)>>) -> Int {\n",
        "  case fields {\n",
        "    Some([(\"id\", Value.Number(id)), ..]) -> Json.number_to_int(id)\n",
        "    _ -> -1\n",
        "  }\n",
        "}\n",
        "pub fn main() -> List<Int> {\n",
        "  [\n",
        "    id(Some([(\"id\", Json.int(41)), (\"x\", Json.int(2))])),\n",
        "    id(Some([(\"name\", Json.int(1))])),\n",
        "    id(Some([])),\n",
        "    id(None),\n",
        "  ]\n",
        "}\n",
    );
    assert_eq!(
        run_both("nested-tuple-pattern", source),
        "\"outcome\":\"success\",\"value\":[41,-1,-1,-1]"
    );
}

const REJECT: &str = concat!(
    "struct Response {\n",
    "  code: Int\n",
    "}\n",
    "enum RequestError {\n",
    "  Invalid(reason: Text)\n",
    "}\n",
    "fn reject(reason: Text) -> Never {\n",
    "  Failure(RequestError.Invalid(reason))\n",
    "}\n",
);

// R1: a `Never` arm adds nothing to the join, in either arm order, so the `case`
// has the other arm's type and the failure still propagates at run time.
#[test]
fn a_never_arm_joins_with_any_type() {
    let source = format!(
        concat!(
            "{}",
            "fn route(method: Text) -> Response {{\n",
            "  case method {{\n",
            "    \"GET\" -> Response(200)\n",
            "    other -> reject(other)\n",
            "  }}\n",
            "}}\n",
            "fn route_first(method: Text) {{\n",
            "  case method {{\n",
            "    \"PUT\" -> reject(method)\n",
            "    _ -> Response(201)\n",
            "  }}\n",
            "}}\n",
            "pub fn main() -> List<Int> {{\n",
            "  [route(\"GET\").code, route_first(\"GET\").code, route(\"POST\").code]\n",
            "}}\n",
        ),
        REJECT
    );
    assert_eq!(
        run_both("never-join", &source),
        concat!(
            "\"outcome\":\"expected_failure\",",
            "\"error\":{\"type\":\"RequestError\",\"variant\":\"Invalid\",\"fields\":{\"reason\":\"POST\"}}",
        )
    );
    let route_first = inspect_at(&source, "route_first(\"GET\")", 0);
    assert_eq!(
        route_first
            .callable()
            .map(chimi::CallableInspection::display),
        Some("fn(Text) -> Response errors RequestError")
    );
}

// R2: a `-> Never` body that can produce a value is `T4308`, labelling the value.
#[test]
fn a_never_function_must_not_produce_a_value() {
    let diagnostics = type_diagnostics("fn bad() -> Never {\n  let x = 1\n  x + 1\n}\n");
    assert_eq!(diagnostics, [("T4308".to_owned(), "x + 1".to_owned())]);
    // The label is the innermost value-producing expression: the arm, or the break.
    for (source, label) in [
        (
            "fn r() -> Never {\n  r()\n}\nfn f(b: Bool) -> Never {\n  case b {\n    True -> r()\n    False -> 1\n  }\n}\n",
            "1",
        ),
        (
            "fn f() -> Never {\n  loop n = 0 {\n    case n {\n      9 -> break\n      _ -> n + 1\n    }\n  }\n}\n",
            "break",
        ),
    ] {
        assert_eq!(
            type_diagnostics(source),
            [("T4308".to_owned(), label.to_owned())],
            "{source}"
        );
    }
    assert!(
        check_codes(&format!(
            "{REJECT}fn fine() -> Never {{\n  reject(\"x\")\n}}\n"
        ))
        .is_empty()
    );
}

// The bottom rule is one-way: a `Never` value fits any expected type, but no
// value of another type fits where `Never` is expected. Each of these once
// compiled and let an `Int` be used as `Text`.
#[test]
fn no_other_value_passes_as_never() {
    let rejected = [
        (
            "fn coerce() -> Never {\n  let n: Never = 41\n  n\n}\n",
            "T4203",
            "41",
        ),
        (
            "fn coerce() -> Never {\n  let f: fn() -> Never = fn() { 41 }\n  f()\n}\n",
            "T4203",
            "fn() { 41 }",
        ),
        (
            "fn items() -> List<Never> {\n  [41, 42]\n}\n",
            "T4203",
            "{\n  [41, 42]\n}",
        ),
        (
            "fn sink(x: Never) -> Int {\n  7\n}\npub fn main() -> Int {\n  sink(3)\n}\n",
            "T4203",
            "3",
        ),
        // A function input is contravariant: a `fn(Never)` would be called with an `Int`.
        (
            concat!(
                "fn sink(x: Never) -> Text {\n  x\n}\n",
                "fn apply(f: fn(Int) -> Text) -> Text {\n  f(3)\n}\n",
                "pub fn main() -> Text {\n  apply(sink)\n}\n",
            ),
            "T4203",
            "sink",
        ),
        ("fn coerce() -> Never {\n  41\n}\n", "T4308", "41"),
    ];
    for (source, code, label) in rejected {
        assert_eq!(
            type_diagnostics(source),
            [(code.to_owned(), label.to_owned())],
            "{source}"
        );
    }
    // The sound direction still holds: a `Never` value or result fits an `Int` slot.
    assert!(
        check_codes(&format!(
            concat!(
                "{}",
                "fn apply(f: fn() -> Int errors RequestError) -> Int {{\n  f()\n}}\n",
                "pub fn main() -> List<Int> {{\n",
                "  let n: Int = reject(\"a\")\n",
                "  [n, apply(fn() {{ reject(\"b\") }})]\n",
                "}}\n",
            ),
            REJECT
        ))
        .is_empty()
    );
}

// R3, an all-`Never` case, and a body that only fails: tooling shows an inferred
// `Never` success type.
#[test]
fn an_inferred_success_type_can_be_never() {
    let source = format!(
        concat!(
            "{}",
            "fn spin() {{\n",
            "  loop n = 0 {{\n",
            "    n + 1\n",
            "  }}\n",
            "}}\n",
            "fn refuse(method: Text) {{\n",
            "  case method {{\n",
            "    \"a\" -> reject(\"a\")\n",
            "    _ -> reject(method)\n",
            "  }}\n",
            "}}\n",
            "fn count() -> Int {{\n",
            "  spin()\n",
            "}}\n",
            "fn later() -> Int {{\n",
            "  refuse(\"b\")\n",
            "}}\n",
            "fn fail_now() {{\n",
            "  Failure(RequestError.Invalid(\"now\"))\n",
            "}}\n",
            "fn sooner() -> Int {{\n",
            "  fail_now()\n",
            "}}\n",
        ),
        REJECT
    );
    assert!(check_codes(&source).is_empty());
    for (needle, contract) in [
        ("spin()", "fn() -> Never"),
        ("refuse(\"b\")", "fn(Text) -> Never errors RequestError"),
        ("fail_now()", "fn() -> Never errors RequestError"),
    ] {
        // The use, not the declaration name, carries the inferred contract.
        let declared = needle == "spin()" || needle == "fail_now()";
        let report = inspect_at(&source, needle, usize::from(declared));
        assert_eq!(
            report.callable().map(chimi::CallableInspection::display),
            Some(contract)
        );
    }
}

// R4: the type `Never` and a value constructor named `Never` do not conflict.
#[test]
fn the_never_type_and_a_never_constructor_coexist() {
    assert!(
        check_codes(concat!(
            "enum Restart {\n",
            "  Never\n",
            "  Always\n",
            "}\n",
            "fn pick() -> Restart {\n",
            "  Restart.Never\n",
            "}\n",
            "fn stop() -> Never {\n",
            "  loop n = 0 {\n",
            "    n\n",
            "  }\n",
            "}\n",
        ))
        .is_empty()
    );
}

// In a failure row, `Never` is the empty failure set.
#[test]
fn errors_never_is_the_empty_failure_row() {
    assert!(
        check_codes(concat!(
            "fn apply(operation: fn() -> Int errors Never) -> Int {\n",
            "  operation()\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  apply(fn() { 1 })\n",
            "}\n",
        ))
        .is_empty()
    );
}

// N11, G1, G2, G5, and `std/range`: `..` builds a half-open range, `..=` normalizes
// to one, a range binds below additive operators, and equality and order use the
// normalized bounds.
#[test]
fn ranges_normalize_and_answer_std_range_queries() {
    let source = concat!(
        "import std/range as Range\n",
        "const dice = 1..=6\n",
        "fn width(span: Range) -> Int {\n",
        "  Range.end(span) - Range.start(span)\n",
        "}\n",
        "pub fn main() -> List<Bool> {\n",
        "  let n = 2\n",
        "  let r = 0..10\n",
        "  [\n",
        "    0..3 == 0..=2,\n",
        "    5..2 != 4..2,\n",
        "    0..3 < 0..4,\n",
        // Order compares the start bound first, even between two empty ranges.
        "    4..2 < 5..2,\n",
        "    Range.length(5..2) == 0,\n",
        "    Range.length(dice) == 6,\n",
        "    width(0..n + 1) == 3,\n",
        "    Range.contains(r, 9),\n",
        "    Range.contains(r, 10) == False,\n",
        "    Range.contains(r, -1) == False,\n",
        "    Range.contains(5..2, 3) == False,\n",
        "  ]\n",
        "}\n",
    );
    assert_eq!(
        run_both("range-queries", source),
        "\"outcome\":\"success\",\"value\":[true,true,true,true,true,true,true,true,true,true,true]"
    );
}

// A range renders as its normalized `start..end`, and an outcome record encodes
// its bounds.
#[test]
fn ranges_render_with_normalized_bounds() {
    let source = concat!(
        "import std/debug as Debug\n",
        "pub fn main() -> List<Text> {\n",
        "  [Debug.render((0..=2, 4..1))]\n",
        "}\n",
    );
    assert_eq!(
        run_both("range-render", source),
        "\"outcome\":\"success\",\"value\":[\"(0..3, 4..1)\"]"
    );
    assert_eq!(
        run_both("range-outcome", "pub fn main() -> Range {\n  -2..=5\n}\n"),
        "\"outcome\":\"success\",\"value\":{\"range\":{\"start\":-2,\"end\":6}}"
    );
}

// G3, and `Range.length` past `Int`: both defect on both paths.
#[test]
fn ranges_defect_when_a_bound_or_length_overflows() {
    let program = |body: &str| {
        format!(
            concat!(
                "import std/range as Range\n",
                "fn largest() -> Int {{\n",
                "  9223372036854775807\n",
                "}}\n",
                "pub fn main() -> Int {{\n",
                "  {}\n",
                "}}\n",
            ),
            body
        )
    };
    let inclusive = run_both(
        "range-inclusive-overflow",
        &program("Range.start(0..=largest())"),
    );
    assert!(
        defect_message(&inclusive).contains("`..=` ends at the largest Int"),
        "{inclusive}"
    );
    let length = run_both(
        "range-length-overflow",
        &program("Range.length(-1..largest())"),
    );
    assert!(
        defect_message(&length).contains("Range.length overflows Int"),
        "{length}"
    );
    assert_eq!(
        run_both(
            "range-length-largest",
            &program("Range.length(0..largest())")
        ),
        "\"outcome\":\"success\",\"value\":9223372036854775807"
    );
}

// G4 and G6.
#[test]
fn range_bounds_must_be_int_and_ranges_do_not_chain() {
    assert_eq!(check_codes("fn f() -> Range {\n  0..1.5\n}\n"), ["T4311"]);
    assert_eq!(
        check_codes("fn f() -> Range {\n  \"a\"..=2\n}\n"),
        ["T4311"]
    );
    assert_eq!(
        check_codes("fn f(a: Int, b: Int, c: Int) -> Range {\n  a..b..c\n}\n"),
        ["S1006"]
    );
}

// The canonical form writes a range without spaces, and keeps list rest patterns.
#[test]
fn ranges_format_without_spaces() {
    let source = concat!(
        "fn f(values: List<Int>, n: Int) -> Range {\n",
        "  case values {\n",
        "    [_, .. rest] -> 0 .. n + 1\n",
        "    _ -> (n) ..= 4\n",
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
            "fn f(values: List<Int>, n: Int) -> Range {\n",
            "  case values {\n",
            "    [_, ..rest] -> 0..n + 1\n",
            "    _ -> (n)..=4\n",
            "  }\n",
            "}\n",
        )
    );
}

// A `Failure(Enum.Variant(payload))` arm binds the payload at the subject's
// instantiated type argument, for a user generic enum as for std ones.
#[test]
fn failure_variant_pattern_binds_a_generic_payload_at_its_instance_type() {
    let source = concat!(
        "enum Bounce<T> {\n",
        "  Back(value: T)\n",
        "  Lost\n",
        "}\n",
        "struct Order {\n",
        "  id: Int,\n",
        "}\n",
        "fn send(order: Order) -> Int {\n",
        "  case order.id {\n",
        "    0 -> Failure(Bounce.Lost)\n",
        "    _ -> Failure(Bounce.Back(order))\n",
        "  }\n",
        "}\n",
        "pub fn main() -> Int {\n",
        "  case send(Order(7)) {\n",
        "    Success(n) -> n\n",
        "    Failure(Bounce.Back(back)) -> back.id\n",
        "    Failure(Bounce.Lost) -> -1\n",
        "  }\n",
        "}\n",
    );
    assert_eq!(
        run_both("failure-generic-payload", source),
        "\"outcome\":\"success\",\"value\":7"
    );
}

// Binding the whole failure keeps the type argument too: matching it later in
// an ordinary case still sees `Bounce<Order>`.
#[test]
fn bound_generic_failure_keeps_its_instance_type() {
    let source = concat!(
        "enum Bounce<T> {\n",
        "  Back(value: T)\n",
        "  Lost\n",
        "}\n",
        "struct Order {\n",
        "  id: Int,\n",
        "}\n",
        "fn send(order: Order) -> Int {\n",
        "  case order.id {\n",
        "    0 -> Failure(Bounce.Lost)\n",
        "    _ -> Failure(Bounce.Back(order))\n",
        "  }\n",
        "}\n",
        "pub fn main() -> Int {\n",
        "  case send(Order(9)) {\n",
        "    Success(n) -> n\n",
        "    Failure(error) -> case error {\n",
        "      Bounce.Back(back) -> back.id\n",
        "      Bounce.Lost -> -1\n",
        "    }\n",
        "  }\n",
        "}\n",
    );
    assert_eq!(
        run_both("failure-generic-bound", source),
        "\"outcome\":\"success\",\"value\":9"
    );
}

/// Each diagnostic as (code, the source text it points at, message).
fn check_diagnostics(source: &str) -> Vec<(String, String, String)> {
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
                diagnostic.message().to_owned(),
            )
        })
        .collect()
}

// T4203 names both types and points at the argument, not the whole call.
#[test]
fn a_mistyped_call_argument_names_both_types_at_the_argument() {
    let source = concat!(
        "fn add(left: Int, right: Int) -> Int {\n",
        "  left + right\n",
        "}\n",
        "pub fn main() -> Int {\n",
        "  add(1, \"two\")\n",
        "}\n",
    );
    let diagnostics = check_diagnostics(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    let (code, site, message) = &diagnostics[0];
    assert_eq!((code.as_str(), site.as_str()), ("T4203", "\"two\""));
    assert!(
        message.contains("`Text`") && message.contains("expected `Int`"),
        "{message}"
    );
}

// A function argument that fails where the parameter may not: the message says
// the difference is the failure row.
#[test]
fn a_fallible_function_argument_is_reported_as_a_failure_row_difference() {
    let source = concat!(
        "enum Problem {\n",
        "  TooBig\n",
        "}\n",
        "fn classify(n: Int) -> Text {\n",
        "  case n > 100 {\n",
        "    True -> Failure(Problem.TooBig)\n",
        "    False -> \"small\"\n",
        "  }\n",
        "}\n",
        "fn label(n: Int, f: fn(Int) -> Text) -> Text {\n",
        "  f(n)\n",
        "}\n",
        "pub fn main() -> Text {\n",
        "  label(3, classify)\n",
        "}\n",
    );
    let diagnostics = check_diagnostics(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    let (code, site, message) = &diagnostics[0];
    assert_eq!((code.as_str(), site.as_str()), ("T4203", "classify"));
    assert!(
        message.contains("`fn(Int) -> Text errors Problem`")
            && message.contains("expected `fn(Int) -> Text`")
            && message.contains("failure row"),
        "{message}"
    );
}

// The forgotten-update error names the state type and the found type, at the
// expression that produced the wrong next state.
#[test]
fn a_mistyped_loop_next_state_is_reported_at_the_producing_expression() {
    let source = concat!(
        "pub fn main() -> Int {\n",
        "  loop i = 0 {\n",
        "    case i == 3 {\n",
        "      True -> break i\n",
        "      False -> {\n",
        "        let next = i + 1\n",
        "        \"next\"\n",
        "      }\n",
        "    }\n",
        "  }\n",
        "}\n",
    );
    let diagnostics = check_diagnostics(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    let (code, site, message) = &diagnostics[0];
    assert_eq!((code.as_str(), site.as_str()), ("T4203", "\"next\""));
    assert!(
        message.contains("`Text`") && message.contains("expected `Int`"),
        "{message}"
    );
}

// T4325 names the type it found.
#[test]
fn a_non_text_interpolation_names_the_found_type() {
    let source = "pub fn main() -> Text {\n  \"${[1, 2]}\"\n}\n";
    let diagnostics = check_diagnostics(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    let (code, _, message) = &diagnostics[0];
    assert_eq!(code, "T4325");
    assert!(message.contains("found `List<Int>`"), "{message}");
}

// A variant written through a module alias (`Order.Less`) is one name error that
// names the owning type; the case arms after it get no coverage follow-ons.
#[test]
fn a_variant_through_a_module_alias_names_its_type_without_coverage_follow_ons() {
    let source = concat!(
        "import std/order as Order\n",
        "import std/order.{Ordering}\n",
        "fn pick(left: Int, right: Int) -> Int {\n",
        "  case Order.compare(left, right) {\n",
        "    Order.Less -> left\n",
        "    _ -> right\n",
        "  }\n",
        "}\n",
    );
    let diagnostics = check_diagnostics(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    let (code, site, message) = &diagnostics[0];
    assert_eq!(
        (code.as_str(), site.as_str()),
        ("E_UNRESOLVED_CONSTRUCTOR", "Less")
    );
    assert!(message.contains("`Ordering.Less`"), "{message}");
}

// Without the type in scope, the hint offers the qualified path, which resolves.
#[test]
fn a_variant_hint_offers_the_qualified_path_when_the_type_is_not_imported() {
    let wrong = concat!(
        "import std/order as Order\n",
        "fn pick(left: Int, right: Int) -> Int {\n",
        "  case Order.compare(left, right) {\n",
        "    Order.Less -> left\n",
        "    _ -> right\n",
        "  }\n",
        "}\n",
    );
    let diagnostics = check_diagnostics(wrong);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert!(
        diagnostics[0].2.contains("`Order.Ordering.Less`"),
        "{diagnostics:?}"
    );
    let fixed = wrong.replace("Order.Less", "Order.Ordering.Less");
    assert_eq!(check_codes(&fixed), Vec::<String>::new());
}

// T4312 covers the normalizing `+ 1` of a constant `start..=end`: ending at the
// largest Int warns, and the range still compiles and defects at run time.
#[test]
fn a_constant_inclusive_range_ending_at_the_largest_int_warns_and_defects() {
    let source = concat!(
        "import std/range as Range\n",
        "pub fn main() -> Int {\n",
        "  Range.length(0..=9223372036854775807)\n",
        "}\n",
    );
    assert!(check_codes(source).is_empty());
    assert_eq!(warning_codes(source), ["T4312"]);
    let outcome = run_both("inclusive-range-overflow", source);
    assert!(outcome.contains("\"outcome\":\"defect\""), "{outcome}");
    for quiet in [
        "pub fn main() -> Range { 0..=9223372036854775806 }\n",
        "pub fn main() -> Range { 0..9223372036854775807 }\n",
        "fn start() -> Int { 0 }\npub fn main() -> Range { start()..=1 }\n",
    ] {
        assert!(warning_codes(quiet).is_empty(), "{quiet}");
    }
}

// Patterns spec "Inference across clauses": a parameter's type unifies every use,
// so an unannotated parameter used as a callee is inferred as a function.
#[test]
fn an_unannotated_parameter_called_in_the_body_is_inferred_as_a_function() {
    let source = concat!(
        "fn apply(f, x: Int) -> Int {\n",
        "  f(x)\n",
        "}\n",
        "fn apply_loop(f, xs: List<Int>) -> Int {\n",
        "  loop (left, n) = (xs, 0) {\n",
        "    case left {\n",
        "      [] -> break n\n",
        "      [x, ..tail] -> (tail, n + f(x))\n",
        "    }\n",
        "  }\n",
        "}\n",
        "pub fn main() -> Int {\n",
        "  apply(fn(x) { x + 1 }, 1) + apply_loop(fn(x) { x * 10 }, [1, 2])\n",
        "}\n",
    );
    assert_eq!(
        run_both("unannotated-callee", source),
        "\"outcome\":\"success\",\"value\":32"
    );
}

// The inferred function type stays generic where the body leaves it open.
#[test]
fn an_inferred_function_parameter_generalizes_over_its_types() {
    let source = concat!(
        "fn twice(f, x) {\n",
        "  f(f(x))\n",
        "}\n",
        "pub fn main() -> Text {\n",
        "  let tripled = twice(fn(n: Int) -> Int { n * 3 }, 2)\n",
        "  let shouted = twice(fn(t: Text) -> Text { \"${t}!\" }, \"hi\")\n",
        "  \"${tripled} ${shouted}\"\n",
        "}\n",
    );
    assert_eq!(
        run_both("unannotated-callee-generic", source),
        "\"outcome\":\"success\",\"value\":\"18 hi!!\""
    );
}

// Every use of the parameter must agree: two calls with different arities do not.
#[test]
fn an_inferred_function_parameter_must_be_called_consistently() {
    let source = concat!("fn both(f) -> Int {\n", "  f(1) + f(1, 2)\n", "}\n",);
    assert_eq!(check_codes(source), ["T4202"]);
}

// T4222 over a fallible subject names what it may fail with and shows the
// `let`-then-`case` rewrite, which is accepted.
#[test]
fn a_plain_case_over_a_fallible_subject_names_the_failures_and_the_rewrite() {
    let wrong = concat!(
        "import std/int as Int\n",
        "fn double(text: Text) -> Int {\n",
        "  case Int.parse(text) {\n",
        "    0 -> 0\n",
        "    n -> n * 2\n",
        "  }\n",
        "}\n",
    );
    let diagnostics = check_diagnostics(wrong);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    let (code, _, message) = &diagnostics[0];
    assert_eq!(code, "T4222");
    assert!(
        message.contains("`ParseError`")
            && message.contains("let next = ")
            && message.contains("case next"),
        "{message}"
    );
    let fixed = wrong.replace(
        "  case Int.parse(text) {\n",
        "  let next = Int.parse(text)\n  case next {\n",
    );
    assert_eq!(check_codes(&fixed), Vec::<String>::new());
}

// Duration (time-and-outcomes.md, ts-f31a20). Arguments pass through a function
// parameter so that the constant checks (T4348, T4349, T4350) stay out of the way.

const DURATION_PRELUDE: &str = "import std/duration as Duration\n\
                                import std/duration.{Duration}\n\
                                import std/debug as Debug\n";

/// The Duration value `min` cannot be written as one `Int` literal.
const DURATION_MIN: &str =
    "Duration.subtract(Duration.nanoseconds(-9223372036854775807), Duration.nanoseconds(1))";
const DURATION_MAX: &str = "Duration.nanoseconds(9223372036854775807)";

/// `run_both` without the inspection sweep, which is quadratic in the program size:
/// for generated programs whose syntax a smaller `run_both` program already covers.
fn run_both_paths(name: &str, source: &str) -> String {
    let scratch = scratch_directory(name);
    let path = scratch.join("main.chimi");
    std::fs::write(&path, source).unwrap();
    let interpreted = interpreter_outcome(&path);
    let native = native_outcome(&scratch, &path);
    assert_eq!(interpreted, native, "execution paths disagree for {name}");
    assert_formatter_round_trips(source);
    std::fs::remove_dir_all(scratch).unwrap();
    interpreted
        .strip_prefix("{\"schema\":\"chimi.runtime.v1\",")
        .and_then(|rest| rest.strip_suffix('}'))
        .unwrap_or_else(|| panic!("unexpected outcome line: {interpreted}"))
        .to_owned()
}

fn duration_program(body: &str) -> String {
    format!(
        "{DURATION_PRELUDE}\
         fn id(count: Int) -> Int {{ count }}\n\
         pub fn main() -> Text {{\n{body}\n}}\n"
    )
}

// Conformance matrix "Duration range": every constructor accepts its boundary count
// in both directions, and conversions truncate toward zero.
#[test]
fn duration_constructors_accept_their_boundary_counts() {
    let source = duration_program(
        "  let values = [\n\
             Duration.hours(id(2562047)), Duration.hours(id(-2562047)),\n\
             Duration.minutes(id(153722867)), Duration.minutes(id(-153722867)),\n\
             Duration.seconds(id(9223372036)), Duration.seconds(id(-9223372036)),\n\
             Duration.milliseconds(id(9223372036854)), Duration.milliseconds(id(-9223372036854)),\n\
             Duration.microseconds(id(9223372036854775)),\n\
             Duration.microseconds(id(-9223372036854775)),\n\
             Duration.nanoseconds(id(9223372036854775807)),\n\
           ]\n\
           let truncated = [\n\
             Duration.to_microseconds(Duration.nanoseconds(id(-1999))),\n\
             Duration.to_milliseconds(Duration.nanoseconds(id(1999999))),\n\
             Duration.to_seconds(Duration.milliseconds(id(-1999))),\n\
             Duration.to_nanoseconds(Duration.hours(id(1))),\n\
           ]\n\
           \"${Debug.render(values)} ${Debug.render(truncated)}\"",
    );
    let outcome = run_both("duration-boundaries", &source);
    assert_eq!(
        outcome,
        concat!(
            "\"outcome\":\"success\",\"value\":\"[2562047h, -2562047h, 2562047h47m, ",
            "-2562047h47m, 2562047h47m16s, -2562047h47m16s, 2562047h47m16.854s, ",
            "-2562047h47m16.854s, 2562047h47m16.854775s, -2562047h47m16.854775s, ",
            "2562047h47m16.854775807s] [-1, 1, -1, 3600000000000]\""
        )
    );
}

// Conformance matrix "Duration range": one more unit defects, as do arithmetic results
// outside the range, a zero divisor, and `negate` / `absolute` of the minimum. The
// defect is the same on both backends, so neither wraps nor saturates.
#[test]
fn duration_overflow_and_zero_divisors_defect_on_both_backends() {
    let min = DURATION_MIN;
    let max = DURATION_MAX;
    let cases = [
        (
            "Duration.hours(id(2562048))".to_owned(),
            "`Duration.hours(2562048)` overflows the Duration range",
        ),
        (
            "Duration.minutes(id(-153722868))".to_owned(),
            "`Duration.minutes(-153722868)` overflows the Duration range",
        ),
        (
            "Duration.seconds(id(9223372037))".to_owned(),
            "`Duration.seconds(9223372037)` overflows the Duration range",
        ),
        (
            "Duration.milliseconds(id(9223372036855))".to_owned(),
            "`Duration.milliseconds(9223372036855)` overflows the Duration range",
        ),
        (
            "Duration.microseconds(id(9223372036854776))".to_owned(),
            "`Duration.microseconds(9223372036854776)` overflows the Duration range",
        ),
        (
            format!("Duration.negate({min})"),
            "`Duration.negate` overflows the Duration range: -(-2562047h47m16.854775808s)",
        ),
        (
            format!("Duration.absolute({min})"),
            "`Duration.absolute` overflows the Duration range: |-2562047h47m16.854775808s|",
        ),
        (
            format!("Duration.add({max}, Duration.nanoseconds(id(1)))"),
            "`Duration.add` overflows the Duration range: 2562047h47m16.854775807s + 1ns",
        ),
        (
            format!("Duration.subtract({min}, Duration.nanoseconds(id(1)))"),
            "`Duration.subtract` overflows the Duration range: -2562047h47m16.854775808s - 1ns",
        ),
        (
            "Duration.multiply(Duration.hours(id(2000000)), id(2))".to_owned(),
            "`Duration.multiply` overflows the Duration range: 2000000h * 2",
        ),
        (
            format!("Duration.divide({min}, id(-1))"),
            "`Duration.divide` overflows the Duration range: -2562047h47m16.854775808s / -1",
        ),
        (
            "Duration.divide(Duration.seconds(id(3)), id(0))".to_owned(),
            "`Duration.divide` by zero: 3s / 0",
        ),
    ];
    for (index, (expression, message)) in cases.into_iter().enumerate() {
        let source = duration_program(&format!("  Duration.to_text({expression})"));
        let name = format!("duration-defect-{index}");
        let outcome = if index == 0 {
            run_both(&name, &source)
        } else {
            run_both_paths(&name, &source)
        };
        let expected = format!("\"outcome\":\"defect\",\"message\":\"{message}\"");
        assert!(outcome.starts_with(&expected), "{expression}: {outcome}");
    }
}

// Conformance matrix "Duration order": derived `==` and the comparison operators agree
// with `to_nanoseconds` for negative, zero, and positive values, and a Duration works
// as a Map key.
#[test]
fn duration_equality_and_order_follow_the_nanosecond_count() {
    let source = format!(
        "{DURATION_PRELUDE}\
         import std/map as Map\n\
         fn agree(left: Duration, right: Duration) -> Bool {{\n\
           let l = Duration.to_nanoseconds(left)\n\
           let r = Duration.to_nanoseconds(right)\n\
           let equal = (left == right) == (l == r) and (left != right) == (l != r)\n\
           let less = (left < right) == (l < r) and (left <= right) == (l <= r)\n\
           let greater = (left > right) == (l > r) and (left >= right) == (l >= r)\n\
           equal and less and greater\n\
         }}\n\
         fn against(left: Duration, rights: List<Duration>) -> Int {{\n\
           case rights {{\n\
             [] -> 0\n\
             [right, ..rest] -> {{\n\
               let miss = case agree(left, right) {{\n\
                 True -> 0\n\
                 False -> 1\n\
               }}\n\
               miss + against(left, rest)\n\
             }}\n\
           }}\n\
         }}\n\
         fn disagreements(lefts: List<Duration>, all: List<Duration>) -> Int {{\n\
           case lefts {{\n\
             [] -> 0\n\
             [left, ..rest] -> against(left, all) + disagreements(rest, all)\n\
           }}\n\
         }}\n\
         pub fn main() -> Text {{\n\
           let values = [{DURATION_MIN}, Duration.seconds(-1), Duration.nanoseconds(-1),\n\
             Duration.zero, Duration.nanoseconds(1), Duration.milliseconds(1000),\n\
             Duration.seconds(1), {DURATION_MAX}]\n\
           let table = Map.from([(Duration.seconds(1), \"one\"), (Duration.nanoseconds(-1), \"minus\")])\n\
           let found = Map.get(table, Duration.milliseconds(1000))\n\
           \"${{disagreements(values, values)}} ${{Debug.render(found)}}\"\n\
         }}\n"
    );
    let outcome = run_both("duration-order", &source);
    assert_eq!(
        outcome,
        "\"outcome\":\"success\",\"value\":\"0 Some(\\\"one\\\")\""
    );
}

/// Nanosecond counts for the text differential: the extremes, every unit boundary,
/// sub-microsecond and mixed-unit values, and a deterministic spread over the range.
fn generated_durations() -> Vec<String> {
    let mut counts: Vec<i64> = vec![
        0,
        1,
        7,
        999,
        1_000,
        1_001,
        1_500,
        999_999,
        1_000_000,
        1_500_000,
        999_999_999,
        1_000_000_000,
        1_000_000_001,
        59_999_999_999,
        60_000_000_000,
        3_599_500_000_000,
        3_600_000_000_000,
        3_600_000_000_001,
        3_630_000_000_000,
        123_250_000_000,
        86_400_000_000_000,
        i64::MAX,
    ];
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    for index in 0..60 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        // Shift by a varying amount so magnitudes cover every unit.
        counts.push(((state >> 1) >> (index % 63)) as i64);
    }
    let negatives = counts.iter().map(|count| -count).collect::<Vec<_>>();
    counts.extend(negatives);
    let mut values = counts
        .iter()
        .map(|count| format!("Duration.nanoseconds({count})"))
        .collect::<Vec<_>>();
    values.push(DURATION_MIN.to_owned());
    values
}

const DURATION_TEXTS: &[&str] = &[
    "",
    "0",
    "-0",
    "0s",
    "-0s",
    "1.5ns",
    "1.0ns",
    "1s1h",
    "1h1h",
    "1m1ms",
    "1ms1m",
    "1.5h",
    "1h30m",
    "2m3.25s",
    "1.000000001s",
    "0.0000000005s",
    "1.0000000000000s",
    "90s",
    "01s",
    "1.50s",
    "0.00000000001h",
    "0.000000000001h",
    "0.0000000000000000000000000001h",
    "2562047h47m16.854775807s",
    "2562047h47m16.854775808s",
    "-2562047h47m16.854775808s",
    "-2562047h47m16.854775809s",
    "9223372036854775807ns",
    "9223372036854775808ns",
    "99999999999999999999999h",
    "99999999999999999999999h1x",
    "1x",
    "1 s",
    "+1s",
    "1µs",
    "1us",
    ".5s",
    "1.s",
    "-",
    "1h-1m",
    "s",
    "1",
];

// Conformance matrix "Text round trip": `parse(to_text(d)) == d` for a generated set
// that includes the minimum, maximum, sub-microsecond, and mixed-unit values. The same
// run is the differential between the Rust model (interpreter) and the C runtime
// (native): `run_both` fails if any text or parse result differs.
#[test]
fn duration_text_round_trips_and_both_backends_agree_on_text_and_parsing() {
    let values = generated_durations().join(",\n    ");
    let texts = DURATION_TEXTS
        .iter()
        .map(|text| format!("{text:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let source = format!(
        "{DURATION_PRELUDE}\
         fn round_trips(values: List<Duration>) -> Text {{\n\
           case values {{\n\
             [] -> \"\"\n\
             [value, ..rest] -> {{\n\
               let text = Duration.to_text(value)\n\
               let back = case Duration.parse(text) {{\n\
                 Success(parsed) if parsed == value -> \"\"\n\
                 Success(parsed) -> \"!${{Duration.to_text(parsed)}}\"\n\
                 Failure(error) -> \"!${{Debug.render(error)}}\"\n\
               }}\n\
               \"${{text}}${{back}} ${{round_trips(rest)}}\"\n\
             }}\n\
           }}\n\
         }}\n\
         fn parses(texts: List<Text>) -> Text {{\n\
           case texts {{\n\
             [] -> \"\"\n\
             [text, ..rest] -> {{\n\
               let result = case Duration.parse(text) {{\n\
                 Success(parsed) -> Duration.to_text(parsed)\n\
                 Failure(error) -> Debug.render(error)\n\
               }}\n\
               \"[${{text}}]=${{result}} ${{parses(rest)}}\"\n\
             }}\n\
           }}\n\
         }}\n\
         pub fn main() -> Text {{\n\
           let values = [\n    {values},\n  ]\n\
           \"${{round_trips(values)}}| ${{parses([{texts}])}}\"\n\
         }}\n"
    );
    let outcome = run_both_paths("duration-text", &source);
    let value = outcome
        .strip_prefix("\"outcome\":\"success\",\"value\":\"")
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or_else(|| panic!("{outcome}"));
    let (round_trips, parses) = value.split_once("| ").expect("both halves");
    assert!(!round_trips.contains('!'), "{round_trips}");
    for expected in [
        "-2562047h47m16.854775808s ",
        "2562047h47m16.854775807s ",
        "1.5us ",
        "1h0m0.000000001s ",
        "2m3.25s ",
        "1h0m30s ",
    ] {
        assert!(
            round_trips.contains(expected),
            "{expected} in {round_trips}"
        );
    }
    let parses = parses
        .split(" [")
        .map(|part| part.trim_start_matches('[').trim_end().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        parses,
        [
            "]=ParseError.Empty",
            "0]=0s",
            "-0]=ParseError.Invalid",
            "0s]=0s",
            "-0s]=0s",
            "1.5ns]=ParseError.Invalid",
            "1.0ns]=1ns",
            "1s1h]=ParseError.Invalid",
            "1h1h]=ParseError.Invalid",
            "1m1ms]=1m0.001s",
            "1ms1m]=ParseError.Invalid",
            "1.5h]=1h30m",
            "1h30m]=1h30m",
            "2m3.25s]=2m3.25s",
            "1.000000001s]=1.000000001s",
            "0.0000000005s]=ParseError.Invalid",
            "1.0000000000000s]=1s",
            "90s]=1m30s",
            "01s]=1s",
            "1.50s]=1.5s",
            "0.00000000001h]=36ns",
            "0.000000000001h]=ParseError.Invalid",
            "0.0000000000000000000000000001h]=ParseError.Invalid",
            "2562047h47m16.854775807s]=2562047h47m16.854775807s",
            "2562047h47m16.854775808s]=ParseError.OutOfRange",
            "-2562047h47m16.854775808s]=-2562047h47m16.854775808s",
            "-2562047h47m16.854775809s]=ParseError.OutOfRange",
            "9223372036854775807ns]=2562047h47m16.854775807s",
            "9223372036854775808ns]=ParseError.OutOfRange",
            "99999999999999999999999h]=ParseError.OutOfRange",
            "99999999999999999999999h1x]=ParseError.Invalid",
            "1x]=ParseError.Invalid",
            "1 s]=ParseError.Invalid",
            "+1s]=ParseError.Invalid",
            "1µs]=ParseError.Invalid",
            "1us]=1us",
            ".5s]=ParseError.Invalid",
            "1.s]=ParseError.Invalid",
            "-]=ParseError.Invalid",
            "1h-1m]=ParseError.Invalid",
            "s]=ParseError.Invalid",
            "1]=ParseError.Invalid",
        ]
    );
}

// Debug rendering uses the canonical text, a module constant holds a Duration
// (`Duration.zero` included) and is loaded from program data, and a Duration outcome
// and a parse failure (no copy of the input) encode the same on both backends.
#[test]
fn duration_renders_as_text_lives_in_constants_and_encodes_outcomes() {
    let source = format!(
        "{DURATION_PRELUDE}\
         const job_timeout = Duration.seconds(30)\n\
         pub const grace = Duration.add(Duration.zero, Duration.milliseconds(1500))\n\
         pub fn describe_status(status: Int) -> Text {{\n\
           \"payments returned ${{status}} after ${{Debug.render(job_timeout)}}\"\n\
         }}\n\
         pub fn main() -> Text {{\n\
           \"${{describe_status(503)}}; ${{Debug.render([grace, Duration.zero])}}; ${{Debug.render(Duration.parse)}}\"\n\
         }}\n"
    );
    assert_eq!(
        run_both("duration-render", &source),
        "\"outcome\":\"success\",\"value\":\"payments returned 503 after 30s; [1.5s, 0s]; <fn parse>\""
    );
    let returned = format!(
        "{DURATION_PRELUDE}pub fn main() -> Duration {{ Duration.negate(Duration.minutes(2)) }}\n"
    );
    assert_eq!(
        run_both("duration-outcome", &returned),
        "\"outcome\":\"success\",\"value\":{\"duration\":-120000000000}"
    );
    let failed = format!(
        "{DURATION_PRELUDE}\
         fn read(text: Text) -> Duration {{ Duration.parse(text) }}\n\
         pub fn main() -> Duration {{ read(\"1s1h\") }}\n"
    );
    assert_eq!(
        run_both("duration-parse-failure", &failed),
        concat!(
            "\"outcome\":\"expected_failure\",",
            "\"error\":{\"type\":\"ParseError\",\"variant\":\"Invalid\",\"fields\":{}}"
        )
    );
}

fn warning_messages(source: &str) -> Vec<String> {
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    Compiler::type_project(vec![module])
        .warnings()
        .iter()
        .map(|diagnostic| format!("{} {}", diagnostic.code(), diagnostic.message()))
        .collect()
}

// Conformance matrix "Operators": arithmetic operators reject Duration with T4303 and
// a help that names the Duration function, for every operator and for unary `-`.
#[test]
fn arithmetic_operators_reject_duration_and_name_the_function() {
    for (expression, function) in [
        ("Duration.seconds(1) + Duration.seconds(2)", "Duration.add"),
        (
            "Duration.seconds(1) - Duration.seconds(2)",
            "Duration.subtract",
        ),
        ("Duration.seconds(1) * 2", "Duration.multiply"),
        ("2 * Duration.seconds(1)", "Duration.multiply"),
        ("Duration.seconds(1) / 2", "Duration.divide"),
        ("-Duration.seconds(1)", "Duration.negate"),
    ] {
        let source = format!("{DURATION_PRELUDE}pub fn main() -> Duration {{ {expression} }}\n");
        let messages = check_messages(&source);
        assert!(
            messages.len() == 1
                && messages[0].starts_with("T4303 ")
                && messages[0].contains(&format!("`{function}`")),
            "{expression}: {messages:?}"
        );
    }
}

// Conformance matrix "Constant parse" and T4348: a literal or module-constant argument
// is checked with the runtime's parser at build time; a computed argument never is.
#[test]
fn constant_duration_arguments_are_checked_at_build_time() {
    let program = |body: &str| {
        format!(
            "{DURATION_PRELUDE}\
             const text = \"90s\"\n\
             const broken = \"1s1h\"\n\
             const count = 3000000\n\
             fn computed(name: Text, hours: Int) -> Duration {{\n{body}\n}}\n"
        )
    };
    // The last case folds to text at build time but is neither a literal nor a module
    // constant, so it is not checked.
    let cases: [(&str, &[&str], &[&str]); 9] = [
        ("  Duration.parse(\"5x\")", &["T4349"], &[]),
        ("  Duration.parse(\"\")", &["T4349"], &[]),
        ("  Duration.parse(\"2562048h\")", &["T4349"], &[]),
        ("  Duration.parse(broken)", &["T4349"], &[]),
        ("  Duration.parse(name)", &[], &[]),
        ("  Duration.hours(count)", &["T4348"], &[]),
        ("  Duration.hours(-2562048)", &["T4348"], &[]),
        ("  Duration.hours(hours)", &[], &[]),
        (
            "  Duration.parse(Duration.to_text(Duration.seconds(5)))",
            &[],
            &[],
        ),
    ];
    for (body, errors, warnings) in cases {
        let source = program(body);
        assert_eq!(check_codes(&source), errors.to_vec(), "{body}");
        assert_eq!(warning_codes(&source), warnings.to_vec(), "{body}");
    }
    assert_eq!(
        check_messages(&program("  Duration.parse(\"5x\")")),
        [concat!(
            "T4349 `Duration.parse(\"5x\")` always fails with `Duration.ParseError.Invalid`: ",
            "a duration is parts such as `1h30m` or `250ms`, with units (`h`, `m`, `s`, `ms`, ",
            "`us`, `ns`) in descending order and exact fractions"
        )]
    );
    for (argument, suggestion) in [
        (
            "\"1h30m\"",
            "Duration.add(Duration.hours(1), Duration.minutes(30))",
        ),
        ("\"5s\"", "Duration.seconds(5)"),
        ("text", "Duration.seconds(90)"),
    ] {
        let source = program(&format!("  Duration.parse({argument})"));
        assert_eq!(check_codes(&source), Vec::<String>::new(), "{argument}");
        assert_eq!(
            warning_messages(&source),
            [format!(
                "T4350 this `Duration.parse` of constant text always succeeds; write \
                 `{suggestion}`, which cannot fail and needs no failure handling"
            )],
            "{argument}"
        );
    }
}

// ts-40e675: `let _ = value` used to panic the compiler while lowering to Core.
#[test]
fn let_wildcard_evaluates_its_value_and_binds_nothing() {
    let outcome = run_both(
        "let-wildcard-evaluates",
        concat!(
            "fn boom() -> Int {\n",
            "  1 / 0\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let _ = boom()\n",
            "  7\n",
            "}\n",
        ),
    );
    assert!(
        outcome.starts_with("\"outcome\":\"defect\""),
        "the discarded value must still run: {outcome}"
    );
    let outcome = run_both(
        "let-wildcard-repeats",
        "pub fn main() -> Int {\n  let _ = 1\n  let _ = 2\n  7\n}\n",
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":7");
    assert!(
        check_codes("pub fn main() -> Int {\n  let _ = 1\n  _\n}\n")
            .contains(&"E_UNRESOLVED_VALUE".to_owned())
    );
}

// ts-d017ce, OC-01..OC-06: a multi-subject case may match any subject as an outcome; coverage
// is the single-subject rule per Success/Failure combination (patterns and
// function clauses, "Outcome patterns in a multi-subject case").
const MULTI_SUBJECT_OUTCOMES: &str = concat!(
    "enum Minor {\n",
    "  Bad\n",
    "}\n",
    "enum Major {\n",
    "  Worse\n",
    "}\n",
    "fn get(n: Int) -> Int {\n",
    "  case n {\n",
    "    0 -> Failure(Minor.Bad)\n",
    "    1 -> Failure(Major.Worse)\n",
    "    n -> n\n",
    "  }\n",
    "}\n",
    "fn pick(n: Int, previous: Int) -> Int {\n",
    "  case get(n), previous {\n",
    "    Success(value), 0 -> value\n",
    "    Success(value), p -> value + p\n",
    "    Failure(Bad), p -> p\n",
    "    Failure(Worse), p if p > 5 -> 1000\n",
    "  }\n",
    "}\n",
    "fn later(previous: Int, n: Int) -> Int {\n",
    "  case previous, get(n) {\n",
    "    0, Success(value) -> value\n",
    "    _, Success(value) -> value * 2\n",
    "    _, Failure(_) -> -1\n",
    "  }\n",
    "}\n",
    "fn both(a: Int, b: Int) -> Int {\n",
    "  case get(a), get(b) {\n",
    "    Success(x), Success(y) -> x + y\n",
    "    Success(_), Failure(_) -> 0\n",
    "    Failure(Bad), Success(_) -> -5\n",
    "    Failure(Bad), Failure(_) -> -6\n",
    "  }\n",
    "}\n",
);

#[test]
fn a_multi_subject_case_matches_an_outcome_in_any_subject() {
    let program = |main: &str| format!("{MULTI_SUBJECT_OUTCOMES}pub fn main() -> {main}\n");
    assert_eq!(
        run_both(
            "multi-subject-outcome",
            &program(concat!(
                "List<Int> {\n",
                "  [pick(7, 0), pick(7, 3), pick(0, 40), pick(1, 9),\n",
                "   later(0, 4), later(2, 4), later(2, 1),\n",
                "   both(2, 3), both(2, 1), both(0, 3), both(0, 1)]\n",
                "}",
            ))
        ),
        "\"outcome\":\"success\",\"value\":[7,10,40,1000,4,8,-1,5,0,-5,-6]"
    );
    // No arm matches `Failure(Worse), 2`, so the failure bubbles; for `both`
    // the first subject's failure bubbles when no arm matches.
    assert_eq!(
        run_both(
            "multi-subject-outcome-bubbles",
            &program("Int { pick(1, 2) }")
        ),
        failure("Major", "Worse")
    );
    assert_eq!(
        run_both(
            "multi-subject-outcome-first",
            &program("Int { both(1, 0) }")
        ),
        failure("Major", "Worse")
    );
}

#[test]
fn a_multi_subject_outcome_case_subtracts_only_fully_covered_failures() {
    let program = |function: &str, row: &str| {
        format!(
            "{MULTI_SUBJECT_OUTCOMES}fn call(operation: fn() -> Int{row}) -> Int {{ operation() }}\n\
             pub fn main() -> Int {{ call(fn() {{ {function} }}) }}\n"
        )
    };
    // `Bad` is covered for every `previous`; `Worse` only under a guard.
    assert!(check_codes(&program("pick(1, 2)", " errors Major")).is_empty());
    assert_eq!(check_codes(&program("pick(1, 2)", "")), ["T4203"]);
    // The first subject's `Worse` bubbles. The second subject fails only after the
    // first succeeded, and every such case is matched, so it adds nothing.
    assert!(check_codes(&program("both(1, 2)", " errors Major")).is_empty());
    // `later` handles every failure of its second subject.
    assert!(check_codes(&program("later(1, 2)", "")).is_empty());
    // A later subject that cannot fail has no failing combination to cover, so an
    // earlier subject's handled failures still leave the row.
    let infallible_later = format!(
        "{MULTI_SUBJECT_OUTCOMES}fn safe(n: Int) -> Int {{ n + 1 }}\n\
         fn guarded(a: Int, b: Int) -> Int {{\n\
         \x20 case get(a), safe(b) {{\n\
         \x20   Success(x), Success(y) -> x + y\n\
         \x20   Failure(_), Success(y) -> y\n\
         \x20 }}\n\
         }}\n\
         fn call(operation: fn() -> Int) -> Int {{ operation() }}\n\
         pub fn main() -> Int {{ call(fn() {{ guarded(0, 1) }}) }}\n"
    );
    assert!(check_codes(&infallible_later).is_empty());
    // A variant covered only for some values of the other subject stays in the row.
    let partial = MULTI_SUBJECT_OUTCOMES.replace("Failure(Bad), p -> p", "Failure(Bad), 0 -> 0");
    let partial = format!(
        "{partial}fn call(operation: fn() -> Int errors Major) -> Int {{ operation() }}\n\
         pub fn main() -> Int {{ call(fn() {{ pick(1, 2) }}) }}\n"
    );
    assert_eq!(check_codes(&partial), ["T4203"]);
}

#[test]
fn a_multi_subject_outcome_case_reports_what_it_does_not_cover() {
    let case = |arms: &str| {
        format!(
            "enum Oops {{\n  Bad\n}}\n\
             fn get(n: Int) -> Int {{ case n {{\n  0 -> Failure(Oops.Bad)\n  n -> n\n}} }}\n\
             pub fn main() -> Int {{\n  case get(1), 2 {{\n{arms}  }}\n}}\n"
        )
    };
    // The all-Success combination must be exhaustive, and the witness names it.
    let messages = check_messages(&case("    Success(v), 0 -> v\n    Failure(_), p -> p\n"));
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].starts_with("T4223")
            && messages[0].contains("`Success(_), ")
            && messages[0].ends_with("` is not matched"),
        "{messages:?}"
    );
    // A fallible subject matched as an outcome needs a Failure arm.
    let messages = check_messages(&case("    Success(v), _ -> v\n"));
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].starts_with("T4223") && messages[0].contains("Failure of subject 1"),
        "{messages:?}"
    );
    // Once a subject is matched as an outcome, every arm must match it that way:
    // a plain pattern never matches a captured outcome.
    assert_eq!(
        check_codes(&case(
            "    Success(v), _ -> v\n    Failure(_), _ -> 0\n    other, _ -> 1\n"
        )),
        ["T4222"]
    );
    // Reachability is per combination.
    assert_eq!(
        check_codes(&case(
            "    Success(v), _ -> v\n    Failure(_), _ -> 0\n    Failure(Bad), 2 -> 1\n"
        )),
        ["T4232"]
    );
}

// RU-12 (ts-8250de): a local base is read directly, but values written out of
// declaration order are still evaluated in source order.
#[test]
fn an_update_of_a_local_base_evaluates_values_in_source_order() {
    let program = |verbose_fails: &str, jobs_fails: &str| {
        format!(
            concat!(
                "{}",
                "enum Step {{\n",
                "  First\n",
                "  Second\n",
                "}}\n",
                "fn jobs(fails: Bool) -> Int {{\n",
                "  case fails {{\n",
                "    True -> Failure(Step.Second)\n",
                "    False -> 2\n",
                "  }}\n",
                "}}\n",
                "fn verbose(fails: Bool) -> Bool {{\n",
                "  case fails {{\n",
                "    True -> Failure(Step.First)\n",
                "    False -> True\n",
                "  }}\n",
                "}}\n",
                "fn change(config: Config, verbose_fails: Bool, jobs_fails: Bool) -> Config {{\n",
                "  {{ ..config, verbose: verbose(verbose_fails), jobs: jobs(jobs_fails) }}\n",
                "}}\n",
                "pub fn main() -> Int {{\n",
                "  let updated = change(Config(\"a\", 1, False), {}, {})\n",
                "  case updated.verbose {{\n",
                "    True -> updated.jobs\n",
                "    False -> 0\n",
                "  }}\n",
                "}}\n",
            ),
            CONFIG, verbose_fails, jobs_fails
        )
    };
    assert_eq!(
        run_both("local-update-order", &program("True", "True")),
        failure("Step", "First")
    );
    assert_eq!(
        run_both("local-update-success", &program("False", "False")),
        "\"outcome\":\"success\",\"value\":2"
    );
}

// ts-6c5e9e: a clause pattern that only names its parameter reads the parameter,
// so a closure that captures that name and a guard that reads it see the argument.
// `factor` is never inspected, so its column is not matched at all.
#[test]
fn clause_parameter_names_reach_closures_and_guards() {
    let outcome = run_both(
        "clause-parameter-aliases",
        concat!(
            "fn scale(factor: Int, 0) -> fn(Int) -> Int {\n",
            "  fn(x) { x * factor }\n",
            "}\n",
            "fn scale(factor, n) if factor > n {\n",
            "  fn(x) { x * factor + n }\n",
            "}\n",
            "fn scale(_, n) {\n",
            "  fn(x) { x - n }\n",
            "}\n",
            "pub fn main() -> List<Int> {\n",
            "  let triple = scale(3, 0)\n",
            "  let shifted = scale(5, 1)\n",
            "  let lowered = scale(1, 4)\n",
            "  [triple(2), shifted(2), lowered(10)]\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":[6,11,6]");
}

// Supervised processes, stage U1 (process-and-supervisor-static-contracts.md). The
// conformance lab's static fixtures cover T4261-T4264 at their contract scenarios;
// these cover the edges the fixtures do not.

const PROCESS_PRELUDE: &str = concat!(
    "import std/duration as Duration\n",
    "import std/process as Process\n",
    "import std/process.{Inbox}\n",
    "import std/supervisor as Supervisor\n",
    "enum Message {\n  Ping\n}\n",
    "fn body(inbox: Inbox<Message>) -> Void {\n  Void\n}\n",
);

fn process_program(declarations: &str) -> String {
    format!("{PROCESS_PRELUDE}{declarations}pub fn main() -> Int {{\n  0\n}}\n")
}

// Rejects a formatter that drops or rewrites the declaration, and a parser that
// reserves `process` or `name` instead of reading them only at item position.
#[test]
fn process_name_declarations_format_and_process_stays_an_ordinary_word() {
    let source = process_program("pub process name counter: Message\n");
    let formatted = Compiler::format(SourceSnapshot::from_text("main.chimi", source.as_str()));
    assert!(
        formatted
            .text()
            .contains("\npub process name counter: Message\n")
    );
    assert_formatter_round_trips(&source);
    assert_inspection_survives_every_offset(&source);
    assert_eq!(
        check_codes(concat!(
            "fn process(name: Int) -> Int {\n  let process = name\n  process\n}\n",
            "pub fn main() -> Int {\n  process(1)\n}\n",
        )),
        Vec::<String>::new()
    );
}

// T4263 applies to literals only (the static contract's definition). Rejects a check
// that folds constants or arithmetic: those values defect where they are received.
#[test]
fn literal_range_checks_ignore_constants_and_computed_values() {
    assert_eq!(
        check_codes(&process_program(concat!(
            "const none = 0\n",
            "fn build() {\n",
            "  Process.spec(body, none, Duration.seconds(5 - 10))\n",
            "}\n",
            "fn restarts() {\n  Supervisor.restarts(0 - 1, Duration.seconds(none))\n}\n",
        ))),
        Vec::<String>::new()
    );
    assert_eq!(
        check_codes(&process_program(
            "fn build() {\n  Process.spec(body, (0), Duration.nanoseconds(-1))\n}\n",
        )),
        ["T4263", "T4263"]
    );
}

// Rejects a T4262 that accepts any expression of type `Address`, such as a constant
// that holds a name, while a parenthesised direct reference stays accepted.
#[test]
fn register_takes_only_a_direct_process_name_reference() {
    let program = |name: &str| {
        process_program(&format!(
            "process name counter: Message\nconst alias = counter\nfn build() {{\n  \
             Process.register(Process.spec(body, 1, Duration.seconds(1)), {name})\n}}\n"
        ))
    };
    assert_eq!(check_codes(&program("alias")), ["T4262"]);
    // A name resolution already rejected reports only its own error.
    let unresolved = check_codes(&program("missing"));
    assert!(!unresolved.contains(&"T4262".to_owned()), "{unresolved:?}");
    let library = "pub enum Note {\n  Ping\n}\nprocess name hidden: Note\n";
    let private = build_codes(&[
        ("lib", library),
        (
            "main",
            &format!(
                "import lib as Lib\n{}",
                process_program(
                    "fn build() {\n  Process.register(Process.spec(body, 1, \
                     Duration.seconds(1)), Lib.hidden)\n}\n"
                )
            ),
        ),
    ]);
    assert!(
        !private.is_empty() && !private.contains(&"T4262".to_owned()),
        "{private:?}"
    );
    assert_eq!(check_codes(&program("(counter)")), Vec::<String>::new());
}

// Rejects a T4264 that follows only one constant, or that keys names by text: two
// modules' `counter` are different names (SP09 covers the module case in the lab).
#[test]
fn duplicate_names_are_found_through_chains_of_constants() {
    assert_eq!(
        check_codes(&process_program(concat!(
            "process name counter: Message\n",
            "const named = Process.spec(body, 1, Duration.seconds(1)) |> Process.register(counter)\n",
            "const member = named |> Supervisor.process(Supervisor.Restart.Never)\n",
            "fn build() {\n",
            "  Supervisor.policy([member, member], 2, Supervisor.restarts(1, Duration.seconds(1)))\n",
            "}\n",
        ))),
        ["T4264"]
    );
}

// The data constructors run at build time with the native kernel's defect texts.
// Rejects an interpreter that skips the range checks for computed values, or that
// lets a spec be registered twice.
#[test]
fn process_data_constructors_check_computed_values_when_they_run() {
    for (declarations, defect) in [
        (
            "const spec = Process.spec(body, 1 - 1, Duration.seconds(1))\n",
            "Process.spec: mailbox capacity must be at least 1, got 0",
        ),
        (
            "const spec = Process.spec(body, 1, Duration.seconds(1 - 2))\n",
            "Process.spec: shutdown_within must be at least zero, got -1s",
        ),
        (
            "const restarts = Supervisor.restarts(1, Duration.seconds(1 - 1))\n",
            "Supervisor.restarts: restart window must be positive, got 0s",
        ),
        (
            "const restarts = Supervisor.restarts(1 - 2, Duration.seconds(1))\n",
            "Supervisor.restarts: restart limit must be at least 0, got -1",
        ),
        (
            concat!(
                "const members = [Process.spec(body, 1, Duration.seconds(1)) ",
                "|> Supervisor.process(Supervisor.Restart.Never)]\n",
                "const policy = Supervisor.policy(members, 1 - 1, ",
                "Supervisor.restarts(1, Duration.seconds(1)))\n",
            ),
            "Supervisor.policy: capacity must be at least 1, got 0",
        ),
        (
            concat!(
                "const two = [Process.spec(body, 1, Duration.seconds(1)) ",
                "|> Supervisor.process(Supervisor.Restart.Never), ",
                "Process.spec(body, 2, Duration.seconds(1)) ",
                "|> Supervisor.process(Supervisor.Restart.Never)]\n",
                "const policy = Supervisor.policy(two, 2 - 1, ",
                "Supervisor.restarts(1, Duration.seconds(1)))\n",
            ),
            "Supervisor.policy: members must not exceed capacity 1, got 2",
        ),
        (
            concat!(
                "process name first: Message\nprocess name second: Message\n",
                "const spec = Process.spec(body, 1, Duration.seconds(1)) ",
                "|> Process.register(first) |> Process.register(second)\n",
            ),
            "Process.register: spec already has a name",
        ),
    ] {
        let messages = build_messages(&process_program(declarations));
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(
            messages[0].starts_with("E5202") && messages[0].contains(defect),
            "{messages:?}"
        );
    }
    assert_eq!(
        build_messages(&process_program(concat!(
            "process name counter: Message\n",
            "const policy = Supervisor.policy(\n",
            "  [Process.spec(body, 4, Duration.zero) |> Process.register(counter) ",
            "|> Supervisor.process(Supervisor.Restart.OnDefect)],\n",
            "  1,\n  Supervisor.restarts(0, Duration.seconds(30)),\n)\n",
        ))),
        Vec::<String>::new()
    );
}

// Rejects classifying a runtime operation as pure, which would let a constant send a
// message while the program is being built.
#[test]
fn process_operations_are_not_build_time_operations() {
    let messages = build_messages(&process_program(concat!(
        "process name pinger: Message\n",
        "fn ping() -> Int {\n  case Process.send(pinger, Message.Ping) {\n",
        "    Success(_) -> 0\n    Failure(_) -> 1\n  }\n}\n",
        "const pinged = ping()\n",
    )));
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].starts_with("E5201") && messages[0].contains("`std/process.send`"),
        "{messages:?}"
    );
}

// Coordinator decision 1 (ts-6ccbc9): the interpreter has no process runtime and says
// so at the first operation that needs one, instead of diverging from native.
#[test]
fn the_interpreter_defects_at_the_first_process_operation() {
    let scratch = scratch_directory("process-interpreter-defect");
    let path = scratch.join("main.chimi");
    std::fs::write(
        &path,
        format!(
            "{PROCESS_PRELUDE}process name pinger: Message\n\
             pub fn main() -> Int {{\n  case Process.send(pinger, Message.Ping) {{\n    \
             Success(_) -> 0\n    Failure(_) -> 1\n  }}\n}}\n"
        ),
    )
    .unwrap();
    let outcome = interpreter_outcome(&path);
    std::fs::remove_dir_all(scratch).unwrap();
    assert!(
        outcome.contains("\"outcome\":\"defect\"")
            && outcome.contains("Process.send requires the supervised process runtime"),
        "{outcome}"
    );
}

// executable-kernel.md "Program entry": the runner's provider plan closes a root
// `clock: Clock` need, including one that reaches `main` through a started task.
// Rejects a closure that ignores the fixed need name (`timer: Clock` must still be
// E5002) or that reaches past the root (a constant still reports T4322).
#[test]
fn the_runner_closes_only_a_root_clock_need() {
    let program = |declarations: &str| {
        check_codes(&format!(
            "import std/clock as Clock\nimport std/duration as Duration\n\
             import std/task as Task\n{declarations}"
        ))
    };
    let nap = "fn nap() -> Int needs { clock: Clock.Clock } {\n  \
               Clock.sleep(Duration.seconds(1))\n  1\n}\n";
    assert_eq!(
        program(&format!("{nap}pub fn main() -> Int {{\n  nap()\n}}\n")),
        Vec::<String>::new()
    );
    assert_eq!(
        program(&format!(
            "{nap}pub fn main() -> Int {{\n  Task.result(Task.start(nap))\n}}\n"
        )),
        Vec::<String>::new()
    );
    assert_eq!(
        program(
            "fn nap() -> Int needs { timer: Clock.Clock } {\n  1\n}\n\
             pub fn main() -> Int {\n  nap()\n}\n"
        ),
        ["E5002"]
    );
    assert_eq!(
        program(
            "const later = Clock.sleep(Duration.seconds(1))\n\
             pub fn main() -> Int {\n  1\n}\n"
        ),
        ["T4322"]
    );
}

// time-and-outcomes.md "Clock slice": the interpreter has no scheduler clock and says
// so at the sleep instead of returning as if no time had passed.
#[test]
fn the_interpreter_defects_at_clock_sleep() {
    let scratch = scratch_directory("clock-interpreter-defect");
    let path = scratch.join("main.chimi");
    std::fs::write(
        &path,
        "import std/clock as Clock\nimport std/duration as Duration\n\
         pub fn main() -> Int {\n  Clock.sleep(Duration.seconds(1))\n  1\n}\n",
    )
    .unwrap();
    let outcome = interpreter_outcome(&path);
    std::fs::remove_dir_all(scratch).unwrap();
    assert!(
        outcome.contains("\"outcome\":\"defect\"")
            && outcome.contains(
                "Clock.sleep requires the native runtime; build the program with `chimi build`"
            ),
        "{outcome}"
    );
}

// Rejects a `Process.spec` that drops the body's needs or keeps them in the spec, and
// a `Clock.sleep` without its `clock` need.
#[test]
fn a_spec_surfaces_its_body_needs_at_the_call() {
    let source = process_program(concat!(
        "import std/clock as Clock\n",
        "fn napping(inbox: Inbox<Message>) -> Void needs { clock: Clock.Clock } {\n",
        "  Clock.sleep(Duration.seconds(1))\n",
        "}\n",
        "pub fn build() {\n  Process.spec(napping, 1, Duration.seconds(1))\n}\n",
    ));
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    let report = Compiler::type_project(vec![module]);
    assert!(report.is_success(), "{}", report.render_diagnostics());
    assert_eq!(
        report.contract("main", "build").as_deref(),
        Some("fn() -> Spec<Message, Never> needs { clock: Clock }")
    );
    assert_eq!(
        report.standard_contract("Clock.sleep").as_deref(),
        Some("fn(Duration) -> Void needs { clock: Clock }")
    );
}

// The static contract's "Types" table. Rejects a wrong capability class, which U2's
// capability rules (T4240, T4241, T4260) build on.
#[test]
fn process_handles_have_the_contracted_capability_classes() {
    use chimi::ValuePropertyStatus::{Always, Never};
    let source = process_program(concat!(
        "pub fn handles(\n",
        "  address: Process.Address<Message>,\n",
        "  inbox: Inbox<Message>,\n",
        "  managed: Supervisor.ManagedProcess<Message>,\n",
        "  running: Supervisor.Running,\n",
        ") -> Int {\n  0\n}\n",
    ));
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    let report = Compiler::type_project(vec![module]);
    let properties = |input| {
        let properties = report
            .function_input_value_properties("main", "handles", input)
            .unwrap();
        (properties.transferable, properties.shareable)
    };
    // Shareable; task-local (neither); exclusive (moves, never shared); shareable.
    assert_eq!(
        [0, 1, 2, 3].map(properties),
        [
            (Always, Always),
            (Never, Never),
            (Always, Never),
            (Always, Always)
        ]
    );
}

const STORE_PROGRAM: &str = concat!(
    "import std/channel.{Receiver}\n",
    "interface Store {\n  read: fn() -> Int\n}\n",
    "fn serve(inbox: Inbox<Message>) -> Void needs { store: Store } {\n",
    "  let value = store.read()\n  Void\n}\n",
);

// A provider whose method captures an exclusive `Receiver`, so it is not shareable.
const LOCAL_STORE: &str = "  let local = { read: fn() -> Int {\n    receiver\n    1\n  } }\n";

// T4260 for a provider that is not shareable, wherever `using` binds it to a spec
// body's need. Rejects a check that looks only at the spec body argument (misses
// the outer `using` and the helper), one that runs functions in declaration order
// without a fixed point (`late_helper` is declared after its caller), and one that
// flags every non-shareable provider rather than those bound to a spec's need.
#[test]
fn a_spec_provider_must_be_shareable_wherever_using_binds_it() {
    let source = process_program(&format!(
        concat!(
            "{program}",
            "fn uses_store() -> Int needs {{ store: Store }} {{\n  store.read()\n}}\n",
            "fn attached(receiver: Receiver<Int>) {{\n{local}",
            "  Process.spec(serve using {{ store: local }}, 1, Duration.seconds(1))\n",
            "}}\n",
            "fn outer(receiver: Receiver<Int>) {{\n{local}",
            "  Process.spec(serve, 1, Duration.seconds(1)) using {{ store: local }}\n",
            "}}\n",
            "fn through_helper(receiver: Receiver<Int>) {{\n{local}",
            "  late_helper() using {{ store: local }}\n",
            "}}\n",
            "fn late_helper() {{\n  Process.spec(serve, 1, Duration.seconds(1))\n}}\n",
            "fn not_a_spec(receiver: Receiver<Int>) -> Int {{\n{local}",
            "  uses_store() using {{ store: local }}\n",
            "}}\n",
        ),
        program = STORE_PROGRAM,
        local = LOCAL_STORE,
    ));
    let diagnostics = check_diagnostics(&source);
    let found = diagnostics
        .iter()
        .map(|(code, text, _)| (code.as_str(), text.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        found,
        [
            ("T4260", "serve using { store: local }"),
            ("T4260", "local"),
            ("T4260", "local"),
        ],
        "{diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .all(|(_, _, message)| message.contains("provider `store`"))
    );
}

// Both send operations move every exclusive value the payload is built from, not
// only a payload that is itself a binding. Rejects a transfer that inspects only a
// bare name (a `Receiver` wrapped in a constructor would stay usable), and one that
// also moves shareable values (a `Running` sent in a message stays usable).
#[test]
fn a_send_moves_the_exclusive_values_its_payload_is_built_from() {
    let source = process_program(concat!(
        "import std/channel as Channel\n",
        "import std/channel.{Receiver, Sender}\n",
        "enum Hand {\n  Over(receiver: Receiver<Int>)\n  Root(root: Supervisor.Running)\n}\n",
        "fn by_channel(sender: Sender<Hand>, receiver: Receiver<Int>) -> Receiver<Int> {\n",
        "  Channel.send(sender, Hand.Over(receiver))\n  receiver\n}\n",
        "fn by_process(to: Process.Address<Hand>, receiver: Receiver<Int>) -> Receiver<Int> {\n",
        "  Process.send(to, Hand.Over(receiver))\n  receiver\n}\n",
        "fn shared(to: Process.Address<Hand>, root: Supervisor.Running) -> Supervisor.Running {\n",
        "  Process.send(to, Hand.Root(root))\n  root\n}\n",
    ));
    let diagnostics = check_diagnostics(&source);
    let found = diagnostics
        .iter()
        .map(|(code, text, _)| (code.as_str(), text.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        found,
        [("T4241", "receiver"), ("T4241", "receiver")],
        "{diagnostics:?}"
    );
}

// Each payload shape that moves a binding, one function each. Rejects a transfer
// that drops the list, tuple, record, or parenthesis case, follows constructors only
// one level deep, or treats a project function's result as a new value that holds
// nothing of its argument (ts-184972).
#[test]
fn a_send_moves_bindings_through_every_payload_shape() {
    let source = process_program(concat!(
        "import std/channel as Channel\n",
        "import std/channel.{Receiver, Sender}\n",
        "enum Hand {\n  Over(receiver: Receiver<Int>)\n}\n",
        "enum Parcel {\n  Hold(hand: Hand)\n}\n",
        "fn list(sender: Sender<List<Receiver<Int>>>, r: Receiver<Int>) -> Receiver<Int> {\n",
        "  Channel.send(sender, [r])\n  r\n}\n",
        "fn tuple(sender: Sender<(Receiver<Int>, Int)>, r: Receiver<Int>) -> Receiver<Int> {\n",
        "  Channel.send(sender, (r, 1))\n  r\n}\n",
        "fn record(sender: Sender<{ r: Receiver<Int> }>, r: Receiver<Int>) -> Receiver<Int> {\n",
        "  Channel.send(sender, { r: r })\n  r\n}\n",
        "fn grouped(to: Process.Address<Hand>, r: Receiver<Int>) -> Receiver<Int> {\n",
        "  Process.send(to, (Hand.Over(r)))\n  r\n}\n",
        "fn nested(to: Process.Address<Parcel>, r: Receiver<Int>) -> Receiver<Int> {\n",
        "  Process.send(to, Parcel.Hold(Hand.Over(r)))\n  r\n}\n",
        "fn wrap(r: Receiver<Int>) -> Hand {\n  Hand.Over(r)\n}\n",
        "fn through_call(to: Process.Address<Hand>, r: Receiver<Int>) -> Receiver<Int> {\n",
        "  Process.send(to, wrap(r))\n  r\n}\n",
    ));
    let diagnostics = check_diagnostics(&source);
    // One `T4241` per function, at the use of `r` after its send.
    let found = diagnostics
        .iter()
        .map(|(code, text, message)| {
            let boundary = if message.contains("by Channel.send") {
                "Channel.send"
            } else if message.contains("by Process.send") {
                "Process.send"
            } else {
                "other"
            };
            (code.as_str(), text.as_str(), boundary)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        found,
        [
            ("T4241", "r", "Channel.send"),
            ("T4241", "r", "Channel.send"),
            ("T4241", "r", "Channel.send"),
            ("T4241", "r", "Process.send"),
            ("T4241", "r", "Process.send"),
            ("T4241", "r", "Process.send"),
        ],
        "{diagnostics:?}"
    );
}

const HAND_PROGRAM: &str = concat!(
    "import std/channel as Channel\n",
    "import std/channel.{Receiver, Sender}\n",
    "import std/task as Task\n",
    "enum Hand {\n  Over(receiver: Receiver<Int>)\n}\n",
    "enum Parcel {\n  Hold(hand: Hand)\n}\n",
    "fn wrap(r: Receiver<Int>) -> Hand {\n  Hand.Over(r)\n}\n",
    "struct Box {\n  inner: Receiver<Int>\n}\n",
    "fn boxed(r: Receiver<Int>) -> Box {\n  Box(r)\n}\n",
    "interface Store {\n  read: fn() -> Int\n}\n",
    "fn uses_store() -> Int needs { store: Store } {\n  store.read()\n}\n",
);

// ts-184972: a value built from an exclusive binding takes it, so the binding cannot
// stay usable while the value crosses a task boundary. One function per route by
// which `r` reaches a sent or bound value, each ending in a use of `r` (or of the
// binding that took it). Rejects a move that looks only at the send's syntax (the
// let, block, case, loop, and alias routes), one that drops what a block, case arm,
// or `break` yields, one that treats a call's result as holding nothing of its
// arguments or of the one-shot closure it calls,
// and a let or loop state that does not move what its value is built from.
#[test]
fn an_exclusive_binding_moves_into_the_value_built_from_it() {
    let source = process_program(&format!(
        concat!(
            "{program}",
            "fn let_bound(s: Sender<Hand>, r: Receiver<Int>) -> Receiver<Int> {{\n",
            "  let m = Hand.Over(r)\n  Channel.send(s, m)\n  r\n}}\n",
            "fn block(s: Sender<Hand>, r: Receiver<Int>) -> Receiver<Int> {{\n",
            "  Channel.send(s, {{\n    Hand.Over(r)\n  }})\n  r\n}}\n",
            "fn case_arm(s: Sender<Hand>, r: Receiver<Int>, q: Receiver<Int>, f: Bool) -> Receiver<Int> {{\n",
            "  Channel.send(s, case f {{\n    True -> Hand.Over(r)\n    False -> Hand.Over(q)\n  }})\n",
            "  r\n}}\n",
            "fn case_later_arm(s: Sender<Hand>, r: Receiver<Int>, q: Receiver<Int>, f: Bool) -> Receiver<Int> {{\n",
            "  Channel.send(s, case f {{\n    True -> Hand.Over(r)\n    False -> Hand.Over(q)\n  }})\n",
            "  q\n}}\n",
            "fn case_swapped(s: Sender<Hand>, r: Receiver<Int>, q: Receiver<Int>, f: Bool) -> Receiver<Int> {{\n",
            "  Channel.send(s, case f {{\n    True -> Hand.Over(q)\n    False -> Hand.Over(r)\n  }})\n",
            "  r\n}}\n",
            "fn updated(s: Sender<{{ r: Receiver<Int>, n: Int }}>, base: {{ r: Receiver<Int>, n: Int }}, r: Receiver<Int>) -> Receiver<Int> {{\n",
            "  Channel.send(s, {{ ..base, r: r }})\n  r\n}}\n",
            "fn function_built(s: Sender<Hand>, r: Receiver<Int>) -> Receiver<Int> {{\n",
            "  let m = wrap(r)\n  Channel.send(s, m)\n  r\n}}\n",
            "fn nested(s: Sender<Parcel>, r: Receiver<Int>) -> Hand {{\n",
            "  let inner = wrap(r)\n  let outer = Parcel.Hold(inner)\n  Channel.send(s, outer)\n",
            "  inner\n}}\n",
            "fn loop_break(s: Sender<Hand>, r: Receiver<Int>) -> Receiver<Int> {{\n",
            "  let m = loop n = 0 {{\n    break Hand.Over(r)\n  }}\n  Channel.send(s, m)\n  r\n}}\n",
            "fn loop_state(r: Receiver<Int>) -> Receiver<Int> {{\n",
            "  let count = loop held = Hand.Over(r) {{\n    break 1\n  }}\n  r\n}}\n",
            "fn closure_built(r: Receiver<Int>) -> Hand {{\n",
            "  let make = fn() -> Hand {{\n    Hand.Over(r)\n  }}\n  let first = make()\n  make()\n}}\n",
            "fn field_of_call(r: Receiver<Int>) -> Receiver<Int> {{\n",
            "  let x = boxed(r).inner\n  r\n}}\n",
            "fn destructured(r: Receiver<Int>) -> Receiver<Int> {{\n",
            "  let (x, n) = (r, 1)\n  r\n}}\n",
            "fn provided(r: Receiver<Int>) -> Int {{\n",
            "  let local = {{ read: fn() -> Int {{\n    r\n    1\n  }} }}\n",
            "  let a = uses_store() using {{ store: local }}\n  local\n  a\n}}\n",
            "fn alias(r: Receiver<Int>) -> Receiver<Int> {{\n",
            "  let x = r\n  let child = Task.start(fn() -> Int {{\n    Channel.receive(x)\n    1\n  }})\n",
            "  r\n}}\n",
        ),
        program = HAND_PROGRAM,
    ));
    let diagnostics = check_diagnostics(&source);
    let found = diagnostics
        .iter()
        .map(|(code, text, message)| {
            let boundary = message
                .split_once("transferred ")
                .and_then(|(_, rest)| rest.split_once(" and "))
                .map_or("", |(boundary, _)| boundary);
            (code.as_str(), text.as_str(), boundary)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        found,
        [
            ("T4241", "r", "into `m`"),
            ("T4241", "r", "by Channel.send"),
            ("T4241", "r", "by Channel.send"),
            ("T4241", "q", "by Channel.send"),
            ("T4241", "r", "by Channel.send"),
            ("T4241", "r", "by Channel.send"),
            ("T4241", "r", "into `m`"),
            ("T4241", "inner", "into `outer`"),
            ("T4241", "r", "into `m`"),
            ("T4241", "r", "into `held`"),
            ("T4241", "make", "into `first`"),
            ("T4241", "r", "into `x`"),
            ("T4241", "r", "into a pattern"),
            ("T4241", "local", "into a one-shot closure"),
            ("T4241", "r", "into `x`"),
        ],
        "{diagnostics:?}"
    );
}

// Building a value from a binding inside a loop takes the binding on the first pass.
// Rejects a let move that the loop rule does not see.
#[test]
fn building_a_value_from_an_outer_binding_inside_a_loop_is_reported() {
    let source = process_program(&format!(
        concat!(
            "{program}",
            "fn each(s: Sender<Hand>, r: Receiver<Int>) -> Int {{\n",
            "  loop count = 0 {{\n    let m = Hand.Over(r)\n",
            "    case count >= 3 {{\n      True -> break count\n      False -> count + 1\n    }}\n",
            "  }}\n}}\n",
        ),
        program = HAND_PROGRAM,
    ));
    let diagnostics = check_diagnostics(&source);
    let found = diagnostics
        .iter()
        .map(|(code, text, message)| (code.as_str(), text.as_str(), message.contains("into `m`")))
        .collect::<Vec<_>>();
    assert_eq!(found, [("T4250", "Hand.Over(r)", true)], "{diagnostics:?}");
}

// Moving into a value must not move what only reads a capability. Each function is
// a legitimate owner pattern that must stay free of diagnostics. Rejects a rule that
// moves a receiver passed to `Channel.receive`, one that moves an argument of a call
// whose result is shareable (a helper that receives once, called on every pass), one
// that moves a record when a field is read out of it (a capability or a closure
// field), one that reports the owner binding itself once its value is bound, and one
// that lets any exclusive argument flow into a non-shareable result: `next_hand` and
// `first_of` read a receiver whose type does not occur in their result (nor does
// `slot_of`, through a generic struct's field), so calling them on every pass or
// twice moves nothing, and calling a one-shot closure whose result is shareable
// does not move the closure.
// `handles` guards the typing of `Receive<T>`, which follows `T` (ts-bb95dc):
// `Channel.receive` must still not carry its receiver.
#[test]
fn reading_or_owning_an_exclusive_binding_does_not_move_it() {
    let source = process_program(&format!(
        concat!(
            "{program}",
            "import std/channel.{{Completion, Receive}}\n",
            "fn next(r: Receiver<Int>) -> Int {{\n",
            "  let next = Channel.receive(r)\n",
            "  case next {{\n    Receive.Received(value) -> value\n    Receive.Drained -> 0\n  }}\n",
            "}}\n",
            "fn reads_then_sends(s: Sender<Hand>, r: Receiver<Int>) -> Int {{\n",
            "  let first = Channel.receive(r)\n  let second = Channel.receive(r)\n",
            "  Channel.send(s, Hand.Over(r))\n  1\n}}\n",
            "fn helper_in_loop(r: Receiver<Int>) -> Int {{\n",
            "  loop total = 0 {{\n    let value = next(r)\n",
            "    case value == 0 {{\n      True -> break total\n      False -> total + value\n    }}\n",
            "  }}\n}}\n",
            "fn handles(rs: Receiver<Receiver<Int>>) -> Int {{\n",
            "  let first = Channel.receive(rs)\n  let second = Channel.receive(rs)\n  1\n}}\n",
            "fn owner_sends(s: Sender<Parcel>, r: Receiver<Int>) -> Int {{\n",
            "  let m = Hand.Over(r)\n  let parcel = Parcel.Hold(m)\n  Channel.send(s, parcel)\n  1\n}}\n",
            "fn fields() -> Int {{\n",
            "  let channel = Channel.bounded(1)\n  let receiver = channel.receiver\n",
            "  let completion = channel.completion\n",
            "  let child = Task.start(fn() -> Int {{\n    Channel.receive(receiver)\n    1\n  }})\n",
            "  Channel.close(completion)\n  1\n}}\n",
            "fn matched(h: Hand) -> Hand {{\n",
            "  let n = case h {{\n    Hand.Over(x) -> 1\n  }}\n  h\n}}\n",
            "fn closure_field(r: Receiver<Int>, c: Completion<Int>) -> Int {{\n",
            "  let job = {{ run: fn() -> Int {{\n    Channel.receive(r)\n    1\n  }}, done: c }}\n",
            "  let run = job.run\n  let done = job.done\n  1\n}}\n",
            "fn next_hand(rs: Receiver<Hand>) -> Hand {{\n",
            "  let got = Channel.receive(rs)\n",
            "  case got {{\n    Receive.Received(h) -> h\n    Receive.Drained -> wrap(rs_fallback())\n  }}\n",
            "}}\n",
            "fn rs_fallback() -> Receiver<Int> {{\n  Channel.bounded(1).receiver\n}}\n",
            "fn dispatch(rs: Receiver<Hand>) -> Int {{\n",
            "  loop n = 0 {{\n    let h = next_hand(rs)\n",
            "    case n >= 3 {{\n      True -> break n\n      False -> n + 1\n    }}\n",
            "  }}\n}}\n",
            "fn first_of(rs: Receiver<Receiver<Int>>) -> Option<Receiver<Int>> {{\n",
            "  let got = Channel.receive(rs)\n",
            "  case got {{\n    Receive.Received(h) -> Option.Some(h)\n    Receive.Drained -> Option.None\n  }}\n",
            "}}\n",
            "fn first_twice(rs: Receiver<Receiver<Int>>) -> Int {{\n",
            "  let a = first_of(rs)\n  let b = first_of(rs)\n  1\n}}\n",
            "struct Slot<T> {{\n  item: T\n}}\n",
            "fn slot_of(rs: Receiver<Receiver<Int>>) -> Slot<Receiver<Int>> {{\n",
            "  Slot(rs_fallback())\n}}\n",
            "fn slot_twice(rs: Receiver<Receiver<Int>>) -> Int {{\n",
            "  let a = slot_of(rs)\n  let b = slot_of(rs)\n  1\n}}\n",
            "fn closure_twice(r: Receiver<Int>) -> Int {{\n",
            "  let peek = fn() -> Int {{\n    r\n    1\n  }}\n  let a = peek()\n  let b = peek()\n  a + b\n}}\n",
        ),
        program = HAND_PROGRAM,
    ));
    let diagnostics = check_diagnostics(&source);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

// ts-bb95dc: `Channel.Receive<T>` is what `T` is, so a received exclusive value is
// exclusive while it is still wrapped. Rejects a `Receive<T>` that is shareable
// whatever `T` is (two children could capture one received receiver), and one that
// is never shareable (a received `Int` captured twice is fine).
#[test]
fn a_received_value_has_the_capability_class_of_its_payload() {
    let source = process_program(&format!(
        concat!(
            "{program}",
            "import std/channel.{{Receive}}\n",
            "fn captured_twice(rs: Receiver<Receiver<Int>>) -> Int {{\n",
            "  let got = Channel.receive(rs)\n",
            "  let a = Task.start(fn() -> Int {{\n    got\n    1\n  }})\n",
            "  let b = Task.start(fn() -> Int {{\n    got\n    2\n  }})\n  1\n}}\n",
            "fn ints_twice(rs: Receiver<Int>) -> Int {{\n",
            "  let got = Channel.receive(rs)\n",
            "  let a = Task.start(fn() -> Int {{\n    got\n    1\n  }})\n",
            "  let b = Task.start(fn() -> Int {{\n    got\n    2\n  }})\n  1\n}}\n",
        ),
        program = HAND_PROGRAM,
    ));
    let found = check_diagnostics(&source);
    let found = found
        .iter()
        .map(|(code, _, message)| (code.as_str(), message.contains("`got` was transferred")))
        .collect::<Vec<_>>();
    assert_eq!(found, [("T4241", true)], "{found:?}");
}

// Removing a managed supervisor consumes its handle, as removing a process does.
// Rejects a `remove_supervisor` that leaves the handle usable after removal or
// across loop passes.
#[test]
fn removing_a_supervisor_consumes_its_handle() {
    let source = process_program(concat!(
        "fn after(root: Supervisor.Running, child: Supervisor.ManagedSupervisor) -> Void {\n",
        "  Supervisor.remove_supervisor(root, child)\n",
        "  Supervisor.remove_supervisor(root, child)\n",
        "}\n",
        "fn each(root: Supervisor.Running, child: Supervisor.ManagedSupervisor) -> Int {\n",
        "  loop count = 0 {\n",
        "    Supervisor.remove_supervisor(root, child)\n",
        "    case count >= 3 {\n      True -> break count\n      False -> count + 1\n    }\n",
        "  }\n",
        "}\n",
    ));
    let diagnostics = check_diagnostics(&source);
    let found = diagnostics
        .iter()
        .map(|(code, text, message)| {
            (
                code.as_str(),
                text.as_str(),
                message.contains("Supervisor.remove_supervisor"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        found,
        [("T4241", "child", true), ("T4250", "child", true)],
        "{diagnostics:?}"
    );
}

// A `SendError`, `Restart`, or `StartError` value is a closed enum: a `case` that
// names every variant is exhaustive, as for `Channel.SendError`. Rejects a checker
// that knows no variant domain for these standard enums and asks for a `_` arm.
#[test]
fn process_standard_enum_values_match_exhaustively() {
    let program = process_program(concat!(
        "fn sent(error: Process.SendError<Message>) -> Int {\n",
        "  case error {\n",
        "    Process.SendError.Full(_) -> 1\n",
        "    Process.SendError.Unavailable(_) -> 2\n",
        "  }\n",
        "}\n",
        "fn restart(policy: Supervisor.Restart) -> Int {\n",
        "  case policy {\n",
        "    Supervisor.Restart.Never -> 1\n",
        "    Supervisor.Restart.OnDefect -> 2\n",
        "  }\n",
        "}\n",
        "fn started(error: Supervisor.StartError) -> Int {\n",
        "  case error {\n",
        "    Supervisor.StartError.CapacityReached -> 1\n",
        "    Supervisor.StartError.NameTaken(_) -> 2\n",
        "    Supervisor.StartError.Stopped -> 3\n",
        "  }\n",
        "}\n",
    ));
    assert_eq!(check_codes(&program), Vec::<String>::new());
    let missing = process_program(concat!(
        "fn sent(error: Process.SendError<Message>) -> Int {\n",
        "  case error {\n",
        "    Process.SendError.Full(_) -> 1\n",
        "  }\n",
        "}\n",
    ));
    assert_eq!(check_codes(&missing), ["T4231"]);
}

// Guards: every `let` of a block stays bound for the block's later steps, an inner
// block's `let` does not change the enclosing block's names, and a block that binds
// nothing still sees every enclosing local (CHI-91 clones a block's locals only at
// its first binding).
#[test]
fn an_inner_block_binding_stays_inside_the_block() {
    let outcome = run_both(
        "block-binding-scope",
        concat!(
            "pub fn main() -> Int {\n",
            "  let x = 1\n",
            "  let y = {\n",
            "    let x = 2\n",
            "    x\n",
            "  }\n",
            "  let z = { { x + y } }\n",
            "  x * 100 + y * 10 + z\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":123");
}

// Guards: Void, True and False are shared values that no pass boundary reclaims, so a
// comparison made in one pass is still a Bool in every later pass.
#[test]
fn booleans_stay_booleans_across_reclaimed_loop_passes() {
    let outcome = run_both(
        "shared-boolean-passes",
        concat!(
            "pub fn main() -> Int {\n",
            "  loop count = 0 {\n",
            "    let even = count % 2 == 0\n",
            "    case count >= 5000 {\n",
            "      True -> break count\n",
            "      False -> case even {\n",
            "        True -> count + 1\n",
            "        False -> count + 3\n",
            "      }\n",
            "    }\n",
            "  }\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":5000");
}

// Guards: a match reads every subject, both when its subjects fit the runtime's
// stack array (up to eight) and when they do not (nine).
#[test]
fn a_match_reads_every_one_of_eight_or_nine_subjects() {
    let outcome = run_both(
        "match-many-subjects",
        concat!(
            "fn eight(a: Int, b: Int, c: Int, d: Int, e: Int, f: Int, g: Int, h: Int) -> Int {\n",
            "  case a, b, c, d, e, f, g, h {\n",
            "    1, 2, 3, 4, 5, 6, 7, 8 -> 8\n",
            "    _, _, _, _, _, _, _, _ -> 0\n",
            "  }\n",
            "}\n",
            "fn nine(a: Int, b: Int, c: Int, d: Int, e: Int, f: Int, g: Int, h: Int, i: Int) -> Int {\n",
            "  case a, b, c, d, e, f, g, h, i {\n",
            "    1, 2, 3, 4, 5, 6, 7, 8, 9 -> 9\n",
            "    _, _, _, _, _, _, _, _, _ -> 0\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  eight(1, 2, 3, 4, 5, 6, 7, 8) * 1000 + eight(1, 2, 3, 4, 5, 6, 7, 0) * 100\n",
            "    + nine(1, 2, 3, 4, 5, 6, 7, 8, 9) * 10 + nine(1, 2, 3, 4, 5, 6, 7, 8, 0)\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":8090");
}
