# Native Direct Integer Loops (R3, CHI-90)

R3 in the [language completion plan](../plans/language-completion-plan.md)
lowers integer loops to machine code. It extends the
[direct integer roots](native-direct-integer-baseline.md), which compile pure
integer expressions, to whole `loop`s.

## What lowers

A `loop` lowers directly when its body, read through blocks that hold a single
expression, is built only from:

- integer arithmetic (`+`, `-`, `*`, `/`, `%`, unary `-`) over the loop state,
  integer literals and module `Int` constants;
- `break <Int expression>` to this loop;
- a `case` with one subject and no guards, whose subject is such an expression
  or an integer comparison (`<`, `<=`, `>`, `>=`, `==`), and whose arms are
  integer or `Bool` literals or `_`.

Anything else (a `let`, a call, a parameter or other local, a guard, a nested
loop) keeps the whole loop on the runtime path. The compiler emits one pass as a
Cranelift function `int32_t pass(int64_t state, int64_t *out)`, and the runtime's
`chimi_rt_direct_loop` drives it.

## Same outcomes as the runtime

- **Defects.** A pass never defects itself. Where the runtime would (overflow,
  division by zero, `Int` minimum `/ -1`), the pass hands back and the runtime
  loop reruns that pass from the same state. The pass is pure, so the rerun
  reports exactly the runtime's defect, operands and origin.
- **Cancellation** is checked before every pass, as in the runtime loop.
- **Tracing.** Only `--trace=off` runs take the direct path. Any tracing needs
  every node's span, so traced runs use the runtime loop from the start.
- **State type.** A body that never reads its state (`loop s = "x" { break 5 }`)
  lowers whatever the state's type; a state that is not an `Int` runs on the
  runtime loop.
- **Memory.** Direct passes allocate nothing and open no allocation region, so
  the memory report shows no collections for such a loop.

Both-backend tests in `tests/language.rs` compare these loops with the
interpreter, including the defects, and `scripts/mutation/direct_loop_mutations.py`
holds the mutants they kill.

## Results

Same programs, machine and method as [native allocation part 1](native-allocation-r2.md):
10,000,000 passes, `--trace=off`, 9 interleaved runs, p50 wall time of the
whole process divided by the pass count. The 1-minute load average was 4 to 6
during these runs, which affects the runtime-path rows more than the direct
rows.

| Workload | Runtime path (`8a35434`) | Direct (`fce208c`) |
| --- | ---: | ---: |
| `loop` | 120 ns | 2 ns |
| `loop-bool` | 183 ns | 2 ns |
| `loop-bind` (has a `let`, so stays on the runtime path) | 229 ns | 228 ns |

The direct rows are dominated by process start: a 10M-pass run takes about
20 ms, against 1.2 s before. Peak RSS is unchanged (14.8 MB).

## Limits and next steps

- Loops that read a parameter or an enclosing `let` do not lower yet. Passing
  those locals into the pass as extra arguments would cover counting loops over
  a function's argument, the most common shape after these.
- `let` inside the body, `and`, `or` and `not` (which lower to matches over
  `Bool`) are the next forms to add when a real program needs them.
- The direct path is not used under `--trace=boundaries`. Supporting it means
  reproducing the runtime's span count for each pass.
