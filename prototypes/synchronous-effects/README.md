# Synchronous Effect Composition Prototype

**PROTOTYPE — throwaway design evidence.**

Question: can Chimi add one synchronous `io` effect row to callable contracts
without confusing effects with dependency implementations, expected failures,
data inputs, or runner configuration?

Run the interactive model with:

```sh
python3 prototypes/synchronous-effects/main.py
```

The model starts with a pure CLI function. Add filesystem and console calls,
provision their dependencies, recover failures, and ask whether the result is
valid in a pure case guard. Every action renders the complete callable shape.

The candidate source shape is:

```chimi
interface FileSystem {
  fold_utf8_lines: fn<A, E, F>(
    Path,
    A,
    fn(A, Text) -> A errors E effects F,
  ) -> A errors E + FileError effects F + { io }
}

interface Console {
  write_line: fn(Text) -> Void errors ConsoleError effects { io }
}

fn summarize(arguments: List<Text>) -> Void
  errors UsageError + FileError + ConsoleError
  needs { files: FileSystem, console: Console }
  effects { io }
```

`FileSystem` and `Console` name implementations that a caller must provide.
`io` says what execution may do. `UsageError`, `FileError`, and `ConsoleError`
are expected values. Arguments remain ordinary data, and trace/cancellation
policy remains outside the callable in runner configuration.

The first resource API is scope-owning: `fold_utf8_lines` opens and closes the
file internally and does not expose a handle that could escape. General
resource and lifetime syntax is not part of this prototype.
