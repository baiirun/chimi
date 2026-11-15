//! The native core of `std/set` on the reference interpreter. `runtime.c`
//! implements the same operations in `invoke_set_operation`, and the
//! `std-tests` programs run on both and must agree.

use super::{Control, Environment, Evaluator, RuntimeValue};
use crate::ids::CoreNodeId;
use crate::standard::StandardFunction;

impl Evaluator<'_> {
    /// Requires: `function` is one of the `std/set` operations of the registry.
    pub(super) fn invoke_set_operation(
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
