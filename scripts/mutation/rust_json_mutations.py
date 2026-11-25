"""Mutants of the Rust JSON decoder (CHI-307): runtime/chimi-runtime/src/json.rs
and its C side in runtime.c (json_build, json_decode).

Each mutant must turn red the decoder's unit tests or the runtime fuzz corpus
replay (tests/runtime_fuzz.rs), which checks every committed input against
its recorded output and against the C decoder the Rust one replaced.

Run: scripts/mutation/mutate.py scripts/mutation/rust_json_mutations.py
"""

CRATE = "compiler/chimi"

DECODER = "../../runtime/chimi-runtime/src/json.rs"
RUNTIME = "runtime/native/runtime.c"


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
    fuzz("keys built as Json.Text", RUNTIME,
         [("      case JSON_NODE_SOURCE_KEY: case JSON_NODE_DECODED_KEY:\n        value = text_from_bytes(text + node->offset, node->length); break;\n",
           "      case JSON_NODE_SOURCE_KEY: case JSON_NODE_DECODED_KEY: {\n        ChimiValue *fields[] = {text_from_bytes(text + node->offset, node->length)};\n        value = standard_value(CHIMI_CONSTRUCTOR_JSON_TEXT, fields, 1); break;\n      }\n")]),
    fuzz("decoded text read from the source", RUNTIME,
         [("const char *text = node->kind == JSON_NODE_DECODED_TEXT || node->kind == JSON_NODE_DECODED_KEY",
           "const char *text = node->kind == JSON_NODE_DECODED_KEY")]),
    fuzz("error message one byte short", RUNTIME,
         [("text_from_bytes(parse.error_message, parse.error_message_length)",
           "text_from_bytes(parse.error_message, parse.error_message_length - 1)")]),
    fuzz("array items taken from the wrong end", RUNTIME,
         [("list_from_values(stack + depth, (uint32_t)node->length)",
           "list_from_values(stack, (uint32_t)node->length)")]),
]
