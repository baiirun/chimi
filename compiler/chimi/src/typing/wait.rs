//! The contracts of `std/wait` (`docs/specification/time-and-outcomes.md`,
//! "Choosing among events"):
//!
//! ```text
//! Wait.receive: fn<T, A, F, R>(Receiver<T>, fn(Receive<T>) -> A errors F needs R)
//!                   -> Event<A, F | Channel.Aborted> needs R
//! Wait.inbox:   fn<T, A, F, R>(Inbox<T>, fn(T) -> A errors F needs R) -> Event<A, F> needs R
//! Wait.task:    fn<T, E, A, F, R>(Task<T, E>, fn(T) -> A errors F needs R)
//!                   -> Event<A, E | F> needs R
//! Wait.after:   fn<A>(Duration, A) -> Event<A, Never> needs { clock: Clock }
//! Wait.at:      fn<A>(Instant, A) -> Event<A, Never> needs { clock: Clock }
//! Wait.first:   fn<A, F>(List<Event<A, F>>) -> A errors F
//! Wait.watch:   fn<T, A, F, R>(Watch.Subscription<T>, fn(Receive<T>) -> A errors F needs R)
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
    pub(super) fn index_wait_contracts(&mut self) {
        use StandardFunction as F;
        use StandardGeneric as G;
        let none = FailureRowId::new(0);
        let source = self.standard_generic_type(G::WaitSource);
        let value = self.standard_generic_type(G::WaitValue);
        let failures = self.resolved.standard_generic(G::WaitFailures);
        let task_failures = self.resolved.standard_generic(G::WaitTaskFailures);
        let needs = self.resolved.standard_generic(G::WaitNeeds);
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
            checker.standard_named_type(StandardType::WaitEvent, vec![value, failures])
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

        let task_failure_type = self.standard_generic_type(G::WaitTaskFailures);
        let task = self.standard_named_type(StandardType::Task, vec![source, task_failure_type]);
        let task_event_failures =
            self.failure_parameters(vec![task_failures, failures], Vec::new());
        let task_event = event(self, task_event_failures);

        let duration = self.standard_named_type(StandardType::Duration, Vec::new());
        let instant = self.standard_named_type(StandardType::Instant, Vec::new());
        let timer_event = event(self, none);

        let failure_type = self.standard_generic_type(G::WaitFailures);
        let any_event =
            self.standard_named_type(StandardType::WaitEvent, vec![value, failure_type]);
        let events = self.builtin_type_with_arguments("List", vec![any_event]);

        let mut contracts = vec![
            (
                F::WaitReceive,
                vec![receiver, receive_mapping],
                receive_event,
                none,
                needs_row,
            ),
            (
                F::WaitWatch,
                vec![subscription, receive_mapping],
                receive_event,
                none,
                needs_row,
            ),
            (
                F::WaitInbox,
                vec![inbox, source_mapping],
                inbox_event,
                none,
                needs_row,
            ),
            (
                F::WaitTask,
                vec![task, source_mapping],
                task_event,
                none,
                needs_row,
            ),
            (
                F::WaitFirst,
                vec![events],
                value,
                map_failures,
                NeedsRowId::new(0),
            ),
        ];
        for (function, limit) in [(F::WaitAfter, duration), (F::WaitAt, instant)] {
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

    /// `T4345`: `Wait.first([])` can never return, so an empty list literal is
    /// rejected where it is written. An empty list built at run time defects.
    pub(super) fn reject_empty_event_list(
        &mut self,
        module_id: ModuleId,
        callee_symbol: Option<SymbolId>,
        arguments: &[CallArgument],
    ) {
        let waits_first = callee_symbol.is_some_and(|symbol| {
            self.resolved.standard_symbol(symbol)
                == Some(StandardSymbol::Function(StandardFunction::WaitFirst))
        });
        if let (true, [argument]) = (waits_first, arguments)
            && let ModuleExpression::List {
                elements, range, ..
            } = &argument.value
            && elements.is_empty()
        {
            self.type_error_range(
                module_id,
                "T4345",
                *range,
                "`Wait.first` needs at least one event; an empty list would wait forever"
                    .to_owned(),
            );
        }
    }
}
