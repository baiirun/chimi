//! `chimi build --target`: the host builds, and a target this compiler cannot
//! build for is refused, before compiling anything, with what it would need.
//! Building for the supported cross targets is `cross_build.rs`.

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
    // `chimi build` keeps its objects under `target/chimi/<executable name>`
    // in the working directory, which every test here shares, so each test's
    // executable needs its own name or parallel builds overwrite each other.
    (source, directory.join(format!("program-{name}")))
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

/// The cross targets other than the host, as `CROSS_TARGETS` in
/// `backend/cross_targets.rs` lists them.
fn other_supported_targets() -> Vec<&'static str> {
    [
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
    ]
    .into_iter()
    .filter(|triple| *triple != host_triple())
    .collect()
}

#[test]
fn an_unsupported_target_is_refused_naming_the_supported_ones() {
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
            "chimi: build failed: cannot build for riscv64gc-unknown-linux-gnu: chimi builds \
             for its host, {}, and, with Zig, {}\n",
            host_triple(),
            other_supported_targets().join(", ")
        )
    );
    assert!(!executable.exists());
}

/// Whether the compiler under test embeds the runtime pack for `triple`:
/// the tests are built in the same cargo run as `chimi`, so they see the
/// `CHIMI_CROSS_TARGETS` its build script saw.
fn embeds_pack(triple: &str) -> bool {
    option_env!("CHIMI_CROSS_TARGETS").is_some_and(|names| {
        names
            .split(',')
            .map(str::trim)
            .any(|name| name == "all" || name == triple)
    })
}

#[test]
fn a_supported_target_without_its_pack_names_the_rebuild_that_adds_it() {
    let Some(triple) = other_supported_targets()
        .into_iter()
        .find(|triple| !embeds_pack(triple))
    else {
        // This compiler embeds every pack; `cross_build.rs` covers them.
        return;
    };
    let (source, executable) = program("no-pack");
    let output = chimi(&[
        "build",
        source.to_str().unwrap(),
        "--target",
        triple,
        "-o",
        executable.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!(
            "chimi: build failed: cannot build for {triple}: this chimi was built without its \
             runtime; rebuild chimi with CHIMI_CROSS_TARGETS={triple} set, which needs `rustup \
             target add {triple}` and Zig\n"
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
