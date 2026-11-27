//! `runtime.c` asserts the heap and value layout that chimi-runtime's
//! `heap/layout.rs` mirrors (CHI-305 step 5, phase 0). This test keeps the
//! two in step in two ways:
//!
//! - The generated block of `_Static_assert`s in `runtime.c` must be exactly
//!   what the Rust table renders, so a Rust change shows up here, and a C
//!   change that moves a listed number fails the C compile.
//! - Each mirrored C definition (in `runtime.c`, or `runtime.h` for the
//!   program tables) must match its digest in
//!   `tests/runtime_layout.digests`, so a change the numbers cannot see (an
//!   enum constant appended, a field added in padding) fails here too, and
//!   points at the mirror to update.
//!
//! After an intended change, update the mirror, run with
//! `CHIMI_LAYOUT_BLESS=1` to rewrite the block and the digests, and review
//! the diff.

use chimi_runtime::heap::layout::{C_BEGIN, C_END, c_asserts};

const RUNTIME: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/native/runtime.c");
const HEADER: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/native/runtime.h");
const DIGESTS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/runtime_layout.digests");

/// The mirrored C definitions: each starts at `start` and ends at the first
/// `end` after it.
const DEFINITIONS: &[(&str, &str)] = &[
    ("typedef enum { VALUE_VOID,", "} ValueKind;"),
    ("typedef enum { CALL_DECLARED,", "} CallableKind;"),
    ("typedef enum { HOST_PROCESS_UTF8_LINES,", "} HostFunction;"),
    ("typedef enum { CONTROL_SUCCESS,", "} ControlKind;"),
    ("typedef struct { uint64_t high;", "} ChimiDecimal;"),
    ("typedef struct { const char *name; ChimiValue *value; }", "} RuntimeField;"),
    ("typedef struct { uint64_t channel;", "} RuntimeCapability;"),
    ("struct ListNode {", "\n};"),
    ("typedef struct { ListNode *root;", "} ListVector;"),
    ("struct MapNode {", "\n};"),
    ("typedef struct ChimiCallable {", "\n} ChimiCallable;"),
    ("struct ChimiValue {", "\n};"),
    ("struct ChimiEnvironment {", "\n};"),
    ("struct ChimiControl {", "\n};"),
    ("typedef enum {\n  ALLOCATION_BYTES,", "} AllocationKind;"),
    ("typedef enum { AREA_YOUNG,", "} AreaRole;"),
    ("struct AllocationHeader {", "\n};"),
    ("enum { YOUNG_CHUNK_BYTES", "YOUNG_CHUNK_LARGEST = YOUNG_CHUNK_CAPACITY / 4 };"),
    ("struct AllocationArea {", "\n};"),
    ("struct AllocationScope {", "\n};"),
    ("struct RuntimeHeap {", "\n};"),
    ("typedef enum { HEAP_OK", "} HeapStatus;"),
    ("typedef struct {\n  int64_t live_allocations;", "} HeapTotals;"),
    ("enum { REGION_FULL_MIN_GROWTH", "};"),
    ("enum { MARK_EPOCH_BLOCK", "};"),
    ("typedef struct {\n  const char *operation;", "} ProfileRow;"),
    ("enum { SMALL_BLOCK_GRAIN", "};"),
    ("typedef struct SmallBlock {", "} SmallBlock;"),
    ("typedef struct {\n  SmallBlock *lists[", "} SmallBlockCache;"),
    ("typedef struct { void **items;", "} PointerStack;"),
    ("typedef struct {\n  RuntimeHeap *heap;\n  AllocationArea *target;", "} FiberContext;"),
    ("typedef struct {\n  FiberContext fiber;", "} Carrier;"),
    ("typedef struct {\n  size_t total_allocated_bytes;", "} ScopeTotals;"),
    ("enum { FRAGMENT_ROOTS", "} RuntimeFragment;"),
    ("struct ChimiBindingCall {", "\n};"),
    ("struct ChimiRuntime {", "\n};"),
    // runtime.h
    ("typedef struct {\n  uint32_t kind;\n  uint32_t name;\n  uint32_t function;", "} ChimiSymbol;"),
    ("typedef struct {\n  uint32_t type_name;\n  uint32_t variant_name;", "} ChimiConstructor;"),
    ("typedef struct {\n  uint32_t file_id;", "} ChimiOrigin;"),
    ("typedef struct {\n  uint32_t origin;\n  uint32_t a;", "} ChimiNode;"),
    ("typedef struct {\n  uint32_t symbol;\n  uint32_t body;", "} ChimiFunction;"),
    ("typedef struct {\n  uint32_t name;\n  uint32_t node;", "} ChimiNamedNode;"),
    ("typedef struct {\n  uint32_t kind;\n  uint32_t symbol;\n  uint32_t node;", "} ChimiStep;"),
    ("typedef struct {\n  uint32_t patterns_offset;", "} ChimiArm;"),
];

/// The text before the block, the block itself (both marker lines
/// included), and the text after it.
fn split(source: &str) -> (&str, &str, &str) {
    let start = source.find(C_BEGIN).expect("runtime.c has no generated layout block");
    let end = source[start..].find(C_END).expect("the layout block has no end marker") + start + C_END.len();
    let end = end + usize::from(source[end..].starts_with('\n'));
    (&source[..start], &source[start..end], &source[end..])
}

/// `text` without comments and with each run of whitespace made one space,
/// so a comment or a reflow does not count as a change.
fn normalized(text: &str) -> String {
    let mut code = String::new();
    let mut rest = text;
    while let Some(open) = rest.find("/*") {
        code.push_str(&rest[..open]);
        code.push(' ');
        rest = rest[open..].find("*/").map_or("", |close| &rest[open + close + 2..]);
    }
    code.push_str(rest);
    code.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// FNV-1a, 64 bits: a digest is only compared with its own earlier value.
fn digest(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3))
}

/// One line per definition: its digest and its first line.
fn digests(source: &str) -> String {
    let mut lines = String::new();
    for &(start, end) in DEFINITIONS {
        let found = source.matches(start).count();
        assert_eq!(found, 1, "runtime.c and runtime.h should have exactly one definition starting {start:?}, has {found}");
        let from = source.find(start).unwrap();
        let to = source[from..].find(end).unwrap_or_else(|| panic!("no {end:?} after {start:?}")) + from + end.len();
        let first = start.lines().next().unwrap();
        lines.push_str(&format!("{:016x} {first}\n", digest(&normalized(&source[from..to]))));
    }
    lines
}

fn blessing() -> bool {
    std::env::var("CHIMI_LAYOUT_BLESS").is_ok_and(|value| value == "1")
}

#[test]
fn runtime_c_asserts_the_mirrored_layout() {
    let source = std::fs::read_to_string(RUNTIME).expect("read runtime.c");
    let (before, block, after) = split(&source);
    let mut expected = String::new();
    c_asserts(&mut expected).expect("render the layout block");
    if blessing() {
        std::fs::write(RUNTIME, format!("{before}{expected}{after}")).expect("write runtime.c");
        return;
    }
    assert!(
        block == expected,
        "runtime.c's layout block differs from chimi-runtime heap/layout.rs; rerun with \
         CHIMI_LAYOUT_BLESS=1 and check that runtime.c still compiles\n--- runtime.c\n{block}\n--- layout.rs\n{expected}"
    );
}

#[test]
fn the_mirrored_c_definitions_are_unchanged() {
    let source = std::fs::read_to_string(RUNTIME).expect("read runtime.c")
        + &std::fs::read_to_string(HEADER).expect("read runtime.h");
    let actual = digests(&source);
    if blessing() {
        std::fs::write(DIGESTS, &actual).expect("write the digests");
        return;
    }
    let recorded = std::fs::read_to_string(DIGESTS).expect("read tests/runtime_layout.digests");
    let changed: Vec<_> = actual
        .lines()
        .zip(recorded.lines().chain(std::iter::repeat("")))
        .filter(|(now, then)| now != then)
        .map(|(now, _)| now.split_once(' ').map_or(now, |(_, name)| name))
        .collect();
    assert!(
        changed.is_empty() && actual.lines().count() == recorded.lines().count(),
        "these runtime.c definitions changed since chimi-runtime heap/layout.rs last mirrored them: \
         {changed:?}. Update the mirror, then rerun with CHIMI_LAYOUT_BLESS=1"
    );
}

#[test]
fn normalizing_ignores_comments_and_spacing_only() {
    assert_eq!(normalized("struct A {\n  int x; /* why */\n  int  y;\n};"), "struct A { int x; int y; };");
    assert_ne!(digest(&normalized("struct A { int x; };")), digest(&normalized("struct A { long x; };")));
}
