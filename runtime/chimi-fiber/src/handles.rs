//! Counts of the language runtime's channel and task handle values, and the
//! releases they lead to (`include/chimi_fiber.h`, "Handle counts").
//!
//! Every live Sender, Receiver or Completion value is one reference to its
//! channel, and every live Task value one reference to its task. The host
//! calls `chimi_fiber_handle_retain` when it makes such a value (or copies
//! one) and `chimi_fiber_handle_drop` when it frees one, from any carrier
//! thread, with no lock of the scheduler's held. When a count reaches zero
//! no value can name the channel or task again, so the scheduler may end it.
//!
//! A count can reach zero where the scheduler cannot act: inside a release
//! hook the scheduler is running, or in a collection on a fiber. So the key
//! waits in a queue of the thread that dropped it, and the scheduler takes
//! it from there at its next chance on that thread: when a fiber running
//! there makes a request (the release is then attributed to that task), or
//! when the carrier has nothing ready.
//!
//! The counts are one table for the process, behind a mutex, because a
//! value made on one carrier can be freed on another: only the total means
//! anything, and a retain must be counted before the copy can reach another
//! carrier. Each `chimi_scheduler_run` starts the table afresh, since every
//! run's scheduler numbers its channels and tasks from the start; a key
//! queued by an earlier run (a value freed after it returned) is discarded.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Set in a key that names a task; a key without it names a channel.
/// Channel and task ids stay far below this bit.
pub const TASK_HANDLE_KEY: u64 = 1 << 63;

/// What a queued key asks the scheduler to release.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Released {
    Channel(u64),
    Task(u64),
}

impl Released {
    fn from_key(key: u64) -> Self {
        if key & TASK_HANDLE_KEY != 0 {
            Self::Task(key & !TASK_HANDLE_KEY)
        } else {
            Self::Channel(key)
        }
    }
}

/// A multiplicative hash: keys are small sequential ids, which SipHash
/// would spend most of a retain on.
#[derive(Default)]
struct IdHasher(u64);

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = value.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

type Counts = HashMap<u64, u32, BuildHasherDefault<IdHasher>>;

/// Handle counts: key to the number of live values naming it.
pub(crate) struct HandleCounts {
    counts: Mutex<Counts>,
}

impl HandleCounts {
    pub(crate) const fn new() -> Self {
        Self {
            counts: Mutex::new(HashMap::with_hasher(BuildHasherDefault::new())),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Counts> {
        // A panic under this lock aborts the process (panic = abort in the
        // runtime build), so a poisoned lock only arises in tests.
        self.counts.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn retain(&self, key: u64) {
        *self.lock().entry(key).or_insert(0) += 1;
    }

    /// Removes one reference. Ensures: true exactly when this removed the
    /// last one, so the caller owns the key's release. No other thread can
    /// retain the key afterwards: once its count is zero no value names it.
    /// A key the table does not hold (one an earlier run counted) is ignored.
    pub(crate) fn drop_one(&self, key: u64) -> bool {
        let mut counts = self.lock();
        let Some(count) = counts.get_mut(&key) else {
            return false;
        };
        *count -= 1;
        if *count != 0 {
            return false;
        }
        counts.remove(&key);
        true
    }

    pub(crate) fn clear(&self) {
        self.lock().clear();
    }

    #[cfg(test)]
    pub(crate) fn count(&self, key: u64) -> u32 {
        self.lock().get(&key).copied().unwrap_or(0)
    }
}

#[cfg(not(test))]
static COUNTS: HandleCounts = HandleCounts::new();

/// Which run's keys a queue holds; bumped at the start of every run.
#[cfg(not(test))]
static RUN: AtomicU64 = AtomicU64::new(0);

// Unit tests run many schedulers at once on their own threads, each of
// them numbering its channels from the start, so under test the table and
// the run number belong to the thread. A test that counts handles across
// threads uses a `HandleCounts` of its own.
#[cfg(test)]
thread_local! {
    static COUNTS: HandleCounts = const { HandleCounts::new() };
    static RUN: AtomicU64 = const { AtomicU64::new(0) };
}

#[cfg(not(test))]
fn with_counts<T>(body: impl FnOnce(&HandleCounts) -> T) -> T {
    body(&COUNTS)
}

#[cfg(test)]
fn with_counts<T>(body: impl FnOnce(&HandleCounts) -> T) -> T {
    COUNTS.with(body)
}

#[cfg(not(test))]
fn with_run<T>(body: impl FnOnce(&AtomicU64) -> T) -> T {
    body(&RUN)
}

#[cfg(test)]
fn with_run<T>(body: impl FnOnce(&AtomicU64) -> T) -> T {
    RUN.with(body)
}

/// Keys this thread dropped to zero and the scheduler has not taken yet,
/// with the run they belong to.
struct Queue {
    run: u64,
    keys: Vec<u64>,
}

thread_local! {
    static QUEUE: RefCell<Queue> = const { RefCell::new(Queue { run: 0, keys: Vec::new() }) };
}

/// Starts a run: no handle is counted, and no key queued by an earlier run
/// is taken. Called before the run's first task, so no value names any of
/// its channels or tasks yet.
pub(crate) fn begin_run() {
    with_counts(HandleCounts::clear);
    with_run(|run| run.fetch_add(1, Ordering::Relaxed));
}

/// Queues `key` on this thread for the scheduler to release.
pub(crate) fn queue(key: u64) {
    let run = with_run(|run| run.load(Ordering::Relaxed));
    // After this thread's locals are gone (a value freed during thread
    // exit) there is no scheduler left to release the key: it is dropped.
    let _ = QUEUE.try_with(|queue| {
        let mut queue = queue.borrow_mut();
        if queue.run != run {
            queue.keys.clear();
            queue.run = run;
        }
        queue.keys.push(key);
    });
}

/// The next release this thread queued for the current run, if any.
pub(crate) fn take() -> Option<Released> {
    let run = with_run(|run| run.load(Ordering::Relaxed));
    QUEUE
        .try_with(|queue| {
            let mut queue = queue.borrow_mut();
            if queue.run != run {
                queue.keys.clear();
                queue.run = run;
                return None;
            }
            queue.keys.pop().map(Released::from_key)
        })
        .ok()
        .flatten()
}

#[unsafe(no_mangle)]
/// Counts one more live value naming `key`: a channel id, or a task id with
/// `CHIMI_TASK_HANDLE_KEY` set. Safe from any thread.
pub extern "C" fn chimi_fiber_handle_retain(key: u64) {
    with_counts(|counts| counts.retain(key));
}

#[unsafe(no_mangle)]
/// Counts one live value naming `key` fewer. At zero the key's channel or
/// task is released by the scheduler at its next chance on this thread.
/// Safe from any thread, and inside a scheduler hook.
pub extern "C" fn chimi_fiber_handle_drop(key: u64) {
    if with_counts(|counts| counts.drop_one(key)) {
        queue(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_reaches_zero_only_with_its_last_reference() {
        let counts = HandleCounts::new();
        counts.retain(7);
        counts.retain(7);
        assert!(!counts.drop_one(7));
        assert_eq!(counts.count(7), 1);
        assert!(counts.drop_one(7));
        assert_eq!(counts.count(7), 0);
        // An unknown key, as a value an earlier run counted, is ignored.
        assert!(!counts.drop_one(7));
    }

    #[test]
    fn task_keys_and_channel_keys_are_apart() {
        assert_eq!(Released::from_key(5), Released::Channel(5));
        assert_eq!(Released::from_key(5 | TASK_HANDLE_KEY), Released::Task(5));
        let counts = HandleCounts::new();
        counts.retain(5);
        counts.retain(5 | TASK_HANDLE_KEY);
        assert!(counts.drop_one(5 | TASK_HANDLE_KEY));
        assert_eq!(counts.count(5), 1);
    }

    /// A value made on one thread and freed on another, while that thread
    /// also inserts and removes keys of its own so the table rehashes: no
    /// drop reaches zero while the maker's anchor reference is held, and
    /// dropping each anchor at the end is the last reference.
    #[test]
    fn counts_hold_across_threads() {
        const KEYS: u64 = 512;
        const ROUNDS: usize = 50;
        let counts = std::sync::Arc::new(HandleCounts::new());
        for key in 1..=KEYS {
            counts.retain(key);
        }
        let (sender, receiver) = std::sync::mpsc::sync_channel::<u64>(1024);
        let producer = {
            let counts = counts.clone();
            std::thread::spawn(move || {
                for round in 0..ROUNDS {
                    for key in 1..=KEYS {
                        counts.retain(key);
                        let own = (1 << 40) + (round as u64) * KEYS + key;
                        counts.retain(own);
                        sender.send(key).unwrap();
                        assert!(counts.drop_one(own));
                    }
                }
            })
        };
        let consumer = {
            let counts = counts.clone();
            std::thread::spawn(move || {
                let mut own = 2_u64 << 40;
                for key in receiver {
                    assert!(!counts.drop_one(key), "key {key} reached zero while its anchor was held");
                    own += 1;
                    counts.retain(own);
                    assert!(counts.drop_one(own));
                }
            })
        };
        producer.join().unwrap();
        consumer.join().unwrap();
        for key in 1..=KEYS {
            assert!(counts.drop_one(key), "the anchor of {key} was not the last reference");
        }
    }

    #[test]
    fn a_queue_holds_only_the_current_runs_keys() {
        begin_run();
        queue(3);
        queue(4 | TASK_HANDLE_KEY);
        assert_eq!(take(), Some(Released::Task(4)));
        begin_run();
        assert_eq!(take(), None, "a key of an earlier run is discarded");
        chimi_fiber_handle_retain(9);
        chimi_fiber_handle_retain(9);
        chimi_fiber_handle_drop(9);
        assert_eq!(take(), None);
        chimi_fiber_handle_drop(9);
        assert_eq!(take(), Some(Released::Channel(9)));
        assert_eq!(take(), None);
    }
}
