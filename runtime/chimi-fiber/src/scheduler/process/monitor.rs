//! Monitors, the own address, and mailbox metrics
//! (`docs/specification/process-monitors-and-metrics.md`).
//!
//! Invariants:
//! - A monitor record lives in `ProcessKernel::monitors` from `monitor` until
//!   its target ends, its watcher ends, or `unmonitor`. While it lives, its id
//!   is in the target's `watched_by` and the watcher's `watching`.
//! - A notice handle (the watcher's `fn(Down) -> M`, a host handle like a
//!   message) belongs to exactly one place: the monitor record, then the
//!   watcher's mailbox once the notice is queued, then the receive that takes
//!   it. Every other path releases it through the message release hook.
//! - A queued notice is not counted against the mailbox capacity; each
//!   monitor queues at most one.

use super::{Address, Entry, IncarnationId, Receipt, TerminalCause};
use crate::scheduler::Scheduler;
use std::num::NonZeroU64;

/// One watch an incarnation set on another with `Process.monitor`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MonitorId(NonZeroU64);

impl MonitorId {
    pub fn get(self) -> u64 {
        self.0.get()
    }

    pub fn from_raw(raw: u64) -> Option<Self> {
        NonZeroU64::new(raw).map(Self)
    }
}

/// How a monitored incarnation ended, as its watcher's notice reports it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Down {
    Returned,
    Failed,
    Defected,
    /// Stopped or removed by its supervisor, cleanup finished or forced.
    Stopped,
    /// No incarnation existed when the monitor was created.
    Unavailable,
}

impl Down {
    /// The `CHIMI_DOWN_*` code a receive reply carries; never 0, which marks
    /// an ordinary message.
    pub fn code(self) -> u32 {
        match self {
            Self::Returned => 1,
            Self::Failed => 2,
            Self::Defected => 3,
            Self::Stopped => 4,
            Self::Unavailable => 5,
        }
    }

    fn from_cause(cause: TerminalCause) -> Self {
        match cause {
            TerminalCause::Returned => Self::Returned,
            TerminalCause::Failed => Self::Failed,
            TerminalCause::Defected => Self::Defected,
            TerminalCause::Cancelled | TerminalCause::Forced => Self::Stopped,
        }
    }
}

/// A notice queued in, or taken from, a watcher's mailbox.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Notice {
    pub monitor: MonitorId,
    /// The watcher's host handle for its `fn(Down) -> M`.
    pub handle: u64,
    pub down: Down,
}

pub(in crate::scheduler) struct MonitorRecord {
    watcher: IncarnationId,
    target: IncarnationId,
    handle: u64,
}

impl Scheduler {
    /// Watches the incarnation `target` resolves to now. Ensures: the notice
    /// handle is owned by the record, or by `Down::Unavailable`'s notice when
    /// there is nothing to watch.
    pub fn process_monitor(
        &mut self,
        watcher: IncarnationId,
        target: Address,
        handle: u64,
    ) -> MonitorId {
        self.kernel.next_monitor += 1;
        let monitor = MonitorId::from_raw(self.kernel.next_monitor).expect("nonzero");
        let Some(target) = self.resolve(target) else {
            self.queue_notice(
                watcher,
                Notice {
                    monitor,
                    handle,
                    down: Down::Unavailable,
                },
            );
            return monitor;
        };
        let incarnations = &mut self.kernel.incarnations;
        incarnations
            .get_mut(&target)
            .expect("resolved")
            .watched_by
            .push(monitor);
        incarnations
            .get_mut(&watcher)
            .expect("live watcher")
            .watching
            .push(monitor);
        self.kernel.monitors.insert(
            monitor,
            MonitorRecord {
                watcher,
                target,
                handle,
            },
        );
        monitor
    }

    /// Ensures: `watcher` never receives `monitor`'s notice. A monitor that
    /// another incarnation set, or that is already gone, is ignored.
    pub fn process_unmonitor(&mut self, watcher: IncarnationId, monitor: MonitorId) {
        if self
            .kernel
            .monitors
            .get(&monitor)
            .is_some_and(|record| record.watcher == watcher)
        {
            let record = self.kernel.monitors.remove(&monitor).expect("checked");
            self.unlink(&record, monitor);
            self.release_message(record.handle);
            return;
        }
        let Some(record) = self.kernel.incarnations.get_mut(&watcher) else {
            return;
        };
        let queued = record
            .mailbox
            .iter()
            .position(|entry| matches!(entry, Entry::Notice(notice) if notice.monitor == monitor));
        if let Some(Entry::Notice(notice)) = queued.and_then(|index| record.mailbox.remove(index)) {
            record.notices -= 1;
            self.release_message(notice.handle);
        }
    }

    /// Entries waiting in `incarnation`'s mailbox (messages and notices) and
    /// its capacity.
    pub fn mailbox_metrics(&self, incarnation: IncarnationId) -> (usize, u32) {
        let record = &self.kernel.incarnations[&incarnation];
        (record.mailbox.len(), record.spec.mailbox_capacity.get())
    }

    /// Ends every monitor that involves `incarnation`, which has just been
    /// removed from the kernel with these lists, ending as `cause`.
    pub(super) fn end_monitors(
        &mut self,
        watched_by: Vec<MonitorId>,
        watching: Vec<MonitorId>,
        cause: TerminalCause,
    ) {
        let down = Down::from_cause(cause);
        for monitor in watched_by {
            let Some(record) = self.kernel.monitors.remove(&monitor) else {
                continue;
            };
            self.unlink(&record, monitor);
            self.queue_notice(
                record.watcher,
                Notice {
                    monitor,
                    handle: record.handle,
                    down,
                },
            );
        }
        for monitor in watching {
            if let Some(record) = self.kernel.monitors.remove(&monitor) {
                self.unlink(&record, monitor);
                self.release_message(record.handle);
            }
        }
    }

    /// Queues `notice` past the capacity, waking a parked receive, or
    /// releases it when the watcher is gone or no longer accepting.
    fn queue_notice(&mut self, watcher: IncarnationId, notice: Notice) {
        let Some(record) = self
            .kernel
            .incarnations
            .get_mut(&watcher)
            .filter(|record| record.accepting)
        else {
            self.release_message(notice.handle);
            return;
        };
        record.mailbox.push_back(Entry::Notice(notice));
        record.notices += 1;
        if std::mem::take(&mut record.receiving) {
            let task = record.task;
            let _ = self.wake(task);
        }
    }

    /// Removes `monitor` from the lists of whichever of its ends still live.
    fn unlink(&mut self, record: &MonitorRecord, monitor: MonitorId) {
        if let Some(target) = self.kernel.incarnations.get_mut(&record.target) {
            target.watched_by.retain(|id| *id != monitor);
        }
        if let Some(watcher) = self.kernel.incarnations.get_mut(&record.watcher) {
            watcher.watching.retain(|id| *id != monitor);
        }
    }
}

/// What a receive takes from the front of a mailbox whose notice count is
/// `notices`.
pub(super) fn receipt(notices: &mut usize, entry: Entry) -> Receipt {
    match entry {
        Entry::Message(message) => Receipt::Message(message),
        Entry::Notice(notice) => {
            *notices -= 1;
            Receipt::Notice(notice)
        }
    }
}
