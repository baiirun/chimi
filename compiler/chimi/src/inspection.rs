use crate::compiler::ValuePropertyStatus;
use crate::diagnostic::Diagnostic;
use crate::ids::{ModuleId, SemanticId, SymbolId, TextRange, TypeId};
use crate::json::write_json_string_escaping_controls;
use crate::operations::{RunnerSupply, TestSupply, runner_supply};
use crate::resolution::{ProjectReference, ProjectSymbolKind, ReferenceResolution};
use crate::semantic::{
    CaseOrigin, ModuleBlockItem, ModuleDeclaration, ModuleDeclarationKind, ModuleExpression,
    Pattern, SemanticModule, is_generated_name,
};
use crate::standard::StandardSymbol;
use crate::syntax::ParseResult;
use crate::typing::{
    CallableContract, CanonicalType, PropertyDerivation, TypedProject, ValueProperty,
};
use std::fmt::{self, Write};
use std::path::PathBuf;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxFrame {
    kind: &'static str,
    range: TextRange,
}

impl SyntaxFrame {
    pub const fn kind(&self) -> &'static str {
        self.kind
    }

    pub const fn range(&self) -> TextRange {
        self.range
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectedToken {
    kind: &'static str,
    text: String,
    range: TextRange,
}

impl InspectedToken {
    pub const fn kind(&self) -> &'static str {
        self.kind
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub const fn range(&self) -> TextRange {
        self.range
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolInspection {
    name: String,
    namespace: &'static str,
    kind: &'static str,
    module_path: Option<String>,
    source_path: Option<PathBuf>,
    declaration_range: Option<TextRange>,
    runner: Option<HostSupply>,
}

impl SymbolInspection {
    /// How the runner supplies this symbol, when it is a host interface.
    pub const fn runner(&self) -> Option<HostSupply> {
        self.runner
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn namespace(&self) -> &'static str {
        self.namespace
    }

    pub const fn kind(&self) -> &'static str {
        self.kind
    }

    pub fn module_path(&self) -> Option<&str> {
        self.module_path.as_deref()
    }

    pub fn source_path(&self) -> Option<&std::path::Path> {
        self.source_path.as_deref()
    }

    pub const fn declaration_range(&self) -> Option<TextRange> {
        self.declaration_range
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferenceInspection {
    text: String,
    namespace: &'static str,
    status: &'static str,
    range: TextRange,
    target: Option<SymbolInspection>,
}

impl ReferenceInspection {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub const fn namespace(&self) -> &'static str {
        self.namespace
    }

    pub const fn status(&self) -> &'static str {
        self.status
    }

    pub const fn range(&self) -> TextRange {
        self.range
    }

    pub const fn target(&self) -> Option<&SymbolInspection> {
        self.target.as_ref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NeedInspection {
    name: String,
    interface: String,
    runner: Option<HostSupply>,
}

impl NeedInspection {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn interface(&self) -> &str {
        &self.interface
    }

    /// How the runner supplies this need's interface, when it is a host
    /// interface the runner closes on `main` under some name.
    pub const fn runner(&self) -> Option<HostSupply> {
        self.runner
    }

    /// Whether the runner closes this need on `main`: its interface is a
    /// host interface and the need has the name the runner closes.
    pub fn supplied_on_main(&self) -> bool {
        self.runner
            .is_some_and(|supply| supply.fixed_name().is_none_or(|fixed| fixed == self.name))
    }

    /// A sentence on where this need comes from, for a need on a host
    /// interface; `None` for any other need.
    pub fn runner_note(&self) -> Option<String> {
        let supply = self.runner?;
        let name = &self.name;
        Some(if self.supplied_on_main() {
            let tests = match supply.supply.in_tests {
                TestSupply::Using => "A test must supply it with `using`.".to_owned(),
                in_tests => format!("A test gets {}.", in_tests.describe()),
            };
            format!("When `{name}` reaches `main`, the runner supplies it. {tests}")
        } else {
            format!(
                "`{name}` is not supplied by the runner, which supplies a `{}` only as `{}`. \
                 Some caller must supply `{name}` with `using`, in a program and in tests.",
                supply.interface,
                supply.fixed_name().unwrap_or_default()
            )
        })
    }
}

/// How the runner supplies a host interface to `main`, and what a test gets
/// for it (docs/runner-supplied-needs.md).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostSupply {
    interface: &'static str,
    supply: RunnerSupply,
}

impl HostSupply {
    pub const fn interface(&self) -> &'static str {
        self.interface
    }

    /// The only need name the runner closes, or `None` when any name is.
    pub const fn fixed_name(&self) -> Option<&'static str> {
        self.supply.fixed_name
    }

    /// What a test gets: `virtual_clock`, `seeded_random`, `captured_console`,
    /// or `using` when the test must supply it.
    pub fn in_tests(&self) -> &'static str {
        self.supply.in_tests.token()
    }

    /// A sentence on how the runner supplies the interface, for hover on its
    /// name.
    pub fn note(&self) -> String {
        let interface = self.interface;
        let main = match self.supply.fixed_name {
            Some(name) => format!(
                "The runner supplies a `{interface}` to `main` when the need is named `{name}`."
            ),
            None => format!("The runner supplies a `{interface}` to `main` under any need name."),
        };
        let tests = match self.supply.in_tests {
            TestSupply::Using => "A test must supply it with `using`.".to_owned(),
            in_tests => format!("A test gets {}.", in_tests.describe()),
        };
        format!("{main} {tests}")
    }
}

fn host_supply(typed: &TypedProject, symbol: SymbolId) -> Option<HostSupply> {
    let Some(StandardSymbol::Type(interface)) = typed.resolved.standard_symbol(symbol) else {
        return None;
    };
    runner_supply(interface).map(|supply| HostSupply {
        interface: interface.name(),
        supply,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallableInspection {
    display: String,
    inputs: Vec<String>,
    success: String,
    failures: Vec<String>,
    failure_parameters: Vec<String>,
    needs: Vec<NeedInspection>,
    needs_parameters: Vec<String>,
}

impl CallableInspection {
    pub fn display(&self) -> &str {
        &self.display
    }

    pub fn inputs(&self) -> &[String] {
        &self.inputs
    }

    pub fn success(&self) -> &str {
        &self.success
    }

    pub fn failures(&self) -> &[String] {
        &self.failures
    }

    pub fn failure_parameters(&self) -> &[String] {
        &self.failure_parameters
    }

    pub fn needs(&self) -> &[NeedInspection] {
        &self.needs
    }

    pub fn needs_parameters(&self) -> &[String] {
        &self.needs_parameters
    }
}

#[derive(Debug)]
pub struct InspectionReport {
    module_path: String,
    source_path: PathBuf,
    offset: u32,
    line: u32,
    byte_column: u32,
    syntax_path: Vec<SyntaxFrame>,
    token: Option<InspectedToken>,
    reference: Option<ReferenceInspection>,
    inferred_type: Option<String>,
    value_properties: Option<ValuePropertyInspection>,
    callable: Option<CallableInspection>,
    instantiated_callable: Option<CallableInspection>,
    need: Option<NeedInspection>,
    executable_origin: Option<u32>,
    clause: Option<ClausePosition>,
    diagnostics: Vec<Diagnostic>,
    semantic_id: Option<SemanticId>,
    module_id: ModuleId,
}

/// Where a function clause sits in its group, counted from 1.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClausePosition {
    index: usize,
    count: usize,
}

impl ClausePosition {
    pub const fn index(&self) -> usize {
        self.index
    }

    pub const fn count(&self) -> usize {
        self.count
    }
}

impl InspectionReport {
    pub fn module_path(&self) -> &str {
        &self.module_path
    }

    pub fn source_path(&self) -> &std::path::Path {
        &self.source_path
    }

    pub const fn offset(&self) -> u32 {
        self.offset
    }

    pub const fn line(&self) -> u32 {
        self.line
    }

    pub const fn byte_column(&self) -> u32 {
        self.byte_column
    }

    pub fn syntax_path(&self) -> &[SyntaxFrame] {
        &self.syntax_path
    }

    pub const fn token(&self) -> Option<&InspectedToken> {
        self.token.as_ref()
    }

    pub const fn reference(&self) -> Option<&ReferenceInspection> {
        self.reference.as_ref()
    }

    /// The need a reference to a need names, such as `console` in
    /// `console.write_line(...)`.
    pub const fn need(&self) -> Option<&NeedInspection> {
        self.need.as_ref()
    }

    pub fn inferred_type(&self) -> Option<&str> {
        self.inferred_type.as_deref()
    }

    /// Whether the inspected value's type is equatable and ordered, derived
    /// as `==` and `<` check it.
    pub const fn value_properties(&self) -> Option<ValuePropertyInspection> {
        self.value_properties
    }

    pub const fn callable(&self) -> Option<&CallableInspection> {
        self.callable.as_ref()
    }

    /// The callee's contract with the types this call fixed, when the cursor
    /// is on a call to a generic function. `callable` stays the declared one.
    pub const fn instantiated_callable(&self) -> Option<&CallableInspection> {
        self.instantiated_callable.as_ref()
    }

    pub const fn executable_origin(&self) -> Option<u32> {
        self.executable_origin
    }

    /// The clause whose head holds the offset, when it is a multi-clause function's.
    pub const fn clause(&self) -> Option<ClausePosition> {
        self.clause
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn render_json(&self) -> String {
        let mut output = String::from("{\"schema\":\"chimi.inspection.v1\",\"module\":");
        write_json_string_escaping_controls(&mut output, &self.module_path);
        output.push_str(",\"path\":");
        write_json_string_escaping_controls(&mut output, &self.source_path.to_string_lossy());
        write!(
            output,
            ",\"offset\":{},\"line\":{},\"byte_column\":{},\"syntax_path\":[",
            self.offset, self.line, self.byte_column
        )
        .expect("writing JSON to a string cannot fail");
        for (index, frame) in self.syntax_path.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            output.push_str("{\"kind\":");
            write_json_string_escaping_controls(&mut output, frame.kind);
            output.push_str(",\"range\":");
            write_json_range(&mut output, frame.range);
            output.push('}');
        }
        output.push_str("],\"token\":");
        if let Some(token) = &self.token {
            output.push_str("{\"kind\":");
            write_json_string_escaping_controls(&mut output, token.kind);
            output.push_str(",\"text\":");
            write_json_string_escaping_controls(&mut output, &token.text);
            output.push_str(",\"range\":");
            write_json_range(&mut output, token.range);
            output.push('}');
        } else {
            output.push_str("null");
        }
        output.push_str(",\"reference\":");
        write_json_reference(&mut output, self.reference.as_ref());
        output.push_str(",\"inferred_type\":");
        write_json_optional_string(&mut output, self.inferred_type.as_deref());
        output.push_str(",\"value_properties\":");
        if let Some(properties) = self.value_properties {
            write!(
                output,
                "{{\"equatable\":\"{}\",\"ordered\":\"{}\"}}",
                status_name(properties.equatable),
                status_name(properties.ordered)
            )
            .expect("writing JSON to a string cannot fail");
        } else {
            output.push_str("null");
        }
        output.push_str(",\"callable\":");
        write_json_callable(&mut output, self.callable.as_ref());
        output.push_str(",\"need\":");
        match &self.need {
            Some(need) => write_json_need(&mut output, need),
            None => output.push_str("null"),
        }
        output.push_str(",\"executable_origin\":");
        if let Some(origin) = self.executable_origin {
            write!(output, "{origin}").expect("writing JSON to a string cannot fail");
        } else {
            output.push_str("null");
        }
        output.push_str(",\"clause\":");
        if let Some(clause) = self.clause {
            write!(
                output,
                "{{\"index\":{},\"count\":{}}}",
                clause.index, clause.count
            )
            .expect("writing JSON to a string cannot fail");
        } else {
            output.push_str("null");
        }
        writeln!(output, ",\"diagnostic_count\":{}}}", self.diagnostics.len())
            .expect("writing JSON to a string cannot fail");
        output
    }

    pub(crate) const fn semantic_location(&self) -> Option<(ModuleId, SemanticId)> {
        match self.semantic_id {
            Some(semantic_id) => Some((self.module_id, semantic_id)),
            None => None,
        }
    }

    pub(crate) fn set_executable_origin(&mut self, origin: Option<u32>) {
        self.executable_origin = origin;
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectionError {
    message: String,
}

impl fmt::Display for InspectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for InspectionError {}

pub(crate) fn inspect(
    typed: &TypedProject,
    module_path: &str,
    offset: usize,
) -> Result<InspectionReport, InspectionError> {
    let module = typed
        .resolved
        .semantic
        .modules
        .iter()
        .find(|module| module.path == module_path)
        .ok_or_else(|| InspectionError {
            message: format!("cannot inspect unknown module `{module_path}`"),
        })?;
    if offset > module.source.text().len() {
        return Err(InspectionError {
            message: format!(
                "inspection offset {offset} exceeds module length {}",
                module.source.text().len()
            ),
        });
    }
    if !module.source.text().is_char_boundary(offset) {
        return Err(InspectionError {
            message: format!("inspection offset {offset} splits a UTF-8 code point"),
        });
    }

    let (syntax_path, token) = inspect_syntax(&module.syntax, offset);
    let reference = reference_at(typed, module, offset);
    let expression = expression_at(module, offset);
    let semantic_id = reference
        .map(|reference| reference.semantic_id)
        .or_else(|| expression.map(ModuleExpression::semantic_id));
    let expression_fact =
        semantic_id.and_then(|semantic_id| typed.expression_facts.get(&(module.id, semantic_id)));
    let pattern_binding_type = expression
        .and_then(|expression| pattern_binding_at(expression, offset))
        .and_then(|semantic_id| typed.pattern_binding_types.get(&(module.id, semantic_id)));
    let inspected_type = pattern_binding_type
        .copied()
        .or_else(|| expression_fact.map(|fact| fact.success));
    let inferred_type = inspected_type.map(|type_id| typed.format_type(type_id));
    let value_properties = inspected_type.map(|type_id| ValuePropertyInspection {
        equatable: property_status(typed.derived_property(type_id, ValueProperty::Equatable)),
        ordered: property_status(typed.derived_property(type_id, ValueProperty::Ordered)),
    });
    let target_symbol = reference.and_then(reference_target);
    let need = target_symbol
        .filter(|symbol| typed.resolved.symbol(*symbol).kind == ProjectSymbolKind::Requirement)
        .zip(inspected_type)
        .map(|(symbol, type_id)| inspect_need(typed, &typed.resolved.symbol(symbol).name, type_id));
    let instantiated_callable = semantic_id
        .and_then(|semantic_id| typed.callee_instantiations.get(&(module.id, semantic_id)))
        .and_then(|type_id| typed.callable_contract_for_type(*type_id))
        .map(|contract| inspect_callable(typed, contract));
    // A reference whose typed value is not callable (a fieldless variant such
    // as `None` named as a value) shows its value type, not its constructor.
    let referenced_value_is_callable =
        expression_fact.is_none_or(|fact| typed.callable_contract_for_type(fact.success).is_some());
    let callable_contract = target_symbol
        .filter(|_| referenced_value_is_callable)
        .and_then(|symbol| typed.contracts.get(&symbol))
        .or_else(|| {
            expression_fact.and_then(|fact| typed.callable_contract_for_type(fact.success))
        });
    // A clause head names the whole group: report its contract and the position.
    // An ordinary function's declared name names it the same way.
    let clause = clause_at(module, offset);
    let callable_contract = callable_contract.or_else(|| {
        let declaration = clause
            .map(|(declaration, _)| declaration)
            .or_else(|| function_named_at(module, offset))?;
        let symbol = typed.resolved.source_symbol(
            module.id,
            declaration.semantic_id,
            ProjectSymbolKind::Function,
            &declaration.name.text,
        )?;
        typed.contracts.get(&symbol)
    });
    let (line, byte_column) = line_byte_column(module.source.text(), offset);

    Ok(InspectionReport {
        module_path: module.path.clone(),
        source_path: module.source.path().to_owned(),
        offset: u32::try_from(offset).expect("source offsets fit in u32"),
        line,
        byte_column,
        syntax_path,
        token,
        reference: reference.map(|reference| inspect_reference(typed, reference)),
        inferred_type,
        value_properties,
        callable: callable_contract.map(|contract| inspect_callable(typed, contract)),
        instantiated_callable,
        need,
        executable_origin: None,
        clause: clause.map(|(_, position)| position),
        diagnostics: typed.diagnostics.clone(),
        semantic_id,
        module_id: module.id,
    })
}

/// The equatable and ordered properties of an inspected value's type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValuePropertyInspection {
    pub equatable: ValuePropertyStatus,
    pub ordered: ValuePropertyStatus,
}

const fn property_status(derivation: PropertyDerivation) -> ValuePropertyStatus {
    match derivation {
        PropertyDerivation::Always => ValuePropertyStatus::Always,
        PropertyDerivation::Never => ValuePropertyStatus::Never,
        PropertyDerivation::ValueDependent => ValuePropertyStatus::ValueDependent,
    }
}

const fn status_name(status: ValuePropertyStatus) -> &'static str {
    match status {
        ValuePropertyStatus::Always => "always",
        ValuePropertyStatus::Never => "never",
        ValuePropertyStatus::ValueDependent => "value_dependent",
    }
}

fn inspect_syntax(
    parsed: &ParseResult,
    offset: usize,
) -> (Vec<SyntaxFrame>, Option<InspectedToken>) {
    let mut path = Vec::new();
    let mut current = parsed.root();
    loop {
        path.push(SyntaxFrame {
            kind: current.kind().name(),
            range: current.range(),
        });
        let Some(child) = current
            .child_nodes()
            .into_iter()
            .find(|child| range_contains(child.range(), offset))
        else {
            break;
        };
        current = child;
    }
    let token = parsed
        .root()
        .descendant_tokens()
        .into_iter()
        .find(|token| range_contains(token.range(), offset))
        .map(|token| InspectedToken {
            kind: token.kind().name(),
            text: token.text().to_owned(),
            range: token.range(),
        });
    (path, token)
}

fn reference_at<'typed>(
    typed: &'typed TypedProject,
    module: &SemanticModule,
    offset: usize,
) -> Option<&'typed ProjectReference> {
    typed
        .resolved
        .references
        .iter()
        .filter(|reference| {
            reference.file_id == module.source.file_id()
                && range_contains(reference.range, offset)
                && !is_generated_name(&reference.text)
        })
        .min_by_key(|reference| reference.range.end() - reference.range.start())
}

fn reference_target(reference: &ProjectReference) -> Option<SymbolId> {
    match reference.resolution {
        ReferenceResolution::Resolved(symbol) | ReferenceResolution::Inaccessible(symbol) => {
            Some(symbol)
        }
        ReferenceResolution::Unresolved
        | ReferenceResolution::Ambiguous
        | ReferenceResolution::DeferredToType => None,
    }
}

fn inspect_reference(typed: &TypedProject, reference: &ProjectReference) -> ReferenceInspection {
    ReferenceInspection {
        text: reference.text.clone(),
        namespace: reference.namespace.name(),
        status: reference.resolution.name(),
        range: reference.range,
        target: reference_target(reference).map(|target| inspect_symbol(typed, target)),
    }
}

fn inspect_symbol(typed: &TypedProject, symbol_id: SymbolId) -> SymbolInspection {
    let symbol = typed.resolved.symbol(symbol_id);
    let module = symbol
        .module_id
        .map(|module_id| &typed.resolved.semantic.modules[module_id.raw() as usize]);
    SymbolInspection {
        name: symbol.name.clone(),
        namespace: symbol.namespace.name(),
        kind: symbol.kind.name(),
        module_path: module.map(|module| module.path.clone()),
        source_path: module.map(|module| module.source.path().to_owned()),
        declaration_range: module.and_then(|module| {
            symbol
                .semantic_id
                .and_then(|semantic_id| declaration_range(module, semantic_id))
        }),
        runner: host_supply(typed, symbol_id),
    }
}

fn inspect_need(typed: &TypedProject, name: &str, interface: TypeId) -> NeedInspection {
    NeedInspection {
        name: name.to_owned(),
        interface: typed.format_type(interface),
        runner: match &typed.types[interface.raw() as usize] {
            CanonicalType::Named { symbol, arguments } if arguments.is_empty() => {
                host_supply(typed, *symbol)
            }
            _ => None,
        },
    }
}

fn inspect_callable(typed: &TypedProject, contract: &CallableContract) -> CallableInspection {
    let failures = &typed.failure_rows[contract.failures.raw() as usize];
    let needs = &typed.needs_rows[contract.needs.raw() as usize];
    CallableInspection {
        display: typed.format_function_contract(contract),
        inputs: contract
            .inputs
            .iter()
            .map(|input| typed.format_type(*input))
            .collect(),
        success: typed.format_type(contract.success),
        failures: failures
            .variants
            .iter()
            .map(|symbol| typed.resolved.symbol(*symbol).name.clone())
            .collect(),
        failure_parameters: failures
            .parameters
            .iter()
            .map(|symbol| typed.format_failure_parameter(failures, *symbol))
            .collect(),
        needs: needs
            .entries
            .iter()
            .map(|need| inspect_need(typed, &need.lexical_name, need.interface))
            .collect(),
        needs_parameters: needs
            .parameters
            .iter()
            .map(|symbol| typed.resolved.symbol(*symbol).name.clone())
            .collect(),
    }
}

fn declaration_range(module: &SemanticModule, semantic_id: SemanticId) -> Option<TextRange> {
    for declaration in &module.declarations {
        if declaration.semantic_id == semantic_id {
            return Some(declaration.name.range);
        }
        for variant in &declaration.variants {
            if variant.semantic_id == semantic_id {
                return Some(variant.name.range);
            }
            if let Some(field) = variant
                .fields
                .iter()
                .find(|field| field.semantic_id == semantic_id)
            {
                return Some(field.name.range);
            }
        }
        if let Some(field) = declaration
            .fields
            .iter()
            .find(|field| field.semantic_id == semantic_id)
        {
            return Some(field.name.range);
        }
    }
    None
}

/// The clause group declaration and clause position whose head, `name(patterns)`,
/// holds `offset`.
fn clause_at(
    module: &SemanticModule,
    offset: usize,
) -> Option<(&ModuleDeclaration, ClausePosition)> {
    module.declarations.iter().find_map(|declaration| {
        let Some(ModuleExpression::Case {
            arms,
            origin: CaseOrigin::Clauses { .. },
            ..
        }) = &declaration.body
        else {
            return None;
        };
        let index = arms
            .iter()
            .position(|arm| range_contains(arm.range, offset))?;
        Some((
            declaration,
            ClausePosition {
                index: index + 1,
                count: arms.len(),
            },
        ))
    })
}

/// The function declaration whose name is at `offset`.
fn function_named_at(module: &SemanticModule, offset: usize) -> Option<&ModuleDeclaration> {
    module.declarations.iter().find(|declaration| {
        declaration.kind == ModuleDeclarationKind::Function
            && range_contains(declaration.name.range, offset)
    })
}

fn expression_at(module: &SemanticModule, offset: usize) -> Option<&ModuleExpression> {
    module
        .declarations
        .iter()
        .filter_map(|declaration| declaration.body.as_ref())
        .filter_map(|expression| expression_at_recursive(expression, offset))
        .min_by_key(|expression| expression.range().end() - expression.range().start())
}

fn expression_at_recursive(
    expression: &ModuleExpression,
    offset: usize,
) -> Option<&ModuleExpression> {
    if !range_contains(expression.range(), offset)
        || matches!(expression, ModuleExpression::Name { name, .. } if is_generated_name(&name.text))
    {
        return None;
    }
    expression_children(expression)
        .into_iter()
        .filter_map(|child| expression_at_recursive(child, offset))
        .min_by_key(|child| child.range().end() - child.range().start())
        .or(Some(expression))
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
        ModuleExpression::UsingRecord {
            expression, record, ..
        } => vec![record, expression],
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

/// The binding pattern under `offset` in an arm of `expression`, when it is a `case`
/// whose patterns, not its subjects or bodies, hold `offset`.
fn pattern_binding_at(expression: &ModuleExpression, offset: usize) -> Option<SemanticId> {
    fn binding_in(pattern: &Pattern, offset: usize) -> Option<SemanticId> {
        if !range_contains(pattern.range(), offset) {
            return None;
        }
        match pattern {
            Pattern::Binding { semantic_id, .. } => Some(*semantic_id),
            _ => pattern
                .children()
                .into_iter()
                .find_map(|child| binding_in(child, offset)),
        }
    }
    let ModuleExpression::Case { arms, .. } = expression else {
        return None;
    };
    arms.iter()
        .flat_map(|arm| &arm.patterns)
        .find_map(|pattern| binding_in(pattern, offset))
}

fn range_contains(range: TextRange, offset: usize) -> bool {
    let offset = u32::try_from(offset).expect("source offsets fit in u32");
    range.start() <= offset
        && (offset < range.end() || range.start() == range.end() && offset == range.start())
}

fn line_byte_column(source: &str, offset: usize) -> (u32, u32) {
    let prefix = &source[..offset];
    let line = u32::try_from(prefix.bytes().filter(|byte| *byte == b'\n').count() + 1)
        .expect("source line count fits in u32");
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let column = u32::try_from(offset - line_start + 1).expect("source column fits in u32");
    (line, column)
}

fn write_json_reference(output: &mut String, reference: Option<&ReferenceInspection>) {
    let Some(reference) = reference else {
        output.push_str("null");
        return;
    };
    output.push_str("{\"text\":");
    write_json_string_escaping_controls(output, &reference.text);
    output.push_str(",\"namespace\":");
    write_json_string_escaping_controls(output, reference.namespace);
    output.push_str(",\"status\":");
    write_json_string_escaping_controls(output, reference.status);
    output.push_str(",\"range\":");
    write_json_range(output, reference.range);
    output.push_str(",\"target\":");
    if let Some(target) = &reference.target {
        output.push_str("{\"name\":");
        write_json_string_escaping_controls(output, &target.name);
        output.push_str(",\"namespace\":");
        write_json_string_escaping_controls(output, target.namespace);
        output.push_str(",\"kind\":");
        write_json_string_escaping_controls(output, target.kind);
        output.push_str(",\"module\":");
        write_json_optional_string(output, target.module_path.as_deref());
        output.push_str(",\"path\":");
        let path = target
            .source_path
            .as_ref()
            .map(|path| path.to_string_lossy());
        write_json_optional_string(output, path.as_deref());
        output.push_str(",\"declaration_range\":");
        if let Some(range) = target.declaration_range {
            write_json_range(output, range);
        } else {
            output.push_str("null");
        }
        output.push_str(",\"runner\":");
        write_json_host_supply(output, target.runner, None);
        output.push('}');
    } else {
        output.push_str("null");
    }
    output.push('}');
}

fn write_json_need(output: &mut String, need: &NeedInspection) {
    output.push_str("{\"name\":");
    write_json_string_escaping_controls(output, &need.name);
    output.push_str(",\"interface\":");
    write_json_string_escaping_controls(output, &need.interface);
    output.push_str(",\"runner\":");
    write_json_host_supply(output, need.runner, Some(need.supplied_on_main()));
    output.push('}');
}

/// `null`, or the runner's supply of a host interface; a need's adds whether
/// the runner closes that need on `main`.
fn write_json_host_supply(output: &mut String, supply: Option<HostSupply>, on_main: Option<bool>) {
    let Some(supply) = supply else {
        output.push_str("null");
        return;
    };
    output.push_str("{\"fixed_name\":");
    write_json_optional_string(output, supply.fixed_name());
    output.push_str(",\"in_tests\":");
    write_json_string_escaping_controls(output, supply.in_tests());
    if let Some(on_main) = on_main {
        write!(output, ",\"supplied_on_main\":{on_main}")
            .expect("writing JSON to a string cannot fail");
    }
    output.push('}');
}

fn write_json_callable(output: &mut String, callable: Option<&CallableInspection>) {
    let Some(callable) = callable else {
        output.push_str("null");
        return;
    };
    output.push_str("{\"display\":");
    write_json_string_escaping_controls(output, &callable.display);
    output.push_str(",\"inputs\":");
    write_json_strings(output, &callable.inputs);
    output.push_str(",\"success\":");
    write_json_string_escaping_controls(output, &callable.success);
    output.push_str(",\"failures\":");
    write_json_strings(output, &callable.failures);
    output.push_str(",\"failure_parameters\":");
    write_json_strings(output, &callable.failure_parameters);
    output.push_str(",\"needs\":[");
    for (index, need) in callable.needs.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        write_json_need(output, need);
    }
    output.push_str("],\"needs_parameters\":");
    write_json_strings(output, &callable.needs_parameters);
    output.push('}');
}

fn write_json_strings(output: &mut String, values: &[String]) {
    output.push('[');
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        write_json_string_escaping_controls(output, value);
    }
    output.push(']');
}

fn write_json_optional_string(output: &mut String, value: Option<&str>) {
    if let Some(value) = value {
        write_json_string_escaping_controls(output, value);
    } else {
        output.push_str("null");
    }
}

fn write_json_range(output: &mut String, range: TextRange) {
    write!(
        output,
        "{{\"start\":{},\"end\":{}}}",
        range.start(),
        range.end()
    )
    .expect("writing JSON to a string cannot fail");
}
