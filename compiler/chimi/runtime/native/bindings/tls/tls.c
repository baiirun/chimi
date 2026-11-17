/* std/tls: the binding over the session engine (tls_session.c). A session is
   a handle; std/tls.chimi moves its ciphertext through the wrapped
   connection's streams, so no call here waits. A connection's reader and
   writer are used from different tasks, so each call on a session holds the
   session's lock. Today one carrier never interleaves two calls anyway; the
   lock covers a session's own state only, and parallel carriers
   (parallel-scheduler.md) will also need mbedTLS's threading support for
   the PSA state that sessions share. */
#define _POSIX_C_SOURCE 200809L
#include "chimi_binding.h"

#include <pthread.h>
#include <stdlib.h>
#include <string.h>

#include "tls_session.h"

/* mbedTLS reads at most one record (16 KiB of plaintext) per call. */
enum { TLS_CHUNK = 16 * 1024 };

/* A session and the lock its calls hold. */
typedef struct {
  pthread_mutex_t lock;
  TlsSession *session;
} Locked;

static void release_session(void *state) {
  Locked *locked = state;
  tls_free_session(locked->session);
  pthread_mutex_destroy(&locked->lock);
  free(locked);
}

static const ChimiBindingType SESSION = {"TlsSession", release_session};

/* The session argument at `index`, locked; NULL when it is closed or was
   handed to another handle. */
static Locked *acquire(ChimiBindingCall *call, uint32_t index) {
  Locked *locked = chimi_binding_handle(call, index, &SESSION);
  if (!locked) return NULL;
  pthread_mutex_lock(&locked->lock);
  if (!locked->session) {
    pthread_mutex_unlock(&locked->lock);
    return NULL;
  }
  return locked;
}

static ChimiBindingResult *unlocked(Locked *locked, ChimiBindingResult *result) {
  pthread_mutex_unlock(&locked->lock);
  return result;
}

static ChimiBindingResult *closed(ChimiBindingCall *call) {
  return chimi_binding_failure(call, TLS_CLOSED, "the TLS connection is closed");
}

static ChimiBindingResult *session_failure(ChimiBindingCall *call, const TlsSession *session) {
  return chimi_binding_failure(call, session->failure, session->message);
}

static ChimiBindingResult *made(ChimiBindingCall *call, TlsSession *session, int code,
                                const char *message) {
  if (!session) return chimi_binding_failure(call, code, message);
  Locked *locked = malloc(sizeof *locked);
  if (!locked || pthread_mutex_init(&locked->lock, NULL) != 0) {
    free(locked);
    tls_free_session(session);
    return chimi_binding_failure(call, TLS_INVALID, "out of memory");
  }
  locked->session = session;
  ChimiBindingValue *handle = chimi_binding_make_handle(call, &SESSION, locked);
  if (!handle) return chimi_binding_failure(call, TLS_INVALID, "too many open TLS sessions");
  return chimi_binding_ok(call, handle);
}

/* native_client(roots: Bytes, host: Text): trusts the PEM `roots`. */
ChimiBindingResult *chimi_tls_client(ChimiBindingCall *call) {
  const unsigned char *roots;
  size_t roots_length;
  const char *host;
  size_t host_length;
  if (!chimi_binding_bytes(call, 0, &roots, &roots_length) ||
      !chimi_binding_text(call, 1, &host, &host_length)) {
    return chimi_binding_defect(call, "tls: client takes roots and a host");
  }
  if (host_length == 0 || strlen(host) != host_length) {
    return chimi_binding_failure(call, TLS_INVALID, "the host name is empty or holds U+0000");
  }
  /* Empty bytes may come with no pointer, which would mean the system's. */
  static const unsigned char none[1] = {0};
  if (!roots) roots = none;
  int code = 0;
  char message[256] = "";
  TlsSession *session = tls_client_session(roots, roots_length, host, &code, message,
                                           sizeof message);
  return made(call, session, code, message);
}

/* native_client_system(host: Text): the system's roots. */
ChimiBindingResult *chimi_tls_client_system(ChimiBindingCall *call) {
  const char *host;
  size_t host_length;
  if (!chimi_binding_text(call, 0, &host, &host_length)) {
    return chimi_binding_defect(call, "tls: client_system takes a host");
  }
  if (host_length == 0 || strlen(host) != host_length) {
    return chimi_binding_failure(call, TLS_INVALID, "the host name is empty or holds U+0000");
  }
  int code = 0;
  char message[256] = "";
  TlsSession *session = tls_client_session(NULL, 0, host, &code, message, sizeof message);
  return made(call, session, code, message);
}

/* native_server(certificate: Bytes, key: Bytes) */
ChimiBindingResult *chimi_tls_server(ChimiBindingCall *call) {
  const unsigned char *certificate, *key;
  size_t certificate_length, key_length;
  if (!chimi_binding_bytes(call, 0, &certificate, &certificate_length) ||
      !chimi_binding_bytes(call, 1, &key, &key_length)) {
    return chimi_binding_defect(call, "tls: server takes a certificate and a key");
  }
  int code = 0;
  char message[256] = "";
  TlsSession *session = tls_server_session(certificate, certificate_length, key, key_length,
                                           &code, message, sizeof message);
  return made(call, session, code, message);
}

/* native_check_roots(pem: Bytes) -> Result<Int, (Int, Text)> */
ChimiBindingResult *chimi_tls_check_roots(ChimiBindingCall *call) {
  const unsigned char *pem;
  size_t length;
  if (!chimi_binding_bytes(call, 0, &pem, &length)) {
    return chimi_binding_defect(call, "tls: check_roots takes bytes");
  }
  char message[256] = "";
  int count = tls_check_roots(pem, length, message, sizeof message);
  if (count < 0) return chimi_binding_failure(call, TLS_INVALID, message);
  return chimi_binding_ok(call, chimi_binding_make_integer(call, count));
}

/* native_check_identity(certificate: Bytes, key: Bytes) -> Result<Void, _>:
   builds a server session and drops it, so a bad identity fails where it is
   made rather than at the first handshake. */
ChimiBindingResult *chimi_tls_check_identity(ChimiBindingCall *call) {
  const unsigned char *certificate, *key;
  size_t certificate_length, key_length;
  if (!chimi_binding_bytes(call, 0, &certificate, &certificate_length) ||
      !chimi_binding_bytes(call, 1, &key, &key_length)) {
    return chimi_binding_defect(call, "tls: check_identity takes a certificate and a key");
  }
  int code = 0;
  char message[256] = "";
  TlsSession *session = tls_server_session(certificate, certificate_length, key, key_length,
                                           &code, message, sizeof message);
  if (!session) return chimi_binding_failure(call, code, message);
  tls_free_session(session);
  return chimi_binding_ok(call, chimi_binding_void(call));
}

/* native_handshake(session) -> Result<Bool, _>: True once complete, False when
   it needs the peer's next bytes (after the pending ones are sent). */
ChimiBindingResult *chimi_tls_handshake(ChimiBindingCall *call) {
  Locked *locked = acquire(call, 0);
  if (!locked) return closed(call);
  int result = tls_handshake(locked->session);
  if (result < 0) return unlocked(locked, session_failure(call, locked->session));
  return unlocked(locked, chimi_binding_ok(call, chimi_binding_make_boolean(call, result)));
}

/* native_feed(session, bytes) -> Result<Void, _> */
ChimiBindingResult *chimi_tls_feed(ChimiBindingCall *call) {
  const unsigned char *bytes;
  size_t length;
  if (!chimi_binding_bytes(call, 1, &bytes, &length)) {
    return chimi_binding_defect(call, "tls: feed takes bytes");
  }
  Locked *locked = acquire(call, 0);
  if (!locked) return closed(call);
  if (tls_feed(locked->session, bytes, length) != 0) {
    return unlocked(locked, chimi_binding_failure(call, TLS_INVALID, "out of memory"));
  }
  return unlocked(locked, chimi_binding_ok(call, chimi_binding_void(call)));
}

/* native_take(session) -> Bytes: the ciphertext to send, now empty here. A
   closed session has none. */
ChimiBindingResult *chimi_tls_take(ChimiBindingCall *call) {
  Locked *locked = acquire(call, 0);
  if (!locked) return chimi_binding_success(call, chimi_binding_make_bytes(call, NULL, 0));
  size_t length;
  const unsigned char *pending = tls_pending(locked->session, &length);
  ChimiBindingValue *bytes = chimi_binding_make_bytes(call, pending, length);
  tls_took(locked->session);
  return unlocked(locked, chimi_binding_success(call, bytes));
}

/* native_send(session, bytes) -> Result<Void, _> */
ChimiBindingResult *chimi_tls_send(ChimiBindingCall *call) {
  const unsigned char *bytes;
  size_t length;
  if (!chimi_binding_bytes(call, 1, &bytes, &length)) {
    return chimi_binding_defect(call, "tls: send takes bytes");
  }
  Locked *locked = acquire(call, 0);
  if (!locked) return closed(call);
  if (tls_send(locked->session, bytes, length) != 0) {
    return unlocked(locked, session_failure(call, locked->session));
  }
  return unlocked(locked, chimi_binding_ok(call, chimi_binding_void(call)));
}

/* native_receive(session) -> Result<(Int, Bytes), _>: (0, plaintext),
   (1, empty) when it needs the peer's next bytes, (2, empty) once the peer
   sent close_notify. */
ChimiBindingResult *chimi_tls_receive(ChimiBindingCall *call) {
  unsigned char *buffer = malloc(TLS_CHUNK);
  if (!buffer) return chimi_binding_failure(call, TLS_INVALID, "out of memory");
  Locked *locked = acquire(call, 0);
  if (!locked) {
    free(buffer);
    return closed(call);
  }
  size_t length = 0;
  int status = tls_receive(locked->session, buffer, TLS_CHUNK, &length);
  if (status < 0) {
    free(buffer);
    return unlocked(locked, session_failure(call, locked->session));
  }
  ChimiBindingValue *items[2] = {
      chimi_binding_make_integer(call, status),
      chimi_binding_make_bytes(call, buffer, status == TLS_DATA ? length : 0),
  };
  free(buffer);
  return unlocked(locked, chimi_binding_ok(call, chimi_binding_make_tuple(call, items, 2)));
}

/* native_close_notify(session) -> Void: queues close_notify for `take`. */
ChimiBindingResult *chimi_tls_close_notify(ChimiBindingCall *call) {
  Locked *locked = acquire(call, 0);
  if (!locked) return chimi_binding_success(call, chimi_binding_void(call));
  tls_close_notify(locked->session);
  return unlocked(locked, chimi_binding_success(call, chimi_binding_void(call)));
}

/* native_handoff(session) -> Result<TlsSession, _>: a new handle owning the
   session, leaving the argument's handle empty (closed to later calls, and
   its release frees nothing). std/tls.chimi makes a session inside
   Resource.scoped, whose release frees it if the handshake fails or is
   cancelled, and hands it off once the handshake is done. */
ChimiBindingResult *chimi_tls_handoff(ChimiBindingCall *call) {
  Locked *locked = acquire(call, 0);
  if (!locked) return closed(call);
  TlsSession *session = locked->session;
  locked->session = NULL;
  pthread_mutex_unlock(&locked->lock);
  return made(call, session, TLS_INVALID, "");
}

/* native_free(session) -> Void: later calls see the session closed. */
ChimiBindingResult *chimi_tls_free(ChimiBindingCall *call) {
  chimi_binding_close_handle(call, 0, &SESSION);
  return chimi_binding_success(call, chimi_binding_void(call));
}

/* native_live_sessions() -> Int: test-only, through Tls.live_sessions_for_tests. */
ChimiBindingResult *chimi_tls_live_sessions(ChimiBindingCall *call) {
  return chimi_binding_success(call, chimi_binding_make_integer(call, tls_live_sessions()));
}
