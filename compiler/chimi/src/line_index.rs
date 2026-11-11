//! Conversion between compiler byte offsets and editor line positions.
//!
//! Compiler ranges are zero-based UTF-8 byte offsets. Editors address text by
//! zero-based line and a character column counted in a negotiated encoding.
//! A line ends before `\n`, `\r\n`, or a lone `\r`, matching LSP 3.17.

use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PositionEncoding {
    /// Columns count UTF-8 bytes.
    Utf8,
    /// Columns count UTF-16 code units; the LSP default.
    Utf16,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct LinePosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PositionError {
    LineOutOfRange { line: u32, line_count: u32 },
    SplitsCodePoint(LinePosition),
}

/// Line table over one immutable text.
#[derive(Clone, Debug)]
pub struct LineIndex {
    text: Arc<str>,
    /// Byte offset of each line start. Never empty; the first entry is 0.
    line_starts: Vec<u32>,
}

impl LineIndex {
    pub fn new(text: Arc<str>) -> Self {
        let bytes = text.as_bytes();
        let mut line_starts = vec![0];
        let mut index = 0;
        while index < bytes.len() {
            let next = match bytes[index] {
                b'\r' if bytes.get(index + 1) == Some(&b'\n') => index + 2,
                b'\r' | b'\n' => index + 1,
                _ => {
                    index += 1;
                    continue;
                }
            };
            line_starts.push(offset_u32(next));
            index = next;
        }
        Self { text, line_starts }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Converts a byte offset to a line position.
    ///
    /// Requires: `offset` is at most the text length and on a code-point
    /// boundary; compiler ranges always are. Panics otherwise.
    pub fn position(&self, offset: u32, encoding: PositionEncoding) -> LinePosition {
        assert!(
            self.text.is_char_boundary(offset as usize),
            "offset {offset} is not a code-point boundary within {} bytes",
            self.text.len()
        );
        let line = self.line_starts.partition_point(|start| *start <= offset) - 1;
        let start = self.line_starts[line] as usize;
        let prefix = &self.text[start..offset as usize];
        LinePosition {
            line: offset_u32(line),
            character: offset_u32(match encoding {
                PositionEncoding::Utf8 => prefix.len(),
                PositionEncoding::Utf16 => prefix.encode_utf16().count(),
            }),
        }
    }

    /// Converts a line position to a byte offset.
    ///
    /// Ensures: a character past the end of its line resolves to the line end,
    /// before its terminator, as LSP requires.
    /// Ensures: `offset(position(o, e), e) == o` for every valid offset `o`
    /// outside a line terminator.
    pub fn offset(
        &self,
        position: LinePosition,
        encoding: PositionEncoding,
    ) -> Result<u32, PositionError> {
        let line = position.line as usize;
        let Some(&start) = self.line_starts.get(line) else {
            return Err(PositionError::LineOutOfRange {
                line: position.line,
                line_count: offset_u32(self.line_starts.len()),
            });
        };
        let start = start as usize;
        let content = &self.text[start..self.line_end(line)];
        let wanted = position.character as usize;
        let mut consumed = 0;
        for (byte, character) in content.char_indices() {
            if consumed == wanted {
                return Ok(offset_u32(start + byte));
            }
            consumed += match encoding {
                PositionEncoding::Utf8 => character.len_utf8(),
                PositionEncoding::Utf16 => character.len_utf16(),
            };
            if consumed > wanted {
                return Err(PositionError::SplitsCodePoint(position));
            }
        }
        Ok(offset_u32(start + content.len()))
    }

    /// The byte offset where `line`'s content ends, before its terminator.
    fn line_end(&self, line: usize) -> usize {
        let Some(&next) = self.line_starts.get(line + 1) else {
            return self.text.len();
        };
        let next = next as usize;
        if self.text[..next].ends_with("\r\n") {
            next - 2
        } else {
            next - 1
        }
    }
}

fn offset_u32(value: usize) -> u32 {
    u32::try_from(value).expect("source offsets fit in u32")
}

#[cfg(test)]
mod tests {
    use super::{LineIndex, LinePosition, PositionEncoding, PositionError};

    const fn at(line: u32, character: u32) -> LinePosition {
        LinePosition { line, character }
    }

    /// Guards the encoding split: an astral code point is 4 UTF-8 bytes but
    /// 2 UTF-16 units, and every terminator form starts a new line.
    #[test]
    fn converts_offsets_across_encodings_and_terminators() {
        // "a😀b\r\nc\rd\n": a=0, 😀=1..5, b=5, \r\n=6..8, c=8, \r=9, d=10, \n=11
        let index = LineIndex::new("a😀b\r\nc\rd\n".into());
        let cases = [
            (5, PositionEncoding::Utf8, at(0, 5)),
            (5, PositionEncoding::Utf16, at(0, 3)),
            (8, PositionEncoding::Utf16, at(1, 0)),
            (10, PositionEncoding::Utf16, at(2, 0)),
            (12, PositionEncoding::Utf16, at(3, 0)),
        ];
        for (offset, encoding, position) in cases {
            assert_eq!(
                index.position(offset, encoding),
                position,
                "{offset} {encoding:?}"
            );
            assert_eq!(
                index.offset(position, encoding),
                Ok(offset),
                "{position:?} {encoding:?}"
            );
        }
    }

    /// Guards the LSP clamping rule and rejection of positions inside a code point.
    #[test]
    fn clamps_past_line_end_and_rejects_split_code_points() {
        let index = LineIndex::new("a😀b\r\nc".into());
        assert_eq!(index.offset(at(0, 99), PositionEncoding::Utf16), Ok(6));
        assert_eq!(
            index.offset(at(0, 2), PositionEncoding::Utf16),
            Err(PositionError::SplitsCodePoint(at(0, 2)))
        );
        assert_eq!(
            index.offset(at(0, 3), PositionEncoding::Utf8),
            Err(PositionError::SplitsCodePoint(at(0, 3)))
        );
        assert_eq!(
            index.offset(at(5, 0), PositionEncoding::Utf16),
            Err(PositionError::LineOutOfRange {
                line: 5,
                line_count: 2
            })
        );
    }
}
