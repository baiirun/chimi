//! The intrinsics of the native runtime that have moved from `runtime.c`
//! (CHI-305 step 5, phase 5): the program's tables and the standard values
//! built from them, then each family of intrinsics. `runtime.c`'s
//! `invoke_intrinsic` still dispatches; it calls a family's entry here.

pub mod collections;
mod collections_abi;
pub mod standard;
