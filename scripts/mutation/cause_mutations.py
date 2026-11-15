"""Mutants of failure causes (G25, CHI-204): the chain rule in lowering
(src/executable.rs), the reference interpreter's causes (src/execution/causes.rs),
and the native runtime's copy (runtime.c). The std-tests/cause programs run on
both backends, so a mutant on either side shows as a mismatch or a wrong
expected record; the cross-heap test covers native transfer.

Run: scripts/mutation/mutate.py scripts/mutation/cause_mutations.py
"""

CRATE = "compiler/chimi"

LOWERING = "src/executable.rs"
CAUSES = "src/execution/causes.rs"
RUNTIME = "runtime/native/runtime.c"


def programs(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["std_modules"],
                filter="standard_programs")


def heaps(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["language"],
                filter="a_cause_crosses_heaps")


MUTATIONS = [
    programs("re-raising the bound error links", LOWERING, [
        ("let keep = argument.is_some_and(|symbol| scope.carriers.contains(&symbol));",
         "let keep = false;")]),
    programs("every conversion keeps the matched cause", LOWERING, [
        ("let keep = argument.is_some_and(|symbol| scope.carriers.contains(&symbol));",
         "let keep = argument.is_some();")]),
    programs("an Err arm is not a cause source", LOWERING, [
        ("if fields.len() == 1 && self.is_result_err(*symbol) =>",
         "if false && fields.len() == 1 && self.is_result_err(*symbol) =>")]),
    programs("Rust: a plain failure reports its cause", CAUSES, [
        ("    !cause.frames.is_empty() || cause.caused_by.is_some() || !cause.suppressed.is_empty()",
         "    true")]),
    programs("Rust: frames drop the oldest", CAUSES, [
        ("        items.remove(limit / 2);", "        items.remove(0);")]),
    programs("Rust: chains are unbounded", CAUSES, [
        ("        if self.links > LINK_LIMIT {", "        if false {")]),
    programs("Rust: a context frame forgets its text", CAUSES, [
        ("            .map(|frame| RuntimeValue::Text(frame.text.clone()))",
         "            .map(|_| RuntimeValue::Text(String::new()))")]),
    programs("C: re-raising the bound error links", RUNTIME, [
        ("ChimiValue *cause = cause_derived(matched, node->c != 0, result->value, node_id);",
         "ChimiValue *cause = cause_derived(matched, false, result->value, node_id);")]),
    programs("C: a plain failure reports its cause", RUNTIME, [
        ("  return list_length(cause_field(cause, CAUSE_FRAMES)) > 0 || cause_below(cause) ||",
         "  return true ||")]),
    programs("C: frames drop the oldest", RUNTIME, [
        ("    if (index != limit / 2) kept[count++] = list_get(items, index);",
         "    if (index != 0) kept[count++] = list_get(items, index);")]),
    programs("C: the chain drop forgets its count", RUNTIME, [
        ("  int64_t dropped = cause_integer(link, CAUSE_DROPPED_LINKS) + 1;",
         "  int64_t dropped = cause_integer(link, CAUSE_DROPPED_LINKS);")]),
    programs("C: Cause.context adds no frame", RUNTIME, [
        ("    return control_with_cause(control, cause_with_frame(cause, arguments[1], call_node));",
         "    return control_with_cause(control, cause);")]),
    programs("C: a stored Err drops its cause", RUNTIME, [
        ("    if (cause) result = success(with_stored_cause(result->value, cause));",
         "    (void)cause;")]),
    heaps("C: Cause.of finds no stored cause", RUNTIME, [
        ("static ChimiValue *stored_cause(const ChimiValue *value) {\n  if (value->kind != VALUE_CONSTRUCTED) return NULL;",
         "static ChimiValue *stored_cause(const ChimiValue *value) {\n  if (value->kind != VALUE_CONSTRUCTED || value->as.constructed.length == 1) return NULL;")]),
]
