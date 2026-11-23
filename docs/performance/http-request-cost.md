# HTTP Request Cost (CHI-303)

While measuring several carriers, a loopback HTTP benchmark used about 2 ms of
CPU per request, and four carriers served it only about 1.3 times faster. This
record finds where that CPU went and what one change took out of it. It is a
local measurement on one cloud host, not a cross-machine promise.

## Run

- Date: 2026-11-23
- Commits: `92691cf` (before) against `92691cf` plus this change (after).
- Host: a cloud container, Linux 6.18 (Firecracker), 4 vCPUs, the same kind of
  host as [the multi-carrier baseline](multi-carrier-baseline.md).
- Benchmark: the program in CHI-303's agent notes. Four client tasks each send
  N `GET` requests with `Http.send` to an `Http.serve` server in the same
  program, over loopback, so each request is parsed and answered on both ends.
  Built with `chimi build --root <dir> app/http -o httpb` and run as
  `./httpb --chimi-runner --clock=real --trace=off --carriers=C -- 300`
  (1,200 requests). Every run returned 1,200.

## Where the CPU went

Callgrind put about 16 million instructions on a request, with `memset`,
`allocate` and the young-chunk sweep at the top. The runtime's operation
profile (`--profile-file`) named the cause: a request allocated about 2.0 MB in
about 44,000 allocations, and most of them came from `std/http` checking bytes
one at a time. `is_token`, `is_visible`, the control-character check over a
whole head, `trim_spaces`, `ascii_lowercase` and the URL checks all turned a
`Bytes` into a `List<Int>` and walked it with `List.all`, `List.any` or
`List.find_index`, and `is_token_byte` built a 15-element list for every byte
it looked at. Each step of such a walk allocates a control, a tuple and its
fields, so a head of a few hundred bytes cost tens of thousands of
allocations. The scheduler stayed under 1%.

Callgrind overstates `memset`: glibc zeroes large blocks with `rep stosb`,
which callgrind counts once per byte, so its share of real time is smaller than
its share of instructions.

## The change

`std/http` now checks a byte class with one private native,
`count_ends(bytes, allowed)`, which returns how many bytes at the start and at
the end of `bytes` are in `allowed`. Each class (token characters, what a head
or a header value may hold, visible characters, host names, IPv6 literals, a
URL target, spaces and tabs) is a module constant. Lowercasing hands all-ASCII
text to `Text.lowercase` and keeps the per-byte path only for text with other
bytes. `is_framing_header` became a `case` instead of a list built per header.

## Results

User CPU and wall time for 1,200 requests, medians of 5 interleaved runs:

| Carriers | Before: user CPU | After: user CPU | Before: wall | After: wall |
| --- | ---: | ---: | ---: | ---: |
| 1 | 1.98 s | 1.03 s | 2.17 s | 1.20 s |
| 4 | 2.32 s | 1.38 s | 1.53 s | 0.83 s |

On one carrier a request now takes about 0.86 ms of CPU instead of 1.65 ms,
and four carriers serve it about 1.45 times faster than one (1.4 before).

| Per request | Before | After |
| --- | ---: | ---: |
| Instructions (callgrind, 80 requests) | 15.8 M | 5.6 M |
| Bytes allocated (`--profile-file`, 200 requests) | 2.0 MB | 0.74 MB |
| Allocations | 43,800 | 15,600 |

## What is left

The remaining allocations are spread thin: `List.map`, `List.filter` and
`List.append` (built on `List.reduce`), `Text.split_once`, `http_date`, and
the runtime's cost of a node, which allocates a control for every result. No
single `std/http` function is above 3% of the bytes. A further large cut needs
the runtime to stop allocating per node, which is its own piece of work.

A shared table of small integers (each of -128 to 1023 one static value, as
Void, True and False already are) was measured on the way: 1.6% fewer
instructions and no change in CPU time beyond noise, so it was not kept.
