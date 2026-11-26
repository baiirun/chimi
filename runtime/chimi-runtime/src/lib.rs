//! The parts of the native runtime written in Rust that every program links,
//! behind the C ABI that `runtime.c` calls (CHI-305), and the models the
//! compiler shares with it. It is `no_std` plus `alloc`: a program without
//! the scheduler links it through `chimi-runtime-standalone`, which supplies
//! the allocator and panic handler, one with the scheduler links it inside
//! `chimi-fiber`, and the compiler links it as an ordinary crate.
//!
//! Nothing the C ABI reaches panics on a value the runtime built. The
//! exported functions are the `unsafe` boundary, besides the lock and the
//! heap layout's field-size reads (`pthread_locked`, `heap::layout`).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

#[cfg(all(feature = "bindings", unix))]
mod binding;
pub mod console;
mod console_abi;
pub mod decimal;
mod decimal_abi;
pub mod duration;
mod duration_abi;
pub mod files;
mod files_abi;
pub mod float;
mod float_abi;
pub mod graphemes;
pub mod handle_counts;
pub mod heap;
#[cfg(unix)]
mod handle_counts_abi;
#[cfg(unix)]
mod pthread_locked;
pub mod handles;
mod handles_abi;
pub mod json;
mod json_abi;
pub mod metrics;
mod metrics_abi;
pub mod random;
mod random_abi;
mod serialized;
pub use serialized::note_exiting;
pub mod text;
mod text_abi;
mod text_buffer;
pub mod timestamp;
mod timestamp_abi;

pub use text_buffer::TextBuffer;
