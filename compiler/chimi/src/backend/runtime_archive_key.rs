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

/// The C compiler and archiver of one build, each a program and its leading
/// arguments: the host's `cc` and `ar`, or Zig's for another target
/// (`cross_targets.rs`). The keys below name the compiler by its identity
/// (`--version` output and, for another target, its target), so archives
/// from different tools never share a key.
pub struct CTools {
    pub cc: Vec<std::ffi::OsString>,
    pub ar: Vec<std::ffi::OsString>,
}

impl CTools {
    pub fn host() -> Self {
        Self {
            cc: vec![RUNTIME_C_COMPILER.into()],
            ar: vec!["ar".into()],
        }
    }

    /// A command running the C compiler, ready for its flags.
    pub fn cc_command(&self) -> std::process::Command {
        tool_command(&self.cc)
    }

    /// A command running the archiver, ready for `rcs <archive> <objects>`.
    pub fn ar_command(&self) -> std::process::Command {
        tool_command(&self.ar)
    }
}

fn tool_command(tool: &[std::ffi::OsString]) -> std::process::Command {
    let (program, arguments) = tool.split_first().expect("a tool names its program");
    let mut command = std::process::Command::new(program);
    command.args(arguments);
    command
}

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

/// The C flags a binding package adds to `RUNTIME_C_FLAGS`, after the
/// `-I` of its own directory: only `-I<directory>` (inside the package
/// directory, since only the package's files are in its key; the compile
/// makes it absolute) and `-D<definition>`, which a
/// test checks, so no flag resolves against the current directory. A vendored library keeps its upstream layout
/// and is configured here rather than by editing it.
pub const BINDING_PACKAGE_FLAGS: &[(&str, &[&str])] = &[
    ("probe", &["-Iinclude", "-DPROBE_PACKAGE_FLAGS=1"]),
    // mbedTLS 4.1 with its default configuration, laid out as upstream ships
    // it (bindings/tls/mbedtls/PROVENANCE.md).
    (
        "tls",
        &[
            "-Imbedtls/include",
            "-Imbedtls/library",
            "-Imbedtls/tf-psa-crypto/include",
            "-Imbedtls/tf-psa-crypto/drivers/builtin/include",
            "-Imbedtls/tf-psa-crypto/drivers/builtin/src",
            "-Imbedtls/tf-psa-crypto/core",
            "-Imbedtls/tf-psa-crypto/dispatch",
            "-Imbedtls/tf-psa-crypto/utilities",
            "-Imbedtls/tf-psa-crypto/platform",
            "-Imbedtls/tf-psa-crypto/extras",
        ],
    ),
    // SQLite 3.54 with the options SQLite recommends for an embedded build
    // (bindings/sqlite/sqlite/PROVENANCE.md). The feature macros expose the
    // POSIX calls its file layer uses under -std=c11.
    (
        "sqlite",
        &[
            "-D_DEFAULT_SOURCE",
            "-D_DARWIN_C_SOURCE",
            "-DSQLITE_THREADSAFE=1",
            "-DSQLITE_DQS=0",
            "-DSQLITE_DEFAULT_MEMSTATUS=0",
            "-DSQLITE_LIKE_DOESNT_MATCH_BLOBS",
            "-DSQLITE_OMIT_DEPRECATED",
            "-DSQLITE_OMIT_LOAD_EXTENSION",
            "-DSQLITE_OMIT_SHARED_CACHE",
            "-DSQLITE_USE_ALLOCA",
        ],
    ),
];

/// The extra flags of `package`, in order; empty for most packages.
pub fn binding_package_flags(package: &str) -> &'static [&'static str] {
    BINDING_PACKAGE_FLAGS
        .iter()
        .find(|(name, _)| *name == package)
        .map_or(&[], |(_, flags)| flags)
}

/// The inputs that decide the bytes of one binding package's archive (G20).
// The build script uses only some of these items.
#[allow(dead_code)]
pub struct BindingArchiveInputs<'a> {
    pub package: &'a str,
    pub target_triple: &'a str,
    /// `cc --version` output (stdout then stderr).
    pub compiler_identity: &'a str,
    /// `chimi_binding.h`, the only header a binding includes from the runtime.
    pub binding_header: &'a [u8],
    /// The package's own flags, `binding_package_flags(package)`.
    pub package_flags: &'a [&'a str],
    /// Each file's path in its package and text, in path order.
    pub sources: &'a [(&'a str, &'a str)],
}

/// Ensures: equal keys only for equal inputs, as [`runtime_archive_key`],
/// including the package's own flags.
#[allow(dead_code)]
pub fn binding_archive_key(inputs: &BindingArchiveInputs<'_>) -> String {
    let flags = RUNTIME_C_FLAGS.join(" ");
    let mut hasher = blake3::Hasher::new();
    let mut part = |bytes: &[u8]| {
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    };
    for fixed in [
        b"chimi.binding-archive.v2".as_slice(),
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
    part(&(inputs.package_flags.len() as u64).to_le_bytes());
    for flag in inputs.package_flags {
        part(flag.as_bytes());
    }
    for (name, text) in inputs.sources {
        part(name.as_bytes());
        part(text.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

/// Compiles one binding package into the archive `archive`, working in
/// `directory`: writes `sources` there at their package paths, compiles each
/// `.c` file (several at once) with the package's flags and `-I
/// header_directory` for `chimi_binding.h`, and archives the objects in path
/// order. The build script and `chimi build` both call this, so a prebuilt
/// archive and a per-project one come from the same commands. `extra_flags`
/// follow `RUNTIME_C_FLAGS`; only a test build of `chimi build` passes any.
#[allow(dead_code)]
pub fn compile_binding_archive(
    package: &str,
    sources: &[(&str, &str)],
    header_directory: &std::path::Path,
    directory: &std::path::Path,
    archive: &std::path::Path,
    extra_flags: &[String],
    tools: &CTools,
) -> Result<(), String> {
    // A fresh directory, so a file removed from the package is not compiled.
    let _ = std::fs::remove_dir_all(directory);
    // On a case-insensitive filesystem the second write would replace the first.
    let mut names = std::collections::BTreeSet::new();
    for (name, _) in sources {
        if !names.insert(name.to_lowercase()) {
            return Err(format!(
                "binding package {package}: two sources differ only in case: {name}"
            ));
        }
    }
    for (name, text) in sources {
        let path = directory.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("create {}: {error}", parent.display()))?;
        }
        std::fs::write(&path, text)
            .map_err(|error| format!("write binding source {name}: {error}"))?;
    }
    let objects_directory = directory.join("objects");
    std::fs::create_dir_all(&objects_directory)
        .map_err(|error| format!("create {}: {error}", objects_directory.display()))?;
    let units: Vec<(std::path::PathBuf, std::path::PathBuf)> = sources
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| name.ends_with(".c"))
        .map(|name| {
            (
                directory.join(name),
                objects_directory.join(format!("{}.o", name.replace('/', "__"))),
            )
        })
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    for (_, object) in &units {
        let folded = object.to_string_lossy().to_lowercase();
        if !seen.insert(folded) {
            return Err(format!(
                "binding package {package}: two sources map to the object {}",
                object.display()
            ));
        }
    }
    let workers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(units.len().max(1));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let failure = std::sync::Mutex::new(None::<String>);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some((source, object)) = units.get(index) else {
                        break;
                    };
                    if failure.lock().map_or(true, |seen| seen.is_some()) {
                        break;
                    }
                    let output = tools
                        .cc_command()
                        .args(RUNTIME_C_FLAGS)
                        .args(extra_flags)
                        .arg("-I")
                        .arg(header_directory)
                        .arg("-I")
                        .arg(directory)
                        .args(binding_package_flags(package).iter().map(|flag| {
                            match flag.strip_prefix("-I") {
                                Some(relative) => {
                                    format!("-I{}", directory.join(relative).display())
                                }
                                None => (*flag).to_owned(),
                            }
                        }))
                        .arg("-c")
                        .arg(source)
                        .arg("-o")
                        .arg(object)
                        .output();
                    let problem = match output {
                        Ok(output) if output.status.success() => None,
                        Ok(output) => Some(format!(
                            "compile {}: {}",
                            source.display(),
                            String::from_utf8_lossy(&output.stderr).trim()
                        )),
                        Err(error) => Some(format!("run {:?}: {error}", tools.cc)),
                    };
                    if let Some(problem) = problem {
                        if let Ok(mut seen) = failure.lock() {
                            seen.get_or_insert(problem);
                        }
                        break;
                    }
                }
            });
        }
    });
    if let Some(problem) = failure.into_inner().ok().flatten() {
        return Err(problem);
    }
    let _ = std::fs::remove_file(archive);
    let output = tools
        .ar_command()
        .arg("rcs")
        .arg(archive)
        .args(units.iter().map(|(_, object)| object))
        .output()
        .map_err(|error| format!("run ar: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "archive binding package {package}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}
