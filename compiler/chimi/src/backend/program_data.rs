//! Immutable runtime metadata encoding shared by native emission and comparison tools.

use crate::executable::{
    CoreEffect, CoreLiteral, CoreOperation, CorePattern, CoreProgram, CoreStep, CoreSymbolKind,
};
use crate::semantic::{BinaryOperator, UnaryOperator};
use std::collections::HashMap;
use std::fmt::Write;

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

pub(super) fn render_program_data(core: &CoreProgram, bytecode: bool) -> String {
    let mut data = ProgramData::default();
    let mut functions = Vec::new();
    let mut function_effects = Vec::new();
    for function in &core.functions {
        let (offset, length) = data.ids(function.parameters.iter().map(|symbol| symbol.raw()));
        functions.push((function.symbol.raw(), function.body.raw(), offset, length));
        let (effect_parameter_offset, effect_parameter_length) =
            data.ids(function.effect_parameters.iter().map(|symbol| symbol.raw()));
        function_effects.push((
            u32::from(function.effects.contains(&CoreEffect::Io)),
            effect_parameter_offset,
            effect_parameter_length,
        ));
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
        "/* Generated immutable Chimi runtime metadata. */\n#include \"runtime.h\"\n\n",
    );
    writeln!(
        output,
        "const char *CHIMI_COMPUTATION = {};",
        c_string(&core.computation)
    )
    .unwrap();
    render_array(
        &mut output,
        "const uint32_t CHIMI_FUNCTION_EFFECTS[][3]",
        &function_effects,
        |r| format!("{{{}, {}, {}}}", r.0, r.1, r.2),
    );
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
    if bytecode {
        render_array(
            &mut output,
            "const uint8_t CHIMI_OPCODES[]",
            &core.nodes,
            |node| operation_opcode(&node.operation).to_string(),
        );
        let entry = core
            .functions
            .iter()
            .find(|function| function.symbol == core.entry)
            .expect("well-formed core has entry");
        writeln!(
            output,
            "const uint32_t CHIMI_ENTRY_NODE = {};",
            entry.body.raw()
        )
        .unwrap();
        output.push_str(
            "extern ChimiControl *chimi_bytecode_eval(uint32_t, ChimiRuntime *, ChimiEnvironment *, uint64_t);\n",
        );
        for node in &core.nodes {
            writeln!(
                output,
                "static ChimiControl *chimi_node_{}(ChimiRuntime *runtime, ChimiEnvironment *environment, uint64_t parent_span) {{ return chimi_bytecode_eval({}, runtime, environment, parent_span); }}",
                node.id.raw(),
                node.id.raw()
            )
            .unwrap();
        }
    } else {
        for node in &core.nodes {
            writeln!(
                output,
                "extern ChimiControl *chimi_node_{}(ChimiRuntime *, ChimiEnvironment *, uint64_t);",
                node.id.raw()
            )
            .unwrap();
        }
    }
    output.push_str("ChimiNodeFn CHIMI_NODE_FUNCTIONS[] = {");
    for node in &core.nodes {
        writeln!(output, "\n  chimi_node_{},", node.id.raw()).unwrap();
    }
    output.push_str("\n};\n");
    output
}

const fn operation_opcode(operation: &CoreOperation) -> u8 {
    match operation {
        CoreOperation::Constant(_) => 0,
        CoreOperation::Reference(_) => 1,
        CoreOperation::Select { .. } => 2,
        CoreOperation::DependencyMethod { .. } => 3,
        CoreOperation::Construct { .. } => 4,
        CoreOperation::Call { .. } => 5,
        CoreOperation::List(_) => 6,
        CoreOperation::Record(_) => 7,
        CoreOperation::Unary { .. } => 8,
        CoreOperation::Binary { .. } => 9,
        CoreOperation::Identity(_) => 10,
        CoreOperation::Provision { .. } => 11,
        CoreOperation::Sequence(_) => 12,
        CoreOperation::Closure { .. } => 13,
        CoreOperation::Match { .. } => 14,
    }
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
