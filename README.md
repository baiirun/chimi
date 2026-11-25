# Chimi

Chimi is a programming language for durable networked services: HTTP APIs,
background workers and real-time services. It also covers command-line tools,
and aims to grow into a general-purpose language from there. A Chimi program
compiles to one native executable that carries its own runtime.

<!-- chimi: check -->
```chimi
import std/clock.{Clock}
import std/http as Http.{Method, Request, Response}
import std/net as Net.{Network}
import std/resource as Resource

fn handle(request: Request) -> Response {
  case (Http.method(request), Http.path(request)) {
    (Method.Get, ["health"]) -> Http.text(200, "ok")
    (Method.Get, ["hello", name]) -> Http.text(200, "Hello, ${name}!")
    _ -> Http.text(404, "not found")
  }
}

pub fn main() -> Void needs {
  network: Network,
  clock: Clock,
} {
  Resource.scoped(fn() { Net.listen("127.0.0.1", 8080) }, Net.close_listener, fn(listener) {
    Http.serve(listener, Http.defaults, handle)
  })
}
```

```sh
$ chimi build server.chimi -o server
$ ./server &
$ curl localhost:8080/hello/chimi
Hello, chimi!
```

Each connection runs in its own lightweight process. A handler that crashes
ends only its own connection, and the server carries on.

## Status

Chimi is early and unreleased. It is built by one person for their own
programs first, and the syntax, semantics and standard library still change
when real programs show something is wrong. There are no releases or
packages to install yet: you build the toolchain from source. It runs on
Linux and macOS.

## What Chimi is like

- **Failures are values.** A function that can fail returns its failure as a
  value, and the compiler tracks which failures each function can produce.
  There are no exceptions and no null. Bugs and broken invariants are
  defects instead: they crash one bounded unit of work, such as a process,
  which a supervisor can restart.
- **Dependencies are in the signature.** What a function needs from the
  outside world (a clock, the network, a database, a mailer) is part of its
  type, written in a `needs` clause or inferred. Callers supply
  implementations with `using`, so a test swaps in a fake without a mocking
  framework.
- **Processes and supervision.** Concurrency is lightweight processes with
  mailboxes, supervisors that restart them, and structured tasks with
  timeouts, deadlines, races and cancellation. Ideas come from Erlang and
  Elixir's BEAM, and processes run across every core by default.
- **Familiar, expressive code.** Enums that carry data, exhaustive pattern
  matching, multi-clause functions, `|>` pipelines, closures, and blocks and
  `case` as expressions. Values are immutable; a `loop` carries its state
  from one pass to the next and `break` returns a value.
- **Explicit at the call site.** Functions from other modules are always
  called through their module, as `List.map` or `Http.serve`, and every
  module name is written at its import. Types are inferred, and the
  toolchain shows the inferred types, failures and needs.
- **Built to be inspected.** A compiled program can write a structured
  trace of what ran, what failed and why, and `chimi trace show` reads it
  back. Logs and metrics are structured too, and export to OpenTelemetry.
- **One toolchain.** `chimi` formats, checks, tests, lints, documents and
  builds. Formatting is canonical with no options.

## A short tour

### Failures as values

`Int.parse` can fail, and so can `parse_price`. Neither signature says so:
the failures are inferred, and a caller either handles them with `case` or
lets them pass on to its own caller.

<!-- chimi: check -->
```chimi
import std/int as Int
import std/list as List
import std/test as Test
import std/text as Text

/// Why a price could not be read.
pub enum PriceError {
  Negative(cents: Int)
}

/// Reads a price in cents, such as "1250".
pub fn parse_price(text: Text) -> Int {
  let cents = text
  |> Text.trim
  |> Int.parse
  case cents < 0 {
    True -> Failure(PriceError.Negative(cents))
    False -> cents
  }
}

/// Totals the prices it can read and counts the ones it can't.
pub fn total(lines: List<Text>) -> (Int, Int) {
  List.reduce(lines, (0, 0), fn(state, line) {
    let (sum, skipped) = state
    case parse_price(line) {
      Success(cents) -> (sum + cents, skipped)
      Failure(_) -> (sum, skipped + 1)
    }
  })
}

test "reads a price" {
  parse_price(" 1250 ")
  |> Test.assert_equal(1250)
}

test "skips what it can't read" {
  total(["100", "abc", "-5", "250"])
  |> Test.assert_equal((350, 2))
}
```

```sh
$ chimi test prices.chimi
test main "reads a price" ... ok
test main "skips what it can't read" ... ok

2 passed, 0 failed

$ chimi doc prices.chimi
...
pub fn parse_price(text: Text) -> Int errors ParseError | PriceError
    Reads a price in cents, such as "1250".
```

### Needs

`sign_up` needs a `Mailer`. In production the program supplies a real one;
the test supplies a fake that records what it was asked to send.

<!-- chimi: check -->
```chimi
import std/test as Test

/// Why an email was not sent.
pub enum MailError {
  Bounced(address: Text)
}

/// Where emails go.
pub interface Mailer {
  send: fn(Text, Text) -> Void errors MailError
}

/// Signs `email` up and welcomes them.
pub fn sign_up(email: Text) -> Void needs {
  mailer: Mailer
} {
  mailer.send(email, "Welcome to the shop!")
}

test "sign_up sends one welcome" {
  let sent = Test.recorder()
  let fake = {
    send: fn(to: Text, _: Text) -> Void {
      Test.record(sent, to)
    },
  }
  sign_up("ada@example.com") using { mailer: fake }
  Test.recorded(sent)
  |> Test.assert_equal(["ada@example.com"])
}
```

The needs the runner provides, such as `clock`, `network` and `console`, are
reached the same way, and tests can replace them too. Tests run on a virtual
clock with seeded randomness, so a test that sleeps, retries or times out
takes no real time and gives the same result on every run.

### Processes and supervision

A process owns its state and receives messages one at a time. Here a counter
crashes on purpose (a division by zero is a defect), and its supervisor
starts a fresh one under the same name.

<!-- chimi: check -->
```chimi
import std/channel as Channel.{Receive, Sender}
import std/clock as Clock.{Clock}
import std/duration as Duration
import std/io.{Console}
import std/process as Process.{Inbox}
import std/supervisor as Supervisor

/// What the counter is asked to do.
pub enum Message {
  Add(amount: Int)
  Crash
  Total(reply: Sender<Int>)
}

/// The counter's registered name.
pub process name counter: Message

fn count(inbox: Inbox<Message>) -> Void {
  loop total = 0 {
    case Process.receive(inbox) {
      Message.Add(amount) -> total + amount
      Message.Crash -> total / 0
      Message.Total(reply) -> {
        Channel.send(reply, total)
        total
      }
    }
  }
}

fn total() -> Int {
  let replies = Channel.bounded(1)
  Process.send(counter, Message.Total(replies.sender))
  let reply = Channel.receive(replies.receiver)
  case reply {
    Receive.Received(total) -> total
    Receive.Drained -> -1
  }
}

pub fn main() -> Void needs {
  clock: Clock,
  console: Console,
} {
  let root = Supervisor.start(Supervisor.policy([], 1, Supervisor.restarts(3, Duration.seconds(5))))
  let spec = Process.spec(count, 16, Duration.seconds(1))
  |> Process.register(counter)
  Supervisor.start_process(root, spec, Supervisor.Restart.OnDefect)

  Process.send(counter, Message.Add(5))
  console.write_line("before the crash: ${total()}")

  Process.send(counter, Message.Crash)
  Clock.sleep(Duration.milliseconds(10))
  Process.send(counter, Message.Add(2))
  console.write_line("after the restart: ${total()}")

  Supervisor.stop(root)
}
```

```sh
$ chimi build counter.chimi -o counter && ./counter
before the crash: 5
after the restart: 2
```

For a larger example, [`programs/checkout-service`](programs/checkout-service)
runs two workers over a shared queue. Each checkout reserves stock and
scores fraud at the same time, retries the card charge with backoff and a
per-attempt timeout, races two shipping carriers, and runs under a deadline.

## Getting started

You need a Rust toolchain (1.92 or newer) and a C compiler on your `PATH`,
which `chimi build` also uses to link executables.

```sh
git clone https://github.com/baiirun/chimi
cd chimi
cargo install --path compiler/chimi --bin chimi
```

That installs `chimi` with the standard library and runtime built in.

Save this as `hello.chimi`:

<!-- chimi: check -->
```chimi
import std/io.{Console}

pub fn main() -> Void needs {
  console: Console
} {
  console.write_line("Hello, world!")
}
```

`console` is a need like any other: the runner supplies the real terminal to
`main`, and a test can supply a fake. Then:

```sh
chimi run hello.chimi              # run it on the reference interpreter
chimi build hello.chimi -o hello   # compile it to a native executable
./hello
chimi test .                       # run the tests in a directory
chimi format hello.chimi           # print the canonical formatting
```

`chimi run` is quick for trying things out, but the interpreter does not run
processes or C-backed modules such as `std/http`; build those.

The [editor guide](editors/README.md) covers the language server
(`compiler/chimi-lsp`) and the Zed extension.

## The toolchain

| Command | What it does |
| --- | --- |
| `chimi build` | Compiles a program and its runtime into one native executable. |
| `chimi test` | Runs `test` blocks and the examples in doc comments. |
| `chimi check` | Type-checks without building; `--json` gives machine-readable diagnostics. |
| `chimi run` | Runs a program on the reference interpreter; `--telemetry-file` and `--outcome-file` record its trace and outcome. |
| `chimi format` | Prints the canonical formatting of a file. |
| `chimi lint` | Reports unused imports, bindings, private code and needs, and missing doc comments. |
| `chimi doc` | Shows a module's public API, with inferred failures and needs; `--html` writes a site. |
| `chimi inspect` | Shows what the compiler knows at a position in a file. |
| `chimi trace` | Summarizes a run's trace, or exports it to OpenTelemetry. |
| `chimi deps` | Updates the lock file of a package's dependencies. |

A project is a directory with a `chimi.toml`. Dependencies are other packages
by path or git, and packages carry Chimi source only.

## The standard library

Most of the standard library is written in Chimi, and each function's
documentation states its cost. `chimi doc std/list` shows one module, and
`chimi doc --html <directory> std` writes them all as a site.

- **Data:** `list`, `map`, `set`, `text`, `bytes`, `int`, `decimal`,
  `option`, `result`, `order`, `json`
- **Services:** `http`, `net`, `tls`, `sql`, `sqlite`, `postgres`, `retry`,
  `schedule`, `resource`, `stream`
- **Concurrency:** `process`, `supervisor`, `task`, `channel`, `broadcast`,
  `watch`, `select`, `signal`
- **Observability:** `log`, `metrics`, `otel`, `cause`, `debug`
- **System:** `cli`, `env`, `exec`, `fs`, `path`, `terminal`, `random`,
  `clock`, `duration`, `io`
- **Testing:** `test`

## What Chimi is not for

Chimi does not aim at browser frontends, embedded or kernel work, or
hard real-time systems. It is not designed for beginners first. There is no
`unsafe` in ordinary code, and no macros or compiler plugins yet.

## Learn more

- [Design philosophy](docs/design-philosophy.md): what Chimi is for and the
  principles behind it.
- [Glossary](CONTEXT.md): the terms the project uses.
- [Decision ledger](docs/decisions.md): every language and toolchain
  decision, newest first, with links to the reasoning.
- [Specification](docs/specification): the contracts the compiler and runtime
  follow.
- [Example programs](programs): complete programs, including the checkout
  service, a supervised pool and an OpenTelemetry worker.

The examples in this README are compiled by the test suite
(`compiler/chimi/tests/doc_examples.rs`), so they stay valid as the language
changes.
