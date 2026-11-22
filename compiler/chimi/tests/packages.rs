//! Packages (packages.md): a build that spans path dependencies, through the
//! commands. The fixture is a workspace where `app` depends on `money`, which
//! depends on `rounding`; `money` has a test, an unused private function and
//! an `internal/` module, which only `app`'s own commands must not touch.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(package: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/packages")
        .join(package)
}

fn chimi(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(arguments)
        .output()
        .expect("run chimi")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn a_program_runs_across_path_dependencies() {
    let app = fixture("app");
    let output = chimi(&["run", "--root", app.to_str().unwrap(), "app/main"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    // 40 + 3 through money, rounded down to even through rounding.
    assert!(
        text(&output.stdout)
            .ends_with("{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"),
        "{}",
        text(&output.stdout)
    );
}

#[test]
fn test_runs_only_the_entry_packages_tests() {
    let app = fixture("app");
    let output = chimi(&["test", app.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "test app/total \"the app's own test runs\" ... ok\n\n1 passed, 0 failed\n"
    );
}

#[test]
fn lint_skips_dependencies() {
    let app = fixture("app");
    let output = chimi(&["lint", "--root", app.to_str().unwrap(), "app/main"]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert!(output.stderr.is_empty(), "{}", text(&output.stderr));

    // The same unused function is a finding once its package is the entry.
    let money = fixture("money");
    let output = chimi(&["lint", "--root", money.to_str().unwrap(), "money/amount"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("the private function never_used is never used"),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn doc_pages_cover_only_the_entry_packages_modules() {
    let site = std::env::temp_dir().join(format!("chimi-packages-doc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&site);
    let app = fixture("app");
    let output = chimi(&[
        "doc",
        "--html",
        site.to_str().unwrap(),
        "--root",
        app.to_str().unwrap(),
        "app/main",
    ]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(site.join("app/total.html").is_file());
    assert!(
        !site.join("money").exists(),
        "a dependency is not documented"
    );
    let _ = std::fs::remove_dir_all(&site);
}

#[test]
fn a_dependencys_module_is_named_by_its_package_in_diagnostics() {
    let scratch = std::env::temp_dir().join(format!("chimi-packages-diag-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    for (path, text) in [
        (
            "chimi.toml",
            "[workspace]\nmembers = [\"app\", \"money\"]\n",
        ),
        (
            "app/chimi.toml",
            "[package]\nname = \"app\"\n[dependencies]\nmoney = { path = \"../money\" }\n",
        ),
        (
            "app/main.chimi",
            "import money/amount as Amount\npub fn main() -> Int { Amount.value() }\n",
        ),
        ("money/chimi.toml", "[package]\nname = \"money\"\n"),
        (
            "money/amount.chimi",
            "pub fn value() -> Int { missing() }\n",
        ),
    ] {
        let file = scratch.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let output = chimi(&[
        "check",
        "--json",
        "--root",
        scratch.join("app").to_str().unwrap(),
        "app/main",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stdout).contains("\"module\":\"money/amount\""),
        "{}",
        text(&output.stdout)
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn a_workspace_tests_every_member() {
    let workspace = fixture("");
    let output = chimi(&["test", workspace.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        concat!(
            "test app/total \"the app's own test runs\" ... ok\n",
            "test money/amount \"money's own test\" ... ok\n",
            "\n2 passed, 0 failed\n",
        )
    );
}

#[test]
fn a_workspace_lints_and_documents_every_member() {
    let workspace = fixture("");
    let output = chimi(&["lint", workspace.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("money/amount.chimi:9:4: warning[unused-private]"),
        "{}",
        text(&output.stderr)
    );

    let site = std::env::temp_dir().join(format!("chimi-workspace-doc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&site);
    let output = chimi(&[
        "doc",
        "--html",
        site.to_str().unwrap(),
        workspace.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    for page in ["app/total.html", "money/amount.html", "rounding/even.html"] {
        assert!(site.join(page).is_file(), "{page}");
    }
    let _ = std::fs::remove_dir_all(&site);

    let output = chimi(&["doc", workspace.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("doc of a directory writes pages"));
}

#[test]
fn a_members_build_keeps_its_artifacts_in_the_workspace_target() {
    let scratch =
        std::env::temp_dir().join(format!("chimi-workspace-target-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    for (path, text) in [
        ("chimi.toml", "[workspace]\nmembers = [\"app\"]\n"),
        ("app/chimi.toml", "[package]\nname = \"app\"\n"),
        ("app/main.chimi", "pub fn main() -> Int { 7 }\n"),
    ] {
        let file = scratch.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let elsewhere = scratch.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&elsewhere)
        .args(["build", "--root"])
        .arg(scratch.join("app"))
        .args(["app/main", "-o"])
        .arg(elsewhere.join("program"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(scratch.join("target/chimi/program").is_dir());
    assert!(!elsewhere.join("target").exists());
    let _ = std::fs::remove_dir_all(&scratch);
}

/// A scratch directory holding `files`, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!("chimi-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let scratch = Self(root);
        for (path, text) in files {
            scratch.write(path, text);
        }
        scratch
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }

    fn write(&self, relative: &str, text: &str) {
        let file = self.0.join(relative);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git(directory: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .current_dir(directory)
        .args([
            "-c",
            "user.name=chimi",
            "-c",
            "user.email=chimi@example.com",
        ])
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(arguments)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        text(&output.stderr)
    );
}

/// `chimi` with its dependency cache at `cache`.
fn chimi_with_cache(cache: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_chimi"))
        .env("CHIMI_CACHE", cache)
        .args(arguments)
        .output()
        .expect("run chimi")
}

/// A repository at `repo/` holding the package `json_tools` in `pkg/`,
/// whose `value()` is `value`, tagged `v1`; and a package `app/` that
/// depends on it at `v1`.
fn git_dependency_scratch(name: &str, value: i64) -> Scratch {
    let scratch = Scratch::new(
        name,
        &[
            ("repo/pkg/chimi.toml", "[package]\nname = \"json_tools\"\n"),
            (
                "app/main.chimi",
                "import json_tools/value as Value\npub fn main() -> Int { Value.value() }\n",
            ),
        ],
    );
    scratch.write(
        "repo/pkg/value.chimi",
        &format!("pub fn value() -> Int {{ {value} }}\n"),
    );
    let url = format!("file://{}", scratch.path("repo").display());
    scratch.write(
        "app/chimi.toml",
        &format!(
            "[package]\nname = \"app\"\n[dependencies]\njson_tools = {{ git = \"{url}\", rev = \"v1\", directory = \"pkg\" }}\n"
        ),
    );
    let repo = scratch.path("repo");
    git(&repo, &["init", "--quiet", "-b", "main"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "first"]);
    git(&repo, &["tag", "v1"]);
    scratch
}

fn runs_to(output: &Output, value: i64) -> bool {
    output.status.success()
        && text(&output.stdout).ends_with(&format!(
            "{{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":{value}}}\n"
        ))
}

#[test]
fn a_build_uses_the_locked_commit_until_the_package_is_updated_by_name() {
    let scratch = git_dependency_scratch("git-lock", 1);
    let cache = scratch.path("cache");
    let app = scratch.path("app");
    let app = app.to_str().unwrap();
    let run = || chimi_with_cache(&cache, &["run", "--root", app, "app/main"]);

    let output = chimi_with_cache(&cache, &["deps", "update", "--root", app]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        text(&output.stderr).contains("locked json_tools at "),
        "{}",
        text(&output.stderr)
    );
    let lock = std::fs::read_to_string(scratch.path("app/chimi.lock")).unwrap();
    assert!(lock.contains("[git.json_tools]\n"), "{lock}");
    assert!(lock.contains("directory = \"pkg\"\n"), "{lock}");
    let output = run();
    assert!(
        runs_to(&output, 1),
        "{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );

    // The tag moves; the lock does not.
    let repo = scratch.path("repo");
    scratch.write("repo/pkg/value.chimi", "pub fn value() -> Int { 2 }\n");
    git(&repo, &["commit", "--quiet", "-am", "second"]);
    git(&repo, &["tag", "--force", "v1"]);
    let output = run();
    assert!(
        runs_to(&output, 1),
        "{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    // A cache that has never seen the repository fetches the locked commit,
    // not what the tag or branch names now.
    let fresh = scratch.path("fresh-cache");
    let output = chimi_with_cache(&fresh, &["run", "--root", app, "app/main"]);
    assert!(
        runs_to(&output, 1),
        "{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );

    // An update that names nothing keeps a pin whose source is unchanged.
    let output = chimi_with_cache(&cache, &["deps", "update", "--root", app]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(output.stderr.is_empty(), "{}", text(&output.stderr));
    assert_eq!(
        std::fs::read_to_string(scratch.path("app/chimi.lock")).unwrap(),
        lock
    );

    let output = chimi_with_cache(&cache, &["deps", "update", "--root", app, "json_tools"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        text(&output.stderr).contains("moved json_tools from "),
        "{}",
        text(&output.stderr)
    );
    let output = run();
    assert!(
        runs_to(&output, 2),
        "{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );

    let output = chimi_with_cache(&cache, &["deps", "update", "--root", app, "money"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("`money` is not a git dependency of the workspace"),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn a_stale_lockfile_fails_the_build_and_is_not_written() {
    let scratch = git_dependency_scratch("git-stale", 1);
    let cache = scratch.path("cache");
    let app = scratch.path("app");
    let app = app.to_str().unwrap();

    // No lock at all.
    let output = chimi_with_cache(&cache, &["check", "--root", app, "app/main"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("chimi.lock is out of date; run `chimi deps update`"),
        "{}",
        text(&output.stderr)
    );
    assert!(
        !scratch.path("app/chimi.lock").exists(),
        "a build never writes the lockfile"
    );

    // A lock for another rev.
    assert!(
        chimi_with_cache(&cache, &["deps", "update", "--root", app])
            .status
            .success()
    );
    let lock = std::fs::read_to_string(scratch.path("app/chimi.lock")).unwrap();
    let manifest = std::fs::read_to_string(scratch.path("app/chimi.toml")).unwrap();
    scratch.write(
        "app/chimi.toml",
        &manifest.replace("rev = \"v1\"", "rev = \"main\""),
    );
    let output = chimi_with_cache(&cache, &["check", "--root", app, "app/main"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("chimi.lock is out of date"),
        "{}",
        text(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(scratch.path("app/chimi.lock")).unwrap(),
        lock
    );
}

#[test]
fn offline_a_missing_commit_fails_without_fetching() {
    let scratch = git_dependency_scratch("git-offline", 5);
    let app = scratch.path("app");
    let app = app.to_str().unwrap();
    assert!(
        chimi_with_cache(&scratch.path("cache"), &["deps", "update", "--root", app])
            .status
            .success()
    );

    // A cache that has never seen the repository.
    let empty = scratch.path("empty-cache");
    let output = chimi_with_cache(&empty, &["--offline", "run", "--root", app, "app/main"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr)
            .contains("is not in the dependency cache, and --offline forbids fetching it"),
        "{}",
        text(&output.stderr)
    );
    assert!(
        !empty.join("git/checkouts").exists(),
        "nothing was checked out"
    );

    let output = chimi_with_cache(&empty, &["run", "--root", app, "app/main"]);
    assert!(
        runs_to(&output, 5),
        "{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    // Now cached, so offline works.
    let output = chimi_with_cache(&empty, &["--offline", "run", "--root", app, "app/main"]);
    assert!(
        runs_to(&output, 5),
        "{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );

    // The cache knows the repository but not the commit the lock now pins.
    scratch.write("repo/pkg/value.chimi", "pub fn value() -> Int { 6 }\n");
    git(
        &scratch.path("repo"),
        &["commit", "--quiet", "-am", "second"],
    );
    git(&scratch.path("repo"), &["tag", "--force", "v1"]);
    let output = chimi_with_cache(
        &scratch.path("cache"),
        &["deps", "update", "--root", app, "json_tools"],
    );
    assert!(output.status.success(), "{}", text(&output.stderr));
    let output = chimi_with_cache(&empty, &["--offline", "run", "--root", app, "app/main"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stdout));
    assert!(
        text(&output.stderr).contains("forbids fetching it"),
        "{}",
        text(&output.stderr)
    );

    let output = chimi_with_cache(&empty, &["--offline", "deps", "update"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn a_git_package_directory_must_stay_inside_its_repository() {
    let scratch = git_dependency_scratch("git-escape", 1);
    let app = scratch.path("app");
    let manifest = std::fs::read_to_string(app.join("chimi.toml")).unwrap();
    scratch.write(
        "app/chimi.toml",
        &manifest.replace("directory = \"pkg\"", "directory = \"..\""),
    );
    let output = chimi_with_cache(
        &scratch.path("cache"),
        &["deps", "update", "--root", app.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("is outside its repository's checkout"),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn two_git_sources_for_one_package_fail_naming_both() {
    let scratch = git_dependency_scratch("git-conflict", 1);
    let url = format!("file://{}", scratch.path("repo").display());
    scratch.write(
        "app/chimi.toml",
        &format!(
            "[package]\nname = \"app\"\n[dependencies]\n\
             json_tools = {{ git = \"{url}\", rev = \"v1\", directory = \"pkg\" }}\n\
             other = {{ path = \"../other\" }}\n"
        ),
    );
    scratch.write(
        "chimi.toml",
        "[workspace]\nmembers = [\"app\", \"other\"]\n",
    );
    scratch.write(
        "other/chimi.toml",
        &format!(
            "[package]\nname = \"other\"\n[dependencies]\n\
             json_tools = {{ git = \"{url}\", rev = \"main\", directory = \"pkg\" }}\n"
        ),
    );
    let app = scratch.path("app");
    let output = chimi_with_cache(
        &scratch.path("cache"),
        &["deps", "update", "--root", app.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(1));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("package `json_tools` has two sources: "),
        "{stderr}"
    );
    assert!(
        stderr.contains("at `v1`") && stderr.contains("at `main`"),
        "{stderr}"
    );
    assert!(!scratch.path("app/chimi.lock").exists());
}

#[test]
fn a_members_update_writes_the_one_lockfile_at_the_workspace_root() {
    let scratch = git_dependency_scratch("git-workspace", 4);
    scratch.write("chimi.toml", "[workspace]\nmembers = [\"app\"]\n");
    let app = scratch.path("app");
    let output = chimi_with_cache(
        &scratch.path("cache"),
        &["deps", "update", "--root", app.to_str().unwrap()],
    );
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(scratch.path("chimi.lock").is_file());
    assert!(!scratch.path("app/chimi.lock").exists());
    let output = chimi_with_cache(
        &scratch.path("cache"),
        &["run", "--root", app.to_str().unwrap(), "app/main"],
    );
    assert!(
        runs_to(&output, 4),
        "{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
}

#[test]
fn a_rev_may_be_a_commit_no_branch_reaches() {
    let scratch = git_dependency_scratch("git-orphan", 1);
    let repo = scratch.path("repo");
    git(&repo, &["checkout", "--quiet", "-b", "side"]);
    scratch.write("repo/pkg/value.chimi", "pub fn value() -> Int { 9 }\n");
    git(&repo, &["commit", "--quiet", "-am", "side"]);
    let commit = Command::new("git")
        .current_dir(&repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let commit = text(&commit.stdout).trim().to_owned();
    git(&repo, &["checkout", "--quiet", "main"]);
    git(&repo, &["branch", "--quiet", "-D", "side"]);
    let manifest = std::fs::read_to_string(scratch.path("app/chimi.toml")).unwrap();
    scratch.write(
        "app/chimi.toml",
        &manifest.replace("rev = \"v1\"", &format!("rev = \"{commit}\"")),
    );
    let app = scratch.path("app");
    let app = app.to_str().unwrap();
    let cache = scratch.path("cache");
    let output = chimi_with_cache(&cache, &["deps", "update", "--root", app]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    let output = chimi_with_cache(&cache, &["run", "--root", app, "app/main"]);
    assert!(
        runs_to(&output, 9),
        "{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
}
