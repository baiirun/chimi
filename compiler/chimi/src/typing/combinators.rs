//! The contracts of `Task.all`, `Task.results`, and `Task.race`
//! (`docs/specification/time-and-outcomes.md`, "Task combinators").

use super::{CallableContract, CanonicalType, FailureRow, NeedsRow, ProjectTypeChecker};
use crate::ids::FailureRowId;
use crate::standard::{StandardFunction, StandardGeneric, StandardType};

impl ProjectTypeChecker {
    /// Registers the three combinator contracts over
    /// `List<fn() -> T errors E needs R>`:
    ///
    /// ```text
    /// Task.all:     -> List<T> errors E needs R
    /// Task.results: -> List<Result<T, E>> needs R
    /// Task.race:    -> T errors E needs R
    /// ```
    ///
    /// `Task.results` is registered only when `std/result` is loaded, which every
    /// program that imports `std/task` does (`standard_tree::imports`).
    pub(super) fn index_combinator_contracts(&mut self) {
        let value = self.standard_generic_type(StandardGeneric::TimeoutValue);
        let failures = self
            .resolved
            .standard_generic(StandardGeneric::TimeoutFailures);
        let needs = self
            .resolved
            .standard_generic(StandardGeneric::TimeoutNeeds);
        let operation_failures = self.intern_failure_row(FailureRow {
            variants: Vec::new(),
            variant_arguments: Vec::new(),
            parameters: vec![failures],
            exclusions: Vec::new(),
        });
        let operation_needs = self.intern_needs_row(NeedsRow {
            entries: Vec::new(),
            parameters: vec![needs],
        });
        let operation = self.intern_type(CanonicalType::Function(CallableContract {
            inputs: Vec::new(),
            success: value,
            failures: operation_failures,
            needs: operation_needs,
        }));
        let operations = self.builtin_type_with_arguments("List", vec![operation]);
        let values = self.builtin_type_with_arguments("List", vec![value]);
        let mut contracts = vec![
            (StandardFunction::TaskAll, values, operation_failures),
            (StandardFunction::TaskRace, value, operation_failures),
        ];
        if let Some(result) = self.resolved.loaded_standard_type(StandardType::Result) {
            let error = self.intern_type(CanonicalType::FailureSetValue(operation_failures));
            let outcome = self.intern_type(CanonicalType::Named {
                symbol: result,
                arguments: vec![value, error],
            });
            let outcomes = self.builtin_type_with_arguments("List", vec![outcome]);
            contracts.push((
                StandardFunction::TaskResults,
                outcomes,
                FailureRowId::new(0),
            ));
        }
        for (function, success, failures) in contracts {
            let symbol = self.standard_function_symbol(function);
            self.contracts.insert(
                symbol,
                CallableContract {
                    inputs: vec![operations],
                    success,
                    failures,
                    needs: operation_needs,
                },
            );
        }
    }
}
