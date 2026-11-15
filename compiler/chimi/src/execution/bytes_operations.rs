//! The native core of `std/bytes` on the reference interpreter. `runtime.c`
//! implements the same operations in `invoke_bytes_operation`, and the
//! `std-tests` programs run on both and must agree.

use super::{Control, Evaluator, RuntimeValue};
use crate::ids::CoreNodeId;
use crate::standard::{StandardConstructor, StandardFunction};
use std::sync::Arc;

/// An `IntLayout` as the private code `std/bytes` passes: its width in bytes,
/// whether it is signed, and whether it is big-endian. Codes follow the
/// declaration order `U8, I8`, then `U16Be, U16Le, I16Be, I16Le` and the same
/// four for 32 bits, then `I64Be, I64Le`.
#[derive(Clone, Copy)]
struct Layout {
    width: usize,
    signed: bool,
    big_endian: bool,
}

/// `bytes` as lowercase hex, two digits per byte: the form of a `Bytes` in
/// program data and outcome records.
pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The bytes `hex` spells. Requires: `hex` came from [`hex`].
pub(super) fn from_hex(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).expect("program data hex is valid"))
        .collect()
}

impl Layout {
    fn from_code(code: i64) -> Option<Self> {
        let (width, signed, big_endian) = match code {
            0 => (1, false, true),
            1 => (1, true, true),
            2..=9 => {
                let within = code - 2;
                (
                    if within < 4 { 2 } else { 4 },
                    within % 4 >= 2,
                    within % 2 == 0,
                )
            }
            10 => (8, true, true),
            11 => (8, true, false),
            _ => return None,
        };
        Some(Self {
            width,
            signed,
            big_endian,
        })
    }

    /// The integer `bytes` (exactly `width` of them) encode.
    fn read(self, bytes: &[u8]) -> i64 {
        let mut raw = [0; 8];
        if self.big_endian {
            raw[8 - self.width..].copy_from_slice(bytes);
        } else {
            raw[8 - self.width..]
                .iter_mut()
                .zip(bytes.iter().rev())
                .for_each(|(to, from)| *to = *from);
        }
        let unsigned = u64::from_be_bytes(raw);
        if self.signed {
            // Sign-extend from the layout's width.
            let shift = 64 - 8 * self.width as u32;
            ((unsigned << shift) as i64) >> shift
        } else {
            unsigned as i64
        }
    }

    /// `value` in `width` bytes, or `None` when it does not fit.
    fn write(self, value: i64) -> Option<Vec<u8>> {
        let bits = 8 * self.width as u32;
        let fits = match (self.signed, bits) {
            (_, 64) => true,
            (true, _) => (-(1i64 << (bits - 1))..(1i64 << (bits - 1))).contains(&value),
            (false, _) => (0..(1i64 << bits)).contains(&value),
        };
        if !fits {
            return None;
        }
        let raw = value.to_be_bytes();
        let mut bytes = raw[8 - self.width..].to_vec();
        if !self.big_endian {
            bytes.reverse();
        }
        Some(bytes)
    }
}

impl Evaluator<'_> {
    /// Requires: `function` is one of the `std/bytes` operations of the
    /// registry, and its Chimi wrapper checked the documented preconditions.
    pub(super) fn invoke_bytes_operation(
        &self,
        function: StandardFunction,
        arguments: Vec<RuntimeValue>,
        call_node: CoreNodeId,
    ) -> Control {
        let defect = |message: String| Control::Defect {
            message,
            node: Some(call_node),
            cause: None,
        };
        let bytes = |value: Vec<u8>| RuntimeValue::Bytes(Arc::from(value));
        let integer =
            |value: usize| RuntimeValue::Integer(i64::try_from(value).unwrap_or(i64::MAX));
        let value = match (function, arguments.as_slice()) {
            (StandardFunction::BytesFrom, [RuntimeValue::List(values)]) => {
                let mut output = Vec::with_capacity(values.len());
                for (index, value) in values.iter().enumerate() {
                    let RuntimeValue::Integer(value) = value else {
                        return self.intrinsic_shape_defect(function, call_node);
                    };
                    let Ok(byte) = u8::try_from(*value) else {
                        return defect(format!(
                            "Bytes.from: element {index} is {value}, outside 0..=255"
                        ));
                    };
                    output.push(byte);
                }
                bytes(output)
            }
            (StandardFunction::BytesLength, [RuntimeValue::Bytes(value)]) => integer(value.len()),
            (
                StandardFunction::BytesGet,
                [RuntimeValue::Bytes(value), RuntimeValue::Integer(index)],
            ) => self.optional(
                usize::try_from(*index)
                    .ok()
                    .and_then(|index| value.get(index))
                    .map(|byte| RuntimeValue::Integer(i64::from(*byte))),
            ),
            (
                StandardFunction::BytesIndexOf,
                [RuntimeValue::Bytes(value), RuntimeValue::Bytes(needle)],
            ) => {
                if needle.is_empty() {
                    return defect("Bytes.index_of: needle must not be empty".to_owned());
                }
                self.optional(
                    value
                        .windows(needle.len())
                        .position(|window| window == &needle[..])
                        .map(integer),
                )
            }
            (StandardFunction::BytesConcat, [RuntimeValue::List(parts)]) => {
                let mut output = Vec::new();
                for part in parts.iter() {
                    let RuntimeValue::Bytes(part) = part else {
                        return self.intrinsic_shape_defect(function, call_node);
                    };
                    output.extend_from_slice(part);
                }
                bytes(output)
            }
            (
                StandardFunction::BytesSlice,
                [
                    RuntimeValue::Bytes(value),
                    RuntimeValue::Range { start, end },
                ],
            ) => bytes(
                value[super::core_operations::clamp_range(*start, *end, value.len())].to_vec(),
            ),
            (StandardFunction::BytesToList, [RuntimeValue::Bytes(value)]) => RuntimeValue::List(
                value
                    .iter()
                    .map(|byte| RuntimeValue::Integer(i64::from(*byte)))
                    .collect(),
            ),
            (StandardFunction::BytesFromText, [RuntimeValue::Text(text)]) => {
                bytes(text.as_bytes().to_vec())
            }
            (StandardFunction::BytesToTextLossy, [RuntimeValue::Bytes(value)]) => {
                RuntimeValue::Text(String::from_utf8_lossy(value).into_owned())
            }
            (StandardFunction::BytesValidUtf8Length, [RuntimeValue::Bytes(value)]) => {
                integer(match std::str::from_utf8(value) {
                    Ok(_) => value.len(),
                    Err(error) => error.valid_up_to(),
                })
            }
            (
                StandardFunction::BytesReadIntCode,
                [
                    RuntimeValue::Bytes(value),
                    RuntimeValue::Integer(at),
                    RuntimeValue::Integer(code),
                ],
            ) => {
                let Some(layout) = Layout::from_code(*code) else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                self.optional(
                    usize::try_from(*at)
                        .ok()
                        .and_then(|at| value.get(at..at.checked_add(layout.width)?))
                        .map(|field| RuntimeValue::Integer(layout.read(field))),
                )
            }
            (
                StandardFunction::BytesIntBytes,
                [RuntimeValue::Integer(value), RuntimeValue::Integer(code)],
            ) => {
                let Some(layout) = Layout::from_code(*code) else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                self.optional(layout.write(*value).map(bytes))
            }
            // Lowering put the file's contents, or their hex, in the argument.
            (StandardFunction::EmbedText, [RuntimeValue::Text(contents)]) => {
                RuntimeValue::Text(contents.clone())
            }
            (StandardFunction::EmbedBytes, [RuntimeValue::Text(hex)]) => {
                RuntimeValue::Bytes(from_hex(hex).into())
            }
            _ => return self.intrinsic_shape_defect(function, call_node),
        };
        Control::Success(value)
    }

    fn optional(&self, value: Option<RuntimeValue>) -> RuntimeValue {
        match value {
            Some(value) => self.standard_value(StandardConstructor::OptionSome, vec![value]),
            None => self.standard_value(StandardConstructor::OptionNone, Vec::new()),
        }
    }
}
