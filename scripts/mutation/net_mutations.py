"""Mutants of std/net (CHI-255): the binding package, in Rust since CHI-309 (half-close, the
lookup thread, failure codes, closing, addresses) and the Chimi module over it
(the failure mapping, the port check, the fakes, the runner provider and the
test-only gate). A mutant that makes a wait hang shows as TIMEOUT, which
counts as caught.

Not listed, as equivalent: the host a lookup's failure names, since `named`
in std/net.chimi replaces it with the host as the caller wrote it.

Run: scripts/mutation/mutate.py --timeout 300 scripts/mutation/net_mutations.py
Results: scripts/mutation/net_results.txt.
"""

CRATE = "compiler/chimi"

NET_RS = "../../runtime/chimi-runtime/src/binding/net.rs"
NET = "std/net.chimi"
OPERATIONS = "src/operations.rs"

NET_TESTS = dict(targets=["net"], filter="")


def mutant(name, file, old, new):
    return dict(name=name, file=file, edits=[(old, new)], **NET_TESTS)


MUTATIONS = [
    # The binding.
    mutant("finishing closes both directions", NET_RS,
           "libc::shutdown(socket.fd, libc::SHUT_WR)",
           "libc::shutdown(socket.fd, libc::SHUT_RDWR)"),
    mutant("refused reads as unreachable", NET_RS,
           "        libc::ECONNREFUSED => code::REFUSED,",
           "        libc::ECONNREFUSED => code::UNREACHABLE,"),
    mutant("an address in use is a plain failure", NET_RS,
           "        libc::EADDRINUSE | libc::EADDRNOTAVAIL => code::ADDRESS_IN_USE,",
           "        libc::EADDRNOTAVAIL => code::ADDRESS_IN_USE,"),
    mutant("a name is looked up as a number only", NET_RS,
           "    let Some(host) = host.filter(|_| status == libc::EAI_NONAME) else {",
           "    let Some(host) = host.filter(|_| false) else {"),
    mutant("a host holding U+0000 is looked up up to it", NET_RS,
           "    if text.to_bytes().len() != host.as_bytes().len() {",
           "    if false {"),
    mutant("the lookup call does not wait for the thread", NET_RS,
           "    while !lookup.done.load(Ordering::Acquire) {",
           "    while false {"),
    mutant("a stopped reader still reads", NET_RS,
           "            Some(socket) if !socket.reading_stopped.get() => socket.fd,",
           "            Some(socket) => socket.fd,"),
    mutant("a failed lookup thread wait reads as ready", NET_RS,
           "            Waited::Failed => return Err(wait_failed(call)),\n        }\n    }\n    drop(read_end);",
           "            Waited::Failed => {}\n        }\n    }\n    drop(read_end);"),
    mutant("a write stops after one send", NET_RS,
           "            rest = rest.get(written..).unwrap_or_default();\n            continue;",
           "            rest = rest.get(written..).unwrap_or_default();\n            break;"),
    mutant("a read gives the peer's end as an empty chunk", NET_RS,
           "        if count == 0 {\n            let none = call.none();\n            return call.ok(none);",
           "        if count == 0 {\n            let bytes = call.make_bytes(&[]);\n            let none = call.some(bytes);\n            return call.ok(none);"),
    mutant("the local address is the peer's", NET_RS,
           "    address_of(unsafe { Call::new(raw) }, &SOCKET, End::Local).into_raw()",
           "    address_of(unsafe { Call::new(raw) }, &SOCKET, End::Peer).into_raw()"),
    mutant("connecting ignores a failed connect", NET_RS,
           "        if error != 0 {\n            return Attempt::Failed(error);\n        }",
           ""),
    mutant("listening ignores a failed bind", NET_RS,
           "            if unsafe { libc::bind(fd.0, address.ai_addr, address.ai_addrlen) } != 0\n                ||",
           "            if (unsafe { libc::bind(fd.0, address.ai_addr, address.ai_addrlen) } != 0 && false)\n                ||"),
    mutant("a write after finishing reaches the socket", NET_RS,
           "            Some(socket) if !socket.writing_finished.get() => socket.fd,",
           "            Some(socket) => socket.fd,"),
    mutant("an empty host listens on whatever the system lists first", NET_RS,
           '    let host = if host.is_empty() { c"0.0.0.0" } else { host };',
           '    let host = if host.is_empty() { c"::" } else { host };'),
    mutant("the port is the decimal of the wrong value", NET_RS,
           "    decimal(port, &mut service);",
           "    decimal(port / 10, &mut service);"),
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
