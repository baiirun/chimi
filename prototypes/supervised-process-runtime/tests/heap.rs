//! Heap decisions: pass cost independent of retained state, flat memory,
//! sharing-preserving copies, outer values untouched, foreign pointers caught.

use chimi_supervised_process_runtime_prototype::heap::*;

fn int(value: i64) -> *mut Value {
    unsafe { chimi_value_int(value) }
}

fn items(values: &[*mut Value]) -> *mut Value {
    unsafe { chimi_value_items(values.as_ptr(), values.len() as u32) }
}

/// Builds a state of `size` distinct ints, then runs `passes` passes that each
/// allocate garbage and keep the state. Returns bytes copied by collection,
/// after checking that the state survived the passes intact.
fn copied_bytes_for_state(size: usize, passes: usize) -> usize {
    let heap = Heap::new(1);
    let _current = heap.enter();
    unsafe { chimi_region_enter() };
    let mut state = items(&(0..size as i64).map(int).collect::<Vec<_>>());
    for pass in 0..passes {
        let _garbage = items(&[int(pass as i64), int(pass as i64 + 1)]);
        assert_eq!(unsafe { chimi_region_pass(&mut state) }, HeapStatus::Ok);
    }
    unsafe { chimi_region_exit() };
    let survived = unsafe { std::slice::from_raw_parts((*state).as_.items, size) };
    let sum: i64 = survived.iter().map(|item| unsafe { (**item).as_.integer }).sum();
    assert_eq!(sum, (size as i64) * (size as i64 - 1) / 2);
    heap.stats().copied_bytes
}

#[test]
fn pass_cost_does_not_grow_with_state_that_already_survived() {
    let small = copied_bytes_for_state(10, 10_000);
    let large = copied_bytes_for_state(100_000, 10_000);
    // The large state is copied at most a constant number of times, not per pass.
    assert!(large < small + 8 * 100_000 * 64, "small {small}, large {large}");
}

#[test]
fn a_million_passes_keep_peak_memory_flat() {
    let heap = Heap::new(1);
    let _current = heap.enter();
    unsafe { chimi_region_enter() };
    let mut state = int(0);
    let mut peak_after_warmup = 0;
    let mut chunks_after_warmup = ChunkStats::default();
    for pass in 0..1_000_000_i64 {
        let next = unsafe { (*state).as_.integer } + 1;
        let _garbage = items(&[int(pass), int(pass)]);
        state = int(next);
        assert_eq!(unsafe { chimi_region_pass(&mut state) }, HeapStatus::Ok);
        if pass == 1_000 {
            peak_after_warmup = heap.stats().peak_live_bytes;
            chunks_after_warmup = chunk_stats();
        }
    }
    // Chunk memory, not only value bytes, stays where warmup left it.
    assert_eq!(chunk_stats(), chunks_after_warmup);
    unsafe { chimi_region_exit() };
    assert_eq!(unsafe { (*state).as_.integer }, 1_000_000);
    assert_eq!(heap.stats().peak_live_bytes, peak_after_warmup);
}

#[test]
fn values_allocated_before_the_loop_survive_its_collections() {
    let heap = Heap::new(1);
    let _current = heap.enter();
    let outer = items(&[int(7), int(8)]);
    unsafe { chimi_region_enter() };
    let mut state = int(0);
    for _ in 0..1_000 {
        let _garbage = items(&[int(1)]);
        assert_eq!(unsafe { chimi_region_pass(&mut state) }, HeapStatus::Ok);
    }
    unsafe { chimi_region_exit() };
    let first = unsafe { *(*outer).as_.items };
    assert_eq!(unsafe { (*first).as_.integer }, 7);
}

#[test]
fn copying_preserves_sharing() {
    let heap = Heap::new(1);
    let _current = heap.enter();
    // Each level points twice to the level below: 2^40 paths, 41 distinct values.
    let mut value = int(1);
    for _ in 0..40 {
        value = items(&[value, value]);
    }
    let fragment = Fragment::copy(value).expect("owned graph");
    let receiver = Heap::new(2);
    let _receiving = receiver.enter();
    let copied = fragment.adopt();
    let children = unsafe { std::slice::from_raw_parts((*copied).as_.items, 2) };
    assert_eq!(children[0], children[1]);
    assert!(receiver.stats().live_bytes < 64 * 1024);
    assert_eq!(unsafe { chimi_value_owner(copied) }, 2);
}

#[test]
fn a_pointer_into_another_heap_is_reported_not_followed() {
    let other = Heap::new(2);
    let foreign = {
        let _current = other.enter();
        int(5)
    };
    let heap = Heap::new(1);
    let _current = heap.enter();
    let mixed = items(&[int(1), foreign]);
    assert_eq!(Fragment::copy(mixed).unwrap_err(), HeapStatus::ForeignPointer);
    unsafe { chimi_region_enter() };
    let mut state = items(&[foreign]);
    assert_eq!(unsafe { chimi_region_pass(&mut state) }, HeapStatus::ForeignPointer);
    unsafe { chimi_region_exit() };
}

#[test]
fn destroying_heaps_and_unadopted_fragments_frees_every_chunk() {
    let before = chunk_stats();
    {
        let heap = Heap::new(1);
        let _current = heap.enter();
        let value = items(&[int(1), int(2)]);
        let _unread = Fragment::copy(value).expect("owned graph");
    }
    assert_eq!(chunk_stats(), before);
}
