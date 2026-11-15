//! Build-time `Duration` values and the checks that use them (`T4348`, `T4349`, `T4350`),
//! plus `T4345`, the one other check of a call's literal argument.
//!
//! `ConstantEvaluator` folds an expression the compiler can know without running the
//! program: literals, module constants, `Duration.zero`, `Int` arithmetic, and calls of
//! the `std/duration` constructors, conversions, and arithmetic. Process typing reuses it
//! for the literal `shutdown_within` and restart-window checks (`T4263`).
//!
//! Every value comes from `crate::duration`, the model the interpreter runs and the native
//! runtime mirrors, so a build-time verdict cannot disagree with the run-time one.

use super::TypedProject;
use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::duration::{self, ParseFailure};
use crate::ids::{ModuleId, SymbolId};
use crate::numeric::{self, ArithmeticOperator};
use crate::resolution::{Namespace, ProjectSymbolKind, ReferenceResolution};
use crate::semantic::{
    Literal, ModuleBlockItem, ModuleExpression, UnaryOperator, interpolated_expressions,
};
use crate::standard::{
    DurationUnitRole, StandardConstant, StandardFunction, StandardModule, StandardSymbol,
};
use std::collections::{HashMap, HashSet};

/// A value known at build time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Folded {
    Int(i64),
    /// Nanoseconds.
    Duration(i64),
    Text(String),
}

/// Why an expression has no build-time value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Unfolded {
    /// It depends on a value the program computes, or on an operation the evaluator
    /// does not model.
    NotConstant,
    /// Its operations are all constant but one of them always defects; the text is that
    /// operation's run-time defect message.
    Defect(String),
}

pub(crate) struct ConstantEvaluator<'typed> {
    typed: &'typed TypedProject,
    /// The constants being folded, so a reference cycle stops instead of recursing.
    active: HashSet<SymbolId>,
    folded: HashMap<SymbolId, Result<Folded, Unfolded>>,
}

impl<'typed> ConstantEvaluator<'typed> {
    pub(crate) fn new(typed: &'typed TypedProject) -> Self {
        Self {
            typed,
            active: HashSet::new(),
            folded: HashMap::new(),
        }
    }

    /// The build-time value of `expression` in `module_id`.
    ///
    /// Ensures: never runs a source function; a value it returns is the value the program
    /// computes at run time, and `Defect` is the defect it reports there.
    pub(crate) fn fold(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
    ) -> Result<Folded, Unfolded> {
        match expression {
            ModuleExpression::Literal(Literal::Integer { value, .. }) => Ok(Folded::Int(*value)),
            ModuleExpression::Literal(Literal::Text { value, .. }) => {
                Ok(Folded::Text(value.clone()))
            }
            ModuleExpression::Group { expression, .. } => self.fold(module_id, expression),
            ModuleExpression::Unary {
                operator: UnaryOperator::Negate,
                operand,
                ..
            } => match self.fold(module_id, operand)? {
                Folded::Int(value) => numeric::integer_negate(value)
                    .map(Folded::Int)
                    .map_err(Unfolded::Defect),
                _ => Err(Unfolded::NotConstant),
            },
            ModuleExpression::Binary {
                operator,
                left,
                right,
                ..
            } => {
                let operator =
                    ArithmeticOperator::from_binary(*operator).ok_or(Unfolded::NotConstant)?;
                match (self.fold(module_id, left)?, self.fold(module_id, right)?) {
                    (Folded::Int(left), Folded::Int(right)) => {
                        numeric::integer_binary(operator, left, right)
                            .map(Folded::Int)
                            .map_err(Unfolded::Defect)
                    }
                    _ => Err(Unfolded::NotConstant),
                }
            }
            ModuleExpression::Name { .. } | ModuleExpression::Select { .. } => {
                let symbol = self
                    .value_symbol(module_id, expression)
                    .ok_or(Unfolded::NotConstant)?;
                self.symbol_value(symbol)
            }
            ModuleExpression::Call {
                callee, arguments, ..
            } => {
                let function = self
                    .duration_function(module_id, callee)
                    .ok_or(Unfolded::NotConstant)?;
                let mut values = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    values.push(self.fold(module_id, &argument.value)?);
                }
                apply(function, &values)
            }
            _ => Err(Unfolded::NotConstant),
        }
    }

    /// The `std/duration` function `callee` names, if it names one.
    pub(crate) fn duration_function(
        &self,
        module_id: ModuleId,
        callee: &ModuleExpression,
    ) -> Option<StandardFunction> {
        let symbol = self.value_symbol(module_id, callee)?;
        match self.typed.resolved.standard_symbol(symbol)? {
            StandardSymbol::Function(function)
                if function.module() == Some(StandardModule::Duration) =>
            {
                Some(function)
            }
            _ => None,
        }
    }

    fn symbol_value(&mut self, symbol: SymbolId) -> Result<Folded, Unfolded> {
        if let Some(StandardSymbol::Constant(StandardConstant::DurationZero)) =
            self.typed.resolved.standard_symbol(symbol)
        {
            return Ok(Folded::Duration(0));
        }
        if let Some(folded) = self.folded.get(&symbol) {
            return folded.clone();
        }
        let project_symbol = self.typed.resolved.symbol(symbol);
        let (ProjectSymbolKind::Constant, Some(module_id), Some(semantic_id)) = (
            project_symbol.kind,
            project_symbol.module_id,
            project_symbol.semantic_id,
        ) else {
            return Err(Unfolded::NotConstant);
        };
        if !self.active.insert(symbol) {
            return Err(Unfolded::NotConstant);
        }
        let body = self.typed.resolved.semantic.modules[module_id.raw() as usize]
            .declarations
            .iter()
            .find(|declaration| declaration.semantic_id == semantic_id)
            .and_then(|declaration| declaration.body.as_ref());
        let folded = match body {
            Some(body) => self.fold(module_id, body),
            None => Err(Unfolded::NotConstant),
        };
        self.active.remove(&symbol);
        self.folded.insert(symbol, folded.clone());
        folded
    }

    fn value_symbol(&self, module_id: ModuleId, expression: &ModuleExpression) -> Option<SymbolId> {
        match expression {
            ModuleExpression::Name { semantic_id, .. }
            | ModuleExpression::Select { semantic_id, .. } => {
                match self.typed.resolved.reference_resolution(
                    module_id,
                    *semantic_id,
                    Namespace::Value,
                )? {
                    ReferenceResolution::Resolved(symbol) => Some(symbol),
                    _ => None,
                }
            }
            ModuleExpression::Group { expression, .. } => self.value_symbol(module_id, expression),
            _ => None,
        }
    }

    fn is_constant_reference(&self, module_id: ModuleId, expression: &ModuleExpression) -> bool {
        self.value_symbol(module_id, expression)
            .is_some_and(|symbol| {
                self.typed.resolved.symbol(symbol).kind == ProjectSymbolKind::Constant
            })
    }
}

/// A `std/duration` call over build-time arguments. `parse` is not folded: it can fail,
/// and its constant checks are `T4349` and `T4350`.
fn apply(function: StandardFunction, arguments: &[Folded]) -> Result<Folded, Unfolded> {
    let defect =
        |result: Result<i64, String>| result.map(Folded::Duration).map_err(Unfolded::Defect);
    match (function.duration_unit(), function, arguments) {
        (Some((unit, DurationUnitRole::Constructor)), _, [Folded::Int(count)]) => {
            defect(duration::construct(unit, *count))
        }
        (Some((unit, DurationUnitRole::Conversion)), _, [Folded::Duration(value)]) => {
            Ok(Folded::Int(duration::whole_units(*value, unit)))
        }
        (None, StandardFunction::DurationAdd, [Folded::Duration(l), Folded::Duration(r)]) => {
            defect(duration::add(*l, *r))
        }
        (None, StandardFunction::DurationSubtract, [Folded::Duration(l), Folded::Duration(r)]) => {
            defect(duration::subtract(*l, *r))
        }
        (None, StandardFunction::DurationMultiply, [Folded::Duration(l), Folded::Int(r)]) => {
            defect(duration::multiply(*l, *r))
        }
        (None, StandardFunction::DurationDivide, [Folded::Duration(l), Folded::Int(r)]) => {
            defect(duration::divide(*l, *r))
        }
        (None, StandardFunction::DurationNegate, [Folded::Duration(value)]) => {
            defect(duration::negate(*value))
        }
        (None, StandardFunction::DurationAbsolute, [Folded::Duration(value)]) => {
            defect(duration::absolute(*value))
        }
        (None, StandardFunction::DurationToText, [Folded::Duration(value)]) => {
            Ok(Folded::Text(duration::to_text(*value)))
        }
        _ => Err(Unfolded::NotConstant),
    }
}

/// `T4348`, `T4349`, and `T4350` for every call in the bodies of `modules`, as
/// (errors, warnings).
///
/// Ensures: an argument that is not a literal or a module constant draws none of them.
pub(super) fn check(
    typed: &TypedProject,
    modules: &HashSet<ModuleId>,
) -> (Vec<Diagnostic>, Vec<Diagnostic>) {
    let mut checker = Checker {
        evaluator: ConstantEvaluator::new(typed),
        errors: Vec::new(),
        warnings: Vec::new(),
    };
    for module in &typed.resolved.semantic.modules {
        if !modules.contains(&module.id) {
            continue;
        }
        for body in module
            .declarations
            .iter()
            .filter_map(|declaration| declaration.body.as_ref())
        {
            checker.visit(module.id, body);
        }
    }
    (checker.errors, checker.warnings)
}

struct Checker<'typed> {
    evaluator: ConstantEvaluator<'typed>,
    errors: Vec<Diagnostic>,
    warnings: Vec<Diagnostic>,
}

impl Checker<'_> {
    fn visit(&mut self, module_id: ModuleId, expression: &ModuleExpression) {
        let mut visit = |child: &ModuleExpression| self.visit(module_id, child);
        match expression {
            ModuleExpression::Literal(_)
            | ModuleExpression::Name { .. }
            | ModuleExpression::Empty { .. } => {}
            ModuleExpression::Select { receiver, .. } => visit(receiver),
            ModuleExpression::Call {
                callee, arguments, ..
            } => {
                visit(callee);
                for argument in arguments {
                    visit(&argument.value);
                }
                self.check_call(module_id, expression);
            }
            ModuleExpression::List { elements, .. }
            | ModuleExpression::Tuple { elements, .. }
            | ModuleExpression::Sequence {
                expressions: elements,
                ..
            } => elements.iter().for_each(visit),
            ModuleExpression::Record { fields, .. } => {
                fields.iter().for_each(|field| visit(&field.value));
            }
            ModuleExpression::Update { base, fields, .. } => {
                visit(base);
                fields.iter().for_each(|field| visit(&field.value));
            }
            ModuleExpression::Unary { operand, .. } => visit(operand),
            ModuleExpression::Binary { left, right, .. } => {
                visit(left);
                visit(right);
            }
            ModuleExpression::Group { expression, .. }
            | ModuleExpression::Function {
                body: expression, ..
            } => visit(expression),
            ModuleExpression::Using {
                expression,
                providers,
                ..
            } => {
                visit(expression);
                providers.iter().for_each(|provider| visit(&provider.value));
            }
            ModuleExpression::Block { items, .. } => {
                for item in items {
                    match item {
                        ModuleBlockItem::Binding { value, .. }
                        | ModuleBlockItem::Expression(value) => {
                            visit(value);
                        }
                    }
                }
            }
            ModuleExpression::Case { subjects, arms, .. } => {
                subjects.iter().for_each(&mut visit);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        visit(guard);
                    }
                    visit(&arm.body);
                }
            }
            ModuleExpression::Loop { initial, body, .. } => {
                visit(initial);
                visit(body);
            }
            ModuleExpression::Break { value, .. } => {
                if let Some(value) = value {
                    visit(value);
                }
            }
            ModuleExpression::Interpolation { parts, .. } => {
                interpolated_expressions(parts).for_each(visit);
            }
        }
    }

    /// `T4345`: an empty list literal passed to `Task.race`, which always defects.
    fn is_race_of_empty_literal(
        &self,
        module_id: ModuleId,
        callee: &ModuleExpression,
        arguments: &[crate::semantic::CallArgument],
    ) -> bool {
        let is_race = self
            .evaluator
            .value_symbol(module_id, callee)
            .and_then(|symbol| self.evaluator.typed.resolved.standard_symbol(symbol))
            == Some(StandardSymbol::Function(StandardFunction::TaskRace));
        is_race
            && matches!(
                arguments,
                [argument] if matches!(&argument.value, ModuleExpression::List { elements, .. } if elements.is_empty())
            )
    }

    fn check_call(&mut self, module_id: ModuleId, call: &ModuleExpression) {
        let ModuleExpression::Call {
            callee, arguments, ..
        } = call
        else {
            return;
        };
        let file_id = self.evaluator.typed.resolved.semantic.modules[module_id.raw() as usize]
            .source
            .file_id();
        let range = call.range();
        if self.is_race_of_empty_literal(module_id, callee, arguments) {
            self.errors.push(Diagnostic::error(
                DiagnosticStage::Type,
                "T4345",
                file_id,
                range,
                "`Task.race` of an empty list has no operation to win, so this call always \
                 defects",
            ));
            return;
        }
        let Some(function) = self.evaluator.duration_function(module_id, callee) else {
            return;
        };
        let [argument] = arguments.as_slice() else {
            return;
        };
        if matches!(
            function.duration_unit(),
            Some((_, DurationUnitRole::Constructor))
        ) {
            let Ok(Folded::Int(_)) = self.evaluator.fold(module_id, &argument.value) else {
                return;
            };
            if let Err(Unfolded::Defect(message)) = self.evaluator.fold(module_id, call) {
                self.errors.push(Diagnostic::error(
                    DiagnosticStage::Type,
                    "T4348",
                    file_id,
                    range,
                    format!(
                        "{message}, which holds about 292 years either way; this call always \
                         defects"
                    ),
                ));
            }
            return;
        }
        if function != StandardFunction::DurationParse {
            return;
        }
        let literal = matches!(
            &argument.value,
            ModuleExpression::Literal(Literal::Text { .. })
        ) || self
            .evaluator
            .is_constant_reference(module_id, &argument.value);
        let Ok(Folded::Text(text)) = self.evaluator.fold(module_id, &argument.value) else {
            return;
        };
        if !literal {
            return;
        }
        match duration::parse(&text) {
            Ok(_) => {
                let expression = duration::constructor_expression(&text)
                    .expect("valid duration text has a constructor expression");
                self.warnings.push(Diagnostic::warning(
                    DiagnosticStage::Type,
                    "T4350",
                    file_id,
                    range,
                    format!(
                        "this `Duration.parse` of constant text always succeeds; write \
                         `{expression}`, which cannot fail and needs no failure handling"
                    ),
                ));
            }
            Err(failure) => {
                let (variant, reason) = match failure {
                    ParseFailure::Empty => ("Empty", "the text is empty"),
                    ParseFailure::Invalid => (
                        "Invalid",
                        "a duration is parts such as `1h30m` or `250ms`, with units \
                         (`h`, `m`, `s`, `ms`, `us`, `ns`) in descending order and exact fractions",
                    ),
                    ParseFailure::OutOfRange => (
                        "OutOfRange",
                        "it is outside the Duration range of about 292 years either way",
                    ),
                };
                self.errors.push(Diagnostic::error(
                    DiagnosticStage::Type,
                    "T4349",
                    file_id,
                    range,
                    format!(
                        "`Duration.parse({text:?})` always fails with \
                         `Duration.ParseError.{variant}`: {reason}"
                    ),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ConstantEvaluator, Folded, Unfolded};
    use crate::semantic::ModuleDeclarationKind;
    use crate::{ProjectSource, source::SourceSnapshot};

    /// Folds the initializer of each `const` in `source`, by name.
    fn fold_constants(source: &str) -> Vec<(String, Result<Folded, Unfolded>)> {
        let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
            .expect("valid module path");
        let typed =
            crate::typing::check_project(crate::compiler::resolve_project_sources(vec![module]));
        let mut evaluator = ConstantEvaluator::new(&typed);
        let module = &typed.resolved.semantic.modules[0];
        module
            .declarations
            .iter()
            .filter(|declaration| declaration.kind == ModuleDeclarationKind::Constant)
            .map(|declaration| {
                let body = declaration.body.as_ref().expect("a constant has a body");
                (
                    declaration.name.text.clone(),
                    evaluator.fold(module.id, body),
                )
            })
            .collect()
    }

    // The contract process typing relies on for T4263: literal constructors, constants,
    // `Duration.zero`, and folded arithmetic fold; anything a program computes does not.
    #[test]
    fn the_evaluator_folds_constant_durations_and_nothing_computed() {
        let folded = fold_constants(
            "import std/duration as Duration\n\
             const base = Duration.seconds(30)\n\
             const minutes = 2\n\
             const window = Duration.add(base, Duration.minutes(minutes * 3))\n\
             const zero = Duration.zero\n\
             const negative = Duration.negate(Duration.milliseconds(-1))\n\
             const shifted = Duration.subtract(Duration.zero, Duration.nanoseconds(1))\n\
             const huge = Duration.hours(9223372036854775807)\n\
             const computed = Duration.seconds(length())\n\
             fn length() -> Int { 3 }\n",
        );
        let expected = [
            ("base", Ok(Folded::Duration(30_000_000_000))),
            ("minutes", Ok(Folded::Int(2))),
            ("window", Ok(Folded::Duration(390_000_000_000))),
            ("zero", Ok(Folded::Duration(0))),
            ("negative", Ok(Folded::Duration(1_000_000))),
            ("shifted", Ok(Folded::Duration(-1))),
            (
                "huge",
                Err(Unfolded::Defect(
                    "`Duration.hours(9223372036854775807)` overflows the Duration range".to_owned(),
                )),
            ),
            ("computed", Err(Unfolded::NotConstant)),
        ];
        for ((name, value), (expected_name, expected_value)) in folded.iter().zip(expected) {
            assert_eq!(name, expected_name);
            assert_eq!(value, &expected_value, "{name}");
        }
    }
}
