//! Immutable runtime metadata encoding shared by native emission and comparison tools.

use crate::executable::{
    ConstantFunction, ConstantValue, CoreLiteral, CoreOperation, CorePattern, CoreProgram,
    CoreStep, CoreSymbolKind,
};
use crate::semantic::{BinaryOperator, UnaryOperator};
use crate::standard::{StandardFunction, StandardSymbol};
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
    constant_values: Vec<(u32, u32, u32, u32, i64)>,
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
    fn named<'a>(&mut self, values: impl IntoIterator<Item = (&'a str, u32)>) -> (u32, u32) {
        let offset = self.named_nodes.len() as u32;
        for (name, node) in values {
            let name = self.string(name);
            self.named_nodes.push((name, node));
        }
        (offset, self.named_nodes.len() as u32 - offset)
    }
    /// Appends `value` after its children and returns its index, so the runtime can
    /// decode `CHIMI_CONSTANT_VALUES` front to back.
    fn constant_value(&mut self, value: &ConstantValue) -> u32 {
        let pairs = |data: &mut Self, entries: &[(String, ConstantValue)]| {
            let ids = entries
                .iter()
                .flat_map(|(name, value)| [data.string(name), data.constant_value(value)])
                .collect::<Vec<_>>();
            (data.ids(ids).0, entries.len() as u32)
        };
        let row = match value {
            ConstantValue::Void => (0, 0, 0, 0, 0),
            ConstantValue::Integer(value) => (1, 0, 0, 0, *value),
            ConstantValue::Duration(value) => (16, 0, 0, 0, *value),
            // Decoded by the process kernel (U5); until then the native build refuses
            // process programs before emitting data.
            ConstantValue::ProcessAddress(symbol) => (17, symbol.raw(), 0, 0, 0),
            // The runtime reads the IEEE 754 bits back with `memcpy`.
            ConstantValue::Float(value) => (2, 0, 0, 0, value.to_bits() as i64),
            ConstantValue::Text(value) => (3, self.string(value), 0, 0, 0),
            ConstantValue::Boolean(value) => (4, 0, 0, 0, i64::from(*value)),
            ConstantValue::List(values) => {
                let ids = values
                    .iter()
                    .map(|value| self.constant_value(value))
                    .collect::<Vec<_>>();
                let (offset, length) = self.ids(ids);
                (5, 0, offset, length, 0)
            }
            ConstantValue::Record(fields) => {
                let (offset, length) = pairs(self, fields);
                (6, 0, offset, length, 0)
            }
            ConstantValue::Range { start, end } => {
                let ids = vec![
                    self.constant_value(&ConstantValue::Integer(*start)),
                    self.constant_value(&ConstantValue::Integer(*end)),
                ];
                let (offset, length) = self.ids(ids);
                (15, 0, offset, length, 0)
            }
            ConstantValue::Tuple(values) => {
                let ids = values
                    .iter()
                    .map(|value| self.constant_value(value))
                    .collect::<Vec<_>>();
                let (offset, length) = self.ids(ids);
                (14, 0, offset, length, 0)
            }
            ConstantValue::Constructed {
                constructor,
                fields,
            } => {
                let ids = fields
                    .iter()
                    .map(|value| self.constant_value(value))
                    .collect::<Vec<_>>();
                let (offset, length) = self.ids(ids);
                (7, constructor.raw(), offset, length, 0)
            }
            ConstantValue::Map(entries) => {
                let ids = entries
                    .iter()
                    .flat_map(|(key, value)| [self.constant_value(key), self.constant_value(value)])
                    .collect::<Vec<_>>();
                let (offset, _) = self.ids(ids);
                (8, 0, offset, entries.len() as u32, 0)
            }
            ConstantValue::JsonNumber(text) => (9, self.string(text), 0, 0, 0),
            ConstantValue::Function(ConstantFunction::Declared {
                symbol,
                requirements,
            }) => {
                let ids = self.constant_requirements(requirements);
                let (offset, _) = self.ids(ids);
                (10, symbol.raw(), offset, 0, requirements.len() as i64)
            }
            ConstantValue::Function(ConstantFunction::Intrinsic(symbol)) => {
                (11, symbol.raw(), 0, 0, 0)
            }
            ConstantValue::Function(ConstantFunction::Constructor(symbol)) => {
                (12, symbol.raw(), 0, 0, 0)
            }
            ConstantValue::Function(ConstantFunction::Closure {
                node,
                captures,
                requirements,
            }) => {
                let mut ids = captures
                    .iter()
                    .flat_map(|(symbol, value)| [symbol.raw(), self.constant_value(value)])
                    .collect::<Vec<_>>();
                ids.extend(self.constant_requirements(requirements));
                let (offset, _) = self.ids(ids);
                (
                    13,
                    node.raw(),
                    offset,
                    captures.len() as u32,
                    requirements.len() as i64,
                )
            }
        };
        self.constant_values.push(row);
        self.constant_values.len() as u32 - 1
    }

    /// `(name string id, value id)` pairs of a function value's bound providers.
    fn constant_requirements(&mut self, requirements: &[(String, ConstantValue)]) -> Vec<u32> {
        requirements
            .iter()
            .flat_map(|(name, value)| [self.string(name), self.constant_value(value)])
            .collect()
    }

    fn pattern(&mut self, pattern: &CorePattern) -> u32 {
        let index = self.patterns.len() as u32;
        self.patterns.push((0, 0, 0, 0, 0));
        self.patterns[index as usize] = match pattern {
            CorePattern::Wildcard => (0, 0, 0, 0, 0),
            CorePattern::Binding(symbol) => (1, symbol.raw(), 0, 0, 0),
            CorePattern::Literal(literal) => match literal {
                CoreLiteral::Integer(value) => (2, 0, 0, 0, *value),
                CoreLiteral::Float(_) => unreachable!("Float has no literal patterns"),
                CoreLiteral::Text(value) => {
                    let text = self.string(value);
                    (2, 1, text, 0, 0)
                }
                CoreLiteral::Boolean(value) => (2, 2, 0, 0, i64::from(*value)),
                CoreLiteral::Void => unreachable!("a Void pattern lowers to a wildcard"),
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
            CorePattern::Alias { pattern, binding } => {
                let child = self.pattern(pattern);
                (6, binding.raw(), child, 1, 0)
            }
            CorePattern::Tuple(elements) => {
                let children = elements
                    .iter()
                    .map(|element| self.pattern(element))
                    .collect::<Vec<_>>();
                let (offset, length) = self.ids(children);
                (7, 0, offset, length, 0)
            }
            // Children are prefix, then the rest pattern when present, then suffix;
            // `a` counts the prefix and `integer` is 1 when a rest pattern is present.
            CorePattern::List {
                prefix,
                rest,
                suffix,
            } => {
                let children = prefix
                    .iter()
                    .chain(rest.as_deref())
                    .chain(suffix)
                    .map(|element| self.pattern(element))
                    .collect::<Vec<_>>();
                let (offset, length) = self.ids(children);
                (
                    5,
                    prefix.len() as u32,
                    offset,
                    length,
                    i64::from(rest.is_some()),
                )
            }
        };
        index
    }
}

pub(super) fn render_program_data(core: &CoreProgram, bytecode: bool) -> String {
    let mut data = ProgramData::default();
    let mut functions = Vec::new();
    for function in &core.functions {
        let (offset, length) = data.ids(function.parameters.iter().map(|symbol| symbol.raw()));
        functions.push((function.symbol.raw(), function.body.raw(), offset, length));
    }
    let mut constructors = Vec::new();
    let mut symbols = Vec::new();
    let mut local_slot_count = 0;
    let mut requirement_slot_count = 0;
    for symbol in &core.symbols {
        let name = data.string(&symbol.name);
        let function = core
            .functions
            .iter()
            .position(|item| item.symbol == symbol.id)
            .map_or(u32::MAX, |i| i as u32);
        let constant = core
            .constants
            .iter()
            .find(|item| item.symbol == symbol.id)
            .map_or(u32::MAX, |item| data.constant_value(&item.value));
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
            constructors.push((
                type_name,
                variant_name,
                offset,
                length,
                outcome,
                shape.ordinal,
            ));
            index
        } else {
            u32::MAX
        };
        let environment_slot = environment_slot(
            symbol.kind,
            &mut local_slot_count,
            &mut requirement_slot_count,
        );
        symbols.push((
            symbol_kind(symbol.kind),
            name,
            function,
            constant,
            constructor,
            match symbol.standard {
                Some(StandardSymbol::Function(function)) => function.runtime_id(),
                _ => u32::MAX,
            },
            match symbol.standard {
                Some(StandardSymbol::Constructor(constructor)) => constructor.runtime_id(),
                _ => u32::MAX,
            },
            match symbol.standard {
                Some(StandardSymbol::Type(kind)) => kind.runtime_id(),
                _ => u32::MAX,
            },
            environment_slot,
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
                // The runtime reads the IEEE 754 bits back with `memcpy`.
                CoreLiteral::Float(value) => {
                    row.1 = 3;
                    row.7 = value.to_bits() as i64;
                }
                CoreLiteral::Void => row.1 = 4,
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
                (row.5, row.6) = data.ids(arguments.iter().map(|id| id.raw()));
            }
            CoreOperation::Call { callee, arguments } => {
                row.1 = callee.raw();
                (row.5, row.6) = data.ids(arguments.iter().map(|id| id.raw()));
            }
            CoreOperation::List(elements) | CoreOperation::Tuple(elements) => {
                (row.5, row.6) = data.ids(elements.iter().map(|id| id.raw()))
            }
            CoreOperation::Record(fields) => {
                (row.5, row.6) = data.named(fields.iter().map(|f| (f.name.as_str(), f.value.raw())))
            }
            CoreOperation::Unary {
                operator: UnaryOperator::Negate,
                operand,
            } => row.1 = operand.raw(),
            CoreOperation::Unary {
                operator: UnaryOperator::Not,
                ..
            } => unreachable!("Core lowers `not` to a Bool match"),
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
                (row.5, row.6) =
                    data.named(providers.iter().map(|f| (f.name.as_str(), f.value.raw())));
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
            CoreOperation::Closure {
                parameters,
                captures,
                body,
            } => {
                row.1 = body.raw();
                (row.2, row.3) = data.ids(captures.iter().map(|id| id.raw()));
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
            CoreOperation::Loop {
                state,
                initial,
                body,
            } => {
                row.1 = state.raw();
                row.2 = initial.raw();
                row.3 = body.raw();
            }
            CoreOperation::Break { target, value } => {
                row.1 = target.raw();
                row.2 = value.map_or(u32::MAX, |value| value.raw());
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
    writeln!(
        output,
        "const uint32_t CHIMI_REQUIRES_SCHEDULER = {};",
        u32::from(requires_scheduler(core))
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
        |r| {
            format!(
                "{{{}, {}, {}, {}, {}, {}, {}, {}, {}}}",
                r.0, r.1, r.2, r.3, r.4, r.5, r.6, r.7, r.8
            )
        },
    );
    writeln!(
        output,
        "const uint32_t CHIMI_SYMBOL_COUNT = {};",
        symbols.len()
    )
    .unwrap();
    writeln!(
        output,
        "const uint32_t CHIMI_LOCAL_SLOT_COUNT = {local_slot_count};"
    )
    .unwrap();
    writeln!(
        output,
        "const uint32_t CHIMI_REQUIREMENT_SLOT_COUNT = {requirement_slot_count};"
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
        |r| format!("{{{}, {}, {}, {}, {}, {}}}", r.0, r.1, r.2, r.3, r.4, r.5),
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
    render_array(
        &mut output,
        "const ChimiConstantValue CHIMI_CONSTANT_VALUES[]",
        &data.constant_values,
        |r| format!("{{{}, {}, {}, {}, {}}}", r.0, r.1, r.2, r.3, r.4),
    );
    writeln!(
        output,
        "const uint32_t CHIMI_CONSTANT_VALUE_COUNT = {};",
        data.constant_values.len()
    )
    .unwrap();
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

fn requires_scheduler(core: &CoreProgram) -> bool {
    core.nodes.iter().any(|node| {
        let CoreOperation::Reference(symbol) = node.operation else {
            return false;
        };
        matches!(
            core.symbol(symbol).standard,
            Some(StandardSymbol::Function(
                StandardFunction::TaskStart
                    | StandardFunction::TaskResult
                    | StandardFunction::TaskCancel
                    | StandardFunction::ChannelSend
                    | StandardFunction::ChannelReceive
                    | StandardFunction::ChannelClose
                    | StandardFunction::ChannelAbort
                    | StandardFunction::ChannelBounded
            ))
        )
    })
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
        CoreOperation::Loop { .. } => 15,
        CoreOperation::Break { .. } => 16,
        CoreOperation::Tuple(_) => 17,
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
        CoreSymbolKind::Constant => 1,
        CoreSymbolKind::Requirement => 2,
        CoreSymbolKind::Local => 3,
        CoreSymbolKind::Constructor => 4,
        CoreSymbolKind::Intrinsic => 5,
        CoreSymbolKind::Other => 6,
        CoreSymbolKind::ProcessName => 7,
    }
}
fn environment_slot(
    kind: CoreSymbolKind,
    local_slot_count: &mut u32,
    requirement_slot_count: &mut u32,
) -> u32 {
    let count = match kind {
        CoreSymbolKind::Local => local_slot_count,
        CoreSymbolKind::Requirement => requirement_slot_count,
        _ => return u32::MAX,
    };
    let slot = *count;
    *count = count
        .checked_add(1)
        .expect("environment slot count fits u32");
    slot
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
        BinaryOperator::Remainder => 8,
        BinaryOperator::Equal => 9,
        BinaryOperator::Range => 10,
        BinaryOperator::RangeInclusive => 11,
        BinaryOperator::And | BinaryOperator::Or | BinaryOperator::NotEqual => {
            panic!("Core lowers `and`, `or`, and `!=` to a Bool match")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CoreSymbolKind, environment_slot};

    #[test]
    fn environment_slots_are_dense_and_independent_by_symbol_kind() {
        let mut locals = 0;
        let mut requirements = 0;
        let slots = [
            CoreSymbolKind::Function,
            CoreSymbolKind::Local,
            CoreSymbolKind::Requirement,
            CoreSymbolKind::Local,
            CoreSymbolKind::Constructor,
            CoreSymbolKind::Requirement,
        ]
        .map(|kind| environment_slot(kind, &mut locals, &mut requirements));

        assert_eq!(slots, [u32::MAX, 0, 0, 1, u32::MAX, 1]);
        assert_eq!(locals, 2);
        assert_eq!(requirements, 2);
    }
}
