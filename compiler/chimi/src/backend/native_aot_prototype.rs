//! PROTOTYPE: disposable host-only Cranelift AOT candidate for the M8 decision.

use super::BackendProgram;
use crate::executable::{
    CoreLiteral, CoreOperation, CorePattern, CoreProgram, CoreStep, CoreSymbolKind,
};
use crate::semantic::{BinaryOperator, UnaryOperator};
use cranelift_codegen::Context;
use cranelift_codegen::ir::{AbiParam, Function, InstBuilder, Signature, UserFuncName, types};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{FuncId, Linkage, Module, default_libcall_names};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::collections::HashMap;
use std::fmt::Write;
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

    std::fs::write(&data_source_path, render_program_data(&program.core))
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

#[derive(Default)]
struct ProgramData {
    strings: Vec<String>,
    string_ids: HashMap<String, u32>,
    ids: Vec<u32>,
    named_nodes: Vec<(u32, u32)>,
    steps: Vec<(u32, u32, u32)>,
    arms: Vec<(u32, u32, u32, u32)>,
    patterns: Vec<(u32, u32, u32, u32, i64)>,
}

impl ProgramData {
    fn string(&mut self, value: &str) -> u32 {
        if let Some(id) = self.string_ids.get(value) {
            return *id;
        }
        let id = self.strings.len() as u32;
        self.strings.push(value.to_owned());
        self.string_ids.insert(value.to_owned(), id);
        id
    }
    fn ids(&mut self, values: impl IntoIterator<Item = u32>) -> (u32, u32) {
        let offset = self.ids.len() as u32;
        self.ids.extend(values);
        (offset, self.ids.len() as u32 - offset)
    }
    fn named<'a>(
        &mut self,
        values: impl IntoIterator<Item = (Option<&'a str>, u32)>,
    ) -> (u32, u32) {
        let offset = self.named_nodes.len() as u32;
        for (name, node) in values {
            let name = name.map_or(u32::MAX, |name| self.string(name));
            self.named_nodes.push((name, node));
        }
        (offset, self.named_nodes.len() as u32 - offset)
    }
    fn pattern(&mut self, pattern: &CorePattern) -> u32 {
        let index = self.patterns.len() as u32;
        self.patterns.push((0, 0, 0, 0, 0));
        self.patterns[index as usize] = match pattern {
            CorePattern::Wildcard => (0, 0, 0, 0, 0),
            CorePattern::Binding(symbol) => (1, symbol.raw(), 0, 0, 0),
            CorePattern::Literal(literal) => match literal {
                CoreLiteral::Integer(value) => (2, 0, 0, 0, *value),
                CoreLiteral::Text(value) => {
                    let text = self.string(value);
                    (2, 1, text, 0, 0)
                }
                CoreLiteral::Boolean(value) => (2, 2, 0, 0, i64::from(*value)),
            },
            CorePattern::Constructor { symbol, fields } => {
                let children = fields
                    .iter()
                    .map(|field| self.pattern(field))
                    .collect::<Vec<_>>();
                let (offset, length) = self.ids(children);
                (3, symbol.raw(), offset, length, 0)
            }
            CorePattern::Outcome { success, value } => {
                let child = self.pattern(value);
                (4, u32::from(*success), child, 1, 0)
            }
        };
        index
    }
}

fn render_program_data(core: &CoreProgram) -> String {
    let mut data = ProgramData::default();
    let mut functions = Vec::new();
    for function in &core.functions {
        let (offset, length) = data.ids(function.parameters.iter().map(|symbol| symbol.raw()));
        functions.push((function.symbol.raw(), function.body.raw(), offset, length));
    }
    let mut constructors = Vec::new();
    let mut symbols = Vec::new();
    for symbol in &core.symbols {
        let name = data.string(&symbol.name);
        let function = core
            .functions
            .iter()
            .position(|item| item.symbol == symbol.id)
            .map_or(u32::MAX, |i| i as u32);
        let binding = core
            .bindings
            .iter()
            .find(|item| item.symbol == symbol.id)
            .map_or(u32::MAX, |item| item.value.raw());
        let constructor = if let Some(shape) = &symbol.constructor {
            let type_name = data.string(&shape.type_name);
            let variant_name = shape
                .variant_name
                .as_deref()
                .map_or(u32::MAX, |name| data.string(name));
            let field_ids = shape
                .fields
                .iter()
                .map(|field| data.string(field))
                .collect::<Vec<_>>();
            let (offset, length) = data.ids(field_ids);
            let outcome = shape.outcome_success.map_or(-1, i32::from);
            let index = constructors.len() as u32;
            constructors.push((type_name, variant_name, offset, length, outcome));
            index
        } else {
            u32::MAX
        };
        symbols.push((
            symbol_kind(symbol.kind),
            name,
            function,
            binding,
            constructor,
        ));
    }

    let mut nodes = Vec::new();
    for node in &core.nodes {
        let mut row = (node.origin.raw(), 0, 0, 0, 0, 0, 0, 0_i64);
        match &node.operation {
            CoreOperation::Constant(literal) => match literal {
                CoreLiteral::Integer(value) => {
                    row.1 = 0;
                    row.7 = *value;
                }
                CoreLiteral::Text(value) => {
                    row.1 = 1;
                    row.2 = data.string(value);
                }
                CoreLiteral::Boolean(value) => {
                    row.1 = 2;
                    row.7 = i64::from(*value);
                }
            },
            CoreOperation::Reference(symbol) => row.1 = symbol.raw(),
            CoreOperation::Select { receiver, member } => {
                row.1 = receiver.raw();
                row.2 = data.string(member);
            }
            CoreOperation::DependencyMethod {
                requirement,
                interface,
                method,
                name,
            } => {
                row.1 = requirement.raw();
                row.2 = interface.raw();
                row.3 = method.raw();
                row.4 = data.string(name);
            }
            CoreOperation::Construct {
                constructor,
                arguments,
            } => {
                row.1 = constructor.raw();
                (row.5, row.6) = data.named(
                    arguments
                        .iter()
                        .map(|a| (a.label.as_deref(), a.value.raw())),
                );
            }
            CoreOperation::Call { callee, arguments } => {
                row.1 = callee.raw();
                (row.5, row.6) = data.named(
                    arguments
                        .iter()
                        .map(|a| (a.label.as_deref(), a.value.raw())),
                );
            }
            CoreOperation::List(elements) => {
                (row.5, row.6) = data.ids(elements.iter().map(|id| id.raw()))
            }
            CoreOperation::Record(fields) => {
                (row.5, row.6) = data.named(
                    fields
                        .iter()
                        .map(|f| (Some(f.name.as_str()), f.value.raw())),
                )
            }
            CoreOperation::Unary {
                operator: UnaryOperator::Negate,
                operand,
            } => row.1 = operand.raw(),
            CoreOperation::Binary {
                operator,
                left,
                right,
            } => {
                row.1 = binary_operator(*operator);
                row.2 = left.raw();
                row.3 = right.raw();
            }
            CoreOperation::Identity(value) => row.1 = value.raw(),
            CoreOperation::Provision {
                expression,
                providers,
            } => {
                row.1 = expression.raw();
                (row.5, row.6) = data.named(
                    providers
                        .iter()
                        .map(|f| (Some(f.name.as_str()), f.value.raw())),
                );
            }
            CoreOperation::Sequence(sequence) => {
                row.5 = data.steps.len() as u32;
                for step in sequence {
                    match step {
                        CoreStep::Bind { symbol, value } => {
                            data.steps.push((0, symbol.raw(), value.raw()))
                        }
                        CoreStep::Expression(value) => data.steps.push((1, u32::MAX, value.raw())),
                    }
                }
                row.6 = data.steps.len() as u32 - row.5;
            }
            CoreOperation::Closure { parameters, body } => {
                row.1 = body.raw();
                (row.5, row.6) = data.ids(parameters.iter().map(|id| id.raw()));
            }
            CoreOperation::Match { subjects, arms } => {
                (row.5, row.6) = data.ids(subjects.iter().map(|id| id.raw()));
                row.1 = data.arms.len() as u32;
                for arm in arms {
                    let patterns = arm
                        .patterns
                        .iter()
                        .map(|pattern| data.pattern(pattern))
                        .collect::<Vec<_>>();
                    let (offset, length) = data.ids(patterns);
                    data.arms.push((
                        offset,
                        length,
                        arm.guard.map_or(u32::MAX, |id| id.raw()),
                        arm.body.raw(),
                    ));
                }
                row.2 = data.arms.len() as u32 - row.1;
            }
        }
        nodes.push(row);
    }

    let mut output = String::from(
        "/* Generated by the disposable native AOT prototype. */\n#include \"runtime.h\"\n\n",
    );
    writeln!(
        output,
        "const char *CHIMI_COMPUTATION = {};",
        c_string(&core.computation)
    )
    .unwrap();
    render_array(
        &mut output,
        "const char *CHIMI_STRINGS[]",
        &data.strings,
        |value| c_string(value),
    );
    render_array(
        &mut output,
        "const uint32_t CHIMI_IDS[]",
        &data.ids,
        u32::to_string,
    );
    render_array(
        &mut output,
        "const ChimiOrigin CHIMI_ORIGINS[]",
        &core.origins,
        |origin| {
            format!(
                "{{{}, {}, {}, {}}}",
                origin.file_id.raw(),
                origin.semantic_id.raw(),
                origin.range.start(),
                origin.range.end()
            )
        },
    );
    render_array(&mut output, "const ChimiNode CHIMI_NODES[]", &nodes, |r| {
        format!(
            "{{{}, {}, {}, {}, {}, {}, {}, {}}}",
            r.0, r.1, r.2, r.3, r.4, r.5, r.6, r.7
        )
    });
    writeln!(output, "const uint32_t CHIMI_NODE_COUNT = {};", nodes.len()).unwrap();
    render_array(
        &mut output,
        "const ChimiSymbol CHIMI_SYMBOLS[]",
        &symbols,
        |r| format!("{{{}, {}, {}, {}, {}}}", r.0, r.1, r.2, r.3, r.4),
    );
    writeln!(
        output,
        "const uint32_t CHIMI_SYMBOL_COUNT = {};",
        symbols.len()
    )
    .unwrap();
    render_array(
        &mut output,
        "const ChimiFunction CHIMI_FUNCTIONS[]",
        &functions,
        |r| format!("{{{}, {}, {}, {}}}", r.0, r.1, r.2, r.3),
    );
    writeln!(
        output,
        "const uint32_t CHIMI_FUNCTION_COUNT = {};",
        functions.len()
    )
    .unwrap();
    render_array(
        &mut output,
        "const ChimiConstructor CHIMI_CONSTRUCTORS[]",
        &constructors,
        |r| format!("{{{}, {}, {}, {}, {}}}", r.0, r.1, r.2, r.3, r.4),
    );
    render_array(
        &mut output,
        "const ChimiNamedNode CHIMI_NAMED_NODES[]",
        &data.named_nodes,
        |r| format!("{{{}, {}}}", r.0, r.1),
    );
    render_array(
        &mut output,
        "const ChimiStep CHIMI_STEPS[]",
        &data.steps,
        |r| format!("{{{}, {}, {}}}", r.0, r.1, r.2),
    );
    render_array(
        &mut output,
        "const ChimiArm CHIMI_ARMS[]",
        &data.arms,
        |r| format!("{{{}, {}, {}, {}}}", r.0, r.1, r.2, r.3),
    );
    render_array(
        &mut output,
        "const ChimiPattern CHIMI_PATTERNS[]",
        &data.patterns,
        |r| format!("{{{}, {}, {}, {}, {}}}", r.0, r.1, r.2, r.3, r.4),
    );
    for node in &core.nodes {
        writeln!(
            output,
            "extern ChimiControl *chimi_node_{}(ChimiRuntime *, ChimiEnvironment *, uint64_t);",
            node.id.raw()
        )
        .unwrap();
    }
    output.push_str("ChimiNodeFn CHIMI_NODE_FUNCTIONS[] = {");
    for node in &core.nodes {
        writeln!(output, "\n  chimi_node_{},", node.id.raw()).unwrap();
    }
    output.push_str("\n};\n");
    output
}

fn render_array<T>(
    output: &mut String,
    declaration: &str,
    values: &[T],
    render: impl Fn(&T) -> String,
) {
    writeln!(output, "{declaration} = {{").unwrap();
    if values.is_empty() {
        output.push_str("  {0},\n");
    } else {
        for value in values {
            writeln!(output, "  {},", render(value)).unwrap();
        }
    }
    output.push_str("};\n");
}

fn c_string(value: &str) -> String {
    let mut output = String::from("\"");
    for byte in value.bytes() {
        match byte {
            b'"' => output.push_str("\\\""),
            b'\\' => output.push_str("\\\\"),
            b'\n' => output.push_str("\\n"),
            0x20..=0x7e => output.push(char::from(byte)),
            _ => write!(output, "\\{:03o}", byte).unwrap(),
        }
    }
    output.push('"');
    output
}

const fn symbol_kind(kind: CoreSymbolKind) -> u32 {
    match kind {
        CoreSymbolKind::Function => 0,
        CoreSymbolKind::Binding => 1,
        CoreSymbolKind::Requirement => 2,
        CoreSymbolKind::Local => 3,
        CoreSymbolKind::Constructor => 4,
        CoreSymbolKind::Other => 5,
    }
}
const fn binary_operator(operator: BinaryOperator) -> u32 {
    match operator {
        BinaryOperator::Add => 0,
        BinaryOperator::Subtract => 1,
        BinaryOperator::Multiply => 2,
        BinaryOperator::Divide => 3,
        BinaryOperator::Less => 4,
        BinaryOperator::LessEqual => 5,
        BinaryOperator::Greater => 6,
        BinaryOperator::GreaterEqual => 7,
        BinaryOperator::Pipe => 8,
    }
}
