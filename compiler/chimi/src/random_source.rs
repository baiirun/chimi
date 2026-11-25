//! The `random` provider's generator, shared with the native runtime in
//! `chimi_runtime::random`, and the operating-system seed the interpreter's
//! `main` draws from.

pub(crate) use chimi_runtime::random::{Generator, test_seed};

/// A generator seeded from the operating system, for a program's `main`.
/// Not suitable for keys or tokens.
pub(crate) fn from_os() -> Generator {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos()),
    );
    Generator::seeded(hasher.finish())
}
