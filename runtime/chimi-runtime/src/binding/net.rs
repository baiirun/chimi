//! std/net: TCP sockets. Every descriptor is non-blocking and waits through
//! the reactor, so a slow peer never stalls another task. Name lookup, which
//! blocks in the C library, runs on a detached thread that touches nothing of
//! the runtime; the call waits on a pipe the thread writes when it is done.

use super::api::{Call, ChimiBindingCall, ChimiBindingResult, ChimiBindingType, Interest, Outcome, Waited};
use super::os::{decimal, errno, error_text};
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::Cell;
use core::ffi::{CStr, c_char, c_int, c_void};
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicUsize, Ordering};

/// The failure codes the Chimi wrapper maps to `NetError`.
mod code {
    pub const REFUSED: i64 = 1;
    pub const UNREACHABLE: i64 = 2;
    pub const RESET: i64 = 3;
    pub const CLOSED: i64 = 4;
    pub const ADDRESS_IN_USE: i64 = 5;
    pub const HOST_NOT_FOUND: i64 = 6;
    pub const TOO_MANY_OPEN: i64 = 7;
    pub const TIMED_OUT: i64 = 8;
    pub const FAILED: i64 = 9;
}

/// The most a read returns.
const CHUNK: usize = 64 * 1024;
const BACKLOG: c_int = 128;
/// The most name lookups in flight at once, across the program.
const LOOKUPS: usize = 8;

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
const SEND_FLAGS: c_int = libc::MSG_NOSIGNAL;
#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
const SEND_FLAGS: c_int = 0;

/// An open socket's state. The descriptor closes when the runtime releases
/// the handle, which drops the box.
struct Socket {
    fd: c_int,
    /// native_stop_reading ran: later reads fail.
    reading_stopped: Cell<bool>,
    /// native_finish ran: later writes fail.
    writing_finished: Cell<bool>,
}

impl Drop for Socket {
    fn drop(&mut self) {
        // SAFETY: the socket owns `fd` and nothing else closes it.
        unsafe { libc::close(self.fd) };
    }
}

static SOCKET: ChimiBindingType = ChimiBindingType::boxed::<Socket>(c"Socket");
static LISTEN_SOCKET: ChimiBindingType = ChimiBindingType::boxed::<Socket>(c"ListenSocket");

fn failure_code(error: c_int) -> i64 {
    match error {
        libc::ECONNREFUSED => code::REFUSED,
        libc::ENETUNREACH | libc::EHOSTUNREACH | libc::ENETDOWN | libc::EHOSTDOWN => {
            code::UNREACHABLE
        }
        libc::ECONNRESET | libc::ECONNABORTED | libc::EPIPE => code::RESET,
        libc::EADDRINUSE | libc::EADDRNOTAVAIL => code::ADDRESS_IN_USE,
        libc::EMFILE | libc::ENFILE | libc::ENOBUFS | libc::ENOMEM => code::TOO_MANY_OPEN,
        libc::ETIMEDOUT => code::TIMED_OUT,
        _ => code::FAILED,
    }
}

fn errno_failure(call: Call<'_>, error: c_int) -> Outcome<'_> {
    // SAFETY: strerror's text is NUL-terminated and read before any other
    // strerror on this thread.
    unsafe { call.failure_raw(failure_code(error), error_text(error)) }
}

fn closed(call: Call<'_>) -> Outcome<'_> {
    call.failure(code::CLOSED, c"the socket is closed")
}

fn wait_failed(call: Call<'_>) -> Outcome<'_> {
    call.failure(code::FAILED, c"cannot wait for the socket")
}

fn table_full(call: Call<'_>) -> Outcome<'_> {
    call.failure(code::TOO_MANY_OPEN, c"the handle table is full")
}

/// What a wait that did not become ready answers.
fn not_ready(call: Call<'_>, waited: Waited) -> Outcome<'_> {
    match waited {
        Waited::Cancelled => call.cancelled(),
        Waited::Closed => closed(call),
        Waited::Ready | Waited::Failed => wait_failed(call),
    }
}

fn set_option(fd: c_int, level: c_int, name: c_int) {
    let on: c_int = 1;
    // SAFETY: `on` is a valid c_int option value of the given size. A failure
    // leaves the default, which every caller accepts.
    unsafe {
        libc::setsockopt(
            fd,
            level,
            name,
            (&raw const on).cast::<c_void>(),
            size_of::<c_int>() as libc::socklen_t,
        )
    };
}

/// Makes `fd` non-blocking and close-on-exec, and keeps a write to a peer
/// that is gone from raising SIGPIPE where send cannot say so. `Err` holds
/// the errno.
fn prepare_descriptor(fd: c_int) -> Result<(), c_int> {
    // SAFETY: fcntl on a descriptor this call owns.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
        || unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0
    {
        return Err(errno());
    }
    #[cfg(target_vendor = "apple")]
    set_option(fd, libc::SOL_SOCKET, libc::SO_NOSIGPIPE);
    Ok(())
}

/// A descriptor this call owns until it becomes a handle's.
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

/// Makes `fd` a handle of `kind` and answers `Ok(handle)`.
fn socket_result<'call>(
    mut call: Call<'call>,
    kind: &'static ChimiBindingType,
    fd: Descriptor,
) -> Outcome<'call> {
    let socket = Box::new(Socket {
        fd: fd.into_raw(),
        reading_stopped: Cell::new(false),
        writing_finished: Cell::new(false),
    });
    // SAFETY: SOCKET and LISTEN_SOCKET are boxed `Socket` types. A full table
    // releases the box, closing the descriptor.
    match unsafe { call.make_handle(kind, socket) } {
        Some(handle) => call.ok(handle),
        None => table_full(call),
    }
}

// ---- Name lookup ------------------------------------------------------

/// The lookups in flight now.
static LIVE_LOOKUPS: AtomicUsize = AtomicUsize::new(0);

/// One lookup, shared by the call and its thread. The thread writes `status`,
/// `error` and `result` and then sets `done`.
struct Lookup {
    host: Vec<u8>,
    service: [u8; 6],
    passive: bool,
    done: AtomicBool,
    status: AtomicI32,
    /// The thread's errno, for EAI_SYSTEM.
    error: AtomicI32,
    result: AtomicPtr<libc::addrinfo>,
    write_fd: c_int,
}

impl Drop for Lookup {
    fn drop(&mut self) {
        let result = *self.result.get_mut();
        if !result.is_null() {
            // SAFETY: getaddrinfo allocated it and nothing else frees it.
            unsafe { libc::freeaddrinfo(result) };
        }
    }
}

fn hints(passive: bool, numeric: bool) -> libc::addrinfo {
    // SAFETY: an all-zero addrinfo is a valid empty hints record.
    let mut hints: libc::addrinfo = unsafe { core::mem::zeroed() };
    hints.ai_family = libc::AF_UNSPEC;
    hints.ai_socktype = libc::SOCK_STREAM;
    hints.ai_flags = libc::AI_NUMERICSERV
        | if passive { libc::AI_PASSIVE } else { 0 }
        | if numeric { libc::AI_NUMERICHOST } else { 0 };
    hints
}

extern "C" fn run_lookup(argument: *mut c_void) -> *mut c_void {
    // SAFETY: `resolve` passed one reference to an `Arc<Lookup>`, which this
    // thread now owns.
    let lookup = unsafe { Arc::from_raw(argument.cast::<Lookup>()) };
    let hints = hints(lookup.passive, false);
    let mut result = core::ptr::null_mut();
    // SAFETY: host and service are NUL-terminated and outlive the call.
    let status = unsafe {
        libc::getaddrinfo(
            lookup.host.as_ptr().cast(),
            lookup.service.as_ptr().cast(),
            &hints,
            &mut result,
        )
    };
    lookup.error.store(errno(), Ordering::Relaxed);
    lookup.status.store(status, Ordering::Relaxed);
    lookup.result.store(result, Ordering::Relaxed);
    lookup.done.store(true, Ordering::Release);
    let byte = 1u8;
    // The call may have stopped waiting and closed its end: EPIPE, not
    // SIGPIPE, since every signal is blocked on this thread. Either way
    // there is nothing to tell.
    // SAFETY: the thread owns `write_fd`.
    unsafe {
        libc::write(lookup.write_fd, (&raw const byte).cast(), 1);
        libc::close(lookup.write_fd);
    }
    LIVE_LOOKUPS.fetch_sub(1, Ordering::AcqRel);
    drop(lookup);
    core::ptr::null_mut()
}

/// Resolved addresses, freed with whatever owns them.
enum Addresses {
    /// From a numeric getaddrinfo on the scheduler thread.
    Own(*mut libc::addrinfo),
    /// From a lookup thread; the lookup frees them.
    Looked(Arc<Lookup>),
}

impl Addresses {
    fn first(&self) -> *const libc::addrinfo {
        match self {
            Addresses::Own(addresses) => *addresses,
            Addresses::Looked(lookup) => lookup.result.load(Ordering::Acquire),
        }
    }

    /// Each address in order.
    fn each(&self) -> impl Iterator<Item = &libc::addrinfo> {
        // SAFETY: each `ai_next` is null or a record of the same list, which
        // lives as long as `self`.
        core::iter::successors(unsafe { self.first().as_ref() }, |address| unsafe {
            address.ai_next.as_ref()
        })
    }
}

impl Drop for Addresses {
    fn drop(&mut self) {
        if let Addresses::Own(addresses) = self
            && !addresses.is_null()
        {
            // SAFETY: getaddrinfo allocated the list and nothing else frees it.
            unsafe { libc::freeaddrinfo(*addresses) };
        }
    }
}

/// The failure for a lookup that answered `status`.
fn resolve_failure<'call>(
    call: Call<'call>,
    host: *const c_char,
    status: c_int,
    error: c_int,
) -> Outcome<'call> {
    // glibc declares EAI_NODATA only for _GNU_SOURCE, which the C binding
    // never set, so only Apple's counts (musl and Android, not targets yet,
    // declared it to the C).
    #[cfg(target_vendor = "apple")]
    let no_data = status == libc::EAI_NODATA;
    #[cfg(not(target_vendor = "apple"))]
    let no_data = false;
    if status == libc::EAI_SYSTEM {
        errno_failure(call, error)
    } else if status == libc::EAI_MEMORY {
        // SAFETY: gai_strerror returns a static NUL-terminated string.
        unsafe { call.failure_raw(code::TOO_MANY_OPEN, libc::gai_strerror(status)) }
    } else if status == libc::EAI_NONAME || status == libc::EAI_AGAIN || status == libc::EAI_FAIL || no_data {
        // The message is the name itself, which `NetError.HostNotFound`
        // carries.
        // SAFETY: `host` is the call's NUL-terminated text argument.
        unsafe { call.failure_raw(code::HOST_NOT_FOUND, host) }
    } else {
        // SAFETY: gai_strerror returns a static NUL-terminated string.
        unsafe { call.failure_raw(code::FAILED, libc::gai_strerror(status)) }
    }
}

/// Resolves `host` (`None` for any address when `passive`) and `port`. A
/// numeric host answers at once; a name is looked up on a thread. `Err`
/// holds the result to return.
fn resolve<'call>(
    call: Call<'call>,
    host: Option<&CStr>,
    port: u16,
    passive: bool,
) -> Result<(Call<'call>, Addresses), Outcome<'call>> {
    let mut service = [0u8; 6];
    decimal(port, &mut service);
    let numeric = hints(passive, true);
    let host_pointer = host.map_or(core::ptr::null(), CStr::as_ptr);
    let mut addresses = core::ptr::null_mut();
    // SAFETY: host (or null) and service are NUL-terminated.
    let status = unsafe {
        libc::getaddrinfo(host_pointer, service.as_ptr().cast(), &numeric, &mut addresses)
    };
    if status == 0 {
        return Ok((call, Addresses::Own(addresses)));
    }
    let error = errno();
    let Some(host) = host.filter(|_| status == libc::EAI_NONAME) else {
        return Err(resolve_failure(call, host.map_or(c"".as_ptr(), CStr::as_ptr), status, error));
    };
    look_up(call, host, service, passive)
}

/// Looks `host` up on a detached thread, waiting on a pipe it writes when
/// it is done.
fn look_up<'call>(
    mut call: Call<'call>,
    host: &CStr,
    service: [u8; 6],
    passive: bool,
) -> Result<(Call<'call>, Addresses), Outcome<'call>> {
    if LIVE_LOOKUPS.fetch_add(1, Ordering::AcqRel) >= LOOKUPS {
        LIVE_LOOKUPS.fetch_sub(1, Ordering::AcqRel);
        return Err(call.failure(code::TOO_MANY_OPEN, c"too many name lookups are in flight"));
    }
    let (read_end, write_end) = match lookup_pipe() {
        Ok(ends) => ends,
        Err(error) => {
            LIVE_LOOKUPS.fetch_sub(1, Ordering::AcqRel);
            return Err(errno_failure(call, if error == 0 { libc::ENOMEM } else { error }));
        }
    };
    let lookup = Arc::new(Lookup {
        host: host.to_bytes_with_nul().to_vec(),
        service,
        passive,
        done: AtomicBool::new(false),
        status: AtomicI32::new(0),
        error: AtomicI32::new(0),
        result: AtomicPtr::new(core::ptr::null_mut()),
        write_fd: write_end.0,
    });
    let started = spawn_lookup(Arc::clone(&lookup));
    if started != 0 {
        LIVE_LOOKUPS.fetch_sub(1, Ordering::AcqRel);
        return Err(errno_failure(call, started));
    }
    // The thread owns the write end now.
    write_end.into_raw();

    while !lookup.done.load(Ordering::Acquire) {
        match call.wait(None, read_end.0, Interest::Readable) {
            Waited::Ready | Waited::Closed => {}
            Waited::Cancelled => return Err(call.cancelled()),
            Waited::Failed => return Err(wait_failed(call)),
        }
    }
    drop(read_end);
    let status = lookup.status.load(Ordering::Relaxed);
    if status != 0 {
        return Err(resolve_failure(
            call,
            host.as_ptr(),
            status,
            lookup.error.load(Ordering::Relaxed),
        ));
    }
    Ok((call, Addresses::Looked(lookup)))
}

/// A pipe whose ends close on exec, with a non-blocking read end. `Err`
/// holds the errno.
fn lookup_pipe() -> Result<(Descriptor, Descriptor), c_int> {
    let mut ends = [-1; 2];
    // SAFETY: `ends` has room for the two descriptors.
    if unsafe { libc::pipe(ends.as_mut_ptr()) } != 0 {
        return Err(errno());
    }
    let [read_end, write_end] = ends;
    let (read_end, write_end) = (Descriptor(read_end), Descriptor(write_end));
    // SAFETY: fcntl on descriptors this call owns.
    if unsafe { libc::fcntl(read_end.0, libc::F_SETFL, libc::O_NONBLOCK) } < 0
        || unsafe { libc::fcntl(read_end.0, libc::F_SETFD, libc::FD_CLOEXEC) } < 0
        || unsafe { libc::fcntl(write_end.0, libc::F_SETFD, libc::FD_CLOEXEC) } < 0
    {
        return Err(errno());
    }
    Ok((read_end, write_end))
}

/// Starts the detached lookup thread with every signal blocked, so the
/// runner's handlers and signal pipe stay on the scheduler thread. Returns
/// pthread_create's answer; on failure the reference is dropped here.
fn spawn_lookup(lookup: Arc<Lookup>) -> c_int {
    let argument = Arc::into_raw(lookup).cast_mut().cast::<c_void>();
    // SAFETY: plain pthread calls on local, initialised records; on success
    // the thread takes the reference in `argument`.
    unsafe {
        let mut all: libc::sigset_t = core::mem::zeroed();
        let mut previous: libc::sigset_t = core::mem::zeroed();
        libc::sigfillset(&mut all);
        libc::pthread_sigmask(libc::SIG_SETMASK, &all, &mut previous);
        let mut attributes: libc::pthread_attr_t = core::mem::zeroed();
        libc::pthread_attr_init(&mut attributes);
        libc::pthread_attr_setdetachstate(&mut attributes, libc::PTHREAD_CREATE_DETACHED);
        let mut thread: libc::pthread_t = core::mem::zeroed();
        let started = libc::pthread_create(&mut thread, &attributes, run_lookup, argument);
        libc::pthread_attr_destroy(&mut attributes);
        libc::pthread_sigmask(libc::SIG_SETMASK, &previous, core::ptr::null_mut());
        if started != 0 {
            drop(Arc::from_raw(argument.cast::<Lookup>()));
        }
        started
    }
}

/// The checked host and port arguments at 0 and 1.
enum Endpoint<'call> {
    /// Arguments of the wrong kind or a port outside 0..65535: a defect.
    Invalid,
    /// A host holding U+0000, which no name or address can: not found.
    NotFound(*const c_char),
    Valid(&'call CStr, u16),
}

fn endpoint<'call>(call: &Call<'call>) -> Endpoint<'call> {
    let (Some(host), Some(port)) = (call.text(0), call.integer(1)) else {
        return Endpoint::Invalid;
    };
    let Ok(port) = u16::try_from(port) else {
        return Endpoint::Invalid;
    };
    // SAFETY: the runtime ends a text argument with a NUL.
    let text = unsafe { CStr::from_ptr(host.as_ptr()) };
    if text.to_bytes().len() != host.as_bytes().len() {
        return Endpoint::NotFound(host.as_ptr());
    }
    Endpoint::Valid(text, port)
}

// ---- Connecting -------------------------------------------------------

/// How one connection attempt ended.
enum Attempt {
    Connected(Descriptor),
    Failed(c_int),
    Cancelled,
}

fn connect_one(call: &mut Call<'_>, address: &libc::addrinfo) -> Attempt {
    // SAFETY: socket takes plain integers.
    let fd = unsafe { libc::socket(address.ai_family, address.ai_socktype, address.ai_protocol) };
    if fd < 0 {
        return Attempt::Failed(errno());
    }
    let fd = Descriptor(fd);
    if let Err(error) = prepare_descriptor(fd.0) {
        return Attempt::Failed(error);
    }
    // SAFETY: the address record is getaddrinfo's, with its own length.
    if unsafe { libc::connect(fd.0, address.ai_addr, address.ai_addrlen) } != 0 {
        let error = errno();
        if error != libc::EINPROGRESS && error != libc::EINTR {
            return Attempt::Failed(error);
        }
        match call.wait(None, fd.0, Interest::Writable) {
            Waited::Ready => {}
            Waited::Cancelled => return Attempt::Cancelled,
            Waited::Failed | Waited::Closed => return Attempt::Failed(libc::EBADF),
        }
        let mut error: c_int = 0;
        let mut size = size_of::<c_int>() as libc::socklen_t;
        // SAFETY: `error` and `size` describe a c_int-sized buffer.
        if unsafe {
            libc::getsockopt(
                fd.0,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&raw mut error).cast(),
                &mut size,
            )
        } != 0
        {
            error = errno();
        }
        if error != 0 {
            return Attempt::Failed(error);
        }
    }
    set_option(fd.0, libc::IPPROTO_TCP, libc::TCP_NODELAY);
    Attempt::Connected(fd)
}

/// `native_connect(host, port) -> Result<Socket, (Int, Text)>`: tries each
/// resolved address in order and reports the last failure.
///
/// # Safety
///
/// Only the runtime calls it, with the live binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_net_connect(raw: *mut ChimiBindingCall) -> *mut ChimiBindingResult {
    // SAFETY: the runtime passes the call it is running.
    let call = unsafe { Call::new(raw) };
    connect(call).into_raw()
}

fn connect(call: Call<'_>) -> Outcome<'_> {
    let (host, port) = match endpoint(&call) {
        Endpoint::Invalid => return call.defect(c"connect takes a host and a port"),
        // SAFETY: the host is the call's NUL-terminated text argument.
        Endpoint::NotFound(host) => return unsafe { call.failure_raw(code::HOST_NOT_FOUND, host) },
        Endpoint::Valid(host, port) => (host, port),
    };
    let (mut call, addresses) = match resolve(call, Some(host), port, false) {
        Ok(resolved) => resolved,
        Err(outcome) => return outcome,
    };
    let mut last = libc::ECONNREFUSED;
    for address in addresses.each() {
        match connect_one(&mut call, address) {
            Attempt::Connected(fd) => return socket_result(call, &SOCKET, fd),
            Attempt::Cancelled => return call.cancelled(),
            Attempt::Failed(error) => last = error,
        }
    }
    errno_failure(call, last)
}

// ---- Listening --------------------------------------------------------

/// `native_listen(host, port) -> Result<ListenSocket, (Int, Text)>`. An
/// empty host listens on 0.0.0.0, every IPv4 address, on every platform;
/// port 0 picks a free port.
///
/// # Safety
///
/// Only the runtime calls it, with the live binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_net_listen(raw: *mut ChimiBindingCall) -> *mut ChimiBindingResult {
    // SAFETY: the runtime passes the call it is running.
    let call = unsafe { Call::new(raw) };
    listen(call).into_raw()
}

fn listen(call: Call<'_>) -> Outcome<'_> {
    let (host, port) = match endpoint(&call) {
        Endpoint::Invalid => return call.defect(c"listen takes a host and a port"),
        // SAFETY: the host is the call's NUL-terminated text argument.
        Endpoint::NotFound(host) => return unsafe { call.failure_raw(code::HOST_NOT_FOUND, host) },
        Endpoint::Valid(host, port) => (host, port),
    };
    let host = if host.is_empty() { c"0.0.0.0" } else { host };
    let (call, addresses) = match resolve(call, Some(host), port, true) {
        Ok(resolved) => resolved,
        Err(outcome) => return outcome,
    };
    let mut last = libc::EADDRNOTAVAIL;
    for address in addresses.each() {
        // SAFETY: socket takes plain integers.
        let fd = unsafe { libc::socket(address.ai_family, address.ai_socktype, address.ai_protocol) };
        if fd < 0 {
            last = errno();
            continue;
        }
        let fd = Descriptor(fd);
        set_option(fd.0, libc::SOL_SOCKET, libc::SO_REUSEADDR);
        let bound = prepare_descriptor(fd.0).and_then(|()| {
            // SAFETY: the address record is getaddrinfo's, with its own
            // length.
            if unsafe { libc::bind(fd.0, address.ai_addr, address.ai_addrlen) } != 0
                || unsafe { libc::listen(fd.0, BACKLOG) } != 0
            {
                Err(errno())
            } else {
                Ok(())
            }
        });
        match bound {
            Ok(()) => return socket_result(call, &LISTEN_SOCKET, fd),
            Err(error) => last = error,
        }
    }
    errno_failure(call, last)
}

/// Whether accept's `error` is a network error already pending on the new
/// connection, which Linux reports from accept (accept(2)).
fn accept_retries(error: c_int) -> bool {
    #[cfg(target_os = "linux")]
    {
        matches!(
            error,
            libc::ENETDOWN
                | libc::EPROTO
                | libc::ENOPROTOOPT
                | libc::EHOSTDOWN
                | libc::ENONET
                | libc::EHOSTUNREACH
                | libc::EOPNOTSUPP
                | libc::ENETUNREACH
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = error;
        false
    }
}

/// `native_accept(listener) -> Result<Socket, (Int, Text)>`: waits for the
/// next connection.
///
/// # Safety
///
/// Only the runtime calls it, with the live binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_net_accept(raw: *mut ChimiBindingCall) -> *mut ChimiBindingResult {
    // SAFETY: the runtime passes the call it is running.
    let call = unsafe { Call::new(raw) };
    accept(call).into_raw()
}

fn accept(mut call: Call<'_>) -> Outcome<'_> {
    loop {
        // SAFETY: LISTEN_SOCKET is a boxed `Socket` type.
        let Some(listener) = (unsafe { call.handle::<Socket>(0, &LISTEN_SOCKET) }) else {
            return closed(call);
        };
        let listener = listener.fd;
        // SAFETY: accept on the listener's descriptor, asking no address.
        let fd = unsafe { libc::accept(listener, core::ptr::null_mut(), core::ptr::null_mut()) };
        if fd >= 0 {
            let fd = Descriptor(fd);
            // SAFETY: an all-zero sockaddr_storage is valid to write into.
            let mut peer: libc::sockaddr_storage = unsafe { core::mem::zeroed() };
            let mut size = size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            // SAFETY: `peer` and `size` describe a sockaddr_storage buffer.
            if unsafe { libc::getpeername(fd.0, (&raw mut peer).cast(), &mut size) } != 0 {
                // The peer reset the connection before it was accepted: it
                // is not this listener's failure, and its addresses cannot
                // be read.
                continue;
            }
            if let Err(error) = prepare_descriptor(fd.0) {
                return errno_failure(call, error);
            }
            set_option(fd.0, libc::IPPROTO_TCP, libc::TCP_NODELAY);
            return socket_result(call, &SOCKET, fd);
        }
        // A connection that was reset before it was accepted is not this
        // listener's failure, nor a network error already pending on it.
        let error = errno();
        if error == libc::EINTR || error == libc::ECONNABORTED || accept_retries(error) {
            continue;
        }
        if error != libc::EAGAIN && error != libc::EWOULDBLOCK {
            return errno_failure(call, error);
        }
        match call.wait(Some(0), listener, Interest::Readable) {
            Waited::Ready => {}
            waited => return not_ready(call, waited),
        }
    }
}

// ---- Reading and writing ----------------------------------------------

/// `native_read(socket) -> Result<Option<Bytes>, (Int, Text)>`: at most 64
/// KiB, waiting until some arrive; `None` once the peer has finished writing.
///
/// # Safety
///
/// Only the runtime calls it, with the live binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_net_read(raw: *mut ChimiBindingCall) -> *mut ChimiBindingResult {
    // SAFETY: the runtime passes the call it is running.
    let call = unsafe { Call::new(raw) };
    read(call).into_raw()
}

fn read(mut call: Call<'_>) -> Outcome<'_> {
    // On the heap: a task's stack is too small to hold a whole chunk.
    let mut buffer: Vec<u8> = Vec::new();
    if buffer.try_reserve_exact(CHUNK).is_err() {
        return errno_failure(call, libc::ENOMEM);
    }
    loop {
        // SAFETY: SOCKET is a boxed `Socket` type.
        let fd = match unsafe { call.handle::<Socket>(0, &SOCKET) } {
            Some(socket) if !socket.reading_stopped.get() => socket.fd,
            _ => return closed(call),
        };
        // SAFETY: the buffer has CHUNK bytes of capacity to write into.
        let count = unsafe { libc::recv(fd, buffer.as_mut_ptr().cast(), CHUNK, 0) };
        if let Ok(length @ 1..) = usize::try_from(count) {
            // SAFETY: recv wrote `length` bytes, at most CHUNK.
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
            return errno_failure(call, error);
        }
        match call.wait(Some(0), fd, Interest::Readable) {
            Waited::Ready => {}
            waited => return not_ready(call, waited),
        }
    }
}

/// `native_write(socket, bytes) -> Result<Void, (Int, Text)>`: writes every
/// byte or fails.
///
/// # Safety
///
/// Only the runtime calls it, with the live binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_net_write(raw: *mut ChimiBindingCall) -> *mut ChimiBindingResult {
    // SAFETY: the runtime passes the call it is running.
    let call = unsafe { Call::new(raw) };
    write(call).into_raw()
}

fn write(mut call: Call<'_>) -> Outcome<'_> {
    let Some(bytes) = call.bytes(1) else {
        return call.defect(c"write takes Bytes");
    };
    let mut rest = bytes;
    while !rest.is_empty() {
        // SAFETY: SOCKET is a boxed `Socket` type.
        let fd = match unsafe { call.handle::<Socket>(0, &SOCKET) } {
            Some(socket) if !socket.writing_finished.get() => socket.fd,
            _ => return closed(call),
        };
        // SAFETY: `rest` is readable for its length.
        let count = unsafe { libc::send(fd, rest.as_ptr().cast(), rest.len(), SEND_FLAGS) };
        if let Ok(written @ 1..) = usize::try_from(count) {
            rest = rest.get(written..).unwrap_or_default();
            continue;
        }
        if count < 0 {
            let error = errno();
            if error == libc::EINTR {
                continue;
            }
            if error != libc::EAGAIN && error != libc::EWOULDBLOCK {
                return errno_failure(call, error);
            }
        }
        match call.wait(Some(0), fd, Interest::Writable) {
            Waited::Ready => {}
            waited => return not_ready(call, waited),
        }
    }
    let void = call.void();
    call.ok(void)
}

/// `native_finish(socket) -> Result<Void, (Int, Text)>`: ends the writing
/// side, so the peer reads the end of input; reading goes on.
///
/// # Safety
///
/// Only the runtime calls it, with the live binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_net_finish(raw: *mut ChimiBindingCall) -> *mut ChimiBindingResult {
    // SAFETY: the runtime passes the call it is running.
    let mut call = unsafe { Call::new(raw) };
    // SAFETY: SOCKET is a boxed `Socket` type.
    let Some(socket) = (unsafe { call.handle::<Socket>(0, &SOCKET) }) else {
        return closed(call).into_raw();
    };
    if !socket.writing_finished.replace(true) {
        // SAFETY: shutdown on the socket's own descriptor.
        if unsafe { libc::shutdown(socket.fd, libc::SHUT_WR) } != 0 {
            let error = errno();
            if error != libc::ENOTCONN {
                return errno_failure(call, error).into_raw();
            }
        }
    }
    let void = call.void();
    call.ok(void).into_raw()
}

/// `native_stop_reading(socket) -> Void`: ends this side's reading only, so
/// a stopped reader leaves the writer working; later reads fail with Closed.
///
/// # Safety
///
/// Only the runtime calls it, with the live binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_net_stop_reading(raw: *mut ChimiBindingCall) -> *mut ChimiBindingResult {
    // SAFETY: the runtime passes the call it is running.
    let mut call = unsafe { Call::new(raw) };
    // SAFETY: SOCKET is a boxed `Socket` type.
    if let Some(socket) = unsafe { call.handle::<Socket>(0, &SOCKET) }
        && !socket.reading_stopped.replace(true)
    {
        // SAFETY: shutdown on the socket's own descriptor; a failure leaves
        // reads to fail on the flag.
        unsafe { libc::shutdown(socket.fd, libc::SHUT_RD) };
    }
    let void = call.void();
    call.success(void).into_raw()
}

/// `native_close(socket) -> Void`
///
/// # Safety
///
/// Only the runtime calls it, with the live binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_net_close(raw: *mut ChimiBindingCall) -> *mut ChimiBindingResult {
    // SAFETY: the runtime passes the call it is running.
    let call = unsafe { Call::new(raw) };
    close(call, &SOCKET).into_raw()
}

/// `native_close_listener(listener) -> Void`
///
/// # Safety
///
/// Only the runtime calls it, with the live binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_net_close_listener(raw: *mut ChimiBindingCall) -> *mut ChimiBindingResult {
    // SAFETY: the runtime passes the call it is running.
    let call = unsafe { Call::new(raw) };
    close(call, &LISTEN_SOCKET).into_raw()
}

fn close<'call>(mut call: Call<'call>, kind: &'static ChimiBindingType) -> Outcome<'call> {
    call.close_handle(0, kind);
    let void = call.void();
    call.success(void)
}

// ---- Addresses --------------------------------------------------------

/// Which end of a socket an address names.
#[derive(Clone, Copy)]
enum End {
    Local,
    Peer,
}

fn address_of<'call>(mut call: Call<'call>, kind: &'static ChimiBindingType, end: End) -> Outcome<'call> {
    // SAFETY: SOCKET and LISTEN_SOCKET are boxed `Socket` types.
    let Some(socket) = (unsafe { call.handle::<Socket>(0, kind) }) else {
        return closed(call);
    };
    let fd = socket.fd;
    // SAFETY: an all-zero sockaddr_storage is valid to write into.
    let mut address: libc::sockaddr_storage = unsafe { core::mem::zeroed() };
    let mut size = size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    let pointer = (&raw mut address).cast::<libc::sockaddr>();
    // SAFETY: `address` and `size` describe a sockaddr_storage buffer.
    let status = unsafe {
        match end {
            End::Peer => libc::getpeername(fd, pointer, &mut size),
            End::Local => libc::getsockname(fd, pointer, &mut size),
        }
    };
    if status != 0 {
        return errno_failure(call, errno());
    }
    let mut host = [0u8; libc::NI_MAXHOST as usize];
    let mut service = [0u8; 32];
    // SAFETY: the buffers are writable for the lengths given, and getnameinfo
    // NUL-terminates what it writes.
    let status = unsafe {
        libc::getnameinfo(
            pointer,
            size,
            host.as_mut_ptr().cast(),
            host.len() as libc::socklen_t,
            service.as_mut_ptr().cast(),
            service.len() as libc::socklen_t,
            libc::NI_NUMERICHOST | libc::NI_NUMERICSERV,
        )
    };
    if status != 0 {
        // SAFETY: gai_strerror returns a static NUL-terminated string.
        return unsafe { call.failure_raw(code::FAILED, libc::gai_strerror(status)) };
    }
    let host = CStr::from_bytes_until_nul(&host).map_or(&[][..], CStr::to_bytes);
    let port = CStr::from_bytes_until_nul(&service)
        .map_or(&[][..], CStr::to_bytes)
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .fold(0i64, |port, digit| port.saturating_mul(10).saturating_add(i64::from(digit - b'0')));
    let parts = [call.make_text(host), call.make_integer(port)];
    let tuple = call.make_tuple(&parts);
    call.ok(tuple)
}

/// `native_local_address(socket) -> Result<(Text, Int), (Int, Text)>`
///
/// # Safety
///
/// Only the runtime calls it, with the live binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_net_local_address(raw: *mut ChimiBindingCall) -> *mut ChimiBindingResult {
    // SAFETY: the runtime passes the call it is running.
    address_of(unsafe { Call::new(raw) }, &SOCKET, End::Local).into_raw()
}

/// `native_peer_address(socket) -> Result<(Text, Int), (Int, Text)>`
///
/// # Safety
///
/// Only the runtime calls it, with the live binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_net_peer_address(raw: *mut ChimiBindingCall) -> *mut ChimiBindingResult {
    // SAFETY: the runtime passes the call it is running.
    address_of(unsafe { Call::new(raw) }, &SOCKET, End::Peer).into_raw()
}

/// `native_listener_address(listener) -> Result<(Text, Int), (Int, Text)>`
///
/// # Safety
///
/// Only the runtime calls it, with the live binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_net_listener_address(
    raw: *mut ChimiBindingCall,
) -> *mut ChimiBindingResult {
    // SAFETY: the runtime passes the call it is running.
    address_of(unsafe { Call::new(raw) }, &LISTEN_SOCKET, End::Local).into_raw()
}

/// `native_allow_real_network() -> Void`: test-only (T4401), so a program
/// cannot reach `Net.host_for_tests`. Does nothing.
///
/// # Safety
///
/// Only the runtime calls it, with the live binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_net_allow_real_network(
    raw: *mut ChimiBindingCall,
) -> *mut ChimiBindingResult {
    // SAFETY: the runtime passes the call it is running.
    let mut call = unsafe { Call::new(raw) };
    let void = call.void();
    call.success(void).into_raw()
}
