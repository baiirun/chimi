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

## Open hypotheses

- Publishing cross-cutting conventions before a fan-out will cut spec conflicts
  by most of N1. Test: Phase 2 standard-library fan-out.
- Requiring mutation checks in implementer briefs will remove most review
  findings of the "tests would not catch it" kind. Test: next implementers.
- Worktrees plus a single integrator will raise compiler throughput without new
  conflict classes. Test: Phase 1 after the switch.
