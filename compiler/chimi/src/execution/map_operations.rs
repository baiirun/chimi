//! The native core of `std/map` on the reference interpreter. `runtime.c`
//! implements the same operations in `invoke_map_operation`, and the
//! `std-tests` programs run on both and must agree.

use super::{Control, Environment, Evaluator, RuntimeValue};
use crate::ids::CoreNodeId;
use crate::standard::StandardFunction;

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
        let _ = (arguments, span, environment);
        Self::not_implemented_yet(function, call_node)
    }
}
