"""Mutants of timestamp text and parsing in Rust (CHI-305,
runtime/chimi-runtime/src/timestamp.rs), caught by its unit tests. The
runtime fuzz target `timestamp` checks the same code against the C it
replaced (tests/fuzz/timestamp-c-reference.c).

Run: scripts/mutation/mutate.py scripts/mutation/timestamp_mutations.py
Results: scripts/mutation/timestamp_results.txt.
"""

CRATE = "runtime/chimi-runtime"
TIMESTAMP = "src/timestamp.rs"


def m(name, old, new):
    return dict(name=name, file=TIMESTAMP, edits=[(old, new)], targets=["lib"], filter="timestamp")


MUTATIONS = [
    m("the fraction keeps its trailing zeros",
      "        while fraction % 10 == 0 {",
      "        while false && fraction % 10 == 0 {"),
    m("a time before 1970 rounds toward zero",
      "    let (date, of_day) = civil_seconds(nanoseconds.div_euclid(NANOS_PER_SECOND));\n    let mut fraction = nanoseconds.rem_euclid(NANOS_PER_SECOND);",
      "    let (date, of_day) = civil_seconds(nanoseconds / NANOS_PER_SECOND);\n    let mut fraction = (nanoseconds % NANOS_PER_SECOND).abs();"),
    m("a day before 1970 rounds toward zero",
      "    (civil_from_days(seconds.div_euclid(SECONDS_PER_DAY)), seconds.rem_euclid(SECONDS_PER_DAY))",
      "    (civil_from_days(seconds / SECONDS_PER_DAY), (seconds % SECONDS_PER_DAY).abs())"),
    m("leap second 60 is accepted",
      "|| minute > 59 || second > 59",
      "|| minute > 59 || second > 60"),
    m("the offset's sign is ignored",
      "            if sign == b'-' { -magnitude } else { magnitude }",
      "            let _ = sign;\n            magnitude"),
    m("1677 is accepted",
      "    if !(1678..=2262).contains(&year)",
      "    if !(1677..=2262).contains(&year)"),
    m("February 29 is accepted in every year",
      "        2 if leap => 29,\n        2 => 28,",
      "        2 => 29,"),
    m("a short fraction is not scaled",
      "        for _ in count..9 {\n            fraction *= 10;\n        }",
      ""),
    m("text after the zone is accepted",
      "    if cursor.at != text.len() {\n        return None;\n    }",
      ""),
    m("an out-of-range result wraps",
      "    seconds.checked_mul(NANOS_PER_SECOND)?.checked_add(fraction)",
      "    Some(seconds.wrapping_mul(NANOS_PER_SECOND).wrapping_add(fraction))"),
    m("a lowercase t is rejected",
      "    cursor.expect(b\"Tt\")?;",
      "    cursor.expect(b\"T\")?;"),
    m("an empty fraction is accepted",
      "        if count == 0 {\n            return None;\n        }",
      ""),
]
