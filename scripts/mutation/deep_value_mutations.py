"""Mutants of the deep-value walks (ts-0e026f, ts-9b1ec1) and shared values
(ts-a512d8): the interpreter's release queue, outcome writer, part sharing, and
constant measuring cache, and the native runtime's outcome writer, debug renderer,
`Json.encode`, Task capture transfer, and generational marking.

Run: scripts/mutation/mutate.py scripts/mutation/deep_value_mutations.py
Each mutant runs the tests named in its filter; it must turn one red. A walk that
recurses again overflows a small test stack, which aborts the test binary.
"""

CRATE = "compiler/chimi"

EXECUTION = "src/execution.rs"
TEARDOWN = "src/execution/teardown.rs"
RUNTIME = "runtime/native/runtime.c"
JSON_INTRINSICS = "../../runtime/chimi-runtime/src/intrinsics/json.rs"
TRANSFER = "../../runtime/chimi-runtime/src/heap/transfer.rs"


def lib(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["lib"], filter="execution::tests")


def language(name, edits, filter):
    return dict(name=name, file=RUNTIME, edits=edits, targets=["language"], filter=filter)


def json_language(name, edits, filter):
    return dict(name=name, file=JSON_INTRINSICS, edits=edits, targets=["language"], filter=filter)


def native(name, edits, filter):
    return dict(name=name, file=RUNTIME, edits=edits, targets=["cli"], filter=filter)


def transfer(name, edits, filter):
    return dict(name=name, file=TRANSFER, edits=edits, targets=["cli"], filter=filter)


# Copies a value's records, tuples, and constructed values on every reference, as
# the interpreter did before they were shared.
UNSHARED = """fn unshared_fields(fields: &[RuntimeField]) -> Arc<Vec<RuntimeField>> {
    Arc::new(fields.iter().map(|field| RuntimeField {
        name: field.name.clone(),
        value: unshared(&field.value),
    }).collect())
}

fn unshared(value: &RuntimeValue) -> RuntimeValue {
    match value {
        RuntimeValue::Record(fields) => RuntimeValue::Record(unshared_fields(fields)),
        RuntimeValue::Tuple(values) => {
            RuntimeValue::Tuple(Arc::new(values.iter().map(unshared).collect()))
        }
        RuntimeValue::Constructed { constructor, type_name, variant_name, fields } => {
            RuntimeValue::Constructed {
                constructor: *constructor,
                type_name: type_name.clone(),
                variant_name: variant_name.clone(),
                fields: unshared_fields(fields),
            }
        }
        other => other.clone(),
    }
}

fn public_value(value: &RuntimeValue) -> ExecutionValue {"""


MUTATIONS = [
    lib("release frees nested parts in place", TEARDOWN, [(
        "        if queue.draining.get() {",
        "        if false {",
    )]),
    lib("maps are released the derived way", EXECUTION, [(
        "                | Self::Map(_)\n                | Self::Callable(_)",
        "                | Self::Callable(_)",
    )]),
    lib("closures are released the derived way", EXECUTION, [(
        "                | Self::Map(_)\n                | Self::Callable(_)",
        "                | Self::Map(_)",
    )]),
    lib("outcome records are freed the derived way", EXECUTION, [(
        "            Self::List(_) | Self::Tuple(_) | Self::Record(_) | Self::Constructed { .. }",
        "            Self::List(_) | Self::Tuple(_) | Self::Constructed { .. }",
    )]),
    lib("outcome writer writes a field's value before its name", EXECUTION, [(
        "        steps.push(JsonStep::Value(&field.value));\n        steps.push(JsonStep::Name(&field.name));",
        "        steps.push(JsonStep::Name(&field.name));\n        steps.push(JsonStep::Value(&field.value));",
    )]),
    language("native outcome writer writes a field's value before its name", [(
        "    json_step(steps, JSON_STEP_VALUE, fields[index].value);\n    if (named) json_step(steps, JSON_STEP_NAME, fields[index].name);",
        "    if (named) json_step(steps, JSON_STEP_NAME, fields[index].name);\n    json_step(steps, JSON_STEP_VALUE, fields[index].value);",
    )], "deeply_nested"),
    language("native debug renderer queues list parts first to last", [(
        "      for (uint32_t i = length; i-- > 0;) debug_part(steps, i, false, NULL, list_get(value, i));",
        "      for (uint32_t i = 0; i < length; ++i) debug_part(steps, i, false, NULL, list_get(value, i));",
    )], "render"),
    language("native debug renderer closes before restoring the depth", [(
        "        --renderer->depth;\n        debug_close(renderer, step.pointer, step.spaced);",
        "        debug_close(renderer, step.pointer, step.spaced);\n        --renderer->depth;",
    )], "render"),
    language("native debug renderer keeps the one-line format after a key", [(
        "        renderer->indented = step.spaced;\n        debug_text(renderer->buffer, \": \");",
        "        debug_text(renderer->buffer, \": \");",
    )], "debug_render_nests_through_map_keys"),
    language("native debug renderer renders a key by recursing", [(
        "      case DEBUG_STEP_KEY:\n"
        "        debug_step(&steps, DEBUG_STEP_KEY_END, NULL, 0, renderer->indented);\n"
        "        debug_step(&steps, DEBUG_STEP_VALUE, step.pointer, 0, false);\n"
        "        renderer->indented = false;\n"
        "        break;",
        "      case DEBUG_STEP_KEY: {\n"
        "        DebugRenderer key = {renderer->buffer, false, renderer->depth};\n"
        "        debug_value(&key, step.pointer);\n"
        "        debug_text(renderer->buffer, \": \");\n"
        "        break;\n"
        "      }",
    )], "debug_render_nests_through_map_keys"),
    language("native outcome writer does not quote a non-Text key", [(
        "        json_string(output, key->name); fputc(':', output);",
        "        fputs(key->name, output); fputc(':', output);",
    )], "debug_render_nests_through_map_keys"),
    json_language('native Json.encode queues Array elements first to last', [('for index in (0..(*items).payload.list.length).rev() {',
        'for index in 0..(*items).payload.list.length {')], 'json_encode_writes_a_deeply_nested_value'),
    json_language("native Json.encode writes an Object entry's value before its key", [('steps.push(Step::Value((*node).value));\n                    steps.push(Step::Key(node));',
        'steps.push(Step::Key(node));\n                    steps.push(Step::Value((*node).value));')], 'json_encode_writes_a_deeply_nested_value'),
    # The walk is Rust since CHI-305 step 5, phase 3 (heap/transfer.rs).
    transfer("native capture transfer scans the visited set linearly", [(
        "    let hash = (pointer as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);\n"
        "    (hash >> (64 - capacity.trailing_zeros())) as usize",
        "    let _ = (pointer, capacity);\n"
        "    0",
    )], "native_capture_transfer_work_is_linear"),
    transfer("native capture transfer skips list elements", [(
        "                    pending.push(Part::Value, list::get(value, index));",
        "                    let _ = list::get(value, index);",
    )], "native_tasks_capture_render_encode_and_send"),
    transfer("native capture transfer skips closure environments", [(
        "                pending.push(Part::Environment, (*callable).environment);",
        "                let _ = (*callable).environment;",
    )], "native_"),
    transfer("native capture transfer gives up after a fixed number of parts", [(
        "        let Some((part, pointer)) = pending.0.pop() else { break };",
        "        let Some((part, pointer)) = pending.0.pop() else { break };\n"
        "        if visited.count > 4096 { transferred = false; break; }",
    )], "native_tasks_capture_render_encode_and_send"),
    native("native capture transfer counts no probes", [(
        "  carrier->capture_transfer_probes += probes;",
        "  (void)probes;",
    )], "native_capture_transfer_work_is_linear"),
    native("native capture transfer leaves a captured Receiver with the parent", [(
        "      return transfer_captured_capability(transfer, value, CHIMI_REQUEST_CHANNEL_TRANSFER_RECEIVER);",
        "      return true;",
    )], "native_"),
    dict(
        name="interpreter copies records, tuples, and constructed values on reference",
        file=EXECUTION,
        edits=[
            ("fn public_value(value: &RuntimeValue) -> ExecutionValue {", UNSHARED),
            (
                "                .get(&symbol)\n                .cloned()\n                .map(Control::Success)\n"
                "                .unwrap_or_else(|| Control::Defect {\n"
                "                    message: format!(\"verified local {} is not bound\", core_symbol.name),",
                "                .get(&symbol)\n                .map(unshared)\n                .map(Control::Success)\n"
                "                .unwrap_or_else(|| Control::Defect {\n"
                "                    message: format!(\"verified local {} is not bound\", core_symbol.name),",
            ),
        ],
        targets=["interpreter_allocations"],
        filter="",
    ),
    dict(
        name="constant measuring walks shared fields again",
        file="src/execution/constants.rs",
        edits=[(
            "        let address = Arc::as_ptr(fields) as usize;\n"
            "        if let Some(size) = self.shared.get(address) {\n"
            "            return Some(size);\n"
            "        }",
            "        let address = Arc::as_ptr(fields) as usize;",
        )],
        targets=["lib"],
        filter="measuring_a_growing_constant",
    ),
    dict(
        name="native marking rescans the whole state on every pass",
        # The collector is Rust since CHI-305 step 5 phase 2.
        file="../../runtime/chimi-runtime/src/heap/collector.rs",
        edits=[(
            "        let full = (*scope).has_baseline\n"
            "            && survivors >= baseline.wrapping_mul(2)\n"
            "            && survivors.wrapping_sub(baseline) >= REGION_FULL_MIN_GROWTH;\n",
            "        let full = true;\n        let _ = (survivors, baseline);\n",
        )],
        targets=["cli"],
        filter="native_marking_work_is_linear",
    ),
]
