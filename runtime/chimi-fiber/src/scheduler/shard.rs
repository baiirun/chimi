//! A carrier's part of the kernel (parallel-scheduler.md, "Step 4 plan",
//! 4d-6): its ready queue, its jobs, and whether it is parked asleep. With
//! one carrier there is one shard, the queues the scheduler always had.
//!
//! Each shard sits in its own `UnsafeCell` on its own cache lines: the
//! shape the local path of step 4e needs, where a carrier on the kernel
//! lock's read side writes its own shard while other carriers write
//! theirs (the access model in the plan). The shared readers are for the
//! write side only.

use std::cell::UnsafeCell;
use std::collections::VecDeque;

use super::TaskId;
use super::jobs::Job;
use super::ready::Carrier;

/// One carrier's state.
#[derive(Debug, Default)]
struct Shard {
    /// Tasks homed here and ready, in FIFO order; only this carrier pops.
    ready: VecDeque<TaskId>,
    /// Work another carrier asked of this one, run before its ready tasks.
    jobs: VecDeque<Job>,
    /// Set while the carrier is parked asleep on its wake pipe, so another
    /// carrier may steal from it (4c). Carrier 0 never parks this way.
    parked: bool,
}

/// A shard on cache lines of its own, so carriers writing neighbouring
/// shards do not share one.
#[repr(align(128))]
#[derive(Debug, Default)]
struct ShardCell(UnsafeCell<Shard>);

/// One shard per carrier, indexed by carrier.
#[derive(Debug)]
pub(super) struct Shards {
    shards: Box<[ShardCell]>,
}

impl Shards {
    /// Requires: `carriers` is at least 1.
    pub(super) fn new(carriers: u32) -> Self {
        assert!(carriers >= 1, "a scheduler has at least one carrier");
        Self {
            shards: (0..carriers).map(|_| ShardCell::default()).collect(),
        }
    }

    pub(super) fn carriers(&self) -> u32 {
        u32::try_from(self.shards.len()).expect("carrier count fits in u32")
    }

    fn shard(&self, carrier: Carrier) -> &Shard {
        // SAFETY: a shard is written through `&mut self`, which cannot
        // coexist with this borrow, or by its own carrier's local path
        // (`own`), which never calls this; the write side, the only other
        // caller, runs with no local path.
        unsafe { &*self.shards[carrier.0 as usize].0.get() }
    }

    fn each(&self) -> impl Iterator<Item = &Shard> {
        (0..self.carriers()).map(|carrier| self.shard(Carrier(carrier)))
    }

    fn shard_mut(&mut self, carrier: Carrier) -> &mut Shard {
        self.shards[carrier.0 as usize].0.get_mut()
    }

    /// `carrier`'s own shard, for the local path.
    ///
    /// # Safety
    ///
    /// As for `Homed::homed_mut`: the caller may write what is homed on
    /// `carrier`, and holds no other reference to this shard.
    #[allow(clippy::mut_from_ref)]
    pub(super) unsafe fn own(&self, carrier: Carrier) -> &mut OwnShard {
        // SAFETY: the caller's contract. `OwnShard` is `Shard` with only
        // the operations a carrier does to its own shard.
        unsafe { &mut *self.shards[carrier.0 as usize].0.get().cast::<OwnShard>() }
    }

    // ----- Ready queues --------------------------------------------------

    pub(super) fn push_ready(&mut self, carrier: Carrier, task: TaskId) {
        self.shard_mut(carrier).ready.push_back(task);
    }

    pub(super) fn push_ready_front(&mut self, carrier: Carrier, task: TaskId) {
        self.shard_mut(carrier).ready.push_front(task);
    }

    pub(super) fn pop_ready(&mut self, carrier: Carrier) -> Option<TaskId> {
        self.shard_mut(carrier).ready.pop_front()
    }

    /// The task `pop_ready` would take from `carrier`'s queue.
    pub(super) fn front_ready(&self, carrier: Carrier) -> Option<TaskId> {
        self.shard(carrier).ready.front().copied()
    }

    /// Takes `task` out of `carrier`'s queue wherever it waits there.
    pub(super) fn remove_ready(&mut self, carrier: Carrier, task: TaskId) {
        let queue = &mut self.shard_mut(carrier).ready;
        if queue.front() == Some(&task) {
            queue.pop_front();
        } else {
            queue.retain(|queued| *queued != task);
        }
    }

    pub(super) fn ready_is_empty_for(&self, carrier: Carrier) -> bool {
        self.shard(carrier).ready.is_empty()
    }

    /// True when no carrier has a task ready.
    pub(super) fn no_ready(&self) -> bool {
        self.each().all(|shard| shard.ready.is_empty())
    }

    // ----- Jobs ----------------------------------------------------------

    pub(super) fn push_job(&mut self, carrier: Carrier, job: Job) {
        self.shard_mut(carrier).jobs.push_back(job);
    }

    pub(super) fn pop_job(&mut self, carrier: Carrier) -> Option<Job> {
        self.shard_mut(carrier).jobs.pop_front()
    }

    pub(super) fn jobs_empty_for(&self, carrier: Carrier) -> bool {
        self.shard(carrier).jobs.is_empty()
    }

    /// True when no carrier has a job waiting.
    pub(super) fn no_jobs(&self) -> bool {
        self.each().all(|shard| shard.jobs.is_empty())
    }

    // ----- Parking -------------------------------------------------------

    pub(super) fn is_parked(&self, carrier: Carrier) -> bool {
        self.shard(carrier).parked
    }

    pub(super) fn set_parked(&mut self, carrier: Carrier, parked: bool) {
        self.shard_mut(carrier).parked = parked;
    }

    /// True when every carrier but 0 is parked.
    pub(super) fn others_parked(&self) -> bool {
        self.each().skip(1).all(|shard| shard.parked)
    }
}

/// A carrier's own shard as the local path reaches it.
#[repr(transparent)]
pub(super) struct OwnShard(Shard);

impl OwnShard {
    pub(super) fn front_ready(&self) -> Option<TaskId> {
        self.0.ready.front().copied()
    }

    pub(super) fn pop_ready(&mut self) -> Option<TaskId> {
        self.0.ready.pop_front()
    }

    pub(super) fn push_ready(&mut self, task: TaskId, front: bool) {
        if front {
            self.0.ready.push_front(task);
        } else {
            self.0.ready.push_back(task);
        }
    }

    pub(super) fn has_jobs(&self) -> bool {
        !self.0.jobs.is_empty()
    }

    /// The carrier is awake: it is taking the lock.
    pub(super) fn unpark(&mut self) {
        self.0.parked = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each carrier's queue is FIFO, a front push jumps only its own queue,
    /// and removing a task leaves the order of the rest.
    #[test]
    fn each_carrier_keeps_its_own_order() {
        let mut shards = Shards::new(2);
        let (zero, one) = (Carrier(0), Carrier(1));
        for id in 1..=3 {
            shards.push_ready(zero, TaskId::from_raw(id));
        }
        shards.push_ready(one, TaskId::from_raw(10));
        shards.push_ready_front(one, TaskId::from_raw(11));
        shards.remove_ready(zero, TaskId::from_raw(2));
        assert_eq!(shards.pop_ready(one), Some(TaskId::from_raw(11)));
        assert_eq!(shards.pop_ready(zero), Some(TaskId::from_raw(1)));
        assert_eq!(shards.pop_ready(zero), Some(TaskId::from_raw(3)));
        assert_eq!(shards.pop_ready(zero), None);
        assert!(!shards.no_ready(), "carrier 1 still holds a task");
        assert_eq!(shards.pop_ready(one), Some(TaskId::from_raw(10)));
        assert!(shards.no_ready());
    }

    /// Jobs and parking are per carrier too.
    #[test]
    fn jobs_and_parking_are_per_carrier() {
        let mut shards = Shards::new(3);
        let job = Job::Unwind { task: TaskId::from_raw(1), forced: false };
        shards.push_job(Carrier(2), job);
        assert!(shards.jobs_empty_for(Carrier(1)) && !shards.jobs_empty_for(Carrier(2)));
        assert!(!shards.no_jobs());
        assert_eq!(shards.pop_job(Carrier(2)), Some(job));
        assert!(shards.no_jobs());
        shards.set_parked(Carrier(1), true);
        assert!(!shards.others_parked(), "carrier 2 is awake");
        shards.set_parked(Carrier(2), true);
        assert!(shards.others_parked() && !shards.is_parked(Carrier(0)));
    }
}
