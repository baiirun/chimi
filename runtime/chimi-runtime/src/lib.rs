//! The parts of the native runtime written in Rust that every program links,
//! behind the C ABI that `runtime.c` calls (CHI-305), and the number models
//! the compiler shares with it. It is `no_std` plus `alloc`: a program without
//! the scheduler links it through `chimi-runtime-standalone`, which supplies
//! the allocator and panic handler, one with the scheduler links it inside
//! `chimi-fiber`, and the compiler links it as an ordinary crate.
//!
//! Code here does not panic on any input; the exported functions are the
//! only `unsafe` boundary.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod decimal;
mod decimal_abi;
pub mod duration;
mod duration_abi;
pub mod json;
mod json_abi;
mod text_buffer;

pub use text_buffer::TextBuffer;
