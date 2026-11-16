//! `chimi build --target`: the host builds, and another target is refused
//! with what it would need.

use std::path::PathBuf;
use std::process::{Command, Output};

fn chimi(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(arguments)
        .output()
        .expect("run chimi build")
}

fn program(name: &str) -> (PathBuf, PathBuf) {
    let directory =
        std::env::temp_dir().join(format!("chimi-build-target-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let source = directory.join("main.chimi");
    std::fs::write(&source, "pub fn main() -> Int {\n  7\n}\n").unwrap();
    (source, directory.join("program"))
}

fn host_triple() -> String {
    let arch = std::env::consts::ARCH;
    if cfg!(target_os = "macos") {
        format!("{arch}-apple-darwin")
    } else {
        format!("{arch}-unknown-linux-gnu")
    }
}

#[test]
fn the_host_target_builds_as_without_one() {
    let (source, executable) = program("host");
    let output = chimi(&[
        "build",
        source.to_str().unwrap(),
        "--target",
        &host_triple(),
        "-o",
        executable.to_str().unwrap(),
    ]);

    assert!(output.status.success(), "{output:?}");
    let run = Command::new(&executable).output().unwrap();
    assert!(run.status.success(), "{run:?}");
}

#[test]
fn other_spellings_of_the_host_build() {
    let arch = std::env::consts::ARCH;
    let spellings = if cfg!(target_os = "macos") {
        vec![
            format!("{arch}-apple-macosx"),
            format!("{arch}-apple-darwin23.0.0"),
            format!("{arch}-apple-macosx14.0"),
        ]
    } else {
        vec![format!("{arch}-linux-gnu"), format!("{arch}-pc-linux-gnu")]
    };
    for spelling in spellings {
        let (source, executable) = program("spelling");
        let output = chimi(&[
            "build",
            source.to_str().unwrap(),
            "--target",
            &spelling,
            "-o",
            executable.to_str().unwrap(),
        ]);
        assert!(output.status.success(), "{spelling}: {output:?}");
    }
}

#[test]
fn another_target_is_refused_with_what_it_needs() {
    let (source, executable) = program("foreign");
    let output = chimi(&[
        "build",
        source.to_str().unwrap(),
        "--target",
        "riscv64gc-unknown-linux-gnu",
        "-o",
        executable.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!(
            "chimi: build failed: cannot build for riscv64gc-unknown-linux-gnu yet: building for \
             another target needs the Chimi runtime compiled for it and a linker and C library \
             for it, which this chimi does not have; it builds for {}\n",
            host_triple()
        )
    );
    assert!(!executable.exists());
}

#[test]
fn an_unknown_target_names_the_host_as_an_example() {
    let (source, _) = program("unknown");
    let output = chimi(&["build", source.to_str().unwrap(), "--target", "nonsense"]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!(
            "chimi: build failed: unknown target `nonsense`; give a target triple such as {}\n",
            host_triple()
        )
    );
}

#[test]
fn the_host_architecture_with_another_environment_is_another_target() {
    let arch = std::env::consts::ARCH;
    let other = if cfg!(target_os = "macos") {
        format!("{arch}-apple-ios")
    } else {
        format!("{arch}-unknown-linux-musl")
    };
    let (source, executable) = program("environment");
    let output = chimi(&[
        "build",
        source.to_str().unwrap(),
        "--target",
        &other,
        "-o",
        executable.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(!executable.exists());
}
