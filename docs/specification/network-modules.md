# Network Modules

## Status

This is the normative contract for the Phase 7b network modules of the
[language completion plan](../plans/language-completion-plan.md) (CHI-253):
the modules a service uses to talk over the network. It covers `std/net`
(CHI-255). `std/tls`, `std/http`, and `std/sql` (CHI-256 to CHI-258) add their
sections as they land.

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
| Bytes cross loopback both ways, a body larger than one read arrives whole, half-close, addresses and port 0 | `tests/net.rs` `a_loopback_connection_carries_bytes_both_ways_after_a_half_close` |
| A name resolves through the lookup thread (that the scheduler keeps running during a slow lookup is not tested: no lookup in the test environment is slow) | `tests/net.rs` `a_name_resolves_on_the_lookup_thread` |
| `Refused`, `AddressInUse`, `HostNotFound(host)` (also for a host holding U+0000), and the port defect | `tests/net.rs` `the_operating_systems_failures_become_net_errors` |
| Closing wakes a task in `accept` or a read with `Closed`; later accepts, reads and writes fail the same way; a scoped connection is released; a stopped reader keeps the writer; a write to a gone peer fails without SIGPIPE | `tests/net.rs` `closing_wakes_a_waiting_task_with_closed` |
| A fake pair and a fake network | `tests/net.rs` `a_fake_pair_and_a_fake_network_stand_in_for_sockets` |
| The runner closes `network` on `main` | `tests/net.rs` `main_gets_the_real_network_by_declaring_the_need` |
| `host_for_tests` is test-only (`T4401`); a test without `using` is `T4400` | `tests/net.rs` `only_a_test_that_asks_gets_the_real_network` |
