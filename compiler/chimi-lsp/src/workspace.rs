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
use chimi::project::{LoadedProject, SourceReader};
use chimi::source::SourceSnapshot;
use chimi::{
    CallableInspection, Compiler, CompilerSession, IncrementalTypeReport, InspectionReport,
    ProjectSource, ReferenceInspection,
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
        let (_, module, report) = self.inspect(path, position)?;
        let markdown = hover_markdown(&report)?;
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

    /// Inspects the latest analysis that covers `path` at `position`.
    ///
    /// Ensures: the project that owns `path` for diagnostics is the one asked,
    /// so hover and diagnostics never disagree about a file's context.
    fn inspect(
        &self,
        path: &DocumentPath,
        position: LinePosition,
    ) -> Option<(&AnalyzedProject, &AnalyzedModule, InspectionReport)> {
        let configured = self
            .configured_project_for(path)
            .and_then(|index| self.analyses.get(&ProjectKey::Configured(index)));
        // `analyze` drops the single-file analysis whenever a configured project
        // owns `path`, including after that project fails to load.
        let project = match configured {
            Some(ProjectAnalysis {
                latest: Some(project),
                ..
            }) if project.modules.contains_key(path) => project,
            _ => self
                .analyses
                .get(&ProjectKey::SingleFile(path.clone()))?
                .latest
                .as_ref()?,
        };
        let module = project.modules.get(path)?;
        let offset = module.lines.offset(position, self.encoding).ok()?;
        let report = project
            .report
            .inspect(&module.module_path, offset as usize)
            .ok()?;
        Some((project, module, report))
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
    use super::{DocumentPath, DocumentVersion, ProjectConfig, Workspace};
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
}
