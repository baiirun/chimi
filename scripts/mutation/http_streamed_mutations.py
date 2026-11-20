"""Mutants of Http.streamed (CHI-263): framing a streamed body on HTTP/1.1
and HTTP/1.0, HEAD, the last chunk, a producer's failure, the write limit on
each write, and a client that stops reading or leaves. A mutant that makes a
wait hang shows as TIMEOUT, which counts as caught.

Run: scripts/mutation/mutate.py --timeout 300 scripts/mutation/http_streamed_mutations.py
Results: scripts/mutation/http_streamed_results.txt.
"""

CRATE = "compiler/chimi"

HTTP = "std/http.chimi"

HTTP_TESTS = dict(targets=["http"], filter="")


def mutant(name, old, new):
    return dict(name=name, file=HTTP, edits=[(old, new)], **HTTP_TESTS)


MUTATIONS = [
    mutant("HTTP/1.0 keeps a streamed connection open",
           "  let keep = keep_alive and (minor == 1 or not is_streamed(response))",
           "  let keep = keep_alive"),
    mutant("HTTP/1.0 is sent chunked",
           "      let chunked = minor == 1\n",
           "      let chunked = True\n"),
    mutant("the last chunk is never sent",
           "            True -> timed_write(writer, Bytes.from_text(\"0\\r\\n\\r\\n\"), limit, given)",
           "            True -> Void"),
    mutant("a failed producer keeps the connection",
           "        False -> False\n      }\n    }\n    _ -> keep",
           "        False -> keep\n      }\n    }\n    _ -> keep"),
    mutant("HEAD runs the producer",
           "    (Body.Streamed(produce), False) -> {",
           "    (Body.Streamed(produce), _) -> {"),
    mutant("a chunk's size is decimal",
           "${hex(Bytes.length(bytes))}",
           "${Int.to_text(Bytes.length(bytes))}"),
    mutant("hexadecimal digits come out reversed",
           "      True -> break more\n",
           "      True -> break List.reverse(more)\n"),
    mutant("a chunk has no line break after its data",
           "bytes, Bytes.from_text(\"\\r\\n\")])",
           "bytes])"),
    mutant("a stream's writes have no limit",
           "    }, limit) using { clock: given }",
           "    }, Duration.seconds(100000)) using { clock: given }"),
    mutant("the write limit bounds the whole stream",
           "      case produce(body_writer(writer, chunked, limit, given)) {",
           "      case Task.timeout(fn() { produce(body_writer(writer, chunked, limit, given)) }, limit) using { clock: given } {"),
    mutant("a failed write is ignored",
           "    Ok(Err(error)) -> Failure(error)",
           "    Ok(Err(_)) -> Void"),
    mutant("a timed-out write is ignored",
           "    Err(_) -> Failure(NetError.TimedOut)",
           "    Err(_) -> Void"),
    mutant("a producer's failure counts as finished",
           "      Ok(_) -> True\n      Err(_) -> False",
           "      Ok(_) -> True\n      Err(_) -> True"),
    mutant("a 304 may be streamed",
           "  let checked = case status == 204 or status == 304 {",
           "  let checked = case status == 204 {"),
    mutant("a streamed response's status is not checked",
           "    False -> checked_status(status)\n  }\n  Response(checked,",
           "    False -> status\n  }\n  Response(checked,"),
]
