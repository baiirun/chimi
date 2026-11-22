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
use crate::ids::{ModuleId, SymbolId, TextRange};
use crate::resolution::ProjectSymbolKind;
use crate::semantic::{CallArgument, ModuleExpression};

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
                "a struct is built by field name: `{name} {{ {} }}`",
                fields
                    .iter()
                    .map(|field| format!("{field}: ..."))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
        if written_as_call && !fields.is_empty() {
            let replacement = format!(
                "{callee_text} {{ {} }}",
                fields
                    .iter()
                    .zip(arguments)
                    .map(|(field, argument)| format!("{field}: {}", slice(argument.value.range())))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            diagnostic = diagnostic.with_fix(range, replacement);
        }
        self.push_type_diagnostic(diagnostic);
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
        let mut fact = self.poison_expression_fact();
        fact.failures = self.union_failure_rows(facts.iter().map(|fact| fact.failures));
        fact.needs = self.union_needs_rows(facts.iter().map(|fact| fact.needs));
        fact
    }
}
