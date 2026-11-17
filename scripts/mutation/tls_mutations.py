"""Mutants of std/tls: the session engine (the certificate checks, the memory
buffers, close_notify, the system roots), the binding over it, and the Chimi
wrapper (the handshake loop, the failure mapping, truncation, close). A mutant
that makes a wait hang shows as TIMEOUT, which counts as caught.

Run: scripts/mutation/mutate.py --timeout 300 scripts/mutation/tls_mutations.py
Results: scripts/mutation/tls_results.txt.
"""

CRATE = "compiler/chimi"

SESSION = "runtime/native/bindings/tls/tls_session.c"
BINDING = "runtime/native/bindings/tls/tls.c"
TLS = "std/tls.chimi"

TLS_TESTS = dict(targets=["tls"], filter="")


def mutant(name, file, old, new):
    return dict(name=name, file=file, edits=[(old, new)], **TLS_TESTS)


MUTATIONS = [
    # The engine.
    mutant("the client does not verify", SESSION,
           "mbedtls_ssl_conf_authmode(&session->config, MBEDTLS_SSL_VERIFY_REQUIRED);",
           "mbedtls_ssl_conf_authmode(&session->config, MBEDTLS_SSL_VERIFY_NONE);"),
    mutant("the client only reports a failed check", SESSION,
           "mbedtls_ssl_conf_authmode(&session->config, MBEDTLS_SSL_VERIFY_REQUIRED);",
           "mbedtls_ssl_conf_authmode(&session->config, MBEDTLS_SSL_VERIFY_OPTIONAL);"),
    mutant("the host name is not checked", SESSION,
           "  result = mbedtls_ssl_set_hostname(&session->ssl, host);",
           "  result = mbedtls_ssl_set_hostname(&session->ssl, NULL);"),
    mutant("an expired certificate reads as untrusted", SESSION,
           "  } else if (flags & (MBEDTLS_X509_BADCERT_EXPIRED | MBEDTLS_X509_BADCERT_FUTURE)) {",
           "  } else if (flags & MBEDTLS_X509_BADCERT_FUTURE) {"),
    mutant("a wrong host reads as untrusted", SESSION,
           "  } else if (flags & MBEDTLS_X509_BADCERT_CN_MISMATCH) {",
           "  } else if (flags & 0) {"),
    mutant("a key that is not the certificate's is accepted", SESSION,
           "  result = mbedtls_pk_check_pair(&session->certificate.pk, &session->key);",
           "  result = 0;"),
    mutant("received bytes are dropped past the first read", SESSION,
           "  incoming->start += length;",
           "  incoming->start = incoming->length;"),
    mutant("a send writes only the first record", SESSION,
           "    sent += (size_t)result;",
           "    sent = length;\n    (void)result;"),
    mutant("close_notify is not the end", SESSION,
           "    if (result == MBEDTLS_ERR_SSL_PEER_CLOSE_NOTIFY) return TLS_PEER_CLOSED;",
           ""),
    mutant("an empty data record is the end", SESSION,
           "    if (result == MBEDTLS_ERR_SSL_PEER_CLOSE_NOTIFY) return TLS_PEER_CLOSED;",
           "    if (result == 0 || result == MBEDTLS_ERR_SSL_PEER_CLOSE_NOTIFY) return TLS_PEER_CLOSED;"),
    mutant("messages buffered in mbedTLS do not count as progress", SESSION,
           "    int progressed = session->incoming.start != consumed || buffered;",
           "    int progressed = session->incoming.start != consumed;"),
    mutant("the system-roots client accepts an empty host", BINDING,
           "    return chimi_binding_defect(call, \"tls: client_system takes a host\");\n  }\n  if (host_length == 0 || strlen(host) != host_length) {",
           "    return chimi_binding_defect(call, \"tls: client_system takes a host\");\n  }\n  if (strlen(host) != host_length) {"),
    mutant("a read waits for input while fed records are unread", SESSION,
           "    int unread = session->incoming.start < session->incoming.length ||\n                 mbedtls_ssl_check_pending(&session->ssl);",
           "    int unread = mbedtls_ssl_check_pending(&session->ssl);"),
    mutant("an untrusted expired certificate reads as expired", SESSION,
           "  if (flags & MBEDTLS_X509_BADCERT_NOT_TRUSTED) {",
           "  if (flags & 0) {"),
    mutant("an empty host is accepted", BINDING,
           "  if (host_length == 0 || strlen(host) != host_length) {\n    return chimi_binding_failure(call, TLS_INVALID, \"the host name is empty or holds U+0000\");\n  }\n  /* Empty",
           "  if (strlen(host) != host_length) {\n    return chimi_binding_failure(call, TLS_INVALID, \"the host name is empty or holds U+0000\");\n  }\n  /* Empty"),
    mutant("close_notify is never sent", SESSION,
           "  if (!session->failure) (void)mbedtls_ssl_close_notify(&session->ssl);",
           "  (void)session;"),
    mutant("SSL_CERT_FILE is ignored", SESSION,
           "  if (named && *named) return loaded(chain, named) ? 0 : -1;",
           "  (void)named;"),
    mutant("a missing SSL_CERT_FILE falls back to the bundles", SESSION,
           "  if (named && *named) return loaded(chain, named) ? 0 : -1;",
           "  if (named && *named && loaded(chain, named)) return 0;"),
    mutant("no roots trusts nothing but goes on", SESSION,
           "    if (tls_system_roots(&session->roots) != 0) {",
           "    if (0) {"),
    # The binding.
    mutant("a receive gives the whole buffer", BINDING,
           "      chimi_binding_make_bytes(call, buffer, status == TLS_DATA ? length : 0),",
           "      chimi_binding_make_bytes(call, buffer, status == TLS_DATA ? TLS_CHUNK : 0),"),
    mutant("take leaves the bytes queued", BINDING,
           "  tls_took(locked->session);",
           ""),
    mutant("handoff frees the session it hands over", BINDING,
           "  TlsSession *session = locked->session;\n  locked->session = NULL;",
           "  TlsSession *session = locked->session;"),
    mutant("free leaves the session open", BINDING,
           "  chimi_binding_close_handle(call, 0, &SESSION);",
           ""),
    # The Chimi wrapper.
    mutant("the handshake does not send its flight", TLS,
           "      Ok(_) -> flush(connection, session)\n",
           "      Ok(_) -> Void\n"),
    mutant("a timed-out handshake keeps its session", TLS,
           "  let session = Resource.scoped(start, native_free, fn(starting) {",
           "  let session = Resource.scoped(start, fn(_) { Void }, fn(starting) {"),
    mutant("a handshake that needs input reads as done", TLS,
           "    case done {\n      True -> break Void\n      False -> {",
           "    case True {\n      True -> break Void\n      False -> {"),
    mutant("a peer ending the handshake reads as done", TLS,
           "          None -> Failure(TlsError.Handshake(\"the peer ended the connection during the handshake\"))",
           "          None -> Void"),
    mutant("truncation reads as a clean end", TLS,
           "          None -> Failure(NetError.Failed(\"TLS: the peer ended the connection without close_notify\"))",
           "          None -> Void"),
    mutant("closing the writer skips close_notify", TLS,
           "  native_close_notify(session)\n  flush(connection, session)\n  Stream.close",
           "  flush(connection, session)\n  Stream.close"),
    mutant("closing the writer leaves the inner writer open", TLS,
           "  flush(connection, session)\n  Stream.close(Net.writer(connection))\n}",
           "  flush(connection, session)\n}"),
    mutant("a write is not sent until the next one", TLS,
           "  net(native_send(session, bytes))\n  flush(connection, session)",
           "  net(native_send(session, bytes))"),
    mutant("WrongHost names no host", TLS,
           "    Err((3, _)) -> Failure(TlsError.WrongHost(host))",
           "    Err((3, _)) -> Failure(TlsError.WrongHost(\"\"))"),
    mutant("NoRoots reads as Invalid", TLS,
           "    Err((8, _)) -> Failure(TlsError.NoRoots)\n",
           ""),
    mutant("roots are not checked when made", TLS,
           "  let _ = tls(native_check_roots(pem), \"\")\n",
           ""),
    mutant("an identity is not checked when made", TLS,
           "  tls(native_check_identity(certificate, key), \"\")\n",
           ""),
    mutant("a closed session reads as a TLS failure", TLS,
           "    Err((7, _)) -> Failure(NetError.Closed)\n    Err((_, message)) -> Failure(NetError.Failed(\"TLS: ${message}\"))",
           "    Err((_, message)) -> Failure(NetError.Failed(\"TLS: ${message}\"))"),
    mutant("the defaults trust the system's roots by mistake for a bundle", TLS,
           "    Some(pem) -> tls(native_client(pem, host), host)",
           "    Some(_) -> tls(native_client_system(host), host)"),
]
