//! PROTOTYPE: disposable host-only embedded-bytecode candidate for M8.

use super::BackendProgram;
use super::prototype_program_data::render_program_data;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

pub struct BytecodePrototypeArtifact {
    pub image_source_path: PathBuf,
    pub image_object_path: PathBuf,
    pub runtime_archive_path: PathBuf,
    pub executable_path: PathBuf,
    pub image_encoding: Duration,
    pub runtime_compilation: Duration,
    pub image_compilation: Duration,
    pub linking: Duration,
}

pub fn build_executable(
    program: &BackendProgram,
    output_directory: &Path,
) -> Result<BytecodePrototypeArtifact, String> {
    std::fs::create_dir_all(output_directory)
        .map_err(|error| format!("create bytecode prototype output directory: {error}"))?;
    let image_source_path = output_directory.join("program_image.c");
    let image_object_path = output_directory.join("program_image.o");
    let runtime_object_path = output_directory.join("runtime.o");
    let vm_object_path = output_directory.join("vm.o");
    let runtime_archive_path = output_directory.join("libchimi_bytecode_rt.a");
    let executable_path = output_directory.join("program");
    let runtime_directory = native_runtime_directory();
    let vm_directory = bytecode_prototype_directory();

    let started = Instant::now();
    let image = render_program_data(&program.core, true);
    std::fs::write(&image_source_path, image)
        .map_err(|error| format!("write prototype bytecode image: {error}"))?;
    let image_encoding = started.elapsed();

    let started = Instant::now();
    compile_c(
        runtime_directory.join("runtime.c"),
        &runtime_object_path,
        &runtime_directory,
        "compile bytecode prototype runtime",
    )?;
    compile_c(
        vm_directory.join("vm.c"),
        &vm_object_path,
        &runtime_directory,
        "compile bytecode prototype VM",
    )?;
    run(
        Command::new("ar")
            .arg("rcs")
            .arg(&runtime_archive_path)
            .arg(&runtime_object_path)
            .arg(&vm_object_path),
        "archive bytecode prototype runtime",
    )?;
    let runtime_compilation = started.elapsed();

    let started = Instant::now();
    compile_c(
        image_source_path.clone(),
        &image_object_path,
        &runtime_directory,
        "compile bytecode prototype image",
    )?;
    let image_compilation = started.elapsed();

    let started = Instant::now();
    run(
        Command::new("cc")
            .arg(&image_object_path)
            .arg(&runtime_archive_path)
            .arg("-o")
            .arg(&executable_path),
        "link bytecode prototype executable",
    )?;
    let linking = started.elapsed();

    Ok(BytecodePrototypeArtifact {
        image_source_path,
        image_object_path,
        runtime_archive_path,
        executable_path,
        image_encoding,
        runtime_compilation,
        image_compilation,
        linking,
    })
}

fn native_runtime_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../prototypes/native-aot")
}

fn bytecode_prototype_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../prototypes/embedded-bytecode")
}

fn compile_c(
    source: PathBuf,
    object: &Path,
    include_directory: &Path,
    operation: &str,
) -> Result<(), String> {
    run(
        Command::new("cc")
            .arg("-std=c11")
            .arg("-O2")
            .arg("-I")
            .arg(include_directory)
            .arg("-c")
            .arg(source)
            .arg("-o")
            .arg(object),
        operation,
    )
}

fn run(command: &mut Command, operation: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("{operation}: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{operation}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}
