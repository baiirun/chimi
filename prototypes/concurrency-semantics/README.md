# Concurrency semantics prototype

PROTOTYPE: disposable state-machine evidence, not Chimi runtime code.

## Question

Can the first task and channel primitives coherently represent ordinary-call
suspension, structured child ownership, rendezvous, bounded backpressure,
explicit `Received`/`Drained` results, recoverable send failures, graceful
close, destructive abort, cancellation, expected failures, and defects without
introducing supervisors or committing to a fiber implementation?

## Run

    cargo run --quiet --manifest-path prototypes/concurrency-semantics/Cargo.toml

Press n to apply the next transition, r to reset, and s to switch scenarios.
Every frame shows the complete task and channel state.

To print a compact transcript of every scenario:

    cargo run --quiet --manifest-path prototypes/concurrency-semantics/Cargo.toml -- --all

The prototype is deliberately a deterministic semantic reducer. It does not
create threads or fibers and cannot answer scheduler-performance questions.
Its channel transitions implement the selected contract in
`docs/research/channel-semantics.md`: capacity-zero rendezvous, FIFO admission,
atomic cancellation-before-admission, graceful drain, and destructive abort.
