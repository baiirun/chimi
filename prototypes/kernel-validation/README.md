# Chimi Executable-Kernel Validation Prototype

> PROTOTYPE: this is a disposable integration audit of the Chimi compiler and
> its earlier component models.

## Question

Do the selected syntax, CST, semantic representations, contracts, diagnostics,
performance evidence, and runner semantics form one coherent executable kernel
that is ready to consolidate into a durable specification?

## Run

From the repository root:

```sh
python3 prototypes/kernel-validation/main.py
```

Use `p` to inspect every executable probe, `h` to inspect the actual pipeline
handoffs, and `g` to inspect what the prototypes still do not prove.

Run the complete noninteractive validation with:

```sh
python3 prototypes/kernel-validation/main.py --check
```

## Verdict

The kernel is specification-ready and its first compiler path is end-to-end.
The Rust checker consumes resolved semantic projects, sealing consumes the
resulting typed contracts, and executable lowering consumes only a verified
root. The validator runs focused Rust probes for both handoffs in addition to
the earlier component probes. Formatting and inspection still consume the
parser and lowering representations established by those component models.

Production scheduling, supervision, durability, recovery, a permanent backend,
and incremental semantic performance remain deliberately outside this kernel.
