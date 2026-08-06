//! Native AOT object emission and host linking behind the backend-neutral program boundary.

use super::BackendProgram;
use super::program_data::render_program_data;
use crate::executable::{CoreOperation, CoreProgram};
use crate::json::write_json_string;
use cranelift_codegen::Context;
use cranelift_codegen::ir::{AbiParam, Function, InstBuilder, Signature, UserFuncName, types};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{FuncId, Linkage, Module, default_libcall_names};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

pub struct NativeArtifact {
    pub object_paths: Vec<PathBuf>,
    pub data_object_path: PathBuf,
    pub runtime_archive_path: PathBuf,
    pub executable_path: PathBuf,
    pub object_emission: Duration,
    pub runtime_compilation: Duration,
    pub data_compilation: Duration,
    pub linking: Duration,
}

#[derive(Debug, Eq, PartialEq)]
pub struct NativeBuildReport {
    pub executable_path: PathBuf,
    pub artifact_directory: PathBuf,
    pub application_object_paths: Vec<PathBuf>,
    pub metadata_object_path: PathBuf,
    pub runtime_archive_path: PathBuf,
    pub manifest_path: PathBuf,
    pub dynamic_dependencies: Vec<String>,
    pub fully_static: bool,
}

impl NativeBuildReport {
    pub fn render_json(&self) -> String {
        let mut output = String::from(
            "{\"schema\":\"chimi.build.v1\",\"backend\":\"native-aot\",\"codegen\":\"cranelift-0.131.3\",\"runtime_abi\":1,\"target\":{\"os\":",
        );
        write_json_string(&mut output, std::env::consts::OS);
        output.push_str(",\"arch\":");
        write_json_string(&mut output, std::env::consts::ARCH);
        output.push_str("},\"executable\":");
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
        output.push_str("}\n");
        output
    }
}

pub fn build_application(
    program: &BackendProgram,
    executable_path: &Path,
    artifact_directory: &Path,
) -> Result<NativeBuildReport, String> {
    let artifact = build_to_path(program, artifact_directory, executable_path)?;
    let dynamic_dependencies = dynamic_dependencies(executable_path)?;
    let manifest_path = artifact_directory.join("build.json");
    let report = NativeBuildReport {
        executable_path: executable_path.to_owned(),
        artifact_directory: artifact_directory.to_owned(),
        application_object_paths: artifact.object_paths,
        metadata_object_path: artifact.data_object_path,
        runtime_archive_path: artifact.runtime_archive_path,
        manifest_path: manifest_path.clone(),
        fully_static: dynamic_dependencies.is_empty(),
        dynamic_dependencies,
    };
    std::fs::write(&manifest_path, report.render_json())
        .map_err(|error| format!("write native build manifest: {error}"))?;
    Ok(report)
}

pub fn build_executable(
    program: &BackendProgram,
    output_directory: &Path,
) -> Result<NativeArtifact, String> {
    build_to_path(program, output_directory, &output_directory.join("program"))
}

fn build_to_path(
    program: &BackendProgram,
    output_directory: &Path,
    executable_path: &Path,
) -> Result<NativeArtifact, String> {
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
    let runtime_archive_path = output_directory.join("libchimi_rt.a");
    let include_directory = output_directory.to_path_buf();

    let started = Instant::now();
    let mut object_paths = Vec::new();
    for (name, object) in emit_objects(&program.core)? {
        let path = output_directory.join(name);
        std::fs::write(&path, object)
            .map_err(|error| format!("write native application object: {error}"))?;
        object_paths.push(path);
    }
    let object_emission = started.elapsed();

    std::fs::write(&data_source_path, render_program_data(&program.core, false))
        .map_err(|error| format!("write native program data: {error}"))?;
    std::fs::write(output_directory.join("runtime.c"), RUNTIME_SOURCE)
        .map_err(|error| format!("write embedded runtime source: {error}"))?;
    std::fs::write(output_directory.join("runtime.h"), RUNTIME_HEADER)
        .map_err(|error| format!("write embedded runtime header: {error}"))?;
    let started = Instant::now();
    run(
        Command::new("cc")
            .arg("-std=c11")
            .arg("-O2")
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
            .arg(&runtime_archive_path)
            .arg(&runtime_object_path),
        "archive native backend runtime",
    )?;
    let runtime_compilation = started.elapsed();

    let started = Instant::now();
    run(
        Command::new("cc")
            .arg("-std=c11")
            .arg("-O2")
            .arg("-I")
            .arg(&include_directory)
            .arg("-c")
            .arg(&data_source_path)
            .arg("-o")
            .arg(&data_object_path),
        "compile native backend program data",
    )?;
    let data_compilation = started.elapsed();

    let started = Instant::now();
    let mut link = Command::new("cc");
    link.args(&object_paths)
        .arg(&data_object_path)
        .arg(&runtime_archive_path)
        .arg("-o")
        .arg(executable_path);
    run(&mut link, "link native backend executable")?;
    let linking = started.elapsed();

    Ok(NativeArtifact {
        object_paths,
        data_object_path,
        runtime_archive_path,
        executable_path: executable_path.to_owned(),
        object_emission,
        runtime_compilation,
        data_compilation,
        linking,
    })
}

const RUNTIME_SOURCE: &str = include_str!("../../runtime/native/runtime.c");
const RUNTIME_HEADER: &str = include_str!("../../runtime/native/runtime.h");

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

fn emit_objects(core: &CoreProgram) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut modules = BTreeMap::<u32, Vec<u32>>::new();
    for node in &core.nodes {
        let file_id = core.origins[node.origin.raw() as usize].file_id.raw();
        modules.entry(file_id).or_default().push(node.id.raw());
    }

    let mut objects = Vec::with_capacity(modules.len() + 1);
    for (file_id, nodes) in modules {
        objects.push((
            format!("module-{file_id}.o"),
            emit_module_object(core, file_id, &nodes)?,
        ));
    }
    objects.push(("entry.o".to_owned(), emit_entry_object(core)?));
    Ok(objects)
}

fn new_object_module(name: &str) -> Result<ObjectModule, String> {
    let mut flags = settings::builder();
    flags
        .set("is_pic", "true")
        .map_err(|error| format!("enable position-independent native code: {error}"))?;
    let isa = cranelift_native::builder()
        .map_err(str::to_owned)?
        .finish(settings::Flags::new(flags))
        .map_err(|error| format!("finish native Cranelift target: {error}"))?;
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
) -> Result<Vec<u8>, String> {
    let mut module = new_object_module(&format!("chimi_module_{file_id}"))?;
    let node_signature = node_signature(&module);
    let runtime_signature = runtime_signature(&module);
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
        "record",
        "unary",
        "binary",
        "identity",
        "provision",
        "sequence",
        "closure",
        "match",
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

    for &node_id in node_ids {
        let node = &core.nodes[node_id as usize];
        define_node(
            &mut module,
            functions[&node_id],
            helpers[operation_name(&node.operation)],
            node_id,
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

fn define_node(
    module: &mut ObjectModule,
    function_id: FuncId,
    helper_id: FuncId,
    node_id: u32,
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
        let helper = module.declare_func_in_func(helper_id, builder.func);
        let node = builder.ins().iconst(types::I32, i64::from(node_id));
        let call = builder
            .ins()
            .call(helper, &[parameters[0], node, parameters[1], parameters[2]]);
        let result = builder.inst_results(call)[0];
        builder.ins().return_(&[result]);
        builder.seal_all_blocks();
        builder.finalize();
    }
    module
        .define_function(function_id, &mut context)
        .map_err(|error| format!("define native node {node_id}: {error}"))
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
        CoreOperation::Record(_) => "record",
        CoreOperation::Unary { .. } => "unary",
        CoreOperation::Binary { .. } => "binary",
        CoreOperation::Identity(_) => "identity",
        CoreOperation::Provision { .. } => "provision",
        CoreOperation::Sequence(_) => "sequence",
        CoreOperation::Closure { .. } => "closure",
        CoreOperation::Match { .. } => "match",
    }
}

#[cfg(test)]
mod tests {
    use super::parse_dependency;

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
