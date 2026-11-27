//! The platform one `chimi build` is for, and the code generator, C tools and
//! runtime archives that build for it (tooling.md, Cross-compilation).
//!
//! The host builds with Cranelift's host code generator, the host's `cc` and
//! `ar`, and the runtime archives `build.rs` made for the compiler's own
//! target. Each other supported target (`cross_targets.rs`) builds with
//! Cranelift's code generator for that target, Zig's C compiler, linker and
//! archiver, and the runtime pack `build.rs` made for it when the compiler
//! was built with `CHIMI_CROSS_TARGETS` naming it.

use super::cross_targets::{
    CROSS_TARGETS, CrossTarget, ZIG_AR_ARGUMENTS, ZIG_VARIABLE, zig_cc_arguments, zig_program,
};
use super::runtime_archive_key::{CTools, RUNTIME_C_COMPILER};
use cranelift_codegen::isa::OwnedTargetIsa;
use cranelift_codegen::settings::{self, Configurable};
use std::ffi::OsString;
use std::process::Command;
use std::str::FromStr;
use target_lexicon::{OperatingSystem, Triple};

/// The variable that names, at the compiler's build, the targets whose
/// runtime packs `build.rs` builds and embeds.
pub const CROSS_TARGETS_VARIABLE: &str = "CHIMI_CROSS_TARGETS";

/// A runtime pack: the Rust runtime archives built for one cross target.
/// The C runtime and binding packages are compiled per project instead,
/// with Zig, from the sources the compiler embeds.
pub(super) struct RuntimePack {
    /// `chimi-fiber`'s static library, for a program that reaches the
    /// scheduler.
    pub(super) fiber: &'static [u8],
    /// `chimi-runtime-standalone`'s static library, for every other program.
    pub(super) standalone: &'static [u8],
}

// Written by `build.rs`: `(triple, fiber archive, standalone archive)` of
// each target `CHIMI_CROSS_TARGETS` named whose pack built; empty without it.
include!(concat!(env!("OUT_DIR"), "/cross_runtime_packs.rs"));

fn embedded_pack(triple: &str) -> Option<RuntimePack> {
    CROSS_RUNTIME_PACKS
        .iter()
        .find(|(name, _, _)| *name == triple)
        .map(|(_, fiber, standalone)| RuntimePack { fiber, standalone })
}

/// The platform a build is for.
pub struct BuildTarget {
    isa: OwnedTargetIsa,
    toolchain: Toolchain,
}

enum Toolchain {
    Host,
    Cross {
        target: &'static CrossTarget,
        zig: OsString,
        /// `zig version`'s output, part of every C archive's key.
        zig_version: String,
        pack: RuntimePack,
    },
}

impl BuildTarget {
    /// The machine running `chimi`.
    pub fn host() -> Result<Self, String> {
        Ok(Self {
            isa: host_isa()?,
            toolchain: Toolchain::Host,
        })
    }

    /// The target `chimi build --target <triple>` names, with everything it
    /// needs present: a target the host is, or a supported cross target whose
    /// runtime pack this compiler embeds, with Zig to run.
    ///
    /// Errors name what is missing and how to get it, before anything is
    /// compiled.
    pub fn resolve(triple: &str) -> Result<Self, String> {
        let host = Self::host()?;
        let host_triple = host.isa.triple().clone();
        let Ok(wanted) = Triple::from_str(triple) else {
            return Err(format!(
                "unknown target `{triple}`; give a target triple such as {host_triple}"
            ));
        };
        if same_platform(&wanted, &host_triple) {
            return Ok(host);
        }
        let Some(target) = CROSS_TARGETS.iter().find(|target| {
            Triple::from_str(target.triple).is_ok_and(|known| same_platform(&wanted, &known))
        }) else {
            return Err(format!(
                "cannot build for {wanted}: chimi builds for {}",
                supported_targets(&host_triple)
            ));
        };
        let Some(pack) = embedded_pack(target.triple) else {
            return Err(format!(
                "cannot build for {}: this chimi was built without its runtime; rebuild chimi \
                 with {CROSS_TARGETS_VARIABLE}={} set, which needs `rustup target add {}` and Zig",
                target.triple, target.triple, target.triple
            ));
        };
        let zig = zig_program();
        let zig_version = zig_version(&zig).map_err(|problem| {
            format!(
                "cannot build for {}: building for another target needs Zig, to compile and \
                 link C for it, and {problem}; install Zig from https://ziglang.org and put \
                 `zig` on PATH, or set {ZIG_VARIABLE} to it",
                target.triple
            )
        })?;
        Ok(Self {
            isa: cross_isa(target.triple)?,
            toolchain: Toolchain::Cross {
                target,
                zig,
                zig_version,
                pack,
            },
        })
    }

    /// Cranelift's code generator for this target.
    pub(super) fn isa(&self) -> OwnedTargetIsa {
        self.isa.clone()
    }

    /// The Rust target triple the runtime archives are built for, which the
    /// C archives' keys name.
    pub fn triple(&self) -> &'static str {
        match &self.toolchain {
            Toolchain::Host => env!("CHIMI_RUNTIME_TARGET"),
            Toolchain::Cross { target, .. } => target.triple,
        }
    }

    pub fn is_host(&self) -> bool {
        matches!(self.toolchain, Toolchain::Host)
    }

    /// Whether this target is macOS, whose symbols carry a leading `_` and
    /// whose linker spells its flags differently.
    pub(super) fn macos(&self) -> bool {
        match &self.toolchain {
            Toolchain::Host => cfg!(target_os = "macos"),
            Toolchain::Cross { target, .. } => target.macos(),
        }
    }

    /// The operating system and architecture, as `std::env::consts` names
    /// them on the target.
    pub fn os_and_arch(&self) -> (&'static str, &'static str) {
        match &self.toolchain {
            Toolchain::Host => (std::env::consts::OS, std::env::consts::ARCH),
            Toolchain::Cross { target, .. } => (target.os, target.arch),
        }
    }

    /// The C compiler, which also links, and the archiver.
    pub(super) fn c_tools(&self) -> CTools {
        match &self.toolchain {
            Toolchain::Host => CTools::host(),
            Toolchain::Cross { target, zig, .. } => CTools {
                cc: std::iter::once(zig.clone())
                    .chain(zig_cc_arguments(target).iter().map(OsString::from))
                    .collect(),
                ar: std::iter::once(zig.clone())
                    .chain(ZIG_AR_ARGUMENTS.iter().map(OsString::from))
                    .collect(),
            },
        }
    }

    /// What names the C compiler in the C archives' keys: the host `cc`'s
    /// `--version` output, or Zig's version and the target it compiles for.
    pub(super) fn c_identity(&self) -> Result<String, String> {
        match &self.toolchain {
            Toolchain::Host => command_identity(RUNTIME_C_COMPILER),
            Toolchain::Cross {
                target,
                zig_version,
                ..
            } => Ok(format!(
                "zig {zig_version} {}",
                zig_cc_arguments(target).join(" ")
            )),
        }
    }

    /// The `-l` libraries the link adds for this target beyond the program's.
    pub(super) fn link_libraries(&self) -> &'static [&'static str] {
        match &self.toolchain {
            Toolchain::Host => &[],
            Toolchain::Cross { target, .. } => target.link_libraries,
        }
    }

    /// The target's code generator and its settings, for the object keys.
    pub(super) fn fingerprint(&self) -> String {
        let mut fingerprint = format!("{}|{}", self.isa.triple(), self.isa.flags());
        for flag in self.isa.isa_flags() {
            fingerprint.push('|');
            fingerprint.push_str(&flag.to_string());
        }
        fingerprint
    }

    /// The runtime pack of a cross target; `None` for the host, whose
    /// archives the compiler finds as it always has.
    pub(super) fn pack(&self) -> Option<&RuntimePack> {
        match &self.toolchain {
            Toolchain::Host => None,
            Toolchain::Cross { pack, .. } => Some(pack),
        }
    }
}

/// The host and every other supported target, for an error naming what
/// `--target` takes.
fn supported_targets(host: &Triple) -> String {
    let others = CROSS_TARGETS
        .iter()
        .filter(|target| {
            Triple::from_str(target.triple).is_ok_and(|known| !same_platform(&known, host))
        })
        .map(|target| target.triple)
        .collect::<Vec<_>>();
    format!("its host, {host}, and, with Zig, {}", others.join(", "))
}

fn pic_flags() -> Result<settings::Flags, String> {
    let mut flags = settings::builder();
    flags
        .set("is_pic", "true")
        .map_err(|error| format!("enable position-independent native code: {error}"))?;
    Ok(settings::Flags::new(flags))
}

fn host_isa() -> Result<OwnedTargetIsa, String> {
    cranelift_native::builder()
        .map_err(str::to_owned)?
        .finish(pic_flags()?)
        .map_err(|error| format!("finish native Cranelift target: {error}"))
}

/// Cranelift's code generator for `triple` with its baseline features: an
/// executable for another machine cannot assume more than its architecture
/// guarantees, as the host's can.
fn cross_isa(triple: &str) -> Result<OwnedTargetIsa, String> {
    let parsed = Triple::from_str(triple).map_err(|error| format!("parse {triple}: {error}"))?;
    cranelift_codegen::isa::lookup(parsed)
        .map_err(|error| format!("Cranelift target {triple}: {error}"))?
        .finish(pic_flags()?)
        .map_err(|error| format!("finish Cranelift target {triple}: {error}"))
}

/// Whether two triples name the same platform: the same architecture,
/// operating system, environment and object format. The vendor is ignored
/// (`x86_64-pc-linux-gnu` is the host `x86_64-unknown-linux-gnu`), and so
/// are `darwin` against `macosx` and a macOS version.
pub(super) fn same_platform(left: &Triple, right: &Triple) -> bool {
    let system = |triple: &Triple| match triple.operating_system {
        OperatingSystem::Darwin(_) | OperatingSystem::MacOSX(_) => OperatingSystem::Darwin(None),
        other => other,
    };
    left.architecture == right.architecture
        && system(left) == system(right)
        && left.environment == right.environment
        && left.binary_format == right.binary_format
}

fn zig_version(zig: &OsString) -> Result<String, String> {
    let output = Command::new(zig)
        .arg("version")
        .output()
        .map_err(|error| format!("`{} version` did not run ({error})", zig.to_string_lossy()))?;
    if !output.status.success() {
        return Err(format!(
            "`{} version` failed: {}",
            zig.to_string_lossy(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub(super) fn command_identity(command: &str) -> Result<String, String> {
    let output = Command::new(command)
        .arg("--version")
        .output()
        .map_err(|error| format!("inspect {command} identity: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "inspect {command} identity: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

#[cfg(test)]
mod tests {
    use super::{CROSS_TARGETS, same_platform};
    use std::str::FromStr;
    use target_lexicon::Triple;

    #[test]
    fn every_cross_target_has_a_code_generator() {
        for target in &CROSS_TARGETS {
            super::cross_isa(target.triple).unwrap_or_else(|error| panic!("{error}"));
        }
    }

    #[test]
    fn the_zig_script_drops_the_target_flags_zig_rejects() {
        let directory = std::env::temp_dir().join(format!("chimi-zig-script-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let script = directory.join("cc");
        let target = &CROSS_TARGETS[3];
        std::fs::write(&script, super::super::cross_targets::zig_cc_script("echo", target)).unwrap();
        let output = std::process::Command::new("sh")
            .arg(&script)
            .args([
                "-O3",
                "--target=arm64-apple-macosx",
                "-mmacosx-version-min=11.0",
                "-c",
                "it's.c",
            ])
            .output()
            .unwrap();
        let probe = std::process::Command::new("sh")
            .arg(&script)
            .arg("-?")
            .output()
            .unwrap();
        let version = std::process::Command::new("sh")
            .arg(&script)
            .arg("--version")
            .current_dir(&directory)
            .env("TMPDIR", &directory)
            .output()
            .unwrap();
        std::fs::remove_dir_all(&directory).unwrap();
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "cc -target aarch64-macos.11.0 -g0 -O3 -c it's.c\n"
        );
        assert_eq!(probe.status.code(), Some(1), "{probe:?}");
        assert!(probe.stdout.is_empty(), "{probe:?}");
        // `--version` reaches Zig, from the temporary directory.
        assert_eq!(
            String::from_utf8(version.stdout).unwrap(),
            "cc -target aarch64-macos.11.0 -g0 --version\n"
        );
    }

    #[test]
    fn spellings_of_one_platform_are_the_same_platform() {
        let parse = |text: &str| Triple::from_str(text).unwrap();
        assert!(same_platform(
            &parse("arm64-apple-macosx14.0"),
            &parse("aarch64-apple-darwin")
        ));
        assert!(same_platform(
            &parse("aarch64-linux-gnu"),
            &parse("aarch64-unknown-linux-gnu")
        ));
        assert!(!same_platform(
            &parse("aarch64-unknown-linux-musl"),
            &parse("aarch64-unknown-linux-gnu")
        ));
        assert!(!same_platform(
            &parse("x86_64-apple-darwin"),
            &parse("aarch64-apple-darwin")
        ));
    }
}
