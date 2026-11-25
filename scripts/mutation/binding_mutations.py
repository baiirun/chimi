"""Mutants of C bindings (G20, CHI-254): the handle table, value builders and
results of the binding API, the descriptor wait, dispatch through the callee's
binding, pay-for-play linking, and the interpreter's refusal. The handle
table is in Rust since CHI-309 part 2 (runtime/chimi-runtime/src/handles.rs),
whose own unit-test mutants are in handle_slots_mutations.py. "Closing twice
releases twice" was dropped there: with the generation moved on, a second close
cannot find the slot, so "close does not bump the generation" covers it.

Run: scripts/mutation/mutate.py scripts/mutation/binding_mutations.py
Each mutant runs tests/bindings.rs, the end-to-end conformance.
"""

CRATE = "compiler/chimi"

C = "runtime/native/runtime.c"
PROBE = "runtime/native/bindings/probe/probe.c"
DATA = "src/backend/program_data.rs"
CORE = "src/executable.rs"
NATIVE = "src/backend/native.rs"
EXEC = "src/execution.rs"
OPS = "src/operations.rs"
HANDLES = "../../runtime/chimi-runtime/src/handles.rs"
HANDLES_ABI = "../../runtime/chimi-runtime/src/handles_abi.rs"


def mutant(name, file, edits, targets=("bindings",), filter=""):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("close does not bump the generation", HANDLES, [(
        "        live.generation = generation.saturating_add(1);",
        "        live.generation = generation;",
    )]),
    mutant("a handle ignores its generation", HANDLES, [(
        "            Some(Slot::Open(live)) if live.generation == generation => Some(live),",
        "            Some(Slot::Open(live)) => Some(live),",
    )]),
    mutant("a handle of another type is accepted", HANDLES_ABI, [(
        "        Some((open, state)) if kind.is_null() || open == Opaque(kind) => state.0.cast_mut(),",
        "        Some((_, state)) => state.0.cast_mut(),",
    )]),
    mutant("Err loses its message", C, [(
        "    chimi_binding_make_text(call, message, message ? strlen(message) : 0),\n  };",
        "    chimi_binding_make_text(call, \"\", 0),\n  };",
    )]),
    mutant("ok returns the bare value", C, [(
        "  ChimiValue *fields[] = {runtime_value(value)};\n  return binding_result(success(standard_value(CHIMI_CONSTRUCTOR_RESULT_OK, fields, 1)));",
        "  return binding_result(success(runtime_value(value)));",
    )]),
    mutant("a defect keeps no message", C, [(
        "  return binding_result(defect(copy_text(message), call->call_node));",
        "  return binding_result(defect(\"binding defect\", call->call_node));",
    )]),
    mutant("bytes lose their last byte", C, [(
        "  return binding_value(bytes_from((const char *)bytes, length));",
        "  return binding_value(bytes_from((const char *)bytes, length ? length - 1 : 0));",
    )]),
    mutant("text stops at U+0000", C, [(
        "    if (!valid_utf8((const unsigned char *)bytes + start, end - start)) return NULL;\n    start = end + 1;",
        "    if (!valid_utf8((const unsigned char *)bytes + start, end - start)) return NULL;\n    if (nul) return NULL;\n    start = end + 1;",
    )]),
    mutant("a wait never parks", C, [(
        "      if (request.status != CHIMI_RESPONSE_PENDING) return CHIMI_BINDING_READY;\n    }\n  }\n  struct pollfd poll_fd = {",
        "      return CHIMI_BINDING_READY;\n    }\n  }\n  struct pollfd poll_fd = {",
    )]),
    mutant("a wait ignores a close while parked", C, [(
        "         its descriptor number may already belong to a new resource. */\n      if (binding_wait_handle_closed(call, handle)) return CHIMI_BINDING_WAIT_CLOSED;",
        "         its descriptor number may already belong to a new resource. */",
    )]),
    mutant("a cancelled wait reports ready", C, [(
        "      if (scheduler_request(&request) == CHIMI_FIBER_CANCEL) return CHIMI_BINDING_WAIT_CANCELLED;",
        "      if (scheduler_request(&request) == CHIMI_FIBER_CANCEL) return CHIMI_BINDING_READY;",
    )]),
    mutant("dispatch calls the first binding", C, [(
        "  ChimiControl *control = (ChimiControl *)CHIMI_BINDINGS[binding].function(&call);",
        "  ChimiControl *control = (ChimiControl *)CHIMI_BINDINGS[0].function(&call);",
    )]),
    mutant("a symbol's binding index is off by one", DATA, [(
        "                .and_then(|binding| bindings.iter().position(|reached| *reached == binding))\n                .map_or(u32::MAX, |index| index as u32),",
        "                .and_then(|binding| bindings.iter().position(|reached| *reached == binding))\n                .map_or(u32::MAX, |index| (index as u32).saturating_sub(1)),",
    )]),
    mutant("every package links whether reached or not", NATIVE, [(
        "    for package in packages_for(&reached_bindings) {",
        "    let reached_bindings = crate::operations::OPERATIONS.iter().filter_map(|operation| operation.binding).collect::<Vec<_>>();\n    for package in packages_for(&reached_bindings) {",
    )]),
    mutant("the binding archive key ignores the sources", "src/backend/runtime_archive_key.rs", [(
        "    for (name, text) in inputs.sources {\n        part(name.as_bytes());\n        part(text.as_bytes());\n    }",
        "    let _ = inputs.sources;",
    )], targets=("lib",), filter="backend::bindings"),
    mutant("the interpreter calls a binding a success", EXEC, [(
        "            StandardFunction::BindingCall => Control::Defect {\n                message: \"E5204: the interpreter cannot call C bindings; build the program with `chimi build`\".to_owned(),",
        "            StandardFunction::BindingCall => Control::Defect {\n                message: \"binding\".to_owned(),",
    )]),
    mutant("a test-only binding is allowed from main", OPS, [(
        "    if !operation.test_only {\n        return None;\n    }",
        "    if !operation.test_only || operation.binding.is_some() {\n        return None;\n    }",
    )]),
    mutant("the probe's read ignores EAGAIN", PROBE, [(
        "    if (errno != EAGAIN && errno != EWOULDBLOCK)\n      return chimi_binding_failure(call, PROBE_CLOSED, strerror(errno));",
        "    return chimi_binding_failure(call, PROBE_CLOSED, strerror(errno));",
    )]),
]
