//! Conformance for `std/sql`'s driver-independent layer (CHI-258): reading
//! rows, the pool (lazy connections, reuse, resets, its size and wait), and
//! transactions (commit, rollback, defects, cancellation, savepoints). Every
//! test runs over `Sql.fake`, a driver that records the calls it gets, under
//! virtual time, so the pool's waits pass at once. Each test writes
//! `cases.chimi` to a scratch directory and runs the CLI there.

use std::path::PathBuf;
use std::process::Command;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str, source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "chimi-sql-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        if path.exists() {
            std::fs::remove_dir_all(&path).unwrap();
        }
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("cases.chimi"), source).unwrap();
        Self(path)
    }

    fn chimi(&self, arguments: &[&str]) -> Run {
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&self.0)
            .args(arguments)
            .output()
            .expect("run chimi");
        Run {
            status: output.status.code(),
            stdout: String::from_utf8(output.stdout).unwrap(),
            stderr: String::from_utf8(output.stderr).unwrap(),
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Run {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

/// A fake driver that records every call and answers a few statements:
/// `select one` gives one row with a column of each kind, `select none` no
/// rows, `select two` two rows, `fail` a constraint failure, and any other
/// statement changes as many rows as it has parameters.
const PRELUDE: &str = r#"import std/bytes as Bytes
import std/channel as Channel
import std/clock as Clock
import std/clock.{Clock}
import std/debug as Debug
import std/decimal as Decimal
import std/duration as Duration
import std/list as List
import std/option as Option
import std/result as Result
import std/result.{Err, Ok}
import std/sql as Sql
import std/sql.{Call, Database, SqlError, Value}
import std/task as Task
import std/test as Test
import std/test.{Recorder}

enum Oops {
  Oops
}

fn answer(calls: Recorder<Call>, call: Call) -> Sql.Reply {
  Test.record(calls, call)
  case call {
    Call.Ran("select one", _) -> Sql.reply(["id", "name", "score", "price", "active", "blob", "email"], [[Value.Int(7), Value.Text("Ada"), Value.Float(1.5), Value.Decimal(Decimal.of(1250, 2)), Value.Bool(True), Value.Bytes(Bytes.from([1, 2])), Value.Null]], 0)
    Call.Ran("select none", _) -> Sql.reply(["id"], [], 0)
    Call.Ran("select two", _) -> Sql.reply(["id"], [[Value.Int(1)], [Value.Int(2)]], 0)
    Call.Ran("fail", _) -> Failure(SqlError.ConstraintFailed("users_email_key", "duplicate"))
    Call.Ran(_, params) -> Sql.reply([], [], List.length(params))
    _ -> Sql.done()
  }
}

fn recording(calls: Recorder<Call>) -> Sql.Driver {
  Sql.fake(fn(call) { answer(calls, call) })
}

// The failure of `operation`, widened to the whole `SqlError`, or `None`.
fn failed<T, R>(operation: fn() -> T errors SqlError needs R) -> Option<SqlError> needs R {
  case Result.capture(operation) {
    Ok(_) -> None
    Err(error) -> Some(error)
  }
}

fn ran(sql: Text) -> Call {
  Call.Ran(sql, [])
}
"#;

fn run_tests(name: &str, tests: &str) -> Run {
    Scratch::new(name, &format!("{PRELUDE}\n{tests}")).chimi(&[
        "test",
        "cases.chimi",
        "--clock=virtual",
    ])
}

fn assert_passed(run: &Run, count: usize) {
    assert_eq!(run.status, Some(0), "{}{}", run.stdout, run.stderr);
    assert!(
        run.stdout.contains(&format!("{count} passed, 0 failed")),
        "{}",
        run.stdout
    );
}


// Rejects a row reader that misreads a column's kind, reads a null as a
// value, or finds a column that is not there, and `one` and `first` that
// miscount rows.
#[test]
fn rows_are_read_by_column_name_with_typed_accessors() {
    let run = run_tests(
        "rows",
        r#"
test "each kind of column" {
  let calls = Test.recorder()
  Sql.open(recording(calls), "app.db", Sql.defaults, fn(db) {
    let row = Sql.one(db, "select one", [])
    Sql.int(row, "id") |> Test.assert_equal(7)
    Sql.text(row, "name") |> Test.assert_equal("Ada")
    Sql.float(row, "score") |> Test.assert_equal(1.5)
    Sql.decimal(row, "price") |> Test.assert_equal(Decimal.of(1250, 2))
    Sql.bool(row, "active") |> Test.assert_equal(True)
    Sql.bytes(row, "blob") |> Test.assert_equal(Bytes.from([1, 2]))
    Sql.nullable(row, "email", Sql.text) |> Test.assert_equal(None)
    Sql.nullable(row, "id", Sql.int) |> Test.assert_equal(Some(7))
    Sql.value(row, "email") |> Test.assert_equal(Value.Null)
    List.length(Sql.columns(row)) |> Test.assert_equal(7)
    List.first(Sql.values(row)) |> Test.assert_equal(Some(Value.Int(7)))
  })
}

test "reading the wrong thing fails" {
  let calls = Test.recorder()
  Sql.open(recording(calls), "app.db", Sql.defaults, fn(db) {
    let row = Sql.one(db, "select one", [])
    failed(fn() { Sql.int(row, "missing") }) |> Test.assert_equal(Some(SqlError.NoColumn("missing")))
    failed(fn() { Sql.int(row, "name") }) |> Test.assert_equal(Some(SqlError.WrongType("name", "Int", "Text")))
    failed(fn() { Sql.text(row, "email") }) |> Test.assert_equal(Some(SqlError.UnexpectedNull("email")))
    failed(fn() { Sql.nullable(row, "name", Sql.int) }) |> Test.assert_equal(Some(SqlError.WrongType("name", "Int", "Text")))
  })
}

test "one and first count rows" {
  let calls = Test.recorder()
  Sql.open(recording(calls), "app.db", Sql.defaults, fn(db) {
    failed(fn() { Sql.one(db, "select none", []) }) |> Test.assert_equal(Some(SqlError.NoRows))
    failed(fn() { Sql.one(db, "select two", []) }) |> Test.assert_equal(Some(SqlError.ManyRows))
    Sql.first(db, "select none", []) |> Test.assert_equal(None)
    Option.map(Sql.first(db, "select two", []), fn(row) { Sql.int(row, "id") }) |> Test.assert_equal(Some(1))
    List.length(Sql.query(db, "select two", [])) |> Test.assert_equal(2)
  })
}
"#,
    );
    assert_passed(&run, 3);
}

// Rejects a pool that opens a connection per statement, reuses one without
// resetting it, keeps one whose reset failed, leaks a slot when a connect
// fails, or leaves connections open when it closes.
#[test]
fn a_pool_opens_connections_lazily_reuses_them_and_closes_them() {
    let run = run_tests(
        "pool",
        r#"
test "one connection serves statements in turn, and closing closes it" {
  let calls = Test.recorder()
  let affected = Sql.open(recording(calls), "app.db", Sql.defaults, fn(db) {
    let first = Sql.execute(db, "insert ?", [Value.Int(1), Value.Text("x")])
    let second = Sql.execute(db, "update", [])
    first + second
  })
  affected |> Test.assert_equal(2)
  Test.recorded(calls) |> Test.assert_equal([Call.Connected("app.db"), Call.Ran("insert ?", [Value.Int(1), Value.Text("x")]), Call.Reset, ran("update"), Call.Reset, Call.Closed])
}

test "tasks share the pool" {
  let calls = Test.recorder()
  let total = Sql.open(recording(calls), "app.db", Sql.defaults, fn(db) {
    let a = Task.start(fn() { Sql.execute(db, "a", [Value.Int(1)]) })
    let b = Task.start(fn() { Sql.execute(db, "b", [Value.Int(1), Value.Int(2)]) })
    Task.result(a) + Task.result(b)
  })
  total |> Test.assert_equal(3)
}

test "a connection whose reset fails is closed, and the next statement opens another" {
  let calls = Test.recorder()
  let driver = Sql.fake(fn(call) {
    Test.record(calls, call)
    case call {
      Call.Reset -> Failure(SqlError.ConnectionLost)
      _ -> Sql.done()
    }
  })
  Sql.open(driver, "app.db", Sql.defaults, fn(db) {
    let _ = Sql.execute(db, "a", [])
    Sql.execute(db, "b", [])
  })
  Test.recorded(calls) |> Test.assert_equal([Call.Connected("app.db"), ran("a"), Call.Reset, Call.Closed, Call.Connected("app.db"), ran("b"), Call.Reset, Call.Closed])
}

test "a failed connect frees its slot" {
  let driver = Sql.fake(fn(call) {
    case call {
      Call.Connected(_) -> Failure(SqlError.ConnectFailed("refused"))
      _ -> Sql.done()
    }
  })
  let outcomes = Sql.open(driver, "app.db", Sql.defaults, fn(db) {
    List.map(List.repeat(Void, 12), fn(_) { failed(fn() { Sql.execute(db, "a", []) }) })
  })
  outcomes |> Test.assert_equal(List.repeat(Some(SqlError.ConnectFailed("refused")), 12))
}

test "a pool used after it closed fails" {
  let calls = Test.recorder()
  let escaped = Sql.open(recording(calls), "app.db", Sql.defaults, fn(db) { db })
  failed(fn() { Sql.execute(escaped, "a", []) }) |> Test.assert_equal(Some(SqlError.PoolClosed))
}
"#,
    );
    assert_passed(&run, 5);
}

// Rejects a pool without a size limit, a wait without a deadline, and a
// waiter that is not served when a connection comes free (virtual time, so
// the waits pass at once).
#[test]
fn a_full_pool_makes_statements_wait_up_to_its_limit() {
    let run = run_tests(
        "full",
        r#"
fn hold(db: Database, seconds: Int) -> Void needs { clock: Clock } {
  Sql.transaction(db, fn(_) { Clock.sleep(Duration.seconds(seconds)) })
}

fn later(db: Database, seconds: Int, sql: Text) -> Int needs { clock: Clock } {
  Clock.sleep(Duration.seconds(seconds))
  Sql.execute(db, sql, [])
}

test "the eleventh statement waits for a connection to come free" {
  let calls = Test.recorder()
  Sql.open(recording(calls), "app.db", Sql.defaults, fn(db) {
    let start = Clock.now()
    let holders = Task.start(fn() { Task.all(List.repeat(fn() { hold(db, 10) }, 10)) })
    Clock.sleep(Duration.seconds(1))
    Sql.execute(db, "late", [Value.Int(1)]) |> Test.assert_equal(1)
    Clock.since(start) |> Test.assert_equal(Duration.seconds(10))
    List.length(Task.result(holders)) |> Test.assert_equal(10)
  })
  List.count(Test.recorded(calls), fn(call) { call == Call.Connected("app.db") }) |> Test.assert_equal(10)
}

test "a statement that waits too long fails" {
  let calls = Test.recorder()
  let driver = Sql.fake(fn(call) {
    let dropping = List.contains(Test.recorded(calls), ran("drop"))
    let reply = answer(calls, call)
    case (call, dropping) {
      (Call.Reset, True) -> Failure(SqlError.ConnectionLost)
      _ -> reply
    }
  })
  Sql.open(driver, "app.db", Sql.defaults, fn(db) {
    let holders = Task.start(fn() { Task.all(List.repeat(fn() { hold(db, 40) }, 10)) })
    Clock.sleep(Duration.seconds(1))
    let start = Clock.now()
    failed(fn() { Sql.execute(db, "late", []) }) |> Test.assert_equal(Some(SqlError.PoolTimedOut(Duration.seconds(30))))
    Clock.since(start) |> Test.assert_equal(Duration.seconds(30))
    // The waiter that gave up holds no slot: the next statement gets the
    // first connection to come free, when the holders finish at 40 s.
    Sql.execute(db, "later", [Value.Int(1)]) |> Test.assert_equal(1)
    Clock.since(start) |> Test.assert_equal(Duration.seconds(39))
    List.length(Task.result(holders)) |> Test.assert_equal(10)
    // All ten slots are free again: once "drop" makes every reset fail, the
    // pool drops a connection after each use and has room to open ten new ones.
    let _ = Sql.execute(db, "drop", [])
    let again = Clock.now()
    List.length(Task.all(List.repeat(fn() { hold(db, 5) }, 10))) |> Test.assert_equal(10)
    Clock.since(again) |> Test.assert_equal(Duration.seconds(5))
  })
  let opened = List.count(Test.recorded(calls), fn(call) { call == Call.Connected("app.db") })
  let closed = List.count(Test.recorded(calls), fn(call) { call == Call.Closed })
  (opened, closed) |> Test.assert_equal((11, 11))
}

test "a pool applies the settings it is given" {
  let calls = Test.recorder()
  Sql.open(recording(calls), "app.db", { ..Sql.defaults, max_connections: 2, wait_timeout: Duration.seconds(3) }, fn(db) {
    let holders = Task.start(fn() { Task.all(List.repeat(fn() { hold(db, 10) }, 2)) })
    Clock.sleep(Duration.seconds(1))
    let start = Clock.now()
    failed(fn() { Sql.execute(db, "late", []) }) |> Test.assert_equal(Some(SqlError.PoolTimedOut(Duration.seconds(3))))
    Clock.since(start) |> Test.assert_equal(Duration.seconds(3))
    List.length(Task.result(holders)) |> Test.assert_equal(2)
  })
  List.count(Test.recorded(calls), fn(call) { call == Call.Connected("app.db") }) |> Test.assert_equal(2)
}

test "a reset slower than its timeout drops the connection" {
  let calls = Test.recorder()
  // A reset that never finishes: it waits on a channel nothing sends to.
  let never = Channel.bounded(1)
  let stuck = never.receiver
  let driver = Sql.driver(fn(target) {
    Test.record(calls, Call.Connected(target))
    Sql.connection(fn(_, _) { Sql.done() }, fn(_) { Void }, fn(_) { Void }, fn(_) { Void }, fn() {
      let _ = Result.capture(fn() { Channel.receive(stuck) })
      Void
    }, fn() { Void })
  })
  let start = Clock.now()
  Sql.open(driver, "app.db", { ..Sql.defaults, reset_timeout: Duration.seconds(1) }, fn(db) {
    let _ = Sql.execute(db, "a", [])
    Sql.execute(db, "b", [])
  })
  Clock.since(start) |> Test.assert_equal(Duration.seconds(2))
  let _ = never.sender
  List.count(Test.recorded(calls), fn(call) { call == Call.Connected("app.db") }) |> Test.assert_equal(2)
}

test "settings that cannot work are a defect" {
  Test.capture_defect(fn() { Sql.open(recording(Test.recorder()), "app.db", { ..Sql.defaults, max_connections: 0 }, fn(_) { Void }) })
    |> Test.assert_equal("Sql: max_connections, wait_timeout and reset_timeout must be positive")
  Test.capture_defect(fn() { Sql.open(recording(Test.recorder()), "app.db", { ..Sql.defaults, wait_timeout: Duration.zero }, fn(_) { Void }) })
    |> Test.assert_equal("Sql: max_connections, wait_timeout and reset_timeout must be positive")
}

test "waiting statements are served in the order they asked" {
  let calls = Test.recorder()
  Sql.open(recording(calls), "app.db", Sql.defaults, fn(db) {
    let holders = Task.start(fn() { Task.all(List.repeat(fn() { hold(db, 10) }, 10)) })
    let first = Task.start(fn() { later(db, 1, "first") })
    let second = Task.start(fn() { later(db, 2, "second") })
    let third = Task.start(fn() { later(db, 3, "third") })
    [Task.result(first), Task.result(second), Task.result(third)] |> Test.assert_equal([0, 0, 0])
    List.length(Task.result(holders)) |> Test.assert_equal(10)
  })
  List.filter(Test.recorded(calls), fn(call) { call == ran("first") or call == ran("second") or call == ran("third") })
    |> Test.assert_equal([ran("first"), ran("second"), ran("third")])
}

test "closing waits at most 30 s for connections tasks still hold" {
  let start = Clock.now()
  let outer = Task.start(fn() {
    Sql.open(recording(Test.recorder()), "app.db", Sql.defaults, fn(db) {
      let _ = Task.start(fn() { hold(db, 1000) })
      Clock.sleep(Duration.seconds(1))
      Void
    })
  })
  Task.result(outer)
  Clock.since(start) |> Test.assert_equal(Duration.seconds(31))
}
test "a connection returned while the pool closes is closed" {
  let calls = Test.recorder()
  let outer = Task.start(fn() {
    Sql.open(recording(calls), "app.db", Sql.defaults, fn(db) {
      let _ = Task.start(fn() { hold(db, 5) })
      Clock.sleep(Duration.seconds(1))
      Void
    })
  })
  Task.result(outer)
  Test.recorded(calls) |> Test.assert_equal([Call.Connected("app.db"), Call.Began(0), Call.Committed(0), Call.Reset, Call.Closed])
}

test "statements waiting when the pool closes fail at once" {
  let outcomes = Test.recorder()
  let start = Clock.now()
  let outer = Task.start(fn() {
    Sql.open(recording(Test.recorder()), "app.db", Sql.defaults, fn(db) {
      let _ = Task.start(fn() { Task.all(List.repeat(fn() { hold(db, 50) }, 10)) })
      let ask = fn(delay) {
        Task.start(fn() {
          Clock.sleep(Duration.seconds(delay))
          let outcome = failed(fn() { Sql.execute(db, "late", []) })
          Test.record(outcomes, (outcome, Clock.since(start)))
        })
      }
      let _ = ask(1)
      let _ = ask(3)
      Clock.sleep(Duration.seconds(2))
      Void
    })
  })
  Task.result(outer)
  Test.recorded(outcomes) |> Test.assert_equal([
    (Some(SqlError.PoolClosed), Duration.seconds(2)),
    (Some(SqlError.PoolClosed), Duration.seconds(3)),
  ])
}
"#,
    );
    assert_passed(&run, 9);
}

// Rejects a transaction that commits after a failure, leaves one open after
// a defect or a cancellation, swallows a commit failure, or nests by
// committing or rolling back the outer transaction.
#[test]
fn a_transaction_commits_or_rolls_back_however_its_body_ends() {
    let run = run_tests(
        "transactions",
        r#"
test "success commits" {
  let calls = Test.recorder()
  let result = Sql.open(recording(calls), "app.db", Sql.defaults, fn(db) {
    Sql.transaction(db, fn(tx) {
      let a = Sql.execute(tx, "a", [Value.Int(1)])
      let b = Sql.execute(tx, "b", [Value.Int(1)])
      a + b
    })
  })
  result |> Test.assert_equal(2)
  Test.recorded(calls) |> Test.assert_equal([Call.Connected("app.db"), Call.Began(0), Call.Ran("a", [Value.Int(1)]), Call.Ran("b", [Value.Int(1)]), Call.Committed(0), Call.Reset, Call.Closed])
}

test "a failure rolls back and is the outcome" {
  let calls = Test.recorder()
  let outcome = Result.capture(fn() {
    Sql.open(recording(calls), "app.db", Sql.defaults, fn(db) {
      Sql.transaction(db, fn(tx) {
        let _ = Sql.execute(tx, "a", [])
        Failure(Oops.Oops)
      })
    })
  })
  outcome |> Test.assert_equal(Err(Oops.Oops))
  Test.recorded(calls) |> Test.assert_equal([Call.Connected("app.db"), Call.Began(0), ran("a"), Call.RolledBack(0), Call.Reset, Call.Closed])
  let failed = Test.recorder()
  Result.capture(fn() { Sql.open(recording(failed), "app.db", Sql.defaults, fn(db) { Sql.transaction(db, fn(tx) { Sql.execute(tx, "fail", []) }) }) })
    |> Test.assert_equal(Err(SqlError.ConstraintFailed("users_email_key", "duplicate")))
}

test "a defect or a cancellation leaves no transaction open" {
  let calls = Test.recorder()
  Test.capture_defect(fn() {
    Sql.open(recording(calls), "app.db", Sql.defaults, fn(db) { Sql.transaction(db, fn(_) { Debug.crash("boom") }) })
  }) |> Test.assert_equal("boom")
  Test.recorded(calls) |> Test.assert_equal([Call.Connected("app.db"), Call.Began(0), Call.Reset, Call.Closed])
  let cancelled = Test.recorder()
  let timed = Result.capture(fn() {
    Task.timeout(fn() {
      Sql.open(recording(cancelled), "app.db", Sql.defaults, fn(db) {
        Sql.transaction(db, fn(_) { Clock.sleep(Duration.seconds(100)) })
      })
    }, Duration.seconds(1))
  })
  let timed_out = case timed {
    Ok(_) -> False
    Err(_) -> True
  }
  timed_out |> Test.assert_equal(True)
  Test.recorded(cancelled) |> Test.assert_equal([Call.Connected("app.db"), Call.Began(0), Call.Reset, Call.Closed])
}

test "a failed commit is the outcome" {
  let calls = Test.recorder()
  let driver = Sql.fake(fn(call) {
    Test.record(calls, call)
    case call {
      Call.Committed(0) -> Failure(SqlError.Conflict("serialization"))
      _ -> Sql.done()
    }
  })
  Result.capture(fn() { Sql.open(driver, "app.db", Sql.defaults, fn(db) { Sql.transaction(db, fn(tx) { Sql.execute(tx, "a", []) }) }) })
    |> Test.assert_equal(Err(SqlError.Conflict("serialization")))
}

test "a transaction inside one is a savepoint" {
  let calls = Test.recorder()
  Sql.open(recording(calls), "app.db", Sql.defaults, fn(db) {
    Sql.transaction(db, fn(tx) {
      let _ = Sql.execute(tx, "a", [])
      let inner = Result.capture(fn() { Sql.transaction(tx, fn(nested) { Sql.execute(nested, "fail", []) }) })
      inner |> Test.assert_equal(Err(SqlError.ConstraintFailed("users_email_key", "duplicate")))
      Sql.transaction(tx, fn(nested) { Sql.transaction(nested, fn(deeper) { Sql.execute(deeper, "b", []) }) })
    })
  })
  Test.recorded(calls) |> Test.assert_equal([
    Call.Connected("app.db"), Call.Began(0), ran("a"),
    Call.Began(1), ran("fail"), Call.RolledBack(1),
    Call.Began(1), Call.Began(2), ran("b"), Call.Committed(2), Call.Committed(1),
    Call.Committed(0), Call.Reset, Call.Closed,
  ])
}
test "a savepoint cut off by a timeout the outer body survives is undone" {
  let calls = Test.recorder()
  Sql.open(recording(calls), "app.db", Sql.defaults, fn(db) {
    Sql.transaction(db, fn(tx) {
      let _ = Sql.execute(tx, "a", [])
      let _ = Result.capture(fn() {
        Task.timeout(fn() {
          Sql.transaction(tx, fn(nested) {
            let _ = Sql.execute(nested, "b", [])
            Clock.sleep(Duration.seconds(5))
          })
        }, Duration.seconds(1))
      })
      Void
    })
  })
  Test.recorded(calls) |> Test.assert_equal([
    Call.Connected("app.db"), Call.Began(0), ran("a"),
    Call.Began(1), ran("b"), Call.RolledBack(1),
    Call.Committed(0), Call.Reset, Call.Closed,
  ])
}

test "a savepoint's failure stays its failure when the rollback fails" {
  let driver = Sql.fake(fn(call) {
    case call {
      Call.RolledBack(1) -> Failure(SqlError.Rejected("25P02", "aborted"))
      Call.Ran("fail", _) -> Failure(SqlError.ConstraintFailed("c", "duplicate"))
      _ -> Sql.done()
    }
  })
  Sql.open(driver, "app.db", Sql.defaults, fn(db) {
    Sql.transaction(db, fn(tx) {
      failed(fn() { Sql.transaction(tx, fn(nested) { Sql.execute(nested, "fail", []) }) })
    })
  }) |> Test.assert_equal(Some(SqlError.ConstraintFailed("c", "duplicate")))
}

test "a driver writes its own transaction and savepoint statements" {
  let statements = Test.recorder()
  // A SQL Server-style dialect: no RELEASE, so committing a savepoint sends
  // nothing, and rolling one back is one statement.
  let driver = Sql.driver(fn(_) {
    let send = fn(sql) { Test.record(statements, sql) }
    Sql.connection(fn(sql, _) {
      send(sql)
      case sql == "fail" {
        True -> Failure(SqlError.ConstraintFailed("c", "duplicate"))
        False -> Sql.done()
      }
    }, fn(depth) {
      case depth {
        0 -> send("BEGIN TRANSACTION")
        n -> send("SAVE TRANSACTION s${n}")
      }
    }, fn(depth) {
      case depth {
        0 -> send("COMMIT TRANSACTION")
        _ -> Void
      }
    }, fn(depth) {
      case depth {
        0 -> send("ROLLBACK TRANSACTION")
        n -> send("ROLLBACK TRANSACTION s${n}")
      }
    }, fn() { Void }, fn() { Void })
  })
  Sql.open(driver, "app.db", Sql.defaults, fn(db) {
    Sql.transaction(db, fn(tx) {
      let _ = Result.capture(fn() { Sql.transaction(tx, fn(nested) { Sql.execute(nested, "fail", []) }) })
      Sql.transaction(tx, fn(nested) { Sql.execute(nested, "b", []) })
    })
  })
  Test.recorded(statements) |> Test.assert_equal([
    "BEGIN TRANSACTION", "SAVE TRANSACTION s1", "fail", "ROLLBACK TRANSACTION s1",
    "SAVE TRANSACTION s1", "b", "COMMIT TRANSACTION",
  ])
}

test "a savepoint whose commit fails is undone and fails" {
  let calls = Test.recorder()
  let driver = Sql.fake(fn(call) {
    Test.record(calls, call)
    case call {
      Call.Committed(1) -> Failure(SqlError.Conflict("release"))
      _ -> Sql.done()
    }
  })
  Sql.open(driver, "app.db", Sql.defaults, fn(db) {
    Sql.transaction(db, fn(tx) {
      failed(fn() { Sql.transaction(tx, fn(nested) { Sql.execute(nested, "b", []) }) })
    })
  }) |> Test.assert_equal(Some(SqlError.Conflict("release")))
  Test.recorded(calls) |> Test.assert_equal([
    Call.Connected("app.db"), Call.Began(0),
    Call.Began(1), ran("b"), Call.Committed(1), Call.RolledBack(1),
    Call.Committed(0), Call.Reset, Call.Closed,
  ])
}

test "a savepoint that defects is undone and the defect is the outcome" {
  let calls = Test.recorder()
  Test.capture_defect(fn() {
    Sql.open(recording(calls), "app.db", Sql.defaults, fn(db) {
      Sql.transaction(db, fn(tx) { Sql.transaction(tx, fn(_) { Debug.crash("boom") }) })
    })
  }) |> Test.assert_equal("boom")
  Test.recorded(calls) |> Test.assert_equal([
    Call.Connected("app.db"), Call.Began(0), Call.Began(1), Call.RolledBack(1), Call.Reset, Call.Closed,
  ])
}

test "a savepoint's failed commit stays the outcome when its rollback fails too" {
  let driver = Sql.fake(fn(call) {
    case call {
      Call.Committed(1) -> Failure(SqlError.Conflict("release"))
      Call.RolledBack(1) -> Failure(SqlError.ConnectionLost)
      _ -> Sql.done()
    }
  })
  Sql.open(driver, "app.db", Sql.defaults, fn(db) {
    Sql.transaction(db, fn(tx) {
      failed(fn() { Sql.transaction(tx, fn(nested) { Sql.execute(nested, "b", []) }) })
    })
  }) |> Test.assert_equal(Some(SqlError.Conflict("release")))
}
"#,
    );
    assert_passed(&run, 11);
}

// Rejects a transaction's database that still reaches the connection after
// the transaction or savepoint ended (returned from its body, kept by a task
// that outlives it, or ended by a defect or a cancellation), and a check that
// refuses the outer database inside one of its savepoints.
#[test]
fn a_transactions_database_fails_once_the_transaction_has_ended() {
    let run = run_tests(
        "transaction-ended",
        r#"
fn kept(calls: Recorder<Call>) -> List<Call> {
  List.filter(Test.recorded(calls), fn(call) {
    case call {
      Call.Ran(sql, _) -> sql != "a"
      _ -> False
    }
  })
}

test "a database returned from its transaction fails and never reaches the driver" {
  let calls = Test.recorder()
  Sql.open(recording(calls), "app.db", Sql.defaults, fn(db) {
    let escaped = Sql.transaction(db, fn(tx) { tx })
    failed(fn() { Sql.execute(escaped, "late", []) }) |> Test.assert_equal(Some(SqlError.TransactionEnded))
    failed(fn() { Sql.transaction(escaped, fn(nested) { Sql.execute(nested, "later", []) }) })
      |> Test.assert_equal(Some(SqlError.TransactionEnded))
    let inner = Sql.transaction(db, fn(tx) {
      let nested = Sql.transaction(tx, fn(nested) { nested })
      failed(fn() { Sql.execute(nested, "inner", []) }) |> Test.assert_equal(Some(SqlError.TransactionEnded))
      Sql.execute(tx, "a", [])
    })
    inner |> Test.assert_equal(0)
  })
  kept(calls) |> Test.assert_equal([])
}

test "a task that keeps a transaction's database fails once it has ended" {
  let calls = Test.recorder()
  let outcome = Sql.open(recording(calls), "app.db", Sql.defaults, fn(db) {
    let later = Sql.transaction(db, fn(tx) {
      Task.start(fn() {
        Clock.sleep(Duration.seconds(1))
        failed(fn() { Sql.execute(tx, "late", []) })
      })
    })
    Task.result(later)
  })
  outcome |> Test.assert_equal(Some(SqlError.TransactionEnded))
  kept(calls) |> Test.assert_equal([])
}

// Waits for a database on `handed`, then runs `sql` on it after `seconds`.
fn run_later(handed: Channel.Receiver<Database>, seconds: Int, sql: Text) -> Option<SqlError> needs { clock: Clock } {
  case Result.capture(fn() { Channel.receive(handed) }) {
    Ok(Channel.Receive.Received(db)) -> {
      Clock.sleep(Duration.seconds(seconds))
      failed(fn() { Sql.execute(db, sql, []) })
    }
    _ -> Some(SqlError.Failed("no database"))
  }
}

test "a savepoint's database ends when the savepoint is cancelled or defects" {
  let calls = Test.recorder()
  Sql.open(recording(calls), "app.db", Sql.defaults, fn(db) {
    Sql.transaction(db, fn(tx) {
      let cut = Channel.bounded(1)
      let cut_handed = cut.receiver
      let after_cut = Task.start(fn() { run_later(cut_handed, 3, "cut") })
      let cut_sender = cut.sender
      let _ = Result.capture(fn() {
        Task.timeout(fn() {
          Sql.transaction(tx, fn(nested) {
            let _ = Result.capture(fn() { Channel.send(cut_sender, nested) })
            Clock.sleep(Duration.seconds(5))
          })
        }, Duration.seconds(1))
      })
      let crash = Channel.bounded(1)
      let crash_handed = crash.receiver
      let after_crash = Task.start(fn() { run_later(crash_handed, 1, "crash") })
      let crash_sender = crash.sender
      let defected = Test.capture_defect(fn() {
        Sql.transaction(tx, fn(nested) {
          let _ = Result.capture(fn() { Channel.send(crash_sender, nested) })
          Debug.crash("boom")
        })
      })
      defected |> Test.assert_equal("boom")
      Task.result(after_cut) |> Test.assert_equal(Some(SqlError.TransactionEnded))
      Task.result(after_crash) |> Test.assert_equal(Some(SqlError.TransactionEnded))
      Sql.execute(tx, "still", [])
    })
  })
  kept(calls) |> Test.assert_equal([ran("still")])
}

test "the outer database still works inside one of its savepoints" {
  let calls = Test.recorder()
  Sql.open(recording(calls), "app.db", Sql.defaults, fn(db) {
    Sql.transaction(db, fn(tx) {
      Sql.transaction(tx, fn(_) { Sql.execute(tx, "outer", []) })
    })
  })
  kept(calls) |> Test.assert_equal([ran("outer")])
}
"#,
    );
    assert_passed(&run, 4);
}
