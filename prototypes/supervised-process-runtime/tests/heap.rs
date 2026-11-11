//! Heap decisions: pass cost independent of retained state, flat memory,
//! sharing-preserving copies, outer values untouched, foreign pointers caught.

use chimi_supervised_process_runtime_prototype::heap::*;

fn int(value: i64) -> *mut Value {
    unsafe { chimi_value_int(value) }
}

fn items(values: &[*mut Value]) -> *mut Value {
    unsafe { chimi_value_items(values.as_ptr(), values.len() as u32) }
}

fn item(list: *mut Value, index: usize) -> *mut Value {
    unsafe { *(*list).as_.items.add(index) }
}

fn integer(value: *mut Value) -> i64 {
    unsafe { (*value).as_.integer }
}

/// Builds a state of `size` distinct ints, then runs `passes` passes that each
/// allocate garbage and keep the state. Returns the heap's collection work,
/// after checking that the state survived the passes intact.
fn work_for_state(size: usize, passes: usize) -> HeapStats {
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
    let sum: i64 = survived.iter().map(|item| integer(*item)).sum();
    assert_eq!(sum, (size as i64) * (size as i64 - 1) / 2);
    heap.stats()
}

#[test]
fn pass_cost_does_not_grow_with_state_that_already_survived() {
    let size = 100_000;
    let small = work_for_state(10, 10_000);
    let large = work_for_state(size, 10_000);
    // The large state is copied and scanned a constant number of times (when
    // promoted, and by at most two full collections), not once per pass.
    assert!(
        large.copied_bytes < small.copied_bytes + 3 * size * 24,
        "small {small:?}, large {large:?}"
    );
    assert!(
        large.scanned_values < small.scanned_values + 3 * (size as u64 + 1),
        "small {small:?}, large {large:?}"
    );
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
fn inner_passes_never_move_values_of_the_enclosing_region_and_exit_hands_survivors_out() {
    let chunks_before = chunk_stats();
    {
        let heap = Heap::new(1);
        let _current = heap.enter();
        unsafe { chimi_region_enter() };
        // Young in the outer region, so only the region check keeps inner
        // passes from moving it.
        let outer_value = items(&[int(7), int(8)]);
        let mut outer_state = int(0);
        unsafe { chimi_region_enter() };
        let mut inner_state = items(&[outer_value, int(-1)]);
        for pass in 0..1_000 {
            let _garbage = items(&[int(pass)]);
            inner_state = items(&[outer_value, int(pass)]);
            assert_eq!(unsafe { chimi_region_pass(&mut inner_state) }, HeapStatus::Ok);
            assert_eq!(item(inner_state, 0), outer_value, "inner pass {pass} moved an outer value");
        }
        unsafe { chimi_region_exit() };
        assert_eq!(item(inner_state, 0), outer_value);
        assert_eq!((integer(item(outer_value, 0)), integer(item(outer_value, 1))), (7, 8));

        // The inner survivors now belong to the outer region, so its passes
        // must collect them like its own young values. Later garbage reuses any
        // chunk released by mistake, so a dangling survivor reads as corrupt.
        outer_state = items(&[inner_state, outer_state]);
        for pass in 0..200 {
            let _garbage = unsafe { chimi_value_text([b'x'; 4096].as_ptr(), 4096) };
            let _more = items(&[int(pass), int(pass)]);
            assert_eq!(unsafe { chimi_region_pass(&mut outer_state) }, HeapStatus::Ok);
        }
        let inner_result = item(outer_state, 0);
        let kept = item(inner_result, 0);
        assert_eq!((integer(item(kept, 0)), integer(item(kept, 1))), (7, 8));
        assert_eq!(integer(item(inner_result, 1)), 999);
        unsafe { chimi_region_exit() };
        assert_eq!(heap.stats().open_regions, 0);
    }
    assert_eq!(chunk_stats(), chunks_before, "a region exit lost or leaked chunks");
}

#[test]
fn adopted_fragments_join_the_innermost_region_and_its_passes_reclaim_them() {
    let sender = Heap::new(2);
    let source = {
        let _current = sender.enter();
        items(&[int(41), int(1)])
    };
    let receiver = Heap::new(1);
    let _current = receiver.enter();
    unsafe { chimi_region_enter() };
    let mut peak_chunks_at_warmup = 0;
    let mut peak_chunks = 0;
    for round in 0..10_000 {
        let fragment = {
            let _sending = sender.enter();
            unsafe { Fragment::copy(source) }.expect("owned graph")
        };
        let adopted = fragment.adopt();
        assert_eq!(unsafe { chimi_value_owner(adopted) }, 1);
        // Keep the newest message as the state; every older one is garbage.
        let mut state = items(&[adopted, int(round)]);
        assert_eq!(unsafe { chimi_region_pass(&mut state) }, HeapStatus::Ok);
        let message = item(state, 0);
        assert_eq!(integer(item(message, 0)) + integer(item(message, 1)), 42);
        peak_chunks = peak_chunks.max(chunk_stats().live_chunks);
        if round == 100 {
            peak_chunks_at_warmup = peak_chunks;
        }
    }
    assert_eq!(peak_chunks, peak_chunks_at_warmup, "adopted chunks were not reclaimed");
    unsafe { chimi_region_exit() };
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
    let fragment = unsafe { Fragment::copy(value) }.expect("owned graph");
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
    assert_eq!(unsafe { Fragment::copy(mixed) }.unwrap_err(), HeapStatus::ForeignPointer);
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
        let _unread = unsafe { Fragment::copy(value) }.expect("owned graph");
    }
    assert_eq!(chunk_stats(), before);
}
