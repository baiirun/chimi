//! Building structs by field name (`docs/specification/executable-kernel.md`,
//! "Structs").
//!
//! Lowering turns `Name { field: value }` for a struct of the module into the
//! constructor call with the values in declaration order (`CallForm::Fields`).
//! What is left for typing: a written positional construction or a constructor
//! used as a function value (`T4216`), and a braced literal whose name is not a
//! struct of the module (`T4217`).

use super::{ExpressionFact, ProjectTypeChecker};
use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::{ModuleId, SymbolId, TextRange, TypeId};
use crate::resolution::ProjectSymbolKind;
use crate::semantic::{CallArgument, CallForm, ModuleExpression, Pattern};

/// What `reject_positional_struct_pattern` reported.
pub(super) struct RejectedStructPattern {
    /// The diagnostic carries an edit that rewrites the whole pattern.
    pub(super) fixed: bool,
    /// The pattern names one sub-pattern per field.
    pub(super) arity_matches: bool,
}

impl ProjectTypeChecker {
    /// Whether `symbol` is a struct's constructor.
    pub(super) fn is_struct_constructor(&self, symbol: SymbolId) -> bool {
        self.resolved.symbol(symbol).kind == ProjectSymbolKind::StructConstructor
    }

    /// Reports `Money(2500)`: a struct is built by field name. Ensures: when the
    /// call writes one positional argument per field, the diagnostic carries
    /// the edit to `Money { cents: 2500 }`.
    pub(super) fn reject_positional_construction(
        &mut self,
        module_id: ModuleId,
        constructor: SymbolId,
        callee: &ModuleExpression,
        arguments: &[CallArgument],
        range: TextRange,
    ) {
        let name = self.resolved.symbol(constructor).name.clone();
        let fields = self
            .fields
            .get(&constructor)
            .map(|fields| fields.iter().map(|field| field.name.clone()).collect::<Vec<_>>())
            .unwrap_or_default();
        let module = &self.resolved.semantic.modules[module_id.raw() as usize];
        let file_id = module.source.file_id();
        let source = module.source.text();
        let slice = |range: TextRange| &source[range.start() as usize..range.end() as usize];
        let callee_text = slice(callee.range());
        // Only a call written as `Name(...)` is rewritten; a pipeline into a
        // constructor has its first argument elsewhere.
        let written_as_call = arguments.len() == fields.len()
            && arguments.iter().all(|argument| argument.label.is_none())
            && range.start() == callee.range().start()
            && slice(range).ends_with(')');
        let mut diagnostic = Diagnostic::error(
            DiagnosticStage::Type,
            "T4216",
            file_id,
            range,
            format!(
                "a struct is built by field name: `{}`",
                braced(
                    &name,
                    fields.iter().map(|field| format!("{field}: ...")).collect()
                )
            ),
        );
        if written_as_call {
            let replacement = braced(
                callee_text,
                fields
                    .iter()
                    .zip(arguments)
                    .map(|(field, argument)| format!("{field}: {}", slice(argument.value.range())))
                    .collect(),
            );
            // A `case` subject's `{` opens the arms, so a literal there is
            // bracketed.
            let replacement = if in_case_subject(source, range) {
                format!("({replacement})")
            } else {
                replacement
            };
            diagnostic = diagnostic.with_fix(range, replacement);
        }
        self.push_type_diagnostic(diagnostic);
    }

    /// Reports `Money(cents)` as a pattern: a struct's fields are matched by
    /// name. Ensures: when the pattern names one sub-pattern per field, the
    /// diagnostic carries the edit to `Money { cents: cents }`, with nested
    /// positional struct patterns rewritten in the same edit; a bare `Money`
    /// gets the edit to `Money { .. }`; and a pattern inside another one's
    /// edit (`outer_fixed`) gets no edit of its own. `None` when `pattern` is
    /// not a struct's.
    pub(super) fn reject_positional_struct_pattern(
        &mut self,
        module_id: ModuleId,
        pattern: &Pattern,
        arguments: &[Pattern],
        range: TextRange,
        outer_fixed: bool,
    ) -> Option<RejectedStructPattern> {
        let constructor = self.resolved_pattern_symbol(module_id, pattern)?;
        if !self.is_struct_constructor(constructor) {
            return None;
        }
        let name = self.resolved.symbol(constructor).name.clone();
        let fields = self.struct_field_names(constructor);
        let arity_matches = arguments.len() == fields.len();
        let module = &self.resolved.semantic.modules[module_id.raw() as usize];
        let file_id = module.source.file_id();
        let source = module.source.text();
        let written = &source[range.start() as usize..range.end() as usize];
        let mut diagnostic = Diagnostic::error(
            DiagnosticStage::Type,
            "T4216",
            file_id,
            range,
            format!(
                "a struct pattern names its fields: `{}`",
                braced(&name, fields.iter().map(|field| format!("{field}: ...")).collect())
            ),
        );
        let replacement = if outer_fixed {
            None
        } else if arguments.is_empty() && !written.contains('(') {
            Some(format!("{written} {{ .. }}"))
        } else if arity_matches && written.ends_with(')') {
            Some(self.braced_struct_pattern(module_id, pattern, arguments, range))
        } else {
            None
        };
        let fixed = replacement.is_some();
        if let Some(replacement) = replacement {
            diagnostic = diagnostic.with_fix(range, replacement);
        }
        self.push_type_diagnostic(diagnostic);
        Some(RejectedStructPattern {
            fixed,
            arity_matches,
        })
    }

    fn struct_field_names(&self, constructor: SymbolId) -> Vec<String> {
        self.fields
            .get(&constructor)
            .map(|fields| fields.iter().map(|field| field.name.clone()).collect())
            .unwrap_or_default()
    }

    /// `Name(a, b)` written as `Name { first: a, second: b }`, with each
    /// argument that is itself a positional struct pattern of the right field
    /// count rewritten the same way.
    fn braced_struct_pattern(
        &self,
        module_id: ModuleId,
        pattern: &Pattern,
        arguments: &[Pattern],
        range: TextRange,
    ) -> String {
        let source = self.resolved.semantic.modules[module_id.raw() as usize]
            .source
            .text();
        let written = &source[range.start() as usize..range.end() as usize];
        let callee = &written[..written.find('(').unwrap_or(written.len())];
        let fields = self
            .resolved_pattern_symbol(module_id, pattern)
            .map(|constructor| self.struct_field_names(constructor))
            .unwrap_or_default();
        braced(
            callee,
            fields
                .iter()
                .zip(arguments)
                .map(|(field, argument)| {
                    format!("{field}: {}", self.rewritten_sub_pattern(module_id, argument))
                })
                .collect(),
        )
    }

    /// `argument` as written, with every positional struct pattern in it, at
    /// any depth, rewritten to the braced form: one sub-pattern per field
    /// becomes `Name { field: pattern, .. }`, and a bare name `Name { .. }`.
    fn rewritten_sub_pattern(&self, module_id: ModuleId, argument: &Pattern) -> String {
        let source = self.resolved.semantic.modules[module_id.raw() as usize]
            .source
            .text();
        let range = argument.range();
        let written = &source[range.start() as usize..range.end() as usize];
        if let Pattern::Constructor {
            arguments,
            form: CallForm::Positional,
            ..
        } = argument
            && let Some(constructor) = self.resolved_pattern_symbol(module_id, argument)
            && self.is_struct_constructor(constructor)
        {
            let fields = self.struct_field_names(constructor).len();
            if fields == arguments.len() && written.ends_with(')') {
                return self.braced_struct_pattern(module_id, argument, arguments, range);
            }
            if arguments.is_empty() && !written.contains('(') {
                return format!("{written} {{ .. }}");
            }
        }
        let mut children = argument.children();
        children.sort_by_key(|child| std::cmp::Reverse(child.range().start()));
        let mut text = written.to_owned();
        for child in children {
            let child_range = child.range();
            let start = (child_range.start() - range.start()) as usize;
            let end = (child_range.end() - range.start()) as usize;
            text.replace_range(start..end, &self.rewritten_sub_pattern(module_id, child));
        }
        text
    }

    /// Reports field braces in a pattern that lowering could not order: an
    /// enum variant (whose payload is positional) or another module's struct.
    /// A struct of this module reports nothing more (lowering said why).
    pub(super) fn reject_unordered_field_pattern(
        &mut self,
        module_id: ModuleId,
        pattern: &crate::semantic::Pattern,
        range: TextRange,
    ) {
        let Some(symbol) = self.resolved_pattern_symbol(module_id, pattern) else {
            return;
        };
        let declared = self.resolved.symbol(symbol);
        let name = declared.name.clone();
        if declared.kind == ProjectSymbolKind::StructConstructor {
            if declared.module_id != Some(module_id) {
                self.type_error_range(
                    module_id,
                    "E_PRIVATE_STRUCT_CONSTRUCTOR",
                    range,
                    format!(
                        "`{name}` has module-private fields, so only its own module can match them"
                    ),
                );
            }
            return;
        }
        self.type_error_range(
            module_id,
            "T4217",
            range,
            format!(
                "`{name}` is an enum variant; a variant's payload is matched in parentheses, \
                 in declaration order"
            ),
        );
    }

    /// Reports a struct constructor used as a value, as `let make = Money`: a
    /// struct is built by field name, so its constructor is not a function.
    pub(super) fn reject_constructor_value(
        &mut self,
        module_id: ModuleId,
        constructor: SymbolId,
        range: TextRange,
    ) {
        let name = self.resolved.symbol(constructor).name.clone();
        self.type_error_range(
            module_id,
            "T4216",
            range,
            format!(
                "a struct is built by field name, so `{name}` is not a function; write \
                 `fn(...) {{ {name} {{ field: value, ... }} }}`"
            ),
        );
    }

    /// Checks `callee { field: value }` that lowering could not order. Ensures:
    /// a struct of this module reports nothing more (lowering said why), an
    /// enum variant or any other value is `T4217`, and every value is still
    /// inferred so its own errors are reported. The literal's type is poisoned.
    pub(super) fn infer_unordered_fields(
        &mut self,
        module_id: ModuleId,
        callee: &ModuleExpression,
        callee_symbol: Option<SymbolId>,
        arguments: &[CallArgument],
        generics: &std::collections::HashMap<String, SymbolId>,
        bindings: &mut std::collections::HashMap<SymbolId, super::ValueBinding>,
    ) -> ExpressionFact {
        let facts = arguments
            .iter()
            .map(|argument| self.infer_expression(module_id, &argument.value, generics, bindings))
            .collect::<Vec<_>>();
        if let Some(symbol) = callee_symbol {
            let declared = self.resolved.symbol(symbol);
            let kind = declared.kind;
            let name = declared.name.clone();
            let own_struct =
                kind == ProjectSymbolKind::StructConstructor && declared.module_id == Some(module_id);
            if !own_struct {
                let message = if kind == ProjectSymbolKind::StructConstructor {
                    format!(
                        "`{name}` has module-private fields, so only its own module can build it; \
                         use the module's public functions"
                    )
                } else if self.is_constructor_symbol(symbol) {
                    format!(
                        "`{name}` is an enum variant; a variant takes its payload in parentheses, \
                         in declaration order"
                    )
                } else {
                    format!("`{name}` is not a struct; braces with field names build a struct")
                };
                let code = if kind == ProjectSymbolKind::StructConstructor {
                    "E_PRIVATE_STRUCT_CONSTRUCTOR"
                } else {
                    "T4217"
                };
                self.type_error_range(module_id, code, callee.range(), message);
            }
        }
        if callee_symbol.is_none() {
            // A field or a call result; a name that does not resolve has its
            // own error, and its type is poisoned.
            let callee_fact = self.infer_expression(module_id, callee, generics, bindings);
            if callee_fact.success != TypeId::new(0) {
                self.type_error_range(
                    module_id,
                    "T4217",
                    callee.range(),
                    "this is not a struct; braces with field names build a struct".to_owned(),
                );
            }
        }
        self.poisoned_with(&facts)
    }

    /// Infers the values of a construction already reported, so their own
    /// errors still show, and gives the construction a poisoned type.
    pub(super) fn infer_rejected_fields(
        &mut self,
        module_id: ModuleId,
        arguments: &[CallArgument],
        generics: &std::collections::HashMap<String, SymbolId>,
        bindings: &mut std::collections::HashMap<SymbolId, super::ValueBinding>,
    ) -> ExpressionFact {
        let facts = arguments
            .iter()
            .map(|argument| self.infer_expression(module_id, &argument.value, generics, bindings))
            .collect::<Vec<_>>();
        self.poisoned_with(&facts)
    }

    /// A poisoned fact that keeps the failures and needs of `facts`.
    fn poisoned_with(&mut self, facts: &[ExpressionFact]) -> ExpressionFact {
        let mut fact = self.poison_expression_fact();
        fact.failures = self.union_failure_rows(facts.iter().map(|fact| fact.failures));
        fact.needs = self.union_needs_rows(facts.iter().map(|fact| fact.needs));
        fact
    }
}

/// `Name { a: 1, b: 2 }`, or `Name {}` for a struct without fields.
pub(super) fn braced(name: &str, fields: Vec<String>) -> String {
    if fields.is_empty() {
        format!("{name} {{}}")
    } else {
        format!("{name} {{ {} }}", fields.join(", "))
    }
}

/// Whether `range` lies between a `case` keyword and the `{` of its arms on
/// the same line, where a struct literal needs brackets.
pub(super) fn in_case_subject(source: &str, range: TextRange) -> bool {
    let before = &source[..range.start() as usize];
    let line = &before[before.rfind('\n').map_or(0, |index| index + 1)..];
    line.rfind("case ").is_some_and(|start| {
        let keyword_ends_word = start == 0
            || !line[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        keyword_ends_word && !line[start..].contains(['{', '}'])
    })
}
