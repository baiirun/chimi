use super::{ProjectSource, TypeArenaStats, TypeReport, resolve_project_sources};
use crate::diagnostic::Diagnostic;
use crate::ids::ModuleId;
use crate::inspection::{InspectionError, InspectionReport, inspect};
use crate::resolution::resolve_project;
use crate::semantic::{self, SemanticModule, SemanticProject};
use crate::source::SourceSnapshot;
use crate::syntax::{self, ParseResult};
use crate::typing::{TypedProject, check_project, check_project_incremental};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IncrementalStatistics {
    pub modules: usize,
    pub reused_parsed_modules: usize,
    pub parsed_modules: usize,
    pub reused_semantic_modules: usize,
    pub lowered_modules: usize,
    pub invalidated_modules: usize,
    pub invalidated_bytes: usize,
    pub rechecked_modules: usize,
    pub reused_type_facts: usize,
    pub invalidated_type_facts: usize,
}

#[derive(Debug)]
pub struct IncrementalTypeReport {
    types: TypeReport,
    statistics: IncrementalStatistics,
}

impl IncrementalTypeReport {
    pub fn is_success(&self) -> bool {
        self.types.is_success()
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        self.types.diagnostics()
    }

    pub fn arena_stats(&self) -> TypeArenaStats {
        self.types.arena_stats()
    }

    pub fn contract(&self, module_path: &str, function_name: &str) -> Option<String> {
        self.types.contract(module_path, function_name)
    }

    pub const fn statistics(&self) -> IncrementalStatistics {
        self.statistics
    }

    /// Answers cursor inspection from this report's retained typed snapshot.
    ///
    /// Ensures: the same syntax, reference, type, and channel facts as
    /// `Compiler::inspect_project` over the same sources.
    /// Must not: reparse, recheck, or seal; `executable_origin` is always absent.
    pub fn inspect(
        &self,
        module_path: &str,
        offset: usize,
    ) -> Result<InspectionReport, InspectionError> {
        inspect(&self.types.typed, module_path, offset)
    }

    /// The symbol whose name is at `offset` in `module_path`, with its
    /// declaration and every reference in the project (find references and
    /// rename), from this report's retained snapshot.
    pub fn occurrences(
        &self,
        module_path: &str,
        offset: usize,
    ) -> Option<crate::SymbolOccurrences> {
        crate::occurrences::occurrences_at(&self.types.typed, module_path, offset)
    }

    /// The public surface of the module that `alias` names in `module_path`
    /// (`import lib as Lib`), for completing `Lib.` at `offset`. `None` when
    /// `alias` is not a module alias there, or `offset` is inside a comment
    /// or a text literal.
    pub fn alias_members(
        &self,
        module_path: &str,
        alias: &str,
        offset: usize,
    ) -> Option<crate::ModuleDocumentation> {
        crate::doc::alias_members(&self.types.typed, module_path, alias, offset)
    }

    /// The inferred type of each `let` in `module_path` that states none,
    /// to show after the binding's name.
    pub fn let_type_hints(&self, module_path: &str) -> Vec<crate::TypeHint> {
        crate::hints::let_type_hints(&self.types.typed, module_path)
    }

    /// The `///` doc comment of the item named at `offset` in `module_path`.
    pub fn doc_comment_at(&self, module_path: &str, offset: usize) -> Option<String> {
        crate::doc::doc_comment_at(&self.types.typed, module_path, offset)
    }

    /// `chimi lint`'s findings for the project's own modules, by source
    /// file, or none while the project has errors: a finding on code that
    /// does not compile misleads.
    pub fn lint_by_source(&self) -> HashMap<&Path, Vec<Diagnostic>> {
        let mut findings: HashMap<&Path, Vec<Diagnostic>> = HashMap::new();
        if !self.diagnostics().is_empty() {
            return findings;
        }
        let resolved = &self.types.typed.resolved;
        let paths: HashMap<_, &Path> = resolved
            .semantic
            .modules
            .iter()
            .map(|module| (module.source.file_id(), module.source.path()))
            .collect();
        for finding in crate::lint::lint(resolved, |origin| {
            origin == crate::compiler::ModuleOrigin::Own
        }) {
            if let Some(path) = paths.get(&finding.file_id()) {
                findings.entry(*path).or_default().push(finding);
            }
        }
        findings
    }

    /// Groups this report's diagnostics by owning module.
    ///
    /// Ensures: exactly one entry per analyzed module in module-path order,
    /// including modules without diagnostics, so callers can clear stale ones.
    pub fn diagnostics_by_module(&self) -> Vec<ModuleDiagnostics<'_>> {
        let diagnostics = self.diagnostics();
        let mut modules = self
            .types
            .typed
            .resolved
            .semantic
            .modules
            .iter()
            .map(|module| {
                let file_id = module.source.file_id();
                ModuleDiagnostics {
                    module_path: &module.path,
                    source_path: module.source.path(),
                    source_text: module.source.text(),
                    diagnostics: diagnostics
                        .iter()
                        .filter(|diagnostic| diagnostic.file_id() == file_id)
                        .collect(),
                }
            })
            .collect::<Vec<_>>();
        modules.sort_by(|left, right| left.module_path.cmp(right.module_path));
        modules
    }
}

/// The diagnostics one module owns in an analysis.
#[derive(Debug)]
pub struct ModuleDiagnostics<'report> {
    module_path: &'report str,
    source_path: &'report Path,
    source_text: &'report str,
    diagnostics: Vec<&'report Diagnostic>,
}

impl<'report> ModuleDiagnostics<'report> {
    pub const fn module_path(&self) -> &'report str {
        self.module_path
    }

    pub const fn source_path(&self) -> &'report Path {
        self.source_path
    }

    /// The analyzed text that diagnostic byte ranges index into.
    pub const fn source_text(&self) -> &'report str {
        self.source_text
    }

    pub fn diagnostics(&self) -> &[&'report Diagnostic] {
        &self.diagnostics
    }
}

#[derive(Debug)]
struct CachedModule {
    source: SourceSnapshot,
    syntax: Arc<ParseResult>,
    semantic: Arc<SemanticModule>,
}

#[derive(Debug, Default)]
pub struct CompilerSession {
    modules: HashMap<String, CachedModule>,
    typed: Option<Arc<TypedProject>>,
}

impl CompilerSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn analyze_project(&mut self, mut sources: Vec<ProjectSource>) -> IncrementalTypeReport {
        crate::standard_tree::add_imported_modules(&mut sources);
        if has_duplicate_module_paths(&sources) {
            return self.analyze_uncached(sources);
        }
        sources.sort_by(|left, right| left.module_path.cmp(&right.module_path));

        let incoming_paths = sources
            .iter()
            .map(|source| source.module_path.clone())
            .collect::<HashSet<_>>();
        let removed = self
            .modules
            .iter()
            .filter(|(path, _)| !incoming_paths.contains(*path))
            .collect::<Vec<_>>();
        let mut statistics = IncrementalStatistics {
            modules: sources.len(),
            invalidated_modules: removed.len(),
            invalidated_bytes: removed
                .iter()
                .map(|(_, module)| module.source.text().len())
                .sum(),
            ..IncrementalStatistics::default()
        };
        let module_set_unchanged = removed.is_empty() && self.modules.len() == sources.len();
        let mut unchanged = module_set_unchanged;
        let mut next_modules = HashMap::with_capacity(sources.len());
        let mut semantic_modules = Vec::with_capacity(sources.len());
        let mut changed_modules = HashSet::new();

        for (index, source) in sources.into_iter().enumerate() {
            let ProjectSource {
                module_path,
                source,
                syntax: supplied_syntax,
                origin,
            } = source;
            let cached = self.modules.get(&module_path);
            let source_unchanged = cached.is_some_and(|cached| {
                same_source(&cached.source, &source) && cached.semantic.origin == origin
            });
            let syntax = if source_unchanged {
                statistics.reused_parsed_modules += 1;
                cached.expect("unchanged module is cached").syntax.clone()
            } else {
                unchanged = false;
                changed_modules.insert(ModuleId::new(index as u32));
                statistics.invalidated_modules += 1;
                statistics.invalidated_bytes += source.text().len();
                supplied_syntax.unwrap_or_else(|| {
                    statistics.parsed_modules += 1;
                    Arc::new(syntax::parse(&source))
                })
            };
            let semantic = if source_unchanged && module_set_unchanged {
                statistics.reused_semantic_modules += 1;
                cached.expect("unchanged module is cached").semantic.clone()
            } else {
                statistics.lowered_modules += 1;
                semantic::lower_module(
                    ModuleId::new(index as u32),
                    module_path.clone(),
                    source.clone(),
                    syntax.clone(),
                    origin,
                )
            };
            semantic_modules.push(semantic.clone());
            next_modules.insert(
                module_path.clone(),
                CachedModule {
                    source: source.clone(),
                    syntax: syntax.clone(),
                    semantic,
                },
            );
        }

        if unchanged && let Some(typed) = &self.typed {
            statistics.reused_type_facts = fact_count(typed);
            self.modules = next_modules;
            return IncrementalTypeReport {
                types: TypeReport {
                    typed: typed.clone(),
                },
                statistics,
            };
        }

        let semantic = SemanticProject {
            modules: semantic_modules,
        };
        if !module_set_unchanged {
            changed_modules = semantic.modules.iter().map(|module| module.id).collect();
        }
        let resolved = resolve_project(semantic);
        let typed = if let Some(previous) = self
            .typed
            .as_deref()
            .filter(|previous| module_set_unchanged && symbols_are_stable(previous, &resolved))
        {
            let affected_modules = affected_module_closure(&resolved.semantic, &changed_modules);
            statistics.rechecked_modules = affected_modules.len();
            statistics.invalidated_type_facts = fact_count_in_modules(previous, &affected_modules);
            statistics.reused_type_facts = fact_count(previous) - statistics.invalidated_type_facts;
            Arc::new(check_project_incremental(
                resolved,
                previous,
                &affected_modules,
            ))
        } else {
            statistics.rechecked_modules = resolved.semantic.modules.len();
            statistics.invalidated_type_facts = self.typed.as_deref().map_or(0, fact_count);
            Arc::new(check_project(resolved))
        };
        self.modules = next_modules;
        self.typed = Some(typed.clone());
        IncrementalTypeReport {
            types: TypeReport { typed },
            statistics,
        }
    }

    fn analyze_uncached(&mut self, mut sources: Vec<ProjectSource>) -> IncrementalTypeReport {
        let invalidated_type_facts = self.typed.as_deref().map_or(0, fact_count);
        let invalidated_bytes = sources
            .iter()
            .map(|source| source.source.text().len())
            .sum();
        let mut parsed_modules = 0;
        for source in &mut sources {
            if source.syntax.is_none() {
                source.syntax = Some(Arc::new(syntax::parse(&source.source)));
                parsed_modules += 1;
            }
        }
        let statistics = IncrementalStatistics {
            modules: sources.len(),
            parsed_modules,
            lowered_modules: sources.len(),
            invalidated_modules: sources.len(),
            invalidated_bytes,
            rechecked_modules: sources.len(),
            invalidated_type_facts,
            ..IncrementalStatistics::default()
        };
        let typed = Arc::new(check_project(resolve_project_sources(sources)));
        self.modules.clear();
        self.typed = None;
        IncrementalTypeReport {
            types: TypeReport { typed },
            statistics,
        }
    }
}

fn has_duplicate_module_paths(sources: &[ProjectSource]) -> bool {
    let mut paths = HashSet::with_capacity(sources.len());
    sources
        .iter()
        .any(|source| !paths.insert(source.module_path.as_str()))
}

fn same_source(left: &SourceSnapshot, right: &SourceSnapshot) -> bool {
    left.path() == right.path() && left.text() == right.text()
}

fn fact_count(typed: &TypedProject) -> usize {
    typed.type_facts.len() + typed.expression_facts.len() + typed.declaration_types.len()
}

fn fact_count_in_modules(typed: &TypedProject, modules: &HashSet<ModuleId>) -> usize {
    typed
        .type_facts
        .keys()
        .filter(|(module_id, _)| modules.contains(module_id))
        .count()
        + typed
            .expression_facts
            .keys()
            .filter(|(module_id, _)| modules.contains(module_id))
            .count()
        + typed
            .declaration_types
            .keys()
            .filter(|(module_id, _)| modules.contains(module_id))
            .count()
}

fn symbols_are_stable(
    previous: &TypedProject,
    current: &crate::resolution::ResolvedProject,
) -> bool {
    previous.resolved.symbols.len() == current.symbols.len()
        && previous
            .resolved
            .symbols
            .iter()
            .zip(&current.symbols)
            .all(|(left, right)| {
                left.id == right.id
                    && left.module_id == right.module_id
                    && left.semantic_id == right.semantic_id
                    && left.namespace == right.namespace
                    && left.kind == right.kind
                    && left.name == right.name
            })
}

fn affected_module_closure(
    semantic: &SemanticProject,
    changed: &HashSet<ModuleId>,
) -> HashSet<ModuleId> {
    let paths = semantic
        .modules
        .iter()
        .map(|module| (module.path.as_str(), module.id))
        .collect::<HashMap<_, _>>();
    let mut dependents = vec![Vec::new(); semantic.modules.len()];
    for module in &semantic.modules {
        for dependency in module
            .imports
            .iter()
            .filter_map(|import| paths.get(import.path.as_str()))
        {
            dependents[dependency.raw() as usize].push(module.id);
        }
    }

    let mut affected = changed.clone();
    let mut queue = changed.iter().copied().collect::<VecDeque<_>>();
    while let Some(module_id) = queue.pop_front() {
        for dependent in &dependents[module_id.raw() as usize] {
            if affected.insert(*dependent) {
                queue.push_back(*dependent);
            }
        }
    }
    affected
}

#[cfg(test)]
mod tests {
    use super::CompilerSession;
    use crate::source::SourceSnapshot;
    use crate::{Compiler, ProjectSource};
    use std::sync::Arc;

    fn module(path: &str, body: &str) -> ProjectSource {
        ProjectSource::new(
            path,
            SourceSnapshot::from_text(format!("{path}.chimi"), body),
        )
        .unwrap()
    }

    #[test]
    fn unchanged_projects_reuse_parses_and_the_complete_typed_snapshot() {
        let sources = vec![
            module("a", "pub fn alpha() -> Int { 1 }\n"),
            module("b", "pub fn beta() -> Int { 2 }\n"),
        ];
        let mut session = CompilerSession::new();
        let initial = session.analyze_project(sources.clone());
        let initial_typed = initial.types.typed.clone();

        let repeated = session.analyze_project(sources);

        assert!(repeated.is_success());
        assert!(Arc::ptr_eq(&initial_typed, &repeated.types.typed));
        assert_eq!(repeated.statistics().reused_parsed_modules, 2);
        assert_eq!(repeated.statistics().parsed_modules, 0);
        assert_eq!(repeated.statistics().reused_semantic_modules, 2);
        assert_eq!(repeated.statistics().lowered_modules, 0);
        assert_eq!(repeated.statistics().invalidated_modules, 0);
        assert_eq!(repeated.statistics().rechecked_modules, 0);
        assert!(repeated.statistics().reused_type_facts > 0);
    }

    #[test]
    fn local_edits_reparse_one_module_and_match_a_clean_check() {
        let before = vec![
            module("a", "pub fn alpha() -> Int { 1 }\n"),
            module("b", "pub fn beta() -> Int { 2 }\n"),
            module("c", "pub fn gamma() -> Int { 3 }\n"),
        ];
        let after = vec![
            module("a", "pub fn alpha() -> Int { 1 }\n"),
            module("b", "pub fn beta() -> Int { 20 }\n"),
            module("c", "pub fn gamma() -> Int { 3 }\n"),
        ];
        let mut session = CompilerSession::new();
        session.analyze_project(before);

        let incremental = session.analyze_project(after.clone());
        let clean = Compiler::type_project(after);

        assert_eq!(incremental.diagnostics(), clean.diagnostics());
        assert_eq!(incremental.arena_stats(), clean.arena_stats());
        assert_eq!(incremental.statistics().reused_parsed_modules, 2);
        assert_eq!(incremental.statistics().parsed_modules, 1);
        assert_eq!(incremental.statistics().reused_semantic_modules, 2);
        assert_eq!(incremental.statistics().lowered_modules, 1);
        assert_eq!(incremental.statistics().invalidated_modules, 1);
        assert_eq!(incremental.statistics().rechecked_modules, 1);
        assert!(incremental.statistics().reused_type_facts > 0);
        assert!(incremental.statistics().invalidated_type_facts > 0);
    }

    #[test]
    fn malformed_edits_match_clean_diagnostics_without_discarding_valid_parses() {
        let before = vec![
            module("a", "pub fn alpha() -> Int { 1 }\n"),
            module("b", "pub fn beta() -> Int { 2 }\n"),
        ];
        let after = vec![
            module("a", "pub fn alpha() -> Int { 1 }\n"),
            module("b", "pub fn beta() -> Int {\n"),
        ];
        let mut session = CompilerSession::new();
        session.analyze_project(before);

        let incremental = session.analyze_project(after.clone());
        let clean = Compiler::type_project(after);

        assert_eq!(incremental.diagnostics(), clean.diagnostics());
        assert_eq!(incremental.statistics().reused_parsed_modules, 1);
        assert_eq!(incremental.statistics().invalidated_modules, 1);
    }

    #[test]
    fn public_contract_edits_recheck_only_the_reverse_import_closure() {
        let before = vec![
            module("shared", "pub fn shared(value: Int) -> Int { 1 }\n"),
            module(
                "app",
                "import shared as dependency\n\npub fn use(value: Int) -> Int { dependency.shared(value) }\n",
            ),
            module("unrelated", "pub fn stable() -> Int { 1 }\n"),
        ];
        let after = vec![
            module("shared", "pub fn shared(value: Text) -> Int { 1 }\n"),
            module(
                "app",
                "import shared as dependency\n\npub fn use(value: Int) -> Int { dependency.shared(value) }\n",
            ),
            module("unrelated", "pub fn stable() -> Int { 1 }\n"),
        ];
        let mut session = CompilerSession::new();
        session.analyze_project(before);

        let incremental = session.analyze_project(after.clone());
        let clean = Compiler::type_project(after);

        assert_eq!(incremental.diagnostics(), clean.diagnostics());
        assert_eq!(incremental.arena_stats(), clean.arena_stats());
        assert_eq!(incremental.statistics().invalidated_modules, 1);
        assert_eq!(incremental.statistics().rechecked_modules, 2);
        assert!(incremental.statistics().reused_type_facts > 0);
    }

    #[test]
    fn symbol_shape_changes_fall_back_to_a_complete_type_check() {
        let before = vec![
            module("a", "pub fn alpha() -> Int { 1 }\n"),
            module("b", "pub fn beta() -> Int { 2 }\n"),
        ];
        let after = vec![
            module(
                "a",
                "pub fn alpha() -> Int { 1 }\npub fn added() -> Int { 2 }\n",
            ),
            module("b", "pub fn beta() -> Int { 2 }\n"),
        ];
        let mut session = CompilerSession::new();
        session.analyze_project(before);

        let incremental = session.analyze_project(after.clone());
        let clean = Compiler::type_project(after);

        assert_eq!(incremental.diagnostics(), clean.diagnostics());
        assert_eq!(incremental.arena_stats(), clean.arena_stats());
        assert_eq!(incremental.statistics().rechecked_modules, 2);
        assert_eq!(incremental.statistics().reused_type_facts, 0);
    }

    #[test]
    fn reused_and_recomputed_diagnostics_keep_clean_project_order() {
        let before = vec![
            module("a", "pub fn alpha() -> Int { 1 }\n"),
            module("b", "pub fn beta() -> Int { \"wrong\" }\n"),
        ];
        let after = vec![
            module("a", "pub fn alpha() -> Int { \"wrong\" }\n"),
            module("b", "pub fn beta() -> Int { \"wrong\" }\n"),
        ];
        let mut session = CompilerSession::new();
        session.analyze_project(before);

        let incremental = session.analyze_project(after.clone());
        let clean = Compiler::type_project(after);

        assert_eq!(incremental.diagnostics(), clean.diagnostics());
        assert_eq!(incremental.diagnostics().len(), 2);
    }

    #[test]
    fn changed_contracts_match_clean_output_when_dense_type_arenas_diverge() {
        let before = vec![module("main", "pub fn first(values: Int) -> Int { 1 }\n")];
        let after = vec![module(
            "main",
            "pub fn first(values: List<Int>) -> Int { 1 }\n",
        )];
        let mut session = CompilerSession::new();
        session.analyze_project(before);

        let incremental = session.analyze_project(after.clone());
        let clean = Compiler::type_project(after);

        assert_eq!(incremental.diagnostics(), clean.diagnostics());
        assert_eq!(
            incremental.contract("main", "first"),
            clean.contract("main", "first")
        );
    }

    /// Guards session inspection against drifting from the clean oracle,
    /// including after an edit that the session rechecks incrementally.
    /// No module declares `main`, so the clean path cannot seal and both
    /// reports omit the executable origin.
    #[test]
    fn session_inspection_matches_clean_inspection_after_an_edit() {
        let library = "pub fn shared(value: Int) -> Int { value }\n";
        let user = "import lib as dependency\n\npub fn use(value: Int) -> Int { dependency.shared(value) }\n";
        let edited = "import lib as dependency\n\npub fn use(count: Int) -> Int { dependency.shared(count) }\n";
        let mut session = CompilerSession::new();
        session.analyze_project(vec![module("lib", library), module("user", user)]);
        let sources = vec![module("lib", library), module("user", edited)];
        let report = session.analyze_project(sources.clone());

        for needle in ["shared(count)", "count)", "dependency."] {
            let offset = edited.rfind(needle).expect("needle is present");
            let incremental = report.inspect("user", offset).expect("session inspection");
            let clean = Compiler::inspect_project(sources.clone(), "user", offset)
                .expect("clean inspection");
            assert_eq!(incremental.render_json(), clean.render_json(), "{needle}");
        }
    }

    /// Guards stale-diagnostic clearing: a module that became clean must still
    /// appear, with an empty list, so an editor replaces its old diagnostics.
    #[test]
    fn diagnostics_by_module_lists_every_module_including_clean_ones() {
        let report = CompilerSession::new().analyze_project(vec![
            module("b", "pub fn broken() -> Int { missing() }\n"),
            module("a", "pub fn fine() -> Int { 1 }\n"),
        ]);
        let grouped = report
            .diagnostics_by_module()
            .iter()
            .map(|module| {
                let codes = module
                    .diagnostics()
                    .iter()
                    .map(|diagnostic| diagnostic.code())
                    .collect::<Vec<_>>();
                (
                    module.module_path().to_owned(),
                    module.source_path().to_owned(),
                    codes,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(grouped.len(), 2);
        assert_eq!(
            (grouped[0].0.as_str(), grouped[0].1.to_str()),
            ("a", Some("a.chimi"))
        );
        assert!(grouped[0].2.is_empty());
        assert_eq!(
            (grouped[1].0.as_str(), grouped[1].1.to_str()),
            ("b", Some("b.chimi"))
        );
        assert!(!grouped[1].2.is_empty());
        assert_eq!(grouped[1].2.len(), report.diagnostics().len());
    }
}
