//! Direct lowering of integer loops (R3, CHI-90).
//!
//! A `loop` whose state is an `Int` and whose body is integer arithmetic and
//! matches over literals compiles one pass to a Cranelift function
//! `int32_t pass(int64_t state, int64_t *out)`. The runtime's
//! `chimi_rt_direct_loop` drives it, so a pass costs a call and a few
//! instructions instead of one runtime helper and one allocation per node.
//!
//! A pass never defects. Where the runtime would (overflow, division by zero, a
//! match with no arm), the pass returns `FALLBACK` and the runtime reruns that
//! pass from the same state, which reports the defect exactly as the runtime
//! path does. Any node outside the forms below makes the loop ineligible, and it
//! runs through the runtime as before.

use crate::executable::{
    ConstantValue, CoreLiteral, CoreOperation, CorePattern, CoreProgram, CoreStep,
};
use crate::ids::SymbolId;
use crate::semantic::{BinaryOperator, UnaryOperator};
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{InstBuilder, MemFlags, Value, types};
use cranelift_frontend::FunctionBuilder;
use std::collections::HashMap;

/// The pass results `chimi_rt_direct_loop` expects (runtime.h).
const CONTINUE: i64 = 0;
const BREAK: i64 = 1;
const FALLBACK: i64 = 2;

/// An `Int` expression over the loop state and integer constants.
#[derive(Debug)]
pub(crate) enum IntExpression {
    Constant(i64),
    State,
    Negate(Box<IntExpression>),
    Arithmetic(BinaryOperator, Box<IntExpression>, Box<IntExpression>),
}

/// A `Bool` expression: a literal or an integer comparison.
#[derive(Debug)]
pub(crate) enum BoolExpression {
    Constant(bool),
    Compare(BinaryOperator, IntExpression, IntExpression),
}

#[derive(Debug)]
pub(crate) enum Subject {
    Int(IntExpression),
    Bool(BoolExpression),
}

/// What one arm tests its subject against.
#[derive(Debug)]
pub(crate) enum ArmPattern {
    Any,
    Int(i64),
    Bool(bool),
}

/// The outcome of a pass, from the body's tail position.
#[derive(Debug)]
pub(crate) enum Pass {
    Continue(IntExpression),
    Break(IntExpression),
    Match(Subject, Vec<(ArmPattern, Pass)>),
}

/// The loops in `core` that lower directly, keyed by loop node, with their passes.
pub(crate) fn direct_loops(core: &CoreProgram) -> HashMap<u32, Pass> {
    let constants: HashMap<SymbolId, i64> = core
        .constants
        .iter()
        .filter_map(|constant| match constant.value {
            ConstantValue::Integer(value) => Some((constant.symbol, value)),
            _ => None,
        })
        .collect();
    core.nodes
        .iter()
        .filter_map(|node| match &node.operation {
            CoreOperation::Loop { state, body, .. } => {
                let reader = Reader {
                    core,
                    constants: &constants,
                    state: *state,
                };
                reader.pass(body.raw()).map(|pass| (node.id.raw(), pass))
            }
            _ => None,
        })
        .collect()
}

struct Reader<'a> {
    core: &'a CoreProgram,
    constants: &'a HashMap<SymbolId, i64>,
    state: SymbolId,
}

impl Reader<'_> {
    fn operation(&self, node: u32) -> &CoreOperation {
        &self.core.nodes[node as usize].operation
    }

    fn pass(&self, node: u32) -> Option<Pass> {
        match self.operation(node) {
            CoreOperation::Identity(inner) => self.pass(inner.raw()),
            CoreOperation::Sequence(steps) => match steps.as_slice() {
                [CoreStep::Expression(inner)] => self.pass(inner.raw()),
                _ => None,
            },
            CoreOperation::Break {
                target,
                value: Some(value),
            } if *target == self.state => Some(Pass::Break(self.int(value.raw())?)),
            CoreOperation::Match { subjects, arms } => {
                let [subject] = subjects.as_slice() else {
                    return None;
                };
                let subject = match self.int(subject.raw()) {
                    Some(int) => Subject::Int(int),
                    None => Subject::Bool(self.bool(subject.raw())?),
                };
                let mut lowered = Vec::with_capacity(arms.len());
                for arm in arms {
                    let ([pattern], None) = (arm.patterns.as_slice(), arm.guard) else {
                        return None;
                    };
                    let pattern = match (pattern, &subject) {
                        (CorePattern::Wildcard, _) => ArmPattern::Any,
                        (CorePattern::Literal(CoreLiteral::Integer(value)), Subject::Int(_)) => {
                            ArmPattern::Int(*value)
                        }
                        (CorePattern::Literal(CoreLiteral::Boolean(value)), Subject::Bool(_)) => {
                            ArmPattern::Bool(*value)
                        }
                        _ => return None,
                    };
                    lowered.push((pattern, self.pass(arm.body.raw())?));
                }
                Some(Pass::Match(subject, lowered))
            }
            _ => Some(Pass::Continue(self.int(node)?)),
        }
    }

    fn int(&self, node: u32) -> Option<IntExpression> {
        match self.operation(node) {
            CoreOperation::Constant(CoreLiteral::Integer(value)) => {
                Some(IntExpression::Constant(*value))
            }
            CoreOperation::Reference(symbol) if *symbol == self.state => Some(IntExpression::State),
            CoreOperation::Reference(symbol) => self
                .constants
                .get(symbol)
                .map(|value| IntExpression::Constant(*value)),
            CoreOperation::Identity(inner) => self.int(inner.raw()),
            CoreOperation::Unary {
                operator: UnaryOperator::Negate,
                operand,
            } => Some(IntExpression::Negate(Box::new(self.int(operand.raw())?))),
            CoreOperation::Binary {
                operator:
                    operator @ (BinaryOperator::Add
                    | BinaryOperator::Subtract
                    | BinaryOperator::Multiply
                    | BinaryOperator::Divide
                    | BinaryOperator::Remainder),
                left,
                right,
            } => Some(IntExpression::Arithmetic(
                *operator,
                Box::new(self.int(left.raw())?),
                Box::new(self.int(right.raw())?),
            )),
            _ => None,
        }
    }

    fn bool(&self, node: u32) -> Option<BoolExpression> {
        match self.operation(node) {
            CoreOperation::Constant(CoreLiteral::Boolean(value)) => {
                Some(BoolExpression::Constant(*value))
            }
            CoreOperation::Identity(inner) => self.bool(inner.raw()),
            CoreOperation::Binary {
                operator:
                    operator @ (BinaryOperator::Less
                    | BinaryOperator::LessEqual
                    | BinaryOperator::Greater
                    | BinaryOperator::GreaterEqual
                    | BinaryOperator::Equal),
                left,
                right,
            } => Some(BoolExpression::Compare(
                *operator,
                self.int(left.raw())?,
                self.int(right.raw())?,
            )),
            _ => None,
        }
    }
}

/// Emits `pass` into a function whose parameters are `(state: i64, out: pointer)` and
/// whose result is the i32 pass result. The builder is positioned in the entry block.
pub(crate) fn emit_pass(builder: &mut FunctionBuilder<'_>, pass: &Pass, state: Value, out: Value) {
    let fallback = builder.create_block();
    emit_tail(builder, pass, state, out, fallback);
    builder.switch_to_block(fallback);
    finish(builder, FALLBACK);
}

fn finish(builder: &mut FunctionBuilder<'_>, result: i64) {
    let result = builder.ins().iconst(types::I32, result);
    builder.ins().return_(&[result]);
}

/// Emits `pass` from the current block; every path ends in a return or a jump to
/// `fallback`.
fn emit_tail(
    builder: &mut FunctionBuilder<'_>,
    pass: &Pass,
    state: Value,
    out: Value,
    fallback: cranelift_codegen::ir::Block,
) {
    match pass {
        Pass::Continue(expression) | Pass::Break(expression) => {
            let value = emit_int(builder, expression, state, fallback);
            builder.ins().store(MemFlags::trusted(), value, out, 0);
            finish(
                builder,
                if matches!(pass, Pass::Break(_)) {
                    BREAK
                } else {
                    CONTINUE
                },
            );
        }
        Pass::Match(subject, arms) => {
            let subject = match subject {
                Subject::Int(expression) => emit_int(builder, expression, state, fallback),
                Subject::Bool(expression) => emit_bool(builder, expression, state, fallback),
            };
            for (pattern, body) in arms {
                let matched = builder.create_block();
                let next = builder.create_block();
                match pattern {
                    ArmPattern::Any => {
                        builder.ins().jump(matched, &[]);
                    }
                    ArmPattern::Int(value) => {
                        let equal = builder.ins().icmp_imm(IntCC::Equal, subject, *value);
                        builder.ins().brif(equal, matched, &[], next, &[]);
                    }
                    ArmPattern::Bool(value) => {
                        let equal =
                            builder
                                .ins()
                                .icmp_imm(IntCC::Equal, subject, i64::from(*value));
                        builder.ins().brif(equal, matched, &[], next, &[]);
                    }
                }
                builder.switch_to_block(matched);
                emit_tail(builder, body, state, out, fallback);
                builder.switch_to_block(next);
            }
            builder.ins().jump(fallback, &[]);
        }
    }
}

/// An I8 0 or 1.
fn emit_bool(
    builder: &mut FunctionBuilder<'_>,
    expression: &BoolExpression,
    state: Value,
    fallback: cranelift_codegen::ir::Block,
) -> Value {
    match expression {
        BoolExpression::Constant(value) => builder.ins().iconst(types::I8, i64::from(*value)),
        BoolExpression::Compare(operator, left, right) => {
            let left = emit_int(builder, left, state, fallback);
            let right = emit_int(builder, right, state, fallback);
            let condition = match operator {
                BinaryOperator::Less => IntCC::SignedLessThan,
                BinaryOperator::LessEqual => IntCC::SignedLessThanOrEqual,
                BinaryOperator::Greater => IntCC::SignedGreaterThan,
                BinaryOperator::GreaterEqual => IntCC::SignedGreaterThanOrEqual,
                BinaryOperator::Equal => IntCC::Equal,
                _ => unreachable!("only integer comparisons are read as Bool expressions"),
            };
            builder.ins().icmp(condition, left, right)
        }
    }
}

/// An I64, branching to `fallback` wherever the runtime would defect.
fn emit_int(
    builder: &mut FunctionBuilder<'_>,
    expression: &IntExpression,
    state: Value,
    fallback: cranelift_codegen::ir::Block,
) -> Value {
    match expression {
        IntExpression::Constant(value) => builder.ins().iconst(types::I64, *value),
        IntExpression::State => state,
        IntExpression::Negate(operand) => {
            let operand = emit_int(builder, operand, state, fallback);
            let zero = builder.ins().iconst(types::I64, 0);
            let (value, overflow) = builder.ins().ssub_overflow(zero, operand);
            continue_unless(builder, overflow, fallback);
            value
        }
        IntExpression::Arithmetic(operator, left, right) => {
            let left = emit_int(builder, left, state, fallback);
            let right = emit_int(builder, right, state, fallback);
            match operator {
                BinaryOperator::Add | BinaryOperator::Subtract | BinaryOperator::Multiply => {
                    let (value, overflow) = match operator {
                        BinaryOperator::Add => builder.ins().sadd_overflow(left, right),
                        BinaryOperator::Subtract => builder.ins().ssub_overflow(left, right),
                        _ => builder.ins().smul_overflow(left, right),
                    };
                    continue_unless(builder, overflow, fallback);
                    value
                }
                BinaryOperator::Divide => {
                    // Division by zero and `Int` minimum / -1 defect in the runtime.
                    let zero = builder.ins().icmp_imm(IntCC::Equal, right, 0);
                    let minus_one = builder.ins().icmp_imm(IntCC::Equal, right, -1);
                    let minimum = builder.ins().icmp_imm(IntCC::Equal, left, i64::MIN);
                    let overflow = builder.ins().band(minus_one, minimum);
                    let defect = builder.ins().bor(zero, overflow);
                    continue_unless(builder, defect, fallback);
                    builder.ins().sdiv(left, right)
                }
                BinaryOperator::Remainder => {
                    // The runtime defects on `x % 0`. Cranelift's `srem` defines
                    // `Int` minimum % -1 as 0 without trapping, as the runtime does.
                    let zero = builder.ins().icmp_imm(IntCC::Equal, right, 0);
                    continue_unless(builder, zero, fallback);
                    builder.ins().srem(left, right)
                }
                _ => unreachable!("only integer arithmetic is read as an Int expression"),
            }
        }
    }
}

/// Branches to `fallback` when `defect` is set and continues in a new block otherwise.
fn continue_unless(
    builder: &mut FunctionBuilder<'_>,
    defect: Value,
    fallback: cranelift_codegen::ir::Block,
) {
    let next = builder.create_block();
    builder.ins().brif(defect, fallback, &[], next, &[]);
    builder.switch_to_block(next);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceSnapshot;
    use crate::{Compiler, ProjectSource};

    fn loops(source: &str) -> Vec<String> {
        let program = Compiler::prepare_backend(vec![
            ProjectSource::new(
                "test/main",
                SourceSnapshot::from_text("test/main.chimi", source),
            )
            .unwrap(),
        ])
        .unwrap();
        let mut found = direct_loops(&program.core)
            .into_values()
            .map(|pass| format!("{pass:?}"))
            .collect::<Vec<_>>();
        found.sort();
        found
    }

    #[test]
    fn reads_only_integer_loops_over_literal_matches() {
        let direct = loops(concat!(
            "const LIMIT: Int = 10\n",
            "pub fn main() -> Int {\n",
            "  loop count = 0 {\n",
            "    case count >= LIMIT {\n",
            "      True -> break count * 2\n",
            "      False -> count + 1\n",
            "    }\n",
            "  }\n",
            "}\n",
        ));
        assert_eq!(direct.len(), 1, "{direct:?}");
        assert!(direct[0].contains("Constant(10)"), "{direct:?}");
        let binding = loops(concat!(
            "pub fn main() -> Int {\n",
            "  loop count = 0 {\n",
            "    let next = count + 1\n",
            "    case next {\n",
            "      10 -> break next\n",
            "      _ -> next\n",
            "    }\n",
            "  }\n",
            "}\n",
        ));
        assert_eq!(binding, Vec::<String>::new());
        let outer_local = loops(concat!(
            "fn run(limit: Int) -> Int {\n",
            "  loop count = 0 {\n",
            "    case count == limit {\n",
            "      True -> break count\n",
            "      False -> count + 1\n",
            "    }\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int { run(3) }\n",
        ));
        assert_eq!(outer_local, Vec::<String>::new());
    }
}
