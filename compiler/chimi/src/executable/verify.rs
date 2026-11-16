//! The Core verifier: the process invariants of verified Core that the backends rely
//! on (`process-and-supervisor-static-contracts.md`, "Verified Core invariants").
//!
//! `lower` runs it in debug builds and tests; the conformance lab runs it on Core it
//! builds by hand (CO02, CO04). Release builds trust the frontend. A violation is a
//! compiler bug, never a user diagnostic.

use super::{CoreNode, CoreOperation, CorePattern, CoreProgram, CoreStep, CoreSymbolKind};
use crate::ids::{CoreNodeId, SymbolId, TypeId};
use crate::standard::{StandardFunction, StandardSymbol, StandardType};
use crate::typing::{CanonicalType, PropertyDerivation, TypedProject};
use std::collections::{HashMap, HashSet};
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CoreViolation {
    /// The second argument of a `Process.register` call is not a reference to a
    /// `process name` symbol (CO02).
    RegisterTakesComputedName { call: CoreNodeId },
    /// A node creates a `Process.Inbox` instead of reading one that a body parameter
    /// introduced (CO04).
    InboxOutsideBody { node: CoreNodeId },
    /// A `Process.spec` body closure captures, or is provided with, a value that is
    /// never shareable.
    SpecBodyNotShareable { call: CoreNodeId, value: CoreNodeId },
}

impl fmt::Display for CoreViolation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RegisterTakesComputedName { call } => write!(
                formatter,
                "Process.register call n{} takes a name that is not a `process name` reference",
                call.raw()
            ),
            Self::InboxOutsideBody { node } => write!(
                formatter,
                "node n{} produces a Process.Inbox that no process body parameter introduced",
                node.raw()
            ),
            Self::SpecBodyNotShareable { call, value } => write!(
                formatter,
                "Process.spec call n{} snapshots node n{}, which is not shareable",
                call.raw(),
                value.raw()
            ),
        }
    }
}

/// Ensures: `Ok` exactly when, over every node of `core`:
/// - every `Process.register` call's second argument is (through parentheses) a
///   `Reference` to a `CoreSymbolKind::ProcessName` symbol;
/// - every node typed `Process.Inbox<_>` only reads an inbox: a reference to a local
///   (a parameter, binding, or capture), a call of a project function or closure, a
///   standard call that is given an inbox-holding argument, or a node that forwards a
///   child's value. Every inbox therefore starts at a parameter, and the runtime binds
///   an inbox parameter only for a process body;
/// - every `Process.spec` body that is a closure captures only shareable values, and
///   providers bound to the body are shareable.
///
/// Requires: `typed` is the project `core` was lowered from (node types index it).
pub(crate) fn verify_core(core: &CoreProgram, typed: &TypedProject) -> Result<(), CoreViolation> {
    let verifier = Verifier::new(core, typed);
    for node in &core.nodes {
        verifier.check_node(node)?;
    }
    Ok(())
}

struct Verifier<'a> {
    core: &'a CoreProgram,
    typed: &'a TypedProject,
    /// The type of each symbol, from the references to it.
    reference_types: HashMap<SymbolId, TypeId>,
}

impl<'a> Verifier<'a> {
    fn new(core: &'a CoreProgram, typed: &'a TypedProject) -> Self {
        let mut reference_types = HashMap::new();
        for node in &core.nodes {
            if let CoreOperation::Reference(symbol) = node.operation {
                reference_types.entry(symbol).or_insert(node.type_id);
            }
        }
        Self {
            core,
            typed,
            reference_types,
        }
    }

    fn check_node(&self, node: &CoreNode) -> Result<(), CoreViolation> {
        if let CoreOperation::Call { callee, arguments } = &node.operation {
            match self.intrinsic(*callee) {
                Some(StandardFunction::ProcessRegister) => {
                    let named = arguments
                        .get(1)
                        .and_then(|name| self.referenced_symbol(*name))
                        .is_some_and(|symbol| {
                            self.core.symbol(symbol).kind == CoreSymbolKind::ProcessName
                        });
                    if !named {
                        return Err(CoreViolation::RegisterTakesComputedName { call: node.id });
                    }
                }
                Some(StandardFunction::ProcessSpec | StandardFunction::ProcessHandoffSpec) => {
                    if let Some(body) = arguments.first() {
                        self.check_spec_body(node.id, *body)?;
                    }
                }
                _ => {}
            }
        }
        if self.is_inbox(node.type_id) && !self.reads_an_inbox(node) {
            return Err(CoreViolation::InboxOutsideBody { node: node.id });
        }
        Ok(())
    }

    fn reads_an_inbox(&self, node: &CoreNode) -> bool {
        match &node.operation {
            CoreOperation::Reference(symbol) => {
                self.core.symbol(*symbol).kind == CoreSymbolKind::Local
            }
            CoreOperation::Call { callee, arguments } => match self.intrinsic(*callee) {
                None => true,
                Some(_) => arguments
                    .iter()
                    .any(|argument| self.mentions_inbox(self.node(*argument).type_id)),
            },
            CoreOperation::Select { .. }
            | CoreOperation::Identity(_)
            | CoreOperation::Provision { .. }
            | CoreOperation::Sequence(_)
            | CoreOperation::Match { .. }
            | CoreOperation::Loop { .. }
            | CoreOperation::Break { .. } => true,
            CoreOperation::Constant(_)
            | CoreOperation::DependencyMethod { .. }
            | CoreOperation::Construct { .. }
            | CoreOperation::List(_)
            | CoreOperation::Tuple(_)
            | CoreOperation::Record(_)
            | CoreOperation::Unary { .. }
            | CoreOperation::Binary { .. }
            | CoreOperation::Closure { .. } => false,
        }
    }

    fn check_spec_body(&self, call: CoreNodeId, body: CoreNodeId) -> Result<(), CoreViolation> {
        let not_shareable = |value: CoreNodeId, type_id: TypeId| {
            (self.typed.value_properties(type_id).shareable == PropertyDerivation::Never)
                .then_some(CoreViolation::SpecBodyNotShareable { call, value })
        };
        match &self.node(body).operation {
            CoreOperation::Identity(inner) => self.check_spec_body(call, *inner),
            CoreOperation::Provision {
                expression,
                providers,
            } => {
                for provider in providers {
                    if let Some(violation) =
                        not_shareable(provider.value, self.node(provider.value).type_id)
                    {
                        return Err(violation);
                    }
                }
                self.check_spec_body(call, *expression)
            }
            CoreOperation::Closure {
                parameters,
                captures,
                body: closure_body,
            } => {
                // A closure's captures also list the names it binds itself, its
                // parameters and its locals; only names bound outside it are
                // snapshotted into the spec.
                let own = self.bound_within(parameters, *closure_body);
                for capture in captures.iter().filter(|capture| !own.contains(capture)) {
                    if let Some(type_id) = self.reference_types.get(capture)
                        && let Some(violation) = not_shareable(body, *type_id)
                    {
                        return Err(violation);
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// The symbols a closure binds: its parameters and every local, loop state,
    /// pattern binding, cause and nested closure parameter in its body.
    fn bound_within(&self, parameters: &[SymbolId], body: CoreNodeId) -> HashSet<SymbolId> {
        fn pattern_bindings(pattern: &CorePattern, bound: &mut HashSet<SymbolId>) {
            match pattern {
                CorePattern::Wildcard | CorePattern::Literal(_) => {}
                CorePattern::Binding(symbol) => {
                    bound.insert(*symbol);
                }
                CorePattern::Constructor { fields, .. } | CorePattern::Tuple(fields) => {
                    fields
                        .iter()
                        .for_each(|field| pattern_bindings(field, bound));
                }
                CorePattern::Outcome { value, .. } => pattern_bindings(value, bound),
                CorePattern::List {
                    prefix,
                    rest,
                    suffix,
                } => {
                    for element in prefix.iter().chain(rest.as_deref()).chain(suffix) {
                        pattern_bindings(element, bound);
                    }
                }
                CorePattern::Alias { pattern, binding } => {
                    bound.insert(*binding);
                    pattern_bindings(pattern, bound);
                }
            }
        }
        let mut bound = parameters.iter().copied().collect::<HashSet<_>>();
        let mut pending = vec![body];
        while let Some(id) = pending.pop() {
            match &self.node(id).operation {
                CoreOperation::Constant(_)
                | CoreOperation::Reference(_)
                | CoreOperation::DependencyMethod { .. } => {}
                CoreOperation::Select { receiver, .. } => pending.push(*receiver),
                CoreOperation::Construct { arguments, .. } => pending.extend(arguments),
                CoreOperation::Call { callee, arguments } => {
                    pending.push(*callee);
                    pending.extend(arguments);
                }
                CoreOperation::List(items) | CoreOperation::Tuple(items) => {
                    pending.extend(items);
                }
                CoreOperation::Record(fields) => {
                    pending.extend(fields.iter().map(|field| field.value));
                }
                CoreOperation::Unary { operand, .. } | CoreOperation::Identity(operand) => {
                    pending.push(*operand);
                }
                CoreOperation::Binary { left, right, .. } => pending.extend([*left, *right]),
                CoreOperation::Provision {
                    expression,
                    providers,
                } => {
                    pending.push(*expression);
                    pending.extend(providers.iter().map(|field| field.value));
                }
                CoreOperation::Sequence(steps) => {
                    for step in steps {
                        match step {
                            CoreStep::Bind { symbol, value } => {
                                bound.insert(*symbol);
                                pending.push(*value);
                            }
                            CoreStep::Expression(value) => pending.push(*value),
                        }
                    }
                }
                CoreOperation::Closure {
                    parameters, body, ..
                } => {
                    bound.extend(parameters.iter().copied());
                    pending.push(*body);
                }
                CoreOperation::Match { subjects, arms } => {
                    pending.extend(subjects);
                    for arm in arms {
                        bound.extend(arm.cause.map(|cause| cause.local));
                        arm.patterns
                            .iter()
                            .for_each(|pattern| pattern_bindings(pattern, &mut bound));
                        pending.extend(arm.guard);
                        pending.push(arm.body);
                    }
                }
                CoreOperation::Loop {
                    state,
                    initial,
                    body,
                } => {
                    bound.insert(*state);
                    pending.extend([*initial, *body]);
                }
                CoreOperation::Break { value, .. } => pending.extend(value),
            }
        }
        bound
    }

    fn node(&self, id: CoreNodeId) -> &CoreNode {
        &self.core.nodes[id.raw() as usize]
    }

    /// The symbol a node references, through parentheses.
    fn referenced_symbol(&self, id: CoreNodeId) -> Option<SymbolId> {
        match self.node(id).operation {
            CoreOperation::Reference(symbol) => Some(symbol),
            CoreOperation::Identity(inner) => self.referenced_symbol(inner),
            _ => None,
        }
    }

    /// The standard function a callee node references, if it is an intrinsic.
    fn intrinsic(&self, callee: CoreNodeId) -> Option<StandardFunction> {
        let symbol = self.core.symbol(self.referenced_symbol(callee)?);
        match (symbol.kind, symbol.standard) {
            (CoreSymbolKind::Intrinsic, Some(StandardSymbol::Function(function))) => Some(function),
            _ => None,
        }
    }

    fn is_inbox(&self, type_id: TypeId) -> bool {
        matches!(
            &self.typed.types[self.typed.resolve_type_id(type_id).raw() as usize],
            CanonicalType::Named { symbol, .. }
                if self.typed.resolved.standard_symbol(*symbol)
                    == Some(StandardSymbol::Type(StandardType::ProcessInbox))
        )
    }

    /// Whether a value of `type_id` can hold an inbox: the type or any type argument,
    /// tuple element, or record field is `Process.Inbox<_>`. Struct and enum fields are
    /// not followed, so such a value is treated as holding none.
    fn mentions_inbox(&self, type_id: TypeId) -> bool {
        let mut pending = vec![type_id];
        let mut seen = HashSet::new();
        while let Some(type_id) = pending.pop() {
            let type_id = self.typed.resolve_type_id(type_id);
            if !seen.insert(type_id) {
                continue;
            }
            if self.is_inbox(type_id) {
                return true;
            }
            match &self.typed.types[type_id.raw() as usize] {
                CanonicalType::Named { arguments, .. } | CanonicalType::Tuple(arguments) => {
                    pending.extend(arguments.iter().copied());
                }
                CanonicalType::Record(fields) => {
                    pending.extend(fields.iter().map(|field| field.type_id));
                }
                _ => {}
            }
        }
        false
    }
}
