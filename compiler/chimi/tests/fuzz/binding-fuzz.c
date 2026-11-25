/* Fuzz targets for the binding packages' C shims: the code that takes bytes
   from a peer, a file or a program and hands them to vendored libraries.
   Each target turns one input into a canonical output line and checks its
   invariants, aborting on a broken one; as in runtime-fuzz.c, the output is
   the contract a Rust replacement must keep (CHI-305).

   - tls-client: the input is a server's reply to a client's ClientHello.
   - tls-server: the input is a client's first flight to a server.
   - tls-records: the input is a script for a real handshake's two ends:
     the server sends chunks of the input, the ciphertext moves in pieces of
     varying size, and the client reads, and must read exactly what was sent.
   - tls-roots: the input is a PEM file of roots (tls_check_roots).
   - scram: the input is a password, a salt, an iteration count and an auth
     message, for std/postgres's SCRAM-SHA-256.
   - sqlite: the input is a statement for std/sqlite's prepare, which must
     refuse a second statement exactly when SQLite finds one (or an error,
     or a NUL) after the first.

   The harness is the binding host, in miniature: it implements the binding
   API (chimi_binding.h) over plain malloc memory, so the shims' own entry
   points run as a program calls them. It links the packages' archives,
   built with the same commands `chimi build` uses (tests/runtime_fuzz.rs,
   scripts/fuzz-runtime.sh). The TLS targets read the fixture certificates
   from the directory CHIMI_FUZZ_TLS_FIXTURES names (tests/fixtures/tls).

   The shims not fuzzed parse nothing a peer controls: net and exec pass
   their arguments to the operating system, terminal reads no input, and the
   postgres gate only stores bytes. */
#define _POSIX_C_SOURCE 200809L
#define FUZZ_NAME "binding-fuzz"
#include "fuzz-output.h"

#include "chimi_binding.h"
#include "tls_session.h"
#include "sqlite3.h"

/* The binding API, over values that live until the input ends. */
enum { V_VOID, V_INTEGER, V_BOOLEAN, V_TEXT, V_BYTES, V_NONE, V_SOME, V_TUPLE, V_LIST, V_HANDLE };
typedef struct {
  const ChimiBindingType *type;
  void *state;
  bool open;
} FuzzHandle;
struct ChimiBindingValue {
  int kind;
  int64_t integer;
  unsigned char *bytes; /* NUL-terminated for text */
  size_t length;
  ChimiBindingValue **items;
  uint32_t count;
  FuzzHandle *handle;
};
struct ChimiBindingCall {
  ChimiBindingValue **arguments;
  uint32_t count;
};
enum { R_SUCCESS, R_OK, R_FAILURE, R_DEFECT, R_CANCELLED };
struct ChimiBindingResult {
  int kind;
  ChimiBindingValue *value;
  int64_t code;
  char message[512];
};

/* Everything allocated for the current input, freed by fz_run_one. */
static void **fz_allocations;
static size_t fz_allocation_count, fz_allocation_capacity;
static FuzzHandle **fz_handles;
static size_t fz_handle_count, fz_handle_capacity;

static void *fz_track(void *pointer) {
  if (!pointer) abort();
  if (fz_allocation_count == fz_allocation_capacity) {
    fz_allocation_capacity = fz_allocation_capacity ? fz_allocation_capacity * 2 : 64;
    fz_allocations = realloc(fz_allocations, fz_allocation_capacity * sizeof *fz_allocations);
    if (!fz_allocations) abort();
  }
  fz_allocations[fz_allocation_count++] = pointer;
  return pointer;
}
static void *fz_alloc(size_t size) { return fz_track(calloc(1, size ? size : 1)); }
static void fz_release_handle(FuzzHandle *handle) {
  if (handle->open) {
    handle->open = false;
    handle->type->release(handle->state);
  }
}
static void fz_free_input(void) {
  for (size_t index = 0; index < fz_handle_count; ++index) fz_release_handle(fz_handles[index]);
  fz_handle_count = 0;
  for (size_t index = 0; index < fz_allocation_count; ++index) free(fz_allocations[index]);
  fz_allocation_count = 0;
}

static ChimiBindingValue *fz_value(int kind) {
  ChimiBindingValue *value = fz_alloc(sizeof *value);
  value->kind = kind;
  return value;
}
static ChimiBindingValue *fz_bytes_value(int kind, const void *bytes, size_t length) {
  ChimiBindingValue *value = fz_value(kind);
  value->bytes = fz_alloc(length + 1);
  if (length) memcpy(value->bytes, bytes, length);
  value->length = length;
  return value;
}
static ChimiBindingValue *fz_argument(const ChimiBindingCall *call, uint32_t index, int kind) {
  return index < call->count && call->arguments[index]->kind == kind ? call->arguments[index] : NULL;
}

uint32_t chimi_binding_argument_count(const ChimiBindingCall *call) { return call->count; }
int chimi_binding_integer(const ChimiBindingCall *call, uint32_t index, int64_t *out) {
  ChimiBindingValue *value = fz_argument(call, index, V_INTEGER);
  if (value) *out = value->integer;
  return value != NULL;
}
int chimi_binding_boolean(const ChimiBindingCall *call, uint32_t index, int *out) {
  ChimiBindingValue *value = fz_argument(call, index, V_BOOLEAN);
  if (value) *out = (int)value->integer;
  return value != NULL;
}
int chimi_binding_text(const ChimiBindingCall *call, uint32_t index, const char **bytes,
                       size_t *length) {
  ChimiBindingValue *value = fz_argument(call, index, V_TEXT);
  if (value) *bytes = (const char *)value->bytes, *length = value->length;
  return value != NULL;
}
int chimi_binding_bytes(const ChimiBindingCall *call, uint32_t index,
                        const unsigned char **bytes, size_t *length) {
  ChimiBindingValue *value = fz_argument(call, index, V_BYTES);
  /* The runtime gives NULL for empty bytes; the shims must cope. */
  if (value) *bytes = value->length ? value->bytes : NULL, *length = value->length;
  return value != NULL;
}
void *chimi_binding_handle(const ChimiBindingCall *call, uint32_t index,
                           const ChimiBindingType *type) {
  ChimiBindingValue *value = fz_argument(call, index, V_HANDLE);
  FUZZ_CHECK(value && value->handle->type == type, "argument %u is not a %s handle", index,
             type->name);
  return value->handle->open ? value->handle->state : NULL;
}
ChimiBindingValue *chimi_binding_void(ChimiBindingCall *call) {
  (void)call;
  return fz_value(V_VOID);
}
ChimiBindingValue *chimi_binding_make_integer(ChimiBindingCall *call, int64_t integer) {
  (void)call;
  ChimiBindingValue *value = fz_value(V_INTEGER);
  value->integer = integer;
  return value;
}
ChimiBindingValue *chimi_binding_make_boolean(ChimiBindingCall *call, int boolean) {
  ChimiBindingValue *value = chimi_binding_make_integer(call, boolean != 0);
  value->kind = V_BOOLEAN;
  return value;
}
ChimiBindingValue *chimi_binding_make_text(ChimiBindingCall *call, const char *bytes,
                                           size_t length) {
  (void)call;
  return fz_bytes_value(V_TEXT, bytes, length);
}
ChimiBindingValue *chimi_binding_make_bytes(ChimiBindingCall *call, const unsigned char *bytes,
                                            size_t length) {
  (void)call;
  return fz_bytes_value(V_BYTES, bytes, length);
}
ChimiBindingValue *chimi_binding_none(ChimiBindingCall *call) {
  (void)call;
  return fz_value(V_NONE);
}
static ChimiBindingValue *fz_items(int kind, ChimiBindingValue **items, uint32_t count) {
  ChimiBindingValue *value = fz_value(kind);
  value->items = fz_alloc(count * sizeof *items);
  if (count) memcpy(value->items, items, count * sizeof *items);
  value->count = count;
  return value;
}
ChimiBindingValue *chimi_binding_some(ChimiBindingCall *call, ChimiBindingValue *item) {
  (void)call;
  return fz_items(V_SOME, &item, 1);
}
ChimiBindingValue *chimi_binding_make_tuple(ChimiBindingCall *call, ChimiBindingValue **items,
                                            uint32_t length) {
  (void)call;
  return fz_items(V_TUPLE, items, length);
}
ChimiBindingValue *chimi_binding_make_list(ChimiBindingCall *call, ChimiBindingValue **items,
                                           uint32_t length) {
  (void)call;
  return fz_items(V_LIST, items, length);
}
ChimiBindingValue *chimi_binding_make_handle(ChimiBindingCall *call, const ChimiBindingType *type,
                                             void *state) {
  (void)call;
  FuzzHandle *handle = fz_alloc(sizeof *handle);
  *handle = (FuzzHandle){type, state, true};
  if (fz_handle_count == fz_handle_capacity) {
    fz_handle_capacity = fz_handle_capacity ? fz_handle_capacity * 2 : 16;
    fz_handles = realloc(fz_handles, fz_handle_capacity * sizeof *fz_handles);
    if (!fz_handles) abort();
  }
  fz_handles[fz_handle_count++] = handle;
  ChimiBindingValue *value = fz_value(V_HANDLE);
  value->handle = handle;
  return value;
}
void chimi_binding_close_handle(ChimiBindingCall *call, uint32_t index,
                                const ChimiBindingType *type) {
  ChimiBindingValue *value = fz_argument(call, index, V_HANDLE);
  if (value && value->handle->type == type) fz_release_handle(value->handle);
}
static ChimiBindingResult *fz_result(int kind, ChimiBindingValue *value) {
  ChimiBindingResult *result = fz_alloc(sizeof *result);
  result->kind = kind;
  result->value = value;
  return result;
}
ChimiBindingResult *chimi_binding_success(ChimiBindingCall *call, ChimiBindingValue *value) {
  (void)call;
  return fz_result(R_SUCCESS, value);
}
ChimiBindingResult *chimi_binding_ok(ChimiBindingCall *call, ChimiBindingValue *value) {
  (void)call;
  return fz_result(R_OK, value);
}
ChimiBindingResult *chimi_binding_failure(ChimiBindingCall *call, int64_t code,
                                          const char *message) {
  (void)call;
  ChimiBindingResult *result = fz_result(R_FAILURE, NULL);
  result->code = code;
  snprintf(result->message, sizeof result->message, "%s", message);
  return result;
}
ChimiBindingResult *chimi_binding_defect(ChimiBindingCall *call, const char *message) {
  ChimiBindingResult *result = chimi_binding_failure(call, 0, message);
  result->kind = R_DEFECT;
  return result;
}
ChimiBindingResult *chimi_binding_cancelled(ChimiBindingCall *call) {
  (void)call;
  return fz_result(R_CANCELLED, NULL);
}
int chimi_binding_is_cancelled(const ChimiBindingCall *call) {
  (void)call;
  return 0;
}

/* Calls `function` with `count` arguments. A defect or cancellation is a
   broken invariant: the arguments are the ones the Chimi wrapper passes. */
static ChimiBindingResult *fz_call(ChimiBindingFunction function, const char *name,
                                   ChimiBindingValue **arguments, uint32_t count) {
  ChimiBindingCall call = {arguments, count};
  ChimiBindingResult *result = function(&call);
  FUZZ_CHECK(result && (result->kind == R_SUCCESS || result->kind == R_OK ||
                        result->kind == R_FAILURE),
             "%s ended with result kind %d: %s", name, result ? result->kind : -1,
             result ? result->message : "(none)");
  return result;
}
static void out_failure(FuzzOutput *out, const ChimiBindingResult *result) {
  out_word(out, "err ");
  out_integer(out, result->code);
  out_word(out, " ");
  out_quoted(out, result->message, strlen(result->message));
}
static void out_hex(FuzzOutput *out, const unsigned char *bytes, size_t length) {
  for (size_t at = 0; at < length; ++at) {
    char pair[3];
    snprintf(pair, sizeof pair, "%02x", bytes[at]);
    out_bytes(out, pair, 2);
  }
}

/* TLS: the fixture certificates, read once. */
typedef struct { unsigned char *bytes; size_t length; } FuzzFile;
static FuzzFile fz_fixture(const char *name) {
  const char *directory = getenv("CHIMI_FUZZ_TLS_FIXTURES");
  char path[4096];
  snprintf(path, sizeof path, "%s/%s", directory ? directory : "tests/fixtures/tls", name);
  FILE *file = fopen(path, "rb");
  if (!file) {
    fprintf(stderr, FUZZ_NAME ": cannot read %s (set CHIMI_FUZZ_TLS_FIXTURES)\n", path);
    exit(2);
  }
  FuzzFile read = {0};
  size_t capacity = 0;
  for (;;) {
    if (read.length == capacity) {
      capacity = capacity ? capacity * 2 : 4096;
      read.bytes = realloc(read.bytes, capacity);
      if (!read.bytes) abort();
    }
    size_t got = fread(read.bytes + read.length, 1, capacity - read.length, file);
    if (got == 0) break;
    read.length += got;
  }
  fclose(file);
  return read;
}
static FuzzFile fz_roots, fz_certificate, fz_key;
static void fz_load_fixtures(void) {
  if (fz_roots.bytes) return;
  fz_roots = fz_fixture("ca.pem");
  fz_certificate = fz_fixture("server.pem");
  fz_key = fz_fixture("server.key");
}
static TlsSession *fz_client(void) {
  fz_load_fixtures();
  int code = 0;
  char message[256] = "";
  TlsSession *client = tls_client_session(fz_roots.bytes, fz_roots.length, "localhost", &code,
                                          message, sizeof message);
  FUZZ_CHECK(client, "the fixture client did not start: %d %s", code, message);
  return client;
}
static TlsSession *fz_server(void) {
  fz_load_fixtures();
  int code = 0;
  char message[256] = "";
  TlsSession *server = tls_server_session(fz_certificate.bytes, fz_certificate.length,
                                          fz_key.bytes, fz_key.length, &code, message,
                                          sizeof message);
  FUZZ_CHECK(server, "the fixture server did not start: %d %s", code, message);
  return server;
}
static void out_session_failure(FuzzOutput *out, const TlsSession *session) {
  out_word(out, "failed ");
  out_integer(out, session->failure);
  out_word(out, " ");
  out_quoted(out, session->message, strlen(session->message));
}
/* Moves `piece` bytes (all when 0) of the ciphertext `from` has written to
   `to`. */
static void fz_move(TlsSession *from, TlsSession *to, size_t piece) {
  size_t length = 0;
  const unsigned char *pending = tls_pending(from, &length);
  for (size_t at = 0; at < length;) {
    size_t step = piece && piece < length - at ? piece : length - at;
    FUZZ_CHECK(tls_feed(to, pending + at, step) == 0, "feeding %zu bytes failed", step);
    at += step;
  }
  tls_took(from);
}

/* Runs the handshake of a session that was just fed `data` until it stops
   making progress, and prints where it stopped. */
static void fz_handshake_input(TlsSession *session, const uint8_t *data, size_t size,
                               FuzzOutput *out) {
  /* The first byte, when there is one, splits the input in two feeds, so
     records cut across feeds are read as one. */
  size_t split = size ? (size_t)data[0] % (size + 1) : 0;
  int state = 0;
  for (int feed = 0; feed < 2 && state == 0; ++feed) {
    const uint8_t *piece = feed == 0 ? data : data + split;
    size_t length = feed == 0 ? split : size - split;
    if (length) FUZZ_CHECK(tls_feed(session, piece, length) == 0, "feeding the input failed");
    state = tls_handshake(session);
    tls_took(session);
  }
  if (state < 0) {
    FUZZ_CHECK(session->failure != 0, "a failed handshake recorded no failure");
    out_session_failure(out, session);
  } else {
    FUZZ_CHECK(session->failure == 0, "a running handshake recorded failure %d", session->failure);
    out_word(out, state == 1 ? "complete" : "needs input");
  }
}

static void fuzz_tls_client(const uint8_t *data, size_t size, FuzzOutput *out) {
  TlsSession *client = fz_client();
  FUZZ_CHECK(tls_handshake(client) == 0, "the client did not wait for the server");
  tls_took(client);
  fz_handshake_input(client, data, size, out);
  tls_free_session(client);
}

static void fuzz_tls_server(const uint8_t *data, size_t size, FuzzOutput *out) {
  TlsSession *server = fz_server();
  fz_handshake_input(server, data, size, out);
  tls_free_session(server);
}

/* A script of operations, one per byte, on a session already handshaken:
   bits 0-1 choose the operation and the rest is its size.
     0  the server sends the next (size + 1) bytes of the script as data
     1  the ciphertext moves to the client in pieces of (size + 1) bytes
     2  the client reads into a buffer of (size + 1) bytes
     3  the server sends close_notify (size ignored); later sends stop
   At the end all ciphertext moves and the client reads until it needs input
   or sees the close. What it read must be exactly what was sent. */
static void fuzz_tls_records(const uint8_t *data, size_t size, FuzzOutput *out) {
  TlsSession *client = fz_client(), *server = fz_server();
  int client_done = 0, server_done = 0;
  for (int round = 0; round < 20 && !(client_done && server_done); ++round) {
    if (!client_done) client_done = tls_handshake(client);
    fz_move(client, server, 0);
    if (!server_done) server_done = tls_handshake(server);
    fz_move(server, client, 0);
    FUZZ_CHECK(client_done >= 0 && server_done >= 0, "the fixture handshake failed: %s %s",
               client->message, server->message);
  }
  FUZZ_CHECK(client_done == 1 && server_done == 1, "the fixture handshake did not finish");
  FuzzOutput sent = {0}, received = {0};
  bool closed = false, peer_closed = false;
  long reads = 0;
  unsigned char buffer[64];
  for (size_t at = 0; at < size; ++at) {
    uint8_t op = data[at] & 3;
    size_t amount = (size_t)(data[at] >> 2) + 1;
    if (op == 0 && !closed) {
      size_t take = amount < size - at - 1 ? amount : size - at - 1;
      FUZZ_CHECK(tls_send(server, data + at + 1, take) == 0, "send failed: %s", server->message);
      out_bytes(&sent, (const char *)data + at + 1, take);
      at += take;
    } else if (op == 1) {
      fz_move(server, client, amount);
    } else if (op == 2 && !peer_closed) {
      size_t length = 0;
      int state = tls_receive(client, buffer, amount < sizeof buffer ? amount : sizeof buffer,
                              &length);
      FUZZ_CHECK(state >= 0, "receive failed: %s", client->message);
      if (state == TLS_DATA) out_bytes(&received, (const char *)buffer, length), ++reads;
      if (state == TLS_PEER_CLOSED) peer_closed = true;
    } else if (op == 3 && !closed) {
      tls_close_notify(server);
      closed = true;
    }
  }
  fz_move(server, client, 0);
  for (int state = TLS_DATA; state == TLS_DATA && !peer_closed;) {
    size_t length = 0;
    state = tls_receive(client, buffer, sizeof buffer, &length);
    FUZZ_CHECK(state >= 0, "receive failed: %s", client->message);
    if (state == TLS_DATA) out_bytes(&received, (const char *)buffer, length), ++reads;
    if (state == TLS_PEER_CLOSED) peer_closed = true;
  }
  FUZZ_CHECK(received.length == sent.length &&
                 (sent.length == 0 || memcmp(received.data, sent.data, sent.length) == 0),
             "the client read %zu bytes of the %zu sent, or other bytes", received.length,
             sent.length);
  FUZZ_CHECK(peer_closed == closed, "the client %s the close the server %s",
             peer_closed ? "saw" : "missed", closed ? "sent" : "never sent");
  out_word(out, "read ");
  out_integer(out, (int64_t)received.length);
  out_word(out, peer_closed ? " bytes, then the close" : " bytes, still open");
  free(sent.data);
  free(received.data);
  tls_free_session(client);
  tls_free_session(server);
}

static void fuzz_tls_roots(const uint8_t *data, size_t size, FuzzOutput *out) {
  char message[256] = "";
  int count = tls_check_roots(data, size, message, sizeof message);
  if (count < 0) {
    out_word(out, "invalid ");
    out_quoted(out, message, strlen(message));
  } else {
    FUZZ_CHECK(count > 0, "tls_check_roots accepted no roots");
    out_word(out, "roots ");
    out_integer(out, count);
  }
}

/* The input is framed by lengths, so any byte may appear in any field: the
   password's length, the salt's length, the iteration count in three bytes
   (big-endian), the password, the salt, and the auth message (the rest).
   A count past 4096 only costs time, so the target lowers one up to the
   shim's limit of a million to 4096, keeping the shim's range check. */
ChimiBindingResult *chimi_tls_postgres_scram(ChimiBindingCall *call);
static void fuzz_scram(const uint8_t *data, size_t size, FuzzOutput *out) {
  if (size < 5 || size - 5 < (size_t)data[0] + data[1]) {
    out_word(out, "skipped: too short for its lengths");
    return;
  }
  size_t password = data[0], salt = data[1];
  int64_t iterations = (int64_t)data[2] << 16 | (int64_t)data[3] << 8 | data[4];
  if (iterations > 4096 && iterations <= 1000000) iterations = 4096;
  const uint8_t *fields = data + 5;
  ChimiBindingValue *arguments[4] = {
      chimi_binding_make_bytes(NULL, fields, password),
      chimi_binding_make_bytes(NULL, fields + password, salt),
      chimi_binding_make_integer(NULL, iterations),
      chimi_binding_make_bytes(NULL, fields + password + salt, size - 5 - password - salt),
  };
  ChimiBindingResult *result = fz_call(chimi_tls_postgres_scram, "scram", arguments, 4);
  if (result->kind == R_FAILURE) {
    out_failure(out, result);
    return;
  }
  ChimiBindingValue *pair = result->value;
  FUZZ_CHECK(result->kind == R_OK && pair->kind == V_TUPLE && pair->count == 2 &&
                 pair->items[0]->kind == V_BYTES && pair->items[0]->length == 32 &&
                 pair->items[1]->kind == V_BYTES && pair->items[1]->length == 32,
             "scram did not give two 32-byte values");
  out_word(out, "proof ");
  out_hex(out, pair->items[0]->bytes, 32);
  out_word(out, " signature ");
  out_hex(out, pair->items[1]->bytes, 32);
}

/* Whether SQLite finds a statement in `sql`: 1 for one, 0 for none, -1 for
   an error. */
static int fz_sqlite_finds(sqlite3 *db, const char *sql, size_t length) {
  sqlite3_stmt *statement = NULL;
  int code = sqlite3_prepare_v2(db, sql, (int)length, &statement, NULL);
  sqlite3_finalize(statement);
  return code != SQLITE_OK ? -1 : statement != NULL;
}

ChimiBindingResult *chimi_sqlite_open(ChimiBindingCall *call);
ChimiBindingResult *chimi_sqlite_prepare(ChimiBindingCall *call);
ChimiBindingResult *chimi_sqlite_parameter_count(ChimiBindingCall *call);
ChimiBindingResult *chimi_sqlite_finalize(ChimiBindingCall *call);
ChimiBindingResult *chimi_sqlite_close(ChimiBindingCall *call);
enum { FUZZ_SQLITE_MANY_STATEMENTS = -2 };
static void fuzz_sqlite(const uint8_t *data, size_t size, FuzzOutput *out) {
  /* The shim takes Text, which is valid UTF-8 (the Chimi side checks it). */
  ChimiBindingValue *target = chimi_binding_make_text(NULL, ":memory:", 8);
  ChimiBindingResult *opened = fz_call(chimi_sqlite_open, "open", &target, 1);
  FUZZ_CHECK(opened->kind == R_OK, "opening :memory: failed: %s", opened->message);
  ChimiBindingValue *db = opened->value;
  size_t valid = 0;
  for (bool ok = true; ok && valid < size;) {
    /* SQLite's own check is enough to know where valid UTF-8 stops. */
    uint8_t lead = data[valid];
    size_t width = lead < 0x80 ? 1 : (lead & 0xe0) == 0xc0 ? 2 : (lead & 0xf0) == 0xe0 ? 3
                 : (lead & 0xf8) == 0xf0 ? 4 : 0;
    ok = width && valid + width <= size;
    for (size_t at = 1; ok && at < width; ++at) ok = (data[valid + at] & 0xc0) == 0x80;
    if (ok && width == 2) ok = lead >= 0xc2;
    if (ok && width == 3) {
      uint32_t point = (uint32_t)(lead & 0x0f) << 12 | (uint32_t)(data[valid + 1] & 0x3f) << 6;
      ok = point >= 0x800 && (point < 0xd800 || point > 0xdfff);
    }
    if (ok && width == 4) {
      uint32_t point = (uint32_t)(lead & 0x07) << 18 | (uint32_t)(data[valid + 1] & 0x3f) << 12;
      ok = point >= 0x10000 && point <= 0x10ffff;
    }
    if (ok) valid += width;
  }
  ChimiBindingValue *arguments[2] = {db, chimi_binding_make_text(NULL, (const char *)data, valid)};
  ChimiBindingResult *prepared = fz_call(chimi_sqlite_prepare, "prepare", arguments, 2);
  {
    /* SQLite's view, from a connection of its own: the first statement, and
       whether anything but blanks follows it. */
    sqlite3 *own = NULL;
    FUZZ_CHECK(sqlite3_open(":memory:", &own) == SQLITE_OK, "opening a second :memory: failed");
    const char *sql = (const char *)arguments[1]->bytes, *tail = NULL;
    sqlite3_stmt *statement = NULL;
    int code = sqlite3_prepare_v2(own, sql, (int)valid, &statement, &tail);
    sqlite3_finalize(statement);
    /* SQLite ends the text at a NUL; the shim refuses a text with one. */
    int rest = 0;
    if (code == SQLITE_OK && tail) {
      size_t length = valid - (size_t)(tail - sql);
      rest = memchr(tail, 0, length) ? 1 : fz_sqlite_finds(own, tail, length);
    }
    sqlite3_close(own);
    bool many = prepared->kind == R_FAILURE && prepared->code == FUZZ_SQLITE_MANY_STATEMENTS;
    /* The shim's own connection refuses some statements SQLite's accepts
       (ATTACH, some pragmas); those fail before the tail is read. */
    bool refused = prepared->kind == R_FAILURE && !many;
    if (code == SQLITE_OK && !refused) {
      FUZZ_CHECK(many == (rest != 0),
                 "the shim %s a second statement, but SQLite finds %s after the first",
                 many ? "found" : "found no", rest > 0 ? "one" : rest < 0 ? "an error" : "none");
    }
  }
  if (prepared->kind == R_FAILURE) {
    out_failure(out, prepared);
  } else if (prepared->value->kind == V_NONE) {
    out_word(out, "none");
  } else {
    ChimiBindingValue *statement = prepared->value->items[0];
    ChimiBindingResult *count = fz_call(chimi_sqlite_parameter_count, "parameter_count", &statement, 1);
    out_word(out, "statement, parameters ");
    out_integer(out, count->value->integer);
    fz_call(chimi_sqlite_finalize, "finalize", &statement, 1);
  }
  fz_call(chimi_sqlite_close, "close", &db, 1);
}

static const FuzzTargetEntry FUZZ_TARGETS[] = {
  {"tls-client", fuzz_tls_client}, {"tls-server", fuzz_tls_server},
  {"tls-records", fuzz_tls_records}, {"tls-roots", fuzz_tls_roots},
  {"scram", fuzz_scram}, {"sqlite", fuzz_sqlite},
};

static void fz_run_one(FuzzTarget target, const uint8_t *data, size_t size, FuzzOutput *out) {
  fuzz_input = data;
  fuzz_input_size = size;
  target(data, size, out);
  fz_free_input();
  FUZZ_CHECK(tls_live_sessions() == 0, "%ld TLS sessions outlived the input", tls_live_sessions());
}

#include "fuzz-main.h"
