//! The intrinsics of the native runtime that have moved from `runtime.c`
//! (CHI-305 step 5, phase 5): the program's tables and the standard values
//! built from them, then each family of intrinsics. The evaluator reaches a
//! family through [`dispatch`]; `runtime.c` serves the intrinsics that have
//! not moved.

mod abi;
pub mod assertions;
pub mod binding_calls;
pub mod binding_values;
pub mod bytes;
pub mod capabilities;
pub mod causes;
#[cfg(all(feature = "runtime-c", unix))]
pub mod clocks;
#[cfg(all(feature = "runtime-c", unix))]
pub mod channels;
pub mod collections;
pub mod console_io;
pub mod core;
pub mod debug;
pub mod decimals;
#[cfg(all(feature = "runtime-c", unix))]
pub mod dispatch;
pub mod durations;
pub mod host;
pub mod input;
pub mod json;
pub(crate) mod message;
pub mod metrics_host;
#[cfg(all(feature = "runtime-c", unix))]
pub mod process_kernel;
mod process_starts;
pub mod processes;
pub mod random_host;
pub mod scalars;
#[cfg(all(feature = "runtime-c", unix))]
pub mod scopes;
pub mod signals;
pub mod standard;
#[cfg(all(feature = "runtime-c", unix))]
pub mod task_entry;
pub mod tasks;
pub mod test_log;
mod text;
#[cfg(all(feature = "runtime-c", unix))]
pub mod waits;
