// The targets `chimi build --target` builds for besides the host, and how
// their C is compiled and archived. The build script (`compiler/chimi/build.rs`)
// includes this file with `include!`, so it must not name anything outside
// itself and the standard library: the runtime packs the build script makes
// for these targets and the per-project builds of `chimi build` use the same
// table and the same Zig commands.

/// A target other than the host that `chimi build --target` supports.
pub struct CrossTarget {
    /// The Rust target triple: what `--target` resolves to, what the runtime
    /// pack is built for and what the build keys name.
    pub triple: &'static str,
    /// The same target as `zig cc -target` spells it. A Linux target names
    /// glibc 2.34, the oldest the Rust runtime archives link against, so an
    /// executable runs on any glibc from 2.34; a macOS target names the
    /// oldest macOS Rust supports on both architectures.
    pub zig_target: &'static str,
    /// The operating system and architecture as Rust's `std::env::consts`
    /// names them on that target, which the build report states.
    pub os: &'static str,
    pub arch: &'static str,
    /// Libraries the link adds for the target. On Linux, Zig's libunwind:
    /// the Rust runtime archives unwind through `_Unwind_*`, which the host's
    /// `cc` takes from GCC's `libgcc_s` and Zig, which ships no `libgcc_s`,
    /// from the libunwind it builds. macOS's `libSystem` has its unwinder.
    pub link_libraries: &'static [&'static str],
}

impl CrossTarget {
    /// Whether the target is macOS: its symbols carry a leading `_` and its
    /// linker takes `-force_load`, `-dead_strip` and `-u _symbol`.
    pub fn macos(&self) -> bool {
        self.os == "macos"
    }
}

/// Linux and macOS on x86_64 and arm64. The host is one of them on a
/// supported machine; it is built with the host's own `cc` instead.
pub const CROSS_TARGETS: [CrossTarget; 4] = [
    CrossTarget {
        triple: "x86_64-unknown-linux-gnu",
        zig_target: "x86_64-linux-gnu.2.34",
        os: "linux",
        arch: "x86_64",
        link_libraries: &["unwind"],
    },
    CrossTarget {
        triple: "aarch64-unknown-linux-gnu",
        zig_target: "aarch64-linux-gnu.2.34",
        os: "linux",
        arch: "aarch64",
        link_libraries: &["unwind"],
    },
    CrossTarget {
        triple: "x86_64-apple-darwin",
        zig_target: "x86_64-macos.11.0",
        os: "macos",
        arch: "x86_64",
        link_libraries: &[],
    },
    CrossTarget {
        triple: "aarch64-apple-darwin",
        zig_target: "aarch64-macos.11.0",
        os: "macos",
        arch: "aarch64",
        link_libraries: &[],
    },
];

/// The supported cross target whose Rust triple is exactly `triple`.
// Only the build script uses this and the script below.
#[allow(dead_code)]
pub fn cross_target(triple: &str) -> Option<&'static CrossTarget> {
    CROSS_TARGETS.iter().find(|target| target.triple == triple)
}

/// The variable that names the Zig executable; `zig` on `PATH` without it.
pub const ZIG_VARIABLE: &str = "CHIMI_ZIG";

/// The Zig executable to run: `CHIMI_ZIG` when set, else `zig`.
pub fn zig_program() -> std::ffi::OsString {
    std::env::var_os(ZIG_VARIABLE)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "zig".into())
}

/// The leading arguments of the C compiler for `target` after the Zig
/// executable. `-g0` because `zig cc` keeps debug information by default and
/// the host's `cc` does not; an executable built for another target should
/// be the size the same build on that target makes.
pub fn zig_cc_arguments(target: &CrossTarget) -> [&'static str; 4] {
    ["cc", "-target", target.zig_target, "-g0"]
}

/// The leading arguments of the archiver for any cross target after the Zig
/// executable. Zig's `ar` writes the archive format and symbol index of the
/// objects it is given, for Mach-O as for ELF, which a host `ar` may not.
pub const ZIG_AR_ARGUMENTS: [&str; 1] = ["ar"];

/// A POSIX shell script that runs Zig's C compiler for `target`, for a tool
/// that takes a C compiler as one executable (the `cc` crate in the build
/// script's nested runtime builds). It drops the `--target=` and
/// `-mmacosx-version-min=` flags that tool adds in Clang's spelling of the
/// target, which `zig cc` rejects, since `-target` already names it, and
/// fails `-?`, the `cc` crate's probe for Microsoft's compiler, which `zig cc`
/// answers by writing an empty `a.o` into the current directory (the
/// fiber library's source directory).
#[allow(dead_code)]
pub fn zig_cc_script(zig: &str, target: &CrossTarget) -> String {
    let arguments = zig_cc_arguments(target)
        .iter()
        .map(|argument| shell_quote(argument))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "#!/bin/sh\n\
         for argument; do\n\
         \x20 shift\n\
         \x20 case \"$argument\" in\n\
         \x20   '-?') exit 1 ;;\n\
         \x20   --target=*|-mmacosx-version-min=*) ;;\n\
         \x20   *) set -- \"$@\" \"$argument\" ;;\n\
         \x20 esac\n\
         done\n\
         exec {} {arguments} \"$@\"\n",
        shell_quote(zig)
    )
}

#[allow(dead_code)]
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}
