//! The native core of `std/option`, `std/text`, `std/list`, and `Debug.crash` on
//! the reference interpreter. `runtime.c` implements the same operations for the
//! native backend, and the `std-tests` programs run on both and must agree.

use super::{Control, Evaluator, RuntimeValue, debug};
use crate::ids::CoreNodeId;
use crate::standard::{StandardConstructor, StandardFunction};

/// Unicode `White_Space`, the characters `Text.trim_start` and `Text.trim_end`
/// remove. `runtime.c` keeps the same table.
pub(crate) fn is_white_space(character: char) -> bool {
    matches!(
        character,
        '\u{9}'..='\u{d}'
            | ' '
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

/// Clamps `start..end` to `0..length`; a reversed range clamps to empty.
pub(super) fn clamp_range(start: i64, end: i64, length: usize) -> std::ops::Range<usize> {
    let clamp = |bound: i64| {
        usize::try_from(bound.max(0))
            .unwrap_or(usize::MAX)
            .min(length)
    };
    let start = clamp(start);
    start..clamp(end).max(start)
}

/// The defect message of a failed `Test.assert_equal` or `Test.assert_not_equal`.
/// `chimi test` reads the two renderings back from it, so it must match
/// `assertion_message` in `runtime.c` byte for byte.
pub(crate) fn comparison_message(heading: &str, actual: &str, expected: &str) -> String {
    format!("{heading}\nactual:\n{actual}\nexpected:\n{expected}")
}

pub(crate) const ASSERT_EQUAL_HEADING: &str = "Test.assert_equal: the values differ";
pub(crate) const ASSERT_NOT_EQUAL_HEADING: &str = "Test.assert_not_equal: the values are equal";

pub(crate) const CAPTURE_RETURNED_HEADING: &str =
    "Test.capture_defect: the operation returned instead of defecting:";
pub(crate) const CAPTURE_FAILED_HEADING: &str =
    "Test.capture_defect: the operation failed with an expected failure instead of defecting:";

impl Evaluator<'_> {
    /// `std/test.capture_defect`: runs `operation` and returns its defect's
    /// message. A defect raised at an assertion call (`CoreProgram::
    /// assertion_calls`) passes through, and an outcome that is not a defect
    /// fails the test at `call_node`. The interpreter has no child tasks, so
    /// there is nothing to cancel; `runtime.c` cancels the scope's children.
    pub(super) fn invoke_capture_defect(
        &mut self,
        arguments: Vec<RuntimeValue>,
        span: u64,
        call_node: CoreNodeId,
        environment: super::Environment,
    ) -> Control {
        let [RuntimeValue::Callable(operation)] = arguments.as_slice() else {
            return self.intrinsic_shape_defect(StandardFunction::TestCaptureDefect, call_node);
        };
        let control = self.invoke(operation.clone(), Vec::new(), span, call_node, environment);
        let not_a_defect = |heading: &str, value: &RuntimeValue| Control::Defect {
            message: format!("{heading}\n{}", debug::render(self.core, value, true)),
            node: Some(call_node),
            cause: None,
        };
        match control {
            Control::Defect { message, node, .. }
                if !node.is_some_and(|node| self.is_assertion_call(node)) =>
            {
                // Native defect messages are C strings, so the text ends at the
                // first NUL there; cutting it here keeps both backends' values equal.
                let message = message.split('\0').next().unwrap_or_default().to_owned();
                Control::Success(RuntimeValue::Text(message))
            }
            Control::Success(value) => not_a_defect(CAPTURE_RETURNED_HEADING, &value),
            Control::Failure(value, _) => not_a_defect(CAPTURE_FAILED_HEADING, &value),
            other => other,
        }
    }

    fn is_assertion_call(&self, node: CoreNodeId) -> bool {
        self.assertion_calls
            .get_or_init(|| self.core.assertion_calls())
            .contains(&node.raw())
    }

    /// `std/test`: each assertion defects at `call_node`, the caller's call.
    pub(super) fn invoke_assertion(
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
        match (function, arguments.as_slice()) {
            (
                StandardFunction::TestAssertEqual | StandardFunction::TestAssertNotEqual,
                [actual, expected],
            ) => {
                let equal = self.structural_order(actual, expected).is_eq();
                let wanted = function == StandardFunction::TestAssertEqual;
                if equal == wanted {
                    return Control::Success(RuntimeValue::Void);
                }
                let heading = if wanted {
                    ASSERT_EQUAL_HEADING
                } else {
                    ASSERT_NOT_EQUAL_HEADING
                };
                defect(comparison_message(
                    heading,
                    &debug::render(self.core, actual, true),
                    &debug::render(self.core, expected, true),
                ))
            }
            (StandardFunction::TestAssert, [RuntimeValue::Boolean(true), _]) => {
                Control::Success(RuntimeValue::Void)
            }
            (
                StandardFunction::TestAssert,
                [RuntimeValue::Boolean(false), RuntimeValue::Text(message)],
            )
            | (StandardFunction::TestFail, [RuntimeValue::Text(message)]) => {
                defect(message.clone())
            }
            _ => self.intrinsic_shape_defect(function, call_node),
        }
    }

    /// Requires: `function` is one of the operations this module implements.
    pub(super) fn invoke_core_operation(
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
        let text = |value: String| Control::Success(RuntimeValue::Text(value));
        let boolean = |value: bool| Control::Success(RuntimeValue::Boolean(value));
        match (function, arguments.as_slice()) {
            (StandardFunction::DebugCrash, [RuntimeValue::Text(message)]) => {
                defect(message.clone())
            }
            (StandardFunction::OptionUnwrapOrFail, [option, error]) => match option {
                RuntimeValue::Constructed {
                    constructor,
                    fields,
                    ..
                } if *constructor
                    == self.standard_constructor_symbol(StandardConstructor::OptionSome) =>
                {
                    Control::Success(fields[0].value.clone())
                }
                _ => Control::failure(error.clone()),
            },
            (StandardFunction::TextConcat, [RuntimeValue::List(parts)]) => {
                let mut output = String::new();
                for part in parts.iter() {
                    let RuntimeValue::Text(part) = part else {
                        return self.intrinsic_shape_defect(function, call_node);
                    };
                    output.push_str(part);
                }
                text(output)
            }
            (StandardFunction::TextByteLength, [RuntimeValue::Text(value)]) => Control::Success(
                RuntimeValue::Integer(i64::try_from(value.len()).unwrap_or(i64::MAX)),
            ),
            (
                StandardFunction::TextByteSlice,
                [
                    RuntimeValue::Text(value),
                    RuntimeValue::Range { start, end },
                ],
            ) => {
                let range = clamp_range(*start, *end, value.len());
                for bound in [range.start, range.end] {
                    if !value.is_char_boundary(bound) {
                        return defect(format!(
                            "Text.byte_slice: bound {bound} falls inside a UTF-8 sequence"
                        ));
                    }
                }
                text(value[range].to_owned())
            }
            (
                StandardFunction::TextContains,
                [RuntimeValue::Text(value), RuntimeValue::Text(needle)],
            ) => {
                if needle.is_empty() {
                    return defect("Text.contains: needle must not be empty".to_owned());
                }
                boolean(value.contains(needle.as_str()))
            }
            (
                StandardFunction::TextStartsWith,
                [RuntimeValue::Text(value), RuntimeValue::Text(prefix)],
            ) => boolean(value.starts_with(prefix.as_str())),
            (
                StandardFunction::TextEndsWith,
                [RuntimeValue::Text(value), RuntimeValue::Text(suffix)],
            ) => boolean(value.ends_with(suffix.as_str())),
            (
                StandardFunction::TextSplit,
                [RuntimeValue::Text(value), RuntimeValue::Text(separator)],
            ) => {
                if separator.is_empty() {
                    return defect("Text.split: separator must not be empty".to_owned());
                }
                Control::Success(RuntimeValue::List(
                    value
                        .split(separator.as_str())
                        .map(|part| RuntimeValue::Text(part.to_owned()))
                        .collect(),
                ))
            }
            (
                StandardFunction::TextReplace,
                [
                    RuntimeValue::Text(value),
                    RuntimeValue::Text(old),
                    RuntimeValue::Text(new),
                ],
            ) => {
                if old.is_empty() {
                    return defect("Text.replace: pattern must not be empty".to_owned());
                }
                text(value.replace(old.as_str(), new))
            }
            (StandardFunction::TextTrimStart, [RuntimeValue::Text(value)]) => {
                text(value.trim_start_matches(is_white_space).to_owned())
            }
            (StandardFunction::TextTrimEnd, [RuntimeValue::Text(value)]) => {
                text(value.trim_end_matches(is_white_space).to_owned())
            }
            // Rust's mappings are the Unicode default case conversion, Final_Sigma
            // included; `runtime.c` holds tables generated from them
            // (`tests/unicode_tables.rs`).
            (StandardFunction::TextLowercase, [RuntimeValue::Text(value)]) => {
                text(value.to_lowercase())
            }
            (StandardFunction::TextUppercase, [RuntimeValue::Text(value)]) => {
                text(value.to_uppercase())
            }
            (StandardFunction::TextGraphemes, [RuntimeValue::Text(value)]) => {
                Control::Success(RuntimeValue::List(
                    crate::graphemes::graphemes(value)
                        .map(|grapheme| RuntimeValue::Text(grapheme.to_owned()))
                        .collect(),
                ))
            }
            (
                StandardFunction::ListGet,
                [RuntimeValue::List(list), RuntimeValue::Integer(index)],
            ) => {
                let found = usize::try_from(*index)
                    .ok()
                    .and_then(|index| list.get(index));
                Control::Success(match found {
                    Some(value) => {
                        self.standard_value(StandardConstructor::OptionSome, vec![value.clone()])
                    }
                    None => self.standard_value(StandardConstructor::OptionNone, Vec::new()),
                })
            }
            (StandardFunction::ListPush, [RuntimeValue::List(list), item]) => {
                Control::Success(RuntimeValue::List(list.push(item.clone())))
            }
            (
                StandardFunction::ListSlice,
                [RuntimeValue::List(list), RuntimeValue::Range { start, end }],
            ) => {
                let range = clamp_range(*start, *end, list.len());
                Control::Success(RuntimeValue::List(
                    list.slice(range)
                        .expect("a clamped range is inside the list"),
                ))
            }
            (
                StandardFunction::ListSet,
                [RuntimeValue::List(list), RuntimeValue::Integer(index), item],
            ) => match usize::try_from(*index) {
                Ok(position) if position < list.len() => {
                    Control::Success(RuntimeValue::List(list.set(position, item.clone())))
                }
                _ => defect(format!(
                    "List.set: index must be in 0..{}, got {index}",
                    list.len()
                )),
            },
            _ => self.intrinsic_shape_defect(function, call_node),
        }
    }
}
