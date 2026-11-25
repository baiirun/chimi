//! Prebuilds the native runtime archive once per compiler build, so `chimi build`
//! does not compile `runtime.c` per project (it dominated a cold build).
//!
//! Writes `$OUT_DIR/libchimi_rt.a` and `$OUT_DIR/libchimi_rt.key`, and the
//! process-kernel variant `libchimi_rt_process.a` and `libchimi_rt_process.key`
//! (`PROCESS_KERNEL_FLAG`), which the compiler embeds. The key is computed from the exact bytes compiled here; the
//! compiler recomputes it from the sources it embeds and its current `cc`, and
//! uses the prebuilt archive only when the two agree. When no archive can be
//! built here (feature off, cross-compiling, no `cc`, a compile error), both
//! files are empty and `chimi build` compiles the runtime into its per-project
//! cache as before. This script never fails the cargo build.
//!
//! It also builds the Rust fiber runtime's static library
//! (`runtime/chimi-fiber`) with a nested `cargo build` and writes it as
//! `$OUT_DIR/libchimi_fiber.a`, which the compiler embeds, so an installed
//! `chimi` links process programs without a Cargo target directory beside it
//! (CHI-314). Cargo cannot hand a dependency's staticlib to a build script on
//! stable, hence the nested build; it uses its own target directory so it
//! never waits on the outer build's lock. When it cannot build, the file is
//! empty and the compiler falls back to looking beside its executable.
//!
//! The Rust runtime archive of a program without the scheduler
//! (`runtime/chimi-runtime-standalone`) is built the same way, as
//! `$OUT_DIR/libchimi_runtime_standalone.a`. Every native program links one
//! of the two Rust archives, so `chimi build` needs this one whenever it
//! does not link the fiber archive.

use std::path::{Path, PathBuf};
use std::process::Command;

include!("src/backend/runtime_archive_key.rs");

const RUNTIME_SOURCE: &str = "runtime/native/runtime.c";
const RUNTIME_HEADER: &str = "runtime/native/runtime.h";
const BINDING_HEADER: &str = "runtime/native/chimi_binding.h";
const FIBER_HEADER: &str = "../../runtime/chimi-fiber/include/chimi_fiber.h";

fn main() {
    for path in [
        "build.rs",
        "src/backend/runtime_archive_key.rs",
        RUNTIME_SOURCE,
        RUNTIME_HEADER,
        BINDING_HEADER,
        FIBER_HEADER,
        "../../runtime/chimi-fiber/Cargo.toml",
        "../../runtime/chimi-fiber/build.rs",
        "../../runtime/chimi-fiber/src",
        "../../runtime/chimi-fiber/include",
        "../../runtime/chimi-fiber/tests/generated_fixture.c",
        "../../runtime/chimi-runtime/Cargo.toml",
        "../../runtime/chimi-runtime/src",
        "../../runtime/chimi-runtime-standalone/Cargo.toml",
        "../../runtime/chimi-runtime-standalone/src",
        "../../Cargo.toml",
        "../../Cargo.lock",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    embed_standard_tree();
    let binding_sources = embed_binding_sources();
    let target = std::env::var("TARGET").expect("cargo sets TARGET");
    println!("cargo:rustc-env=CHIMI_RUNTIME_TARGET={target}");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    prebuild_bindings(&out, &target, &binding_sources);
    let fiber = if prebuilds(&target) {
        prebuild_fiber_runtime(&out, &target).unwrap_or_else(|error| {
            println!(
                "cargo:warning=fiber runtime not embedded ({error}); chimi build will look for it beside the compiler"
            );
            Vec::new()
        })
    } else {
        Vec::new()
    };
    std::fs::write(out.join("libchimi_fiber.a"), fiber).expect("write embedded fiber runtime");
    // Unlike the C archives, this needs no host C compiler for the target,
    // only Rust's standard library for it, so a cross-compiled compiler
    // carries it too.
    let standalone = if std::env::var_os("CARGO_FEATURE_NATIVE_BACKEND").is_some() {
        prebuild_standalone_runtime(&out, &target).unwrap_or_else(|error| {
            println!(
                "cargo:warning=Rust runtime archive not embedded ({error}); chimi build will look for it beside the compiler"
            );
            Vec::new()
        })
    } else {
        Vec::new()
    };
    let standalone_path = out.join("libchimi_runtime_standalone.a");
    std::fs::write(&standalone_path, standalone).expect("write embedded Rust runtime archive");
    // For the tests that link C fixtures against the runtime by hand.
    println!(
        "cargo:rustc-env=CHIMI_BUILT_STANDALONE_RUNTIME_ARCHIVE={}",
        standalone_path.display()
    );
    for (name, process_kernel) in [("libchimi_rt", false), ("libchimi_rt_process", true)] {
        let (bytes, key) = match prebuild(&out, &target, name, process_kernel) {
            Ok(Some(built)) => built,
            Ok(None) => (Vec::new(), String::new()),
            Err(error) => {
                println!(
                    "cargo:warning=native runtime {name} not prebuilt ({error}); chimi build will compile it per project"
                );
                (Vec::new(), String::new())
            }
        };
        std::fs::write(out.join(format!("{name}.a")), bytes)
            .expect("write prebuilt runtime archive");
        std::fs::write(out.join(format!("{name}.key")), key).expect("write prebuilt runtime key");
    }
}

/// Writes `$OUT_DIR/standard_tree.rs`: every `.chimi` file under `std/`, as
/// `(module path, text)` pairs in ascending module path, so the compiler embeds the
/// whole tree and a new file needs no registration.
fn embed_standard_tree() {
    println!("cargo:rerun-if-changed=std");
    let root = Path::new("std");
    let mut files = Vec::new();
    collect_standard_files(root, &mut files);
    files.sort();
    let mut generated = String::from("pub(crate) const EMBEDDED_MODULES: &[(&str, &str)] = &[\n");
    for file in &files {
        let relative = file
            .strip_prefix(root)
            .expect("collected files are under std/")
            .with_extension("");
        let module = format!(
            "std/{}",
            relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        );
        let absolute = std::fs::canonicalize(file).expect("canonicalize a std source file");
        generated.push_str(&format!(
            "    ({module:?}, include_str!({:?})),\n",
            absolute.to_string_lossy()
        ));
    }
    generated.push_str("];\n");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    std::fs::write(out.join("standard_tree.rs"), generated).expect("write standard_tree.rs");
}

/// Writes `$OUT_DIR/binding_sources.rs`: every `.c` and `.h` file of each binding
/// package under `runtime/native/bindings/<package>/`, at any depth, as
/// `(package, path in the package, text)` triples in ascending order, so a new
/// file needs no registration and a vendored library keeps its layout.
/// Returns the same triples with the bytes read, for `prebuild_bindings`.
fn embed_binding_sources() -> Vec<(String, String, String)> {
    let root = Path::new("runtime/native/bindings");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    if let Ok(packages) = std::fs::read_dir(root) {
        for package in packages {
            let package = package.expect("read a binding package entry").path();
            if package.is_dir() {
                collect_binding_files(&package, &mut files);
            }
        }
    }
    files.sort();
    let mut generated =
        String::from("pub(crate) const BINDING_SOURCES: &[(&str, &str, &str)] = &[\n");
    let mut embedded = Vec::new();
    for file in &files {
        let relative = file
            .strip_prefix(root)
            .expect("collected under the bindings root");
        let mut parts = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned());
        let package = parts.next().expect("a binding source sits in a package");
        let name = parts.collect::<Vec<_>>().join("/");
        let absolute = std::fs::canonicalize(file).expect("canonicalize a binding source");
        generated.push_str(&format!(
            "    ({package:?}, {name:?}, include_str!({:?})),\n",
            absolute.to_string_lossy()
        ));
        let text = std::fs::read_to_string(file).expect("read a binding source as UTF-8");
        embedded.push((package, name, text));
    }
    generated.push_str("];\n");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    std::fs::write(out.join("binding_sources.rs"), generated).expect("write binding_sources.rs");
    embedded
}

fn collect_binding_files(directory: &Path, files: &mut Vec<PathBuf>) {
    println!("cargo:rerun-if-changed={}", directory.display());
    let entries = std::fs::read_dir(directory).expect("read a binding package directory");
    for entry in entries {
        let path = entry.expect("read a binding source entry").path();
        if path.is_dir() {
            collect_binding_files(&path, files);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "c" || extension == "h")
        {
            println!("cargo:rerun-if-changed={}", path.display());
            files.push(path);
        }
    }
}

/// Writes `$OUT_DIR/prebuilt_bindings.rs`: each binding package compiled from
/// the bytes `embed_binding_sources` embedded, with its key, so `chimi build`
/// compiles no package per project. A package that cannot be built here is
/// left out with a warning, and `chimi build` compiles it as before.
fn prebuild_bindings(out: &Path, target: &str, sources: &[(String, String, String)]) {
    let mut generated = String::from("const PREBUILT_BINDINGS: &[(&str, &str, &[u8])] = &[\n");
    if prebuilds(target) {
        let mut packages = sources
            .iter()
            .map(|(package, _, _)| package.as_str())
            .collect::<Vec<_>>();
        packages.dedup();
        for package in packages {
            match prebuild_binding(out, target, package, sources) {
                Ok((archive, key)) => generated.push_str(&format!(
                    "    ({package:?}, {key:?}, include_bytes!({:?})),\n",
                    archive.to_string_lossy()
                )),
                Err(error) => println!(
                    "cargo:warning=binding package {package} not prebuilt ({}); chimi build will compile it per project",
                    error.split_whitespace().collect::<Vec<_>>().join(" ")
                ),
            }
        }
    }
    generated.push_str("];\n");
    std::fs::write(out.join("prebuilt_bindings.rs"), generated)
        .expect("write prebuilt_bindings.rs");
}

fn prebuild_binding(
    out: &Path,
    target: &str,
    package: &str,
    sources: &[(String, String, String)],
) -> Result<(PathBuf, String), String> {
    let files = sources
        .iter()
        .filter(|(name, _, _)| name == package)
        .map(|(_, path, text)| (path.as_str(), text.as_str()))
        .collect::<Vec<_>>();
    let binding_header =
        std::fs::read(BINDING_HEADER).map_err(|error| format!("read {BINDING_HEADER}: {error}"))?;
    let directory = out.join(format!("binding-{package}"));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("create {directory:?}: {error}"))?;
    std::fs::write(directory.join("chimi_binding.h"), &binding_header)
        .map_err(|error| format!("write chimi_binding.h: {error}"))?;
    let archive = out.join(format!("libchimi_bind_{package}.a"));
    compile_binding_archive(
        package,
        &files,
        &directory,
        &directory.join("package"),
        &archive,
        &[],
    )?;
    let key = binding_archive_key(&BindingArchiveInputs {
        package,
        package_flags: binding_package_flags(package),
        target_triple: target,
        compiler_identity: &compiler_identity()?,
        binding_header: &binding_header,
        sources: &files,
    });
    Ok((archive, key))
}

/// Builds `chimi-fiber`'s static library for `target` in the outer build's
/// profile and returns its bytes.
fn prebuild_fiber_runtime(out: &Path, target: &str) -> Result<Vec<u8>, String> {
    let release = std::env::var("PROFILE").ok().as_deref() == Some("release");
    let mut arguments = vec!["build", "-p", "chimi-fiber", "--lib"];
    if release {
        arguments.push("--release");
    }
    nested_runtime_build(
        out,
        target,
        &arguments,
        if release { "release" } else { "debug" },
        "libchimi_fiber.a",
    )
}

/// Builds `chimi-runtime-standalone` as a static library for `target`, always
/// optimized and aborting on panic (the `runtime-standalone` profile), and
/// returns the bytes of an archive holding only its own object.
///
/// Rust's archive also carries `compiler_builtins`: Rust's copies of the C
/// toolchain's helper routines (`__modti3` and the like) and of libm. Linked
/// before the toolchain's own, they would replace them for `runtime.c` too,
/// and they refer to Rust's unwinding personality, which a `no_std` archive
/// lacks. With LTO everything the Rust code calls is in its one object,
/// which needs nothing from them, so they are left out and the toolchain's
/// libraries serve both languages.
fn prebuild_standalone_runtime(out: &Path, target: &str) -> Result<Vec<u8>, String> {
    let full = nested_runtime_build(
        out,
        target,
        &[
            "rustc",
            "-p",
            "chimi-runtime-standalone",
            "--lib",
            "--profile",
            "runtime-standalone",
            "--crate-type",
            "staticlib",
        ],
        "runtime-standalone",
        "libchimi_runtime_standalone.a",
    )?;
    let directory = out.join("runtime-standalone-objects");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("create {}: {error}", directory.display()))?;
    let full_path = directory.join("full.a");
    std::fs::write(&full_path, full).map_err(|error| format!("write full.a: {error}"))?;
    let listing = Command::new("ar")
        .arg("t")
        .arg(&full_path)
        .output()
        .map_err(|error| format!("run ar t: {error}"))?;
    if !listing.status.success() {
        return Err(format!(
            "ar t: {}",
            String::from_utf8_lossy(&listing.stderr)
        ));
    }
    let members = String::from_utf8_lossy(&listing.stdout)
        .lines()
        .filter(|name| name.starts_with("chimi_runtime_standalone-"))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if members.len() != 1 {
        return Err(format!(
            "expected one chimi_runtime_standalone object in the archive, found {members:?}"
        ));
    }
    run(Command::new("ar")
        .current_dir(&directory)
        .arg("x")
        .arg("full.a")
        .args(&members))?;
    let archive = directory.join("libchimi_runtime_standalone.a");
    run(Command::new("ar")
        .current_dir(&directory)
        .arg("rcs")
        .arg(&archive)
        .args(&members))?;
    std::fs::read(&archive).map_err(|error| format!("read {}: {error}", archive.display()))
}

/// Runs a nested `cargo` with `arguments` for `target` and returns the bytes
/// of `archive` from its `profile_directory`. The nested target directory
/// sits beside the outer profile's (`<target-dir>/<profile>/chimi-fiber-runtime`),
/// so it stays warm across builds of the compiler.
fn nested_runtime_build(
    out: &Path,
    target: &str,
    arguments: &[&str],
    profile_directory_name: &str,
    archive: &str,
) -> Result<Vec<u8>, String> {
    let profile_directory = out
        .ancestors()
        .nth(3)
        .ok_or_else(|| format!("OUT_DIR {} has no profile directory", out.display()))?;
    let target_directory = profile_directory.join("chimi-fiber-runtime");
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .current_dir(&workspace)
        .args(arguments)
        .args(["--quiet", "--offline"])
        .arg("--target")
        .arg(target)
        .arg("--target-dir")
        .arg(&target_directory)
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET");
    if workspace.join("Cargo.lock").is_file() {
        command.arg("--locked");
    }
    run(&mut command)?;
    let archive = target_directory
        .join(target)
        .join(profile_directory_name)
        .join(archive);
    std::fs::read(&archive).map_err(|error| format!("read {}: {error}", archive.display()))
}

/// Whether this build prebuilds native archives at all (by design, not failure).
fn prebuilds(target: &str) -> bool {
    std::env::var_os("CARGO_FEATURE_NATIVE_BACKEND").is_some()
        && std::env::var("HOST").ok().as_deref() == Some(target)
}

fn collect_standard_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries {
        let path = entry.expect("read a std directory entry").path();
        if path.is_dir() {
            println!("cargo:rerun-if-changed={}", path.display());
            collect_standard_files(&path, files);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "chimi")
        {
            println!("cargo:rerun-if-changed={}", path.display());
            files.push(path);
        }
    }
}

/// Returns `None` when this build does not prebuild (by design, not failure).
fn prebuild(
    out: &Path,
    target: &str,
    name: &str,
    process_kernel: bool,
) -> Result<Option<(Vec<u8>, String)>, String> {
    if !prebuilds(target) {
        return Ok(None);
    }
    let read = |path: &str| std::fs::read(path).map_err(|error| format!("read {path}: {error}"));
    let runtime_source = read(RUNTIME_SOURCE)?;
    let runtime_header = read(RUNTIME_HEADER)?;
    let binding_header = read(BINDING_HEADER)?;
    let fiber_header = read(FIBER_HEADER)?;
    // Compile the bytes that are hashed, not the files again: the sources may
    // change while this script runs.
    let directory = out.join(name);
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("create {directory:?}: {error}"))?;
    let write = |name: &str, bytes: &[u8]| {
        std::fs::write(directory.join(name), bytes)
            .map_err(|error| format!("write {name}: {error}"))
    };
    write("runtime.c", &runtime_source)?;
    write("runtime.h", &runtime_header)?;
    write("chimi_binding.h", &binding_header)?;
    write("chimi_fiber.h", &fiber_header)?;
    let abi_version = abi_version(&runtime_header)?;
    let compiler_identity = compiler_identity()?;
    let object = directory.join("runtime.o");
    let archive = directory.join(format!("{name}.a"));
    let _ = std::fs::remove_file(&archive);
    run(Command::new(RUNTIME_C_COMPILER)
        .args(runtime_c_flags(process_kernel))
        .arg("-I")
        .arg(&directory)
        .arg("-c")
        .arg(directory.join("runtime.c"))
        .arg("-o")
        .arg(&object))?;
    run(Command::new("ar").arg("rcs").arg(&archive).arg(&object))?;
    let bytes = std::fs::read(&archive).map_err(|error| format!("read archive: {error}"))?;
    let key = runtime_archive_key(&RuntimeArchiveInputs {
        abi_version: &abi_version,
        target_triple: target,
        compiler_identity: &compiler_identity,
        runtime_source: &runtime_source,
        runtime_header: &runtime_header,
        binding_header: &binding_header,
        fiber_header: &fiber_header,
        process_kernel,
    });
    Ok(Some((bytes, key)))
}

fn abi_version(header: &[u8]) -> Result<String, String> {
    String::from_utf8_lossy(header)
        .lines()
        .find_map(|line| line.strip_prefix("#define CHIMI_RUNTIME_ABI_VERSION "))
        .map(|value| value.trim().to_owned())
        .ok_or_else(|| "runtime.h does not define CHIMI_RUNTIME_ABI_VERSION".to_owned())
}

/// Must match `command_identity` in `src/backend/native.rs`.
fn compiler_identity() -> Result<String, String> {
    let output = Command::new(RUNTIME_C_COMPILER)
        .arg("--version")
        .output()
        .map_err(|error| format!("run {RUNTIME_C_COMPILER} --version: {error}"))?;
    if !output.status.success() {
        return Err(format!("{RUNTIME_C_COMPILER} --version failed"));
    }
    Ok(format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

fn run(command: &mut Command) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("run {command:?}: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{command:?}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}
