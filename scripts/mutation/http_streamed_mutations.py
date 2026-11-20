"""Mutants of Http.streamed (CHI-263): framing a streamed body on HTTP/1.1
and HTTP/1.0, HEAD, the last chunk, a producer's failure, the write limit on
each chunk, a client that stops reading or leaves, a write failure the
producer caught, closing the writer, and writes from the producer's tasks. A mutant that makes a
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
    mutant("a cut-short stream keeps the connection",
           "      keep and whole\n",
           "      keep\n"),
    mutant("HTTP/1.0 is sent chunked",
           "      let whole = send_streamed(writer, produce, minor == 1, limit)",
           "      let whole = send_streamed(writer, produce, True, limit)"),
    mutant("the last chunk is never sent",
           "      Task.timeout(fn() {\n        Stream.write(writer, Bytes.from_text(\"0\\r\\n\\r\\n\"))\n      }, limit)\n",
           "      Void\n"),
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
    mutant("a chunk's send has no limit",
           "          Task.timeout(fn() { Stream.write(writer, framed) }, limit)",
           "          Task.timeout(fn() { Stream.write(writer, framed) }, Duration.seconds(100000))"),
    mutant("a failed chunk is skipped and the body goes on",
           "          Err(_) -> {\n            Channel.abort(receiver)\n            break False\n          }",
           "          Err(_) -> 0"),
    # Not a mutant: not aborting the channel after a failed chunk survives.
    # The producer's next write then waits on a channel nobody reads until
    # the connection's task ends and cancels it, which sends the same bytes.
    mutant("a failed producer ends the body as whole",
           "      Err(_) -> Piece.Failed\n",
           "      Err(_) -> Piece.End\n"),
    mutant("a producer's failure counts as finished",
           "      Ok(_) -> Void\n      Err(_) -> Failure(StreamError.Aborted)",
           "      Ok(_) -> Void\n      Err(_) -> Void"),
    mutant("closing the writer does not end the body",
           "    set(ended)\n    hand(sender, Piece.End)\n",
           "    Void\n"),
    mutant("writes after a close go through",
           "    case is_set(ended) {\n      True -> Failure(StreamError.Closed)",
           "    case False {\n      True -> Failure(StreamError.Closed)"),
    mutant("the producer writes straight to the connection",
           "      False -> hand(sender, Piece.Data(bytes))",
           "      False -> Stream.write(writer, case chunked {\n        True -> Bytes.concat([Bytes.from_text(\"${hex(Bytes.length(bytes))}\\r\\n\"), bytes, Bytes.from_text(\"\\r\\n\")])\n        False -> bytes\n      })"),
    mutant("a 304 may be streamed",
           "  let checked = case status == 204 or status == 304 {",
           "  let checked = case status == 204 {"),
    mutant("a streamed response's status is not checked",
           "    False -> checked_status(status)\n  }\n  Response(checked,",
           "    False -> status\n  }\n  Response(checked,"),
]
