"""Mutants of direct integer loops (CHI-90, R3).

Run: scripts/mutation/mutate.py scripts/mutation/direct_loop_mutations.py
Each mutant must turn one of the named native tests red.
"""

CRATE = "compiler/chimi"

L = "src/backend/direct_loop.rs"
R = "runtime/native/runtime.c"

LANGUAGE = dict(targets=["language"], filter="direct_integer_loop")
DIRECT = dict(targets=["cli"], filter="native_integer_loop_runs_as_direct_passes")
TRACED = dict(targets=["cli"], filter="native_traced_integer_loop_reports_every_pass")


def m(name, file, edits, selection):
    return dict(name=name, file=file, edits=edits, **selection)


MUTATIONS = [
 m("arithmetic wraps instead of falling back", L,
   [("                    continue_unless(builder, overflow, fallback);\n                    value\n                }\n                BinaryOperator::Divide",
     "                    let _ = overflow;\n                    value\n                }\n                BinaryOperator::Divide")], LANGUAGE),
 m("division by zero is not checked", L,
   [("                    let defect = builder.ins().bor(zero, overflow);",
     "                    let defect = builder.ins().bor(overflow, overflow);\n                    let _ = zero;")], LANGUAGE),
 m(">= compiles as >", L,
   [("BinaryOperator::GreaterEqual => IntCC::SignedGreaterThanOrEqual,",
     "BinaryOperator::GreaterEqual => IntCC::SignedGreaterThan,")], LANGUAGE),
 m("division truncates the wrong way", L,
   [("                    builder.ins().sdiv(left, right)", "                    builder.ins().udiv(left, right)")], LANGUAGE),
 m("a break continues the loop", L,
   [("                if matches!(pass, Pass::Break(_)) {\n                    BREAK\n",
     "                if matches!(pass, Pass::Break(_)) {\n                    CONTINUE\n")], LANGUAGE),
 m("Int minimum / -1 is not checked", L,
   [("                    let defect = builder.ins().bor(zero, overflow);",
     "                    let defect = zero;\n                    let _ = overflow;")], LANGUAGE),
 m("a state that is not an Int defects", R,
   [("    return run_loop_passes(runtime, node_id, incoming, parent, initial->value);",
     "    return defect(\"verified direct loop state is not an integer\", node_id);")],
   dict(targets=["language"], filter="direct_loops_over_other_states")),
 m("no loop is lowered directly", L,
   [("                reader.pass(body.raw()).map(|pass| (node.id.raw(), pass))",
     "                reader.pass(body.raw()).and(None)")], DIRECT),
 m("tracing runs the direct passes", R,
   [("  if (!runtime->silent) return chimi_rt_loop(runtime, node_id, incoming, parent);\n", "")], TRACED),
]
