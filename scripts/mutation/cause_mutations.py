"""Mutants of failure causes (G25, CHI-204): the chain rule in lowering
(src/executable.rs), the reference interpreter's causes (src/execution/causes.rs),
and the native runtime's copy (chimi-runtime intrinsics/causes.rs, with
Cause.context and the callers in runtime.c). The std-tests/cause programs run on
both backends, so a mutant on either side shows as a mismatch or a wrong
expected record; the cross-heap test covers native transfer.

Run: scripts/mutation/mutate.py scripts/mutation/cause_mutations.py
"""

CRATE = "compiler/chimi"

# The core node evaluators, `invoke` and the node frame are Rust since
# CHI-305 step 5, phase 7.
EVAL = "../../runtime/chimi-runtime/src/eval.rs"

LOWERING = "src/executable.rs"
CAUSES = "src/execution/causes.rs"
RUNTIME = "runtime/native/runtime.c"
OUTCOME = "../../runtime/chimi-runtime/src/outcome.rs"
SCOPES = "../../runtime/chimi-runtime/src/intrinsics/scopes.rs"
CORE_INTRINSICS = "../../runtime/chimi-runtime/src/intrinsics/core.rs"
RUST_CAUSES = "../../runtime/chimi-runtime/src/intrinsics/causes.rs"


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
    programs("a nested case hides the enclosing cause arm", LOWERING, [
        ("            if let Some((_, carriers)) = &source {\n                self.cause_scopes.push(Some(CauseScope {",
         "            self.cause_scopes.push(None);\n            if let Some((_, carriers)) = &source {\n                self.cause_scopes.pop();\n                self.cause_scopes.push(Some(CauseScope {"),
        ("            let scope = source\n                .is_some()\n                .then(|| self.cause_scopes.pop().flatten())\n                .flatten();",
         "            let scope = self.cause_scopes.pop().flatten();")]),
    programs("a function literal links to the enclosing arm", LOWERING, [
        ("                self.cause_scopes.push(None);\n                let lowered_body = self.expression(module_id, body);\n                self.cause_scopes.pop();",
         "                let lowered_body = self.expression(module_id, body);")]),
    programs("native: a crash message lives in the loop's heap", CORE_INTRINSICS, [
        ("defect_with(lasting_message(bytes_of(arguments[0])), node)",
         "defect_with((*new_text(bytes_of(arguments[0]))).payload.text.bytes, node)")]),
    programs("outcome: the root report stops at a NUL", OUTCOME, [
        ("                out.json(text_bytes(&*cause_text(cause)));",
         "                out.json_c((*cause_text(cause)).payload.text.bytes);")]),
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
    programs("native: re-raising the bound error links", EVAL, [
        ("            let cause = cause_derived(matched, node.c != 0, (*result).value, i64::from(node_id));",
         "            let cause = cause_derived(matched, false, (*result).value, i64::from(node_id));")]),
    programs("native: a plain failure reports its cause", RUST_CAUSES, [
        ("        (*field(cause, Slot::FRAMES)).payload.list.length > 0\n",
         "        true\n")]),
    programs("native: frames drop the oldest", RUST_CAUSES, [
        ("        for index in (0..length).filter(|&index| index != limit / 2) {",
         "        for index in (0..length).filter(|&index| index != 0) {")]),
    programs("native: the chain drop forgets its count", RUST_CAUSES, [
        ("        let dropped = integer(current, Slot::DROPPED_LINKS) + 1;",
         "        let dropped = integer(current, Slot::DROPPED_LINKS);")]),
    programs("native: chains are unbounded", RUST_CAUSES, [
        ("        if integer(cause, Slot::LINKS) <= LINK_LIMIT {",
         "        if true {")]),
    programs("native: a dropped-frames line is missing", RUST_CAUSES, [
        ("                    self.dropped(depth, dropped_frames, b\"context frame\");",
         "                    let _ = dropped_frames;")]),
    dict(name="native: a suppressed cause shows no reason", file=RUST_CAUSES, edits=[
        ("                self.text(b\"suppressed (\");",
         "                self.text(b\"suppressed \");")],
        targets=["task_combinators"], filter="a_root_task_all_failure_reports_its_sibling_causes"),
    programs("native: a frame's location is missing", RUST_CAUSES, [
        ("                self.location((*(*frame.add(1)).value).payload.integer);",
         "                let _ = frame;")]),
    programs("native: Cause.frames forgets the texts", RUST_CAUSES, [
        ("                    *texts.add(index as usize) = (*(*list::get(frames, index)).payload.record.fields).value;",
         "                    *texts.add(index as usize) = void_value();")]),
    # No mutant for Cause.summary of a defect or a cancellation: a program can
    # bind only an expected failure's cause (cause arms, Cause.of, Task.all's
    # siblings), so that branch is unreachable from Chimi code.
    programs("native: Cause.context adds no frame", SCOPES, [
        ("        control_with_cause(control, cause_with_frame(cause, arguments[1], i64::from(call.call_node)))",
         "        control_with_cause(control, cause)")]),
    programs("native: a stored Err drops its cause", EVAL, [
        ("                return success(with_stored_cause((*result).value, cause));",
         "                return result;")]),
    heaps("native: Cause.of finds no stored cause", RUST_CAUSES, [
        ("        if (*value).kind != ValueKind::CONSTRUCTED {\n            return ptr::null_mut();",
         "        if (*value).kind != ValueKind::CONSTRUCTED || (*value).payload.constructed.length == 1 {\n            return ptr::null_mut();")]),
]
