// How the native runtime archive is compiled and identified. The build script
// (`compiler/chimi/build.rs`) includes this file with `include!`, so it must not
// name anything outside itself and `blake3`: the prebuilt archive and the
// per-project fallback are compiled with the same flags and keyed by the same
// function, which is what lets the compiler tell whether its prebuilt archive
// was built from the runtime sources it embeds.

/// The C compiler command and flags that build `runtime.c` into an object.
/// Both the build script and `chimi build` pass exactly these, followed by
/// `-I <dir> -c <dir>/runtime.c -o <object>`.
pub const RUNTIME_C_COMPILER: &str = "cc";
pub const RUNTIME_C_FLAGS: [&str; 2] = ["-std=c11", "-O2"];

/// Added to `RUNTIME_C_FLAGS` for the process-kernel variant of the runtime,
/// which only programs that reach `std/process` or `std/supervisor` link
/// (pay-for-play; supervised-process-conformance.md, MF06).
pub const PROCESS_KERNEL_FLAG: &str = "-DCHIMI_PROCESS_KERNEL=1";

/// The C flags of one runtime variant.
pub fn runtime_c_flags(process_kernel: bool) -> Vec<&'static str> {
    let mut flags = RUNTIME_C_FLAGS.to_vec();
    if process_kernel {
        flags.push(PROCESS_KERNEL_FLAG);
    }
    flags
}

/// The inputs that decide the bytes of the runtime archive.
pub struct RuntimeArchiveInputs<'a> {
    /// `CHIMI_RUNTIME_ABI_VERSION` as the compiler states it.
    pub abi_version: &'a str,
    /// The Rust target triple the compiler was built for.
    pub target_triple: &'a str,
    /// `cc --version` output (stdout then stderr).
    pub compiler_identity: &'a str,
    pub runtime_source: &'a [u8],
    pub runtime_header: &'a [u8],
    /// `chimi_binding.h`, which `runtime.h` includes.
    pub binding_header: &'a [u8],
    pub fiber_header: &'a [u8],
    /// The process-kernel variant (`PROCESS_KERNEL_FLAG`).
    pub process_kernel: bool,
}

/// Ensures: equal keys only for equal inputs (each part is length-prefixed,
/// so moving bytes between parts changes the key).
pub fn runtime_archive_key(inputs: &RuntimeArchiveInputs<'_>) -> String {
    let flags = runtime_c_flags(inputs.process_kernel).join(" ");
    let parts: [&[u8]; 11] = [
        b"chimi.runtime-archive.v1",
        RUNTIME_C_COMPILER.as_bytes(),
        flags.as_bytes(),
        b"ar:rcs",
        inputs.abi_version.as_bytes(),
        inputs.target_triple.as_bytes(),
        inputs.compiler_identity.as_bytes(),
        inputs.runtime_header,
        inputs.binding_header,
        inputs.fiber_header,
        inputs.runtime_source,
    ];
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    hasher.finalize().to_hex().to_string()
}

/// The inputs that decide the bytes of one binding package's archive (G20).
// The build script includes this file and prebuilds no binding archive.
#[allow(dead_code)]
pub struct BindingArchiveInputs<'a> {
    pub package: &'a str,
    pub target_triple: &'a str,
    /// `cc --version` output (stdout then stderr).
    pub compiler_identity: &'a str,
    /// `chimi_binding.h`, the only header a binding includes from the runtime.
    pub binding_header: &'a [u8],
    /// Each source file's name and text, in the order they are compiled.
    pub sources: &'a [(&'a str, &'a str)],
}

/// Ensures: equal keys only for equal inputs, as [`runtime_archive_key`].
#[allow(dead_code)]
pub fn binding_archive_key(inputs: &BindingArchiveInputs<'_>) -> String {
    let flags = RUNTIME_C_FLAGS.join(" ");
    let mut hasher = blake3::Hasher::new();
    let mut part = |bytes: &[u8]| {
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    };
    for fixed in [
        b"chimi.binding-archive.v1".as_slice(),
        RUNTIME_C_COMPILER.as_bytes(),
        flags.as_bytes(),
        b"ar:rcs",
        inputs.package.as_bytes(),
        inputs.target_triple.as_bytes(),
        inputs.compiler_identity.as_bytes(),
        inputs.binding_header,
    ] {
        part(fixed);
    }
    for (name, text) in inputs.sources {
        part(name.as_bytes());
        part(text.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}
