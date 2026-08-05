use crate::diagnostic::Diagnostic;
use crate::executable::{lower, seal_entry};
use crate::execution::{ExecutionConfig, ExecutionReport, execute};
use crate::ids::SymbolId;
use crate::resolution::{
    Namespace, ProjectSymbolKind, ReferenceResolution, ResolvedProject, resolve, resolve_project,
};
use crate::semantic;
use crate::source::SourceSnapshot;
use crate::typing::check;
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
        let mut output = self
            .resolved
            .diagnostics
            .iter()
            .map(|diagnostic| {
                let source =
                    &self.resolved.semantic.modules[diagnostic.file_id().raw() as usize].source;
                diagnostic.render(source)
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !output.is_empty() {
            output.push('\n');
        }
        output
    }
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

pub struct Compiler;

impl Compiler {
    pub fn resolve_project(sources: Vec<ProjectSource>) -> NameReport {
        let semantic = semantic::lower_project(
            sources
                .into_iter()
                .map(|source| semantic::ModuleSource {
                    path: source.module_path,
                    source: source.source,
                })
                .collect(),
        );
        NameReport {
            resolved: resolve_project(semantic),
        }
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
        let core = lower(verified);
        Ok(execute(core, config))
    }
}

fn seal(source: Arc<SourceSnapshot>) -> Result<crate::executable::VerifiedRoot, Vec<Diagnostic>> {
    let semantic = semantic::lower(source);
    let resolved = resolve(semantic);
    let typed = check(resolved);
    seal_entry(typed)
}

#[cfg(test)]
mod tests {
    use super::{Compiler, ProjectSource};
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
}
