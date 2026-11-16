# Network Modules

## Status

This is the normative contract for the Phase 7b network modules of the
[language completion plan](../plans/language-completion-plan.md) (CHI-253):
the modules a service uses to talk over the network. It covers `std/net`
(CHI-255), the `std/http` server (CHI-257), and `std/sql`'s
driver-independent layer (CHI-258). `std/tls`, the HTTP client, and the SQL
drivers add their sections as they land.

The words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are normative. The
modules follow the [standard library conventions](standard-library-conventions.md),
the [system module rules](system-modules.md#runner-provided-capabilities) for
runner-provided capabilities, and the
[byte stream contract](byte-streams.md). Their host code is a C binding
package ([G20](standard-modules-and-constants.md)), so a program that reaches
one runs on the native backend; the interpreter refuses it with `E5204`.
Targets are macOS and Linux.

## Goals

- Opening a socket is an authority a function declares with `needs`, as
  reading the environment is, and a test replaces it with a fake.
- A connection is a pair of [byte streams](byte-streams.md), so copying,
  limits, and closing are the ones every other stream uses, and higher
  layers (TLS, HTTP) take and give the same shapes.
- One slow peer never stalls another task: every socket is non-blocking and
  waits through the scheduler's reactor (G21).

## std/net

```text
pub interface Network {
  connect: fn(Text, Int) -> Connection errors NetError
  listen: fn(Text, Int) -> Listener errors NetError
}
pub struct Address        // Net.address(host, port), Net.host, Net.port
pub struct Connection     // Net.reader, Net.writer, Net.local_address, Net.peer_address, Net.close
pub struct Listener       // Net.accept, Net.listener_address, Net.close_listener
pub enum NetError {
  Refused, Unreachable, Reset, Closed, AddressInUse, HostNotFound(host: Text),
  TooManyOpen, TimedOut, Failed(message: Text)
}

pub fn connect(host: Text, port: Int) -> Connection     needs { network: Network }
pub fn listen(host: Text, port: Int) -> Listener        needs { network: Network }
pub fn pair(capacity: Int) -> (Connection, Connection)
pub fn connection(reader, writer, local, peer, close) -> Connection
pub fn listener(accept, address, close) -> Listener
pub fn host_for_tests()                                 // test-only; its result is a provider record
```

### The capability

- `Network` is a runner-provided capability with the fixed need name
  `network` (provider `std/net.system`). Name resolution is part of it: there is
  no separate need for looking a name up.
- Listening needs nothing beyond `network`. A service that only connects and
  one that also listens declare the same need.
- `host_for_tests()` returns the operating system's provider. It MUST be
  test-only (`T4401` from a program), so the only program code that opens a
  real socket is code whose needs reach `main`. A test that reaches `network`
  without `using` is rejected with `T4400`, as for every runner-provided
  capability, so a test opens real sockets only when it asks to.
- A fake pair's read after its peer's task ended without closing fails with
  `Closed`, where a real socket would report `Reset`: the fake cannot tell
  that apart from this side stopping.
- A fake is a record of `connect` and `listen`. `Net.pair`, `Net.connection`,
  `Net.listener`, and `Net.address` build the values a fake returns; the
  structs' fields stay private, as every published struct's do.

### Connections

- A connection's reader gives chunks of at most 64 KiB as they arrive and ends
  once the peer finishes writing. A write sends every byte or fails.
- `Stream.close` on the writer MUST finish only the sending side
  (`shutdown(SHUT_WR)`): the peer reads the end of input, and this side can
  still read the answer.
- `Net.close` releases the socket. A task waiting to read, write, or accept on
  a released socket or listener MUST wake with `NetError.Closed`, and every
  later operation on it fails the same way. Closing again does nothing. Code
  ties a connection or listener to a scope with `Resource.scoped` and
  `Net.close` or `Net.close_listener`; there are no `with_` twins, as there
  are none for streams.
- Every connection sets `TCP_NODELAY`, since a service writes whole messages
  and waits for answers.
- A write to a peer that has gone MUST fail with a `NetError` (`Reset` or
  `Closed` when the peer reset or closed), never raise `SIGPIPE`.
- Stopping the reader (`Stream.stop`, which `Stream.read_all` does past its
  limit) MUST end only this side's reading: the writer keeps working, and
  later reads fail with `NetError.Closed`. A fake pair behaves the same way.
  The writer working does not promise the peer reads the answer: closing a
  socket with unread bytes resets it, which can discard what was just
  written. A close that drains first belongs to the HTTP layer.
- A write after this side's `Stream.close(writer)` MUST fail with
  `NetError.Closed`, on a real socket and a fake alike.
- A connection whose peer reset it before it was accepted is skipped:
  `accept` waits for the next one. On Linux the network errors accept(2)
  reports for a new connection are skipped the same way.
- Deadlines come from `Task.timeout`; no operation takes a timeout parameter.
  Cancelling a task waiting on a socket withdraws its wait.

### Addresses and names

- `host` is a name or a numeric IPv4 or IPv6 address. A numeric address is
  used without a lookup. A name is looked up on a detached thread that blocks
  every signal and touches nothing of the runtime, while the calling task
  waits through the scheduler; at most 8 lookups run at once, and a further
  one fails with `NetError.TooManyOpen`. A task cancelled during a lookup
  stops waiting, and the thread's result is dropped when it finishes; the
  cancelled lookup keeps its place among the 8 until then.
- A host holding U+0000 is `HostNotFound(host)`, not a defect: it can come
  from untrusted input, and no name or address holds one.
- `EAI_AGAIN`, a resolver that cannot answer now, is also `HostNotFound`.
  A caller that wants to retry it wraps the call in `Retry`.
- `connect` tries each address of a name in order and fails with the last
  address's failure.
- `listen` with an empty `host` MUST listen on `0.0.0.0`, every IPv4
  address, on every platform. `"::"` listens on every IPv6 address, and on
  IPv4 too on Linux and macOS by default (`IPV6_V6ONLY` is left unset).
  `listen` on a name binds the first of its addresses that binds, where
  `connect` tries them all. Port 0 picks a free
  port, which `Net.listener_address` reports. A listener sets `SO_REUSEADDR`,
  so a restarted service can listen again on a port whose old connections are
  still closing.
- An `Address` holds a numeric host and a port, as the operating system
  reports them.
- A port outside 0 to 65535 is a defect: it is a programming mistake, not
  something the network reports.

### Failures

| Operating system outcome | `NetError` |
| --- | --- |
| `ECONNREFUSED` | `Refused` |
| `ENETUNREACH`, `EHOSTUNREACH`, `ENETDOWN`, `EHOSTDOWN` | `Unreachable` |
| `ECONNRESET`, `EPIPE`, `ECONNABORTED` | `Reset` |
| the socket was released | `Closed` |
| `EADDRINUSE`, `EADDRNOTAVAIL` | `AddressInUse` |
| a name that does not resolve (`EAI_NONAME`, `EAI_AGAIN`, `EAI_FAIL`) | `HostNotFound(host)` |
| `EMFILE`, `ENFILE`, `ENOBUFS`, `ENOMEM`, `EAI_MEMORY` | `TooManyOpen` |
| `ETIMEDOUT` | `TimedOut` |
| anything else | `Failed(message)`, with the operating system's words |

### Virtual time

Under the virtual clock a task waiting on a socket holds virtual time, as
every host operation in flight does, so an idle `accept` keeps a virtual-clock
test from advancing. A test that combines sockets and timers runs with
`chimi test --clock=real`.

## std/http (server)

The request shape is **provisional**: it follows the recommendation on an
open decision card (a handler is a function from `Request` to `Response`,
routed with `case`). The wire behaviour below does not depend on the answer.

A server takes its settings as an anonymous record, `Http.defaults` updated
with what differs: `Http.serve(listener, { ..Http.defaults, request_timeout:
Duration.seconds(5) }, handle)` (Byron, 2026-11-16). A record rather than a
struct, because a struct's fields are private outside its module, so only a
record can be updated at the call site.

```text
pub enum Method { Get, Head, Post, Put, Patch, Delete, Options, Other(name: Text) }
pub struct Request     // Http.method, Http.path, Http.query, Http.header, Http.headers, Http.body
pub struct Response    // Http.status, Http.response_headers, Http.response_body

pub fn request(method, path, query, headers, body) -> Request        // for testing handlers
pub fn response(status: Int, headers, body: Bytes) -> Response
pub fn text(status: Int, text: Text) -> Response
pub const defaults: { max_head_bytes: Int, max_headers: Int, max_body_bytes: Int,
  request_timeout: Duration, write_timeout: Duration, max_connections: Int }
pub fn serve(listener: Listener, settings, handle: fn(Request) -> Response errors E needs R)
  -> Void needs R + { clock: Clock }
pub fn serve_connection(connection: Connection, settings, handle) -> Void needs R + { clock: Clock }
```

### Requests

- The server MUST accept `HTTP/1.1` and `HTTP/1.0` request lines, and reads a
  later `HTTP/1.x` as 1.1. Another `HTTP/d.d` version is 505; anything else on
  the request line is 400. A method is a token. The target is origin form
  (`/path?query`), absolute form (`http://host/path`, whose authority then
  replaces the `host` header), or `*` for `OPTIONS`, which arrives as the path
  `["*"]`. Empty lines before a request line are skipped.
- `Http.path` is the target's path split on `/` with empty segments dropped,
  so `/users/7/` is `["users", "7"]`. Segments are not percent-decoded, and
  `.` and `..` segments reach the handler as they are, so a handler that maps
  a path to files MUST check them itself. `Http.query` is the text after the
  first `?`.
- A head holding a control character other than HTAB, or a CR or LF that is
  not part of a CRLF line end, is 400, so neither can reach a handler that
  echoes it into a response header.
- Header names are matched without regard to ASCII case and kept lowercase;
  values are trimmed of spaces and tabs only. Comparisons of header tokens
  (`chunked`, `close`, `keep-alive`, `100-continue`) fold ASCII case only, so a
  Unicode character that lowercases to an ASCII letter does not match. An
  HTTP/1.1 request without `host`, or any request with two, is 400.
- The body is framed by `content-length` (one value, or repeated equal values
  in one header or several) or by `transfer-encoding`. Every
  `transfer-encoding` header counts, joined in order: exactly `chunked` is
  read; other codings before a final `chunked` are 501; a coding list that
  does not end in `chunked`, `transfer-encoding` together with
  `content-length`, `transfer-encoding` on HTTP/1.0, conflicting lengths, or a
  length that is not digits (an empty one included) is 400. A chunked body's
  extensions and trailers are read and dropped; a control character or a bare
  CR or LF in a chunk line or trailer is 400.
- An HTTP/1.1 request with `expect: 100-continue` gets `100 Continue` before its
  body is read; HTTP/1.0 has no such status, so it gets none.
- Reading is linear in the bytes received however the client splits its
  writes, and a buffer of pipelined requests is not copied once per request.

### Limits

| Limit | Setting | Default | Past it |
| --- | --- | --- | --- |
| request line and headers | `max_head_bytes` | 64 KiB | 431 |
| header lines | `max_headers` | 100 | 431 |
| body; for a chunked body, its framing and trailers count too | `max_body_bytes` | 8 MiB | 413 |
| trailer lines of a chunked body | `max_headers` | 100 | 431 |
| time for a request to arrive, body included, counted from the previous answer | `request_timeout` | 60 s | the connection closes without an answer |
| time to send one response | `write_timeout` | 60 s | the connection closes |
| connections served at once | `max_connections` | 512 | a new connection is closed as soon as it is accepted |

### Responses and connections

- A response's status MUST be 200 to 599; the server sends `100 Continue`
  itself. A header name that is not a token, or a value holding a control
  character other than HTAB, is a defect when the response is built, so a
  value from a request cannot write headers of its own.
- The server frames every response itself: the handler's `content-length`,
  `transfer-encoding`, `connection`, `te`, `trailer`, `upgrade`, `keep-alive`
  and `proxy-connection` headers are dropped. A response carries
  `content-length` of its body, except that a 204 carries none and a 304 keeps
  only a `content-length` its handler gave (the length of the body it stands
  for). `HEAD`, 204 and 304 answers send no body.
- An HTTP/1.1 connection stays open between requests unless the request or
  the response says `connection: close`; an HTTP/1.0 connection stays open
  only when the request says `connection: keep-alive`, and its answers then
  say `connection: keep-alive`. A closing answer carries `connection: close`.
  Pipelined requests are answered in order.
- A malformed request gets its status with `connection: close`, and the
  connection closes, so nothing after it is read.
- A handler's expected failure answers 500 and the connection carries on.
  A handler's defect closes its connection without an answer, and the server
  and its other connections carry on (as Go's `net/http` does with a panic).
- `serve_connection` closes its connection however it ends, including by
  cancellation or a defect.
- `serve` runs each connection in its own process, started with
  `Supervisor.start_handoff` under a supervisor of its own that never
  restarts one and whose capacity is `max_connections`. At the cap, a new
  connection is closed as soon as it is accepted, as Node's
  `server.maxConnections` does, so the accept loop never waits and closing
  the listener always ends serving. The handler's needs are captured into
  the process, so their providers MUST be shareable.
- A failed accept (out of file descriptors, a client that reset before it
  was accepted) is retried after a pause that grows from 10 ms to 1 s and
  starts again at 10 ms after a success; only closing the listener ends
  serving. When serving ends, by the listener closing or the calling task
  being cancelled, the supervisor is stopped and the connections still open
  are closed at once.

### Not yet

- A connection accepted just before serving ends, whose serving has not
  begun (its process, or the task inside it, has not run yet), is discarded
  by the stop without being closed, so its client waits until the program
  exits. This is a runtime gap: a discarded start argument or task capture
  frees its memory but does not release the host handles it holds.
- `Date` headers, streamed request and response bodies, a graceful drain of
  open connections when serving stops, and a close that drains unread input
  before it closes (so a peer that is still sending does not see a reset).
- The HTTP client, and HTTPS after `std/tls`.

## std/sql (driver-independent layer)

Nested transactions are **provisional**: they follow the recommendation on an
open decision card (a transaction inside a transaction is a savepoint). A
pool takes its settings as a record, `Sql.defaults` updated with what differs,
as std/http does. Which database the first driver
talks to is also open; this layer has no driver of its own beyond `Sql.fake`.
`std/sql` is written in Chimi with no binding package, but its pool uses
bounded channels, so it runs on the native backend only.

```text
pub enum Value { Null, Bool, Int, Float, Decimal, Text, Bytes }
pub enum SqlError { ConnectFailed, ConnectionLost, PoolTimedOut, PoolClosed, Rejected,
  ConstraintFailed, Conflict, NoRows, ManyRows, NoColumn, WrongType, UnexpectedNull, Failed }
pub struct Database   // a pool, or the transaction a body is given
pub struct Row        // Sql.value, Sql.int, Sql.float, Sql.decimal, Sql.text, Sql.bool,
                      // Sql.bytes, Sql.nullable, Sql.columns, Sql.values

pub const defaults: { max_connections: Int, wait_timeout: Duration, reset_timeout: Duration }
pub fn open(driver: Driver, target: Text, settings, use: fn(Database) -> T errors E needs R)
  -> T needs R
pub fn execute(db, sql, params: List<Value>) -> Int
pub fn query(db, sql, params) -> List<Row>
pub fn one(db, sql, params) -> Row
pub fn first(db, sql, params) -> Option<Row>
pub fn transaction(db, body: fn(Database) -> T errors E needs R) -> T needs R

// For writing a driver, and for tests.
pub fn driver(connect: fn(Text) -> Connection errors SqlError) -> Driver
pub fn connection(run, begin, commit, rollback, reset, close) -> Connection
pub fn reply(columns, rows, affected) -> Reply
pub fn fake(answer: fn(Call) -> Reply errors SqlError) -> Driver
```

### Statements and rows

- Parameters MUST reach the driver as the list of `Value`s, unchanged and
  apart from the statement's text. Placeholders (`?`, `$1`) are the driver's
  dialect and are not rewritten.
- A result is read whole. `Sql.one` fails `NoRows` or `ManyRows` unless there
  is exactly one row; `Sql.first` gives the first or `None`.
- A row is read by column name. A typed accessor fails `NoColumn` for a
  missing column, `UnexpectedNull` for a null, and `WrongType(column,
  expected, found)` for another kind. `Sql.nullable(row, name, read)` is the
  only way to read a null, as `None`.
- A failure's payload names columns and constraints, never a row's contents.

### The pool

- `Sql.open` runs its body with a pool and closes the pool when the body ends,
  however it ends. Closing closes every idle connection, fails statements
  still waiting with `PoolClosed`, and waits up to `wait_timeout` for connections that
  tasks the body started still hold, closing each as it comes back. Using the
  pool after that fails `PoolClosed`.
- A connection is opened only when a statement needs one and none is idle,
  and is reused after the statement. At most `max_connections` (default 10)
  are open at once. A statement that waits more than `wait_timeout` (default
  30 s) for one fails `PoolTimedOut`, and a
  statement that gave up holds no place in the pool afterwards.
- Waiting statements are served in the order they asked.
- After every use the pool resets the connection (rolling back anything left
  open) within `reset_timeout` (default 5 s) before it is reused, however its
  use ended. A connection
  whose reset fails or times out is closed instead, and the next statement
  opens a new one. A driver whose connection cannot be trusted after a
  statement was cut off mid-way fails every later call on it with
  `ConnectionLost`, including `reset`; inside a transaction the outer body
  may still run `ROLLBACK TO SAVEPOINT` and further statements on that
  connection before its reset. A failed connect
  frees its place.
- The pool can be shared by every task the body starts. The pool's state is
  owned by one task, and other tasks reach it through a channel.
- A statement is not retried by the pool on `ConnectionLost`; retrying is
  `std/retry`'s job.

### Transactions

- `Sql.transaction(db, body)` begins a transaction on one connection and runs
  `body` with a database whose statements all use that connection. When
  `body` succeeds it commits, and a failed commit is the outcome. When `body`
  fails it rolls back and the body's failure is the outcome. When `body`
  defects or is cancelled, the pool's reset rolls the transaction back.
- A transaction inside a transaction is a savepoint (`SAVEPOINT chimi_<n>`,
  then `RELEASE SAVEPOINT` or `ROLLBACK TO SAVEPOINT`), so an inner failure
  that the outer body catches undoes only the inner statements. A savepoint
  is undone whenever its body does not succeed, including a defect or a
  cancellation the outer body survives (a `Task.timeout` inside it), and the
  body's failure stays the outcome even when `ROLLBACK TO SAVEPOINT` fails.

### Drivers and tests

- A driver is passed to `Sql.open` explicitly; there is no database need. A
  driver that needs the network or the file system takes that capability when
  it is built, so the capability still appears in `main`.
- `Sql.fake(answer)` is a driver that hands every call (`Connected`, `Ran`,
  `Began`, `Committed`, `RolledBack`, `Reset`, `Closed`) to `answer`. Tests
  record the calls with `Test.recorder` and script failures in `answer`.

## Decisions

- **One `network` need covers connecting, listening, and name lookup.** A
  separate need for each would double every service's signature for no
  authority a reader could tell apart.
- **Accessor functions, not fields.** Published structs never expose fields,
  so `Net.reader(connection)` is the only spelling, the same shape as
  `Stream.read(reader)`.
- **Lookups on a thread, not a resolver of our own.** The C library honours
  `/etc/hosts`, `nsswitch`, and the platform's resolver settings; a thread per
  lookup with a small cap keeps it off the scheduler without reimplementing
  DNS.
- **Idle waits hold virtual time.** A test of a server under virtual time
  would otherwise advance its clock while the server waits for a peer that
  never comes; tests that mix sockets and timers use the real clock.
- **How a test opts into the real network** is `Net.host_for_tests()`, the
  recommendation on Byron's open decision card; `Test.real_network()` or no
  real network in tests are the alternatives.

## Conformance

| Rule | Evidence |
| --- | --- |
| HTTP: pipelined requests in order, `connection: close` from either side, HTTP/1.0 and its `keep-alive` answer, `HEAD`, other methods | `tests/http.rs` `a_connection_answers_requests_in_order_and_closes_when_asked` |
| HTTP: chunked bodies with extensions and trailers, `100 Continue` | `tests/http.rs` `chunked_bodies_and_100_continue` |
| HTTP: 400, 413, 431, 501 and 505, each closing the connection; the body limit is inclusive; 101 header lines is 431 | `tests/http.rs` `a_malformed_request_gets_its_status_and_closes_the_connection` |
| HTTP: control characters, bare CR and LF, two hosts, transfer codings across headers, transfer-encoding on HTTP/1.0, ASCII-only case folding, versions | `tests/http.rs` `ambiguous_requests_are_refused` |
| HTTP: heads, bodies and chunks split across reads; a head that never ends is 431 in one read or many; empty lines skipped; 100 header lines accepted; repeated equal lengths; absolute and `*` targets; `100 Continue` before the body is sent, and not on HTTP/1.0 | `tests/http.rs` `requests_arrive_however_they_are_split_and_in_every_target_form` |
| HTTP: the handler's framing headers are replaced; 204 and 304 lengths | `tests/http.rs` `the_server_frames_every_response_itself` |
| HTTP: the write limit; a handler's defect closes the connection; failed accepts are retried with a growing pause | `tests/http.rs` `slow_clients_and_failed_accepts_do_not_stop_the_server_and_a_defect_closes_its_connection` |
| HTTP: a handler failure is 500; the idle limit; status and header defects; `Http.request` for handler tests | `tests/http.rs` `a_handler_failure_answers_500_and_an_idle_client_is_closed` |
| HTTP: connections served at once over loopback (within a time limit, so a server that answers one at a time fails); closing the listener ends `serve` and closes open connections; a handler's defect closes only its connection; past `max_connections` a client waits | `tests/http.rs` `a_server_on_loopback_serves_clients_at_once_and_stops_with_its_listener` |
| SQL: rows read by name, each typed accessor, nulls, missing columns, wrong kinds, `one` and `first` | `tests/sql.rs` `rows_are_read_by_column_name_with_typed_accessors` |
| SQL: lazy connections, reuse, parameters unchanged, a failed reset or connect, sharing across tasks, use after close | `tests/sql.rs` `a_pool_opens_connections_lazily_reuses_them_and_closes_them` |
| SQL: the pool's size and wait, and a waiter that gave up | `tests/sql.rs` `a_full_pool_makes_statements_wait_up_to_its_limit` |
| SQL: commit, rollback, defect, cancellation, a failed commit, savepoints | `tests/sql.rs` `a_transaction_commits_or_rolls_back_however_its_body_ends` |
| Bytes cross loopback both ways, a body larger than one read arrives whole, half-close, addresses and port 0 | `tests/net.rs` `a_loopback_connection_carries_bytes_both_ways_after_a_half_close` |
| A name resolves through the lookup thread (that the scheduler keeps running during a slow lookup is not tested: no lookup in the test environment is slow) | `tests/net.rs` `a_name_resolves_on_the_lookup_thread` |
| `Refused`, `AddressInUse`, `HostNotFound(host)` (also for a host holding U+0000), and the port defect | `tests/net.rs` `the_operating_systems_failures_become_net_errors` |
| Closing wakes a task in `accept` or a read with `Closed`; later accepts, reads and writes fail the same way; a scoped connection is released; a stopped reader keeps the writer; a write to a gone peer fails without SIGPIPE | `tests/net.rs` `closing_wakes_a_waiting_task_with_closed` |
| A fake pair and a fake network | `tests/net.rs` `a_fake_pair_and_a_fake_network_stand_in_for_sockets` |
| The runner closes `network` on `main` | `tests/net.rs` `main_gets_the_real_network_by_declaring_the_need` |
| `host_for_tests` is test-only (`T4401`); a test without `using` is `T4400` | `tests/net.rs` `only_a_test_that_asks_gets_the_real_network` |
