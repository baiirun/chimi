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
        text(&output.stdout).ends_with("{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"),
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
    assert!(!site.join("money").exists(), "a dependency is not documented");
    let _ = std::fs::remove_dir_all(&site);
}

#[test]
fn a_dependencys_module_is_named_by_its_package_in_diagnostics() {
    let scratch = std::env::temp_dir().join(format!("chimi-packages-diag-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    for (path, text) in [
        ("chimi.toml", "[workspace]\nmembers = [\"app\", \"money\"]\n"),
        (
            "app/chimi.toml",
            "[package]\nname = \"app\"\n[dependencies]\nmoney = { path = \"../money\" }\n",
        ),
        (
            "app/main.chimi",
            "import money/amount as Amount\npub fn main() -> Int { Amount.value() }\n",
        ),
        ("money/chimi.toml", "[package]\nname = \"money\"\n"),
        ("money/amount.chimi", "pub fn value() -> Int { missing() }\n"),
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
    let output = chimi(&["doc", "--html", site.to_str().unwrap(), workspace.to_str().unwrap()]);
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
    let scratch = std::env::temp_dir().join(format!("chimi-workspace-target-{}", std::process::id()));
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
