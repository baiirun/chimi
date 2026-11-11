//! Structural order over interpreter values: the one comparison behind `==`,
//! `<`, `Order.compare`, and `Map` keys.
//!
//! For ordered types it is the source-level order. For types that are only
//! equatable (records, maps, `Json.Value`) it is an internal total order that is
//! consistent with equality and never observable through enumeration.
//!
//! The walk keeps its own work stack, so nesting depth costs heap, never host
//! stack. `runtime.c` mirrors it in `chimi_value_compare`.

use super::{RuntimeField, RuntimeValue};
use crate::ids::SymbolId;
use std::cmp::Ordering;

enum Step<'value> {
    Compare(&'value RuntimeValue, &'value RuntimeValue),
    /// Decides the comparison when every step pushed above it was equal, such as
    /// the length rule after the shared prefix of two lists.
    Decide(Ordering),
}

/// Requires: both values have the same type, and that type is equatable.
/// `ordinal` gives a constructor's position in its type's declaration order.
pub(super) fn structural_order(
    left: &RuntimeValue,
    right: &RuntimeValue,
    ordinal: &dyn Fn(SymbolId) -> u32,
) -> Ordering {
    let mut steps = vec![Step::Compare(left, right)];
    while let Some(step) = steps.pop() {
        let (left, right) = match step {
            Step::Decide(Ordering::Equal) => continue,
            Step::Decide(decided) => return decided,
            Step::Compare(left, right) => (left, right),
        };
        let order = match (left, right) {
            (RuntimeValue::Void, RuntimeValue::Void) => Ordering::Equal,
            (RuntimeValue::Integer(left), RuntimeValue::Integer(right)) => left.cmp(right),
            // Finite, so total; `-0.0` and `0.0` are equal.
            (RuntimeValue::Float(left), RuntimeValue::Float(right)) => left
                .partial_cmp(right)
                .expect("Float values are always finite"),
            (RuntimeValue::Boolean(left), RuntimeValue::Boolean(right)) => left.cmp(right),
            (RuntimeValue::Text(left), RuntimeValue::Text(right)) => {
                left.as_bytes().cmp(right.as_bytes())
            }
            (RuntimeValue::JsonNumber(left), RuntimeValue::JsonNumber(right)) => {
                crate::json::compare_numbers(left, right)
            }
            (RuntimeValue::List(left), RuntimeValue::List(right)) => {
                steps.push(Step::Decide(left.len().cmp(&right.len())));
                let pairs = left.iter().zip(right.iter()).collect::<Vec<_>>();
                steps.extend(
                    pairs
                        .into_iter()
                        .rev()
                        .map(|(left, right)| Step::Compare(left, right)),
                );
                Ordering::Equal
            }
            // Typing gives both tuples one arity; elements compare left to right.
            (RuntimeValue::Tuple(left), RuntimeValue::Tuple(right)) => {
                steps.extend(
                    left.iter()
                        .zip(right)
                        .rev()
                        .map(|(left, right)| Step::Compare(left, right)),
                );
                Ordering::Equal
            }
            // Normalized bounds, so `0..3 == 0..=2`; empty ranges keep their bounds.
            (
                RuntimeValue::Range {
                    start: left_start,
                    end: left_end,
                },
                RuntimeValue::Range {
                    start: right_start,
                    end: right_end,
                },
            ) => (left_start, left_end).cmp(&(right_start, right_end)),
            (RuntimeValue::Record(left), RuntimeValue::Record(right)) => {
                // Records have no declaration order; any fixed order serves.
                let (left, right) = (by_name(left), by_name(right));
                steps.extend(
                    left.into_iter()
                        .zip(right)
                        .rev()
                        .map(|(left, right)| Step::Compare(left, right)),
                );
                Ordering::Equal
            }
            (
                RuntimeValue::Constructed {
                    constructor: left_constructor,
                    fields: left_fields,
                    ..
                },
                RuntimeValue::Constructed {
                    constructor: right_constructor,
                    fields: right_fields,
                    ..
                },
            ) => {
                let variant = ordinal(*left_constructor).cmp(&ordinal(*right_constructor));
                if variant == Ordering::Equal {
                    steps.extend(
                        left_fields
                            .iter()
                            .zip(right_fields)
                            .rev()
                            .map(|(left, right)| Step::Compare(&left.value, &right.value)),
                    );
                }
                variant
            }
            (RuntimeValue::Map(left), RuntimeValue::Map(right)) => {
                // Entries come in key order, so equal maps yield equal sequences
                // whatever their insertion history.
                let (left, right) = (left.entry_refs(), right.entry_refs());
                steps.push(Step::Decide(left.len().cmp(&right.len())));
                for ((left_key, left_value), (right_key, right_value)) in
                    left.into_iter().zip(right).rev()
                {
                    steps.push(Step::Compare(left_value, right_value));
                    steps.push(Step::Compare(left_key, right_key));
                }
                Ordering::Equal
            }
            // Typing admits no other pair; rank keeps the order total regardless.
            (left, right) => rank(left).cmp(&rank(right)),
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    Ordering::Equal
}

fn by_name(fields: &[RuntimeField]) -> Vec<&RuntimeValue> {
    let mut fields = fields.iter().collect::<Vec<_>>();
    fields.sort_by(|left, right| left.name.cmp(&right.name));
    fields.into_iter().map(|field| &field.value).collect()
}

const fn rank(value: &RuntimeValue) -> u8 {
    match value {
        RuntimeValue::Void => 0,
        RuntimeValue::Integer(_) => 1,
        RuntimeValue::Float(_) => 2,
        RuntimeValue::Text(_) => 3,
        RuntimeValue::Boolean(_) => 4,
        RuntimeValue::List(_) => 5,
        RuntimeValue::Record(_) => 6,
        RuntimeValue::Constructed { .. } => 7,
        RuntimeValue::Map(_) => 8,
        RuntimeValue::JsonNumber(_) => 9,
        RuntimeValue::Callable(_) => 10,
        RuntimeValue::Tuple(_) => 11,
        RuntimeValue::Range { .. } => 12,
    }
}

#[cfg(test)]
mod tests {
    use super::{RuntimeValue, structural_order};
    use std::cmp::Ordering;

    fn nested(depth: usize, leaf: i64) -> RuntimeValue {
        let mut value = RuntimeValue::Integer(leaf);
        for _ in 0..depth {
            value = RuntimeValue::List(std::iter::once(value).collect());
        }
        value
    }

    // E18: the walk keeps its own stack, so a million levels cost heap, not host stack.
    #[test]
    fn comparing_a_million_levels_of_nesting_does_not_recurse_on_the_host_stack() {
        let depth = 1_000_000;
        let (left, same, larger) = (nested(depth, 1), nested(depth, 1), nested(depth, 2));
        assert_eq!(structural_order(&left, &same, &|_| 0), Ordering::Equal);
        assert_eq!(structural_order(&left, &larger, &|_| 0), Ordering::Less);
        // The interpreter's derived `Drop` recurses, so a value this deep is leaked.
        std::mem::forget((left, same, larger));
    }
}
