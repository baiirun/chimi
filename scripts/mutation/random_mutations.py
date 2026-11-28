"""Mutants of `std/random` and `chimi test --seed` (Phase 5, CHI-208, part 4a):
the generator on both backends, each test's seed, the Chimi functions over
`next_int`, and naming the seed in a failure report.

Run: scripts/mutation/mutate.py scripts/mutation/random_mutations.py
Each mutant runs tests/testing.rs, the end-to-end conformance for both backends.
"""

CRATE = "compiler/chimi"

C = "runtime/native/runtime.c"
RANDOM = "../../runtime/chimi-runtime/src/intrinsics/random_host.rs"
SOURCE = "../../runtime/chimi-runtime/src/random.rs"
EXECUTION = "src/execution.rs"
TESTING = "src/testing.rs"
STD = "std/random.chimi"


def mutant(name, file, edits, targets=("testing",), filter=""):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("both: the generator shifts by 16", SOURCE, [(
        "        let shifted = *b << 17;",
        "        let shifted = *b << 16;",
    )]),
    mutant("native: the runner's seed is ignored", RANDOM, [(
        "        stream.generator.get_or_insert_with(|| Generator::seeded(seed.unwrap_or_else(os_seed))).next()",
        "        stream.generator.get_or_insert_with(|| Generator::seeded(seed.map_or_else(os_seed, |_| 0))).next()",
    )]),
    mutant("native: the outcome never says a test drew", C, [(
        '  if (chimi_runtime_random_read_v1()) fputs(",\\"random_read\\":true", output);',
        "",
    )]),
    mutant("both: the generator rotates by 44", SOURCE, [(
        "        *d = d.rotate_left(45);",
        "        *d = d.rotate_left(44);",
    )]),
    mutant("seed: the test's name is not hashed", SOURCE, [(
        "    for byte in module.bytes().chain([0]).chain(name.bytes()) {",
        "    for byte in module.bytes() {",
    )]),
    mutant("seed: the run seed is ignored", SOURCE, [(
        "    let mut mix = run_seed ^ hash;",
        "    let mut mix = hash;",
    )]),
    mutant("interpreter: the configured seed is ignored", EXECUTION, [(
        "                        crate::random_source::Generator::seeded,",
        "                        |_| crate::random_source::Generator::seeded(0),",
    )]),
    mutant("report: the seed line is never written", TESTING, [(
        "            if result.random_read {",
        "            if false {",
    )]),
    mutant("Random.int: the range start is dropped", STD, [(
        "    False -> Range.start(range) + below(count)",
        "    False -> below(count)",
    )]),
    mutant("Random.int: an empty range draws", STD, [(
        "  case count == 0 {",
        "  case count < 0 {",
    )]),
    mutant("Random.float: 52 bits", STD, [(
        "  Float.from_int(bits_63() / 1024) / 9007199254740992.0",
        "  Float.from_int(bits_63() / 2048) / 4503599627370496.0",
    )]),
    mutant("Random.shuffle: the first item never moves", STD, [(
        "    case last < 1 {",
        "    case last < 2 {",
    )]),
    mutant("Random.pick: always the first item", STD, [(
        "    length -> List.get(list, below(length))",
        "    length -> List.get(list, 0)",
    )]),
    mutant("bits_63: the sign is cleared off by one", STD, [(
        "    True -> bits + 9223372036854775807 + 1",
        "    True -> bits + 9223372036854775807",
    )]),
]
