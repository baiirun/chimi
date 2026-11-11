# Supervised Process Runtime Prototype

> PROTOTYPE: disposable evidence for `prog show ts-a6b43b`, not Chimi runtime code.

## Question

Can long-lived processes run on the existing stackful fibers with memory that
depends on live state rather than uptime, safe per-pass reclamation, cheap
restart and shutdown, and an exact lifecycle trace?

## Design under test

- **Per-owner heaps.** Each incarnation owns a heap; the runtime switches the
  current heap on every fiber switch.
- **Copy on transfer.** A message is copied into a detached fragment when it is
  accepted, preserving sharing inside the message. Receiving splices the
  fragment into the receiver's heap without a second copy.
- **Regions.** A loop opens a region. Each pass collects only the region, with
  the loop state as the only root. Immutability means older values never point
  to newer ones, so values from before the loop and outer call frames are safe
  without stack maps. Survivors are promoted, so a pass does not re-walk state
  that already survived.
- **Owner tags.** Every chunk records its owner. Copy and collection report a
  pointer into another heap instead of following it.
- **Supervision outside fibers.** Supervisors run no user code, so they are
  kernel data structures, not fibers.

## Run

```sh
cargo test --release --manifest-path prototypes/supervised-process-runtime/Cargo.toml
cargo run --release --manifest-path prototypes/supervised-process-runtime/Cargo.toml --example measure
```

Results and the decision they support are in
[the architecture record](../../docs/architecture/supervised-process-runtime.md).
