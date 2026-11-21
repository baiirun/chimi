# Multi-Carrier Baseline

This record holds the first measurements of programs on several carriers
(parallel-scheduler.md, step 3 PR 6a), to inform the choice of the default
carrier count. It is a local measurement on one cloud host, not a
cross-machine promise. The single-carrier numbers are in
[the supervised-process baseline](supervised-process-conformance-baseline.md).

## Run

- Date: 2026-11-21
- Commit: `a8e8e74` plus this pull request (`take_mark_epoch`,
  `take_spans` and the lab's `--carriers`).
- Host: a cloud container, Linux 6.18 (Firecracker), 4 vCPUs of an Intel
  Xeon at 2.30 GHz, 15 GiB of memory.
- Toolchain: rustc 1.97.0, gcc 13.3.0.
- Command, from the repository root with release binaries built
  (`cargo build --release -p chimi --bin chimi --bin chimi-process-lab`):

```sh
scripts/measure_carriers.py --chimi target/release/chimi \
  --lab target/release/chimi-process-lab --carriers 1,2,4,8
```

## Results

Times are medians of 7 runs after one unmeasured run (the lab's gates keep
their own method: the p95 of 7). Every run ended with its expected outcome.

| Measurement | 1 carrier | 2 | 4 | 8 |
| --- | ---: | ---: | ---: | ---: |
| Parallel counting, 8 processes (ms) | 2,381 | 1,234 | 618 | 590 |
| Parallel list building, 8 processes (ms) | 2,749 | 1,429 | 768 | 753 |
| Host ceiling: n one-carrier copies, 1/n of the counting each (ms) | 2,394 | 1,262 | 613 | 612 |
| Serial round trip, main to two echo processes (µs) | 3.7 | 21.7 | 39.1 | 34.4 |
| PG04 µs per start (limit 50) | 25.5 | 37.8 | 48.5 | 44.6 |
| PG04 µs per stop (limit 30) | 15.4 | 49.3 | 65.5 | 64.6 |
| PG05 cost per process at 1,000 over 100 (limit 1.75) | 1.00 | 1.07 | 1.20 | 1.00 |
| PG08 resident bytes of 1,000 parked processes | 12,353,536 | 8,609,792 | 6,725,632 | 5,840,896 |

Parallel counting under `--trace=boundaries` (one run each, by hand, with
the telemetry written to `/dev/null`): 2.70 s on one carrier and 0.82 s on
four. Before span ids were taken in blocks it was 6.2 to 7.7 s on four.

What they show:

- Parallel work scales with the cores. Four carriers run the eight counting
  jobs 3.85 times faster than one, and the list jobs 3.58 times; this host
  gives four unrelated processes 3.91 times. Eight carriers
  on four cores add nothing.
- A hand-off between carriers is slow. A round trip that stays on one
  carrier takes 3.7 µs; one that crosses takes 22 to 39 µs. Each crossing
  writes the other carrier's wake pipe, and that carrier, asleep in `poll`,
  has to be woken by the kernel; `strace -c` at two carriers shows one
  `write`, one `poll` and two `read`s per crossing, with `poll` the bulk
  of the time.
- PG04's stop ends processes on every carrier, so it pays that hand-off per
  process: from 2 carriers up it is over its one-carrier limit of 30 µs.
  Start costs up to twice as much and stays under its limit. PG04 runs under
  `--trace=boundaries`; with span ids now taken in blocks (below), what is
  left of its growth is the hand-off, which 6c measures directly.
- PG05 stays flat: a process costs the same at 1,000 as at 100.
- PG08's resident difference falls with more carriers because the empty run
  it subtracts already holds every carrier thread's stack and allocator
  arenas; it is not a saving.

## Findings fixed in this run

The first run had four carriers count only 1.74 times faster than one, with
user time growing with the carrier count. Stack samples put the time in
`allocation_scope_sweep`, which took a fresh mark epoch with one atomic
fetch-add per collection pass; a loop pass collects, so the counter's cache
line moved between carriers on every pass. A carrier now takes 4,096
epochs at once (`take_mark_epoch` in runtime.c).

The review found the same pattern in tracing: every node took its span id
with a fetch-add on the runtime's counter, so a traced counting loop ran
2.5 times slower on four carriers than on one. With several carriers on
threads a carrier now takes 4,096 span ids at once (`take_spans`); span
ids stay unique but are no longer dense or ordered across carriers.

One carrier alone, and the simulation build, take both straight from the
counter, so they see the same epochs and span ids as before.
`fixtures/carrier-threads-stress.c` checks that neither repeats on four
threads and that one carrier's stay consecutive.

## Spinning before a carrier blocks (step 3 PR 6c)

An idle carrier now spins for up to 50 µs on a wake flag before it parks or
sleeps in `poll` (carrier 0's descriptor wait does not spin); the waker sets the flag after writing the pipe byte, and a spin that
sees it empties the pipe and runs again. It spins only when every carrier
can have a core (`available_parallelism`): with more carriers than cores a
spinning carrier would take a core from one with work. Same host, same
command, the spin in place:

| Measurement | 1 carrier | 2 | 4 | 8 (no spin) |
| --- | ---: | ---: | ---: | ---: |
| Parallel counting, 8 processes (ms) | 2,324 | 1,244 | 619 | 636 |
| Serial round trip (µs) | 3.3 | 8.1 | 9.8 | 40.8 |
| PG04 µs per start | 23.5 | 42.5 | 46.8 | 50.1 |
| PG04 µs per stop | 14.6 | 68.6 | 82.2 | 75.7 |

- A round trip across carriers is 3 to 4 times cheaper: 8 to 10 µs instead
  of 22 to 39. What is left is the pipe write and drain and the kernel lock
  changing hands. A 200 µs spin gave the same round trip, so the 50 µs spin
  catches these wakes. A 20 µs spin missed them at 4 carriers (24 µs).
- PG04 does not improve, and its stop got slower. Three runs each at the
  same commit, start and stop in µs:
  - 2 carriers: with spin 40/50, 47/83, 46/71; without 37/44, 35/40, 39/60.
  - 4 carriers: with spin 46/77, 48/77, 43/78; without 43/75, 36/55, 39/49.

  Start and stop are kernel work under one lock: each start and end takes
  the kernel lock several times and writes boundary telemetry. A carrier
  that sleeps comes back to more queued work per lock hand-off than one that
  spins and comes back at once. Sharding the kernel ("Order of work" step 4)
  is what addresses this, not the wake.
- Parallel work is unchanged.

## One carrier

`chimi-process-lab performance` on the same host and commit: every gate
passes except PG06's list payload (94,287 ns per message against 70,000),
which fails on `main` on this host too (parallel-scheduler.md, PR 4 notes).
PG01 1,093 ns per message, PG03 9.1 µs per restart, PG04 18.4 µs per start
and 12.4 µs per stop, PG09 1.5 ms. This host is not the baseline's, so these
are not compared with its numbers.

## Method

- `chimi-process-lab performance --carriers <n>` runs PG04, PG05 and PG08
  on n carriers. Several carriers need the real clock; their `members`
  workload never sleeps, so the clock changes no behaviour. Every other gate
  reports `unsupported`: its workload paces itself with virtual-clock
  sleeps, which would be real milliseconds.
- `scripts/carriers/app/fan_out.chimi` starts eight processes under one
  supervisor and gives each one job. Placement is membership id modulo the
  carrier count, so four carriers hold two each.
- `scripts/carriers/app/ping_pong.chimi` sends from main to two echo
  processes in turn and waits for each answer. Their membership ids are
  adjacent, so with two or more carriers at least one is on a carrier
  other than main's. The value is the difference between 100,000 and
  10,000 round trips over 90,000, so process start cancels out.
- The host ceiling runs n one-carrier copies of the counting program at
  once, each with 1/n of the work, so nothing is shared between them.
