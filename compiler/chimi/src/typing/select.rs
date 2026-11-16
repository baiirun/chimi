//! The contracts of `std/select` (`docs/specification/time-and-outcomes.md`,
//! "Choosing among events"):
//!
//! ```text
//! Select.receive: fn<T, A, F, R>(Receiver<T>, fn(Receive<T>) -> A errors F needs R)
//!                   -> Event<A, F | Channel.Aborted> needs R
//! Select.inbox:   fn<T, A, F, R>(Inbox<T>, fn(T) -> A errors F needs R) -> Event<A, F> needs R
//! Select.task:    fn<T, E, A, F, R>(Task<T, E>, fn(T) -> A errors F needs R)
//!                   -> Event<A, E | F> needs R
//! Select.after:   fn<A>(Duration, A) -> Event<A, Never> needs { clock: Clock }
//! Select.at:      fn<A>(Instant, A) -> Event<A, Never> needs { clock: Clock }
//! Select.first:   fn<A, F>(List<Event<A, F>>) -> A errors F
//! Select.watch:   fn<T, A, F, R>(Watch.Subscription<T>, fn(Receive<T>) -> A errors F needs R)
//!                   -> Event<A, F | Channel.Aborted> needs R
//! ```
//!
//! An event's failure set is its second type argument, the value projection of
//! a failure row, as `Task<T, E>` carries its task's failures.

use super::{CallableContract, CanonicalType, FailureRow, NeedsRow, ProjectTypeChecker};
use crate::ids::{FailureRowId, ModuleId, NeedsRowId, SymbolId};
use crate::semantic::{CallArgument, ModuleExpression};

use crate::standard::{
    StandardConstructor, StandardFunction, StandardGeneric, StandardSymbol, StandardType,
};

impl ProjectTypeChecker {
    pub(super) fn index_select_contracts(&mut self) {
        use StandardFunction as F;
        use StandardGeneric as G;
        let none = FailureRowId::new(0);
        let source = self.standard_generic_type(G::SelectSource);
        let value = self.standard_generic_type(G::SelectValue);
        let failures = self.resolved.standard_generic(G::SelectFailures);
        let task_failures = self.resolved.standard_generic(G::SelectTaskFailures);
        let needs = self.resolved.standard_generic(G::SelectNeeds);
        let needs_row = self.intern_needs_row(NeedsRow {
            entries: Vec::new(),
            parameters: vec![needs],
        });
        let map_failures = self.failure_parameters(vec![failures], Vec::new());
        let mapping = |checker: &mut Self, input| {
            checker.intern_type(CanonicalType::Function(CallableContract {
                inputs: vec![input],
                success: value,
                failures: map_failures,
                needs: needs_row,
            }))
        };
        let event = |checker: &mut Self, failures: FailureRowId| {
            let failures = checker.failure_set_argument_type(failures);
            checker.standard_named_type(StandardType::SelectEvent, vec![value, failures])
        };

        let receiver = self.standard_named_type(StandardType::ChannelReceiver, vec![source]);
        let receive = self.standard_named_type(StandardType::ChannelReceive, vec![source]);
        let receive_mapping = mapping(self, receive);
        let aborted = self.standard_constructor_symbol(StandardConstructor::ChannelAborted);
        let receive_failures = self.failure_parameters(vec![failures], vec![aborted]);
        let receive_event = event(self, receive_failures);
        let subscription = self.standard_named_type(StandardType::WatchSubscription, vec![source]);

        let inbox = self.standard_named_type(StandardType::ProcessInbox, vec![source]);
        let source_mapping = mapping(self, source);
        let inbox_event = event(self, map_failures);

        let task_failure_type = self.standard_generic_type(G::SelectTaskFailures);
        let task = self.standard_named_type(StandardType::Task, vec![source, task_failure_type]);
        let task_event_failures =
            self.failure_parameters(vec![task_failures, failures], Vec::new());
        let task_event = event(self, task_event_failures);

        let duration = self.standard_named_type(StandardType::Duration, Vec::new());
        let instant = self.standard_named_type(StandardType::Instant, Vec::new());
        let timer_event = event(self, none);

        let failure_type = self.standard_generic_type(G::SelectFailures);
        let any_event =
            self.standard_named_type(StandardType::SelectEvent, vec![value, failure_type]);
        let events = self.builtin_type_with_arguments("List", vec![any_event]);

        let mut contracts = vec![
            (
                F::SelectReceive,
                vec![receiver, receive_mapping],
                receive_event,
                none,
                needs_row,
            ),
            (
                F::SelectWatch,
                vec![subscription, receive_mapping],
                receive_event,
                none,
                needs_row,
            ),
            (
                F::SelectInbox,
                vec![inbox, source_mapping],
                inbox_event,
                none,
                needs_row,
            ),
            (
                F::SelectTask,
                vec![task, source_mapping],
                task_event,
                none,
                needs_row,
            ),
            (
                F::SelectFirst,
                vec![events],
                value,
                map_failures,
                NeedsRowId::new(0),
            ),
        ];
        for (function, limit) in [(F::SelectAfter, duration), (F::SelectAt, instant)] {
            let clock = self.clock_need(function, Vec::new());
            contracts.push((function, vec![limit, value], timer_event, none, clock));
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

    /// A failure row of `parameters` and the standard `variants`.
    fn failure_parameters(
        &mut self,
        parameters: Vec<SymbolId>,
        mut variants: Vec<SymbolId>,
    ) -> FailureRowId {
        variants.sort_unstable();
        self.intern_failure_row(FailureRow {
            variants,
            variant_arguments: Vec::new(),
            parameters,
            exclusions: Vec::new(),
        })
    }

    /// `T4345`: `Select.first([])` can never return, so an empty list literal is
    /// rejected where it is written. An empty list built at run time defects.
    pub(super) fn reject_empty_event_list(
        &mut self,
        module_id: ModuleId,
        callee_symbol: Option<SymbolId>,
        arguments: &[CallArgument],
    ) {
        let selects = callee_symbol.is_some_and(|symbol| {
            self.resolved.standard_symbol(symbol)
                == Some(StandardSymbol::Function(StandardFunction::SelectFirst))
        });
        if let (true, [argument]) = (selects, arguments)
            && let ModuleExpression::List {
                elements, range, ..
            } = &argument.value
            && elements.is_empty()
        {
            self.type_error_range(
                module_id,
                "T4345",
                *range,
                "`Select.first` needs at least one event; an empty list would wait forever"
                    .to_owned(),
            );
        }
    }
}
