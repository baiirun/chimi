//! The intrinsics of the native runtime that have moved from `runtime.c`
//! (CHI-305 step 5, phase 5): the program's tables and the standard values
//! built from them, then each family of intrinsics. The evaluator reaches a
//! family through [`dispatch`]; `runtime.c` serves the intrinsics that have
//! not moved.

mod abi;
pub mod assertions;
pub mod binding_values;
pub mod bytes;
pub mod causes;
pub mod collections;
pub mod core;
pub mod debug;
pub mod decimals;
#[cfg(all(feature = "runtime-c", unix))]
pub mod dispatch;
pub mod durations;
pub mod host;
pub mod json;
pub(crate) mod message;
pub mod scalars;
pub mod standard;
pub mod test_log;
mod text;
