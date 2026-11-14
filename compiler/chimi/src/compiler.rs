use crate::backend::BackendProgram;
use crate::diagnostic::Diagnostic;
use crate::executable::{lower, lower_legacy, seal_entry, seal_legacy_entry};
use crate::execution::{ExecutionConfig, ExecutionReport, execute, execute_legacy};
use crate::formatter::FormatReport;
use crate::ids::{SymbolId, TypeId};
use crate::inspection::{InspectionError, InspectionReport, inspect};
use crate::resolution::{ReferenceResolution, ResolvedProject, resolve, resolve_project};
use crate::semantic;
use crate::source::SourceSnapshot;
use crate::syntax::ParseResult;
use crate::typing::{
    CapabilityClass, GenericKind, PropertyDerivation, TypedProject, ValueProperty, check,
    check_project,
};
use std::sync::Arc;

mod session;
pub use session::{
    CompilerSession, IncrementalStatistics, IncrementalTypeReport, ModuleDiagnostics,
};

#[derive(Clone, Debug)]
pub struct ProjectSource {
    module_path: String,
    source: SourceSnapshot,
    syntax: Option<Arc<ParseResult>>,
}

impl ProjectSource {
    pub fn new(
        module_path: impl Into<String>,
        source: SourceSnapshot,
    ) -> Result<Self, ProjectPathError> {
        let module_path = module_path.into();
        if !Self::is_valid_path(&module_path) {
            return Err(ProjectPathError { module_path });
        }
        Ok(Self {
            module_path,
            source,
            syntax: None,
        })
    }

    pub(crate) fn parsed(
        module_path: impl Into<String>,
        source: SourceSnapshot,
    ) -> Result<Self, ProjectPathError> {
        let mut source = Self::new(module_path, source)?;
        source.syntax = Some(Arc::new(crate::syntax::parse(&source.source)));
        Ok(source)
    }

    pub fn module_path(&self) -> &str {
        &self.module_path
    }

    pub(crate) fn source(&self) -> &SourceSnapshot {
        &self.source
    }

    pub(crate) fn syntax(&self) -> Option<&Arc<ParseResult>> {
        self.syntax.as_ref()
    }

    pub(crate) fn is_valid_path(module_path: &str) -> bool {
        !module_path.is_empty()
            && !module_path.starts_with('/')
            && !module_path.ends_with('/')
            && module_path.split('/').all(|part| {
                !part.is_empty()
                    && part != "."
                    && part != ".."
                    && part
                        .chars()
                        .all(|character| character == '_' || character.is_ascii_alphanumeric())
            })
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct ProjectPathError {
    module_path: String,
}

impl ProjectPathError {
    pub(crate) fn new(module_path: String) -> Self {
        Self { module_path }
    }
}

impl std::fmt::Display for ProjectPathError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "invalid workspace-relative module path `{}`",
            self.module_path
        )
    }
}

impl std::error::Error for ProjectPathError {}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SymbolHandle(SymbolId);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolInfo {
    handle: SymbolHandle,
    module_path: Option<String>,
    name: String,
    namespace: &'static str,
    kind: &'static str,
    public: bool,
    source_backed: bool,
}

impl SymbolInfo {
    pub const fn handle(&self) -> SymbolHandle {
        self.handle
    }

    pub fn module_path(&self) -> Option<&str> {
        self.module_path.as_deref()
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

    pub const fn is_public(&self) -> bool {
        self.public
    }

    pub const fn is_source_backed(&self) -> bool {
        self.source_backed
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferenceInfo {
    module_path: String,
    text: String,
    namespace: &'static str,
    status: &'static str,
    target: Option<SymbolHandle>,
    target_module_path: Option<String>,
    range: crate::ids::TextRange,
}

impl ReferenceInfo {
    pub fn module_path(&self) -> &str {
        &self.module_path
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub const fn namespace(&self) -> &'static str {
        self.namespace
    }

    pub const fn status(&self) -> &'static str {
        self.status
    }

    pub const fn target(&self) -> Option<SymbolHandle> {
        self.target
    }

    pub fn target_module_path(&self) -> Option<&str> {
        self.target_module_path.as_deref()
    }

    pub const fn range(&self) -> crate::ids::TextRange {
        self.range
    }
}

#[derive(Debug)]
pub struct NameReport {
    resolved: ResolvedProject,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenericParameterKind {
    Unconstrained,
    Value,
    FailureRow,
    FailureSet,
    NeedsRow,
    Conflicting,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TypeArenaStats {
    pub types: usize,
    pub failure_rows: usize,
    pub needs_rows: usize,
    pub callable_contracts: usize,
    pub type_facts: usize,
    pub expression_facts: usize,
    pub declaration_types: usize,
    pub field_shapes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValuePropertyStatus {
    Always,
    Never,
    ValueDependent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueCapabilityClass {
    Ordinary,
    Exclusive,
    ParentLocal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValuePropertyReport {
    pub transferable: ValuePropertyStatus,
    pub shareable: ValuePropertyStatus,
    pub capability: ValueCapabilityClass,
    pub equatable: ValuePropertyStatus,
    pub ordered: ValuePropertyStatus,
}

#[derive(Debug)]
pub struct TypeReport {
    typed: Arc<TypedProject>,
}

impl TypeReport {
    pub fn is_success(&self) -> bool {
        self.typed.diagnostics.is_empty()
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.typed.diagnostics
    }

    /// Warnings, such as `T4312`. They are never part of `diagnostics` and never
    /// affect `is_success`.
    pub fn warnings(&self) -> &[Diagnostic] {
        &self.typed.warnings
    }

    pub fn contract(&self, module_path: &str, function_name: &str) -> Option<String> {
        self.typed
            .contract(module_path, function_name)
            .map(|contract| self.typed.format_function_contract(contract))
    }

    /// The rendered contract of a standard function named as written in source,
    /// such as `Process.send`, or `None` when no standard module has it.
    pub fn standard_contract(&self, qualified_name: &str) -> Option<String> {
        let (qualifier, name) = qualified_name.split_once('.')?;
        let function = crate::standard::StandardFunction::ALL
            .into_iter()
            .find(|function| {
                function.name() == name
                    && function
                        .module()
                        .is_some_and(|module| module.qualifier() == qualifier)
            })?;
        let symbol = self.typed.resolved.symbols.iter().find(|symbol| {
            self.typed.resolved.standard_symbol(symbol.id)
                == Some(crate::standard::StandardSymbol::Function(function))
        })?;
        self.typed
            .contracts
            .get(&symbol.id)
            .map(|contract| self.typed.format_function_contract(contract))
    }

    pub fn generic_kind(
        &self,
        module_path: &str,
        declaration_name: &str,
        generic_name: &str,
    ) -> Option<GenericParameterKind> {
        self.typed
            .generic_kind(module_path, declaration_name, generic_name)
            .map(|kind| match kind {
                GenericKind::Unconstrained => GenericParameterKind::Unconstrained,
                GenericKind::Value => GenericParameterKind::Value,
                GenericKind::FailureRow => GenericParameterKind::FailureRow,
                GenericKind::FailureSet => GenericParameterKind::FailureSet,
                GenericKind::NeedsRow => GenericParameterKind::NeedsRow,
                GenericKind::Conflicting => GenericParameterKind::Conflicting,
            })
    }

    pub fn arena_stats(&self) -> TypeArenaStats {
        TypeArenaStats {
            types: self.typed.types.len(),
            failure_rows: self.typed.failure_rows.len(),
            needs_rows: self.typed.needs_rows.len(),
            callable_contracts: self.typed.contracts.len(),
            type_facts: self.typed.type_facts.len(),
            expression_facts: self.typed.expression_facts.len(),
            declaration_types: self.typed.declaration_types.len(),
            field_shapes: self.typed.fields.values().map(Vec::len).sum(),
        }
    }

    pub fn function_input_value_properties(
        &self,
        module_path: &str,
        function_name: &str,
        input: usize,
    ) -> Option<ValuePropertyReport> {
        let type_id = *self
            .typed
            .contract(module_path, function_name)?
            .inputs
            .get(input)?;
        Some(value_property_report(&self.typed, type_id))
    }

    pub fn function_success_value_properties(
        &self,
        module_path: &str,
        function_name: &str,
    ) -> Option<ValuePropertyReport> {
        let type_id = self.typed.contract(module_path, function_name)?.success;
        Some(value_property_report(&self.typed, type_id))
    }

    pub fn render_diagnostics(&self) -> String {
        render_project_diagnostics(&self.typed.resolved, &self.typed.diagnostics)
    }
}

fn value_property_report(typed: &TypedProject, type_id: TypeId) -> ValuePropertyReport {
    let properties = typed.value_properties(type_id);
    let property = |value| match value {
        PropertyDerivation::Always => ValuePropertyStatus::Always,
        PropertyDerivation::Never => ValuePropertyStatus::Never,
        PropertyDerivation::ValueDependent => ValuePropertyStatus::ValueDependent,
    };
    ValuePropertyReport {
        transferable: property(properties.transferable),
        shareable: property(properties.shareable),
        capability: match properties.capability {
            CapabilityClass::Ordinary => ValueCapabilityClass::Ordinary,
            CapabilityClass::Exclusive => ValueCapabilityClass::Exclusive,
            CapabilityClass::ParentLocal => ValueCapabilityClass::ParentLocal,
        },
        equatable: property(typed.derived_property(type_id, ValueProperty::Equatable)),
        ordered: property(typed.derived_property(type_id, ValueProperty::Ordered)),
    }
}

impl NameReport {
    pub fn is_success(&self) -> bool {
        self.resolved.diagnostics.is_empty()
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.resolved.diagnostics
    }

    pub fn dependency_order(&self) -> Vec<&str> {
        self.resolved
            .dependency_order
            .iter()
            .map(|module_id| {
                self.resolved.semantic.modules[module_id.raw() as usize]
                    .path
                    .as_str()
            })
            .collect()
    }

    pub fn symbols(&self) -> Vec<SymbolInfo> {
        self.resolved
            .symbols
            .iter()
            .map(|symbol| SymbolInfo {
                handle: SymbolHandle(symbol.id),
                module_path: symbol.module_id.map(|module_id| {
                    self.resolved.semantic.modules[module_id.raw() as usize]
                        .path
                        .clone()
                }),
                name: symbol.name.clone(),
                namespace: symbol.namespace.name(),
                kind: symbol.kind.name(),
                public: symbol.public,
                source_backed: symbol.semantic_id.is_some(),
            })
            .collect()
    }

    pub fn references(&self) -> Vec<ReferenceInfo> {
        self.resolved
            .references
            .iter()
            .map(|reference| {
                let module_path = self.resolved.semantic.modules[reference.file_id.raw() as usize]
                    .path
                    .clone();
                let (status, target) = match reference.resolution {
                    ReferenceResolution::Resolved(id) => ("resolved", Some(id)),
                    ReferenceResolution::Unresolved => ("unresolved", None),
                    ReferenceResolution::Ambiguous => ("ambiguous", None),
                    ReferenceResolution::Inaccessible(id) => ("inaccessible", Some(id)),
                    ReferenceResolution::DeferredToType => ("deferred-to-type", None),
                };
                let target_module_path = target.and_then(|id| {
                    self.resolved.symbol(id).module_id.map(|module_id| {
                        self.resolved.semantic.modules[module_id.raw() as usize]
                            .path
                            .clone()
                    })
                });
                ReferenceInfo {
                    module_path,
                    text: reference.text.clone(),
                    namespace: reference.namespace.name(),
                    status,
                    target: target.map(SymbolHandle),
                    target_module_path,
                    range: reference.range,
                }
            })
            .collect()
    }

    pub fn render_diagnostics(&self) -> String {
        render_project_diagnostics(&self.resolved, &self.resolved.diagnostics)
    }
}

fn render_project_diagnostics(resolved: &ResolvedProject, diagnostics: &[Diagnostic]) -> String {
    let mut output = diagnostics
        .iter()
        .map(|diagnostic| {
            let source = &resolved.semantic.modules[diagnostic.file_id().raw() as usize].source;
            diagnostic.render(source)
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !output.is_empty() {
        output.push('\n');
    }
    output
}

#[derive(Debug)]
pub struct CheckReport {
    source: Arc<SourceSnapshot>,
    diagnostics: Vec<Diagnostic>,
}

impl CheckReport {
    fn success(source: Arc<SourceSnapshot>) -> Self {
        Self {
            source,
            diagnostics: Vec::new(),
        }
    }

    fn failure(source: Arc<SourceSnapshot>, diagnostics: Vec<Diagnostic>) -> Self {
        debug_assert!(!diagnostics.is_empty());
        Self {
            source,
            diagnostics,
        }
    }

    pub fn is_success(&self) -> bool {
        self.diagnostics.is_empty()
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn render_diagnostics(&self) -> String {
        let mut output = self
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.render(&self.source))
            .collect::<Vec<_>>()
            .join("\n");
        if !output.is_empty() {
            output.push('\n');
        }
        output
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerificationStats {
    pub reachable_declarations: usize,
    pub origins: usize,
    pub reachable_modules: usize,
    /// Diagnostics with `Severity::Warning`; they never make verification fail.
    pub warnings: Vec<Diagnostic>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoreStats {
    pub functions: usize,
    pub module_constants: usize,
    pub nodes: usize,
    pub origins: usize,
    pub constants: usize,
    pub constructions: usize,
    pub calls: usize,
    pub sequences: usize,
    pub dependencies: usize,
}

pub struct Compiler;

impl Compiler {
    pub fn format(source: SourceSnapshot) -> FormatReport {
        FormatReport::new(source)
    }

    pub fn inspect_project(
        sources: Vec<ProjectSource>,
        module_path: &str,
        offset: usize,
    ) -> Result<InspectionReport, InspectionError> {
        let typed = check_project(resolve_project_sources(sources));
        let mut report = inspect(&typed, module_path, offset)?;
        let location = report.semantic_location();
        if let Ok(root) = seal_entry(typed) {
            let origin = location
                .and_then(|(module_id, semantic_id)| root.origin_id(module_id, semantic_id));
            report.set_executable_origin(origin);
        }
        Ok(report)
    }

    pub fn resolve_project(sources: Vec<ProjectSource>) -> NameReport {
        NameReport {
            resolved: resolve_project_sources(sources),
        }
    }

    pub fn type_project(sources: Vec<ProjectSource>) -> TypeReport {
        TypeReport {
            typed: Arc::new(check_project(resolve_project_sources(sources))),
        }
    }

    pub fn verify_project(
        sources: Vec<ProjectSource>,
    ) -> Result<VerificationStats, Vec<Diagnostic>> {
        let root = seal_entry(check_project(resolve_project_sources(sources)))?;
        debug_assert!(root.typed.contracts.contains_key(&root.entry));
        let (reachable_declarations, origins, reachable_modules) = root.verification_stats();
        let warnings = root.typed.warnings.clone();
        // Constants are evaluated as part of the build, so checking evaluates them too:
        // a constant that cannot be built is a compile error, not a build-time surprise.
        lower(root)?;
        Ok(VerificationStats {
            reachable_declarations,
            origins,
            reachable_modules,
            warnings,
        })
    }

    pub fn lower_project(sources: Vec<ProjectSource>) -> Result<CoreStats, Vec<Diagnostic>> {
        let core = lower(seal_entry(check_project(resolve_project_sources(sources)))?)?;
        let (
            functions,
            module_constants,
            nodes,
            origins,
            constants,
            constructions,
            calls,
            sequences,
            dependencies,
        ) = core.stats();
        Ok(CoreStats {
            functions,
            module_constants,
            nodes,
            origins,
            constants,
            constructions,
            calls,
            sequences,
            dependencies,
        })
    }

    pub fn prepare_backend(sources: Vec<ProjectSource>) -> Result<BackendProgram, Vec<Diagnostic>> {
        let core = lower(seal_entry(check_project(resolve_project_sources(sources)))?)?;
        Ok(BackendProgram::new(core))
    }

    pub fn run_project(
        sources: Vec<ProjectSource>,
        config: ExecutionConfig,
    ) -> Result<ExecutionReport, Vec<Diagnostic>> {
        let core = lower(seal_entry(check_project(resolve_project_sources(sources)))?)?;
        Ok(execute(&core, config))
    }

    pub fn check(source: SourceSnapshot) -> CheckReport {
        let source = Arc::new(source);
        match seal(source.clone()) {
            Ok(_) => CheckReport::success(source),
            Err(diagnostics) => CheckReport::failure(source, diagnostics),
        }
    }

    pub fn run(
        source: SourceSnapshot,
        config: ExecutionConfig,
    ) -> Result<ExecutionReport, CheckReport> {
        let source = Arc::new(source);
        let verified = seal(source.clone())
            .map_err(|diagnostics| CheckReport::failure(source, diagnostics))?;
        let core = lower_legacy(verified);
        Ok(execute_legacy(core, config))
    }
}

pub(crate) fn resolve_project_sources(mut sources: Vec<ProjectSource>) -> ResolvedProject {
    crate::standard_tree::add_imported_modules(&mut sources);
    let semantic = semantic::lower_project(
        sources
            .into_iter()
            .map(|source| semantic::ModuleSource {
                path: source.module_path,
                source: source.source,
                syntax: source.syntax,
            })
            .collect(),
    );
    resolve_project(semantic)
}

fn seal(
    source: Arc<SourceSnapshot>,
) -> Result<crate::executable::LegacyVerifiedRoot, Vec<Diagnostic>> {
    let semantic = semantic::lower(source);
    let resolved = resolve(semantic);
    let typed = check(resolved);
    seal_legacy_entry(typed)
}

#[cfg(test)]
mod tests {
    use super::{
        Compiler, GenericParameterKind, ProjectSource, ValueCapabilityClass, ValuePropertyReport,
        ValuePropertyStatus, resolve_project_sources,
    };
    use crate::diagnostic::{Diagnostic, DiagnosticStage};
    use crate::execution::{
        EventKind, ExecutionAttributeValue, ExecutionConfig, ExecutionOutcome, ExecutionValue,
        TraceMode, ValueField,
    };
    use crate::source::SourceSnapshot;
    use crate::typing::check_project;
    use std::sync::Arc;

    const VALID: &str = "struct Answer { value: Int } pub fn main() { Answer(42) }";

    fn source(text: &str) -> SourceSnapshot {
        SourceSnapshot::from_text("test.chimi", text)
    }

    fn module(path: &str, source_path: &str, text: &str) -> ProjectSource {
        ProjectSource::new(path, SourceSnapshot::from_text(source_path, text))
            .expect("fixture module path is valid")
    }

    fn diagnostic_codes(report: &super::NameReport) -> Vec<&'static str> {
        report.diagnostics().iter().map(Diagnostic::code).collect()
    }

    #[test]
    fn actual_source_executes_through_the_closed_root() {
        let report = Compiler::run(
            source(VALID),
            ExecutionConfig::new("test-execution").unwrap(),
        )
        .expect("valid source executes");

        assert_eq!(report.events.len(), 2);
        assert_eq!(report.events[0].sequence, 0);
        assert_eq!(report.events[1].sequence, 1);
        let expression_start = VALID.find("Answer(42)").unwrap() as u32;
        assert_eq!(report.events[0].source_range.start(), expression_start);
        assert_eq!(
            report.events[0].source_range.end(),
            expression_start + "Answer(42)".len() as u32
        );
        assert_eq!(report.events[0].file_id, 0);
        let ExecutionOutcome::Success(ExecutionValue::Constructed {
            type_name,
            variant_name,
            fields,
        }) = &report.outcome
        else {
            panic!("legacy execution returns its constructed success value")
        };
        assert_eq!(type_name, "Answer");
        assert_eq!(*variant_name, None);
        assert_eq!(fields[0].name, "value");
        assert_eq!(fields[0].value, ExecutionValue::Integer(42));
    }

    #[test]
    fn source_ranges_survive_lossless_cst_lowering() {
        let text = concat!(
            "// retained trivia\r\n",
            "struct Answer {\n  value: Int\n}\n\n",
            "pub fn main() {\n  Answer( 42 ) // retained too\n}\n",
        );
        let report = Compiler::run(
            source(text),
            ExecutionConfig::new("source-range-test").unwrap(),
        )
        .expect("source with trivia executes");

        let expression_start = text.find("Answer( 42 )").unwrap() as u32;
        assert_eq!(report.events[0].source_range.start(), expression_start);
        assert_eq!(
            report.events[0].source_range.end(),
            expression_start + "Answer( 42 )".len() as u32
        );
    }

    #[test]
    fn missing_or_renamed_main_cannot_seal() {
        for text in [
            "struct Answer { value: Int }",
            "struct Answer { value: Int } pub fn start() { Answer(42) }",
        ] {
            let report = Compiler::check(source(text));
            assert_eq!(report.diagnostics().len(), 1);
            assert_eq!(
                report.diagnostics()[0].stage(),
                DiagnosticStage::ExecutionSetup
            );
            assert_eq!(report.diagnostics()[0].code(), "E5001");
        }
    }

    #[test]
    fn unresolved_constructor_stops_at_name_resolution() {
        let text = "struct Answer { value: Int } pub fn main() { Missing(42) }";
        let report = Compiler::check(source(text));

        assert_eq!(report.diagnostics().len(), 1);
        assert_eq!(report.diagnostics()[0].stage(), DiagnosticStage::Name);
        assert_eq!(report.diagnostics()[0].code(), "N3001");
        assert_eq!(
            report.diagnostics()[0].range().start() as usize,
            text.find("Missing").unwrap()
        );
    }

    #[test]
    fn invalid_field_value_stops_at_type_checking() {
        let text = "struct Answer { value: Int } pub fn main() { Answer(\"forty-two\") }";
        let report = Compiler::check(source(text));

        assert_eq!(report.diagnostics().len(), 1);
        assert_eq!(report.diagnostics()[0].stage(), DiagnosticStage::Type);
        assert_eq!(report.diagnostics()[0].code(), "T4001");
        assert_eq!(
            report.diagnostics()[0].range().start() as usize,
            text.find("\"forty-two\"").unwrap()
        );
    }

    #[test]
    fn malformed_source_is_diagnosed_without_panicking() {
        for text in [
            "struct Answer { value: Int } pub fn main() { Answer(999999999999999999999999999999999999) }",
            "struct Answer { value: Int } pub fn main() { Answer(λ) }",
        ] {
            let report = Compiler::check(source(text));
            assert!(!report.is_success());
            assert_eq!(report.diagnostics()[0].stage(), DiagnosticStage::Syntax);
        }
    }

    #[test]
    fn syntax_beyond_the_executable_subset_cannot_silently_seal() {
        let text = concat!(
            "struct Answer { value: Int }\n",
            "struct Other { value: Int }\n",
            "pub fn main() { Answer(42) }\n",
        );
        let report = Compiler::check(source(text));

        assert!(!report.is_success());
        assert_eq!(report.diagnostics()[0].stage(), DiagnosticStage::Lowering);
        assert_eq!(report.diagnostics()[0].code(), "L2001");
    }

    #[test]
    fn complete_name_resolution_corpus_runs_through_the_rust_frontend() {
        const MONEY: &str = include_str!("../../../prototypes/golden/valid/modules/money.chimi");
        const BILLING: &str =
            include_str!("../../../prototypes/name-resolution/modules/billing/money.chimi");
        const LEDGER: &str =
            include_str!("../../../prototypes/name-resolution/modules/ledger/money.chimi");

        let scenarios = [
            (
                "qualified module success",
                vec![
                    module("modules/money", "money.chimi", MONEY),
                    module(
                        "modules/use_money",
                        "use_money.chimi",
                        include_str!("../../../prototypes/golden/valid/modules/use_money.chimi"),
                    ),
                ],
                vec![],
            ),
            (
                "private representation",
                vec![
                    module("modules/money", "money.chimi", MONEY),
                    module(
                        "cases/private_constructor",
                        "private_struct_construction.chimi",
                        include_str!(
                            "../../../prototypes/golden/invalid/private_struct_construction.chimi"
                        ),
                    ),
                ],
                vec!["E_PRIVATE_STRUCT_CONSTRUCTOR"],
            ),
            (
                "public enum constructor",
                vec![
                    module("modules/money", "money.chimi", MONEY),
                    module(
                        "cases/public_enum_constructor",
                        "public_enum_constructor.chimi",
                        include_str!(
                            "../../../prototypes/name-resolution/cases/public_enum_constructor.chimi"
                        ),
                    ),
                ],
                vec![],
            ),
            (
                "explicit constructor import",
                vec![
                    module("modules/money", "money.chimi", MONEY),
                    module(
                        "cases/explicit_constructor_import",
                        "explicit_constructor_import.chimi",
                        include_str!(
                            "../../../prototypes/name-resolution/cases/explicit_constructor_import.chimi"
                        ),
                    ),
                ],
                vec![],
            ),
            (
                "function member import rejected",
                vec![
                    module("modules/money", "money.chimi", MONEY),
                    module(
                        "cases/unqualified_function_import",
                        "unqualified_function_import.chimi",
                        include_str!(
                            "../../../prototypes/name-resolution/cases/unqualified_function_import.chimi"
                        ),
                    ),
                ],
                vec!["E_UNQUALIFIED_FUNCTION_IMPORT"],
            ),
            (
                "no member injection",
                vec![
                    module("modules/money", "money.chimi", MONEY),
                    module(
                        "cases/unqualified_external",
                        "unqualified_external.chimi",
                        include_str!(
                            "../../../prototypes/name-resolution/cases/unqualified_external.chimi"
                        ),
                    ),
                ],
                vec!["E_UNRESOLVED_VALUE"],
            ),
            (
                "import binding collision",
                vec![
                    module("billing/money", "billing_money.chimi", BILLING),
                    module("ledger/money", "ledger_money.chimi", LEDGER),
                    module(
                        "cases/ambiguous_import",
                        "ambiguous_import.chimi",
                        include_str!(
                            "../../../prototypes/name-resolution/cases/ambiguous_import.chimi"
                        ),
                    ),
                ],
                vec!["E_IMPORT_BINDING_CONFLICT"],
            ),
            (
                "explicit module aliases",
                vec![
                    module("billing/money", "billing_money.chimi", BILLING),
                    module("ledger/money", "ledger_money.chimi", LEDGER),
                    module(
                        "cases/aliased_modules",
                        "aliased_modules.chimi",
                        include_str!(
                            "../../../prototypes/name-resolution/cases/aliased_modules.chimi"
                        ),
                    ),
                ],
                vec![],
            ),
            (
                "explicit generic parameters",
                vec![module(
                    "cases/explicit_generics",
                    "explicit_generics.chimi",
                    include_str!(
                        "../../../prototypes/name-resolution/cases/explicit_generics.chimi"
                    ),
                )],
                vec![],
            ),
            (
                "undeclared generic rejected",
                vec![module(
                    "cases/undeclared_generic",
                    "undeclared_generic.chimi",
                    include_str!(
                        "../../../prototypes/name-resolution/cases/undeclared_generic.chimi"
                    ),
                )],
                vec!["E_UNRESOLVED_TYPE", "E_UNRESOLVED_TYPE"],
            ),
        ];

        for (name, sources, expected) in scenarios {
            let report = Compiler::resolve_project(sources);
            assert_eq!(
                diagnostic_codes(&report),
                expected,
                "{name}: {}",
                report.render_diagnostics()
            );
        }
    }

    #[test]
    fn project_symbols_are_deterministic_and_keep_qualified_origin() {
        const MONEY: &str = include_str!("../../../prototypes/golden/valid/modules/money.chimi");
        const USE_MONEY: &str =
            include_str!("../../../prototypes/golden/valid/modules/use_money.chimi");
        let analyze = || {
            Compiler::resolve_project(vec![
                module("modules/use_money", "use_money.chimi", USE_MONEY),
                module("modules/money", "money.chimi", MONEY),
            ])
        };
        let first = analyze();
        let second = analyze();

        assert_eq!(first.symbols(), second.symbols());
        assert_eq!(first.references(), second.references());
        assert_eq!(
            first.dependency_order(),
            ["modules/money", "std/int", "modules/use_money"]
        );
        assert!(first.references().iter().any(|reference| {
            reference.text() == "from_cents"
                && reference.target_module_path() == Some("modules/money")
        }));
    }

    #[test]
    fn invalid_name_references_keep_expected_spans_and_sentinels() {
        const MONEY: &str = include_str!("../../../prototypes/golden/valid/modules/money.chimi");
        const PRIVATE: &str =
            include_str!("../../../prototypes/golden/invalid/private_struct_construction.chimi");
        let private = Compiler::resolve_project(vec![
            module("modules/money", "money.chimi", MONEY),
            module(
                "cases/private_constructor",
                "private_struct_construction.chimi",
                PRIVATE,
            ),
        ]);
        let diagnostic = private
            .diagnostics()
            .iter()
            .find(|diagnostic| diagnostic.code() == "E_PRIVATE_STRUCT_CONSTRUCTOR")
            .expect("private constructor is diagnosed");
        let expected = "Money.Money(-100)";
        let start = PRIVATE.rfind(expected).unwrap() as u32;
        assert_eq!(diagnostic.range().start(), start);
        assert_eq!(diagnostic.range().end(), start + expected.len() as u32);
        assert!(private.references().iter().any(|reference| {
            reference.text() == "Money"
                && reference.status() == "inaccessible"
                && reference.target_module_path() == Some("modules/money")
        }));

        const GENERIC: &str =
            include_str!("../../../prototypes/name-resolution/cases/undeclared_generic.chimi");
        let generic = Compiler::resolve_project(vec![module(
            "cases/undeclared_generic",
            "undeclared_generic.chimi",
            GENERIC,
        )]);
        let parameter = GENERIC.find("value: A").unwrap() + "value: ".len();
        let returned = GENERIC.rfind("-> A").unwrap() + "-> ".len();
        let expected_offsets = vec![parameter as u32, returned as u32];
        let actual_offsets = generic
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.range().start())
            .collect::<Vec<_>>();
        assert_eq!(actual_offsets, expected_offsets);
        assert!(
            generic
                .references()
                .iter()
                .filter(|reference| reference.text() == "A")
                .all(|reference| reference.status() == "unresolved")
        );
    }

    #[test]
    fn project_module_paths_are_validated_at_the_boundary() {
        for invalid in ["", "/money", "money/", "billing/../money", "money.chimi"] {
            assert!(
                ProjectSource::new(invalid, source("")).is_err(),
                "{invalid}"
            );
        }
        assert!(ProjectSource::new("billing/money", source("")).is_ok());
    }

    #[test]
    fn nested_callable_types_guards_and_record_values_resolve_in_lexical_scope() {
        let text = concat!(
            "fn helper(value: Int) { True }\n",
            "fn preserve<A>(value: A) {\n",
            "  let transform = fn(item: A) -> A { item }\n",
            "  case value {\n",
            "    item if helper(1) -> { result: transform(item) }\n",
            "  }\n",
            "}\n",
        );
        let report = Compiler::resolve_project(vec![module(
            "cases/lexical_scope",
            "lexical_scope.chimi",
            text,
        )]);

        assert!(report.is_success(), "{}", report.render_diagnostics());
        for expected in ["helper", "item", "transform", "value"] {
            assert!(report.references().iter().any(|reference| {
                reference.text() == expected && reference.status() == "resolved"
            }));
        }
        assert_eq!(
            report
                .references()
                .iter()
                .filter(|reference| reference.text() == "A")
                .count(),
            3
        );
    }

    #[test]
    fn conflicting_member_imports_do_not_resolve_to_the_first_module() {
        let report = Compiler::resolve_project(vec![
            module(
                "billing/money",
                "billing.chimi",
                "pub struct Money { cents: Int }",
            ),
            module(
                "ledger/money",
                "ledger.chimi",
                "pub struct Money { cents: Int }",
            ),
            module(
                "cases/member_conflict",
                "member_conflict.chimi",
                concat!(
                    "import billing/money.{Money}\n",
                    "import ledger/money.{Money}\n",
                    "fn inspect(value: Money) { value }\n",
                ),
            ),
        ]);

        assert_eq!(
            diagnostic_codes(&report),
            ["E_IMPORT_MEMBER_CONFLICT", "E_AMBIGUOUS_TYPE"]
        );
        assert!(
            report.references().iter().any(|reference| {
                reference.text() == "Money" && reference.status() == "ambiguous"
            })
        );
    }

    #[test]
    fn callable_annotations_intern_all_three_channels_and_infer_generic_kinds() {
        let report = Compiler::type_project(vec![module(
            "contracts/preserve",
            "preserve.chimi",
            include_str!("../../../prototypes/name-resolution/cases/explicit_generics.chimi"),
        )]);

        assert!(report.is_success(), "{}", report.render_diagnostics());
        assert_eq!(
            report.contract("contracts/preserve", "preserve").as_deref(),
            Some("fn(fn(A) -> A errors E needs R) -> fn(A) -> A errors E needs R")
        );
        assert_eq!(
            report.generic_kind("contracts/preserve", "preserve", "A"),
            Some(GenericParameterKind::Value)
        );
        assert_eq!(
            report.generic_kind("contracts/preserve", "preserve", "E"),
            Some(GenericParameterKind::FailureRow)
        );
        assert_eq!(
            report.generic_kind("contracts/preserve", "preserve", "R"),
            Some(GenericParameterKind::NeedsRow)
        );
        let stats = report.arena_stats();
        assert!(stats.types > 1);
        assert!(stats.failure_rows > 1);
        assert!(stats.needs_rows > 1);
        assert!(stats.type_facts >= 6);
    }

    #[test]
    fn function_contract_display_keeps_failure_variants_and_named_needs() {
        let report = Compiler::type_project(vec![module(
            "checkout",
            "checkout.chimi",
            concat!(
                "struct Money { cents: Int }\n",
                "struct Receipt { id: Text }\n",
                "enum PaymentError { Declined(reason: Text), GatewayUnavailable }\n",
                "interface Payment { charge: fn(Money) -> Receipt errors PaymentError }\n",
                "fn checkout(\n",
                "  operation: fn(Money) -> Receipt errors PaymentError,\n",
                "  total: Money,\n",
                ") -> Receipt needs { payment: Payment } {\n",
                "  payment.charge(total)\n",
                "}\n",
            ),
        )]);

        assert!(report.is_success(), "{}", report.render_diagnostics());
        assert_eq!(
            report.contract("checkout", "checkout").as_deref(),
            Some(
                "fn(fn(Money) -> Receipt errors PaymentError, Money) -> Receipt errors PaymentError needs { payment: Payment }"
            )
        );
    }

    #[test]
    fn one_generic_can_project_to_a_failure_row_and_its_value_union() {
        let text = concat!(
            "fn forward<E>(\n",
            "  operation: fn(Int) -> Int errors E,\n",
            "  error: E,\n",
            ") -> Int { Failure(error) }\n",
        );
        let report = Compiler::type_project(vec![module(
            "contracts/failure_set",
            "failure_set.chimi",
            text,
        )]);

        assert!(report.is_success(), "{}", report.render_diagnostics());
        assert_eq!(
            report
                .contract("contracts/failure_set", "forward")
                .as_deref(),
            Some("fn(fn(Int) -> Int errors E, E) -> Int errors E")
        );
        assert_eq!(
            report.generic_kind("contracts/failure_set", "forward", "E"),
            Some(GenericParameterKind::FailureSet)
        );
    }

    #[test]
    fn failure_set_value_arguments_must_match_the_bound_failure_row() {
        let report = Compiler::type_project(vec![module(
            "contracts/failure_set_mismatch",
            "failure_set_mismatch.chimi",
            concat!(
                "enum FirstError { First }\n",
                "enum SecondError { Second }\n",
                "fn forward<E>(operation: fn() -> Int errors E, error: E) -> Int {\n",
                "  operation()\n",
                "}\n",
                "fn operation() -> Int { Failure(First()) }\n",
                "fn mismatch() -> Int { forward(operation, Second()) }\n",
            ),
        )]);

        assert_eq!(
            report
                .diagnostics()
                .iter()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            ["T4203"]
        );
    }

    #[test]
    fn repeated_annotations_share_canonical_types() {
        let report = Compiler::type_project(vec![module(
            "contracts/interning",
            "interning.chimi",
            "fn add(left: Int, right: Int, label: Text) -> Int { left }\n",
        )]);

        assert!(report.is_success(), "{}", report.render_diagnostics());
        let contract = report
            .typed
            .contract("contracts/interning", "add")
            .expect("add has a contract");
        assert_eq!(contract.inputs[0], contract.inputs[1]);
        assert_eq!(contract.inputs[0], contract.success);
        assert_ne!(contract.inputs[0], contract.inputs[2]);
    }

    // An unresolved annotation is reported once per spelling and then poisons
    // its uses: the body and the Int argument produce no further type errors.
    #[test]
    fn unresolved_annotations_still_receive_poison_facts() {
        let source = concat!(
            "fn preserve(value: Missing) -> Missing { value }\n",
            "fn caller(count: Int) -> Int { preserve(count) }\n",
        );
        let sources = vec![module("contracts/poison", "poison.chimi", source)];
        let report = Compiler::type_project(sources.clone());

        assert_eq!(
            report
                .diagnostics()
                .iter()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            ["E_UNRESOLVED_TYPE", "E_UNRESOLVED_TYPE"],
            "{}",
            report.render_diagnostics()
        );
        let inspection = Compiler::inspect_project(
            sources,
            "contracts/poison",
            source.find("{ value }").unwrap() + 2,
        )
        .expect("inspect the body");
        assert_eq!(inspection.token().unwrap().text(), "value");
        assert_eq!(inspection.inferred_type(), Some("<invalid>"));
    }

    #[test]
    fn calls_infer_failure_and_needs_channels_from_function_bodies() {
        let report = Compiler::type_project(vec![module(
            "lookup",
            "lookup.chimi",
            concat!(
                "enum LookupError { Missing }\n",
                "interface Store { lookup: fn(Int) -> Text errors LookupError }\n",
                "fn load(key: Int) -> Text needs { store: Store } {\n",
                "  store.lookup(key)\n",
                "}\n",
            ),
        )]);

        assert!(report.is_success(), "{}", report.render_diagnostics());
        assert_eq!(
            report.contract("lookup", "load").as_deref(),
            Some("fn(Int) -> Text errors LookupError needs { store: Store }")
        );
        assert!(report.arena_stats().expression_facts >= 4);
    }

    #[test]
    fn body_results_and_call_arguments_must_match_declared_types() {
        let text = concat!(
            "fn takes(value: Int) -> Int { value }\n",
            "fn wrong_result(value: Text) -> Int { value }\n",
            "fn wrong_argument() -> Int { takes(\"not an int\") }\n",
        );
        let report = Compiler::type_project(vec![module(
            "contracts/body_mismatch",
            "body_mismatch.chimi",
            text,
        )]);

        assert_eq!(
            report
                .diagnostics()
                .iter()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            ["T4203", "T4203"]
        );
        assert!(
            report
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.stage() == DiagnosticStage::Type)
        );
        assert_eq!(
            report.diagnostics()[1].range().start(),
            text.find("\"not an int\"").unwrap() as u32
        );
    }

    #[test]
    fn callable_channels_reach_a_fixed_point_across_forward_references() {
        let report = Compiler::type_project(vec![module(
            "lookup/forward",
            "forward.chimi",
            concat!(
                "enum LookupError { Missing }\n",
                "interface Store { lookup: fn(Int) -> Text errors LookupError }\n",
                "fn caller(key: Int) -> Text { load(key) }\n",
                "fn load(key: Int) -> Text needs { store: Store } {\n",
                "  store.lookup(key)\n",
                "}\n",
            ),
        )]);

        assert!(report.is_success(), "{}", report.render_diagnostics());
        assert_eq!(
            report.contract("lookup/forward", "caller").as_deref(),
            Some("fn(Int) -> Text errors LookupError needs { store: Store }")
        );
    }

    #[test]
    fn nested_functions_infer_inputs_from_calls_and_preserve_generic_channels() {
        let report = Compiler::type_project(vec![module(
            "contracts/closure",
            "closure.chimi",
            concat!(
                "fn preserve<E, R>(\n",
                "  operation: fn(Int) -> Text errors E needs R,\n",
                ") -> fn(Int) -> Text errors E needs R {\n",
                "  fn(value) { operation(value) }\n",
                "}\n",
            ),
        )]);

        assert!(report.is_success(), "{}", report.render_diagnostics());
        assert_eq!(
            report.contract("contracts/closure", "preserve").as_deref(),
            Some("fn(fn(Int) -> Text errors E needs R) -> fn(Int) -> Text errors E needs R")
        );
    }

    #[test]
    fn generic_partial_application_through_nested_closures_converges() {
        let report = Compiler::type_project(vec![
            module(
                "lib/partial",
                "partial.chimi",
                concat!(
                    "pub fn with<A>() -> fn(fn() -> A) -> fn() -> A {\n",
                    "  fn(operation: fn() -> A) { fn() { operation() } }\n",
                    "}\n",
                ),
            ),
            module(
                "app/main",
                "main.chimi",
                concat!(
                    "import lib/partial as Partial\n",
                    "fn operation() -> Int { 1 }\n",
                    "const configured = operation |> (Partial.with())\n",
                    "pub fn main() -> Int { configured() }\n",
                ),
            ),
        ]);

        assert!(report.is_success(), "{}", report.render_diagnostics());
        assert_eq!(
            report.contract("lib/partial", "with").as_deref(),
            Some("fn() -> fn(fn() -> A) -> fn() -> A")
        );
    }

    #[test]
    fn generic_composition_instantiates_value_failure_and_needs_rows_per_call() {
        let report = Compiler::type_project(vec![module(
            "contracts/map_success",
            "map_success.chimi",
            concat!(
                "enum LeftError { LeftFailed }\n",
                "enum RightError { RightFailed }\n",
                "interface LeftDep { get: fn(Int) -> Text errors LeftError }\n",
                "interface RightDep { get: fn(Text) -> Int errors RightError }\n",
                "fn map_success<A, B, C, E, R>(\n",
                "  operation: fn(A) -> B errors E needs R,\n",
                "  transform: fn(B) -> C,\n",
                ") -> fn(A) -> C errors E needs R {\n",
                "  fn(input) { transform(operation(input)) }\n",
                "}\n",
                "fn left(value: Int) -> Text needs { dependency: LeftDep } {\n",
                "  dependency.get(value)\n",
                "}\n",
                "fn text_to_int(value: Text) -> Int { 1 }\n",
                "fn right(value: Text) -> Int needs { dependency: RightDep } {\n",
                "  dependency.get(value)\n",
                "}\n",
                "fn int_to_text(value: Int) -> Text { \"done\" }\n",
                "fn composed_left() -> fn(Int) -> Int errors LeftError needs { dependency: LeftDep } {\n",
                "  map_success(left, text_to_int)\n",
                "}\n",
                "fn composed_right() -> fn(Text) -> Text errors RightError needs { dependency: RightDep } {\n",
                "  map_success(right, int_to_text)\n",
                "}\n",
            ),
        )]);

        assert!(report.is_success(), "{}", report.render_diagnostics());
        assert_eq!(
            report
                .contract("contracts/map_success", "composed_left")
                .as_deref(),
            Some("fn() -> fn(Int) -> Int errors LeftError needs { dependency: LeftDep }")
        );
        assert_eq!(
            report
                .contract("contracts/map_success", "composed_right")
                .as_deref(),
            Some("fn() -> fn(Text) -> Text errors RightError needs { dependency: RightDep }")
        );
    }

    #[test]
    fn inspection_omits_user_facing_effect_rows() {
        let source = concat!(
            "fn work() -> Int { 1 }\n",
            "pub fn main() -> Int { work() }\n"
        );
        let sources = vec![module("main", "main.chimi", source)];
        let inspection = Compiler::inspect_project(
            sources.clone(),
            "main",
            source.rfind("work").expect("work call"),
        )
        .expect("inspect entry");
        let callable = inspection
            .callable()
            .expect("entry has callable inspection");
        assert_eq!(callable.display(), "fn() -> Int");
        assert!(!inspection.render_json().contains("\"effects\""));
    }

    #[test]
    fn one_generic_call_keeps_repeated_value_parameters_consistent() {
        let text = concat!(
            "fn choose<A>(left: A, right: A) -> A { left }\n",
            "fn invalid() -> Int { choose(1, \"text\") }\n",
        );
        let report = Compiler::type_project(vec![module(
            "contracts/repeated_generic",
            "repeated_generic.chimi",
            text,
        )]);

        assert_eq!(report.diagnostics().len(), 1);
        assert_eq!(report.diagnostics()[0].stage(), DiagnosticStage::Type);
        assert_eq!(report.diagnostics()[0].code(), "T4203");
        assert_eq!(
            report.diagnostics()[0].range().start(),
            text.find("\"text\"").unwrap() as u32
        );
    }

    #[test]
    fn generic_needs_rows_preserve_concrete_entries_while_substituting_the_remainder() {
        let report = Compiler::type_project(vec![module(
            "contracts/needs_extension",
            "needs_extension.chimi",
            concat!(
                "interface Clock { now: fn() -> Int }\n",
                "interface Mailer { send: fn(Int) -> Text }\n",
                "fn add_clock<R>(\n",
                "  operation: fn(Int) -> Text needs R,\n",
                ") -> fn(Int) -> Text needs R + { clock: Clock } {\n",
                "  fn(value) needs { clock: Clock } { operation(value) }\n",
                "}\n",
                "fn mail(value: Int) -> Text needs { mailer: Mailer } {\n",
                "  mailer.send(value)\n",
                "}\n",
                "fn configured() -> fn(Int) -> Text needs { clock: Clock, mailer: Mailer } {\n",
                "  add_clock(mail)\n",
                "}\n",
            ),
        )]);

        assert!(report.is_success(), "{}", report.render_diagnostics());
        assert_eq!(
            report
                .contract("contracts/needs_extension", "configured")
                .as_deref(),
            Some("fn() -> fn(Int) -> Text needs { clock: Clock, mailer: Mailer }")
        );
    }

    #[test]
    fn structural_expressions_preserve_labels_elements_operators_and_pipes() {
        let report = Compiler::type_project(vec![module(
            "expressions/structural",
            "structural.chimi",
            concat!(
                "struct Money { id: Text, cents: Int }\n",
                "struct Box<A> { value: A }\n",
                "enum Maybe<A> { Some(value: A), None }\n",
                "const provider = { value: 7, label: \"seven\" }\n",
                "fn make_money() -> Money { Money(\"m-1\", 2500) }\n",
                "fn boxed_int() -> Box<Int> { Box(1) }\n",
                "fn boxed_text() -> Box<Text> { Box(\"text\") }\n",
                "fn maybe_int() -> Maybe<Int> { Some(1) }\n",
                "fn read_money(value: Money) -> Int { value.cents }\n",
                "fn read_record() -> Int { provider.value }\n",
                "fn integers() -> List<Int> { [1, 2, 3] }\n",
                "fn empty_integers() -> List<Int> { [] }\n",
                "fn arithmetic() -> Bool { (1 + 2 * 3) > -1 }\n",
                "fn boolean() -> Bool { True }\n",
                "fn stringify(value: Int) -> Text { \"value\" }\n",
                "fn piped() -> Text { 1 |> stringify }\n",
            ),
        )]);

        assert!(report.is_success(), "{}", report.render_diagnostics());
        for (function, contract) in [
            ("make_money", "fn() -> Money"),
            ("boxed_int", "fn() -> Box<Int>"),
            ("boxed_text", "fn() -> Box<Text>"),
            ("maybe_int", "fn() -> Maybe<Int>"),
            ("read_money", "fn(Money) -> Int"),
            ("read_record", "fn() -> Int"),
            ("integers", "fn() -> List<Int>"),
            ("empty_integers", "fn() -> List<Int>"),
            ("arithmetic", "fn() -> Bool"),
            ("boolean", "fn() -> Bool"),
            ("piped", "fn() -> Text"),
        ] {
            assert_eq!(
                report
                    .contract("expressions/structural", function)
                    .as_deref(),
                Some(contract),
                "unexpected contract for {function}"
            );
        }
    }

    #[test]
    fn structural_expression_errors_stop_at_the_owning_shape() {
        let report = Compiler::type_project(vec![module(
            "expressions/invalid_structural",
            "invalid_structural.chimi",
            concat!(
                "struct Money { cents: Int }\n",
                "fn identity(value: Int) -> Int { value }\n",
                "fn wrong_arity() -> Money { Money(1, 2) }\n",
                "fn wrong_field_type() -> Money { Money(\"many\") }\n",
                "fn mixed_list() -> List<Int> { [1, \"two\"] }\n",
                "fn wrong_call_arity() -> Int { identity() }\n",
            ),
        )]);

        assert_eq!(
            report
                .diagnostics()
                .iter()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            ["T4202", "T4203", "T4203", "T4202"]
        );
        assert!(
            report
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.stage() == DiagnosticStage::Type)
        );
    }

    #[test]
    fn positional_construction_fills_fields_in_declaration_order() {
        let report = Compiler::run_project(
            vec![module(
                "app/main",
                "main.chimi",
                concat!(
                    "struct Payment { id: Text, cents: Int }\n",
                    "enum Charge { Settled(payment: Payment, attempts: Int) }\n",
                    "pub fn main() -> Charge { Settled(Payment(\"p-1\", 2500), 3) }\n",
                ),
            )],
            ExecutionConfig::new("positional-construction").unwrap(),
        )
        .expect("positional construction executes");

        let ExecutionOutcome::Success(ExecutionValue::Constructed {
            variant_name,
            fields,
            ..
        }) = &report.outcome
        else {
            panic!("main returns the constructed charge")
        };
        assert_eq!(variant_name.as_deref(), Some("Settled"));
        assert_eq!(fields[0].name, "payment");
        assert_eq!(
            fields[0].value,
            ExecutionValue::Constructed {
                type_name: "Payment".to_owned(),
                variant_name: None,
                fields: vec![
                    ValueField {
                        name: "id".to_owned(),
                        value: ExecutionValue::Text("p-1".to_owned()),
                    },
                    ValueField {
                        name: "cents".to_owned(),
                        value: ExecutionValue::Integer(2500),
                    },
                ],
            }
        );
        assert_eq!(fields[1].name, "attempts");
        assert_eq!(fields[1].value, ExecutionValue::Integer(3));
    }

    #[test]
    fn positional_construction_rejects_wrong_order_and_arity() {
        let report = Compiler::type_project(vec![module(
            "expressions/positional",
            "positional.chimi",
            concat!(
                "struct Payment { id: Text, cents: Int }\n",
                "fn swapped() -> Payment { Payment(2500, \"p-1\") }\n",
                "fn missing() -> Payment { Payment(\"p-1\") }\n",
                "fn extra() -> Payment { Payment(\"p-1\", 2500, 3) }\n",
            ),
        )]);

        let codes = report
            .diagnostics()
            .iter()
            .map(Diagnostic::code)
            .collect::<Vec<_>>();
        assert!(codes.contains(&"T4203"), "{codes:?}");
        assert_eq!(codes.iter().filter(|code| **code == "T4202").count(), 2);
    }

    #[test]
    fn labelled_arguments_are_rejected_with_the_positional_form() {
        let text = concat!(
            "struct Payment { id: Text, cents: Int }\n",
            "fn identity(value: Int) -> Int { value }\n",
            "fn reordered() -> Payment { Payment(cents: 2500, id: \"p-1\") }\n",
            "fn unknown() -> Payment { Payment(id: \"p-1\", dollars: 1) }\n",
            "fn function() -> Int { identity(value: 1) }\n",
        );
        let report =
            Compiler::type_project(vec![module("expressions/labelled", "labelled.chimi", text)]);

        let diagnostics = report
            .diagnostics()
            .iter()
            .map(|diagnostic| (diagnostic.code(), diagnostic.message()))
            .collect::<Vec<_>>();
        assert_eq!(
            diagnostics,
            [
                (
                    "T4210",
                    "Chimi arguments are positional; write `Payment(\"p-1\", 2500)`"
                ),
                (
                    "T4210",
                    "Chimi arguments are positional; pass `Payment` fields in declaration order (id, cents)"
                ),
                (
                    "T4210",
                    "Chimi arguments are positional; remove the `value:` label"
                ),
            ],
            "a recognised constructor rewrite must not cascade into type errors"
        );
        let label = text.find("cents: 2500").unwrap() as u32;
        assert_eq!(report.diagnostics()[0].range().start(), label);
    }

    #[test]
    fn using_closes_executed_and_latent_needs_at_arbitrary_boundaries() {
        let report = Compiler::type_project(vec![module(
            "dependencies/provision",
            "provision.chimi",
            concat!(
                "interface Clock { now: fn() -> Int }\n",
                "interface Mailer { send: fn(Int) -> Text }\n",
                "const clock = { now: fn() -> Int { 1 }, extra: \"allowed\" }\n",
                "const mailer = { send: fn(value: Int) -> Text { \"sent\" } }\n",
                "fn timestamp() -> Int needs { clock: Clock } { clock.now() }\n",
                "fn notify() -> Text needs { clock: Clock, mailer: Mailer } {\n",
                "  mailer.send(clock.now())\n",
                "}\n",
                "fn closed_call() -> Int { timestamp() using { clock: clock } }\n",
                "fn configured() -> fn() -> Int { timestamp using { clock: clock } }\n",
                "fn partial() -> Text needs { mailer: Mailer } {\n",
                "  notify() using { clock: clock }\n",
                "}\n",
                "fn closed_nested() -> Text {\n",
                "  let operation = notify using { clock: clock }\n",
                "  let fully_configured = operation using { mailer: mailer }\n",
                "  fully_configured()\n",
                "}\n",
            ),
        )]);

        assert!(report.is_success(), "{}", report.render_diagnostics());
        for (function, contract) in [
            ("closed_call", "fn() -> Int"),
            ("configured", "fn() -> fn() -> Int"),
            ("partial", "fn() -> Text needs { mailer: Mailer }"),
            ("closed_nested", "fn() -> Text"),
        ] {
            assert_eq!(
                report
                    .contract("dependencies/provision", function)
                    .as_deref(),
                Some(contract),
                "unexpected contract for {function}"
            );
        }
    }

    #[test]
    fn using_reports_unknown_duplicate_and_mismatched_providers_without_discharge() {
        let report = Compiler::type_project(vec![module(
            "dependencies/invalid_provision",
            "invalid_provision.chimi",
            concat!(
                "interface Clock { now: fn() -> Int }\n",
                "const wrong = { now: fn(value: Int) -> Int { value } }\n",
                "const clock = { now: fn() -> Int { 1 } }\n",
                "fn timestamp() -> Int needs { clock: Clock } { clock.now() }\n",
                "fn mismatch() -> Int {\n",
                "  timestamp() using { clock: wrong }\n",
                "}\n",
                "fn unknown() -> Int {\n",
                "  timestamp() using { logger: clock }\n",
                "}\n",
                "fn duplicate() -> Int {\n",
                "  timestamp() using { clock: clock, clock: clock }\n",
                "}\n",
                "interface Source { read: fn() -> Int }\n",
                "interface FlexibleClock { now: fn() -> Int needs { source: Source } }\n",
                "interface ValueSource<A> { get: fn() -> A }\n",
                "const narrow = { now: fn() -> Int { 1 } }\n",
                "const wide = { now: fn() -> Int needs { source: Source } { 1 } }\n",
                "const missing = { other: fn() -> Int { 1 } }\n",
                "const int_source = { get: fn() -> Int { 1 } }\n",
                "fn flexible() -> Int needs { clock: FlexibleClock } { 1 }\n",
                "fn strict() -> Int needs { clock: Clock } { 1 }\n",
                "fn generic() -> Int needs { source: ValueSource<Int> } { 1 }\n",
                "fn narrowed() -> Int { flexible() using { clock: narrow } }\n",
                "fn generic_closed() -> Int { generic() using { source: int_source } }\n",
                "fn widened() -> Int {\n",
                "  strict() using { clock: wide }\n",
                "}\n",
                "fn missing_member() -> Int {\n",
                "  strict() using { clock: missing }\n",
                "}\n",
            ),
        )]);

        assert_eq!(
            report
                .diagnostics()
                .iter()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            ["T4213", "T4212", "T4214", "T4213", "T4213"]
        );
        assert!(
            report.diagnostics()[0]
                .message()
                .contains("member `now`: input count differs")
        );
        assert!(
            report.diagnostics()[3]
                .message()
                .contains("needs dependencies outside the interface contract")
        );
        assert!(
            report.diagnostics()[4]
                .message()
                .contains("missing member `now`")
        );
        for function in [
            "mismatch",
            "unknown",
            "duplicate",
            "widened",
            "missing_member",
        ] {
            assert_eq!(
                report
                    .contract("dependencies/invalid_provision", function)
                    .as_deref(),
                Some("fn() -> Int needs { clock: Clock }"),
                "failed provision unexpectedly discharged {function}"
            );
        }
        assert!(
            report
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.stage() == DiagnosticStage::Type)
        );
    }

    #[test]
    fn outcomes_construct_bubble_recover_and_remap_failure_variants() {
        let report = Compiler::type_project(vec![module(
            "outcomes/recovery",
            "recovery.chimi",
            concat!(
                "struct Receipt { id: Text }\n",
                "enum PaymentError { Declined(reason: Text), Offline }\n",
                "enum CheckoutError { Payment(error: PaymentError) }\n",
                "fn operation(mode: Int) -> Receipt {\n",
                "  case mode {\n",
                "    0 -> Failure(Declined(\"declined\"))\n",
                "    _ -> Failure(Offline())\n",
                "  }\n",
                "}\n",
                "fn bubble(mode: Int) -> Receipt { operation(mode) }\n",
                "fn explicit_success() -> Int { Success(1) }\n",
                "fn decline_only() -> Receipt { Failure(Declined(\"declined\")) }\n",
                "fn capture_precise_failure() -> Text {\n",
                "  case decline_only() {\n",
                "    Success(_receipt) -> \"success\"\n",
                "    Failure(error) -> case error {\n",
                "      Declined(reason) -> reason\n",
                "    }\n",
                "  }\n",
                "}\n",
                "fn recover_declined(mode: Int) -> Receipt {\n",
                "  case operation(mode) {\n",
                "    Success(receipt) -> receipt\n",
                "    Failure(Declined(_reason)) -> Receipt(\"fallback\")\n",
                "  }\n",
                "}\n",
                "fn recover_all(mode: Int) -> Receipt {\n",
                "  case operation(mode) {\n",
                "    Success(receipt) -> receipt\n",
                "    Failure(_error) -> Receipt(\"fallback\")\n",
                "  }\n",
                "}\n",
                "fn guarded_recovery(mode: Int) -> Receipt {\n",
                "  case operation(mode) {\n",
                "    Success(receipt) -> receipt\n",
                "    Failure(Declined(_reason)) if True -> Receipt(\"declined\")\n",
                "    Failure(Offline) -> Receipt(\"offline\")\n",
                "  }\n",
                "}\n",
                "fn remap(mode: Int) -> Receipt {\n",
                "  case operation(mode) {\n",
                "    Success(receipt) -> receipt\n",
                "    Failure(error) -> Failure(Payment(error))\n",
                "  }\n",
                "}\n",
            ),
        )]);

        assert!(report.is_success(), "{}", report.render_diagnostics());
        for (function, contract) in [
            ("operation", "fn(Int) -> Receipt errors PaymentError"),
            ("bubble", "fn(Int) -> Receipt errors PaymentError"),
            ("explicit_success", "fn() -> Int"),
            (
                "decline_only",
                "fn() -> Receipt errors PaymentError.Declined",
            ),
            ("capture_precise_failure", "fn() -> Text"),
            (
                "recover_declined",
                "fn(Int) -> Receipt errors PaymentError.Offline",
            ),
            ("recover_all", "fn(Int) -> Receipt"),
            (
                "guarded_recovery",
                "fn(Int) -> Receipt errors PaymentError.Declined",
            ),
            ("remap", "fn(Int) -> Receipt errors CheckoutError"),
        ] {
            assert_eq!(
                report.contract("outcomes/recovery", function).as_deref(),
                Some(contract),
                "unexpected contract for {function}"
            );
        }
    }

    #[test]
    fn generic_recovery_thunks_subtract_handled_failure_variants() {
        let report = Compiler::type_project(vec![module(
            "outcomes/generic_recovery",
            "generic_recovery.chimi",
            concat!(
                "struct Receipt { id: Text }\n",
                "enum PaymentError { Declined(reason: Text), Offline }\n",
                "fn recover_declined<E>(operation: fn() -> Receipt errors E) {\n",
                "  fn() {\n",
                "    case operation() {\n",
                "      Success(receipt) -> receipt\n",
                "      Failure(Declined(reason)) -> Receipt(reason)\n",
                "    }\n",
                "  }\n",
                "}\n",
                "fn recover_declined_when<E>(operation: fn() -> Receipt errors E) {\n",
                "  fn() {\n",
                "    case operation() {\n",
                "      Success(receipt) -> receipt\n",
                "      Failure(Declined(reason)) if True -> Receipt(reason)\n",
                "    }\n",
                "  }\n",
                "}\n",
                "fn describe_failure<E>(operation: fn() -> Receipt errors E) {\n",
                "  case operation() {\n",
                "    Success(_receipt) -> \"success\"\n",
                "    Failure(error) -> case error {\n",
                "      Declined(reason) -> reason\n",
                "      _ -> \"other\"\n",
                "    }\n",
                "  }\n",
                "}\n",
                "fn operation() -> Receipt { Failure(Offline()) }\n",
                "fn configured() {\n",
                "  operation |> recover_declined\n",
                "}\n",
            ),
        )]);

        assert!(report.is_success(), "{}", report.render_diagnostics());
        assert_eq!(
            report
                .contract("outcomes/generic_recovery", "recover_declined")
                .as_deref(),
            Some(
                "fn(fn() -> Receipt errors E) -> fn() -> Receipt errors E - PaymentError.Declined"
            )
        );
        assert_eq!(
            report
                .contract("outcomes/generic_recovery", "recover_declined_when")
                .as_deref(),
            Some("fn(fn() -> Receipt errors E) -> fn() -> Receipt errors E")
        );
        assert_eq!(
            report
                .contract("outcomes/generic_recovery", "describe_failure")
                .as_deref(),
            Some("fn(fn() -> Receipt errors E) -> Text")
        );
        assert_eq!(
            report
                .contract("outcomes/generic_recovery", "configured")
                .as_deref(),
            Some("fn() -> fn() -> Receipt errors PaymentError.Offline")
        );
    }

    #[test]
    fn captured_generic_failure_values_require_exhaustive_ordinary_cases() {
        let report = Compiler::type_project(vec![module(
            "outcomes/generic_failure_value",
            "generic_failure_value.chimi",
            concat!(
                "struct Receipt { id: Text }\n",
                "enum PaymentError { Declined(reason: Text), Offline }\n",
                "fn describe<E>(operation: fn() -> Receipt errors E) {\n",
                "  case operation() {\n",
                "    Success(_receipt) -> \"success\"\n",
                "    Failure(error) -> case error {\n",
                "      Declined(reason) -> reason\n",
                "    }\n",
                "  }\n",
                "}\n",
            ),
        )]);

        assert_eq!(
            report
                .diagnostics()
                .iter()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            ["T4231"]
        );
    }

    #[test]
    fn generic_selective_recovery_erases_to_existing_core_outcome_matching() {
        let report = Compiler::run_project(
            vec![module(
                "outcomes/generic_recovery_runtime",
                "generic_recovery_runtime.chimi",
                concat!(
                    "struct Receipt { id: Text }\n",
                    "enum PaymentError { Declined(reason: Text), Offline }\n",
                    "fn recover_declined<E>(operation: fn() -> Receipt errors E) {\n",
                    "  fn() {\n",
                    "    case operation() {\n",
                    "      Success(receipt) -> receipt\n",
                    "      Failure(Declined(reason)) -> Receipt(reason)\n",
                    "    }\n",
                    "  }\n",
                    "}\n",
                    "fn operation() -> Receipt { Failure(Declined(\"recovered\")) }\n",
                    "pub fn main() -> Receipt {\n",
                    "  let configured = operation |> recover_declined\n",
                    "  configured()\n",
                    "}\n",
                ),
            )],
            ExecutionConfig::new("generic-selective-recovery").unwrap(),
        )
        .expect("generic recovery must execute through ordinary Core matching");

        assert!(matches!(
            report.outcome,
            ExecutionOutcome::Success(ExecutionValue::Constructed {
                ref type_name,
                variant_name: None,
                ref fields,
            }) if type_name == "Receipt"
                && fields[0].name == "id"
                && fields[0].value == ExecutionValue::Text("recovered".to_owned())
        ));
    }

    #[test]
    fn generic_selective_recovery_does_not_catch_defects_or_cancellation() {
        let sources = || {
            vec![module(
                "outcomes/generic_recovery_control",
                "generic_recovery_control.chimi",
                concat!(
                    "struct Receipt { id: Text }\n",
                    "enum PaymentError { Declined(reason: Text) }\n",
                    "fn recover_declined<E>(operation: fn() -> Receipt errors E) {\n",
                    "  fn() {\n",
                    "    case operation() {\n",
                    "      Success(receipt) -> receipt\n",
                    "      Failure(Declined(reason)) -> Receipt(reason)\n",
                    "    }\n",
                    "  }\n",
                    "}\n",
                    "fn operation() -> Receipt {\n",
                    "  let _crash = 1 / 0\n",
                    "  Receipt(\"unreachable\")\n",
                    "}\n",
                    "pub fn main() -> Receipt {\n",
                    "  let configured = operation |> recover_declined\n",
                    "  configured()\n",
                    "}\n",
                ),
            )]
        };

        let defect = Compiler::run_project(
            sources(),
            ExecutionConfig::new("generic-recovery-defect").unwrap(),
        )
        .expect("the generic recovery program compiles before its runtime defect");
        assert!(matches!(
            defect.outcome,
            ExecutionOutcome::Defect(ref defect) if defect.message.starts_with("integer division by zero")
        ));

        let cancellation = Compiler::run_project(
            sources(),
            ExecutionConfig::new("generic-recovery-cancel")
                .unwrap()
                .with_cancellation_requested(),
        )
        .expect("the generic recovery program seals before cancellation");
        assert!(matches!(
            cancellation.outcome,
            ExecutionOutcome::Cancellation(_)
        ));
    }

    #[test]
    fn inspection_exposes_generic_failure_row_subtraction() {
        let source = concat!(
            "struct Receipt { id: Text }\n",
            "enum PaymentError { Declined(reason: Text), Offline }\n",
            "fn recover_declined<E>(operation: fn() -> Receipt errors E) {\n",
            "  case operation() {\n",
            "    Success(receipt) -> receipt\n",
            "    Failure(Declined(reason)) -> Receipt(reason)\n",
            "  }\n",
            "}\n",
            "fn operation() -> Receipt { Failure(Offline()) }\n",
            "fn configured() { recover_declined(operation) }\n",
        );
        let offset = source.rfind("recover_declined").unwrap();
        let report = Compiler::inspect_project(
            vec![module(
                "outcomes/generic_recovery_inspection",
                "generic_recovery_inspection.chimi",
                source,
            )],
            "outcomes/generic_recovery_inspection",
            offset,
        )
        .expect("inspect the generic recovery reference");

        let callable = report.callable().expect("recovery is callable");
        assert_eq!(
            callable.display(),
            "fn(fn() -> Receipt errors E) -> Receipt errors E - PaymentError.Declined"
        );
        assert_eq!(callable.failure_parameters(), ["E - PaymentError.Declined"]);
    }

    #[test]
    fn outcome_errors_are_bounded_and_leave_unhandled_failures_visible() {
        let report = Compiler::type_project(vec![module(
            "outcomes/invalid",
            "invalid_outcomes.chimi",
            concat!(
                "struct Receipt { id: Text }\n",
                "enum PaymentError { Declined(reason: Text), Offline }\n",
                "fn operation() -> Receipt { Failure(Declined(\"no\")) }\n",
                "fn raw_case() -> Receipt {\n",
                "  case operation() { _ -> Receipt(\"raw\") }\n",
                "}\n",
                "fn incomplete() -> Receipt {\n",
                "  case operation() {\n",
                "    Success(receipt) -> receipt\n",
                "  }\n",
                "}\n",
                "fn impossible() -> Receipt {\n",
                "  case operation() {\n",
                "    Success(receipt) -> receipt\n",
                "    Failure(Offline) -> Receipt(\"offline\")\n",
                "  }\n",
                "}\n",
                "fn invalid_pattern() -> Receipt {\n",
                "  case operation() {\n",
                "    Success(receipt) -> receipt\n",
                "    Failure(True) -> Receipt(\"invalid\")\n",
                "  }\n",
                "}\n",
                "fn unreachable_outcome() -> Receipt {\n",
                "  case operation() {\n",
                "    Success(receipt) -> receipt\n",
                "    Failure(_error) -> Receipt(\"fallback\")\n",
                "    Failure(Declined(_reason)) -> Receipt(\"duplicate\")\n",
                "  }\n",
                "}\n",
                "fn bad_failure() -> Int { Failure(1) }\n",
                "fn bad_success() -> Int { Success() }\n",
                "fn standalone() -> Int { Failure }\n",
            ),
        )]);

        assert_eq!(
            report
                .diagnostics()
                .iter()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            [
                "T4222", "T4223", "T4224", "T4222", "T4232", "T4220", "T4221", "T4221"
            ]
        );
        for function in ["raw_case", "incomplete", "impossible", "invalid_pattern"] {
            assert_eq!(
                report.contract("outcomes/invalid", function).as_deref(),
                Some("fn() -> Receipt errors PaymentError.Declined"),
                "invalid recovery unexpectedly consumed {function}'s failure"
            );
        }
        assert!(
            report
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.stage() == DiagnosticStage::Type)
        );
    }

    #[test]
    fn ordinary_case_analysis_covers_nested_and_multi_subject_spaces() {
        let report = Compiler::type_project(vec![module(
            "patterns/coverage",
            "coverage.chimi",
            concat!(
                "enum State { Empty, Ready, Submitted(id: Text) }\n",
                "enum Request { View, Transfer(amount: Int) }\n",
                "enum OptionBool { Some(value: Bool), None }\n",
                "fn state_label(state: State) -> Text {\n",
                "  case state {\n",
                "    Empty -> \"empty\"\n",
                "    Ready -> \"ready\"\n",
                "    Submitted(id) -> id\n",
                "  }\n",
                "}\n",
                "fn boolean_label(value: Bool) -> Text {\n",
                "  case value {\n",
                "    True -> \"true\"\n",
                "    False -> \"false\"\n",
                "  }\n",
                "}\n",
                "fn nested(value: OptionBool) -> Text {\n",
                "  case value {\n",
                "    Some(True) -> \"true\"\n",
                "    Some(False) -> \"false\"\n",
                "    None -> \"none\"\n",
                "  }\n",
                "}\n",
                "fn matrix(state: State, request: Request) -> Text {\n",
                "  case state, request {\n",
                "    Empty, _ -> \"empty\"\n",
                "    Ready, View -> \"view\"\n",
                "    Ready, Transfer(_amount) -> \"transfer\"\n",
                "    Submitted(_id), _ -> \"submitted\"\n",
                "  }\n",
                "}\n",
                "fn open_literal(value: Int) -> Text {\n",
                "  case value {\n",
                "    0 -> \"zero\"\n",
                "    _ -> \"other\"\n",
                "  }\n",
                "}\n",
                "fn pure_guard(value: Int) -> Text {\n",
                "  case value {\n",
                "    candidate if candidate > 0 -> \"positive\"\n",
                "    _ -> \"other\"\n",
                "  }\n",
                "}\n",
            ),
        )]);

        assert!(report.is_success(), "{}", report.render_diagnostics());
    }

    #[test]
    fn ordinary_case_diagnostics_bound_exhaustiveness_reachability_and_guards() {
        let report = Compiler::type_project(vec![module(
            "patterns/invalid",
            "invalid_patterns.chimi",
            concat!(
                "enum State { Empty, Ready, Submitted(id: Text) }\n",
                "enum GuardError { Unavailable }\n",
                "interface Risk { check: fn(State) -> Bool }\n",
                "fn fallible_guard() -> Bool { Failure(Unavailable()) }\n",
                "fn incomplete(state: State) -> Text {\n",
                "  case state {\n",
                "    Empty -> \"empty\"\n",
                "    Ready -> \"ready\"\n",
                "  }\n",
                "}\n",
                "fn duplicate(state: State) -> Text {\n",
                "  case state {\n",
                "    Empty -> \"first\"\n",
                "    Empty -> \"second\"\n",
                "    _ -> \"other\"\n",
                "  }\n",
                "}\n",
                "fn early_catch_all(state: State) -> Text {\n",
                "  case state {\n",
                "    _ -> \"all\"\n",
                "    Ready -> \"ready\"\n",
                "  }\n",
                "}\n",
                "fn guarded_only(state: State) -> Text {\n",
                "  case state {\n",
                "    candidate if True -> \"guarded\"\n",
                "  }\n",
                "}\n",
                "fn wrong_guard(state: State) -> Text {\n",
                "  case state {\n",
                "    candidate if 1 -> \"wrong\"\n",
                "    _ -> \"other\"\n",
                "  }\n",
                "}\n",
                "fn observable_guard(state: State) -> Text needs { risk: Risk } {\n",
                "  case state {\n",
                "    candidate if risk.check(candidate) -> \"risk\"\n",
                "    _ -> \"other\"\n",
                "  }\n",
                "}\n",
                "fn failed_guard(state: State) -> Text {\n",
                "  case state {\n",
                "    candidate if fallible_guard() -> \"guarded\"\n",
                "    _ -> \"other\"\n",
                "  }\n",
                "}\n",
                "fn wrong_pattern(state: State) -> Text {\n",
                "  case state {\n",
                "    True -> \"wrong\"\n",
                "    _ -> \"other\"\n",
                "  }\n",
                "}\n",
            ),
        )]);

        assert_eq!(
            report
                .diagnostics()
                .iter()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            [
                "T4231", "T4232", "T4233", "T4232", "T4231", "T4234", "T4234", "T4234", "T4230"
            ]
        );
        assert!(
            report
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.stage() == DiagnosticStage::Type)
        );
    }

    #[test]
    fn runtime_roots_require_unexplained_needs_without_provider_cascades() {
        let declarations = concat!(
            "interface Clock { now: fn() -> Int }\n",
            "const clock = { now: fn() -> Int { 1 } }\n",
            "const wrong = { now: fn(value: Int) -> Int { value } }\n",
            "fn timestamp() -> Int needs { clock: Clock } { clock.now() }\n",
        );

        let closed = Compiler::type_project(vec![module(
            "roots/closed",
            "closed.chimi",
            &format!(
                "{declarations}pub fn main() -> Int {{ timestamp() using {{ clock: clock }} }}\n"
            ),
        )]);
        assert!(closed.is_success(), "{}", closed.render_diagnostics());

        let missing = Compiler::type_project(vec![module(
            "roots/missing",
            "missing.chimi",
            &format!("{declarations}pub fn main() -> Int {{ timestamp() }}\n"),
        )]);
        assert_eq!(missing.diagnostics().len(), 1);
        assert_eq!(missing.diagnostics()[0].code(), "E5002");
        assert_eq!(
            missing.diagnostics()[0].stage(),
            DiagnosticStage::ExecutionSetup
        );
        assert_eq!(
            missing.diagnostics()[0].message(),
            "main needs clock: Clock"
        );

        let mismatch = Compiler::type_project(vec![module(
            "roots/mismatch",
            "mismatch.chimi",
            &format!(
                concat!(
                    "{}fn misconfigured() -> Int {{ timestamp() using {{ clock: wrong }} }}\n",
                    "pub fn main() -> Int {{ misconfigured() }}\n"
                ),
                declarations
            ),
        )]);
        assert_eq!(
            mismatch
                .diagnostics()
                .iter()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            ["T4213"]
        );

        let mixed = Compiler::type_project(vec![module(
            "roots/mixed",
            "mixed.chimi",
            &format!(
                concat!(
                    "{}pub fn main() -> Int {{\n",
                    "  timestamp() using {{ clock: wrong }}\n",
                    "  timestamp()\n",
                    "}}\n"
                ),
                declarations
            ),
        )]);
        assert_eq!(
            mixed
                .diagnostics()
                .iter()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            ["T4213", "E5002"]
        );
    }

    #[test]
    fn project_entry_sealing_requires_one_concrete_zero_input_main() {
        let valid = Compiler::verify_project(vec![
            module(
                "entry/helper",
                "helper.chimi",
                "pub fn helper(value: Int) -> Int { value }",
            ),
            module(
                "entry/valid",
                "valid.chimi",
                concat!(
                    "import entry/helper as Helper\n",
                    "fn unused() -> Int { 0 }\n",
                    "pub fn main() -> Int { Helper.helper(1) }\n",
                ),
            ),
        ]);
        // `unused` is not reachable from `main`; `helper` in the other module is.
        let valid = valid.expect("valid project seals");
        assert_eq!(valid.reachable_declarations, 2);
        assert_eq!(valid.reachable_modules, 2);

        for (source, code) in [
            ("fn main() -> Int { 1 }", "E5001"),
            ("pub fn main(value: Int) -> Int { value }", "E5004"),
            ("pub fn main<A>() -> Int { 1 }", "E5005"),
        ] {
            let diagnostics =
                Compiler::verify_project(vec![module("entry/invalid", "invalid.chimi", source)])
                    .expect_err("invalid entry must not seal");
            assert_eq!(diagnostics.len(), 1, "{source}");
            assert_eq!(diagnostics[0].code(), code, "{source}");
            assert_eq!(
                diagnostics[0].stage(),
                DiagnosticStage::ExecutionSetup,
                "{source}"
            );
        }

        let duplicate = Compiler::verify_project(vec![
            module("entry/one", "one.chimi", "pub fn main() -> Int { 1 }"),
            module("entry/two", "two.chimi", "pub fn main() -> Int { 2 }"),
        ])
        .expect_err("multiple exported roots must not seal");
        assert_eq!(duplicate.len(), 1);
        assert_eq!(duplicate[0].code(), "E5003");

        let mut incomplete = Compiler::type_project(vec![module(
            "entry/incomplete",
            "incomplete.chimi",
            "pub fn main() -> Int { 1 }",
        )]);
        let expression = *incomplete
            .typed
            .expression_facts
            .keys()
            .next()
            .expect("typed main has expression facts");
        Arc::get_mut(&mut incomplete.typed)
            .expect("the test report uniquely owns its typed snapshot")
            .expression_facts
            .remove(&expression);
        let diagnostics = crate::executable::seal_entry(incomplete.typed)
            .err()
            .expect("incomplete typed snapshots cannot seal");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code(), "E5006");
    }

    #[test]
    fn verified_roots_lower_to_typed_origin_linked_core_arenas() {
        let basic = || {
            vec![module(
                "core/basic",
                "basic_core.chimi",
                concat!(
                    "struct Answer { value: Int }\n",
                    "fn make(value: Int) -> Answer { Answer(value) }\n",
                    "pub fn main() -> Answer {\n",
                    "  let answer = make(42)\n",
                    "  answer\n",
                    "}\n",
                ),
            )]
        };
        let stats = Compiler::lower_project(basic()).expect("verified structural program lowers");

        // Every Core node links back to a source origin, lowering is
        // deterministic, and the lowered program computes the right value.
        assert_eq!(stats.nodes, stats.origins);
        assert_eq!(Compiler::lower_project(basic()).unwrap(), stats);
        let report = Compiler::run_project(basic(), ExecutionConfig::new("core-basic").unwrap())
            .expect("basic program executes");
        assert_eq!(
            report.outcome,
            ExecutionOutcome::Success(ExecutionValue::Constructed {
                type_name: "Answer".to_owned(),
                variant_name: None,
                fields: vec![ValueField {
                    name: "value".to_owned(),
                    value: ExecutionValue::Integer(42),
                }],
            })
        );

        let rich = Compiler::lower_project(vec![module(
            "core/rich",
            "rich_core.chimi",
            concat!(
                "enum State { Ready }\n",
                "interface Source { read: fn() -> Int }\n",
                "fn classify(state: State) -> Int {\n",
                "  case state { Ready -> 1 }\n",
                "}\n",
                "fn read() -> Int needs { source: Source } { source.read() }\n",
                "pub fn main() -> Int {\n",
                "  let transform = fn(value: Int) -> Int { value + 1 }\n",
                "  let values = [classify(Ready())]\n",
                "  let source = { read: fn() -> Int { 1 } }\n",
                "  let configured = read() using { source: source }\n",
                "  let negative = -(1)\n",
                "  let transformed = transform(configured)\n",
                "  let provider = { value: transformed }\n",
                "  provider.value\n",
                "}\n",
            ),
        )])
        .expect("the complete typed expression algebra lowers without semantic fallback nodes");
        assert_eq!(rich.nodes, rich.origins);
        assert!(rich.functions >= 3);
        assert_eq!(rich.dependencies, 1);

        let outcomes = Compiler::lower_project(vec![module(
            "core/outcomes",
            "outcome_core.chimi",
            concat!(
                "enum Problem { Failed }\n",
                "fn operation() -> Int { Failure(Failed()) }\n",
                "pub fn main() -> Int {\n",
                "  case operation() {\n",
                "    Success(value) -> value\n",
                "    Failure(Failed) -> 0\n",
                "  }\n",
                "}\n",
            ),
        )])
        .expect("contextual outcome patterns remain explicit during lowering");
        assert_eq!(outcomes.nodes, outcomes.origins);
    }

    #[test]
    fn project_core_executes_values_dependencies_outcomes_and_full_traces() {
        let sources = || {
            vec![module(
                "runtime/main",
                "runtime_main.chimi",
                concat!(
                    "interface Source { read: fn() -> Int }\n",
                    "struct Answer { value: Int }\n",
                    "enum Problem { Failed }\n",
                    "fn read() -> Int needs { source: Source } { source.read() }\n",
                    "fn maybe(value: Int) -> Int {\n",
                    "  case value {\n",
                    "    0 -> Failure(Failed())\n",
                    "    _ -> value\n",
                    "  }\n",
                    "}\n",
                    "const provider = { read: fn() -> Int { 41 } }\n",
                    "pub fn main() -> Answer {\n",
                    "  let increment = fn(value: Int) -> Int { value + 1 }\n",
                    "  let configured_read = read using { source: provider }\n",
                    "  case maybe(configured_read()) {\n",
                    "    Success(value) -> Answer(value |> increment)\n",
                    "    Failure(Failed) -> Answer(-1)\n",
                    "  }\n",
                    "}\n",
                ),
            )]
        };
        let report =
            Compiler::run_project(sources(), ExecutionConfig::new("project-success").unwrap())
                .expect("closed project core executes");
        assert!(matches!(
            &report.outcome,
            ExecutionOutcome::Success(ExecutionValue::Constructed {
                type_name,
                variant_name: None,
                fields,
            }) if type_name == "Answer"
                && fields[0].name == "value"
                && fields[0].value == ExecutionValue::Integer(42)
        ));
        assert_eq!(report.events[0].kind, EventKind::ComputationStarted);
        assert_eq!(
            report.events.last().unwrap().kind,
            EventKind::ComputationCompleted
        );
        assert!(
            report
                .events
                .iter()
                .any(|event| event.kind == EventKind::DependencyCalled
                    && event.attributes.iter().any(|attribute| {
                        attribute.name == "requirement"
                            && attribute.value == ExecutionAttributeValue::Text("source".to_owned())
                    }))
        );
        assert!(
            report
                .events
                .iter()
                .enumerate()
                .all(|(sequence, event)| event.sequence == sequence as u64)
        );
        assert!(
            report
                .events
                .iter()
                .all(|event| event.parent_span_id != Some(event.span_id))
        );
        assert_eq!(
            report
                .events
                .iter()
                .filter(|event| event.kind == EventKind::OperationStarted)
                .count(),
            report
                .events
                .iter()
                .filter(|event| event.kind == EventKind::OperationCompleted)
                .count()
        );
        assert!(report.events.iter().all(|event| {
            event.computation == "main"
                && event.execution_id == "project-success"
                && event.source_range.start() < event.source_range.end()
        }));

        let boundaries = Compiler::run_project(
            sources(),
            ExecutionConfig::new("project-boundaries")
                .unwrap()
                .with_trace_mode(TraceMode::Boundaries),
        )
        .expect("boundary-only tracing does not change execution");
        assert_eq!(boundaries.outcome, report.outcome);
        assert_eq!(boundaries.events.len(), 3);
        assert_eq!(boundaries.events[1].kind, EventKind::DependencyCalled);
    }

    #[test]
    fn project_core_executes_the_dependency_forcing_program_from_source() {
        let report = Compiler::run_project(
            vec![module(
                "forcing/checkout",
                "dependencies_and_execution.chimi",
                include_str!("../../../prototypes/golden/valid/dependencies_and_execution.chimi"),
            )],
            ExecutionConfig::new("checkout-forcing-program").unwrap(),
        )
        .expect("the self-contained dependency forcing program executes");

        assert!(
            matches!(
                &report.outcome,
                ExecutionOutcome::Success(ExecutionValue::Constructed {
                    type_name,
                    fields,
                    ..
                }) if type_name == "Order"
                    && matches!(
                        &fields[0].value,
                        ExecutionValue::Constructed {
                            type_name,
                            fields,
                            ..
                        } if type_name == "OrderId"
                            && fields[0].value == ExecutionValue::Text("order-1".to_owned())
                    )
            ),
            "unexpected forcing outcome: {:#?}",
            report.outcome
        );
        assert_eq!(
            report
                .events
                .iter()
                .filter(|event| event.kind == EventKind::DependencyCalled)
                .count(),
            2
        );
    }

    #[test]
    fn inspection_composes_syntax_resolution_types_contracts_and_executable_origins() {
        const SOURCE: &str =
            include_str!("../../../prototypes/golden/valid/dependencies_and_execution.chimi");
        let offset = SOURCE.rfind("submit(").unwrap();
        let report = Compiler::inspect_project(
            vec![module("main", "dependencies_and_execution.chimi", SOURCE)],
            "main",
            offset,
        )
        .expect("inspect the reachable submit call");

        assert_eq!(report.module_path(), "main");
        assert_eq!(report.token().unwrap().text(), "submit");
        assert!(
            report
                .syntax_path()
                .iter()
                .any(|frame| frame.kind() == "name_reference")
        );
        let reference = report.reference().expect("submit is a resolved reference");
        assert_eq!(reference.status(), "resolved");
        let target = reference.target().expect("submit has a target");
        assert_eq!(target.name(), "submit");
        assert_eq!(target.kind(), "function");
        assert_eq!(target.module_path(), Some("main"));
        assert!(target.declaration_range().is_some());
        assert!(
            report
                .inferred_type()
                .unwrap()
                .starts_with("fn(Cart) -> Order")
        );
        let callable = report
            .callable()
            .expect("submit exposes its callable channels");
        assert_eq!(callable.inputs(), ["Cart"]);
        assert_eq!(callable.success(), "Order");
        assert_eq!(callable.failures(), ["PaymentFailed", "InventoryFailed"]);
        assert!(
            callable
                .needs()
                .iter()
                .any(|need| need.name() == "payment" && need.interface() == "Payment")
        );
        assert!(report.executable_origin().is_some());
        assert!(report.diagnostics().is_empty());
    }

    #[test]
    fn inspection_resolves_imported_names_to_their_declaring_module() {
        const USE_MONEY: &str =
            include_str!("../../../prototypes/golden/valid/modules/use_money.chimi");
        let offset = USE_MONEY.rfind("cents(money)").unwrap();
        let report = Compiler::inspect_project(
            vec![
                module("modules/use_money", "use_money.chimi", USE_MONEY),
                module(
                    "modules/money",
                    "money.chimi",
                    include_str!("../../../prototypes/golden/valid/modules/money.chimi"),
                ),
            ],
            "modules/use_money",
            offset,
        )
        .expect("inspect the imported Money.cents reference");

        let target = report
            .reference()
            .and_then(|reference| reference.target())
            .expect("member reference resolves");
        assert_eq!(target.name(), "cents");
        assert_eq!(target.module_path(), Some("modules/money"));
        assert_eq!(
            target.source_path().unwrap(),
            std::path::Path::new("money.chimi")
        );
        assert_eq!(report.callable().unwrap().display(), "fn(Money) -> Int");
        assert_eq!(report.executable_origin(), None);
    }

    /// Guards call-site instantiation: a generic callee also reports the types
    /// this call fixed, for standard and user functions alike, while
    /// `callable` keeps the declared contract.
    #[test]
    fn inspection_shows_generic_callees_instantiated_at_the_call() {
        let source = "import std/map as Map\nimport std/map.{Map}\n\n\
fn same<T>(value: T) -> T { value }\n\n\
pub fn main(counts: Map<Text, Int>) -> Map<Text, Int> { Map.put(counts, \"key\", same(1)) }\n";
        let inspect_at = |needle: &str| {
            Compiler::inspect_project(
                vec![module("main", "main.chimi", source)],
                "main",
                source.rfind(needle).unwrap() + 1,
            )
            .expect("inspect the call")
        };

        let put = inspect_at("put(");
        assert!(put.diagnostics().is_empty(), "{:?}", put.diagnostics());
        assert_eq!(
            put.instantiated_callable().unwrap().display(),
            "fn(Map<Text, Int>, Text, Int) -> Map<Text, Int>"
        );
        assert_eq!(
            put.callable().unwrap().display(),
            "fn<K: equatable>(Map<K, V>, K, V) -> Map<K, V>"
        );
        let same = inspect_at("same(1)");
        assert_eq!(
            same.instantiated_callable().unwrap().display(),
            "fn(Int) -> Int"
        );
        assert_eq!(same.callable().unwrap().display(), "fn(T) -> T");
        assert!(inspect_at("counts,").instantiated_callable().is_none());
    }

    /// Guards that standard generic parameters print as readable names, never
    /// as the internal identifiers they are registered under.
    #[test]
    fn standard_contracts_print_without_internal_generic_names() {
        let typed = check_project(resolve_project_sources(vec![module(
            "main",
            "main.chimi",
            "pub fn main() -> Int { 0 }\n",
        )]));
        let leaked = typed
            .contracts
            .values()
            .map(|contract| typed.format_function_contract(contract))
            .filter(|display| display.contains('$'))
            .collect::<Vec<_>>();
        assert!(leaked.is_empty(), "{leaked:#?}");
    }

    #[test]
    fn malformed_snapshots_remain_inspectable() {
        let source = "pub fn main() {\n  Answer(value 42)\n";
        let offset = source.find("value").unwrap();
        let report = Compiler::inspect_project(
            vec![module("main", "malformed.chimi", source)],
            "main",
            offset,
        )
        .expect("syntax damage does not remove the CST cursor");

        assert_eq!(report.token().unwrap().text(), "value");
        assert!(!report.syntax_path().is_empty());
        assert!(!report.diagnostics().is_empty());
    }

    #[test]
    fn inspection_rejects_offsets_inside_utf8_code_points() {
        let source = "💥\n";
        let error =
            Compiler::inspect_project(vec![module("main", "unicode.chimi", source)], "main", 1)
                .expect_err("byte offsets inside UTF-8 code points are invalid");

        assert_eq!(
            error.to_string(),
            "inspection offset 1 splits a UTF-8 code point"
        );
    }

    #[test]
    fn parsed_project_sources_reuse_the_exact_frontend_snapshot() {
        let source = ProjectSource::parsed(
            "main",
            SourceSnapshot::from_text("main.chimi", "pub fn main() { 1 }\n"),
        )
        .expect("main is a valid module path");
        let parsed = source.syntax.as_ref().unwrap().clone();

        let resolved = super::resolve_project_sources(vec![source]);

        assert!(Arc::ptr_eq(&parsed, &resolved.semantic.modules[0].syntax));
    }

    #[test]
    fn reused_parse_diagnostics_are_rebased_to_deterministic_file_ids() {
        let valid = ProjectSource::parsed(
            "a",
            SourceSnapshot::from_text("a.chimi", "pub fn valid() { 1 }\n"),
        )
        .unwrap();
        let malformed = ProjectSource::parsed(
            "z",
            SourceSnapshot::from_text("z.chimi", "pub fn broken() {\n"),
        )
        .unwrap();

        let report = Compiler::type_project(vec![malformed, valid]);

        let diagnostic = report
            .diagnostics()
            .iter()
            .find(|diagnostic| diagnostic.stage() == DiagnosticStage::Syntax)
            .expect("malformed reused parse retains its syntax diagnostic");
        assert_eq!(
            diagnostic.file_id(),
            crate::ids::FileId::new(1),
            "{:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn project_core_executes_retry_as_ordinary_thunk_composition() {
        const APP: &str = include_str!("../../../prototypes/retry-composition/app/main.chimi");
        let report = Compiler::run_project(
            vec![
                module("app/main", "app/main.chimi", APP),
                module(
                    "lib/retry",
                    "lib/retry.chimi",
                    include_str!("../../../prototypes/retry-composition/lib/retry.chimi"),
                ),
            ],
            ExecutionConfig::new("retry-composition").unwrap(),
        )
        .expect("ordinary higher-order functions implement retry");

        assert!(matches!(
            report.outcome,
            ExecutionOutcome::ExpectedFailure(ExecutionValue::Constructed {
                ref type_name,
                variant_name: Some(ref variant),
                ..
            }) if type_name == "AttemptError" && variant == "Temporary"
        ));
        assert_eq!(
            report
                .events
                .iter()
                .filter(|event| event.kind == EventKind::DependencyCalled)
                .count(),
            2,
            "the composed thunk must invoke the required operation twice"
        );
    }

    #[test]
    fn project_core_executes_the_json_and_map_standard_surface() {
        let report = Compiler::run_project(
            vec![module(
                "stdlib/summary",
                "summary.chimi",
                concat!(
                    "import std/json as Json\n",
                    "import std/json.{Value}\n",
                    "import std/map as Map\n",
                    "import std/map.{Map}\n",
                    "\n",
                    "fn increment(counts: Map<Text, Int>, key: Text) {\n",
                    "  Map.update(counts, key, fn(current) {\n",
                    "    case current {\n",
                    "      Some(count) -> count + 1\n",
                    "      None -> 1\n",
                    "    }\n",
                    "  })\n",
                    "}\n",
                    "\n",
                    "pub fn main() -> Text {\n",
                    "  let empty = Map.empty()\n",
                    "  let once = increment(empty, \"zeta\")\n",
                    "  let twice = increment(once, \"alpha\")\n",
                    "  let counts = increment(twice, \"zeta\")\n",
                    "  let encoded = Json.encode(Json.object(Map.map_values(counts, Json.int)))\n",
                    "  let decoded = Json.decode(encoded)\n",
                    "  case decoded {\n",
                    "    Value.Object(fields) -> Json.encode(Json.object(fields))\n",
                    "    _ -> \"unexpected\"\n",
                    "  }\n",
                    "}\n",
                ),
            )],
            ExecutionConfig::new("stdlib-summary").unwrap(),
        )
        .expect("official JSON and map modules compile and execute");

        assert_eq!(
            report.outcome,
            ExecutionOutcome::Success(ExecutionValue::Text("{\"alpha\":1,\"zeta\":2}".to_owned()))
        );
    }

    #[test]
    fn json_null_remains_distinct_from_map_absence() {
        let report = Compiler::run_project(
            vec![module(
                "stdlib/null",
                "null.chimi",
                concat!(
                    "import std/json as Json\n",
                    "import std/json.{Value}\n",
                    "import std/map as Map\n",
                    "import std/map.{Map}\n",
                    "\n",
                    "fn classify(fields: Map<Text, Value>) -> Text {\n",
                    "  case Map.get(fields, \"value\") {\n",
                    "    None -> \"null\"\n",
                    "    Some(_) -> \"present\"\n",
                    "  }\n",
                    "}\n",
                    "\n",
                    "pub fn main() -> Text {\n",
                    "  let decoded = Json.decode(\"null\")\n",
                    "  case decoded {\n",
                    "    Value.Null -> classify(Map.empty())\n",
                    "    _ -> \"not-null\"\n",
                    "  }\n",
                    "}\n",
                ),
            )],
            ExecutionConfig::new("stdlib-null").unwrap(),
        )
        .expect("JSON null and map absence have distinct patterns");

        assert_eq!(
            report.outcome,
            ExecutionOutcome::Success(ExecutionValue::Text("null".to_owned()))
        );
    }

    #[test]
    fn malformed_json_is_an_expected_failure() {
        let report = Compiler::run_project(
            vec![module(
                "stdlib/malformed",
                "malformed.chimi",
                concat!(
                    "import std/json as Json\n",
                    "import std/json.{Value}\n",
                    "\n",
                    "pub fn main() -> Value { Json.decode(\"{\") }\n",
                ),
            )],
            ExecutionConfig::new("stdlib-malformed").unwrap(),
        )
        .expect("malformed JSON reaches the expected failure channel");

        assert!(matches!(
            report.outcome,
            ExecutionOutcome::ExpectedFailure(ExecutionValue::Constructed {
                ref type_name,
                variant_name: Some(ref variant),
                ..
            }) if type_name == "DecodeError" && variant == "InvalidJson"
        ));
    }

    #[test]
    fn map_transformations_preserve_callback_failure_channels() {
        let report = Compiler::run_project(
            vec![module(
                "stdlib/map_failure",
                "map_failure.chimi",
                concat!(
                    "import std/map as Map\n",
                    "import std/map.{Map}\n",
                    "\n",
                    "enum UpdateError { Stopped }\n",
                    "\n",
                    "pub fn main() -> Map<Text, Int> {\n",
                    "  Map.update(Map.empty(), \"key\", fn(current) {\n",
                    "    case current {\n",
                    "      Some(count) -> count + 1\n",
                    "      None -> Failure(Stopped())\n",
                    "    }\n",
                    "  })\n",
                    "}\n",
                ),
            )],
            ExecutionConfig::new("stdlib-map-failure").unwrap(),
        )
        .expect("map callback failures remain expected values");

        assert!(matches!(
            report.outcome,
            ExecutionOutcome::ExpectedFailure(ExecutionValue::Constructed {
                ref type_name,
                variant_name: Some(ref variant),
                ..
            }) if type_name == "UpdateError" && variant == "Stopped"
        ));
    }

    #[test]
    fn pure_chimi_classifies_and_aggregates_trace_records() {
        let report = Compiler::run_project(
            vec![module(
                "app/main",
                "app/main.chimi",
                include_str!("../../../programs/chimi-trace-summary/app/main.chimi"),
            )],
            ExecutionConfig::new("trace-summary-core")
                .unwrap()
                .with_application_arguments(["-"])
                .with_standard_input(concat!(
                    "{\"schema\":\"chimi.runtime.v1\",\"event\":\"operation_completed\",",
                    "\"sequence\":0,\"execution_id\":\"example\",\"computation\":\"main\",",
                    "\"operation\":\"call\"}\n",
                    "{\"schema\":\"chimi.runtime.v1\",\"event\":\"dependency_called\",",
                    "\"sequence\":1,\"execution_id\":\"example\",\"computation\":\"main\",",
                    "\"attributes\":{\"requirement\":\"payment\",\"method\":\"charge\"}}\n",
                    "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n",
                )),
        )
        .expect("trace summary executes through ordinary Chimi code");

        assert_eq!(
            report.outcome,
            ExecutionOutcome::Success(ExecutionValue::Void)
        );
        assert_eq!(
            report.application_stdout,
            concat!(
                "{\"dependency_calls\":{\"payment.charge\":1},",
                "\"events\":2,\"operations\":{\"call\":1},",
                "\"records\":3,\"schema\":\"chimi.trace-summary.v1\",",
                "\"terminal_outcomes\":{\"success\":1}}\n",
            )
        );
    }

    #[test]
    fn oracle_supplies_arguments_and_scoped_host_capabilities() {
        let input = std::env::temp_dir().join(format!(
            "chimi-oracle-host-io-{}-{:?}.txt",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&input, "\"alpha\"\r\n\n\"β\"").unwrap();
        let report = Compiler::run_project(
            vec![module(
                "app/main",
                "app/main.chimi",
                include_str!("../tests/fixtures/project-host-io/app/main.chimi"),
            )],
            ExecutionConfig::new("oracle-host-io")
                .unwrap()
                .with_application_arguments([input.to_string_lossy()]),
        )
        .expect("host-capability root executes through the oracle");

        assert_eq!(
            report.outcome,
            ExecutionOutcome::Success(ExecutionValue::Void)
        );
        assert_eq!(report.application_stdout, "\"alpha\"\n\"β\"\n");
        assert_eq!(report.application_stderr, "processed input\n");
        assert_eq!(
            report
                .events
                .iter()
                .filter(|event| event.kind == EventKind::DependencyCalled)
                .count(),
            4
        );
        std::fs::remove_file(input).unwrap();
    }

    // G7 E5203: each constant's evaluation stops at its operation budget, and a value
    // larger than the size budget is not stored. Small budgets stand in for the build's.
    #[test]
    fn constant_evaluation_enforces_its_operation_and_size_budgets() {
        let source = concat!(
            "fn count(limit: Int) -> Int {\n",
            "  loop value = 0 {\n",
            "    case value < limit {\n",
            "      True -> value + 1\n",
            "      False -> break value\n",
            "    }\n",
            "  }\n",
            "}\n",
            "const small = count(2)\n",
            "const counted = count(1_000)\n",
            "const label = \"a label longer than the tiny size budget\"\n",
            "pub fn main() -> Int { small + counted }\n",
        );
        let mut core = crate::executable::lower(
            crate::executable::seal_entry(crate::typing::check_project(resolve_project_sources(
                vec![module("main", "main.chimi", source)],
            )))
            .expect("program verifies"),
        )
        .expect("the build's budgets admit every constant");
        // Re-evaluate from scratch under the small budgets.
        core.constants.clear();
        let codes = |limits| match crate::execution::evaluate_constants_with(&core, limits) {
            Ok(_) => Vec::new(),
            Err(diagnostics) => diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.code(), diagnostic.message().to_owned()))
                .collect::<Vec<_>>(),
        };
        let operations = codes(crate::execution::ConstantLimits {
            operations: 500,
            call_depth: 10_000,
            value_bytes: 1024,
        });
        assert_eq!(operations.len(), 1, "{operations:?}");
        assert_eq!(operations[0].0, "E5203");
        assert!(operations[0].1.contains("`counted`"), "{operations:?}");
        let size = codes(crate::execution::ConstantLimits {
            operations: 1_000_000,
            call_depth: 10_000,
            value_bytes: 32,
        });
        assert_eq!(size.len(), 1, "{size:?}");
        assert_eq!(size[0].0, "E5203");
        assert!(size[0].1.contains("`label`"), "{size:?}");
    }

    // G7 E5203: the size budget charges a shared part once per reference, because the
    // stored value and the native table hold a copy for each one. Under a budget that
    // fits `inner` but not two copies of it, `pair` fails while `inner` and `again`,
    // which refers to `inner` once, are stored.
    #[test]
    fn constant_size_budget_charges_a_shared_part_per_reference() {
        let source = concat!(
            "const inner = [1, 2, 3, 4, 5, 6, 7, 8]\n",
            "const again = [inner]\n",
            "const pair = [inner, inner]\n",
            "pub fn main() -> Int { 0 }\n",
        );
        let mut core = crate::executable::lower(
            crate::executable::seal_entry(crate::typing::check_project(resolve_project_sources(
                vec![module("main", "main.chimi", source)],
            )))
            .expect("program verifies"),
        )
        .expect("the build's budgets admit every constant");
        core.constants.clear();
        let limits = crate::execution::ConstantLimits {
            operations: 1_000_000,
            call_depth: 10_000,
            value_bytes: 400,
        };
        let diagnostics = match crate::execution::evaluate_constants_with(&core, limits) {
            Ok(_) => Vec::new(),
            Err(diagnostics) => diagnostics,
        };
        let failed = diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.code(), diagnostic.message().to_owned()))
            .collect::<Vec<_>>();
        assert_eq!(failed.len(), 1, "{failed:?}");
        assert_eq!(failed[0].0, "E5203");
        assert!(failed[0].1.contains("`pair`"), "{failed:?}");
    }

    // ts-a512d8: constant evaluation measures a constant's value after every step,
    // and records, tuples, and constructed values are shared, so `Measure` caches
    // their sizes by address. Growing a constant tree that holds the previous level
    // twice then measures a few new values per level. Without the cache every step
    // walks the whole tree, 2^depth values: 256 times as many for 16 levels as for 8.
    #[test]
    fn measuring_a_growing_constant_is_linear_in_its_depth() {
        let measured = |depth: u32| {
            let source = format!(
                concat!(
                    "enum Tree {{\n  Leaf\n  Node(left: Tree, right: Tree)\n}}\n",
                    "struct Pair {{ tree: Tree, levels: Int }}\n",
                    "fn grown(depth: Int) -> Pair {{\n",
                    "  loop state = {{ left: depth, pair: Pair(Leaf, 0) }} {{\n",
                    "    case state.left {{\n",
                    "      0 -> break state.pair\n",
                    "      _ -> {{\n",
                    "        left: state.left - 1,\n",
                    "        pair: Pair(Node(state.pair.tree, state.pair.tree), state.pair.levels + 1),\n",
                    "      }}\n",
                    "    }}\n",
                    "  }}\n",
                    "}}\n",
                    "const grown_tree = grown({depth})\n",
                    "pub fn main() -> Int {{ grown_tree.levels }}\n",
                ),
                depth = depth
            );
            let mut core = crate::executable::lower(
                crate::executable::seal_entry(crate::typing::check_project(
                    resolve_project_sources(vec![module("main", "main.chimi", &source)]),
                ))
                .expect("program verifies"),
            )
            .expect("the build's budgets admit every constant");
            core.constants.clear();
            let (measured, evaluated) = crate::execution::measured_values_to_evaluate(
                &core,
                crate::execution::ConstantLimits::BUILD,
            );
            assert!(evaluated, "the tree fits the build's budgets");
            measured
        };
        let base = measured(0);
        let eight = measured(8) - base;
        let sixteen = measured(16) - base;
        assert!(
            sixteen <= 4 * eight,
            "16 levels measured {sixteen} values, 8 levels {eight}"
        );
    }

    #[test]
    fn module_binding_closure_inference_converges() {
        let report = Compiler::type_project(vec![module(
            "typing/closure_binding",
            "closure_binding.chimi",
            concat!(
                "const provider = { apply: fn(_value) { 1 } }\n",
                "pub fn main() -> Int { 1 }\n",
            ),
        )]);
        // The open parameter type settles as undetermined instead of growing a fresh
        // inference variable every pass, so the fixed point converges with one T4324.
        let codes = report
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code())
            .collect::<Vec<_>>();
        assert_eq!(codes, ["T4324"]);
    }

    #[test]
    fn task_contracts_capture_needs_and_store_expected_failures() {
        let report = Compiler::type_project(vec![module(
            "concurrency/task_contracts",
            "task_contracts.chimi",
            concat!(
                "import std/task as Task\n",
                "import std/task.{Task}\n",
                "\n",
                "enum WorkError { Stopped }\n",
                "interface Clock { now: fn() -> Int }\n",
                "\n",
                "fn work() -> Int needs { clock: Clock } {\n",
                "  case clock.now() {\n",
                "    0 -> Failure(Stopped())\n",
                "    _ -> 42\n",
                "  }\n",
                "}\n",
                "\n",
                "fn launch() -> Task<Int, WorkError> needs { clock: Clock } {\n",
                "  Task.start(work)\n",
                "}\n",
                "\n",
                "fn await(task: Task<Int, WorkError>) -> Int { Task.result(task) }\n",
                "fn stop(task: Task<Int, WorkError>) -> Void { Task.cancel(task) }\n",
            ),
        )]);

        assert!(report.is_success(), "{}", report.render_diagnostics());
        assert_eq!(
            report
                .contract("concurrency/task_contracts", "launch")
                .as_deref(),
            Some("fn() -> Task<Int, WorkError> needs { clock: Clock }")
        );
        assert_eq!(
            report
                .contract("concurrency/task_contracts", "await")
                .as_deref(),
            Some("fn(Task<Int, WorkError>) -> Int errors WorkError")
        );
        assert_eq!(
            report
                .contract("concurrency/task_contracts", "stop")
                .as_deref(),
            Some("fn(Task<Int, WorkError>) -> Void")
        );
    }

    #[test]
    fn value_properties_derive_through_data_and_runtime_capabilities() {
        let report = Compiler::type_project(vec![module(
            "concurrency/value_properties",
            "value_properties.chimi",
            concat!(
                "import std/channel.{Completion, Receiver, Sender}\n",
                "import std/task.{Task}\n",
                "\n",
                "enum WorkError { Stopped }\n",
                "struct Payload { label: Text, counts: List<Int> }\n",
                "struct Envelope<A> { value: A }\n",
                "enum Consumer { Waiting, Active(receiver: Receiver<Int>) }\n",
                "\n",
                "fn payload(value: Payload) -> Payload { value }\n",
                "fn sender(value: Sender<Int>) -> Sender<Int> { value }\n",
                "fn receiver(value: Receiver<Int>) -> Receiver<Int> { value }\n",
                "fn completion(value: Completion<Int>) -> Completion<Int> { value }\n",
                "fn wrapped(value: Envelope<Receiver<Int>>) -> Envelope<Receiver<Int>> { value }\n",
                "fn consumer(value: Consumer) -> Consumer { value }\n",
                "fn child(value: Task<Int, WorkError>) -> Task<Int, WorkError> { value }\n",
                "fn callback(value: fn() -> Int) -> fn() -> Int { value }\n",
            ),
        )]);
        assert!(report.is_success(), "{}", report.render_diagnostics());

        let always = ValuePropertyStatus::Always;
        let never = ValuePropertyStatus::Never;
        let dependent = ValuePropertyStatus::ValueDependent;
        let ordinary = ValueCapabilityClass::Ordinary;
        let exclusive = ValueCapabilityClass::Exclusive;
        let parent_local = ValueCapabilityClass::ParentLocal;
        let properties = |function| {
            report
                .function_input_value_properties("concurrency/value_properties", function, 0)
                .expect("fixture function has one input")
        };

        assert_eq!(
            properties("payload"),
            ValuePropertyReport {
                transferable: always,
                shareable: always,
                capability: ordinary,
                equatable: always,
                ordered: always,
            }
        );
        assert_eq!(
            properties("sender"),
            ValuePropertyReport {
                transferable: always,
                shareable: always,
                capability: ordinary,
                equatable: never,
                ordered: never,
            }
        );
        for function in ["receiver", "completion", "wrapped", "consumer"] {
            assert_eq!(
                properties(function),
                ValuePropertyReport {
                    transferable: always,
                    shareable: never,
                    capability: exclusive,
                    equatable: never,
                    ordered: never,
                },
                "{function}"
            );
        }
        assert_eq!(
            properties("child"),
            ValuePropertyReport {
                transferable: never,
                shareable: never,
                capability: parent_local,
                equatable: never,
                ordered: never,
            }
        );
        assert_eq!(
            properties("callback"),
            ValuePropertyReport {
                transferable: dependent,
                shareable: dependent,
                capability: ordinary,
                equatable: never,
                ordered: never,
            }
        );
    }

    #[test]
    fn shared_closures_remain_reusable_across_task_starts() {
        let report = Compiler::type_project(vec![module(
            "concurrency/shared_task_capture",
            "shared_task_capture.chimi",
            concat!(
                "import std/channel.{Sender}\n",
                "import std/task as Task\n",
                "\n",
                "struct Payload { value: Int }\n",
                "\n",
                "fn start_shared(sender: Sender<Int>, payload: Payload) -> Int {\n",
                "  let thunk = fn() -> Int {\n",
                "    sender\n",
                "    payload\n",
                "    1\n",
                "  }\n",
                "  let first = Task.start(thunk)\n",
                "  let second = Task.start(thunk)\n",
                "  1\n",
                "}\n",
            ),
        )]);

        assert!(report.is_success(), "{}", report.render_diagnostics());
    }

    #[test]
    fn exclusive_and_parent_local_captures_report_ownership_transitions() {
        let report = Compiler::type_project(vec![module(
            "concurrency/invalid_task_capture",
            "invalid_task_capture.chimi",
            concat!(
                "import std/channel.{Receiver}\n",
                "import std/task as Task\n",
                "import std/task.{Task}\n",
                "\n",
                "enum WorkError { Stopped }\n",
                "\n",
                "fn start_twice(receiver: Receiver<Int>) -> Int {\n",
                "  let thunk = fn() -> Int {\n",
                "    receiver\n",
                "    1\n",
                "  }\n",
                "  let first = Task.start(thunk)\n",
                "  let second = Task.start(thunk)\n",
                "  1\n",
                "}\n",
                "\n",
                "fn use_old_owner(receiver: Receiver<Int>) -> Int {\n",
                "  let thunk = fn() -> Int {\n",
                "    receiver\n",
                "    1\n",
                "  }\n",
                "  receiver\n",
                "  1\n",
                "}\n",
                "\n",
                "fn capture_twice(receiver: Receiver<Int>) -> Int {\n",
                "  let first = fn() -> Int { receiver\n 1 }\n",
                "  let second = fn() -> Int { receiver\n 2 }\n",
                "  1\n",
                "}\n",
                "\n",
                "fn capture_child(child: Task<Int, WorkError>) -> Int {\n",
                "  let nested = Task.start(fn() -> Int { child\n 1 })\n",
                "  1\n",
                "}\n",
            ),
        )]);

        assert!(!report.is_success());
        let codes = report
            .diagnostics()
            .iter()
            .map(Diagnostic::code)
            .collect::<Vec<_>>();
        assert_eq!(codes, ["T4241", "T4241", "T4241", "T4240"]);
        let rendered = report.render_diagnostics();
        assert!(rendered.contains("`thunk` was transferred"));
        assert!(rendered.contains("`receiver` was transferred"));
        assert!(rendered.contains("cannot be captured again"));
        assert!(rendered.contains("capture `child` (Task<Int, WorkError>) is parent-local"));
    }

    #[test]
    fn task_start_requires_attached_providers_to_be_shareable() {
        let report = Compiler::type_project(vec![module(
            "concurrency/task_provider_safety",
            "task_provider_safety.chimi",
            concat!(
                "import std/channel.{Receiver}\n",
                "import std/task as Task\n",
                "\n",
                "interface Store { read: fn() -> Int }\n",
                "fn work() -> Int needs { store: Store } { store.read() }\n",
                "\n",
                "fn shared_provider() -> Int {\n",
                "  let provider = { read: fn() -> Int { 1 } }\n",
                "  let child = Task.start(work using { store: provider })\n",
                "  1\n",
                "}\n",
                "\n",
                "fn task_local_provider(receiver: Receiver<Int>) -> Int {\n",
                "  let provider = { read: fn() -> Int { receiver\n 1 } }\n",
                "  let child = Task.start(work using { store: provider })\n",
                "  1\n",
                "}\n",
            ),
        )]);

        assert_eq!(
            report
                .diagnostics()
                .iter()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            ["T4242"]
        );
        assert!(
            report
                .render_diagnostics()
                .contains("provider `store` to be shareable across tasks")
        );
    }

    #[test]
    fn channel_operations_expose_transfer_recovery_and_terminal_contracts() {
        let report = Compiler::type_project(vec![module(
            "concurrency/channel_contracts",
            "channel_contracts.chimi",
            concat!(
                "import std/channel as Channel\n",
                "import std/channel.{Completion, Receiver, Receive, Sender, SendError}\n",
                "\n",
                "fn send(sender: Sender<Int>, value: Int) -> Void {\n",
                "  Channel.send(sender, value)\n",
                "}\n",
                "fn receive(receiver: Receiver<Int>) -> Receive<Int> {\n",
                "  Channel.receive(receiver)\n",
                "}\n",
                "fn close(completion: Completion<Int>) -> Void {\n",
                "  Channel.close(completion)\n",
                "}\n",
                "fn abort(receiver: Receiver<Int>) -> Void {\n",
                "  Channel.abort(receiver)\n",
                "}\n",
                "fn recover(\n",
                "  sender: Sender<Receiver<Int>>,\n",
                "  value: Receiver<Int>,\n",
                ") -> Option<Receiver<Int>> {\n",
                "  case Channel.send(sender, value) {\n",
                "    Success(_) -> None()\n",
                "    Failure(error) -> case error {\n",
                "      SendError.Closed(recovered) -> Some(recovered)\n",
                "      SendError.Aborted(recovered) -> Some(recovered)\n",
                "    }\n",
                "  }\n",
                "}\n",
            ),
        )]);

        assert!(report.is_success(), "{}", report.render_diagnostics());
        assert_eq!(
            report
                .contract("concurrency/channel_contracts", "send")
                .as_deref(),
            Some("fn(Sender<Int>, Int) -> Void errors SendError<Int>")
        );
        assert_eq!(
            report
                .contract("concurrency/channel_contracts", "receive")
                .as_deref(),
            Some("fn(Receiver<Int>) -> Receive<Int> errors ChannelError")
        );
        assert_eq!(
            report
                .contract("concurrency/channel_contracts", "close")
                .as_deref(),
            Some("fn(Completion<Int>) -> Void")
        );
        assert_eq!(
            report
                .contract("concurrency/channel_contracts", "abort")
                .as_deref(),
            Some("fn(Receiver<Int>) -> Void")
        );
        assert_eq!(
            report
                .contract("concurrency/channel_contracts", "recover")
                .as_deref(),
            Some("fn(Sender<Receiver<Int>>, Receiver<Int>) -> Option<Receiver<Int>>")
        );
    }

    #[test]
    fn channel_send_rejects_parent_local_payloads_and_transfers_exclusive_values() {
        let report = Compiler::type_project(vec![module(
            "concurrency/invalid_channel_transfer",
            "invalid_channel_transfer.chimi",
            concat!(
                "import std/channel as Channel\n",
                "import std/channel.{Receiver, Sender, SendError}\n",
                "import std/task as Task\n",
                "import std/task.{Task}\n",
                "\n",
                "enum WorkError { Stopped }\n",
                "\n",
                "fn use_after_send(\n",
                "  sender: Sender<Receiver<Int>>,\n",
                "  value: Receiver<Int>,\n",
                ") -> Int {\n",
                "  Channel.send(sender, value)\n",
                "  value\n",
                "  1\n",
                "}\n",
                "fn send_child(\n",
                "  sender: Sender<Task<Int, WorkError>>,\n",
                "  child: Task<Int, WorkError>,\n",
                ") -> Void {\n",
                "  Channel.send(sender, child)\n",
                "}\n",
                "fn start_recovered_twice(\n",
                "  sender: Sender<Receiver<Int>>,\n",
                "  value: Receiver<Int>,\n",
                ") -> Int {\n",
                "  let thunk = case Channel.send(sender, value) {\n",
                "    Success(_) -> fn() -> Int { 1 }\n",
                "    Failure(error) -> case error {\n",
                "      SendError.Closed(recovered) -> fn() -> Int { recovered\n 1 }\n",
                "      SendError.Aborted(recovered) -> fn() -> Int { recovered\n 1 }\n",
                "    }\n",
                "  }\n",
                "  let first = Task.start(thunk)\n",
                "  let second = Task.start(thunk)\n",
                "  1\n",
                "}\n",
            ),
        )]);

        assert_eq!(
            report
                .diagnostics()
                .iter()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            ["T4241", "T4243", "T4241"]
        );
        let rendered = report.render_diagnostics();
        assert!(rendered.contains("`value` was transferred by Channel.send"));
        assert!(rendered.contains("Channel.send cannot transfer Task<Int, WorkError>"));
    }

    #[test]
    fn channel_send_failure_recovers_the_exact_payload_type() {
        let report = Compiler::type_project(vec![module(
            "concurrency/wrong_channel_recovery",
            "wrong_channel_recovery.chimi",
            concat!(
                "import std/channel as Channel\n",
                "import std/channel.{Receiver, Sender, SendError}\n",
                "\n",
                "fn recover_as_text(\n",
                "  sender: Sender<Receiver<Int>>,\n",
                "  value: Receiver<Int>,\n",
                ") -> Option<Text> {\n",
                "  case Channel.send(sender, value) {\n",
                "    Success(_) -> None()\n",
                "    Failure(error) -> case error {\n",
                "      SendError.Closed(recovered) -> Some(recovered)\n",
                "      SendError.Aborted(recovered) -> Some(recovered)\n",
                "    }\n",
                "  }\n",
                "}\n",
            ),
        )]);

        assert!(report.diagnostics().iter().any(|diagnostic| {
            diagnostic.code() == "T4203" && diagnostic.stage() == DiagnosticStage::Type
        }));
    }

    #[test]
    fn channel_capability_fixtures_remain_connected() {
        let valid = Compiler::type_project(vec![module(
            "concurrency/fixture_valid",
            "channel-static-valid.chimi",
            include_str!("../tests/fixtures/channel-static-valid.chimi"),
        )]);
        assert!(valid.is_success(), "{}", valid.render_diagnostics());

        let invalid = Compiler::type_project(vec![module(
            "concurrency/fixture_invalid",
            "channel-static-invalid.chimi",
            include_str!("../tests/fixtures/channel-static-invalid.chimi"),
        )]);
        assert_eq!(
            invalid
                .diagnostics()
                .iter()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            ["T4241", "T4243", "T4241"]
        );

        let wrong_recovery = Compiler::type_project(vec![module(
            "concurrency/fixture_wrong_recovery",
            "channel-static-wrong-recovery.chimi",
            include_str!("../tests/fixtures/channel-static-wrong-recovery.chimi"),
        )]);
        assert!(
            wrong_recovery
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code() == "T4203")
        );
    }

    #[test]
    fn project_execution_preserves_all_terminal_outcome_classes() {
        let failure = Compiler::run_project(
            vec![module(
                "runtime/failure",
                "failure.chimi",
                concat!(
                    "enum Problem { Failed }\n",
                    "pub fn main() -> Int { Failure(Failed()) }\n",
                ),
            )],
            ExecutionConfig::new("project-failure").unwrap(),
        )
        .expect("fallible roots execute");
        assert!(matches!(
            &failure.outcome,
            ExecutionOutcome::ExpectedFailure(ExecutionValue::Constructed {
                type_name,
                variant_name: Some(variant),
                ..
            }) if type_name == "Problem" && variant == "Failed"
        ));

        let defect = Compiler::run_project(
            vec![module(
                "runtime/defect",
                "defect.chimi",
                "pub fn main() -> Int { 1 / 0 }",
            )],
            ExecutionConfig::new("project-defect").unwrap(),
        )
        .expect("runtime defects occur after successful compilation");
        assert!(matches!(
            defect.outcome,
            ExecutionOutcome::Defect(ref defect) if defect.message.starts_with("integer division by zero")
        ));

        let cancellation = Compiler::run_project(
            vec![module(
                "runtime/cancel",
                "cancel.chimi",
                "pub fn main() -> Int { 1 }",
            )],
            ExecutionConfig::new("project-cancel")
                .unwrap()
                .with_cancellation_requested(),
        )
        .expect("cancelled execution still requires a verified root");
        assert!(matches!(
            cancellation.outcome,
            ExecutionOutcome::Cancellation(_)
        ));
        assert_eq!(cancellation.events.len(), 2);
    }

    fn run_loop(text: &str, trace_mode: TraceMode) -> crate::execution::ExecutionReport {
        Compiler::run_project(
            vec![module("app/main", "main.chimi", text)],
            ExecutionConfig::new("loop-test")
                .unwrap()
                .with_trace_mode(trace_mode),
        )
        .expect("loop source executes")
    }

    fn loop_diagnostics(text: &str) -> Vec<&'static str> {
        Compiler::verify_project(vec![module("app/main", "main.chimi", text)])
            .err()
            .unwrap_or_default()
            .iter()
            .map(Diagnostic::code)
            .collect()
    }

    /// A module whose `main` decodes `2^doublings` JSON ones and passes them to `run`.
    fn large_list_loop(doublings: u32, run: &str) -> String {
        format!(
            concat!(
                "import std/json as Json\n",
                "import std/json.{{Value}}\n",
                "import std/text as Text\n",
                "fn ones(doublings: Int) -> Text {{\n",
                "  loop state = {{ text: \"1\", left: doublings }} {{\n",
                "    case state.left {{\n",
                "      0 -> break state.text\n",
                "      _ -> {{ text: Text.append(Text.append(state.text, \",\"), state.text), left: state.left - 1 }}\n",
                "    }}\n",
                "  }}\n",
                "}}\n",
                "{run}",
                "pub fn main() -> Int {{\n",
                "  let decoded = Json.decode(Text.append(Text.append(\"[\", ones({doublings})), \"]\"))\n",
                "  case decoded {{\n",
                "    Value.Array(values) -> run(values)\n",
                "    _ -> -1\n",
                "  }}\n",
                "}}\n",
            ),
            doublings = doublings,
            run = run
        )
    }

    // LP-07 in the interpreter: `[.., last]` reads one element, so 10,000 matches of
    // a 1,048,576-element list stay fast. A copy per match would copy about 10^10
    // elements.
    #[test]
    fn interpreter_last_element_pattern_does_not_copy_a_large_list() {
        let source = large_list_loop(
            20,
            concat!(
                "fn run(values: List<Value>) -> Int {\n",
                "  loop state = { left: 10000, found: 0 } {\n",
                "    case state.left {\n",
                "      0 -> break state.found\n",
                "      _ -> case values {\n",
                "        [.., Value.Number(_)] -> { left: state.left - 1, found: state.found + 1 }\n",
                "        _ -> { left: state.left - 1, found: state.found }\n",
                "      }\n",
                "    }\n",
                "  }\n",
                "}\n",
            ),
        );
        let started = std::time::Instant::now();
        let report = run_loop(&source, TraceMode::Off);
        let elapsed = started.elapsed();
        assert_eq!(
            report.outcome,
            ExecutionOutcome::Success(ExecutionValue::Integer(10_000))
        );
        assert!(elapsed.as_secs() < 30, "10,000 matches took {elapsed:?}");
    }

    // LP-08 in the interpreter: each `rest` view is O(1) amortized, so draining
    // 131,072 elements stays linear. A copy per view would copy about 8.6 * 10^9.
    #[test]
    fn interpreter_draining_loop_takes_constant_time_per_rest_view() {
        let source = large_list_loop(
            17,
            concat!(
                "fn run(values: List<Value>) -> Int {\n",
                "  loop state = { rest: values, total: 0 } {\n",
                "    case state.rest {\n",
                "      [] -> break state.total\n",
                "      [Value.Number(_), ..rest] -> { rest: rest, total: state.total + 1 }\n",
                "      [_, ..rest] -> { rest: rest, total: state.total }\n",
                "    }\n",
                "  }\n",
                "}\n",
            ),
        );
        let started = std::time::Instant::now();
        let report = run_loop(&source, TraceMode::Off);
        let elapsed = started.elapsed();
        assert_eq!(
            report.outcome,
            ExecutionOutcome::Success(ExecutionValue::Integer(1 << 17))
        );
        assert!(elapsed.as_secs() < 30, "draining took {elapsed:?}");
    }

    // Guards: unbounded repetition must not grow the host stack with the pass count.
    #[test]
    fn loop_runs_many_passes_without_growing_the_stack() {
        let report = run_loop(
            concat!(
                "pub fn main() -> Int {\n",
                "  loop count = 0 {\n",
                "    case count {\n",
                "      100000 -> break count\n",
                "      _ -> count + 1\n",
                "    }\n",
                "  }\n",
                "}\n",
            ),
            TraceMode::Off,
        );
        assert_eq!(
            report.outcome,
            ExecutionOutcome::Success(ExecutionValue::Integer(100_000))
        );
    }

    // Guards: each pass is traced as a sibling under the loop, not nested in the previous pass.
    #[test]
    fn loop_passes_are_sibling_spans_under_the_loop() {
        let report = run_loop(
            concat!(
                "pub fn main() -> Int {\n",
                "  loop count = 0 {\n",
                "    case count {\n",
                "      3 -> break count\n",
                "      _ -> count + 1\n",
                "    }\n",
                "  }\n",
                "}\n",
            ),
            TraceMode::Full,
        );
        let started = |operation: &str| {
            report
                .events
                .iter()
                .filter(|event| event.kind == EventKind::OperationStarted)
                .filter(|event| event.operation == operation)
                .collect::<Vec<_>>()
        };
        let loops = started("loop");
        assert_eq!(loops.len(), 1);
        let passes = started("sequence")
            .into_iter()
            .filter(|pass| pass.parent_span_id == Some(loops[0].span_id))
            .count();
        assert_eq!(passes, 4, "one body pass for each count 0, 1, 2, and 3");
        assert_eq!(started("match").len(), 4);
    }

    // Guards: break leaves only the nearest loop; the outer loop keeps its own state.
    #[test]
    fn inner_break_leaves_only_the_inner_loop() {
        let report = run_loop(
            concat!(
                "pub fn main() -> Int {\n",
                "  loop total = 0 {\n",
                "    let inner = loop step = 0 {\n",
                "      case step {\n",
                "        2 -> break 10\n",
                "        _ -> step + 1\n",
                "      }\n",
                "    }\n",
                "    case total {\n",
                "      30 -> break total\n",
                "      _ -> total + inner\n",
                "    }\n",
                "  }\n",
                "}\n",
            ),
            TraceMode::Off,
        );
        assert_eq!(
            report.outcome,
            ExecutionOutcome::Success(ExecutionValue::Integer(30))
        );
    }

    // Guards: an expected failure in a pass ends the loop through the failure channel.
    #[test]
    fn failure_in_a_pass_ends_the_loop_as_a_failure() {
        let report = run_loop(
            concat!(
                "enum CountError { TooFar(at: Int) }\n",
                "pub fn main() -> Int {\n",
                "  loop count = 0 {\n",
                "    case count {\n",
                "      2 -> Failure(TooFar(count))\n",
                "      _ -> count + 1\n",
                "    }\n",
                "  }\n",
                "}\n",
            ),
            TraceMode::Off,
        );
        // The payload names the pass that failed: earlier passes ran, later ones did not.
        assert_eq!(
            report.outcome,
            ExecutionOutcome::ExpectedFailure(ExecutionValue::Constructed {
                type_name: "CountError".to_owned(),
                variant_name: Some("TooFar".to_owned()),
                fields: vec![ValueField {
                    name: "at".to_owned(),
                    value: ExecutionValue::Integer(2),
                }],
            })
        );
    }

    // Guards: a forgotten state update is a type error, not a silent stale pass.
    #[test]
    fn body_value_must_have_the_state_type() {
        let codes = loop_diagnostics(concat!(
            "pub fn main() -> Int {\n",
            "  loop count = 0 {\n",
            "    case count {\n",
            "      3 -> break count\n",
            "      _ -> \"next\"\n",
            "    }\n",
            "  }\n",
            "}\n",
        ));
        assert_eq!(codes, ["T4203"]);
    }

    // Guards: break never crosses a function boundary, including a closure in a loop.
    #[test]
    fn break_outside_a_loop_of_its_own_function_is_rejected() {
        let codes = loop_diagnostics(concat!(
            "fn outside() -> Int {\n",
            "  break 1\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  loop count = 0 {\n",
            "    let escape = fn() -> Int { break count }\n",
            "    count + 1\n",
            "  }\n",
            "}\n",
        ));
        assert_eq!(codes, ["E_BREAK_OUTSIDE_LOOP", "E_BREAK_OUTSIDE_LOOP"]);
    }

    // Guards: a loop with no break never returns, so it fits any expected type.
    #[test]
    fn loop_without_break_fits_any_expected_type() {
        let codes = loop_diagnostics(concat!(
            "fn spin() -> Text {\n",
            "  loop count = 0 {\n",
            "    count + 1\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int { 1 }\n",
        ));
        assert!(codes.is_empty(), "{codes:?}");
    }

    // Guards: a pass may not transfer a capability that the next pass would use again.
    #[test]
    fn loop_body_cannot_transfer_an_outer_capability() {
        let codes = loop_diagnostics(concat!(
            "import std/channel as Channel\n",
            "import std/task as Task\n",
            "pub fn main() -> Int {\n",
            "  let channel = Channel.bounded(1)\n",
            "  let receiver = channel.receiver\n",
            "  loop count = 0 {\n",
            "    let child = Task.start(fn() { Channel.receive(receiver) })\n",
            "    case count {\n",
            "      2 -> break count\n",
            "      _ -> count + 1\n",
            "    }\n",
            "  }\n",
            "}\n",
        ));
        assert_eq!(codes, ["T4250"]);
    }

    // Regression: a block whose first item is a call with several arguments is a block,
    // not a record; only a comma at the brace's own nesting level marks a record.
    #[test]
    fn block_starting_with_a_multi_argument_call_is_a_block() {
        let report = run_loop(
            concat!(
                "fn add(left: Int, right: Int) -> Int { left + right }\n",
                "pub fn main() -> Int {\n",
                "  let total = {\n",
                "    add(1, 2)\n",
                "    add(3, 4)\n",
                "  }\n",
                "  total\n",
                "}\n",
            ),
            TraceMode::Off,
        );
        assert_eq!(
            report.outcome,
            ExecutionOutcome::Success(ExecutionValue::Integer(7))
        );
    }

    // Guards: list literals, decoded JSON arrays, and List.first/length agree
    // past the 32-element boundary where the persistent vector leaves its tail.
    #[test]
    fn lists_longer_than_one_vector_leaf_round_trip_through_json() {
        let elements = (0..40)
            .map(|index| format!("Json.int({index})"))
            .collect::<Vec<_>>()
            .join(", ");
        let source = format!(
            concat!(
                "import std/json as Json\n",
                "import std/json.{{Value}}\n",
                "import std/list as List\n",
                "\n",
                "fn summarize(values: List<Value>) -> Text {{\n",
                "  case List.first(values) {{\n",
                "    Some(first) -> Json.encode(Json.array([Json.array(values), first, Json.int(List.length(values))]))\n",
                "    None -> \"empty\"\n",
                "  }}\n",
                "}}\n",
                "\n",
                "pub fn main() -> Text {{\n",
                "  let decoded = Json.decode(Json.encode(Json.array([{elements}])))\n",
                "  case decoded {{\n",
                "    Value.Array(values) -> summarize(values)\n",
                "    _ -> \"unexpected\"\n",
                "  }}\n",
                "}}\n",
            ),
            elements = elements
        );
        let report = Compiler::run_project(
            vec![module("lists/long", "long.chimi", &source)],
            ExecutionConfig::new("lists-long").unwrap(),
        )
        .expect("long list program compiles and executes");

        let numbers = (0..40).map(|index| index.to_string()).collect::<Vec<_>>();
        assert_eq!(
            report.outcome,
            ExecutionOutcome::Success(ExecutionValue::Text(format!(
                "[[{}],0,40]",
                numbers.join(",")
            )))
        );
    }
}
