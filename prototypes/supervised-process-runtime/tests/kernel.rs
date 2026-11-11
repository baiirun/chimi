//! Kernel decisions: restart containment and accounting, routing across
//! incarnations, never-parking bounded mailboxes, shutdown order and deadlines,
//! admission, escalation, and flat memory for a long-lived process.

use chimi_supervised_process_runtime_prototype::heap::*;
use chimi_supervised_process_runtime_prototype::kernel::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::time::Duration;

const STACK: usize = 64 * 1024;
const COUNTER: NameId = NameId(1);
const PEER: NameId = NameId(2);

thread_local! {
    static TOTALS: RefCell<HashMap<IncarnationId, i64>> = RefCell::new(HashMap::new());
    static ORDER: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) };
}

fn total(incarnation: IncarnationId) -> i64 {
    TOTALS.with(|totals| totals.borrow().get(&incarnation).copied().unwrap_or(0))
}

/// Adds each positive message to a per-incarnation total kept as loop state in
/// its own heap. Message 0 defects. Cancellation returns.
fn counter(context: &mut ProcessContext) -> BodyEnd {
    let incarnation = context.incarnation();
    unsafe { chimi_region_enter() };
    let mut state = unsafe { chimi_value_int(0) };
    let end = loop {
        match context.receive() {
            Received::Cancelled => break BodyEnd::Returned,
            Received::Message(message) => {
                let amount = unsafe { (*message).as_.integer };
                if amount == 0 {
                    break BodyEnd::Defected;
                }
                ORDER.with(|order| order.borrow_mut().push(amount));
                let next = unsafe { (*state).as_.integer } + amount;
                state = unsafe { chimi_value_int(next) };
                TOTALS.with(|totals| totals.borrow_mut().insert(incarnation, next));
                assert_eq!(unsafe { chimi_region_pass(&mut state) }, HeapStatus::Ok);
            }
        }
    };
    unsafe { chimi_region_exit() };
    end
}

/// Ignores cancellation and keeps waiting, so shutdown must force it.
fn stubborn(context: &mut ProcessContext) -> BodyEnd {
    loop {
        let _ = context.receive();
    }
}

fn spec(body: Body, name: Option<NameId>, capacity: u32) -> ProcessSpec {
    ProcessSpec {
        body,
        mailbox_capacity: NonZeroU32::new(capacity).unwrap(),
        shutdown_within: Duration::from_secs(5),
        name,
    }
}

fn policy(members: Vec<Member>, limit: u32) -> Policy {
    Policy {
        members,
        capacity: NonZeroU32::new(16).unwrap(),
        restarts: Restarts { limit, within: Duration::from_secs(30) },
    }
}

fn named_counter(name: NameId) -> Member {
    Member::Process(spec(counter, Some(name), 64), Restart::OnDefect)
}

/// A host heap that builds messages; kept alive for the whole test.
struct Host(Heap);

impl Host {
    fn new() -> Self {
        Self(Heap::new(u64::MAX))
    }

    fn message(&self, value: i64) -> Fragment {
        let _current = self.0.enter();
        Fragment::copy(unsafe { chimi_value_int(value) }).expect("host value")
    }
}

fn send(kernel: &mut Kernel, host: &Host, address: Address, value: i64) {
    kernel.send(address, host.message(value)).expect("accepted");
}

fn started(kernel: &Kernel) -> Vec<(MembershipId, IncarnationId)> {
    kernel
        .events()
        .iter()
        .filter_map(|event| match event {
            LifecycleEvent::Started { membership, incarnation, .. } => {
                Some((*membership, *incarnation))
            }
            _ => None,
        })
        .collect()
}

fn terminations(kernel: &Kernel) -> Vec<(MembershipId, TerminalCause)> {
    kernel
        .events()
        .iter()
        .filter_map(|event| match event {
            LifecycleEvent::Terminated { membership, cause, .. } => Some((*membership, *cause)),
            _ => None,
        })
        .collect()
}

#[test]
#[ignore = "pending kernel implementation"]
fn a_defect_restarts_only_that_member_and_the_peer_keeps_its_state() {
    let host = Host::new();
    let mut kernel = Kernel::new(STACK);
    kernel
        .start(policy(vec![named_counter(COUNTER), named_counter(PEER)], 2))
        .expect("started");
    send(&mut kernel, &host, Address::Named(COUNTER), 5);
    send(&mut kernel, &host, Address::Named(PEER), 7);
    kernel.run_until_idle();
    let before = kernel.events().len();
    send(&mut kernel, &host, Address::Named(COUNTER), 0);
    kernel.run_until_idle();
    send(&mut kernel, &host, Address::Named(COUNTER), 3);
    send(&mut kernel, &host, Address::Named(PEER), 1);
    kernel.run_until_idle();

    let [(counter_membership, first), (_, peer), (restarted_membership, second)] =
        started(&kernel)[..]
    else {
        panic!("expected two starts and one restart: {:?}", kernel.events());
    };
    assert_eq!(restarted_membership, counter_membership);
    assert_eq!((total(first), total(second), total(peer)), (5, 3, 8));
    let cycle: Vec<_> = kernel.events()[before..]
        .iter()
        .filter(|event| !matches!(event, LifecycleEvent::Sent { .. }))
        .copied()
        .collect();
    assert!(matches!(
        cycle[..],
        [
            LifecycleEvent::Terminated { cause: TerminalCause::Defected, .. },
            LifecycleEvent::RestartAttempted { attempts_in_window: 1, .. },
            LifecycleEvent::Started { .. },
        ]
    ), "{cycle:?}");
}

#[test]
#[ignore = "pending kernel implementation"]
fn exceeding_the_restart_limit_stops_peers_and_escalates_from_the_root() {
    let host = Host::new();
    let mut kernel = Kernel::new(STACK);
    kernel
        .start(policy(vec![named_counter(COUNTER), named_counter(PEER)], 1))
        .expect("started");
    for _ in 0..2 {
        send(&mut kernel, &host, Address::Named(COUNTER), 0);
        kernel.run_until_idle();
    }
    let events = kernel.events();
    assert!(events.iter().any(|e| matches!(e, LifecycleEvent::RestartLimitExceeded { .. })));
    assert!(events.iter().any(|e| matches!(e, LifecycleEvent::Escalated { parent: None, .. })));
    assert!(terminations(&kernel).iter().any(|(_, cause)| *cause == TerminalCause::Cancelled));
    assert!(kernel.program_terminated());
}

#[test]
#[ignore = "pending kernel implementation"]
fn defects_further_apart_than_the_window_do_not_accumulate() {
    let host = Host::new();
    let mut kernel = Kernel::new(STACK);
    kernel.start(policy(vec![named_counter(COUNTER)], 1)).expect("started");
    for _ in 0..3 {
        send(&mut kernel, &host, Address::Named(COUNTER), 0);
        kernel.run_until_idle();
        kernel.advance(Duration::from_secs(31));
    }
    assert!(!kernel.program_terminated());
    assert_eq!(started(&kernel).len(), 4);
}

#[test]
#[ignore = "pending kernel implementation"]
fn names_follow_the_replacement_and_direct_addresses_stay_unavailable() {
    let host = Host::new();
    let mut kernel = Kernel::new(STACK);
    let root = kernel.start(policy(vec![], 2)).expect("started");
    let managed = kernel
        .start_process(root, spec(counter, Some(COUNTER), 8), Restart::OnDefect)
        .expect("admitted");
    let direct = kernel.address(&managed);
    send(&mut kernel, &host, Address::Named(COUNTER), 0);
    kernel.run_until_idle();

    assert!(matches!(kernel.send(direct, host.message(1)), Err(SendError::Unavailable(_))));
    send(&mut kernel, &host, Address::Named(COUNTER), 4);
    kernel.run_until_idle();
    let [_, (_, replacement)] = started(&kernel)[..] else { panic!("{:?}", kernel.events()) };
    assert_eq!(total(replacement), 4);
    assert_ne!(kernel.address(&managed), direct);
}

#[test]
#[ignore = "pending kernel implementation"]
fn a_full_mailbox_returns_the_message_and_delivery_is_fifo() {
    let host = Host::new();
    let mut kernel = Kernel::new(STACK);
    let root = kernel.start(policy(vec![], 2)).expect("started");
    kernel
        .start_process(root, spec(counter, Some(COUNTER), 2), Restart::Never)
        .expect("admitted");
    ORDER.with(|order| order.borrow_mut().clear());
    send(&mut kernel, &host, Address::Named(COUNTER), 1);
    send(&mut kernel, &host, Address::Named(COUNTER), 2);
    let Err(SendError::Full(returned)) = kernel.send(Address::Named(COUNTER), host.message(3))
    else {
        panic!("third send must be refused without parking");
    };
    let _current = host.0.enter();
    assert_eq!(unsafe { (*returned.adopt()).as_.integer }, 3);
    drop(_current);
    kernel.run_until_idle();
    assert_eq!(ORDER.with(|order| order.borrow().clone()), vec![1, 2]);
}

#[test]
#[ignore = "pending kernel implementation"]
fn stop_runs_newest_dynamic_first_then_declared_in_reverse_and_frees_unread_messages() {
    let host = Host::new();
    let chunks_before = chunk_stats();
    {
        let mut kernel = Kernel::new(STACK);
        let root = kernel
            .start(policy(vec![named_counter(COUNTER), named_counter(PEER)], 2))
            .expect("started");
        kernel.start_process(root, spec(counter, None, 8), Restart::Never).expect("third");
        kernel.start_process(root, spec(counter, None, 8), Restart::Never).expect("fourth");
        kernel.run_until_idle();
        send(&mut kernel, &host, Address::Named(COUNTER), 9);
        let memberships: Vec<_> = started(&kernel).into_iter().map(|(m, _)| m).collect();
        kernel.stop(root);

        let order: Vec<_> = terminations(&kernel).into_iter().map(|(m, _)| m).collect();
        let expected: Vec<_> = memberships.iter().rev().copied().collect();
        assert_eq!(order, expected);
        assert!(terminations(&kernel).iter().all(|(_, c)| *c == TerminalCause::Cancelled));
    }
    let _ = &host;
    assert_eq!(chunk_stats().live_chunks, chunks_before.live_chunks);
}

#[test]
#[ignore = "pending kernel implementation"]
fn cleanup_that_misses_its_deadline_is_forced() {
    let mut kernel = Kernel::new(STACK);
    let root = kernel
        .start(policy(vec![Member::Process(spec(stubborn, None, 1), Restart::Never)], 0))
        .expect("started");
    kernel.run_until_idle();
    kernel.stop(root);
    assert_eq!(terminations(&kernel)[..].last().map(|(_, c)| *c), Some(TerminalCause::Forced));
}

#[test]
#[ignore = "pending kernel implementation"]
fn admission_fails_fast_and_removal_releases_the_name() {
    let mut kernel = Kernel::new(STACK);
    let root = kernel.start(policy(vec![named_counter(COUNTER)], 0)).expect("started");
    let taken = kernel.start_process(root, spec(counter, Some(COUNTER), 8), Restart::Never);
    assert_eq!(taken.unwrap_err(), StartError::NameTaken(COUNTER));

    let tight = Policy { capacity: NonZeroU32::new(1).unwrap(), ..policy(vec![], 0) };
    let small = kernel.start(tight).expect("started");
    let only = kernel.start_process(small, spec(counter, Some(PEER), 8), Restart::Never);
    let full = kernel.start_process(small, spec(counter, None, 8), Restart::Never);
    assert_eq!(full.unwrap_err(), StartError::CapacityReached);

    kernel.remove_process(small, only.expect("admitted"));
    kernel.start_process(small, spec(counter, Some(PEER), 8), Restart::Never).expect("reused");
    kernel.stop(small);
    let stopped = kernel.start_process(small, spec(counter, None, 8), Restart::Never);
    assert_eq!(stopped.unwrap_err(), StartError::Stopped);
}

#[test]
#[ignore = "pending kernel implementation"]
fn a_failed_root_start_stops_the_members_it_already_started() {
    let mut kernel = Kernel::new(STACK);
    let clash = policy(vec![named_counter(COUNTER), named_counter(COUNTER)], 0);
    assert_eq!(kernel.start(clash).unwrap_err(), StartError::NameTaken(COUNTER));
    assert_eq!(terminations(&kernel).len(), started(&kernel).len());
    kernel.start(policy(vec![named_counter(COUNTER)], 0)).expect("name was released");
}

#[test]
#[ignore = "pending kernel implementation"]
fn a_nested_supervisor_that_exceeds_its_limit_is_restarted_by_its_parent() {
    let host = Host::new();
    let mut kernel = Kernel::new(STACK);
    let inner = policy(vec![named_counter(COUNTER)], 0);
    kernel
        .start(policy(vec![Member::Supervisor(inner, Restart::OnDefect)], 1))
        .expect("started");
    send(&mut kernel, &host, Address::Named(COUNTER), 0);
    kernel.run_until_idle();

    let events = kernel.events();
    assert!(events.iter().any(|e| matches!(e, LifecycleEvent::Escalated { parent: Some(_), .. })));
    assert!(!kernel.program_terminated());
    send(&mut kernel, &host, Address::Named(COUNTER), 2);
    kernel.run_until_idle();
    let (_, rebuilt) = *started(&kernel).last().expect("rebuilt member");
    assert_eq!(total(rebuilt), 2);
}

#[test]
#[ignore = "pending kernel implementation"]
fn a_process_handles_a_million_messages_in_flat_memory() {
    let host = Host::new();
    let mut kernel = Kernel::new(STACK);
    kernel.start(policy(vec![named_counter(COUNTER)], 0)).expect("started");
    let mut peak_after_warmup = 0;
    for batch in 0..15_625 {
        for _ in 0..64 {
            send(&mut kernel, &host, Address::Named(COUNTER), 1);
        }
        kernel.run_until_idle();
        if batch == 100 {
            peak_after_warmup = kernel.heap_stats(Address::Named(COUNTER)).unwrap().peak_live_bytes;
        }
    }
    let stats = kernel.heap_stats(Address::Named(COUNTER)).expect("running");
    assert_eq!(stats.peak_live_bytes, peak_after_warmup);
    let [(_, incarnation)] = started(&kernel)[..] else { panic!() };
    assert_eq!(total(incarnation), 1_000_000);
}
