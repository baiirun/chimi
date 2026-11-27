"""Mutants of the Rust JSON decoder (CHI-307): runtime/chimi-runtime/src/json.rs
and the builder of values from its tape, intrinsics/json.rs (C until CHI-305
step 5, phase 5).

Each mutant must turn red the decoder's unit tests or the runtime fuzz corpus
replay (tests/runtime_fuzz.rs), which checks every committed input against
its recorded output and against the C decoder the Rust one replaced.

Run: scripts/mutation/mutate.py scripts/mutation/rust_json_mutations.py
"""

CRATE = "compiler/chimi"

DECODER = "../../runtime/chimi-runtime/src/json.rs"
RUNTIME = "runtime/native/runtime.c"
JSON_INTRINSICS = "../../runtime/chimi-runtime/src/intrinsics/json.rs"


def fuzz(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["runtime_fuzz"], filter="")


MUTATIONS = [
    fuzz("accept one level deeper", DECODER,
         [("if frames.len() > MAX_DEPTH {", "if frames.len() > MAX_DEPTH + 1 {")]),
    fuzz("carriage return is not whitespace", DECODER,
         [("while let Some(b' ' | b'\\t' | b'\\r' | b'\\n') = self.peek() {",
           "while let Some(b' ' | b'\\t' | b'\\n') = self.peek() {")]),
    fuzz("allow duplicate keys", DECODER,
         [("if !keys.insert(bytes) {", "if !keys.insert(bytes) && false {")]),
    fuzz("allow leading zeroes", DECODER,
         [("return fail(self.offset, \"JSON numbers cannot contain leading zeroes\");",
           "{}")]),
    fuzz("accept trailing input", DECODER,
         [("if self.offset != self.source.len() {", "if false {")]),
    fuzz("control characters pass as plain bytes", DECODER,
         [(".position(|&byte| byte == b'\"' || byte == b'\\\\' || byte < 0x20)",
           ".position(|&byte| byte == b'\"' || byte == b'\\\\' || byte < 0x1f)")]),
    fuzz("isolated low surrogate accepted", DECODER,
         [("return fail(self.offset, \"isolated low surrogate in JSON string\");", "return Ok(0xfffd);")]),
    fuzz("unpaired high surrogate reported after the backslash", DECODER,
         [("return fail(self.offset + 1, UNPAIRED_SURROGATE);",
           "return fail(self.offset, UNPAIRED_SURROGATE);")]),
    fuzz("invalid hex reported past the digits", DECODER,
         [("return fail(start, \"invalid JSON Unicode escape\");",
           "return fail(start + 4, \"invalid JSON Unicode escape\");")]),
    fuzz("escaped solidus dropped", DECODER,
         [("b'\"' | b'\\\\' | b'/' => escaped,", "b'\"' | b'\\\\' => escaped,\n            b'/' => b'\\\\',")]),
    fuzz("plain run after an escape loses its first byte", DECODER,
         [(".extend_from_slice(&self.source[self.offset - 1..end]);",
           ".extend_from_slice(&self.source[self.offset..end]);")]),
    fuzz("exponent sign not consumed", DECODER,
         [("if let Some(b'+' | b'-') = self.peek() {\n                self.offset += 1;\n            }", "")]),
    fuzz('keys built as Json.Text', JSON_INTRINSICS, [('NodeKind::SourceKey | NodeKind::DecodedKey => {\n                    let text = located();\n                    text_value(text.as_ptr().cast(), text.len())\n',
        'NodeKind::SourceKey | NodeKind::DecodedKey => {\n                    let text = located();\n                    standard_value(StandardConstructor::JSON_TEXT, &[text_value(text.as_ptr().cast(), text.len())])\n')]),
    fuzz('decoded text read from the source', JSON_INTRINSICS, [('NodeKind::DecodedText | NodeKind::DecodedKey => &tape.decoded[..],',
        'NodeKind::DecodedKey => &tape.decoded[..],')]),
    fuzz('error message one byte short', JSON_INTRINSICS, [('error.message.as_ptr().cast::<c_char>(), error.message.len())',
        'error.message.as_ptr().cast::<c_char>(), error.message.len() - 1)')]),
    fuzz('array items taken from the wrong end', JSON_INTRINSICS, [('list::from_values(stack.as_ptr().add(start), length)',
        'list::from_values(stack.as_ptr(), length)')]),
]
