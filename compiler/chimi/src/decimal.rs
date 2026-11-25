//! The `std/decimal` number model lives in `chimi-runtime`, which the native
//! runtime links too, so both backends compute with one implementation. The
//! defect messages are the compiler's own; `runtime.c` keeps matching ones.

pub(crate) use chimi_runtime::decimal::{Decimal, MAX_SCALE, Rounding};

use crate::numeric::ArithmeticOperator;

/// Applies a `Decimal` operator.
///
/// Ensures: `Err` holds the defect message, which names the operator and both
/// operands in `Decimal.to_text` form. Typing rejects `/` and `%` (`T4306`).
pub(crate) fn decimal_binary(
    operator: ArithmeticOperator,
    left: Decimal,
    right: Decimal,
) -> Result<Decimal, String> {
    let symbol = operator.symbol();
    let result = match operator {
        ArithmeticOperator::Add => left.add(right),
        ArithmeticOperator::Subtract => left.subtract(right),
        ArithmeticOperator::Multiply => left.multiply(right),
        ArithmeticOperator::Divide | ArithmeticOperator::Remainder => {
            return Err(format!("`{symbol}` is not a `Decimal` operator"));
        }
    };
    result.ok_or_else(|| {
        format!(
            "decimal overflow in `{symbol}`: {} {symbol} {} needs more than 38 digits or scale 38",
            left.to_text(),
            right.to_text()
        )
    })
}
