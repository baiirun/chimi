//! Two native builds into one artifact directory take turns: a build waits
//! while another holds the directory, so neither links the other's
//! intermediate files (CHI-299).

use std::process::Command;
use std::time::Duration;

// Rejects a build that writes its artifact directory while another build
// holds it: the test holds the lock, and `chimi build` must wait for it and
// then succeed. The test's hold is shared, so a build that took the lock
// shared too would not wait.
#[test]
fn a_build_waits_for_the_artifact_directory() {
    let scratch = std::env::temp_dir().join(format!("chimi-build-lock-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let artifacts = scratch.join("target/chimi/program");
    std::fs::create_dir_all(&artifacts).unwrap();
    std::fs::write(
        scratch.join("main.chimi"),
        "pub fn main() -> Int {\n  1 + 2\n}\n",
    )
    .unwrap();
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(artifacts.join(".build-lock"))
        .unwrap();
    lock.lock_shared().unwrap();
    let mut build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", "main.chimi", "-o", "program"])
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        build.try_wait().unwrap().is_none(),
        "the build did not wait for the lock"
    );
    lock.unlock().unwrap();
    assert!(build.wait().unwrap().success());
    assert!(scratch.join("program").exists());
    let _ = std::fs::remove_dir_all(&scratch);
}
