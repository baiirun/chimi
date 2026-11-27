//! The JSON decoder behind `Json.decode`.
//!
//! The decoder checks the whole input and writes a *tape*: the document's
//! values in post-order, each container after its members, so
//! `intrinsics/json.rs` builds the Chimi values bottom-up with a stack and
//! never sees bytes that failed to decode.
//!
//! Every error carries the message and byte offset that the interpreter's
//! decoder (`compiler/chimi/src/json.rs`) reports for the same input, so
//! `DecodeError.InvalidJson` is the same on both backends. Containers are
//! decoded with an explicit stack, not recursion, so depth costs heap, never
//! the calling fiber's stack.

use alloc::borrow::Cow;
use alloc::collections::BTreeSet;
use alloc::vec::Vec;

/// The deepest nesting accepted: the top-level value is at depth 0, and a
/// value at depth 129 is an error.
pub const MAX_DEPTH: usize = 128;

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeKind {
    Null = 0,
    False = 1,
    True = 2,
    /// The number as written: `offset` and `length` select it in the source.
    Number = 3,
    /// A string with no escapes: `offset` and `length` select its contents,
    /// without the quotes, in the source.
    SourceText = 4,
    /// A string that had escapes: `offset` and `length` select its decoded
    /// bytes in [`Tape::decoded`].
    DecodedText = 5,
    /// The last `length` values are its items, in order.
    Array = 6,
    /// The last `2 * length` values are its members as key, value pairs, in
    /// source order. Each key is a key node, and no two keys are equal.
    Object = 7,
    /// An object member's key, located as [`NodeKind::SourceText`] is.
    SourceKey = 8,
    /// An object member's key, located as [`NodeKind::DecodedText`] is.
    DecodedKey = 9,
}

/// One value of the tape.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Node {
    pub kind: NodeKind,
    pub offset: usize,
    pub length: usize,
}

/// A decoded document. Its nodes describe exactly one value, in post-order.
#[derive(Debug, Default)]
pub struct Tape {
    pub nodes: Vec<Node>,
    /// The contents of every [`NodeKind::DecodedText`] string, back to back.
    /// Each is valid UTF-8 when the source is.
    pub decoded: Vec<u8>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Error {
    /// The byte offset in the source that the message is about.
    pub offset: usize,
    /// Text for a person; the bytes of a duplicate key are quoted as
    /// decoded, so it is UTF-8 when the source is.
    pub message: Cow<'static, [u8]>,
}

/// Decodes `source`, which must hold exactly one JSON value with optional
/// whitespace around it.
pub fn decode(source: &[u8]) -> Result<Tape, Error> {
    Decoder {
        source,
        offset: 0,
        tape: Tape::default(),
    }
    .document()
}

/// An open container and what it has read so far.
enum Frame<'s> {
    Array {
        count: usize,
    },
    Object {
        count: usize,
        /// Its keys so far, to reject a duplicate. A key without escapes
        /// borrows the source; only an escaped key is copied.
        keys: BTreeSet<Cow<'s, [u8]>>,
    },
}

struct Decoder<'s> {
    source: &'s [u8],
    offset: usize,
    tape: Tape,
}

const UNPAIRED_SURROGATE: &str = "high surrogate must be followed by a low surrogate";

fn fail<T>(offset: usize, message: &'static str) -> Result<T, Error> {
    Err(Error {
        offset,
        message: Cow::Borrowed(message.as_bytes()),
    })
}

impl<'s> Decoder<'s> {
    fn document(mut self) -> Result<Tape, Error> {
        let mut frames: Vec<Frame<'s>> = Vec::new();
        'value: loop {
            // Read one value, at depth `frames.len()`. A container that is
            // not empty opens a frame and goes on to its first member.
            self.skip_whitespace();
            if frames.len() > MAX_DEPTH {
                return fail(self.offset, "JSON nesting exceeds the configured limit");
            }
            let Some(&first) = self.source.get(self.offset) else {
                return fail(self.offset, "expected a JSON value");
            };
            match first {
                b'[' => {
                    self.offset += 1;
                    self.skip_whitespace();
                    if self.peek() == Some(b']') {
                        self.offset += 1;
                        self.push(NodeKind::Array, 0, 0);
                    } else {
                        frames.push(Frame::Array { count: 0 });
                        continue 'value;
                    }
                }
                b'{' => {
                    self.offset += 1;
                    self.skip_whitespace();
                    if self.peek() == Some(b'}') {
                        self.offset += 1;
                        self.push(NodeKind::Object, 0, 0);
                    } else {
                        let mut keys = BTreeSet::new();
                        self.member_key(&mut keys)?;
                        frames.push(Frame::Object { count: 0, keys });
                        continue 'value;
                    }
                }
                b'"' => {
                    let text = self.string()?;
                    self.tape.nodes.push(text);
                }
                b'-' | b'0'..=b'9' => self.number()?,
                b'n' | b't' | b'f' => self.literal()?,
                _ => return fail(self.offset, "expected a JSON value"),
            }
            // A value is complete: count it in its container, then read the
            // separator, closing each container that ends here.
            loop {
                match frames.last_mut() {
                    None => {
                        self.skip_whitespace();
                        if self.offset != self.source.len() {
                            return fail(self.offset, "unexpected characters after JSON value");
                        }
                        return Ok(self.tape);
                    }
                    Some(Frame::Array { count }) => {
                        *count += 1;
                        self.skip_whitespace();
                        match self.peek() {
                            Some(b',') => {
                                self.offset += 1;
                                continue 'value;
                            }
                            Some(b']') => {
                                self.offset += 1;
                                let count = *count;
                                frames.pop();
                                self.push(NodeKind::Array, 0, count);
                            }
                            _ => return fail(self.offset, "expected `,` or `]` in JSON array"),
                        }
                    }
                    Some(Frame::Object { count, keys }) => {
                        *count += 1;
                        self.skip_whitespace();
                        match self.peek() {
                            Some(b',') => {
                                self.offset += 1;
                                self.skip_whitespace();
                                self.member_key(keys)?;
                                continue 'value;
                            }
                            Some(b'}') => {
                                self.offset += 1;
                                let count = *count;
                                frames.pop();
                                self.push(NodeKind::Object, 0, count);
                            }
                            _ => return fail(self.offset, "expected `,` or `}` in JSON object"),
                        }
                    }
                }
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.source.get(self.offset).copied()
    }

    fn push(&mut self, kind: NodeKind, offset: usize, length: usize) {
        self.tape.nodes.push(Node {
            kind,
            offset,
            length,
        });
    }

    fn skip_whitespace(&mut self) {
        while let Some(b' ' | b'\t' | b'\r' | b'\n') = self.peek() {
            self.offset += 1;
        }
    }

    /// Reads a member's key and its `:`, leaving the offset before the value.
    /// The offset is at the next byte after whitespace.
    fn member_key(&mut self, keys: &mut BTreeSet<Cow<'s, [u8]>>) -> Result<(), Error> {
        if self.peek() != Some(b'"') {
            return fail(self.offset, "expected a string key in JSON object");
        }
        let key_offset = self.offset;
        let key = self.string()?;
        let source = self.source;
        let bytes = match key.kind {
            NodeKind::SourceText => Cow::Borrowed(&source[key.offset..key.offset + key.length]),
            _ => Cow::Owned(self.tape.decoded[key.offset..key.offset + key.length].to_vec()),
        };
        if !keys.insert(bytes) {
            let bytes = match key.kind {
                NodeKind::SourceText => &source[key.offset..key.offset + key.length],
                _ => &self.tape.decoded[key.offset..key.offset + key.length],
            };
            let mut message = Vec::from(&b"duplicate JSON object key `"[..]);
            message.extend_from_slice(bytes);
            message.push(b'`');
            return Err(Error {
                offset: key_offset,
                message: Cow::Owned(message),
            });
        }
        self.tape.nodes.push(Node {
            kind: match key.kind {
                NodeKind::SourceText => NodeKind::SourceKey,
                _ => NodeKind::DecodedKey,
            },
            ..key
        });
        self.skip_whitespace();
        if self.peek() != Some(b':') {
            return fail(self.offset, "expected `:` after JSON object key");
        }
        self.offset += 1;
        Ok(())
    }

    fn literal(&mut self) -> Result<(), Error> {
        let rest = &self.source[self.offset..];
        let (kind, length) = if rest.starts_with(b"null") {
            (NodeKind::Null, 4)
        } else if rest.starts_with(b"true") {
            (NodeKind::True, 4)
        } else if rest.starts_with(b"false") {
            (NodeKind::False, 5)
        } else {
            return fail(self.offset, "invalid JSON literal");
        };
        self.offset += length;
        self.push(kind, 0, 0);
        Ok(())
    }

    fn number(&mut self) -> Result<(), Error> {
        let start = self.offset;
        if self.peek() == Some(b'-') {
            self.offset += 1;
        }
        match self.peek() {
            Some(b'0') => {
                self.offset += 1;
                if let Some(b'0'..=b'9') = self.peek() {
                    return fail(self.offset, "JSON numbers cannot contain leading zeroes");
                }
            }
            Some(b'1'..=b'9') => {
                self.skip_digits();
            }
            _ => return fail(self.offset, "expected digits in JSON number"),
        }
        if self.peek() == Some(b'.') {
            self.offset += 1;
            if self.skip_digits() == 0 {
                return fail(self.offset, "expected digits after JSON decimal point");
            }
        }
        if let Some(b'e' | b'E') = self.peek() {
            self.offset += 1;
            if let Some(b'+' | b'-') = self.peek() {
                self.offset += 1;
            }
            if self.skip_digits() == 0 {
                return fail(self.offset, "expected digits in JSON exponent");
            }
        }
        self.push(NodeKind::Number, start, self.offset - start);
        Ok(())
    }

    fn skip_digits(&mut self) -> usize {
        let start = self.offset;
        while let Some(b'0'..=b'9') = self.peek() {
            self.offset += 1;
        }
        self.offset - start
    }

    /// The end of the run of bytes from `from` that a string copies as they
    /// are: everything but `"`, `\` and control characters.
    fn plain_run_end(&self, from: usize) -> usize {
        self.source[from..]
            .iter()
            .position(|&byte| byte == b'"' || byte == b'\\' || byte < 0x20)
            .map_or(self.source.len(), |length| from + length)
    }

    /// Reads a string starting at its opening quote. A string without
    /// escapes is not copied.
    fn string(&mut self) -> Result<Node, Error> {
        self.offset += 1;
        let start = self.offset;
        self.offset = self.plain_run_end(start);
        if self.peek() == Some(b'"') {
            let node = Node {
                kind: NodeKind::SourceText,
                offset: start,
                length: self.offset - start,
            };
            self.offset += 1;
            return Ok(node);
        }
        let decoded_start = self.tape.decoded.len();
        self.tape
            .decoded
            .extend_from_slice(&self.source[start..self.offset]);
        loop {
            let Some(byte) = self.peek() else {
                return fail(self.offset, "unterminated JSON string");
            };
            if byte < 0x20 {
                return fail(self.offset, "unescaped control character in JSON string");
            }
            self.offset += 1;
            match byte {
                b'"' => {
                    return Ok(Node {
                        kind: NodeKind::DecodedText,
                        offset: decoded_start,
                        length: self.tape.decoded.len() - decoded_start,
                    });
                }
                b'\\' => self.escape()?,
                _ => {
                    // `byte` starts a plain run, which ends before the next
                    // byte that needs a look.
                    let end = self.plain_run_end(self.offset);
                    self.tape
                        .decoded
                        .extend_from_slice(&self.source[self.offset - 1..end]);
                    self.offset = end;
                }
            }
        }
    }

    /// Decodes the escape after a `\`.
    fn escape(&mut self) -> Result<(), Error> {
        let Some(escaped) = self.peek() else {
            return fail(self.offset, "unterminated JSON escape");
        };
        self.offset += 1;
        let byte = match escaped {
            b'"' | b'\\' | b'/' => escaped,
            b'b' => 0x08,
            b'f' => 0x0c,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'u' => {
                let code = self.unicode_escape()?;
                push_utf8(&mut self.tape.decoded, code);
                return Ok(());
            }
            _ => return fail(self.offset, "invalid JSON escape"),
        };
        self.tape.decoded.push(byte);
        Ok(())
    }

    /// Reads the digits of a `\u` escape, and the low half of a surrogate
    /// pair, and returns the scalar value they name.
    fn unicode_escape(&mut self) -> Result<u32, Error> {
        let code = self.code_unit()?;
        if (0xdc00..=0xdfff).contains(&code) {
            return fail(self.offset, "isolated low surrogate in JSON string");
        }
        if !(0xd800..=0xdbff).contains(&code) {
            return Ok(code);
        }
        if self.peek() != Some(b'\\') {
            return fail(self.offset, UNPAIRED_SURROGATE);
        }
        if self.source.get(self.offset + 1) != Some(&b'u') {
            return fail(self.offset + 1, UNPAIRED_SURROGATE);
        }
        self.offset += 2;
        let low = self.code_unit()?;
        if !(0xdc00..=0xdfff).contains(&low) {
            return fail(self.offset, UNPAIRED_SURROGATE);
        }
        Ok(0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00))
    }

    /// Reads four hex digits. On failure the offset is that of the first.
    fn code_unit(&mut self) -> Result<u32, Error> {
        let start = self.offset;
        let Some(digits) = self.source.get(start..start + 4) else {
            return fail(start, "incomplete JSON Unicode escape");
        };
        let mut value = 0;
        for &digit in digits {
            let Some(digit) = (digit as char).to_digit(16) else {
                return fail(start, "invalid JSON Unicode escape");
            };
            value = value * 16 + digit;
        }
        self.offset = start + 4;
        Ok(value)
    }
}

/// Appends `code`, a Unicode scalar value, as UTF-8.
fn push_utf8(output: &mut Vec<u8>, code: u32) {
    let mut buffer = [0; 4];
    let encoded = char::from_u32(code)
        .unwrap_or(char::REPLACEMENT_CHARACTER)
        .encode_utf8(&mut buffer);
    output.extend_from_slice(encoded.as_bytes());
}

#[cfg(test)]
mod tests;
