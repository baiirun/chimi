# Coordinator handoff (2026-11-14)

> **Historical.** This records the state at the 2026-11-14 handoff. The
> current way of working is in [procedure.md](procedure.md), and tickets moved
> from prog to Linear (team CHI) on 2026-11-15.

The first coordinator session (started 2026-11-11) hands the language build
to the projects coordinator. This note says where the work stands, how we work,
and what has and has not worked. The
[orchestration AAR](orchestration-aar.md) holds the evidence. Read its "What is
not working" table (N1 to N32) before changing the process.

## Where the work stands

- **Plan:** [language completion plan](../plans/language-completion-plan.md),
  epic `ep-a6f138` in prog (`prog show ep-a6f138` has a state line per
  integration).
- **Phase 0 (runtime memory) and Phase 1 (core language):** done.
- **Phase 4 processes (`ep-f29d47`):** stages U0 to U10 are built. All but one
  of the 131 lab scenarios pass. `CURRENT_STAGE` stays at U9 because the PG02
  full-trace gate fails (122x against < 5x, `ts-8f201b`). `ts-1765ba` stays
  open until that is decided. *Update 2026-11-15: Byron decided PG02 (full
  tracing is an ungated debugging mode), so `CURRENT_STAGE` is U10.*
- **Phase 2 (std in Chimi, `ts-0ba63e`):** `std/option`, `int`, `text`,
  `list`, and `order` are Chimi source bound through `native fn` and run on
  both backends. Still to do: `native type`, then `std/map`, `std/set`,
  `std/json`, `std/debug`, and `std/decimal`. Also missing: the grapheme and
  case functions, `--std-dir`, `chimi std check`, and `operations.h`.
- **CI:** `.github/workflows/ci.yml` runs the full workspace tests, the
  feature-gated check, and clippy on macOS and Linux for every push to `main`
  and `wip/**`. The repo is public, so minutes are free. A full macOS run takes
  about 13 minutes. Linux had never been built before 2026-11-14, and its first
  two runs found two real portability bugs.

### Branches in flight

| Branch | State | Next step |
| --- | --- | --- |
| `wip/ci-linux` (`5f028fd`) | Two Linux fixes: `_GNU_SOURCE` in `runtime.c` and `-lm` in the tests that call `cc` directly. Its CI run was still going at handoff | When CI passes on both systems, fast-forward `main` to it |
| `wip/lane-std2` (`e740174`) | Stopped mid-skeleton: the start of `native type` plus execution modules for map, json, set, and decimal. It may not compile | Continue from the std2 brief, or discard and restart from `ci-linux` |
| `wip/lane-fixes` (`e30de0b`) | Stopped early: an unmeasured change making `language.rs` inspect token boundaries instead of every byte | Continue the fixes brief |

Both lane branches build on `ci-linux`, and their worktrees are
`.claude/worktrees/lane-std2` and `.claude/worktrees/lane-fixes`. The workflow
that ran them (`std2-and-fixes`) was stopped and cannot be resumed from another
session. Its script holds the exact briefs: the skeleton, then three parallel
module agents, then a merge, then review beside mutants, then verify. It is
saved outside the repo with the review rules and the bug repro programs, at
`~/.claude/projects/-Users-byronguina-code-chimi/handoff/`.

### Decisions waiting on Byron

Both were decided on 2026-11-15 as recommended; see `docs/decisions.md`.

- **PG02 (`ts-8f201b`).** Recommended: put the 5x gate on `--trace=boundaries`
  (it passes at 1.05x) and document `--trace=full` as an ungated debugging mode.
- **`errors` rows (`ts-936427`).** Two conventions examples write an `errors`
  row in plain function headers, while the std modules spec allows it only in
  `native fn`. Recommended: change the examples. This is less certain, because
  the reason the conventions spec wrote them that way was never checked.

### Open tickets worth knowing

- `ts-ce73fb`: native Text is a NUL-terminated C string, so text with `\u{0}`
  differs between the backends. Fixing it means changing how native Text is
  stored. Repros are in the handoff `probes/` folder.
- `ts-270757` (tuple-pattern closure crashes natively) and `ts-1bba2b` (list
  spread in a loop overflows the compiler stack): from an implementer report,
  not yet reproduced by a coordinator.
- `ts-ab8b7e`: `tests/language.rs` takes about 382 s, nearly all of it in
  per-byte inspection.
- `ts-cc1f03`: the PG05 scaling gate cannot see 2x to 5x regressions.
- `ts-f4e749`: a Receiver handed back to its parent aborts its channel.
- `ts-51bbf7`: may already be fixed by the std lane. Not yet checked.

## How we work

Byron approved each of these rules. The AAR entry each one came from is in
brackets.

- **Roles.** One coordinator delegates, triages, escalates, and integrates. It
  does not implement features. Subagents implement, review, run mutants, and
  verify.
- **Per-branch loop, run as a Workflow** [N25]. Implement, then a mutation run
  and an independent Sonnet review in parallel, then a verifier reproduces or
  refutes each finding [N30]. Add an Opus review for soundness-critical work.
- **Worktrees.** One worktree and branch per lane, under `.claude/worktrees/`.
  Only the coordinator integrates, with `scripts/integrate-branch.sh <branch>
  <scratch>`. That script rebases, runs the tests, compares the outputs of 12
  programs, checks a clean export, fast-forwards, pushes, and removes the
  worktree [N9, N15].
- **Heavy commands go through `scripts/heavy.sh`** [N30]. It holds one lock
  across all worktrees and runs at nice 10 with 3 jobs and 3 test threads,
  because Byron's 8-core laptop overheated with parallel builds. Agents work in
  parallel and only heavy commands take turns. Never nest it.
- **Test what matters** [N31]. Implementers run only the tests that cover what
  they changed. CI and the merge run the full suite.
- **CI gates merges.** Wait for green CI on the branch before the
  fast-forward. Byron's condition: parallel work is never blocked by that wait.
- **Mutation testing.** `scripts/mutation/mutate.py`, once per branch before
  merge. It reuses baselines at the same commit, so `--only` splits are cheap.
- **Push early** [N27]. Push every agent commit to `wip/<topic>`, plain push,
  never force [N29]. Doc-only commits go to `main` right away.
- **Checkpoints.** After each commit, `prog log <ticket> "[checkpoint] done /
  verified / next"`. The coordinator adds a state line to the epic at each
  integration [N23].
- **Briefs** state the scope and any user concern already handled [N28]. They
  also state the token budget and a fallback for design-heavy work, with a
  check-in at half the budget. Stop and rethink after two review rounds in a
  row with new HIGH findings [N20, N26].
- **Review triage.** Fix only findings with a reachable failure scenario, a
  broken contract or test claim, or that carry into code we keep. File tickets
  for the rest, and drop nits [N2].
- **Test bar.** Tests assert observable behaviour (program output,
  diagnostics, outcomes, contractual bounds), never implementation details.
  Each test must reject a plausible wrong implementation. No wall-clock gates
  in the default suite [N18].
- **Records.** `docs/decisions.md` is the only decision ledger [N8]. Add an AAR
  run-log row each time an agent or workflow finishes [N22].
- **Git hygiene.** Agents stage only their own paths, never `git stash` or `-A`
  in a shared checkout, and add no Claude attribution [N4, N11, N14].
- **Standing constraints from Byron.** Ask before evals, installs, or
  machine or agent config changes. Edits to Claude Code or Codex config need
  his direct confirmation. Leave other Chimi sessions alone.

## What is working

- **Independent review plus verification** [W1]. Every serious defect so far
  came from a read-only reviewer, and the verifier filters out false
  positives. The last wave's four reviews and verifiers cost about 290k tokens
  and found 1 real medium bug (NUL text) and 4 lows.
- **Mutation testing as the quality bar** [W2]. It reliably finds missing
  tests: the last std run had 2 survivors, each closed by a new test.
- **Workflows for the per-branch loop.** No orphaned stages and almost no
  notification noise: 0 pings in the first workflow trial, against about 30
  from one hand-run agent [N24].
- **Parallel lanes in worktrees behind the heavy-command lock.** Agents in the last
  wave rarely waited on the lock (about one wait each), and review beside mutants cut the check phase
  from 25 to 30 minutes to about 10 minutes per lane.
- **The integration script.** 22 merges, each with a clean-export check and
  program-output comparison, and no merge has had to be reverted.
- **Compiling every doc example** [W6]. It keeps the specs and the compiler
  honest with each other.
- **CI.** The first two Linux runs found two portability bugs that local work
  could never see.

## What is not working, or is unproven

- **Wall-clock time per wave** [N31]. The last wave took 99 minutes and 1.19M
  tokens. Half of implementer time was full test runs, now replaced by targeted
  tests plus CI, and the other half was one agent doing a long serial task. The
  stopped std2 brief tests the fix: a short skeleton, then parallel module
  agents. That remains unmeasured.
- **The test suite is slow** (`ts-ab8b7e`). The merge still runs the 6-minute
  `language.rs`.
- **Semantic drift in parallel spec work** [N1]. It is fixed by assigning one
  owner spec per concept before a fan-out, but that fix has not been tested
  since.
- **Coordinator context pressure** [N7]. Agent reports are long, so ask for
  short structured results with details in a file.
- **Design work that does not converge** [N20, N26]. The capability checker
  took about 1.4M tokens over three rounds. Budgets and fallbacks are now in
  the briefs, but have not been tested since.
- **Open hypotheses** in the AAR: whether published conventions prevent spec
  conflicts, and whether mutation-checked briefs reduce "tests would not catch
  it" findings.

## Where things are

| What | Where |
| --- | --- |
| Plan and specs | `docs/plans/`, `docs/specification/` |
| Decisions | `docs/decisions.md` |
| Process evidence | `docs/process/orchestration-aar.md` |
| Integration, lock, checks | `scripts/integrate-branch.sh`, `scripts/heavy.sh`, `scripts/check-head.sh` |
| Mutation runner and lists | `scripts/mutation/` |
| Process conformance lab | `compiler/chimi/src/process_lab/`, `compiler/chimi/tests/process-lab/` |
| Std sources and tests | `compiler/chimi/std/`, `compiler/chimi/std-tests/`, `compiler/chimi/tests/std_modules.rs` |
| Workflow scripts, review rules, repros (local only) | `~/.claude/projects/-Users-byronguina-code-chimi/handoff/` |
| Tickets | `prog show ep-a6f138`, `prog ready -p chimi` |
