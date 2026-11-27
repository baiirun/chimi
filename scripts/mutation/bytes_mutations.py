"""Mutants of std/bytes (Phase 6a, CHI-210): the interpreter core in
src/execution/bytes_operations.rs, its native twin (runtime.c until CHI-305
step 5 phase 5, now runtime/chimi-runtime/src/intrinsics/bytes.rs), and
the Chimi wrappers in std/bytes.chimi. Rust and C sides are mutated separately
because std_modules compares both backends against the same expected files.

Run: scripts/mutation/mutate.py scripts/mutation/bytes_mutations.py
"""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"
DEBUG_RENDER = "../../runtime/chimi-runtime/src/intrinsics/debug.rs"
BYTES_INTRINSICS = "../../runtime/chimi-runtime/src/intrinsics/bytes.rs"
CORE = "src/execution/bytes_operations.rs"
WRAPPER = "std/bytes.chimi"
ORDER = "src/execution/structural.rs"


def programs(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["std_modules"],
                filter="standard_")


MUTATIONS = [
    # Construction and queries.
    programs("Rust from accepts 256", CORE, [("u8::try_from(*value)", "u8::try_from(*value % 256)")]),
    programs('native from accepts 256', BYTES_INTRINSICS, [('let Ok(byte) = u8::try_from(byte) else {', 'let Ok(byte) = u8::try_from(byte % 256) else {')]),
    programs('native get admits index == length', BYTES_INTRINSICS, [('.and_then(|index| bytes_of(arguments[0]).get(index));', '.and_then(|index| bytes_of(arguments[0]).get(index).or(bytes_of(arguments[0]).get(index.wrapping_sub(1))));')]),
    programs('native get reads a signed char', BYTES_INTRINSICS, [('|&byte| int(i64::from(byte))))', '|&byte| int(i64::from(byte as i8))))')]),
    programs("Rust index_of allows an empty needle", CORE, [("if needle.is_empty() {", "if false {")]),
    programs('native slice skips clamping', BYTES_INTRINSICS, [('let clamp = |bound: i64| usize::try_from(bound).map_or(0, |bound| bound.min(length));', 'let clamp = |bound: i64| (bound as usize).min(length);')]),
    programs("ends_with drops the length guard", WRAPPER, [
        ("length(suffix) <= end and slice", "slice")]),
    programs("split_once keeps the separator", WRAPPER, [
        ("slice(bytes, at + length(separator)..length(bytes))", "slice(bytes, at..length(bytes))")]),
    # Order.
    programs("Rust Bytes order reversed", ORDER, [
        ("(RuntimeValue::Bytes(left), RuntimeValue::Bytes(right)) => left.cmp(right),",
         "(RuntimeValue::Bytes(left), RuntimeValue::Bytes(right)) => right.cmp(left),")]),
    # UTF-8.
    programs('native accepts surrogates', BYTES_INTRINSICS, [('            if first == 0xed {\n                high = 0x9f;\n            }\n', '')]),
    programs('native accepts overlong three-byte forms', BYTES_INTRINSICS, [('            if first == 0xe0 {\n                low = 0xa0;\n            }\n', '')]),
    programs('native accepts past U+10FFFF', BYTES_INTRINSICS, [('            if first == 0xf4 {\n                high = 0x8f;\n            }\n', '')]),
    programs('native replaces a whole truncated sequence per byte', BYTES_INTRINSICS, [('_ => return (taken, false),', '_ => return (1, false),')]),
    programs("to_text reports the end, not the first invalid byte", WRAPPER, [
        ("Failure(DecodeError.InvalidUtf8(valid))", "Failure(DecodeError.InvalidUtf8(length(bytes)))")]),
    # Integers.
    programs("Rust layout swaps byte order for 16 and 32 bits", CORE, [
        ("within % 2 == 0,", "within % 2 == 1,")]),
    programs('native reads without sign extension', BYTES_INTRINSICS, [('if layout.signed && bits < 64 &&', 'if false && layout.signed && bits < 64 &&')]),
    programs("Rust read without sign extension", CORE, [
        ("        if self.signed {\n            // Sign-extend", "        if false {\n            // Sign-extend")]),
    programs('native read admits a field one past the end', BYTES_INTRINSICS, [('.and_then(|rest| rest.get(..layout.width));', '.and_then(|rest| rest.get(..layout.width).or_else(|| rest.get(..layout.width - 1)));')]),
    programs("Rust unsigned write admits 2^bits", CORE, [
        ("(false, _) => (0..(1i64 << bits)).contains(&value),", "(false, _) => (0..=(1i64 << bits)).contains(&value),")]),
    programs('native signed write admits 2^(bits-1)', BYTES_INTRINSICS, [('(-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 1)', '(-(1i64 << (bits - 1)), 1i64 << (bits - 1))')]),
    programs('native little-endian write is big-endian', BYTES_INTRINSICS, [('raw[if layout.big_endian { index } else { layout.width - 1 - index }] = byte;', 'raw[index] = byte;')]),
    programs("layout codes swap I64Be and I64Le", WRAPPER, [
        ("IntLayout.I64Be -> 10\n    IntLayout.I64Le -> 11", "IntLayout.I64Be -> 11\n    IntLayout.I64Le -> 10")]),
    # Constants and rendering.
    programs("C constant hex decodes the low digit as high", RUNTIME, [
        ("raw[i] = (char)(hex_value(hex[2 * i]) << 4 | hex_value(hex[2 * i + 1]));",
         "raw[i] = (char)(hex_value(hex[2 * i + 1]) << 4 | hex_value(hex[2 * i]));")]),
    programs("native renders without separators", DEBUG_RENDER, [("self.text(if index > 0 { &digits } else { &digits[1..] });", "self.text(&digits[1..]);")]),
]
