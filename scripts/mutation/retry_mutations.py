"""Mutants of std/schedule and std/retry (CHI-206) and the contravariant
fitting they need in typing: the delay table and its overflow guard, the
retry loop's guard, count, and sleep, T4347, and the flipped failure-set
checks under function inputs and input-only type arguments.

Run: scripts/mutation/mutate.py scripts/mutation/retry_mutations.py
"""

CRATE = "compiler/chimi"

SCHEDULE = "std/schedule.chimi"
RETRY = "std/retry.chimi"
TYPING = "src/typing.rs"
SETS = "src/typing/failure_sets.rs"
CONSTANTS = "src/typing/durations.rs"


def programs(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["std_modules"],
                filter="standard_programs")


def retry(name, file, edits, filter=""):
    return dict(name=name, file=file, edits=edits, targets=["retry"], filter=filter)


def language(name, file, edits, filter):
    return dict(name=name, file=file, edits=edits, targets=["language"], filter=filter)


MUTATIONS = [
    programs("doubling past half the limit overflows", SCHEDULE, [
        ("        True -> (limit, left - 1)", "        True -> (Duration.add(wait, wait), left - 1)")]),
    programs("the last attempt still has a delay", SCHEDULE, [
        ("  case used < attempts(schedule) {", "  case used <= attempts(schedule) {")]),
    programs("a first wait above the limit is kept", SCHEDULE, [
        ("  loop state = (min(first, limit), attempt) {", "  loop state = (first, attempt) {")]),
    programs("a negative wait is accepted", SCHEDULE, [
        ("  case wait < Duration.zero {", "  case False {")]),
    programs("a zero first wait doubles forever", SCHEDULE, [
        ("    case left == 1 or wait == limit or wait == Duration.zero {", "    case left == 1 or wait == limit {")]),
    programs("an immediate schedule waits", SCHEDULE, [
        ("      Immediate(_) -> Duration.zero", "      Immediate(_) -> Duration.seconds(1)")]),
    retry("every failure is retried", RETRY, [
        ("      Failure(error) if policy.retryable(error) ->", "      Failure(error) ->")]),
    retry("attempts are counted twice", RETRY, [
        ("          attempt + 1\n", "          attempt + 2\n")]),
    retry("each delay is slept twice", RETRY, [
        ("          Clock.sleep(wait)\n", "          Clock.sleep(wait)\n          Clock.sleep(wait)\n")]),
    retry("an input set must fit the bound, not hold it", TYPING, [
        ("                                self.failure_row_is_subset(bound, actual)",
         "                                self.failure_row_is_subset(actual, bound)")]),
    retry("input-only type arguments are invariant in calls", TYPING, [
        ("        if self.type_parameter_is_contravariant(owner, index) {\n            self.function_input_depth += 1;",
         "        if false {\n            self.function_input_depth += 1;")]),
    retry("an inferred predicate input is not bound", TYPING, [
        ("                            let bound = self.failure_set_type(bound);\n                            return self.unify(actual, bound);",
         "                            return false;")]),
    retry("T4347 rejects one attempt", CONSTANTS, [
        ("        if attempts >= 1 {", "        if attempts > 1 {")]),
    retry("T4346 names the wrong side under an input", SETS, [
        ("        let (narrower, wider) = if flipped {\n            (expected, actual)",
         "        let (narrower, wider) = if flipped {\n            (actual, expected)")]),
    language("generic callback inputs are not flipped", TYPING, [
        ("                .all(|(actual, expected)| {\n                    self.function_input_depth += 1;\n                    let matched = self.match_call_type(actual, expected, substitution);\n                    self.function_input_depth -= 1;\n                    matched\n                })\n            && self.match_call_type(\n                actual_contract.success,",
         "                .all(|(actual, expected)| self.match_call_type(actual, expected, substitution))\n            && self.match_call_type(\n                actual_contract.success,")],
        "a_generic_function_does_not_take_a_callback"),
    language("input-only type arguments are invariant in inference", TYPING, [
        ("                    if self.type_parameter_is_contravariant(left_symbol, index) {",
         "                    if false {")],
        "a_type_argument_used_only_as_an_input"),
    language("a covariant use makes no difference", SETS, [
        ("            && fields\n                .iter()\n                .all(|field| !self.occurs_covariantly(field.type_id, parameter, false, depth))",
         "")],
        "a_type_argument_used_as_input_and_output"),
]
