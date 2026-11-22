//! The packages of a build (packages.md): the entry package and every package
//! it depends on, directly or not, each name mapped to one directory. The
//! graph is built from manifests alone, before any module loads, so a
//! conflict, a cycle or an escaping path fails before anything is parsed.

use super::git::{GitCache, GitError};
use crate::manifest::{
    DependencySource, GitSource, LOCK_FILE, LockedGit, Lockfile, MANIFEST_FILE, Manifest,
    PackageManifest, PackageName,
};
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
    /// build; its own manifest for an own package.
    declared_by: PathBuf,
    /// The package the command names, or a member of the workspace it
    /// names: its modules are [`ModuleOrigin::Own`](crate::compiler::ModuleOrigin).
    pub(crate) own: bool,
    pub(crate) dependencies: Vec<PackageName>,
    /// The git source and commit of a fetched package; `None` for a
    /// package of the workspace, or a path package inside a fetched one.
    pub(crate) git: Option<LockedGit>,
    /// The directory its path dependencies must stay inside.
    boundary: Boundary,
}

#[derive(Clone, Debug)]
enum Boundary {
    Workspace(PathBuf),
    /// The checkout of a fetched repository.
    Repository(PathBuf),
}

impl Boundary {
    fn directory(&self) -> &Path {
        match self {
            Self::Workspace(directory) | Self::Repository(directory) => directory,
        }
    }
}

/// Where each git package's commit comes from.
pub(crate) enum Pins<'cache> {
    /// A build: from the lockfile, which must match every manifest.
    Locked {
        lock: Lockfile,
        cache: &'cache GitCache,
    },
    /// `chimi deps update`: kept from `previous` where the manifest still
    /// matches it, unless the package is in `moving`, and resolved from the
    /// repository otherwise; every pin the build uses ends up in `next`.
    Updating {
        previous: Lockfile,
        moving: Vec<String>,
        next: Lockfile,
        cache: &'cache GitCache,
    },
}

impl Pins<'_> {
    fn cache(&self) -> &GitCache {
        match self {
            Self::Locked { cache, .. } | Self::Updating { cache, .. } => cache,
        }
    }

    fn commit(
        &mut self,
        name: &PackageName,
        source: &GitSource,
        manifest: &Path,
    ) -> Result<String, PackageError> {
        match self {
            Self::Locked { lock, .. } => lock
                .entries
                .get(name)
                .filter(|locked| locked.source == *source)
                .map(|locked| locked.commit.clone())
                .ok_or_else(|| PackageError::LockOutOfDate {
                    manifest: manifest.to_owned(),
                    name: name.clone(),
                }),
            Self::Updating {
                previous,
                moving,
                next,
                cache,
            } => {
                let kept = previous
                    .entries
                    .get(name)
                    .filter(|locked| {
                        locked.source == *source
                            && !moving.iter().any(|moved| moved == name.as_str())
                    })
                    .map(|locked| locked.commit.clone());
                let commit = match kept {
                    Some(commit) => commit,
                    None => cache.resolve(&source.url, &source.rev).map_err(|error| {
                        PackageError::Git {
                            manifest: manifest.to_owned(),
                            name: name.clone(),
                            error,
                        }
                    })?,
                };
                next.entries.insert(
                    name.clone(),
                    LockedGit {
                        source: source.clone(),
                        commit: commit.clone(),
                    },
                );
                Ok(commit)
            }
        }
    }
}

/// Why the packages of a build cannot be resolved.
#[derive(Debug)]
pub enum PackageError {
    Manifest(crate::manifest::ManifestError),
    /// `--root` names a workspace, whose members are the packages.
    RootIsWorkspace {
        manifest: PathBuf,
    },
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
    /// A fetched package's path dependency, or its `directory`, leaves the
    /// repository it came from.
    OutsideRepository {
        manifest: PathBuf,
        name: PackageName,
        directory: PathBuf,
        repository: PathBuf,
    },
    /// One package name with two sources: each the manifest that names it
    /// and the source, as written for a person.
    Conflict {
        name: PackageName,
        first: (PathBuf, String),
        second: (PathBuf, String),
    },
    Cycle {
        packages: Vec<PackageName>,
    },
    Lock(crate::manifest::ManifestError),
    /// A git dependency with no lock entry, or one for another source.
    LockOutOfDate {
        manifest: PathBuf,
        name: PackageName,
    },
    Git {
        manifest: PathBuf,
        name: PackageName,
        error: GitError,
    },
    /// `chimi deps update <name>` names a package no member names as a git
    /// dependency and the lockfile does not pin, checked before fetching. A
    /// new git dependency of a dependency is pinned by an update without
    /// names first.
    NotNamedOrPinned {
        name: String,
    },
    /// `chimi deps update <name>` names no git package of the workspace.
    NotAGitDependency {
        name: String,
    },
    /// `chimi deps update` outside any package or workspace.
    NoManifest {
        directory: PathBuf,
    },
    /// A package under a workspace directory that the workspace does not list.
    UnlistedMember {
        package: PathBuf,
        workspace: PathBuf,
    },
    MemberMissing {
        workspace: PathBuf,
        member: PathBuf,
        source: std::io::Error,
    },
    MemberNotAPackage {
        workspace: PathBuf,
        member: PathBuf,
    },
    MemberOutsideWorkspace {
        workspace: PathBuf,
        member: PathBuf,
    },
    MemberListedTwice {
        workspace: PathBuf,
        member: PathBuf,
    },
    DuplicateMember {
        name: PackageName,
        first: PathBuf,
        second: PathBuf,
    },
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
            Self::OutsideRepository {
                manifest,
                name,
                directory,
                repository,
            } => write!(
                formatter,
                "{}: dependency `{name}` at {} is outside its repository's checkout {}",
                manifest.display(),
                directory.display(),
                repository.display()
            ),
            Self::Conflict {
                name,
                first,
                second,
            } => write!(
                formatter,
                "package `{name}` has two sources: {} (from {}) and {} (from {})",
                first.1,
                first.0.display(),
                second.1,
                second.0.display()
            ),
            Self::Lock(error) => error.fmt(formatter),
            Self::LockOutOfDate { manifest, name } => write!(
                formatter,
                "{}: git dependency `{name}`: {LOCK_FILE} is out of date; run `chimi deps update`",
                manifest.display()
            ),
            Self::Git {
                manifest,
                name,
                error,
            } => write!(
                formatter,
                "{}: git dependency `{name}`: {error}",
                manifest.display()
            ),
            Self::NoManifest { directory } => {
                write!(formatter, "{} has no {MANIFEST_FILE}", directory.display())
            }
            Self::NotNamedOrPinned { name } => write!(
                formatter,
                "`{name}` is not a git dependency of the workspace: no member names it and \
                 {LOCK_FILE} does not pin it (a dependency's new git dependency is pinned by \
                 `chimi deps update` without names)"
            ),
            Self::NotAGitDependency { name } => {
                write!(
                    formatter,
                    "`{name}` is not a git dependency of the workspace"
                )
            }
            Self::Cycle { packages } => {
                let names: Vec<&str> = packages.iter().map(PackageName::as_str).collect();
                write!(
                    formatter,
                    "package dependency cycle: {}",
                    names.join(" -> ")
                )
            }
            Self::MemberMissing {
                workspace,
                member,
                source,
            } => write!(
                formatter,
                "{}: member {}: {source}",
                workspace.display(),
                member.display()
            ),
            Self::MemberNotAPackage { workspace, member } => write!(
                formatter,
                "{}: member {} has no {MANIFEST_FILE} with a [package]",
                workspace.display(),
                member.display()
            ),
            Self::MemberOutsideWorkspace { workspace, member } => write!(
                formatter,
                "{}: member {} is outside the workspace directory",
                workspace.display(),
                member.display()
            ),
            Self::MemberListedTwice { workspace, member } => write!(
                formatter,
                "{}: member {} is listed twice",
                workspace.display(),
                member.display()
            ),
            Self::DuplicateMember {
                name,
                first,
                second,
            } => write!(
                formatter,
                "workspace members {} and {} are both named `{name}`",
                first.display(),
                second.display()
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

/// The lockfile `chimi deps update` would write, and the one it replaces.
pub(crate) struct LockUpdate {
    pub(crate) path: PathBuf,
    pub(crate) previous: Lockfile,
    pub(crate) next: Lockfile,
}

struct OwnPackages {
    /// Each package's directory, manifest path and manifest.
    packages: Vec<(PathBuf, PathBuf, PackageManifest)>,
    workspace: PathBuf,
    is_workspace: bool,
}

/// What a build reads from `root`.
pub(crate) enum Discovered {
    /// No manifest: a plain project.
    Plain,
    Package(PackageGraph),
    /// A workspace manifest, with every member's packages.
    Workspace(PackageGraph),
}

impl PackageGraph {
    /// The packages of a build rooted at the directory `root` (canonical):
    /// the package there and its dependencies, a workspace's members and
    /// theirs, or nothing for a plain project. A workspace is an error unless
    /// `workspace_allowed`, as for `--root`, which names one package. Git
    /// packages come at the commits `chimi.lock` pins, from `cache`.
    pub(crate) fn discover(
        root: &Path,
        workspace_allowed: bool,
        cache: &GitCache,
    ) -> Result<Discovered, PackageError> {
        let Some(found) = Self::own_packages(root, workspace_allowed)? else {
            return Ok(Discovered::Plain);
        };
        let lock = Lockfile::read(&found.workspace.join(LOCK_FILE)).map_err(PackageError::Lock)?;
        let graph = Self::resolve(
            found.packages,
            &found.workspace,
            &mut Pins::Locked { lock, cache },
        )?;
        Ok(if found.is_workspace {
            Discovered::Workspace(graph)
        } else {
            Discovered::Package(graph)
        })
    }

    /// `chimi deps update` from the directory `root` (canonical): resolves
    /// every member of its workspace, keeping each pin whose source still
    /// matches unless its package is in `moving`, and returns the lockfile
    /// before and after. Nothing is written here.
    pub(crate) fn update(
        root: &Path,
        moving: Vec<String>,
        cache: &GitCache,
    ) -> Result<LockUpdate, PackageError> {
        let manifest_path = root.join(MANIFEST_FILE);
        if !manifest_path.is_file() {
            return Err(PackageError::NoManifest {
                directory: root.to_owned(),
            });
        }
        let workspace = match Manifest::read(&manifest_path).map_err(PackageError::Manifest)? {
            Manifest::Package(_) => workspace_root(root)?,
            Manifest::Workspace(_) => root.to_owned(),
        };
        let found =
            Self::own_packages(&workspace, true)?.expect("the workspace root holds a manifest");
        let path = workspace.join(LOCK_FILE);
        let previous = Lockfile::read(&path).map_err(PackageError::Lock)?;
        // A name to move is checked before anything is fetched, so a typo
        // fails at once: it must be a git dependency a member names, or a
        // package the lockfile already pins (a dependency's own git
        // dependency, say). The check after resolving catches the rest.
        let known = |name: &str| {
            previous
                .entries
                .keys()
                .any(|locked| locked.as_str() == name)
                || found.packages.iter().any(|(_, _, manifest)| {
                    manifest.dependencies.iter().any(|dependency| {
                        dependency.name.as_str() == name
                            && matches!(dependency.source, DependencySource::Git(_))
                    })
                })
        };
        if let Some(name) = moving.iter().find(|name| !known(name)) {
            return Err(PackageError::NotNamedOrPinned { name: name.clone() });
        }
        let mut pins = Pins::Updating {
            previous,
            moving,
            next: Lockfile::default(),
            cache,
        };
        Self::resolve(found.packages, &workspace, &mut pins)?;
        let Pins::Updating {
            previous,
            moving,
            next,
            ..
        } = pins
        else {
            unreachable!("the pins were built updating")
        };
        if let Some(name) = moving
            .into_iter()
            .find(|name| next.entries.keys().all(|locked| locked.as_str() != name))
        {
            return Err(PackageError::NotAGitDependency { name });
        }
        Ok(LockUpdate {
            path,
            previous,
            next,
        })
    }

    /// The packages the command names at `root`, and the workspace they
    /// belong to; `None` without a manifest.
    fn own_packages(
        root: &Path,
        workspace_allowed: bool,
    ) -> Result<Option<OwnPackages>, PackageError> {
        let manifest_path = root.join(MANIFEST_FILE);
        if !manifest_path.is_file() {
            return Ok(None);
        }
        match Manifest::read(&manifest_path).map_err(PackageError::Manifest)? {
            Manifest::Package(package) => Ok(Some(OwnPackages {
                packages: vec![(root.to_owned(), manifest_path, package)],
                workspace: workspace_root(root)?,
                is_workspace: false,
            })),
            Manifest::Workspace(_) if !workspace_allowed => Err(PackageError::RootIsWorkspace {
                manifest: manifest_path,
            }),
            Manifest::Workspace(workspace) => {
                let mut own: Vec<(PathBuf, PathBuf, PackageManifest)> = Vec::new();
                for member in &workspace.members {
                    let requested = root.join(member);
                    let directory =
                        requested
                            .canonicalize()
                            .map_err(|source| PackageError::MemberMissing {
                                workspace: manifest_path.clone(),
                                member: requested.clone(),
                                source,
                            })?;
                    if !directory.starts_with(root) {
                        return Err(PackageError::MemberOutsideWorkspace {
                            workspace: manifest_path,
                            member: directory,
                        });
                    }
                    if own.iter().any(|(listed, _, _)| *listed == directory) {
                        return Err(PackageError::MemberListedTwice {
                            workspace: manifest_path,
                            member: directory,
                        });
                    }
                    let member_manifest = directory.join(MANIFEST_FILE);
                    let package = match member_manifest
                        .is_file()
                        .then(|| Manifest::read(&member_manifest))
                        .transpose()
                        .map_err(PackageError::Manifest)?
                    {
                        Some(Manifest::Package(package)) => package,
                        _ => {
                            return Err(PackageError::MemberNotAPackage {
                                workspace: manifest_path,
                                member: directory,
                            });
                        }
                    };
                    if let Some((prior, _, _)) =
                        own.iter().find(|(_, _, prior)| prior.name == package.name)
                    {
                        return Err(PackageError::DuplicateMember {
                            name: package.name,
                            first: prior.clone(),
                            second: directory,
                        });
                    }
                    own.push((directory, member_manifest, package));
                }
                Ok(Some(OwnPackages {
                    packages: own,
                    workspace: root.to_owned(),
                    is_workspace: true,
                }))
            }
        }
    }

    /// The graph of the `own` packages (directory, manifest path, manifest)
    /// and everything they depend on, every path package inside `workspace`
    /// or inside the repository of the fetched package that names it.
    fn resolve(
        own: Vec<(PathBuf, PathBuf, PackageManifest)>,
        workspace: &Path,
        pins: &mut Pins<'_>,
    ) -> Result<Self, PackageError> {
        let mut graph = Self {
            packages: Vec::new(),
        };
        let mut pending = Vec::new();
        for (root, manifest_path, package) in own {
            graph.packages.push(Package {
                name: package.name,
                root,
                declared_by: manifest_path.clone(),
                manifest: manifest_path,
                own: true,
                dependencies: Vec::new(),
                git: None,
                boundary: Boundary::Workspace(workspace.to_owned()),
            });
            pending.push((graph.packages.len() - 1, package.dependencies));
        }
        while let Some((index, dependencies)) = pending.pop() {
            for dependency in dependencies {
                let importer_manifest = graph.packages[index].manifest.clone();
                graph.packages[index]
                    .dependencies
                    .push(dependency.name.clone());
                let (directory, git, boundary) = match &dependency.source {
                    DependencySource::Path(relative) => {
                        let importer = &graph.packages[index];
                        let directory = existing(
                            importer.root.join(relative),
                            &importer.manifest,
                            &dependency.name,
                        )?;
                        let boundary = importer.boundary.clone();
                        if !directory.starts_with(boundary.directory()) {
                            return Err(match boundary {
                                Boundary::Workspace(workspace) => PackageError::OutsideWorkspace {
                                    manifest: importer_manifest,
                                    name: dependency.name,
                                    directory,
                                    workspace,
                                },
                                Boundary::Repository(repository) => {
                                    PackageError::OutsideRepository {
                                        manifest: importer_manifest,
                                        name: dependency.name,
                                        directory,
                                        repository,
                                    }
                                }
                            });
                        }
                        (directory, None, boundary)
                    }
                    DependencySource::Git(source) => {
                        // A package already fetched for this name is reused
                        // only for the same source, before any pin is asked.
                        if let Some(known) = graph.find(&dependency.name) {
                            if known.git.as_ref().map(|locked| &locked.source) == Some(source) {
                                continue;
                            }
                            return Err(PackageError::Conflict {
                                name: dependency.name.clone(),
                                first: (known.declared_by.clone(), known.describe_source()),
                                second: (importer_manifest, describe_git(source)),
                            });
                        }
                        let commit = pins.commit(&dependency.name, source, &importer_manifest)?;
                        let checkout =
                            pins.cache()
                                .checkout(&source.url, &commit)
                                .map_err(|error| PackageError::Git {
                                    manifest: importer_manifest.clone(),
                                    name: dependency.name.clone(),
                                    error,
                                })?;
                        let checkout = existing(checkout, &importer_manifest, &dependency.name)?;
                        let directory = existing(
                            checkout.join(source.directory.as_deref().unwrap_or_default()),
                            &importer_manifest,
                            &dependency.name,
                        )?;
                        if !directory.starts_with(&checkout) {
                            return Err(PackageError::OutsideRepository {
                                manifest: importer_manifest,
                                name: dependency.name,
                                directory,
                                repository: checkout,
                            });
                        }
                        let locked = LockedGit {
                            source: source.clone(),
                            commit,
                        };
                        (directory, Some(locked), Boundary::Repository(checkout))
                    }
                };
                if let Some(known) = graph.find(&dependency.name) {
                    if known.root != directory {
                        return Err(PackageError::Conflict {
                            name: dependency.name,
                            first: (known.declared_by.clone(), known.describe_source()),
                            second: (importer_manifest, directory.display().to_string()),
                        });
                    }
                    continue;
                }
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
                    own: false,
                    dependencies: Vec::new(),
                    git,
                    boundary,
                });
                pending.push((graph.packages.len() - 1, package.dependencies));
            }
        }
        graph.reject_cycles()?;
        Ok(graph)
    }

    /// Every package of the build.
    pub(crate) fn packages(&self) -> impl Iterator<Item = &Package> {
        self.packages.iter()
    }

    /// The packages the command names, in member order.
    pub(crate) fn own(&self) -> impl Iterator<Item = &Package> {
        self.packages.iter().filter(|package| package.own)
    }

    pub(crate) fn find(&self, name: &PackageName) -> Option<&Package> {
        self.find_str(name.as_str())
    }

    pub(crate) fn find_str(&self, name: &str) -> Option<&Package> {
        self.packages
            .iter()
            .find(|package| package.name.as_str() == name)
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
                let next = graph
                    .find(dependency)
                    .expect("every dependency is in the graph");
                visit(graph, next, states, stack)?;
            }
            stack.pop();
            states.insert(&package.name, Visit::Done);
            Ok(())
        }
        let mut states = HashMap::new();
        for package in self.own() {
            visit(self, package, &mut states, &mut Vec::new())?;
        }
        Ok(())
    }
}

/// `requested`, canonical, or the error naming the dependency that asked
/// for it.
fn existing(
    requested: PathBuf,
    manifest: &Path,
    name: &PackageName,
) -> Result<PathBuf, PackageError> {
    requested
        .canonicalize()
        .map_err(|source| PackageError::DependencyMissing {
            manifest: manifest.to_owned(),
            name: name.clone(),
            directory: requested,
            source,
        })
}

fn describe_git(source: &GitSource) -> String {
    match &source.directory {
        Some(directory) => format!("{} at `{}`, directory {directory}", source.url, source.rev),
        None => format!("{} at `{}`", source.url, source.rev),
    }
}

impl Package {
    fn describe_source(&self) -> String {
        match &self.git {
            Some(locked) => describe_git(&locked.source),
            None => self.root.display().to_string(),
        }
    }
}

/// The root of the workspace `package` belongs to: the nearest directory
/// above it whose manifest is a `[workspace]` listing it, or `package` itself
/// when no workspace manifest is above it. A workspace above it that does not
/// list it is an error, so it never gets a lockfile of its own by accident.
pub(crate) fn workspace_root(package: &Path) -> Result<PathBuf, PackageError> {
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
