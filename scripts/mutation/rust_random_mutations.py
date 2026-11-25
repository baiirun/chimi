"""Mutants of the shared random generator (CHI-307):
runtime/chimi-runtime/src/random.rs and its C glue in runtime.c.

Both backends draw from this code, so comparing them cannot see a mutant
here. The oracle is the recorded draws at run seed 0 in tests/testing.rs,
which an independent model of xoshiro256** seeded through SplitMix64
reproduces, checked on both backends. Each mutant targets only that test.

Run: scripts/mutation/mutate.py scripts/mutation/rust_random_mutations.py
"""

CRATE = "compiler/chimi"

RANDOM = "../../runtime/chimi-runtime/src/random.rs"
RUNTIME = "runtime/native/runtime.c"


def draws(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["testing"],
                filter="a_test_draws_the_same_numbers")


MUTATIONS = [
    draws("output multiplier 3", RANDOM,
          [("b.wrapping_mul(5)", "b.wrapping_mul(3)")]),
    draws("state rotation 44", RANDOM,
          [("*d = d.rotate_left(45);", "*d = d.rotate_left(44);")]),
    draws("shift 16", RANDOM,
          [("let shifted = *b << 17;", "let shifted = *b << 16;")]),
    draws("state words not mixed", RANDOM,
          [("        *a ^= *d;\n", "")]),
    draws("SplitMix64 increment off by one", RANDOM,
          [("state.wrapping_add(0x9e37_79b9_7f4a_7c15)", "state.wrapping_add(0x9e37_79b9_7f4a_7c16)")]),
    draws("test seed ignores the separator", RANDOM,
          [(".chain([0])", "")]),
    draws("test seed ignores the run seed", RANDOM,
          [("let mut mix = run_seed ^ hash;", "let mut mix = hash;")]),
    draws("C reseeds on every draw", RUNTIME,
          [("    random_read = true;\n  }\n  uint64_t result", "  }\n  uint64_t result")]),
]
