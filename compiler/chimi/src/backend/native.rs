//! Native AOT object emission and host linking behind the backend-neutral program boundary.

use super::BackendProgram;
use super::bindings::{BINDING_HEADER, BindingPackage, packages_for};
use super::direct_loop::{Pass, direct_loops, emit_pass};
use super::program_data::{render_program_data, requires_process_runtime};
use super::runtime_archive_key::{
    BindingArchiveInputs, RUNTIME_C_COMPILER, RuntimeArchiveInputs, binding_archive_key,
    binding_package_flags, compile_binding_archive, runtime_archive_key, runtime_c_flags,
};
use crate::executable::{CoreOperation, CoreProgram};
use crate::json::write_json_string;
use crate::semantic::{BinaryOperator, UnaryOperator};
use cranelift_codegen::Context;
use cranelift_codegen::ir::{
    AbiParam, Function, InstBuilder, Signature, UserFuncName, Value, types,
};
use cranelift_codegen::isa::OwnedTargetIsa;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{FuncId, Linkage, Module, default_libcall_names};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const CACHE_SCHEMA: &str = "chimi.native-cache.v1";
const CODEGEN_IDENTITY: &str = "cranelift-0.131.3";
const CODEGEN_SETTINGS: &str = "is_pic=true;debug=false";
const C_SETTINGS: &str = "cc:-std=c11,-O2;debug=false";
const NATIVE_LOWERING_REVISION: &str = "fiber-runtime-abi-v5-process-kernel;direct-integer-loop-v1";
const RUNTIME_ABI_VERSION: u32 = 57;
const FIBER_RUNTIME_ABI_VERSION: u32 = chimi_fiber::ABI_VERSION;
static CACHE_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub struct NativeArtifact {
    pub object_paths: Vec<PathBuf>,
    pub data_object_path: PathBuf,
    pub runtime_archive_path: PathBuf,
    pub fiber_runtime_archive_path: Option<PathBuf>,
    /// The C binding packages the program reaches (G20), each linked from its
    /// own archive.
    pub binding_packages: Vec<LinkedBindingPackage>,
    pub executable_path: PathBuf,
    pub object_emission: Duration,
    pub runtime_compilation: Duration,
    pub data_compilation: Duration,
    pub linking: Duration,
    pub cache_hits: Vec<String>,
    pub cache_misses: Vec<String>,
    /// Artifacts taken from the compiler itself instead of the project cache.
    pub prebuilt: Vec<String>,
}

/// A binding package linked into an executable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkedBindingPackage {
    pub name: String,
    pub archive_path: PathBuf,
    /// Operating-system libraries the package links (`-l<name>`).
    pub system_libraries: Vec<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct NativeBuildReport {
    /// The `--std-dir` directory the build read `std/` modules from, or `None`
    /// for the tree embedded in the compiler.
    pub standard_override: Option<PathBuf>,
    pub executable_path: PathBuf,
    pub artifact_directory: PathBuf,
    pub application_object_paths: Vec<PathBuf>,
    pub metadata_object_path: PathBuf,
    pub runtime_archive_path: PathBuf,
    pub fiber_runtime_archive_path: Option<PathBuf>,
    /// The C binding packages linked in (G20). A C fault in one ends the
    /// whole process, so the report names every one.
    pub binding_packages: Vec<LinkedBindingPackage>,
    pub manifest_path: PathBuf,
    pub dynamic_dependencies: Vec<String>,
    pub fully_static: bool,
    pub cache_hits: Vec<String>,
    pub cache_misses: Vec<String>,
    pub prebuilt: Vec<String>,
    /// Each package the build read, with where its source came from; empty
    /// for a project with no manifest.
    pub packages: Vec<crate::project::BuiltPackage>,
}

impl NativeBuildReport {
    pub fn render_json(&self) -> String {
        let mut output =
            String::from("{\"schema\":\"chimi.build.v1\",\"backend\":\"native-aot\",\"codegen\":");
        write_json_string(&mut output, CODEGEN_IDENTITY);
        output.push_str(",\"runtime_abi\":");
        output.push_str(&RUNTIME_ABI_VERSION.to_string());
        let limits = crate::execution::ConstantLimits::BUILD;
        output.push_str(&format!(
            ",\"constant_limits\":{{\"operations\":{},\"call_depth\":{},\"value_bytes\":{}}}",
            limits.operations, limits.call_depth, limits.value_bytes
        ));
        output.push_str(",\"target\":{\"os\":");
        write_json_string(&mut output, std::env::consts::OS);
        output.push_str(",\"arch\":");
        write_json_string(&mut output, std::env::consts::ARCH);
        output.push_str("},\"standard_library\":");
        match &self.standard_override {
            None => output.push_str("{\"source\":\"embedded\"}"),
            Some(directory) => {
                output.push_str("{\"source\":\"override\",\"directory\":");
                write_path(&mut output, directory);
                output.push('}');
            }
        }
        output.push_str(",\"executable\":");
        write_path(&mut output, &self.executable_path);
        output.push_str(",\"artifacts\":{\"directory\":");
        write_path(&mut output, &self.artifact_directory);
        output.push_str(",\"application_objects\":[");
        for (index, path) in self.application_object_paths.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            write_path(&mut output, path);
        }
        output.push(']');
        output.push_str(",\"metadata_object\":");
        write_path(&mut output, &self.metadata_object_path);
        output.push_str(",\"runtime_archive\":");
        write_path(&mut output, &self.runtime_archive_path);
        output.push_str(",\"fiber_runtime_archive\":");
        if let Some(path) = &self.fiber_runtime_archive_path {
            write_path(&mut output, path);
        } else {
            output.push_str("null");
        }
        output.push_str(",\"binding_packages\":[");
        for (index, package) in self.binding_packages.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            output.push_str("{\"name\":");
            write_json_string(&mut output, &package.name);
            output.push_str(",\"archive\":");
            write_path(&mut output, &package.archive_path);
            output.push_str(",\"system_libraries\":[");
            write_strings(&mut output, &package.system_libraries);
            output.push_str("]}");
        }
        output.push(']');
        output.push_str(",\"manifest\":");
        write_path(&mut output, &self.manifest_path);
        output.push_str("},\"dynamic_dependencies\":[");
        for (index, dependency) in self.dynamic_dependencies.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            write_json_string(&mut output, dependency);
        }
        output.push_str("],\"fully_static\":");
        output.push_str(if self.fully_static { "true" } else { "false" });
        output.push_str(",\"cache\":{\"hits\":[");
        write_strings(&mut output, &self.cache_hits);
        output.push_str("],\"misses\":[");
        write_strings(&mut output, &self.cache_misses);
        output.push_str("],\"prebuilt\":[");
        write_strings(&mut output, &self.prebuilt);
        output.push_str("]}");
        output.push_str(",\"packages\":[");
        for (index, package) in self.packages.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            output.push_str("{\"name\":");
            write_json_string(&mut output, &package.name);
            output.push_str(",\"own\":");
            output.push_str(if package.own { "true" } else { "false" });
            match &package.source {
                crate::project::BuiltSource::Path(directory) => {
                    output.push_str(",\"source\":{\"path\":");
                    write_path(&mut output, directory);
                }
                crate::project::BuiltSource::Git { url, rev, commit } => {
                    output.push_str(",\"source\":{\"git\":");
                    write_json_string(&mut output, url);
                    output.push_str(",\"rev\":");
                    write_json_string(&mut output, rev);
                    output.push_str(",\"commit\":");
                    write_json_string(&mut output, commit);
                }
            }
            output.push_str("}}");
        }
        output.push(']');
        output.push_str("}\n");
        output
    }
}

pub fn build_application(
    program: &BackendProgram,
    standard: &crate::project::StandardTree,
    executable_path: &Path,
    artifact_directory: &Path,
    packages: Vec<crate::project::BuiltPackage>,
) -> Result<NativeBuildReport, String> {
    let _lock = lock_artifact_directory(artifact_directory)?;
    let artifact = build_to_path(program, artifact_directory, executable_path)?;
    let dynamic_dependencies = dynamic_dependencies(executable_path)?;
    let manifest_path = artifact_directory.join("build.json");
    let report = NativeBuildReport {
        standard_override: standard.override_directory().map(Path::to_owned),
        executable_path: executable_path.to_owned(),
        artifact_directory: artifact_directory.to_owned(),
        application_object_paths: artifact.object_paths,
        metadata_object_path: artifact.data_object_path,
        runtime_archive_path: artifact.runtime_archive_path,
        fiber_runtime_archive_path: artifact.fiber_runtime_archive_path,
        binding_packages: artifact.binding_packages,
        manifest_path: manifest_path.clone(),
        fully_static: dynamic_dependencies.is_empty(),
        dynamic_dependencies,
        cache_hits: artifact.cache_hits,
        cache_misses: artifact.cache_misses,
        prebuilt: artifact.prebuilt,
        packages,
    };
    std::fs::write(&manifest_path, report.render_json())
        .map_err(|error| format!("write native build manifest: {error}"))?;
    Ok(report)
}

pub fn build_executable(
    program: &BackendProgram,
    output_directory: &Path,
) -> Result<NativeArtifact, String> {
    let _lock = lock_artifact_directory(output_directory)?;
    build_to_path(program, output_directory, &output_directory.join("program"))
}

/// Holds `directory` for one build until the returned file is dropped. Builds
/// of different programs to one directory (two `chimi build`s run in the same
/// place, or parallel tests) write the same intermediate files, so without
/// the lock one build can replace `program_data.o` or the runtime archive
/// while another links them.
fn lock_artifact_directory(directory: &Path) -> Result<std::fs::File, String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("create native artifact directory: {error}"))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(directory.join(".build-lock"))
        .map_err(|error| format!("open native build lock: {error}"))?;
    lock.lock()
        .map_err(|error| format!("lock native artifact directory: {error}"))?;
    Ok(lock)
}

fn build_to_path(
    program: &BackendProgram,
    output_directory: &Path,
    executable_path: &Path,
) -> Result<NativeArtifact, String> {
    let process_kernel = requires_process_runtime(&program.core);
    std::fs::create_dir_all(output_directory)
        .map_err(|error| format!("create native artifact directory: {error}"))?;
    if let Some(parent) = executable_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create executable output directory: {error}"))?;
    }
    let data_source_path = output_directory.join("program_data.c");
    let data_object_path = output_directory.join("program_data.o");
    let runtime_object_path = output_directory.join("runtime.o");
    let runtime_archive_name = if process_kernel {
        "libchimi_rt_process.a"
    } else {
        "libchimi_rt.a"
    };
    let runtime_archive_path = output_directory.join(runtime_archive_name);
    let fiber_runtime_archive_path =
        requires_fiber_runtime(&program.core).then(|| output_directory.join("libchimi_fiber.a"));
    let include_directory = output_directory.to_path_buf();
    let cache_directory = output_directory
        .parent()
        .unwrap_or(output_directory)
        .join(".cache/native-v1");
    let mut cache = ArtifactCache::new(cache_directory)?;
    let target = native_target_fingerprint()?;
    let cc = command_identity(RUNTIME_C_COMPILER)?;

    let started = Instant::now();
    let mut object_paths = Vec::new();
    let direct_integer_roots = direct_integer_roots(&program.core);
    let direct_loops = direct_loops(&program.core);
    for (file_id, node_ids) in module_nodes(&program.core) {
        let name = format!("module-{file_id}.o");
        let path = output_directory.join(&name);
        let key = module_cache_key(&program.core, file_id, &node_ids, &direct_loops, &target);
        cache.materialize(&key, "o", &path, &name, |temporary| {
            std::fs::write(
                temporary,
                emit_module_object(
                    &program.core,
                    file_id,
                    &node_ids,
                    &direct_integer_roots,
                    &direct_loops,
                )?,
            )
            .map_err(|error| format!("write native module object: {error}"))
        })?;
        object_paths.push(path);
    }
    let entry_path = output_directory.join("entry.o");
    let entry_key = entry_cache_key(&program.core, &target);
    cache.materialize(&entry_key, "o", &entry_path, "entry.o", |temporary| {
        std::fs::write(temporary, emit_entry_object(&program.core)?)
            .map_err(|error| format!("write native entry object: {error}"))
    })?;
    object_paths.push(entry_path);
    let object_emission = started.elapsed();

    let data_source = render_program_data(&program.core, false);
    std::fs::write(&data_source_path, &data_source)
        .map_err(|error| format!("write native program data: {error}"))?;
    std::fs::write(output_directory.join("runtime.h"), RUNTIME_HEADER)
        .map_err(|error| format!("write embedded runtime header: {error}"))?;
    std::fs::write(output_directory.join("chimi_binding.h"), BINDING_HEADER)
        .map_err(|error| format!("write embedded binding header: {error}"))?;
    std::fs::write(output_directory.join("chimi_fiber.h"), FIBER_RUNTIME_HEADER)
        .map_err(|error| format!("write embedded fiber runtime header: {error}"))?;
    if let Some(path) = &fiber_runtime_archive_path {
        std::fs::copy(bundled_fiber_archive()?, path)
            .map_err(|error| format!("materialize bundled fiber runtime: {error}"))?;
    }
    let started = Instant::now();
    let runtime_abi = RUNTIME_ABI_VERSION.to_string();
    let test_defines = test_runtime_defines()?;
    let runtime_key = runtime_archive_key(&RuntimeArchiveInputs {
        abi_version: &runtime_abi,
        target_triple: env!("CHIMI_RUNTIME_TARGET"),
        compiler_identity: &cc,
        runtime_source: RUNTIME_SOURCE.as_bytes(),
        runtime_header: RUNTIME_HEADER.as_bytes(),
        binding_header: BINDING_HEADER.as_bytes(),
        fiber_header: FIBER_RUNTIME_HEADER.as_bytes(),
        process_kernel,
    });
    let runtime_key = if test_defines.is_empty() {
        runtime_key
    } else {
        let mut hasher = blake3::Hasher::new();
        hasher.update(runtime_key.as_bytes());
        for define in &test_defines {
            hasher.update(&(define.len() as u64).to_le_bytes());
            hasher.update(define.as_bytes());
        }
        hasher.finalize().to_hex().to_string()
    };
    if let Some(archive) = prebuilt_runtime_archive(&runtime_key, process_kernel) {
        std::fs::write(&runtime_archive_path, archive)
            .map_err(|error| format!("materialize prebuilt native runtime: {error}"))?;
        cache.record_prebuilt(runtime_archive_name);
    } else {
        std::fs::write(include_directory.join("runtime.c"), RUNTIME_SOURCE)
            .map_err(|error| format!("write embedded runtime source: {error}"))?;
        cache.materialize(
            &runtime_key,
            "a",
            &runtime_archive_path,
            runtime_archive_name,
            |temporary| {
                run(
                    Command::new(RUNTIME_C_COMPILER)
                        .args(runtime_c_flags(process_kernel))
                        .args(&test_defines)
                        .arg("-I")
                        .arg(&include_directory)
                        .arg("-c")
                        .arg(include_directory.join("runtime.c"))
                        .arg("-o")
                        .arg(&runtime_object_path),
                    "compile native backend runtime",
                )?;
                run(
                    Command::new("ar")
                        .arg("rcs")
                        .arg(temporary)
                        .arg(&runtime_object_path),
                    "archive native backend runtime",
                )
            },
        )?;
    }
    let mut binding_packages = Vec::new();
    for package in packages_for(&program.core.reached_bindings()) {
        binding_packages.push(build_binding_archive(
            &package,
            output_directory,
            &mut cache,
            &cc,
        )?);
    }
    let runtime_compilation = started.elapsed();

    let started = Instant::now();
    let data_key = metadata_cache_key(&data_source, &target, &cc);
    cache.materialize(
        &data_key,
        "o",
        &data_object_path,
        "program_data.o",
        |temporary| {
            run(
                Command::new("cc")
                    .arg("-std=c11")
                    .arg("-O2")
                    .arg("-I")
                    .arg(&include_directory)
                    .arg("-c")
                    .arg(&data_source_path)
                    .arg("-o")
                    .arg(temporary),
                "compile native backend program data",
            )
        },
    )?;
    let data_compilation = started.elapsed();

    let started = Instant::now();
    let mut link = Command::new("cc");
    // Binding archives call into the runtime, so they come before it.
    link.args(&object_paths).arg(&data_object_path);
    for package in &binding_packages {
        link.arg(&package.archive_path);
    }
    link.arg(&runtime_archive_path);
    for package in &binding_packages {
        for library in &package.system_libraries {
            link.arg(format!("-l{library}"));
        }
    }
    if let Some(path) = &fiber_runtime_archive_path {
        if cfg!(target_os = "macos") {
            link.arg(format!("-Wl,-force_load,{}", path.display()));
        } else {
            link.arg("-Wl,--whole-archive")
                .arg(path)
                .arg("-Wl,--no-whole-archive");
        }
    }
    // The runtime calls `fmod`, which glibc keeps in libm.
    link.arg("-lm").arg("-o").arg(executable_path);
    run(&mut link, "link native backend executable")?;
    let linking = started.elapsed();
    let (cache_hits, cache_misses, prebuilt) = cache.finish();

    Ok(NativeArtifact {
        object_paths,
        data_object_path,
        runtime_archive_path,
        fiber_runtime_archive_path,
        binding_packages,
        executable_path: executable_path.to_owned(),
        object_emission,
        runtime_compilation,
        data_compilation,
        linking,
        cache_hits,
        cache_misses,
        prebuilt,
    })
}

/// Materializes `package`'s archive in `output_directory`: the one `build.rs`
/// prebuilt when its key matches, otherwise one compiled into the project
/// cache, keyed by its sources and flags.
fn build_binding_archive(
    package: &BindingPackage,
    output_directory: &Path,
    cache: &mut ArtifactCache,
    cc: &str,
) -> Result<LinkedBindingPackage, String> {
    let archive_name = package.archive_name();
    let archive_path = output_directory.join(&archive_name);
    let key = binding_archive_key(&BindingArchiveInputs {
        package: package.name,
        package_flags: binding_package_flags(package.name),
        target_triple: env!("CHIMI_RUNTIME_TARGET"),
        compiler_identity: cc,
        binding_header: BINDING_HEADER.as_bytes(),
        sources: &package.files,
    });
    if let Some(archive) = prebuilt_binding_archive(package.name, &key) {
        std::fs::write(&archive_path, archive)
            .map_err(|error| format!("materialize prebuilt binding package: {error}"))?;
        cache.record_prebuilt(&archive_name);
    } else {
        cache.materialize(&key, "a", &archive_path, &archive_name, |temporary| {
            compile_binding_archive(
                package.name,
                &package.files,
                output_directory,
                &output_directory.join(format!("binding-{}", package.name)),
                temporary,
            )
        })?;
    }
    Ok(LinkedBindingPackage {
        name: package.name.to_owned(),
        archive_path,
        system_libraries: package
            .system_libraries
            .iter()
            .map(|library| (*library).to_owned())
            .collect(),
    })
}

// Written by `build.rs`: `(package, key, archive)` of each binding package it
// prebuilt; empty when it prebuilt none.
include!(concat!(env!("OUT_DIR"), "/prebuilt_bindings.rs"));

/// The archive `build.rs` compiled for `package`, when it was compiled from
/// exactly the inputs `key` names; otherwise `None`.
fn prebuilt_binding_archive(package: &str, key: &str) -> Option<&'static [u8]> {
    PREBUILT_BINDINGS
        .iter()
        .find(|(name, prebuilt_key, _)| *name == package && *prebuilt_key == key)
        .map(|(_, _, archive)| *archive)
}

const RUNTIME_SOURCE: &str = include_str!("../../runtime/native/runtime.c");
const RUNTIME_HEADER: &str = include_str!("../../runtime/native/runtime.h");
const FIBER_RUNTIME_HEADER: &str =
    include_str!("../../../../runtime/chimi-fiber/include/chimi_fiber.h");
/// The runtime test builds `CHIMI_TEST_RUNTIME_DEFINES` may name.
const TEST_RUNTIME_DEFINES: [&str; 1] = ["-DCHIMI_CARRIER_SIMULATION"];

// Written by `build.rs`; each is empty when the compiler was built without it.
const PREBUILT_RUNTIME_ARCHIVE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/libchimi_rt.a"));
const PREBUILT_RUNTIME_KEY: &str = include_str!(concat!(env!("OUT_DIR"), "/libchimi_rt.key"));
const PREBUILT_PROCESS_RUNTIME_ARCHIVE: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/libchimi_rt_process.a"));
const PREBUILT_PROCESS_RUNTIME_KEY: &str =
    include_str!(concat!(env!("OUT_DIR"), "/libchimi_rt_process.key"));

/// Test builds of the runtime: in a debug build of the compiler,
/// `CHIMI_TEST_RUNTIME_DEFINES` names `-D` flags (space-separated, each one of
/// `TEST_RUNTIME_DEFINES`) added to the runtime's compile, such as
/// `-DCHIMI_CARRIER_SIMULATION` (parallel-scheduler.md). They change the
/// archive's key, so a test build never reuses the prebuilt runtime. A
/// release build ignores the variable.
fn test_runtime_defines() -> Result<Vec<String>, String> {
    if !cfg!(debug_assertions) {
        return Ok(Vec::new());
    }
    let Ok(value) = std::env::var("CHIMI_TEST_RUNTIME_DEFINES") else {
        return Ok(Vec::new());
    };
    value
        .split_whitespace()
        .map(|define| {
            if TEST_RUNTIME_DEFINES.contains(&define) {
                Ok(define.to_owned())
            } else {
                Err(format!(
                    "CHIMI_TEST_RUNTIME_DEFINES: `{define}` is not one of {TEST_RUNTIME_DEFINES:?}"
                ))
            }
        })
        .collect()
}

/// The archive of the variant `build.rs` compiled, when it was compiled from
/// exactly the inputs `key` names (the embedded sources and today's `cc`);
/// otherwise `None`, and the caller compiles the runtime into the project cache.
fn prebuilt_runtime_archive(key: &str, process_kernel: bool) -> Option<&'static [u8]> {
    let (archive, prebuilt_key) = if process_kernel {
        (
            PREBUILT_PROCESS_RUNTIME_ARCHIVE,
            PREBUILT_PROCESS_RUNTIME_KEY,
        )
    } else {
        (PREBUILT_RUNTIME_ARCHIVE, PREBUILT_RUNTIME_KEY)
    };
    (!prebuilt_key.is_empty() && prebuilt_key == key).then_some(archive)
}

pub(crate) fn bundled_fiber_archive() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("CHIMI_FIBER_RUNTIME_ARCHIVE") {
        let path = PathBuf::from(path);
        return path
            .is_file()
            .then_some(path)
            .ok_or_else(|| "CHIMI_FIBER_RUNTIME_ARCHIVE does not name a file".to_owned());
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("locate compiler executable for fiber runtime: {error}"))?;
    let parent = executable
        .parent()
        .ok_or_else(|| "compiler executable has no parent directory".to_owned())?;
    let mut directories = vec![parent.to_path_buf(), parent.join("deps")];
    let mut profile = parent;
    if parent.file_name().is_some_and(|name| name == "deps")
        && let Some(above) = parent.parent()
    {
        directories.push(above.to_path_buf());
        profile = above;
    }
    // Cargo's build-directory layout keeps a dependency's artifacts in
    // `<profile>/build/<package>/<hash>/out` instead of `<profile>/deps`, so a
    // compiler built without chimi-fiber as a root package finds it only there.
    if let Ok(builds) = std::fs::read_dir(profile.join("build").join("chimi-fiber")) {
        directories.extend(builds.flatten().map(|build| build.path().join("out")));
    }
    let mut candidates = Vec::new();
    for directory in directories {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if (name == "libchimi_fiber.a"
                || name.starts_with("libchimi_fiber-") && name.ends_with(".a"))
                && path.is_file()
            {
                candidates.push(path);
            }
        }
    }
    candidates.sort_by_key(|path| {
        path.metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
    });
    candidates.pop().ok_or_else(|| {
        format!(
            "bundled Chimi fiber runtime ABI {FIBER_RUNTIME_ABI_VERSION} is missing; set CHIMI_FIBER_RUNTIME_ARCHIVE"
        )
    })
}

fn requires_fiber_runtime(core: &CoreProgram) -> bool {
    core.nodes
        .iter()
        .any(|node| super::program_data::reaches_scheduler(core, &node.operation))
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

fn command_identity(command: &str) -> Result<String, String> {
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

fn write_strings(output: &mut String, values: &[String]) {
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        write_json_string(output, value);
    }
}

struct ArtifactCache {
    directory: PathBuf,
    hits: Vec<String>,
    misses: Vec<String>,
    prebuilt: Vec<String>,
}

impl ArtifactCache {
    fn new(directory: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("create native cache directory: {error}"))?;
        Ok(Self {
            directory,
            hits: Vec::new(),
            misses: Vec::new(),
            prebuilt: Vec::new(),
        })
    }

    fn materialize(
        &mut self,
        key: &str,
        extension: &str,
        output_path: &Path,
        label: &str,
        create: impl FnOnce(&Path) -> Result<(), String>,
    ) -> Result<(), String> {
        let cached = self.directory.join(format!("{key}.{extension}"));
        let hit = cached.is_file();
        if !hit {
            let sequence = CACHE_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let temporary =
                self.directory
                    .join(format!(".{key}.{}.{}.tmp", std::process::id(), sequence));
            if let Err(error) = create(&temporary) {
                let _ = std::fs::remove_file(&temporary);
                return Err(error);
            }
            if let Err(error) = std::fs::rename(&temporary, &cached) {
                if cached.is_file() {
                    let _ = std::fs::remove_file(&temporary);
                } else {
                    let _ = std::fs::remove_file(&temporary);
                    return Err(format!("publish cached native artifact: {error}"));
                }
            }
        }
        std::fs::copy(&cached, output_path)
            .map_err(|error| format!("materialize cached native artifact {label}: {error}"))?;
        if hit {
            self.hits.push(label.to_owned());
        } else {
            self.misses.push(label.to_owned());
        }
        Ok(())
    }

    fn record_prebuilt(&mut self, label: &str) {
        self.prebuilt.push(label.to_owned());
    }

    fn finish(self) -> (Vec<String>, Vec<String>, Vec<String>) {
        (self.hits, self.misses, self.prebuilt)
    }
}

fn cache_key(kind: &str, parts: &[&[u8]]) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in [CACHE_SCHEMA.as_bytes(), kind.as_bytes()]
        .into_iter()
        .chain(parts.iter().copied())
    {
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    hasher.finalize().to_hex().to_string()
}

fn module_cache_key(
    core: &CoreProgram,
    file_id: u32,
    node_ids: &[u32],
    direct_loops: &HashMap<u32, Pass>,
    target: &str,
) -> String {
    let mut content = format!("file={file_id}\n");
    for &node_id in node_ids {
        let node = &core.nodes[node_id as usize];
        let origin = &core.origins[node.origin.raw() as usize];
        content.push_str(&format!("{node:?}\n{origin:?}\n"));
        // A direct pass inlines the values of the constants it reads, which may
        // live in another module, so the pass itself is part of the key.
        if let Some(pass) = direct_loops.get(&node_id) {
            content.push_str(&format!("{pass:?}\n"));
        }
    }
    let runtime_abi = RUNTIME_ABI_VERSION.to_string();
    cache_key(
        "module-object",
        &[
            CODEGEN_IDENTITY.as_bytes(),
            CODEGEN_SETTINGS.as_bytes(),
            NATIVE_LOWERING_REVISION.as_bytes(),
            runtime_abi.as_bytes(),
            target.as_bytes(),
            content.as_bytes(),
        ],
    )
}

fn entry_cache_key(core: &CoreProgram, target: &str) -> String {
    let entry = core
        .functions
        .iter()
        .find(|function| function.symbol == core.entry)
        .expect("well-formed core has entry");
    let content = format!("entry={:?};body={:?}", core.entry, entry.body);
    let runtime_abi = RUNTIME_ABI_VERSION.to_string();
    cache_key(
        "entry-object",
        &[
            CODEGEN_IDENTITY.as_bytes(),
            CODEGEN_SETTINGS.as_bytes(),
            NATIVE_LOWERING_REVISION.as_bytes(),
            runtime_abi.as_bytes(),
            target.as_bytes(),
            content.as_bytes(),
        ],
    )
}

fn metadata_cache_key(source: &str, target: &str, cc: &str) -> String {
    cache_key(
        "metadata-object",
        &[
            C_SETTINGS.as_bytes(),
            target.as_bytes(),
            cc.as_bytes(),
            RUNTIME_HEADER.as_bytes(),
            source.as_bytes(),
        ],
    )
}

fn native_isa() -> Result<OwnedTargetIsa, String> {
    let mut flags = settings::builder();
    flags
        .set("is_pic", "true")
        .map_err(|error| format!("enable position-independent native code: {error}"))?;
    cranelift_native::builder()
        .map_err(str::to_owned)?
        .finish(settings::Flags::new(flags))
        .map_err(|error| format!("finish native Cranelift target: {error}"))
}

/// Checks that `chimi build --target <triple>` can build for `triple`. Only
/// the host is supported: another target needs the runtime compiled for it
/// and a linker and C library for it, which no build ships yet.
pub fn check_build_target(triple: &str) -> Result<(), String> {
    use std::str::FromStr;
    let host = native_isa()?.triple().clone();
    let Ok(target) = target_lexicon::Triple::from_str(triple) else {
        return Err(format!(
            "unknown target `{triple}`; give a target triple such as {host}"
        ));
    };
    if same_platform(&target, &host) {
        return Ok(());
    }
    Err(format!(
        "cannot build for {target} yet: building for another target needs the \
         Chimi runtime compiled for it and a linker and C library for it, \
         which this chimi does not have; it builds for {host}"
    ))
}

/// Whether two triples name the same platform: the same architecture,
/// operating system, environment and object format. The vendor is ignored
/// (`x86_64-pc-linux-gnu` is the host `x86_64-unknown-linux-gnu`), and so
/// are `darwin` against `macosx` and a macOS version.
fn same_platform(left: &target_lexicon::Triple, right: &target_lexicon::Triple) -> bool {
    use target_lexicon::OperatingSystem;
    let system = |triple: &target_lexicon::Triple| match triple.operating_system {
        OperatingSystem::Darwin(_) | OperatingSystem::MacOSX(_) => OperatingSystem::Darwin(None),
        other => other,
    };
    left.architecture == right.architecture
        && system(left) == system(right)
        && left.environment == right.environment
        && left.binary_format == right.binary_format
}

fn native_target_fingerprint() -> Result<String, String> {
    let isa = native_isa()?;
    let mut fingerprint = format!("{}|{}", isa.triple(), isa.flags());
    for flag in isa.isa_flags() {
        fingerprint.push('|');
        fingerprint.push_str(&flag.to_string());
    }
    Ok(fingerprint)
}

fn dynamic_dependencies(executable: &Path) -> Result<Vec<String>, String> {
    let output = if cfg!(target_os = "macos") {
        Command::new("otool").arg("-L").arg(executable).output()
    } else {
        Command::new("ldd").arg(executable).output()
    }
    .map_err(|error| format!("inspect native dependencies: {error}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        let combined = format!("{stdout}\n{stderr}");
        if combined.contains("not a dynamic executable") || combined.contains("statically linked") {
            return Ok(Vec::new());
        }
        return Err(format!("inspect native dependencies: {}", stderr.trim()));
    }

    let dependencies = stdout
        .lines()
        .skip(usize::from(cfg!(target_os = "macos")))
        .filter_map(parse_dependency)
        .collect();
    Ok(dependencies)
}

fn parse_dependency(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || line == "statically linked" {
        return None;
    }
    let target = match line.split_once("=>") {
        Some((name, target)) if target.trim() == "not found" => {
            return Some(format!("{} => not found", name.trim()));
        }
        Some((_, target)) => target.trim(),
        None => line,
    };
    let dependency = target
        .split_once(" (")
        .map_or(target, |(path, _)| path)
        .trim();
    (!dependency.is_empty() && dependency != "not found").then(|| dependency.to_owned())
}

fn write_path(output: &mut String, path: &Path) {
    write_json_string(output, &path.to_string_lossy());
}

fn module_nodes(core: &CoreProgram) -> BTreeMap<u32, Vec<u32>> {
    let mut modules = BTreeMap::<u32, Vec<u32>>::new();
    for node in &core.nodes {
        let file_id = core.origins[node.origin.raw() as usize].file_id.raw();
        modules.entry(file_id).or_default().push(node.id.raw());
    }
    modules
}

fn new_object_module(name: &str) -> Result<ObjectModule, String> {
    let isa = native_isa()?;
    let builder = ObjectBuilder::new(isa, name, default_libcall_names())
        .map_err(|error| format!("create Cranelift object builder: {error}"))?;
    Ok(ObjectModule::new(builder))
}

fn node_signature(module: &ObjectModule) -> Signature {
    let pointer = module.target_config().pointer_type();
    Signature {
        params: vec![
            AbiParam::new(pointer),
            AbiParam::new(pointer),
            AbiParam::new(types::I64),
        ],
        returns: vec![AbiParam::new(pointer)],
        call_conv: module.target_config().default_call_conv,
    }
}

fn runtime_signature(module: &ObjectModule) -> Signature {
    let pointer = module.target_config().pointer_type();
    Signature {
        params: vec![
            AbiParam::new(pointer),
            AbiParam::new(types::I32),
            AbiParam::new(pointer),
            AbiParam::new(types::I64),
        ],
        returns: vec![AbiParam::new(pointer)],
        call_conv: module.target_config().default_call_conv,
    }
}

fn emit_module_object(
    core: &CoreProgram,
    file_id: u32,
    node_ids: &[u32],
    direct_integer_roots: &HashMap<u32, u32>,
    direct_loops: &HashMap<u32, Pass>,
) -> Result<Vec<u8>, String> {
    let mut module = new_object_module(&format!("chimi_module_{file_id}"))?;
    let node_signature = node_signature(&module);
    let runtime_signature = runtime_signature(&module);
    let pointer = module.target_config().pointer_type();
    let full_trace_signature = Signature {
        params: vec![AbiParam::new(pointer)],
        returns: vec![AbiParam::new(types::I32)],
        call_conv: module.target_config().default_call_conv,
    };
    let direct_integer_success_signature = Signature {
        params: vec![
            AbiParam::new(pointer),
            AbiParam::new(types::I32),
            AbiParam::new(types::I64),
        ],
        returns: vec![AbiParam::new(pointer)],
        call_conv: module.target_config().default_call_conv,
    };
    let mut functions = HashMap::new();
    for &node_id in node_ids {
        functions.insert(
            node_id,
            module
                .declare_function(
                    &format!("chimi_node_{node_id}"),
                    Linkage::Export,
                    &node_signature,
                )
                .map_err(|error| format!("declare native node {node_id}: {error}"))?,
        );
    }

    let mut helpers = HashMap::new();
    for operation in [
        "constant",
        "reference",
        "select",
        "dependency_method",
        "construct",
        "call",
        "list",
        "tuple",
        "record",
        "unary",
        "binary",
        "identity",
        "provision",
        "sequence",
        "closure",
        "match",
        "loop",
        "break",
    ] {
        helpers.insert(
            operation,
            module
                .declare_function(
                    &format!("chimi_rt_{operation}"),
                    Linkage::Import,
                    &runtime_signature,
                )
                .map_err(|error| format!("declare runtime helper {operation}: {error}"))?,
        );
    }
    let full_trace = module
        .declare_function(
            "chimi_rt_full_trace",
            Linkage::Import,
            &full_trace_signature,
        )
        .map_err(|error| format!("declare full-trace runtime query: {error}"))?;
    let direct_integer_success = module
        .declare_function(
            "chimi_rt_direct_integer_success",
            Linkage::Import,
            &direct_integer_success_signature,
        )
        .map_err(|error| format!("declare direct-integer runtime result: {error}"))?;
    let direct_loop_signature = Signature {
        params: vec![
            AbiParam::new(pointer),
            AbiParam::new(types::I32),
            AbiParam::new(pointer),
            AbiParam::new(types::I64),
            AbiParam::new(pointer),
        ],
        returns: vec![AbiParam::new(pointer)],
        call_conv: module.target_config().default_call_conv,
    };
    let direct_loop = module
        .declare_function(
            "chimi_rt_direct_loop",
            Linkage::Import,
            &direct_loop_signature,
        )
        .map_err(|error| format!("declare direct-loop runtime driver: {error}"))?;

    for &node_id in node_ids {
        if let Some(pass) = direct_loops.get(&node_id) {
            define_direct_loop(
                &mut module,
                functions[&node_id],
                node_id,
                pass,
                direct_loop,
                node_signature.clone(),
            )?;
            continue;
        }
        let node = &core.nodes[node_id as usize];
        define_node(
            &mut module,
            core,
            functions[&node_id],
            node_id,
            direct_integer_roots.get(&node_id).copied(),
            NodeRuntimeFunctions {
                fallback: helpers[operation_name(&node.operation)],
                full_trace,
                direct_integer_success,
            },
            node_signature.clone(),
        )?;
    }

    module
        .finish()
        .emit()
        .map_err(|error| format!("emit native module {file_id} object: {error}"))
}

fn emit_entry_object(core: &CoreProgram) -> Result<Vec<u8>, String> {
    let mut module = new_object_module("chimi_entry")?;
    let pointer = module.target_config().pointer_type();
    let call_conv = module.target_config().default_call_conv;
    let main_signature = Signature {
        params: vec![AbiParam::new(types::I32), AbiParam::new(pointer)],
        returns: vec![AbiParam::new(types::I32)],
        call_conv,
    };
    let runtime_main_signature = Signature {
        params: vec![
            AbiParam::new(pointer),
            AbiParam::new(types::I32),
            AbiParam::new(pointer),
        ],
        returns: vec![AbiParam::new(types::I32)],
        call_conv,
    };
    let runtime_main = module
        .declare_function("chimi_rt_main", Linkage::Import, &runtime_main_signature)
        .map_err(|error| format!("declare native runtime main: {error}"))?;
    let main = module
        .declare_function("main", Linkage::Export, &main_signature)
        .map_err(|error| format!("declare native main: {error}"))?;
    let entry = core
        .functions
        .iter()
        .find(|function| function.symbol == core.entry)
        .expect("well-formed core has entry");
    let entry_function = module
        .declare_function(
            &format!("chimi_node_{}", entry.body.raw()),
            Linkage::Import,
            &node_signature(&module),
        )
        .map_err(|error| format!("declare native entry node: {error}"))?;
    define_main(
        &mut module,
        main,
        runtime_main,
        entry_function,
        main_signature,
    )?;
    module
        .finish()
        .emit()
        .map_err(|error| format!("emit native entry object: {error}"))
}

#[derive(Clone, Copy)]
struct NodeRuntimeFunctions {
    fallback: FuncId,
    full_trace: FuncId,
    direct_integer_success: FuncId,
}

fn define_node(
    module: &mut ObjectModule,
    core: &CoreProgram,
    function_id: FuncId,
    node_id: u32,
    direct_integer_node_count: Option<u32>,
    runtime: NodeRuntimeFunctions,
    signature: Signature,
) -> Result<(), String> {
    let mut context = Context::new();
    context.func =
        Function::with_name_signature(UserFuncName::user(0, function_id.as_u32()), signature);
    let mut builder_context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        let parameters = builder.block_params(entry).to_vec();
        let fallback = module.declare_func_in_func(runtime.fallback, builder.func);
        if let Some(node_count) = direct_integer_node_count {
            let full_trace = module.declare_func_in_func(runtime.full_trace, builder.func);
            let direct_success =
                module.declare_func_in_func(runtime.direct_integer_success, builder.func);
            let direct_block = builder.create_block();
            let success_block = builder.create_block();
            let fallback_block = builder.create_block();
            let return_block = builder.create_block();
            builder.append_block_param(return_block, module.target_config().pointer_type());

            let trace_call = builder.ins().call(full_trace, &[parameters[0]]);
            let requires_trace = builder.inst_results(trace_call)[0];
            builder
                .ins()
                .brif(requires_trace, fallback_block, &[], direct_block, &[]);

            builder.switch_to_block(direct_block);
            let (value, overflow) = emit_pure_integer(&mut builder, core, node_id);
            builder
                .ins()
                .brif(overflow, fallback_block, &[], success_block, &[]);

            builder.switch_to_block(success_block);
            let count = builder.ins().iconst(types::I32, i64::from(node_count));
            let call = builder
                .ins()
                .call(direct_success, &[parameters[0], count, value]);
            let result = builder.inst_results(call)[0];
            builder.ins().jump(return_block, &[result.into()]);

            builder.switch_to_block(fallback_block);
            let node = builder.ins().iconst(types::I32, i64::from(node_id));
            let call = builder.ins().call(
                fallback,
                &[parameters[0], node, parameters[1], parameters[2]],
            );
            let result = builder.inst_results(call)[0];
            builder.ins().jump(return_block, &[result.into()]);

            builder.switch_to_block(return_block);
            let result = builder.block_params(return_block)[0];
            builder.ins().return_(&[result]);
        } else {
            let node = builder.ins().iconst(types::I32, i64::from(node_id));
            let call = builder.ins().call(
                fallback,
                &[parameters[0], node, parameters[1], parameters[2]],
            );
            let result = builder.inst_results(call)[0];
            builder.ins().return_(&[result]);
        }
        builder.seal_all_blocks();
        builder.finalize();
    }
    module
        .define_function(function_id, &mut context)
        .map_err(|error| format!("define native node {node_id}: {error}"))
}

/// Defines a loop node as a call to `chimi_rt_direct_loop` with its pass, which is
/// emitted beside it as a local function.
fn define_direct_loop(
    module: &mut ObjectModule,
    function_id: FuncId,
    node_id: u32,
    pass: &Pass,
    direct_loop: FuncId,
    signature: Signature,
) -> Result<(), String> {
    let pointer = module.target_config().pointer_type();
    let pass_signature = Signature {
        params: vec![AbiParam::new(types::I64), AbiParam::new(pointer)],
        returns: vec![AbiParam::new(types::I32)],
        call_conv: module.target_config().default_call_conv,
    };
    let pass_id = module
        .declare_function(
            &format!("chimi_loop_pass_{node_id}"),
            Linkage::Local,
            &pass_signature,
        )
        .map_err(|error| format!("declare direct loop pass {node_id}: {error}"))?;
    let mut builder_context = FunctionBuilderContext::new();

    let mut context = Context::new();
    context.func =
        Function::with_name_signature(UserFuncName::user(0, pass_id.as_u32()), pass_signature);
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        let parameters = builder.block_params(entry).to_vec();
        emit_pass(&mut builder, pass, parameters[0], parameters[1]);
        builder.seal_all_blocks();
        builder.finalize();
    }
    module
        .define_function(pass_id, &mut context)
        .map_err(|error| format!("define direct loop pass {node_id}: {error}"))?;

    let mut context = Context::new();
    context.func =
        Function::with_name_signature(UserFuncName::user(0, function_id.as_u32()), signature);
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        let parameters = builder.block_params(entry).to_vec();
        let driver = module.declare_func_in_func(direct_loop, builder.func);
        let pass = module.declare_func_in_func(pass_id, builder.func);
        let pass_address = builder.ins().func_addr(pointer, pass);
        let node = builder.ins().iconst(types::I32, i64::from(node_id));
        let call = builder.ins().call(
            driver,
            &[
                parameters[0],
                node,
                parameters[1],
                parameters[2],
                pass_address,
            ],
        );
        let result = builder.inst_results(call)[0];
        builder.ins().return_(&[result]);
        builder.seal_all_blocks();
        builder.finalize();
    }
    module
        .define_function(function_id, &mut context)
        .map_err(|error| format!("define native direct loop {node_id}: {error}"))
}

#[derive(Clone, Copy)]
enum PureIntegerState {
    Unknown,
    Visiting,
    Known(Option<u32>),
}

fn pure_integer_node_counts(core: &CoreProgram) -> Vec<Option<u32>> {
    let mut states = vec![PureIntegerState::Unknown; core.nodes.len()];
    for node_id in 0..core.nodes.len() as u32 {
        pure_integer_node_count(core, node_id, &mut states);
    }
    states
        .into_iter()
        .map(|state| match state {
            PureIntegerState::Known(count) => count,
            PureIntegerState::Unknown | PureIntegerState::Visiting => {
                unreachable!("every Core node was classified")
            }
        })
        .collect()
}

fn pure_integer_node_count(
    core: &CoreProgram,
    node_id: u32,
    states: &mut [PureIntegerState],
) -> Option<u32> {
    match states[node_id as usize] {
        PureIntegerState::Known(count) => return count,
        PureIntegerState::Visiting => return None,
        PureIntegerState::Unknown => states[node_id as usize] = PureIntegerState::Visiting,
    }
    let node = &core.nodes[node_id as usize];
    let count = (|| -> Option<u32> {
        match &node.operation {
            CoreOperation::Constant(crate::executable::CoreLiteral::Integer(_)) => Some(1),
            CoreOperation::Unary {
                operator: UnaryOperator::Negate,
                operand,
            } => pure_integer_node_count(core, operand.raw(), states)?.checked_add(1),
            CoreOperation::Identity(value) => {
                pure_integer_node_count(core, value.raw(), states)?.checked_add(1)
            }
            CoreOperation::Binary {
                operator: BinaryOperator::Add | BinaryOperator::Subtract | BinaryOperator::Multiply,
                left,
                right,
            } => pure_integer_node_count(core, left.raw(), states)?
                .checked_add(pure_integer_node_count(core, right.raw(), states)?)?
                .checked_add(1),
            _ => None,
        }
    })();
    states[node_id as usize] = PureIntegerState::Known(count);
    count
}

fn direct_integer_roots(core: &CoreProgram) -> HashMap<u32, u32> {
    let counts = pure_integer_node_counts(core);
    let mut roots = counts
        .iter()
        .enumerate()
        .filter_map(|(node_id, count)| count.map(|count| (node_id as u32, count)))
        .collect::<HashMap<_, _>>();
    for node in &core.nodes {
        if counts[node.id.raw() as usize].is_none() {
            continue;
        }
        match &node.operation {
            CoreOperation::Unary { operand, .. } => {
                roots.remove(&operand.raw());
            }
            CoreOperation::Identity(value) => {
                roots.remove(&value.raw());
            }
            CoreOperation::Binary { left, right, .. } => {
                roots.remove(&left.raw());
                roots.remove(&right.raw());
            }
            _ => {}
        }
    }
    roots
}

fn emit_pure_integer(
    builder: &mut FunctionBuilder<'_>,
    core: &CoreProgram,
    node_id: u32,
) -> (Value, Value) {
    let node = &core.nodes[node_id as usize];
    match &node.operation {
        CoreOperation::Constant(crate::executable::CoreLiteral::Integer(value)) => (
            builder.ins().iconst(types::I64, *value),
            builder.ins().iconst(types::I8, 0),
        ),
        CoreOperation::Unary {
            operator: UnaryOperator::Negate,
            operand,
        } => {
            let (operand, child_overflow) = emit_pure_integer(builder, core, operand.raw());
            let zero = builder.ins().iconst(types::I64, 0);
            let (value, overflow) = builder.ins().ssub_overflow(zero, operand);
            (value, builder.ins().bor(child_overflow, overflow))
        }
        CoreOperation::Identity(value) => emit_pure_integer(builder, core, value.raw()),
        CoreOperation::Binary {
            operator,
            left,
            right,
        } => {
            let (left, left_overflow) = emit_pure_integer(builder, core, left.raw());
            let (right, right_overflow) = emit_pure_integer(builder, core, right.raw());
            let child_overflow = builder.ins().bor(left_overflow, right_overflow);
            let (value, overflow) = match operator {
                BinaryOperator::Add => builder.ins().sadd_overflow(left, right),
                BinaryOperator::Subtract => builder.ins().ssub_overflow(left, right),
                BinaryOperator::Multiply => builder.ins().smul_overflow(left, right),
                _ => unreachable!("only direct-lowerable integer operators reach emission"),
            };
            (value, builder.ins().bor(child_overflow, overflow))
        }
        _ => unreachable!("only direct-lowerable integer nodes reach emission"),
    }
}

fn define_main(
    module: &mut ObjectModule,
    main_id: FuncId,
    runtime_main_id: FuncId,
    entry_id: FuncId,
    signature: Signature,
) -> Result<(), String> {
    let mut context = Context::new();
    context.func =
        Function::with_name_signature(UserFuncName::user(1, main_id.as_u32()), signature);
    let mut builder_context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        let parameters = builder.block_params(entry).to_vec();
        let runtime_main = module.declare_func_in_func(runtime_main_id, builder.func);
        let program_entry = module.declare_func_in_func(entry_id, builder.func);
        let program_entry = builder
            .ins()
            .func_addr(module.target_config().pointer_type(), program_entry);
        let call = builder
            .ins()
            .call(runtime_main, &[program_entry, parameters[0], parameters[1]]);
        let status = builder.inst_results(call)[0];
        builder.ins().return_(&[status]);
        builder.seal_all_blocks();
        builder.finalize();
    }
    module
        .define_function(main_id, &mut context)
        .map_err(|error| format!("define native main: {error}"))
}

fn operation_name(operation: &CoreOperation) -> &'static str {
    match operation {
        CoreOperation::Constant(_) => "constant",
        CoreOperation::Reference(_) => "reference",
        CoreOperation::Select { .. } => "select",
        CoreOperation::DependencyMethod { .. } => "dependency_method",
        CoreOperation::Construct { .. } => "construct",
        CoreOperation::Call { .. } => "call",
        CoreOperation::List(_) => "list",
        CoreOperation::Tuple(_) => "tuple",
        CoreOperation::Record(_) => "record",
        CoreOperation::Unary { .. } => "unary",
        CoreOperation::Binary { .. } => "binary",
        CoreOperation::Identity(_) => "identity",
        CoreOperation::Provision { .. } => "provision",
        CoreOperation::Sequence(_) => "sequence",
        CoreOperation::Closure { .. } => "closure",
        CoreOperation::Match { .. } => "match",
        CoreOperation::Loop { .. } => "loop",
        CoreOperation::Break { .. } => "break",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FIBER_RUNTIME_ABI_VERSION, FIBER_RUNTIME_HEADER, RUNTIME_ABI_VERSION, RUNTIME_HEADER,
        cache_key, parse_dependency, requires_fiber_runtime,
    };
    use crate::source::SourceSnapshot;
    use crate::{BackendProgram, Compiler, ProjectSource};

    fn prepared(source: &str) -> BackendProgram {
        Compiler::prepare_backend(vec![
            ProjectSource::new(
                "test/main",
                SourceSnapshot::from_text("test/main.chimi", source),
            )
            .unwrap(),
        ])
        .unwrap()
    }

    #[test]
    fn cache_keys_preserve_input_and_artifact_boundaries() {
        assert_eq!(
            cache_key("module", &[b"alpha", b"beta"]),
            cache_key("module", &[b"alpha", b"beta"])
        );
        assert_ne!(
            cache_key("module", &[b"alpha", b"beta"]),
            cache_key("entry", &[b"alpha", b"beta"])
        );
        assert_ne!(
            cache_key("module", &[b"ab", b"c"]),
            cache_key("module", &[b"a", b"bc"])
        );
    }

    #[test]
    fn rust_and_embedded_runtime_agree_on_the_abi_version() {
        assert!(RUNTIME_HEADER.contains(&format!(
            "#define CHIMI_RUNTIME_ABI_VERSION {RUNTIME_ABI_VERSION}"
        )));
        assert!(FIBER_RUNTIME_HEADER.contains(&format!(
            "#define CHIMI_FIBER_ABI_VERSION {FIBER_RUNTIME_ABI_VERSION}u"
        )));
    }

    #[test]
    fn fiber_archive_is_linked_only_for_reachable_concurrency_operations() {
        assert!(!requires_fiber_runtime(
            &prepared("pub fn main() -> Int { 1 }").core
        ));
        assert!(requires_fiber_runtime(
            &prepared(concat!(
                "import std/task as Task\n",
                "enum WorkError { Stopped }\n",
                "fn work() -> Int {\n",
                "  case 1 {\n",
                "    0 -> Failure(Stopped())\n",
                "    _ -> 1\n",
                "  }\n",
                "}\n",
                "pub fn main() -> Int {\n",
                "  let child = Task.start(work)\n",
                "  Task.result(child)\n",
                "}\n",
            ))
            .core
        ));
    }

    #[test]
    fn parses_platform_dependency_report_lines_without_hiding_missing_libraries() {
        assert_eq!(
            parse_dependency("/usr/lib/libSystem.B.dylib (compatibility version 1.0.0)"),
            Some("/usr/lib/libSystem.B.dylib".to_owned())
        );
        assert_eq!(
            parse_dependency("libc.so.6 => /lib/libc.so.6 (0x1234)"),
            Some("/lib/libc.so.6".to_owned())
        );
        assert_eq!(
            parse_dependency("libmissing.so => not found"),
            Some("libmissing.so => not found".to_owned())
        );
        assert_eq!(parse_dependency("  "), None);
        assert_eq!(parse_dependency("statically linked"), None);
    }
}
