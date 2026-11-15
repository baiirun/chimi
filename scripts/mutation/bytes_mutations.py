"""Mutants of std/bytes (Phase 6a, CHI-210): the interpreter core in
src/execution/bytes_operations.rs, its C twin in runtime/native/runtime.c, and
the Chimi wrappers in std/bytes.chimi. Rust and C sides are mutated separately
because std_modules compares both backends against the same expected files.

Run: scripts/mutation/mutate.py scripts/mutation/bytes_mutations.py
"""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"
CORE = "src/execution/bytes_operations.rs"
WRAPPER = "std/bytes.chimi"
ORDER = "src/execution/structural.rs"


def programs(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["std_modules"],
                filter="standard_programs")


MUTATIONS = [
    # Construction and queries.
    programs("Rust from accepts 256", CORE, [("u8::try_from(*value)", "u8::try_from(*value % 256)")]),
    programs("C from accepts 256", RUNTIME, [("element->as.integer > 255", "element->as.integer > 256")]),
    programs("C get admits index == length", RUNTIME, [
        ("(uint64_t)index >= arguments[0]->as.text.length) return optional_value(NULL);",
         "(uint64_t)index > arguments[0]->as.text.length) return optional_value(NULL);")]),
    programs("C get reads a signed char", RUNTIME, [
        ("integer_value((unsigned char)arguments[0]->as.text.bytes[index])",
         "integer_value((signed char)arguments[0]->as.text.bytes[index])")]),
    programs("Rust index_of allows an empty needle", CORE, [("if needle.is_empty() {", "if false {")]),
    programs("C slice skips clamping", RUNTIME, [
        ("clamp_range(arguments[1]->as.range.start, arguments[1]->as.range.end, arguments[0]->as.text.length, &from, &to);",
         "from = (size_t)arguments[1]->as.range.start; to = (size_t)arguments[1]->as.range.end; if (to > arguments[0]->as.text.length) to = arguments[0]->as.text.length;")]),
    programs("ends_with drops the length guard", WRAPPER, [
        ("length(suffix) <= end and slice", "slice")]),
    programs("split_once keeps the separator", WRAPPER, [
        ("slice(bytes, at + length(separator)..length(bytes))", "slice(bytes, at..length(bytes))")]),
    # Order.
    programs("Rust Bytes order reversed", ORDER, [
        ("(RuntimeValue::Bytes(left), RuntimeValue::Bytes(right)) => left.cmp(right),",
         "(RuntimeValue::Bytes(left), RuntimeValue::Bytes(right)) => right.cmp(left),")]),
    # UTF-8.
    programs("C accepts surrogates", RUNTIME, [("if (first == 0xed) high = 0x9f;", "")]),
    programs("C accepts overlong three-byte forms", RUNTIME, [("if (first == 0xe0) low = 0xa0;", "")]),
    programs("C accepts past U+10FFFF", RUNTIME, [("if (first == 0xf4) high = 0x8f;", "")]),
    programs("C replaces a whole truncated sequence per byte", RUNTIME, [
        ("{ *complete = false; return taken; }", "{ *complete = false; return 1; }")]),
    programs("to_text reports the end, not the first invalid byte", WRAPPER, [
        ("Failure(DecodeError.InvalidUtf8(valid))", "Failure(DecodeError.InvalidUtf8(length(bytes)))")]),
    # Integers.
    programs("Rust layout swaps byte order for 16 and 32 bits", CORE, [
        ("within % 2 == 0,", "within % 2 == 1,")]),
    programs("C reads without sign extension", RUNTIME, [
        ("if (is_signed && width < 8 &&", "if (false && is_signed && width < 8 &&")]),
    programs("Rust read without sign extension", CORE, [
        ("        if self.signed {\n            // Sign-extend", "        if false {\n            // Sign-extend")]),
    programs("C read admits a field one past the end", RUNTIME, [
        ("length - (size_t)at < width) return optional_value(NULL);", "length - (size_t)at + 1 < width) return optional_value(NULL);")]),
    programs("Rust unsigned write admits 2^bits", CORE, [
        ("(false, _) => (0..(1i64 << bits)).contains(&value),", "(false, _) => (0..=(1i64 << bits)).contains(&value),")]),
    programs("C signed write admits 2^(bits-1)", RUNTIME, [
        ("((int64_t)1 << (bits - 1)) - 1 :", "((int64_t)1 << (bits - 1)) :")]),
    programs("C little-endian write is big-endian", RUNTIME, [
        ("raw[big_endian ? i : width - 1 - i] = byte;", "raw[i] = byte;")]),
    programs("layout codes swap I64Be and I64Le", WRAPPER, [
        ("IntLayout.I64Be -> 10\n    IntLayout.I64Le -> 11", "IntLayout.I64Be -> 11\n    IntLayout.I64Le -> 10")]),
    # Constants and rendering.
    programs("C constant hex decodes the low digit as high", RUNTIME, [
        ("raw[i] = (char)(hex_value(hex[2 * i]) << 4 | hex_value(hex[2 * i + 1]));",
         "raw[i] = (char)(hex_value(hex[2 * i + 1]) << 4 | hex_value(hex[2 * i]));")]),
    programs("C renders without separators", RUNTIME, [("debug_text(buffer, i ? digits : digits + 1);", "debug_text(buffer, digits + 1);")]),
]
