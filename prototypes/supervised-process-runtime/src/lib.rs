//! PROTOTYPE: disposable evidence for `prog show ts-a6b43b`, not Chimi runtime code.
//!
//! Two components:
//! - `heap`: per-owner heaps with region collection and copy-on-transfer (C).
//! - `kernel`: process incarnations on real `chimi-fiber` fibers, bounded
//!   mailboxes, registered names, and supervisors with restart accounting.
//!
//! The kernel depends on the heap; the heap knows nothing about processes.

pub mod heap;
pub mod kernel;
