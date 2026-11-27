//! `chimi build --target` for each cross target whose runtime pack the
//! compiler under test embeds (tooling.md, Cross-compilation): a program
//! without the scheduler, one with it, and one with a C binding package each
//! build into an executable of the target's format and architecture, and the
//! build report names the target. Without `CHIMI_CROSS_TARGETS` at the build
//! no pack is embedded and these tests have no target to check; CI's `cross`
//! jobs build with it, and also run the executables (compare-programs.sh).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The supported cross targets, as `CROSS_TARGETS` in
/// `backend/cross_targets.rs` lists them: triple, operating system and
/// architecture as `std::env::consts` names them.
const TARGETS: [(&str, &str, &str); 4] = [
    ("x86_64-unknown-linux-gnu", "linux", "x86_64"),
    ("aarch64-unknown-linux-gnu", "linux", "aarch64"),
    ("x86_64-apple-darwin", "macos", "x86_64"),
    ("aarch64-apple-darwin", "macos", "aarch64"),
];

/// The targets other than the host whose pack `chimi` embeds. The tests are
/// built in the same cargo run as `chimi`, so they see the
/// `CHIMI_CROSS_TARGETS` its build script saw.
fn embedded_targets() -> Vec<(&'static str, &'static str, &'static str)> {
    let names = option_env!("CHIMI_CROSS_TARGETS").unwrap_or("");
    let named = |triple: &str| {
        names
            .split(',')
            .map(str::trim)
            .any(|name| name == "all" || name == triple)
    };
    TARGETS
        .into_iter()
        .filter(|(triple, os, arch)| {
            named(triple) && !(*os == std::env::consts::OS && *arch == std::env::consts::ARCH)
        })
        .collect()
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "chimi-cross-build-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    /// Runs `chimi` in the scratch directory, so its `target/chimi` objects
    /// stay there.
    fn chimi(&self, arguments: &[&str], environment: &[(&str, &str)]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&self.0)
            .args(arguments)
            .envs(environment.iter().copied())
            .output()
            .expect("run chimi build")
    }

    fn write(&self, name: &str, source: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, source).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const PLAIN: &str = "pub fn main() -> Int {\n  7\n}\n";

const SCHEDULER: &str = r#"import std/task as Task

pub fn main() -> Int {
  let task = Task.start(fn() { 20 + 1 })
  Task.result(task) * 2
}
"#;

const BINDING: &str = r#"import std/clock.{Clock}
import std/fs.{Files}
import std/io.{Console}
import std/sql as Sql
import std/sql.{Value}
import std/sqlite as Sqlite

pub fn main() -> Int needs { clock: Clock, console: Console, files: Files } {
  Sql.open(Sqlite.driver(), ":memory:", { ..Sql.defaults, max_connections: 1 }, fn(db) {
    Sql.execute(db, "create table users (id integer primary key, name text)", [])
    Sql.execute(db, "insert into users (name) values (?)", [Value.Text("Ada")])
    console.write_line("inserted")
  })
  0
}
"#;

/// The operating system and architecture the executable at `path` is for,
/// from its ELF or Mach-O header.
fn executable_platform(path: &Path) -> (&'static str, &'static str) {
    let bytes = std::fs::read(path).unwrap();
    let u16_at = |offset: usize| u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    let u32_at = |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    if bytes.starts_with(b"\x7fELF") {
        // e_type: 2 is an executable, 3 a position-independent one.
        assert!(matches!(u16_at(16), 2 | 3), "{}: not an ELF executable", path.display());
        let arch = match u16_at(18) {
            62 => "x86_64",
            183 => "aarch64",
            other => panic!("{}: ELF machine {other}", path.display()),
        };
        ("linux", arch)
    } else if u32_at(0) == 0xfeed_facf {
        // filetype 2 is MH_EXECUTE.
        assert_eq!(u32_at(12), 2, "{}: not a Mach-O executable", path.display());
        let arch = match u32_at(4) {
            0x0100_0007 => "x86_64",
            0x0100_000c => "aarch64",
            other => panic!("{}: Mach-O CPU type {other:#x}", path.display()),
        };
        ("macos", arch)
    } else {
        panic!("{}: neither ELF nor 64-bit Mach-O", path.display());
    }
}

#[test]
fn each_embedded_target_builds_executables_for_its_platform() {
    for (triple, os, arch) in embedded_targets() {
        let scratch = Scratch::new(&format!("builds-{triple}"));
        for (name, source) in [("plain", PLAIN), ("scheduler", SCHEDULER), ("binding", BINDING)] {
            let source = scratch.write(&format!("{name}.chimi"), source);
            let executable = scratch.0.join(format!("{name}-{triple}"));
            let output = scratch.chimi(
                &[
                    "build",
                    source.to_str().unwrap(),
                    "--target",
                    triple,
                    "-o",
                    executable.to_str().unwrap(),
                ],
                &[],
            );
            assert!(output.status.success(), "{triple} {name}: {output:?}");
            assert_eq!(executable_platform(&executable), (os, arch), "{triple} {name}");

            let report = String::from_utf8(output.stdout).unwrap();
            assert!(
                report.contains(&format!(
                    "\"target\":{{\"os\":\"{os}\",\"arch\":\"{arch}\",\"triple\":\"{triple}\"}}"
                )),
                "{triple} {name}: {report}"
            );
            let libc = if os == "linux" {
                "\"libc.so.6\""
            } else {
                "\"/usr/lib/libSystem.B.dylib\""
            };
            assert!(
                report.contains(&format!("\"dynamic_dependencies\":[{libc}")),
                "{triple} {name}: {report}"
            );
            if name == "binding" {
                assert!(report.contains("\"name\":\"sqlite\""), "{triple}: {report}");
            }
        }
    }
}

#[test]
fn other_spellings_of_an_embedded_target_build_for_it() {
    for (triple, os, arch) in embedded_targets() {
        let spelling = match triple {
            "aarch64-apple-darwin" => "arm64-apple-macosx14.0",
            "x86_64-apple-darwin" => "x86_64-apple-macosx",
            "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
            _ => "x86_64-pc-linux-gnu",
        };
        let scratch = Scratch::new(&format!("spelling-{triple}"));
        let source = scratch.write("plain.chimi", PLAIN);
        let executable = scratch.0.join("plain");
        let output = scratch.chimi(
            &[
                "build",
                source.to_str().unwrap(),
                "--target",
                spelling,
                "-o",
                executable.to_str().unwrap(),
            ],
            &[],
        );
        assert!(output.status.success(), "{spelling}: {output:?}");
        assert_eq!(executable_platform(&executable), (os, arch), "{spelling}");
    }
}

#[test]
fn a_missing_zig_is_named_before_anything_compiles() {
    let Some((triple, _, _)) = embedded_targets().into_iter().next() else {
        return;
    };
    let scratch = Scratch::new("no-zig");
    let source = scratch.write("plain.chimi", PLAIN);
    let executable = scratch.0.join("plain");
    let missing = scratch.0.join("no-such-zig");
    let missing = missing.to_str().unwrap();
    let output = scratch.chimi(
        &[
            "build",
            source.to_str().unwrap(),
            "--target",
            triple,
            "-o",
            executable.to_str().unwrap(),
        ],
        &[("CHIMI_ZIG", missing)],
    );

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with(&format!(
            "chimi: build failed: cannot build for {triple}: building for another target needs \
             Zig, to compile and link C for it, and `{missing} version` did not run ("
        )),
        "{stderr}"
    );
    assert!(
        stderr.ends_with(
            "); install Zig from https://ziglang.org and put `zig` on PATH, or set CHIMI_ZIG to it\n"
        ),
        "{stderr}"
    );
    assert!(!executable.exists());
    assert!(!scratch.0.join("target").exists(), "nothing compiled");
}
