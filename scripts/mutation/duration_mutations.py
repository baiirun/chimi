"""Mutants of `std/duration` (ts-f31a20): the Rust model, the native runtime's copy, the
derived order on both backends, and the build-time checks.

Run: scripts/mutation/mutate.py scripts/mutation/duration_mutations.py
Each mutant runs the Duration unit tests and the `duration` / constant-check language
tests; it must turn one red.
"""

CRATE = "compiler/chimi"

D = "src/duration.rs"
C = "runtime/native/runtime.c"
S = "src/execution/structural.rs"
T = "src/typing/durations.rs"
Y = "src/typing.rs"


def m(name, file, edits, target="language", filter="duration"):
    return dict(name=name, file=file, edits=edits, targets=[target], filter=filter)


def both(name, file, edits):
    """A model mutant: the Rust unit tests and the language differential both see it."""
    return dict(name=name, file=file, edits=edits, targets=["lib", "language"], filter="duration")


MUTATIONS = [
    # Rust model: text form.
    both("Rust: one second renders in ms", D, [(
        "if magnitude < unit(Unit::Seconds) {",
        "if magnitude <= unit(Unit::Seconds) {")]),
    both("Rust: keep trailing fraction zeros", D, [(
        "text.push_str(digits.trim_end_matches('0'));",
        "text.push_str(&digits);")]),
    both("Rust: omit a zero unit between nonzero units", D, [(
        "    for (count, part_unit) in &parts[first..=last] {\n",
        "    for (count, part_unit) in parts[first..=last].iter().filter(|(count, _)| *count != 0) {\n")]),
    # Rust model: parser.
    both("Rust: allow a repeated unit", D, [(
        ".is_some_and(|previous| previous.unit.nanoseconds() <= unit.nanoseconds())",
        ".is_some_and(|previous| previous.unit.nanoseconds() < unit.nanoseconds())")]),
    both("Rust: round an inexact fraction", D, [(
        "        if numerator % denominator != 0 {\n            return Err(ParseFailure::Invalid);\n        }\n",
        "")]),
    both("Rust: range-check the magnitude before the sign", D, [(
        """    let value = if parsed.negative {
        -magnitude
    } else {
        magnitude
    };
    i64::try_from(value).map_err(|_| ParseFailure::OutOfRange)""",
        """    let magnitude = i64::try_from(magnitude).map_err(|_| ParseFailure::OutOfRange)?;
    Ok(if parsed.negative { -magnitude } else { magnitude })""")]),
    # Rust model: overflow.
    both("Rust: constructors wrap", D, [(
        "count.checked_mul(unit.nanoseconds()).ok_or_else(|| {",
        "Some(count.wrapping_mul(unit.nanoseconds())).ok_or_else(|| {")]),
    both("Rust: negate wraps", D, [(
        "        .checked_neg()\n",
        "        .checked_neg()\n        .or(Some(duration))\n")]),
    both("Rust: add saturates", D, [(
        "left.checked_add(right)",
        "Some(left.saturating_add(right))")]),
    # Interpreter derived order.
    m("interpreter: Duration order reversed", S, [(
        "(RuntimeValue::Duration(left), RuntimeValue::Duration(right)) => left.cmp(right),",
        "(RuntimeValue::Duration(left), RuntimeValue::Duration(right)) => right.cmp(left),")]),
    # Native runtime copy.
    m("C: one second renders in ms", C, [(
        "if (magnitude < (uint64_t)DURATION_UNITS[2]) {",
        "if (magnitude <= (uint64_t)DURATION_UNITS[2]) {")]),
    m("C: keep trailing fraction zeros", C, [(
        "size_t length = strlen(digits); while (length > 0 && digits[length - 1] == '0') --length;",
        "size_t length = strlen(digits);")]),
    m("C: allow a repeated unit", C, [(
        "if (unit < 0 || unit <= previous) return CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID;",
        "if (unit < 0 || unit < previous) return CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID;")]),
    m("C: round an inexact fraction", C, [(
        "if (numerator % denominator != 0) return CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID;",
        "")]),
    m("C: no range check on the total", C, [(
        "if (total > INT64_MAX || total < INT64_MIN) return CHIMI_CONSTRUCTOR_DURATION_PARSE_OUT_OF_RANGE;",
        "")]),
    m("C: constructors wrap", C, [(
        "if (__builtin_mul_overflow(count, DURATION_UNITS[unit], &result))",
        "result = count * DURATION_UNITS[unit]; if (0)")]),
    m("C: negate and absolute of the minimum wrap", C, [(
        "      if (value == INT64_MIN) {\n        snprintf(operation, sizeof(operation), negate ? \"-(%s)\" : \"|%s|\", left);",
        "      if (0) {\n        snprintf(operation, sizeof(operation), negate ? \"-(%s)\" : \"|%s|\", left);")]),
    m("C: Duration order reversed", C, [(
        "      case VALUE_INTEGER:\n      case VALUE_DURATION: order = order_of(left->as.integer, right->as.integer); break;",
        "      case VALUE_INTEGER: order = order_of(left->as.integer, right->as.integer); break;\n"
        "      case VALUE_DURATION: order = order_of(right->as.integer, left->as.integer); break;")]),
    # Build-time checks.
    m("T4349/T4350 for a computed argument", T, [(
        "        if !literal {\n            return;\n        }\n", "")], filter="constant_duration"),
    m("no T4350 warning", T, [(
        "                self.warnings.push(Diagnostic::warning(\n                    DiagnosticStage::Type,\n                    \"T4350\",",
        "                std::mem::take(&mut self.warnings).push(Diagnostic::warning(\n                    DiagnosticStage::Type,\n                    \"T4350\",")],
      filter="constant_duration"),
    m("no T4348 for a constructor", T, [(
        "            if let Err(Unfolded::Defect(message)) = self.evaluator.fold(module_id, call) {",
        "            if let Err(Unfolded::Defect(message)) = Err::<Folded, _>(Unfolded::NotConstant) {")],
      filter="constant_duration"),
    m("constants do not fold through references", T, [(
        "                self.symbol_value(symbol)\n",
        "                let _ = symbol;\n                Err(Unfolded::NotConstant)\n")],
      target="lib", filter="durations"),
    m("T4303 help without the Duration function", Y, [(
        "            } else if is_duration {\n                duration_operator_help(operator)\n",
        "            } else if is_duration {\n                String::new()\n")],
      filter="arithmetic_operators_reject_duration"),
]
