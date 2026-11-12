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

use std::path::{Path, PathBuf};
use std::process::Command;

include!("src/backend/runtime_archive_key.rs");

const RUNTIME_SOURCE: &str = "runtime/native/runtime.c";
const RUNTIME_HEADER: &str = "runtime/native/runtime.h";
const FIBER_HEADER: &str = "../../runtime/chimi-fiber/include/chimi_fiber.h";

fn main() {
    for path in [
        "build.rs",
        "src/backend/runtime_archive_key.rs",
        RUNTIME_SOURCE,
        RUNTIME_HEADER,
        FIBER_HEADER,
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    let target = std::env::var("TARGET").expect("cargo sets TARGET");
    println!("cargo:rustc-env=CHIMI_RUNTIME_TARGET={target}");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
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

/// Returns `None` when this build does not prebuild (by design, not failure).
fn prebuild(
    out: &Path,
    target: &str,
    name: &str,
    process_kernel: bool,
) -> Result<Option<(Vec<u8>, String)>, String> {
    if std::env::var_os("CARGO_FEATURE_NATIVE_BACKEND").is_none()
        || std::env::var("HOST").ok().as_deref() != Some(target)
    {
        return Ok(None);
    }
    let read = |path: &str| std::fs::read(path).map_err(|error| format!("read {path}: {error}"));
    let runtime_source = read(RUNTIME_SOURCE)?;
    let runtime_header = read(RUNTIME_HEADER)?;
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
