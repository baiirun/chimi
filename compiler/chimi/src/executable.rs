use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::{CoreNodeId, FileId, ModuleId, OriginId, SemanticId, SymbolId, TextRange, TypeId};
use crate::resolution::{
    Namespace, ProjectSymbolKind, ReferenceResolution, Resolution, SymbolKind,
};
use crate::semantic::{
    BinaryOperator, CaseOrigin, Literal, ModuleBlockItem, ModuleCaseArm, ModuleDeclarationKind,
    ModuleExpression, Pattern, PatternLiteral, UnaryOperator,
};
use crate::standard::{StandardConstant, StandardFunction, StandardSymbol};
use crate::typing::{CanonicalType, TypeFact, TypedProgram, TypedProject};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

mod verify;
pub(crate) use verify::{CoreViolation, verify_core};

#[derive(Clone, Debug)]
pub(crate) struct Origin {
    pub(crate) origin_id: OriginId,
    pub(crate) file_id: FileId,
    pub(crate) module_id: ModuleId,
    pub(crate) semantic_id: SemanticId,
    pub(crate) range: TextRange,
}

#[derive(Clone, Debug)]
pub(crate) struct LegacyCoreProgram {
    pub(crate) computation: String,
    pub(crate) root_node_id: CoreNodeId,
    pub(crate) origin: Origin,
    pub(crate) constructor_name: String,
    pub(crate) field_name: String,
    pub(crate) value: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct CoreProgram {
    pub(crate) computation: String,
    pub(crate) entry: SymbolId,
    /// A test build's `test` items in module path then declaration order; empty for
    /// a program. `entry` is the first of them.
    pub(crate) tests: Vec<SymbolId>,
    type_count: usize,
    pub(crate) symbols: Vec<CoreSymbol>,
    pub(crate) functions: Vec<CoreFunction>,
    /// Every constant's initializer, kept so diagnostics and tools can point at it.
    /// Nothing evaluates an initializer after the build.
    pub(crate) constant_initializers: Vec<CoreConstantInitializer>,
    /// Every constant's build-time value, one per initializer, in initializer order.
    pub(crate) constants: Vec<CoreConstant>,
    pub(crate) nodes: Vec<CoreNode>,
    pub(crate) origins: Vec<Origin>,
    /// Where each origin starts, as `module/path.chimi:line`, which causes show.
    pub(crate) origin_locations: Vec<String>,
}

impl CoreProgram {
    pub(crate) const fn type_count(&self) -> usize {
        self.type_count
    }

    /// The ids of the call nodes whose callee is a `std/test` operation that
    /// fails the test as an assertion: the four assertions, `next` on an
    /// exhausted script, and `capture_defect` when its operation did not defect. The runner reports a
    /// defect at one of these as an assertion, and `capture_defect` lets one
    /// through instead of capturing it (testing.md, "Defects"). Program data
    /// gives the native runtime the same set (`CHIMI_ASSERTION_NODES`).
    pub(crate) fn assertion_calls(&self) -> std::collections::HashSet<u32> {
        let assertion = |node: CoreNodeId| match self.nodes[node.raw() as usize].operation {
            CoreOperation::Reference(symbol) => matches!(
                self.symbol(symbol).standard,
                Some(StandardSymbol::Function(
                    StandardFunction::TestAssertEqual
                        | StandardFunction::TestAssertNotEqual
                        | StandardFunction::TestAssert
                        | StandardFunction::TestFail
                        | StandardFunction::TestCaptureDefect
                        | StandardFunction::TestNext
                ))
            ),
            _ => false,
        };
        self.nodes
            .iter()
            .filter(|node| {
                matches!(&node.operation, CoreOperation::Call { callee, .. } if assertion(*callee))
            })
            .map(|node| node.id.raw())
            .collect()
    }

    pub(crate) fn symbol(&self, id: SymbolId) -> &CoreSymbol {
        self.symbols
            .get(id.raw() as usize)
            .filter(|symbol| symbol.id == id)
            .expect("core symbol ID belongs to this program")
    }

    /// Replaces one node's operation, keeping its type and origin. The conformance lab
    /// uses it to build Core the frontend never produces, to show that `verify_core`
    /// rejects it (CO02, CO04); nothing in the compiler calls it.
    #[cfg(feature = "native-backend")]
    pub(crate) fn replace_operation(&mut self, node: CoreNodeId, operation: CoreOperation) {
        self.nodes[node.raw() as usize].operation = operation;
    }

    fn assert_well_formed(&self) {
        debug_assert!(
            self.functions
                .iter()
                .any(|function| function.symbol == self.entry)
        );
        debug_assert!(!self.computation.is_empty());
        for (index, symbol) in self.symbols.iter().enumerate() {
            debug_assert_eq!(symbol.id.raw() as usize, index);
        }
        for (index, node) in self.nodes.iter().enumerate() {
            debug_assert_eq!(node.id.raw() as usize, index);
            debug_assert!((node.origin.raw() as usize) < self.origins.len());
            debug_assert!((node.type_id.raw() as usize) < self.type_count);
            self.assert_operation(&node.operation);
        }
        for function in &self.functions {
            debug_assert!((function.symbol.raw() as usize) < self.symbols.len());
            debug_assert!(
                function
                    .parameters
                    .iter()
                    .all(|parameter| (parameter.raw() as usize) < self.symbols.len())
            );
            debug_assert!((function.body.raw() as usize) < self.nodes.len());
        }
        for initializer in &self.constant_initializers {
            debug_assert!((initializer.symbol.raw() as usize) < self.symbols.len());
            debug_assert!((initializer.value.raw() as usize) < self.nodes.len());
        }
        for (index, origin) in self.origins.iter().enumerate() {
            debug_assert_eq!(origin.origin_id.raw() as usize, index);
            let _ = (
                origin.file_id,
                origin.module_id,
                origin.semantic_id,
                origin.range,
            );
        }
    }

    pub(crate) fn stats(
        &self,
    ) -> (
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
    ) {
        let mut constants = 0;
        let mut constructions = 0;
        let mut calls = 0;
        let mut sequences = 0;
        let mut dependencies = 0;
        for node in &self.nodes {
            match node.operation {
                CoreOperation::Constant(_) => constants += 1,
                CoreOperation::Construct { .. } => constructions += 1,
                CoreOperation::Call { .. } => calls += 1,
                CoreOperation::Sequence(_) => sequences += 1,
                CoreOperation::DependencyMethod { .. } => dependencies += 1,
                _ => {}
            }
        }
        (
            self.functions.len(),
            self.constant_initializers.len(),
            self.nodes.len(),
            self.origins.len(),
            constants,
            constructions,
            calls,
            sequences,
            dependencies,
        )
    }

    fn assert_operation(&self, operation: &CoreOperation) {
        let node_exists = |node: CoreNodeId| (node.raw() as usize) < self.nodes.len();
        match operation {
            CoreOperation::Constant(literal) => touch_literal(literal),
            CoreOperation::Reference(symbol) => {
                debug_assert!((symbol.raw() as usize) < self.symbols.len());
            }
            CoreOperation::Select { receiver, member } => {
                debug_assert!(node_exists(*receiver));
                debug_assert!(!member.is_empty());
            }
            CoreOperation::DependencyMethod {
                requirement,
                interface,
                method,
                name,
            } => {
                debug_assert!((requirement.raw() as usize) < self.symbols.len());
                debug_assert!((interface.raw() as usize) < self.symbols.len());
                let _ = method;
                debug_assert!(!name.is_empty());
            }
            CoreOperation::Construct {
                constructor,
                arguments,
                cause,
            } => {
                debug_assert!((constructor.raw() as usize) < self.symbols.len());
                debug_assert!(
                    cause.is_none_or(|cause| { (cause.local.raw() as usize) < self.symbols.len() })
                );
                debug_assert!(arguments.iter().all(|argument| node_exists(*argument)));
            }
            CoreOperation::Call { callee, arguments } => {
                debug_assert!(node_exists(*callee));
                debug_assert!(arguments.iter().all(|argument| node_exists(*argument)));
            }
            CoreOperation::List(elements) | CoreOperation::Tuple(elements) => {
                debug_assert!(elements.iter().all(|element| node_exists(*element)));
            }
            CoreOperation::Record(fields) => {
                for field in fields {
                    debug_assert!(!field.name.is_empty());
                    debug_assert!(node_exists(field.value));
                }
            }
            CoreOperation::Unary { operator, operand } => {
                debug_assert_ne!(*operator, UnaryOperator::Not, "`not` lowers to a match");
                debug_assert!(node_exists(*operand));
            }
            CoreOperation::Binary {
                operator,
                left,
                right,
            } => {
                debug_assert!(
                    !matches!(
                        operator,
                        BinaryOperator::And | BinaryOperator::Or | BinaryOperator::NotEqual
                    ),
                    "`and`, `or`, and `!=` lower to a match"
                );
                debug_assert!(node_exists(*left));
                debug_assert!(node_exists(*right));
            }
            CoreOperation::Identity(value) => debug_assert!(node_exists(*value)),
            CoreOperation::Provision {
                expression,
                providers,
            } => {
                debug_assert!(node_exists(*expression));
                for provider in providers {
                    debug_assert!(!provider.name.is_empty());
                    debug_assert!(node_exists(provider.value));
                }
            }
            CoreOperation::Sequence(steps) => {
                for step in steps {
                    match step {
                        CoreStep::Bind { symbol, value } => {
                            debug_assert!((symbol.raw() as usize) < self.symbols.len());
                            debug_assert!(node_exists(*value));
                        }
                        CoreStep::Expression(value) => debug_assert!(node_exists(*value)),
                    }
                }
            }
            CoreOperation::Closure {
                parameters,
                captures,
                body,
            } => {
                debug_assert!(
                    parameters
                        .iter()
                        .all(|parameter| (parameter.raw() as usize) < self.symbols.len())
                );
                debug_assert!(
                    captures
                        .iter()
                        .all(|capture| (capture.raw() as usize) < self.symbols.len())
                );
                debug_assert!(node_exists(*body));
            }
            CoreOperation::Match { subjects, arms } => {
                debug_assert!(subjects.iter().all(|subject| node_exists(*subject)));
                for arm in arms {
                    for pattern in &arm.patterns {
                        touch_pattern(pattern);
                    }
                    if let Some(guard) = arm.guard {
                        debug_assert!(node_exists(guard));
                    }
                    debug_assert!(node_exists(arm.body));
                }
            }
            CoreOperation::Loop {
                state,
                initial,
                body,
            } => {
                debug_assert!((state.raw() as usize) < self.symbols.len());
                debug_assert!(node_exists(*initial));
                debug_assert!(node_exists(*body));
            }
            CoreOperation::Break { target, value } => {
                debug_assert!((target.raw() as usize) < self.symbols.len());
                debug_assert!(value.is_none_or(node_exists));
            }
        }
    }
}

/// `case subject { True -> when_true  False -> when_false }`, the meaning of `and`,
/// `or`, and `not`. Only the selected arm runs, which makes the operators short-circuit.
fn boolean_match(
    subject: CoreNodeId,
    when_true: CoreNodeId,
    when_false: CoreNodeId,
) -> CoreOperation {
    let arm = |value: bool, body: CoreNodeId| CoreArm {
        patterns: vec![CorePattern::Literal(CoreLiteral::Boolean(value))],
        guard: None,
        body,
        cause: None,
    };
    CoreOperation::Match {
        subjects: vec![subject],
        arms: vec![arm(true, when_true), arm(false, when_false)],
    }
}

fn touch_literal(literal: &CoreLiteral) {
    match literal {
        CoreLiteral::Integer(value) => {
            let _ = value;
        }
        CoreLiteral::Float(value) => debug_assert!(value.is_finite()),
        CoreLiteral::Text(value) => {
            let _ = value;
        }
        CoreLiteral::Boolean(value) => {
            let _ = value;
        }
        CoreLiteral::Void => {}
    }
}

fn touch_pattern(pattern: &CorePattern) {
    match pattern {
        CorePattern::Wildcard => {}
        CorePattern::Binding(symbol) => {
            let _ = symbol;
        }
        CorePattern::Literal(literal) => touch_literal(literal),
        CorePattern::Constructor { symbol, fields } => {
            let _ = symbol;
            for field in fields {
                touch_pattern(field);
            }
        }
        CorePattern::Tuple(elements) => {
            for element in elements {
                touch_pattern(element);
            }
        }
        CorePattern::Outcome { success, value } => {
            let _ = success;
            touch_pattern(value);
        }
        CorePattern::List {
            prefix,
            rest,
            suffix,
        } => {
            for element in prefix.iter().chain(rest.as_deref()).chain(suffix) {
                touch_pattern(element);
            }
        }
        CorePattern::Alias { pattern, binding } => {
            let _ = binding;
            touch_pattern(pattern);
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CoreSymbol {
    pub(crate) id: SymbolId,
    pub(crate) name: String,
    pub(crate) kind: CoreSymbolKind,
    pub(crate) constructor: Option<CoreConstructor>,
    pub(crate) standard: Option<StandardSymbol>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoreSymbolKind {
    Function,
    /// A module constant; a reference loads its build-time value.
    Constant,
    /// A `process name` declaration; a reference is its named `Process.Address`,
    /// identified by this symbol, never by its text.
    ProcessName,
    Requirement,
    Local,
    Constructor,
    Intrinsic,
    Other,
}

#[derive(Clone, Debug)]
pub(crate) struct CoreConstructor {
    pub(crate) type_name: String,
    pub(crate) variant_name: Option<String>,
    pub(crate) fields: Vec<String>,
    pub(crate) outcome_success: Option<bool>,
    /// The constructor's place in one order over every constructor in the
    /// program. A type's constructors are contiguous and in declaration order,
    /// which the derived order follows; distinct types never share an ordinal,
    /// so values of two enums in one failure set are never equal. The order
    /// between types is internal: types are numbered by first symbol.
    pub(crate) ordinal: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct CoreFunction {
    pub(crate) symbol: SymbolId,
    pub(crate) parameters: Vec<SymbolId>,
    pub(crate) body: CoreNodeId,
}

#[derive(Clone, Debug)]
pub(crate) struct CoreConstantInitializer {
    pub(crate) symbol: SymbolId,
    pub(crate) value: CoreNodeId,
}

#[derive(Clone, Debug)]
pub(crate) struct CoreConstant {
    pub(crate) symbol: SymbolId,
    pub(crate) value: ConstantValue,
}

/// A value computed at build time and stored in the executable.
///
/// Invariant: it holds no runtime capability and no host provider (a provider is
/// itself a constant value), so every backend can store it as immutable data and
/// every task can read it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ConstantValue {
    Void,
    Integer(i64),
    /// Always finite.
    Float(f64),
    Text(String),
    Boolean(bool),
    List(Vec<ConstantValue>),
    Tuple(Vec<ConstantValue>),
    /// A normalized half-open range.
    Range {
        start: i64,
        end: i64,
    },
    /// A `Duration` in nanoseconds.
    Duration(i64),
    Decimal(crate::decimal::Decimal),
    /// A `std/bytes.Bytes` as lowercase hex, two digits per byte, so program
    /// data keeps it in the string table.
    Bytes(String),
    /// A named `Process.Address`: the `process name` declaration's symbol.
    ProcessAddress(SymbolId),
    Record(Vec<(String, ConstantValue)>),
    /// A struct or enum value; field names come from the constructor's shape.
    Constructed {
        constructor: SymbolId,
        fields: Vec<ConstantValue>,
    },
    /// Entries in ascending byte order of their keys.
    /// Key and value pairs; keys are any equatable value.
    Map(Vec<(ConstantValue, ConstantValue)>),
    /// Items in ascending structural order.
    Set(Vec<ConstantValue>),
    JsonNumber(String),
    Function(ConstantFunction),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ConstantFunction {
    /// `requirements` holds the providers a `using` boundary bound to the function,
    /// as when `const configured = timestamp using { clock: fake }`.
    Declared {
        symbol: SymbolId,
        requirements: ConstantRequirements,
    },
    Intrinsic(SymbolId),
    Constructor(SymbolId),
    /// `node` is the `CoreOperation::Closure` that made it; `captures` binds exactly
    /// that node's capture symbols.
    Closure {
        node: CoreNodeId,
        captures: Vec<(SymbolId, ConstantValue)>,
        requirements: ConstantRequirements,
    },
}

/// Providers bound to a function value, by requirement name in ascending byte order,
/// so the stored data does not depend on hash order.
pub(crate) type ConstantRequirements = Vec<(String, ConstantValue)>;

#[derive(Clone, Debug)]
pub(crate) struct CoreNode {
    pub(crate) id: CoreNodeId,
    pub(crate) origin: OriginId,
    pub(crate) type_id: TypeId,
    pub(crate) operation: CoreOperation,
}

#[derive(Clone, Debug)]
pub(crate) enum CoreOperation {
    Constant(CoreLiteral),
    Reference(SymbolId),
    Select {
        receiver: CoreNodeId,
        member: String,
    },
    DependencyMethod {
        requirement: SymbolId,
        interface: SymbolId,
        method: SemanticId,
        name: String,
    },
    /// `cause` is set only for a `Failure(...)` or `Result.Err(...)` written in
    /// the body of an arm that matched a live failure or a stored `Err`
    /// (time-and-outcomes.md, "Cause chains").
    Construct {
        constructor: SymbolId,
        arguments: Vec<CoreNodeId>,
        cause: Option<ConstructCause>,
    },
    Call {
        callee: CoreNodeId,
        arguments: Vec<CoreNodeId>,
    },
    List(Vec<CoreNodeId>),
    /// A tuple of two or more elements, evaluated left to right.
    Tuple(Vec<CoreNodeId>),
    Record(Vec<CoreField>),
    Unary {
        operator: UnaryOperator,
        operand: CoreNodeId,
    },
    Binary {
        operator: BinaryOperator,
        left: CoreNodeId,
        right: CoreNodeId,
    },
    Identity(CoreNodeId),
    Provision {
        expression: CoreNodeId,
        providers: Vec<CoreField>,
    },
    Sequence(Vec<CoreStep>),
    Closure {
        parameters: Vec<SymbolId>,
        captures: Vec<SymbolId>,
        body: CoreNodeId,
    },
    Match {
        subjects: Vec<CoreNodeId>,
        arms: Vec<CoreArm>,
    },
    /// Binds `state` to `initial`, then evaluates `body` repeatedly, rebinding `state` to
    /// each successful body value. Ends only when a `Break` naming this `state` arrives.
    /// `state` doubles as the loop's identity: every loop defines exactly one.
    Loop {
        state: SymbolId,
        initial: CoreNodeId,
        body: CoreNodeId,
    },
    /// Leaves the loop whose state symbol is `target` with `value`, or `Void` when absent.
    Break {
        target: SymbolId,
        value: Option<CoreNodeId>,
    },
}

#[derive(Clone, Debug)]
pub(crate) enum CoreLiteral {
    Integer(i64),
    /// A finite `Float`. Never a pattern: `Float` has no literal patterns.
    Float(f64),
    Text(String),
    Boolean(bool),
    /// The `Void` value. Never a pattern: a `Void` pattern lowers to a wildcard,
    /// because `Void` has one value.
    Void,
}

#[derive(Clone, Debug)]
pub(crate) struct CoreField {
    pub(crate) name: String,
    pub(crate) value: CoreNodeId,
}

#[derive(Clone, Debug)]
pub(crate) enum CoreStep {
    Bind { symbol: SymbolId, value: CoreNodeId },
    Expression(CoreNodeId),
}

#[derive(Clone, Debug)]
pub(crate) struct CoreArm {
    pub(crate) patterns: Vec<CorePattern>,
    pub(crate) guard: Option<CoreNodeId>,
    pub(crate) body: CoreNodeId,
    /// Binds the cause of one matched subject for the constructions in `body`.
    pub(crate) cause: Option<ArmCause>,
}

/// When an arm matches, the cause of subject `subject` is bound to the
/// compiler-generated local `local`: the cause of a live failure, or the cause a
/// stored `Err` carries, or `Void` when that `Err` carries none.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ArmCause {
    pub(crate) subject: usize,
    pub(crate) local: SymbolId,
}

/// How a construction in a cause arm uses the matched cause in `local`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ConstructCause {
    pub(crate) local: SymbolId,
    /// The argument is the variable the arm bound to the matched failure itself,
    /// so the construction keeps that cause. Otherwise it gets a new cause whose
    /// `caused_by` is the matched one.
    pub(crate) keep: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum CorePattern {
    Wildcard,
    Binding(SymbolId),
    Literal(CoreLiteral),
    Constructor {
        symbol: SymbolId,
        fields: Vec<CorePattern>,
    },
    Outcome {
        success: bool,
        value: Box<CorePattern>,
    },
    /// Matches a list by its length, then `prefix` against the first elements and
    /// `suffix` against the last ones, each by one indexed read. Without `rest` the
    /// length must equal `prefix.len()`; with it the length must be at least
    /// `prefix.len() + suffix.len()` and `rest` matches the O(1) view of the elements
    /// between them.
    List {
        prefix: Vec<CorePattern>,
        rest: Option<Box<CorePattern>>,
        suffix: Vec<CorePattern>,
    },
    /// Matches a tuple, whose arity typing fixed, element by element.
    Tuple(Vec<CorePattern>),
    /// Matches `pattern` and also binds the whole value to `binding`.
    Alias {
        pattern: Box<CorePattern>,
        binding: SymbolId,
    },
}

pub(crate) struct LegacyVerifiedRoot {
    typed: Arc<TypedProgram>,
    entry: SymbolId,
}

pub(crate) struct VerifiedRoot {
    pub(crate) typed: Arc<TypedProject>,
    pub(crate) entry: SymbolId,
    tests: Vec<SymbolId>,
    reachable_declarations: Vec<SymbolId>,
    origins: Vec<Origin>,
}

impl VerifiedRoot {
    pub(crate) fn origin_id(&self, module_id: ModuleId, semantic_id: SemanticId) -> Option<u32> {
        self.origins
            .iter()
            .find(|origin| origin.module_id == module_id && origin.semantic_id == semantic_id)
            .map(|origin| origin.origin_id.raw())
    }

    pub(crate) fn verification_stats(&self) -> (usize, usize, usize) {
        let reachable_modules = self
            .origins
            .iter()
            .map(|origin| origin.module_id)
            .collect::<HashSet<_>>()
            .len();
        (
            self.reachable_declarations.len(),
            self.origins.len(),
            reachable_modules,
        )
    }
}

/// The needs of `main` that a runner provider written in std source closes
/// (`operations::RUNNER_PROVIDERS`), each with the private function that
/// builds its provider. Lowering wraps `main`'s body in a provision of them,
/// as if `main` were called `using` those providers.
fn runner_provisions(typed: &TypedProject, entry: SymbolId) -> Vec<(String, SymbolId)> {
    let Some(contract) = typed.contracts.get(&entry) else {
        return Vec::new();
    };
    let resolved = &typed.resolved;
    typed.needs_rows[contract.needs.raw() as usize]
        .entries
        .iter()
        .filter_map(|need| {
            let CanonicalType::Named { symbol, .. } = &typed.types[need.interface.raw() as usize]
            else {
                return None;
            };
            let Some(StandardSymbol::Type(interface)) = resolved.standard_symbol(*symbol) else {
                return None;
            };
            let provider = crate::operations::runner_provider(interface, &need.lexical_name)?;
            let (module_path, name) = provider
                .provider
                .split_once('.')
                .expect("a provider is a qualified function name");
            let function = resolved.symbols.iter().find(|symbol| {
                symbol.kind == ProjectSymbolKind::Function
                    && symbol.name == name
                    && symbol.module_id.is_some_and(|module| {
                        resolved.semantic.modules[module.raw() as usize].path == module_path
                    })
            })?;
            Some((need.lexical_name.clone(), function.id))
        })
        .collect()
}

pub(crate) fn seal_entry(
    typed: impl Into<Arc<TypedProject>>,
) -> Result<VerifiedRoot, Vec<Diagnostic>> {
    let typed = typed.into();
    if !typed.diagnostics.is_empty() {
        return Err(typed.diagnostics.clone());
    }

    let roots = typed
        .resolved
        .semantic
        .modules
        .iter()
        .flat_map(|module| {
            module
                .declarations
                .iter()
                .filter(|declaration| {
                    declaration.kind == crate::semantic::ModuleDeclarationKind::Function
                        && declaration.public
                        && declaration.name.text == "main"
                })
                .map(move |declaration| (module.id, declaration))
        })
        .collect::<Vec<_>>();
    let Some((root_module, root)) = roots.first().copied() else {
        let (file_id, range) = typed.resolved.semantic.modules.first().map_or(
            (FileId::new(0), TextRange::empty(0)),
            |module| {
                (
                    module.source.file_id(),
                    module.declarations.first().map_or_else(
                        || TextRange::empty(module.source.text().len()),
                        |declaration| declaration.name.range,
                    ),
                )
            },
        );
        return Err(vec![Diagnostic::error(
            DiagnosticStage::ExecutionSetup,
            "E5001",
            file_id,
            range,
            "program must export exactly one `pub fn main()`",
        )]);
    };
    if roots.len() != 1 {
        let module = &typed.resolved.semantic.modules[root_module.raw() as usize];
        return Err(vec![Diagnostic::error(
            DiagnosticStage::ExecutionSetup,
            "E5003",
            module.source.file_id(),
            root.name.range,
            "program exports more than one `main` function",
        )]);
    }

    let entry = typed
        .resolved
        .symbols
        .iter()
        .find(|symbol| {
            symbol.kind == crate::resolution::ProjectSymbolKind::Function
                && symbol.module_id == Some(root_module)
                && symbol.semantic_id == Some(root.semantic_id)
        })
        .map(|symbol| symbol.id)
        .expect("resolved entry function has a symbol");
    let contract = typed
        .contracts
        .get(&entry)
        .expect("typed entry function has a callable contract");
    let module = &typed.resolved.semantic.modules[root_module.raw() as usize];
    if !(contract.inputs.is_empty()
        || matches!(
            contract.inputs.as_slice(),
            [input] if is_application_arguments_type(&typed, *input)
        ))
    {
        return Err(vec![Diagnostic::error(
            DiagnosticStage::ExecutionSetup,
            "E5004",
            module.source.file_id(),
            root.name.range,
            "program entry `main` must accept no arguments or one `List<Text>`",
        )]);
    }
    if !root.generic_parameters.is_empty()
        || !typed.failure_rows[contract.failures.raw() as usize]
            .parameters
            .is_empty()
        || !typed.needs_rows[contract.needs.raw() as usize]
            .parameters
            .is_empty()
        || matches!(
            resolved_type(&typed, contract.success),
            CanonicalType::InferenceVariable(_) | CanonicalType::Generic(_) | CanonicalType::Poison
        )
    {
        return Err(vec![Diagnostic::error(
            DiagnosticStage::ExecutionSetup,
            "E5005",
            module.source.file_id(),
            root.name.range,
            "program entry `main` must have a fully concrete contract",
        )]);
    }

    let mut roots = vec![entry];
    roots.extend(
        runner_provisions(&typed, entry)
            .into_iter()
            .map(|(_, provider)| provider),
    );
    let reachable = collect_reachable_program(&typed, &roots)?;
    if let Some(found) = &reachable.test_only {
        let mut path = found
            .path
            .iter()
            .map(|symbol| typed.resolved.symbol(*symbol).name.clone())
            .collect::<Vec<_>>();
        path.push(found.operation.clone());
        return Err(vec![Diagnostic::error(
            DiagnosticStage::ExecutionSetup,
            "T4401",
            found.file_id,
            found.range,
            format!(
                "`{}` is test-only, but the program reaches it: {}; call it only from `test` items and the helpers they use",
                found.operation,
                path.join(" -> ")
            ),
        )]);
    }
    Ok(VerifiedRoot {
        typed,
        entry,
        tests: Vec::new(),
        reachable_declarations: reachable.declarations,
        origins: reachable.origins,
    })
}

/// Seals a test build: every `test` item of the project is a root, in module
/// path then declaration order, and the program's `main`, if any, is not.
///
/// Ensures: `Err` with the project's diagnostics when typing failed, and `E5009`
/// when the project declares no test.
pub(crate) fn seal_tests(
    typed: impl Into<Arc<TypedProject>>,
) -> Result<VerifiedRoot, Vec<Diagnostic>> {
    let typed = typed.into();
    if !typed.diagnostics.is_empty() {
        return Err(typed.diagnostics.clone());
    }
    let mut modules = typed.resolved.semantic.modules.iter().collect::<Vec<_>>();
    modules.sort_by(|left, right| left.path.cmp(&right.path));
    let tests = modules
        .into_iter()
        .flat_map(|module| {
            module
                .declarations
                .iter()
                .filter(|declaration| declaration.test)
                .map(move |declaration| (module.id, declaration.semantic_id))
        })
        .map(|(module_id, semantic_id)| {
            typed
                .resolved
                .symbols
                .iter()
                .find(|symbol| {
                    symbol.kind == crate::resolution::ProjectSymbolKind::Function
                        && symbol.module_id == Some(module_id)
                        && symbol.semantic_id == Some(semantic_id)
                })
                .map(|symbol| symbol.id)
                .expect("resolved test item has a symbol")
        })
        .collect::<Vec<_>>();
    let Some(&entry) = tests.first() else {
        let (file_id, length) = typed
            .resolved
            .semantic
            .modules
            .first()
            .map_or((FileId::new(0), 0), |module| {
                (module.source.file_id(), module.source.text().len())
            });
        return Err(vec![Diagnostic::error(
            DiagnosticStage::ExecutionSetup,
            "E5009",
            file_id,
            TextRange::empty(length),
            "no tests found; declare one with `test \"name\" { ... }`",
        )]);
    };
    let reachable = collect_reachable_program(&typed, &tests)?;
    Ok(VerifiedRoot {
        typed,
        entry,
        tests,
        reachable_declarations: reachable.declarations,
        origins: reachable.origins,
    })
}

fn is_application_arguments_type(typed: &TypedProject, type_id: TypeId) -> bool {
    let CanonicalType::Named { symbol, arguments } = resolved_type(typed, type_id) else {
        return false;
    };
    if typed.resolved.symbol(*symbol).name != "List" || arguments.len() != 1 {
        return false;
    }
    let CanonicalType::Named {
        symbol: value_symbol,
        arguments: value_arguments,
    } = resolved_type(typed, arguments[0])
    else {
        return false;
    };
    typed.resolved.symbol(*value_symbol).name == "Text" && value_arguments.is_empty()
}

struct ReachableProgram {
    declarations: Vec<SymbolId>,
    origins: Vec<Origin>,
    /// The first reference to a test-only operation the walk met, with the
    /// declaration chain that reached it, root first.
    test_only: Option<TestOnlyUse>,
}

struct TestOnlyUse {
    path: Vec<SymbolId>,
    operation: String,
    file_id: FileId,
    range: TextRange,
}

#[derive(Default)]
struct ReachabilityState {
    queue: VecDeque<SymbolId>,
    seen_declarations: HashSet<SymbolId>,
    origins: Vec<(ModuleId, SemanticId, TextRange)>,
    seen_origins: HashSet<(ModuleId, SemanticId)>,
    diagnostics: Vec<Diagnostic>,
    /// The declaration whose body the walk is in.
    current: Option<SymbolId>,
    /// The declaration that first queued each symbol, for `T4401`'s call path.
    callers: HashMap<SymbolId, SymbolId>,
    /// The caller, the reference text, and where it is.
    test_only: Option<(SymbolId, String, FileId, TextRange)>,
}

fn collect_reachable_program(
    typed: &TypedProject,
    roots: &[SymbolId],
) -> Result<ReachableProgram, Vec<Diagnostic>> {
    let mut references = HashMap::<(FileId, SemanticId), Vec<_>>::new();
    for reference in &typed.resolved.references {
        if reference.namespace == Namespace::Value {
            references
                .entry((reference.file_id, reference.semantic_id))
                .or_default()
                .push(reference);
        }
    }

    // Every constant is a root: the build evaluates each one, referenced or not, so a
    // broken constant fails the build wherever it is.
    let constants = typed
        .resolved
        .symbols
        .iter()
        .filter(|symbol| symbol.kind == ProjectSymbolKind::Constant)
        .map(|symbol| symbol.id);
    let mut state = ReachabilityState {
        queue: roots.iter().copied().chain(constants).collect(),
        ..ReachabilityState::default()
    };
    while let Some(symbol_id) = state.queue.pop_front() {
        // A `native fn` has no body: a call to it is an intrinsic call.
        if typed.resolved.native_operation(symbol_id).is_some()
            || !state.seen_declarations.insert(symbol_id)
        {
            continue;
        }
        let symbol = typed.resolved.symbol(symbol_id);
        let (Some(module_id), Some(semantic_id)) = (symbol.module_id, symbol.semantic_id) else {
            continue;
        };
        let module = &typed.resolved.semantic.modules[module_id.raw() as usize];
        let Some(declaration) = module
            .declarations
            .iter()
            .find(|declaration| declaration.semantic_id == semantic_id)
        else {
            state.diagnostics.push(Diagnostic::error(
                DiagnosticStage::ExecutionSetup,
                "E5006",
                module.source.file_id(),
                TextRange::empty(0),
                "reachable symbol has no semantic declaration",
            ));
            continue;
        };
        if declaration.kind == ModuleDeclarationKind::Function
            && !typed.contracts.contains_key(&symbol_id)
        {
            state.diagnostics.push(Diagnostic::error(
                DiagnosticStage::ExecutionSetup,
                "E5006",
                module.source.file_id(),
                declaration.name.range,
                format!(
                    "reachable function `{}` has no callable contract",
                    declaration.name.text
                ),
            ));
        }
        if let Some(body) = &declaration.body {
            state.current = Some(symbol_id);
            collect_reachable_expression(typed, module_id, body, &references, &mut state);
        }
    }
    if !state.diagnostics.is_empty() {
        return Err(state.diagnostics);
    }
    let test_only = state
        .test_only
        .take()
        .map(|(caller, operation, file_id, range)| {
            let mut path = vec![caller];
            while let Some(&previous) = state.callers.get(path.last().expect("non-empty")) {
                if path.contains(&previous) {
                    break;
                }
                path.push(previous);
            }
            path.reverse();
            TestOnlyUse {
                path,
                operation,
                file_id,
                range,
            }
        });

    let mut declarations = state.seen_declarations.into_iter().collect::<Vec<_>>();
    declarations.sort_unstable();
    state
        .origins
        .sort_by_key(|(module_id, semantic_id, _)| (*module_id, *semantic_id));
    let origins = state
        .origins
        .into_iter()
        .enumerate()
        .map(|(index, (module_id, semantic_id, range))| Origin {
            origin_id: OriginId::new(index as u32),
            file_id: typed.resolved.semantic.modules[module_id.raw() as usize]
                .source
                .file_id(),
            module_id,
            semantic_id,
            range,
        })
        .collect();
    Ok(ReachableProgram {
        declarations,
        origins,
        test_only,
    })
}

fn collect_reachable_expression(
    typed: &TypedProject,
    module_id: ModuleId,
    expression: &ModuleExpression,
    references: &HashMap<(FileId, SemanticId), Vec<&crate::resolution::ProjectReference>>,
    state: &mut ReachabilityState,
) {
    let module = &typed.resolved.semantic.modules[module_id.raw() as usize];
    let semantic_id = expression.semantic_id();
    let range = expression.range();
    if range.start() == range.end() || range.end() as usize > module.source.text().len() {
        state.diagnostics.push(Diagnostic::error(
            DiagnosticStage::ExecutionSetup,
            "E5008",
            module.source.file_id(),
            range,
            "reachable expression has no valid source range",
        ));
    }
    if state.seen_origins.insert((module_id, semantic_id)) {
        state.origins.push((module_id, semantic_id, range));
    }
    match typed.expression_facts.get(&(module_id, semantic_id)) {
        Some(fact) if type_is_nonpoisoned(typed, fact.success, &mut HashSet::new()) => {}
        _ => state.diagnostics.push(Diagnostic::error(
            DiagnosticStage::ExecutionSetup,
            "E5006",
            module.source.file_id(),
            range,
            "reachable expression has no complete type fact",
        )),
    }
    if let Some(expression_references) = references.get(&(module.source.file_id(), semantic_id)) {
        for reference in expression_references {
            match reference.resolution {
                ReferenceResolution::Resolved(symbol) => {
                    if matches!(
                        typed.resolved.symbol(symbol).kind,
                        ProjectSymbolKind::Function | ProjectSymbolKind::Constant
                    ) {
                        if let Some(current) = state.current {
                            if let Some(name) = typed
                                .resolved
                                .native_operation(symbol)
                                .filter(|_| state.test_only.is_none())
                                .and_then(crate::operations::test_only_name)
                            {
                                state.test_only =
                                    Some((current, name, reference.file_id, reference.range));
                            }
                            if symbol != current {
                                state.callers.entry(symbol).or_insert(current);
                            }
                        }
                        state.queue.push_back(symbol);
                    }
                }
                ReferenceResolution::DeferredToType => {}
                ReferenceResolution::Unresolved
                | ReferenceResolution::Ambiguous
                | ReferenceResolution::Inaccessible(_) => {
                    state.diagnostics.push(Diagnostic::error(
                        DiagnosticStage::ExecutionSetup,
                        "E5007",
                        module.source.file_id(),
                        reference.range,
                        "reachable reference is not executable",
                    ))
                }
            }
        }
    }

    let mut visit = |child: &ModuleExpression| {
        collect_reachable_expression(typed, module_id, child, references, state);
    };
    match expression {
        ModuleExpression::Literal(_)
        | ModuleExpression::Name { .. }
        | ModuleExpression::Empty { .. } => {}
        ModuleExpression::Select { receiver, .. } => {
            let resolved_module_member = references
                .get(&(module.source.file_id(), semantic_id))
                .is_some_and(|references| {
                    references.iter().any(|reference| {
                        matches!(reference.resolution, ReferenceResolution::Resolved(_))
                    })
                })
                && match receiver.as_ref() {
                    ModuleExpression::Name { .. } => true,
                    // `Alias.Type.Variant`, resolved as a whole to the variant.
                    ModuleExpression::Select { receiver, .. } => {
                        matches!(receiver.as_ref(), ModuleExpression::Name { .. })
                    }
                    _ => false,
                };
            let dependency_receiver =
                referenced_expression_symbol(references, module.source.file_id(), receiver)
                    .is_some_and(|symbol| {
                        typed.resolved.symbol(symbol).kind == ProjectSymbolKind::Requirement
                    });
            if !resolved_module_member && !dependency_receiver {
                visit(receiver);
            }
        }
        ModuleExpression::Group {
            expression: receiver,
            ..
        }
        | ModuleExpression::Unary {
            operand: receiver, ..
        } => visit(receiver),
        ModuleExpression::Interpolation { parts, .. } => {
            for part in crate::semantic::interpolated_expressions(parts) {
                visit(part);
            }
        }
        ModuleExpression::Call {
            callee, arguments, ..
        } => {
            let callee_symbol =
                referenced_expression_symbol(references, module.source.file_id(), callee);
            let constructor =
                callee_symbol.is_some_and(|symbol| is_core_constructor(typed, symbol));
            if !constructor {
                visit(callee);
            }
            for argument in arguments {
                visit(&argument.value);
            }
        }
        ModuleExpression::List { elements, .. }
        | ModuleExpression::Tuple { elements, .. }
        | ModuleExpression::Sequence {
            expressions: elements,
            ..
        } => {
            for element in elements {
                visit(element);
            }
        }
        ModuleExpression::Record { fields, .. } => {
            for field in fields {
                visit(&field.value);
            }
        }
        ModuleExpression::Update { base, fields, .. } => {
            visit(base);
            for field in fields {
                visit(&field.value);
            }
        }
        ModuleExpression::Binary { left, right, .. } => {
            visit(left);
            visit(right);
        }
        ModuleExpression::Using {
            expression,
            providers,
            ..
        } => {
            visit(expression);
            for provider in providers {
                visit(&provider.value);
            }
        }
        ModuleExpression::Block { items, .. } => {
            for item in items {
                match item {
                    ModuleBlockItem::Binding { value, .. } | ModuleBlockItem::Expression(value) => {
                        visit(value)
                    }
                }
            }
        }
        ModuleExpression::Function { body, .. } => visit(body),
        ModuleExpression::Loop { initial, body, .. } => {
            visit(initial);
            visit(body);
        }
        ModuleExpression::Break { value, .. } => {
            if let Some(value) = value {
                visit(value);
            }
        }
        ModuleExpression::Case { subjects, arms, .. } => {
            for subject in subjects {
                visit(subject);
            }
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    visit(guard);
                }
                visit(&arm.body);
            }
        }
    }
}

fn referenced_expression_symbol(
    references: &HashMap<(FileId, SemanticId), Vec<&crate::resolution::ProjectReference>>,
    file_id: FileId,
    expression: &ModuleExpression,
) -> Option<SymbolId> {
    match expression {
        ModuleExpression::Name { semantic_id, .. }
        | ModuleExpression::Select { semantic_id, .. } => references
            .get(&(file_id, *semantic_id))?
            .iter()
            .find_map(|reference| match reference.resolution {
                ReferenceResolution::Resolved(symbol) => Some(symbol),
                _ => None,
            }),
        ModuleExpression::Group { expression, .. } => {
            referenced_expression_symbol(references, file_id, expression)
        }
        _ => None,
    }
}

fn type_is_nonpoisoned(
    typed: &TypedProject,
    type_id: crate::ids::TypeId,
    seen: &mut HashSet<crate::ids::TypeId>,
) -> bool {
    if !seen.insert(type_id) {
        return true;
    }
    match resolved_type(typed, type_id) {
        CanonicalType::Poison => false,
        // A variable that no constraint ever reached is erased: values are
        // uniform at run time, and every type-directed operation (arithmetic,
        // field access, pattern tests) would have bound it or reported a type
        // error. Its only inhabitants are values that never mention it, such as
        // `None` or an empty collection. See executable-kernel.md "Generics".
        CanonicalType::InferenceVariable(_) => true,
        CanonicalType::Named { arguments, .. } => arguments
            .iter()
            .all(|argument| type_is_nonpoisoned(typed, *argument, seen)),
        CanonicalType::Function(contract) | CanonicalType::PolymorphicFunction { contract, .. } => {
            contract
                .inputs
                .iter()
                .all(|input| type_is_nonpoisoned(typed, *input, seen))
                && type_is_nonpoisoned(typed, contract.success, seen)
        }
        CanonicalType::Record(fields) => fields
            .iter()
            .all(|field| type_is_nonpoisoned(typed, field.type_id, seen)),
        CanonicalType::Tuple(elements) => elements
            .iter()
            .all(|element| type_is_nonpoisoned(typed, *element, seen)),
        CanonicalType::Generic(_) | CanonicalType::FailureSetValue(_) => true,
    }
}

/// Lowers the verified program to Core and evaluates every constant.
///
/// Ensures: on success, `constants` holds one value per constant initializer.
/// A constant whose evaluation breaks a build-time rule yields its `E52xx` diagnostic.
pub(crate) fn lower(root: VerifiedRoot) -> Result<CoreProgram, Vec<Diagnostic>> {
    let computation = root.typed.resolved.symbol(root.entry).name.clone();
    let mut symbols = lower_core_symbols(&root.typed);
    let origin_ids = root
        .origins
        .iter()
        .map(|origin| ((origin.module_id, origin.semantic_id), origin.origin_id))
        .collect();
    let mut lowerer = CoreLowerer {
        typed: Arc::clone(&root.typed),
        origin_ids,
        nodes: Vec::new(),
        clause_parameter_aliases: HashMap::new(),
        unbound_temporaries: HashSet::new(),
        at_build_time: false,
        embed_callee: false,
        diagnostics: Vec::new(),
        cause_scopes: Vec::new(),
        symbol_count: symbols.len(),
        synthetic_locals: Vec::new(),
    };
    let mut functions = Vec::new();
    let mut constant_initializers = Vec::new();
    for symbol_id in root.reachable_declarations {
        let symbol = lowerer.typed.resolved.symbol(symbol_id);
        let module_id = symbol
            .module_id
            .expect("reachable declaration belongs to a module");
        let semantic_id = symbol
            .semantic_id
            .expect("reachable declaration has semantic identity");
        let declaration = lowerer.typed.resolved.semantic.modules[module_id.raw() as usize]
            .declarations
            .iter()
            .find(|declaration| declaration.semantic_id == semantic_id)
            .expect("verified reachable symbol owns a declaration");
        let kind = declaration.kind;
        let parameters = declaration
            .function
            .as_ref()
            .map(|signature| {
                signature
                    .parameters
                    .iter()
                    .map(|parameter| {
                        lowerer
                            .local_symbol(
                                module_id,
                                parameter.semantic_id,
                                ProjectSymbolKind::Parameter,
                            )
                            .expect("verified function parameter has a symbol")
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let body = declaration
            .body
            .as_ref()
            .expect("verified executable declaration owns a body")
            .clone();
        lowerer.at_build_time = kind == ModuleDeclarationKind::Constant;
        let mut body = lowerer.expression(module_id, &body);
        lowerer.at_build_time = false;
        if symbol_id == root.entry {
            body = lowerer.provide_runner_capabilities(body, root.entry);
        }
        match kind {
            ModuleDeclarationKind::Function => {
                functions.push(CoreFunction {
                    symbol: symbol_id,
                    parameters,
                    body,
                });
            }
            ModuleDeclarationKind::Constant => {
                constant_initializers.push(CoreConstantInitializer {
                    symbol: symbol_id,
                    value: body,
                })
            }
            ModuleDeclarationKind::Struct
            | ModuleDeclarationKind::Enum
            | ModuleDeclarationKind::Interface
            | ModuleDeclarationKind::NativeType
            | ModuleDeclarationKind::ProcessName => {
                unreachable!("only executable declarations enter reachability")
            }
        }
    }
    // A name aliased to its clause parameter, an update temporary left unbound,
    // or a local of an unreachable function binds nothing, so it takes no
    // environment slot: a frame holds only the locals reachable code binds.
    symbols.extend(lowerer.synthetic_locals.iter().map(|&id| CoreSymbol {
        id,
        name: "cause".to_owned(),
        kind: CoreSymbolKind::Local,
        constructor: None,
        standard: None,
    }));
    let bound = bound_locals(&functions, &lowerer.nodes);
    for symbol in &mut symbols {
        if lowerer.clause_parameter_aliases.contains_key(&symbol.id)
            || lowerer.unbound_temporaries.contains(&symbol.id)
            || (symbol.kind == CoreSymbolKind::Local && !bound.contains(&symbol.id))
        {
            symbol.kind = CoreSymbolKind::Other;
        }
    }
    let lowerer_diagnostics = std::mem::take(&mut lowerer.diagnostics);
    let core = CoreProgram {
        computation,
        entry: root.entry,
        tests: root.tests,
        type_count: root.typed.types.len(),
        symbols,
        functions,
        constant_initializers,
        constants: Vec::new(),
        nodes: lowerer.nodes,
        origin_locations: origin_locations(&root.typed, &root.origins),
        origins: root.origins,
    };
    if !lowerer_diagnostics.is_empty() {
        return Err(lowerer_diagnostics);
    }
    let mut core = core;
    core.constants = crate::execution::evaluate_constants(&core)?;
    core.assert_well_formed();
    if cfg!(debug_assertions)
        && let Err(violation) = verify_core(&core, &root.typed)
    {
        panic!("Core verifier: {violation}");
    }
    Ok(core)
}

struct CoreLowerer {
    typed: Arc<TypedProject>,
    origin_ids: HashMap<(ModuleId, SemanticId), OriginId>,
    nodes: Vec<CoreNode>,
    /// A clause pattern that only names its parameter (`fn f(state, "a")`) binds
    /// nothing in Core: references to that name read the generated parameter.
    clause_parameter_aliases: HashMap<SymbolId, SymbolId>,
    /// Update temporaries the lowering did not need to bind.
    unbound_temporaries: HashSet<SymbolId>,
    /// Whether the expression being lowered runs at build time: it is part of a
    /// constant initializer and not inside a function value, which could run later.
    at_build_time: bool,
    /// Whether the reference being lowered is the callee of an `Embed.text` or
    /// `Embed.bytes` call, the only place either may be named.
    embed_callee: bool,
    /// `std/embed` misuse and unreadable files (`E5206`–`E5209`).
    diagnostics: Vec<Diagnostic>,
    /// One entry per enclosing case arm: its cause scope when it matched a live
    /// failure or a stored `Err`, `None` otherwise and at each function value,
    /// whose body is not in the arm's body.
    cause_scopes: Vec<Option<CauseScope>>,
    /// The typed project's symbol count; generated locals are numbered after it.
    symbol_count: usize,
    /// The locals generated for cause arms, in numbering order.
    synthetic_locals: Vec<SymbolId>,
}

struct CauseScope {
    /// Generated on the first construction that reads the cause.
    local: Option<SymbolId>,
    /// The variables the arm binds to the matched failure value itself.
    carriers: Vec<SymbolId>,
}

impl CoreLowerer {
    fn expression(&mut self, module_id: ModuleId, expression: &ModuleExpression) -> CoreNodeId {
        let operation = match expression {
            ModuleExpression::Literal(literal) => CoreOperation::Constant(match literal {
                Literal::Integer { value, .. } => CoreLiteral::Integer(*value),
                Literal::Float { value, .. } => CoreLiteral::Float(*value),
                Literal::Text { value, .. } => CoreLiteral::Text(value.clone()),
                Literal::Boolean { value, .. } => CoreLiteral::Boolean(*value),
                Literal::Void { .. } => CoreLiteral::Void,
            }),
            ModuleExpression::Name { semantic_id, .. } => {
                let symbol = self
                    .value_symbol(module_id, *semantic_id)
                    .expect("verified value reference resolves");
                self.value_reference(module_id, expression, symbol)
            }
            ModuleExpression::Select {
                semantic_id,
                receiver,
                member,
                ..
            } => {
                if let Some(symbol) = self.value_symbol(module_id, *semantic_id) {
                    self.value_reference(module_id, expression, symbol)
                } else if let Some((requirement, interface, method)) =
                    self.dependency_method(module_id, receiver, &member.text)
                {
                    CoreOperation::DependencyMethod {
                        requirement,
                        interface,
                        method,
                        name: member.text.clone(),
                    }
                } else {
                    let receiver = self.expression(module_id, receiver);
                    CoreOperation::Select {
                        receiver,
                        member: member.text.clone(),
                    }
                }
            }
            ModuleExpression::Call {
                callee, arguments, ..
            } => {
                let callee_symbol = self.expression_symbol(module_id, callee);
                let embedded = match callee_symbol
                    .and_then(|symbol| self.typed.resolved.native_operation(symbol))
                {
                    Some(
                        function @ (StandardFunction::EmbedText | StandardFunction::EmbedBytes),
                    ) => Some(self.embed(module_id, expression, function, arguments)),
                    _ => None,
                };
                let arguments = match embedded {
                    Some(argument) => vec![argument],
                    None => arguments
                        .iter()
                        .map(|argument| self.expression(module_id, &argument.value))
                        .collect::<Vec<_>>(),
                };
                if let Some(constructor) =
                    callee_symbol.filter(|symbol| is_core_constructor(&self.typed, *symbol))
                {
                    let cause = self.construct_cause(constructor, &arguments);
                    CoreOperation::Construct {
                        constructor,
                        arguments,
                        cause,
                    }
                } else {
                    self.embed_callee = embedded.is_some();
                    let callee = self.expression(module_id, callee);
                    self.embed_callee = false;
                    CoreOperation::Call { callee, arguments }
                }
            }
            ModuleExpression::List { elements, .. } => CoreOperation::List(
                elements
                    .iter()
                    .map(|element| self.expression(module_id, element))
                    .collect(),
            ),
            ModuleExpression::Tuple { elements, .. } => CoreOperation::Tuple(
                elements
                    .iter()
                    .map(|element| self.expression(module_id, element))
                    .collect(),
            ),
            ModuleExpression::Interpolation { parts, .. } => {
                self.interpolation(module_id, expression, parts)
            }
            ModuleExpression::Update {
                semantic_id,
                base,
                fields,
                ..
            } => self.update(module_id, *semantic_id, base, fields),
            ModuleExpression::Record { fields, .. } => CoreOperation::Record(
                fields
                    .iter()
                    .map(|field| CoreField {
                        name: field.name.text.clone(),
                        value: self.expression(module_id, &field.value),
                    })
                    .collect(),
            ),
            ModuleExpression::Unary {
                operator: UnaryOperator::Not,
                operand,
                ..
            } => {
                let subject = self.expression(module_id, operand);
                let when_true = self.boolean_constant(module_id, expression, false);
                let when_false = self.boolean_constant(module_id, expression, true);
                boolean_match(subject, when_true, when_false)
            }
            ModuleExpression::Binary {
                operator: BinaryOperator::And,
                left,
                right,
                ..
            } => {
                let subject = self.expression(module_id, left);
                let when_true = self.expression(module_id, right);
                let when_false = self.boolean_constant(module_id, expression, false);
                boolean_match(subject, when_true, when_false)
            }
            ModuleExpression::Binary {
                operator: BinaryOperator::Or,
                left,
                right,
                ..
            } => {
                let subject = self.expression(module_id, left);
                let when_true = self.boolean_constant(module_id, expression, true);
                let when_false = self.expression(module_id, right);
                boolean_match(subject, when_true, when_false)
            }
            ModuleExpression::Binary {
                operator: BinaryOperator::NotEqual,
                left,
                right,
                ..
            } => {
                let equal = CoreOperation::Binary {
                    operator: BinaryOperator::Equal,
                    left: self.expression(module_id, left),
                    right: self.expression(module_id, right),
                };
                let subject = self.push_node(module_id, expression, equal);
                let when_true = self.boolean_constant(module_id, expression, false);
                let when_false = self.boolean_constant(module_id, expression, true);
                boolean_match(subject, when_true, when_false)
            }
            ModuleExpression::Unary {
                operator, operand, ..
            } => CoreOperation::Unary {
                operator: *operator,
                operand: self.expression(module_id, operand),
            },
            ModuleExpression::Binary {
                operator,
                left,
                right,
                ..
            } => CoreOperation::Binary {
                operator: *operator,
                left: self.expression(module_id, left),
                right: self.expression(module_id, right),
            },
            ModuleExpression::Group { expression, .. } => {
                CoreOperation::Identity(self.expression(module_id, expression))
            }
            ModuleExpression::Using {
                expression,
                providers,
                ..
            } => CoreOperation::Provision {
                expression: self.expression(module_id, expression),
                providers: providers
                    .iter()
                    .map(|provider| CoreField {
                        name: provider.name.text.clone(),
                        value: self.expression(module_id, &provider.value),
                    })
                    .collect(),
            },
            ModuleExpression::Sequence { expressions, .. } => CoreOperation::Sequence(
                expressions
                    .iter()
                    .map(|expression| CoreStep::Expression(self.expression(module_id, expression)))
                    .collect(),
            ),
            ModuleExpression::Block { items, .. } => CoreOperation::Sequence(
                items
                    .iter()
                    .map(|item| match item {
                        ModuleBlockItem::Binding { value, .. } => CoreStep::Bind {
                            symbol: self
                                .local_symbol(
                                    module_id,
                                    value.semantic_id(),
                                    ProjectSymbolKind::LocalBinding,
                                )
                                .expect("verified local binding has a symbol"),
                            value: self.expression(module_id, value),
                        },
                        ModuleBlockItem::Expression(expression) => {
                            CoreStep::Expression(self.expression(module_id, expression))
                        }
                    })
                    .collect(),
            ),
            ModuleExpression::Function {
                signature, body, ..
            } => {
                let parameters = signature
                    .parameters
                    .iter()
                    .map(|parameter| {
                        self.local_symbol(
                            module_id,
                            parameter.semantic_id,
                            ProjectSymbolKind::Parameter,
                        )
                        .expect("verified closure parameter has a symbol")
                    })
                    .collect();
                // Lowering the body registers its clause parameter aliases, which
                // decide the symbols that its names capture.
                let at_build_time = std::mem::replace(&mut self.at_build_time, false);
                self.cause_scopes.push(None);
                let lowered_body = self.expression(module_id, body);
                self.cause_scopes.pop();
                self.at_build_time = at_build_time;
                CoreOperation::Closure {
                    parameters,
                    captures: self.closure_captures(module_id, body),
                    body: lowered_body,
                }
            }
            ModuleExpression::Case {
                subjects,
                arms,
                origin,
                ..
            } => self.lower_case(
                module_id,
                subjects,
                arms,
                matches!(origin, CaseOrigin::Clauses { .. }),
            ),
            ModuleExpression::Loop {
                state_id,
                initial,
                body,
                ..
            } => self.lower_loop(module_id, *state_id, initial, body),
            ModuleExpression::Break {
                semantic_id, value, ..
            } => self.lower_break(module_id, *semantic_id, value.as_deref()),
            ModuleExpression::Empty { .. } => {
                unreachable!("diagnostic-free verified programs contain no empty expressions")
            }
        };
        self.push_node(module_id, expression, operation)
    }

    /// Pushes a `Bool` constant that carries the origin of the `and`, `or`, or `not`
    /// expression it was lowered from, so traces attribute it to that source node.
    fn boolean_constant(
        &mut self,
        module_id: ModuleId,
        operator_expression: &ModuleExpression,
        value: bool,
    ) -> CoreNodeId {
        self.push_node(
            module_id,
            operator_expression,
            CoreOperation::Constant(CoreLiteral::Boolean(value)),
        )
    }

    fn pattern(&self, module_id: ModuleId, pattern: &Pattern) -> CorePattern {
        match pattern {
            Pattern::Wildcard { .. }
            | Pattern::Literal {
                literal: PatternLiteral::Void,
                ..
            } => CorePattern::Wildcard,
            Pattern::Literal { literal, .. } => CorePattern::Literal(match literal {
                PatternLiteral::Integer(value) => CoreLiteral::Integer(*value),
                PatternLiteral::Text(value) => CoreLiteral::Text(value.clone()),
                PatternLiteral::Boolean(value) => CoreLiteral::Boolean(*value),
                PatternLiteral::Float => unreachable!("typing rejects Float literal patterns"),
                PatternLiteral::Void => unreachable!("a Void pattern lowers to a wildcard"),
            }),
            Pattern::Binding { semantic_id, .. }
            | Pattern::Typed {
                semantic_id,
                name: Some(_),
                ..
            } => CorePattern::Binding(
                self.local_symbol(module_id, *semantic_id, ProjectSymbolKind::PatternBinding)
                    .expect("verified pattern binding resolves"),
            ),
            Pattern::Typed { name: None, .. } => CorePattern::Wildcard,
            Pattern::Alias {
                semantic_id,
                pattern,
                ..
            } => CorePattern::Alias {
                pattern: Box::new(self.pattern(module_id, pattern)),
                binding: self
                    .local_symbol(module_id, *semantic_id, ProjectSymbolKind::PatternBinding)
                    .expect("verified alias binding resolves"),
            },
            Pattern::Tuple { elements, .. } => {
                CorePattern::Tuple(self.patterns(module_id, elements))
            }
            Pattern::List {
                prefix,
                rest,
                suffix,
                ..
            } => CorePattern::List {
                prefix: self.patterns(module_id, prefix),
                rest: rest
                    .as_deref()
                    .map(|rest| Box::new(self.pattern(module_id, rest))),
                suffix: self.patterns(module_id, suffix),
            },
            Pattern::Constructor {
                semantic_id,
                arguments,
                ..
            } => {
                let symbol = self
                    .value_symbol(module_id, *semantic_id)
                    .expect("verified constructor pattern resolves");
                let resolved = self.typed.resolved.symbol(symbol);
                if resolved.module_id.is_none()
                    && resolved.kind == ProjectSymbolKind::BuiltinValue
                    && matches!(resolved.name.as_str(), "Success" | "Failure")
                {
                    let [value] = arguments.as_slice() else {
                        unreachable!("verified outcome patterns have one argument")
                    };
                    CorePattern::Outcome {
                        success: resolved.name == "Success",
                        value: Box::new(self.pattern(module_id, value)),
                    }
                } else {
                    CorePattern::Constructor {
                        symbol,
                        fields: self.patterns(module_id, arguments),
                    }
                }
            }
        }
    }

    /// The Core pattern rows one source arm becomes. A failure-type pattern covers
    /// several variants, so its arm becomes one arm per variant, tried in order and
    /// sharing the source arm's guard and body nodes. Every other arm is one row.
    /// Lowers a `case`. For a clause group's case (`clauses`), whose subjects are
    /// the generated parameters, a pattern that only binds a parameter becomes a
    /// wildcard and the name an alias of the parameter, and a subject every arm
    /// then matches with a wildcard is dropped. A clause group therefore lowers to
    /// the same Match as the `case` written over the parameters it inspects: an
    /// arm binds, and the runtime clones the environment, only for names its
    /// patterns take apart.
    fn lower_case(
        &mut self,
        module_id: ModuleId,
        subjects: &[ModuleExpression],
        arms: &[ModuleCaseArm],
        clauses: bool,
    ) -> CoreOperation {
        let parameters = subjects
            .iter()
            .map(|subject| match subject {
                ModuleExpression::Name { semantic_id, .. } if clauses => {
                    self.value_symbol(module_id, *semantic_id)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let mut lowered = Vec::new();
        for arm in arms {
            let mut alternatives = self.arm_alternatives(module_id, &arm.patterns);
            for row in &mut alternatives {
                for (pattern, parameter) in row.iter_mut().zip(&parameters) {
                    if let (CorePattern::Binding(binding), Some(parameter)) = (&*pattern, parameter)
                    {
                        self.clause_parameter_aliases.insert(*binding, *parameter);
                        *pattern = CorePattern::Wildcard;
                    }
                }
            }
            let guard = arm
                .guard
                .as_ref()
                .map(|guard| self.expression(module_id, guard));
            // An arm that matches no failure leaves the enclosing cause arm's
            // scope in place, so a `Failure(...)` in a nested `case` still links.
            let source = self.cause_source(&alternatives);
            if let Some((_, carriers)) = &source {
                self.cause_scopes.push(Some(CauseScope {
                    local: None,
                    carriers: carriers.clone(),
                }));
            }
            let body = self.expression(module_id, &arm.body);
            let scope = source
                .is_some()
                .then(|| self.cause_scopes.pop().flatten())
                .flatten();
            let cause = source
                .zip(scope.and_then(|scope| scope.local))
                .map(|((subject, _), local)| ArmCause { subject, local });
            lowered.extend(alternatives.into_iter().map(|patterns| CoreArm {
                patterns,
                guard,
                body,
                cause,
            }));
        }
        let mut kept = (0..subjects.len())
            .map(|column| {
                !clauses
                    || lowered
                        .iter()
                        .any(|arm| !matches!(arm.patterns[column], CorePattern::Wildcard))
            })
            .collect::<Vec<_>>();
        // A Match keeps one subject, so a group of catch-all clauses stays a Match.
        if !kept.contains(&true)
            && let Some(first) = kept.first_mut()
        {
            *first = true;
        }
        for arm in &mut lowered {
            let mut column = 0;
            arm.patterns.retain(|_| {
                column += 1;
                kept[column - 1]
            });
            if let Some(cause) = &mut arm.cause {
                debug_assert!(kept[cause.subject], "a cause subject is never a wildcard");
                cause.subject = kept[..cause.subject].iter().filter(|kept| **kept).count();
            }
        }
        CoreOperation::Match {
            subjects: subjects
                .iter()
                .zip(&kept)
                .filter(|(_, kept)| **kept)
                .map(|(subject, _)| self.expression(module_id, subject))
                .collect(),
            arms: lowered,
        }
    }

    /// The subject whose cause an arm's constructions read, and the variables
    /// bound to that subject's failure value itself: the first subject that every
    /// row matches with a live `Failure(...)` pattern or a `Result.Err(...)` pattern.
    fn cause_source(&self, rows: &[Vec<CorePattern>]) -> Option<(usize, Vec<SymbolId>)> {
        let first = rows.first()?;
        (0..first.len()).find_map(|column| {
            let mut carriers = Vec::new();
            for row in rows {
                let value: &CorePattern = match &row[column] {
                    CorePattern::Outcome {
                        success: false,
                        value,
                    } => value,
                    CorePattern::Constructor { symbol, fields }
                        if fields.len() == 1 && self.is_result_err(*symbol) =>
                    {
                        &fields[0]
                    }
                    _ => return None,
                };
                match value {
                    CorePattern::Binding(binding) | CorePattern::Alias { binding, .. } => {
                        carriers.push(*binding);
                    }
                    _ => {}
                }
            }
            Some((column, carriers))
        })
    }

    fn is_result_err(&self, symbol: SymbolId) -> bool {
        let resolved = self.typed.resolved.symbol(symbol);
        resolved.name == "Err"
            && resolved.kind == ProjectSymbolKind::EnumConstructor
            && resolved.module_id.is_some_and(|module_id| {
                self.typed.resolved.semantic.modules[module_id.raw() as usize].path == "std/result"
            })
    }

    /// The cause a `Failure(argument)` or `Result.Err(argument)` construction
    /// takes in the innermost enclosing cause arm, if it is lexically inside one
    /// and not inside a function value written there.
    fn construct_cause(
        &mut self,
        constructor: SymbolId,
        arguments: &[CoreNodeId],
    ) -> Option<ConstructCause> {
        let resolved = self.typed.resolved.symbol(constructor);
        let is_failure = resolved.module_id.is_none()
            && resolved.kind == ProjectSymbolKind::BuiltinValue
            && resolved.name == "Failure";
        let [argument] = arguments else {
            return None;
        };
        if !(is_failure || self.is_result_err(constructor)) {
            return None;
        }
        let argument = match &self.nodes[argument.raw() as usize].operation {
            CoreOperation::Reference(symbol) => Some(*symbol),
            _ => None,
        };
        let next = SymbolId::new(
            u32::try_from(self.symbol_count + self.synthetic_locals.len())
                .expect("symbol count fits u32"),
        );
        let scope = self.cause_scopes.last_mut()?.as_mut()?;
        let keep = argument.is_some_and(|symbol| scope.carriers.contains(&symbol));
        let local = match scope.local {
            Some(local) => local,
            None => {
                scope.local = Some(next);
                self.synthetic_locals.push(next);
                next
            }
        };
        Some(ConstructCause { local, keep })
    }

    fn arm_alternatives(&self, module_id: ModuleId, patterns: &[Pattern]) -> Vec<Vec<CorePattern>> {
        let mut rows = vec![Vec::new()];
        for pattern in patterns {
            let alternatives = self.pattern_alternatives(module_id, pattern);
            rows = rows
                .into_iter()
                .flat_map(|row| {
                    alternatives.iter().map(move |alternative| {
                        let mut row = row.clone();
                        row.push(alternative.clone());
                        row
                    })
                })
                .collect();
        }
        rows
    }

    /// The alternatives `pattern` expands to: a variant test per variant of a
    /// failure-type pattern, reached directly, through `as`, or inside an outcome
    /// pattern; otherwise the pattern itself.
    fn pattern_alternatives(&self, module_id: ModuleId, pattern: &Pattern) -> Vec<CorePattern> {
        match pattern {
            Pattern::Typed {
                semantic_id, name, ..
            } => match self
                .typed
                .failure_type_patterns
                .get(&(module_id, *semantic_id))
            {
                Some(covered) => covered
                    .iter()
                    .map(|(variant, fields)| {
                        let test = CorePattern::Constructor {
                            symbol: *variant,
                            fields: vec![CorePattern::Wildcard; *fields],
                        };
                        match name {
                            Some(_) => CorePattern::Alias {
                                pattern: Box::new(test),
                                binding: self
                                    .local_symbol(
                                        module_id,
                                        *semantic_id,
                                        ProjectSymbolKind::PatternBinding,
                                    )
                                    .expect("verified typed binding resolves"),
                            },
                            None => test,
                        }
                    })
                    .collect(),
                None => vec![self.pattern(module_id, pattern)],
            },
            Pattern::Alias {
                semantic_id,
                pattern: inner,
                ..
            } => {
                let binding = self
                    .local_symbol(module_id, *semantic_id, ProjectSymbolKind::PatternBinding)
                    .expect("verified alias binding resolves");
                self.pattern_alternatives(module_id, inner)
                    .into_iter()
                    .map(|alternative| CorePattern::Alias {
                        pattern: Box::new(alternative),
                        binding,
                    })
                    .collect()
            }
            Pattern::Constructor { arguments, .. } => match self.pattern(module_id, pattern) {
                CorePattern::Outcome { success, .. } => {
                    let [value] = arguments.as_slice() else {
                        unreachable!("verified outcome patterns have one argument")
                    };
                    self.pattern_alternatives(module_id, value)
                        .into_iter()
                        .map(|value| CorePattern::Outcome {
                            success,
                            value: Box::new(value),
                        })
                        .collect()
                }
                lowered => vec![lowered],
            },
            _ => vec![self.pattern(module_id, pattern)],
        }
    }

    fn patterns(&self, module_id: ModuleId, patterns: &[Pattern]) -> Vec<CorePattern> {
        patterns
            .iter()
            .map(|pattern| self.pattern(module_id, pattern))
            .collect()
    }

    /// Lowers `{ ..base, f: v }` to a sequence that binds the base, then each value in
    /// source order, and ends with a construction of the base's type whose fields
    /// read either the bound value or the base's field.
    fn update(
        &mut self,
        module_id: ModuleId,
        semantic_id: SemanticId,
        base: &ModuleExpression,
        fields: &[crate::semantic::RecordValueField],
    ) -> CoreOperation {
        let shape = self.typed.record_updates[&(module_id, semantic_id)].clone();
        let temporary = |lowerer: &Self, origin: SemanticId| {
            lowerer
                .typed
                .resolved
                .source_symbol(
                    module_id,
                    origin,
                    ProjectSymbolKind::PatternBinding,
                    crate::resolution::UPDATE_TEMPORARY,
                )
                .expect("resolution defines update temporaries")
        };
        let base_type = self.typed.expression_facts[&(module_id, base.semantic_id())].success;
        // A local base is read where each kept field needs it, as the written-out
        // constructor would; any other base is evaluated once, first, and bound.
        let local_base = match base {
            ModuleExpression::Name { semantic_id, .. } => self
                .value_symbol(module_id, *semantic_id)
                .filter(|symbol| self.is_local(*symbol)),
            _ => None,
        };
        let mut steps = Vec::new();
        let base_symbol = match local_base {
            Some(symbol) => {
                self.unbound_temporaries
                    .insert(temporary(self, semantic_id));
                symbol
            }
            None => {
                let symbol = temporary(self, semantic_id);
                steps.push(CoreStep::Bind {
                    symbol,
                    value: self.expression(module_id, base),
                });
                symbol
            }
        };
        // The constructor evaluates its arguments in declaration order. When the
        // new values are written in that order they are its arguments directly;
        // otherwise each is bound in source order first.
        let replacements = shape
            .fields
            .iter()
            .filter_map(|(_, _, replacement)| *replacement)
            .collect::<Vec<_>>();
        let in_order = replacements.windows(2).all(|pair| pair[0] < pair[1]);
        let mut values = Vec::new();
        for field in fields {
            let symbol = temporary(self, field.value.semantic_id());
            if in_order {
                self.unbound_temporaries.insert(symbol);
                values.push(None);
            } else {
                steps.push(CoreStep::Bind {
                    symbol,
                    value: self.expression(module_id, &field.value),
                });
                values.push(Some(symbol));
            }
        }
        let mut arguments = Vec::new();
        for (name, type_id, replacement) in &shape.fields {
            let value = match replacement {
                Some(index) => match values[*index] {
                    Some(symbol) => self.push_typed_node(
                        module_id,
                        semantic_id,
                        *type_id,
                        CoreOperation::Reference(symbol),
                    ),
                    None => self.expression(module_id, &fields[*index].value),
                },
                None => {
                    let receiver = self.push_typed_node(
                        module_id,
                        semantic_id,
                        base_type,
                        CoreOperation::Reference(base_symbol),
                    );
                    self.push_typed_node(
                        module_id,
                        semantic_id,
                        *type_id,
                        CoreOperation::Select {
                            receiver,
                            member: name.clone(),
                        },
                    )
                }
            };
            arguments.push((name.clone(), value));
        }
        let construction = match shape.constructor {
            Some(constructor) => CoreOperation::Construct {
                constructor,
                arguments: arguments.into_iter().map(|(_, value)| value).collect(),
                cause: None,
            },
            None => CoreOperation::Record(
                arguments
                    .into_iter()
                    .map(|(name, value)| CoreField { name, value })
                    .collect(),
            ),
        };
        if steps.is_empty() {
            return construction;
        }
        let result_type = self.typed.expression_facts[&(module_id, semantic_id)].success;
        let construction = self.push_typed_node(module_id, semantic_id, result_type, construction);
        steps.push(CoreStep::Expression(construction));
        CoreOperation::Sequence(steps)
    }

    fn is_local(&self, symbol: SymbolId) -> bool {
        matches!(
            self.typed.resolved.symbol(symbol).kind,
            ProjectSymbolKind::Parameter
                | ProjectSymbolKind::LocalBinding
                | ProjectSymbolKind::PatternBinding
        )
    }

    /// A node with the origin of `semantic_id` but its own type, for the parts of a
    /// lowering that have no source expression of their own.
    fn push_typed_node(
        &mut self,
        module_id: ModuleId,
        semantic_id: SemanticId,
        type_id: TypeId,
        operation: CoreOperation,
    ) -> CoreNodeId {
        let id = CoreNodeId::new(self.nodes.len() as u32);
        self.nodes.push(CoreNode {
            id,
            origin: self.origin_ids[&(module_id, semantic_id)],
            type_id,
            operation,
        });
        id
    }

    /// `body` evaluated with each runner-closed need of `entry` bound to its
    /// std provider, as if `main` were called `using` them. The synthesized
    /// nodes take the body's origin, so a trace places them at `main`.
    fn provide_runner_capabilities(&mut self, body: CoreNodeId, entry: SymbolId) -> CoreNodeId {
        let provisions = runner_provisions(&self.typed, entry);
        if provisions.is_empty() {
            return body;
        }
        let origin = self.nodes[body.raw() as usize].origin;
        let body_type = self.nodes[body.raw() as usize].type_id;
        let mut providers = Vec::new();
        for (need, provider) in provisions {
            let symbol = self.typed.resolved.symbol(provider);
            let function_type = self.typed.declaration_types[&(
                symbol.module_id.expect("a provider is a module function"),
                symbol.semantic_id.expect("a provider has semantic identity"),
            )];
            let provider_type = self.typed.contracts[&provider].success;
            let callee = self.synthetic_node(origin, function_type, CoreOperation::Reference(provider));
            let call = self.synthetic_node(
                origin,
                provider_type,
                CoreOperation::Call {
                    callee,
                    arguments: Vec::new(),
                },
            );
            providers.push(CoreField {
                name: need,
                value: call,
            });
        }
        self.synthetic_node(
            origin,
            body_type,
            CoreOperation::Provision {
                expression: body,
                providers,
            },
        )
    }

    fn synthetic_node(&mut self, origin: OriginId, type_id: TypeId, operation: CoreOperation) -> CoreNodeId {
        let id = CoreNodeId::new(self.nodes.len() as u32);
        self.nodes.push(CoreNode {
            id,
            origin,
            type_id,
            operation,
        });
        id
    }

    fn push_node(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        operation: CoreOperation,
    ) -> CoreNodeId {
        let id = CoreNodeId::new(self.nodes.len() as u32);
        let semantic_id = expression.semantic_id();
        self.nodes.push(CoreNode {
            id,
            origin: self.origin_ids[&(module_id, semantic_id)],
            type_id: self.typed.expression_facts[&(module_id, semantic_id)].success,
            operation,
        });
        id
    }

    fn expression_symbol(
        &self,
        module_id: ModuleId,
        expression: &ModuleExpression,
    ) -> Option<SymbolId> {
        match expression {
            ModuleExpression::Name { semantic_id, .. }
            | ModuleExpression::Select { semantic_id, .. } => {
                self.value_symbol(module_id, *semantic_id)
            }
            ModuleExpression::Group { expression, .. } => {
                self.expression_symbol(module_id, expression)
            }
            _ => None,
        }
    }

    fn dependency_method(
        &self,
        module_id: ModuleId,
        receiver: &ModuleExpression,
        member: &str,
    ) -> Option<(SymbolId, SymbolId, SemanticId)> {
        let requirement = self.expression_symbol(module_id, receiver)?;
        if self.typed.resolved.symbol(requirement).kind != ProjectSymbolKind::Requirement {
            return None;
        }
        let receiver_type = self
            .typed
            .expression_facts
            .get(&(module_id, receiver.semantic_id()))?
            .success;
        let CanonicalType::Named {
            symbol: interface, ..
        } = resolved_type(&self.typed, receiver_type)
        else {
            return None;
        };
        let method = self
            .typed
            .fields
            .get(interface)?
            .iter()
            .find(|field| field.name == member)?
            .semantic_id;
        Some((requirement, *interface, method))
    }

    /// Requires: `loop_id` defines the loop's state binding in resolution.
    fn lower_loop(
        &mut self,
        module_id: ModuleId,
        loop_id: SemanticId,
        initial: &ModuleExpression,
        body: &ModuleExpression,
    ) -> CoreOperation {
        CoreOperation::Loop {
            state: self.loop_state(module_id, loop_id),
            initial: self.expression(module_id, initial),
            body: self.expression(module_id, body),
        }
    }

    /// Requires: resolution linked `break_id` to an enclosing loop in the same function.
    fn lower_break(
        &mut self,
        module_id: ModuleId,
        break_id: SemanticId,
        value: Option<&ModuleExpression>,
    ) -> CoreOperation {
        let loop_id = self
            .typed
            .resolved
            .break_target(module_id, break_id)
            .expect("verified break has a target loop");
        CoreOperation::Break {
            target: self.loop_state(module_id, loop_id),
            value: value.map(|value| self.expression(module_id, value)),
        }
    }

    fn loop_state(&self, module_id: ModuleId, loop_id: SemanticId) -> SymbolId {
        self.local_symbol(module_id, loop_id, ProjectSymbolKind::LocalBinding)
            .expect("verified loop defines its state binding")
    }

    fn closure_captures(
        &self,
        module_id: ModuleId,
        expression: &ModuleExpression,
    ) -> Vec<SymbolId> {
        fn collect(
            lowerer: &CoreLowerer,
            module_id: ModuleId,
            expression: &ModuleExpression,
            captures: &mut HashSet<SymbolId>,
        ) {
            if let ModuleExpression::Name { semantic_id, .. } = expression
                && let Some(symbol) = lowerer.value_symbol(module_id, *semantic_id)
                && matches!(
                    lowerer.typed.resolved.symbol(symbol).kind,
                    ProjectSymbolKind::Parameter
                        | ProjectSymbolKind::LocalBinding
                        | ProjectSymbolKind::PatternBinding
                )
            {
                captures.insert(symbol);
            }
            match expression {
                ModuleExpression::Literal(_)
                | ModuleExpression::Name { .. }
                | ModuleExpression::Empty { .. } => {}
                ModuleExpression::Select { receiver, .. }
                | ModuleExpression::Unary {
                    operand: receiver, ..
                }
                | ModuleExpression::Group {
                    expression: receiver,
                    ..
                } => collect(lowerer, module_id, receiver, captures),
                ModuleExpression::Call {
                    callee, arguments, ..
                } => {
                    collect(lowerer, module_id, callee, captures);
                    for argument in arguments {
                        collect(lowerer, module_id, &argument.value, captures);
                    }
                }
                ModuleExpression::List { elements, .. }
                | ModuleExpression::Tuple { elements, .. }
                | ModuleExpression::Sequence {
                    expressions: elements,
                    ..
                } => {
                    for element in elements {
                        collect(lowerer, module_id, element, captures);
                    }
                }
                ModuleExpression::Interpolation { parts, .. } => {
                    for part in crate::semantic::interpolated_expressions(parts) {
                        collect(lowerer, module_id, part, captures);
                    }
                }
                ModuleExpression::Record { fields, .. } => {
                    for field in fields {
                        collect(lowerer, module_id, &field.value, captures);
                    }
                }
                ModuleExpression::Update { base, fields, .. } => {
                    collect(lowerer, module_id, base, captures);
                    for field in fields {
                        collect(lowerer, module_id, &field.value, captures);
                    }
                }
                ModuleExpression::Binary { left, right, .. } => {
                    collect(lowerer, module_id, left, captures);
                    collect(lowerer, module_id, right, captures);
                }
                ModuleExpression::Using {
                    expression,
                    providers,
                    ..
                } => {
                    collect(lowerer, module_id, expression, captures);
                    for provider in providers {
                        collect(lowerer, module_id, &provider.value, captures);
                    }
                }
                ModuleExpression::Block { items, .. } => {
                    for item in items {
                        let value = match item {
                            ModuleBlockItem::Binding { value, .. }
                            | ModuleBlockItem::Expression(value) => value,
                        };
                        collect(lowerer, module_id, value, captures);
                    }
                }
                ModuleExpression::Function { body, .. } => {
                    collect(lowerer, module_id, body, captures);
                }
                ModuleExpression::Loop { initial, body, .. } => {
                    collect(lowerer, module_id, initial, captures);
                    collect(lowerer, module_id, body, captures);
                }
                ModuleExpression::Break { value, .. } => {
                    if let Some(value) = value {
                        collect(lowerer, module_id, value, captures);
                    }
                }
                ModuleExpression::Case { subjects, arms, .. } => {
                    for subject in subjects {
                        collect(lowerer, module_id, subject, captures);
                    }
                    for arm in arms {
                        if let Some(guard) = &arm.guard {
                            collect(lowerer, module_id, guard, captures);
                        }
                        collect(lowerer, module_id, &arm.body, captures);
                    }
                }
            }
        }

        let mut captures = HashSet::new();
        collect(self, module_id, expression, &mut captures);
        let mut captures = captures.into_iter().collect::<Vec<_>>();
        captures.sort_unstable();
        captures
    }

    /// A fieldless constructor named as a value is that value, so it constructs
    /// here. Any other constructor reference evaluates to a callable value.
    /// Lowers an interpolated literal to `Text.append` calls over its parts, left to
    /// right, after converting an `Int` part with `Int.to_text` and a `Float` part with
    /// `Float.to_text`. Every node it adds carries the literal's origin.
    fn interpolation(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        parts: &[crate::semantic::InterpolationPart],
    ) -> CoreOperation {
        let mut pieces = Vec::new();
        for part in parts {
            let piece = match part {
                crate::semantic::InterpolationPart::Text(text) => self.push_node(
                    module_id,
                    expression,
                    CoreOperation::Constant(CoreLiteral::Text(text.clone())),
                ),
                crate::semantic::InterpolationPart::Expression(value) => {
                    let node = self.expression(module_id, value);
                    let fact = self.typed.expression_facts[&(module_id, value.semantic_id())];
                    let conversion = match resolved_type(&self.typed, fact.success) {
                        CanonicalType::Named { symbol, .. } => {
                            match self.typed.resolved.symbol(*symbol).name.as_str() {
                                "Int" => Some(StandardFunction::IntToText),
                                "Float" => Some(StandardFunction::FloatToText),
                                _ => None,
                            }
                        }
                        _ => None,
                    };
                    match conversion {
                        Some(function) => {
                            let callee = self.standard_function(function);
                            let callee = self.push_node(
                                module_id,
                                expression,
                                CoreOperation::Reference(callee),
                            );
                            self.push_node(
                                module_id,
                                expression,
                                CoreOperation::Call {
                                    callee,
                                    arguments: vec![node],
                                },
                            )
                        }
                        None => node,
                    }
                }
            };
            pieces.push(piece);
        }
        let mut pieces = pieces.into_iter();
        let mut text = pieces.next().expect("an interpolation has a part");
        let Some(mut next) = pieces.next() else {
            return CoreOperation::Identity(text);
        };
        let append = self.standard_function(StandardFunction::TextAppend);
        loop {
            let callee = self.push_node(module_id, expression, CoreOperation::Reference(append));
            let operation = CoreOperation::Call {
                callee,
                arguments: vec![text, next],
            };
            match pieces.next() {
                Some(after) => {
                    text = self.push_node(module_id, expression, operation);
                    next = after;
                }
                None => return operation,
            }
        }
    }

    /// The Core argument of an `Embed.text` or `Embed.bytes` call: the build reads
    /// the file its literal path names, relative to the module's directory, and the
    /// argument becomes the file's text or the lowercase hex of its bytes, so
    /// evaluation never touches the host. A misused call reports a diagnostic and
    /// keeps its argument.
    fn embed(
        &mut self,
        module_id: ModuleId,
        call: &ModuleExpression,
        function: StandardFunction,
        arguments: &[crate::semantic::CallArgument],
    ) -> CoreNodeId {
        let name = match function {
            StandardFunction::EmbedText => "Embed.text",
            _ => "Embed.bytes",
        };
        let source = &self.typed.resolved.semantic.modules[module_id.raw() as usize].source;
        let file_id = source.file_id();
        let directory = source.path().parent().map(std::path::Path::to_path_buf);
        let argument = &arguments[0].value;
        let error = |code, range, message| {
            Diagnostic::error(
                DiagnosticStage::ExecutionSetup,
                code,
                file_id,
                range,
                message,
            )
        };
        let contents = if !self.at_build_time {
            Err(error(
                "E5206",
                call.range(),
                format!(
                    "`{name}` reads a file when the program is built, so it may only \
                     initialize a module constant, outside any function value"
                ),
            ))
        } else if let ModuleExpression::Literal(Literal::Text {
            value: path, range, ..
        }) = argument
        {
            embedded_file(path, directory.as_deref())
                .map_err(|(code, message)| error(code, *range, message))
                .and_then(|contents| match function {
                    StandardFunction::EmbedText => String::from_utf8(contents).map_err(|invalid| {
                        error(
                            "E5209",
                            *range,
                            format!(
                                "`{name}` needs UTF-8, but `{path}` is not valid UTF-8 at byte \
                                 {}; embed it with `Embed.bytes`",
                                invalid.utf8_error().valid_up_to()
                            ),
                        )
                    }),
                    _ => Ok(contents.iter().map(|byte| format!("{byte:02x}")).collect()),
                })
        } else {
            Err(error(
                "E5207",
                argument.range(),
                format!("`{name}` needs a text literal path, so the build knows its inputs"),
            ))
        };
        match contents {
            Ok(contents) => self.push_node(
                module_id,
                argument,
                CoreOperation::Constant(CoreLiteral::Text(contents)),
            ),
            Err(diagnostic) => {
                self.diagnostics.push(diagnostic);
                self.expression(module_id, argument)
            }
        }
    }

    fn standard_function(&self, function: StandardFunction) -> SymbolId {
        self.typed
            .resolved
            .symbols
            .iter()
            .find(|symbol| {
                self.typed.resolved.standard_symbol(symbol.id)
                    == Some(StandardSymbol::Function(function))
            })
            .map(|symbol| symbol.id)
            .expect("every standard function is indexed")
    }

    /// A reference to `symbol`, except that a standard module constant becomes the
    /// call that builds its value: `Duration.zero` is `Duration.nanoseconds(0)`.
    fn value_reference(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        symbol: SymbolId,
    ) -> CoreOperation {
        // As a value, `Embed.text` could be called after the build, with no file read.
        if !self.embed_callee
            && let Some(StandardFunction::EmbedText | StandardFunction::EmbedBytes) =
                self.typed.resolved.native_operation(symbol)
        {
            let source = &self.typed.resolved.semantic.modules[module_id.raw() as usize].source;
            self.diagnostics.push(Diagnostic::error(
                DiagnosticStage::ExecutionSetup,
                "E5206",
                source.file_id(),
                expression.range(),
                "`Embed.text` and `Embed.bytes` read a file when the program is built, so they \
                 may only be called directly in a module constant's initializer, not used as \
                 values"
                    .to_owned(),
            ));
        }
        match self.typed.resolved.standard_symbol(symbol) {
            Some(StandardSymbol::Constant(StandardConstant::DurationZero)) => {
                let constructor = self.standard_function(StandardFunction::DurationNanoseconds);
                let callee =
                    self.push_node(module_id, expression, CoreOperation::Reference(constructor));
                let zero = self.push_node(
                    module_id,
                    expression,
                    CoreOperation::Constant(CoreLiteral::Integer(0)),
                );
                CoreOperation::Call {
                    callee,
                    arguments: vec![zero],
                }
            }
            _ => self.reference(symbol),
        }
    }

    fn reference(&self, symbol: SymbolId) -> CoreOperation {
        if is_core_constructor(&self.typed, symbol) {
            let (shape, _) = lower_core_constructor(&self.typed, symbol);
            if shape.outcome_success.is_none() && shape.fields.is_empty() {
                return CoreOperation::Construct {
                    constructor: symbol,
                    arguments: Vec::new(),
                    cause: None,
                };
            }
        }
        CoreOperation::Reference(symbol)
    }

    fn value_symbol(&self, module_id: ModuleId, semantic_id: SemanticId) -> Option<SymbolId> {
        self.typed
            .resolved
            .reference_resolution(module_id, semantic_id, Namespace::Value)
            .and_then(|resolution| match resolution {
                ReferenceResolution::Resolved(symbol) => Some(
                    self.clause_parameter_aliases
                        .get(&symbol)
                        .copied()
                        .unwrap_or(symbol),
                ),
                ReferenceResolution::Unresolved
                | ReferenceResolution::Ambiguous
                | ReferenceResolution::Inaccessible(_)
                | ReferenceResolution::DeferredToType => None,
            })
    }

    fn local_symbol(
        &self,
        module_id: ModuleId,
        semantic_id: SemanticId,
        kind: ProjectSymbolKind,
    ) -> Option<SymbolId> {
        self.typed
            .resolved
            .symbols
            .iter()
            .find(|symbol| {
                symbol.module_id == Some(module_id)
                    && symbol.semantic_id == Some(semantic_id)
                    && symbol.kind == kind
            })
            .map(|symbol| symbol.id)
    }
}

/// The type a constructor builds, which decides where its ordinal starts.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum OwningType {
    Standard(crate::standard::StandardType),
    Outcome,
    Struct(SymbolId),
    Enum(ModuleId, usize),
}

fn lower_core_symbols(typed: &TypedProject) -> Vec<CoreSymbol> {
    let mut symbols = typed
        .resolved
        .symbols
        .iter()
        .map(|symbol| {
            let kind = match symbol.kind {
                ProjectSymbolKind::Function
                    if typed.resolved.native_operation(symbol.id).is_some() =>
                {
                    CoreSymbolKind::Intrinsic
                }
                ProjectSymbolKind::Function => CoreSymbolKind::Function,
                ProjectSymbolKind::Constant => CoreSymbolKind::Constant,
                ProjectSymbolKind::ProcessName => CoreSymbolKind::ProcessName,
                ProjectSymbolKind::Requirement => CoreSymbolKind::Requirement,
                ProjectSymbolKind::Parameter
                | ProjectSymbolKind::LocalBinding
                | ProjectSymbolKind::PatternBinding => CoreSymbolKind::Local,
                ProjectSymbolKind::StructConstructor | ProjectSymbolKind::EnumConstructor => {
                    CoreSymbolKind::Constructor
                }
                ProjectSymbolKind::BuiltinValue
                    if matches!(symbol.name.as_str(), "Success" | "Failure") =>
                {
                    CoreSymbolKind::Constructor
                }
                ProjectSymbolKind::BuiltinValue
                    if matches!(
                        typed.resolved.standard_symbol(symbol.id),
                        Some(StandardSymbol::Constructor(_))
                    ) =>
                {
                    CoreSymbolKind::Constructor
                }
                ProjectSymbolKind::BuiltinValue
                    if matches!(
                        typed.resolved.standard_symbol(symbol.id),
                        Some(StandardSymbol::Function(_))
                    ) =>
                {
                    CoreSymbolKind::Intrinsic
                }
                _ => CoreSymbolKind::Other,
            };
            let (constructor, owner) = if kind == CoreSymbolKind::Constructor {
                let (constructor, owner) = lower_core_constructor(typed, symbol.id);
                (Some(constructor), Some(owner))
            } else {
                (None, None)
            };
            // A `process name` carries its trace label, which the native runtime
            // prints and puts in `NameTaken`.
            let name = if kind == CoreSymbolKind::ProcessName {
                crate::typing::process::process_name_label(typed, symbol.id)
            } else {
                symbol.name.clone()
            };
            let symbol = CoreSymbol {
                id: symbol.id,
                name,
                kind,
                constructor,
                standard: typed.resolved.standard_symbol(symbol.id).or_else(|| {
                    typed
                        .resolved
                        .native_operation(symbol.id)
                        .map(StandardSymbol::Function)
                }),
            };
            (symbol, owner)
        })
        .collect::<Vec<_>>();
    number_constructors_across_types(&mut symbols);
    symbols.into_iter().map(|(symbol, _)| symbol).collect()
}

/// `module/path.chimi:line` for each origin, with the one-based line of its start.
fn origin_locations(typed: &TypedProject, origins: &[Origin]) -> Vec<String> {
    let mut indexes = HashMap::new();
    origins
        .iter()
        .map(|origin| {
            let module = &typed.resolved.semantic.modules[origin.module_id.raw() as usize];
            let index = indexes.entry(origin.module_id).or_insert_with(|| {
                crate::line_index::LineIndex::new(Arc::from(module.source.text()))
            });
            let start = origin.range.start();
            let line = if index.text().is_char_boundary(start as usize) {
                index
                    .position(start, crate::line_index::PositionEncoding::Utf8)
                    .line
                    + 1
            } else {
                0
            };
            format!("{}.chimi:{line}", module.path)
        })
        .collect()
}

/// Every local symbol that lowered code binds: function and closure parameters,
/// closure captures, block bindings, loop states, and pattern bindings.
fn bound_locals(functions: &[CoreFunction], nodes: &[CoreNode]) -> HashSet<SymbolId> {
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
    let mut bound = functions
        .iter()
        .flat_map(|function| function.parameters.iter().copied())
        .collect::<HashSet<_>>();
    for node in nodes {
        match &node.operation {
            CoreOperation::Closure {
                parameters,
                captures,
                ..
            } => bound.extend(parameters.iter().chain(captures).copied()),
            CoreOperation::Sequence(steps) => {
                for step in steps {
                    if let CoreStep::Bind { symbol, .. } = step {
                        bound.insert(*symbol);
                    }
                }
            }
            CoreOperation::Loop { state, .. } => {
                bound.insert(*state);
            }
            CoreOperation::Match { arms, .. } => {
                for arm in arms {
                    bound.extend(arm.cause.map(|cause| cause.local));
                    arm.patterns
                        .iter()
                        .for_each(|pattern| pattern_bindings(pattern, &mut bound));
                }
            }
            _ => {}
        }
    }
    bound
}

/// Turns each constructor's position within its type into its program-wide
/// ordinal: each type gets a block as wide as its largest position, in order of
/// the type's first constructor symbol.
fn number_constructors_across_types(symbols: &mut [(CoreSymbol, Option<OwningType>)]) {
    let mut widths = HashMap::<OwningType, u32>::new();
    let mut owners = Vec::new();
    for (symbol, owner) in symbols.iter() {
        let (Some(constructor), Some(owner)) = (&symbol.constructor, owner) else {
            continue;
        };
        let width = widths.entry(*owner).or_insert_with(|| {
            owners.push(*owner);
            0
        });
        *width = (*width).max(constructor.ordinal + 1);
    }
    let mut starts = HashMap::new();
    let mut next = 0_u32;
    for owner in owners {
        starts.insert(owner, next);
        next = next
            .checked_add(widths[&owner])
            .expect("constructor count fits u32");
    }
    for (symbol, owner) in symbols.iter_mut() {
        if let (Some(constructor), Some(owner)) = (&mut symbol.constructor, owner) {
            constructor.ordinal += starts[owner];
        }
    }
}

/// The constructor's shape, with its position within its own type as the
/// ordinal, and the type it builds.
fn lower_core_constructor(
    typed: &TypedProject,
    constructor: SymbolId,
) -> (CoreConstructor, OwningType) {
    let symbol = typed.resolved.symbol(constructor);
    if let Some(StandardSymbol::Constructor(standard)) = typed.resolved.standard_symbol(constructor)
    {
        let constructor = standard;
        let shape = CoreConstructor {
            type_name: constructor.type_kind().name().to_owned(),
            variant_name: Some(constructor.name().to_owned()),
            fields: constructor
                .field_names()
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
            outcome_success: None,
            ordinal: constructor.ordinal(),
        };
        return (shape, OwningType::Standard(constructor.type_kind()));
    }
    if symbol.kind == ProjectSymbolKind::BuiltinValue {
        let shape = CoreConstructor {
            type_name: "Outcome".to_owned(),
            variant_name: Some(symbol.name.clone()),
            fields: vec!["value".to_owned()],
            outcome_success: Some(symbol.name == "Success"),
            ordinal: 0,
        };
        return (shape, OwningType::Outcome);
    }
    let fields = typed
        .fields
        .get(&constructor)
        .expect("verified nominal constructor has fields")
        .iter()
        .map(|field| field.name.clone())
        .collect();
    if symbol.kind == ProjectSymbolKind::StructConstructor {
        let shape = CoreConstructor {
            type_name: symbol.name.clone(),
            variant_name: None,
            fields,
            outcome_success: None,
            ordinal: 0,
        };
        return (shape, OwningType::Struct(constructor));
    }
    let module_id = symbol
        .module_id
        .expect("enum constructor belongs to a module");
    let semantic_id = symbol
        .semantic_id
        .expect("enum constructor has semantic identity");
    let (declaration_index, declaration) = typed.resolved.semantic.modules
        [module_id.raw() as usize]
        .declarations
        .iter()
        .enumerate()
        .find(|(_, declaration)| {
            declaration.kind == ModuleDeclarationKind::Enum
                && declaration
                    .variants
                    .iter()
                    .any(|variant| variant.semantic_id == semantic_id)
        })
        .expect("verified enum constructor belongs to an enum");
    let ordinal = declaration
        .variants
        .iter()
        .position(|variant| variant.semantic_id == semantic_id)
        .expect("verified enum constructor is one of its enum's variants");
    let shape = CoreConstructor {
        type_name: declaration.name.text.clone(),
        variant_name: Some(symbol.name.clone()),
        fields,
        outcome_success: None,
        ordinal: u32::try_from(ordinal).expect("variant count fits u32"),
    };
    (shape, OwningType::Enum(module_id, declaration_index))
}

fn is_core_constructor(typed: &TypedProject, symbol: SymbolId) -> bool {
    let symbol = typed.resolved.symbol(symbol);
    matches!(
        symbol.kind,
        ProjectSymbolKind::StructConstructor | ProjectSymbolKind::EnumConstructor
    ) || (symbol.kind == ProjectSymbolKind::BuiltinValue
        && (matches!(symbol.name.as_str(), "Success" | "Failure")
            || matches!(
                typed.resolved.standard_symbol(symbol.id),
                Some(StandardSymbol::Constructor(_))
            )))
}

fn resolved_type(typed: &TypedProject, mut type_id: crate::ids::TypeId) -> &CanonicalType {
    loop {
        match &typed.types[type_id.raw() as usize] {
            CanonicalType::InferenceVariable(variable) => {
                let Some(bound) = typed.inference_bindings.get(variable).copied() else {
                    return &typed.types[type_id.raw() as usize];
                };
                type_id = bound;
            }
            resolved => return resolved,
        }
    }
}

pub(crate) fn seal_legacy_entry(
    typed: TypedProgram,
) -> Result<LegacyVerifiedRoot, Vec<Diagnostic>> {
    let typed = Arc::new(typed);
    if !typed.diagnostics.is_empty() {
        return Err(typed.diagnostics.clone());
    }

    let entry_function = typed
        .resolved
        .semantic
        .functions
        .iter()
        .find(|function| function.public && function.name.text == "main");
    let Some(entry_function) = entry_function else {
        let range = typed.resolved.semantic.functions.first().map_or_else(
            || TextRange::empty(typed.resolved.semantic.source.text().len()),
            |function| function.name.range,
        );
        return Err(vec![Diagnostic::error(
            DiagnosticStage::ExecutionSetup,
            "E5001",
            typed.resolved.semantic.source.file_id(),
            range,
            "program must export `pub fn main()`",
        )]);
    };

    let Some(TypeFact::Known(construction_type)) = typed.construction_type else {
        panic!("diagnostic-free typed program must have a known entry construction type");
    };
    debug_assert_eq!(construction_type, crate::typing::STRUCT_TYPE);
    debug_assert_eq!(
        typed.construction_semantic_id,
        Some(entry_function.body.semantic_id)
    );
    debug_assert_eq!(
        typed.literal_semantic_id,
        Some(entry_function.body.value.semantic_id())
    );
    let Some(TypeFact::Known(_)) = typed.literal_type else {
        panic!("diagnostic-free typed program must have a known literal type");
    };

    let entry = typed
        .resolved
        .symbols
        .iter()
        .find(|symbol| {
            symbol.kind == SymbolKind::Function
                && symbol.semantic_id == entry_function.semantic_id
                && symbol.name == "main"
        })
        .map(|symbol| symbol.id)
        .expect("resolved main function has a symbol");
    Ok(LegacyVerifiedRoot { typed, entry })
}

pub(crate) fn lower_legacy(root: LegacyVerifiedRoot) -> LegacyCoreProgram {
    let typed = root.typed;
    let entry_symbol = typed.resolved.symbol(root.entry);
    let entry = typed
        .resolved
        .semantic
        .functions
        .iter()
        .find(|function| function.semantic_id == entry_symbol.semantic_id)
        .expect("verified entry resolves to its function");
    let structure = typed
        .resolved
        .structure()
        .expect("verified construction has a struct declaration");
    let Resolution::Resolved(constructor) = typed
        .resolved
        .constructor_resolution
        .expect("verified construction has a resolution")
    else {
        panic!("verified constructor must resolve");
    };
    let field = typed
        .resolved
        .symbols
        .iter()
        .find(|symbol| {
            symbol.kind == SymbolKind::Field && symbol.semantic_id == structure.field_semantic_id
        })
        .map(|symbol| symbol.id)
        .expect("verified struct field has a symbol");
    let Literal::Integer { value, .. } = entry.body.value else {
        panic!("verified M1 field value must be an integer");
    };

    LegacyCoreProgram {
        computation: entry_symbol.name.clone(),
        root_node_id: CoreNodeId::new(0),
        origin: Origin {
            origin_id: OriginId::new(entry.body.semantic_id.raw()),
            file_id: typed.resolved.semantic.source.file_id(),
            module_id: ModuleId::new(0),
            semantic_id: entry.body.semantic_id,
            range: entry.body.range,
        },
        constructor_name: typed.resolved.symbol(constructor).name.clone(),
        field_name: typed.resolved.symbol(field).name.clone(),
        value,
    }
}

/// The bytes of the file that `path` names under `directory`, a module's
/// directory, or the code and message of why it cannot be embedded.
fn embedded_file(
    path: &str,
    directory: Option<&std::path::Path>,
) -> Result<Vec<u8>, (&'static str, String)> {
    // An absolute path starts with an empty segment.
    if path.is_empty()
        || path.contains('\\')
        || path
            .split('/')
            .any(|segment| matches!(segment, "" | "." | ".."))
    {
        return Err((
            "E5207",
            format!(
                "embedded path `{path}` must be relative to this module's directory, with `/` \
                 between segments and no empty, `.`, or `..` segment"
            ),
        ));
    }
    let Some(directory) = directory else {
        return Err((
            "E5208",
            format!("cannot embed `{path}`: this module has no directory"),
        ));
    };
    let unreadable = |error: std::io::Error| ("E5208", format!("cannot embed `{path}`: {error}"));
    // A symbolic link may point anywhere, so the file it resolves to must still be
    // inside the module's directory.
    let file = directory.join(path).canonicalize().map_err(unreadable)?;
    if !file.starts_with(directory.canonicalize().map_err(unreadable)?) {
        return Err((
            "E5207",
            format!("embedded path `{path}` resolves outside this module's directory"),
        ));
    }
    std::fs::read(file).map_err(unreadable)
}
