# Orchestration After-Action Review

## Purpose

A running record of how the multi-agent build of Chimi is working, kept as data
for designing future agent orchestration. Each entry states what happened, the
evidence, the rationale for why it worked or failed, and what changed as a
result. Newest entries go at the bottom of each section; the run log is
append-only.

## Setup

This describes the setup when the AAR began (local sessions on Byron's
laptop, tickets in `prog`). The current procedure is in
[procedure.md](procedure.md).

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
| N19 | Sub-coordinators end mid-stage and orphan their implementers | Three lane coordinators in a row handed back while their implementers were still running (the harness ends an agent that has nothing to do but wait); implementer reports had to be re-routed to the main coordinator by message | A coordinator agent cannot idle while waiting for children; long-lived coordination only persists in the main session | Lane coordinators write a HANDOFF.md and tell implementers to report to the main coordinator; the main coordinator owns waiting, review, and integration |
| N20 | An agent kept polishing a design that wasn't converging | The cross-function capability checker went through three review rounds (Sonnet+Opus each time); every round fixed the previous findings and surfaced new HIGH ones (compiler hang, binary-tree walks rejected, aliasing through generic wrappers). ~1M implementer tokens plus ~400k review tokens | Each fix added mechanism (summaries, path automata, generic requirements), and each mechanism added edge cases; nobody stepped back to ask whether the approach fit the budget | Coordinator escalated after round three; user chose to simplify (keep the within-function rule, rely on the runtime owner check, archive the branch). Rule: if two consecutive review rounds of the same branch both find new HIGH issues, stop and reconsider the approach before another fix round |
| N21 | Finished reviewers kept re-sending their report | A review agent with leftover background work woke repeatedly and re-delivered the same report four times | Agents with pending background jobs resume when those jobs end | Stop review agents once their report is received |
| N22 | The run log stopped being kept | No rows after the Phase 1 scheduler fixes: process stages U0–U9, the three capability rounds, and the paired review are missing, though they were the costliest work | Rows were batched for later instead of written when each agent finished | Write the row in the same step that stops the finished agent; workflow runs return per-agent tokens and minutes to copy in |
| N23 | Work-in-progress state lived only in agent and coordinator context | At Byron's pause (2026-11-12) both implementers were mid final mutation run (U8/U9 had waited ~25 min); resume notes had to be rebuilt from git logs. Correction: the agents had not failed to report, they were stopped by the pause | Progress was recorded only in the final report, and verification is tail-loaded | After every commit an implementer writes `prog log <ticket> "[checkpoint] done / verified / next"`; the coordinator adds one state line to the epic at each integration |
| N24 | Notification noise from agents idling on background work | ~30 "waiting on background work" pings from one agent; 9 leftover polling shells all waiting on one mutation output file | An agent that starts a job in the background and ends its turn wakes, and pings, each time a job finishes | Briefs: run tests and mutation runs in the foreground (split into runs of 10 minutes or less), one shell, never end a turn to wait. A prog message board was considered and rejected: it adds durability, not fewer pings |
| N25 | The fixed per-branch loop was run by hand | N19 (sub-coordinators ending mid-stage), N16/N17 (scripting slips), N12/N18 (ad hoc concurrency hit the cap and loaded the machine) | A subagent cannot block on other agents, and each loop was rebuilt by hand | Byron approved (2026-11-12): the per-branch loop (implement or fix → tests and mutants → review, with Sonnet+Opus for soundness-critical work → verify findings) runs as a Workflow, which awaits each agent, with at most 3 heavy agents at once. The coordinator keeps triage, escalation, and integration (`integrate-branch.sh`). Workflow resume works only within one session, so prog stays the record across sessions |
| N26 | Design-heavy work escalated too late | Extends N20: ~1.4M tokens over three rounds before escalation | No budget or fallback was set up front, so there was no point at which to stop | Briefs for design-heavy work state a token budget and the fallback approach; the coordinator checks in with Byron at 50% of the budget |
| N27 | Unpushed work lived only on one disk | On 2026-11-12 the checkout went to the Trash, most likely during an app-uninstall cleanup. 9 commits on two agent branches, the archive branch, and an AAR commit existed nowhere else; restored from the Trash | Agent branches were never pushed, and doc commits waited on a full test run before pushing | Push each agent branch to `wip/<topic>` on origin at every checkpoint; push doc-only commits to main right away; the full clean-export check still gates code commits on main |
| N28 | Workflow agents stood down because of an earlier user remark | The U8/U9 finish, review, and verify agents (first workflow trial, 2026-11-13) all declined to build or test because Byron had said "my computer fans are CRANKING"; ~3 agents' tokens bought static reads only. The capability branch, in the same run, did its work | Workflow agents see the conversation, read a remark the coordinator had already acted on (nice limits, one agent at a time) as a standing instruction, and chose to obey it | Briefs say when a user concern has already been handled and how, so agents proceed under the stated limits |
| N29 | A subagent's force push of a rebased wip branch was blocked | U8/U9 finish agent: the permission check refused `push --force-with-lease` to `wip/process-u8-u9` (combined with a prog log in one command) | Force pushes need approval even for wip branches | Rebase wip branches only when integrating; otherwise push plain. The coordinator raises any blocked push with Byron rather than retrying it |
| N30 | The serial per-branch loop and repeated mutation baselines throttled throughput | Capability branch: ~2 h from finish to merge, mostly waiting. U8/U9: 41 mutants took ~3 min but 6 per-piece baselines took ~30 min (each reran the whole crate's tests, even for lists whose mutants only run a lab command) | The fan fix serialized whole agents, though only builds and tests load the CPU; mutate.py reran a full-crate baseline on every --only piece | `scripts/heavy.sh` takes a lock shared by all worktrees, so agents work in parallel and only heavy commands take turns (nice 10, 3 jobs, 3 test threads). `mutate.py` baselines only the selections the chosen mutants use and records passes per commit, so split runs share one baseline (two U9 pieces: 8 s and 5 s). Review starts as soon as code is committed, in parallel with tests. Mutants run once per branch before merge, and are rerun after a rebase only if it changed a mutated file |
| N31 | Implementers spent half their time in full test runs | U10/std wave: std implementer 88 min, of which 44 min in `cargo test` and 1 lock wait; U10 implementer 23 of 42 min in tests and lab runs (from agent transcripts). `tests/language.rs` takes 382 s; one test takes 73 s alone | Implementers reran the whole suite while iterating. In the suite, `run_both` (172 tests) runs a full `inspect_project` at every byte offset: 1,362 offsets x ~55 ms debug = ~70 s for one program, heavier now that analysis loads imported std modules | Byron (2026-11-14): implementers run only the tests that cover what they changed; the full suite runs once, in `integrate-branch.sh`. The per-offset inspection cost is ticketed |
| N32 | The native backend had never been built on Linux | First CI run (2026-11-14): runtime.c failed to compile on Ubuntu (glibc hides open_memstream, strdup, clock_gettime under -std=c11); second run: 3 cli tests and the list fuzz test failed to link without -lm | All work ran on one macOS laptop; nothing exercised the Linux code paths the runtime claims | GitHub Actions CI on macOS and Linux for main and wip/*; fixes on `wip/ci-linux` (`_GNU_SOURCE` on Linux, `-lm` in direct cc tests). Merges wait for green CI without blocking parallel work |

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
| Phase 1 perf sweep (Sonnet) | perf | 146k | 30.9 | re-measured at low load after discarding a noisy pass; found a 2-6x semantic-pipeline regression and bisected it to a commit range; 2nd Sonnet data point, good signal | — |
| Scheduler review fixes | fix | 288k | 89.8 | 9 findings fixed, each test red before fix; 95 tests; 17/17 mutants killed (coordinator verified tests and results file) | — |
| Workflow trial: paused branches (7 agents, one at a time) | workflow | 524k | 44.3 | Capability: 4 runtime-backstop tests and 1 archive-lookup fix; 8/8 + 54/54 mutants killed; every cross-function route reviewers found is caught at compile time or defects at run time; Sonnet and Opus reviews plus verifier confirmed 1 medium (a Receiver handed back to its parent aborts the channel) and 4 low. U8/U9: no work done (N28) | First workflow run was stopped after ~4 min: two parallel workspace builds put load at 36 on 8 cores; rerun one agent at a time with nice and job limits. 0 notification pings during the run (vs ~30 from one hand-run agent, N24) |
| Workflow: U10 and std lanes in parallel (8 agents, heavy.sh lock, review beside mutants) | workflow | 1,193k | 99.1 | U10: supervised-pool forcing program, MF02/MF03, PG01-PG10 harness; 130/131 lab scenarios, PG02 full trace fails at 122x (ts-8f201b), 12/12 mutants killed. Std: option/int/text/list/order as Chimi source with `native fn`, both backends, 17/17 mutants killed after 2 new tests. Reviews plus verifiers confirmed 1 medium (native Text stops at NUL, pre-existing, ts-fd4720) and 4 low (2 doc fixes made by coordinator) | Per agent: implement 268k/42 min (U10) and 511k/88 min (std); mutants 57k/9 min and 68k/10 min; Sonnet reviews 111k/9 min and 76k/9 min; verifiers 49k/1 min and 53k/2 min. Check phase took ~10 min per lane with review beside mutants, against ~25-30 min serial before. Implementers found 2 compiler bugs on the side (ts-270757, ts-1bba2b) |
| Std step 2, cloud thread (skeleton, then 5 parallel lanes: debug tooling, decimal, text, map/set, json; each with review, mutants, fixes; ~25 agents) | lanes | 2,690k known (some mutant and review runs unrecorded) | 207 agent-min known; wall clock not recorded | PR #8: native type, std/map, set, decimal, json, debug in Chimi, graphemes and case functions, --std-dir, std check; 57 of 65 recorded mutants killed on the first run, and most survivors were then closed by new tests | Reviews found 2 HIGH (Decimal.parse u128 wraparound on both backends; native Text NUL, known CHI-174), 3 MEDIUM (O(n^2) native graphemes, Json.Number rendering through f64, Map.put replacing the held key), and the merge found a constant-kind collision (Decimal and Set both 18). Lessons: give each parallel mutant run its own scratch dir; a std_modules filter can select no tests; lane worktrees started from main, not the skeleton; the 4-core container OOMs and fills its disk with more than about 4 builds; CI's `stable` Rust moved Unicode to 18.0.0 and broke the vendored tables, so CI now pins 1.97.0 |
| Lane 3 fixes, cloud thread (one session plus a Sonnet review agent, no workflow) | fix | review 135k; session not measured | ~55 | ts-270757 (captures computed before clause aliases), ts-1bba2b (no occurs check: `T = List<T>` overflowed formatting), ts-ce73fb (native Text carries its length; ABI 20). Each test red before its fix. 22/22 mutants killed after dropping one unreachable mutant (Text.equal intrinsic). language.rs at 3 threads on a 4-core cloud box: every byte 524 s, token boundaries (e30de0b) 303 s, every byte from one CompilerSession analysis 105 s | Review found 1 real HIGH (C header ABI left at 19) and 1 known medium (defect messages still cut at U+0000, ticketed); the rest refuted or low. Verification done inline, not by a separate verifier agent. Reproduced ts-1bba2b showed the ticket's cause (parser) was wrong: the bug was in typing |
| Lane 3 dev profile, cloud thread (same session) | build config | not measured | ~20 | `[profile.dev] opt-level = 1` in the root Cargo.toml. Sequential runs on a 4-core cloud box at fb19d63, default vs opt-level 1: cold workspace test build 53 s vs 130 s (mostly dependencies, which CI caches through rust-cache); rebuild after a chimi change 7 s vs 6 s; language.rs at 3 threads 96 s vs 47 s; cargo test --workspace 196 s vs 135 s | Kept as a net win: every run after the first cold build is faster, and the warm rebuild cost is unchanged. The cold build is 77 s slower, paid once per fresh cache |
| Lane B bug backlog, cloud thread (one session plus two Sonnet reviews, no workflow) | fix | reviews 65k and 102k; session not measured | ~110 | 10 tickets. 7 were already fixed on main (CHI-174, 188, 192, 187, 173, 171, 172) and were closed with evidence; that took ~15 min of probing instead of fixes. 3 fixes: CHI-177 outcome maps as key/value pairs (PR #12, 6/6 mutants killed), CHI-158 contract at a function's declared name, CHI-176 per-body native environments (ABI 22; a 2,000-call loop beside a 400-local function went from 16.7 MB to under 2 MB; 4/4 mutants killed after removing 1 equivalent mutant). Each regression test was red before its fix | Both reviews found no defects. Lessons: check `git log --grep=<prog id>` and existing tests before reproducing; a ticket backlog drifts behind fixes that landed under other tickets. The single allowed push branch serialises PRs, so CHI-158 and CHI-176 share one PR to save a CI cycle. One language.rs run on the 4-core box was OOM-killed by three 100,000-level deep-value tests at once |
| Lane D testing spec, cloud thread (one session plus a Sonnet review agent) | spec | review 119k; session not measured | ~25 | testing.md draft (T1, G16) with 4 questions for Byron batched on one card, which he answered within minutes (all four recommendations); CHI-109 finished (last 17 unannotated doc blocks annotated, manifest emptied); Phase 5 build ticket CHI-208 filed | Sonnet review found 10 items, checked inline against the cited lines: 9 real and fixed (wrong equatable code T4301 for T4300; an example that T4349 rejects; recorders handing out exclusive values twice; imports inside example bodies; codes inside another spec's block; a cancellation case that cannot occur; the program `random` plan unrecorded; mailbox type and rendering clashes; flag spelling). Refuted 1: `capture_defect` needs no escalation, because it is test-only and programs still cannot observe defects |
| Lane P runtime fixes, cloud thread (one session, a Sonnet review agent, a Sonnet verifier) | fix | review 143k, verifier 87k; session not measured | ~140 | PR #15: CHI-185 (a Receiver or Completion moves with the message or task result that carries it, with in-flight, handed-back and discarded states), CHI-184 (Channel.SendError follows its payload's capability class), CHI-183 (process defects named in the root outcome with tracing off; fiber ABI 7; lab OB08), CHI-190 (PG05 gates marginal per-process cost from medians; injected 3x scored 3.21 and failed; 4 clean release runs scored 1.00 to 1.54), Supervisor time row, T4263 status. Each bug test red before its fix. Mutants: the capability-boundary list 9/9 killed; the Lane P list killed all but one stale and one survivor on the first run, and both were closed. Follow-ups CHI-213 and CHI-214 | Review found no confirmed bug and 5 possible issues: the verifier refuted 3, confirmed 1 as a leak only (CHI-214), and found PG05 noise real (a clean run at 1.64 against 1.75), which the switch to medians fixed. Review test gaps closed with 8 registry-level parcel tests and ABI layout checks. Lessons: never run `cargo fmt --all` here, because the repo is not fmt-clean; a usage-error test must use a readable input, or it passes for the wrong reason
| Procedure doc, cloud thread (one session plus a Sonnet review agent) | docs | not measured | ~15 | docs/process/procedure.md written from the wave rules, the handoff and current practice; handoff.md and the AAR setup marked historical and linked to it | — |
| Phase 6a bytes and embedding, cloud thread (one session plus two Sonnet reviews) | lanes | reviews 88k and 66k; session not measured | ~140 | PR #16: std/bytes on both backends (Bytes type, 12-layout read_int/from_int, strict and lossy UTF-8; ABI 22); 24/24 mutants killed after 1 new test and 1 stale mutant fixed. PR #21: std/embed (Embed.text/bytes in constants, E5206-E5209), mutants in embed_mutations.py | Bytes review found nothing reachable (1 low: native Bytes buffers lacked a trailing NUL, fixed). Embed review found 2 real holes, both fixed with tests: Embed used as a value skipped the build-time check, and a symbolic link could escape the module directory. Lessons: a Python anchor that matches a prototype and a definition needs an index, not a replace; a native rebuild test must clear its telemetry files between runs |
| Lane K clock, timeouts and scopes, cloud thread (one session; Fable advisor on the design, Sonnet and Opus reviews, Opus verifier) | lanes | Sonnet review 126k, Opus review 177k; session not measured | Sonnet 4.7, Opus 13.4; session ~150 | PR #17 (CHI-201): Instant, Timestamp, structural Clock, --clock-start, Task.timeout/deadline, cancellation scopes in the scheduler. 16 tests in tests/clock.rs; 15/15 mutants killed after two fixes to the list (one survivor needed a real-clock test). | The advisor found 8 design problems before any code was written. Sonnet found a real-clock hang: a busy loop never timed out because the scheduler fired timers only between tasks. Opus found the same hang, plus a leak: a forced process inside a timeout was abandoned mid-unwind. Both were fixed with regression tests. |
| Lane T failure-set types and std/result, cloud thread (one session; Sonnet and Opus reviews, Opus verifier) | lanes | Opus review 110k, verifier 130k; session not measured | Opus review 9, verifier 16; session ~200 | PR #23 (CHI-200): `A \| B` failure-set types in type positions and after `errors`, the value projection into type arguments, subset fitting and union joins, open sets for failure-set generics, T4340/T4341/T4346, and std/result (`capture`, `get`) in Chimi. 21/21 mutants killed after 4 first-run survivors got new tests. Follow-ups CHI-217, CHI-218, CHI-222 | The reviews found 3 soundness holes, each a program that type-checked and then hit a no-matching-arm defect: callbacks were checked in the wrong direction, invariant arguments such as `Sender<A>` passed for `Sender<A \| B>`, and a set already checked as narrow could widen later. All 3 were fixed with regression tests. The verifier found no unsound program, but caught nested joins leaking `Result<T, E>`, which was fixed. Lesson: subtyping needs variance from the start; adding it after the joins worked cost a second review round |
| Phase 8 performance lane, cloud thread (one session, Sonnet and Opus review agents, no workflow) | implement | reviews: cold build Sonnet 62k, R2 Sonnet 86k and Opus 127k, R3 Opus 97k, T6 Sonnet 77k; session not measured | ~330 | PR #14 cold build note (Linux quiet-machine check). PR #22 R2 part 1: 10 to 7 allocations per loop pass (lazy local clones, stack match subjects, shared Void/True/False). PR #24 R3: pure integer loops compiled as Cranelift passes, about 2 ns a pass against 120 ns, with defects handed back to the runtime loop so they match the interpreter. T6: `--profile-file` operation profile. Mutants killed after dropping equivalent ones | R3 Opus review found one HIGH (a loop whose state was not an Int defected instead of falling back), fixed with a test before merge. T6 Sonnet review found no bug, but a profile with benchmark repeats added up the counts; now rejected. Lessons: old commits cannot build native programs on Linux without forcing the per-project build path; rustfmt splitting a string makes a mutant stale, so rerun stale mutants after formatting |
| Select lane, cloud thread (one session, interrupted once by a service error; Sonnet and Opus reviews, Opus verifier) | lanes | Sonnet review 86k, Opus review 172k, verifier 105k; session not measured | Sonnet 0.9, Opus review 8.6, verifier 7.3; session ~240 | PR #29 (CHI-203): CHIMI_REQUEST_SELECT (fiber ABI 10 after merging G27), std/select on the native backend (runtime ABI 27), checker contracts, T4345. 8 scheduler tests and 22 end-to-end tests; 24/24 mutants killed after the first pass's 7 survivors were covered by new tests or recorded as equivalent. | Sonnet found OB07 still expected fiber ABI 8 and weak task-locality asserts. Opus found two concurrency bugs: a woken select refused by a fired scope kept its watchers, and cancelling user-clock timers before the take broke atomicity. The verifier found two more user-clock timer bugs (a due timer lost list order; a failed timer start cancelled task 0). A first mutation pass filled the disk: a livelocking mutant wrote 9 GB of telemetry, so native test runs now have a 15 s limit. All fixed with failing-first tests. |
| G11 generic interface members, cloud thread (one session; Fable advisor, Sonnet and Opus review agents; the lead verified findings) | lanes | advisor 71k, Sonnet review 77k, Opus review 92k; session not measured | advisor 2, Sonnet 1, Opus 8; session ~150 including a 2-hour API outage | PR #31 (CHI-104): `fn<A>(..)` as an interface member's type, T4360 elsewhere, rigid conformance for providers and record updates, member generics in value, failure and needs positions. A user fake now stands in for `FileSystem`. 12/12 mutants killed after one equivalent mutant was dropped | The advisor's design check found 4 soundness traps before any code (interface generics not substituted into members, requirements slipping past, row-blind escape checks, the shared fitting path). Testing found a record-update hole. The Opus review found 2 HIGH: a join that kept a generic type while holding a fixed function (a hole already on main for plain generic functions) and needs-row member generics out of scope. All fixed with tests. Lesson: an advisor design pass on soundness work is cheap and paid for itself; the remaining holes were in old paths the new feature reached |
| G27 process monitors and metrics, cloud thread (one session; Sonnet review, Opus review) | lanes | Opus review 120k; Sonnet review and session not measured | Opus review 7; session ~280 | PR #26 (CHI-220): `Process.address`, `queued`, `capacity`, `monitor`, `unmonitor`, and `Down`, with notices queued in the watcher's own inbox past the capacity. Links skipped by decision. Lab stage U11 (SF27, SF28, MB12-MB18). 11/11 mutants killed on the first run. Follow-up CHI-223 | The Opus review found one soundness hole and reproduced it: the notice closure was not moved into the monitor, so a notice capturing a `Receiver` could hand out a second live copy. Fixed by treating it like `Process.send`, with SF28. The PR sat idle about 2 hours because main moved twice and took its ABI numbers; GitHub runs no PR CI while a branch conflicts, so no failure event arrived. Lesson: after a sibling lane merges, check your PR for conflicts instead of waiting on CI events |
| CHI-204 failure causes, cloud thread (one session; Fable advisor on the design, Sonnet and Opus reviews; the session reproduced each finding before fixing it) | lanes | Sonnet review 147k, Opus review 150k; session not measured | Sonnet 3.5, Opus 9.3; session ~240 (one service-error restart) | PR #33: causes beside every outcome on both backends, `std/cause`, the chain rule for `Failure` and `Err` arms, stored `Err` causes, bounded chains, and a `cause` field on detailed root records (ABI 26). 18/18 mutants killed after 1 new test. Sonnet found 2 real bugs (a nested `case` lost causes; a NUL cut the native report). Opus ran ASan and found a use-after-free of `Debug.crash` messages left in loop heaps (older, but the new cause reader exposed it). All 3 fixed with regression tests; the rest went to CHI-226 | 3 reproduced bugs fixed before merge |

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

## Milestone: Phase 1 complete (2026-11-11)

- All eight Phase 1 features, fix batches A-K, the runtime heap port, and the
  scheduler-side process kernel are on `origin/main` (65172a6). The forcing
  program (trace-summary rewritten idiomatically) produces identical outputs.
- Integration: 11 worktree branches merged through `scripts/integrate-branch.sh`;
  4 needed conflict resolution, 2 of them delegated to a rebase agent; the script
  stopped 4 times correctly (two flaky timing gates, one renumbering-only parity
  difference, one fast-forward race) and never let a red commit reach origin.
- Three usage-limit stops; every agent resumed by message with no lost work.
- Hypothesis check (worktrees + single integrator raise throughput without new
  conflict classes): throughput rose (2-3 features in flight), but a new class
  appeared: semantic clashes between independently correct branches (tuples vs
  inferred contracts, tuple types vs record types, Never vs call matching). Each
  was found only because rebases ran the full suite; rebase agents pinned each
  with a test.

## Milestone: processes run natively (2026-11-12)

- Stages U0-U5 of the supervised-process lab are on main (7df2f24): Chimi
  `process` programs build and run natively on the scheduler kernel, with
  per-incarnation heaps, copy-on-transfer messages, trace family 3, and 65 lab
  scenarios passing. Every stage branch was reviewed independently (Sonnet)
  before merge; reviews found a crash path, a stack-depth limit of ~150 frames,
  untested release paths, a mutation tool that scored compile failures as kills,
  and a pre-existing capability aliasing hole (since fixed, ts-184972).
- Process lesson: the main coordinator now runs the review/fix/integrate loop
  directly (N19); implementers report to it. Throughput held with three
  implementers at a time.

## Paired review: Sonnet vs Opus (2026-11-12)

Same prompt, same branch (capability per-place moves, 495b56e), separate scratch
directories, run concurrently.

| | Sonnet | Opus |
| --- | --- | --- |
| Tokens | 74k | 106k |
| Wall time | 3.3 min | 11.5 min |
| Programs probed | ~35 | ~40, with a both-builds comparison script |
| HIGH | 0 | 1: two tasks can own one receiver through a loop's next state (a regression main rejected, plus an older hole on both) |
| MEDIUM | 2: generic helpers like `dup<T>` rejected; record update replacing a moved field rejected | 2: recursive worker-pool shape rejected by the depth cut; no test pins the claimed depth (any cut 1-4 passes) |
| LOW | 1: depth cut on 5-level structs | 2: depth cut merges tuple positions; corpus claim cannot cover generics |
| Overlap | generic-helper break, depth-cut imprecision | same |
| Unique | record-update false rejection | loop-state aliasing (the only soundness hole found), recursive-pool rejection, depth test gap |

Reading: both were useful and neither found everything. Opus found the one
soundness hole and the most consequential false rejection; Sonnet found a real
false rejection Opus missed, at about a third of the time and tokens. Neither
produced noise. For soundness-critical changes (type and capability rules,
runtime memory), run both; for routine branches, Sonnet alone.

## Open hypotheses

- Publishing cross-cutting conventions before a fan-out will cut spec conflicts
  by most of N1. Test: Phase 2 standard-library fan-out.
- Requiring mutation checks in implementer briefs will remove most review
  findings of the "tests would not catch it" kind. Test: next implementers.
- Worktrees plus a single integrator will raise compiler throughput without new
  conflict classes. Result so far: held. U0–U7 merged through
  `integrate-branch.sh` with only ordinary rebase conflicts.
- Running the per-branch loop as a Workflow (N25) removes orphaned stages and
  most notification pings at similar token cost. Test: the two branches paused
  on 2026-11-12 (process U8/U9, capability simplification), compared with the
  hand-run reviews on tokens, minutes, pings, and confirmed findings.
