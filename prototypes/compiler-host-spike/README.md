# Chimi Compiler Host Spike

> PROTOTYPE: this is a disposable comparison workload, not compiler code and
> not a commitment to either implementation.

## Question

Which bootstrap host—Go or Rust—gives Chimi the better implementation
foundation when both perform the same representative compiler work?

Each implementation generates identical Chimi source, losslessly tokenizes it
into compact source spans, arena-allocates line and brace nodes, traverses the
retained representation, and reports the same result schema. The shell rejects
a comparison unless both implementations produce identical input size, token
count, node count, round-trip status, and traversal checksum.

The spike measures the host implementation, not Chimi's eventual backend.
Neither implementation may define Chimi semantics or become a dependency of
the real compiler.

## Run

```sh
python3 prototypes/compiler-host-spike/run.py
```

Press `r` to build and run both candidates. For noninteractive output:

```sh
python3 prototypes/compiler-host-spike/run.py --all
```

Use `--declarations` or `--iterations` to change workload size and sample
count. Builds happen in a temporary directory and leave no compiler artifacts
in the repository.

## Interpretation

The measurements answer only M0's bootstrap-host question:

- fresh and one-file-change release build iteration;
- executable size and external runtime independence;
- steady-state frontend throughput;
- total allocation and retained representation memory;
- peak process RSS;
- implementation complexity and future profiling, fuzzing, and backend seams.

One local run is evidence, not a universal language benchmark. The accepted M0
decision must record the machine, toolchain versions, source revision, raw
results, and the architectural tradeoff that determines the selection.
