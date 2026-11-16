/* std/net: TCP sockets over the binding API (G20). Every descriptor is
   non-blocking and waits through the reactor, so a slow peer never stalls
   another task. Name lookup, which blocks in the C library, runs on a
   detached thread that touches nothing of the runtime; the call waits on a
   pipe the thread writes when it is done. */
#define _POSIX_C_SOURCE 200809L
#define _DEFAULT_SOURCE
#define _DARWIN_C_SOURCE
#include "chimi_binding.h"

#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netdb.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

/* The failure codes the Chimi wrapper maps to `NetError`. */
enum {
  NET_REFUSED = 1,
  NET_UNREACHABLE = 2,
  NET_RESET = 3,
  NET_CLOSED = 4,
  NET_ADDRESS_IN_USE = 5,
  NET_HOST_NOT_FOUND = 6,
  NET_TOO_MANY_OPEN = 7,
  NET_TIMED_OUT = 8,
  NET_FAILED = 9,
};

enum { NET_CHUNK = 64 * 1024, NET_BACKLOG = 128, NET_LOOKUPS = 8 };

#ifdef MSG_NOSIGNAL
#define NET_SEND_FLAGS MSG_NOSIGNAL
#else
#define NET_SEND_FLAGS 0
#endif

typedef struct {
  int fd;
  int reading_stopped; /* native_stop_reading ran: later reads fail */
} Socket;

static void release_socket(void *state) {
  Socket *socket_state = state;
  close(socket_state->fd);
  free(socket_state);
}

static const ChimiBindingType SOCKET = {"Socket", release_socket};
static const ChimiBindingType LISTEN_SOCKET = {"ListenSocket", release_socket};

static int failure_code(int error) {
  switch (error) {
    case ECONNREFUSED: return NET_REFUSED;
    case ENETUNREACH:
    case EHOSTUNREACH:
    case ENETDOWN:
#ifdef EHOSTDOWN
    case EHOSTDOWN:
#endif
      return NET_UNREACHABLE;
    case ECONNRESET:
    case ECONNABORTED:
    case EPIPE: return NET_RESET;
    case EADDRINUSE:
    case EADDRNOTAVAIL: return NET_ADDRESS_IN_USE;
    case EMFILE:
    case ENFILE:
    case ENOBUFS:
    case ENOMEM: return NET_TOO_MANY_OPEN;
    case ETIMEDOUT: return NET_TIMED_OUT;
    default: return NET_FAILED;
  }
}

static ChimiBindingResult *errno_failure(ChimiBindingCall *call, int error) {
  return chimi_binding_failure(call, failure_code(error), strerror(error));
}

static ChimiBindingResult *closed(ChimiBindingCall *call) {
  return chimi_binding_failure(call, NET_CLOSED, "the socket is closed");
}

static ChimiBindingResult *wait_failed(ChimiBindingCall *call) {
  return chimi_binding_failure(call, NET_FAILED, "cannot wait for the socket");
}

/* What a wait that did not become ready answers: a cancellation, a closed
   handle, or a reactor that cannot wait. */
static ChimiBindingResult *not_ready(ChimiBindingCall *call, int waited) {
  if (waited == CHIMI_BINDING_WAIT_CANCELLED) return chimi_binding_cancelled(call);
  if (waited == CHIMI_BINDING_WAIT_CLOSED) return closed(call);
  return wait_failed(call);
}

/* Makes `fd` non-blocking and close-on-exec, and keeps a write to a peer
   that is gone from raising SIGPIPE where send cannot say so. */
static int prepare_descriptor(int fd) {
  int flags = fcntl(fd, F_GETFL);
  if (flags < 0 || fcntl(fd, F_SETFL, flags | O_NONBLOCK) < 0) return -1;
  if (fcntl(fd, F_SETFD, FD_CLOEXEC) < 0) return -1;
#ifdef SO_NOSIGPIPE
  int on = 1;
  (void)setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, sizeof(on));
#endif
  return 0;
}

static ChimiBindingValue *socket_handle(ChimiBindingCall *call, const ChimiBindingType *type,
                                        int fd) {
  Socket *state = malloc(sizeof(*state));
  if (state == NULL) {
    close(fd);
    return NULL;
  }
  state->fd = fd;
  state->reading_stopped = 0;
  /* make_handle releases `state` (closing fd) when the table is full. */
  return chimi_binding_make_handle(call, type, state);
}

/* ---- Name lookup ---------------------------------------------------- */

/* One lookup, shared by the call and its thread; whichever lets go last
   frees it. The thread writes only `status`, `error`, `result` and `done`. */
typedef struct {
  atomic_int references;
  atomic_bool done;
  char *host;
  char service[8];
  int passive;
  int status;
  int error; /* the thread's errno, for EAI_SYSTEM */
  struct addrinfo *result;
  int write_fd;
} Lookup;

static atomic_int live_lookups;

static void drop_lookup(Lookup *lookup) {
  if (atomic_fetch_sub(&lookup->references, 1) != 1) return;
  if (lookup->result != NULL) freeaddrinfo(lookup->result);
  free(lookup->host);
  free(lookup);
}

static struct addrinfo lookup_hints(int passive, int numeric) {
  struct addrinfo hints;
  memset(&hints, 0, sizeof(hints));
  hints.ai_family = AF_UNSPEC;
  hints.ai_socktype = SOCK_STREAM;
  hints.ai_flags = AI_NUMERICSERV | (passive ? AI_PASSIVE : 0) | (numeric ? AI_NUMERICHOST : 0);
  return hints;
}

static void *run_lookup(void *argument) {
  Lookup *lookup = argument;
  struct addrinfo hints = lookup_hints(lookup->passive, 0);
  lookup->status = getaddrinfo(lookup->host, lookup->service, &hints, &lookup->result);
  lookup->error = errno;
  atomic_store_explicit(&lookup->done, true, memory_order_release);
  char byte = 1;
  /* The call may have stopped waiting and closed its end: EPIPE, not
     SIGPIPE, since every signal is blocked on this thread. */
  if (write(lookup->write_fd, &byte, 1) < 0) {
    /* Nothing to tell: the call already stopped waiting. */
  }
  close(lookup->write_fd);
  atomic_fetch_sub(&live_lookups, 1);
  drop_lookup(lookup);
  return NULL;
}

/* A lookup's outcome: the addresses, or a result to return. */
typedef struct {
  struct addrinfo *addresses;
  Lookup *lookup;
  ChimiBindingResult *result;
} Resolved;

static Resolved resolve_failure(ChimiBindingCall *call, const char *host, int status, int error) {
  Resolved resolved = {NULL, NULL, NULL};
  if (status == EAI_SYSTEM) {
    resolved.result = errno_failure(call, error);
  } else if (status == EAI_MEMORY) {
    resolved.result = chimi_binding_failure(call, NET_TOO_MANY_OPEN, gai_strerror(status));
  } else if (status == EAI_NONAME || status == EAI_AGAIN || status == EAI_FAIL
#ifdef EAI_NODATA
             || status == EAI_NODATA
#endif
  ) {
    /* The message is the name itself, which `NetError.HostNotFound` carries. */
    resolved.result = chimi_binding_failure(call, NET_HOST_NOT_FOUND, host);
  } else {
    resolved.result = chimi_binding_failure(call, NET_FAILED, gai_strerror(status));
  }
  return resolved;
}

/* Resolves `host` (NULL for any address when `passive`) and `port`. A
   numeric host answers at once; a name is looked up on a thread. */
static Resolved resolve(ChimiBindingCall *call, const char *host, int64_t port, int passive) {
  Resolved resolved = {NULL, NULL, NULL};
  char service[8];
  snprintf(service, sizeof(service), "%d", (int)port);
  struct addrinfo hints = lookup_hints(passive, 1);
  int status = getaddrinfo(host, service, &hints, &resolved.addresses);
  if (status == 0) return resolved;
  if (status != EAI_NONAME || host == NULL) return resolve_failure(call, host ? host : "", status, errno);

  if (atomic_fetch_add(&live_lookups, 1) >= NET_LOOKUPS) {
    atomic_fetch_sub(&live_lookups, 1);
    resolved.result =
      chimi_binding_failure(call, NET_TOO_MANY_OPEN, "too many name lookups are in flight");
    return resolved;
  }
  Lookup *lookup = calloc(1, sizeof(*lookup));
  int ends[2] = {-1, -1};
  if (lookup == NULL || (lookup->host = strdup(host)) == NULL || pipe(ends) != 0 ||
      fcntl(ends[0], F_SETFL, O_NONBLOCK) < 0 || fcntl(ends[0], F_SETFD, FD_CLOEXEC) < 0 ||
      fcntl(ends[1], F_SETFD, FD_CLOEXEC) < 0) {
    int error = errno;
    if (ends[0] >= 0) close(ends[0]);
    if (ends[1] >= 0) close(ends[1]);
    if (lookup != NULL) free(lookup->host);
    free(lookup);
    atomic_fetch_sub(&live_lookups, 1);
    resolved.result = errno_failure(call, error ? error : ENOMEM);
    return resolved;
  }
  memcpy(lookup->service, service, sizeof(service));
  lookup->passive = passive;
  lookup->write_fd = ends[1];
  atomic_init(&lookup->references, 2);
  atomic_init(&lookup->done, false);

  /* Block every signal on the thread, so the runner's handlers and signal
     pipe stay on the scheduler thread. */
  sigset_t all, previous;
  sigfillset(&all);
  pthread_sigmask(SIG_SETMASK, &all, &previous);
  pthread_attr_t attributes;
  pthread_attr_init(&attributes);
  pthread_attr_setdetachstate(&attributes, PTHREAD_CREATE_DETACHED);
  pthread_t thread;
  int started = pthread_create(&thread, &attributes, run_lookup, lookup);
  pthread_attr_destroy(&attributes);
  pthread_sigmask(SIG_SETMASK, &previous, NULL);
  if (started != 0) {
    close(ends[0]);
    close(ends[1]);
    atomic_fetch_sub(&live_lookups, 1);
    free(lookup->host);
    free(lookup);
    resolved.result = errno_failure(call, started);
    return resolved;
  }

  for (;;) {
    if (atomic_load_explicit(&lookup->done, memory_order_acquire)) break;
    int waited = chimi_binding_wait(call, CHIMI_BINDING_NO_HANDLE, ends[0], CHIMI_BINDING_READABLE);
    if (waited == CHIMI_BINDING_WAIT_CANCELLED || waited == CHIMI_BINDING_WAIT_FAILED) {
      close(ends[0]);
      drop_lookup(lookup);
      resolved.result = waited == CHIMI_BINDING_WAIT_CANCELLED
                          ? chimi_binding_cancelled(call)
                          : wait_failed(call);
      return resolved;
    }
  }
  close(ends[0]);
  if (lookup->status != 0) {
    resolved = resolve_failure(call, host, lookup->status, lookup->error);
    drop_lookup(lookup);
    return resolved;
  }
  resolved.addresses = lookup->result;
  resolved.lookup = lookup;
  return resolved;
}

static void release_resolved(Resolved *resolved) {
  if (resolved->lookup != NULL) {
    drop_lookup(resolved->lookup); /* frees the addresses with it */
  } else if (resolved->addresses != NULL) {
    freeaddrinfo(resolved->addresses);
  }
}

/* The host and port arguments at 0 and 1, checked: 0 for arguments of the
   wrong kind (a defect), -1 for a host holding U+0000, which no name or
   address can, so it is a host that is not found. */
static int host_and_port(const ChimiBindingCall *call, const char **host, int64_t *port) {
  size_t length;
  if (!chimi_binding_text(call, 0, host, &length) || !chimi_binding_integer(call, 1, port) ||
      *port < 0 || *port > 65535)
    return 0;
  return strlen(*host) == length ? 1 : -1;
}

/* ---- Connecting ----------------------------------------------------- */

/* Connects to one address; 0 with the new descriptor in `out`, or an errno.
   A cancellation answers -1. */
static int connect_one(ChimiBindingCall *call, const struct addrinfo *address, int *out) {
  int fd = socket(address->ai_family, address->ai_socktype, address->ai_protocol);
  if (fd < 0) return errno;
  if (prepare_descriptor(fd) != 0) {
    int error = errno;
    close(fd);
    return error;
  }
  if (connect(fd, address->ai_addr, address->ai_addrlen) != 0) {
    if (errno != EINPROGRESS && errno != EINTR) {
      int error = errno;
      close(fd);
      return error;
    }
    int waited = chimi_binding_wait(call, CHIMI_BINDING_NO_HANDLE, fd, CHIMI_BINDING_WRITABLE);
    if (waited != CHIMI_BINDING_READY) {
      close(fd);
      return waited == CHIMI_BINDING_WAIT_CANCELLED ? -1 : EBADF;
    }
    int error = 0;
    socklen_t size = sizeof(error);
    if (getsockopt(fd, SOL_SOCKET, SO_ERROR, &error, &size) != 0) error = errno;
    if (error != 0) {
      close(fd);
      return error;
    }
  }
  int on = 1;
  (void)setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &on, sizeof(on));
  *out = fd;
  return 0;
}

/* native_connect(host, port) -> Result<Socket, (Int, Text)>: tries each
   resolved address in order and reports the last failure. */
ChimiBindingResult *chimi_net_connect(ChimiBindingCall *call) {
  const char *host;
  int64_t port;
  int arguments = host_and_port(call, &host, &port);
  if (arguments == 0) return chimi_binding_defect(call, "connect takes a host and a port");
  if (arguments < 0) return chimi_binding_failure(call, NET_HOST_NOT_FOUND, host);
  Resolved resolved = resolve(call, host, port, 0);
  if (resolved.result != NULL) return resolved.result;
  int last = ECONNREFUSED;
  for (const struct addrinfo *address = resolved.addresses; address; address = address->ai_next) {
    int fd;
    int error = connect_one(call, address, &fd);
    if (error == -1) {
      release_resolved(&resolved);
      return chimi_binding_cancelled(call);
    }
    if (error == 0) {
      release_resolved(&resolved);
      ChimiBindingValue *handle = socket_handle(call, &SOCKET, fd);
      if (handle == NULL)
        return chimi_binding_failure(call, NET_TOO_MANY_OPEN, "the handle table is full");
      return chimi_binding_ok(call, handle);
    }
    last = error;
  }
  release_resolved(&resolved);
  return errno_failure(call, last);
}

/* ---- Listening ------------------------------------------------------ */

/* native_listen(host, port) -> Result<ListenSocket, (Int, Text)>. An empty
   host listens on every address; port 0 picks a free port. */
ChimiBindingResult *chimi_net_listen(ChimiBindingCall *call) {
  const char *host;
  int64_t port;
  int arguments = host_and_port(call, &host, &port);
  if (arguments == 0) return chimi_binding_defect(call, "listen takes a host and a port");
  if (arguments < 0) return chimi_binding_failure(call, NET_HOST_NOT_FOUND, host);
  Resolved resolved = resolve(call, host[0] ? host : NULL, port, 1);
  if (resolved.result != NULL) return resolved.result;
  int last = EADDRNOTAVAIL;
  for (const struct addrinfo *address = resolved.addresses; address; address = address->ai_next) {
    int fd = socket(address->ai_family, address->ai_socktype, address->ai_protocol);
    if (fd < 0) {
      last = errno;
      continue;
    }
    int on = 1;
    (void)setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &on, sizeof(on));
    if (prepare_descriptor(fd) != 0 || bind(fd, address->ai_addr, address->ai_addrlen) != 0 ||
        listen(fd, NET_BACKLOG) != 0) {
      last = errno;
      close(fd);
      continue;
    }
    release_resolved(&resolved);
    ChimiBindingValue *handle = socket_handle(call, &LISTEN_SOCKET, fd);
    if (handle == NULL)
      return chimi_binding_failure(call, NET_TOO_MANY_OPEN, "the handle table is full");
    return chimi_binding_ok(call, handle);
  }
  release_resolved(&resolved);
  return errno_failure(call, last);
}

static int accept_retries(int error) {
#ifdef __linux__
  switch (error) {
    case ENETDOWN:
    case EPROTO:
    case ENOPROTOOPT:
    case EHOSTDOWN:
    case ENONET:
    case EHOSTUNREACH:
    case EOPNOTSUPP:
    case ENETUNREACH: return 1;
    default: return 0;
  }
#else
  (void)error;
  return 0;
#endif
}

/* native_accept(listener) -> Result<Socket, (Int, Text)>: waits for the next
   connection. */
ChimiBindingResult *chimi_net_accept(ChimiBindingCall *call) {
  for (;;) {
    Socket *listener = chimi_binding_handle(call, 0, &LISTEN_SOCKET);
    if (listener == NULL) return closed(call);
    struct sockaddr_storage peer;
    socklen_t peer_size = sizeof(peer);
    int fd = accept(listener->fd, NULL, NULL);
    if (fd >= 0 && getpeername(fd, (struct sockaddr *)&peer, &peer_size) != 0) {
      /* The peer reset the connection before it was accepted: it is not this
         listener's failure, and its addresses cannot be read. */
      close(fd);
      continue;
    }
    if (fd >= 0) {
      if (prepare_descriptor(fd) != 0) {
        int error = errno;
        close(fd);
        return errno_failure(call, error);
      }
      int on = 1;
      (void)setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &on, sizeof(on));
      ChimiBindingValue *handle = socket_handle(call, &SOCKET, fd);
      if (handle == NULL)
        return chimi_binding_failure(call, NET_TOO_MANY_OPEN, "the handle table is full");
      return chimi_binding_ok(call, handle);
    }
    /* A connection that was reset before it was accepted is not this
       listener's failure, nor (on Linux, per accept(2)) a network error
       already pending on the new connection. */
    if (errno == EINTR || errno == ECONNABORTED || accept_retries(errno)) continue;
    if (errno != EAGAIN && errno != EWOULDBLOCK) return errno_failure(call, errno);
    int waited = chimi_binding_wait(call, 0, listener->fd, CHIMI_BINDING_READABLE);
    if (waited != CHIMI_BINDING_READY) return not_ready(call, waited);
  }
}

/* ---- Reading and writing -------------------------------------------- */

/* native_read(socket) -> Result<Option<Bytes>, (Int, Text)>: at most 64 KiB,
   waiting until some arrive; None once the peer has finished writing. */
ChimiBindingResult *chimi_net_read(ChimiBindingCall *call) {
  /* On the heap: a task's stack is too small to hold a whole chunk. */
  unsigned char *buffer = malloc(NET_CHUNK);
  if (buffer == NULL) return errno_failure(call, ENOMEM);
  ChimiBindingResult *result;
  for (;;) {
    Socket *socket_state = chimi_binding_handle(call, 0, &SOCKET);
    if (socket_state == NULL || socket_state->reading_stopped) {
      result = closed(call);
      break;
    }
    ssize_t count = recv(socket_state->fd, buffer, NET_CHUNK, 0);
    if (count > 0) {
      result = chimi_binding_ok(
        call, chimi_binding_some(call, chimi_binding_make_bytes(call, buffer, (size_t)count)));
      break;
    }
    if (count == 0) {
      result = chimi_binding_ok(call, chimi_binding_none(call));
      break;
    }
    if (errno == EINTR) continue;
    if (errno != EAGAIN && errno != EWOULDBLOCK) {
      result = errno_failure(call, errno);
      break;
    }
    int waited = chimi_binding_wait(call, 0, socket_state->fd, CHIMI_BINDING_READABLE);
    if (waited != CHIMI_BINDING_READY) {
      result = not_ready(call, waited);
      break;
    }
  }
  free(buffer);
  return result;
}

/* native_write(socket, bytes) -> Result<Void, (Int, Text)>: writes every
   byte or fails. */
ChimiBindingResult *chimi_net_write(ChimiBindingCall *call) {
  const unsigned char *bytes;
  size_t length;
  if (!chimi_binding_bytes(call, 1, &bytes, &length)) return chimi_binding_defect(call, "write takes Bytes");
  size_t written = 0;
  while (written < length) {
    Socket *socket_state = chimi_binding_handle(call, 0, &SOCKET);
    if (socket_state == NULL) return closed(call);
    ssize_t count = send(socket_state->fd, bytes + written, length - written, NET_SEND_FLAGS);
    if (count > 0) {
      written += (size_t)count;
      continue;
    }
    if (count < 0 && errno == EINTR) continue;
    if (count < 0 && errno != EAGAIN && errno != EWOULDBLOCK) return errno_failure(call, errno);
    int waited = chimi_binding_wait(call, 0, socket_state->fd, CHIMI_BINDING_WRITABLE);
    if (waited != CHIMI_BINDING_READY) return not_ready(call, waited);
  }
  return chimi_binding_ok(call, chimi_binding_void(call));
}

/* native_finish(socket) -> Result<Void, (Int, Text)>: ends the writing side,
   so the peer reads the end of input; reading goes on. */
ChimiBindingResult *chimi_net_finish(ChimiBindingCall *call) {
  Socket *socket_state = chimi_binding_handle(call, 0, &SOCKET);
  if (socket_state == NULL) return closed(call);
  if (shutdown(socket_state->fd, SHUT_WR) != 0 && errno != ENOTCONN)
    return errno_failure(call, errno);
  return chimi_binding_ok(call, chimi_binding_void(call));
}

/* native_stop_reading(socket) -> Void: ends this side's reading only, so a
   stopped reader leaves the writer working; later reads fail with Closed. */
ChimiBindingResult *chimi_net_stop_reading(ChimiBindingCall *call) {
  Socket *socket_state = chimi_binding_handle(call, 0, &SOCKET);
  if (socket_state != NULL && !socket_state->reading_stopped) {
    socket_state->reading_stopped = 1;
    (void)shutdown(socket_state->fd, SHUT_RD);
  }
  return chimi_binding_success(call, chimi_binding_void(call));
}

ChimiBindingResult *chimi_net_close(ChimiBindingCall *call) {
  chimi_binding_close_handle(call, 0, &SOCKET);
  return chimi_binding_success(call, chimi_binding_void(call));
}

ChimiBindingResult *chimi_net_close_listener(ChimiBindingCall *call) {
  chimi_binding_close_handle(call, 0, &LISTEN_SOCKET);
  return chimi_binding_success(call, chimi_binding_void(call));
}

/* ---- Addresses ------------------------------------------------------ */

static ChimiBindingResult *address_of(ChimiBindingCall *call, const ChimiBindingType *type,
                                      int peer) {
  Socket *socket_state = chimi_binding_handle(call, 0, type);
  if (socket_state == NULL) return closed(call);
  struct sockaddr_storage address;
  socklen_t size = sizeof(address);
  int status = peer ? getpeername(socket_state->fd, (struct sockaddr *)&address, &size)
                    : getsockname(socket_state->fd, (struct sockaddr *)&address, &size);
  if (status != 0) return errno_failure(call, errno);
  char host[NI_MAXHOST];
  char service[NI_MAXSERV];
  status = getnameinfo((struct sockaddr *)&address, size, host, sizeof(host), service,
                       sizeof(service), NI_NUMERICHOST | NI_NUMERICSERV);
  if (status != 0) return chimi_binding_failure(call, NET_FAILED, gai_strerror(status));
  ChimiBindingValue *parts[] = {
    chimi_binding_make_text(call, host, strlen(host)),
    chimi_binding_make_integer(call, strtoll(service, NULL, 10)),
  };
  return chimi_binding_ok(call, chimi_binding_make_tuple(call, parts, 2));
}

/* native_local_address(socket) -> Result<(Text, Int), (Int, Text)>. */
ChimiBindingResult *chimi_net_local_address(ChimiBindingCall *call) {
  return address_of(call, &SOCKET, 0);
}

/* native_peer_address(socket) -> Result<(Text, Int), (Int, Text)>. */
ChimiBindingResult *chimi_net_peer_address(ChimiBindingCall *call) {
  return address_of(call, &SOCKET, 1);
}

/* native_listener_address(listener) -> Result<(Text, Int), (Int, Text)>. */
ChimiBindingResult *chimi_net_listener_address(ChimiBindingCall *call) {
  return address_of(call, &LISTEN_SOCKET, 0);
}

/* native_allow_real_network() -> Void: test-only (T4401), so a program cannot
   reach `Net.host_for_tests`. Does nothing. */
ChimiBindingResult *chimi_net_allow_real_network(ChimiBindingCall *call) {
  return chimi_binding_success(call, chimi_binding_void(call));
}
