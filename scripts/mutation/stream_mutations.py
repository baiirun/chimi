"""Mutants of byte streams (G18): `std/stream.chimi`, and the checker fix that
lets a failure-set parameter pass through a struct's type argument.

Run: scripts/mutation/mutate.py scripts/mutation/stream_mutations.py
The stream mutants are judged by the native programs in tests/streams.rs, the
checker mutants by tests/language.rs and tests/streams.rs (std/stream relies
on the fix). Results: scripts/mutation/stream_results.txt.

Left out as equivalent: `from_bytes` skipping its own empty check, since
`write` already writes nothing for empty bytes.
"""

CRATE = "compiler/chimi"

STREAM = "std/stream.chimi"
TYPING = "src/typing.rs"


def streams(name, edits):
    return dict(name=name, file=STREAM, edits=edits, targets=["streams"])


def checker(name, edits):
    return dict(name=name, file=TYPING, edits=edits, targets=["language", "streams"])


MUTATIONS = [
    streams("an empty write hands out an empty chunk", [(
        "    True -> Void\n    False -> writer.push(Push.Chunk(bytes))",
        "    True -> writer.push(Push.Chunk(bytes))\n    False -> writer.push(Push.Chunk(bytes))")]),
    streams("read_all has no limit", [(
        "        case size > limit {",
        "        case size > limit + 1000000 {")]),
    streams("read_all rejects a stream of exactly its limit", [(
        "        case size > limit {",
        "        case size >= limit {")]),
    streams("a producer's failure is lost", [(
        "        Success(Receive.Received(error)) -> Failure(error)",
        "        Success(Receive.Received(error)) -> Failure(StreamError.Aborted)")]),
    streams("a vanished writer reads as a clean end", [(
        "      Failure(ChannelError.Aborted) -> Failure(StreamError.Aborted)\n    }\n    Pull.Stop",
        "      Failure(ChannelError.Aborted) -> None\n    }\n    Pull.Stop")]),
    streams("stop does not reach the producer", [(
        "      Channel.abort(receiver)\n      None",
        "      None")]),
    streams("produce never closes the writer", [(
        "  body(out)\n  close(out)",
        "  body(out)")]),
    streams("produce ignores its capacity", [(
        "  let (out, chunks) = pipe(capacity)",
        "  let (out, chunks) = pipe(capacity + 8)")]),
    streams("copy counts chunks, not bytes", [(
        "        copied + Bytes.length(chunk)",
        "        copied + 1")]),
    checker("a struct's type argument does not make a generic a failure set", [(
        "                            if failures_index.is_none() {\n"
        "                                self.constrain_failure_set_arguments(module_id, symbol, arguments);",
        "                            if false {\n"
        "                                self.constrain_failure_set_arguments(module_id, symbol, arguments);")]),
    checker("a field call does not read failures from the type argument", [(
        "                .or(from_value)",
        "                .or(from_value.filter(|_| false))")]),
    checker("a constructor does not bind a failure-row parameter", [(
        "                    Some(GenericKind::FailureSet | GenericKind::FailureRow)\n"
        "                ) {\n"
        "                    // A struct's failure-row parameter",
        "                    Some(GenericKind::FailureSet)\n"
        "                ) {\n"
        "                    // A struct's failure-row parameter")]),
]
