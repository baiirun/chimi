//! The bytes waiting for one console file (standard output, standard error,
//! or `--log-file`) when the native runtime writes it without blocking
//! (CHI-266), and the lines their writers wait on.
//!
//! A writer queues its line and waits until the queue's writes pass the
//! line's end. Offsets count every byte ever queued, so a line's end stays
//! valid while the front of the queue is written and dropped. A failed write
//! drops every queued byte and fails each line not yet fully written; a line
//! already written stays written. `intrinsics/console_io.rs` makes the
//! system calls and holds the lock; this keeps the state.
//!
//! Growing the queue reports running out of memory instead of aborting, so
//! the runtime exits as it does for any exhausted allocation and still
//! writes the lines already queued.

use alloc::vec::Vec;

use crate::timestamp;

/// Where a waiting line stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineState {
    Pending,
    Written,
    /// A failed write dropped it, with this error number.
    Failed(i32),
}

#[derive(Clone, Copy, Debug)]
struct Line {
    id: u64,
    /// Queue offset just past the line.
    end: u64,
    /// Nonzero once a failed write dropped the line.
    error: i32,
}

/// One file's queue.
pub struct ConsoleQueue {
    bytes: Vec<u8>,
    /// Index of the first unwritten byte in `bytes`.
    start: usize,
    /// Queue offset of `bytes[start]`: every byte before it is written or
    /// dropped.
    resolved: u64,
    /// The lines writers wait on, in queue order.
    lines: Vec<Line>,
    next_line: u64,
}

impl Default for ConsoleQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl ConsoleQueue {
    pub const fn new() -> Self {
        Self {
            bytes: Vec::new(),
            start: 0,
            resolved: 0,
            lines: Vec::new(),
            next_line: 0,
        }
    }

    /// Bytes queued and not yet written.
    pub fn queued(&self) -> usize {
        self.bytes.len() - self.start
    }

    /// Appends `bytes`, moving the unwritten bytes to the front first when
    /// that avoids growing the buffer. False, queueing nothing, when the
    /// buffer cannot grow.
    #[must_use]
    pub fn push(&mut self, bytes: &[u8]) -> bool {
        if self.start != 0 && self.bytes.len() + bytes.len() > self.bytes.capacity() {
            self.bytes.drain(..self.start);
            self.start = 0;
        }
        if self.bytes.try_reserve(bytes.len()).is_err() {
            return false;
        }
        self.bytes.extend_from_slice(bytes);
        true
    }

    /// The unwritten bytes, oldest first.
    pub fn front(&self) -> &[u8] {
        &self.bytes[self.start..]
    }

    /// Marks the first `count` unwritten bytes written.
    pub fn advance(&mut self, count: usize) {
        let count = count.min(self.queued());
        self.start += count;
        self.resolved += count as u64;
        if self.start == self.bytes.len() {
            self.bytes.clear();
            self.start = 0;
        }
    }

    /// Drops every queued byte and fails each line not yet fully written
    /// with `error`.
    pub fn fail(&mut self, error: i32) {
        for line in &mut self.lines {
            if line.end > self.resolved {
                line.error = error;
            }
        }
        self.resolved += self.queued() as u64;
        self.bytes.clear();
        self.start = 0;
    }

    /// Starts waiting on the line that ends with the last byte queued, and
    /// returns its id for `line_state` and `end_line`; None when the record
    /// cannot be allocated.
    pub fn begin_line(&mut self) -> Option<u64> {
        self.lines.try_reserve(1).ok()?;
        let id = self.next_line;
        self.next_line += 1;
        let end = self.resolved + self.queued() as u64;
        self.lines.push(Line { id, end, error: 0 });
        Some(id)
    }

    /// Where line `id` stands; a line no writer waits on reads as written.
    pub fn line_state(&self, id: u64) -> LineState {
        match self.lines.iter().find(|line| line.id == id) {
            Some(line) if line.error != 0 => LineState::Failed(line.error),
            Some(line) if self.resolved < line.end => LineState::Pending,
            _ => LineState::Written,
        }
    }

    /// Stops waiting on line `id`.
    pub fn end_line(&mut self, id: u64) {
        self.lines.retain(|line| line.id != id);
    }
}

/// The line std/log writes, `{"time":..,"level":..,"message":..,"fields":..}`,
/// for a message and fields already encoded as JSON, logged `millis` into
/// second `seconds` since the epoch. A level outside 0 (debug) through 3
/// (error) is clamped. None when the line cannot be allocated.
pub fn log_line(seconds: i64, millis: i64, level: i64, message: &[u8], fields: &[u8]) -> Option<Vec<u8>> {
    const LEVELS: [&[u8]; 4] = [b"debug", b"info", b"warn", b"error"];
    let (date, of_day) = timestamp::civil_seconds(seconds);
    let mut time = timestamp::TimestampText::new();
    timestamp::push_date_time(&mut time, date, of_day);
    let mut line = Vec::new();
    line.try_reserve_exact(64 + message.len() + fields.len()).ok()?;
    line.extend_from_slice(b"{\"time\":\"");
    line.extend_from_slice(time.as_bytes());
    let millis = millis.clamp(0, 999);
    let digit = |value: i64| b'0' + (value % 10) as u8;
    line.extend_from_slice(&[b'.', digit(millis / 100), digit(millis / 10), digit(millis)]);
    line.extend_from_slice(b"Z\",\"level\":\"");
    line.extend_from_slice(LEVELS[level.clamp(0, 3) as usize]);
    line.extend_from_slice(b"\",\"message\":");
    line.extend_from_slice(message);
    line.extend_from_slice(b",\"fields\":");
    line.extend_from_slice(fields);
    line.push(b'}');
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_written_once_the_writes_pass_its_end() {
        let mut queue = ConsoleQueue::new();
        assert!(queue.push(b"first\n"));
        let first = queue.begin_line().unwrap();
        assert!(queue.push(b"second\n"));
        let second = queue.begin_line().unwrap();
        assert_eq!(queue.front(), b"first\nsecond\n");
        queue.advance(5);
        assert_eq!(queue.line_state(first), LineState::Pending);
        queue.advance(1);
        assert_eq!(queue.line_state(first), LineState::Written);
        assert_eq!(queue.line_state(second), LineState::Pending);
        assert_eq!(queue.front(), b"second\n");
        queue.advance(7);
        assert_eq!(queue.line_state(second), LineState::Written);
        assert_eq!(queue.queued(), 0);
    }

    #[test]
    fn a_failure_fails_only_the_lines_not_yet_written() {
        let mut queue = ConsoleQueue::new();
        assert!(queue.push(b"a\n"));
        let written = queue.begin_line().unwrap();
        assert!(queue.push(b"b\n"));
        let torn = queue.begin_line().unwrap();
        assert!(queue.push(b"c\n"));
        let waiting = queue.begin_line().unwrap();
        queue.advance(3);
        queue.fail(32);
        assert_eq!(queue.line_state(written), LineState::Written);
        assert_eq!(queue.line_state(torn), LineState::Failed(32));
        assert_eq!(queue.line_state(waiting), LineState::Failed(32));
        assert_eq!(queue.queued(), 0);
        assert!(queue.push(b"d\n"));
        let later = queue.begin_line().unwrap();
        assert_eq!(queue.line_state(later), LineState::Pending, "a later line is not failed");
        queue.advance(2);
        assert_eq!(queue.line_state(later), LineState::Written);
    }

    #[test]
    fn the_unwritten_bytes_move_to_the_front_instead_of_growing() {
        let mut queue = ConsoleQueue::new();
        assert!(queue.push(&[b'x'; 64]));
        let capacity = queue.bytes.capacity();
        queue.advance(60);
        assert!(queue.push(&vec![b'y'; capacity - 8]));
        assert_eq!(queue.bytes.capacity(), capacity);
        assert_eq!(queue.queued(), capacity - 4);
        assert_eq!(&queue.front()[..4], b"xxxx");
    }

    #[test]
    fn an_ended_line_is_forgotten() {
        let mut queue = ConsoleQueue::new();
        assert!(queue.push(b"a\n"));
        let line = queue.begin_line().unwrap();
        queue.end_line(line);
        queue.fail(5);
        assert_eq!(queue.line_state(line), LineState::Written);
        assert!(queue.lines.is_empty());
    }
    #[test]
    fn a_log_line_is_one_json_object() {
        let line = log_line(1_700_000_000, 7, 1, b"\"hi\"", b"{\"a\":1}").unwrap();
        assert_eq!(
            line,
            b"{\"time\":\"2023-11-14T22:13:20.007Z\",\"level\":\"info\",\"message\":\"hi\",\"fields\":{\"a\":1}}"
        );
    }

    #[test]
    fn a_level_outside_the_four_is_clamped() {
        let level = |raw| {
            let line = log_line(0, 999, raw, b"1", b"2").unwrap();
            String::from_utf8(line).unwrap()
        };
        assert!(level(-5).contains("\"level\":\"debug\""), "{}", level(-5));
        assert!(level(9).contains("\"level\":\"error\""));
        assert!(level(9).starts_with("{\"time\":\"1970-01-01T00:00:00.999Z\""), "{}", level(9));
    }
}
