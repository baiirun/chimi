"""Mutants of the std/bytes intrinsics in Rust (CHI-305 step 5, phase 5,
third PR; runtime/chimi-runtime/src/intrinsics/bytes.rs and message.rs).
The std bytes checks (std_modules) compare both backends, defect messages
included. Mutants of the C these replaced were retargeted in bytes,
hardening and http_byte_class.

Run: scripts/mutation/mutate.py scripts/mutation/bytes_intrinsic_mutations.py
Results: scripts/mutation/bytes_intrinsic_results.txt.
"""

CRATE = "compiler/chimi"
INTRINSICS = "../../runtime/chimi-runtime/src/intrinsics/"
BYTES = INTRINSICS + "bytes.rs"
MESSAGE = INTRINSICS + "message.rs"


def programs(name, file, old, new):
    return dict(name=name, file=file, edits=[(old, new)], targets=["std_modules"], filter="standard_")


MUTATIONS = [
    programs("Bytes.from's defect names the next element", BYTES,
             ".integer(i64::from(index))", ".integer(i64::from(index) + 1)"),
    programs("a defect message drops its last piece", MESSAGE,
             "let taken = bytes.len().min(room);", "let taken = bytes.len().min(room).min(16);"),
    programs("Bytes.length counts one more", BYTES,
             "success(int((*arguments[0]).payload.text.length as i64))",
             "success(int((*arguments[0]).payload.text.length as i64 + 1))"),
    programs("index_of reports one past the match", BYTES,
             "int(found.cast::<u8>().offset_from(haystack.as_ptr()) as i64)",
             "int(found.cast::<u8>().offset_from(haystack.as_ptr()) as i64 + 1)"),
    programs("concat drops the first part", BYTES,
             "output.push_bytes(bytes_of(part));", "if index > 0 {\n                        output.push_bytes(bytes_of(part));\n                    }"),
    programs("to_list adds one to each byte", BYTES,
             "*items.add(index) = int(i64::from(byte));", "*items.add(index) = int(i64::from(byte) + 1);"),
    programs("from_text keeps the Text kind", BYTES,
             "        (*value).kind = ValueKind::BYTES;\n        value\n", "        value\n"),
    programs("valid_utf8_length counts every byte", BYTES,
             "|error| error.valid_up_to()", "|_| bytes.len()"),
    programs("a read is always big-endian", BYTES,
             "field[if layout.big_endian { index } else { layout.width - 1 - index }]", "field[index]"),
]
