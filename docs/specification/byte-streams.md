# Byte Streams

## Status

This is the normative contract for byte streams (G18) from the
[language completion plan](../plans/language-completion-plan.md), part of
Phase 6b (CHI-209). `std/stream` is written in Chimi on top of bounded
channels, tasks, and [scope-owned resources](scope-owned-resources.md). Pipes
and produced streams need channels, so they run on the native backend only;
`Stream.reader`, `Stream.writer`, `read_all`, and `copy` over them run on
both.

The words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are normative. The
module follows the [standard library conventions](standard-library-conventions.md).

## Goals

- One shape for reading and writing bytes in bounded chunks, so that files,
  sockets, subprocess pipes, HTTP bodies, and compression (Phases 6c and 7b)
  connect to each other, and copying, limits, and closing are written once.
- Backpressure: a producer that runs ahead of its reader waits, so a stream
  never buffers without bound.
- A stream is a resource: `Resource.scoped` ties it to the code that uses it,
  and stopping it early tells the producer.
- No new syntax and no traits: a stream is a value whose operations are
  ordinary functions.

## Example

<!-- chimi: check -->
```chimi
import std/bytes as Bytes
import std/bytes.{Bytes}
import std/resource as Resource
import std/stream as Stream
import std/stream.{Reader, StreamError}

/// Every byte `open` gives, up to `limit`, stopping the stream however
/// reading ends.
fn read_body<E>(open: fn() -> Reader<E>, limit: Int) -> Bytes {
  Resource.scoped(open, Stream.stop, fn(body) { Stream.read_all(body, limit) })
}

/// A stream of `text` in chunks of at most `size` bytes, written by a child
/// task that waits whenever two chunks are already waiting.
fn chunked(text: Text, size: Int) -> Reader<StreamError> {
  let bytes = Bytes.from_text(text)
  Stream.produce(2, fn(out) {
    loop start = 0 {
      case start >= Bytes.length(bytes) {
        True -> break Void
        False -> {
          Stream.write(out, Bytes.slice(bytes, start..start + size))
          start + size
        }
      }
    }
  })
}
```

## Contract

```text
pub enum StreamError { Closed, Aborted, TooLarge(limit: Int) }
pub struct Reader<E>
pub struct Writer<E>

pub fn reader<E>(next: fn() -> Option<Bytes> errors E, stop: fn() -> Void) -> Reader<E>
pub fn writer<E>(write: fn(Bytes) -> Void errors E, close: fn() -> Void errors E) -> Writer<E>
pub fn pipe(capacity: Int) -> (Writer<StreamError>, Reader<StreamError>)
pub fn produce<E, R>(capacity: Int, body: fn(Writer<StreamError>) -> Void errors E needs R)
  -> Reader<E | StreamError>
pub fn from_bytes(bytes: Bytes) -> Reader<StreamError>

pub fn read<E>(reader: Reader<E>) -> Option<Bytes>        // fails with E
pub fn stop<E>(reader: Reader<E>) -> Void
pub fn write<E>(writer: Writer<E>, bytes: Bytes) -> Void  // fails with E
pub fn close<E>(writer: Writer<E>) -> Void                // fails with E
pub fn read_all<E>(reader: Reader<E>, limit: Int) -> Bytes
pub fn copy<E, F>(reader: Reader<E>, writer: Writer<F>) -> Int
```

### Shape

- `Reader<E>` and `Writer<E>` MUST be opaque structs whose operations are the
  functions above. `E` is what their operations fail with, so a file reader
  can fail with an IO error and a pipe with `StreamError`.
- `Stream.reader` and `Stream.writer` build a stream from functions, for host
  streams and test fakes. They add no buffering. `Stream.reader` MUST skip an
  empty chunk from `next`. The other reading rules below (stopping twice, a
  read after a stop) bind pipes and produced streams; for a host stream they
  are obligations on its `next` and `stop`.
- The functions a stream is built from need no capabilities: a stream's
  authority comes from the handle it was opened with, as an opened file needs
  no file-system provider to read it. Opening a file or a socket, once those
  modules exist, is where `needs` appears.

### Reading

- `read` MUST return the next chunk, or `None` once the stream has ended. A
  chunk MUST NOT be empty. After `None`, every later `read` of a pipe or a
  produced stream returns `None` again.
- `read` takes no size. A chunk is whatever the writer wrote; a reader that
  needs a bound uses `read_all`'s limit.
- `stop` ends a reader early. A later `read` fails with `StreamError.Aborted`
  (the stream did not end, so its bytes may be incomplete), and the
  producer's next `write` fails with `StreamError.Closed`. Stopping again
  MUST do nothing, and `stop` itself MUST NOT fail, so it can be a release.

### Writing

- `write` MUST hand `bytes` to the reader as one chunk, and MUST write
  nothing for empty `bytes`. On a pipe it waits while `capacity` chunks are
  already waiting.
- `close` ends the stream: the reader gets every chunk written before it, and
  then `None`. A write after `close` fails with `StreamError.Closed`.

### Pipes and produced streams

- `pipe(capacity)` MUST return a connected writer and reader with at most
  `capacity` chunks between them. If the task that holds the writer ends
  without closing it, the reader MUST fail with `StreamError.Aborted`, never
  end as if the stream were complete. Chunks it had not read yet MAY be lost,
  as an aborted channel discards them.
- `produce(capacity, body)` MUST run `body` once in a new child task of the
  caller, with the writer of a pipe of that capacity, and close the writer
  when `body` returns.
- If `body` fails, the reader MUST fail with that failure after every chunk
  written before it, and end after that. A defect in `body` follows the rules for any child
  task's defect.
- Stopping a produced reader makes `body`'s next write fail with
  `StreamError.Closed`, so a producer of an endless stream ends.
- Dropping a reader is not a stop: its producer stays parked until the task
  that started it ends. Code that may not read to the end SHOULD hold the
  reader with `Resource.scoped(open, Stream.stop, use)`.

### Whole streams

- `read_all(reader, limit)` MUST return every remaining byte as one `Bytes`
  when there are at most `limit` of them. As soon as more have arrived it MUST
  stop the reader and fail with `StreamError.TooLarge(limit)`, so a large or
  endless stream cannot fill memory.
- `copy(reader, writer)` MUST write every remaining chunk of `reader` to
  `writer`, in order, and return the number of bytes copied. It MUST NOT
  close `writer`, so several readers can be copied into one writer.

## Decisions

- **Structs of functions, not interfaces.** Chimi has no traits, so a stream
  is a struct whose one field is a function. Each stream keeps one function,
  taking a private request (`Next` or `Stop`, `Chunk` or `Finish`), because a
  channel end can be captured by only one closure.
- **Read after stop is `Aborted`, not `None`.** Without mutation a reader has
  nowhere to remember that it was stopped, so the pipe's receiver answers;
  `Aborted` also says truthfully that the stream did not end.
- **Producers run in a child task.** `produce` is the shape HTTP bodies and
  subprocess output will use. A child task gives backpressure through the
  channel and makes a stop reach the producer as a failed write. The body's
  failure travels on the same channel behind its chunks, so none are lost.
- **No needs row on a stream.** `Reader<E>` has no capability parameter, so
  every signature that passes a stream stays short; authority is taken once,
  where the stream is opened.

## Conformance

| Rule | Evidence (`compiler/chimi/tests/streams.rs`) |
| --- | --- |
| Chunks arrive in order, empty writes are skipped, `None` after the end | `a_pipe_gives_chunks_in_order_and_skips_empty_writes` |
| `read_all` joins chunks and fails past its limit | `read_all_joins_the_chunks_and_stops_past_the_limit` |
| A producer's failure reaches the reader; `stop` closes the producer and later reads abort | `a_producer_failure_reaches_the_reader_and_stop_closes_the_producer` |
| Backpressure | `a_produced_stream_waits_while_the_reader_is_behind` |
| `copy`, `from_bytes`, and the host constructors | `copy_from_bytes_and_host_fakes` |
| `Stream.stop` as a `Resource.scoped` release | `resource_scoped_with_stop_as_the_release_closes_the_producer` |
| A writer's task that ends without closing aborts the reader | `a_pipe_whose_writer_task_ends_without_closing_aborts` |
| A producer's failure comes after its chunks; an ended stream stays ended; a write after `close` fails; a second stop does nothing | `a_producer_failure_comes_after_its_chunks_and_closed_streams_stay_closed` |
| A host reader skips empty chunks | `a_host_reader_skips_empty_chunks` |
| Host streams, `read_all`'s limit on an endless reader, and `copy` failures on both backends | `std-tests/stream/host_streams.chimi` |

## Not covered yet

- File, socket, and subprocess streams come with `std/io`, `std/net`, and
  `std/exec` (Phases 6c and 7b), built with `Stream.reader` and
  `Stream.writer` over host operations.
- Transforms (gzip, TLS, line splitting) are ordinary functions from a
  `Reader` to a `Reader`, built on `produce`; none ship yet.
