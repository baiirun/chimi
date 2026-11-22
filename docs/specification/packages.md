# Packages and workspaces

Status: implemented in part (T5, CHI-290). Packages, path dependencies,
imports across packages, module origins and the editor finding a file's
package are implemented. Workspace commands (PR 3) and git dependencies,
the lockfile and the cache (PR 4) are not yet; until then a git dependency
fails with "git dependencies are not supported yet".

## Purpose

A Chimi program today is one directory of modules (`--root <project>`) plus
the standard tree. Real services split into parts that change for different
reasons (a domain core, an HTTP front, a worker) and reuse code from other
repositories. T5 lets a program be built from several packages, with these
properties, in the order principle 14 gives them:

1. **Local workspaces.** Several packages in one repository build and test
   together, depending on each other by path.
2. **Reproducible builds.** The same checkout builds the same source on every
   machine: every dependency outside the repository is pinned to a commit in a
   lockfile that the repository commits.
3. **Dependency boundaries.** A module reaches only its own package, the
   standard tree, and the packages its own manifest names. What a dependency
   depends on is not visible to its users.
4. **Curated dependencies.** Dependencies are named one by one, by path or by
   a git repository and revision. There is no registry, no version solving
   and no install step.

Packages carry Chimi source only (decisions.md, 2026-11-22): no macros,
compiler plugins, build scripts or native code. A package module is an
application module, so it may not declare `native` items (`N3101`).

## Scope

In scope: the manifest (`chimi.toml`), the workspace manifest, path and git
dependencies, how imports resolve across packages, the lockfile
(`chimi.lock`), the dependency cache, and how the existing commands (`check`,
`run`, `build`, `test`, `doc`, `lint`, the language server) take a package or
a workspace.

Non-goals, each to be revisited when a real program needs it:

- A public registry, publishing, semantic versions and version solving.
- Renaming a dependency at the manifest (`alias = { package = ... }`).
- Two revisions of one package in one build.
- Overrides or patches of a transitive dependency.
- Optional dependencies, features and per-target dependencies.
- Package-level native code, build scripts and generated source.

## Behavior contract

### Packages

- A **package** is a directory with a `chimi.toml` whose `[package]` table
  names it. Its modules are the `.chimi` files under that directory: the file
  `orders.chimi` of the package `shop` is the module `shop/orders`, and
  `http/routes.chimi` is `shop/http/routes`. A directory with no manifest is
  still a project, loaded as it is today, with modules named by their plain
  path from the root.
- A package name MUST be a lowercase ASCII letter followed by lowercase
  letters, digits and underscores, and MUST NOT be `std`. Anything else is a
  manifest error naming the field.
- A subdirectory that holds its own `chimi.toml` is another package, not part
  of the enclosing one. `target/` and hidden entries are skipped, as today.

### Imports across packages

One rule covers every import: the first segment of an import path names a
package, and the rest names a module of it. `std` is the standard tree's
package, so `import std/list` already reads this way.

- In a module of package P, the first segment of an import MUST be `std`, P
  itself, or one of P's dependencies. Inside `shop`, `import shop/orders`
  names its own module, and `import money/amount` names the module `amount` of
  the dependency `money`.
- Any other first segment is a load error. When the package is in the build
  through another package, the error MUST say that it is not a dependency of
  P and name P's manifest; otherwise it says no package of that name exists.
  What a dependency depends on is therefore never importable by its users.
- An import of a package with no module path (`import money`) is an error: a
  package is not a module.
- A module MUST NOT import another package's module under `internal/`
  (`money/internal/rounding`); a package's own modules may. This is the rule
  `std/internal/` already follows (`N3103`), applied to every package.
- Module paths are the same in every build: a package's module is named
  `<package>/<path>` whether its package is the entry or a dependency, in
  diagnostics, `chimi inspect`, `chimi doc`, traces and the build report.
- A package MAY expose types from its own dependencies in its public
  functions. Its users can use such values but, without the dependency of
  their own, cannot name the type in an import.

### Manifest

`chimi.toml` is TOML. The compiler reads the subset below and rejects any
other table or key, naming it, so a typo never silently drops a dependency.

```toml
[package]
name = "shop_core"

[dependencies]
money = { path = "../money" }
json_tools = { git = "https://github.com/example/json_tools", rev = "v0.3.0" }
```

- `[package]` MUST have `name`. It has no `version`: a git revision is what a
  user pins, and there is no registry to publish versions to.
- Each entry in `[dependencies]` is keyed by the dependency's package name,
  and MUST be exactly one of:
  - `{ path = "<directory>" }`, relative to the manifest's directory, naming a
    directory with its own `chimi.toml`. It MUST stay inside the workspace
    root (and inside the repository, for a git package), so the same checkout
    always holds the same source;
  - `{ git = "<url>", rev = "<revision>" }`, where `rev` is a tag, a branch or
    a commit. `rev` is required, so the manifest always says what it means to
    get. An optional `directory = "<path>"` names the package's directory
    inside the repository, for repositories that hold several packages.
- The key MUST equal the `name` in the dependency's own manifest; a mismatch
  is an error naming both.

### Workspaces

```toml
[workspace]
members = ["app", "packages/shop_core", "packages/money"]
```

- A **workspace** is a directory whose `chimi.toml` has a `[workspace]` table
  and no `[package]` table. `members` lists member package directories,
  relative to it, each holding its own `chimi.toml`. A workspace member MAY
  also be named by another member's path dependency, and usually is.
- A package belongs to the workspace whose manifest lists it, found by
  walking up from the package's directory. A package with no workspace
  manifest above it is its own workspace of one. A package under a workspace
  directory that the workspace does not list is an error naming both, as in
  Cargo, so it never gets a lockfile and `target/` of its own by accident.
- The workspace root holds the one `chimi.lock` and the `target/` directory
  for every member.
- Two members MUST NOT have the same name.

### Resolving a build

- A build starts at one **entry package** and walks its dependencies. Every
  package name in the build MUST resolve to one source: one directory, or one
  git repository at one commit (and one `directory`). Two dependencies that
  name the same package with different sources are an error that names both
  manifests. There is no version solving.
- A dependency cycle between packages is an error that names the cycle, as an
  import cycle is.
- Only the modules the entry module reaches are loaded, as today.

### Lockfile

```toml
# Written by `chimi deps update`. Commit this file.
version = 1

[git.json_tools]
url = "https://github.com/example/json_tools"
rev = "v0.3.0"
commit = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
```

- The lockfile is a table per git package, `[git.<name>]`. It records, for
  each git package in the workspace's builds, the
  manifest's `url`, `rev` and `directory`, and the commit `rev` resolved to.
  Path packages are not locked: they are the repository's own source.
- `chimi deps update` resolves the git dependencies that have no lock entry
  or whose `url`, `rev` or `directory` changed, fetches them, and rewrites
  `chimi.lock` with entries sorted by name; `chimi deps update <name>` also
  moves that package's pin to what its `rev` points to now. These are the
  only commands that write the lockfile or move a pinned commit.
- `check`, `run`, `build`, `test`, `doc` and `lint` MUST NOT write the
  lockfile. A git dependency with no lock entry, or whose entry's `url`,
  `rev` or `directory` differs from the manifest, fails the command with
  "chimi.lock is out of date; run `chimi deps update`". An entry no manifest
  needs is ignored by builds and dropped by the next update.
- A build MUST use exactly the locked commit, whatever the branch or tag
  points to now.

### Dependency cache

- Git packages are fetched with the system `git` into a cache directory:
  `$CHIMI_CACHE` when set, otherwise `$XDG_CACHE_HOME/chimi`, otherwise
  `~/.cache/chimi`. A locked commit is checked out once into a directory named
  by its URL and commit, and never changes after that, so builds share it.
- A build fetches a locked commit missing from the cache. `--offline` makes a
  missing commit an error instead, naming the package, without touching the
  network.
- Fetched source is read only through the lockfile: a build MUST NOT take a
  commit from the cache that the lockfile does not name.

### Commands

- `--root <directory>` keeps its meaning. When the directory holds a
  `chimi.toml` with `[package]`, it is the entry package and its dependencies
  load as above.
- `chimi test <directory>`, given a package, tests every module of that
  package (not its dependencies); given a workspace, tests every member in
  member order and fails if any member fails.
- `chimi lint` and `chimi doc --html` lint and document the entry package's
  own modules; dependencies are type-checked but never linted, as `std/`
  is not.
- The language server finds the package of an open file by walking up to the
  nearest `chimi.toml`, with the same function the commands use, and resolves
  imports as a build would.
- A command given one file (`chimi check app.chimi`) reads that file alone,
  as today, even inside a package; `--root` names the package.

## Interfaces

| Surface | Form |
| --- | --- |
| Manifest | `chimi.toml`, `[package]` + `[dependencies]`, or `[workspace]` |
| Lockfile | `chimi.lock` at the workspace root, written only by `chimi deps update` |
| Cache | `$CHIMI_CACHE`, `$XDG_CACHE_HOME/chimi`, or `~/.cache/chimi` |
| Commands | `chimi deps update`; `--offline` on commands that load a package |
| Library | `LoadedProject::load` and `load_directory` read a manifest in the root; `ProjectSource::origin`; `project::package_root_of` |
| Build report | `chimi.build.v1` lists each package with its source and commit |

## Implementation model

- The **manifest reader** parses the TOML subset into a typed manifest. A
  hand-written reader keeps the compiler's dependency list as short as it is;
  the subset is small enough that a full TOML library would be mostly unused.
- The **package graph** is built before any module loads: the entry
  package's manifest, then each dependency's, with every name mapped to one
  source. Conflicts, cycles, escaping paths and lock mismatches fail here,
  before parsing.
- The **project loader** keeps loading modules from imports. An import's
  first segment picks the package, and the rest is a file under that
  package's directory, so a written import path is already the module's
  name in every later stage; resolution and typing see no packages.
- Each loaded module records its **origin**: the entry package, another
  package of the workspace, a fetched package, or the standard tree. Lint,
  doc, test and rename ask the origin instead of testing for a `std/` prefix.
- **Fetching** lives behind one function that turns a locked git package into
  a directory, so tests run against local repositories (`file://` URLs) and
  never need the network.

## Verification

| Contract | Test |
| --- | --- |
| A path dependency's module is imported by package name | `project::tests::a_package_imports_its_own_modules_and_its_dependencies_by_package_name`, `tests/packages.rs` `a_program_runs_across_path_dependencies` |
| An undeclared package in the build cannot be imported | `project::tests::imports_that_break_a_package_boundary_fail_naming_it` |
| `internal/` modules are private to their package | the same test, and the first one for a package's own |
| A package's own module is imported by its package name | `project::tests::a_package_imports_its_own_modules_and_its_dependencies_by_package_name` |
| `import money` with no module path fails | `project::tests::imports_that_break_a_package_boundary_fail_naming_it` |
| Two sources for one package name fail, naming both | `project::tests::the_package_graph_rejects_bad_dependencies_naming_both_ends` |
| Unknown manifest keys fail, naming them | `manifest::tests::rejects_anything_outside_the_shape_naming_it` |
| Test, lint and doc cover only the entry package | `tests/packages.rs` |
| A dependency's module is named `<package>/<path>` in diagnostics | `tests/packages.rs` |
| The editor analyzes an open file in its package | chimi-lsp `a_file_in_a_package_is_analyzed_with_its_package` |
| A workspace tests every member | added with PR 3 |
| An unlisted package under a workspace fails | `project::tests::a_package_the_workspace_above_does_not_list_fails` |
| A path dependency outside the workspace fails | `project::tests::a_dependency_outside_the_workspace_fails` |
| A build uses the locked commit after the tag moves | added with PR 4 |
| A stale lockfile fails the build and is not written | added with PR 4 |
| `--offline` with a missing commit fails without fetching | added with PR 4 |

Mutation lists: `scripts/mutation/packages_mutations.py` and
`packages_editor_mutations.py`.

## Notes

The plan, in small pull requests:

1. This contract.
2. Manifests, path dependencies and cross-package imports, with module
   origins and the language server finding a file's package.
3. Workspaces: members, one `target/`, `chimi test`, `lint` and `doc` over a
   workspace, and the forcing program, a multi-package workspace with docs
   and lint (language-completion-plan.md, Phase 9).
4. Git dependencies, `chimi.lock`, the cache, `chimi deps update` and
   `--offline`.

Why the first segment always names a package: one rule then covers every
import, the standard tree's included, and a reader knows where a module comes
from without opening the manifest (principle 4). Go and Gleam read this way.
Rust's `crate::` and Node's `./` mark own modules with syntax instead, and an
unmarked own path (`import orders` next to `import money/amount`) would need
the manifest to tell the two apart, plus a rule for an own directory named
like a dependency.

Why builds never write the lockfile: as in Go, the files a build reads are
the files the repository commits, so a build on CI and on a laptop agree, and
moving a dependency is a visible, reviewable change.
