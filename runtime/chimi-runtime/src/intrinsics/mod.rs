//! The intrinsics of the native runtime that have moved from `runtime.c`
//! (CHI-305 step 5, phase 5): the program's tables and the standard values
//! built from them, then each family of intrinsics. `runtime.c`'s
//! `invoke_intrinsic` still dispatches; it calls a family's entry here.

mod abi;
pub mod bytes;
pub mod collections;
pub mod core;
pub mod debug;
pub mod decimals;
pub mod durations;
pub mod json;
mod message;
pub mod standard;
mod text;
