//! Immutable runtime metadata encoding shared by native emission and comparison tools.

use crate::executable::{
    ConstantFunction, ConstantValue, CoreLiteral, CoreOperation, CorePattern, CoreProgram,
    CoreStep, CoreSymbolKind,
};
use crate::ids::{CoreNodeId, SymbolId};
use crate::semantic::{BinaryOperator, UnaryOperator};
use crate::standard::{StandardModule, StandardSymbol, StandardType};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

#[derive(Default)]
struct ProgramData {
    strings: Vec<String>,
    string_ids: HashMap<String, u32>,
    ids: Vec<u32>,
    named_nodes: Vec<(u32, u32)>,
    steps: Vec<(u32, u32, u32)>,
    arms: Vec<(u32, u32, u32, u32, u32, u32)>,
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
            // The scale with the sign in bit 8, then the magnitude's high 64
            // bits as two halves and its low 64 bits.
            ConstantValue::Decimal(value) => {
                let (negative, magnitude, scale) = value.parts();
                let high = (magnitude >> 64) as u64;
                (
                    18,
                    scale | (u32::from(negative) << 8),
                    (high >> 32) as u32,
                    high as u32,
                    magnitude as u64 as i64,
                )
            }
            // A named address: the runtime's name id is the symbol index plus 1.
            ConstantValue::ProcessAddress(symbol) => (17, symbol.raw(), 0, 0, 0),
            // The runtime reads the IEEE 754 bits back with `memcpy`.
            ConstantValue::Float(value) => (2, 0, 0, 0, value.to_bits() as i64),
            ConstantValue::Text(value) => (3, self.string(value), 0, 0, 0),
            ConstantValue::Bytes(hex) => (20, self.string(hex), 0, 0, 0),
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
            ConstantValue::Set(items) => {
                let ids = items
                    .iter()
                    .map(|item| self.constant_value(item))
                    .collect::<Vec<_>>();
                let (offset, length) = self.ids(ids);
                (19, 0, offset, length, 0)
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
    let origin_locations = core
        .origin_locations
        .iter()
        .map(|location| data.string(location))
        .collect::<Vec<_>>();
    let locals = local_slots(core);
    let mut functions = Vec::new();
    for (function, local_count) in core.functions.iter().zip(&locals.function_counts) {
        let (offset, length) = data.ids(function.parameters.iter().map(|symbol| symbol.raw()));
        functions.push((
            function.symbol.raw(),
            function.body.raw(),
            offset,
            length,
            *local_count,
        ));
    }
    let mut constructors = Vec::new();
    let mut symbols = Vec::new();
    let mut requirement_slot_count = 0_u32;
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
        let environment_slot = match symbol.kind {
            CoreSymbolKind::Local => locals.slots[symbol.id.raw() as usize],
            CoreSymbolKind::Requirement => {
                let slot = requirement_slot_count;
                requirement_slot_count = slot
                    .checked_add(1)
                    .expect("environment slot count fits u32");
                slot
            }
            _ => u32::MAX,
        };
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
            // `b` is the cause local's symbol plus one, 0 without one; `c` is 1
            // when the construction keeps that cause rather than linking to it.
            CoreOperation::Construct {
                constructor,
                arguments,
                cause,
            } => {
                row.1 = constructor.raw();
                if let Some(cause) = cause {
                    row.2 = cause.local.raw() + 1;
                    row.3 = u32::from(cause.keep);
                }
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
                row.4 = locals.closure_count(node.id);
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
                    let (cause_subject, cause_local) = arm
                        .cause
                        .map_or((u32::MAX, u32::MAX), |cause| {
                            (cause.subject as u32, cause.local.raw())
                        });
                    data.arms.push((
                        offset,
                        length,
                        arm.guard.map_or(u32::MAX, |id| id.raw()),
                        arm.body.raw(),
                        cause_subject,
                        cause_local,
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
        "const uint32_t CHIMI_STRING_LENGTHS[]",
        &data.strings,
        |value| value.len().to_string(),
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
        &core.origins.iter().zip(&origin_locations).collect::<Vec<_>>(),
        |(origin, location)| {
            format!(
                "{{{}, {}, {}, {}, {}}}",
                origin.file_id.raw(),
                origin.semantic_id.raw(),
                origin.range.start(),
                origin.range.end(),
                location
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
        "const uint32_t CHIMI_CLOCK_REQUIREMENT_SLOT = {requirement_slot_count};"
    )
    .unwrap();
    writeln!(
        output,
        "const uint32_t CHIMI_REQUIREMENT_SLOT_COUNT = {};",
        requirement_slot_count + 1
    )
    .unwrap();
    render_array(
        &mut output,
        "const ChimiFunction CHIMI_FUNCTIONS[]",
        &functions,
        |r| format!("{{{}, {}, {}, {}, {}}}", r.0, r.1, r.2, r.3, r.4),
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
        |r| format!("{{{}, {}, {}, {}, {}, {}}}", r.0, r.1, r.2, r.3, r.4, r.5),
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

/// Whether any reachable node calls into `std/process` or `std/supervisor`, or
/// names a `process name`: such a program links the process-kernel runtime.
/// `Clock.sleep` runs on the scheduler alone.
pub(super) fn requires_process_runtime(core: &CoreProgram) -> bool {
    core.nodes.iter().any(|node| {
        let CoreOperation::Reference(symbol) = node.operation else {
            return false;
        };
        let symbol = core.symbol(symbol);
        symbol.kind == CoreSymbolKind::ProcessName
            || matches!(
                symbol.standard,
                Some(StandardSymbol::Function(function)) if matches!(
                    function.module(),
                    Some(StandardModule::Process | StandardModule::Supervisor)
                )
            )
    })
}

fn requires_scheduler(core: &CoreProgram) -> bool {
    core.nodes
        .iter()
        .any(|node| reaches_scheduler(core, &node.operation))
}

/// Whether one operation needs the fiber scheduler: a standard function that runs
/// on it, or a read of the `clock` need, whose execution provider is the
/// scheduler's clock.
pub(crate) fn reaches_scheduler(core: &CoreProgram, operation: &CoreOperation) -> bool {
    match operation {
        CoreOperation::Reference(symbol) => {
            let symbol = core.symbol(*symbol);
            matches!(
                symbol.standard,
                Some(StandardSymbol::Function(function)) if function.requires_scheduler()
            ) || symbol.kind == CoreSymbolKind::Requirement && symbol.name == "clock"
        }
        CoreOperation::DependencyMethod { interface, .. } => matches!(
            core.symbol(*interface).standard,
            Some(StandardSymbol::Type(StandardType::Clock))
        ),
        _ => false,
    }
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
/// Where each local lives in a native environment, and how large each
/// environment is.
struct LocalSlots {
    /// By symbol ID; `u32::MAX` for a symbol that is not a local.
    slots: Vec<u32>,
    /// By function, in `core.functions` order.
    function_counts: Vec<u32>,
    /// By closure node: the local count of the body it is written in.
    closure_counts: HashMap<CoreNodeId, u32>,
    /// The largest body's local count.
    largest: u32,
}

impl LocalSlots {
    /// A closure no body contains never runs; the largest count covers any slot.
    fn closure_count(&self, closure: CoreNodeId) -> u32 {
        self.closure_counts
            .get(&closure)
            .copied()
            .unwrap_or(self.largest)
    }
}

/// An environment only ever holds the locals of one body: a function's, or a
/// constant initializer's, with the closures written inside it. So each body
/// numbers its locals from the same start, and each environment has one slot
/// per local of its body. A local that more than one body names, or that no
/// body names, gets a slot below that start, which no body reuses.
fn local_slots(core: &CoreProgram) -> LocalSlots {
    let roots = core
        .functions
        .iter()
        .map(|function| (function.parameters.as_slice(), function.body))
        .chain(
            core.constant_initializers
                .iter()
                .map(|initializer| (&[][..], initializer.value)),
        );
    let mut bodies = Vec::new();
    let mut named_by = vec![0_u32; core.symbols.len()];
    for (parameters, body) in roots {
        let body = body_locals(core, parameters, body);
        for symbol in &body.locals {
            named_by[symbol.raw() as usize] += 1;
        }
        bodies.push(body);
    }
    let mut slots = vec![u32::MAX; core.symbols.len()];
    let mut shared = 0_u32;
    for symbol in &core.symbols {
        if symbol.kind == CoreSymbolKind::Local && named_by[symbol.id.raw() as usize] != 1 {
            slots[symbol.id.raw() as usize] = shared;
            shared += 1;
        }
    }
    let mut counts = Vec::with_capacity(bodies.len());
    let mut closure_counts = HashMap::new();
    for body in &bodies {
        let mut next = shared;
        for symbol in &body.locals {
            if named_by[symbol.raw() as usize] == 1 {
                slots[symbol.raw() as usize] = next;
                next += 1;
            }
        }
        counts.push(next);
        for closure in &body.closures {
            let count = closure_counts.entry(*closure).or_insert(0);
            *count = next.max(*count);
        }
    }
    let largest = counts.iter().copied().max().unwrap_or(shared);
    counts.truncate(core.functions.len());
    LocalSlots {
        slots,
        function_counts: counts,
        closure_counts,
        largest,
    }
}

struct BodyLocals {
    /// Distinct, in first-seen order.
    locals: Vec<SymbolId>,
    closures: Vec<CoreNodeId>,
}

/// The locals `parameters` and the nodes under `body` bind or read, and the
/// closures written there.
fn body_locals(core: &CoreProgram, parameters: &[SymbolId], body: CoreNodeId) -> BodyLocals {
    let mut seen = HashSet::new();
    let mut locals = Vec::new();
    let mut closures = Vec::new();
    let mut add = |symbol: SymbolId, locals: &mut Vec<SymbolId>| {
        if core.symbol(symbol).kind == CoreSymbolKind::Local && seen.insert(symbol) {
            locals.push(symbol);
        }
    };
    for symbol in parameters {
        add(*symbol, &mut locals);
    }
    let mut pending = vec![body];
    let mut patterns = Vec::new();
    while let Some(node) = pending.pop() {
        match &core.nodes[node.raw() as usize].operation {
            CoreOperation::Constant(_) | CoreOperation::DependencyMethod { .. } => {}
            CoreOperation::Reference(symbol) => add(*symbol, &mut locals),
            CoreOperation::Select { receiver, .. } => pending.push(*receiver),
            CoreOperation::Construct {
                arguments, cause, ..
            } => {
                pending.extend(arguments);
                if let Some(cause) = cause {
                    add(cause.local, &mut locals);
                }
            }
            CoreOperation::Call { callee, arguments } => {
                pending.push(*callee);
                pending.extend(arguments);
            }
            CoreOperation::List(items) | CoreOperation::Tuple(items) => pending.extend(items),
            CoreOperation::Record(fields) => {
                pending.extend(fields.iter().map(|field| field.value));
            }
            CoreOperation::Unary { operand, .. } | CoreOperation::Identity(operand) => {
                pending.push(*operand);
            }
            CoreOperation::Binary { left, right, .. } => pending.extend([*left, *right]),
            CoreOperation::Provision {
                expression,
                providers,
            } => {
                pending.push(*expression);
                pending.extend(providers.iter().map(|field| field.value));
            }
            CoreOperation::Sequence(steps) => {
                for step in steps {
                    match step {
                        CoreStep::Bind { symbol, value } => {
                            add(*symbol, &mut locals);
                            pending.push(*value);
                        }
                        CoreStep::Expression(value) => pending.push(*value),
                    }
                }
            }
            CoreOperation::Closure {
                parameters,
                captures,
                body,
            } => {
                for symbol in parameters.iter().chain(captures) {
                    add(*symbol, &mut locals);
                }
                closures.push(node);
                pending.push(*body);
            }
            CoreOperation::Match { subjects, arms } => {
                pending.extend(subjects);
                for arm in arms {
                    if let Some(cause) = arm.cause {
                        add(cause.local, &mut locals);
                    }
                    patterns.extend(&arm.patterns);
                    pending.extend(arm.guard);
                    pending.push(arm.body);
                }
            }
            CoreOperation::Loop {
                state,
                initial,
                body,
            } => {
                add(*state, &mut locals);
                pending.extend([*initial, *body]);
            }
            CoreOperation::Break { target, value } => {
                add(*target, &mut locals);
                pending.extend(value);
            }
        }
        while let Some(pattern) = patterns.pop() {
            match pattern {
                CorePattern::Wildcard | CorePattern::Literal(_) => {}
                CorePattern::Binding(symbol) => add(*symbol, &mut locals),
                CorePattern::Constructor { fields, .. } | CorePattern::Tuple(fields) => {
                    patterns.extend(fields);
                }
                CorePattern::Outcome { value, .. } => patterns.push(value),
                CorePattern::List {
                    prefix,
                    rest,
                    suffix,
                } => {
                    patterns.extend(prefix);
                    patterns.extend(rest.as_deref());
                    patterns.extend(suffix);
                }
                CorePattern::Alias { pattern, binding } => {
                    add(*binding, &mut locals);
                    patterns.push(pattern);
                }
            }
        }
    }
    BodyLocals { locals, closures }
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
    use super::{body_locals, local_slots};
    use crate::compiler::{ProjectSource, resolve_project_sources};
    use crate::executable::{CoreOperation, lower, seal_entry};
    use crate::source::SourceSnapshot;
    use crate::typing::check_project;
    use std::collections::HashSet;

    // An environment holds one body's locals, so each local's slot must be below
    // its body's count and the locals of one body must never share a slot; a body
    // with few locals must get a small environment whatever other bodies declare.
    #[test]
    fn each_body_gets_distinct_slots_and_its_own_count() {
        let source = concat!(
            "fn wide(x: Int) -> Int {\n",
            "  let a = x\n  let b = a + 1\n  let c = b + 1\n  let d = c + 1\n",
            "  let add = fn(y: Int) { y + d }\n",
            "  case add(a) {\n    0 -> b\n    n -> n + c\n  }\n",
            "}\n",
            "fn step(i: Int) -> Int { i + 1 }\n",
            "pub fn main() -> Int { step(wide(1)) }\n",
        );
        let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
            .expect("valid module path");
        let root = seal_entry(check_project(resolve_project_sources(vec![module])))
            .unwrap_or_else(|_| panic!("the program compiles"));
        let core = lower(root).unwrap_or_else(|_| panic!("the program lowers"));
        let slots = local_slots(&core);
        let mut counts = Vec::new();
        for (function, count) in core.functions.iter().zip(&slots.function_counts) {
            let body = body_locals(&core, &function.parameters, function.body);
            let taken = body
                .locals
                .iter()
                .map(|symbol| slots.slots[symbol.raw() as usize])
                .collect::<Vec<_>>();
            assert!(
                taken.iter().all(|slot| slot < count),
                "{taken:?} within {count}"
            );
            assert_eq!(taken.iter().collect::<HashSet<_>>().len(), taken.len());
            for closure in &body.closures {
                assert_eq!(slots.closure_count(*closure), *count);
                assert!(matches!(
                    core.nodes[closure.raw() as usize].operation,
                    CoreOperation::Closure { .. }
                ));
            }
            counts.push((core.symbol(function.symbol).name.clone(), *count));
        }
        let count_of = |name: &str| {
            counts
                .iter()
                .find(|(function, _)| function == name)
                .map(|(_, count)| *count)
                .expect("the function is lowered")
        };
        assert!(count_of("step") < count_of("wide"), "{counts:?}");
    }
}
