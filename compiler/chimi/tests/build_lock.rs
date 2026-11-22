//! Native builds into one artifact directory take turns, and so do test runs
//! in one place: a build or a run waits while another holds the directory,
//! so neither links nor runs the other's files (CHI-299).

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Duration;

fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("chimi-lock-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn lock_file(path: &Path) -> File {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .unwrap()
}

fn chimi(directory: &Path, arguments: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(directory)
        .args(arguments)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap()
}

/// Holds `lock` while `command` starts, checks that it waits, then releases
/// it and checks that `command` succeeds. The hold is shared, so a command
/// that took the lock shared too would not wait.
fn waits_for(lock: &Path, mut command: impl FnMut() -> Child) {
    let held = lock_file(lock);
    held.lock_shared().unwrap();
    let mut child = command();
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        child.try_wait().unwrap().is_none(),
        "did not wait for {}",
        lock.display()
    );
    held.unlock().unwrap();
    assert!(child.wait().unwrap().success());
}

// Rejects a build that writes its artifact directory while another build
// holds it.
#[test]
fn a_build_waits_for_the_artifact_directory() {
    let directory = scratch("build");
    std::fs::write(
        directory.join("main.chimi"),
        "pub fn main() -> Int {\n  1 + 2\n}\n",
    )
    .unwrap();
    let lock = directory.join("target/chimi/program/.build-lock");
    waits_for(&lock, || {
        chimi(&directory, &["build", "main.chimi", "-o", "program"])
    });
    assert!(directory.join("program").exists());
    let _ = std::fs::remove_dir_all(&directory);
}

// Rejects a build that takes the lock and lets it go before it is done: the
// lock must be seen held while the build runs.
#[test]
fn a_build_holds_the_artifact_directory_until_it_is_done() {
    let directory = scratch("hold");
    std::fs::write(
        directory.join("main.chimi"),
        "pub fn main() -> Int {\n  1 + 2\n}\n",
    )
    .unwrap();
    let probe = lock_file(&directory.join("target/chimi/program/.build-lock"));
    let mut build = chimi(&directory, &["build", "main.chimi", "-o", "program"]);
    let mut seen_held = false;
    while build.try_wait().unwrap().is_none() {
        match probe.try_lock() {
            Ok(()) => probe.unlock().unwrap(),
            Err(_) => seen_held = true,
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(build.wait().unwrap().success());
    assert!(seen_held, "the build never held its artifact directory");
    let _ = std::fs::remove_dir_all(&directory);
}

// Rejects a test run that rebuilds or runs the shared `tests` executable, or
// clears the traces, while another run in the same place is going.
#[test]
fn a_test_run_waits_for_another_in_the_same_place() {
    let directory = scratch("test");
    let cases =
        "import std/test as Test\n\ntest \"adds\" {\n  (1 + 2) |> Test.assert_equal(3)\n}\n";
    std::fs::write(directory.join("cases.chimi"), cases).unwrap();
    let lock = directory.join("target/chimi/cases/test/.test-lock");
    waits_for(&lock, || chimi(&directory, &["test", "cases.chimi"]));
    let _ = std::fs::remove_dir_all(&directory);
}
