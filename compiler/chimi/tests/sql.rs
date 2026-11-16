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
  Sql.open(recording(calls), "app.db", fn(db) {
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
  Sql.open(recording(calls), "app.db", fn(db) {
    let row = Sql.one(db, "select one", [])
    failed(fn() { Sql.int(row, "missing") }) |> Test.assert_equal(Some(SqlError.NoColumn("missing")))
    failed(fn() { Sql.int(row, "name") }) |> Test.assert_equal(Some(SqlError.WrongType("name", "Int", "Text")))
    failed(fn() { Sql.text(row, "email") }) |> Test.assert_equal(Some(SqlError.UnexpectedNull("email")))
    failed(fn() { Sql.nullable(row, "name", Sql.int) }) |> Test.assert_equal(Some(SqlError.WrongType("name", "Int", "Text")))
  })
}

test "one and first count rows" {
  let calls = Test.recorder()
  Sql.open(recording(calls), "app.db", fn(db) {
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
  let affected = Sql.open(recording(calls), "app.db", fn(db) {
    let first = Sql.execute(db, "insert ?", [Value.Int(1), Value.Text("x")])
    let second = Sql.execute(db, "update", [])
    first + second
  })
  affected |> Test.assert_equal(2)
  Test.recorded(calls) |> Test.assert_equal([Call.Connected("app.db"), Call.Ran("insert ?", [Value.Int(1), Value.Text("x")]), Call.Reset, ran("update"), Call.Reset, Call.Closed])
}

test "tasks share the pool" {
  let calls = Test.recorder()
  let total = Sql.open(recording(calls), "app.db", fn(db) {
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
  Sql.open(driver, "app.db", fn(db) {
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
  let outcomes = Sql.open(driver, "app.db", fn(db) {
    List.map(List.repeat(Void, 12), fn(_) { failed(fn() { Sql.execute(db, "a", []) }) })
  })
  outcomes |> Test.assert_equal(List.repeat(Some(SqlError.ConnectFailed("refused")), 12))
}

test "a pool used after it closed fails" {
  let calls = Test.recorder()
  let escaped = Sql.open(recording(calls), "app.db", fn(db) { db })
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

test "the eleventh statement waits for a connection to come free" {
  let calls = Test.recorder()
  Sql.open(recording(calls), "app.db", fn(db) {
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
  Sql.open(recording(calls), "app.db", fn(db) {
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
  })
}
"#,
    );
    assert_passed(&run, 2);
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
  let result = Sql.open(recording(calls), "app.db", fn(db) {
    Sql.transaction(db, fn(tx) {
      let a = Sql.execute(tx, "a", [Value.Int(1)])
      let b = Sql.execute(tx, "b", [Value.Int(1)])
      a + b
    })
  })
  result |> Test.assert_equal(2)
  Test.recorded(calls) |> Test.assert_equal([Call.Connected("app.db"), Call.Began, Call.Ran("a", [Value.Int(1)]), Call.Ran("b", [Value.Int(1)]), Call.Committed, Call.Reset, Call.Closed])
}

test "a failure rolls back and is the outcome" {
  let calls = Test.recorder()
  let outcome = Result.capture(fn() {
    Sql.open(recording(calls), "app.db", fn(db) {
      Sql.transaction(db, fn(tx) {
        let _ = Sql.execute(tx, "a", [])
        Failure(Oops.Oops)
      })
    })
  })
  outcome |> Test.assert_equal(Err(Oops.Oops))
  Test.recorded(calls) |> Test.assert_equal([Call.Connected("app.db"), Call.Began, ran("a"), Call.RolledBack, Call.Reset, Call.Closed])
  let failed = Test.recorder()
  Result.capture(fn() { Sql.open(recording(failed), "app.db", fn(db) { Sql.transaction(db, fn(tx) { Sql.execute(tx, "fail", []) }) }) })
    |> Test.assert_equal(Err(SqlError.ConstraintFailed("users_email_key", "duplicate")))
}

test "a defect or a cancellation leaves no transaction open" {
  let calls = Test.recorder()
  Test.capture_defect(fn() {
    Sql.open(recording(calls), "app.db", fn(db) { Sql.transaction(db, fn(_) { Debug.crash("boom") }) })
  }) |> Test.assert_equal("boom")
  Test.recorded(calls) |> Test.assert_equal([Call.Connected("app.db"), Call.Began, Call.Reset, Call.Closed])
  let cancelled = Test.recorder()
  let timed = Result.capture(fn() {
    Task.timeout(fn() {
      Sql.open(recording(cancelled), "app.db", fn(db) {
        Sql.transaction(db, fn(_) { Clock.sleep(Duration.seconds(100)) })
      })
    }, Duration.seconds(1))
  })
  let timed_out = case timed {
    Ok(_) -> False
    Err(_) -> True
  }
  timed_out |> Test.assert_equal(True)
  Test.recorded(cancelled) |> Test.assert_equal([Call.Connected("app.db"), Call.Began, Call.Reset, Call.Closed])
}

test "a failed commit is the outcome" {
  let calls = Test.recorder()
  let driver = Sql.fake(fn(call) {
    Test.record(calls, call)
    case call {
      Call.Committed -> Failure(SqlError.Conflict("serialization"))
      _ -> Sql.done()
    }
  })
  Result.capture(fn() { Sql.open(driver, "app.db", fn(db) { Sql.transaction(db, fn(tx) { Sql.execute(tx, "a", []) }) }) })
    |> Test.assert_equal(Err(SqlError.Conflict("serialization")))
}

test "a transaction inside one is a savepoint" {
  let calls = Test.recorder()
  Sql.open(recording(calls), "app.db", fn(db) {
    Sql.transaction(db, fn(tx) {
      let _ = Sql.execute(tx, "a", [])
      let inner = Result.capture(fn() { Sql.transaction(tx, fn(nested) { Sql.execute(nested, "fail", []) }) })
      inner |> Test.assert_equal(Err(SqlError.ConstraintFailed("users_email_key", "duplicate")))
      Sql.transaction(tx, fn(nested) { Sql.transaction(nested, fn(deeper) { Sql.execute(deeper, "b", []) }) })
    })
  })
  Test.recorded(calls) |> Test.assert_equal([
    Call.Connected("app.db"), Call.Began, ran("a"),
    ran("SAVEPOINT chimi_1"), ran("fail"), ran("ROLLBACK TO SAVEPOINT chimi_1"), ran("RELEASE SAVEPOINT chimi_1"),
    ran("SAVEPOINT chimi_1"), ran("SAVEPOINT chimi_2"), ran("b"), ran("RELEASE SAVEPOINT chimi_2"), ran("RELEASE SAVEPOINT chimi_1"),
    Call.Committed, Call.Reset, Call.Closed,
  ])
}
"#,
    );
    assert_passed(&run, 5);
}
