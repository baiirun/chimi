"""Mutants of the std/json and std/decimal intrinsics in Rust (CHI-305 step 5,
phase 5, second PR; runtime/chimi-runtime/src/intrinsics/json.rs, decimals.rs
and text.rs). The std json and decimal checks (std_modules) compare both
backends, and the deep Json.encode test grows the output past one block.
Mutants of the code these replaced were retargeted in deep_value, hardening,
lane_fixes, rust_decimal_duration, rust_json and std2_json.

Run: scripts/mutation/mutate.py scripts/mutation/json_decimal_intrinsic_mutations.py
Results: scripts/mutation/json_decimal_intrinsic_results.txt.
"""

CRATE = "compiler/chimi"
INTRINSICS = "../../runtime/chimi-runtime/src/intrinsics/"
JSON = INTRINSICS + "json.rs"
DECIMALS = INTRINSICS + "decimals.rs"
TEXT = INTRINSICS + "text.rs"
MESSAGE = INTRINSICS + "message.rs"


def programs(name, file, old, new):
    return dict(name=name, file=file, edits=[(old, new)], targets=["std_modules"], filter="standard_")


def deep(name, file, old, new):
    return dict(name=name, file=file, edits=[(old, new)], targets=["language"],
                filter="json_encode_writes_a_deeply_nested_value")


MUTATIONS = [
    programs('Json.int writes a negative number without its sign', MESSAGE, "output[start] = b'-';", "output[start] = b'+';"),
    programs('Json.int writes each digit one too high', MESSAGE, "output[start] = b'0' + (magnitude % 10) as u8;", "output[start] = b'1' + (magnitude % 10) as u8;"),
    programs("a control character escapes with its high digit twice", JSON,
             "HEX[usize::from(byte & 0xf)]", "HEX[usize::from(byte >> 4)]"),
    programs("Json.encode writes False as true", JSON,
             "if (*field).payload.boolean { b\"true\" } else { b\"false\" }",
             "if !(*field).payload.boolean { b\"true\" } else { b\"false\" }"),
    programs("a decoded object pairs each value with the next key", JSON,
             "map::put(tree, member[0], member[1])", "map::put(tree, member[1], member[0])"),
    programs("a decode error points one byte late", JSON,
             "error.offset as i64)", "error.offset as i64 + 1)"),
    programs("a number's trailing zeros leave its exponent", JSON,
             "let trailing = (significant.len() - trimmed) as i64;", "let trailing = 0i64;"),
    programs("to_decimal keeps a scale that does not fit", JSON,
             "number.written_scale.clamp(0, max_scale).min(max_scale - length - number.exponent)",
             "number.written_scale.clamp(0, max_scale)"),
    programs("NotInteger reads as OutOfRange", JSON,
             "NotInt::NotInteger => StandardConstructor::JSON_NOT_INTEGER,",
             "NotInt::NotInteger => StandardConstructor::JSON_OUT_OF_RANGE,"),
    programs("Decimal.of drops the coefficient's sign", DECIMALS,
             "Decimal::of(integer(0), scale)", "Decimal::of(integer(0).abs(), scale)"),
    programs("Decimal.to_float skips the first character", DECIMALS,
             "strtod(terminated.as_ptr().cast(), core::ptr::null_mut())",
             "strtod(terminated.as_ptr().add(1).cast(), core::ptr::null_mut())"),
    programs("checked multiply adds", DECIMALS,
             "optional(decimal(0).multiply(decimal(1)))", "optional(decimal(0).add(decimal(1)))"),
    deep("a growing text keeps only half its bytes", TEXT,
         "ptr::copy_nonoverlapping(self.data, data, self.capacity.min(capacity));",
         "ptr::copy_nonoverlapping(self.data, data, self.capacity.min(capacity) / 2);"),
]
