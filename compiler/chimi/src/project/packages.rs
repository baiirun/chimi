//! The packages of a build (packages.md): the entry package and every package
//! it depends on, directly or not, each name mapped to one directory. The
//! graph is built from manifests alone, before any module loads, so a
//! conflict, a cycle or an escaping path fails before anything is parsed.

use crate::manifest::{DependencySource, MANIFEST_FILE, Manifest, PackageName};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) struct PackageGraph {
    /// The entry package first.
    packages: Vec<Package>,
}

#[derive(Debug)]
pub(crate) struct Package {
    pub(crate) name: PackageName,
    /// Canonical.
    pub(crate) root: PathBuf,
    pub(crate) manifest: PathBuf,
    /// The manifest whose dependency first brought this package into the
    /// build; the entry package's own manifest for the entry.
    declared_by: PathBuf,
    pub(crate) dependencies: Vec<PackageName>,
}

/// Why the packages of a build cannot be resolved.
#[derive(Debug)]
pub enum PackageError {
    Manifest(crate::manifest::ManifestError),
    /// `--root` names a workspace, whose members are the packages.
    RootIsWorkspace { manifest: PathBuf },
    DependencyMissing {
        manifest: PathBuf,
        name: PackageName,
        directory: PathBuf,
        source: std::io::Error,
    },
    NotAPackage {
        manifest: PathBuf,
        name: PackageName,
        directory: PathBuf,
    },
    NameMismatch {
        manifest: PathBuf,
        name: PackageName,
        found: PackageName,
        dependency_manifest: PathBuf,
    },
    OutsideWorkspace {
        manifest: PathBuf,
        name: PackageName,
        directory: PathBuf,
        workspace: PathBuf,
    },
    /// One package name with two sources.
    Conflict {
        name: PackageName,
        first: (PathBuf, PathBuf),
        second: (PathBuf, PathBuf),
    },
    Cycle { packages: Vec<PackageName> },
    /// Git dependencies arrive with the lockfile (packages.md, PR 4).
    GitNotSupported { manifest: PathBuf, name: PackageName },
    /// A package under a workspace directory that the workspace does not list.
    UnlistedMember { package: PathBuf, workspace: PathBuf },
}

impl fmt::Display for PackageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest(error) => error.fmt(formatter),
            Self::RootIsWorkspace { manifest } => write!(
                formatter,
                "{} is a workspace; pass one of its member packages with --root",
                manifest.display()
            ),
            Self::DependencyMissing {
                manifest,
                name,
                directory,
                source,
            } => write!(
                formatter,
                "{}: dependency `{name}` at {}: {source}",
                manifest.display(),
                directory.display()
            ),
            Self::NotAPackage {
                manifest,
                name,
                directory,
            } => write!(
                formatter,
                "{}: dependency `{name}` at {} has no {MANIFEST_FILE} with a [package]",
                manifest.display(),
                directory.display()
            ),
            Self::NameMismatch {
                manifest,
                name,
                found,
                dependency_manifest,
            } => write!(
                formatter,
                "{}: dependency `{name}` names the package `{found}` ({})",
                manifest.display(),
                dependency_manifest.display()
            ),
            Self::OutsideWorkspace {
                manifest,
                name,
                directory,
                workspace,
            } => write!(
                formatter,
                "{}: dependency `{name}` at {} is outside the workspace {}",
                manifest.display(),
                directory.display(),
                workspace.display()
            ),
            Self::Conflict {
                name,
                first,
                second,
            } => write!(
                formatter,
                "package `{name}` has two sources: {} (from {}) and {} (from {})",
                first.1.display(),
                first.0.display(),
                second.1.display(),
                second.0.display()
            ),
            Self::Cycle { packages } => {
                let names: Vec<&str> = packages.iter().map(PackageName::as_str).collect();
                write!(formatter, "package dependency cycle: {}", names.join(" -> "))
            }
            Self::GitNotSupported { manifest, name } => write!(
                formatter,
                "{}: git dependency `{name}`: git dependencies are not supported yet",
                manifest.display()
            ),
            Self::UnlistedMember { package, workspace } => write!(
                formatter,
                "package {} is inside the workspace {} but not one of its members",
                package.display(),
                workspace.display()
            ),
        }
    }
}

impl std::error::Error for PackageError {}

impl PackageGraph {
    /// The packages of a build whose entry is the directory `root`
    /// (canonical), or `None` when `root` has no manifest and is a plain
    /// project.
    pub(crate) fn discover(root: &Path) -> Result<Option<Self>, PackageError> {
        let manifest_path = root.join(MANIFEST_FILE);
        if !manifest_path.is_file() {
            return Ok(None);
        }
        let entry = match Manifest::read(&manifest_path).map_err(PackageError::Manifest)? {
            Manifest::Package(package) => package,
            Manifest::Workspace(_) => {
                return Err(PackageError::RootIsWorkspace {
                    manifest: manifest_path,
                });
            }
        };
        let workspace = workspace_root(root)?;
        let mut graph = Self {
            packages: vec![Package {
                name: entry.name,
                root: root.to_owned(),
                declared_by: manifest_path.clone(),
                manifest: manifest_path,
                dependencies: Vec::new(),
            }],
        };
        let mut pending = vec![(0, entry.dependencies)];
        while let Some((index, dependencies)) = pending.pop() {
            for dependency in dependencies {
                let importer = &graph.packages[index];
                let DependencySource::Path(relative) = &dependency.source else {
                    return Err(PackageError::GitNotSupported {
                        manifest: importer.manifest.clone(),
                        name: dependency.name,
                    });
                };
                let requested = importer.root.join(relative);
                let directory =
                    requested
                        .canonicalize()
                        .map_err(|source| PackageError::DependencyMissing {
                            manifest: importer.manifest.clone(),
                            name: dependency.name.clone(),
                            directory: requested.clone(),
                            source,
                        })?;
                if !directory.starts_with(&workspace) {
                    return Err(PackageError::OutsideWorkspace {
                        manifest: importer.manifest.clone(),
                        name: dependency.name,
                        directory,
                        workspace,
                    });
                }
                graph.packages[index]
                    .dependencies
                    .push(dependency.name.clone());
                if let Some(known) = graph.find(&dependency.name) {
                    if known.root != directory {
                        return Err(PackageError::Conflict {
                            name: dependency.name,
                            first: (known.declared_by.clone(), known.root.clone()),
                            second: (graph.packages[index].manifest.clone(), directory),
                        });
                    }
                    continue;
                }
                let importer_manifest = graph.packages[index].manifest.clone();
                let manifest_path = directory.join(MANIFEST_FILE);
                let package = match manifest_path
                    .is_file()
                    .then(|| Manifest::read(&manifest_path))
                    .transpose()
                    .map_err(PackageError::Manifest)?
                {
                    Some(Manifest::Package(package)) => package,
                    _ => {
                        return Err(PackageError::NotAPackage {
                            manifest: importer_manifest,
                            name: dependency.name,
                            directory,
                        });
                    }
                };
                if package.name != dependency.name {
                    return Err(PackageError::NameMismatch {
                        manifest: importer_manifest,
                        name: dependency.name,
                        found: package.name,
                        dependency_manifest: manifest_path,
                    });
                }
                graph.packages.push(Package {
                    name: package.name,
                    root: directory,
                    manifest: manifest_path,
                    declared_by: importer_manifest,
                    dependencies: Vec::new(),
                });
                pending.push((graph.packages.len() - 1, package.dependencies));
            }
        }
        graph.reject_cycles()?;
        Ok(Some(graph))
    }

    pub(crate) fn entry(&self) -> &Package {
        &self.packages[0]
    }

    pub(crate) fn find(&self, name: &PackageName) -> Option<&Package> {
        self.find_str(name.as_str())
    }

    pub(crate) fn find_str(&self, name: &str) -> Option<&Package> {
        self.packages.iter().find(|package| package.name.as_str() == name)
    }

    fn reject_cycles(&self) -> Result<(), PackageError> {
        #[derive(Clone, Copy, PartialEq)]
        enum Visit {
            Active,
            Done,
        }
        fn visit<'graph>(
            graph: &'graph PackageGraph,
            package: &'graph Package,
            states: &mut HashMap<&'graph PackageName, Visit>,
            stack: &mut Vec<PackageName>,
        ) -> Result<(), PackageError> {
            match states.get(&package.name) {
                Some(Visit::Done) => return Ok(()),
                Some(Visit::Active) => {
                    let start = stack
                        .iter()
                        .position(|name| *name == package.name)
                        .expect("an active package is on the stack");
                    let mut packages = stack[start..].to_vec();
                    packages.push(package.name.clone());
                    return Err(PackageError::Cycle { packages });
                }
                None => {}
            }
            states.insert(&package.name, Visit::Active);
            stack.push(package.name.clone());
            for dependency in &package.dependencies {
                let next = graph.find(dependency).expect("every dependency is in the graph");
                visit(graph, next, states, stack)?;
            }
            stack.pop();
            states.insert(&package.name, Visit::Done);
            Ok(())
        }
        visit(self, self.entry(), &mut HashMap::new(), &mut Vec::new())
    }
}

/// The root of the workspace `package` belongs to: the nearest directory
/// above it whose manifest is a `[workspace]` listing it, or `package` itself
/// when no workspace manifest is above it. A workspace above it that does not
/// list it is an error, so it never gets a lockfile of its own by accident.
fn workspace_root(package: &Path) -> Result<PathBuf, PackageError> {
    for directory in package.ancestors().skip(1) {
        let manifest_path = directory.join(MANIFEST_FILE);
        if !manifest_path.is_file() {
            continue;
        }
        let Manifest::Workspace(workspace) =
            Manifest::read(&manifest_path).map_err(PackageError::Manifest)?
        else {
            continue;
        };
        let listed = workspace.members.iter().any(|member| {
            directory
                .join(member)
                .canonicalize()
                .is_ok_and(|member| member == package)
        });
        if !listed {
            return Err(PackageError::UnlistedMember {
                package: package.to_owned(),
                workspace: manifest_path,
            });
        }
        return Ok(directory.to_owned());
    }
    Ok(package.to_owned())
}
