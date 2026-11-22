//! `chimi.toml`: a package manifest (`[package]` and `[dependencies]`) or a
//! workspace manifest (`[workspace]`), as `packages.md` specifies. Any table
//! or key outside that shape is an error naming it, so a typo never drops a
//! dependency silently.

mod lock;
mod toml;

pub(crate) use lock::{LOCK_FILE, LockedGit, Lockfile};

use std::fmt;
use std::path::{Path, PathBuf};
use toml::{Entry, Table, Value};

pub(crate) const MANIFEST_FILE: &str = "chimi.toml";

/// A package name: a lowercase ASCII letter, then lowercase letters, digits
/// and underscores, and never `std`. It is the first segment of the package's
/// module paths, so it is always a valid module path segment.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PackageName(String);

impl PackageName {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let mut characters = text.chars();
        let valid = characters.next().is_some_and(|first| first.is_ascii_lowercase())
            && characters.all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
            })
            && text != "std";
        valid.then(|| Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PackageName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Manifest {
    Package(PackageManifest),
    Workspace(WorkspaceManifest),
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct PackageManifest {
    pub(crate) name: PackageName,
    /// In the order written; names are unique.
    pub(crate) dependencies: Vec<Dependency>,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Dependency {
    pub(crate) name: PackageName,
    pub(crate) source: DependencySource,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum DependencySource {
    /// Relative to the manifest's directory, as written.
    Path(PathBuf),
    Git(GitSource),
}

/// Where a git package comes from, as a manifest writes it and a lockfile
/// records it beside the commit `rev` resolved to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GitSource {
    pub(crate) url: String,
    /// A tag, a branch or a commit.
    pub(crate) rev: String,
    /// The package's directory inside the repository.
    pub(crate) directory: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct WorkspaceManifest {
    /// Member directories relative to the workspace root, as written.
    pub(crate) members: Vec<PathBuf>,
}

/// A manifest that cannot be read, naming the file and, when it has one,
/// the line.
#[derive(Debug)]
pub struct ManifestError {
    path: PathBuf,
    line: Option<usize>,
    message: String,
}

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(formatter, "{}:{line}: {}", self.path.display(), self.message),
            None => write!(formatter, "{}: {}", self.path.display(), self.message),
        }
    }
}

impl std::error::Error for ManifestError {}

impl Manifest {
    /// The manifest at `path`.
    pub(crate) fn read(path: &Path) -> Result<Self, ManifestError> {
        let text = std::fs::read_to_string(path).map_err(|error| ManifestError {
            path: path.to_owned(),
            line: None,
            message: format!("cannot read: {error}"),
        })?;
        Self::parse(&text).map_err(|(line, message)| ManifestError {
            path: path.to_owned(),
            line,
            message,
        })
    }

    fn parse(text: &str) -> Result<Self, (Option<usize>, String)> {
        let document = toml::parse(text).map_err(|error| (Some(error.line), error.message))?;
        if let Some(entry) = document.root.entries.first() {
            return Err(unknown_key(entry, None));
        }
        if let Some(table) = document.tables.iter().find(|table| table.array_element) {
            return Err((Some(table.line), format!("unknown table [[{}]]", table.name)));
        }
        let table = |name: &str| document.tables.iter().find(|table| table.name == name);
        if let Some(unknown) = document
            .tables
            .iter()
            .find(|table| !matches!(table.name.as_str(), "package" | "dependencies" | "workspace"))
        {
            return Err((Some(unknown.line), format!("unknown table [{}]", unknown.name)));
        }
        match (table("package"), table("workspace")) {
            (Some(package), None) => {
                package_manifest(package, table("dependencies")).map(Manifest::Package)
            }
            (None, Some(workspace)) => {
                if let Some(dependencies) = table("dependencies") {
                    return Err((
                        Some(dependencies.line),
                        "a workspace manifest has no [dependencies]; members list their own"
                            .to_owned(),
                    ));
                }
                workspace_manifest(workspace).map(Manifest::Workspace)
            }
            (Some(_), Some(workspace)) => Err((
                Some(workspace.line),
                "a manifest is either a [package] or a [workspace], not both".to_owned(),
            )),
            (None, None) => Err((None, "expected a [package] or a [workspace] table".to_owned())),
        }
    }
}

fn package_manifest(
    package: &Table,
    dependencies: Option<&Table>,
) -> Result<PackageManifest, (Option<usize>, String)> {
    let mut name = None;
    for entry in &package.entries {
        match entry.key.as_str() {
            "name" => {
                let text = string(entry)?;
                name = Some(PackageName::parse(text).ok_or_else(|| {
                    (
                        Some(entry.line),
                        format!(
                            "package name `{text}` must be a lowercase letter followed by \
                             lowercase letters, digits and underscores, and not `std`"
                        ),
                    )
                })?);
            }
            _ => return Err(unknown_key(entry, Some("package"))),
        }
    }
    let name = name.ok_or((Some(package.line), "[package] needs a `name`".to_owned()))?;
    let dependencies = dependencies
        .map(|table| table.entries.iter().map(dependency).collect())
        .transpose()?
        .unwrap_or_default();
    Ok(PackageManifest { name, dependencies })
}

fn dependency(entry: &Entry) -> Result<Dependency, (Option<usize>, String)> {
    let line = Some(entry.line);
    let name = PackageName::parse(&entry.key).ok_or_else(|| {
        (
            line,
            format!("dependency `{}` is not a valid package name", entry.key),
        )
    })?;
    let Value::InlineTable(fields) = &entry.value else {
        return Err((
            line,
            format!(
                "dependency `{name}` must be an inline table, as `{{ path = \"../{name}\" }}`"
            ),
        ));
    };
    let field = |key: &str| {
        fields
            .iter()
            .find(|(field, _)| field == key)
            .map(|(_, value)| value.clone())
    };
    if let Some((unknown, _)) = fields
        .iter()
        .find(|(key, _)| !matches!(key.as_str(), "path" | "git" | "rev" | "directory"))
    {
        return Err((line, format!("unknown key `{unknown}` in dependency `{name}`")));
    }
    let source = match (field("path"), field("git")) {
        (Some(path), None) => {
            if let Some(extra) = ["rev", "directory"].into_iter().find(|key| field(key).is_some()) {
                return Err((
                    line,
                    format!("dependency `{name}` has `path`, so it takes no `{extra}`"),
                ));
            }
            DependencySource::Path(PathBuf::from(path))
        }
        (None, Some(url)) => {
            let rev = field("rev").ok_or_else(|| {
                (
                    line,
                    format!("git dependency `{name}` needs a `rev`: a tag, a branch or a commit"),
                )
            })?;
            // Both are passed to `git` as arguments, which a leading `-`
            // would turn into options.
            if let Some(option) = [&url, &rev].into_iter().find(|text| text.starts_with('-')) {
                return Err((
                    line,
                    format!("git dependency `{name}`: `{option}` must not start with `-`"),
                ));
            }
            DependencySource::Git(GitSource {
                url,
                rev,
                directory: field("directory"),
            })
        }
        (Some(_), Some(_)) => {
            return Err((
                line,
                format!("dependency `{name}` has both `path` and `git`; give one"),
            ));
        }
        (None, None) => {
            return Err((
                line,
                format!("dependency `{name}` needs a `path` or a `git` source"),
            ));
        }
    };
    Ok(Dependency { name, source })
}

fn workspace_manifest(workspace: &Table) -> Result<WorkspaceManifest, (Option<usize>, String)> {
    let mut members = None;
    for entry in &workspace.entries {
        match entry.key.as_str() {
            "members" => {
                let Value::Array(items) = &entry.value else {
                    return Err((
                        Some(entry.line),
                        format!("`members` must be an array of strings, not {}", entry.value.kind()),
                    ));
                };
                members = Some(items.iter().map(PathBuf::from).collect());
            }
            _ => return Err(unknown_key(entry, Some("workspace"))),
        }
    }
    let members =
        members.ok_or((Some(workspace.line), "[workspace] needs `members`".to_owned()))?;
    Ok(WorkspaceManifest { members })
}

fn string(entry: &Entry) -> Result<&str, (Option<usize>, String)> {
    match &entry.value {
        Value::String(text) => Ok(text),
        other => Err((
            Some(entry.line),
            format!("`{}` must be a string, not {}", entry.key, other.kind()),
        )),
    }
}

fn unknown_key(entry: &Entry, table: Option<&str>) -> (Option<usize>, String) {
    let place = table.map_or_else(|| "outside any table".to_owned(), |table| format!("in [{table}]"));
    (Some(entry.line), format!("unknown key `{}` {place}", entry.key))
}

#[cfg(test)]
mod tests {
    use super::{DependencySource, GitSource, Manifest, PackageName};
    use std::path::PathBuf;

    #[test]
    fn reads_a_package_with_path_and_git_dependencies() {
        let manifest = Manifest::parse(concat!(
            "[package]\nname = \"shop_core\"\n\n",
            "[dependencies]\n",
            "money = { path = \"../money\" }\n",
            "json = { git = \"https://example.com/json\", rev = \"v1\", directory = \"pkg\" }\n",
        ))
        .unwrap();
        let Manifest::Package(package) = manifest else {
            panic!("a package manifest")
        };
        assert_eq!(package.name.as_str(), "shop_core");
        assert_eq!(package.dependencies[0].name.as_str(), "money");
        assert_eq!(
            package.dependencies[0].source,
            DependencySource::Path(PathBuf::from("../money"))
        );
        assert_eq!(
            package.dependencies[1].source,
            DependencySource::Git(GitSource {
                url: "https://example.com/json".into(),
                rev: "v1".into(),
                directory: Some("pkg".into()),
            })
        );
    }

    #[test]
    fn reads_a_workspace() {
        let manifest = Manifest::parse("[workspace]\nmembers = [\"app\", \"lib/money\"]\n").unwrap();
        assert_eq!(
            manifest,
            Manifest::Workspace(super::WorkspaceManifest {
                members: vec![PathBuf::from("app"), PathBuf::from("lib/money")],
            })
        );
    }

    #[test]
    fn rejects_anything_outside_the_shape_naming_it() {
        let cases = [
            ("[package]\nnmae = \"a\"\n", "2: unknown key `nmae` in [package]"),
            ("[package]\nname = \"a\"\n[dependency]\n", "3: unknown table [dependency]"),
            ("[package]\nname = \"Shop\"\n", "2: package name `Shop` must be"),
            ("[package]\nname = \"std\"\n", "2: package name `std` must be"),
            ("[package]\nname = \"9a\"\n", "2: package name `9a` must be"),
            ("[package]\n", "1: [package] needs a `name`"),
            ("", "expected a [package] or a [workspace] table"),
            ("name = \"a\"\n", "1: unknown key `name` outside any table"),
            (
                "[package]\nname = \"a\"\n[dependencies]\nb = \"../b\"\n",
                "4: dependency `b` must be an inline table",
            ),
            (
                "[package]\nname = \"a\"\n[dependencies]\nb = { paht = \"../b\" }\n",
                "4: unknown key `paht` in dependency `b`",
            ),
            (
                "[package]\nname = \"a\"\n[dependencies]\nb = { git = \"u\" }\n",
                "4: git dependency `b` needs a `rev`",
            ),
            (
                "[package]\nname = \"a\"\n[dependencies]\nb = { git = \"u\", rev = \"--upload-pack=x\" }\n",
                "4: git dependency `b`: `--upload-pack=x` must not start with `-`",
            ),
            (
                "[package]\nname = \"a\"\n[dependencies]\nb = { path = \"p\", git = \"u\" }\n",
                "4: dependency `b` has both `path` and `git`",
            ),
            (
                "[package]\nname = \"a\"\n[dependencies]\nb = { path = \"p\", rev = \"r\" }\n",
                "4: dependency `b` has `path`, so it takes no `rev`",
            ),
            (
                "[package]\nname = \"a\"\n[dependencies]\nB = { path = \"p\" }\n",
                "4: dependency `B` is not a valid package name",
            ),
            (
                "[package]\nname = \"a\"\n[workspace]\nmembers = []\n",
                "3: a manifest is either a [package] or a [workspace]",
            ),
            ("[workspace]\nmembers = \"app\"\n", "2: `members` must be an array of strings"),
            ("[workspace]\n", "1: [workspace] needs `members`"),
            (
                "[workspace]\nmembers = []\n[dependencies]\n",
                "3: a workspace manifest has no [dependencies]",
            ),
            ("[[package]]\nname = \"a\"\n", "1: unknown table [[package]]"),
        ];
        for (text, expected) in cases {
            let (line, message) = Manifest::parse(text).expect_err(text);
            let rendered = match line {
                Some(line) => format!("{line}: {message}"),
                None => message,
            };
            assert!(rendered.starts_with(expected), "{text:?}: {rendered}");
        }
    }

    #[test]
    fn package_names_are_lowercase_identifiers() {
        for valid in ["a", "shop_core", "http2"] {
            assert!(PackageName::parse(valid).is_some(), "{valid}");
        }
        for invalid in ["", "_a", "A", "shop-core", "std", "a/b", "é"] {
            assert!(PackageName::parse(invalid).is_none(), "{invalid}");
        }
    }
}
