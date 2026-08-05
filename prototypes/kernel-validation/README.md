# Chimi Executable-Kernel Validation Prototype

> PROTOTYPE: this is a disposable integration audit, not a Chimi compiler.

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

The explored kernel is coherent and ready to consolidate into a specification,
but it is not an end-to-end compiler. Source through name resolution is truly
connected: the parser produces the CST consumed by lowering, and the resolver
consumes lowered modules. Formatting and inspection consume those same parser
and lowering representations.

Two central handoffs are still independent models:

- type-contract scenarios do not consume resolved semantic expressions;
- the runner consumes hand-built executable nodes rather than typed expressions.

This is acceptable evidence for a language-design specification because both
models exercise the same accepted corpus concepts and their boundaries are
explicit. It is not evidence that arbitrary `.chimi` source can already compile
and run. Connecting those handoffs belongs to the first implementation plan,
not to further surface-language invention.

Production scheduling, supervision, durability, recovery, a permanent backend,
and incremental semantic performance remain deliberately outside this kernel.
