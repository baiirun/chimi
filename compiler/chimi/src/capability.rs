//! Flow-sensitive checks for values that cross task and process ownership boundaries.
//!
//! Process rules (`process-and-supervisor-static-contracts.md`):
//! - `T4260`: a `Process.spec` body must be shareable, because every incarnation runs it
//!   again. A capture that is not shareable is reported at the body argument. A provider
//!   that is not shareable is reported at the body argument when `using` binds it to the
//!   body, and at the provider when a `using` binds it to a need that flowed out of a
//!   `Process.spec` call, directly or through calls of named functions.
//! - `Process.send` is a transfer boundary for its message, like `Channel.send`.
//! - `Supervisor.remove_process` and `remove_supervisor` consume their handle.
//!
//! Exclusive values move (`task-and-channel-capabilities.md`): a value built from an
//! exclusive binding, or from a part of one, carries that place (`FlowValue::carries`),
//! and the carried places move when the value is bound by `let` or a loop state, taken
//! apart by patterns, crosses a transfer boundary, or is passed to a function whose
//! body transfers that parameter (`TransferSummaries`).

use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::{ModuleId, SemanticId, SymbolId, TextRange};
use crate::resolution::{Namespace, ProjectSymbolKind, ReferenceResolution};
use crate::semantic::{
    BinaryOperator, FunctionSignature, ModuleBlockItem, ModuleDeclaration, ModuleDeclarationKind,
    ModuleExpression, Pattern,
};
use crate::standard::{StandardFunction, StandardSymbol};
use crate::typing::{CapabilityClass, PropertyDerivation, TypedProject, ValueProperties};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub(crate) struct CapabilityReport {
    pub(crate) expression_properties: HashMap<(ModuleId, SemanticId), ValueProperties>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

/// A guard on path length. Paths through recursive data stay short because a call
/// collapses repeated recursive steps (`Place::through_call`), so no program in the
/// corpus comes near this; a longer path is cut to its prefix, which names a larger
/// part and so over-approximates.
const MAX_PATH_DEPTH: usize = 32;

/// The shape of a list pattern: the elements it names before and after its rest,
/// and whether it has a rest. Without one, the list has exactly `front + back`
/// elements.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ListShape {
    front: usize,
    back: usize,
    rest: bool,
}

impl ListShape {
    fn admits(self, length: usize) -> bool {
        if self.rest {
            length >= self.front + self.back
        } else {
            length == self.front + self.back
        }
    }
}

/// One step from a value to a part of it.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum PathSegment {
    /// A named field: `channel.receiver`, or a constructor pattern's argument.
    Field(String),
    /// A tuple pattern's element, or a constructor pattern's argument whose field
    /// name is unknown.
    Position(usize),
    /// The element at `index` from the front of a list matched by `shape`.
    Front { index: usize, shape: ListShape },
    /// The element at `index` from the back of a list matched by `shape`.
    Back { index: usize, shape: ListShape },
    /// The elements between the front and back ones of a list matched by `shape`.
    Rest { shape: ListShape },
    /// Any sequence, possibly empty, of these steps (kept sorted and without
    /// repeats): what a recursive function reaches by taking the same steps
    /// (`next`, or a list's rest) at every level.
    Repeat(Vec<PathSegment>),
}

impl PathSegment {
    /// The list elements this step reaches when the list has `length` elements, as
    /// a half-open index range; `None` when the step's pattern does not match that
    /// length or the step is not into a list.
    fn list_elements(&self, length: usize) -> Option<(usize, usize)> {
        match self {
            Self::Front { index, shape } => shape.admits(length).then_some((*index, index + 1)),
            Self::Back { index, shape } => shape
                .admits(length)
                .then(|| (length - 1 - index, length - index)),
            Self::Rest { shape } => shape
                .admits(length)
                .then(|| (shape.front, length - shape.back)),
            Self::Field(_) | Self::Position(_) | Self::Repeat(_) => None,
        }
    }

    /// Whether this step goes from a value to a value of a type that can contain it
    /// again: a field (only a recursive type repeats one) or a list's rest. Repeats
    /// of these are what recursion over a value produces.
    fn may_recur(&self) -> bool {
        matches!(self, Self::Field(_) | Self::Rest { .. })
    }

    fn list_shape(&self) -> Option<ListShape> {
        match self {
            Self::Front { shape, .. } | Self::Back { shape, .. } | Self::Rest { shape } => {
                Some(*shape)
            }
            Self::Field(_) | Self::Position(_) | Self::Repeat(_) => None,
        }
    }

    /// Whether the parts these steps reach from one value may be the same part.
    /// Different fields, and different tuple or constructor positions, are apart.
    /// Two list steps overlap when some list length that both patterns match puts
    /// them on a common element, so `[r, ..rest]` keeps `r` and `rest` apart. A field
    /// and a position, whose correspondence is not known here, may overlap.
    fn may_reach_the_same_part(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Field(a), Self::Field(b)) => a == b,
            (Self::Position(a), Self::Position(b)) => a == b,
            _ => match (self.list_shape(), other.list_shape()) {
                (Some(left), Some(right)) => {
                    // An overlap needs a length no greater than this, or holds at
                    // every greater length too: a front step stays put, a back step
                    // keeps its distance from the end, and a rest only grows.
                    let longest = left.front + left.back + right.front + right.back + 2;
                    (0..=longest).any(|length| {
                        match (self.list_elements(length), other.list_elements(length)) {
                            (Some((a, b)), Some((c, d))) => a < d && c < b,
                            _ => false,
                        }
                    })
                }
                _ => true,
            },
        }
    }
}

/// A path as an automaton over steps, so that `Repeat` can stand for any number of
/// repetitions: `edges[state]` leads by a step, or by no step (`None`), to another
/// state, and the path ends at `end`.
struct PathAutomaton<'a> {
    edges: Vec<Vec<(Option<&'a PathSegment>, usize)>>,
    end: usize,
}

impl<'a> PathAutomaton<'a> {
    fn new(path: &'a [PathSegment]) -> Self {
        let mut automaton = Self {
            edges: vec![Vec::new()],
            end: 0,
        };
        automaton.end = automaton.add(path, 0);
        automaton
    }

    /// Adds `path` from `from`; returns the state where it ends.
    fn add(&mut self, path: &'a [PathSegment], mut from: usize) -> usize {
        for segment in path {
            if let PathSegment::Repeat(steps) = segment {
                // Each step of the repeat leads back to the same state.
                for step in steps {
                    self.edges[from].push((Some(step), from));
                }
            } else {
                self.edges.push(Vec::new());
                let to = self.edges.len() - 1;
                self.edges[from].push((Some(segment), to));
                from = to;
            }
        }
        from
    }
}

/// Whether some reading of `left` and of `right` (each `Repeat` taken any number of
/// times) walks both paths step by step, each step pair possibly reaching the same
/// part, until `done` says one of them has ended.
fn paths_meet(
    left: &[PathSegment],
    right: &[PathSegment],
    done: impl Fn(bool, bool) -> bool,
) -> bool {
    let left = PathAutomaton::new(left);
    let right = PathAutomaton::new(right);
    let mut seen = BTreeSet::new();
    let mut pending = vec![(0, 0)];
    while let Some((at_left, at_right)) = pending.pop() {
        if !seen.insert((at_left, at_right)) {
            continue;
        }
        if done(at_left == left.end, at_right == right.end) {
            return true;
        }
        for (step_left, to_left) in &left.edges[at_left] {
            if step_left.is_none() {
                pending.push((*to_left, at_right));
            }
        }
        for (step_right, to_right) in &right.edges[at_right] {
            if step_right.is_none() {
                pending.push((at_left, *to_right));
            }
        }
        for (step_left, to_left) in &left.edges[at_left] {
            for (step_right, to_right) in &right.edges[at_right] {
                if let (Some(step_left), Some(step_right)) = (step_left, step_right)
                    && step_left.may_reach_the_same_part(step_right)
                {
                    pending.push((*to_left, *to_right));
                }
            }
        }
    }
    false
}

/// Whether the parts at two paths from one binding may share a value: one path
/// leads into the other (the empty path is the whole binding).
fn paths_overlap(left: &[PathSegment], right: &[PathSegment]) -> bool {
    paths_meet(left, right, |left_ended, right_ended| {
        left_ended || right_ended
    })
}

/// Whether the part at `inner` lies inside the part at `outer` (or may).
fn path_inside(inner: &[PathSegment], outer: &[PathSegment]) -> bool {
    paths_meet(inner, outer, |_, outer_ended| outer_ended)
}

/// Appends `segment` to `path`. When it repeats a recursive step already on the
/// path (as a step or inside a `Repeat`), the steps from that one on become one
/// `Repeat` of all of them, merged with a `Repeat` just before. So a recursive call
/// adds nothing new after one level, while the steps after the recursion (the
/// `.receiver` of each list element) stay exact. `Repeat` sets only grow from steps
/// already on the path, so the paths of a recursive function are finitely many.
fn push_collapsing(path: &mut Vec<PathSegment>, segment: PathSegment) {
    let repeats = |step: &PathSegment| match step {
        PathSegment::Repeat(steps) => steps.contains(&segment),
        step => *step == segment,
    };
    if segment.may_recur()
        && let Some(first) = path.iter().position(repeats)
    {
        let mut steps = Vec::new();
        for step in path.split_off(first) {
            match step {
                PathSegment::Repeat(inner) => steps.extend(inner),
                step => steps.push(step),
            }
        }
        if let Some(PathSegment::Repeat(before)) = path.last_mut() {
            steps.append(before);
            path.pop();
        }
        steps.sort();
        steps.dedup();
        path.push(PathSegment::Repeat(steps));
    }
    if let (PathSegment::Repeat(steps), Some(PathSegment::Repeat(before))) =
        (&segment, path.last_mut())
    {
        before.extend(steps.iter().cloned());
        before.sort();
        before.dedup();
        return;
    }
    path.push(segment);
}

/// A binding, or a part of one reached by field reads or patterns. Every exclusive
/// place has one owner: taking it moves it out of its binding.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Place {
    root: SymbolId,
    path: Vec<PathSegment>,
}

impl Place {
    fn whole(root: SymbolId) -> Self {
        Self {
            root,
            path: Vec::new(),
        }
    }

    fn child(&self, segment: PathSegment) -> Self {
        self.joined(std::slice::from_ref(&segment))
    }

    /// `path` from this place, cut to `MAX_PATH_DEPTH` steps.
    fn joined(&self, path: &[PathSegment]) -> Self {
        let mut place = self.clone();
        place.path.extend_from_slice(path);
        place.path.truncate(MAX_PATH_DEPTH);
        place
    }

    /// The part at `path` of a callee's parameter, when this place is the argument.
    /// Repeated recursive steps collapse (`push_collapsing`), so the summaries of a
    /// recursive function reach a fixed point.
    fn through_call(&self, path: &[PathSegment]) -> Self {
        let mut place = self.clone();
        for segment in path {
            push_collapsing(&mut place.path, segment.clone());
        }
        place.path.truncate(MAX_PATH_DEPTH);
        place
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.root == other.root && paths_overlap(&self.path, &other.path)
    }

    fn name(&self, typed: &TypedProject) -> String {
        let mut name = typed.resolved.symbol(self.root).name.clone();
        for segment in &self.path {
            match segment {
                PathSegment::Field(field) => name.push_str(&format!(".{field}")),
                PathSegment::Position(index) => name.push_str(&format!(".{index}")),
                PathSegment::Front { index, .. } => name.push_str(&format!("[{index}]")),
                PathSegment::Back { index, .. } => {
                    name.push_str(&format!("[-{}]", index + 1));
                }
                PathSegment::Rest { .. } => name.push_str("[..]"),
                PathSegment::Repeat(_) => name.push_str("[...]"),
            }
        }
        name
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Transfer {
    at: TextRange,
    boundary: TransferBoundary,
}

/// What a read of a place uses of the value there, and so which moves out of its
/// binding make the read `T4241`.
#[derive(Clone, Copy)]
enum ReadCheck<'a> {
    /// The whole value: any move that overlaps the place.
    Whole,
    /// Only its tag, as a `case` subject: a move of the place or of a place it lies
    /// in. The pattern bindings check the parts they read.
    Tag,
    /// The parts at these paths, which the function it is passed to reads.
    Paths(&'a BTreeSet<Vec<PathSegment>>),
}

/// For a function or closure, the parts of each parameter, by position, that its
/// body transfers: a call moves those parts of the argument.
type ParameterPaths = BTreeMap<usize, BTreeSet<Vec<PathSegment>>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransferBoundary {
    ClosureCapture,
    TaskStart,
    ChannelSend,
    ProcessSend,
    ProcessRemoval,
    SupervisorRemoval,
    /// A `let` or loop state binding took the value that carried it.
    IntoBinding(SymbolId),
    /// The patterns of a `case`, or of a destructuring `let`, took apart a value
    /// that carried it.
    IntoPattern,
    /// A call of a function (the callee's binding, when it has one) whose body
    /// transfers the parameter.
    IntoCall(Option<SymbolId>),
    /// A `break` took it into the value of its loop.
    IntoLoopValue,
}

impl TransferBoundary {
    /// Whether the value leaves its binding's reach here. A move into a `let` or a
    /// pattern stays in the function until that binding escapes in turn.
    fn escapes(self) -> bool {
        !matches!(
            self,
            Self::IntoBinding(_) | Self::IntoPattern | Self::IntoLoopValue
        )
    }

    fn description(self, typed: &TypedProject) -> String {
        match self {
            Self::ClosureCapture => "into a one-shot closure".to_owned(),
            Self::TaskStart => "by Task.start".to_owned(),
            Self::ChannelSend => "by Channel.send".to_owned(),
            Self::ProcessSend => "by Process.send".to_owned(),
            Self::ProcessRemoval => "by Supervisor.remove_process".to_owned(),
            Self::SupervisorRemoval => "by Supervisor.remove_supervisor".to_owned(),
            Self::IntoBinding(owner) => format!("into `{}`", typed.resolved.symbol(owner).name),
            Self::IntoPattern => "into a pattern".to_owned(),
            Self::IntoCall(Some(callee)) => {
                format!("by a call of `{}`", typed.resolved.symbol(callee).name)
            }
            Self::IntoCall(None) => "by a call".to_owned(),
            Self::IntoLoopValue => "by a `break`".to_owned(),
        }
    }
}

#[derive(Clone, Debug)]
struct FlowValue {
    properties: ValueProperties,
    /// What makes the value never transferable, for diagnostics.
    blocked_by: Option<String>,
    /// What makes the value never shareable, for diagnostics.
    unshared_by: Option<String>,
    nonshareable_provider: Option<String>,
    /// The owned places (see `owned`) this value may hold: the place itself when it is
    /// read out of one (a binding, a field of one, or a pattern binding over a part of
    /// one), and those it was built from by constructors, literals, blocks, `case`
    /// arms, `break`, and calls whose result type can hold an argument's type. Taking
    /// the value moves them.
    carries: BTreeSet<Place>,
    /// When the value is a function, what a call of it transfers.
    parameter_transfers: ParameterPaths,
    /// The value's type holds an open type parameter, whose argument may be
    /// exclusive (`x: T`, `Option<T>`).
    holds_type_parameter: bool,
}

impl FlowValue {
    fn from_type(typed: &TypedProject, type_id: crate::ids::TypeId) -> Self {
        let properties = typed.value_properties(type_id);
        Self {
            properties,
            blocked_by: (properties.transferable == PropertyDerivation::Never)
                .then(|| typed.format_type(type_id)),
            unshared_by: (properties.shareable == PropertyDerivation::Never)
                .then(|| typed.format_type(type_id)),
            nonshareable_provider: None,
            carries: BTreeSet::new(),
            parameter_transfers: ParameterPaths::new(),
            holds_type_parameter: typed.holds_type_parameter(type_id),
        }
    }

    fn shareable() -> Self {
        Self {
            properties: ValueProperties::SHAREABLE,
            blocked_by: None,
            unshared_by: None,
            nonshareable_provider: None,
            carries: BTreeSet::new(),
            parameter_transfers: ParameterPaths::new(),
            holds_type_parameter: false,
        }
    }

    fn invalid() -> Self {
        Self {
            properties: ValueProperties::INVALID,
            blocked_by: None,
            unshared_by: None,
            nonshareable_provider: None,
            carries: BTreeSet::new(),
            parameter_transfers: ParameterPaths::new(),
            holds_type_parameter: false,
        }
    }

    fn combine(mut self, mut other: Self) -> Self {
        self.carries.append(&mut other.carries);
        add_parameter_transfers(
            &mut self.parameter_transfers,
            std::mem::take(&mut other.parameter_transfers),
        );
        let properties = self.properties.combine(other.properties);
        let blocked_by = if properties.transferable == PropertyDerivation::Never {
            self.blocked_by.or(other.blocked_by)
        } else {
            None
        };
        let unshared_by = if properties.shareable == PropertyDerivation::Never {
            self.unshared_by.or(other.unshared_by)
        } else {
            None
        };
        Self {
            properties,
            blocked_by,
            unshared_by,
            nonshareable_provider: self.nonshareable_provider.or(other.nonshareable_provider),
            carries: self.carries,
            parameter_transfers: self.parameter_transfers,
            holds_type_parameter: self.holds_type_parameter || other.holds_type_parameter,
        }
    }
}

#[derive(Clone, Debug)]
struct FlowBinding {
    value: FlowValue,
    /// The parts of this binding that moved out, by path (the empty path is the
    /// whole binding). Empty for a binding that is `part_of` another.
    moved: Vec<(Vec<PathSegment>, Transfer)>,
    requirement: bool,
    /// For a pattern binding over a part of a binding, that part: the pattern
    /// binding reads it, and taking it moves the part out of its binding.
    part_of: Option<Place>,
}

impl FlowBinding {
    fn owner(value: FlowValue) -> Self {
        Self {
            value,
            moved: Vec::new(),
            requirement: false,
            part_of: None,
        }
    }
}

type FlowEnvironment = HashMap<SymbolId, FlowBinding>;

/// For each named function, the needs (by lexical name) that flow out of a
/// `Process.spec` call in its body and stay open: a `using` around a call of the
/// function snapshots its providers into a spec.
type SpecNeedSummaries = HashMap<SymbolId, BTreeSet<String>>;

/// For each named function whose body transfers a parameter, the parts it
/// transfers. A part is transferred when it, or a binding it was moved into, reaches
/// a send, `Task.start`, a `using` provider, a removal, a closure capture, or a call
/// that transfers it in turn; a move into a `let` or pattern that goes no further
/// does not count. A call of the function moves those parts out of its arguments.
type TransferSummaries = HashMap<SymbolId, ParameterPaths>;

/// For each named function, the parts of each parameter its body reads, directly or
/// through calls. A call checks only those parts of an argument, so a record whose
/// `receiver` has moved can still go to a function that reads only its `sender`.
type ReadSummaries = HashMap<SymbolId, ParameterPaths>;

/// For each named generic function, the parameters (by position) whose type
/// parameter's value the body keeps more than one owner of, as `(x, x)` does. Such
/// a body is fine for a shareable `T`, so the function requires one: a call with an
/// exclusive argument there is `T4241`, like the inferred equatable and ordered
/// requirements on type parameters.
type SharedSummaries = HashMap<SymbolId, BTreeSet<usize>>;

#[derive(Default, Eq, PartialEq)]
struct Summaries {
    spec_needs: SpecNeedSummaries,
    transfers: TransferSummaries,
    reads: ReadSummaries,
    shared: SharedSummaries,
}

/// Ensures: the report of a pass whose summaries are a fixed point, so a function's
/// summary is the same whether its callers come before or after it. Every summary
/// only grows from pass to pass, from facts in the bodies and from the previous
/// pass's summaries. A call collapses repeated recursive steps into a `Repeat`
/// (`Place::through_call`), so the paths a recursive function reaches are finitely
/// many, and `MAX_PATH_DEPTH` cuts any path longer than that: there are finitely
/// many summaries and the loop ends.
pub(crate) fn check(typed: &TypedProject) -> CapabilityReport {
    let mut summaries = Summaries::default();
    loop {
        let mut checker = CapabilityChecker {
            typed,
            summaries: &summaries,
            expression_properties: HashMap::new(),
            diagnostics: Vec::new(),
            spec_needs: BTreeSet::new(),
            next_summaries: Summaries::default(),
            loop_breaks: Vec::new(),
            took: HashMap::new(),
            escaped: BTreeSet::new(),
            read_places: BTreeSet::new(),
            type_parameters: BTreeMap::new(),
            shared: BTreeSet::new(),
        };
        checker.check_modules();
        if checker.next_summaries == summaries {
            return CapabilityReport {
                expression_properties: checker.expression_properties,
                diagnostics: checker.diagnostics,
            };
        }
        summaries = checker.next_summaries;
    }
}

struct CapabilityChecker<'a> {
    typed: &'a TypedProject,
    /// The previous pass's summaries, read at calls.
    summaries: &'a Summaries,
    expression_properties: HashMap<(ModuleId, SemanticId), ValueProperties>,
    diagnostics: Vec<Diagnostic>,
    /// Needs that flowed out of `Process.spec` calls evaluated so far in the current
    /// declaration and that no enclosing `using` has bound yet.
    spec_needs: BTreeSet<String>,
    next_summaries: Summaries,
    /// For each enclosing loop of the function being evaluated, innermost last, what
    /// its `break` values carry: the loop's value carries them.
    loop_breaks: Vec<BTreeSet<Place>>,
    /// For each binding of the current declaration, the places moved into it when it
    /// was bound. Kept after the binding leaves scope, so an escape of a value built
    /// in a block still reaches what the block's bindings took.
    took: HashMap<SymbolId, BTreeSet<Place>>,
    /// The places of the current declaration that reached a transfer boundary,
    /// directly or through the bindings they were moved into.
    escaped: BTreeSet<Place>,
    /// The places the current declaration reads.
    read_places: BTreeSet<Place>,
    /// The parameters of the current declaration (by symbol, with their position)
    /// that are tracked only because their type holds a type parameter.
    type_parameters: BTreeMap<SymbolId, usize>,
    /// Those of them the body keeps more than one owner of.
    shared: BTreeSet<usize>,
}

impl CapabilityChecker<'_> {
    fn check_modules(&mut self) {
        for module in &self.typed.resolved.semantic.modules {
            for declaration in &module.declarations {
                self.check_declaration(module.id, declaration);
            }
        }
    }

    fn check_declaration(&mut self, module_id: ModuleId, declaration: &ModuleDeclaration) {
        let Some(body) = &declaration.body else {
            return;
        };
        let mut environment = FlowEnvironment::new();
        self.spec_needs.clear();
        self.took.clear();
        self.escaped.clear();
        self.read_places.clear();
        self.type_parameters.clear();
        self.shared.clear();
        let mut function = None;
        let mut parameters = Vec::new();
        if declaration.kind == ModuleDeclarationKind::Function {
            let Some(symbol) = self.typed.resolved.source_symbol(
                module_id,
                declaration.semantic_id,
                ProjectSymbolKind::Function,
                &declaration.name.text,
            ) else {
                return;
            };
            let Some(contract) = self.typed.contracts.get(&symbol) else {
                return;
            };
            parameters = self.add_signature_bindings(
                module_id,
                declaration
                    .function
                    .as_ref()
                    .expect("function declaration owns a signature"),
                contract,
                &mut environment,
            );
            function = Some(symbol);
            for (index, parameter) in parameters.iter().enumerate() {
                if let Some(parameter) = parameter
                    && let Some(binding) = environment.get(parameter)
                    && binding.value.properties.capability != CapabilityClass::Exclusive
                    && binding.value.holds_type_parameter
                    && owned(&binding.value)
                {
                    self.type_parameters.insert(*parameter, index);
                }
            }
        }
        self.evaluate(module_id, body, &mut environment);
        if let Some(symbol) = function {
            if !self.spec_needs.is_empty() {
                self.next_summaries
                    .spec_needs
                    .insert(symbol, std::mem::take(&mut self.spec_needs));
            }
            let transfers = parameter_paths(&parameters, &self.escaped);
            if !transfers.is_empty() {
                self.next_summaries.transfers.insert(symbol, transfers);
            }
            let reads = parameter_paths(&parameters, &self.read_places);
            self.next_summaries.reads.insert(symbol, reads);
            if !self.shared.is_empty() {
                self.next_summaries
                    .shared
                    .insert(symbol, std::mem::take(&mut self.shared));
            }
        }
    }

    /// Returns the parameter symbols by position (`None` where a parameter has no
    /// symbol), for `parameter_paths`.
    fn add_signature_bindings(
        &self,
        module_id: ModuleId,
        signature: &FunctionSignature,
        contract: &crate::typing::CallableContract,
        environment: &mut FlowEnvironment,
    ) -> Vec<Option<SymbolId>> {
        let mut parameters = Vec::new();
        for (parameter, type_id) in signature.parameters.iter().zip(&contract.inputs) {
            let symbol = self.typed.resolved.source_symbol(
                module_id,
                parameter.semantic_id,
                ProjectSymbolKind::Parameter,
                &parameter.name.text,
            );
            if let Some(symbol) = symbol {
                environment.insert(
                    symbol,
                    FlowBinding::owner(FlowValue::from_type(self.typed, *type_id)),
                );
            }
            parameters.push(symbol);
        }
        for need in &self.typed.needs_rows[contract.needs.raw() as usize].entries {
            environment.insert(
                need.requirement,
                FlowBinding {
                    requirement: true,
                    ..FlowBinding::owner(FlowValue::from_type(self.typed, need.interface))
                },
            );
        }
        parameters
    }

    fn evaluate(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        environment: &mut FlowEnvironment,
    ) -> FlowValue {
        let value = match expression {
            ModuleExpression::Literal(_) => self.type_value(module_id, expression),
            ModuleExpression::Name { semantic_id, name } => {
                let Some(symbol) = self.resolved_value_symbol(module_id, *semantic_id) else {
                    return self.record(module_id, expression, FlowValue::invalid());
                };
                if environment.contains_key(&symbol) {
                    self.read_place(
                        module_id,
                        expression,
                        &name.text,
                        name.range,
                        ReadCheck::Whole,
                        environment,
                    )
                } else if matches!(
                    self.typed.resolved.symbol(symbol).kind,
                    ProjectSymbolKind::Function
                        | ProjectSymbolKind::StructConstructor
                        | ProjectSymbolKind::EnumConstructor
                        | ProjectSymbolKind::BuiltinValue
                ) {
                    let mut value = FlowValue::shareable();
                    if let Some(transfers) = self.summaries.transfers.get(&symbol) {
                        value.parameter_transfers = transfers.clone();
                    }
                    value
                } else {
                    self.type_value(module_id, expression)
                }
            }
            ModuleExpression::Select { range, .. } => {
                if let Some(place) = self.expression_place(module_id, expression, environment) {
                    let text = place.name(self.typed);
                    self.read_place(
                        module_id,
                        expression,
                        &text,
                        *range,
                        ReadCheck::Whole,
                        environment,
                    )
                } else {
                    self.evaluate_select(module_id, expression, environment)
                }
            }
            ModuleExpression::Call {
                callee, arguments, ..
            } => {
                let mut callee_value = self.evaluate(module_id, callee, environment);
                let callee_symbol = self.expression_binding_symbol(module_id, callee);
                // A named function's argument is checked only at the parts the
                // function reads, from its summary in the previous pass.
                let reads = callee_symbol
                    .filter(|symbol| self.is_project_function(*symbol))
                    .map(|symbol| self.summaries.reads.get(&symbol));
                let unread = BTreeSet::new();
                let arguments = arguments
                    .iter()
                    .enumerate()
                    .map(|(index, argument)| {
                        let argument = &argument.value;
                        let value = match reads {
                            Some(reads)
                                if self
                                    .expression_place(module_id, argument, environment)
                                    .is_some() =>
                            {
                                let paths =
                                    reads.and_then(|reads| reads.get(&index)).unwrap_or(&unread);
                                let value = self.read_argument(
                                    module_id,
                                    argument,
                                    ReadCheck::Paths(paths),
                                    environment,
                                );
                                self.record(module_id, argument, value)
                            }
                            _ => self.evaluate(module_id, argument, environment),
                        };
                        (argument, value)
                    })
                    .collect::<Vec<_>>();
                // A call of a function whose body transfers a parameter moves the
                // argument: what the callee's value says, and for a named function
                // its summary from the previous pass.
                let mut transfers = std::mem::take(&mut callee_value.parameter_transfers);
                if let Some(summary) =
                    callee_symbol.and_then(|symbol| self.summaries.transfers.get(&symbol))
                {
                    add_parameter_transfers(&mut transfers, summary.clone());
                }
                self.transfer_parameters(
                    module_id,
                    &transfers,
                    TransferBoundary::IntoCall(callee_symbol),
                    &arguments,
                    environment,
                );
                if let Some(callee) =
                    callee_symbol.filter(|symbol| self.is_project_function(*symbol))
                {
                    self.check_shared_arguments(module_id, callee, &arguments);
                }
                match self.standard_function(module_id, callee) {
                    Some(StandardFunction::TaskStart) => {
                        if let Some((argument, thunk)) = arguments.first() {
                            self.check_task_transfer(module_id, argument, thunk, environment);
                        }
                    }
                    Some(StandardFunction::ChannelSend) => {
                        if let Some((argument, payload)) = arguments.get(1) {
                            self.check_message_transfer(
                                module_id,
                                argument,
                                payload,
                                TransferBoundary::ChannelSend,
                                environment,
                            );
                        }
                    }
                    Some(StandardFunction::ProcessSend) => {
                        if let Some((argument, message)) = arguments.get(1) {
                            self.check_message_transfer(
                                module_id,
                                argument,
                                message,
                                TransferBoundary::ProcessSend,
                                environment,
                            );
                        }
                    }
                    Some(StandardFunction::ProcessSpec) => {
                        if let Some((argument, body)) = arguments.first() {
                            self.check_spec_body(module_id, argument, body);
                        }
                    }
                    Some(
                        function @ (StandardFunction::SupervisorRemoveProcess
                        | StandardFunction::SupervisorRemoveSupervisor),
                    ) => {
                        let boundary = if function == StandardFunction::SupervisorRemoveProcess {
                            TransferBoundary::ProcessRemoval
                        } else {
                            TransferBoundary::SupervisorRemoval
                        };
                        if let Some((argument, handle)) = arguments.get(1) {
                            self.transfer_carried(handle, argument.range(), boundary, environment);
                        }
                    }
                    Some(_) => {}
                    None => {
                        if let Some(symbol) = self.expression_binding_symbol(module_id, callee)
                            && let Some(needs) = self.summaries.spec_needs.get(&symbol)
                        {
                            self.spec_needs.extend(needs.iter().cloned());
                        }
                    }
                }
                // The callee is not followed: a result that could hold an exclusive
                // value may hold the closure called and every argument whose type
                // occurs in the result's type.
                let mut result = self.type_value(module_id, expression);
                if result.properties.shareable != PropertyDerivation::Always {
                    result.carries = callee_value.carries;
                    let result_type = self.expression_type(module_id, expression);
                    for (argument, value) in arguments {
                        let embeds = match (result_type, self.expression_type(module_id, argument))
                        {
                            (Some(result_type), Some(argument_type)) => {
                                self.typed.type_may_hold(result_type, argument_type)
                            }
                            _ => true,
                        };
                        if embeds {
                            self.hold_part(module_id, &mut result, argument.range(), value);
                        }
                    }
                }
                result
            }
            ModuleExpression::List { elements, .. } | ModuleExpression::Tuple { elements, .. } => {
                let mut value = FlowValue::shareable();
                for element in elements {
                    let part = self.evaluate(module_id, element, environment);
                    value = self.join_part(module_id, value, element.range(), part);
                }
                value
            }
            ModuleExpression::Interpolation { parts, .. } => {
                for part in crate::semantic::interpolated_expressions(parts) {
                    self.evaluate(module_id, part, environment);
                }
                FlowValue::shareable()
            }
            ModuleExpression::Record { fields, .. } => {
                let mut value = FlowValue::shareable();
                for field in fields {
                    let part = self.evaluate(module_id, &field.value, environment);
                    value = self.join_part(module_id, value, field.value.range(), part);
                }
                value
            }
            ModuleExpression::Update { base, fields, .. } => {
                let mut value = self.evaluate_update_base(module_id, base, fields, environment);
                for field in fields {
                    let part = self.evaluate(module_id, &field.value, environment);
                    value = self.join_part(module_id, value, field.value.range(), part);
                }
                value
            }
            ModuleExpression::Unary { operand, .. } => {
                self.evaluate(module_id, operand, environment);
                self.type_value(module_id, expression)
            }
            ModuleExpression::Binary {
                operator: BinaryOperator::And | BinaryOperator::Or,
                left,
                right,
                ..
            } => {
                // The right operand runs only on one branch of the left, like a `case` arm,
                // so its transfers are possible transfers.
                self.evaluate(module_id, left, environment);
                let mut branch = environment.clone();
                self.evaluate(module_id, right, &mut branch);
                self.merge_transfers(environment, &[branch]);
                self.type_value(module_id, expression)
            }
            ModuleExpression::Binary { left, right, .. } => {
                self.evaluate(module_id, left, environment);
                self.evaluate(module_id, right, environment);
                self.type_value(module_id, expression)
            }
            ModuleExpression::Group { expression, .. } => {
                self.evaluate(module_id, expression, environment)
            }
            ModuleExpression::Using {
                expression,
                providers,
                ..
            } => {
                let outer_spec_needs = std::mem::take(&mut self.spec_needs);
                let mut value = self.evaluate(module_id, expression, environment);
                let mut inner_spec_needs = std::mem::take(&mut self.spec_needs);
                for provider in providers {
                    let mut provided = self.evaluate(module_id, &provider.value, environment);
                    if inner_spec_needs.remove(&provider.name.text)
                        && provided.properties.shareable == PropertyDerivation::Never
                    {
                        self.nonshareable_spec_provider(
                            module_id,
                            provider.value.range(),
                            &provider.name.text,
                        );
                    }
                    if provided.properties.shareable == PropertyDerivation::Never {
                        provided.nonshareable_provider = Some(provider.name.text.clone());
                    }
                    self.transfer_carried(
                        &provided,
                        provider.value.range(),
                        TransferBoundary::ClosureCapture,
                        environment,
                    );
                    value = value.combine(provided);
                }
                self.spec_needs.extend(outer_spec_needs);
                self.spec_needs.append(&mut inner_spec_needs);
                value
            }
            ModuleExpression::Sequence { expressions, .. } => expressions
                .iter()
                .map(|child| self.evaluate(module_id, child, environment))
                .last()
                .unwrap_or_else(FlowValue::invalid),
            ModuleExpression::Block { items, .. } => {
                self.evaluate_block(module_id, items, environment)
            }
            ModuleExpression::Function {
                signature, body, ..
            } => self.evaluate_function(module_id, expression, signature, body, environment),
            ModuleExpression::Loop {
                state_id,
                initial,
                body,
                ..
            } => {
                let mut loop_value = self.type_value(module_id, expression);
                loop_value.carries =
                    self.evaluate_loop(module_id, *state_id, initial, body, environment);
                loop_value
            }
            ModuleExpression::Break { value, .. } => {
                if let Some(value) = value {
                    let carries = self.evaluate(module_id, value, environment).carries;
                    for place in &carries {
                        self.transfer_place(
                            place,
                            value.range(),
                            TransferBoundary::IntoLoopValue,
                            environment,
                        );
                    }
                    if let Some(breaks) = self.loop_breaks.last_mut() {
                        breaks.extend(carries);
                    }
                }
                FlowValue::invalid()
            }
            ModuleExpression::Case { subjects, arms, .. } => {
                // A subject read out of a place leaves its parts there: a pattern
                // binding reads its part of the place, and taking it moves the part.
                // A subject built here is taken apart by the patterns, as by a
                // destructuring `let`: they take what it carries.
                let mut subject_places = Vec::new();
                let mut subject_carries = Vec::new();
                for subject in subjects {
                    let place = self.expression_place(module_id, subject, environment);
                    if place.is_some() {
                        let value =
                            self.read_argument(module_id, subject, ReadCheck::Tag, environment);
                        self.record(module_id, subject, value);
                        subject_carries.push(BTreeSet::new());
                    } else {
                        let value = self.evaluate(module_id, subject, environment);
                        self.transfer_carried(
                            &value,
                            subject.range(),
                            TransferBoundary::IntoPattern,
                            environment,
                        );
                        subject_carries.push(value.carries);
                    }
                    subject_places.push(place);
                }
                let mut values = Vec::new();
                let mut arm_environments = Vec::new();
                for arm in arms {
                    let mut arm_environment = environment.clone();
                    for (index, pattern) in arm.patterns.iter().enumerate() {
                        let place = subject_places.get(index).cloned().flatten();
                        let took = subject_carries.get(index).cloned().unwrap_or_default();
                        self.add_pattern_bindings(
                            module_id,
                            pattern,
                            place,
                            &took,
                            &mut arm_environment,
                        );
                    }
                    if let Some(guard) = &arm.guard {
                        self.evaluate(module_id, guard, &mut arm_environment);
                    }
                    values.push(self.evaluate(module_id, &arm.body, &mut arm_environment));
                    arm_environments.push(arm_environment);
                }
                self.merge_transfers(environment, &arm_environments);
                values
                    .into_iter()
                    .fold(FlowValue::shareable(), FlowValue::combine)
            }
            ModuleExpression::Empty { .. } => FlowValue::invalid(),
        };
        self.record(module_id, expression, value)
    }

    fn evaluate_block(
        &mut self,
        module_id: ModuleId,
        items: &[ModuleBlockItem],
        environment: &mut FlowEnvironment,
    ) -> FlowValue {
        let outer_symbols = environment.keys().copied().collect::<BTreeSet<_>>();
        let mut value = FlowValue::invalid();
        for item in items {
            match item {
                ModuleBlockItem::Binding {
                    name,
                    value: expression,
                    ..
                } => {
                    value = self.evaluate(module_id, expression, environment);
                    if let Some(symbol) = self.typed.resolved.source_symbol(
                        module_id,
                        expression.semantic_id(),
                        ProjectSymbolKind::LocalBinding,
                        &name.text,
                    ) {
                        self.bind_owner(symbol, value.clone(), expression.range(), environment);
                    }
                }
                ModuleBlockItem::Expression(expression) => {
                    value = self.evaluate(module_id, expression, environment);
                }
            }
        }
        environment.retain(|symbol, _| outer_symbols.contains(symbol));
        value
    }

    fn evaluate_function(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        signature: &FunctionSignature,
        body: &ModuleExpression,
        environment: &mut FlowEnvironment,
    ) -> FlowValue {
        let captures = self.capture_places(module_id, body, environment);
        let outer_loop_breaks = std::mem::take(&mut self.loop_breaks);
        let mut closure = FlowValue::shareable();
        let mut function_environment = environment.clone();
        for (place, read) in captures {
            let name = place.name(self.typed);
            if let Some(transfer) = self.moved_part(environment, &place) {
                self.use_after_transfer(module_id, &place, body.range(), transfer.boundary);
            }
            self.read_places.insert(place.clone());
            let mut captured = self.place_value(module_id, read, environment);
            captured.parameter_transfers.clear();
            if captured.properties.transferable == PropertyDerivation::Never {
                captured.blocked_by = Some(format!(
                    "capture `{name}` ({})",
                    captured
                        .blocked_by
                        .as_deref()
                        .unwrap_or("non-transferable value")
                ));
            }
            if captured.properties.shareable == PropertyDerivation::Never {
                captured.unshared_by = Some(format!(
                    "capture `{name}` ({})",
                    captured
                        .unshared_by
                        .as_deref()
                        .unwrap_or("non-shareable value")
                ));
            }
            if owned(&captured) {
                self.transfer_place(
                    &place,
                    expression.range(),
                    TransferBoundary::ClosureCapture,
                    environment,
                );
            }
            closure = closure.combine(captured);
            // Inside the closure the capture is its own, whatever happened to it before.
            if let Some(binding) = function_environment.get_mut(&place.root) {
                binding
                    .moved
                    .retain(|(path, _)| !paths_overlap(path, &place.path));
            }
        }

        let mut parameters = Vec::new();
        if let Some(fact) = self
            .typed
            .expression_facts
            .get(&(module_id, expression.semantic_id()))
            && let Some(contract) = self.typed.callable_contract_for_type(fact.success)
        {
            parameters = self.add_signature_bindings(
                module_id,
                signature,
                contract,
                &mut function_environment,
            );
        }
        self.evaluate(module_id, body, &mut function_environment);
        self.loop_breaks = outer_loop_breaks;
        closure.parameter_transfers = parameter_paths(&parameters, &self.escaped);
        closure
    }

    fn check_task_transfer(
        &mut self,
        module_id: ModuleId,
        argument: &ModuleExpression,
        thunk: &FlowValue,
        environment: &mut FlowEnvironment,
    ) {
        if let Some(provider) = &thunk.nonshareable_provider {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticStage::Type,
                "T4242",
                self.file_id(module_id),
                argument.range(),
                format!("Task.start requires provider `{provider}` to be shareable across tasks"),
            ));
            return;
        }
        if thunk.properties.transferable == PropertyDerivation::Never {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticStage::Type,
                "T4240",
                self.file_id(module_id),
                argument.range(),
                format!(
                    "Task.start cannot transfer thunk because {} is parent-local",
                    thunk.blocked_by.as_deref().unwrap_or("a captured value")
                ),
            ));
            return;
        }
        self.transfer_carried(
            thunk,
            argument.range(),
            TransferBoundary::TaskStart,
            environment,
        );
    }

    /// `Channel.send` and `Process.send`: the payload moves to the receiver.
    ///
    /// Ensures: `T4243` when the payload is never transferable; otherwise every
    /// place the payload carries is transferred, so a later use is `T4241`.
    fn check_message_transfer(
        &mut self,
        module_id: ModuleId,
        argument: &ModuleExpression,
        payload: &FlowValue,
        boundary: TransferBoundary,
        environment: &mut FlowEnvironment,
    ) {
        let operation = match boundary {
            TransferBoundary::ProcessSend => "Process.send",
            _ => "Channel.send",
        };
        if payload.properties.transferable == PropertyDerivation::Never {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticStage::Type,
                "T4243",
                self.file_id(module_id),
                argument.range(),
                format!(
                    "{operation} cannot transfer {} because it is parent-local",
                    payload.blocked_by.as_deref().unwrap_or("the payload")
                ),
            ));
            return;
        }
        self.transfer_carried(payload, argument.range(), boundary, environment);
    }

    /// `T4260` for a `Process.spec` body: every incarnation runs it again, so its
    /// captures and the providers bound to it must be shareable. Records the needs that
    /// flow out of the call, for an enclosing `using` to check.
    fn check_spec_body(
        &mut self,
        module_id: ModuleId,
        argument: &ModuleExpression,
        body: &FlowValue,
    ) {
        if let Some(provider) = &body.nonshareable_provider {
            self.nonshareable_spec_provider(module_id, argument.range(), provider);
        } else if body.properties.shareable == PropertyDerivation::Never
            && let Some(unshared_by) = &body.unshared_by
        {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticStage::Type,
                "T4260",
                self.file_id(module_id),
                argument.range(),
                format!(
                    "a Process.spec body must be shareable, because every incarnation runs it \
                     again, but {unshared_by} is not shareable"
                ),
            ));
        }
        if let Some(fact) = self
            .typed
            .expression_facts
            .get(&(module_id, argument.semantic_id()))
            && let Some(contract) = self.typed.callable_contract_for_type(fact.success)
        {
            let needs = &self.typed.needs_rows[contract.needs.raw() as usize].entries;
            self.spec_needs
                .extend(needs.iter().map(|need| need.lexical_name.clone()));
        }
    }

    fn nonshareable_spec_provider(
        &mut self,
        module_id: ModuleId,
        range: TextRange,
        provider: &str,
    ) {
        self.diagnostics.push(Diagnostic::error(
            DiagnosticStage::Type,
            "T4260",
            self.file_id(module_id),
            range,
            format!(
                "Process.spec requires provider `{provider}` to be shareable, because every \
                 incarnation of the process uses it"
            ),
        ));
    }

    /// Binds `owner` to `value`: the places the value carries move into it, so the
    /// new binding is their only owner.
    fn bind_owner(
        &mut self,
        owner: SymbolId,
        mut value: FlowValue,
        at: TextRange,
        environment: &mut FlowEnvironment,
    ) {
        let carries = std::mem::take(&mut value.carries);
        for place in &carries {
            self.transfer_place(place, at, TransferBoundary::IntoBinding(owner), environment);
        }
        self.took.entry(owner).or_default().extend(carries);
        environment.insert(owner, FlowBinding::owner(value));
    }

    fn transfer_carried(
        &mut self,
        value: &FlowValue,
        at: TextRange,
        boundary: TransferBoundary,
        environment: &mut FlowEnvironment,
    ) {
        for place in &value.carries {
            self.transfer_place(place, at, boundary, environment);
        }
    }

    /// Moves `place` out of its binding, unless a part of it that overlaps has moved
    /// already (the use that took it again was reported where it read the place).
    /// At a boundary that escapes, the place and what its binding took escape.
    fn transfer_place(
        &mut self,
        place: &Place,
        at: TextRange,
        boundary: TransferBoundary,
        environment: &mut FlowEnvironment,
    ) {
        if boundary.escapes() {
            self.escape(place);
        }
        let Some(binding) = environment.get_mut(&place.root) else {
            return;
        };
        if !binding
            .moved
            .iter()
            .any(|(path, _)| paths_overlap(path, &place.path))
        {
            binding
                .moved
                .push((place.path.clone(), Transfer { at, boundary }));
        }
    }

    /// Adds `place`, and what its binding took, transitively, to `reached`.
    fn reach(&self, place: &Place, reached: &mut BTreeSet<Place>) {
        if !reached.insert(place.clone()) {
            return;
        }
        for taken in self.took.get(&place.root).into_iter().flatten() {
            self.reach(taken, reached);
        }
    }

    fn escape(&mut self, place: &Place) {
        if !self.escaped.insert(place.clone()) {
            return;
        }
        // A part of a binding may hold anything the binding took.
        let took = self.took.get(&place.root).cloned().unwrap_or_default();
        for taken in &took {
            self.escape(taken);
        }
    }

    /// The move that took `place`, or a part of it, or a place it is part of.
    fn moved_part(&self, environment: &FlowEnvironment, place: &Place) -> Option<Transfer> {
        environment
            .get(&place.root)?
            .moved
            .iter()
            .find_map(|(path, transfer)| paths_overlap(path, &place.path).then_some(*transfer))
    }

    /// The place `expression` reads: a binding by name (a pattern binding over a part
    /// of a binding reads that part), or a field of a place. `None` for anything else,
    /// and for a need, which no value takes.
    fn expression_place(
        &self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        environment: &FlowEnvironment,
    ) -> Option<Place> {
        match expression {
            ModuleExpression::Name { semantic_id, .. } => {
                let symbol = self.resolved_value_symbol(module_id, *semantic_id)?;
                let binding = environment.get(&symbol)?;
                if binding.requirement {
                    return None;
                }
                Some(
                    binding
                        .part_of
                        .clone()
                        .unwrap_or_else(|| Place::whole(symbol)),
                )
            }
            ModuleExpression::Group { expression, .. } => {
                self.expression_place(module_id, expression, environment)
            }
            ModuleExpression::Select {
                receiver, member, ..
            } => Some(
                self.expression_place(module_id, receiver, environment)?
                    .child(PathSegment::Field(member.text.clone())),
            ),
            _ => None,
        }
    }

    /// Reads the place `expression` names, as `check` says (`T4241` when a part it
    /// uses moved), or the need it names. The value carries the place when it is
    /// owned.
    fn read_place(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        text: &str,
        range: TextRange,
        check: ReadCheck<'_>,
        environment: &FlowEnvironment,
    ) -> FlowValue {
        let place = self.expression_place(module_id, expression, environment);
        let moved = place.as_ref().and_then(|place| match check {
            ReadCheck::Whole => {
                self.read_places.insert(place.clone());
                self.moved_part(environment, place)
            }
            ReadCheck::Tag => environment
                .get(&place.root)?
                .moved
                .iter()
                .find_map(|(path, transfer)| path_inside(&place.path, path).then_some(*transfer)),
            ReadCheck::Paths(paths) => paths.iter().find_map(|path| {
                let part = place.through_call(path);
                self.read_places.insert(part.clone());
                self.moved_part(environment, &part)
            }),
        });
        if let (Some(transfer), Some(place)) = (moved, &place) {
            let diagnostic = Diagnostic::error(
                DiagnosticStage::Type,
                "T4241",
                self.file_id(module_id),
                range,
                format!(
                    "`{text}` was transferred {} and cannot be used by its previous owner",
                    transfer.boundary.description(self.typed)
                ),
            );
            self.owner_conflict(place, diagnostic);
        }
        let mut value = self.place_value(module_id, expression, environment);
        if let Some(place) = place
            && owned(&value)
        {
            value.carries.insert(place);
        }
        value
    }

    /// `read_place` for a place passed on whole: an argument or a `case` subject.
    fn read_argument(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        check: ReadCheck<'_>,
        environment: &FlowEnvironment,
    ) -> FlowValue {
        let text = self
            .expression_place(module_id, expression, environment)
            .map_or_else(String::new, |place| place.name(self.typed));
        self.read_place(
            module_id,
            expression,
            &text,
            expression.range(),
            check,
            environment,
        )
    }

    /// Whether `symbol` is a function declared in the project, whose body the
    /// checker summarizes.
    fn is_project_function(&self, symbol: SymbolId) -> bool {
        self.typed.resolved.symbol(symbol).kind == ProjectSymbolKind::Function
            && self.typed.resolved.standard_symbol(symbol).is_none()
    }

    /// The value at a place or need, without checking or taking it: a binding's
    /// value, or a field's, which is the value it was read out of when the field's
    /// type alone does not decide (a function field).
    fn place_value(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        environment: &FlowEnvironment,
    ) -> FlowValue {
        let mut value = match expression {
            ModuleExpression::Name { semantic_id, .. } => self
                .resolved_value_symbol(module_id, *semantic_id)
                .and_then(|symbol| environment.get(&symbol))
                .map_or_else(
                    || self.type_value(module_id, expression),
                    |binding| binding.value.clone(),
                ),
            ModuleExpression::Group { expression, .. } => {
                self.place_value(module_id, expression, environment)
            }
            ModuleExpression::Select { receiver, .. } => {
                let receiver = self.place_value(module_id, receiver, environment);
                let value = self.type_value(module_id, expression);
                if value.properties.transferable == PropertyDerivation::ValueDependent {
                    receiver
                } else {
                    value
                }
            }
            _ => self.type_value(module_id, expression),
        };
        value.carries.clear();
        self.record(module_id, expression, value)
    }

    /// The base of `{ ..base, field: value }`. When it is a place of a struct or
    /// record type, only the fields the update keeps are read and carried, so the
    /// update may replace a field that moved out; otherwise the base is a whole value.
    fn evaluate_update_base(
        &mut self,
        module_id: ModuleId,
        base: &ModuleExpression,
        replaced: &[crate::semantic::RecordValueField],
        environment: &mut FlowEnvironment,
    ) -> FlowValue {
        let place = self.expression_place(module_id, base, environment);
        let kept = self
            .expression_type(module_id, base)
            .and_then(|type_id| self.typed.value_fields(type_id))
            .map(|fields| {
                fields
                    .into_iter()
                    .filter(|(name, _)| !replaced.iter().any(|field| field.name.text == *name))
                    .collect::<Vec<_>>()
            });
        let (Some(place), Some(kept)) = (place, kept) else {
            return self.evaluate(module_id, base, environment);
        };
        let paths = kept
            .iter()
            .map(|(name, _)| vec![PathSegment::Field(name.clone())])
            .collect::<BTreeSet<_>>();
        let mut value = self.read_argument(module_id, base, ReadCheck::Paths(&paths), environment);
        value.carries.clear();
        for (name, type_id) in kept {
            if owned(&FlowValue::from_type(self.typed, type_id)) {
                value.carries.insert(place.child(PathSegment::Field(name)));
            }
        }
        self.record(module_id, base, value)
    }

    /// A field read out of a value that is not a place carries what that value
    /// carries, as `boxed(r).inner` holds `r`.
    fn evaluate_select(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        environment: &mut FlowEnvironment,
    ) -> FlowValue {
        let ModuleExpression::Select { receiver, .. } = expression else {
            return self.type_value(module_id, expression);
        };
        let receiver = self.evaluate(module_id, receiver, environment);
        let mut value = self.type_value(module_id, expression);
        let carries = if value.properties.shareable == PropertyDerivation::Always {
            BTreeSet::new()
        } else {
            receiver.carries.clone()
        };
        if value.properties.transferable == PropertyDerivation::ValueDependent {
            value = receiver;
        }
        value.carries = carries;
        value
    }

    /// Adds `part` to the value built from `whole`. `T4241` at `at` when the part
    /// carries a place that `whole` already holds: the built value would hold one
    /// exclusive value twice.
    fn join_part(
        &mut self,
        module_id: ModuleId,
        whole: FlowValue,
        at: TextRange,
        part: FlowValue,
    ) -> FlowValue {
        self.check_held_once(module_id, &whole.carries, at, &part.carries);
        whole.combine(part)
    }

    /// `join_part` for a call's result, whose properties are its type's.
    fn hold_part(
        &mut self,
        module_id: ModuleId,
        result: &mut FlowValue,
        at: TextRange,
        part: FlowValue,
    ) {
        self.check_held_once(module_id, &result.carries, at, &part.carries);
        result.carries.extend(part.carries);
    }

    fn check_held_once(
        &mut self,
        module_id: ModuleId,
        held: &BTreeSet<Place>,
        at: TextRange,
        part: &BTreeSet<Place>,
    ) {
        if let Some(place) = part
            .iter()
            .find(|place| held.iter().any(|held| held.overlaps(place)))
        {
            self.taken_twice(module_id, place, at);
        }
    }

    fn taken_twice(&mut self, module_id: ModuleId, place: &Place, at: TextRange) {
        let diagnostic = Diagnostic::error(
            DiagnosticStage::Type,
            "T4241",
            self.file_id(module_id),
            at,
            format!(
                "`{}` is taken twice here; an exclusive value has one owner",
                place.name(self.typed)
            ),
        );
        self.owner_conflict(place, diagnostic);
    }

    /// Reports a second owner of `place`, unless the place holds only values of the
    /// function's type parameters: then the function requires those parameters'
    /// arguments to be shareable (`SharedSummaries`) instead.
    fn owner_conflict(&mut self, place: &Place, diagnostic: Diagnostic) {
        match self.type_parameter_origins(place, &mut BTreeSet::new()) {
            Some(origins) => self.shared.extend(origins),
            None => self.diagnostics.push(diagnostic),
        }
    }

    /// The type-parameter parameters whose values are all that `place` may hold: the
    /// parameter it is part of, or those its binding took. `None` when it may hold
    /// anything else.
    fn type_parameter_origins(
        &self,
        place: &Place,
        visiting: &mut BTreeSet<SymbolId>,
    ) -> Option<BTreeSet<usize>> {
        if let Some(index) = self.type_parameters.get(&place.root) {
            return Some(BTreeSet::from([*index]));
        }
        if !visiting.insert(place.root) {
            return Some(BTreeSet::new());
        }
        let took = self.took.get(&place.root).filter(|took| !took.is_empty())?;
        let mut origins = BTreeSet::new();
        for taken in took {
            origins.extend(self.type_parameter_origins(taken, visiting)?);
        }
        Some(origins)
    }

    /// `T4241` for an exclusive argument where the callee keeps more than one owner
    /// of its parameter's type-parameter value. A type-parameter argument passes the
    /// requirement on to the caller.
    fn check_shared_arguments(
        &mut self,
        module_id: ModuleId,
        callee: SymbolId,
        arguments: &[(&ModuleExpression, FlowValue)],
    ) {
        let Some(shared) = self.summaries.shared.get(&callee) else {
            return;
        };
        for index in shared {
            let Some((argument, value)) = arguments.get(*index) else {
                continue;
            };
            if value.properties.capability == CapabilityClass::Exclusive {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticStage::Type,
                    "T4241",
                    self.file_id(module_id),
                    argument.range(),
                    format!(
                        "`{}` keeps more than one owner of this argument, so it must be \
                         shareable",
                        self.typed.resolved.symbol(callee).name
                    ),
                ));
                continue;
            }
            for place in &value.carries {
                if let Some(origins) = self.type_parameter_origins(place, &mut BTreeSet::new()) {
                    self.shared.extend(origins);
                }
            }
        }
    }

    /// Moves, out of each argument, the parts of it that the callee transfers: the
    /// parts of the place an argument reads, or what a built argument carries.
    /// `T4241` when one call takes a place twice.
    fn transfer_parameters(
        &mut self,
        module_id: ModuleId,
        transfers: &ParameterPaths,
        boundary: TransferBoundary,
        arguments: &[(&ModuleExpression, FlowValue)],
        environment: &mut FlowEnvironment,
    ) {
        // What earlier arguments gave up; parts of one argument may overlap.
        let mut taken = Vec::<Place>::new();
        for (index, paths) in transfers {
            let Some((argument, value)) = arguments.get(*index) else {
                continue;
            };
            let places = match self.expression_place(module_id, argument, environment) {
                Some(place) => paths.iter().map(|path| place.through_call(path)).collect(),
                None => value.carries.iter().cloned().collect::<Vec<_>>(),
            };
            if let Some(place) = places
                .iter()
                .find(|place| taken.iter().any(|taken| taken.overlaps(place)))
            {
                self.taken_twice(module_id, place, argument.range());
                continue;
            }
            for place in &places {
                self.transfer_place(place, argument.range(), boundary, environment);
            }
            taken.extend(places);
        }
    }

    /// Collects the places a closure body reads from `environment`, each with the
    /// expression that reads it. A place inside another captured place is left out.
    fn capture_places<'e>(
        &self,
        module_id: ModuleId,
        expression: &'e ModuleExpression,
        environment: &FlowEnvironment,
    ) -> Vec<(Place, &'e ModuleExpression)> {
        let mut captures = BTreeMap::new();
        self.collect_capture_places(module_id, expression, environment, &mut captures);
        let mut kept = Vec::<(Place, &ModuleExpression)>::new();
        for (place, read) in captures {
            if !kept
                .iter()
                .any(|(outer, _)| outer.root == place.root && place.path.starts_with(&outer.path))
            {
                kept.push((place, read));
            }
        }
        kept
    }

    fn collect_capture_places<'e>(
        &self,
        module_id: ModuleId,
        expression: &'e ModuleExpression,
        environment: &FlowEnvironment,
        captures: &mut BTreeMap<Place, &'e ModuleExpression>,
    ) {
        if let Some(place) = self.expression_place(module_id, expression, environment) {
            captures.entry(place).or_insert(expression);
            return;
        }
        for child in expression_children(expression) {
            self.collect_capture_places(module_id, child, environment, captures);
        }
    }

    fn merge_transfers(&self, environment: &mut FlowEnvironment, branches: &[FlowEnvironment]) {
        for (symbol, binding) in environment {
            for branch in branches {
                let Some(branch_binding) = branch.get(symbol) else {
                    continue;
                };
                for (path, transfer) in &branch_binding.moved {
                    if !binding
                        .moved
                        .iter()
                        .any(|(moved, _)| paths_overlap(moved, path))
                    {
                        binding.moved.push((path.clone(), *transfer));
                    }
                }
            }
        }
    }

    /// Binds the names in `pattern`, which matches the value at `place` when the
    /// subject was read out of one: each pattern binding is then part of that place.
    /// Otherwise each pattern binding took `took`, what the built subject carried.
    fn add_pattern_bindings(
        &mut self,
        module_id: ModuleId,
        pattern: &Pattern,
        place: Option<Place>,
        took: &BTreeSet<Place>,
        environment: &mut FlowEnvironment,
    ) {
        if let Pattern::Binding {
            semantic_id, name, ..
        }
        | Pattern::Typed {
            semantic_id,
            name: Some(name),
            ..
        }
        | Pattern::Alias {
            semantic_id, name, ..
        } = pattern
            && let Some(symbol) = self.typed.resolved.source_symbol(
                module_id,
                *semantic_id,
                ProjectSymbolKind::PatternBinding,
                &name.text,
            )
            && let Some(type_id) = self
                .typed
                .pattern_binding_types
                .get(&(module_id, *semantic_id))
        {
            environment.insert(
                symbol,
                FlowBinding {
                    part_of: place.clone(),
                    ..FlowBinding::owner(FlowValue::from_type(self.typed, *type_id))
                },
            );
            if place.is_none() {
                self.took
                    .entry(symbol)
                    .or_default()
                    .extend(took.iter().cloned());
            }
        }
        let part = |segment| place.as_ref().map(|place| place.child(segment));
        match pattern {
            Pattern::Alias { pattern, .. } => {
                self.add_pattern_bindings(module_id, pattern, place.clone(), took, environment);
            }
            Pattern::Constructor { arguments, .. } => {
                let fields = self
                    .resolved_value_symbol(module_id, pattern.semantic_id())
                    .and_then(|constructor| self.typed.fields.get(&constructor));
                for (index, argument) in arguments.iter().enumerate() {
                    let segment = fields
                        .and_then(|fields| fields.get(index))
                        .map_or(PathSegment::Position(index), |field| {
                            PathSegment::Field(field.name.clone())
                        });
                    self.add_pattern_bindings(
                        module_id,
                        argument,
                        part(segment),
                        took,
                        environment,
                    );
                }
            }
            Pattern::Tuple { elements, .. } => {
                for (index, element) in elements.iter().enumerate() {
                    let segment = PathSegment::Position(index);
                    self.add_pattern_bindings(module_id, element, part(segment), took, environment);
                }
            }
            Pattern::List {
                prefix,
                rest,
                suffix,
                ..
            } => {
                let shape = ListShape {
                    front: prefix.len(),
                    back: suffix.len(),
                    rest: rest.is_some(),
                };
                for (index, element) in prefix.iter().enumerate() {
                    let segment = PathSegment::Front { index, shape };
                    self.add_pattern_bindings(module_id, element, part(segment), took, environment);
                }
                if let Some(rest) = rest {
                    let segment = PathSegment::Rest { shape };
                    self.add_pattern_bindings(module_id, rest, part(segment), took, environment);
                }
                for (index, element) in suffix.iter().rev().enumerate() {
                    let segment = PathSegment::Back { index, shape };
                    self.add_pattern_bindings(module_id, element, part(segment), took, environment);
                }
            }
            Pattern::Wildcard { .. }
            | Pattern::Binding { .. }
            | Pattern::Literal { .. }
            | Pattern::Typed { .. } => {}
        }
    }

    fn expression_type(
        &self,
        module_id: ModuleId,
        expression: &ModuleExpression,
    ) -> Option<crate::ids::TypeId> {
        self.typed
            .expression_facts
            .get(&(module_id, expression.semantic_id()))
            .map(|fact| fact.success)
    }

    fn type_value(&self, module_id: ModuleId, expression: &ModuleExpression) -> FlowValue {
        self.typed
            .expression_facts
            .get(&(module_id, expression.semantic_id()))
            .map_or_else(FlowValue::invalid, |fact| {
                FlowValue::from_type(self.typed, fact.success)
            })
    }

    fn record(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        value: FlowValue,
    ) -> FlowValue {
        self.expression_properties
            .insert((module_id, expression.semantic_id()), value.properties);
        value
    }

    fn standard_function(
        &self,
        module_id: ModuleId,
        expression: &ModuleExpression,
    ) -> Option<StandardFunction> {
        let symbol = self.expression_binding_symbol(module_id, expression)?;
        match self.typed.resolved.standard_symbol(symbol) {
            Some(StandardSymbol::Function(function)) => Some(function),
            _ => None,
        }
    }

    fn expression_binding_symbol(
        &self,
        module_id: ModuleId,
        expression: &ModuleExpression,
    ) -> Option<SymbolId> {
        match expression {
            ModuleExpression::Name { semantic_id, .. }
            | ModuleExpression::Select { semantic_id, .. } => {
                self.resolved_value_symbol(module_id, *semantic_id)
            }
            ModuleExpression::Group { expression, .. } => {
                self.expression_binding_symbol(module_id, expression)
            }
            _ => None,
        }
    }

    fn resolved_value_symbol(
        &self,
        module_id: ModuleId,
        semantic_id: SemanticId,
    ) -> Option<SymbolId> {
        match self
            .typed
            .resolved
            .reference_resolution(module_id, semantic_id, Namespace::Value)?
        {
            ReferenceResolution::Resolved(symbol) => Some(symbol),
            _ => None,
        }
    }

    fn use_after_transfer(
        &mut self,
        module_id: ModuleId,
        place: &Place,
        range: TextRange,
        boundary: TransferBoundary,
    ) {
        let diagnostic = Diagnostic::error(
            DiagnosticStage::Type,
            "T4241",
            self.file_id(module_id),
            range,
            format!(
                "`{}` was transferred {} and cannot be captured again",
                place.name(self.typed),
                boundary.description(self.typed)
            ),
        );
        self.owner_conflict(place, diagnostic);
    }

    /// Requires: `loop_id` is the loop's `state_id`, which defines its state binding.
    /// Ensures: the body's value, the next pass's state, takes what it carries. A
    /// capability bound outside the loop, or a part of one, that the pass moves so that
    /// it escapes or reaches the next state is `T4250`, because the next pass would
    /// take it again. The state binding is exempt: each pass rebinds it. Returns what
    /// the loop's `break` values carry.
    fn evaluate_loop(
        &mut self,
        module_id: ModuleId,
        loop_id: SemanticId,
        initial: &ModuleExpression,
        body: &ModuleExpression,
        environment: &mut FlowEnvironment,
    ) -> BTreeSet<Place> {
        let initial_value = self.evaluate(module_id, initial, environment);
        let state = self
            .typed
            .resolved
            .source_symbols(module_id, loop_id)
            .iter()
            .copied()
            .find(|symbol| {
                self.typed.resolved.symbol(*symbol).kind == ProjectSymbolKind::LocalBinding
            });
        // The initial value's bindings move into the state, which only the pass binds.
        if let Some(state) = state {
            let boundary = TransferBoundary::IntoBinding(state);
            self.transfer_carried(&initial_value, initial.range(), boundary, environment);
        }
        let mut pass = environment.clone();
        if let Some(state) = state {
            self.bind_owner(state, initial_value, initial.range(), &mut pass);
        }
        self.loop_breaks.push(BTreeSet::new());
        let escaped_before = self.escaped.clone();
        let next = self.evaluate(module_id, body, &mut pass);
        let breaks = self.loop_breaks.pop().unwrap_or_default();
        // The next state takes what the body's value carries, and with each place
        // what its binding took (`h` in `let h = Hand.Over(r)`).
        let mut into_state = BTreeSet::new();
        if let Some(state) = state {
            for place in &next.carries {
                self.transfer_place(
                    place,
                    body.range(),
                    TransferBoundary::IntoBinding(state),
                    &mut pass,
                );
                self.reach(place, &mut into_state);
            }
        }
        // A move that escaped during the pass would be repeated by the next pass. One
        // into a binding of the pass that goes no further leaves the value in the
        // task, as a read does.
        let escaped_in_pass = self
            .escaped
            .difference(&escaped_before)
            .cloned()
            .collect::<Vec<_>>();
        let mut reused = environment
            .iter()
            .filter_map(|(symbol, before)| Some((*symbol, before, pass.get(symbol)?)))
            .flat_map(|(symbol, before, after)| {
                after
                    .moved
                    .iter()
                    .filter(|(path, _)| !before.moved.iter().any(|(moved, _)| moved == path))
                    .map(move |(path, transfer)| {
                        (
                            Place {
                                root: symbol,
                                path: path.clone(),
                            },
                            *transfer,
                        )
                    })
            })
            .filter(|(place, _)| {
                escaped_in_pass
                    .iter()
                    .chain(&into_state)
                    .any(|reused| reused.overlaps(place))
            })
            .collect::<Vec<_>>();
        reused.sort_by_key(|(_, transfer)| transfer.at.start());
        for (place, transfer) in reused {
            let diagnostic = Diagnostic::error(
                DiagnosticStage::Type,
                "T4250",
                self.file_id(module_id),
                transfer.at,
                format!(
                    "`{}` is transferred {} inside a loop; the next pass would use it again",
                    place.name(self.typed),
                    transfer.boundary.description(self.typed)
                ),
            );
            self.owner_conflict(&place, diagnostic);
        }
        pass.retain(|symbol, _| environment.contains_key(symbol));
        *environment = pass;
        breaks
    }

    fn file_id(&self, module_id: ModuleId) -> crate::ids::FileId {
        self.typed.resolved.semantic.modules[module_id.raw() as usize]
            .source
            .file_id()
    }
}

/// The paths of `places` rooted at each parameter, by position.
fn parameter_paths(parameters: &[Option<SymbolId>], places: &BTreeSet<Place>) -> ParameterPaths {
    parameters
        .iter()
        .enumerate()
        .filter_map(|(index, symbol)| {
            let symbol = (*symbol)?;
            let paths = places
                .iter()
                .filter(|place| place.root == symbol)
                .map(|place| place.path.clone())
                .collect::<BTreeSet<_>>();
            (!paths.is_empty()).then_some((index, paths))
        })
        .collect()
}

/// Whether a value has one owner whose moves are tracked: an exclusive value, or one
/// that holds a type parameter's value, which may be exclusive. A function or
/// interface value whose captures are not known is not: calling it twice is fine.
fn owned(value: &FlowValue) -> bool {
    value.properties.capability == CapabilityClass::Exclusive
        || (value.holds_type_parameter
            && value.properties.shareable == PropertyDerivation::ValueDependent)
}

fn add_parameter_transfers(into: &mut ParameterPaths, transfers: ParameterPaths) {
    for (index, mut paths) in transfers {
        into.entry(index).or_default().append(&mut paths);
    }
}

fn expression_children(expression: &ModuleExpression) -> Vec<&ModuleExpression> {
    match expression {
        ModuleExpression::Literal(_)
        | ModuleExpression::Name { .. }
        | ModuleExpression::Empty { .. } => Vec::new(),
        ModuleExpression::Select { receiver, .. }
        | ModuleExpression::Unary {
            operand: receiver, ..
        }
        | ModuleExpression::Group {
            expression: receiver,
            ..
        } => vec![receiver],
        ModuleExpression::Call {
            callee, arguments, ..
        } => std::iter::once(callee.as_ref())
            .chain(arguments.iter().map(|argument| &argument.value))
            .collect(),
        ModuleExpression::List { elements, .. }
        | ModuleExpression::Tuple { elements, .. }
        | ModuleExpression::Sequence {
            expressions: elements,
            ..
        } => elements.iter().collect(),
        ModuleExpression::Record { fields, .. } => {
            fields.iter().map(|field| &field.value).collect()
        }
        ModuleExpression::Update { base, fields, .. } => std::iter::once(base.as_ref())
            .chain(fields.iter().map(|field| &field.value))
            .collect(),
        ModuleExpression::Interpolation { parts, .. } => {
            crate::semantic::interpolated_expressions(parts).collect()
        }
        ModuleExpression::Binary { left, right, .. } => vec![left, right],
        ModuleExpression::Using {
            expression,
            providers,
            ..
        } => std::iter::once(expression.as_ref())
            .chain(providers.iter().map(|provider| &provider.value))
            .collect(),
        ModuleExpression::Block { items, .. } => items
            .iter()
            .map(|item| match item {
                ModuleBlockItem::Binding { value, .. } | ModuleBlockItem::Expression(value) => {
                    value
                }
            })
            .collect(),
        ModuleExpression::Function { body, .. } => vec![body],
        ModuleExpression::Loop { initial, body, .. } => vec![initial, body],
        ModuleExpression::Break { value, .. } => value.iter().map(AsRef::as_ref).collect(),
        ModuleExpression::Case { subjects, arms, .. } => subjects
            .iter()
            .chain(
                arms.iter()
                    .flat_map(|arm| arm.guard.iter().chain(std::iter::once(&arm.body))),
            )
            .collect(),
    }
}
