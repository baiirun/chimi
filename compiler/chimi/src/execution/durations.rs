//! `std/duration` on the reference interpreter. The arithmetic, text form, and parser
//! are `crate::duration`, which the type checker's constant checks share.

use super::{Control, Evaluator, RuntimeValue};
use crate::duration::{self, ParseFailure};
use crate::ids::CoreNodeId;
use crate::standard::{DurationUnitRole, StandardConstructor, StandardFunction};

impl Evaluator<'_> {
    /// Requires: `function` is in `std/duration`.
    pub(super) fn invoke_duration(
        &self,
        function: StandardFunction,
        arguments: &[RuntimeValue],
        call_node: CoreNodeId,
    ) -> Control {
        let result = match (function.duration_unit(), function, arguments) {
            (Some((unit, DurationUnitRole::Constructor)), _, [RuntimeValue::Integer(count)]) => {
                duration::construct(unit, *count).map(RuntimeValue::Duration)
            }
            (Some((unit, DurationUnitRole::Conversion)), _, [RuntimeValue::Duration(value)]) => {
                Ok(RuntimeValue::Integer(duration::whole_units(*value, unit)))
            }
            (
                None,
                StandardFunction::DurationAdd,
                [RuntimeValue::Duration(left), RuntimeValue::Duration(right)],
            ) => duration::add(*left, *right).map(RuntimeValue::Duration),
            (
                None,
                StandardFunction::DurationSubtract,
                [RuntimeValue::Duration(left), RuntimeValue::Duration(right)],
            ) => duration::subtract(*left, *right).map(RuntimeValue::Duration),
            (
                None,
                StandardFunction::DurationMultiply,
                [RuntimeValue::Duration(value), RuntimeValue::Integer(factor)],
            ) => duration::multiply(*value, *factor).map(RuntimeValue::Duration),
            (
                None,
                StandardFunction::DurationDivide,
                [
                    RuntimeValue::Duration(value),
                    RuntimeValue::Integer(divisor),
                ],
            ) => duration::divide(*value, *divisor).map(RuntimeValue::Duration),
            (None, StandardFunction::DurationNegate, [RuntimeValue::Duration(value)]) => {
                duration::negate(*value).map(RuntimeValue::Duration)
            }
            (None, StandardFunction::DurationAbsolute, [RuntimeValue::Duration(value)]) => {
                duration::absolute(*value).map(RuntimeValue::Duration)
            }
            (None, StandardFunction::DurationToText, [RuntimeValue::Duration(value)]) => {
                Ok(RuntimeValue::Text(duration::to_text(*value)))
            }
            (None, StandardFunction::DurationParse, [RuntimeValue::Text(text)]) => {
                return match duration::parse(text) {
                    Ok(value) => Control::Success(RuntimeValue::Duration(value)),
                    Err(failure) => Control::failure(self.standard_value(
                        match failure {
                            ParseFailure::Empty => StandardConstructor::DurationParseEmpty,
                            ParseFailure::Invalid => StandardConstructor::DurationParseInvalid,
                            ParseFailure::OutOfRange => {
                                StandardConstructor::DurationParseOutOfRange
                            }
                        },
                        Vec::new(),
                    )),
                };
            }
            _ => return self.intrinsic_shape_defect(function, call_node),
        };
        match result {
            Ok(value) => Control::Success(value),
            Err(message) => Control::Defect {
                message,
                node: Some(call_node),
                cause: None,
            },
        }
    }
}
