//! std/test's recorders, scripts and test mailboxes, and `Process.send` to a
//! test address; `execution/core_operations.rs`'s `TestLog` is the
//! interpreter's, and the defect messages match it. Each holds detached
//! fragments outside every heap, in one table behind one lock: a log's value
//! can reach tasks on any carrier. Nothing under the lock parks; it is taken
//! before the handle counts' lock. Values are allocated in the order
//! `runtime.c` allocated them, so memory reports do not change.

use alloc::vec;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr;

use super::message::Message;
use super::standard::{defect, defect_with, failure, shape_defect, standard_value, success, void_value};
use crate::heap::allocator::current_carrier;
use crate::heap::fragment;
use crate::heap::layout::{
    ADDRESS_TEST, AllocationKind, Control, Field, HeapStatus, Intrinsic, RuntimeFragment, StandardConstructor, Value, ValueKind,
};
use crate::heap::list;
use crate::heap::values::{allocate_kind, new_value};
use crate::pthread_locked::PthreadLocked;

unsafe extern "C" {
    /// Whether `root` reaches a Receiver or Completion (`runtime.c`, which
    /// keeps the capability transfer state until the scheduler requests move).
    fn chimi_runtime_carries_capability_v1(root: *mut Value) -> bool;
}

/// One Recorder, Script or Mailbox: the kind of the value that names it, the
/// fragments it holds, how many a Script has handed out, and a Mailbox's
/// capacity.
struct TestLog {
    kind: ValueKind,
    values: Vec<*mut RuntimeFragment>,
    taken: usize,
    limit: i64,
}

/// Every log, a value's id being its index plus one.
struct Logs(Vec<TestLog>);

// SAFETY: the fragments are detached, owned by the table, and reached only
// under its lock.
unsafe impl Send for Logs {}

static LOGS: PthreadLocked<Logs> = PthreadLocked::new(Logs(Vec::new()));

impl Logs {
    /// A new log of `kind` and the value naming it.
    ///
    /// # Safety
    /// Runs on a fiber that may allocate.
    unsafe fn create(&mut self, kind: ValueKind) -> *mut Value {
        self.0.push(TestLog { kind, values: Vec::new(), taken: 0, limit: 0 });
        // SAFETY: the caller's contract.
        let value = unsafe { new_value(kind) };
        // SAFETY: `value` is the address-shaped value just made.
        unsafe { (*value).payload.address.target = self.0.len() as u64 };
        value
    }

    /// The log `value` names; a value of the wrong kind or an unknown id is a
    /// runtime bug, so it aborts.
    ///
    /// # Safety
    /// `value` is a live Recorder, Script or Mailbox.
    unsafe fn of(&mut self, value: *const Value) -> &mut TestLog {
        // SAFETY: the caller's contract.
        let (id, kind) = unsafe { ((*value).payload.address.target, (*value).kind) };
        self.by_id(id, kind)
    }

    fn by_id(&mut self, id: u64, kind: ValueKind) -> &mut TestLog {
        match id.checked_sub(1).and_then(|index| self.0.get_mut(index as usize)) {
            Some(log) if log.kind == kind => log,
            // SAFETY: abort takes no arguments.
            _ => unsafe { libc::abort() },
        }
    }
}

/// A detached copy of `value`, or the defect naming the heap it reaches into.
///
/// # Safety
/// Runs on a fiber that may allocate; `value` is live.
unsafe fn detach(value: *mut Value, operation: &[u8], node: i64) -> Result<*mut RuntimeFragment, *mut Control> {
    // SAFETY: the caller's contract.
    unsafe {
        let here = current_carrier();
        let roots = [value.cast::<c_void>()];
        let mut copy = ptr::null_mut();
        if fragment::copy(here, roots.as_ptr(), &AllocationKind::VALUE, 1, &mut copy) == HeapStatus::OK {
            return Ok(copy);
        }
        Err(defect_with(foreign_message(operation, b"value", b"caller"), node))
    }
}

/// "<operation>: the <subject> points into owner N, outside the <side>'s
/// heap (owner M)", for a copy that met a pointer into another heap.
///
/// # Safety
/// Right after a [`fragment::copy`] that returned `FOREIGN_POINTER`.
unsafe fn foreign_message(operation: &[u8], subject: &[u8], side: &[u8]) -> *const core::ffi::c_char {
    // SAFETY: the caller's contract; the carrier's fiber has a live heap.
    unsafe {
        let here = current_carrier();
        Message::new()
            .text(operation)
            .text(b": the ")
            .text(subject)
            .text(b" points into owner ")
            .integer((*here).fragment_copy_foreign_owner as i64)
            .text(b", outside the ")
            .text(side)
            .text(b"'s heap (owner ")
            .integer((*(*here).fiber.heap).owner as i64)
            .text(b")")
            .finish()
    }
}

/// A List of the roots of `fragments`, each copied (`keep`) or adopted into
/// the running fiber's heap.
///
/// # Safety
/// Runs on a fiber that may allocate; every fragment is live and unadopted.
unsafe fn list_of(fragments: &mut [*mut RuntimeFragment], keep: bool) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let here = current_carrier();
        let mut values: Vec<*mut Value> = vec![ptr::null_mut(); fragments.len()];
        for (slot, fragment) in values.iter_mut().zip(fragments.iter_mut()) {
            let root = (slot as *mut *mut Value).cast::<*mut c_void>();
            if keep {
                fragment::copy_into_current(here, *fragment, root);
            } else {
                fragment::adopt(here, *fragment, root);
                *fragment = ptr::null_mut();
            }
        }
        let length = values.len() as u32;
        list::list_value(list::from_values(values.as_ptr(), length), 0, length)
    }
}

/// The intrinsic `intrinsic` of this family on `arguments`.
///
/// # Safety
/// Runs on a fiber that may allocate; `arguments` are live.
pub unsafe fn test_log_operation(intrinsic: u32, arguments: &[*mut Value], call_node: u32) -> *mut Control {
    // SAFETY: the caller's contract.
    LOGS.with(|logs| unsafe { locked(logs, intrinsic, arguments, call_node) })
}

/// [`test_log_operation`] with the table's lock held.
///
/// # Safety
/// As for `test_log_operation`.
unsafe fn locked(logs: &mut Logs, intrinsic: u32, arguments: &[*mut Value], call_node: u32) -> *mut Control {
    use ValueKind as K;
    // SAFETY: the caller's contract; payloads are read as their kinds say.
    unsafe {
        let node = i64::from(call_node);
        let kinds = |expected: &[ValueKind]| {
            arguments.len() == expected.len() && arguments.iter().zip(expected).all(|(&value, &kind)| (*value).kind == kind)
        };
        match intrinsic {
            Intrinsic::TEST_RECORDER if arguments.is_empty() => success(logs.create(K::TEST_RECORDER)),
            Intrinsic::TEST_SCRIPT if kinds(&[K::LIST]) => {
                let script = logs.create(K::TEST_SCRIPT);
                for index in 0..(*arguments[0]).payload.list.length {
                    match detach(list::get(arguments[0], index), b"Test.script", node) {
                        Ok(copy) => logs.of(script).values.push(copy),
                        Err(defect) => return defect,
                    }
                }
                success(script)
            }
            Intrinsic::TEST_RECORD if arguments.len() == 2 && (*arguments[0]).kind == K::TEST_RECORDER => {
                match detach(arguments[1], b"Test.record", node) {
                    Ok(copy) => logs.of(arguments[0]).values.push(copy),
                    Err(defect) => return defect,
                }
                success(void_value())
            }
            Intrinsic::TEST_RECORDED if kinds(&[K::TEST_RECORDER]) => {
                success(list_of(&mut logs.of(arguments[0]).values, true))
            }
            Intrinsic::TEST_NEXT if kinds(&[K::TEST_SCRIPT]) => {
                let log = logs.of(arguments[0]);
                if log.taken == log.values.len() {
                    let message = Message::new()
                        .text(b"Test.next: script exhausted after ")
                        .integer(log.taken as i64)
                        .text(b" values");
                    return defect_with(message.finish(), node);
                }
                let mut root = ptr::null_mut();
                fragment::adopt(current_carrier(), log.values[log.taken], &mut root);
                log.values[log.taken] = ptr::null_mut();
                log.taken += 1;
                success(root.cast())
            }
            Intrinsic::TEST_MAILBOX if kinds(&[K::INTEGER]) => {
                let capacity = (*arguments[0]).payload.integer;
                if capacity < 1 {
                    let message = Message::new()
                        .text(b"Test.mailbox: the capacity must be at least 1, not ")
                        .integer(capacity);
                    return defect_with(message.finish(), node);
                }
                let mailbox = logs.create(K::TEST_MAILBOX);
                logs.of(mailbox).limit = capacity;
                let pair = new_value(K::TUPLE);
                (*pair).payload.record.length = 2;
                let fields = allocate_kind::<Field>(2, AllocationKind::FIELDS);
                (*pair).payload.record.fields = fields;
                let address = new_value(K::PROCESS_ADDRESS);
                (*address).payload.address.kind = ADDRESS_TEST;
                (*address).payload.address.target = (*mailbox).payload.address.target;
                *fields = Field { name: ptr::null(), value: address };
                *fields.add(1) = Field { name: ptr::null(), value: mailbox };
                success(pair)
            }
            Intrinsic::TEST_RECEIVED if kinds(&[K::TEST_MAILBOX]) => {
                let log = logs.of(arguments[0]);
                let list = list_of(&mut log.values, false);
                log.values.clear();
                success(list)
            }
            _ => shape_defect(call_node),
        }
    }
}

/// Frees the fragments the log `value` still holds and empties it, returning
/// how many it held: the carrier stress test's leak check
/// (tests/fixtures/carrier-threads-stress.c); programs never release a log.
///
/// # Safety
/// Runs on a carrier; `value` is a live Recorder, Script or Mailbox.
pub unsafe fn release(value: *const Value) -> u32 {
    LOGS.with(|logs| {
        // SAFETY: the caller's contract.
        let log = unsafe { logs.of(value) };
        let held = log.values.len() as u32;
        for &fragment in log.values.iter().filter(|fragment| !fragment.is_null()) {
            // SAFETY: the table owns each unadopted fragment.
            unsafe { fragment::free(current_carrier(), fragment) };
        }
        log.values.clear();
        log.taken = 0;
        held
    })
}

/// `Process.send` of `message` to the test address `address`: the mailbox
/// rules without a scheduler. It never parks; once the mailbox holds its
/// capacity it refuses with `SendError.Full`, and the sender keeps the
/// message. A message carrying a Receiver or Completion would need the
/// capability transfer a process receive makes, so a mailbox with room
/// refuses one as a defect.
///
/// # Safety
/// Runs on a fiber that may allocate; `address` is a live test address and
/// `message` live.
pub unsafe fn send_to_test_mailbox(address: *const Value, message: *mut Value, call_node: u32) -> *mut Control {
    // SAFETY: the caller's contract.
    LOGS.with(|logs| unsafe {
        let node = i64::from(call_node);
        let log = logs.by_id((*address).payload.address.target, ValueKind::TEST_MAILBOX);
        if log.values.len() as i64 >= log.limit {
            return failure(standard_value(StandardConstructor::PROCESS_SEND_FULL, &[message]));
        }
        let here = current_carrier();
        let roots = [message.cast::<c_void>()];
        let mut copy = ptr::null_mut();
        (*here).copy_counting = true;
        let status = fragment::copy(here, roots.as_ptr(), &AllocationKind::VALUE, 1, &mut copy);
        (*here).copy_counting = false;
        if status != HeapStatus::OK {
            return defect_with(foreign_message(b"Process.send", b"message", b"sender"), node);
        }
        if chimi_runtime_carries_capability_v1((*copy).roots[0].cast()) {
            fragment::free(here, copy);
            return defect(c"Process.send: a test mailbox cannot hold a message with a Receiver or Completion", node);
        }
        log.values.push(copy);
        success(void_value())
    })
}
