use crate::compiler::{ProjectPathError, ProjectSource};
use crate::diagnostic::Diagnostic;
use crate::ids::{FileId, TextRange};
use crate::semantic::{self, ImportBinding, ModuleSource};
use crate::source::SourceSnapshot;
use std::collections::HashMap;
use std::fmt::{self, Write};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct LoadedProject {
    modules: Vec<ProjectSource>,
}

impl LoadedProject {
    pub fn single_file(path: impl AsRef<Path>) -> Result<Self, ProjectLoadError> {
        let path = path.as_ref();
        let source = SourceSnapshot::read(path).map_err(|source| ProjectLoadError::ReadSource {
            module: "main".to_owned(),
            path: path.to_owned(),
            source,
        })?;
        let module = ProjectSource::new("main", source)
            .expect("the reserved single-file module path is valid");
        Ok(Self {
            modules: vec![module],
        })
    }

    pub fn load(
        root: impl AsRef<Path>,
        entry_module: impl Into<String>,
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
            root,
            states: HashMap::new(),
            files: HashMap::new(),
            stack: Vec::new(),
            modules: Vec::new(),
        };
        loader.load_module(&entry_module)?;
        Ok(Self {
            modules: loader.modules,
        })
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

    pub fn render_diagnostics_json(&self, diagnostics: &[Diagnostic]) -> String {
        let sources = self.diagnostic_sources();
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
    sources: &[(String, SourceSnapshot)],
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

fn write_json_span(
    output: &mut String,
    file_id: FileId,
    range: TextRange,
    sources: &[(String, SourceSnapshot)],
) {
    let (module_path, source) = sources
        .get(file_id.raw() as usize)
        .expect("diagnostic file ID belongs to the loaded project");
    output.push_str("{\"module\":");
    write_json_string(output, module_path);
    output.push_str(",\"path\":");
    write_json_string(output, &source.path().to_string_lossy());
    write!(
        output,
        ",\"start\":{},\"end\":{}}}",
        range.start(),
        range.end()
    )
    .expect("writing JSON to a string cannot fail");
}

fn write_json_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                write!(output, "\\u{:04x}", character as u32)
                    .expect("writing JSON to a string cannot fail");
            }
            character => output.push(character),
        }
    }
    output.push('"');
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
        }
    }
}

impl std::error::Error for ProjectLoadError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoadState {
    Loading,
    Loaded,
}

struct Loader {
    root: PathBuf,
    states: HashMap<String, LoadState>,
    files: HashMap<PathBuf, String>,
    stack: Vec<String>,
    modules: Vec<ProjectSource>,
}

impl Loader {
    fn load_module(&mut self, module_path: &str) -> Result<(), ProjectLoadError> {
        validate_module_path(module_path.to_owned())?;
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

        let requested_path = self.root.join(module_path).with_extension("chimi");
        let path =
            requested_path
                .canonicalize()
                .map_err(|source| ProjectLoadError::ReadSource {
                    module: module_path.to_owned(),
                    path: requested_path,
                    source,
                })?;
        if !path.starts_with(&self.root) {
            return Err(ProjectLoadError::PathEscapesRoot {
                module: module_path.to_owned(),
                path,
                root: self.root.clone(),
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

        let source =
            SourceSnapshot::read(&path).map_err(|source| ProjectLoadError::ReadSource {
                module: module_path.to_owned(),
                path: path.clone(),
                source,
            })?;
        let imports = imports(module_path, source.clone());

        self.states
            .insert(module_path.to_owned(), LoadState::Loading);
        self.stack.push(module_path.to_owned());
        for import in imports {
            self.load_module(&import)?;
        }
        let popped = self.stack.pop();
        debug_assert_eq!(popped.as_deref(), Some(module_path));
        self.states
            .insert(module_path.to_owned(), LoadState::Loaded);
        self.modules.push(
            ProjectSource::new(module_path, source)
                .expect("loaded module paths were validated before filesystem access"),
        );
        Ok(())
    }
}

fn imports(module_path: &str, source: SourceSnapshot) -> Vec<String> {
    let mut project = semantic::lower_project(vec![ModuleSource {
        path: module_path.to_owned(),
        source,
    }]);
    project
        .modules
        .pop()
        .expect("one source lowers to one semantic module")
        .imports
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
    use super::{LoadedProject, ProjectLoadError, write_json_string};
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
    fn structured_diagnostic_strings_escape_json_control_characters() {
        let mut output = String::new();
        write_json_string(&mut output, "quote\" slash\\ line\n\t\u{1}");

        assert_eq!(output, "\"quote\\\" slash\\\\ line\\n\\t\\u0001\"");
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

        let project = LoadedProject::load(fixture.path(), "app/main").expect("load project");
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

        let error = LoadedProject::load(fixture.path(), "app").expect_err("missing import fails");
        let message = error.to_string();
        assert!(message.contains("cannot read module missing/module"));
        assert!(message.contains("missing/module.chimi"));
    }

    #[test]
    fn incomplete_imports_are_left_for_source_diagnostics() {
        let fixture = TestDirectory::new();
        fixture.write("app.chimi", "import absent\npub fn main() -> Int { 0 }");

        let project = LoadedProject::load(fixture.path(), "app")
            .expect("loader does not mask an incomplete import diagnostic");
        assert_eq!(project.sources().len(), 1);
    }

    #[test]
    fn import_cycles_report_the_closed_path() {
        let fixture = TestDirectory::new();
        fixture.write("a.chimi", "import b as B\npub fn main() -> Int { 0 }");
        fixture.write("b.chimi", "import a as A\npub fn value() -> Int { 1 }");

        let error = LoadedProject::load(fixture.path(), "a").expect_err("cycle fails");
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

        let error = LoadedProject::load(fixture.path().join("project"), "app")
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

        let error = LoadedProject::load(fixture.path(), "app")
            .expect_err("duplicate physical module fails");
        assert!(matches!(error, ProjectLoadError::DuplicateFile { .. }));
        assert!(error.to_string().contains("real and alias"));
    }
}
