"""Mutants of std/http's byte-class checks (CHI-303): the classes a head, a
header name and value, a host and a target may hold, trimming, ASCII
lowercasing, and the native `count_ends` on both backends.

Run: scripts/mutation/mutate.py --timeout 600 scripts/mutation/http_byte_class_mutations.py
Results: scripts/mutation/http_byte_class_results.txt.
"""

CRATE = "compiler/chimi"

HTTP = "std/http.chimi"
RUNTIME = "runtime/native/runtime.c"
BYTES_INTRINSICS = "../../runtime/chimi-runtime/src/intrinsics/bytes.rs"
INTERPRETER = "src/execution/bytes_operations.rs"

SERVER = dict(targets=["http"], filter="")
CLIENT = dict(targets=["http_client"], filter="")
BOTH = dict(targets=["std_modules"], filter="standard_tests_pass_on_both_backends")
UNIT = dict(targets=["lib"], filter="count_ends")


def mutant(name, old, new, tests, file=HTTP):
    return dict(name=name, file=file, edits=[(old, new)], **tests)


MUTATIONS = [
    # Classes.
    mutant("a backquote is not a token character",
           "Bytes.from_text(\"!#$%&'*+-.^_`|~0123456789${letter_bytes}\")",
           "Bytes.from_text(\"!#$%&'*+-.^_|~0123456789${letter_bytes}\")", BOTH),
    mutant("a parenthesis is a token character",
           "Bytes.from_text(\"!#$%&'*+-.^_`|~0123456789${letter_bytes}\")",
           "Bytes.from_text(\"()!#$%&'*+-.^_`|~0123456789${letter_bytes}\")", SERVER),
    mutant("an empty name is a token",
           "  (not Bytes.is_empty(bytes)) and is_only(bytes, allowed)",
           "  is_only(bytes, allowed)", BOTH),
    mutant("a head may hold DEL",
           "const head_bytes: Bytes = Bytes.from(List.concat([[9, 10, 13], byte_span(32, 127), byte_span(128, 256)]))",
           "const head_bytes: Bytes = Bytes.from(List.concat([[9, 10, 13], byte_span(32, 128), byte_span(128, 256)]))", SERVER),
    mutant("a head may hold a unit separator",
           "const head_bytes: Bytes = Bytes.from(List.concat([[9, 10, 13], byte_span(32, 127), byte_span(128, 256)]))",
           "const head_bytes: Bytes = Bytes.from(List.concat([[9, 10, 13, 31], byte_span(32, 127), byte_span(128, 256)]))", SERVER),
    mutant("a head may not hold non-ASCII",
           "const head_bytes: Bytes = Bytes.from(List.concat([[9, 10, 13], byte_span(32, 127), byte_span(128, 256)]))",
           "const head_bytes: Bytes = Bytes.from(List.concat([[9, 10, 13], byte_span(32, 127)]))", SERVER),
    mutant("a head may not hold HTAB",
           "const head_bytes: Bytes = Bytes.from(List.concat([[9, 10, 13], byte_span(32, 127), byte_span(128, 256)]))",
           "const head_bytes: Bytes = Bytes.from(List.concat([[10, 13], byte_span(32, 127), byte_span(128, 256)]))", SERVER),
    mutant("a header value may hold CR",
           "const line_bytes: Bytes = Bytes.from(List.concat([[9], byte_span(32, 127), byte_span(128, 256)]))",
           "const line_bytes: Bytes = Bytes.from(List.concat([[9, 13], byte_span(32, 127), byte_span(128, 256)]))", BOTH),
    mutant("a header value may hold DEL",
           "const line_bytes: Bytes = Bytes.from(List.concat([[9], byte_span(32, 127), byte_span(128, 256)]))",
           "const line_bytes: Bytes = Bytes.from(List.concat([[9], byte_span(32, 128), byte_span(128, 256)]))", BOTH),
    mutant("a visible character may be a space",
           "const visible_bytes: Bytes = Bytes.from(List.concat([byte_span(33, 127), byte_span(128, 256)]))",
           "const visible_bytes: Bytes = Bytes.from(List.concat([byte_span(32, 127), byte_span(128, 256)]))", CLIENT),
    mutant("a host name may hold an underscore no more",
           "const name_bytes: Bytes = Bytes.from_text(\"-._~0123456789${letter_bytes}\")",
           "const name_bytes: Bytes = Bytes.from_text(\"-.~0123456789${letter_bytes}\")", CLIENT),
    mutant("an IPv6 literal may hold letters past f",
           "const ipv6_bytes: Bytes = Bytes.from_text(\"0123456789ABCDEFabcdef:.\")",
           "const ipv6_bytes: Bytes = Bytes.from_text(\"0123456789ABCDEFabcdefg:.\")", CLIENT),
    mutant("a target may hold a backslash",
           "const target_bytes: Bytes = Bytes.from_text(\"-._~0123456789${letter_bytes}!$&'()*+,;=:@/?%\")",
           "const target_bytes: Bytes = Bytes.from_text(\"-._~0123456789${letter_bytes}!$&'()*+,;=:@/?%\\\\\")", CLIENT),
    mutant("a query does not end the authority",
           "const authority_bytes: Bytes = Bytes.from(List.concat([byte_span(0, 47), byte_span(48, 63), byte_span(64, 256)]))",
           "const authority_bytes: Bytes = Bytes.from(List.concat([byte_span(0, 47), byte_span(48, 256)]))", CLIENT),
    mutant("nine is not a digit",
           "const digit_bytes: Bytes = Bytes.from_text(\"0123456789\")",
           "const digit_bytes: Bytes = Bytes.from_text(\"012345678\")", CLIENT),
    # Trimming.
    mutant("a tab is not trimmed",
           "const space_bytes: Bytes = Bytes.from_text(\" \\t\")",
           "const space_bytes: Bytes = Bytes.from_text(\" \")", SERVER),
    mutant("a no-break space is trimmed",
           "const space_bytes: Bytes = Bytes.from_text(\" \\t\")",
           "const space_bytes: Bytes = Bytes.from_text(\" \\t\\u{a0}\")", SERVER),
    mutant("the end of a value is not trimmed",
           "Bytes.slice(bytes, start..(length - end))",
           "Bytes.slice(bytes, start..length)", SERVER),
    mutant("the start of a value is not trimmed",
           "Bytes.slice(bytes, start..(length - end))",
           "Bytes.slice(bytes, 0..(length - end))", SERVER),
    # Lowercasing.
    mutant("any text lowercases as Unicode",
           "      False -> lowercase_bytes(bytes)",
           "      False -> Text.lowercase(text)", BOTH),
    mutant("Z counts as lowercase",
           "const not_upper_bytes: Bytes = Bytes.from(List.concat([byte_span(0, 65), byte_span(91, 256)]))",
           "const not_upper_bytes: Bytes = Bytes.from(List.concat([byte_span(0, 65), byte_span(90, 256)]))", BOTH),
    mutant("every byte counts as ASCII",
           "const ascii_bytes: Bytes = Bytes.from(byte_span(0, 128))",
           "const ascii_bytes: Bytes = Bytes.from(byte_span(0, 256))", BOTH),
    # The native.
    mutant('native: the last allowed byte is skipped', 'for &byte in bytes_of(arguments[1]) {', 'for &byte in bytes_of(arguments[1]).split_last().map_or(&[][..], |(_, rest)| rest) {', SERVER, BYTES_INTRINSICS),
    mutant('native: the end is never counted', 'bytes.iter().rev().take_while(|&&byte| allowed[usize::from(byte)]).count()', '0', SERVER, BYTES_INTRINSICS),
    mutant('native: the end counts from the start', 'bytes.iter().rev().take_while(|&&byte| allowed[usize::from(byte)]).count()', 'bytes.iter().take_while(|&&byte| allowed[usize::from(byte)]).count()', SERVER, BYTES_INTRINSICS),
    mutant('native: the start stops one short', 'let leading = bytes.iter().take_while(|&&byte| allowed[usize::from(byte)]).count();', 'let leading = bytes.iter().take_while(|&&byte| allowed[usize::from(byte)]).count().min(bytes.len().saturating_sub(1));', BOTH, BYTES_INTRINSICS),
    mutant("interpreter: the end counts from the start",
           "        value.iter().rev().take_while(held).count(),",
           "        value.iter().take_while(held).count(),", UNIT, INTERPRETER),
    mutant("interpreter: the start counts from the end",
           "        value.iter().take_while(held).count(),\n        value.iter().rev()",
           "        value.iter().rev().take_while(held).count(),\n        value.iter().rev()", UNIT, INTERPRETER),
]
