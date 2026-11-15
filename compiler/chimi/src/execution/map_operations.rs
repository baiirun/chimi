//! The native cores of `std/map` and `std/set` on the reference interpreter.
//! `runtime.c` implements the same operations in `invoke_map_operation` and
//! `invoke_set_operation`, and the `std-tests` programs run on both and must
//! agree. A set is a map from each item to `Void`.

use super::{Control, Environment, Evaluator, RuntimeCallable, RuntimeMap, RuntimeValue};
use crate::ids::CoreNodeId;
use crate::standard::{StandardConstructor, StandardFunction};

impl Evaluator<'_> {
    /// Requires: `function` is one of the `std/map` operations of the registry.
    pub(super) fn invoke_map_operation(
        &mut self,
        function: StandardFunction,
        arguments: Vec<RuntimeValue>,
        span: u64,
        call_node: CoreNodeId,
        environment: Environment,
    ) -> Control {
        let map = |map: RuntimeMap| Control::Success(RuntimeValue::Map(map));
        match (function, arguments.as_slice()) {
            (StandardFunction::MapEmpty, []) => map(RuntimeMap::default()),
            (StandardFunction::MapGet, [RuntimeValue::Map(entries), key]) => {
                let found = entries.get(key, &self.key_order());
                Control::Success(match found {
                    Some(value) => {
                        self.standard_value(StandardConstructor::OptionSome, vec![value])
                    }
                    None => self.standard_value(StandardConstructor::OptionNone, Vec::new()),
                })
            }
            (StandardFunction::MapPut, [RuntimeValue::Map(entries), key, value]) => {
                map(entries.put(key.clone(), value.clone(), &self.key_order()))
            }
            (StandardFunction::MapRemove, [RuntimeValue::Map(entries), key]) => map(entries
                .remove(key, &self.key_order())
                .unwrap_or_else(|| entries.clone())),
            (StandardFunction::MapSize, [RuntimeValue::Map(entries)]) => size(entries),
            (
                StandardFunction::MapReduceEntries,
                [
                    RuntimeValue::Map(entries),
                    initial,
                    RuntimeValue::Callable(step),
                ],
            ) => {
                let steps = entries
                    .entries()
                    .into_iter()
                    .map(|(key, value)| vec![key, value]);
                let (initial, step) = (initial.clone(), step.clone());
                self.reduce_in_order(step, initial, steps, span, call_node, environment)
            }
            _ => self.intrinsic_shape_defect(function, call_node),
        }
    }

    /// Requires: `function` is one of the `std/set` operations of the registry.
    pub(super) fn invoke_set_operation(
        &mut self,
        function: StandardFunction,
        arguments: Vec<RuntimeValue>,
        span: u64,
        call_node: CoreNodeId,
        environment: Environment,
    ) -> Control {
        let set = |items: RuntimeMap| Control::Success(RuntimeValue::Set(items));
        match (function, arguments.as_slice()) {
            (StandardFunction::SetEmpty, []) => set(RuntimeMap::default()),
            (StandardFunction::SetContains, [RuntimeValue::Set(items), item]) => Control::Success(
                RuntimeValue::Boolean(items.get(item, &self.key_order()).is_some()),
            ),
            // `put` keeps an equal item already held.
            (StandardFunction::SetAdd, [RuntimeValue::Set(items), item]) => {
                set(items.put(item.clone(), RuntimeValue::Void, &self.key_order()))
            }
            (StandardFunction::SetRemove, [RuntimeValue::Set(items), item]) => set(items
                .remove(item, &self.key_order())
                .unwrap_or_else(|| items.clone())),
            (StandardFunction::SetSize, [RuntimeValue::Set(items)]) => size(items),
            (
                StandardFunction::SetReduceItems,
                [
                    RuntimeValue::Set(items),
                    initial,
                    RuntimeValue::Callable(step),
                ],
            ) => {
                let steps = items.entries().into_iter().map(|(item, _)| vec![item]);
                let (initial, step) = (initial.clone(), step.clone());
                self.reduce_in_order(step, initial, steps, span, call_node, environment)
            }
            _ => self.intrinsic_shape_defect(function, call_node),
        }
    }

    /// Calls `step` with the state, starting at `initial`, followed by each
    /// group of `steps`, and stops at the first call that does not succeed.
    fn reduce_in_order(
        &mut self,
        step: RuntimeCallable,
        initial: RuntimeValue,
        steps: impl Iterator<Item = Vec<RuntimeValue>>,
        span: u64,
        call_node: CoreNodeId,
        environment: Environment,
    ) -> Control {
        let mut state = initial;
        for parts in steps {
            let mut arguments = vec![state];
            arguments.extend(parts);
            match self.invoke(
                step.clone(),
                arguments,
                span,
                call_node,
                environment.clone(),
            ) {
                Control::Success(next) => state = next,
                other => return other,
            }
        }
        Control::Success(state)
    }
}

fn size(entries: &RuntimeMap) -> Control {
    Control::Success(RuntimeValue::Integer(
        i64::try_from(entries.size()).unwrap_or(i64::MAX),
    ))
}
