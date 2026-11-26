//! The `no_std` shell around `chimi-runtime` for programs without the
//! scheduler: allocation goes to the C library's `malloc`, as the C runtime's
//! own scratch memory does, running out of memory ends the program as the C
//! runtime's own allocations do, and a panic aborts the process.

// Its unit-test build (`cargo clippy --all-targets`) links `std`, which brings
// its own allocator and panic handler, so the shell is left out of it.
#![cfg_attr(not(test), no_std)]

extern crate chimi_runtime;

#[cfg(not(test))]
mod shell {
    use core::alloc::{GlobalAlloc, Layout};

    unsafe extern "C" {
        fn malloc(size: usize) -> *mut u8;
        fn realloc(pointer: *mut u8, size: usize) -> *mut u8;
        fn free(pointer: *mut u8);
        fn aligned_alloc(alignment: usize, size: usize) -> *mut u8;
        fn abort() -> !;
        /// runtime.c: prints "Chimi runtime exhausted memory" and exits 70.
        fn chimi_runtime_exhausted_memory() -> !;
    }

    /// Rust's own response to a failed allocation is a panic, which here
    /// aborts with no message; the C runtime's is to report it and exit 70.
    fn exhausted(pointer: *mut u8) -> *mut u8 {
        if pointer.is_null() {
            // The exit runs the console drain, which may reach the value
            // whose call is allocating.
            chimi_runtime::note_exiting();
            unsafe { chimi_runtime_exhausted_memory() }
        }
        pointer
    }

    /// `malloc` returns memory aligned for any fundamental type; larger
    /// alignments go to `aligned_alloc`, which `realloc` must not resize.
    const MALLOC_ALIGNMENT: usize = 2 * core::mem::size_of::<usize>();

    struct CAllocator;

    // SAFETY: each method forwards to the C library allocator, which returns
    // memory of at least `size` bytes at the requested alignment, or null.
    unsafe impl GlobalAlloc for CAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            exhausted(if layout.align() <= MALLOC_ALIGNMENT {
                unsafe { malloc(layout.size()) }
            } else {
                // aligned_alloc wants a size that is a multiple of the alignment.
                let size = layout.size().next_multiple_of(layout.align());
                unsafe { aligned_alloc(layout.align(), size) }
            })
        }

        unsafe fn dealloc(&self, pointer: *mut u8, _layout: Layout) {
            unsafe { free(pointer) }
        }

        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            if layout.align() <= MALLOC_ALIGNMENT {
                return exhausted(unsafe { realloc(pointer, new_size) });
            }
            // SAFETY: GlobalAlloc::realloc's contract makes this a valid layout.
            let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
            let moved = unsafe { self.alloc(new_layout) };
            if !moved.is_null() {
                unsafe {
                    core::ptr::copy_nonoverlapping(pointer, moved, layout.size().min(new_size));
                    free(pointer);
                }
            }
            moved
        }
    }

    #[global_allocator]
    static ALLOCATOR: CAllocator = CAllocator;

    /// The runtime's Rust code is written not to panic; one that does is a
    /// bug, and the process ends as the C runtime's own assertions end it.
    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
        unsafe { abort() }
    }
}
