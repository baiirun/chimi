//! A fixed-capacity text for number forms whose longest spelling is known,
//! so printing one needs neither the heap nor `core::fmt`.

/// ASCII text of at most `N` bytes.
///
/// Invariant: `bytes[..length]` is ASCII, so it is always valid UTF-8.
#[derive(Clone, Copy)]
pub struct TextBuffer<const N: usize> {
    bytes: [u8; N],
    length: usize,
}

impl<const N: usize> TextBuffer<N> {
    pub(crate) const fn new() -> Self {
        Self {
            bytes: [0; N],
            length: 0,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    pub(crate) const fn len(&self) -> usize {
        self.length
    }

    pub fn as_str(&self) -> &str {
        // Every byte pushed is ASCII (see `push`).
        core::str::from_utf8(self.as_bytes()).unwrap_or_default()
    }

    /// Requires: `byte` is ASCII and the text has room; the callers' bounds
    /// are what size `N`, so a full buffer is a bug and drops the byte.
    pub(crate) fn push(&mut self, byte: u8) {
        debug_assert!(byte.is_ascii() && self.length < N);
        if let Some(slot) = self.bytes.get_mut(self.length) {
            *slot = byte;
            self.length += 1;
        }
    }

    pub(crate) fn push_str(&mut self, text: &str) {
        for byte in text.bytes() {
            self.push(byte);
        }
    }

    /// Writes `value` in decimal with at least `width` digits, zero-padded.
    pub(crate) fn push_digits(&mut self, mut value: u128, width: usize) {
        // u128::MAX has 39 digits.
        let mut digits = [0u8; 39];
        let mut count = 0;
        while count < digits.len() && (value != 0 || count == 0) {
            digits[count] = b'0' + (value % 10) as u8;
            value /= 10;
            count += 1;
        }
        for _ in count..width {
            self.push(b'0');
        }
        for digit in digits[..count].iter().rev() {
            self.push(*digit);
        }
    }

    /// Drops trailing `byte`s, keeping at least `keep` bytes.
    pub(crate) fn trim_end(&mut self, byte: u8, keep: usize) {
        while self.length > keep && self.bytes[self.length - 1] == byte {
            self.length -= 1;
        }
    }
}
