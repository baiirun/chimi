# Trace Data Surface Prototype

> PROTOTYPE: this is throwaway evidence for `ts-2ac2de`, not a Chimi
> specification or standard-library implementation.

## Question

Can one small immutable API surface decode the current runtime JSONL, represent
external JSON null without nullable Chimi values, update dynamic text-keyed
counts, process lines without retaining them, and render deterministic output?

The prototype models the proposed surface as pure transitions over immutable
running state. Its terminal shell shows the source shape beside concrete record
sequences, including invalid transitions that must fail without producing a
partial summary.

## Run

```sh
python3 prototypes/trace-data-surface/main.py
```

Print every scenario without interaction:

```sh
python3 prototypes/trace-data-surface/main.py --all
```

## Candidate surface

- `Json.Value` is a closed external-data model with an explicit `Value.Null`.
- `Json.decode` produces that model; domain decoding remains ordinary Chimi.
- `Map<K, V>` is the keyed collection; the first implementation supports
  compiler-known scalar keys, including the required `Text` keys, while custom
  key behavior remains deferred.
- `Map.update(map, key, fn(Option<Value>) -> Value)` performs one immutable
  insert-or-update with explicit absence.
- `FileSystem.process_utf8_lines` carries running state through bounded lines;
  there is no general iterator protocol yet.
- JSON object encoding uses locale-independent UTF-8 byte order so output does
  not depend on map insertion order or host locale.

The prototype deliberately rejects nullable values, mutable map syntax,
reflection-derived domain decoding, a general lazy iterator abstraction, and a
new `for` statement. Those are not required to express the forcing program.
