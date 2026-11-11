# Repetition

## Status

This is the normative contract for unbounded repetition in Chimi source
(`prog show ts-3e25fc`). It is implemented through parsing, resolution, typing,
capability checking, executable Core, the reference interpreter, and the native
runtime.

## Goals

Repetition must be visible at the construct, bounded in stack use by
construction, and compatible with immutable values. A long-lived process, a
channel consumer, a retry, and a numeric search use the same form. No edit
inside a loop body may silently turn bounded repetition into stack growth.

## Canonical form

<!-- chimi: check -->
```chimi
loop state = initial {
  body
}
```

- `state` is an ordinary local binding visible only in `body`.
- `initial` is evaluated once, in the enclosing scope, before the first pass.
- Each pass evaluates `body` with `state` bound to the current value.
- The value `body` produces is the state for the next pass, by the same rule
  that makes a block's final expression its result.
- `break value` ends the loop; the loop expression evaluates to `value`.
  `break` with no value means `break Void`.

Several pieces of state use a struct:

<!-- chimi: check -->
```chimi
struct Progress {
  passes: Int,
  total: Int,
}

fn sum_to(limit: Int) -> Int {
  loop progress = Progress(0, 0) {
    case progress.passes {
      passes if passes > limit -> break progress.total
      passes -> Progress(passes + 1, progress.total + passes)
    }
  }
}
```

A channel consumer:

<!-- chimi: check -->
```chimi
loop processed = 0 {
  let next = Channel.receive(jobs)
  case next {
    Receive.Received(value) -> {
      Channel.send(results, apply_transform(value))
      processed + 1
    }
    Receive.Drained -> {
      Channel.close(completion)
      break processed
    }
  }
}
```

There is no `continue` keyword. Reaching the end of the body continues.

## Static rules

- The body MUST produce the type of `initial`. A branch that forgets to produce
  the next state is a type error (`T4203`), not a stale pass.
- Every `break` of one loop MUST supply the same type; that type is the loop's
  success type.
- A loop that no `break` targets never produces a value. Its type is
  unconstrained, so it fits any expected type, and it ends only through
  failure, defect, or cancellation.
- `break` leaves the nearest enclosing `loop`. Labels do not exist.
- `break` MUST NOT cross a function boundary. A `break` outside every loop of
  its own function, including one inside a closure created in a loop body, is
  rejected with `E_BREAK_OUTSIDE_LOOP`.
- The failures and needs of a loop are the union of those of `initial` and
  `body`. An expected failure in a pass leaves the loop through the ordinary
  failure channel.
- A pass MUST NOT transfer a capability bound outside the loop (into a task, a
  one-shot closure, or a channel), because the next pass would use it again
  (`T4250`). The check is conservative: it also rejects a transfer that is
  followed by `break` in the same branch.

## Execution

- Passes run iteratively. Host stack use is independent of the pass count.
- Cancellation is observed before every pass.
- Each pass's operations are traced as children of the loop's span, so passes
  are siblings rather than nested.
- In the native runtime, a program that needs no scheduler reclaims each pass's
  allocations once the pass ends, keeping only what the next state reaches.
  Peak live memory is bounded by one pass rather than the pass count. Programs
  that use tasks or channels do not yet reclaim per pass
  (`prog show ts-da83af`).

## Recursion

Recursion remains an ordinary feature for structurally bounded work such as
walking a tree. Chimi does not guarantee that calls in tail position reuse the
caller's stack frame, so recursion is not a supported way to repeat work an
unbounded number of times. A marked, compiler-checked tail call may be added if
mutually recursive state machines prove necessary.

## Rejected alternatives

- **Recursion as the canonical model with implicit tail calls.** Wrapping the
  recursive call in `using`, failure handling, or arithmetic silently removes
  the guarantee, and each pass would nest a trace span.
- **Local `var` reassignment with `while`/`for`.** It introduces statements and
  local mutation into an expression language.
- **`continue value`.** Unfamiliar; the body's final expression already carries
  the next state.
- **Rebinding with `let` followed by bare `continue`.** A forgotten `let`
  silently repeats the old state.
- **A runtime-owned loop function per producer.** Every new kind of repetition
  would need its own runtime implementation.
