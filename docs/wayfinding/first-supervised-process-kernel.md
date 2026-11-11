# Define Chimi's First Supervised-Process Kernel

The canonical map is the `prog` epic `ep-f29d47`. Use
`prog show ep-f29d47` for the destination and `prog graph -p chimi` for the
dependency graph.

## Destination

Produce an executable, measured supervised-process layer above Chimi's finite
Task and Channel semantics. The layer must define typed process messaging,
isolated incarnation-local state, dynamic supervisor ownership, defect
containment and restart, bounded shutdown, native execution, and structured
observability without implying persistence.

## Accepted Foundation

The completed [first concurrency kernel](first-concurrency-kernel.md) remains
the source of accepted Task, Channel, process, and supervision direction. This
map resolves the intentionally provisional process surface and carries the
result through a bounded native implementation. Examples must use actual Chimi
syntax; the forcing program validates the model but does not organize it.

## Frontier

The current frontier is:

- **Choose the native supervised-process representation** — `prog show ts-a6b43b`.

## Decisions So Far

- **Specify process and supervisor static contracts** — `prog show ts-5961ee`.
  Positional functions, one `Supervisor.Member` type without the message type,
  a `Supervisor.Restart` argument, and `T4260`–`T4264`; see
  [process and supervisor static contracts](../specification/process-and-supervisor-static-contracts.md).

- **Define the complete supervisor policy contract** — `prog show ts-535827`.
  Inert specifications, a shareable running handle, exclusive membership
  handles, fail-fast admission, one rolling restart limit per supervisor, and
  newest-first shutdown; see
  [supervisor policy and lifecycle](../specification/supervisor-policy-and-lifecycle.md).
- **Define process identity and registered routing** — `prog show ts-7aff55`.
  One `Process.Address<Message>` send target with named or direct routing;
  names are `process name` declarations; see
  [process identity and routing](../specification/process-identity-and-routing.md).
- **Choose how Chimi expresses repetition** — `prog show ts-3e25fc`. A
  state-rebinding `loop` expression with `break value` is the canonical
  unbounded repetition; see [repetition](../specification/repetition.md).
- **Define process execution and mailbox semantics** — `prog show ts-a20d02`.
  Bounded FIFO mailboxes, a never-parking `Process.send` that returns the
  message on `Full` or `Unavailable`, no selective receive, and supervisor-owned
  stopping; see
  [process execution and mailboxes](../specification/process-execution-and-mailboxes.md).
- **Choose process restart composition** — `prog show ts-3380c6`. Defect
  restart remains supervisor-owned composition with shared owner accounting;
  ordinary retry remains for expected failures.

## Blocked Route

| Decision or gate | `prog` pointer | Depends on |
| --- | --- | --- |
| Design the supervised-process conformance lab | `prog show ts-b9cbe1` | Static contracts and native representation |
| Connect supervised processes to native execution | `prog show ts-6ccbc9` | Static contracts, native representation, and lab design |
| Run supervised-process conformance and performance gates | `prog show ts-1765ba` | Connected implementation |

The final forcing program is a small dynamic pool: one managed process defects
and restarts while its peers continue, with exact lifecycle and causal trace
assertions. Independent fixtures remain the evidence for each semantic,
static, operational, and performance contract.

## Scope Boundary

Durable mailboxes, acknowledgement and redelivery, persisted process state,
automatic topology recovery, distributed process location, clustering, hot
code upgrades, transparent reference retargeting, and production deployment
orchestration remain outside this map.
