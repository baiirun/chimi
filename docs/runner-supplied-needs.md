# Needs the runner supplies

In hello world, `main` needs a `console`, and nothing in the program creates
one:

```chimi
import std/io.{Console}

pub fn main() -> Void needs {
  console: Console
} {
  console.write_line("Hello, world!")
}
```

`console` is an ordinary need. It is declared and passed on like any other,
and code can swap it with `using`. What is special is only where it ends up.
When a need on one of the host interfaces below reaches `main` with no
`using` supplying it first, the program's **runner** supplies the real one.
The runner is what starts `main`, in a built program or under `chimi run`.
A test is started by the test runner, which supplies far less, so a test
never reads the machine by accident.

The editor shows this when you hover a need, `main`, or a host interface's
name. `chimi inspect` reports it in each need's `runner` field.

## The list

| Interface | Import | Need name on `main` | In a test |
| --- | --- | --- | --- |
| `Console` | `std/io` | any name | a captured console, shown when the test fails |
| `FileSystem` | `std/io` | any name | supply it with `using` |
| `Clock` | `std/clock` | `clock` | the virtual clock |
| `Random` | `std/random` | `random` | a generator seeded from the run seed |
| `Environment` | `std/env` | `environment` | supply it with `using` |
| `Files` | `std/fs` | `files` | supply it with `using` |
| `Signals` | `std/signal` | `signals` | supply it with `using` |
| `Commands` | `std/exec` | `commands` | supply it with `using` |
| `Terminal` | `std/terminal` | `terminal` | supply it with `using` |
| `Network` | `std/net` | `network` | supply it with `using` |
| `Log` | `std/log` | `log` | supply it with `using` |
| `Metrics` | `std/metrics` | `metrics` | supply it with `using` |

## The name matters

Most host needs are supplied only under the name in the table. `clock: Clock`
is supplied, but `time: Clock` is an ordinary need, so `main` must supply it
with `using`, or the program fails to build with `E5002`. One name per host
interface keeps every function that reads the clock on the same need.
`Console` and `FileSystem` are older and still accept any name.

## In tests

A test gets the virtual clock, a seeded random source and a captured console.
Every other host need must be supplied with `using`, or the test is rejected
with `T4400`. A fake is a record of the interface's functions:

```chimi
import std/log as Log.{Log}

fn work() -> Void needs { log: Log } {
  Log.info("working", [])
}

test "work runs without a real log" {
  work() using { log: { write: fn(_) { Void } } }
}
```

The rules are in the specification:
[system modules](specification/system-modules.md#runner-provided-capabilities),
[the test provider plan](specification/testing.md#the-test-provider-plan) and
[time](specification/time-and-outcomes.md).
