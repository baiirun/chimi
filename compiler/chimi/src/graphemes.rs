//! Extended grapheme clusters live in `chimi-runtime`, which the native
//! runtime links too, so both backends split text with one implementation.

pub(crate) use chimi_runtime::graphemes::graphemes;
