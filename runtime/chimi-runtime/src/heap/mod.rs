//! The heap's shared layout with `runtime.c`, the collector's oracle, and
//! the parts of the allocator that have moved to Rust; the rest of the
//! allocator and the collector follow in the later phases of CHI-305 step 5.

pub mod layout;
#[cfg(unix)]
pub mod small_blocks;
#[cfg(unix)]
mod small_blocks_abi;
pub mod verify;
mod verify_abi;
