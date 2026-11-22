"""Mutants of the OpenSSL interop test's server start (tests/tls.rs).

Run: scripts/mutation/mutate.py scripts/mutation/tls_port_mutations.py
"""

CRATE = "compiler/chimi"

TLS = "tests/tls.rs"


def mutant(name, edits):
    return dict(name=name, file=TLS, edits=edits, targets=["tls"], filter="openssl")


MUTATIONS = [
    mutant("a server that exited counts as listening", [(
        "                // The server exited before it listened.\n                let _ = server.wait();",
        "                return (server, port);",
    )]),
    mutant("only the first port is tried", [(
        "    for port in ports.take(5) {",
        "    for port in ports.take(1) {",
    )]),
]
