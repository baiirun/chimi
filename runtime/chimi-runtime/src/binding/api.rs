//! The binding API of `compiler/chimi/runtime/native/chimi_binding.h`, as
//! `runtime.c` exports it, and a safe view of one call over it.
//!
//! A [`Call`] lives for one binding function's run. The runtime owns every
//! value it hands out; they stay valid until the function returns, which the
//! `'call` lifetime enforces.

use core::ffi::{c_char, c_int};
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

unsafe extern "C" {
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
        let length = u32::try_from(items.len()).unwrap_or(u32::MAX);
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
