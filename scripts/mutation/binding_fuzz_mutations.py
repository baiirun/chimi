"""Mutants of the binding shims the binding fuzz corpus pins (CHI-304): the
TLS session engine, the PEM roots check, SCRAM and std/sqlite's prepare.

Run: scripts/mutation/mutate.py scripts/mutation/binding_fuzz_mutations.py
Each runs the binding corpus replay in `runtime_fuzz`, which must report a
changed output, a broken invariant or a sanitizer error, except the one the
engine fixture in `tls` guards.
"""

CRATE = "compiler/chimi"

T = "runtime/native/bindings/tls/tls_session.c"
P = "runtime/native/bindings/tls/postgres.c"
S = "runtime/native/bindings/sqlite/sqlite.c"


def corpus(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["runtime_fuzz"], filter="binding")


MUTATIONS = [
    corpus("the transport hands mbedTLS more bytes than it holds", T, [(
        "  if (length > available) length = available;\n",
        "",
    )]),
    corpus("compacting the incoming buffer drops a byte", T, [(
        "memmove(buffer->bytes, buffer->bytes + buffer->start, buffer->length - buffer->start);",
        "memmove(buffer->bytes, buffer->bytes + buffer->start + 1, buffer->length - buffer->start - 1);",
    )]),
    # The fuzz harness cannot pack several messages into one record, which
    # this needs; the engine fixture in tests/tls.rs does. (Lowering the idle
    # bound itself, `idle > 4` to `idle > 0`, changes nothing observable: a
    # pass that neither consumes input nor works through buffered messages
    # is not followed by one that does.)
    dict(name="working through buffered messages counts as idle", file=T, edits=[(
        "    int progressed = session->incoming.start != consumed || buffered;",
        "    int progressed = session->incoming.start != consumed;",
    )], targets=["tls"], filter="the_engine_reads_past_records_without_data"),
    corpus("close_notify reads as missing input", T, [(
        "    if (result == MBEDTLS_ERR_SSL_PEER_CLOSE_NOTIFY) return TLS_PEER_CLOSED;",
        "    if (result == MBEDTLS_ERR_SSL_PEER_CLOSE_NOTIFY) return TLS_NEEDS_INPUT;",
    )]),
    corpus("the roots check counts only the first root", T, [(
        "each && each->raw.len; each = each->next) count++;",
        "each && each->raw.len; each = NULL) count++;",
    )]),
    corpus("SCRAM derives the server key with the client's label", P, [(
        '(const unsigned char *)"Server Key", 10, server_key)',
        '(const unsigned char *)"Client Key", 10, server_key)',
    )]),
    corpus("SCRAM accepts an iteration count of zero", P, [(
        "  if (iterations < 1 || iterations > 1000000) {",
        "  if (iterations < 0 || iterations > 1000000) {",
    )]),
    corpus("prepare ignores text after a NUL", S, [(
        "  if (memchr(rest, 0, (size_t)(end - rest))) return 0;\n",
        "",
    )]),
    corpus("prepare takes an error after the first statement as blank", S, [(
        "  return code == SQLITE_OK && statement == NULL;",
        "  return statement == NULL;",
    )]),
]
