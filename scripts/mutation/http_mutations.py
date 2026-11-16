"""Mutants of the std/http server (CHI-257): keep-alive and closing, framing,
the error statuses and limits, the idle timeout, handler failures, header
checks, and serving connections at once. A mutant that makes a wait hang
shows as TIMEOUT, which counts as caught.

Run: scripts/mutation/mutate.py --timeout 300 scripts/mutation/http_mutations.py
Results: scripts/mutation/http_results.txt.
"""

CRATE = "compiler/chimi"

HTTP = "std/http.chimi"

HTTP_TESTS = dict(targets=["http"], filter="")


def mutant(name, old, new):
    return dict(name=name, file=HTTP, edits=[(old, new)], **HTTP_TESTS)


MUTATIONS = [
    mutant("HTTP/1.1 closes by default",
           "    True -> not List.contains(tokens, \"close\")",
           "    True -> List.contains(tokens, \"keep-alive\")"),
    mutant("HTTP/1.0 keeps by default",
           "    False -> List.contains(tokens, \"keep-alive\")",
           "    False -> not List.contains(tokens, \"close\")"),
    mutant("a handler's connection: close is ignored",
           "let keep = keep_alive and not asks_to_close(answer.headers)",
           "let keep = keep_alive"),
    mutant("pipelined bytes are dropped",
           "          True -> rest\n          False -> break Void",
           "          True -> Bytes.empty()\n          False -> break Void"),
    mutant("HEAD sends its body",
           "case head_only or response.status == 204 or response.status == 304 {",
           "case response.status == 204 or response.status == 304 {"),
    mutant("content-length and transfer-encoding together are allowed",
           "    (Some(_), [_, ..]) -> Failure(Refusal.Malformed(\"both content-length and transfer-encoding\"))\n",
           ""),
    mutant("conflicting lengths take the first",
           "    (None, [first, ..rest]) -> case List.all(rest, fn(other) { other == first }) {",
           "    (None, [first, ..rest]) -> case True {"),
    mutant("a length need not be digits",
           "  case all_digits(value) {",
           "  case True {"),
    mutant("HTTP/1.1 needs no host",
           "  case minor == 1 and find_header(headers, \"host\") == None {",
           "  case False {"),
    mutant("the header count is not limited",
           "      case List.length(header_lines) > bounds.header_count {",
           "      case False {"),
    mutant("a head found in one read is not limited",
           "      Some(at) -> case at > limit {",
           "      Some(at) -> case False {"),
    mutant("the body limit excludes its own size",
           "    Success(Framing.Length(length)) -> case length > bounds.body_bytes {",
           "    Success(Framing.Length(length)) -> case length >= bounds.body_bytes {"),
    mutant("a chunked body is not limited",
           "      False -> case size + length > limit {",
           "      False -> case False {"),
    mutant("chunk sizes are decimal",
           "total * 16 + hex_digit(byte)",
           "total * 10 + hex_digit(byte)"),
    mutant("no 100 Continue",
           "      True -> Stream.write(writer, Bytes.from_text(\"HTTP/1.1 100 Continue\\r\\n\\r\\n\"))",
           "      True -> Void"),
    mutant("a handler failure ends the connection",
           "    Err(_) -> text(500, \"internal server error\")",
           "    Err(_) -> Debug.crash(\"handler failed\")"),
    mutant("the idle limit is half a minute",
           "  Limits(65536, 100, 8388608, Duration.seconds(60))",
           "  Limits(65536, 100, 8388608, Duration.seconds(30))"),
    mutant("a header value may hold a line break",
           "    (_, True) -> Debug.crash(\"Http: the value of the header ${name} holds a line break\")\n",
           ""),
    mutant("connections are served one at a time",
           "        let _connection = Task.start(fn() { serve_connection(connection, handle) })",
           "        serve_connection(connection, handle)"),
    mutant("a malformed request leaves the connection open",
           "        write_response(writer, refusal_response(refusal), False, False)\n        break Void",
           "        write_response(writer, refusal_response(refusal), True, False)\n        pending"),
    mutant("header names keep their case",
           "      True -> (Text.lowercase(name), Text.trim(value))",
           "      True -> (name, Text.trim(value))"),
    mutant("the query stays in the path",
           "    Some((path, query)) -> (path, query)",
           "    Some((path, query)) -> (target, query)"),
]
