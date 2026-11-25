//! The `Duration` model lives in `chimi-runtime`, which the native runtime
//! links too, so both backends print and parse durations with one
//! implementation.

pub(crate) use chimi_runtime::duration::*;
