//! The binding API of `compiler/chimi/runtime/native/chimi_binding.h`, as
//! the runtime exports it (`runtime.c` for handles and waits,
//! `intrinsics/binding_values.rs` for values and results), and a safe view of
//! one call over it.
//!
//! A [`Call`] lives for one binding function's run. The runtime owns every
//! value it hands out; they stay valid until the function returns. The
//! `'call` lifetime ties values to their call, but `Call::new`'s caller
//! picks it, so keeping values within the function is that caller's duty.

use alloc::boxed::Box;
use core::ffi::{CStr, c_char, c_int, c_void};
use core::marker::PhantomData;

#[repr(C)]
pub struct ChimiBindingCall {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct ChimiBindingValue {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct ChimiBindingResult {
    _opaque: [u8; 0],
}

/// The kind of resource behind a handle, as `chimi_binding.h` lays it out.
/// The runtime tells types apart by address, so each is one `static`.
#[repr(C)]
pub struct ChimiBindingType {
    name: *const c_char,
    release: unsafe extern "C" fn(state: *mut c_void),
}

// SAFETY: the name is a `'static` C string and the release a plain
// function; neither is ever written.
unsafe impl Sync for ChimiBindingType {}

impl ChimiBindingType {
    /// The type of handles whose state is a `Box<T>`; releasing one drops
    /// the box, so `T`'s `Drop` frees the resource.
    pub const fn boxed<T>(name: &'static CStr) -> Self {
        ChimiBindingType {
            name: name.as_ptr(),
            release: release_box::<T>,
        }
    }
}

/// # Safety
///
/// `state` must be a `Box<T>` that [`Call::make_handle`] gave the runtime,
/// released once, as the runtime guarantees.
unsafe extern "C" fn release_box<T>(state: *mut c_void) {
    // SAFETY: the caller guarantees `state` is an unreleased `Box<T>`.
    drop(unsafe { Box::from_raw(state.cast::<T>()) });
}

unsafe extern "C" {
    fn chimi_binding_text(
        call: *const ChimiBindingCall,
        index: u32,
        bytes: *mut *const c_char,
        length: *mut usize,
    ) -> c_int;
    fn chimi_binding_bytes(
        call: *const ChimiBindingCall,
        index: u32,
        bytes: *mut *const u8,
        length: *mut usize,
    ) -> c_int;
    fn chimi_binding_handle(
        call: *const ChimiBindingCall,
        index: u32,
        kind: *const ChimiBindingType,
    ) -> *mut c_void;
    fn chimi_binding_void(call: *mut ChimiBindingCall) -> *mut ChimiBindingValue;
    fn chimi_binding_make_text(
        call: *mut ChimiBindingCall,
        bytes: *const c_char,
        length: usize,
    ) -> *mut ChimiBindingValue;
    fn chimi_binding_make_bytes(
        call: *mut ChimiBindingCall,
        bytes: *const u8,
        length: usize,
    ) -> *mut ChimiBindingValue;
    fn chimi_binding_make_handle(
        call: *mut ChimiBindingCall,
        kind: *const ChimiBindingType,
        state: *mut c_void,
    ) -> *mut ChimiBindingValue;
    fn chimi_binding_close_handle(
        call: *mut ChimiBindingCall,
        index: u32,
        kind: *const ChimiBindingType,
    );
    fn chimi_binding_ok(
        call: *mut ChimiBindingCall,
        value: *mut ChimiBindingValue,
    ) -> *mut ChimiBindingResult;
    fn chimi_binding_failure(
        call: *mut ChimiBindingCall,
        code: i64,
        message: *const c_char,
    ) -> *mut ChimiBindingResult;
    fn chimi_binding_cancelled(call: *mut ChimiBindingCall) -> *mut ChimiBindingResult;
    fn chimi_binding_wait(call: *mut ChimiBindingCall, handle: u32, fd: c_int, interest: c_int)
    -> c_int;
    fn chimi_binding_wait_child(call: *mut ChimiBindingCall, handle: u32, pid: c_int) -> c_int;
    fn chimi_binding_integer(call: *const ChimiBindingCall, index: u32, out: *mut i64) -> c_int;
    fn chimi_binding_make_integer(call: *mut ChimiBindingCall, value: i64) -> *mut ChimiBindingValue;
    fn chimi_binding_make_boolean(call: *mut ChimiBindingCall, value: c_int) -> *mut ChimiBindingValue;
    fn chimi_binding_none(call: *mut ChimiBindingCall) -> *mut ChimiBindingValue;
    fn chimi_binding_some(
        call: *mut ChimiBindingCall,
        value: *mut ChimiBindingValue,
    ) -> *mut ChimiBindingValue;
    fn chimi_binding_make_tuple(
        call: *mut ChimiBindingCall,
        items: *mut *mut ChimiBindingValue,
        length: u32,
    ) -> *mut ChimiBindingValue;
    fn chimi_binding_success(
        call: *mut ChimiBindingCall,
        value: *mut ChimiBindingValue,
    ) -> *mut ChimiBindingResult;
    fn chimi_binding_defect(
        call: *mut ChimiBindingCall,
        message: *const c_char,
    ) -> *mut ChimiBindingResult;
}

/// One binding call: its arguments, and the values and result built for it.
pub struct Call<'call> {
    raw: *mut ChimiBindingCall,
    _call: PhantomData<&'call mut ChimiBindingCall>,
}

/// A value built for the call it belongs to. Transparent over the C
/// pointer, so a slice of them is a `ChimiBindingValue **` array.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Value<'call> {
    raw: *mut ChimiBindingValue,
    _call: PhantomData<&'call ChimiBindingCall>,
}

/// The one result a binding function returns.
pub struct Outcome<'call> {
    raw: *mut ChimiBindingResult,
    _call: PhantomData<&'call ChimiBindingCall>,
}

/// A text argument: its UTF-8 bytes, which the runtime also ends with a NUL.
#[derive(Clone, Copy)]
pub struct Text<'call> {
    start: *const c_char,
    bytes: &'call [u8],
}

impl Text<'_> {
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes
    }

    /// The NUL-terminated text, for C functions; it ends at the first U+0000.
    pub fn as_ptr(&self) -> *const c_char {
        self.start
    }
}

/// What the descriptor is waited for.
#[derive(Clone, Copy)]
pub enum Interest {
    Readable = 1,
    Writable = 2,
}

/// How a wait ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Waited {
    /// Ready, or a hangup or error the next read or write reports.
    Ready,
    /// The task was cancelled: return [`Call::cancelled`].
    Cancelled,
    /// The descriptor cannot be waited on.
    Failed,
    /// The handle the descriptor belongs to was closed: do not touch it.
    Closed,
}

fn waited(answer: c_int) -> Waited {
    match answer {
        0 => Waited::Ready,
        1 => Waited::Cancelled,
        3 => Waited::Closed,
        _ => Waited::Failed,
    }
}

/// Installs the runtime's SIGCHLD handler, once; call it before starting a
/// child, so the child's exit wakes the scheduler.
pub fn prepare_children() {
    crate::intrinsics::signals::chimi_binding_prepare_children();
}

/// A descriptor that belongs to no handle argument.
const NO_HANDLE: u32 = u32::MAX;

impl<'call> Call<'call> {
    /// # Safety
    ///
    /// `raw` must be the call the runtime passed to the binding function
    /// running now, and the `Call` must not outlive that function.
    pub unsafe fn new(raw: *mut ChimiBindingCall) -> Self {
        Call { raw, _call: PhantomData }
    }

    /// The integer argument at `index`, or `None` when it is not one.
    pub fn integer(&self, index: u32) -> Option<i64> {
        let mut out = 0;
        // SAFETY: `raw` is the live call; `out` is a valid place to write.
        (unsafe { chimi_binding_integer(self.raw, index, &mut out) } != 0).then_some(out)
    }

    /// The text argument at `index`, or `None` when it is not one.
    pub fn text(&self, index: u32) -> Option<Text<'call>> {
        let mut start = core::ptr::null();
        let mut length = 0;
        // SAFETY: `raw` is the live call; the out pointers are valid.
        if unsafe { chimi_binding_text(self.raw, index, &mut start, &mut length) } == 0 {
            return None;
        }
        // SAFETY: the runtime gives `length` bytes at `start`, borrowed for
        // the call.
        let bytes = unsafe { slice(start.cast(), length) };
        Some(Text { start, bytes })
    }

    /// The bytes argument at `index`, or `None` when it is not one.
    pub fn bytes(&self, index: u32) -> Option<&'call [u8]> {
        let mut start = core::ptr::null();
        let mut length = 0;
        // SAFETY: `raw` is the live call; the out pointers are valid.
        if unsafe { chimi_binding_bytes(self.raw, index, &mut start, &mut length) } == 0 {
            return None;
        }
        // SAFETY: the runtime gives `length` bytes at `start`, borrowed for
        // the call.
        Some(unsafe { slice(start, length) })
    }

    /// The state of the open handle of `kind` at `index`, or `None` when it
    /// is closed or not one.
    ///
    /// The borrow ends at the next [`Call::wait`], during which another task
    /// may close the handle, so read it again after every wait.
    ///
    /// # Safety
    ///
    /// `kind` must be a type made with `ChimiBindingType::boxed::<T>`.
    pub unsafe fn handle<T>(&self, index: u32, kind: &'static ChimiBindingType) -> Option<&T> {
        // SAFETY: `raw` is the live call; an open handle of `kind` holds a
        // `Box<T>`, which stays alive until it is closed, which takes a wait
        // or `close_handle`, both needing `&mut self`.
        unsafe { chimi_binding_handle(self.raw, index, kind).cast::<T>().as_ref() }
    }

    /// A new handle of `kind` that owns `state`, or `None` when the handle
    /// table is full, after the runtime released `state`.
    ///
    /// # Safety
    ///
    /// `kind` must be a type made with `ChimiBindingType::boxed::<T>`.
    pub unsafe fn make_handle<T>(
        &mut self,
        kind: &'static ChimiBindingType,
        state: Box<T>,
    ) -> Option<Value<'call>> {
        let state = Box::into_raw(state).cast::<c_void>();
        // SAFETY: `raw` is the live call; the runtime owns `state` from now
        // on and frees it with `kind`'s release, which drops a `Box<T>`.
        let raw = unsafe { chimi_binding_make_handle(self.raw, kind, state) };
        (!raw.is_null()).then(|| self.value(raw))
    }

    /// Releases the handle argument at `index` if it is an open handle of
    /// `kind`.
    pub fn close_handle(&mut self, index: u32, kind: &'static ChimiBindingType) {
        // SAFETY: `raw` is the live call and `kind` a static type.
        unsafe { chimi_binding_close_handle(self.raw, index, kind) }
    }

    /// Waits until `fd`, which belongs to the handle argument at `handle`
    /// (or to no handle), is ready for `interest`, letting other tasks run.
    pub fn wait(&mut self, handle: Option<u32>, fd: c_int, interest: Interest) -> Waited {
        // SAFETY: `raw` is the live call.
        waited(unsafe {
            chimi_binding_wait(self.raw, handle.unwrap_or(NO_HANDLE), fd, interest as c_int)
        })
    }

    /// Waits until the child `pid`, which belongs to the handle argument at
    /// `handle` (or to no handle), has exited, letting other tasks run. It
    /// does not reap the child, and `Ready` can come early.
    pub fn wait_child(&mut self, handle: Option<u32>, pid: c_int) -> Waited {
        // SAFETY: `raw` is the live call.
        waited(unsafe { chimi_binding_wait_child(self.raw, handle.unwrap_or(NO_HANDLE), pid) })
    }

    pub fn void(&mut self) -> Value<'call> {
        // SAFETY: `raw` is the live call.
        self.value(unsafe { chimi_binding_void(self.raw) })
    }

    /// A text value; `bytes` must be UTF-8, or the result is a defect.
    pub fn make_text(&mut self, bytes: &[u8]) -> Value<'call> {
        // SAFETY: `raw` is the live call; the runtime copies the bytes.
        self.value(unsafe { chimi_binding_make_text(self.raw, bytes.as_ptr().cast(), bytes.len()) })
    }

    pub fn make_bytes(&mut self, bytes: &[u8]) -> Value<'call> {
        // SAFETY: `raw` is the live call; the runtime copies the bytes.
        self.value(unsafe { chimi_binding_make_bytes(self.raw, bytes.as_ptr(), bytes.len()) })
    }

    pub fn make_integer(&mut self, value: i64) -> Value<'call> {
        // SAFETY: `raw` is the live call.
        self.value(unsafe { chimi_binding_make_integer(self.raw, value) })
    }

    pub fn make_boolean(&mut self, value: bool) -> Value<'call> {
        // SAFETY: `raw` is the live call.
        self.value(unsafe { chimi_binding_make_boolean(self.raw, c_int::from(value)) })
    }

    pub fn none(&mut self) -> Value<'call> {
        // SAFETY: `raw` is the live call.
        self.value(unsafe { chimi_binding_none(self.raw) })
    }

    pub fn some(&mut self, value: Value<'call>) -> Value<'call> {
        // SAFETY: `raw` is the live call and `value` was built for it.
        self.value(unsafe { chimi_binding_some(self.raw, value.raw) })
    }

    pub fn make_tuple(&mut self, items: &[Value<'call>]) -> Value<'call> {
        let Ok(length) = u32::try_from(items.len()) else {
            // The runtime refuses a null value with a defect.
            return self.value(core::ptr::null_mut());
        };
        // SAFETY: `raw` is the live call, `Value` is transparent over the
        // pointer, the items were built for this call, and the runtime only
        // reads the array, copying each item into the tuple.
        self.value(unsafe {
            chimi_binding_make_tuple(self.raw, items.as_ptr().cast_mut().cast(), length)
        })
    }

    pub fn success(self, value: Value<'call>) -> Outcome<'call> {
        // SAFETY: `raw` is the live call and `value` was built for it.
        Outcome::new(unsafe { chimi_binding_success(self.raw, value.raw) })
    }

    /// `Ok(value)`, for a `native fn` returning `Result<T, (Int, Text)>`.
    pub fn ok(self, value: Value<'call>) -> Outcome<'call> {
        // SAFETY: `raw` is the live call and `value` was built for it.
        Outcome::new(unsafe { chimi_binding_ok(self.raw, value.raw) })
    }

    /// `Err((code, message))`.
    pub fn failure(self, code: i64, message: &CStr) -> Outcome<'call> {
        // SAFETY: `message` is NUL-terminated and outlives the call.
        unsafe { self.failure_raw(code, message.as_ptr()) }
    }

    /// `Err((code, message))` with a message from the C library.
    ///
    /// # Safety
    ///
    /// `message` must be a NUL-terminated string, valid for this call.
    pub unsafe fn failure_raw(self, code: i64, message: *const c_char) -> Outcome<'call> {
        // SAFETY: `raw` is the live call; the caller vouches for `message`,
        // which the runtime copies.
        Outcome::new(unsafe { chimi_binding_failure(self.raw, code, message) })
    }

    /// The task was cancelled while the call waited.
    pub fn cancelled(self) -> Outcome<'call> {
        // SAFETY: `raw` is the live call.
        Outcome::new(unsafe { chimi_binding_cancelled(self.raw) })
    }

    /// A defect: a bug in the program, such as an argument of the wrong kind.
    pub fn defect(self, message: &core::ffi::CStr) -> Outcome<'call> {
        // SAFETY: `raw` is the live call and `message` is NUL-terminated.
        Outcome::new(unsafe { chimi_binding_defect(self.raw, message.as_ptr()) })
    }

    fn value(&self, raw: *mut ChimiBindingValue) -> Value<'call> {
        Value { raw, _call: PhantomData }
    }
}

impl Outcome<'_> {
    fn new(raw: *mut ChimiBindingResult) -> Self {
        Outcome { raw, _call: PhantomData }
    }

    pub fn into_raw(self) -> *mut ChimiBindingResult {
        self.raw
    }
}

/// # Safety
///
/// `start` must point to `length` readable bytes that outlive `'a`; it may be
/// null when `length` is 0.
unsafe fn slice<'a>(start: *const u8, length: usize) -> &'a [u8] {
    if length == 0 || start.is_null() {
        &[]
    } else {
        // SAFETY: the caller guarantees `length` readable bytes.
        unsafe { core::slice::from_raw_parts(start, length) }
    }
}
