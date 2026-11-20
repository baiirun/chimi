"""Mutants of Tls.listener (CHI-257): the deferred handshake on a
connection's first read or write, the failure it reports, the session's
lifetime, and what the listener passes through. A mutant that makes a wait
hang shows as TIMEOUT, which counts as caught.

Run: scripts/mutation/mutate.py --timeout 300 scripts/mutation/tls_listener_mutations.py
Results: scripts/mutation/tls_listener_results.txt.
"""

CRATE = "compiler/chimi"

TLS = "std/tls.chimi"

TLS_TESTS = dict(targets=["tls"], filter="")


def mutant(name, old, new):
    return dict(name=name, file=TLS, edits=[(old, new)], **TLS_TESTS)


MUTATIONS = [
    mutant("a read does not handshake",
           "    Stream.reader(fn() {\n      settle(connection, session, turns)\n",
           "    Stream.reader(fn() {\n"),
    mutant("a write does not handshake",
           "    Stream.writer(fn(bytes) {\n      settle(connection, session, turns)\n",
           "    Stream.writer(fn(bytes) {\n"),
    mutant("accept waits for the handshake",
           "      Ok(session) -> deferred(accepted, session)\n",
           "      Ok(session) -> {\n        settle(accepted, session, Watch.starting_with(0).publisher)\n        deferred(accepted, session)\n      }\n"),
    mutant("a failed handshake is ignored",
           "    Err(TlsError.Handshake(message)) -> failed(message)\n",
           "    Err(TlsError.Handshake(_)) -> Void\n"),
    mutant("a failed handshake does not say it is one",
           "  Failure(NetError.Failed(\"TLS: the handshake failed: ${message}\"))",
           "  Failure(NetError.Failed(message))"),
    mutant("closing does not free the session",
           "    }, fn() { finish(connection, session) }),\n    Net.local_address(connection),\n    Net.peer_address(connection),\n    fn() {\n      native_free(session)\n",
           "    }, fn() { finish(connection, session) }),\n    Net.local_address(connection),\n    Net.peer_address(connection),\n    fn() {\n      Void\n"),
    mutant("closing the TLS listener leaves the inner one open",
           "  }, Net.listener_address(listener), fn() { Net.close_listener(listener) })",
           "  }, Net.listener_address(listener), fn() { Void })"),
    mutant("the listener has another address",
           "  }, Net.listener_address(listener), fn() { Net.close_listener(listener) })",
           "  }, Net.address(\"127.0.0.1\", 1), fn() { Net.close_listener(listener) })"),
    mutant("two tasks drive one handshake",
           "    case native_claim(session) {",
           "    case True {"),
    mutant("a waiting task is never woken",
           "        let _ = Result.capture(fn() { Watch.publish(turns, seen + 1) })\n",
           ""),
    mutant("a waiting task wakes at once",
           "        Receive.Received(count) -> case count == seen {",
           "        Receive.Received(count) -> case False {"),
    mutant("the claim is never given back",
           "        native_unclaim(session)\n        let _",
           "        let _"),
    mutant("a connection is not encrypted",
           "      Ok(session) -> deferred(accepted, session)\n",
           "      Ok(_) -> accepted\n"),
]
