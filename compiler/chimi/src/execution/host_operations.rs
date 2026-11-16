//! The host core of the system modules (`std/env`) on the reference
//! interpreter. `runtime.c` implements the same operations in
//! `invoke_host_operation`; `tests/host_modules.rs` runs programs on both and
//! compares them.

use super::{Control, Evaluator, RuntimeValue};
use crate::ids::CoreNodeId;
use crate::standard::{StandardConstructor, StandardFunction};
use std::sync::Arc;

impl Evaluator<'_> {
    pub(super) fn invoke_host_operation(
        &self,
        function: StandardFunction,
        arguments: Vec<RuntimeValue>,
        call_node: CoreNodeId,
    ) -> Control {
        if !arguments.is_empty() {
            return self.intrinsic_shape_defect(function, call_node);
        }
        if self.config.cancellation_requested {
            return Control::Cancellation(None);
        }
        match function {
            // `vars_os` reads `environ` in order, as the native runtime does;
            // a pair that is not UTF-8 is left out on both.
            StandardFunction::EnvHostVariables => Control::Success(RuntimeValue::List(
                std::env::vars_os()
                    .filter_map(|(name, value)| {
                        Some((name.into_string().ok()?, value.into_string().ok()?))
                    })
                    .map(|(name, value)| {
                        RuntimeValue::Tuple(Arc::new(vec![
                            RuntimeValue::Text(name),
                            RuntimeValue::Text(value),
                        ]))
                    })
                    .collect(),
            )),
            StandardFunction::EnvHostCurrentDirectory => Control::Success(
                match std::env::current_dir()
                    .ok()
                    .and_then(|path| path.into_os_string().into_string().ok())
                {
                    Some(path) => self.standard_value(
                        StandardConstructor::OptionSome,
                        vec![RuntimeValue::Text(path)],
                    ),
                    None => self.standard_value(StandardConstructor::OptionNone, Vec::new()),
                },
            ),
            _ => self.intrinsic_shape_defect(function, call_node),
        }
    }
}
