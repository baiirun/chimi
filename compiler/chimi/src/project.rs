use crate::compiler::{ModuleOrigin, ProjectPathError, ProjectSource};
use crate::diagnostic::Diagnostic;
use crate::ids::{FileId, TextRange};
use crate::json::write_json_string;
use crate::semantic::{self, ImportBinding};
use crate::source::SourceSnapshot;
use std::collections::HashMap;
use std::fmt::{self, Write};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod packages;
use packages::PackageGraph;
pub use packages::PackageError;

#[derive(Debug)]
pub struct LoadedProject {
    modules: Vec<ProjectSource>,
    /// Each doc example module's path and the module it documents.
    examples: Vec<(String, String)>,
}

/// Where a build reads `std/` modules from: the tree embedded in the compiler,
/// or a directory that a compiler developer passes with `--std-dir`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StandardTree {
    /// Canonical; `None` for the embedded tree.
    directory: Option<PathBuf>,
}

impl StandardTree {
    pub const fn embedded() -> Self {
        Self { directory: None }
    }

    /// The tree in `directory`, which holds `list.chimi` for `std/list` as
    /// `compiler/chimi/std/` does. Its modules are standard source, so they may
    /// declare `native` items.
    pub fn directory(directory: impl AsRef<Path>) -> Result<Self, ProjectLoadError> {
        let requested = directory.as_ref();
        let read_error = |source| ProjectLoadError::ReadStandardDirectory {
            path: requested.to_owned(),
            source,
        };
        let directory = requested.canonicalize().map_err(read_error)?;
        if !directory.is_dir() {
            return Err(read_error(io::Error::new(
                io::ErrorKind::NotADirectory,
                "not a directory",
            )));
        }
        Ok(Self {
            directory: Some(directory),
        })
    }

    /// The directory that replaces the embedded tree, if any.
    pub fn override_directory(&self) -> Option<&Path> {
        self.directory.as_deref()
    }

    /// Adds the standard modules `modules` import, directly or transitively.
    /// An override directory replaces the whole embedded tree, so it must hold
    /// every embedded module that the build imports.
    fn add_imported_modules(
        &self,
        modules: &mut Vec<ProjectSource>,
    ) -> Result<(), ProjectLoadError> {
        let Some(directory) = &self.directory else {
            crate::standard_tree::add_imported_modules(modules);
            return Ok(());
        };
        crate::standard_tree::add_imported_modules_from(modules, |module_path| {
            read_standard_module(directory, module_path)
        })
    }
}

/// Standard module `module_path` from the tree in `directory`, or `None` when
/// neither the directory nor the embedded tree has it (a built-in module such
/// as `std/map`, or none at all). A module that only the embedded tree has is
/// an error: falling back to it would mix two trees in one build.
fn read_standard_module(
    directory: &Path,
    module_path: &str,
) -> Result<Option<ProjectSource>, ProjectLoadError> {
    let Some(relative) = module_path
        .strip_prefix("std/")
        .filter(|_| ProjectSource::is_valid_path(module_path))
    else {
        return Ok(None);
    };
    let path = directory.join(format!("{relative}.chimi"));
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some(crate::standard_tree::standard_module(
            module_path,
            text,
        ))),
        Err(error)
            if error.kind() == io::ErrorKind::NotFound
                && crate::standard_tree::embedded_module(module_path).is_none() =>
        {
            Ok(None)
        }
        Err(source) => Err(ProjectLoadError::ReadSource {
            module: module_path.to_owned(),
            path,
            source,
        }),
    }
}

/// The module path of each `.chimi` file under `directory`, as `std/list` for
/// `list.chimi`, in ascending order.
fn standard_module_paths(directory: &Path) -> Result<Vec<String>, ProjectLoadError> {
    let paths = module_paths_under(directory, |_| true)
        .map_err(|(path, source)| ProjectLoadError::ReadStandardDirectory { path, source })?;
    Ok(paths.into_iter().map(|path| format!("std/{path}")).collect())
}

/// The path from `directory` of each `.chimi` file under it, without the
/// extension and joined with `/`, in ascending order. It skips symbolic links
/// and hidden files, and descends only into subdirectories whose name
/// `accept` takes. An error names the directory it could not read.
fn module_paths_under(
    directory: &Path,
    accept: impl Fn(&str) -> bool,
) -> Result<Vec<String>, (PathBuf, io::Error)> {
    let mut paths = Vec::new();
    let mut pending = vec![directory.to_owned()];
    while let Some(current) = pending.pop() {
        let failed = |source| (current.clone(), source);
        for entry in std::fs::read_dir(&current).map_err(failed)? {
            let entry = entry.map_err(failed)?;
            let kind = entry.file_type().map_err(failed)?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if kind.is_dir() {
                if accept(&name) {
                    pending.push(path);
                }
            } else if kind.is_file()
                && !name.starts_with('.')
                && path
                    .extension()
                    .is_some_and(|extension| extension == "chimi")
            {
                let relative = path
                    .strip_prefix(directory)
                    .expect("walked paths are inside the directory")
                    .with_extension("");
                paths.push(
                    relative
                        .components()
                        .map(|component| component.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/"),
                );
            }
        }
    }
    paths.sort();
    Ok(paths)
}

impl LoadedProject {
    /// Every module of `standard`, for checking the whole tree against the
    /// operation registry (`chimi std check`).
    pub fn standard_tree(standard: &StandardTree) -> Result<Self, ProjectLoadError> {
        let Some(directory) = &standard.directory else {
            let modules = crate::standard_tree::embedded_module_paths()
                .map(|path| {
                    crate::standard_tree::standard_source(path).expect("listed module is embedded")
                })
                .collect();
            return Ok(Self {
            modules,
            examples: Vec::new(),
        });
        };
        let mut modules = Vec::new();
        for module_path in standard_module_paths(directory)? {
            let module = read_standard_module(directory, &module_path)?.ok_or_else(|| {
                ProjectLoadError::InvalidModule(ProjectPathError::new(module_path.clone()))
            })?;
            modules.push(module);
        }
        standard.add_imported_modules(&mut modules)?;
        Ok(Self {
            modules,
            examples: Vec::new(),
        })
    }

    pub fn single_file(
        path: impl AsRef<Path>,
        standard: &StandardTree,
    ) -> Result<Self, ProjectLoadError> {
        let path = path.as_ref();
        let source = SourceSnapshot::read(path).map_err(|source| ProjectLoadError::ReadSource {
            module: "main".to_owned(),
            path: path.to_owned(),
            source,
        })?;
        let module = ProjectSource::parsed("main", source)
            .expect("the reserved single-file module path is valid");
        let mut modules = vec![module];
        standard.add_imported_modules(&mut modules)?;
        Ok(Self {
            modules,
            examples: Vec::new(),
        })
    }

    pub fn load(
        root: impl AsRef<Path>,
        entry_module: impl Into<String>,
        standard: &StandardTree,
    ) -> Result<Self, ProjectLoadError> {
        Self::load_with(root, entry_module, &FileSystemReader, standard)
    }

    /// Loads a project like [`LoadedProject::load`], taking module text from `reader`.
    ///
    /// Ensures: module discovery, validation, and errors match `load`; only
    /// where text comes from differs. Paths are still canonicalized on disk.
    pub fn load_with(
        root: impl AsRef<Path>,
        entry_module: impl Into<String>,
        reader: &dyn SourceReader,
        standard: &StandardTree,
    ) -> Result<Self, ProjectLoadError> {
        let requested_root = root.as_ref();
        let root = requested_root
            .canonicalize()
            .map_err(|source| ProjectLoadError::ReadRoot {
                path: requested_root.to_owned(),
                source,
            })?;
        if !root.is_dir() {
            return Err(ProjectLoadError::RootNotDirectory { path: root });
        }

        let entry_module = entry_module.into();
        validate_module_path(entry_module.clone())?;
        let mut loader = Loader {
            packages: PackageGraph::discover(&root)?,
            root,
            reader,
            states: HashMap::new(),
            files: HashMap::new(),
            stack: Vec::new(),
            modules: Vec::new(),
        };
        loader.load_module(&entry_module, None)?;
        let mut modules = loader.modules;
        standard.add_imported_modules(&mut modules)?;
        Ok(Self {
            modules,
            examples: Vec::new(),
        })
    }

    /// Every module under `root`, as `chimi test <directory>` reads it: each
    /// `.chimi` file is a module named by its path from `root`, loaded with its
    /// imports. Hidden files and directories, `target` (where builds write),
    /// and symbolic links are skipped. A file whose path is not a valid module
    /// path is an error, as an import of it would be. When `root` is a
    /// package, its modules are named `<package>/<path>`, and directories
    /// holding another package's manifest are skipped.
    pub fn load_directory(
        root: impl AsRef<Path>,
        standard: &StandardTree,
    ) -> Result<Self, ProjectLoadError> {
        Self::load_directory_with(root, &FileSystemReader, standard)
    }

    /// Loads a directory like [`LoadedProject::load_directory`], taking module
    /// text from `reader`. Which modules exist is still read from disk.
    pub fn load_directory_with(
        root: impl AsRef<Path>,
        reader: &dyn SourceReader,
        standard: &StandardTree,
    ) -> Result<Self, ProjectLoadError> {
        let requested_root = root.as_ref();
        let root = requested_root
            .canonicalize()
            .map_err(|source| ProjectLoadError::ReadRoot {
                path: requested_root.to_owned(),
                source,
            })?;
        if !root.is_dir() {
            return Err(ProjectLoadError::RootNotDirectory { path: root });
        }
        let packages = PackageGraph::discover(&root)?;
        let mut module_paths = module_paths_under(&root, |name| {
            !name.starts_with('.') && name != "target"
        })
        .map_err(|(path, source)| ProjectLoadError::ReadDirectory { path, source })?;
        if let Some(graph) = &packages {
            let package = graph.entry().name.to_string();
            module_paths = module_paths
                .into_iter()
                .filter(|path| nested_manifest(&root, &root.join(path)).is_none())
                .map(|path| format!("{package}/{path}"))
                .collect();
        }
        if module_paths.is_empty() {
            return Err(ProjectLoadError::NoModules {
                path: requested_root.to_owned(),
            });
        }
        let mut loader = Loader {
            root,
            packages,
            reader,
            states: HashMap::new(),
            files: HashMap::new(),
            stack: Vec::new(),
            modules: Vec::new(),
        };
        for module_path in &module_paths {
            loader.load_module(module_path, None)?;
        }
        let mut modules = loader.modules;
        standard.add_imported_modules(&mut modules)?;
        Ok(Self {
            modules,
            examples: Vec::new(),
        })
    }

    /// Adds a test module for each example in the doc comments of this
    /// project's own modules (testing.md, "Documentation examples"), with the
    /// standard modules the examples import. Returns each example module's path
    /// and the documented module's, so the runner can report an example under
    /// the module it documents. An example module is the documented module's
    /// path plus `__example_<n>`; its source keeps the documented file's path,
    /// so diagnostics name the doc comment's line.
    pub fn add_doc_examples(
        &mut self,
        standard: &StandardTree,
    ) -> Result<Vec<(String, String)>, ProjectLoadError> {
        self.add_examples_of(standard, false)
    }

    /// Like [`Self::add_doc_examples`], but for the `std/` modules only
    /// (`chimi std test`).
    pub fn add_standard_doc_examples(
        &mut self,
        standard: &StandardTree,
    ) -> Result<Vec<(String, String)>, ProjectLoadError> {
        self.add_examples_of(standard, true)
    }

    fn add_examples_of(
        &mut self,
        standard: &StandardTree,
        standard_modules: bool,
    ) -> Result<Vec<(String, String)>, ProjectLoadError> {
        let mut added = Vec::new();
        let documented: Vec<ProjectSource> = self
            .modules
            .iter()
            .filter(|module| {
                let wanted = if standard_modules {
                    ModuleOrigin::Standard
                } else {
                    ModuleOrigin::Own
                };
                module.origin() == wanted
            })
            .cloned()
            .collect();
        for module in documented {
            for (index, example) in crate::doc_examples::examples(module.source().text())
                .into_iter()
                .enumerate()
            {
                // `std/` paths are reserved for the standard tree (`N3102`),
                // so a standard module's examples live under `example/`.
                let prefix = if standard_modules { "example/" } else { "" };
                let path = format!("{prefix}{}/__example_{}", module.module_path(), index + 1);
                let source = SourceSnapshot::from_text(module.source().path(), example.text);
                let source = ProjectSource::parsed(path.clone(), source)
                    .map_err(ProjectLoadError::InvalidModule)?;
                self.modules.push(source);
                added.push((path, module.module_path().to_owned()));
            }
        }
        standard.add_imported_modules(&mut self.modules)?;
        self.examples.extend(added.iter().cloned());
        Ok(added)
    }

    /// Where the loaded module `module_path` comes from, if it is loaded.
    pub fn origin_of(&self, module_path: &str) -> Option<ModuleOrigin> {
        self.modules
            .iter()
            .find(|module| module.module_path() == module_path)
            .map(ProjectSource::origin)
    }

    pub fn sources(&self) -> Vec<ProjectSource> {
        self.modules.clone()
    }

    pub fn render_diagnostics(&self, diagnostics: &[Diagnostic]) -> String {
        let sources = self.diagnostic_sources();

        let mut output = diagnostics
            .iter()
            .map(|diagnostic| {
                let index = diagnostic.file_id().raw() as usize;
                let source = &sources
                    .get(index)
                    .expect("diagnostic file ID belongs to the loaded project")
                    .1;
                diagnostic.render(source)
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !output.is_empty() {
            output.push('\n');
        }
        output
    }

    /// A lookup of `path:line:column` for an offset in the module with a
    /// file id, the id a diagnostic or a runtime trace event of this project
    /// carries. Each module's line index is built once.
    pub fn source_locator(&self) -> impl Fn(u32, u32) -> Option<String> + use<> {
        use crate::line_index::{LineIndex, PositionEncoding};
        let sources: Vec<(String, Arc<str>, LineIndex)> = self
            .diagnostic_sources()
            .into_iter()
            .map(|(_, source)| {
                let text = Arc::<str>::from(source.text());
                let index = LineIndex::new(Arc::clone(&text));
                (source.path().display().to_string(), text, index)
            })
            .collect();
        move |file_id, offset| {
            let (path, text, index) = sources.get(file_id as usize)?;
            if offset as usize > text.len() || !text.is_char_boundary(offset as usize) {
                return None;
            }
            let position = index.position(offset, PositionEncoding::Utf16);
            Some(format!("{path}:{}:{}", position.line + 1, position.character + 1))
        }
    }

    pub fn render_diagnostics_json(&self, diagnostics: &[Diagnostic]) -> String {
        let sources: Vec<JsonSource> = self
            .diagnostic_sources()
            .into_iter()
            .map(|(module_path, source)| {
                let documented = self
                    .examples
                    .iter()
                    .find(|(example, _)| *example == module_path)
                    .and_then(|(_, documented)| {
                        self.modules
                            .iter()
                            .find(|module| module.module_path() == documented)
                    })
                    .map(|module| {
                        (
                            module.module_path().to_owned(),
                            Arc::<str>::from(module.source().text()),
                        )
                    });
                JsonSource {
                    module_path,
                    source,
                    documented,
                }
            })
            .collect();
        let mut output = String::from("{\"schema\":\"chimi.diagnostics.v1\",\"diagnostics\":[");
        for (index, diagnostic) in diagnostics.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            write_json_diagnostic(&mut output, diagnostic, &sources);
        }
        output.push_str("]}\n");
        output
    }

    /// `(module path, one-based line, one-based UTF-16 column)` of each
    /// diagnostic's primary start, as `render_diagnostics` prints it.
    #[cfg(feature = "native-backend")]
    pub(crate) fn diagnostic_positions(
        &self,
        diagnostics: &[Diagnostic],
    ) -> Vec<(String, u32, u32)> {
        use crate::line_index::{LineIndex, PositionEncoding};
        let sources = self.diagnostic_sources();
        diagnostics
            .iter()
            .map(|diagnostic| {
                let (module_path, source) = &sources[diagnostic.file_id().raw() as usize];
                let position = LineIndex::new(std::sync::Arc::from(source.text()))
                    .position(diagnostic.range().start(), PositionEncoding::Utf16);
                (
                    module_path.clone(),
                    position.line + 1,
                    position.character + 1,
                )
            })
            .collect()
    }

    fn diagnostic_sources(&self) -> Vec<(String, SourceSnapshot)> {
        let mut sources = self
            .modules
            .iter()
            .map(|module| (module.module_path().to_owned(), module.source().clone()))
            .collect::<Vec<_>>();
        sources.sort_by(|left, right| left.0.cmp(&right.0));
        for (index, (_, source)) in sources.iter_mut().enumerate() {
            source.assign_file_id(FileId::new(index as u32));
        }
        sources
    }
}

fn write_json_diagnostic(
    output: &mut String,
    diagnostic: &Diagnostic,
    sources: &[JsonSource],
) {
    output.push_str("{\"stage\":");
    write_json_string(output, &diagnostic.stage().to_string());
    output.push_str(",\"code\":");
    write_json_string(output, diagnostic.code());
    output.push_str(",\"severity\":");
    write_json_string(output, &diagnostic.severity().to_string());
    output.push_str(",\"message\":");
    write_json_string(output, diagnostic.message());
    output.push_str(",\"primary\":");
    write_json_span(output, diagnostic.file_id(), diagnostic.range(), sources);
    output.push_str(",\"labels\":[");
    for (index, label) in diagnostic.labels().iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str("{\"message\":");
        write_json_string(output, label.message());
        output.push_str(",\"span\":");
        write_json_span(output, label.file_id(), label.range(), sources);
        output.push('}');
    }
    output.push_str("],\"fixes\":[");
    for (index, fix) in diagnostic.fixes().iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str("{\"replacement\":");
        write_json_string(output, fix.replacement());
        output.push_str(",\"span\":");
        write_json_span(output, fix.file_id(), fix.range(), sources);
        output.push('}');
    }
    output.push_str("]}");
}

/// A diagnostic source as JSON names it. A doc example module is reported as
/// the module it documents, with offsets into that module's own text.
struct JsonSource {
    module_path: String,
    source: SourceSnapshot,
    documented: Option<(String, Arc<str>)>,
}

fn write_json_span(
    output: &mut String,
    file_id: FileId,
    range: TextRange,
    sources: &[JsonSource],
) {
    let json = sources
        .get(file_id.raw() as usize)
        .expect("diagnostic file ID belongs to the loaded project");
    let (module_path, start, end) = match &json.documented {
        Some((documented, text)) => (
            documented.as_str(),
            same_place(json.source.text(), range.start(), text),
            same_place(json.source.text(), range.end(), text),
        ),
        None => (json.module_path.as_str(), range.start(), range.end()),
    };
    output.push_str("{\"module\":");
    write_json_string(output, module_path);
    output.push_str(",\"path\":");
    write_json_string(output, &json.source.path().to_string_lossy());
    write!(output, ",\"start\":{start},\"end\":{end}}}")
        .expect("writing JSON to a string cannot fail");
}

/// The byte offset in `target` at the line and byte column `offset` has in
/// `text`, clamped to the end of that line. A doc example module keeps its
/// documented file's lines and the columns of its code.
fn same_place(text: &str, offset: u32, target: &str) -> u32 {
    let offset = (offset as usize).min(text.len());
    let line = text[..offset].matches('\n').count();
    let column = offset - text[..offset].rfind('\n').map_or(0, |newline| newline + 1);
    let line_start = match line.checked_sub(1) {
        None => 0,
        Some(previous) => target
            .match_indices('\n')
            .nth(previous)
            .map_or(target.len(), |(newline, _)| newline + 1),
    };
    let line_end = target[line_start..]
        .find('\n')
        .map_or(target.len(), |newline| line_start + newline);
    (line_start + column).min(line_end) as u32
}

/// Supplies module text during project loading, so editors can substitute
/// unsaved buffers for files on disk.
pub trait SourceReader {
    /// Requires: `path` is canonical and inside the project root.
    fn read(&self, path: &Path) -> io::Result<String>;
}

/// Reads module text from the filesystem.
#[derive(Clone, Copy, Debug, Default)]
pub struct FileSystemReader;

impl SourceReader for FileSystemReader {
    fn read(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }
}

#[derive(Debug)]
pub enum ProjectLoadError {
    InvalidModule(ProjectPathError),
    ReadRoot {
        path: PathBuf,
        source: io::Error,
    },
    RootNotDirectory {
        path: PathBuf,
    },
    /// `chimi test <directory>` found no `.chimi` file to load.
    NoModules {
        path: PathBuf,
    },
    ReadDirectory {
        path: PathBuf,
        source: io::Error,
    },
    ReadStandardDirectory {
        path: PathBuf,
        source: io::Error,
    },
    ReadSource {
        module: String,
        path: PathBuf,
        source: io::Error,
    },
    PathEscapesRoot {
        module: String,
        path: PathBuf,
        root: PathBuf,
    },
    DuplicateFile {
        module: String,
        prior_module: String,
        path: PathBuf,
    },
    ImportCycle {
        modules: Vec<String>,
    },
    Package(PackageError),
    /// The entry module of a package build is not one of its own modules.
    NotInEntryPackage {
        module: String,
        package: String,
    },
    /// An import whose first segment names no package of the build.
    UnknownPackage {
        module: String,
        importer: String,
    },
    /// An import of a package in the build that the importer's manifest
    /// does not list.
    NotADependency {
        module: String,
        importer: String,
        manifest: PathBuf,
    },
    /// `import money`: a package is not a module.
    PackageIsNotAModule {
        module: String,
        importer: String,
    },
    InternalModule {
        module: String,
        importer: String,
    },
    /// A module path that reaches into a subdirectory holding another
    /// package's manifest.
    NestedPackage {
        module: String,
        manifest: PathBuf,
    },
}

impl fmt::Display for ProjectLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidModule(error) => error.fmt(formatter),
            Self::ReadRoot { path, source } => {
                write!(
                    formatter,
                    "cannot read project root {}: {source}",
                    path.display()
                )
            }
            Self::RootNotDirectory { path } => {
                write!(
                    formatter,
                    "project root is not a directory: {}",
                    path.display()
                )
            }
            Self::NoModules { path } => {
                write!(formatter, "no .chimi modules under {}", path.display())
            }
            Self::ReadDirectory { path, source } => {
                write!(formatter, "cannot read directory {}: {source}", path.display())
            }
            Self::ReadStandardDirectory { path, source } => write!(
                formatter,
                "cannot read standard tree directory {}: {source}",
                path.display()
            ),
            Self::ReadSource {
                module,
                path,
                source,
            } => write!(
                formatter,
                "cannot read module {module} at {}: {source}",
                path.display()
            ),
            Self::PathEscapesRoot { module, path, root } => write!(
                formatter,
                "module {module} resolves outside project root {}: {}",
                root.display(),
                path.display()
            ),
            Self::DuplicateFile {
                module,
                prior_module,
                path,
            } => write!(
                formatter,
                "modules {prior_module} and {module} resolve to the same file {}",
                path.display()
            ),
            Self::ImportCycle { modules } => {
                write!(formatter, "import cycle: {}", modules.join(" -> "))
            }
            Self::Package(error) => error.fmt(formatter),
            Self::NotInEntryPackage { module, package } => write!(
                formatter,
                "module {module} is not in package {package}: its modules are named \
                 {package}/<path>"
            ),
            Self::UnknownPackage { module, importer } => write!(
                formatter,
                "module {importer} imports {module}, but no package `{}` is in the build",
                first_segment(module)
            ),
            Self::NotADependency {
                module,
                importer,
                manifest,
            } => write!(
                formatter,
                "module {importer} imports {module}, but `{}` is not a dependency of `{}` \
                 ({})",
                first_segment(module),
                first_segment(importer),
                manifest.display()
            ),
            Self::PackageIsNotAModule { module, importer } => write!(
                formatter,
                "module {importer} imports {module}, which is a package, not a module; \
                 name one of its modules, as {module}/<path>"
            ),
            Self::InternalModule { module, importer } => write!(
                formatter,
                "module {importer} imports {module}, but `internal/` modules are private \
                 to package `{}`",
                first_segment(module)
            ),
            Self::NestedPackage { module, manifest } => write!(
                formatter,
                "module {module} is inside another package ({})",
                manifest.display()
            ),
        }
    }
}

impl std::error::Error for ProjectLoadError {}

impl From<PackageError> for ProjectLoadError {
    fn from(error: PackageError) -> Self {
        Self::Package(error)
    }
}

fn first_segment(module_path: &str) -> &str {
    module_path.split('/').next().unwrap_or(module_path)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoadState {
    Loading,
    Loaded,
}

struct Loader<'reader> {
    root: PathBuf,
    /// `None` for a project with no manifest, whose module paths are plain
    /// paths from `root`.
    packages: Option<PackageGraph>,
    reader: &'reader dyn SourceReader,
    states: HashMap<String, LoadState>,
    files: HashMap<PathBuf, String>,
    stack: Vec<String>,
    modules: Vec<ProjectSource>,
}

impl Loader<'_> {
    /// Loads `module_path`, which `importer` imports, or which the command
    /// asked for when `importer` is `None`, and everything it imports.
    fn load_module(
        &mut self,
        module_path: &str,
        importer: Option<&str>,
    ) -> Result<(), ProjectLoadError> {
        validate_module_path(module_path.to_owned())?;
        let target = self.locate(module_path, importer)?;
        match self.states.get(module_path) {
            Some(LoadState::Loaded) => return Ok(()),
            Some(LoadState::Loading) => {
                let cycle_start = self
                    .stack
                    .iter()
                    .position(|module| module == module_path)
                    .expect("a loading module is present in the import stack");
                let mut modules = self.stack[cycle_start..].to_vec();
                modules.push(module_path.to_owned());
                return Err(ProjectLoadError::ImportCycle { modules });
            }
            None => {}
        }

        let Located {
            root,
            relative,
            origin,
        } = target;
        let requested_path = root.join(relative).with_extension("chimi");
        let path =
            requested_path
                .canonicalize()
                .map_err(|source| ProjectLoadError::ReadSource {
                    module: module_path.to_owned(),
                    path: requested_path,
                    source,
                })?;
        if !path.starts_with(&root) {
            return Err(ProjectLoadError::PathEscapesRoot {
                module: module_path.to_owned(),
                path,
                root,
            });
        }
        if self.packages.is_some()
            && let Some(manifest) = nested_manifest(&root, &path)
        {
            return Err(ProjectLoadError::NestedPackage {
                module: module_path.to_owned(),
                manifest,
            });
        }
        if let Some(prior_module) = self.files.get(&path) {
            if prior_module != module_path {
                return Err(ProjectLoadError::DuplicateFile {
                    module: module_path.to_owned(),
                    prior_module: prior_module.clone(),
                    path,
                });
            }
        } else {
            self.files.insert(path.clone(), module_path.to_owned());
        }

        let text = self
            .reader
            .read(&path)
            .map_err(|source| ProjectLoadError::ReadSource {
                module: module_path.to_owned(),
                path: path.clone(),
                source,
            })?;
        let source = SourceSnapshot::from_text(path.clone(), text);
        let module = ProjectSource::parsed(module_path, source)
            .expect("loaded module paths were validated before filesystem access")
            .with_origin(origin);
        let imports = imports(&module);

        self.states
            .insert(module_path.to_owned(), LoadState::Loading);
        self.stack.push(module_path.to_owned());
        // `std/` imports come from the compiler's embedded tree, never from the
        // workspace; `add_imported_modules` adds them once the workspace is loaded.
        for import in imports {
            if !import.starts_with("std/") {
                self.load_module(&import, Some(module_path))?;
            }
        }
        let popped = self.stack.pop();
        debug_assert_eq!(popped.as_deref(), Some(module_path));
        self.states
            .insert(module_path.to_owned(), LoadState::Loaded);
        self.modules.push(module);
        Ok(())
    }
}

/// The directory of the package that holds the file `path`: the nearest
/// directory above it with a `chimi.toml` that is a `[package]`. `None` when
/// the nearest manifest is a workspace's, or there is none. Editors use it to
/// find an open file's package as `--root` names one.
pub fn package_root_of(path: &Path) -> Option<PathBuf> {
    let directory = path
        .ancestors()
        .skip(1)
        .find(|directory| directory.join(crate::manifest::MANIFEST_FILE).is_file())?;
    match crate::manifest::Manifest::read(&directory.join(crate::manifest::MANIFEST_FILE)) {
        Ok(crate::manifest::Manifest::Workspace(_)) => None,
        // An unreadable manifest still marks a package; loading it reports
        // the manifest's error.
        Ok(crate::manifest::Manifest::Package(_)) | Err(_) => Some(directory.to_owned()),
    }
}

/// Where a module's file is: a path relative to a directory it must stay in.
struct Located {
    root: PathBuf,
    relative: PathBuf,
    origin: ModuleOrigin,
}

impl Loader<'_> {
    /// Where `module_path` lives, after checking that `importer` may import
    /// it (packages.md, "Imports across packages"): its first segment names
    /// the importer's own package or one of its dependencies, it names a
    /// module rather than a package, and it is not another package's
    /// `internal/` module.
    fn locate(
        &self,
        module_path: &str,
        importer: Option<&str>,
    ) -> Result<Located, ProjectLoadError> {
        let Some(graph) = &self.packages else {
            return Ok(Located {
                root: self.root.clone(),
                relative: PathBuf::from(module_path),
                origin: ModuleOrigin::Own,
            });
        };
        let (first, rest) = module_path.split_once('/').unwrap_or((module_path, ""));
        let entry = graph.entry();
        let Some(importer) = importer else {
            if first != entry.name.as_str() || rest.is_empty() {
                return Err(ProjectLoadError::NotInEntryPackage {
                    module: module_path.to_owned(),
                    package: entry.name.to_string(),
                });
            }
            return Ok(Located {
                root: entry.root.clone(),
                relative: PathBuf::from(rest),
                origin: ModuleOrigin::Own,
            });
        };
        let importing = graph
            .find_str(first_segment(importer))
            .expect("a loaded module belongs to a package of the build");
        let edge_error = |make: fn(String, String) -> ProjectLoadError| {
            make(module_path.to_owned(), importer.to_owned())
        };
        let target = if first == importing.name.as_str() {
            importing
        } else if importing
            .dependencies
            .iter()
            .any(|dependency| dependency.as_str() == first)
        {
            graph.find_str(first).expect("every dependency is in the graph")
        } else if graph.find_str(first).is_some() {
            return Err(ProjectLoadError::NotADependency {
                module: module_path.to_owned(),
                importer: importer.to_owned(),
                manifest: importing.manifest.clone(),
            });
        } else {
            return Err(edge_error(|module, importer| ProjectLoadError::UnknownPackage {
                module,
                importer,
            }));
        };
        if rest.is_empty() {
            return Err(edge_error(|module, importer| {
                ProjectLoadError::PackageIsNotAModule { module, importer }
            }));
        }
        if target.name != importing.name && rest.starts_with("internal/") {
            return Err(edge_error(|module, importer| ProjectLoadError::InternalModule {
                module,
                importer,
            }));
        }
        Ok(Located {
            root: target.root.clone(),
            relative: PathBuf::from(rest),
            origin: if target.name == entry.name {
                ModuleOrigin::Own
            } else {
                ModuleOrigin::Dependency
            },
        })
    }
}

/// The manifest of a package nested in the one at `root` that holds the
/// file `path`, if any: a `chimi.toml` in a directory between them.
fn nested_manifest(root: &Path, path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .skip(1)
        .take_while(|directory| *directory != root)
        .map(|directory| directory.join(crate::manifest::MANIFEST_FILE))
        .find(|manifest| manifest.is_file())
}

fn imports(module: &ProjectSource) -> Vec<String> {
    semantic::discover_imports(
        module
            .syntax()
            .expect("filesystem modules are parsed before import discovery"),
    )
    .into_iter()
    .filter(|import| !matches!(import.binding, ImportBinding::Missing))
    .map(|import| import.path)
    .filter(|path| ProjectSource::is_valid_path(path))
    .collect()
}

fn validate_module_path(module_path: String) -> Result<(), ProjectLoadError> {
    if ProjectSource::is_valid_path(&module_path) {
        Ok(())
    } else {
        Err(ProjectLoadError::InvalidModule(ProjectPathError::new(
            module_path,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::{LoadedProject, ModuleOrigin, ProjectLoadError, SourceReader, StandardTree};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> Self {
            let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "chimi-project-loader-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("create isolated project loader fixture");
            Self { path }
        }

        fn write(&self, relative: &str, text: &str) {
            let path = self.path.join(relative);
            fs::create_dir_all(path.parent().expect("fixture file has a parent"))
                .expect("create fixture module directory");
            fs::write(path, text).expect("write fixture module");
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.path).expect("remove isolated project loader fixture");
        }
    }

    #[test]
    fn recursively_loads_only_modules_reachable_from_the_entry() {
        let fixture = TestDirectory::new();
        fixture.write(
            "app/main.chimi",
            "import lib/answer as Answer\npub fn main() -> Int { Answer.value() }",
        );
        fixture.write("lib/answer.chimi", "pub fn value() -> Int { 42 }");
        fixture.write("unused.chimi", "pub fn main() -> Int { 0 }");

        let project = LoadedProject::load(fixture.path(), "app/main", &StandardTree::embedded())
            .expect("load project");
        let mut paths = project
            .sources()
            .iter()
            .map(|source| source.module_path().to_owned())
            .collect::<Vec<_>>();
        paths.sort();
        assert_eq!(paths, ["app/main", "lib/answer"]);
    }

    #[test]
    fn missing_imports_name_the_module_and_expected_file() {
        let fixture = TestDirectory::new();
        fixture.write(
            "app.chimi",
            "import missing/module as Missing\npub fn main() -> Int { 0 }",
        );

        let error = LoadedProject::load(fixture.path(), "app", &StandardTree::embedded())
            .expect_err("missing import fails");
        let message = error.to_string();
        assert!(message.contains("cannot read module missing/module"));
        assert!(message.contains("missing/module.chimi"));
    }

    #[test]
    fn incomplete_imports_are_left_for_source_diagnostics() {
        let fixture = TestDirectory::new();
        fixture.write("app.chimi", "import absent\npub fn main() -> Int { 0 }");

        let project = LoadedProject::load(fixture.path(), "app", &StandardTree::embedded())
            .expect("loader does not mask an incomplete import diagnostic");
        assert_eq!(project.sources().len(), 1);
    }

    #[test]
    fn import_cycles_report_the_closed_path() {
        let fixture = TestDirectory::new();
        fixture.write("a.chimi", "import b as B\npub fn main() -> Int { 0 }");
        fixture.write("b.chimi", "import a as A\npub fn value() -> Int { 1 }");

        let error = LoadedProject::load(fixture.path(), "a", &StandardTree::embedded())
            .expect_err("cycle fails");
        assert!(matches!(error, ProjectLoadError::ImportCycle { .. }));
        assert_eq!(error.to_string(), "import cycle: a -> b -> a");
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_escape_the_project_root() {
        use std::os::unix::fs::symlink;

        let fixture = TestDirectory::new();
        fixture.write(
            "project/app.chimi",
            "import escaped as Escaped\npub fn main() -> Int { 0 }",
        );
        fixture.write("outside.chimi", "pub fn value() -> Int { 1 }");
        symlink(
            fixture.path().join("outside.chimi"),
            fixture.path().join("project/escaped.chimi"),
        )
        .expect("create escaping module symlink");

        let error = LoadedProject::load(
            fixture.path().join("project"),
            "app",
            &StandardTree::embedded(),
        )
        .expect_err("escaping symlink fails");
        assert!(matches!(error, ProjectLoadError::PathEscapesRoot { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn two_module_paths_cannot_name_the_same_file() {
        use std::os::unix::fs::symlink;

        let fixture = TestDirectory::new();
        fixture.write(
            "app.chimi",
            concat!(
                "import real as Real\n",
                "import alias as Alias\n",
                "pub fn main() -> Int { 0 }"
            ),
        );
        fixture.write("real.chimi", "pub fn value() -> Int { 1 }");
        symlink(
            fixture.path().join("real.chimi"),
            fixture.path().join("alias.chimi"),
        )
        .expect("create duplicate module symlink");

        let error = LoadedProject::load(fixture.path(), "app", &StandardTree::embedded())
            .expect_err("duplicate physical module fails");
        assert!(matches!(error, ProjectLoadError::DuplicateFile { .. }));
        assert!(error.to_string().contains("real and alias"));
    }

    /// One workspace member: its directory, manifest and `(file, text)` modules.
    type Member<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);

    /// A workspace at the fixture root listing `members`.
    fn workspace(members: &[Member<'_>]) -> TestDirectory {
        let fixture = TestDirectory::new();
        let listed: Vec<String> = members
            .iter()
            .map(|(directory, _, _)| format!("\"{directory}\""))
            .collect();
        fixture.write(
            "chimi.toml",
            &format!("[workspace]\nmembers = [{}]\n", listed.join(", ")),
        );
        for (directory, manifest, files) in members {
            fixture.write(&format!("{directory}/chimi.toml"), manifest);
            for (file, text) in *files {
                fixture.write(&format!("{directory}/{file}"), text);
            }
        }
        fixture
    }

    const APP: &str = "[package]\nname = \"app\"\n[dependencies]\nmoney = { path = \"../money\" }\n";
    const MONEY: &str = "[package]\nname = \"money\"\n";

    fn load_app(fixture: &TestDirectory) -> Result<LoadedProject, ProjectLoadError> {
        LoadedProject::load(fixture.path().join("app"), "app/main", &StandardTree::embedded())
    }

    #[test]
    fn a_package_imports_its_own_modules_and_its_dependencies_by_package_name() {
        let fixture = workspace(&[
            (
                "app",
                APP,
                &[
                    ("main.chimi", "import app/total as Total\npub fn main() -> Int { Total.value() }"),
                    ("total.chimi", "import money/amount as Amount\npub fn value() -> Int { Amount.value() }"),
                ],
            ),
            (
                "money",
                MONEY,
                &[
                    ("amount.chimi", "import money/internal/cents as Cents\npub fn value() -> Int { Cents.value() }"),
                    ("internal/cents.chimi", "pub fn value() -> Int { 42 }"),
                ],
            ),
        ]);
        let project = load_app(&fixture).unwrap();
        let mut modules: Vec<_> = project
            .sources()
            .iter()
            .map(|source| (source.module_path().to_owned(), source.origin()))
            .collect();
        modules.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(
            modules,
            [
                ("app/main".to_owned(), ModuleOrigin::Own),
                ("app/total".to_owned(), ModuleOrigin::Own),
                ("money/amount".to_owned(), ModuleOrigin::Dependency),
                ("money/internal/cents".to_owned(), ModuleOrigin::Dependency),
            ]
        );
    }

    #[test]
    fn imports_that_break_a_package_boundary_fail_naming_it() {
        let rounding = "[package]\nname = \"rounding\"\n";
        let money_with_rounding =
            "[package]\nname = \"money\"\n[dependencies]\nrounding = { path = \"../rounding\" }\n";
        let cases: [(&str, &str, &str); 5] = [
            (
                "import rounding/even as Even\npub fn main() -> Int { 0 }",
                "module app/main imports rounding/even, but `rounding` is not a dependency of `app`",
                "NotADependency",
            ),
            (
                "import nowhere/thing as Thing\npub fn main() -> Int { 0 }",
                "module app/main imports nowhere/thing, but no package `nowhere` is in the build",
                "UnknownPackage",
            ),
            (
                "import money as Money\npub fn main() -> Int { 0 }",
                "module app/main imports money, which is a package, not a module",
                "PackageIsNotAModule",
            ),
            (
                "import money/internal/cents as Cents\npub fn main() -> Int { 0 }",
                "module app/main imports money/internal/cents, but `internal/` modules are private to package `money`",
                "InternalModule",
            ),
            (
                "import main as Main\npub fn main() -> Int { 0 }",
                "module app/main imports main, but no package `main` is in the build",
                "UnknownPackage",
            ),
        ];
        for (main, message, variant) in cases {
            let fixture = workspace(&[
                ("app", APP, &[("main.chimi", main)]),
                (
                    "money",
                    money_with_rounding,
                    &[("internal/cents.chimi", "pub fn value() -> Int { 1 }")],
                ),
                ("rounding", rounding, &[("even.chimi", "pub fn value() -> Int { 1 }")]),
            ]);
            let error = load_app(&fixture).expect_err(main).to_string();
            assert!(error.starts_with(message), "{variant}: {error}");
        }
    }

    #[test]
    fn the_entry_module_of_a_package_is_one_of_its_own() {
        let fixture = workspace(&[("app", APP, &[("main.chimi", "pub fn main() -> Int { 0 }")]), ("money", MONEY, &[])]);
        let error = LoadedProject::load(fixture.path().join("app"), "main", &StandardTree::embedded())
            .expect_err("plain entry path in a package");
        assert!(matches!(error, ProjectLoadError::NotInEntryPackage { .. }), "{error}");
        assert_eq!(
            error.to_string(),
            "module main is not in package app: its modules are named app/<path>"
        );
    }

    #[test]
    fn a_module_path_cannot_reach_into_a_nested_package() {
        let fixture = workspace(&[
            ("app", APP, &[("main.chimi", "import app/inner/x as X\npub fn main() -> Int { 0 }")]),
            ("money", MONEY, &[]),
        ]);
        fixture.write("app/inner/chimi.toml", "[package]\nname = \"inner\"\n");
        fixture.write("app/inner/x.chimi", "pub fn value() -> Int { 1 }");
        let error = load_app(&fixture).expect_err("nested package");
        assert!(matches!(error, ProjectLoadError::NestedPackage { .. }), "{error}");
    }

    #[test]
    fn a_package_directory_loads_its_own_modules_named_by_the_package() {
        let fixture = workspace(&[
            ("app", APP, &[("main.chimi", "import money/amount as Amount\npub fn main() -> Int { Amount.value() }")]),
            ("money", MONEY, &[("amount.chimi", "pub fn value() -> Int { 1 }")]),
        ]);
        fixture.write("app/inner/chimi.toml", "[package]\nname = \"inner\"\n");
        fixture.write("app/inner/x.chimi", "pub fn value() -> Int { 1 }");
        let project =
            LoadedProject::load_directory(fixture.path().join("app"), &StandardTree::embedded())
                .unwrap();
        let mut modules: Vec<_> = project
            .sources()
            .iter()
            .map(|source| source.module_path().to_owned())
            .collect();
        modules.sort();
        assert_eq!(modules, ["app/main", "money/amount"]);
    }

    #[test]
    fn the_package_graph_rejects_bad_dependencies_naming_both_ends() {
        let cases: [(&[(&str, &str)], &str); 7] = [
            (
                &[("app", "[package]\nname = \"app\"\n[dependencies]\nmoney = { path = \"../cash\" }\n"), ("cash", MONEY)],
                "",
            ),
            (
                &[("app", "[package]\nname = \"app\"\n[dependencies]\nmoney = { path = \"../cash\" }\n"), ("cash", "[package]\nname = \"cash\"\n")],
                "dependency `money` names the package `cash`",
            ),
            (
                &[("app", "[package]\nname = \"app\"\n[dependencies]\nmoney = { path = \"../missing\" }\n")],
                "dependency `money` at",
            ),
            (
                &[("app", "[package]\nname = \"app\"\n[dependencies]\nmoney = { git = \"https://example.com/m\", rev = \"v1\" }\n")],
                "git dependency `money`: git dependencies are not supported yet",
            ),
            (
                &[
                    ("app", "[package]\nname = \"app\"\n[dependencies]\nmoney = { path = \"../money\" }\n"),
                    ("money", "[package]\nname = \"money\"\n[dependencies]\napp = { path = \"../app\" }\n"),
                ],
                "package dependency cycle: app -> money -> app",
            ),
            (
                &[
                    ("app", "[package]\nname = \"app\"\n[dependencies]\nmoney = { path = \"../money\" }\nbank = { path = \"../bank\" }\n"),
                    ("money", MONEY),
                    ("bank", "[package]\nname = \"bank\"\n[dependencies]\nmoney = { path = \"../other/money\" }\n"),
                    ("other/money", MONEY),
                ],
                "package `money` has two sources",
            ),
            (
                &[("app", "[package]\nname = \"app\"\n[dependencies]\nmoney = { path = \"../cash\" }\n"), ("cash", "[workspace]\nmembers = []\n")],
                "has no chimi.toml with a [package]",
            ),
        ];
        for (packages, message) in cases {
            let members: Vec<Member<'_>> = packages
                .iter()
                .map(|(directory, manifest)| (*directory, *manifest, &[][..]))
                .collect();
            let fixture = workspace(&members);
            fixture.write("app/main.chimi", "pub fn main() -> Int { 0 }");
            let result = load_app(&fixture);
            if message.is_empty() {
                // `money` from a directory named `cash` is fine: the
                // manifest, not the directory, names a package.
                result.expect("a dependency's directory may have any name");
                continue;
            }
            let error = result.expect_err(message).to_string();
            assert!(error.contains(message), "{message}: {error}");
        }
    }

    #[test]
    fn a_dependency_outside_the_workspace_fails() {
        let fixture = TestDirectory::new();
        fixture.write("shop/chimi.toml", "[workspace]\nmembers = [\"app\"]\n");
        fixture.write("shop/app/chimi.toml", APP.replace("../money", "../../money").as_str());
        fixture.write("shop/app/main.chimi", "pub fn main() -> Int { 0 }");
        fixture.write("money/chimi.toml", MONEY);
        let error = LoadedProject::load(
            fixture.path().join("shop/app"),
            "app/main",
            &StandardTree::embedded(),
        )
        .expect_err("escaping path dependency")
        .to_string();
        assert!(error.contains("is outside the workspace"), "{error}");
    }

    #[test]
    fn a_package_the_workspace_above_does_not_list_fails() {
        let fixture = TestDirectory::new();
        fixture.write("chimi.toml", "[workspace]\nmembers = [\"other\"]\n");
        fixture.write("app/chimi.toml", MONEY.replace("money", "app").as_str());
        fixture.write("app/main.chimi", "pub fn main() -> Int { 0 }");
        let error = load_app(&fixture).expect_err("unlisted member").to_string();
        assert!(error.contains("but not one of its members"), "{error}");
    }

    #[test]
    fn a_workspace_root_is_not_a_package() {
        let fixture = workspace(&[("app", APP, &[])]);
        let error = LoadedProject::load(fixture.path(), "app/main", &StandardTree::embedded())
            .expect_err("workspace root")
            .to_string();
        assert!(error.contains("is a workspace; pass one of its member packages"), "{error}");
    }

    struct Overlay {
        path: PathBuf,
        text: &'static str,
    }

    impl SourceReader for Overlay {
        fn read(&self, path: &Path) -> std::io::Result<String> {
            if path == self.path {
                Ok(self.text.to_owned())
            } else {
                fs::read_to_string(path)
            }
        }
    }

    /// Guards that import discovery follows reader text, not disk text: the
    /// unsaved entry imports a module that the saved entry does not.
    #[test]
    fn load_with_discovers_imports_from_reader_text() {
        let fixture = TestDirectory::new();
        fixture.write("app.chimi", "pub fn main() -> Int { 0 }");
        fixture.write("lib/answer.chimi", "pub fn value() -> Int { 42 }");
        let overlay = Overlay {
            path: fixture.path().join("app.chimi").canonicalize().unwrap(),
            text: "import lib/answer as Answer\npub fn main() -> Int { Answer.value() }",
        };

        let project =
            LoadedProject::load_with(fixture.path(), "app", &overlay, &StandardTree::embedded())
                .unwrap();

        let modules = project
            .sources()
            .iter()
            .map(|module| module.module_path().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(modules, ["lib/answer", "app"]);
        assert_eq!(project.sources()[1].source().text(), overlay.text);
    }
}
