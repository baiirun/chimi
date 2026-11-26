//! `runtime.c` asserts the heap and value layout that chimi-runtime's
//! `heap/layout.rs` mirrors (CHI-305 step 5, phase 0). This test keeps the
//! two in step: the generated block of `_Static_assert`s in `runtime.c` must
//! be exactly what the Rust table renders, so a Rust change shows up here and
//! a C change fails the C compile against the blessed block. Run with
//! `CHIMI_LAYOUT_BLESS=1` after an intended change to rewrite the block, and
//! review the diff.

use chimi_runtime::heap::layout::{C_BEGIN, C_END, c_asserts};
use std::path::Path;

const RUNTIME: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/native/runtime.c");

/// The text before the block, the block itself (both marker lines
/// included), and the text after it.
fn split(source: &str) -> (&str, &str, &str) {
    let start = source.find(C_BEGIN).expect("runtime.c has no generated layout block");
    let end = source[start..].find(C_END).expect("the layout block has no end marker") + start + C_END.len();
    let end = end + usize::from(source[end..].starts_with('\n'));
    (&source[..start], &source[start..end], &source[end..])
}

#[test]
fn runtime_c_asserts_the_mirrored_layout() {
    let source = std::fs::read_to_string(RUNTIME).expect("read runtime.c");
    let (before, block, after) = split(&source);
    let mut expected = String::new();
    c_asserts(&mut expected).expect("render the layout block");
    if std::env::var_os("CHIMI_LAYOUT_BLESS").is_some() {
        std::fs::write(Path::new(RUNTIME), format!("{before}{expected}{after}")).expect("write runtime.c");
        return;
    }
    assert!(
        block == expected,
        "runtime.c's layout block differs from chimi-runtime heap/layout.rs; rerun with \
         CHIMI_LAYOUT_BLESS=1 and check that runtime.c still compiles\n--- runtime.c\n{block}\n--- layout.rs\n{expected}"
    );
}
