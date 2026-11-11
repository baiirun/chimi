//! Supervision kernel: process incarnations, mailboxes, names, and supervisors.
//!
//! Global invariants:
//! - Each incarnation runs on its own fiber with its own `Heap`; the kernel
//!   switches the current heap around every resume.
//! - Every message crosses into a mailbox as a `Fragment` and belongs to exactly
//!   one mailbox until it is received (adopted) or discarded (freed).
//! - Incarnations of one membership never overlap, and a name maps to at most
//!   one live incarnation.
//! - Time is virtual. Only `advance` and a body's `work` move it forward, so
//!   every run is deterministic and `events` is an exact transcript.
//! - `events` is an in-memory transcript for tests and grows with every event.
//!   The native runtime emits the same events to the trace ABI instead.
//! - A membership record lives while its slot does. A `ManagedProcess` handle
//!   whose record is gone keeps only its last incarnation, until
//!   `remove_process` consumes the handle.
//!
//! The host-side API stands in for intrinsic calls from a root task; bodies
//! reach the kernel through `ProcessContext`, which parks on fiber requests.

use crate::heap::{Fragment, Heap, HeapStats, Value, chimi_heap_switch};
use chimi_fiber::{
    ChimiFiber, ChimiFiberCommand, ChimiFiberConfig, ChimiFiberResult, ChimiFiberState,
    chimi_fiber_create, chimi_fiber_destroy, chimi_fiber_request, chimi_fiber_resume,
};
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::num::NonZeroU32;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NameId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct IncarnationId(u64);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct SupervisorId(u64);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct MembershipId(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Restart {
    Never,
    OnDefect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Restarts {
    pub limit: u32,
    pub within: Duration,
}

/// How a body ends. A Rust panic inside a body is also a defect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BodyEnd {
    Returned,
    Failed,
    Defected,
}

pub type Body = fn(&mut ProcessContext) -> BodyEnd;

#[derive(Clone, Debug)]
pub struct ProcessSpec {
    pub body: Body,
    pub mailbox_capacity: NonZeroU32,
    pub shutdown_within: Duration,
    pub name: Option<NameId>,
}

#[derive(Clone, Debug)]
pub enum Member {
    Process(ProcessSpec, Restart),
    Supervisor(Policy, Restart),
}

#[derive(Clone, Debug)]
pub struct Policy {
    pub members: Vec<Member>,
    pub capacity: NonZeroU32,
    pub restarts: Restarts,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Address {
    Named(NameId),
    Direct(IncarnationId),
}

/// Shareable handle to a running supervisor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Running(SupervisorId);

/// Exclusive membership handle; `remove_process` consumes it.
#[derive(Debug)]
pub struct ManagedProcess(MembershipId);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartError {
    CapacityReached,
    NameTaken(NameId),
    Stopped,
}

/// A failed send hands the unsent message back.
#[derive(Debug)]
pub enum SendError {
    Full(Fragment),
    Unavailable(Fragment),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalCause {
    Returned,
    Failed,
    Defected,
    /// Requested by stop or removal; cleanup finished within `shutdown_within`.
    Cancelled,
    /// Requested by stop or removal; cleanup did not finish in time.
    Forced,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendOutcome {
    Accepted,
    Full,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleEvent {
    Started {
        supervisor: SupervisorId,
        membership: MembershipId,
        incarnation: IncarnationId,
    },
    AdmissionRejected {
        supervisor: SupervisorId,
        error: StartError,
    },
    Terminated {
        membership: MembershipId,
        incarnation: IncarnationId,
        cause: TerminalCause,
    },
    RestartAttempted {
        supervisor: SupervisorId,
        membership: MembershipId,
        attempts_in_window: u32,
    },
    RestartLimitExceeded {
        supervisor: SupervisorId,
    },
    /// `parent: None` means the root escalated and the program terminates.
    Escalated {
        supervisor: SupervisorId,
        parent: Option<SupervisorId>,
    },
    Removed {
        membership: MembershipId,
    },
    Stopped {
        supervisor: SupervisorId,
    },
    Sent {
        address: Address,
        incarnation: Option<IncarnationId>,
        outcome: SendOutcome,
    },
}

/// What a body sees. Only valid on the body's own fiber.
pub struct ProcessContext {
    incarnation: IncarnationId,
    exchange: *mut Exchange,
}

#[derive(Debug)]
pub enum Received {
    /// The oldest message, adopted into this incarnation's heap.
    Message(*mut Value),
    /// A stop or removal was requested and every message accepted before it
    /// has been received. Every later `receive` returns this immediately; the
    /// body should clean up and return.
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendFailure {
    Full,
    Unavailable,
}

/// A body's call into the kernel, passed through `chimi_fiber_request`.
enum Request {
    Receive,
    Send { address: Address, message: Fragment },
    Work(Duration),
}

enum Reply {
    Message(Fragment),
    Sent(Result<(), SendFailure>),
    Cancelled,
    Worked,
    /// Cleanup missed its deadline; the body unwinds from its kernel call.
    Forced,
}

/// One request and its reply. Owned by the incarnation, so it outlives the fiber.
#[derive(Default)]
struct Exchange {
    request: Option<Request>,
    reply: Option<Reply>,
}

/// Unwind payload for forced termination. Raised with `resume_unwind`, so no
/// panic hook runs, and caught by `process_entry`.
struct ForcedTermination;

impl ProcessContext {
    /// Ensures: parks only while the mailbox is empty and no cancellation is
    /// pending; FIFO across senders. After cancellation it never parks: it
    /// returns already-accepted messages until the mailbox is empty, then
    /// `Cancelled`.
    pub fn receive(&mut self) -> Received {
        match self.call(Request::Receive) {
            (_, Some(Reply::Message(message))) => Received::Message(message.adopt()),
            (_, Some(Reply::Cancelled)) => Received::Cancelled,
            _ => panic!("kernel resumed a receive without a message or cancellation"),
        }
    }

    /// Stands in for work that takes `duration` of virtual time, such as a slow
    /// host operation during cleanup. Never parks. During cleanup, work that
    /// would run past the shutdown deadline is forced at the deadline.
    pub fn work(&mut self, duration: Duration) {
        match self.call(Request::Work(duration)) {
            (_, Some(Reply::Worked)) => {}
            _ => panic!("kernel resumed work without its outcome"),
        }
    }

    /// Ensures: never parks. On failure the copy is discarded and the caller
    /// still holds `message`.
    ///
    /// # Safety
    /// `message` points to a live value in this incarnation's heap.
    pub unsafe fn send(&mut self, address: Address, message: *const Value) -> Result<(), SendFailure> {
        let message = unsafe { Fragment::copy(message) }.expect("a process sends only values it owns");
        match self.call(Request::Send { address, message }) {
            (_, Some(Reply::Sent(result))) => result,
            _ => panic!("kernel resumed a send without its outcome"),
        }
    }

    pub fn incarnation(&self) -> IncarnationId {
        self.incarnation
    }

    fn call(&mut self, request: Request) -> (ChimiFiberCommand, Option<Reply>) {
        // SAFETY: the exchange lives in the incarnation, which outlives its fiber,
        // and the kernel touches it only while this fiber is suspended.
        unsafe { (*self.exchange).request = Some(request) };
        let command = unsafe { chimi_fiber_request(self.exchange.cast(), ptr::null()) };
        let reply = unsafe { (*self.exchange).reply.take() };
        if matches!(reply, Some(Reply::Forced)) {
            std::panic::resume_unwind(Box::new(ForcedTermination));
        }
        (command, reply)
    }
}

/// What `process_entry` runs. Boxed so its address is stable for the fiber.
struct ProcessStart {
    body: Body,
    context: ProcessContext,
}

const END_RETURNED: i64 = 0;
const END_FAILED: i64 = 1;
const END_DEFECTED: i64 = 2;
const END_FORCED: i64 = 3;

unsafe extern "C" fn process_entry(context: *mut c_void) -> ChimiFiberResult {
    // SAFETY: `spawn` passes a `ProcessStart` that outlives the fiber.
    let start = unsafe { &mut *context.cast::<ProcessStart>() };
    let end = catch_unwind(AssertUnwindSafe(|| (start.body)(&mut start.context)));
    let value = match end {
        Ok(BodyEnd::Returned) => END_RETURNED,
        Ok(BodyEnd::Failed) => END_FAILED,
        Ok(BodyEnd::Defected) => END_DEFECTED,
        Err(payload) if payload.is::<ForcedTermination>() => END_FORCED,
        Err(_) => END_DEFECTED,
    };
    ChimiFiberResult { state: ChimiFiberState::Succeeded as u32, value, defect_code: 0 }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Wait {
    /// Created; the body has not run yet.
    Fresh,
    /// Parked in `receive` on an empty mailbox.
    Receiving,
}

struct Incarnation {
    membership: MembershipId,
    spec: ProcessSpec,
    fiber: *mut ChimiFiber,
    mailbox: VecDeque<Fragment>,
    /// False once shutdown begins or the body ends; sends then fail.
    accepting: bool,
    /// Set when shutdown begins: the virtual time cleanup must finish by.
    deadline: Option<Duration>,
    wait: Wait,
    queued: bool,
    heap: Heap,
    _start: Box<ProcessStart>,
    exchange: Box<Exchange>,
}

impl Drop for Incarnation {
    fn drop(&mut self) {
        // The kernel drops an incarnation only after its fiber has finished, so
        // destroy never resumes the body. Mailbox fragments and the heap follow.
        unsafe { chimi_fiber_destroy(self.fiber) };
    }
}

#[derive(Clone, Copy, Debug)]
enum Current {
    None,
    Process(IncarnationId),
    Supervisor(SupervisorId),
}

struct Membership {
    supervisor: SupervisorId,
    member: Member,
    current: Current,
    last_incarnation: Option<IncarnationId>,
    /// A `ManagedProcess` handle refers to this membership.
    handle: bool,
}

impl Membership {
    fn restart(&self) -> Restart {
        match &self.member {
            Member::Process(_, restart) | Member::Supervisor(_, restart) => *restart,
        }
    }
}

struct Supervision {
    policy: Policy,
    parent: Option<(SupervisorId, MembershipId)>,
    declared: Vec<MembershipId>,
    dynamic: Vec<MembershipId>,
    restart_times: VecDeque<Duration>,
    /// Set when shutdown or escalation begins; admission and restart stop.
    closed: bool,
}

pub struct Kernel {
    stack_bytes: usize,
    now: Duration,
    next_id: u64,
    events: Vec<LifecycleEvent>,
    program_terminated: bool,
    supervisors: HashMap<SupervisorId, Supervision>,
    memberships: HashMap<MembershipId, Membership>,
    /// Last incarnation of each handle whose membership record is gone.
    retired: HashMap<MembershipId, IncarnationId>,
    incarnations: HashMap<IncarnationId, Incarnation>,
    names: HashMap<NameId, IncarnationId>,
    ready: VecDeque<IncarnationId>,
    roots: Vec<SupervisorId>,
}

impl Kernel {
    pub fn new(stack_bytes: usize) -> Self {
        Self {
            stack_bytes,
            now: Duration::ZERO,
            next_id: 1,
            events: Vec::new(),
            program_terminated: false,
            supervisors: HashMap::new(),
            memberships: HashMap::new(),
            retired: HashMap::new(),
            incarnations: HashMap::new(),
            names: HashMap::new(),
            ready: VecDeque::new(),
            roots: Vec::new(),
        }
    }

    /// Ensures: every declared member has started, or every started member is
    /// stopped again and the first failure is returned.
    pub fn start(&mut self, policy: Policy) -> Result<Running, StartError> {
        let supervisor = self.start_supervisor(policy, None)?;
        self.roots.push(supervisor);
        Ok(Running(supervisor))
    }

    /// Ensures: fail-fast admission; a name is registered before the body runs.
    pub fn start_process(
        &mut self,
        running: Running,
        spec: ProcessSpec,
        restart: Restart,
    ) -> Result<ManagedProcess, StartError> {
        let Running(supervisor) = running;
        match self.admit(supervisor, Member::Process(spec, restart), true) {
            Ok(membership) => {
                self.memberships.get_mut(&membership).expect("just admitted").handle = true;
                Ok(ManagedProcess(membership))
            }
            Err(error) => {
                self.events.push(LifecycleEvent::AdmissionRejected { supervisor, error });
                Err(error)
            }
        }
    }

    /// Ensures: a direct address pinned to the current incarnation, or to the
    /// last one once the membership has ended, which stays `Unavailable`.
    pub fn address(&self, managed: &ManagedProcess) -> Address {
        let incarnation = match self.memberships.get(&managed.0) {
            Some(entry) => entry.last_incarnation,
            None => self.retired.get(&managed.0).copied(),
        };
        Address::Direct(incarnation.expect("a handle's membership always has had an incarnation"))
    }

    /// Host-side send for tests. Same outcomes as `ProcessContext::send`.
    pub fn send(&mut self, address: Address, message: Fragment) -> Result<(), SendError> {
        self.deliver(address, message)
    }

    /// Ensures: on return no incarnation of the membership runs and no pending
    /// restart can create one. A membership that already ended, including by
    /// a stop or escalation of its supervisor, is removed without an event.
    pub fn remove_process(&mut self, running: Running, managed: ManagedProcess) {
        let membership = managed.0;
        if self.retired.remove(&membership).is_some() {
            return;
        }
        let entry = &self.memberships[&membership];
        assert_eq!(entry.supervisor, running.0, "removed through a different supervisor");
        if let Current::Process(incarnation) = entry.current {
            self.shut_down(incarnation);
        }
        self.release_slot(membership);
        self.memberships.remove(&membership);
        self.events.push(LifecycleEvent::Removed { membership });
    }

    /// Ensures: on return the whole subtree has terminated, dynamic members
    /// newest first, then declared members in reverse order; unread messages
    /// are freed. Jumps virtual time to deadlines that cleanup does not meet.
    pub fn stop(&mut self, running: Running) {
        let supervisor = running.0;
        if self.supervisors.get(&supervisor).is_none_or(|s| s.closed) {
            return;
        }
        self.stop_members(supervisor);
        self.events.push(LifecycleEvent::Stopped { supervisor });
    }

    /// Runs ready fibers until every live incarnation is parked or finished.
    pub fn run_until_idle(&mut self) {
        while let Some(id) = self.ready.pop_front() {
            let Some(incarnation) = self.incarnations.get_mut(&id) else { continue };
            incarnation.queued = false;
            match incarnation.wait {
                Wait::Fresh => self.step(id, ChimiFiberCommand::Resume),
                Wait::Receiving => {
                    let Some(message) = incarnation.mailbox.pop_front() else { continue };
                    incarnation.exchange.reply = Some(Reply::Message(message));
                    self.step(id, ChimiFiberCommand::Resume);
                }
            }
        }
    }

    /// Moves virtual time forward; restart windows and deadlines use it.
    pub fn advance(&mut self, by: Duration) {
        self.now += by;
    }

    pub fn events(&self) -> &[LifecycleEvent] {
        &self.events
    }

    /// True once a restart-limit escalation has reached the root.
    pub fn program_terminated(&self) -> bool {
        self.program_terminated
    }

    /// Membership bookkeeping the kernel holds: one record per occupied slot,
    /// plus one retired incarnation per handle not yet passed to
    /// `remove_process`. It does not grow with restarts or ended members.
    pub fn membership_records(&self) -> usize {
        self.memberships.len() + self.retired.len()
    }

    /// Heap statistics of the incarnation an address currently reaches.
    pub fn heap_stats(&self, address: Address) -> Option<HeapStats> {
        let id = self.resolve(address)?;
        self.incarnations.get(&id).map(|incarnation| incarnation.heap.stats())
    }

    fn fresh_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn resolve(&self, address: Address) -> Option<IncarnationId> {
        match address {
            Address::Named(name) => self.names.get(&name).copied(),
            Address::Direct(incarnation) => Some(incarnation),
        }
    }

    fn deliver(&mut self, address: Address, message: Fragment) -> Result<(), SendError> {
        let target = self
            .resolve(address)
            .and_then(|id| self.incarnations.get_mut(&id).map(|incarnation| (id, incarnation)))
            .filter(|(_, incarnation)| incarnation.accepting);
        let (outcome, accepted, result) = match target {
            None => (SendOutcome::Unavailable, None, Err(SendError::Unavailable(message))),
            Some((_, incarnation))
                if incarnation.mailbox.len() >= incarnation.spec.mailbox_capacity.get() as usize =>
            {
                (SendOutcome::Full, None, Err(SendError::Full(message)))
            }
            Some((id, incarnation)) => {
                incarnation.mailbox.push_back(message);
                if incarnation.wait == Wait::Receiving && !incarnation.queued {
                    incarnation.queued = true;
                    self.ready.push_back(id);
                }
                (SendOutcome::Accepted, Some(id), Ok(()))
            }
        };
        self.events.push(LifecycleEvent::Sent { address, incarnation: accepted, outcome });
        result
    }

    fn start_supervisor(
        &mut self,
        policy: Policy,
        parent: Option<(SupervisorId, MembershipId)>,
    ) -> Result<SupervisorId, StartError> {
        let supervisor = SupervisorId(self.fresh_id());
        let members = policy.members.clone();
        self.supervisors.insert(
            supervisor,
            Supervision {
                policy,
                parent,
                declared: Vec::new(),
                dynamic: Vec::new(),
                restart_times: VecDeque::new(),
                closed: false,
            },
        );
        for member in members {
            if let Err(error) = self.admit(supervisor, member, false) {
                self.events.push(LifecycleEvent::AdmissionRejected { supervisor, error });
                self.stop_members(supervisor);
                self.supervisors.remove(&supervisor);
                return Err(error);
            }
        }
        Ok(supervisor)
    }

    /// Reserves a slot and starts the member's first incarnation.
    fn admit(
        &mut self,
        supervisor: SupervisorId,
        member: Member,
        dynamic: bool,
    ) -> Result<MembershipId, StartError> {
        let supervision = self.supervisors.get(&supervisor).ok_or(StartError::Stopped)?;
        if supervision.closed {
            return Err(StartError::Stopped);
        }
        let members = supervision.declared.len() + supervision.dynamic.len();
        if members >= supervision.policy.capacity.get() as usize {
            return Err(StartError::CapacityReached);
        }
        if let Member::Process(spec, _) = &member {
            self.check_name(spec)?;
        }
        let membership = MembershipId(self.fresh_id());
        self.memberships.insert(
            membership,
            Membership {
                supervisor,
                member,
                current: Current::None,
                last_incarnation: None,
                handle: false,
            },
        );
        let supervision = self.supervisors.get_mut(&supervisor).expect("checked above");
        if dynamic {
            supervision.dynamic.push(membership);
        } else {
            supervision.declared.push(membership);
        }
        if let Err(error) = self.start_incarnation(membership) {
            self.release_slot(membership);
            self.memberships.remove(&membership);
            return Err(error);
        }
        Ok(membership)
    }

    fn check_name(&self, spec: &ProcessSpec) -> Result<(), StartError> {
        match spec.name {
            Some(name) if self.names.contains_key(&name) => Err(StartError::NameTaken(name)),
            _ => Ok(()),
        }
    }

    /// Starts a new incarnation of a process, or rebuilds a nested supervisor
    /// from its declared members.
    fn start_incarnation(&mut self, membership: MembershipId) -> Result<(), StartError> {
        let entry = &self.memberships[&membership];
        let supervisor = entry.supervisor;
        match entry.member.clone() {
            Member::Process(spec, _) => {
                self.check_name(&spec)?;
                let incarnation = self.spawn(membership, spec);
                let entry = self.memberships.get_mut(&membership).expect("membership record");
                entry.current = Current::Process(incarnation);
                entry.last_incarnation = Some(incarnation);
                self.events.push(LifecycleEvent::Started { supervisor, membership, incarnation });
            }
            Member::Supervisor(policy, _) => {
                let child = self.start_supervisor(policy, Some((supervisor, membership)))?;
                self.memberships.get_mut(&membership).expect("membership record").current =
                    Current::Supervisor(child);
            }
        }
        Ok(())
    }

    /// Creates the heap, fiber, and mailbox, and registers the name, before the
    /// body runs. The body first runs from `run_until_idle`.
    fn spawn(&mut self, membership: MembershipId, spec: ProcessSpec) -> IncarnationId {
        let id = IncarnationId(self.fresh_id());
        let mut exchange = Box::new(Exchange::default());
        let mut start = Box::new(ProcessStart {
            body: spec.body,
            context: ProcessContext { incarnation: id, exchange: &mut *exchange },
        });
        let config = ChimiFiberConfig { stack_bytes: self.stack_bytes, capture_backtraces: 0 };
        let fiber = unsafe {
            chimi_fiber_create(
                Some(process_entry),
                (&mut *start as *mut ProcessStart).cast(),
                config,
            )
        };
        assert!(!fiber.is_null(), "fiber creation failed for {} byte stacks", self.stack_bytes);
        if let Some(name) = spec.name {
            self.names.insert(name, id);
        }
        self.incarnations.insert(
            id,
            Incarnation {
                membership,
                spec,
                fiber,
                mailbox: VecDeque::new(),
                accepting: true,
                deadline: None,
                wait: Wait::Fresh,
                queued: true,
                heap: Heap::new(id.0),
                _start: start,
                exchange,
            },
        );
        self.ready.push_back(id);
        id
    }

    /// Resumes an incarnation and serves its requests until it parks or ends.
    fn step(&mut self, id: IncarnationId, first: ChimiFiberCommand) {
        let mut command = first;
        loop {
            let incarnation = self.incarnations.get_mut(&id).expect("live incarnation");
            let previous = unsafe { chimi_heap_switch(incarnation.heap.raw()) };
            let event = unsafe { chimi_fiber_resume(incarnation.fiber, command as u32) };
            unsafe { chimi_heap_switch(previous) };
            command = ChimiFiberCommand::Resume;
            let cancelling = incarnation.deadline.is_some();
            if event.state != ChimiFiberState::Parked as u32 {
                let cause = terminal_cause(event.state, event.value, cancelling);
                let natural = !cancelling;
                let membership = self.terminate(id, cause);
                if natural {
                    self.member_ended(membership, cause);
                }
                return;
            }
            match incarnation.exchange.request.take() {
                Some(Request::Send { address, message }) => {
                    let result = self.deliver(address, message).map_err(|error| match error {
                        SendError::Full(_) => SendFailure::Full,
                        SendError::Unavailable(_) => SendFailure::Unavailable,
                    });
                    let incarnation = self.incarnations.get_mut(&id).expect("live incarnation");
                    incarnation.exchange.reply = Some(Reply::Sent(result));
                }
                Some(Request::Receive) => match incarnation.mailbox.pop_front() {
                    Some(message) => incarnation.exchange.reply = Some(Reply::Message(message)),
                    None if cancelling => incarnation.exchange.reply = Some(Reply::Cancelled),
                    None => {
                        incarnation.wait = Wait::Receiving;
                        return;
                    }
                },
                Some(Request::Work(duration)) => {
                    let finish = self.now + duration;
                    let reply = match incarnation.deadline {
                        Some(deadline) if finish > deadline => {
                            self.now = self.now.max(deadline);
                            Reply::Forced
                        }
                        _ => {
                            self.now = finish;
                            Reply::Worked
                        }
                    };
                    incarnation.exchange.reply = Some(reply);
                }
                None => panic!("a body parked without a kernel request"),
            }
        }
    }

    /// Releases routing, frees the fiber, mailbox, and heap, and records the end.
    fn terminate(&mut self, id: IncarnationId, cause: TerminalCause) -> MembershipId {
        let incarnation = self.incarnations.remove(&id).expect("live incarnation");
        if let Some(name) = incarnation.spec.name
            && self.names.get(&name) == Some(&id)
        {
            self.names.remove(&name);
        }
        let membership = incarnation.membership;
        drop(incarnation);
        if let Some(entry) = self.memberships.get_mut(&membership) {
            entry.current = Current::None;
        }
        self.events.push(LifecycleEvent::Terminated { membership, incarnation: id, cause });
        membership
    }

    /// Requested shutdown: unroute, then cancel and run cleanup to its end.
    /// Cleanup cannot park (receive answers at once after cancellation), so it
    /// ends by returning, by defecting, or by `work` reaching the deadline.
    fn shut_down(&mut self, id: IncarnationId) {
        let Some(incarnation) = self.incarnations.get_mut(&id) else { return };
        if let Some(name) = incarnation.spec.name
            && self.names.get(&name) == Some(&id)
        {
            self.names.remove(&name);
        }
        incarnation.accepting = false;
        incarnation.deadline = Some(self.now + incarnation.spec.shutdown_within);
        if incarnation.wait == Wait::Receiving {
            let reply = incarnation.mailbox.pop_front().map_or(Reply::Cancelled, Reply::Message);
            incarnation.exchange.reply = Some(reply);
        }
        self.step(id, ChimiFiberCommand::Cancel);
        assert!(
            !self.incarnations.contains_key(&id),
            "cleanup parked, or a forced body caught its termination unwind"
        );
    }

    /// Reacts to a body ending on its own: restart a defect, or free the slot
    /// and the membership record.
    fn member_ended(&mut self, membership: MembershipId, cause: TerminalCause) {
        let Some(entry) = self.memberships.get(&membership) else { return };
        let supervisor = entry.supervisor;
        let open = self.supervisors.get(&supervisor).is_some_and(|s| !s.closed);
        if cause == TerminalCause::Defected && entry.restart() == Restart::OnDefect && open {
            self.restart(supervisor, membership);
        } else {
            self.release_slot(membership);
            self.forget(membership);
        }
    }

    /// Drops a membership record, keeping only what a live handle needs.
    fn forget(&mut self, membership: MembershipId) -> Option<Membership> {
        let entry = self.memberships.remove(&membership)?;
        if entry.handle
            && let Some(incarnation) = entry.last_incarnation
        {
            self.retired.insert(membership, incarnation);
        }
        Some(entry)
    }

    /// Charges the rolling limit before the replacement starts.
    fn restart(&mut self, supervisor: SupervisorId, membership: MembershipId) {
        let now = self.now;
        let supervision = self.supervisors.get_mut(&supervisor).expect("open supervisor");
        let within = supervision.policy.restarts.within;
        while supervision.restart_times.front().is_some_and(|at| *at + within <= now) {
            supervision.restart_times.pop_front();
        }
        let attempts = supervision.restart_times.len() as u32 + 1;
        if attempts > supervision.policy.restarts.limit {
            self.escalate(supervisor);
            return;
        }
        supervision.restart_times.push_back(now);
        self.events.push(LifecycleEvent::RestartAttempted {
            supervisor,
            membership,
            attempts_in_window: attempts,
        });
        if let Err(error) = self.start_incarnation(membership) {
            self.events.push(LifecycleEvent::AdmissionRejected { supervisor, error });
            self.member_ended(membership, TerminalCause::Defected);
        }
    }

    /// The supervisor stops its members and defects to its parent. A root
    /// escalation terminates the program; the prototype assumes one root
    /// supervisor per program, so it does not stop other roots.
    fn escalate(&mut self, supervisor: SupervisorId) {
        self.events.push(LifecycleEvent::RestartLimitExceeded { supervisor });
        self.stop_members(supervisor);
        let parent = self.supervisors[&supervisor].parent;
        self.events.push(LifecycleEvent::Escalated { supervisor, parent: parent.map(|p| p.0) });
        match parent {
            None => self.program_terminated = true,
            Some((_, membership)) => {
                self.supervisors.remove(&supervisor);
                self.memberships.get_mut(&membership).expect("membership record").current =
                    Current::None;
                self.member_ended(membership, TerminalCause::Defected);
            }
        }
    }

    /// Closes admission, unroutes the subtree, then stops members newest
    /// dynamic first and declared in reverse.
    fn stop_members(&mut self, supervisor: SupervisorId) {
        self.close_and_unroute(supervisor);
        let supervision = self.supervisors.get_mut(&supervisor).expect("supervisor record");
        let mut order = std::mem::take(&mut supervision.dynamic);
        order.reverse();
        order.extend(std::mem::take(&mut supervision.declared).into_iter().rev());
        for membership in order {
            let Some(entry) = self.forget(membership) else { continue };
            match entry.current {
                Current::Process(incarnation) => self.shut_down(incarnation),
                Current::Supervisor(child) => {
                    self.stop_members(child);
                    self.supervisors.remove(&child);
                    self.events.push(LifecycleEvent::Stopped { supervisor: child });
                }
                Current::None => {}
            }
        }
    }

    fn close_and_unroute(&mut self, supervisor: SupervisorId) {
        let Some(supervision) = self.supervisors.get_mut(&supervisor) else { return };
        supervision.closed = true;
        let members: Vec<_> =
            supervision.dynamic.iter().chain(&supervision.declared).copied().collect();
        for membership in members {
            match self.memberships.get(&membership).map(|entry| entry.current) {
                Some(Current::Process(id)) => {
                    let incarnation = self.incarnations.get_mut(&id).expect("live incarnation");
                    incarnation.accepting = false;
                    if let Some(name) = incarnation.spec.name
                        && self.names.get(&name) == Some(&id)
                    {
                        self.names.remove(&name);
                    }
                }
                Some(Current::Supervisor(child)) => self.close_and_unroute(child),
                _ => {}
            }
        }
    }

    fn release_slot(&mut self, membership: MembershipId) {
        let supervisor = self.memberships[&membership].supervisor;
        if let Some(supervision) = self.supervisors.get_mut(&supervisor) {
            supervision.declared.retain(|m| *m != membership);
            supervision.dynamic.retain(|m| *m != membership);
        }
    }
}

impl Drop for Kernel {
    fn drop(&mut self) {
        for root in std::mem::take(&mut self.roots) {
            self.stop(Running(root));
        }
        let mut leftover: Vec<_> = self.incarnations.keys().copied().collect();
        leftover.sort();
        for id in leftover {
            self.shut_down(id);
        }
    }
}

fn terminal_cause(state: u32, value: i64, cancelling: bool) -> TerminalCause {
    let end = if state == ChimiFiberState::Succeeded as u32 {
        value
    } else if state == ChimiFiberState::Cancelled as u32 {
        END_RETURNED
    } else {
        END_DEFECTED
    };
    match (end, cancelling) {
        (END_FORCED, _) => TerminalCause::Forced,
        (END_DEFECTED, _) => TerminalCause::Defected,
        (_, true) => TerminalCause::Cancelled,
        (END_FAILED, false) => TerminalCause::Failed,
        _ => TerminalCause::Returned,
    }
}
