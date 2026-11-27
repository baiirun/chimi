"""Mutants of std/test's assertions in Rust (CHI-305 step 5, phase 5, seventh
PR; runtime/chimi-runtime/src/intrinsics/assertions.rs). The testing suite
runs `chimi test` on both backends and reads back the actual and expected
renderings from a failed comparison's message. Mutants of the C these
replaced were retargeted in test_runner.

Run: scripts/mutation/mutate.py scripts/mutation/assertion_intrinsic_mutations.py
Results: scripts/mutation/assertion_intrinsic_results.txt.
"""

CRATE = "compiler/chimi"
ASSERTIONS = "../../runtime/chimi-runtime/src/intrinsics/assertions.rs"


def testing(name, old, new):
    return dict(name=name, file=ASSERTIONS, edits=[(old, new)], targets=["testing"], filter="")


MUTATIONS = [
    testing("assert_not_equal fails on unequal values", "let wanted = intrinsic == Intrinsic::TEST_ASSERT_EQUAL;",
            "let wanted = true;"),
    testing("the actual rendering is on one line", "message.push_bytes(bytes_of(render(arguments[0], true)));",
            "message.push_bytes(bytes_of(render(arguments[0], false)));"),
    testing("expected and actual swap", "message.push_bytes(bytes_of(render(arguments[1], true)));",
            "message.push_bytes(bytes_of(render(arguments[0], true)));"),
    testing("the expected heading is dropped", 'message.push_bytes(b"\\nexpected:\\n");', 'message.push_bytes(b"\\n");'),
    testing("Test.fail drops its message", "Intrinsic::TEST_FAIL if arguments.len() == 1 && kind(0) == ValueKind::TEXT => message_defect(arguments[0], node),",
            "Intrinsic::TEST_FAIL if arguments.len() == 1 && kind(0) == ValueKind::TEXT => super::standard::defect(c\"fail\", node),"),
    testing("a failed assert reports an empty message",
            "{ success(void_value()) } else { message_defect(arguments[1], node) }",
            "{ success(void_value()) } else { super::standard::defect(c\"\", node) }"),
]
