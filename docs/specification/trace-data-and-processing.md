# Trace Data and Processing

## Purpose

Chimi needs a small official data surface that can implement
`chimi-trace-summary` as ordinary source code. The surface must decode runtime
JSONL, keep JSON null distinct from Chimi absence, aggregate dynamic keys
without general mutation, process input incrementally, and encode stable output
without turning trace processing into privileged syntax.

This contract is the accepted outcome of `prog show ts-2ac2de`. The interactive
evidence is retained on the throwaway `codex/trace-data-surface-prototype`
branch.

## Scope

This contract fixes:

- the JSON value model and the minimal decode, inspect, construct, and encode
  operations;
- an immutable `Map<K, V>` surface sufficient for text-keyed aggregation;
- scope-owned incremental UTF-8 line processing;
- deterministic JSON object ordering;
- allocation and retention boundaries visible to callers.

It does not add a general iterator protocol, lazy sequences, `yield`, a `for`
statement, mutable bindings or collections, reflection-derived codecs, custom
map key behaviors, streaming JSON across records, or a compatibility promise
for a broader collections library.

## Behavior Contract

### External JSON values

`Json.decode` MUST decode one complete JSON text into a closed `Value`:

```chimi
enum Value {
  Null
  Boolean(value: Bool)
  Number(value: Number)
  Text(value: Text)
  Array(values: List<Value>)
  Object(fields: Map<Text, Value>)
}
```

`Value.Null` is external data. It MUST NOT become a Chimi null, nil, sentinel,
or implicit absence. Chimi APIs continue to represent absence as `Option<T>`.
For example, a missing object member is `None`, while a present member whose
JSON value is null is `Some(Value.Null)`.

`Number` is an opaque validated JSON number. The first surface does not invent
a floating-point or decimal language type merely to hold external JSON. It MUST
support exact conversion to `Int` when the represented value is an in-range
integer and return a failure value otherwise.

The decoder MUST reject malformed JSON, invalid UTF-8 supplied outside the line
boundary, excess configured nesting, and duplicate object member names. It
MUST NOT silently choose one duplicate member. Domain decoding remains ordinary
Chimi functions and exhaustive patterns; no struct derivation, reflection, or
typed-decoder syntax is implicit.

### Immutable maps

The official keyed collection is spelled `Map<K, V>`, including the forcing
program's `Map<Text, Int>` counts and JSON's `Map<Text, Value>` objects. The
minimum behavior is equivalent to:

```chimi
Map.empty<K, V>() -> Map<K, V>
Map.get<K, V>(Map<K, V>, K) -> Option<V>
Map.put<K, V>(Map<K, V>, K, V) -> Map<K, V>
Map.update<K, V, F, R, X>(
  Map<K, V>,
  K,
  fn(Option<V>) -> V errors F needs R effects X,
) -> Map<K, V> errors F needs R effects X
Map.map_values<K, V, W, F, R, X>(
  Map<K, V>,
  fn(V) -> W errors F needs R effects X,
) -> Map<K, W> errors F needs R effects X
```

`Map.get` MUST distinguish absence with `Option`. `Map.update` MUST call its
function exactly once with `None` or `Some(current)`, then return a new logical
map containing the returned value. Callback failures, needs, and effects MUST
propagate independently. The prior map remains a valid value; source syntax
does not expose in-place mutation or collection identity.

The first implementation MUST support `Text` keys. Other compiler-known scalar
keys MAY be added only with stable equality and hashing. Caller-defined
equality, hashing, or ordering and automatic structural keys are deferred until
the language has a concrete behavior-constraint use case.

`Map` MUST NOT promise enumeration order based on insertion or hashing. A
single update MUST NOT require copying or allocating storage proportional to
every existing entry. The implementation may use structural sharing or reuse
uniquely owned storage, but that optimization is unobservable.

### Incremental UTF-8 lines

The scope-owned filesystem operation is named `process_utf8_lines`. Its
contract is equivalent to:

```chimi
interface FileSystem {
  process_utf8_lines: fn<A, F, R, X>(
    Path,
    A,
    fn(A, Text) -> A errors F needs R effects X,
  ) -> A
    errors F + FileError + Utf8Error
    needs R
    effects X + { io }
}
```

The second argument is the initial running state. The callback receives that
state and one line, and its successful value becomes the state for the next
line. The operation returns the final state. It MUST invoke callbacks in input
order, MUST NOT retain completed lines, and MUST stop before reading another
line when the callback fails, defects, or is cancelled.

Line text excludes LF and the CRLF terminator. Empty physical lines are emitted
as empty `Text`; the trace program rejects them as a domain rule. A final line
without a terminator is emitted once. An empty input invokes no callbacks and
returns the initial state.

Text passed to Chimi MUST already be valid UTF-8. Invalid input produces an
expected `Utf8Error` before invoking the callback for that line. The provider
MUST enforce the runner's configured maximum line bytes without allowing its
buffer to grow beyond the documented bound, and MUST return a structured
`FileError.LineTooLong` rather than truncate input.

The operation owns opening and cleanup as specified by the synchronous host
capability contract. Raw streams and handles MUST NOT escape through this API.

### Deterministic JSON output

The minimum construction and encoding surface is:

```chimi
Json.decode(Text) -> Value errors DecodeError
Json.encode(Value) -> Text
Json.boolean(Bool) -> Value
Json.int(Int) -> Value
Json.text(Text) -> Value
Json.array(List<Value>) -> Value
Json.object(Map<Text, Value>) -> Value
Json.number_to_int(Number) -> Int errors NumberError
Text.equal(Text, Text) -> Bool
```

`Json.encode` MUST recursively sort object keys by ascending UTF-8 bytes. It
MUST NOT depend on `Map` storage order, insertion order, randomized hash seeds,
host locale, or process state. Arrays retain list order.

The encoder emits no insignificant whitespace and does not append a newline;
`Console.write_line` owns the application line terminator. It emits non-ASCII
text directly as UTF-8, does not escape `/`, escapes quotes and backslashes,
uses the short JSON escapes for backspace, form feed, newline, carriage return,
and tab, and uses lowercase four-digit Unicode escapes for other control
characters. `Json.int` encodes canonical base-ten integers. A decoded opaque
`Number` retains its validated source spelling when re-encoded.
`Text.equal` compares Unicode scalar sequences exactly and performs no locale,
case, or normalization folding. It remains a general text operation; trace
identity is application policy.

## Interfaces

The module origin remains explicit at use sites:

```chimi
import std/json as Json
import std/json.{Value}
import std/map as Map
import std/map.{Map}

fn increment(counts: Map<Text, Int>, key: Text) {
  Map.update(counts, key, fn(current) {
    case current {
      Some(count) -> count + 1
      None -> 1
    }
  })
}
```

The explicit member imports bind the data types; `Json.decode`, `Json.encode`,
and `Map.update` retain visible behavior origins. Standard-library status does
not inject behavior names into local scope.

A representative processing boundary is:

```chimi
FileSystem.process_utf8_lines(path, Summary.empty(), fn(state, line) {
  let record = line |> Json.decode |> TraceRecord.decode
  Summary.add(state, record)
})
```

If any line fails, this expression produces only its failure channel. A partial
`Summary` is not returned and therefore cannot accidentally reach application
stdout. A domain failure MAY carry the one-based line number maintained in the
ordinary running state.

## Implementation Model

The standard library owns JSON values/codecs and immutable map behavior. The
filesystem dependency and runtime own bounded byte buffering, UTF-8 validation,
resource cleanup, and cancellation checks. Trace-specific record validation
and aggregation remain in Chimi application modules.

At most the configured line buffer, the current decoded JSON tree, the running
state, and provider/runtime overhead need remain live during processing. The
JSON tree for a completed line becomes unreachable before the next callback.
Map state grows with distinct aggregation keys, not total records. The line
processor MUST use constant call-stack space across the number of lines.

`Map.map_values` allocates work proportional to its entry count. `Json.encode`
allocates the returned `Text` and MAY allocate temporary object-key ordering
storage proportional to the largest encoded object. Those costs are bounded by
the output and distinct-key sizes, not input record count. Native reclamation
and measured peak-live enforcement remain tracked by `prog show ts-da83af`.

## Rejected Surface

The following source forms and models are deliberately absent:

- Chimi `null`, `nil`, nullable values, or using JSON null as `None`;
- mutable map literals, assignment-based counting, or in-place update syntax;
- treating anonymous records and maps as the same value shape;
- implicit JSON-to-struct decoding, reflection, derives, or schema annotations;
- general `Iterator`, lazy `Sequence`, `yield`, generator, or `for` syntax;
- a public raw line stream or file handle;
- caller-supplied hash/equality functions or a new trait constraint solely for
  this map use;
- a trace-specific JSON primitive or aggregation operation in the language or
  runtime.

These choices can be reconsidered only when another maintained program supplies
a use case not expressible through the accepted functions-as-values surface.

## Verification

The standard-library implementation ticket MUST prove:

1. every current runtime event and terminal outcome JSONL shape decodes;
2. missing members and present `Value.Null` remain distinguishable;
3. malformed JSON, duplicate keys, invalid numeric conversion, invalid UTF-8,
   excessive nesting, blank trace lines, and oversized lines fail explicitly;
4. `Map.update` handles absent and present keys, invokes its function once, and
   leaves retained prior maps unchanged;
5. callback failures, needs, and effects propagate through map transformations
   and line processing;
6. LF, CRLF, empty lines, empty input, and an unterminated final line follow the
   stated line contract;
7. differently inserted but equivalent JSON objects encode byte-for-byte
   identically, including nested objects and non-ASCII keys;
8. a processing failure cannot emit a partial summary;
9. generated large input retains memory proportional only to configured line
   size, current JSON, distinct keys, output, and fixed runtime overhead.

Unit tests should exercise pure JSON, map, and summary behavior. Integration
tests should use the real filesystem provider and native executable. The
performance proof belongs to `ts-da83af`; the behavior implementation belongs
to `ts-41796d`.
