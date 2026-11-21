//! Work one carrier asks of another (parallel-scheduler.md, "Step 3 plan",
//! PR 5b-3). A started fiber runs only on its home carrier's thread, so an
//! end decided elsewhere reaches it in one of two ways: as a job on the home
//! carrier when the fiber is parked or ready, or as a pending end the home
//! carrier applies when the fiber is running there. With one carrier every
//! decision is made at home, so neither is ever used.

use std::collections::VecDeque;

use super::ready::Carrier;
use super::{TaskId, TaskOutcome};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Job {
    /// Resume the draining task's fiber with Cancel, without the kernel,
    /// then end the task (`Scheduler::end_after_unwind`). `forced` when a
    /// forced end decided it, so its descendants end without waiting.
    Unwind { task: TaskId, forced: bool },
}

/// An end decided while the task's fiber ran on another carrier: its home
/// carrier applies it once the resume returns, as a cancel (`cancel_task`)
/// or, when `forced`, a forced end (`force_terminal`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PendingEnd {
    pub(super) outcome: TaskOutcome,
    pub(super) forced: bool,
}

/// One FIFO of jobs per carrier; a carrier runs its jobs before its ready
/// tasks.
#[derive(Debug)]
pub(super) struct Jobs {
    queues: Vec<VecDeque<Job>>,
}

impl Jobs {
    /// Requires: `carriers` is at least 1.
    pub(super) fn new(carriers: u32) -> Self {
        assert!(carriers >= 1, "a scheduler has at least one carrier");
        Self {
            queues: (0..carriers).map(|_| VecDeque::new()).collect(),
        }
    }

    pub(super) fn push_back(&mut self, carrier: Carrier, job: Job) {
        self.queues[carrier.0 as usize].push_back(job);
    }

    pub(super) fn pop_front(&mut self, carrier: Carrier) -> Option<Job> {
        self.queues[carrier.0 as usize].pop_front()
    }

    /// True when no carrier has a job waiting.
    #[allow(dead_code, reason = "global idle (PR 5b-6) counts jobs")]
    pub(super) fn is_empty(&self) -> bool {
        self.queues.iter().all(VecDeque::is_empty)
    }
}
