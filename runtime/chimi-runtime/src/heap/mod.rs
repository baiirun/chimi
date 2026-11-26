//! The heap's shared layout with `runtime.c`, the collector's oracle, and
//! the parts that have moved to Rust: allocation, the young chunks, the
//! regions' start and end, the collector, fragment copies, the heaps'
//! creation and end, and the capability walk; the task and process heap
//! lifecycle and the values follow in the later phases of CHI-305 step 5.

#[cfg(all(feature = "runtime-c", unix))]
pub mod allocator;
#[cfg(all(feature = "runtime-c", unix))]
pub mod collector;
#[cfg(all(feature = "runtime-c", unix))]
mod collector_abi;
#[cfg(all(feature = "runtime-c", unix))]
pub mod fragment;
#[cfg(all(feature = "runtime-c", unix))]
mod fragment_abi;
#[cfg(all(feature = "runtime-c", unix))]
pub mod list;
#[cfg(all(feature = "runtime-c", unix))]
mod allocator_abi;
pub mod layout;
#[cfg(unix)]
pub mod small_blocks;
#[cfg(all(feature = "runtime-c", unix))]
mod small_blocks_abi;
#[cfg(all(feature = "runtime-c", unix))]
pub mod transfer;
#[cfg(all(feature = "runtime-c", unix))]
mod transfer_abi;
pub mod verify;
mod verify_abi;
#[cfg(all(feature = "runtime-c", unix))]
pub mod young;
#[cfg(all(feature = "runtime-c", unix))]
mod young_abi;
