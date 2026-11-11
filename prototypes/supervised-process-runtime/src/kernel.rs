//! Supervision kernel: process incarnations, mailboxes, names, and supervisors.
//!
//! Global invariants:
//! - Each incarnation runs on its own fiber with its own `Heap`; the kernel
//!   switches the current heap around every resume.
//! - Every message crosses into a mailbox as a `Fragment` and belongs to exactly
//!   one mailbox until it is received (adopted) or discarded (freed).
//! - Incarnations of one membership never overlap, and a name maps to at most
//!   one live incarnation.
//! - Time is virtual. The kernel only moves it forward inside `advance` and
//!   when a blocking operation must reach a shutdown deadline, so every run is
//!   deterministic and `events` is an exact transcript.
//!
//! The host-side API stands in for intrinsic calls from a root task; bodies
//! reach the kernel through `ProcessContext`, which parks on fiber requests.

use crate::heap::{Fragment, HeapStats, Value};
use std::num::NonZeroU32;
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
    _private: (),
}

#[derive(Debug)]
pub enum Received {
    /// The oldest message, adopted into this incarnation's heap.
    Message(*mut Value),
    /// A stop or removal was requested; the body should clean up and return.
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendFailure {
    Full,
    Unavailable,
}

impl ProcessContext {
    /// Ensures: parks only while the mailbox is empty; FIFO across senders.
    pub fn receive(&mut self) -> Received {
        todo!()
    }

    /// Requires: `message` belongs to this incarnation's heap.
    /// Ensures: never parks. On failure the copy is discarded and the caller
    /// still holds `message`.
    pub fn send(&mut self, address: Address, message: *const Value) -> Result<(), SendFailure> {
        todo!("send {address:?} {message:p}")
    }

    pub fn incarnation(&self) -> IncarnationId {
        todo!()
    }
}

pub struct Kernel {
    _private: (),
}

impl Kernel {
    pub fn new(stack_bytes: usize) -> Self {
        todo!("kernel with {stack_bytes} byte stacks")
    }

    /// Ensures: every declared member has started, or every started member is
    /// stopped again and the first failure is returned.
    pub fn start(&mut self, policy: Policy) -> Result<Running, StartError> {
        todo!("{policy:?}")
    }

    /// Ensures: fail-fast admission; a name is registered before the body runs.
    pub fn start_process(
        &mut self,
        running: Running,
        spec: ProcessSpec,
        restart: Restart,
    ) -> Result<ManagedProcess, StartError> {
        todo!("{running:?} {spec:?} {restart:?}")
    }

    /// Ensures: a direct address pinned to the current incarnation.
    pub fn address(&self, managed: &ManagedProcess) -> Address {
        todo!("{managed:?}")
    }

    /// Host-side send for tests. Same outcomes as `ProcessContext::send`.
    pub fn send(&mut self, address: Address, message: Fragment) -> Result<(), SendError> {
        todo!("{address:?} {message:?}")
    }

    /// Ensures: on return no incarnation of the membership runs and no pending
    /// restart can create one.
    pub fn remove_process(&mut self, running: Running, managed: ManagedProcess) {
        todo!("{running:?} {managed:?}")
    }

    /// Ensures: on return the whole subtree has terminated, dynamic members
    /// newest first, then declared members in reverse order; unread messages
    /// are freed. Jumps virtual time to deadlines that cleanup does not meet.
    pub fn stop(&mut self, running: Running) {
        todo!("{running:?}")
    }

    /// Runs ready fibers until every live incarnation is parked or finished.
    pub fn run_until_idle(&mut self) {
        todo!()
    }

    /// Moves virtual time forward; restart windows and deadlines use it.
    pub fn advance(&mut self, by: Duration) {
        todo!("{by:?}")
    }

    pub fn events(&self) -> &[LifecycleEvent] {
        todo!()
    }

    /// True once a restart-limit escalation has reached the root.
    pub fn program_terminated(&self) -> bool {
        todo!()
    }

    /// Heap statistics of the incarnation an address currently reaches.
    pub fn heap_stats(&self, address: Address) -> Option<HeapStats> {
        todo!("{address:?}")
    }
}
