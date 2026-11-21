//! Wake descriptors (parallel-scheduler.md, "Step 3 plan", PR 5b-5). An idle
//! carrier blocks without the kernel lock, so work another carrier hands it
//! meanwhile must end that wait: each carrier has a non-blocking pipe, and
//! its idle waits also watch the pipe's read end. With one carrier nothing
//! is ever handed over, so there are no pipes and marking costs nothing.
//!
//! The kernel only marks carriers while it is locked; the carrier holding the
//! lock writes the bytes after unlocking (`Locked`), so a woken carrier never
//! blocks on the lock its waker still holds. A byte written while its carrier
//! is busy ends the carrier's next wait at once, which then finds the work.

use std::io;

use super::ready::Carrier;

/// One wake pipe per carrier, and which carriers to wake once the kernel
/// lock is released.
pub(super) struct Wakes {
    pipes: Vec<Pipe>,
    marked: Vec<bool>,
}

impl Wakes {
    /// No pipes for one carrier, otherwise one per carrier.
    pub(super) fn new(carriers: u32) -> io::Result<Self> {
        let pipes = if carriers > 1 {
            (0..carriers).map(|_| Pipe::open()).collect::<io::Result<_>>()?
        } else {
            Vec::new()
        };
        let marked = vec![false; pipes.len()];
        Ok(Self { pipes, marked })
    }

    /// Asks for `carrier` to be woken when the kernel lock is next released.
    pub(super) fn mark(&mut self, carrier: Carrier) {
        if let Some(marked) = self.marked.get_mut(carrier.0 as usize) {
            *marked = true;
        }
    }

    /// The marked carriers' write ends, clearing the marks. A carrier marked
    /// more than once since the last take is written once.
    pub(super) fn take(&mut self) -> Marked {
        let mut ends = Vec::new();
        for (pipe, marked) in self.pipes.iter().zip(&mut self.marked) {
            if std::mem::take(marked) {
                ends.push(pipe.write);
            }
        }
        Marked(ends)
    }

    /// The descriptor `carrier`'s idle waits watch, if it has one.
    pub(super) fn read_fd(&self, carrier: Carrier) -> Option<i32> {
        self.pipes.get(carrier.0 as usize).map(|pipe| pipe.read)
    }

    /// True when some carrier is marked and not yet written.
    pub(super) fn any_marked(&self) -> bool {
        self.marked.contains(&true)
    }

    /// True when `fd` is one of the pipes' ends: no task may wait on it.
    pub(super) fn owns(&self, fd: i32) -> bool {
        self.pipes
            .iter()
            .any(|pipe| pipe.read == fd || pipe.write == fd)
    }
}

/// What an idle wait watches besides its own work: at most the signal pipe
/// and the waiting carrier's wake pipe, held without allocating.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct WakeFds {
    fds: [i32; 2],
    len: usize,
}

impl WakeFds {
    /// Requires: fewer than two descriptors pushed so far.
    pub(super) fn push(&mut self, fd: i32) {
        self.fds[self.len] = fd;
        self.len += 1;
    }

    pub(super) fn as_slice(&self) -> &[i32] {
        &self.fds[..self.len]
    }
}

/// The write ends of the carriers to wake, taken under the kernel lock and
/// written after it is released. A write end stays open while the kernel
/// lives, which outlives every `Locked` access to it.
#[must_use = "a marked carrier sleeps on until its byte is written"]
pub(super) struct Marked(Vec<i32>);

impl Marked {
    pub(super) fn write(self) {
        #[cfg(test)]
        assert!(
            self.0.is_empty() || !super::carrier::kernel_held(),
            "a wake is written only once the kernel lock is released"
        );
        for fd in self.0 {
            sys::write_byte(fd);
        }
    }
}

struct Pipe {
    read: i32,
    write: i32,
}

impl Pipe {
    fn open() -> io::Result<Self> {
        #[cfg(test)]
        if test_hooks::FAIL_OPEN_AFTER.with(|left| match left.get() {
            Some(0) => true,
            Some(n) => {
                left.set(Some(n - 1));
                false
            }
            None => false,
        }) {
            return Err(io::Error::from_raw_os_error(libc::EMFILE));
        }
        let (read, write) = sys::open_pipe()?;
        #[cfg(test)]
        test_hooks::OPENED.with(|opened| opened.borrow_mut().extend([read, write]));
        Ok(Self { read, write })
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        for fd in [self.read, self.write] {
            let _closed = sys::close(fd);
            #[cfg(test)]
            if _closed {
                test_hooks::CLOSED.with(|closed| closed.borrow_mut().push(fd));
            }
        }
    }
}

/// Lets a test make every pipe after `n` more fail, until it resets the
/// count, and see which descriptors this thread's pipes opened and closed
/// (a descriptor counts as closed only when close(2) succeeded).
#[cfg(test)]
pub(super) mod test_hooks {
    use std::cell::{Cell, RefCell};

    thread_local! {
        pub(in crate::scheduler) static FAIL_OPEN_AFTER: Cell<Option<u32>> = const { Cell::new(None) };
        pub(in crate::scheduler) static OPENED: RefCell<Vec<i32>> = const { RefCell::new(Vec::new()) };
        pub(in crate::scheduler) static CLOSED: RefCell<Vec<i32>> = const { RefCell::new(Vec::new()) };
    }
}

#[cfg(all(
    unix,
    any(target_os = "macos", target_os = "linux"),
    any(target_arch = "aarch64", target_arch = "x86_64")
))]
mod sys {
    use std::io;

    /// A pipe whose ends are non-blocking and closed on exec, set atomically
    /// so a fork on another thread cannot inherit it.
    #[cfg(target_os = "linux")]
    pub(super) fn open_pipe() -> io::Result<(i32, i32)> {
        let mut ends = [0_i32; 2];
        // SAFETY: `ends` has room for the two descriptors pipe2(2) writes.
        if unsafe { libc::pipe2(ends.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((ends[0], ends[1]))
    }

    /// A pipe whose ends are non-blocking and closed on exec. macOS has no
    /// pipe2, so a fork on another thread between the calls could inherit
    /// it; the scheduler forks no children of its own.
    #[cfg(target_os = "macos")]
    pub(super) fn open_pipe() -> io::Result<(i32, i32)> {
        let mut ends = [0_i32; 2];
        // SAFETY: `ends` has room for the two descriptors pipe(2) writes.
        if unsafe { libc::pipe(ends.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        for fd in ends {
            // SAFETY: fcntl on a descriptor this function just opened.
            let flagged = unsafe {
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) == 0
                    && libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) == 0
            };
            if !flagged {
                let error = io::Error::last_os_error();
                close(ends[0]);
                close(ends[1]);
                return Err(error);
            }
        }
        Ok((ends[0], ends[1]))
    }

    /// Writes one byte. A full pipe is already readable, so EAGAIN loses no
    /// wake; nothing else can fail on an open pipe whose read end is open.
    pub(super) fn write_byte(fd: i32) {
        let byte = 1_u8;
        // SAFETY: a one-byte buffer that outlives the call.
        let _ = unsafe { libc::write(fd, (&raw const byte).cast(), 1) };
    }

    /// True when the descriptor was open and is now closed.
    pub(super) fn close(fd: i32) -> bool {
        // SAFETY: the pipe owns `fd` and closes it once, when dropped.
        unsafe { libc::close(fd) == 0 }
    }
}

/// Off the poll(2) hosts there is only ever one carrier, so no pipe is
/// opened and nothing is written.
#[cfg(not(all(
    unix,
    any(target_os = "macos", target_os = "linux"),
    any(target_arch = "aarch64", target_arch = "x86_64")
)))]
mod sys {
    use std::io;

    pub(super) fn open_pipe() -> io::Result<(i32, i32)> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "more than one carrier needs pipes",
        ))
    }

    pub(super) fn write_byte(_fd: i32) {}

    pub(super) fn close(_fd: i32) -> bool {
        true
    }
}

#[cfg(all(
    test,
    unix,
    any(target_os = "macos", target_os = "linux"),
    any(target_arch = "aarch64", target_arch = "x86_64")
))]
mod tests {
    use super::*;

    fn readable(fd: i32) -> bool {
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd for the duration of the call.
        unsafe { libc::poll(&mut poll, 1, 0) > 0 }
    }

    /// One carrier gets no pipe, so a mark is a no-op and nothing is written.
    #[test]
    fn one_carrier_has_no_wake_pipe() {
        let mut wakes = Wakes::new(1).expect("no pipe to open");
        assert_eq!(wakes.read_fd(Carrier(0)), None);
        wakes.mark(Carrier(0));
        assert!(wakes.take().0.is_empty());
    }

    /// A mark writes only to its own carrier's pipe, once, and is cleared by
    /// the take that writes it.
    #[test]
    fn a_mark_wakes_only_its_carrier_once() {
        let mut wakes = Wakes::new(3).expect("pipes");
        wakes.mark(Carrier(2));
        wakes.mark(Carrier(2));
        let marked = wakes.take();
        assert_eq!(marked.0.len(), 1, "two marks of one carrier write once");
        assert!(wakes.take().0.is_empty(), "the take cleared the mark");
        let fd = |carrier| wakes.read_fd(Carrier(carrier)).expect("a pipe");
        assert!(!readable(fd(2)), "nothing is written before the take's write");
        marked.write();
        assert!(readable(fd(2)));
        assert!(!readable(fd(0)) && !readable(fd(1)));
        assert!(wakes.owns(fd(1)) && wakes.owns(wakes.pipes[1].write) && !wakes.owns(-1));
    }

    /// A pipe that fails to open fails the whole set, and closes every pipe
    /// already opened for it.
    #[test]
    fn a_failed_pipe_closes_the_ones_opened_before_it() {
        test_hooks::FAIL_OPEN_AFTER.with(|left| left.set(Some(2)));
        let opened_before = test_hooks::OPENED.with(|opened| opened.borrow().len());
        let error = Wakes::new(4).err().expect("the third pipe fails");
        test_hooks::FAIL_OPEN_AFTER.with(|left| left.set(None));
        assert_eq!(error.raw_os_error(), Some(libc::EMFILE));
        let opened: Vec<i32> =
            test_hooks::OPENED.with(|opened| opened.borrow()[opened_before..].to_vec());
        assert_eq!(opened.len(), 4, "two pipes opened before the failure");
        let closed = test_hooks::CLOSED.with(|closed| closed.borrow().clone());
        assert!(opened.iter().all(|fd| closed.contains(fd)), "each was closed");
    }

    /// A full pipe still reads as woken, and a write to it does not block.
    #[test]
    fn a_full_pipe_keeps_its_wake() {
        let mut wakes = Wakes::new(2).expect("pipes");
        for _ in 0..70_000 {
            wakes.mark(Carrier(1));
            wakes.take().write();
        }
        assert!(readable(wakes.read_fd(Carrier(1)).expect("a pipe")));
    }
}
