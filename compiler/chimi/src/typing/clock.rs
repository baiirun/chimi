//! The contracts of `std/clock` beyond `Clock.sleep`, of `Task.timeout` and
//! `Task.deadline`, and the fields of the structural `Clock` interface
//! (`docs/specification/time-and-outcomes.md`, "Time sources" and "Timeouts and
//! deadlines").

use super::{
    CallableContract, CanonicalType, FailureRow, FieldShape, Need, NeedsRow, ProjectTypeChecker,
};
use crate::ids::{FailureRowId, NeedsRowId, SemanticId, TypeId};
use crate::standard::{StandardConstructor, StandardFunction, StandardGeneric, StandardType};

impl ProjectTypeChecker {
    /// Registers the clock and timeout contracts and the `Clock` interface fields.
    pub(super) fn index_clock_contracts(&mut self) {
        use StandardFunction as F;
        let int = self.builtin_type("Int");
        let text = self.builtin_type("Text");
        let void = self.builtin_type("Void");
        let duration = self.standard_named_type(StandardType::Duration, Vec::new());
        let instant = self.standard_named_type(StandardType::Instant, Vec::new());
        let timestamp = self.standard_named_type(StandardType::Timestamp, Vec::new());
        let clock = self.standard_named_type(StandardType::Clock, Vec::new());
        let none = FailureRowId::new(0);
        let pure = NeedsRowId::new(0);

        // The interface itself: `now`, `timestamp`, and `sleep_until`.
        let method = |checker: &mut Self, inputs: Vec<TypeId>, success: TypeId| {
            checker.intern_type(CanonicalType::Function(CallableContract {
                inputs,
                success,
                failures: none,
                needs: pure,
            }))
        };
        let now_method = method(self, Vec::new(), instant);
        let timestamp_method = method(self, Vec::new(), timestamp);
        let sleep_until_method = method(self, vec![instant], void);
        let clock_symbol = self.resolved.standard_type(StandardType::Clock);
        self.fields.insert(
            clock_symbol,
            [
                ("now", now_method),
                ("timestamp", timestamp_method),
                ("sleep_until", sleep_until_method),
            ]
            .into_iter()
            .enumerate()
            .map(|(index, (name, type_id))| FieldShape {
                semantic_id: SemanticId::new(index as u32),
                name: name.to_owned(),
                type_id,
            })
            .collect(),
        );

        let mut contracts = vec![
            (F::ClockAdd, vec![instant, duration], instant, none, pure),
            (
                F::ClockBetween,
                vec![instant, instant],
                duration,
                none,
                pure,
            ),
            (F::ClockTimestampToText, vec![timestamp], text, none, pure),
            (F::ClockUnixNanoseconds, vec![timestamp], int, none, pure),
            (
                F::ClockFromUnixNanoseconds,
                vec![int],
                timestamp,
                none,
                pure,
            ),
            (
                F::ClockTimestampBetween,
                vec![timestamp, timestamp],
                duration,
                none,
                pure,
            ),
            (
                F::ClockStartingAt,
                vec![clock, timestamp],
                clock,
                none,
                pure,
            ),
        ];
        for (function, inputs, success) in [
            (F::ClockNow, Vec::new(), instant),
            (F::ClockTimestamp, Vec::new(), timestamp),
            (F::ClockSince, vec![instant], duration),
            (F::ClockSleepUntil, vec![instant], void),
        ] {
            let needs = self.clock_need(function, Vec::new());
            contracts.push((function, inputs, success, none, needs));
        }

        // Task.timeout: fn<T, E, R>(fn() -> T errors E needs R, Duration)
        //   -> T errors E | Timeout needs R + { clock: Clock }; Task.deadline takes
        // an Instant instead. The `clock` need is the spec's contract, so code
        // that times out needs time like code that sleeps; the native runtime
        // still arms the scope timer on the execution clock, because only a
        // scheduler timer can cancel the scope (decisions.md, CHI-201).
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
        let timed_out = self.standard_constructor_symbol(StandardConstructor::TimeoutTimedOut);
        let result_failures = self.intern_failure_row(FailureRow {
            variants: vec![timed_out],
            variant_arguments: Vec::new(),
            parameters: vec![failures],
            exclusions: Vec::new(),
        });
        for (function, limit) in [(F::TaskTimeout, duration), (F::TaskDeadline, instant)] {
            let result_needs = self.clock_need(function, vec![needs]);
            contracts.push((
                function,
                vec![operation, limit],
                value,
                result_failures,
                result_needs,
            ));
        }

        for (function, inputs, success, failures, needs) in contracts {
            let symbol = self.standard_function_symbol(function);
            self.contracts.insert(
                symbol,
                CallableContract {
                    inputs,
                    success,
                    failures,
                    needs,
                },
            );
        }
    }

    /// `needs { clock: Clock }` plus `parameters`. A standard function has no body,
    /// so its own symbol stands in for the requirement binding, as for `Clock.sleep`.
    fn clock_need(
        &mut self,
        function: StandardFunction,
        parameters: Vec<crate::ids::SymbolId>,
    ) -> NeedsRowId {
        let clock = self.standard_named_type(StandardType::Clock, Vec::new());
        let requirement = self.standard_function_symbol(function);
        self.intern_needs_row(NeedsRow {
            entries: vec![Need {
                requirement,
                lexical_name: "clock".to_owned(),
                interface: clock,
                explained: false,
            }],
            parameters,
        })
    }

    /// The fields of `Timeout.TimedOut` and the type it builds.
    pub(super) fn timeout_constructor_shape(&mut self) -> (Vec<TypeId>, TypeId) {
        (
            vec![self.standard_named_type(StandardType::Duration, Vec::new())],
            self.standard_named_type(StandardType::Timeout, Vec::new()),
        )
    }
}
