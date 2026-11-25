//! The parts of the native runtime written in Rust that every program links,
//! behind the C ABI that `runtime.c` calls (CHI-305). It is `no_std` plus
//! `alloc`: a program without the scheduler links it through
//! `chimi-runtime-standalone`, which supplies the allocator and panic
//! handler, and one with the scheduler links it inside `chimi-fiber`.
//!
//! Code here does not panic on any input; the exported functions are the
//! only `unsafe` boundary.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod json;
mod json_abi;
