//! `std/resource.scoped` on the reference interpreter. `runtime.c` implements
//! the same contract in `resource_scoped`; the `std-tests/resource` programs
//! run on both and must agree. The interpreter has no scheduler, so nothing
//! can cancel a release here and masking is a native concern.

use super::{Control, Environment, Evaluator, RuntimeValue};
use crate::ids::CoreNodeId;
use crate::standard::StandardFunction;

impl Evaluator<'_> {
    pub(super) fn invoke_resource_scoped(
        &mut self,
        arguments: Vec<RuntimeValue>,
        span: u64,
        call_node: CoreNodeId,
        environment: Environment,
    ) -> Control {
        let [
            RuntimeValue::Callable(acquire),
            RuntimeValue::Callable(release),
            RuntimeValue::Callable(use_resource),
        ] = arguments.as_slice()
        else {
            return self.intrinsic_shape_defect(StandardFunction::ResourceScoped, call_node);
        };
        let (release, use_resource) = (release.clone(), use_resource.clone());
        let handle = match self.invoke(acquire.clone(), Vec::new(), span, call_node, environment.clone()) {
            Control::Success(handle) => handle,
            other => return other,
        };
        let used = self.invoke(use_resource, vec![handle.clone()], span, call_node, environment.clone());
        let released = self.invoke(release, vec![handle], span, call_node, environment);
        after_release(used, released)
    }
}

/// The outcome of a scoped use once its release has run
/// (`std/resource.chimi`, `scoped`): a release failure replaces the use's value
/// or failure; a defect or cancellation in the use stays primary; a release
/// defect replaces everything except a use defect.
fn after_release(used: Control, released: Control) -> Control {
    match (used, released) {
        (used @ Control::Defect { .. }, _) => used,
        (_, released @ Control::Defect { .. }) => released,
        (used @ Control::Cancellation, _) => used,
        (_, released @ (Control::Failure(_) | Control::Cancellation)) => released,
        (used, _) => used,
    }
}
