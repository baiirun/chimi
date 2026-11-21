# Multi-Carrier Baseline

This record holds the first measurements of programs on several carriers
(parallel-scheduler.md, step 3 PR 6a), to inform the choice of the default
carrier count. It is a local measurement on one cloud host, not a
cross-machine promise. The single-carrier numbers are in
[the supervised-process baseline](supervised-process-conformance-baseline.md).

## Run

- Date: 2026-11-21
- Commit: `a8e8e74` plus this pull request (`take_mark_epoch` and the lab's
  `--carriers`).
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
| Parallel counting, 8 processes (ms) | 2,359 | 1,182 | 645 | 602 |
| Parallel list building, 8 processes (ms) | 2,670 | 1,390 | 724 | 734 |
| Host ceiling: n one-carrier copies, 1/n of the counting each (ms) | 2,362 | 1,190 | 627 | 629 |
| Serial round trip, main to two echo processes (µs) | 3.3 | 23.4 | 37.5 | 39.3 |
| PG04 µs per start (limit 50) | 20.6 | 44.5 | 44.3 | 47.4 |
| PG04 µs per stop (limit 30) | 15.1 | 54.5 | 59.4 | 68.9 |
| PG05 cost per process at 1,000 over 100 (limit 1.75) | 1.21 | 1.05 | 1.04 | 1.01 |
| PG08 resident bytes of 1,000 parked processes | 12,173,312 | 8,671,232 | 6,701,056 | 6,336,512 |

What they show:

- Parallel work scales with the cores. Four carriers run the eight counting
  jobs 3.66 times faster than one, and the list jobs 3.69 times, which is
  what this host gives four unrelated processes (3.77 times). Eight carriers
  on four cores add nothing.
- A hand-off between carriers is slow. A round trip that stays on one
  carrier takes 3.3 µs; one that crosses takes 23 to 39 µs. Each crossing
  writes the other carrier's wake pipe, and that carrier, asleep in `poll`,
  has to be woken by the kernel; `strace -c` at two carriers shows one
  `write`, one `poll` and two `read`s per crossing, with `poll` the bulk
  of the time.
- PG04's stop ends processes on every carrier, so it pays that hand-off per
  process: from 2 carriers up it is over its one-carrier limit of 30 µs.
  Start costs about twice as much and stays under its limit.
- PG05 stays flat: a process costs the same at 1,000 as at 100.
- PG08's resident difference falls with more carriers because the empty run
  it subtracts already holds every carrier thread's stack and allocator
  arenas; it is not a saving.

## Finding fixed in this run

The first run had four carriers count only 1.74 times faster than one, with
user time growing with the carrier count. Stack samples put the time in
`allocation_scope_sweep`, which took a fresh mark epoch with one atomic
fetch-add per collection pass; a loop pass collects, so the counter's cache
line moved between carriers on every pass. A carrier now takes 4,096
epochs at once (`take_mark_epoch` in runtime.c). One carrier alone takes
consecutive blocks, so it sees the same epochs as before.

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
