//! The report files a run writes at exit (CHI-305 step 5, phase 8): the
//! operation profile (`--profile-file`) and the memory report
//! (`--memory-report-file`).

use alloc::vec::Vec;
use core::ffi::{CStr, c_char, c_void};

use crate::heap::allocator::{carrier_count, current_carrier, folded_peaks, footprint_add, profile_rows};
use crate::heap::layout::{AllocationKind, Carrier, HeapTotals, Node, Origin, Runtime};
use crate::telemetry::Out;

unsafe extern "C" {
    static CHIMI_NODES: [Node; 0];
    static CHIMI_ORIGINS: [Origin; 0];
    static CHIMI_NODE_COUNT: u32;
    fn chimi_runtime_exhausted_memory() -> !;
    fn chimi_runtime_carrier_at(index: usize) -> *mut Carrier;
    /// Weak in runtime.c, returning 0, where no fiber archive is linked.
    fn chimi_scheduler_live_memberships() -> usize;
    fn chimi_scheduler_peak_task_records() -> usize;
}

/// A report file opened with `mode`, closed in every child process the run
/// starts (std/exec), so a child cannot hold it open or write into it.
///
/// # Safety
/// `path` and `mode` are C strings.
unsafe fn open_report(path: *const c_char, mode: &CStr) -> *mut libc::FILE {
    // SAFETY: the caller's contract.
    unsafe {
        let output = libc::fopen(path, mode.as_ptr());
        if !output.is_null() && libc::fcntl(libc::fileno(output), libc::F_SETFD, libc::FD_CLOEXEC) != 0 {
            let error = *errno();
            libc::fclose(output);
            *errno() = error;
            return core::ptr::null_mut();
        }
        output
    }
}

#[cfg(target_os = "linux")]
fn errno() -> *mut i32 {
    // SAFETY: the calling thread's errno.
    unsafe { libc::__errno_location() }
}

#[cfg(not(target_os = "linux"))]
fn errno() -> *mut i32 {
    // SAFETY: the calling thread's errno.
    unsafe { libc::__error() }
}

/// Flushes and closes `output`; whether either failed.
///
/// # Safety
/// `output` is an open stream.
unsafe fn close_output(output: *mut libc::FILE) -> bool {
    // SAFETY: the caller's contract.
    unsafe {
        let failed = libc::fflush(output) != 0;
        (libc::fclose(output) != 0) || failed
    }
}

/// `cannot open <what> file <path>: <reason>` on stderr, for the `errno` of
/// the failed open.
///
/// # Safety
/// `path` is a C string.
unsafe fn report_open_failure(what: &[u8], path: *const c_char) {
    // SAFETY: the caller's contract; strerror returns a C string.
    unsafe {
        let reason = CStr::from_ptr(libc::strerror(*errno())).to_bytes();
        let mut line = Vec::new();
        for part in [b"cannot open ".as_slice(), what, b" file ", CStr::from_ptr(path).to_bytes(), b": ", reason, b"\n"] {
            line.extend_from_slice(part);
        }
        libc::write(2, line.as_ptr().cast(), line.len());
    }
}

/// A profile row's sort key.
struct Ranked {
    allocated_bytes: u64,
    evaluations: u64,
    node: u32,
}

/// Most allocated bytes first, then most evaluations, then node id.
unsafe extern "C" fn compare_ranked(left: *const c_void, right: *const c_void) -> i32 {
    // SAFETY: qsort passes two elements of the `Ranked` array.
    let (x, y) = unsafe { (&*left.cast::<Ranked>(), &*right.cast::<Ranked>()) };
    let order = y.allocated_bytes.cmp(&x.allocated_bytes).then(y.evaluations.cmp(&x.evaluations)).then(x.node.cmp(&y.node));
    order as i32
}

/// Writes the profile to `path`, a new file: one JSON object with every
/// evaluated node, most allocated bytes first, then most evaluations, then
/// node id, so the same program always writes the same profile. Returns
/// whether writing failed; without a path there is nothing to write.
///
/// # Safety
/// `path` is null or a C string; no fiber runs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_write_profile_v1(path: *const c_char) -> bool {
    if path.is_null() {
        return false;
    }
    // SAFETY: the caller's contract; the profile has a row per node.
    unsafe {
        let output = open_report(path, c"wx");
        if output.is_null() {
            report_open_failure(b"profile", path);
            return true;
        }
        let rows = profile_rows();
        let count = CHIMI_NODE_COUNT as usize;
        let mut order: Vec<Ranked> = Vec::new();
        if order.try_reserve_exact(count).is_err() {
            chimi_runtime_exhausted_memory();
        }
        order.extend((0..CHIMI_NODE_COUNT).filter(|&node| (*rows.add(node as usize)).evaluations != 0).map(|node| {
            let row = &*rows.add(node as usize);
            Ranked { allocated_bytes: row.allocated_bytes, evaluations: row.evaluations, node }
        }));
        // libc's qsort rather than slice sorting, whose generic code would
        // add about 13 KB to every program; the order is total, so both agree.
        libc::qsort(order.as_mut_ptr().cast(), order.len(), size_of::<Ranked>(), Some(compare_ranked));
        let out = Out(output);
        out.bytes(b"{\"schema\":\"chimi.runtime-profile.v1\",\"operations\":[");
        for (index, &Ranked { node, .. }) in order.iter().enumerate() {
            let row = &*rows.add(node as usize);
            let shape = &*(&raw const CHIMI_NODES).cast::<Node>().add(node as usize);
            let origin = &*(&raw const CHIMI_ORIGINS).cast::<Origin>().add(shape.origin as usize);
            out.bytes(if index > 0 { b",{\"core_node_id\":" } else { b"{\"core_node_id\":" });
            out.unsigned(u64::from(node));
            for (key, value) in [
                (&b",\"origin_id\":"[..], shape.origin),
                (b",\"file_id\":", origin.file_id),
                (b",\"source_start\":", origin.start),
                (b",\"source_end\":", origin.end),
            ] {
                out.bytes(key);
                out.unsigned(u64::from(value));
            }
            out.bytes(b",\"operation\":");
            out.json_c(row.operation);
            for (key, value) in [
                (&b",\"evaluations\":"[..], row.evaluations),
                (b",\"allocations\":", row.allocations),
                (b",\"allocated_bytes\":", row.allocated_bytes),
            ] {
                out.bytes(key);
                out.unsigned(value);
            }
            out.bytes(b"}");
        }
        out.bytes(b"]}\n");
        close_output(output)
    }
}

/// Writes the memory report to `path`, a new file: one JSON object summing
/// every carrier's allocation counts, with the largest of each peak, or the
/// folded process-wide peaks with several carrier threads. Only the process
/// kernel variant (`process_kernel`) has process heaps and memberships to
/// report. Returns whether writing failed; without a path there is nothing
/// to write.
///
/// # Safety
/// `path` is null or a C string, `runtime` the finished `Runtime`; no fiber
/// runs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_write_memory_report_v1(
    path: *const c_char,
    runtime: *const Runtime,
    process_kernel: bool,
) -> bool {
    if path.is_null() {
        return false;
    }
    // SAFETY: the caller's contract; every carrier is idle, so its counts are
    // final.
    unsafe {
        let output = open_report(path, c"wx");
        if output.is_null() {
            report_open_failure(b"memory report", path);
            return true;
        }
        footprint_add(current_carrier(), 0); // records the peak
        let mut sum = MemorySum::default();
        for index in 0..carrier_count() {
            sum.add(&*chimi_runtime_carrier_at(index));
        }
        raise_to_folded_peaks(
            &mut sum.peak_footprint_bytes,
            sum.footprint_bytes,
            &mut sum.peak_fragment_footprint_bytes,
            sum.fragment_footprint_bytes,
        );
        let by_kind = |kind: AllocationKind| sum.allocated_bytes_by_kind[kind.0 as usize] as u64;
        let out = Out(output);
        let fields: [(&[u8], u64); 27] = [
            (b"{\"schema\":\"chimi.runtime-memory.v1\",\"scoped_total_allocated_bytes\":", sum.total_allocated_bytes as u64),
            (b",\"scoped_allocated_bytes_by_kind\":{\"bytes\":", by_kind(AllocationKind::BYTES)),
            (b",\"values\":", by_kind(AllocationKind::VALUE)),
            (b",\"controls\":", by_kind(AllocationKind::CONTROL)),
            (
                b",\"environments\":",
                by_kind(AllocationKind::ENVIRONMENT).wrapping_add(by_kind(AllocationKind::FRAME_ENVIRONMENT)),
            ),
            (b",\"value_pointers\":", by_kind(AllocationKind::VALUE_POINTERS)),
            (b",\"fields\":", by_kind(AllocationKind::FIELDS)),
            (b",\"callables\":", by_kind(AllocationKind::CALLABLE)),
            (b",\"map_nodes\":", by_kind(AllocationKind::MAP_NODE)),
            (b",\"list_nodes\":", by_kind(AllocationKind::LIST_NODE)),
            (b"},\"scoped_peak_live_bytes\":", sum.peak_live_bytes as u64),
            (b",\"scoped_retained_bytes\":", sum.retained_bytes as u64),
            (b",\"scoped_collections\":", sum.collections),
            (b",\"scoped_full_collections\":", sum.full_collections),
            (b",\"scoped_scanned_allocations\":", sum.scanned_allocations),
            (b",\"unscoped_allocated_bytes\":", sum.unscoped_allocated_bytes),
            (b",\"peak_footprint_bytes\":", sum.peak_footprint_bytes as u64),
            (b",\"peak_fragment_footprint_bytes\":", sum.peak_fragment_footprint_bytes as u64),
            (b",\"capture_transfer_probes\":", sum.capture_transfer_probes),
            // Reported per allocation: young chunks are not counted separately.
            (b",\"heap_chunks_live\":", sum.live_allocations as u64),
            (b",\"heap_chunk_bytes_peak\":", sum.peak_footprint_bytes as u64),
            (b",\"fragments_live\":", sum.live_fragments as u64),
            (b",\"fragment_bytes_peak\":", sum.peak_fragment_footprint_bytes as u64),
            (
                b",\"process_heap_peak_value_bytes\":",
                if process_kernel { sum.process_heap_peak_value_bytes as u64 } else { 0 },
            ),
            (b",\"copied_values\":", sum.copied_values),
            (b",\"copied_bytes\":", sum.copied_bytes),
            (b",\"trace_events_constructed\":", (*runtime).trace_events_constructed),
        ];
        for (key, value) in fields {
            out.bytes(key);
            out.unsigned(value);
        }
        out.bytes(b",\"memberships_live\":");
        out.unsigned(if process_kernel { chimi_scheduler_live_memberships() as u64 } else { 0 });
        out.bytes(b",\"task_records_peak\":");
        out.unsigned(chimi_scheduler_peak_task_records() as u64);
        out.bytes(b"}\n");
        close_output(output)
    }
}

/// With several carrier threads, replaces summed peaks (each one carrier's)
/// with the process-wide peaks the carriers fold, raised to the exact sum
/// now, which is read once no fiber runs. They are then within
/// `carrier_count * FOOTPRINT_FOLD_SLACK` of the true peak.
fn raise_to_folded_peaks(peak: &mut i64, footprint: i64, fragment_peak: &mut i64, fragment_footprint: i64) {
    if let Some((folded, folded_fragments)) = folded_peaks() {
        *peak = folded.max(footprint);
        *fragment_peak = folded_fragments.max(fragment_footprint);
    }
}

/// [`raise_to_folded_peaks`] on summed `HeapTotals`, for the carrier-thread
/// stress fixture (tests/fixtures/carrier-threads-stress.c), which checks
/// the report's peaks against the true ones.
///
/// # Safety
/// `totals` is a live record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_report_process_peaks_v1(totals: *mut HeapTotals) {
    // SAFETY: the caller's contract.
    let totals = unsafe { &mut *totals };
    raise_to_folded_peaks(
        &mut totals.peak_footprint_bytes,
        totals.footprint_bytes,
        &mut totals.peak_fragment_footprint_bytes,
        totals.fragment_footprint_bytes,
    );
}

/// Every carrier's memory counts, summed, with the largest of each peak.
#[derive(Default)]
struct MemorySum {
    live_allocations: i64,
    footprint_bytes: i64,
    peak_footprint_bytes: i64,
    live_fragments: i64,
    fragment_footprint_bytes: i64,
    peak_fragment_footprint_bytes: i64,
    total_allocated_bytes: usize,
    allocated_bytes_by_kind: [usize; AllocationKind::COUNT],
    peak_live_bytes: usize,
    retained_bytes: usize,
    collections: u64,
    full_collections: u64,
    scanned_allocations: u64,
    unscoped_allocated_bytes: u64,
    capture_transfer_probes: u64,
    copied_values: u64,
    copied_bytes: u64,
    process_heap_peak_value_bytes: usize,
}

impl MemorySum {
    /// Adds one carrier's counts; its footprint includes the young bytes it
    /// still defers.
    fn add(&mut self, carrier: &Carrier) {
        let heap = &carrier.heap_totals;
        let scopes = &carrier.scope_totals;
        self.live_allocations = self.live_allocations.wrapping_add(heap.live_allocations);
        self.footprint_bytes =
            self.footprint_bytes.wrapping_add(heap.footprint_bytes).wrapping_add(carrier.pending_footprint_bytes);
        self.peak_footprint_bytes = self.peak_footprint_bytes.max(heap.peak_footprint_bytes);
        self.live_fragments = self.live_fragments.wrapping_add(heap.live_fragments);
        self.fragment_footprint_bytes = self.fragment_footprint_bytes.wrapping_add(heap.fragment_footprint_bytes);
        self.peak_fragment_footprint_bytes = self.peak_fragment_footprint_bytes.max(heap.peak_fragment_footprint_bytes);
        self.total_allocated_bytes = self.total_allocated_bytes.wrapping_add(scopes.total_allocated_bytes);
        for (total, own) in self.allocated_bytes_by_kind.iter_mut().zip(scopes.allocated_bytes_by_kind) {
            *total = total.wrapping_add(own);
        }
        self.peak_live_bytes = self.peak_live_bytes.max(scopes.peak_live_bytes);
        self.retained_bytes = self.retained_bytes.wrapping_add(scopes.retained_bytes);
        self.collections = self.collections.wrapping_add(scopes.collections);
        self.full_collections = self.full_collections.wrapping_add(scopes.full_collections);
        self.scanned_allocations = self.scanned_allocations.wrapping_add(scopes.scanned_allocations);
        self.unscoped_allocated_bytes = self.unscoped_allocated_bytes.wrapping_add(carrier.unscoped_allocated_bytes);
        self.capture_transfer_probes = self.capture_transfer_probes.wrapping_add(carrier.capture_transfer_probes);
        self.copied_values = self.copied_values.wrapping_add(carrier.copied_values);
        self.copied_bytes = self.copied_bytes.wrapping_add(carrier.copied_bytes);
        self.process_heap_peak_value_bytes = self.process_heap_peak_value_bytes.max(carrier.process_heap_peak_value_bytes);
    }
}
