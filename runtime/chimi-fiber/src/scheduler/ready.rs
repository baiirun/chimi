//! The ready queues: one FIFO per carrier, held in the kernel
//! (parallel-scheduler.md, "Step 3 plan", PR 2). A task is queued on its home
//! carrier, the one its fiber runs on, and only that carrier pops it. With one
//! carrier this is the single queue the scheduler always had.

use std::collections::VecDeque;

use super::TaskId;

/// A carrier thread's index, from 0. Carrier 0 owns the root, signals and
/// standard input.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct Carrier(pub(super) u32);

#[derive(Debug)]
pub(super) struct ReadyQueues {
    queues: Vec<VecDeque<TaskId>>,
}

impl ReadyQueues {
    /// Requires: `carriers` is at least 1.
    pub(super) fn new(carriers: u32) -> Self {
        assert!(carriers >= 1, "a scheduler has at least one carrier");
        Self {
            queues: (0..carriers).map(|_| VecDeque::new()).collect(),
        }
    }

    pub(super) fn push_back(&mut self, carrier: Carrier, task: TaskId) {
        self.queue(carrier).push_back(task);
    }

    pub(super) fn push_front(&mut self, carrier: Carrier, task: TaskId) {
        self.queue(carrier).push_front(task);
    }

    pub(super) fn pop_front(&mut self, carrier: Carrier) -> Option<TaskId> {
        self.queue(carrier).pop_front()
    }

    /// Takes `task` out of `carrier`'s queue wherever it waits there.
    pub(super) fn remove(&mut self, carrier: Carrier, task: TaskId) {
        let queue = self.queue(carrier);
        if queue.front() == Some(&task) {
            queue.pop_front();
        } else {
            queue.retain(|queued| *queued != task);
        }
    }

    pub(super) fn carriers(&self) -> u32 {
        u32::try_from(self.queues.len()).expect("carrier count fits in u32")
    }

    pub(super) fn is_empty_for(&self, carrier: Carrier) -> bool {
        self.queues[carrier.0 as usize].is_empty()
    }

    /// True when no carrier has a task ready.
    pub(super) fn is_empty(&self) -> bool {
        self.queues.iter().all(VecDeque::is_empty)
    }

    fn queue(&mut self, carrier: Carrier) -> &mut VecDeque<TaskId> {
        &mut self.queues[carrier.0 as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each carrier's queue is FIFO, a front push jumps only its own queue,
    /// and removing a task leaves the order of the rest.
    #[test]
    fn each_carrier_keeps_its_own_order() {
        let mut ready = ReadyQueues::new(2);
        let (zero, one) = (Carrier(0), Carrier(1));
        for id in 1..=3 {
            ready.push_back(zero, TaskId(id));
        }
        ready.push_back(one, TaskId(10));
        ready.push_front(one, TaskId(11));
        ready.remove(zero, TaskId(2));
        assert_eq!(ready.pop_front(one), Some(TaskId(11)));
        assert_eq!(ready.pop_front(zero), Some(TaskId(1)));
        assert_eq!(ready.pop_front(zero), Some(TaskId(3)));
        assert_eq!(ready.pop_front(zero), None);
        assert!(!ready.is_empty(), "carrier 1 still holds a task");
        assert_eq!(ready.pop_front(one), Some(TaskId(10)));
        assert!(ready.is_empty());
    }
}
