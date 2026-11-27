//! Binding packages written in Rust (G20): the standard packages with no C
//! library under them. Each exports the same `ChimiBindingFunction` symbols
//! its C file did and reaches the runtime only through `chimi_binding.h`,
//! which [`api`] wraps.
//!
//! The module is built only into the runtime archives (the `bindings`
//! feature), because it imports the parts of the binding API `runtime.c`
//! still exports. A program links only the functions it reaches: the link
//! drops unreferenced sections.
//!
//! Two rules C did not need:
//!
//! - Nothing here may panic. A panic aborts the whole program (the standalone
//!   archive aborts on panic, and a panic cannot unwind out of an `extern
//!   "C"` function), so the lints below refuse the usual sources. Integer
//!   arithmetic is left to review: every count here is bounded by a buffer.
//! - Keep owning values held across a wait small and bounded. A task stopped
//!   while it waits is abandoned at the park and its frames never run their
//!   destructors, so whatever it holds there leaks. State that must be
//!   released lives in a handle, whose `release` the runtime runs exactly
//!   once. A wait-local buffer, a pending lookup or a socket still being
//!   connected may leak this way, as they did in the C packages.
#![deny(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

mod api;
mod exec;
mod net;
mod os;
mod terminal;
