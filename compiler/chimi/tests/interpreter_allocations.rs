//! Interpreter work bounds measured by heap allocations, not wall-clock time.
//!
//! This binary installs a counting global allocator, so it holds only tests that
//! read the count, one at a time.

use chimi::execution::{ExecutionConfig, ExecutionOutcome, ExecutionValue, TraceMode};
use chimi::source::SourceSnapshot;
use chimi::{Compiler, ProjectSource};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountingAllocator;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call forwards to the system allocator unchanged.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Allocations made while compiling and running `source` on the interpreter.
fn allocations_to_run(source: &str, expected: i64) -> usize {
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    let config = ExecutionConfig::new("allocations")
        .expect("valid execution id")
        .with_trace_mode(TraceMode::Off);
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    let report = Compiler::run_project(vec![module], config).expect("program compiles");
    let after = ALLOCATIONS.load(Ordering::Relaxed);
    assert_eq!(
        report.outcome,
        ExecutionOutcome::Success(ExecutionValue::Integer(expected))
    );
    after - before
}

/// A tree that holds the previous level twice, `depth` times over, so it has
/// `2^depth` leaves but only `depth + 1` distinct parts. Each pass reads the tree
/// out of a record state and a constructed value, and puts it in both.
fn doubling(depth: u32) -> String {
    format!(
        concat!(
            "enum Tree {{\n  Leaf\n  Node(left: Tree, right: Tree)\n}}\n",
            "struct Pair {{ tree: Tree, levels: Int }}\n",
            "fn grown(depth: Int) -> Pair {{\n",
            "  loop state = {{ left: depth, pair: Pair {{ tree: Leaf, levels: 0 }} }} {{\n",
            "    case state.left {{\n",
            "      0 -> break state.pair\n",
            "      _ -> {{\n",
            "        left: state.left - 1,\n",
            "        pair: Pair {{ tree: Node(state.pair.tree, state.pair.tree), levels: state.pair.levels + 1 }},\n",
            "      }}\n",
            "    }}\n",
            "  }}\n",
            "}}\n",
            "pub fn main() -> Int {{ grown({depth}).levels }}\n",
        ),
        depth = depth
    )
}

// ts-a512d8: records, tuples, and constructed values are shared, not copied, when a
// program refers to them, so growing a doubling tree costs the same allocations per
// level at every depth. Copying on reference would allocate about 2^depth parts per
// level: the 16-level run would cost hundreds of times what the 8-level run costs.
#[test]
fn building_a_doubling_tree_allocates_linearly_in_its_depth() {
    let base = allocations_to_run(&doubling(0), 0);
    let eight = allocations_to_run(&doubling(8), 8);
    let sixteen = allocations_to_run(&doubling(16), 16);
    let (first, second) = (eight.saturating_sub(base), sixteen.saturating_sub(eight));
    assert!(
        second <= 2 * first,
        "levels 9 to 16 allocated {second} times, levels 1 to 8 {first} times"
    );
}
