//! The native core of `std/decimal` on the reference interpreter. `runtime.c`
//! implements the same operations in `invoke_decimal_operation`, and the
//! `std-tests` programs run on both and must agree.

use super::{Control, Environment, Evaluator, RuntimeValue};
use crate::decimal::{Decimal, Rounding};
use crate::ids::CoreNodeId;
use crate::standard::{StandardConstructor, StandardFunction};

impl Evaluator<'_> {
    /// Requires: `function` is one of the `std/decimal` operations of the
    /// registry, and its Chimi wrapper checked the documented preconditions.
    pub(super) fn invoke_decimal_operation(
        &mut self,
        function: StandardFunction,
        arguments: Vec<RuntimeValue>,
        _span: u64,
        call_node: CoreNodeId,
        _environment: Environment,
    ) -> Control {
        let value = match (function, arguments.as_slice()) {
            (
                StandardFunction::DecimalScaled,
                [
                    RuntimeValue::Integer(coefficient),
                    RuntimeValue::Integer(scale),
                ],
            ) => match u32::try_from(*scale) {
                Ok(scale) if scale <= crate::decimal::MAX_SCALE => {
                    RuntimeValue::Decimal(Decimal::of(*coefficient, scale))
                }
                _ => return self.intrinsic_shape_defect(function, call_node),
            },
            (StandardFunction::DecimalRead, [RuntimeValue::Text(text)]) => {
                self.optional_decimal(Decimal::parse(text))
            }
            (StandardFunction::DecimalToText, [RuntimeValue::Decimal(value)]) => {
                RuntimeValue::Text(value.to_text())
            }
            (StandardFunction::DecimalToFloat, [RuntimeValue::Decimal(value)]) => {
                RuntimeValue::Float(value.to_float())
            }
            (
                StandardFunction::DecimalAddChecked
                | StandardFunction::DecimalSubtractChecked
                | StandardFunction::DecimalMultiplyChecked,
                [RuntimeValue::Decimal(left), RuntimeValue::Decimal(right)],
            ) => self.optional_decimal(match function {
                StandardFunction::DecimalAddChecked => left.add(*right),
                StandardFunction::DecimalSubtractChecked => left.subtract(*right),
                _ => left.multiply(*right),
            }),
            (
                StandardFunction::DecimalDivideRounded,
                [
                    RuntimeValue::Decimal(dividend),
                    RuntimeValue::Decimal(divisor),
                    RuntimeValue::Integer(scale),
                    RuntimeValue::Integer(rounding),
                ],
            ) => {
                let (Ok(scale), Some(rounding)) =
                    (u32::try_from(*scale), Rounding::from_code(*rounding))
                else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                if scale > crate::decimal::MAX_SCALE {
                    return self.intrinsic_shape_defect(function, call_node);
                }
                self.optional_decimal(dividend.divide(*divisor, scale, rounding))
            }
            _ => return self.intrinsic_shape_defect(function, call_node),
        };
        Control::Success(value)
    }

    fn optional_decimal(&self, value: Option<Decimal>) -> RuntimeValue {
        match value {
            Some(value) => self.standard_value(
                StandardConstructor::OptionSome,
                vec![RuntimeValue::Decimal(value)],
            ),
            None => self.standard_value(StandardConstructor::OptionNone, Vec::new()),
        }
    }
}
