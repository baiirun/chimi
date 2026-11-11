# Orchestration After-Action Review

## Purpose

A running record of how the multi-agent build of Chimi is working, kept as data
for designing future agent orchestration. Each entry states what happened, the
evidence, the rationale for why it worked or failed, and what changed as a
result. Newest entries go at the bottom of each section; the run log is
append-only.

## Setup

- One coordinator (the main session) owns delegation, alignment, triage, and
  integration. It does not implement features itself.
- Background subagents own disjoint files on `main`. The compiler core is one
  lane; since 2026-11-11 its implementers may use worktrees that the lane
  coordinator merges.
- Read-only reviewers and auditors sweep each checkpoint. Findings are triaged
  by the coordinator and routed to the owning agent.
- State lives in `prog` (work queue, status, dependencies), specs (normative
  content), `docs/decisions.md` (decision ledger), and coordinator memory.
- Pushes happen after `scripts/check-head.sh` verifies a clean export of the
  commit.

## Dimensions tracked

Problems are recorded only when observed, against these dimensions:

| Dimension | Measure | Evidence so far |
| --- | --- | --- |
| Correctness | defects found after an agent reported green; mutants surviving | crash after `stop`, false doc claim, 2 surviving mutants in the heap, 19 spec conflicts |
| Safety | memory-safety findings; destructive git or data loss events | none in code; one shared-checkout merge needed manual save and reapply |
| Efficiency | tokens per landed feature; coordination vs construction share | ~1.2M tokens of spec and reconciliation before most Phase 1 features |
| Fidelity to user intent | user reversals; decisions made on the user's behalf and not reviewed | proposals for struct settings and labels reversed; ~30 coordinator decisions in the ledger |
| User attention | questions per phase; confusion moments; reply length | "wdym", "kinda confused", "did you answer my q" (twice) |
| Durability | unpushed commits; uncommitted work at risk; context loss recovered | ~40 commits unpushed (remote diverged); one context summary survived via memory files |
| Throughput | features landed per hour; serialized lanes | Phase 1 serialized through one lane until worktrees were allowed |

## What is working

| # | Practice | Evidence | Rationale |
| --- | --- | --- | --- |
| W1 | Read-only reviewers after each checkpoint | 5 reviews/audits at 95–240k tokens each found every serious defect so far: a crash after `stop`, a false mutation claim in an architecture doc, test gaps proven by mutants, 19 cross-spec conflicts | Implementers report "green" with high confidence; independent readers who try to break the work find what self-checks miss |
| W2 | Mutation checks as the quality bar | 30/30 prototype mutants caught after reviewers demanded it; two of three implementers only adopted it after a reviewer showed gaps | Turns "tests pass" into "tests would catch a regression"; cheap relative to the defects it finds |
| W3 | Strict path ownership in one checkout | ~12 concurrent agents, zero edit collisions | Agents stage and commit only their own paths |
| W4 | Cheap coordinator interventions mid-flight | Per-spec diagnostic code ranges prevented collisions; forwarding "rest slices must be O(1)" changed the list design before it shipped | One short message costs far less than a later fix |
| W5 | Agents decide minor details and list open questions | ~25 open questions raised; 1 needed the user (pipe rule) | Escalating everything would stall the run; the philosophy doc plus recorded decisions resolve most questions |
| W6 | Compiling every doc example | First run: 74 of 100 blocks failed; surfaced 4 real compiler bugs including a type-checker panic | Specs drift silently unless a test ties them to the compiler |

## What is not working

| # | Problem | Evidence | Root cause | Change made |
| --- | --- | --- | --- | --- |
| N1 | Parallel spec writing produced semantic conflicts | 19 cross-spec conflicts (constructor spelling, constants, rendering, root needs, guard purity); ~450k tokens to find and fix | Work was partitioned by file, not by concept; no owner for cross-cutting concepts and no shared conventions before fan-out | Coordinator now assigns one owning spec per cross-cutting concept, publishes the ledger first, forwards decisions mid-flight, checks early drafts |
| N2 | Reviewers over-report | About a third of early findings were nits, theoretical (Miri-only) issues, or polish on a disposable prototype, and were forwarded | Prompts asked for "confident findings" plus simplification ideas | Every finding must name a reachable failure scenario, a broken contract, or a test gap; coordinator triages before forwarding |
| N3 | No visibility into running agents | Agents report only at the end; the compiler lane was resumed several times by coordinator messages without reporting | No checkpoint reporting in briefs | Coordinator checks commits and `prog` activity at each notification; next: require checkpoint `prog log` entries |
| N4 | Shared checkout leaks state | A commit could pass only because of another agent's uncommitted change; another session merged PRs on GitHub so `main` and `origin` diverged and pushing blocked | Many writers in one working tree plus an external writer to the remote | `scripts/check-head.sh` tests a clean export; integration merge delegated to the lane at a quiet point; worktrees allowed for compiler work |
| N5 | Serial compiler lane | All Phase 1 features queued behind one agent | Every feature touches the same compiler files and worktrees were disallowed | Worktrees allowed; lane coordinator merges 2–3 features in parallel |
| N6 | Tooling friction | Subagents cannot load the `code-quality` skill; every brief must name its sub-skills | Skill is user-invocable only | Workaround in briefs; candidate fix: make it model-invocable |
| N7 | Coordinator context pressure | Final reports run 2–5k tokens each; context fills quickly | Reports carry full detail inline | Candidate: cap reports at a short summary plus a file path for details |
| N8 | Decisions recorded in four places | `prog` logs, specs, `docs/decisions.md`, coordinator memory | No single owner for decision records | Ledger is the single list; `prog` logs should point to it |
| N9 | Worktree agents cannot integrate their own work | The spec steward's sandbox refused every git command aimed at the main checkout, so it could not fast-forward `main`; its auto-created worktree also started from a stale base (`419d1e5` plus pre-squash PR copies), not `main` | Worktree isolation is enforced by the harness, and the worktree base follows the remote default branch, not local `main` | Coordinator integrates worktree branches; briefs tell worktree agents to reset to local `main` first |
| N10 | Integrating into a dirty shared checkout collides with in-flight edits | Fast-forwarding the steward's spec changes refused because the compiler lane had an uncommitted 6-line edit in the same spec; the coordinator saved, merged, and reapplied it by hand, resolving one conflict | Agents leave work uncommitted between checkpoints in a tree others integrate into | Ask lanes to commit small and often; coordinator merges only when touched files are clean, otherwise saves and reapplies the in-flight diff |
| N11 | Staging during another agent's merge joins that merge | The coordinator's `git add` of the AAR during the lane's `origin/main` merge staged it into the pending merge commit; caught and unstaged before the commit. Other agents' commits fail while `MERGE_HEAD` exists | A shared index: any writer's `git add` lands in whatever commit is next, including someone else's merge | Agents check for `MERGE_HEAD` before staging; mergers stage only merge files and verify `git diff --cached` before committing |
| N12 | Usage limit stopped every agent at once | All running agents (lane coordinator, two worktree implementers, tools importer) terminated mid-step at the 5-hour limit; work survived in worktrees and the checkout, and agents were resumed by message after reset | Many concurrent long-running agents share one usage budget, so parallelism converts directly into hitting the cap | Resume by message, never restart; consider budgeting concurrency against the remaining window |
| N13 | An agent closed its ticket before coordinator decisions on its open questions were implemented | ts-3d5b38 marked done at commit time; decisions logged one minute later were never built; found only by the independent review | Agent reports open questions and finishes; nothing holds the ticket open for the answers | Tickets with open questions stay in progress until the coordinator answers; reviewer checks logged decisions against code |
| N14 | An agent ran `git stash` in the shared checkout | The nested-supervisor agent's stash/pop briefly took another lane's uncommitted `standard-library-map.md` edit; the pop restored it | Whole-tree git commands act on every agent's uncommitted work, not just the caller's | Rule: never `stash`, `reset`, `checkout .`, or `merge --abort` in the shared checkout |
| N15 | The lane coordinator loses git access to main | Each time a resumed worktree implementer reports, the coordinator's shell is moved into that worktree and the sandbox blocks git on the main checkout; merges stalled until the main coordinator ran them | Worktree isolation follows the session's working directory | The main coordinator is the single integrator; lanes send merge requests |
| N16 | A shell-quoting slip caused failed pushes that were misreported as transient | `$R:refs/heads/main` in zsh applies the `:r` modifier, mangling the ref; reported to the user as a transient failure before the cause was found | Unbraced variable followed by `:` in zsh | Always `"${R}:refs/heads/main"`; corrected the report |
| N17 | Hand-written integration pipelines hid failing steps | A merge command piped `cargo test` through `grep`, so a failing test would not have stopped the fast-forward; caught and killed before merging. A doc commit landed while the doc-example test still failed for the same reason | Exit statuses lost in pipelines and `&&` chains written ad hoc each time | `scripts/integrate-branch.sh` runs every integration step fail-fast with real exit statuses |
| N18 | Timing gates in the default test suite flake under agent load | The clean-export check failed once on a concurrency-lab trace-overhead gate (2.08x); three reruns passed; load average 20-32 from parallel builds | Wall-clock thresholds assume a quiet machine; parallel agents make it noisy | The script stopped the push correctly; timing gates move out of the default suite into an explicit perf run (lane ticket) |

## Run log

Agent-level data, one row per completed agent. Tokens and durations are from
task notifications.

| Agent | Kind | Tokens | Minutes | Outcome | Rework caused or found |
| --- | --- | ---: | ---: | --- | --- |
| Positional construction | implement | 162k | 12.4 | 115 call sites migrated, green | none |
| Patterns spec | spec | 151k | 9.2 | spec committed | part of N1 |
| Std mechanism spec | spec | 156k | 10.3 | spec committed | part of N1 |
| Core data spec | spec | 158k | 11.0 | spec committed | part of N1 |
| Time and outcomes spec | spec | 196k | 13.8 | spec committed; raised pipe rule | part of N1 |
| Persistent list | implement | 161k | 14.4 | O(1) append and slice, green | 1 test gap found later |
| Runtime prototype | implement | 198k | 17.0 | 19 tests green | crash + false doc claim found later |
| Heap review | review | 94k | 8.4 | no memory bugs; 2 test gaps proven by mutants | — |
| List review | review | 107k | 6.1 | no bugs; 1 untested path | — |
| Kernel review | review | 152k | 7.2 | 1 crash, 1 false claim, mapping gaps | — |
| Conformance lab design | spec | 177k | 11.3 | 131 scenarios, perf gates | — |
| Doc-example harness | tooling | 160k | 9.7 | harness green; 4 compiler bugs found | — |
| Test audit | review | 205k | 6.8 | ~90% behaviour-focused; key missing tests | — |
| Prototype fixes | fix | 217k | 16.7 | 26 tests, 30/30 mutants caught | — |
| Cross-spec review | review | 238k | 6.9 | 19 conflicts | — |
| Spec steward (worktree) | fix | 130k | 8.0 | 19 resolutions, doc examples green | could not self-merge (N9); merge conflict with lane edit (N10) |
| Scheduler kernel port | implement | 385k | 36.2 | 84 chimi-fiber tests green (verified by coordinator rerun); 20 mutants caught; reported a latent use-after-free in existing code | review found 2 P1 (UAF reproduced as SIGSEGV; shutdown leaves wake sources armed), 3 P2, 1 surviving claimed mutant; ticket closed before logged decisions were implemented |
| Scheduler kernel review | review | 177k | 9.5 | 7 confirmed findings with repro tests | — |
| Verification tools import | tooling | 146k | 4.3 (after reset) | list fuzz, heap stress, mutation runner, program parity in repo; runner found 3 surviving scheduler mutants on first use | — |
| Nested supervisors + structured stop | implement | 323k | 21.3 | 105 fiber tests, 28/28 mutants killed; fixed a pre-existing panic | review found 2 HIGH bugs (leaked replacement supervisor; draining-owner hang) and 3 untested claims |
| Nested supervisor review (Sonnet) | review | 118k | 6.2 | 2 reachable HIGH bugs reproduced as tests, 3 surviving mutants of claimed guarantees, no noise findings | first Sonnet data point: comparable signal to Opus reviews on this task |
| Scheduler review fixes | fix | 288k | 89.8 | 9 findings fixed, each test red before fix; 95 tests; 17/17 mutants killed (coordinator verified tests and results file) | — |

## Is parallel faster than one agent?

Assessment at 2026-11-11, after about 20 completed agents:

- Elapsed time: roughly 5.2 agent-hours of non-lane work (plus the long-running
  compiler lane) landed in a few wall-clock hours, an estimated 2-3x speedup.
  Soft: start and end times were not recorded precisely.
- Tokens: about 4M+ across agents; a single agent would have spent less, with no
  briefs, reports, or the ~450k-token spec reconciliation.
- The larger benefits were not speed: context isolation (a single session could
  not hold a 5k-line port or 1.1k-line specs and still coordinate) and
  independent review (every serious defect came from an agent that did not write
  the code).
- Costs: coordination effort, conflicts created by splitting shared design, the
  usage cap reached faster and stopping all agents at once, late visibility.
- Working conclusion: parallelize implementation and review; keep cross-cutting
  design in one owner; size concurrency to the remaining usage window.

## Open hypotheses

- Publishing cross-cutting conventions before a fan-out will cut spec conflicts
  by most of N1. Test: Phase 2 standard-library fan-out.
- Requiring mutation checks in implementer briefs will remove most review
  findings of the "tests would not catch it" kind. Test: next implementers.
- Worktrees plus a single integrator will raise compiler throughput without new
  conflict classes. Test: Phase 1 after the switch.
