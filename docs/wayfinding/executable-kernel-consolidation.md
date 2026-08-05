# Consolidate Chimi's Executable Kernel

The canonical Wayfinder map is the `prog` epic `ep-aae72a`. Use
`prog show ep-aae72a` for the current destination and `prog graph -p chimi` for
the dependency graph.

## Destination

Turn the validated executable-kernel decisions into one durable normative
specification and an implementation-ready compiler plan without expanding the
language surface.

## Tickets

| Ticket | Type | `prog` pointer |
| --- | --- | --- |
| [Inventory accepted decisions and remaining specification questions](../specification/executable-kernel-decision-inventory.md) | Specification | `prog show ts-5c7c94` |
| [Write the normative executable-kernel specification](../specification/executable-kernel.md) | Specification | `prog show ts-5073cc` |
| [Specify compiler representation handoffs](../architecture/compiler-representation-handoffs.md) | Architecture | `prog show ts-73855d` |
| [Produce the first compiler implementation roadmap](../plans/first-compiler-implementation-roadmap.md) | Plan | `prog show ts-acb78f` |

## Frontier

This map is complete. Continue with
[Build Chimi's First Connected Compiler](first-connected-compiler.md)
(`prog show ep-703bbc`).

## Scope Boundary

This phase consolidates accepted behavior. New concurrency, durability,
effects, backend, package, deployment, and interoperability design belongs in
later maps.
