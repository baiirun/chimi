//! The heap's shared layout with `runtime.c`, the collector's oracle, and
//! the parts of the allocator that have moved to Rust: allocation, the
//! young chunks and the regions' start and end; the collector follows in
//! the later phases of CHI-305 step 5.

#[cfg(all(feature = "runtime-c", unix))]
pub mod allocator;
#[cfg(all(feature = "runtime-c", unix))]
mod allocator_abi;
pub mod layout;
#[cfg(unix)]
pub mod small_blocks;
#[cfg(all(feature = "runtime-c", unix))]
mod small_blocks_abi;
pub mod verify;
mod verify_abi;
#[cfg(all(feature = "runtime-c", unix))]
pub mod young;
#[cfg(all(feature = "runtime-c", unix))]
mod young_abi;
