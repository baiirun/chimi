//! A carrier's index. Each carrier's ready queue lives in its shard
//! (`shard.rs`): a task is queued on its home carrier, the one its fiber
//! runs on, and only that carrier pops it.

/// A carrier thread's index, from 0. Carrier 0 owns the root, signals and
/// standard input.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Carrier(pub(super) u32);
