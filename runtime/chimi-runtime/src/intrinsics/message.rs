//! Defect messages with numbers in them, as `runtime.c`'s `numeric_message`
//! writes them: in a 256-byte block outside every heap, because a control's
//! message is not traced and a defect ends its unit of work, and cut at 255
//! bytes. The numbers are written by hand, as `%` conversions would write
//! them, so no formatting machinery joins every program.

use core::ffi::c_char;

unsafe extern "C" {
    /// Prints "Chimi runtime exhausted memory" and exits 70.
    fn chimi_runtime_exhausted_memory() -> !;
}

const CAPACITY: usize = 256;

pub struct Message {
    data: *mut u8,
    length: usize,
}

impl Message {
    pub fn new() -> Self {
        // SAFETY: a plain allocation; failure exits.
        let data = unsafe { libc::malloc(CAPACITY) }.cast::<u8>();
        if data.is_null() {
            // SAFETY: exits.
            unsafe { chimi_runtime_exhausted_memory() }
        }
        Self { data, length: 0 }
    }

    /// Appends `bytes`, as far as the block holds them.
    pub fn text(mut self, bytes: &[u8]) -> Self {
        let room = CAPACITY - 1 - self.length;
        let taken = bytes.len().min(room);
        // SAFETY: the block holds CAPACITY bytes and `taken` fits after `length`.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), self.data.add(self.length), taken) };
        self.length += taken;
        self
    }

    /// Appends `value` in decimal, as `%PRId64` writes it.
    pub fn integer(self, value: i64) -> Self {
        let mut spelling = [0; 20];
        let digits = integer_spelling(value, &mut spelling);
        self.text(digits)
    }

    /// Appends `value` in decimal, as `%PRIu64` writes it.
    pub fn unsigned(self, mut value: u64) -> Self {
        let mut spelling = [0; 20];
        let mut start = spelling.len();
        loop {
            start -= 1;
            spelling[start] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        self.text(&spelling[start..])
    }

    /// The NUL-terminated message, which lives until the program ends.
    pub fn finish(self) -> *const c_char {
        // SAFETY: `length` stays below CAPACITY.
        unsafe { *self.data.add(self.length) = 0 };
        self.data.cast()
    }
}

/// The decimal digits of `value`, with a `-` when negative.
pub fn integer_spelling(value: i64, output: &mut [u8; 20]) -> &[u8] {
    let mut magnitude = value.unsigned_abs();
    let mut start = output.len();
    loop {
        start -= 1;
        output[start] = b'0' + (magnitude % 10) as u8;
        magnitude /= 10;
        if magnitude == 0 {
            break;
        }
    }
    if value < 0 {
        start -= 1;
        output[start] = b'-';
    }
    &output[start..]
}
