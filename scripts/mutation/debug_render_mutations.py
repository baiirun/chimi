"""Mutants of the native debug renderer in Rust (CHI-305 step 5, phase 5,
sixth PR; runtime/chimi-runtime/src/intrinsics/debug.rs). The language suite
renders values of every kind on both backends and compares the text. Mutants
of the C it replaced were retargeted in bytes, deep_value, lane_fixes,
mailbox, std2_decimal and test_log.

Run: scripts/mutation/mutate.py scripts/mutation/debug_render_mutations.py
Results: scripts/mutation/debug_render_results.txt.
"""

CRATE = "compiler/chimi"
DEBUG_RENDER = "../../runtime/chimi-runtime/src/intrinsics/debug.rs"


def language(name, old, new):
    return dict(name=name, file=DEBUG_RENDER, edits=[(old, new)], targets=["language"], filter="")


def clock(name, old, new):
    return dict(name=name, file=DEBUG_RENDER, edits=[(old, new)], targets=["clock"],
                filter="debug_render_shows_an_instant_as_its_offset")


MUTATIONS = [
    language("a quote inside Text is not escaped", "b'\"' => self.text(b\"\\\\\\\"\"),", "b'\"' => self.text(b\"\\\"\"),"),
    language("a tab is written raw", "b'\\t' => self.text(b\"\\\\t\"),", "b'\\t' => self.text(b\"\\t\"),"),
    language("every dollar sign is escaped", "b'$' if text.get(at + 1) == Some(&b'{') =>", "b'$' =>"),
    language("negative zero keeps its sign", "if raw == 0.0 { 0.0 } else { raw }", "raw"),
    language("record fields keep their declared order",
             "while at > 0 && name(*order.add(at - 1)) > name(index as u32) {",
             "while at > 0 && false {"),
    language("Option variants are qualified",
             "standard == StandardConstructor::OPTION_SOME || standard == StandardConstructor::OPTION_NONE;",
             "false;"),
    language("a single-field variant labels its field",
             "self.fields((b\"(\", b\")\"), false, constructed.fields, None, length, length > 1);",
             "self.fields((b\"(\", b\")\"), false, constructed.fields, None, length, true);"),
    language("a set renders like a list", "let open: &[u8] = if with_values { b\"Map {\" } else { b\"Set {\" };",
             "let open: &[u8] = if with_values { b\"Map {\" } else { b\"[\" };"),
    language("a declared function loses its name", "CallableKind::DECLARED => symbol_name(callable.symbol),",
             "CallableKind::DECLARED => core::ptr::null(),"),
    language("indentation is one space a level", "self.text(b\"  \");", "self.text(b\" \");"),
    language("an indented part ends without a comma", "renderer.text(b\",\");", "renderer.text(b\"\");"),
    clock("an Instant renders as its Duration", "self.text(b\"Instant(\");", "self.text(b\"(\");"),
    language("a range drops its end", "self.text(b\"..\");\n                    self.integer(payload.range.end);",
             "self.text(b\"..\");"),
]
