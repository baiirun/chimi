"""Mutants of std/net (CHI-255): the C binding package (half-close, the
lookup thread, failure codes, closing, addresses) and the Chimi module over it
(the failure mapping, the port check, the fakes, the runner provider and the
test-only gate). A mutant that makes a wait hang shows as TIMEOUT, which
counts as caught.

Run: scripts/mutation/mutate.py --timeout 300 scripts/mutation/net_mutations.py
Results: scripts/mutation/net_results.txt.
"""

CRATE = "compiler/chimi"

NET_C = "runtime/native/bindings/net/net.c"
NET = "std/net.chimi"
OPERATIONS = "src/operations.rs"

NET_TESTS = dict(targets=["net"], filter="")


def mutant(name, file, old, new):
    return dict(name=name, file=file, edits=[(old, new)], **NET_TESTS)


MUTATIONS = [
    # The binding.
    mutant("finishing closes both directions", NET_C,
           "shutdown(socket_state->fd, SHUT_WR)",
           "shutdown(socket_state->fd, SHUT_RDWR)"),
    mutant("refused reads as unreachable", NET_C,
           "    case ECONNREFUSED: return NET_REFUSED;",
           "    case ECONNREFUSED: return NET_UNREACHABLE;"),
    mutant("an address in use is a plain failure", NET_C,
           "    case EADDRINUSE:\n",
           ""),
    mutant("a name is looked up as a number only", NET_C,
           "  if (status != EAI_NONAME || host == NULL) return resolve_failure(call, host ? host : \"\", status, errno);",
           "  return resolve_failure(call, host ? host : \"\", status, errno);"),
    mutant("a host holding U+0000 is looked up up to it", NET_C,
           "  return strlen(*host) == length ? 1 : -1;",
           "  return 1;"),
    mutant("the lookup call does not wait for the thread", NET_C,
           "    if (atomic_load_explicit(&lookup->done, memory_order_acquire)) break;",
           "    break;"),
    mutant("a stopped reader still reads", NET_C,
           "    if (socket_state == NULL || socket_state->reading_stopped) {",
           "    if (socket_state == NULL) {"),
    mutant("a failed lookup thread wait reads as ready", NET_C,
           "    if (waited == CHIMI_BINDING_WAIT_CANCELLED || waited == CHIMI_BINDING_WAIT_FAILED) {",
           "    if (waited == CHIMI_BINDING_WAIT_CANCELLED) {"),
    mutant("a write stops after one send", NET_C,
           "      written += (size_t)count;\n      continue;",
           "      written += (size_t)count;\n      break;"),
    mutant("a read gives the peer's end as an empty chunk", NET_C,
           "    if (count == 0) {\n      result = chimi_binding_ok(call, chimi_binding_none(call));",
           "    if (count == 0) {\n      result = chimi_binding_ok(call, chimi_binding_some(call, chimi_binding_make_bytes(call, buffer, 0)));"),
    mutant("the local address is the peer's", NET_C,
           "  return address_of(call, &SOCKET, 0);",
           "  return address_of(call, &SOCKET, 1);"),
    mutant("connecting ignores a failed connect", NET_C,
           "    if (error != 0) {\n      close(fd);\n      return error;\n    }\n  }",
           "  }"),
    mutant("listening ignores a failed bind", NET_C,
           "bind(fd, address->ai_addr, address->ai_addrlen) != 0 ||",
           "(bind(fd, address->ai_addr, address->ai_addrlen), 0) ||"),
    mutant("a write after finishing reaches the socket", NET_C,
           "    if (socket_state == NULL || socket_state->writing_finished) return closed(call);",
           "    if (socket_state == NULL) return closed(call);"),
    mutant("an empty host listens on whatever the system lists first", NET_C,
           "resolve(call, host[0] ? host : \"0.0.0.0\", port, 1)",
           "resolve(call, host[0] ? host : \"::\", port, 1)"),
    # The Chimi module.
    mutant("stopping the reader releases the socket", NET,
           "fn() { native_stop_reading(socket) }",
           "fn() { native_close(socket) }"),
    mutant("a fake's aborted read is a reset", NET,
           "    Err(StreamError.Aborted) -> Failure(NetError.Closed)\n    Err(StreamError.Closed) -> Failure(NetError.Closed)",
           "    Err(StreamError.Aborted) -> Failure(NetError.Reset)\n    Err(StreamError.Closed) -> Failure(NetError.Closed)"),
    mutant("a host with U+0000 is named as C saw it", NET,
           "    Err(NetError.HostNotFound(_)) -> Failure(NetError.HostNotFound(host))",
           "    Err(NetError.HostNotFound(seen)) -> Failure(NetError.HostNotFound(seen))"),
    mutant("a closed socket reads as a reset", NET,
           "    Err((4, _)) -> Failure(NetError.Closed)",
           "    Err((4, _)) -> Failure(NetError.Reset)"),
    mutant("the port check lets 65536 through", NET,
           "port >= 0 and port <= 65535",
           "port >= 0 and port <= 65536"),
    mutant("a fake pair crosses its wires", NET,
           "  (fake_connection(left_in, left_out, left, right), fake_connection(right_in, right_out, right, left))",
           "  (fake_connection(left_in, right_out, left, right), fake_connection(right_in, left_out, right, left))"),
    mutant("the fake's peer is itself", NET,
           "fake_connection(left_in, left_out, left, right)",
           "fake_connection(left_in, left_out, left, left)"),
    mutant("closing a fake leaves its writer open", NET,
           "      Stream.stop(input)\n      let _ = Result.capture(fn() { Stream.close(output) })",
           "      Stream.stop(input)\n      let _ = output"),
    mutant("Net.close does nothing", NET,
           "pub fn close(connection: Connection) -> Void {\n  connection.close()",
           "pub fn close(connection: Connection) -> Void {\n  let _ = connection\n  Void"),
    # The registry.
    mutant("host_for_tests is reachable from a program", OPERATIONS,
           "\"chimi_net_allow_real_network\", true)",
           "\"chimi_net_allow_real_network\", false)"),
    mutant("the runner does not close network", OPERATIONS,
           "        need: \"network\",",
           "        need: \"net\","),
]
