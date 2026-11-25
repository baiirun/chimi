//! std/exec: child processes. A command is built on a Builder handle one
//! piece at a time, then started with posix_spawn. The program is looked up
//! on the parent's PATH here, before the spawn, so a missing or
//! non-executable program fails at once with its own code, and the path
//! found is made absolute, so a relative one still names the same file after
//! the child changes directory. The child starts with an empty signal mask.
//! Its signal actions are what exec leaves: the runtime's handlers reset to
//! the default, and an action the parent inherited as ignored (SIGHUP under
//! nohup) stays ignored, as a shell would leave it.
//!
//! Waiting goes through `Call::wait_child`, which leaves the child unreaped;
//! this file is the only one that reaps it, by pid, and records its status
//! in the Child handle, so a second wait answers from the record and a
//! signal is never sent to a pid after it was reaped (and possibly reused).
//! Releasing a Child that is still running kills and reaps it.

use super::api::{
    Call, ChimiBindingCall, ChimiBindingResult, ChimiBindingType, Interest, Outcome, Value, Waited,
    prepare_children,
};
use super::os::{errno, error_text};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use core::ffi::{CStr, c_char, c_int};

/// The failure codes the Chimi wrapper maps to `ExecError`.
mod code {
    pub const NOT_FOUND: i64 = 1;
    pub const PERMISSION_DENIED: i64 = 2;
    pub const INVALID_DIRECTORY: i64 = 3;
    pub const CLOSED: i64 = 4;
    pub const FAILED: i64 = 5;
    /// A write to a pipe the child no longer reads.
    pub const BROKEN_PIPE: i64 = 6;
}

/// How a standard stream of the child is connected (std/exec `Stdio`).
#[derive(Clone, Copy, Eq, PartialEq)]
enum Stdio {
    Inherit,
    Null,
    Pipe,
}

impl Stdio {
    fn from_code(code: i64) -> Option<Self> {
        match code {
            0 => Some(Stdio::Inherit),
            1 => Some(Stdio::Null),
            2 => Some(Stdio::Pipe),
            _ => None,
        }
    }
}

/// The bytes one read of a pipe returns at most.
const PIPE_CHUNK: usize = 65536;

unsafe extern "C" {
    static environ: *const *const c_char;
}

#[cfg(target_vendor = "apple")]
unsafe extern "C" {
    fn posix_spawn_file_actions_addinherit_np(
        actions: *mut libc::posix_spawn_file_actions_t,
        fd: c_int,
    ) -> c_int;
    fn posix_spawn_file_actions_addchdir_np(
        actions: *mut libc::posix_spawn_file_actions_t,
        path: *const c_char,
    ) -> c_int;
}

#[cfg(target_vendor = "apple")]
use posix_spawn_file_actions_addchdir_np as add_chdir;
#[cfg(not(target_vendor = "apple"))]
use libc::posix_spawn_file_actions_addchdir_np as add_chdir;

/// Darwin's fcntl command that makes a write to a pipe with no reader fail
/// with EPIPE instead of raising SIGPIPE.
#[cfg(target_vendor = "apple")]
const F_SETNOSIGPIPE: c_int = 73;

/// A NUL-terminated byte string the binding owns.
type Owned = Vec<u8>;

fn owned(text: &CStr) -> Owned {
    text.to_bytes_with_nul().to_vec()
}

/// A command being built.
struct Builder {
    program: Owned,
    /// argv[1..]
    arguments: Vec<Owned>,
    /// "NAME=value"; a later entry of a name wins.
    variables: Vec<Owned>,
    clear_environment: bool,
    /// `None`: the parent's.
    directory: Option<Owned>,
}

/// A started child. Releasing one still running kills and reaps it, so no
/// child outlives its handle and no zombie stays behind. `Exec.start` stops
/// the child first with `native_stop`, which waits parked, so this reap
/// finds it reaped; it blocks only when that could not finish, and at the
/// program's end, for a child whose handle outlived every start (the exit
/// backstop).
struct Child {
    pid: libc::pid_t,
    reaped: Cell<bool>,
    /// waitpid's status, once reaped.
    status: Cell<c_int>,
}

impl Child {
    fn record(&self, status: c_int) {
        self.reaped.set(true);
        self.status.set(status);
    }

    /// Reaps the child if it has exited; true once it is reaped.
    fn reap_if_exited(&self) -> bool {
        if self.reaped.get() {
            return true;
        }
        let mut status = 0;
        let reaped = loop {
            // SAFETY: waitpid on this child's own pid.
            let reaped = unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG) };
            if reaped >= 0 || errno() != libc::EINTR {
                break reaped;
            }
        };
        if reaped == self.pid {
            self.record(status);
        }
        self.reaped.get()
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if !self.reaped.get() {
            let mut status = 0;
            // SAFETY: the child is unreaped, so its pid is still ours.
            unsafe { libc::kill(self.pid, libc::SIGKILL) };
            while unsafe { libc::waitpid(self.pid, &mut status, 0) } < 0 && errno() == libc::EINTR {}
        }
    }
}

/// The parent's end of a pipe to one of the child's standard streams.
struct Pipe {
    fd: c_int,
}

impl Drop for Pipe {
    fn drop(&mut self) {
        // SAFETY: the pipe owns its descriptor.
        unsafe { libc::close(self.fd) };
    }
}

static BUILDER: ChimiBindingType = ChimiBindingType::boxed::<RefCell<Builder>>(c"Command");
static CHILD: ChimiBindingType = ChimiBindingType::boxed::<Child>(c"Child");
static PIPE: ChimiBindingType = ChimiBindingType::boxed::<Pipe>(c"Pipe");

/// A descriptor this call owns until it is handed on.
struct Descriptor(c_int);

impl Descriptor {
    fn into_raw(self) -> c_int {
        let fd = self.0;
        core::mem::forget(self);
        fd
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: the descriptor is owned and closed once.
        unsafe { libc::close(self.0) };
    }
}

fn os_failure(call: Call<'_>, code: i64, error: c_int) -> Outcome<'_> {
    // SAFETY: strerror's text is NUL-terminated and read at once.
    unsafe { call.failure_raw(code, error_text(error)) }
}

/// Text without U+0000: C sees text only up to its first NUL, so a program,
/// argument or variable holding one would silently be cut short.
fn text_argument<'call>(call: &Call<'call>, index: u32) -> Option<&'call CStr> {
    let text = call.text(index)?;
    // SAFETY: the runtime ends a text argument with a NUL.
    let c = unsafe { CStr::from_ptr(text.as_ptr()) };
    (c.to_bytes().len() == text.as_bytes().len()).then_some(c)
}

fn void_success(mut call: Call<'_>) -> Outcome<'_> {
    let void = call.void();
    call.success(void)
}

/// The builder argument at 0, borrowed for one change.
fn with_builder<'call>(
    call: Call<'call>,
    change: impl FnOnce(&Call<'call>, &mut Builder) -> Result<(), &'static CStr>,
) -> Outcome<'call> {
    // SAFETY: BUILDER is a boxed `RefCell<Builder>` type.
    let changed = match unsafe { call.handle::<RefCell<Builder>>(0, &BUILDER) } {
        None => Err(c"Exec: the command was already started"),
        Some(builder) => match builder.try_borrow_mut() {
            Ok(mut builder) => change(&call, &mut builder),
            Err(_) => Err(c"Exec: the command is in use"),
        },
    };
    match changed {
        Ok(()) => void_success(call),
        Err(message) => call.defect(message),
    }
}

macro_rules! binding {
    ($(#[$doc:meta])* $name:ident => $body:expr) => {
        $(#[$doc])*
        ///
        /// # Safety
        ///
        /// Only the runtime calls it, with the live binding call.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(raw: *mut ChimiBindingCall) -> *mut ChimiBindingResult {
            // SAFETY: the runtime passes the call it is running.
            let call = unsafe { Call::new(raw) };
            let body: fn(Call<'_>) -> Outcome<'_> = $body;
            body(call).into_raw()
        }
    };
}

binding!(
    /// `native_command(program) -> Command`
    chimi_exec_command => |mut call| {
        let Some(program) = text_argument(&call, 0) else {
            return call.defect(c"Exec: a program name holds U+0000");
        };
        let builder = Box::new(RefCell::new(Builder {
            program: owned(program),
            arguments: Vec::new(),
            variables: Vec::new(),
            clear_environment: false,
            directory: None,
        }));
        // SAFETY: BUILDER is a boxed `RefCell<Builder>` type.
        match unsafe { call.make_handle(&BUILDER, builder) } {
            Some(handle) => call.success(handle),
            None => call.defect(c"Exec: too many commands at once"),
        }
    }
);

binding!(
    /// `native_argument(command, argument) -> Void`
    chimi_exec_argument => |call| with_builder(call, |call, builder| {
        let argument = text_argument(call, 1).ok_or(c"Exec: an argument holds U+0000")?;
        builder.arguments.push(owned(argument));
        Ok(())
    })
);

binding!(
    /// `native_variable(command, name, value) -> Void`
    chimi_exec_variable => |call| with_builder(call, |call, builder| {
        let (Some(name), Some(value)) = (text_argument(call, 1), text_argument(call, 2)) else {
            return Err(c"Exec: an environment variable holds U+0000");
        };
        let name = name.to_bytes();
        if name.is_empty() || name.contains(&b'=') {
            return Err(c"Exec: an environment variable name is empty or holds '='");
        }
        let mut entry = Vec::with_capacity(name.len() + value.to_bytes().len() + 2);
        entry.extend_from_slice(name);
        entry.push(b'=');
        entry.extend_from_slice(value.to_bytes_with_nul());
        builder.variables.push(entry);
        Ok(())
    })
);

binding!(
    /// `native_clear_environment(command) -> Void`
    chimi_exec_clear_environment => |call| with_builder(call, |_, builder| {
        builder.clear_environment = true;
        Ok(())
    })
);

binding!(
    /// `native_directory(command, directory) -> Void`
    chimi_exec_directory => |call| with_builder(call, |call, builder| {
        let directory = text_argument(call, 1).ok_or(c"Exec: a directory holds U+0000")?;
        builder.directory = Some(owned(directory));
        Ok(())
    })
);

// ---- Starting ---------------------------------------------------------

/// Whether `path` is an executable regular file; `Err` holds the errno.
fn executable(path: &CStr) -> Result<(), c_int> {
    // SAFETY: an all-zero stat is valid to write into.
    let mut info: libc::stat = unsafe { core::mem::zeroed() };
    // SAFETY: `path` is NUL-terminated and `info` writable.
    if unsafe { libc::stat(path.as_ptr(), &mut info) } != 0 {
        return Err(errno());
    }
    // SAFETY: `path` is NUL-terminated.
    if info.st_mode & libc::S_IFMT == libc::S_IFDIR
        || unsafe { libc::access(path.as_ptr(), libc::X_OK) } != 0
    {
        return Err(libc::EACCES);
    }
    Ok(())
}

/// `path` made absolute against the parent's working directory. `Err` holds
/// the errno when the directory cannot be named.
fn absolute(path: &[u8]) -> Result<Owned, c_int> {
    if path.first() == Some(&b'/') {
        return Ok(path.to_vec());
    }
    let mut directory: Vec<u8> = Vec::with_capacity(256);
    loop {
        // SAFETY: the buffer is writable for its capacity.
        let found = unsafe {
            libc::getcwd(directory.as_mut_ptr().cast(), directory.capacity())
        };
        if !found.is_null() {
            // SAFETY: getcwd wrote a NUL-terminated path into the buffer.
            let length = unsafe { CStr::from_ptr(found) }.to_bytes().len();
            // SAFETY: getcwd initialised `length` bytes.
            unsafe { directory.set_len(length) };
            break;
        }
        let error = errno();
        if error != libc::ERANGE {
            return Err(error);
        }
        // `reserve` counts from the length, which is zero here: ask for
        // twice the capacity, or getcwd sees the same size again.
        let wanted = directory.capacity().saturating_mul(2);
        if directory.try_reserve_exact(wanted).is_err() {
            return Err(libc::ENOMEM);
        }
    }
    directory.push(b'/');
    directory.extend_from_slice(path);
    Ok(directory)
}

/// Why no path to spawn was found.
struct NotResolved {
    code: i64,
    error: c_int,
}

/// The path to spawn for `program`, looked up on the parent's PATH when it
/// names no directory, as a shell does. A program found only without
/// execute permission is PermissionDenied.
fn resolve(program: &[u8]) -> Result<Owned, NotResolved> {
    let found = |candidate: &[u8]| {
        absolute(candidate).map_err(|error| NotResolved { code: code::FAILED, error })
    };
    // `program` ends with its NUL.
    let name = program.strip_suffix(b"\0").unwrap_or(program);
    if name.is_empty() {
        return Err(NotResolved { code: code::NOT_FOUND, error: libc::ENOENT });
    }
    if name.contains(&b'/') {
        let path = CStr::from_bytes_with_nul(program)
            .map_err(|_| NotResolved { code: code::NOT_FOUND, error: libc::ENOENT })?;
        return match executable(path) {
            Ok(()) => found(program),
            Err(libc::EACCES) => Err(NotResolved { code: code::PERMISSION_DENIED, error: libc::EACCES }),
            Err(_) => Err(NotResolved { code: code::NOT_FOUND, error: libc::ENOENT }),
        };
    }
    // SAFETY: getenv returns null or a NUL-terminated string.
    let path = unsafe { libc::getenv(c"PATH".as_ptr()).as_ref() }
        // SAFETY: the pointer is non-null and NUL-terminated.
        .map(|path| unsafe { CStr::from_ptr(path) }.to_bytes())
        .filter(|path| !path.is_empty())
        .unwrap_or(b"/usr/bin:/bin");
    let mut denied = false;
    let mut candidate = Vec::new();
    for directory in path.split(|byte| *byte == b':') {
        // An empty entry is the current directory.
        let directory = if directory.is_empty() { b"." } else { directory };
        candidate.clear();
        candidate.extend_from_slice(directory);
        candidate.push(b'/');
        candidate.extend_from_slice(program);
        let Ok(path) = CStr::from_bytes_with_nul(&candidate) else {
            continue;
        };
        match executable(path) {
            Ok(()) => return found(path.to_bytes()).map(|mut absolute| {
                absolute.push(0);
                absolute
            }),
            Err(error) => denied = denied || error == libc::EACCES,
        }
    }
    Err(if denied {
        NotResolved { code: code::PERMISSION_DENIED, error: libc::EACCES }
    } else {
        NotResolved { code: code::NOT_FOUND, error: libc::ENOENT }
    })
}

/// The name of an environment entry: the bytes before its first '='.
fn entry_name(entry: &[u8]) -> &[u8] {
    entry.split(|byte| *byte == b'=').next().unwrap_or(entry)
}

/// The child's environment: the parent's (unless cleared) with the command's
/// variables set over it, a later variable of a name winning. The strings
/// are the builder's or the parent's; only the array is new, null-ended.
fn child_environment(builder: &Builder) -> Vec<*const c_char> {
    let mut entries = Vec::new();
    if !builder.clear_environment {
        // SAFETY: `environ` is the null-ended array of NUL-terminated
        // entries the C library keeps; nothing changes it during the call.
        let mut cursor = unsafe { environ };
        while let Some(entry) = unsafe { cursor.as_ref() }.copied().filter(|entry| !entry.is_null()) {
            // SAFETY: each entry is NUL-terminated.
            let name = entry_name(unsafe { CStr::from_ptr(entry) }.to_bytes());
            let replaced = builder.variables.iter().any(|set| entry_name(set) == name);
            if !replaced {
                entries.push(entry);
            }
            // SAFETY: the array continues until its null entry.
            cursor = unsafe { cursor.add(1) };
        }
    }
    for (index, set) in builder.variables.iter().enumerate() {
        let name = entry_name(set);
        let later = builder
            .variables
            .iter()
            .skip(index + 1)
            .any(|other| entry_name(other) == name);
        if !later {
            entries.push(set.as_ptr().cast());
        }
    }
    entries.push(core::ptr::null());
    entries
}

/// `fd` moved above the standard streams and closed in every child.
fn private_descriptor(fd: c_int) -> Result<Descriptor, c_int> {
    let fd = Descriptor(fd);
    // SAFETY: fcntl on a descriptor this call owns.
    let moved = unsafe { libc::fcntl(fd.0, libc::F_DUPFD_CLOEXEC, libc::STDERR_FILENO + 1) };
    if moved < 0 {
        return Err(errno());
    }
    Ok(Descriptor(moved))
}

/// The two ends of a pipe for one of the child's streams.
struct PipeEnds {
    parent: Descriptor,
    child: Descriptor,
}

/// A pipe for the child's stream `target`: both ends closed on exec, so no
/// other child ever holds them. The parent's end does not block, and a
/// write to it after the child stopped reading fails with EPIPE where the
/// platform allows (F_SETNOSIGPIPE) instead of raising SIGPIPE. `Err` holds
/// the errno.
fn open_pipe(target: c_int) -> Result<PipeEnds, c_int> {
    let mut raw = [-1; 2];
    // SAFETY: `raw` has room for the two descriptors.
    #[cfg(target_os = "linux")]
    let made = unsafe { libc::pipe2(raw.as_mut_ptr(), libc::O_CLOEXEC) };
    // SAFETY: `raw` has room for the two descriptors.
    #[cfg(not(target_os = "linux"))]
    let made = unsafe { libc::pipe(raw.as_mut_ptr()) };
    if made != 0 {
        return Err(errno());
    }
    let [read_end, write_end] = raw;
    let read_end = match private_descriptor(read_end) {
        Ok(moved) => moved,
        Err(error) => {
            drop(Descriptor(write_end));
            return Err(error);
        }
    };
    let write_end = private_descriptor(write_end)?;
    let ends = if target == libc::STDIN_FILENO {
        PipeEnds { parent: write_end, child: read_end }
    } else {
        PipeEnds { parent: read_end, child: write_end }
    };
    // SAFETY: fcntl on a descriptor this call owns.
    let flags = unsafe { libc::fcntl(ends.parent.0, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(ends.parent.0, libc::F_SETFL, flags | libc::O_NONBLOCK) } != 0 {
        return Err(errno());
    }
    // write_quietly relies on it: without it a write would raise SIGPIPE.
    #[cfg(target_vendor = "apple")]
    if target == libc::STDIN_FILENO && unsafe { libc::fcntl(ends.parent.0, F_SETNOSIGPIPE, 1) } != 0 {
        return Err(errno());
    }
    Ok(ends)
}

/// A child holds its three standard streams and no other descriptor of the
/// program's, even one another carrier thread has just opened and not yet
/// marked close-on-exec, where that takes a second call (sockets and pipes
/// off Linux). macOS closes every descriptor the file actions do not name
/// (POSIX_SPAWN_CLOEXEC_DEFAULT), so an inherited stream is named; glibc
/// closes those above standard error as the last action (addclosefrom_np,
/// glibc 2.34, which a glibc build therefore needs: the C checked the
/// version and went without it before 2.34). Elsewhere (musl)
/// only close-on-exec protects them.
#[cfg(target_vendor = "apple")]
const SPAWN_FLAGS: c_int = libc::POSIX_SPAWN_SETSIGMASK | libc::POSIX_SPAWN_CLOEXEC_DEFAULT;
#[cfg(not(target_vendor = "apple"))]
const SPAWN_FLAGS: c_int = libc::POSIX_SPAWN_SETSIGMASK;

fn close_other_descriptors(actions: &mut libc::posix_spawn_file_actions_t) -> c_int {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: `actions` is initialised.
        unsafe { libc::posix_spawn_file_actions_addclosefrom_np(actions, libc::STDERR_FILENO + 1) }
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    {
        let _ = actions;
        0
    }
}

/// Adds the file action that connects the child's stream `fd`; false when
/// it cannot.
fn connect_stdio(
    actions: &mut libc::posix_spawn_file_actions_t,
    fd: c_int,
    mode: Stdio,
    child_end: Option<&Descriptor>,
) -> bool {
    match mode {
        Stdio::Inherit => {
            // A stream the parent has closed stays closed, as without the
            // flag; naming it would fail the spawn.
            #[cfg(target_vendor = "apple")]
            {
                // SAFETY: fcntl only asks; `actions` is initialised.
                unsafe {
                    libc::fcntl(fd, libc::F_GETFD) == -1
                        || posix_spawn_file_actions_addinherit_np(actions, fd) == 0
                }
            }
            #[cfg(not(target_vendor = "apple"))]
            {
                let _ = (actions, fd);
                true
            }
        }
        Stdio::Null => {
            let flags = if fd == libc::STDIN_FILENO { libc::O_RDONLY } else { libc::O_WRONLY };
            // SAFETY: `actions` is initialised and the path NUL-terminated.
            unsafe { libc::posix_spawn_file_actions_addopen(actions, fd, c"/dev/null".as_ptr(), flags, 0) == 0 }
        }
        Stdio::Pipe => child_end.is_some_and(|end| {
            // SAFETY: `actions` is initialised.
            unsafe { libc::posix_spawn_file_actions_adddup2(actions, end.0, fd) == 0 }
        }),
    }
}

/// A failure of posix_spawn itself. The program and directory were checked
/// before it, so ENOENT and EACCES here mean they changed in between, or the
/// program's interpreter (a script's #! line) is missing or not executable;
/// ENOEXEC is a file the kernel cannot run, which is not a permission. The
/// message is the operating system's words alone: `ExecError` names the
/// program.
fn spawn_failure(call: Call<'_>, error: c_int) -> Outcome<'_> {
    let code = match error {
        libc::ENOENT => code::NOT_FOUND,
        libc::EACCES => code::PERMISSION_DENIED,
        _ => code::FAILED,
    };
    os_failure(call, code, error)
}

/// The spawned child and its pipes as handles. If one cannot be made (the
/// handle table is full), the child is killed and reaped at once; a pipe
/// handle made before that is closed when its value is freed.
fn started(mut call: Call<'_>, child: Box<Child>, ends: [Option<Descriptor>; 3]) -> Outcome<'_> {
    let mut pipes = [call.none(), call.none(), call.none()];
    let mut made = true;
    for (slot, end) in pipes.iter_mut().zip(ends) {
        let Some(end) = end else { continue };
        if !made {
            continue;
        }
        let pipe = Box::new(Pipe { fd: end.into_raw() });
        // SAFETY: PIPE is a boxed `Pipe` type.
        match unsafe { call.make_handle(&PIPE, pipe) } {
            Some(handle) => *slot = call.some(handle),
            None => made = false,
        }
    }
    if !made {
        drop(child);
        return os_failure(call, code::FAILED, libc::EMFILE);
    }
    // With no handle made, the runtime has already released the child,
    // which killed and reaped it.
    // SAFETY: CHILD is a boxed `Child` type.
    let Some(child) = (unsafe { call.make_handle(&CHILD, child) }) else {
        return os_failure(call, code::FAILED, libc::EMFILE);
    };
    let [stdin, stdout, stderr] = pipes;
    let parts: [Value<'_>; 4] = [child, stdin, stdout, stderr];
    let tuple = call.make_tuple(&parts);
    call.ok(tuple)
}

/// posix_spawn's file actions and attributes, destroyed when dropped.
struct Spawn {
    actions: libc::posix_spawn_file_actions_t,
    attributes: libc::posix_spawnattr_t,
}

impl Spawn {
    fn new() -> Option<Box<Self>> {
        // SAFETY: both records are initialised by their init calls before
        // any other use; zeroed is only their storage.
        let mut spawn = Box::new(unsafe { core::mem::zeroed::<Spawn>() });
        // SAFETY: init on owned storage.
        if unsafe { libc::posix_spawn_file_actions_init(&mut spawn.actions) } != 0 {
            return None;
        }
        // SAFETY: init on owned storage.
        if unsafe { libc::posix_spawnattr_init(&mut spawn.attributes) } != 0 {
            // SAFETY: the actions were initialised above.
            unsafe { libc::posix_spawn_file_actions_destroy(&mut spawn.actions) };
            return None;
        }
        Some(spawn)
    }
}

impl Drop for Spawn {
    fn drop(&mut self) {
        // SAFETY: both were initialised in `new`.
        unsafe {
            libc::posix_spawn_file_actions_destroy(&mut self.actions);
            libc::posix_spawnattr_destroy(&mut self.attributes);
        }
    }
}

binding!(
    /// `native_spawn(builder, stdin, stdout, stderr) -> Result<(Process,
    /// Option<Pipe>, Option<Pipe>, Option<Pipe>), (Int, Text)>`: starts the
    /// command and closes the builder, with a pipe for each stream given
    /// Stdio.Pipe.
    chimi_exec_spawn => |mut call| {
        let outcome = spawn(&mut call);
        call.close_handle(0, &BUILDER);
        outcome.finish(call)
    }
);

/// What `spawn` decided, made into a result once the builder is closed.
enum Spawned {
    Defect(&'static CStr),
    Failure(i64, c_int),
    InvalidDirectory,
    SpawnFailed(c_int),
    Started(Box<Child>, [Option<Descriptor>; 3]),
}

impl Spawned {
    fn finish(self, call: Call<'_>) -> Outcome<'_> {
        match self {
            Spawned::Defect(message) => call.defect(message),
            Spawned::Failure(code, error) => os_failure(call, code, error),
            Spawned::InvalidDirectory => {
                call.failure(code::INVALID_DIRECTORY, c"not a directory it can enter")
            }
            Spawned::SpawnFailed(error) => spawn_failure(call, error),
            Spawned::Started(child, ends) => started(call, child, ends),
        }
    }
}

/// Whether `directory` is one the child can enter: a directory with search
/// permission, so a failure is not found only inside the child.
fn enterable(directory: &CStr) -> bool {
    // SAFETY: an all-zero stat is valid to write into.
    let mut info: libc::stat = unsafe { core::mem::zeroed() };
    // SAFETY: `directory` is NUL-terminated and `info` writable.
    unsafe {
        libc::stat(directory.as_ptr(), &mut info) == 0
            && info.st_mode & libc::S_IFMT == libc::S_IFDIR
            && libc::access(directory.as_ptr(), libc::X_OK) == 0
    }
}

fn spawn(call: &mut Call<'_>) -> Spawned {
    let mut modes = [Stdio::Inherit; 3];
    for (index, mode) in (1..).zip(modes.iter_mut()) {
        match call.integer(index).and_then(Stdio::from_code) {
            Some(code) => *mode = code,
            None => return Spawned::Defect(c"Exec: an unknown Stdio"),
        }
    }
    // SAFETY: BUILDER is a boxed `RefCell<Builder>` type.
    let Some(builder) = (unsafe { call.handle::<RefCell<Builder>>(0, &BUILDER) }) else {
        return Spawned::Defect(c"Exec: the command was already started");
    };
    let Ok(builder) = builder.try_borrow() else {
        return Spawned::Defect(c"Exec: the command is in use");
    };
    if let Some(directory) = &builder.directory
        && !CStr::from_bytes_with_nul(directory).is_ok_and(enterable)
    {
        return Spawned::InvalidDirectory;
    }
    let path = match resolve(&builder.program) {
        Ok(path) => path,
        Err(NotResolved { code, error }) => return Spawned::Failure(code, error),
    };
    let mut argv: Vec<*const c_char> = Vec::with_capacity(builder.arguments.len() + 2);
    argv.push(builder.program.as_ptr().cast());
    argv.extend(builder.arguments.iter().map(|argument| argument.as_ptr().cast::<c_char>()));
    argv.push(core::ptr::null());
    let envp = child_environment(&builder);
    let Some(mut spawn) = Spawn::new() else {
        return Spawned::SpawnFailed(libc::ENOMEM);
    };
    let mut ends: [Option<PipeEnds>; 3] = [None, None, None];
    for (fd, (mode, end)) in (0..).zip(modes.iter().zip(ends.iter_mut())) {
        if *mode == Stdio::Pipe {
            match open_pipe(fd) {
                Ok(pipe) => *end = Some(pipe),
                Err(error) => return Spawned::SpawnFailed(error),
            }
        }
    }
    // SAFETY: an all-zero sigset is valid storage for sigemptyset.
    let mut empty: libc::sigset_t = unsafe { core::mem::zeroed() };
    // SAFETY: the attributes are initialised; `empty` is a valid set.
    let mut error = unsafe {
        libc::sigemptyset(&mut empty);
        match libc::posix_spawnattr_setsigmask(&mut spawn.attributes, &empty) {
            0 => libc::posix_spawnattr_setflags(&mut spawn.attributes, SPAWN_FLAGS as _),
            error => error,
        }
    };
    for (fd, (mode, end)) in (0..).zip(modes.iter().zip(ends.iter())) {
        if error == 0 && !connect_stdio(&mut spawn.actions, fd, *mode, end.as_ref().map(|end| &end.child)) {
            error = libc::EINVAL;
        }
    }
    if error == 0
        && let Some(directory) = &builder.directory
    {
        // SAFETY: the actions are initialised and the directory NUL-terminated.
        error = unsafe { add_chdir(&mut spawn.actions, directory.as_ptr().cast()) };
    }
    if error == 0 {
        error = close_other_descriptors(&mut spawn.actions);
    }
    let mut pid: libc::pid_t = 0;
    if error == 0 {
        prepare_children();
        // SAFETY: every pointer is NUL-terminated or a null-ended array that
        // lives until the call returns.
        error = unsafe {
            libc::posix_spawn(
                &mut pid,
                path.as_ptr().cast(),
                &spawn.actions,
                &spawn.attributes,
                argv.as_ptr().cast(),
                envp.as_ptr().cast(),
            )
        };
    }
    // The child holds its ends now; the parent never uses them. A failed
    // spawn closes the parent's ends too.
    let parents = ends.map(|end| end.map(|PipeEnds { parent, child }| {
        drop(child);
        parent
    }));
    if error != 0 {
        return Spawned::SpawnFailed(error);
    }
    Spawned::Started(
        Box::new(Child { pid, reaped: Cell::new(false), status: Cell::new(0) }),
        parents,
    )
}

// ---- Waiting ----------------------------------------------------------

/// (0, code) for an exit, (1, signal) for a death by signal.
fn status_result(mut call: Call<'_>, status: c_int) -> Outcome<'_> {
    let (kind, value) = if libc::WIFSIGNALED(status) {
        (1, libc::WTERMSIG(status))
    } else {
        (0, libc::WEXITSTATUS(status))
    };
    let parts = [call.make_integer(kind), call.make_integer(i64::from(value))];
    let tuple = call.make_tuple(&parts);
    call.ok(tuple)
}

fn child_closed(call: Call<'_>) -> Outcome<'_> {
    call.failure(code::CLOSED, c"the child was released")
}

binding!(
    /// `native_wait(child)`: its status, waiting for the exit first.
    chimi_exec_wait => |mut call| loop {
        // SAFETY: CHILD is a boxed `Child` type.
        let Some(child) = (unsafe { call.handle::<Child>(0, &CHILD) }) else {
            return child_closed(call);
        };
        if child.reaped.get() {
            let status = child.status.get();
            return status_result(call, status);
        }
        let pid = child.pid;
        let mut status = 0;
        // SAFETY: waitpid on this child's own, unreaped pid.
        let reaped = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        if reaped == pid {
            child.record(status);
            return status_result(call, status);
        }
        if reaped < 0 {
            let error = errno();
            if error != libc::EINTR {
                return os_failure(call, code::FAILED, error);
            }
        }
        match call.wait_child(Some(0), pid) {
            Waited::Cancelled => return call.cancelled(),
            Waited::Closed => return child_closed(call),
            // Ready, or failed because another task reaped it meanwhile:
            // the record answers on the next pass.
            Waited::Ready | Waited::Failed => {}
        }
    }
);

fn signal(call: Call<'_>, signal: c_int) -> Outcome<'_> {
    // SAFETY: CHILD is a boxed `Child` type.
    if let Some(child) = unsafe { call.handle::<Child>(0, &CHILD) }
        && !child.reaped.get()
    {
        // SAFETY: the child is unreaped, so its pid is still ours.
        unsafe { libc::kill(child.pid, signal) };
    }
    void_success(call)
}

binding!(
    /// `native_terminate(child) -> Void`
    chimi_exec_terminate => |call| signal(call, libc::SIGTERM)
);

binding!(
    /// `native_kill(child) -> Void`
    chimi_exec_kill => |call| signal(call, libc::SIGKILL)
);

// ---- Stopping ---------------------------------------------------------

/// How often a stopping child is looked at. The scheduler wakes a child
/// waiter only at the exit, so the deadline needs a clock of its own: a
/// descriptor that turns readable every tick (timerfd, or a kqueue timer).
/// It runs in real time, as the child does, whatever the program's clock.
const STOP_TICK_MS: i64 = 10;

/// Past this many milliseconds the grace is as good as forever, and adding
/// it to the clock cannot overflow.
const STOP_GRACE_LIMIT_MS: i64 = 1 << 52;

#[cfg(target_os = "linux")]
fn open_ticker() -> Option<Descriptor> {
    // SAFETY: timerfd_create takes plain flags.
    let fd = unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_NONBLOCK | libc::TFD_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    let fd = Descriptor(fd);
    let tick = libc::timespec { tv_sec: 0, tv_nsec: (STOP_TICK_MS * 1_000_000) as _ };
    let every = libc::itimerspec { it_interval: tick, it_value: tick };
    // SAFETY: `every` is a valid timer setting.
    (unsafe { libc::timerfd_settime(fd.0, 0, &every, core::ptr::null_mut()) } == 0).then_some(fd)
}

#[cfg(target_vendor = "apple")]
fn open_ticker() -> Option<Descriptor> {
    // SAFETY: kqueue takes no arguments.
    let fd = unsafe { libc::kqueue() };
    if fd < 0 {
        return None;
    }
    let fd = Descriptor(fd);
    // SAFETY: fcntl on a descriptor this call owns.
    unsafe { libc::fcntl(fd.0, libc::F_SETFD, libc::FD_CLOEXEC) };
    // SAFETY: an all-zero kevent is valid before its fields are set.
    let mut timer: libc::kevent = unsafe { core::mem::zeroed() };
    timer.ident = 1;
    timer.filter = libc::EVFILT_TIMER;
    timer.flags = libc::EV_ADD | libc::EV_ENABLE;
    timer.data = STOP_TICK_MS as _;
    // SAFETY: one valid change, no events asked back.
    (unsafe { libc::kevent(fd.0, &timer, 1, core::ptr::null_mut(), 0, core::ptr::null()) } == 0)
        .then_some(fd)
}

#[cfg(target_os = "linux")]
fn drain_ticker(fd: c_int) {
    let mut ticks = 0u64;
    // SAFETY: a timerfd read writes one u64. A failed read leaves the next
    // tick to wake the wait again.
    unsafe { libc::read(fd, (&raw mut ticks).cast(), size_of::<u64>()) };
}

#[cfg(target_vendor = "apple")]
fn drain_ticker(fd: c_int) {
    // SAFETY: an all-zero kevent is valid storage to receive into.
    let mut fired: libc::kevent = unsafe { core::mem::zeroed() };
    let now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: room for one event, without waiting.
    unsafe { libc::kevent(fd, core::ptr::null(), 0, &mut fired, 1, &now) };
}

// `time_t` and the nanosecond field are not `i64` on every target.
#[allow(clippy::useless_conversion)]
fn monotonic_ms() -> i64 {
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `now` is writable.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    i64::from(now.tv_sec) * 1000 + i64::from(now.tv_nsec) / 1_000_000
}

/// How a stop's wait ended.
#[derive(Clone, Copy, Eq, PartialEq)]
enum StopWait {
    Exited,
    Deadline,
    Cancelled,
}

/// Whether the child argument is gone or reaped (reaping it if it exited).
fn child_done(call: &Call<'_>) -> bool {
    // SAFETY: CHILD is a boxed `Child` type.
    unsafe { call.handle::<Child>(0, &CHILD) }.is_none_or(Child::reap_if_exited)
}

/// Waits, letting other tasks run, until the child is reaped or `deadline`
/// (monotonic ms) passes.
fn await_exit_until(call: &mut Call<'_>, ticker: c_int, deadline: i64) -> StopWait {
    loop {
        if child_done(call) {
            return StopWait::Exited;
        }
        if monotonic_ms() >= deadline {
            return StopWait::Deadline;
        }
        if call.wait(None, ticker, Interest::Readable) != Waited::Ready {
            return StopWait::Cancelled;
        }
        drain_ticker(ticker);
    }
}

/// Waits, letting other tasks run, until the child is reaped. It wakes at
/// the exit, so no ticker is needed. A wait that does not finish leaves the
/// reap to the release that follows.
fn await_exit(call: &mut Call<'_>) {
    loop {
        // SAFETY: CHILD is a boxed `Child` type.
        let pid = match unsafe { call.handle::<Child>(0, &CHILD) } {
            Some(child) if !child.reap_if_exited() => child.pid,
            _ => return,
        };
        if call.wait_child(Some(0), pid) != Waited::Ready {
            return;
        }
    }
}

binding!(
    /// `native_stop(child, milliseconds) -> Void`: a child still running
    /// gets SIGTERM and `milliseconds` to exit, then SIGKILL, and is reaped;
    /// zero sends SIGKILL at once, and so does any grace when no ticker can
    /// be opened (a SIGTERM cut short at once is worse than none). Other
    /// tasks run while it waits. A cancelled wait (a task ended outright,
    /// which no release masks, such as a process body forced at its shutdown
    /// deadline) must not be followed by another, which would park a task
    /// that is unwinding: the child gets SIGKILL, the stop answers
    /// cancelled, and the handle's release at the program's end reaps it.
    chimi_exec_stop => |mut call| {
        let Some(grace) = call.integer(1).filter(|grace| *grace >= 0) else {
            return call.defect(c"Exec: a negative stop_within");
        };
        let grace = grace.min(STOP_GRACE_LIMIT_MS);
        // SAFETY: CHILD is a boxed `Child` type.
        let pid = match unsafe { call.handle::<Child>(0, &CHILD) } {
            Some(child) if !child.reap_if_exited() => child.pid,
            _ => return void_success(call),
        };
        let mut waited = StopWait::Deadline;
        if let Some(ticker) = (grace > 0).then(open_ticker).flatten() {
            // SAFETY: the child is unreaped, so its pid is still ours.
            unsafe { libc::kill(pid, libc::SIGTERM) };
            waited = await_exit_until(&mut call, ticker.0, monotonic_ms() + grace);
        }
        // SAFETY: CHILD is a boxed `Child` type.
        let running = unsafe { call.handle::<Child>(0, &CHILD) }
            .filter(|child| !child.reaped.get())
            .map(|child| child.pid);
        if waited != StopWait::Exited
            && let Some(pid) = running
        {
            // SAFETY: the child is unreaped, so its pid is still ours.
            unsafe { libc::kill(pid, libc::SIGKILL) };
            if waited == StopWait::Cancelled {
                return call.cancelled();
            }
            await_exit(&mut call);
        }
        void_success(call)
    }
);

binding!(
    /// `native_release(child)`: kills a running child, reaps it, frees the
    /// handle.
    chimi_exec_release => |mut call| {
        call.close_handle(0, &CHILD);
        void_success(call)
    }
);

binding!(
    /// `native_allow_real_commands() -> Void`: test-only (T4401), so a
    /// program cannot reach `Exec.host_for_tests`. Does nothing.
    chimi_exec_allow_real_commands => void_success
);

// ---- Pipes ------------------------------------------------------------

fn pipe_closed(call: Call<'_>) -> Outcome<'_> {
    call.failure(code::CLOSED, c"the stream was closed")
}

fn pipe_not_ready(call: Call<'_>, waited: Waited) -> Outcome<'_> {
    match waited {
        Waited::Cancelled => call.cancelled(),
        Waited::Closed => pipe_closed(call),
        Waited::Ready | Waited::Failed => os_failure(call, code::FAILED, libc::EBADF),
    }
}

binding!(
    /// `native_read(pipe) -> Result<Option<Bytes>, (Int, Text)>`: the next
    /// bytes the child wrote, or `None` at the end, once every writer has
    /// closed.
    chimi_exec_read => |mut call| {
        // On the heap: a task's stack is too small to hold a whole chunk.
        let mut buffer: Vec<u8> = Vec::new();
        if buffer.try_reserve_exact(PIPE_CHUNK).is_err() {
            return os_failure(call, code::FAILED, libc::ENOMEM);
        }
        loop {
            // SAFETY: PIPE is a boxed `Pipe` type.
            let Some(fd) = (unsafe { call.handle::<Pipe>(0, &PIPE) }).map(|pipe| pipe.fd) else {
                return pipe_closed(call);
            };
            // SAFETY: the buffer has PIPE_CHUNK bytes of capacity.
            let count = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), PIPE_CHUNK) };
            if let Ok(length @ 1..) = usize::try_from(count) {
                // SAFETY: read wrote `length` bytes, at most PIPE_CHUNK.
                unsafe { buffer.set_len(length) };
                let bytes = call.make_bytes(&buffer);
                let some = call.some(bytes);
                return call.ok(some);
            }
            if count == 0 {
                let none = call.none();
                return call.ok(none);
            }
            let error = errno();
            if error == libc::EINTR {
                continue;
            }
            if error != libc::EAGAIN && error != libc::EWOULDBLOCK {
                return os_failure(call, code::FAILED, error);
            }
            match call.wait(Some(0), fd, Interest::Readable) {
                Waited::Ready => {}
                waited => return pipe_not_ready(call, waited),
            }
        }
    }
);

/// One write that fails with EPIPE, not SIGPIPE, when the child no longer
/// reads. Darwin's pipe has F_SETNOSIGPIPE; elsewhere the signal is blocked
/// on this thread for the write, and one it raised is taken back before
/// unblocking. Returns write's count, and its errno when it failed.
#[cfg(target_vendor = "apple")]
fn write_quietly(fd: c_int, bytes: &[u8]) -> (isize, c_int) {
    // SAFETY: `bytes` is readable for its length.
    let count = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
    (count, if count < 0 { errno() } else { 0 })
}

#[cfg(not(target_vendor = "apple"))]
fn write_quietly(fd: c_int, bytes: &[u8]) -> (isize, c_int) {
    // SAFETY: plain signal-set calls on local, initialised sets, and a write
    // of `bytes`, which is readable for its length.
    unsafe {
        let mut pipe_signal: libc::sigset_t = core::mem::zeroed();
        let mut previous: libc::sigset_t = core::mem::zeroed();
        let mut pending: libc::sigset_t = core::mem::zeroed();
        libc::sigemptyset(&mut pipe_signal);
        libc::sigaddset(&mut pipe_signal, libc::SIGPIPE);
        libc::sigpending(&mut pending);
        let already = libc::sigismember(&pending, libc::SIGPIPE) == 1;
        libc::pthread_sigmask(libc::SIG_BLOCK, &pipe_signal, &mut previous);
        let count = libc::write(fd, bytes.as_ptr().cast(), bytes.len());
        let error = if count < 0 { errno() } else { 0 };
        if count < 0 && error == libc::EPIPE && !already {
            let now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
            while libc::sigtimedwait(&pipe_signal, core::ptr::null_mut(), &now) < 0
                && errno() == libc::EINTR
            {}
        }
        libc::pthread_sigmask(libc::SIG_SETMASK, &previous, core::ptr::null_mut());
        (count, error)
    }
}

binding!(
    /// `native_write(pipe, bytes) -> Result<Void, (Int, Text)>`: writes
    /// every byte or fails; BROKEN_PIPE once the child stopped reading.
    chimi_exec_write => |mut call| {
        let Some(bytes) = call.bytes(1) else {
            return call.defect(c"write takes Bytes");
        };
        let mut rest = bytes;
        while !rest.is_empty() {
            // SAFETY: PIPE is a boxed `Pipe` type.
            let Some(fd) = (unsafe { call.handle::<Pipe>(0, &PIPE) }).map(|pipe| pipe.fd) else {
                return pipe_closed(call);
            };
            let (count, error) = write_quietly(fd, rest);
            if let Ok(written @ 1..) = usize::try_from(count) {
                rest = rest.get(written..).unwrap_or_default();
                continue;
            }
            if count < 0 {
                if error == libc::EINTR {
                    continue;
                }
                if error == libc::EPIPE {
                    return os_failure(call, code::BROKEN_PIPE, libc::EPIPE);
                }
                if error != libc::EAGAIN && error != libc::EWOULDBLOCK {
                    return os_failure(call, code::FAILED, error);
                }
            }
            match call.wait(Some(0), fd, Interest::Writable) {
                Waited::Ready => {}
                waited => return pipe_not_ready(call, waited),
            }
        }
        let void = call.void();
        call.ok(void)
    }
);

binding!(
    /// `native_close_pipe(pipe) -> Void`: closes the parent's end, so the
    /// child reads the end of its input, or a write to its output fails with
    /// EPIPE. Tasks waiting on it are woken first and answer Closed.
    chimi_exec_close_pipe => |mut call| {
        call.close_handle(0, &PIPE);
        void_success(call)
    }
);
