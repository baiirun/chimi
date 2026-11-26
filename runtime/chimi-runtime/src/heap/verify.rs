//! A check of one region right after a collection pass (CHI-305 step 5,
//! phase 0): the collector's oracle while it moves from C to Rust. It is a
//! second, independent reading of the heap through the layout mirrors, so
//! it catches a pass that freed what is still reachable, left an allocation
//! in the region's young generation, or miscounted the survivors, whichever
//! language the pass ran in.
//!
//! `runtime.c` calls it after every pass in a checking build
//! (`CHIMI_HEAP_VERIFY`, on by default under AddressSanitizer). A minor pass
//! checks what it promoted, and that this is all it promoted, so the check
//! costs what the pass did; a full pass or a region's end checks every
//! survivor, that each was marked, and that each pointer into the region
//! names a survivor.
//!
//! The pass tells the check, by address, what it freed and which young
//! chunks it emptied, so a pointer to either is reported without reading
//! the memory it names; a target's header is read only once the pointer is
//! known to name live memory.
//!
//! What a pointer from a survivor may name, besides another survivor of the
//! same region: an allocation of an enclosing region or of the heap's base
//! (older, so still live), a static constant, or anything outside the heap,
//! which the pass itself reports as foreign. It may not name the region's own
//! young area, which the pass has just emptied, or a region of the same heap
//! that is not an enclosing one, which has ended.

use alloc::vec::Vec;
use core::ptr;

use super::layout::{
    AllocationArea, AllocationHeader, AllocationKind, AllocationScope, AreaRole, Callable, Control,
    Environment, Field, ListNode, MapNode, RuntimeHeap, Value, ValueKind,
};

/// Why a region failed the check; `runtime.c` prints it and aborts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Failure {
    /// The young area or its adopted areas still hold allocations.
    YoungNotEmpty = 1,
    /// A survivor's header names another area, or a kind out of range.
    BadSurvivorHeader = 2,
    /// The survivor area's byte or allocation count is not the sum of its
    /// list.
    SurvivorCountsWrong = 3,
    /// A pointer names an allocation of another kind than its slot holds.
    WrongKind = 4,
    /// A pointer names this region's young area, emptied by the pass.
    PointsIntoYoung = 5,
    /// A pointer names a region of this heap that has ended.
    PointsIntoEndedRegion = 6,
    /// A pointer names an allocation of this region that is not a survivor:
    /// the pass freed something still reachable.
    PointsToFreed = 7,
    /// An allocation still sits in a young chunk after the pass.
    StillInChunk = 8,
    /// A full pass kept a survivor it did not mark.
    KeptUnmarked = 9,
    /// The allocations a minor pass marked do not add up to what it
    /// promoted.
    PromotionsWrong = 10,
}

impl Failure {
    /// Every failure, for turning a code back into one.
    pub const ALL: [Self; 10] = [
        Self::YoungNotEmpty,
        Self::BadSurvivorHeader,
        Self::SurvivorCountsWrong,
        Self::WrongKind,
        Self::PointsIntoYoung,
        Self::PointsIntoEndedRegion,
        Self::PointsToFreed,
        Self::StillInChunk,
        Self::KeptUnmarked,
        Self::PromotionsWrong,
    ];
}

impl Failure {
    pub fn message(self) -> &'static core::ffi::CStr {
        match self {
            Self::YoungNotEmpty => c"the young generation is not empty after the pass",
            Self::BadSurvivorHeader => c"a survivor's header names another area or no kind",
            Self::SurvivorCountsWrong => c"the survivor counts do not match the survivor list",
            Self::WrongKind => c"a pointer names an allocation of the wrong kind",
            Self::PointsIntoYoung => c"a pointer names the emptied young generation",
            Self::PointsIntoEndedRegion => c"a pointer names a region that has ended",
            Self::PointsToFreed => c"a pointer names an allocation the pass freed",
            Self::StillInChunk => c"an allocation is still in a young chunk after the pass",
            Self::KeptUnmarked => c"a full pass kept an allocation it did not mark",
            Self::PromotionsWrong => c"the allocations a minor pass marked do not match what it promoted",
        }
    }
}

/// What one pass did, as `runtime.c` reports it.
pub struct Pass<'a> {
    /// The pass's mark epoch.
    pub epoch: u64,
    /// Whether it marked and swept the survivors too.
    pub full: bool,
    /// The bytes it moved to the survivors.
    pub promoted: usize,
    /// The payload addresses of the allocations it freed, sorted.
    pub freed: &'a [usize],
    /// The address ranges, start inclusive and end exclusive, of the young
    /// chunks it emptied.
    pub young_chunks: &'a [(usize, usize)],
}

/// The header in front of an allocation's payload.
fn header_of(payload: *const u8) -> *const AllocationHeader {
    payload.wrapping_sub(size_of::<AllocationHeader>()).cast()
}

struct Check<'a> {
    scope: &'a AllocationScope,
    heap: *const RuntimeHeap,
    pass: &'a Pass<'a>,
    /// Payload addresses of the region's survivors, sorted; filled only for a
    /// full check.
    survivors: Option<Vec<usize>>,
}

impl Check<'_> {
    /// Whether `area` is an area of a region enclosing this one, of the
    /// heap's base, or adopted by one of those.
    ///
    /// # Safety
    /// The region's ancestors and heap are live.
    unsafe fn is_older_area(&self, area: *const AllocationArea) -> bool {
        // SAFETY: the caller's contract covers every scope and heap read.
        unsafe {
            let heap = &*self.heap;
            if ptr::eq(area, &heap.base) || in_list(heap.adopted, area) {
                return true;
            }
            let mut ancestor = self.scope.parent.cast_const();
            while let Some(scope) = ancestor.as_ref() {
                if ptr::eq(area, &scope.young) || ptr::eq(area, &scope.survivors) || in_list(scope.adopted, area) {
                    return true;
                }
                ancestor = scope.parent;
            }
            false
        }
    }

    /// Checks one pointer of a live allocation, `expected` being the kind its
    /// slot holds.
    ///
    /// # Safety
    /// Every allocation the region can reach is readable: that is what the
    /// check assumes in order to test it.
    unsafe fn child(&self, payload: *const u8, expected: AllocationKind) -> Result<(), Failure> {
        if payload.is_null() {
            return Ok(());
        }
        let address = payload as usize;
        if self.pass.freed.binary_search(&address).is_ok() {
            return Err(Failure::PointsToFreed);
        }
        if self.pass.young_chunks.iter().any(|&(start, end)| (start..end).contains(&address)) {
            return Err(Failure::PointsIntoYoung);
        }
        // Each field is read on its own through the raw pointer, so a bug
        // that left a stale header (an `in_chunk` byte that is no bool, say)
        // gives a report rather than undefined behavior.
        // SAFETY: the caller's contract; the pointer names no memory this
        // pass freed or emptied.
        let (kind, area, in_chunk) = unsafe {
            let header = header_of(payload);
            (
                ptr::addr_of!((*header).kind).read(),
                ptr::addr_of!((*header).area).read().cast_const(),
                ptr::addr_of!((*header).in_chunk).cast::<u8>().read() != 0,
            )
        };
        if kind != expected {
            return Err(Failure::WrongKind);
        }
        if area.is_null() || ptr::eq(area, &self.scope.young) {
            return Err(Failure::PointsIntoYoung);
        }
        if ptr::eq(area, &self.scope.survivors) {
            if in_chunk {
                return Err(Failure::StillInChunk);
            }
            let missing = self
                .survivors
                .as_ref()
                .is_some_and(|survivors| survivors.binary_search(&address).is_err());
            return if missing { Err(Failure::PointsToFreed) } else { Ok(()) };
        }
        // SAFETY: the caller's contract covers the ancestors and the heap.
        if unsafe { self.is_older_area(area) } {
            return Ok(());
        }
        // Not this region and not an older one: a static constant, a value
        // of another heap (which the pass reports as foreign), or a region
        // of this heap that has ended.
        // SAFETY: the caller's contract.
        let (heap, role) =
            unsafe { (ptr::addr_of!((*area).heap).read().cast_const(), ptr::addr_of!((*area).role).read()) };
        let ended = ptr::eq(heap, self.heap) && (role == AreaRole::YOUNG || role == AreaRole::SURVIVOR);
        if ended { Err(Failure::PointsIntoEndedRegion) } else { Ok(()) }
    }

    /// Checks every pointer the allocation at `payload` holds, following the
    /// same slots the collector marks.
    ///
    /// # Safety
    /// As for [`Check::child`].
    unsafe fn children(&self, payload: *const u8) -> Result<(), Failure> {
        use AllocationKind as Kind;
        // SAFETY: the caller's contract; each cast reads the payload as the
        // kind its header names.
        unsafe {
            let header = &*header_of(payload);
            let child = |pointer: *const u8, kind| self.child(pointer, kind);
            match header.kind {
                Kind::VALUE => {
                    let value = &*payload.cast::<Value>();
                    let p = &value.payload;
                    match value.kind {
                        ValueKind::TEXT | ValueKind::BYTES => child(p.text.bytes.cast(), Kind::BYTES),
                        ValueKind::JSON_NUMBER => child(p.json_number.cast(), Kind::BYTES),
                        ValueKind::LIST => {
                            child(p.list.vector.root.cast(), Kind::LIST_NODE)?;
                            child(p.list.vector.tail.cast(), Kind::LIST_NODE)
                        }
                        ValueKind::RECORD | ValueKind::TUPLE | ValueKind::CAUSE => {
                            child(p.record.fields.cast(), Kind::FIELDS)
                        }
                        ValueKind::CONSTRUCTED => child(p.constructed.fields.cast(), Kind::FIELDS),
                        ValueKind::CALLABLE => child(p.callable.cast(), Kind::CALLABLE),
                        ValueKind::MAP | ValueKind::SET => child(p.map.cast(), Kind::MAP_NODE),
                        _ => Ok(()),
                    }
                }
                Kind::LIST_NODE => {
                    let node = &*payload.cast::<ListNode>();
                    let kind = if node.height == 0 { Kind::VALUE } else { Kind::LIST_NODE };
                    // From the payload pointer, not `node.slots`, whose
                    // borrow covers no slots.
                    let slots = payload.add(core::mem::offset_of!(ListNode, slots)).cast::<*const u8>();
                    for index in 0..node.count as usize {
                        child(*slots.add(index), kind)?;
                    }
                    Ok(())
                }
                Kind::FIELDS => {
                    let fields = payload.cast::<Field>();
                    for index in 0..header.bytes / size_of::<Field>() {
                        child((*fields.add(index)).value.cast(), Kind::VALUE)?;
                    }
                    Ok(())
                }
                Kind::VALUE_POINTERS => {
                    let values = payload.cast::<*const Value>();
                    for index in 0..header.bytes / size_of::<*const Value>() {
                        child((*values.add(index)).cast(), Kind::VALUE)?;
                    }
                    Ok(())
                }
                Kind::ENVIRONMENT => {
                    let environment = &*payload.cast::<Environment>();
                    child(environment.locals.cast(), Kind::VALUE_POINTERS)?;
                    child(environment.requirements.cast(), Kind::VALUE_POINTERS)
                }
                Kind::CALLABLE => {
                    let callable = &*payload.cast::<Callable>();
                    child(callable.environment.cast(), Kind::ENVIRONMENT)?;
                    child(callable.inner.cast(), Kind::CALLABLE)
                }
                Kind::MAP_NODE => {
                    let node = &*payload.cast::<MapNode>();
                    child(node.key.cast(), Kind::VALUE)?;
                    child(node.value.cast(), Kind::VALUE)?;
                    child(node.left.cast(), Kind::MAP_NODE)?;
                    child(node.right.cast(), Kind::MAP_NODE)
                }
                Kind::CONTROL => {
                    let control = &*payload.cast::<Control>();
                    child(control.value.cast(), Kind::VALUE)?;
                    child(control.cause.cast(), Kind::VALUE)
                }
                Kind::BYTES => Ok(()),
                _ => Err(Failure::BadSurvivorHeader),
            }
        }
    }
}

/// Whether `area` is on the list of areas that starts at `first`.
///
/// # Safety
/// Every area on the list is live.
unsafe fn in_list(mut next: *mut AllocationArea, area: *const AllocationArea) -> bool {
    // SAFETY: the caller's contract.
    while let Some(candidate) = unsafe { next.as_ref() } {
        if ptr::eq(candidate, area) {
            return true;
        }
        next = candidate.next;
    }
    false
}

/// Checks `scope` right after `pass`, which kept what `value` and `control`
/// reach. A full pass checks every survivor, the survivor counts, that each
/// survivor was marked, and that every pointer into the region names a
/// survivor; a minor pass checks the allocations it promoted, which lead the
/// survivor list, and that they add up to `pass.promoted`.
///
/// # Safety
/// `scope` is the region the pass just collected, with its ancestors and
/// heap live, and `value` and `control` are null or the pass's roots.
pub unsafe fn verify(
    scope: *const AllocationScope,
    value: *const Value,
    control: *const Control,
    pass: &Pass<'_>,
) -> Result<(), Failure> {
    // SAFETY: the caller's contract covers every read below.
    unsafe {
        let scope = &*scope;
        if !scope.young.allocations.is_null() || scope.young.bytes != 0 || scope.young.count != 0 {
            return Err(Failure::YoungNotEmpty);
        }
        if !scope.adopted.is_null() {
            return Err(Failure::YoungNotEmpty);
        }
        let survivors_area: *const AllocationArea = &scope.survivors;
        let mut payloads = Vec::new();
        let (mut bytes, mut count) = (0usize, 0usize);
        let mut header = scope.survivors.allocations.cast_const();
        while let Some(current) = header.as_ref() {
            // More links than the area counts: a cycle, or a wrong count.
            if count == scope.survivors.count {
                return Err(Failure::SurvivorCountsWrong);
            }
            if !ptr::eq(current.area, survivors_area) || current.kind.0 as usize >= AllocationKind::COUNT {
                return Err(Failure::BadSurvivorHeader);
            }
            if ptr::addr_of!(current.in_chunk).cast::<u8>().read() != 0 {
                return Err(Failure::StillInChunk);
            }
            if current.mark_epoch != pass.epoch {
                if pass.full {
                    return Err(Failure::KeptUnmarked);
                }
                // A minor pass links what it promotes to the front of the
                // list, marked with its epoch, so it checks that prefix and
                // stops: walking every survivor on every pass would make the
                // check cost far more than the pass.
                break;
            }
            bytes += current.bytes;
            count += 1;
            payloads.push(header.add(1).cast::<u8>());
            header = current.next;
        }
        if pass.full {
            if bytes != scope.survivors.bytes || count != scope.survivors.count {
                return Err(Failure::SurvivorCountsWrong);
            }
        } else if bytes != pass.promoted {
            return Err(Failure::PromotionsWrong);
        }
        let mut sorted = None;
        if pass.full {
            let mut addresses: Vec<usize> = payloads.iter().map(|payload| *payload as usize).collect();
            addresses.sort_unstable();
            sorted = Some(addresses);
        }
        let check = Check { scope, heap: scope.heap, pass, survivors: sorted };
        check.child(value.cast(), AllocationKind::VALUE)?;
        check.child(control.cast(), AllocationKind::CONTROL)?;
        for payload in payloads {
            check.children(payload)?;
        }
        Ok(())
    }
}
