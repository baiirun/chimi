//! PROTOTYPE: disposable host-only Cranelift AOT candidate for the M8 decision.

use super::BackendProgram;
use super::prototype_program_data::render_program_data;
use crate::executable::{CoreOperation, CoreProgram};
use cranelift_codegen::Context;
use cranelift_codegen::ir::{AbiParam, Function, InstBuilder, Signature, UserFuncName, types};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{FuncId, Linkage, Module, default_libcall_names};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

pub struct NativePrototypeArtifact {
    pub object_path: PathBuf,
    pub runtime_archive_path: PathBuf,
    pub executable_path: PathBuf,
    pub object_emission: Duration,
    pub runtime_compilation: Duration,
    pub data_compilation: Duration,
    pub linking: Duration,
}

pub fn build_executable(
    program: &BackendProgram,
    output_directory: &Path,
) -> Result<NativePrototypeArtifact, String> {
    std::fs::create_dir_all(output_directory)
        .map_err(|error| format!("create prototype output directory: {error}"))?;
    let object_path = output_directory.join("program.o");
    let data_source_path = output_directory.join("program_data.c");
    let data_object_path = output_directory.join("program_data.o");
    let runtime_object_path = output_directory.join("runtime.o");
    let runtime_archive_path = output_directory.join("libchimi_rt.a");
    let executable_path = output_directory.join("program");
    let include_directory = prototype_directory();

    let started = Instant::now();
    let object = emit_object(&program.core)?;
    std::fs::write(&object_path, object)
        .map_err(|error| format!("write prototype object: {error}"))?;
    let object_emission = started.elapsed();

    std::fs::write(&data_source_path, render_program_data(&program.core, false))
        .map_err(|error| format!("write prototype program data: {error}"))?;
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
        "compile native prototype runtime",
    )?;
    run(
        Command::new("ar")
            .arg("rcs")
            .arg(&runtime_archive_path)
            .arg(&runtime_object_path),
        "archive native prototype runtime",
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
        "compile native prototype program data",
    )?;
    let data_compilation = started.elapsed();

    let started = Instant::now();
    run(
        Command::new("cc")
            .arg(&object_path)
            .arg(&data_object_path)
            .arg(&runtime_archive_path)
            .arg("-o")
            .arg(&executable_path),
        "link native prototype executable",
    )?;
    let linking = started.elapsed();

    Ok(NativePrototypeArtifact {
        object_path,
        runtime_archive_path,
        executable_path,
        object_emission,
        runtime_compilation,
        data_compilation,
        linking,
    })
}

fn prototype_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../prototypes/native-aot")
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

fn emit_object(core: &CoreProgram) -> Result<Vec<u8>, String> {
    let mut flags = settings::builder();
    flags
        .set("is_pic", "true")
        .map_err(|error| format!("enable position-independent native code: {error}"))?;
    let isa = cranelift_native::builder()
        .map_err(str::to_owned)?
        .finish(settings::Flags::new(flags))
        .map_err(|error| format!("finish native Cranelift target: {error}"))?;
    let builder = ObjectBuilder::new(isa, "chimi_aot_program", default_libcall_names())
        .map_err(|error| format!("create Cranelift object builder: {error}"))?;
    let mut module = ObjectModule::new(builder);
    let pointer = module.target_config().pointer_type();
    let call_conv = module.target_config().default_call_conv;
    let node_signature = Signature {
        params: vec![
            AbiParam::new(pointer),
            AbiParam::new(pointer),
            AbiParam::new(types::I64),
        ],
        returns: vec![AbiParam::new(pointer)],
        call_conv,
    };
    let runtime_signature = Signature {
        params: vec![
            AbiParam::new(pointer),
            AbiParam::new(types::I32),
            AbiParam::new(pointer),
            AbiParam::new(types::I64),
        ],
        returns: vec![AbiParam::new(pointer)],
        call_conv,
    };
    let mut nodes = Vec::with_capacity(core.nodes.len());
    for node in &core.nodes {
        nodes.push(
            module
                .declare_function(
                    &format!("chimi_node_{}", node.id.raw()),
                    Linkage::Export,
                    &node_signature,
                )
                .map_err(|error| format!("declare native node {}: {error}", node.id.raw()))?,
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
    for node in &core.nodes {
        define_node(
            &mut module,
            nodes[node.id.raw() as usize],
            *helpers
                .get(operation_name(&node.operation))
                .expect("helper exists"),
            node.id.raw(),
            node_signature.clone(),
        )?;
    }

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
    define_main(
        &mut module,
        main,
        runtime_main,
        nodes[entry.body.raw() as usize],
        main_signature,
    )?;
    module
        .finish()
        .emit()
        .map_err(|error| format!("emit native prototype object: {error}"))
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
