//! Editor workspace state and analysis, independent of the LSP transport.
//!
//! Invariants:
//! - Analysis reads open-document text in preference to disk for every module.
//! - A file that received diagnostics from a project receives a fresh, possibly
//!   empty, list on that project's next analysis, even after it leaves the
//!   project, so editors never keep stale diagnostics.
//! - Cursor queries answer from the latest analysis and convert positions with
//!   the text that analysis saw, never with newer or older text.

use chimi::diagnostic::{Diagnostic, DiagnosticStage};
use chimi::line_index::{LineIndex, LinePosition, PositionEncoding};
use chimi::project::{LoadedProject, SourceReader, StandardTree};
use chimi::source::SourceSnapshot;
use chimi::{
    CallableInspection, Compiler, CompilerSession, IncrementalTypeReport, InspectionReport,
    Occurrence, ProjectSource, ReferenceInspection, SymbolOccurrences, TypeReport,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// A canonical filesystem path to a `.chimi` document.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DocumentPath(PathBuf);

impl DocumentPath {
    /// Fails when the file does not exist on disk; unsaved new files are not
    /// analyzable until they are written once.
    pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<Self> {
        path.as_ref().canonicalize().map(Self)
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

/// The editor's monotonically increasing version of an open document.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DocumentVersion(pub i32);

/// A project the editor configured explicitly, because Chimi has no manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectConfig {
    root: PathBuf,
    entry_module: String,
}

impl ProjectConfig {
    /// Canonicalizes `root` so document paths can be matched against it.
    pub fn new(root: impl AsRef<Path>, entry_module: impl Into<String>) -> io::Result<Self> {
        Ok(Self {
            root: root.as_ref().canonicalize()?,
            entry_module: entry_module.into(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LineRange {
    pub start: LinePosition,
    pub end: LinePosition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticOrigin {
    Compiler {
        stage: DiagnosticStage,
        code: &'static str,
    },
    /// The project could not be loaded, so no compiler stage ran.
    ProjectLoad,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorDiagnostic {
    pub range: LineRange,
    pub origin: DiagnosticOrigin,
    pub message: String,
}

/// The complete diagnostic set for one file, replacing any earlier set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileDiagnostics {
    pub path: DocumentPath,
    /// The open document's version that was analyzed; `None` for disk text.
    pub version: Option<DocumentVersion>,
    pub diagnostics: Vec<EditorDiagnostic>,
}

/// One edit replacing `range` with `text`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextReplacement {
    pub range: LineRange,
    pub text: String,
}

/// A range in a specific document, such as a declaration's name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentRange {
    pub path: DocumentPath,
    pub range: LineRange,
}

/// What to show for the item under the cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HoverInfo {
    /// The span the hover describes, such as the whole reference.
    pub range: LineRange,
    pub markdown: String,
}

/// An inferred type to show inline, as `: List<Int>` after `let items`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeHintInfo {
    pub position: LinePosition,
    pub label: String,
}

/// One name an editor can insert at the cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Completion {
    pub label: String,
    /// The item's kind as `chimi doc` names it: `fn`, `struct`, `enum`,
    /// `const`, `interface`, ...
    pub kind: &'static str,
    /// The item's signature, with a function's derived contract.
    pub detail: String,
    /// The item's `///` doc comment, as Markdown.
    pub documentation: Option<String>,
}

#[derive(Debug)]
struct OpenDocument {
    version: DocumentVersion,
    lines: LineIndex,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum ProjectKey {
    /// Index into `Workspace::projects`.
    Configured(usize),
    /// A document no configured project reaches, analyzed alone as `main`.
    SingleFile(DocumentPath),
}

#[derive(Debug, Default)]
struct ProjectAnalysis {
    session: CompilerSession,
    published: BTreeSet<DocumentPath>,
    /// `None` before the first analysis and after a load failure, so queries
    /// never pair a report with text it did not analyze.
    latest: Option<AnalyzedProject>,
}

/// One analysis result together with the text of every module it analyzed.
#[derive(Debug)]
struct AnalyzedProject {
    report: IncrementalTypeReport,
    modules: HashMap<DocumentPath, AnalyzedModule>,
}

impl AnalyzedProject {
    fn document_range(
        &self,
        occurrence: &Occurrence,
        encoding: PositionEncoding,
    ) -> Option<DocumentRange> {
        let path = DocumentPath(occurrence.source_path().to_owned());
        let lines = &self.modules.get(&path)?.lines;
        let range = occurrence.range();
        Some(DocumentRange {
            range: line_range(range.start(), range.end(), lines, encoding),
            path,
        })
    }

    /// The analyzed modules type-checked with every span in `edits` replaced
    /// by `text`.
    fn type_after(
        &self,
        edits: &BTreeMap<DocumentPath, Vec<(u32, u32)>>,
        text: &str,
    ) -> TypeReport {
        let sources = self
            .modules
            .iter()
            .filter_map(|(path, module)| {
                let mut edited = module.lines.text().to_owned();
                let mut spans = edits.get(path).cloned().unwrap_or_default();
                spans.sort_unstable();
                for (start, end) in spans.into_iter().rev() {
                    edited.replace_range(start as usize..end as usize, text);
                }
                ProjectSource::new(
                    module.module_path.clone(),
                    SourceSnapshot::from_text(path.as_path(), edited),
                )
                .ok()
            })
            .collect();
        Compiler::type_project(sources)
    }

    /// Whether renaming every span of `edits` to `text` keeps the program's
    /// meaning: no more errors than before, and the symbol at `anchor` (a
    /// module path, a file and an offset of one renamed span) is written at
    /// exactly the renamed spans afterwards. A shadowing that still
    /// type-checks, as a parameter renamed to a constant's name, moves a
    /// reference into or out of that set.
    fn rename_keeps_meaning(
        &self,
        edits: &BTreeMap<DocumentPath, Vec<(u32, u32)>>,
        text: &str,
        anchor: (&str, &Path, u32),
    ) -> bool {
        let renamed = self.type_after(edits, text);
        if renamed.diagnostics().len() > self.report.diagnostics().len() {
            return false;
        }
        let (module_path, anchor_path, anchor_start) = anchor;
        let length = text.len() as u32;
        let mut expected = BTreeSet::new();
        let mut anchor_offset = None;
        for (document, spans) in edits {
            let mut spans = spans.clone();
            spans.sort_unstable();
            // Every span holds the old name, so each one shifts the spans
            // after it in the file by the same length difference.
            let mut shift: i64 = 0;
            for (start, end) in spans {
                let moved = (i64::from(start) + shift) as u32;
                if document.as_path() == anchor_path && start == anchor_start {
                    anchor_offset = Some(moved);
                }
                expected.insert((document.as_path().to_owned(), moved, moved + length));
                shift += i64::from(length) - i64::from(end - start);
            }
        }
        let Some(offset) = anchor_offset else {
            return false;
        };
        let Some(after) = renamed.occurrences(module_path, offset as usize) else {
            return false;
        };
        let found: BTreeSet<_> = after
            .occurrences()
            .iter()
            .map(|occurrence| {
                (
                    occurrence.source_path().to_owned(),
                    occurrence.range().start(),
                    occurrence.range().end(),
                )
            })
            .collect();
        found == expected
    }
}

/// Why a rename was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RenameError {
    NothingToRename,
    /// A standard or built-in symbol, or a module alias.
    NotRenamable(String),
    /// Not an identifier, or one whose case differs from the old name's.
    InvalidName(String),
    /// The renamed project would not compile, or a reference would name
    /// another symbol: the name collides or shadows.
    Conflict(String),
}

impl std::fmt::Display for RenameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NothingToRename => formatter.write_str("there is no name to rename here"),
            Self::NotRenamable(name) => {
                write!(
                    formatter,
                    "`{name}` is not declared in this project's source"
                )
            }
            Self::InvalidName(name) => write!(
                formatter,
                "`{name}` is not a valid name here; keep the first letter's case"
            ),
            Self::Conflict(name) => write!(
                formatter,
                "renaming to `{name}` would collide with or shadow another name, or change what a name refers to"
            ),
        }
    }
}

fn is_identifier(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn starts_uppercase(name: &str) -> bool {
    name.trim_start_matches('_')
        .starts_with(|character: char| character.is_ascii_uppercase())
}

#[derive(Debug)]
struct AnalyzedModule {
    module_path: String,
    /// The text the report analyzed, which may differ from disk.
    lines: LineIndex,
}

/// Project loading reads open buffers before disk.
struct OpenBuffers<'a>(&'a HashMap<DocumentPath, OpenDocument>);

impl SourceReader for OpenBuffers<'_> {
    fn read(&self, path: &Path) -> io::Result<String> {
        match self.0.get(&DocumentPath(path.to_owned())) {
            Some(document) => Ok(document.lines.text().to_owned()),
            None => fs::read_to_string(path),
        }
    }
}

#[derive(Debug)]
pub struct Workspace {
    projects: Vec<ProjectConfig>,
    encoding: PositionEncoding,
    documents: HashMap<DocumentPath, OpenDocument>,
    analyses: HashMap<ProjectKey, ProjectAnalysis>,
}

impl Workspace {
    pub fn new(projects: Vec<ProjectConfig>, encoding: PositionEncoding) -> Self {
        Self {
            projects,
            encoding,
            documents: HashMap::new(),
            analyses: HashMap::new(),
        }
    }

    pub const fn encoding(&self) -> PositionEncoding {
        self.encoding
    }

    pub fn open(&mut self, path: DocumentPath, version: DocumentVersion, text: String) {
        self.documents.insert(
            path,
            OpenDocument {
                version,
                lines: LineIndex::new(text.into()),
            },
        );
    }

    /// Replaces the full text of an open document.
    pub fn change(&mut self, path: &DocumentPath, version: DocumentVersion, text: String) {
        self.open(path.clone(), version, text);
    }

    /// Stops overriding disk text for `path`.
    ///
    /// Ensures: a single-file project for `path` is dropped, and the returned
    /// sets clear every file it had published.
    pub fn close(&mut self, path: &DocumentPath) -> Vec<FileDiagnostics> {
        self.documents.remove(path);
        self.analyses
            .remove(&ProjectKey::SingleFile(path.clone()))
            .map(|analysis| analysis.published.into_iter().map(cleared).collect())
            .unwrap_or_default()
    }

    /// Re-analyzes the project that owns `path`.
    ///
    /// Ensures: one set per file, covering every file in the project and every
    /// file published for it before; ranges use the negotiated encoding.
    /// Ensures: a file under a configured root that its entry does not reach
    /// is analyzed alone, as is a file under no configured root.
    /// Ensures: a load failure becomes one `ProjectLoad` diagnostic at the start
    /// of `path`, and every other previously published file is cleared.
    pub fn analyze(&mut self, path: &DocumentPath) -> Vec<FileDiagnostics> {
        let mut sets = BTreeMap::new();
        let reached = match self.configured_project_for(path) {
            Some(index) => self.analyze_configured(index, path, &mut sets),
            None => false,
        };
        if reached {
            self.analyses.remove(&ProjectKey::SingleFile(path.clone()));
        } else {
            self.analyze_single_file(path, &mut sets);
        }
        sets.into_values().collect()
    }

    /// Formats an open document.
    ///
    /// Ensures: `None` when the document is syntax-invalid, already canonical,
    /// or not open; never a partial rewrite.
    pub fn format(&self, path: &DocumentPath) -> Option<TextReplacement> {
        let document = self.documents.get(path)?;
        let text = document.lines.text();
        let report = Compiler::format(SourceSnapshot::from_text(path.as_path(), text));
        if report.is_blocked() || !report.is_changed() {
            return None;
        }
        let end = u32::try_from(text.len()).expect("source offsets fit in u32");
        Some(TextReplacement {
            range: LineRange {
                start: LinePosition {
                    line: 0,
                    character: 0,
                },
                end: document.lines.position(end, self.encoding),
            },
            text: report.text().to_owned(),
        })
    }

    /// Describes the item at `position` in `path`.
    ///
    /// Ensures: `None` when nothing typed is under the cursor, the position is
    /// invalid, or the owning project's latest analysis failed to load.
    pub fn hover(&self, path: &DocumentPath, position: LinePosition) -> Option<HoverInfo> {
        let (project, module, report) = self.inspect(path, position)?;
        let doc = project
            .report
            .doc_comment_at(&module.module_path, report.offset() as usize);
        let markdown = match (hover_markdown(&report), doc) {
            (Some(markdown), Some(doc)) => format!("{markdown}\n\n---\n\n{doc}"),
            (Some(markdown), None) => markdown,
            (None, Some(doc)) => doc,
            (None, None) => return None,
        };
        let span = match (report.reference(), report.token()) {
            (Some(reference), _) => reference.range(),
            (None, Some(token)) => token.range(),
            (None, None) => return None,
        };
        Some(HoverInfo {
            range: line_range(span.start(), span.end(), &module.lines, self.encoding),
            markdown,
        })
    }

    /// The names that can follow `Alias.` at `position` in `path`: the public
    /// items of the module the alias imports, in source order, with their
    /// signatures and doc comments.
    ///
    /// Ensures: `None` when the text before the cursor is not a module alias,
    /// a dot and part of a name. The alias is looked up in the module's last
    /// analysis, so a half-typed `Lib.` that does not parse still completes.
    pub fn completion(
        &self,
        path: &DocumentPath,
        position: LinePosition,
    ) -> Option<Vec<Completion>> {
        let project = self.latest_for(path)?;
        let module = project.modules.get(path)?;
        let offset = module.lines.offset(position, self.encoding).ok()? as usize;
        let alias = alias_before(module.lines.text(), offset)?;
        let members = project
            .report
            .alias_members(&module.module_path, alias, offset)?;
        Some(
            members
                .items()
                .iter()
                .map(|item| Completion {
                    label: item.name().to_owned(),
                    kind: item.kind(),
                    detail: item.signature().to_owned(),
                    documentation: item.doc().map(str::to_owned),
                })
                .collect(),
        )
    }

    /// The inferred type of each `let` in `path` that states none, placed
    /// after the binding's name, for the lines `first_line..=last_line`.
    pub fn inlay_hints(
        &self,
        path: &DocumentPath,
        first_line: u32,
        last_line: u32,
    ) -> Option<Vec<TypeHintInfo>> {
        let project = self.latest_for(path)?;
        let module = project.modules.get(path)?;
        Some(
            project
                .report
                .let_type_hints(&module.module_path)
                .into_iter()
                .map(|hint| TypeHintInfo {
                    position: line_range(
                        hint.offset(),
                        hint.offset(),
                        &module.lines,
                        self.encoding,
                    )
                    .start,
                    label: hint.label().to_owned(),
                })
                .filter(|hint| (first_line..=last_line).contains(&hint.position.line))
                .collect(),
        )
    }

    /// Where the symbol referenced at `position` in `path` is declared.
    ///
    /// Ensures: the target may be any module of the same project, open or not.
    /// Ensures: `None` for unresolved references and for symbols the compiler
    /// records no declaration name for, such as parameters and local bindings.
    pub fn definition(&self, path: &DocumentPath, position: LinePosition) -> Option<DocumentRange> {
        let (project, _, report) = self.inspect(path, position)?;
        let target = report.reference()?.target()?;
        let target_path = DocumentPath(target.source_path()?.to_owned());
        let range = target.declaration_range()?;
        let lines = &project.modules.get(&target_path)?.lines;
        Some(DocumentRange {
            range: line_range(range.start(), range.end(), lines, self.encoding),
            path: target_path,
        })
    }

    /// Every place the symbol at `position` in `path` is written: its
    /// declaration when `include_declaration`, then its references, across the
    /// whole project that owns `path`.
    ///
    /// Ensures: `None` when no symbol's name is under the cursor.
    pub fn references(
        &self,
        path: &DocumentPath,
        position: LinePosition,
        include_declaration: bool,
    ) -> Option<Vec<DocumentRange>> {
        let (project, occurrences) = self.occurrences(path, position)?;
        Some(
            occurrences
                .occurrences()
                .iter()
                .filter(|occurrence| include_declaration || !occurrence.is_declaration())
                .filter_map(|occurrence| project.document_range(occurrence, self.encoding))
                .collect(),
        )
    }

    /// The edits that rename the symbol at `position` in `path` to `new_name`.
    ///
    /// Ensures: the edits cover the declaration and every reference, grouped
    /// by file; the project with the edits applied has no more errors than
    /// without them, so a rename never introduces a collision or shadowing.
    /// Must not: rename a standard or built-in symbol, a module alias, or a
    /// name to one of another case, such as a type to a lowercase name.
    pub fn rename(
        &self,
        path: &DocumentPath,
        position: LinePosition,
        new_name: &str,
    ) -> Result<BTreeMap<DocumentPath, Vec<TextReplacement>>, RenameError> {
        let (project, occurrences) = self
            .occurrences(path, position)
            .ok_or(RenameError::NothingToRename)?;
        if !occurrences.is_renamable() {
            return Err(RenameError::NotRenamable(occurrences.name().to_owned()));
        }
        if !is_identifier(new_name)
            || starts_uppercase(new_name) != starts_uppercase(occurrences.name())
        {
            return Err(RenameError::InvalidName(new_name.to_owned()));
        }
        let mut edits: BTreeMap<DocumentPath, Vec<(u32, u32)>> = BTreeMap::new();
        for occurrence in occurrences.occurrences() {
            let document = DocumentPath(occurrence.source_path().to_owned());
            edits
                .entry(document)
                .or_default()
                .push((occurrence.range().start(), occurrence.range().end()));
        }
        let anchor = &occurrences.occurrences()[0];
        let anchor = (
            anchor.module_path(),
            anchor.source_path(),
            anchor.range().start(),
        );
        if !project.rename_keeps_meaning(&edits, new_name, anchor) {
            return Err(RenameError::Conflict(new_name.to_owned()));
        }
        Ok(edits
            .into_iter()
            .filter_map(|(document, spans)| {
                let lines = &project.modules.get(&document)?.lines;
                let replacements = spans
                    .into_iter()
                    .map(|(start, end)| TextReplacement {
                        range: line_range(start, end, lines, self.encoding),
                        text: new_name.to_owned(),
                    })
                    .collect();
                Some((document, replacements))
            })
            .collect())
    }

    /// The latest analysis that covers `path`, and the symbol named at
    /// `position` there.
    fn occurrences(
        &self,
        path: &DocumentPath,
        position: LinePosition,
    ) -> Option<(&AnalyzedProject, SymbolOccurrences)> {
        let project = self.latest_for(path)?;
        let module = project.modules.get(path)?;
        let offset = module.lines.offset(position, self.encoding).ok()?;
        let occurrences = project
            .report
            .occurrences(&module.module_path, offset as usize)?;
        Some((project, occurrences))
    }

    /// Inspects the latest analysis that covers `path` at `position`.
    ///
    /// Ensures: the project that owns `path` for diagnostics is the one asked,
    /// so hover and diagnostics never disagree about a file's context.
    fn inspect(
        &self,
        path: &DocumentPath,
        position: LinePosition,
    ) -> Option<(&AnalyzedProject, &AnalyzedModule, InspectionReport)> {
        let project = self.latest_for(path)?;
        let module = project.modules.get(path)?;
        let offset = module.lines.offset(position, self.encoding).ok()?;
        let report = project
            .report
            .inspect(&module.module_path, offset as usize)
            .ok()?;
        Some((project, module, report))
    }

    /// The latest analysis of the project that owns `path`.
    fn latest_for(&self, path: &DocumentPath) -> Option<&AnalyzedProject> {
        let configured = self
            .configured_project_for(path)
            .and_then(|index| self.analyses.get(&ProjectKey::Configured(index)));
        // `analyze` drops the single-file analysis whenever a configured project
        // owns `path`, including after that project fails to load.
        match configured {
            Some(ProjectAnalysis {
                latest: Some(project),
                ..
            }) if project.modules.contains_key(path) => Some(project),
            _ => self
                .analyses
                .get(&ProjectKey::SingleFile(path.clone()))?
                .latest
                .as_ref(),
        }
    }

    /// The configured project with the longest root containing `path`.
    fn configured_project_for(&self, path: &DocumentPath) -> Option<usize> {
        self.projects
            .iter()
            .enumerate()
            .filter(|(_, project)| path.as_path().starts_with(&project.root))
            .max_by_key(|(_, project)| project.root.components().count())
            .map(|(index, _)| index)
    }

    /// Analyzes a configured project into `sets` and reports whether it reached `path`.
    fn analyze_configured(
        &mut self,
        index: usize,
        path: &DocumentPath,
        sets: &mut BTreeMap<DocumentPath, FileDiagnostics>,
    ) -> bool {
        let project = &self.projects[index];
        let loaded = LoadedProject::load_with(
            &project.root,
            project.entry_module.clone(),
            &OpenBuffers(&self.documents),
            &StandardTree::embedded(),
        );
        let analysis = self
            .analyses
            .entry(ProjectKey::Configured(index))
            .or_default();
        let previous = std::mem::take(&mut analysis.published);
        analysis.latest = None;
        let loaded = match loaded {
            Ok(loaded) => loaded,
            Err(error) => {
                for stale in previous {
                    sets.insert(stale.clone(), cleared(stale));
                }
                let failure = load_failure(path, &self.documents, error.to_string());
                analysis.published.insert(path.clone());
                sets.insert(path.clone(), failure);
                return true;
            }
        };

        let report = analysis.session.analyze_project(loaded.sources());
        let mut modules = HashMap::new();
        for module in report.diagnostics_by_module() {
            let document = DocumentPath(module.source_path().to_owned());
            let open = self.documents.get(&document);
            let lines = analyzed_lines(open, module.source_text());
            let diagnostics = module
                .diagnostics()
                .iter()
                .map(|diagnostic| editor_diagnostic(diagnostic, &lines, self.encoding))
                .collect();
            modules.insert(
                document.clone(),
                AnalyzedModule {
                    module_path: module.module_path().to_owned(),
                    lines,
                },
            );
            analysis.published.insert(document.clone());
            sets.insert(
                document.clone(),
                FileDiagnostics {
                    path: document,
                    version: open.map(|open| open.version),
                    diagnostics,
                },
            );
        }
        for stale in previous.difference(&analysis.published) {
            sets.insert(stale.clone(), cleared(stale.clone()));
        }
        analysis.latest = Some(AnalyzedProject { report, modules });
        analysis.published.contains(path)
    }

    /// Analyzes `path` alone as module `main` into `sets`.
    fn analyze_single_file(
        &mut self,
        path: &DocumentPath,
        sets: &mut BTreeMap<DocumentPath, FileDiagnostics>,
    ) {
        let analysis = self
            .analyses
            .entry(ProjectKey::SingleFile(path.clone()))
            .or_default();
        analysis.published.insert(path.clone());
        analysis.latest = None;
        let text = match OpenBuffers(&self.documents).read(path.as_path()) {
            Ok(text) => text,
            Err(error) => {
                let message = format!("cannot read {}: {error}", path.as_path().display());
                sets.insert(path.clone(), load_failure(path, &self.documents, message));
                return;
            }
        };
        let source = ProjectSource::new("main", SourceSnapshot::from_text(path.as_path(), text))
            .expect("`main` is a valid module path");
        let report = analysis.session.analyze_project(vec![source]);
        let open = self.documents.get(path);
        let grouped = report.diagnostics_by_module();
        let module = grouped
            .first()
            .expect("a single-file project has one module");
        let lines = analyzed_lines(open, module.source_text());
        sets.insert(
            path.clone(),
            FileDiagnostics {
                path: path.clone(),
                version: open.map(|open| open.version),
                diagnostics: module
                    .diagnostics()
                    .iter()
                    .map(|diagnostic| editor_diagnostic(diagnostic, &lines, self.encoding))
                    .collect(),
            },
        );
        let analyzed = AnalyzedModule {
            module_path: module.module_path().to_owned(),
            lines,
        };
        drop(grouped);
        analysis.latest = Some(AnalyzedProject {
            report,
            modules: HashMap::from([(path.clone(), analyzed)]),
        });
    }
}

/// The alias in `Alias.partial` that ends at `offset`, where `partial` is
/// the start of a name, possibly empty. `None` for a longer path such as
/// `a.Alias.`, whose first segment is not an alias.
fn alias_before(text: &str, offset: usize) -> Option<&str> {
    let before = text.get(..offset)?;
    let is_name = |c: char| c == '_' || c.is_ascii_alphanumeric();
    let partial_start = before.trim_end_matches(is_name).len();
    let dotted = before[..partial_start].strip_suffix('.')?;
    let alias_start = dotted.trim_end_matches(is_name).len();
    let alias = &dotted[alias_start..];
    let starts_name = alias
        .chars()
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic());
    (starts_name && !dotted[..alias_start].ends_with('.')).then_some(alias)
}

/// Renders hover text: the callable contract when the cursor names a function,
/// as instantiated at this call when the function is generic, otherwise the
/// inferred type, headed by the referenced name.
///
/// Ensures: `None` when the report has neither a contract nor a type.
fn hover_markdown(report: &InspectionReport) -> Option<String> {
    let description = report
        .instantiated_callable()
        .or_else(|| report.callable())
        .map(CallableInspection::display)
        .or_else(|| report.inferred_type())?;
    let reference = report.reference();
    let signature = match reference {
        Some(reference) => format!("{}: {description}", reference.text()),
        None => description.to_owned(),
    };
    let mut markdown = format!("```chimi\n{signature}\n```");
    if let Some(target) = reference.and_then(ReferenceInspection::target) {
        match target.module_path() {
            Some(module) => write!(markdown, "\n\n{} in `{module}`", target.kind()),
            None => write!(markdown, "\n\n{}", target.kind()),
        }
        .expect("writing to a string cannot fail");
    }
    Some(markdown)
}

/// The analyzed text of a module: the open buffer's index when the module was
/// read from it, otherwise a new index over the text the loader read.
fn analyzed_lines(open: Option<&OpenDocument>, analyzed_text: &str) -> LineIndex {
    match open {
        Some(open) if open.lines.text() == analyzed_text => open.lines.clone(),
        _ => LineIndex::new(analyzed_text.into()),
    }
}

fn line_range(start: u32, end: u32, lines: &LineIndex, encoding: PositionEncoding) -> LineRange {
    LineRange {
        start: lines.position(start, encoding),
        end: lines.position(end, encoding),
    }
}

fn cleared(path: DocumentPath) -> FileDiagnostics {
    FileDiagnostics {
        path,
        version: None,
        diagnostics: Vec::new(),
    }
}

fn load_failure(
    path: &DocumentPath,
    documents: &HashMap<DocumentPath, OpenDocument>,
    message: String,
) -> FileDiagnostics {
    let start = LinePosition {
        line: 0,
        character: 0,
    };
    FileDiagnostics {
        path: path.clone(),
        version: documents.get(path).map(|document| document.version),
        diagnostics: vec![EditorDiagnostic {
            range: LineRange { start, end: start },
            origin: DiagnosticOrigin::ProjectLoad,
            message,
        }],
    }
}

fn editor_diagnostic(
    diagnostic: &Diagnostic,
    lines: &LineIndex,
    encoding: PositionEncoding,
) -> EditorDiagnostic {
    EditorDiagnostic {
        range: {
            let range = diagnostic.range();
            line_range(range.start(), range.end(), lines, encoding)
        },
        origin: DiagnosticOrigin::Compiler {
            stage: diagnostic.stage(),
            code: diagnostic.code(),
        },
        message: diagnostic.message().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DocumentPath, DocumentRange, DocumentVersion, ProjectConfig, RenameError, Workspace,
        alias_before,
    };
    use chimi::line_index::{LinePosition, PositionEncoding};
    use std::fs;
    use std::path::{Path, PathBuf};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("chimi-lsp-{name}-{}", std::process::id()));
            if path.exists() {
                fs::remove_dir_all(&path).unwrap();
            }
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn write(&self, relative: &str, text: &str) -> DocumentPath {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text).unwrap();
            DocumentPath::canonicalize(path).unwrap()
        }

        fn root(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Guards the stale-diagnostic invariant across files: fixing an imported
    /// module in an unsaved buffer must clear what was published for it.
    #[test]
    fn fixing_an_imported_module_clears_its_published_diagnostics() {
        let fixture = Fixture::new("clear");
        let app = fixture.write(
            "app.chimi",
            "import lib as Lib\npub fn main() -> Int { Lib.value() }\n",
        );
        let lib = fixture.write("lib.chimi", "pub fn value() -> Int { missing() }\n");
        let project = ProjectConfig::new(fixture.root(), "app").unwrap();
        let mut workspace = Workspace::new(vec![project], PositionEncoding::Utf16);

        let before = workspace.analyze(&app);
        let lib_before = before
            .iter()
            .find(|set| set.path == lib)
            .expect("lib published");
        assert!(!lib_before.diagnostics.is_empty());

        workspace.open(
            lib.clone(),
            DocumentVersion(2),
            "pub fn value() -> Int { 1 }\n".into(),
        );
        let after = workspace.analyze(&app);
        let lib_after = after
            .iter()
            .find(|set| set.path == lib)
            .expect("lib republished");
        assert_eq!(lib_after.version, Some(DocumentVersion(2)));
        assert!(lib_after.diagnostics.is_empty());
    }

    /// Guards the formatter contract at the editor boundary: syntax-invalid
    /// buffers are never rewritten, valid ones are replaced whole.
    #[test]
    fn formatting_rewrites_only_syntax_valid_documents() {
        let fixture = Fixture::new("format");
        let path = fixture.write("main.chimi", "");
        let mut workspace = Workspace::new(Vec::new(), PositionEncoding::Utf16);

        workspace.open(
            path.clone(),
            DocumentVersion(1),
            "pub fn main() {\n  Answer(value 42)\n}\n".into(),
        );
        assert_eq!(workspace.format(&path), None);

        workspace.change(
            &path,
            DocumentVersion(2),
            "pub fn main( ) -> Int {1}\n".into(),
        );
        let edit = workspace
            .format(&path)
            .expect("valid source is reformatted");
        assert_eq!(
            edit.range.start,
            chimi::line_index::LinePosition {
                line: 0,
                character: 0
            }
        );
        assert_eq!(
            edit.range.end,
            chimi::line_index::LinePosition {
                line: 1,
                character: 0
            }
        );
        assert_ne!(edit.text, "pub fn main( ) -> Int {1}\n");
    }

    const fn at(line: u32, character: u32) -> LinePosition {
        LinePosition { line, character }
    }

    /// Guards cross-module navigation: the target file is not open, so its
    /// range must come from the analyzed text, not an open buffer.
    #[test]
    fn definition_resolves_into_an_unopened_imported_module() {
        let fixture = Fixture::new("definition");
        let app = fixture.write(
            "app.chimi",
            "import lib as Lib\npub fn main() -> Int { Lib.value() }\n",
        );
        let lib = fixture.write("lib.chimi", "\n\npub fn value() -> Int { 1 }\n");
        let project = ProjectConfig::new(fixture.root(), "app").unwrap();
        let mut workspace = Workspace::new(vec![project], PositionEncoding::Utf16);
        workspace.analyze(&app);

        // `value` in `Lib.value()` on line 1.
        let target = workspace.definition(&app, at(1, 28)).expect("resolves");

        assert_eq!(target.path, lib);
        assert_eq!(target.range.start, at(2, 7));
        assert_eq!(target.range.end, at(2, 12));
    }

    /// Guards the query invariant: positions follow the analyzed unsaved text,
    /// and the hover shows a generic callee as this call instantiates it, over
    /// the reference's span.
    #[test]
    fn hover_follows_unsaved_text_and_shows_the_callee_contract() {
        let fixture = Fixture::new("hover");
        let app = fixture.write("app.chimi", "pub fn main() -> Int { 0 }\n");
        let project = ProjectConfig::new(fixture.root(), "app").unwrap();
        let mut workspace = Workspace::new(vec![project], PositionEncoding::Utf16);
        workspace.open(
            app.clone(),
            DocumentVersion(2),
            "pub fn same<T>(value: T) -> T { value }\n\npub fn main() -> Int { same(1) }\n".into(),
        );
        workspace.analyze(&app);

        let hover = workspace.hover(&app, at(2, 25)).expect("hovers `same`");

        assert_eq!(hover.range.start, at(2, 23));
        assert_eq!(hover.range.end, at(2, 27));
        assert!(
            hover.markdown.contains("same: fn(Int) -> Int"),
            "{}",
            hover.markdown
        );
    }

    /// `Lib.` completes to the imported module's public items, in source
    /// order, with their signatures and doc comments, while the line does not
    /// parse yet.
    #[test]
    fn completion_after_an_alias_lists_the_module_public_items() {
        let fixture = Fixture::new("completion");
        fixture.write(
            "lib.chimi",
            "/// A place on the grid.\npub struct Point {\n  x: Int,\n  y: Int,\n}\n\nfn hidden() -> Int { 0 }\n\n/// The sum of both coordinates.\npub fn total(point: Point) -> Int {\n  point.x + point.y\n}\n\npub const origin = 0\n",
        );
        let app = fixture.write(
            "app.chimi",
            "import lib as Lib\n\npub fn main() -> Int { 0 }\n",
        );
        let project = ProjectConfig::new(fixture.root(), "app").unwrap();
        let mut workspace = Workspace::new(vec![project], PositionEncoding::Utf16);
        workspace.open(
            app.clone(),
            DocumentVersion(2),
            "import lib as Lib\nimport std/list as List\n\npub fn main() -> Int {\n  Lib.to\n  List.\n  // Lib.\n  let a = \"Lib.\n  let b = \"Lib.x\"\n}\n".into(),
        );
        workspace.analyze(&app);

        let items = workspace
            .completion(&app, at(4, 8))
            .expect("completes `Lib.`");
        let labels: Vec<(&str, &str)> = items
            .iter()
            .map(|item| (item.label.as_str(), item.kind))
            .collect();
        assert_eq!(
            labels,
            vec![("Point", "struct"), ("total", "fn"), ("origin", "const")]
        );
        assert_eq!(items[1].detail, "pub fn total(point: Point) -> Int");
        assert_eq!(
            items[1].documentation.as_deref(),
            Some("The sum of both coordinates.")
        );
        assert_eq!(items[2].documentation, None);
        // Right after the dot, and for a standard module.
        assert_eq!(
            workspace
                .completion(&app, at(4, 6))
                .map(|items| items.len()),
            Some(3)
        );
        let standard = workspace
            .completion(&app, at(5, 7))
            .expect("completes `List.`");
        assert!(standard.iter().any(|item| item.label == "length"));
        // Not after an alias, nor inside a comment or a text literal.
        assert_eq!(workspace.completion(&app, at(3, 4)), None);
        assert_eq!(workspace.completion(&app, at(6, 9)), None);
        assert_eq!(workspace.completion(&app, at(7, 15)), None);
        assert_eq!(workspace.completion(&app, at(8, 15)), None);
    }

    #[test]
    fn completion_needs_an_alias_before_the_dot() {
        assert_eq!(alias_before("  Lib.to", 8), Some("Lib"));
        assert_eq!(alias_before("  Lib.", 6), Some("Lib"));
        assert_eq!(alias_before("  Lib", 5), None);
        assert_eq!(alias_before("  point.Lib.", 12), None);
        assert_eq!(alias_before("  1.", 4), None);
        assert_eq!(alias_before("  .x", 4), None);
    }

    /// Each `let` without an annotation shows its inferred type after the
    /// name; an annotated one, and one whose type an error leaves unknown,
    /// show none.
    #[test]
    fn inlay_hints_show_the_type_of_unannotated_lets() {
        let fixture = Fixture::new("inlay");
        let app = fixture.write(
            "app.chimi",
            "pub fn main() -> Int {\n  let items = [1, 2]\n  let total: Int = 3\n  let pair = (total, \"x\")\n  let broken = missing()\n  let wrapped = [missing()]\n  let open = fn(a) { 1 }\n  let generic = same\n  let double = fn(a: Int) { a * 2 }\n  total\n}\n\nfn same<T>(value: T) -> T { value }\n",
        );
        let project = ProjectConfig::new(fixture.root(), "app").unwrap();
        let mut workspace = Workspace::new(vec![project], PositionEncoding::Utf16);
        workspace.analyze(&app);

        let hints = workspace.inlay_hints(&app, 0, 10).expect("hints");
        let shown: Vec<(u32, u32, &str)> = hints
            .iter()
            .map(|hint| {
                (
                    hint.position.line,
                    hint.position.character,
                    hint.label.as_str(),
                )
            })
            .collect();
        assert_eq!(
            shown,
            vec![
                (1, 11, ": List<Int>"),
                (3, 10, ": (Int, Text)"),
                (8, 12, ": fn(Int) -> Int"),
            ]
        );
        // Only the lines asked for.
        assert_eq!(
            workspace.inlay_hints(&app, 2, 3).map(|hints| hints.len()),
            Some(1)
        );
    }

    /// Hover shows the item's doc comment under its contract, at a use in
    /// another module and at the declaration.
    #[test]
    fn hover_shows_the_doc_comment() {
        let fixture = Fixture::new("hover-doc");
        let lib = fixture.write(
            "lib.chimi",
            "/// Adds one.\npub fn bump(value: Int) -> Int { value + 1 }\n\npub fn plain() -> Int { 0 }\n\n/// Adds one.\npub fn keep<T>(value: T) -> T { value }\n",
        );
        let app = fixture.write(
            "app.chimi",
            "import lib as Lib\n\npub fn main() -> Int {\n  Lib.bump(Lib.plain())\n}\n",
        );
        let project = ProjectConfig::new(fixture.root(), "app").unwrap();
        let mut workspace = Workspace::new(vec![project], PositionEncoding::Utf16);
        workspace.analyze(&app);

        let used = workspace.hover(&app, at(3, 7)).expect("hovers `bump`");
        assert!(
            used.markdown.ends_with("\n\n---\n\nAdds one."),
            "{}",
            used.markdown
        );
        assert!(
            used.markdown.starts_with("```chimi\nbump: "),
            "{}",
            used.markdown
        );
        let plain = workspace.hover(&app, at(3, 16)).expect("hovers `plain`");
        assert!(!plain.markdown.contains("---"), "{}", plain.markdown);
        // A type parameter shares its function's id, not its doc.
        assert!(
            workspace
                .hover(&lib, at(6, 22))
                .is_none_or(|hover| !hover.markdown.contains("Adds one."))
        );
        let declared = workspace.hover(&lib, at(1, 8));
        assert!(declared.is_some_and(|hover| hover.markdown.contains("Adds one.")),);
    }

    /// Guards that a failed load never answers from an older report, whose
    /// offsets belong to text the editor no longer shows.
    #[test]
    fn queries_answer_nothing_after_a_load_failure() {
        let fixture = Fixture::new("load-failure");
        let app = fixture.write("app.chimi", "pub fn main() -> Int { main() }\n");
        let project = ProjectConfig::new(fixture.root(), "app").unwrap();
        let mut workspace = Workspace::new(vec![project], PositionEncoding::Utf16);
        workspace.analyze(&app);
        assert!(workspace.definition(&app, at(0, 24)).is_some());

        workspace.change(
            &app,
            DocumentVersion(2),
            "import missing as Missing\npub fn main() -> Int { main() }\n".into(),
        );
        workspace.analyze(&app);

        assert_eq!(workspace.definition(&app, at(1, 24)), None);
        assert_eq!(workspace.hover(&app, at(1, 24)), None);
    }

    /// Guards the fallback for files the configured entry never imports: they
    /// are analyzed alone instead of silently receiving no diagnostics.
    #[test]
    fn unreached_files_under_a_configured_root_are_analyzed_alone() {
        let fixture = Fixture::new("unreached");
        fixture.write("app.chimi", "pub fn main() -> Int { 0 }\n");
        let scratch = fixture.write("scratch.chimi", "pub fn main() -> Int { missing() }\n");
        let project = ProjectConfig::new(fixture.root(), "app").unwrap();
        let mut workspace = Workspace::new(vec![project], PositionEncoding::Utf16);

        let sets = workspace.analyze(&scratch);

        let scratch_set = sets
            .iter()
            .find(|set| set.path == scratch)
            .expect("scratch published");
        assert!(!scratch_set.diagnostics.is_empty());
        assert_eq!(sets.iter().filter(|set| set.path == scratch).count(), 1);
    }

    /// The ranges' (line, start column, end column), sorted by path then start.
    fn spans(ranges: &[DocumentRange]) -> Vec<(PathBuf, u32, u32, u32)> {
        let mut spans: Vec<_> = ranges
            .iter()
            .map(|target| {
                (
                    target.path.as_path().to_owned(),
                    target.range.start.line,
                    target.range.start.character,
                    target.range.end.character,
                )
            })
            .collect();
        spans.sort();
        spans
    }

    fn two_module_project(name: &str) -> (Fixture, DocumentPath, DocumentPath, Workspace) {
        let fixture = Fixture::new(name);
        let app = fixture.write(
            "app.chimi",
            "import lib as Lib\nimport lib.{Point}\n\npub fn main() -> Int {\n  let start = Lib.origin()\n  Lib.total(start) + Lib.total(Point(1, 2))\n}\n",
        );
        let lib = fixture.write(
            "lib.chimi",
            "pub struct Point {\n  x: Int,\n  y: Int,\n}\n\npub fn origin() -> Point {\n  Point(0, 0)\n}\n\npub fn total(point: Point) -> Int {\n  point.x + point.y\n}\n",
        );
        let project = ProjectConfig::new(fixture.root(), "app").unwrap();
        let mut workspace = Workspace::new(vec![project], PositionEncoding::Utf16);
        workspace.analyze(&app);
        (fixture, app, lib, workspace)
    }

    /// Find references covers every module of the project, from a reference
    /// or from the declaration, with or without the declaration itself.
    #[test]
    fn references_cover_the_declaration_and_every_module() {
        let (_fixture, app, lib, workspace) = two_module_project("references");
        let app_path = app.as_path().to_owned();
        let lib_path = lib.as_path().to_owned();

        // `total` in `Lib.total(start)` on line 5.
        let all = workspace
            .references(&app, at(5, 8), true)
            .expect("a symbol");
        assert_eq!(
            spans(&all),
            vec![
                (app_path.clone(), 5, 6, 11),
                (app_path.clone(), 5, 25, 30),
                (lib_path.clone(), 9, 7, 12),
            ]
        );
        // From the declaration, without it.
        let uses = workspace
            .references(&lib, at(9, 8), false)
            .expect("a symbol");
        assert_eq!(
            spans(&uses),
            vec![(app_path.clone(), 5, 6, 11), (app_path, 5, 25, 30)]
        );
        assert_eq!(workspace.references(&app, at(2, 0), true), None);
    }

    /// A local binding's references stay in its scope, and the declaration
    /// is the binding's name.
    #[test]
    fn references_of_a_local_binding() {
        let (_fixture, app, _lib, workspace) = two_module_project("local-references");
        let app_path = app.as_path().to_owned();

        let all = workspace
            .references(&app, at(4, 7), true)
            .expect("a symbol");

        assert_eq!(
            spans(&all),
            vec![(app_path.clone(), 4, 6, 11), (app_path, 5, 12, 17)]
        );
    }

    /// Renaming a struct renames its type uses, its constructor calls, the
    /// member import and the declaration, in every module.
    #[test]
    fn rename_covers_type_constructor_and_import() {
        let (_fixture, app, lib, workspace) = two_module_project("rename");

        // `Point` in `Point(1, 2)` on line 5.
        let edits = workspace.rename(&app, at(5, 32), "Spot").expect("renames");

        let lines: Vec<(u32, u32)> = edits[&lib]
            .iter()
            .map(|edit| (edit.range.start.line, edit.range.start.character))
            .collect();
        assert_eq!(lines, vec![(0, 11), (5, 19), (6, 2), (9, 20)]);
        let lines: Vec<(u32, u32)> = edits[&app]
            .iter()
            .map(|edit| (edit.range.start.line, edit.range.start.character))
            .collect();
        assert_eq!(lines, vec![(1, 12), (5, 31)]);
        assert!(edits.values().flatten().all(|edit| edit.text == "Spot"));
    }

    #[test]
    fn rename_refuses_names_outside_the_project_and_bad_names() {
        let (_fixture, app, _lib, workspace) = two_module_project("rename-refused");

        // The alias `Lib`: a module, not a declaration.
        assert!(matches!(
            workspace.rename(&app, at(5, 3), "Library"),
            Err(RenameError::NotRenamable(_))
        ));
        // `total` cannot become a type name, nor a non-identifier.
        assert_eq!(
            workspace.rename(&app, at(5, 8), "Total"),
            Err(RenameError::InvalidName("Total".to_owned()))
        );
        assert_eq!(
            workspace.rename(&app, at(5, 8), "to-tal"),
            Err(RenameError::InvalidName("to-tal".to_owned()))
        );
        // `total` to `origin` collides with the other function.
        assert_eq!(
            workspace.rename(&app, at(5, 8), "origin"),
            Err(RenameError::Conflict("origin".to_owned()))
        );
        assert_eq!(
            workspace.rename(&app, at(2, 0), "anything"),
            Err(RenameError::NothingToRename)
        );
    }

    #[test]
    fn rename_refuses_a_standard_function() {
        let fixture = Fixture::new("rename-std");
        let app = fixture.write(
            "app.chimi",
            "import std/list as List\n\npub fn main() -> Int {\n  List.length([1])\n}\n",
        );
        let project = ProjectConfig::new(fixture.root(), "app").unwrap();
        let mut workspace = Workspace::new(vec![project], PositionEncoding::Utf16);
        workspace.analyze(&app);

        assert_eq!(
            workspace.rename(&app, at(3, 8), "size"),
            Err(RenameError::NotRenamable("length".to_owned()))
        );
    }

    /// A rename that still type-checks but makes a local shadow an item, or
    /// an item take a local's name, changes what a reference means.
    #[test]
    fn rename_refuses_a_shadowing_that_type_checks() {
        let fixture = Fixture::new("rename-shadow");
        let app = fixture.write(
            "app.chimi",
            "const limit = 5\n\nfn f(n: Int) -> Int {\n  n + limit\n}\n\nfn spare() -> Int { 2 }\n\npub fn main() -> Int { f(1) }\n\nfn extra() -> Int { 3 }\n",
        );
        let project = ProjectConfig::new(fixture.root(), "app").unwrap();
        let mut workspace = Workspace::new(vec![project], PositionEncoding::Utf16);
        workspace.analyze(&app);

        // The constant to the parameter's name, and the parameter to the
        // constant's.
        assert_eq!(
            workspace.rename(&app, at(0, 7), "n"),
            Err(RenameError::Conflict("n".to_owned()))
        );
        assert_eq!(
            workspace.rename(&app, at(2, 5), "limit"),
            Err(RenameError::Conflict("limit".to_owned()))
        );
        // A name nothing else uses is fine, and a longer one shifts the
        // later spans in the file.
        let edits = workspace.rename(&app, at(2, 5), "count").expect("renames");
        let spans: Vec<(u32, u32)> = edits[&app]
            .iter()
            .map(|edit| (edit.range.start.line, edit.range.start.character))
            .collect();
        assert_eq!(spans, vec![(2, 5), (3, 2)]);
        assert!(workspace.rename(&app, at(0, 7), "maximum").is_ok());
        // An unused function renamed to the name of another, not next to it,
        // changes no reference but declares the name twice.
        assert_eq!(
            workspace.rename(&app, at(6, 4), "extra"),
            Err(RenameError::Conflict("extra".to_owned()))
        );
    }

    /// A parameter that shares an item's name is not the imported item.
    #[test]
    fn a_local_named_like_an_imported_item_is_not_imported() {
        let fixture = Fixture::new("local-import-name");
        let lib = fixture.write(
            "lib.chimi",
            "pub const total = 3\n\npub fn twice(total: Int) -> Int {\n  total + total\n}\n",
        );
        let app = fixture.write(
            "app.chimi",
            "import lib.{total}\nimport lib as Lib\n\npub fn main() -> Int {\n  Lib.twice(total)\n}\n",
        );
        let project = ProjectConfig::new(fixture.root(), "app").unwrap();
        let mut workspace = Workspace::new(vec![project], PositionEncoding::Utf16);
        workspace.analyze(&app);
        let lib_path = lib.as_path().to_owned();

        let uses = workspace
            .references(&lib, at(2, 14), true)
            .expect("a symbol");

        assert_eq!(
            spans(&uses),
            vec![
                (lib_path.clone(), 2, 13, 18),
                (lib_path.clone(), 3, 2, 7),
                (lib_path, 3, 10, 15),
            ]
        );
    }

    /// Every clause head of a clause group is the function's name.
    #[test]
    fn rename_covers_every_clause_of_a_clause_group() {
        let fixture = Fixture::new("rename-clauses");
        let app = fixture.write(
            "app.chimi",
            "pub fn main() -> Int {\n  sign(3)\n}\n\nfn sign(0) -> Int { 0 }\nfn sign(n) { 1 }\n",
        );
        let project = ProjectConfig::new(fixture.root(), "app").unwrap();
        let mut workspace = Workspace::new(vec![project], PositionEncoding::Utf16);
        workspace.analyze(&app);

        let edits = workspace.rename(&app, at(1, 3), "signum").expect("renames");

        let lines: Vec<(u32, u32)> = edits[&app]
            .iter()
            .map(|edit| (edit.range.start.line, edit.range.start.character))
            .collect();
        assert_eq!(lines, vec![(4, 3), (5, 3), (1, 2)]);
    }
}
