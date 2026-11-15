# Build procedure

How the Chimi language build runs now. It is written for Byron and for any
agent that picks up the work. It replaces the "How we work" section of the
[2026-11-14 handoff](handoff.md), which is kept as history. The evidence behind
each rule is in the [orchestration AAR](orchestration-aar.md), and the
AAR entry a rule came from is in brackets.

What the language should be lives in the vision docs, not here:
[design-philosophy.md](../design-philosophy.md), [CONTEXT.md](../../CONTEXT.md)
and [decisions.md](../decisions.md).

## Goal

Make steady progress through the remaining language work without waiting on
Byron. He is asked only about important choices that turn on his taste and
that the vision docs cannot predict.

## Roles

- **Byron** sets direction and answers the escalated taste questions. He does
  not review skeletons or intermediate work.
- **The coordinator** (the project chat) plans waves, keeps the queue, starts
  lanes as slots free up, batches questions for Byron, and schedules check-ins
  so nothing stalls. It does not implement.
- **A lane** is one cloud thread working one brief. It implements, gets the
  work reviewed and verified, merges its own PR, and updates the records.
- **The advisor** (Fable 5.2, `claude-fable-5-2`) is consulted for wave
  planning, recurring failures, soundness-critical design, and a check before
  declaring hard work done.

## Waves, lanes and the queue

- Work runs in waves of parallel lanes. Each lane has a brief with its
  tickets, goal, spec sections, files, done condition, dependencies, and what
  later work needs from it ("builds toward").
- **No file ownership.** Each lane works on its own branch, so a lane may
  edit any file its work needs. When `main` moves, the lane updates its branch
  and resolves any merge conflicts itself. Revisit this only if conflicts
  become a real problem (Byron, 2026-11-15).
- The briefs and the ordered queue live in
  `/mnt/project-files/plans/next-waves.md` (the project's shared folder). Its
  "Rules every lane follows" block is pasted into each brief.
- Plan the next wave while the current one wraps up, as soon as its first
  lanes enter review, so the next lane starts the moment a slot frees.
- Plan only for known near-term dependencies. Do not map the whole graph or
  design for later phases; refactoring later is fine.
- A lane tells the coordinator when it has merged, so the next queue item can
  start.

## Inside a lane

1. **Branch.** Work on the lane's own branch from the latest `main`, and open
   one PR (or a few small ones when the lane has separable parts). Push early,
   plain push, never force [N27, N29].
2. **Implement** with the code-quality skill
   ([github.com/baiirun/skills](https://github.com/baiirun/skills)), skipping
   its approval gates.
3. **Test what matters** [N31]. Locally, run only the tests that cover the
   change; CI runs the full suite. Tests assert observable behaviour only:
   program output, diagnostics, outcomes and contractual bounds. Each test must
   reject a plausible wrong implementation. No wall-clock gates in the default
   suite [N18].
4. **Check**, with these run in parallel:
   - a mutation run with `scripts/mutation/mutate.py`, once per branch [W2,
     N30]. Rerun it after a rebase only if the rebase changed a mutated file;
   - an independent review by Sonnet, plus Opus for soundness-critical work
     (types, memory, scheduling) [W1].
5. **Verify.** A verifier that defaults to "refuted" reproduces or refutes each
   finding [W1]. Fix only findings with a reachable failure, a broken contract
   or a wrong test claim. File tickets for the rest and drop nits [N2].
6. **Stop and rethink** after two review rounds in a row with new HIGH
   findings [N20, N26].

Briefs state the scope and any user concern already handled [N28]. For
design-heavy work they also give a token budget and a fallback, with a
check-in at half the budget. Agent reports stay short, with the details in a
file [N7].

## Merging

Byron's standing approval: a lane merges its own PR into `main` once review and
the verifier are done and every CI check passes.

- Subscribe to your PR's activity right after opening it, and watch its CI,
  passes as well as failures. Do not wait to be woken.
- If `main` moved since the last CI run, update the branch and wait for CI to
  rerun. There is no branch protection, so this is on the lane.
- Merge or rebase, never force-push.
- The checks that must pass are `test (macos-latest)` and `test (ubuntu-latest)`.
  `program outputs (*)` compares 12 program outputs with the base branch. A red
  `program outputs` check may be merged only when the difference is expected
  and the program outputs are identical, with the reason noted on the PR
  (Byron, 2026-11-15; the check itself is in decisions.md).
- Never call CI green while any check is red.
- If auto mode blocks the merge, ask Byron in the lane's thread to write the
  go-ahead, naming the PR.

## Decisions

- Decide from the vision docs, and record every decision in
  [decisions.md](../decisions.md), the only decision ledger [N8].
- Escalate only important choices whose answer the vision docs cannot predict.
  The open questions are listed in the wave plan's rules block. Until they
  are decided, add no `try` or `?` sugar.
- Batch escalations: each question with two to four options and a
  recommendation, put to Byron at once. Keep working on the recommended option
  while waiting, unless the step cannot be undone.

## Tracking

- **Tickets** are in Linear, team "chimi" (key CHI). Titles keep their old prog
  id, such as `[ts-8f201b]`. Epics are parent issues with the `epic` label, and
  dependencies are blocked-by relations. Prog is historical.
- Update tickets as work lands. Write comments for a person: plain sentences on
  what changed, why it matters, and what's next. Explain any internal code you
  mention. Put commit hashes, test counts and file lists in a collapsed
  section at the end:

  ```
  +++ Agent notes

  Details for agents go here.

  +++
  ```
- Add a run-log row to the [AAR](orchestration-aar.md) when a lane finishes,
  and keep its numbers honest: say "not measured" rather than guess.

## Standing constraints

- Ask Byron before running evals, installing anything, or changing machine
  or agent config. Edits to Claude Code or Codex config need his direct
  confirmation.
- Stage only your own paths. No `git stash` or `git add -A` in a shared
  checkout, and no attribution trailers on commits [N4, N11, N14].
- Leave other Chimi sessions alone.

## Where things are

| What | Where |
| --- | --- |
| Vision | `docs/design-philosophy.md`, `CONTEXT.md`, `docs/decisions.md` |
| Plan and specs | `docs/plans/`, `docs/specification/` |
| Wave briefs and queue | `/mnt/project-files/plans/next-waves.md` |
| Process evidence | `docs/process/orchestration-aar.md` |
| CI | `.github/workflows/ci.yml` |
| Mutation runner and lists | `scripts/mutation/` |
| Process conformance lab | `compiler/chimi/src/process_lab/`, `compiler/chimi/tests/process-lab/` |
| Std sources and tests | `compiler/chimi/std/`, `compiler/chimi/std-tests/`, `compiler/chimi/tests/std_modules.rs` |
| Tickets | Linear team "chimi" (CHI) |

`scripts/integrate-branch.sh` and `scripts/heavy.sh` came from local runs on
Byron's laptop. Cloud lanes merge through CI instead, and use `heavy.sh` only
when several builds share one machine.
