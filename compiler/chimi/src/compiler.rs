use crate::diagnostic::Diagnostic;
use crate::executable::{lower, lower_legacy, seal_entry, seal_legacy_entry};
use crate::execution::{ExecutionConfig, ExecutionReport, execute};
use crate::ids::SymbolId;
use crate::resolution::{
    Namespace, ProjectSymbolKind, ReferenceResolution, ResolvedProject, resolve, resolve_project,
};
use crate::semantic;
use crate::source::SourceSnapshot;
use crate::typing::{GenericKind, TypedProject, check, check_project};
use std::sync::Arc;

#[derive(Debug)]
pub struct ProjectSource {
    module_path: String,
    source: SourceSnapshot,
}

impl ProjectSource {
    pub fn new(
        module_path: impl Into<String>,
        source: SourceSnapshot,
    ) -> Result<Self, ProjectPathError> {
        let module_path = module_path.into();
        let valid = !module_path.is_empty()
            && !module_path.starts_with('/')
            && !module_path.ends_with('/')
            && module_path.split('/').all(|part| {
                !part.is_empty()
                    && part != "."
                    && part != ".."
                    && part
                        .chars()
                        .all(|character| character == '_' || character.is_ascii_alphanumeric())
            });
        if !valid {
            return Err(ProjectPathError { module_path });
        }
        Ok(Self {
            module_path,
            source,
        })
    }

    pub fn module_path(&self) -> &str {
        &self.module_path
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct ProjectPathError {
    module_path: String,
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

#[derive(Debug)]
pub struct TypeReport {
    typed: TypedProject,
}

impl TypeReport {
    pub fn is_success(&self) -> bool {
        self.typed.diagnostics.is_empty()
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.typed.diagnostics
    }

    pub fn contract(&self, module_path: &str, function_name: &str) -> Option<String> {
        self.typed
            .contract(module_path, function_name)
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

    pub fn render_diagnostics(&self) -> String {
        render_project_diagnostics(&self.typed.resolved, &self.typed.diagnostics)
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
                namespace: namespace_name(symbol.namespace),
                kind: symbol_kind_name(symbol.kind),
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
                    namespace: namespace_name(reference.namespace),
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

const fn namespace_name(namespace: Namespace) -> &'static str {
    match namespace {
        Namespace::Module => "module",
        Namespace::Type => "type",
        Namespace::Value => "value",
    }
}

const fn symbol_kind_name(kind: ProjectSymbolKind) -> &'static str {
    match kind {
        ProjectSymbolKind::BuiltinModule => "builtin module",
        ProjectSymbolKind::BuiltinType => "builtin type",
        ProjectSymbolKind::BuiltinValue => "builtin value",
        ProjectSymbolKind::Module => "module",
        ProjectSymbolKind::Struct => "struct",
        ProjectSymbolKind::StructConstructor => "struct constructor",
        ProjectSymbolKind::Enum => "enum",
        ProjectSymbolKind::EnumConstructor => "enum constructor",
        ProjectSymbolKind::Interface => "interface",
        ProjectSymbolKind::Function => "function",
        ProjectSymbolKind::ModuleBinding => "module binding",
        ProjectSymbolKind::GenericParameter => "generic parameter",
        ProjectSymbolKind::Parameter => "parameter",
        ProjectSymbolKind::Requirement => "requirement",
        ProjectSymbolKind::LocalBinding => "local binding",
        ProjectSymbolKind::PatternBinding => "pattern binding",
    }
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerificationStats {
    pub reachable_declarations: usize,
    pub origins: usize,
    pub reachable_modules: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoreStats {
    pub functions: usize,
    pub bindings: usize,
    pub nodes: usize,
    pub origins: usize,
    pub constants: usize,
    pub constructions: usize,
    pub calls: usize,
    pub sequences: usize,
}

pub struct Compiler;

impl Compiler {
    pub fn resolve_project(sources: Vec<ProjectSource>) -> NameReport {
        NameReport {
            resolved: resolve_project_sources(sources),
        }
    }

    pub fn type_project(sources: Vec<ProjectSource>) -> TypeReport {
        TypeReport {
            typed: check_project(resolve_project_sources(sources)),
        }
    }

    pub fn verify_project(
        sources: Vec<ProjectSource>,
    ) -> Result<VerificationStats, Vec<Diagnostic>> {
        let root = seal_entry(check_project(resolve_project_sources(sources)))?;
        debug_assert!(root.typed.contracts.contains_key(&root.entry));
        let (reachable_declarations, origins, reachable_modules) = root.verification_stats();
        Ok(VerificationStats {
            reachable_declarations,
            origins,
            reachable_modules,
        })
    }

    pub fn lower_project(sources: Vec<ProjectSource>) -> Result<CoreStats, Vec<Diagnostic>> {
        let core = lower(seal_entry(check_project(resolve_project_sources(sources)))?);
        let (functions, bindings, nodes, origins, constants, constructions, calls, sequences) =
            core.stats();
        Ok(CoreStats {
            functions,
            bindings,
            nodes,
            origins,
            constants,
            constructions,
            calls,
            sequences,
        })
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
        Ok(execute(core, config))
    }
}

fn resolve_project_sources(sources: Vec<ProjectSource>) -> ResolvedProject {
    let semantic = semantic::lower_project(
        sources
            .into_iter()
            .map(|source| semantic::ModuleSource {
                path: source.module_path,
                source: source.source,
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
    use super::{Compiler, GenericParameterKind, ProjectSource};
    use crate::diagnostic::{Diagnostic, DiagnosticStage};
    use crate::execution::{ExecutionConfig, ExecutionOutcome};
    use crate::source::SourceSnapshot;

    const VALID: &str = "struct Answer { value: Int } pub fn main() { Answer(value: 42) }";

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
        let expression_start = VALID.find("Answer(value: 42)").unwrap() as u32;
        assert_eq!(report.events[0].source_range.start(), expression_start);
        assert_eq!(
            report.events[0].source_range.end(),
            expression_start + "Answer(value: 42)".len() as u32
        );
        assert_eq!(report.events[0].origin_id, report.events[0].semantic_id);
        assert_eq!(report.events[0].file_id, 0);
        let ExecutionOutcome::Success(value) = report.outcome;
        assert_eq!(value.type_name, "Answer");
        assert_eq!(value.field_name, "value");
        assert_eq!(value.integer_value, 42);
    }

    #[test]
    fn source_ranges_survive_lossless_cst_lowering() {
        let text = concat!(
            "// retained trivia\r\n",
            "struct Answer {\n  value: Int\n}\n\n",
            "pub fn main() {\n  Answer( value: 42 ) // retained too\n}\n",
        );
        let report = Compiler::run(
            source(text),
            ExecutionConfig::new("source-range-test").unwrap(),
        )
        .expect("source with trivia executes");

        let expression_start = text.find("Answer( value: 42 )").unwrap() as u32;
        assert_eq!(report.events[0].source_range.start(), expression_start);
        assert_eq!(
            report.events[0].source_range.end(),
            expression_start + "Answer( value: 42 )".len() as u32
        );
    }

    #[test]
    fn missing_or_renamed_main_cannot_seal() {
        for text in [
            "struct Answer { value: Int }",
            "struct Answer { value: Int } pub fn start() { Answer(value: 42) }",
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
        let text = "struct Answer { value: Int } pub fn main() { Missing(value: 42) }";
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
        let text = "struct Answer { value: Int } pub fn main() { Answer(value: \"forty-two\") }";
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
            "struct Answer { value: Int } pub fn main() { Answer(value: 999999999999999999999999999999999999) }",
            "struct Answer { value: Int } pub fn main() { Answer(value: λ) }",
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
            "pub fn main() { Answer(value: 42) }\n",
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
            ["modules/money", "modules/use_money"]
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
        let expected = "Money.Money(cents: -100)";
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
    fn one_generic_cannot_be_both_a_value_and_failure_row() {
        let text = concat!(
            "fn invalid<A>(\n",
            "  operation: fn(Int) -> Int errors A,\n",
            "  value: A,\n",
            ") -> Int { 0 }\n",
        );
        let report = Compiler::type_project(vec![module(
            "contracts/invalid_kind",
            "invalid_kind.chimi",
            text,
        )]);

        assert_eq!(report.diagnostics().len(), 1);
        assert_eq!(report.diagnostics()[0].stage(), DiagnosticStage::Type);
        assert_eq!(report.diagnostics()[0].code(), "T4101");
        assert_eq!(
            report.generic_kind("contracts/invalid_kind", "invalid", "A"),
            Some(GenericParameterKind::Conflicting)
        );
        let expected = text.find("value: A").unwrap() + "value: ".len();
        assert_eq!(report.diagnostics()[0].range().start(), expected as u32);
    }

    #[test]
    fn repeated_annotations_share_canonical_types() {
        let report = Compiler::type_project(vec![module(
            "contracts/interning",
            "interning.chimi",
            "fn add(left: Int, right: Int) -> Int { left }\n",
        )]);

        assert!(report.is_success(), "{}", report.render_diagnostics());
        assert_eq!(report.arena_stats().types, 3);
        assert_eq!(report.arena_stats().type_facts, 3);
    }

    #[test]
    fn unresolved_annotations_still_receive_poison_facts() {
        let report = Compiler::type_project(vec![module(
            "contracts/poison",
            "poison.chimi",
            "fn preserve(value: Missing) -> Missing { value }\n",
        )]);

        assert!(!report.is_success());
        assert_eq!(report.arena_stats().types, 2);
        assert_eq!(report.arena_stats().type_facts, 2);
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
            text.find("takes(\"not an int\")").unwrap() as u32
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
            text.find("choose(1, \"text\")").unwrap() as u32
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
                "let provider = { value: 7, label: \"seven\" }\n",
                "fn make_money() -> Money { Money(cents: 2500, id: \"m-1\") }\n",
                "fn boxed_int() -> Box<Int> { Box(value: 1) }\n",
                "fn boxed_text() -> Box<Text> { Box(value: \"text\") }\n",
                "fn maybe_int() -> Maybe<Int> { Some(value: 1) }\n",
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
                "fn unknown_field() -> Money { Money(dollars: 1) }\n",
                "fn wrong_field_type() -> Money { Money(cents: \"many\") }\n",
                "fn mixed_list() -> List<Int> { [1, \"two\"] }\n",
                "fn named_function_argument() -> Int { identity(value: 1) }\n",
            ),
        )]);

        assert_eq!(
            report
                .diagnostics()
                .iter()
                .map(Diagnostic::code)
                .collect::<Vec<_>>(),
            ["T4210", "T4203", "T4203", "T4210"]
        );
        assert!(
            report
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.stage() == DiagnosticStage::Type)
        );
    }

    #[test]
    fn using_closes_executed_and_latent_needs_at_arbitrary_boundaries() {
        let report = Compiler::type_project(vec![module(
            "dependencies/provision",
            "provision.chimi",
            concat!(
                "interface Clock { now: fn() -> Int }\n",
                "interface Mailer { send: fn(Int) -> Text }\n",
                "let clock = { now: fn() -> Int { 1 }, extra: \"allowed\" }\n",
                "let mailer = { send: fn(value: Int) -> Text { \"sent\" } }\n",
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
                "let wrong = { now: fn(value: Int) -> Int { value } }\n",
                "let clock = { now: fn() -> Int { 1 } }\n",
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
                "let narrow = { now: fn() -> Int { 1 } }\n",
                "let wide = { now: fn() -> Int needs { source: Source } { 1 } }\n",
                "let missing = { other: fn() -> Int { 1 } }\n",
                "let int_source = { get: fn() -> Int { 1 } }\n",
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
                "    0 -> Failure(Declined(reason: \"declined\"))\n",
                "    _ -> Failure(Offline())\n",
                "  }\n",
                "}\n",
                "fn bubble(mode: Int) -> Receipt { operation(mode) }\n",
                "fn explicit_success() -> Int { Success(1) }\n",
                "fn recover_declined(mode: Int) -> Receipt {\n",
                "  case operation(mode) {\n",
                "    Success(receipt) -> receipt\n",
                "    Failure(Declined(_reason)) -> Receipt(id: \"fallback\")\n",
                "  }\n",
                "}\n",
                "fn recover_all(mode: Int) -> Receipt {\n",
                "  case operation(mode) {\n",
                "    Success(receipt) -> receipt\n",
                "    Failure(_error) -> Receipt(id: \"fallback\")\n",
                "  }\n",
                "}\n",
                "fn guarded_recovery(mode: Int) -> Receipt {\n",
                "  case operation(mode) {\n",
                "    Success(receipt) -> receipt\n",
                "    Failure(Declined(_reason)) if True -> Receipt(id: \"declined\")\n",
                "    Failure(Offline) -> Receipt(id: \"offline\")\n",
                "  }\n",
                "}\n",
                "fn remap(mode: Int) -> Receipt {\n",
                "  case operation(mode) {\n",
                "    Success(receipt) -> receipt\n",
                "    Failure(error) -> Failure(Payment(error: error))\n",
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
    fn outcome_errors_are_bounded_and_leave_unhandled_failures_visible() {
        let report = Compiler::type_project(vec![module(
            "outcomes/invalid",
            "invalid_outcomes.chimi",
            concat!(
                "struct Receipt { id: Text }\n",
                "enum PaymentError { Declined(reason: Text), Offline }\n",
                "fn operation() -> Receipt { Failure(Declined(reason: \"no\")) }\n",
                "fn raw_case() -> Receipt {\n",
                "  case operation() { _ -> Receipt(id: \"raw\") }\n",
                "}\n",
                "fn incomplete() -> Receipt {\n",
                "  case operation() {\n",
                "    Success(receipt) -> receipt\n",
                "  }\n",
                "}\n",
                "fn impossible() -> Receipt {\n",
                "  case operation() {\n",
                "    Success(receipt) -> receipt\n",
                "    Failure(Offline) -> Receipt(id: \"offline\")\n",
                "  }\n",
                "}\n",
                "fn invalid_pattern() -> Receipt {\n",
                "  case operation() {\n",
                "    Success(receipt) -> receipt\n",
                "    Failure(True) -> Receipt(id: \"invalid\")\n",
                "  }\n",
                "}\n",
                "fn unreachable_outcome() -> Receipt {\n",
                "  case operation() {\n",
                "    Success(receipt) -> receipt\n",
                "    Failure(_error) -> Receipt(id: \"fallback\")\n",
                "    Failure(Declined(_reason)) -> Receipt(id: \"duplicate\")\n",
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
            "let clock = { now: fn() -> Int { 1 } }\n",
            "let wrong = { now: fn(value: Int) -> Int { value } }\n",
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
        assert_eq!(
            valid.expect("valid project seals"),
            super::VerificationStats {
                reachable_declarations: 2,
                origins: 6,
                reachable_modules: 2,
            }
        );

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
        incomplete.typed.expression_facts.remove(&expression);
        let diagnostics = crate::executable::seal_entry(incomplete.typed)
            .err()
            .expect("incomplete typed snapshots cannot seal");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code(), "E5006");
    }

    #[test]
    fn verified_roots_lower_to_typed_origin_linked_core_arenas() {
        let stats = Compiler::lower_project(vec![module(
            "core/basic",
            "basic_core.chimi",
            concat!(
                "struct Answer { value: Int }\n",
                "fn make(value: Int) -> Answer { Answer(value: value) }\n",
                "pub fn main() -> Answer {\n",
                "  let answer = make(42)\n",
                "  answer\n",
                "}\n",
            ),
        )])
        .expect("verified structural program lowers");

        assert_eq!(
            stats,
            super::CoreStats {
                functions: 2,
                bindings: 0,
                nodes: 8,
                origins: 8,
                constants: 1,
                constructions: 1,
                calls: 1,
                sequences: 2,
            }
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
}
