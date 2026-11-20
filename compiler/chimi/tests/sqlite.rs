//! Conformance for `std/sqlite`, the SQLite driver for `std/sql` (CHI-258):
//! values both ways, failures, transactions and savepoints, a locked
//! database, and the `files` provider's limits. Every test opens a real
//! database file in `Fs.temporary_directory` (or `:memory:`) under virtual
//! time, so a wait on a lock passes at once. Each test writes `cases.chimi`
//! to a scratch directory and runs the CLI there.

use std::path::PathBuf;
use std::process::Command;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str, source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "chimi-sqlite-{name}-{}-{:?}",
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

/// `database(body)` runs `body` on a pool over a new file in a temporary
/// directory; `memory(body)` on one connection to `:memory:`; `failure(f)`
/// is what `f` failed with.
const PRELUDE: &str = r#"import std/bytes as Bytes
import std/clock as Clock
import std/decimal as Decimal
import std/duration as Duration
import std/fs as Fs
import std/list as List
import std/path as Path
import std/path.{Path}
import std/result as Result
import std/result.{Err, Ok}
import std/sql as Sql
import std/sql.{Database, SqlError, Value}
import std/sqlite as Sqlite
import std/task as Task
import std/test as Test

fn database<T, E, R>(body: fn(Database) -> T errors E needs R) -> T needs R {
  Fs.temporary_directory(fn(directory) {
    within(directory, "app.db", body)
  })
}

fn within<T, E, R>(directory: Path, name: Text, body: fn(Database) -> T errors E needs R) -> T needs R {
  let driver = Sqlite.driver() using { files: Fs.files_within(directory) }
  Sql.open(driver, Path.to_text(Path.join(directory, name)), Sql.defaults, body)
}

fn memory<T, E, R>(body: fn(Database) -> T errors E needs R) -> T needs R {
  Fs.temporary_directory(fn(directory) {
    let driver = Sqlite.driver() using { files: Fs.files_within(directory) }
    Sql.open(driver, ":memory:", { ..Sql.defaults, max_connections: 1 }, body)
  })
}

fn failure<T, R>(operation: fn() -> T errors SqlError needs R) -> SqlError needs R {
  case Result.capture(operation) {
    Ok(_) -> Debug.crash("expected a failure")
    Err(error) -> error
  }
}

fn names(db: Database) -> List<Text> {
  List.map(Sql.query(db, "select name from users order by id", []), fn(row) { Sql.text(row, "name") })
}

fn users(db: Database) -> Void {
  Sql.execute(db, "create table users (id integer primary key, name text not null, email text unique)", [])
  Void
}
"#;

fn run_tests(name: &str, tests: &str) -> Run {
    Scratch::new(name, &format!("{PRELUDE}\nimport std/debug as Debug\n{tests}")).chimi(&[
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

// Rejects a value that changes on its way in or out: a lost digit, a Bool
// read back as an Int from a BOOLEAN column (or the other way round from an
// INTEGER one), an empty blob read as null, a float that does not round-trip,
// a Decimal column read as text, or a row count for a read.
#[test]
fn values_round_trip() {
    let run = run_tests(
        "values",
        r#"
test "every kind of value comes back as it went in" {
  database(fn(db) {
    Sql.execute(db, "create table things (i integer, b boolean, f real, d decimal_text, n numeric, t text, x blob, e blob, z text)", [])
    let params = [Value.Int(-9223372036854775807), Value.Bool(False), Value.Float(0.1), Value.Decimal(Decimal.parse("12345678901234567890.50")), Value.Decimal(Decimal.parse("2.5")), Value.Text("héllo, 世界"), Value.Bytes(Bytes.from([0, 255, 7])), Value.Bytes(Bytes.empty()), Value.Null]
    Test.assert_equal(Sql.execute(db, "insert into things values (?, ?, ?, ?, ?, ?, ?, ?, ?)", params), 1)
    let row = Sql.one(db, "select * from things", [])
    Test.assert_equal(Sql.values(row), params)
    Test.assert_equal(Sql.columns(row), ["i", "b", "f", "d", "n", "t", "x", "e", "z"])
  })
}

test "a Bool outside a BOOLEAN column is an Int, and an expression keeps SQLite's type" {
  memory(fn(db) {
    Sql.execute(db, "create table flags (on_off integer, yes bool)", [])
    Sql.execute(db, "insert into flags values (?, ?)", [Value.Bool(True), Value.Bool(True)])
    Test.assert_equal(Sql.values(Sql.one(db, "select on_off, yes, yes + 1 as more, 2.0 as two from flags", [])), [Value.Int(1), Value.Bool(True), Value.Int(2), Value.Float(2.0)])
    Sql.execute(db, "update flags set yes = 2", [])
    Test.assert_equal(Sql.value(Sql.one(db, "select yes from flags", []), "yes"), Value.Int(2))
  })
}

test "a read changes no rows, and a write counts the rows it changed" {
  memory(fn(db) {
    users(db)
    Sql.execute(db, "insert into users (name) values ('a'), ('b'), ('c')", [])
    Test.assert_equal(Sql.execute(db, "select * from users", []), 0)
    Test.assert_equal(Sql.execute(db, "update users set name = 'x' where id > 1", []), 2)
    Test.assert_equal(Sql.execute(db, "-- nothing to run", []), 0)
  })
}

test "placeholders bind in order, however they are written" {
  memory(fn(db) {
    users(db)
    Sql.execute(db, "insert into users (name, email) values (?1, ?2)", [Value.Text("Ada"), Value.Text("ada@example.com")])
    Sql.execute(db, "insert into users (name, email) values (:name, $email)", [Value.Text("Bo"), Value.Text("bo@example.com")])
    Test.assert_equal(names(db), ["Ada", "Bo"])
  })
}

test "a DECIMAL column's large and small numbers read back without an exponent" {
  memory(fn(db) {
    Sql.execute(db, "create table prices (p decimal)", [])
    Sql.execute(db, "insert into prices values (?), (?), (?)", [Value.Decimal(Decimal.parse("100000000000000000000")), Value.Decimal(Decimal.parse("0.0000001")), Value.Decimal(Decimal.parse("-2.5"))])
    let read = List.map(Sql.query(db, "select p from prices", []), fn(row) { Sql.value(row, "p") })
    Test.assert_equal(read, [Value.Decimal(Decimal.parse("100000000000000000000")), Value.Decimal(Decimal.parse("0.0000001")), Value.Decimal(Decimal.parse("-2.5"))])
  })
}

test "a statement that writes no rows changes none, even after a write" {
  memory(fn(db) {
    users(db)
    Test.assert_equal(Sql.execute(db, "insert into users (name) values ('a'), ('b'), ('c')", []), 3)
    Test.assert_equal(Sql.execute(db, "create table other (x)", []), 0)
    Test.assert_equal(Sql.execute(db, "drop table other", []), 0)
    Test.assert_equal(Sql.execute(db, "delete from users where name = 'nobody'", []), 0)
  })
}

test "the version is the vendored one" {
  Test.assert_equal(Sqlite.version(), "3.54.1")
}
"#,
    );
    assert_passed(&run, 7);
}

// Rejects a failure mapped to the wrong `SqlError`, a constraint named by
// the whole message, a second statement run or dropped silently, a count of
// parameters that is not checked, a value a `Float` cannot hold returned as
// something else, or text that is not UTF-8 crossing into Chimi.
#[test]
fn failures_name_what_went_wrong() {
    let run = run_tests(
        "failures",
        r#"
test "a broken constraint names it" {
  memory(fn(db) {
    users(db)
    Sql.execute(db, "insert into users (name, email) values ('a', 'a@x')", [])
    Test.assert_equal(failure(fn() { Sql.execute(db, "insert into users (name, email) values ('b', 'a@x')", []) }), SqlError.ConstraintFailed("users.email", "UNIQUE constraint failed: users.email"))
    Test.assert_equal(failure(fn() { Sql.execute(db, "insert into users (email) values ('c@x')", []) }), SqlError.ConstraintFailed("users.name", "NOT NULL constraint failed: users.name"))
  })
}

test "a foreign key is enforced" {
  memory(fn(db) {
    users(db)
    Sql.execute(db, "create table posts (id integer primary key, user_id integer references users(id))", [])
    Test.assert_equal(failure(fn() { Sql.execute(db, "insert into posts (user_id) values (7)", []) }), SqlError.ConstraintFailed("FOREIGN KEY constraint failed", "FOREIGN KEY constraint failed"))
  })
}

test "a statement SQLite refuses is rejected with its code" {
  memory(fn(db) {
    Test.assert_equal(failure(fn() { Sql.execute(db, "selec 1", []) }), SqlError.Rejected("SQLITE_ERROR", "near \"selec\": syntax error"))
    Test.assert_equal(failure(fn() { Sql.query(db, "select * from missing", []) }), SqlError.Rejected("SQLITE_ERROR", "no such table: missing"))
  })
}

test "the parameters must match the placeholders" {
  memory(fn(db) {
    Test.assert_equal(failure(fn() { Sql.query(db, "select ?, ?", [Value.Int(1)]) }), SqlError.Rejected("SQLITE_RANGE", "the statement takes 2 parameters, and 1 were given"))
    Test.assert_equal(failure(fn() { Sql.query(db, "select 1", [Value.Int(1)]) }), SqlError.Rejected("SQLITE_RANGE", "the statement takes 0 parameters, and 1 were given"))
    Test.assert_equal(failure(fn() { Sql.execute(db, "-- nothing", [Value.Int(1)]) }), SqlError.Rejected("SQLITE_RANGE", "the statement takes 0 parameters, and 1 were given"))
  })
}

test "one call runs one statement" {
  memory(fn(db) {
    users(db)
    let refused = failure(fn() { Sql.execute(db, "insert into users (name) values ('a'); delete from users", []) })
    Test.assert_equal(refused, SqlError.Rejected("MANY_STATEMENTS", "SQLite: the text holds more than one statement; run one statement per call"))
    Test.assert_equal(names(db), [])
    Sql.execute(db, "insert into users (name) values ('b'); -- done\n", [])
    Test.assert_equal(names(db), ["b"])
  })
}

test "values a Float or Text cannot hold fail" {
  memory(fn(db) {
    Test.assert_equal(failure(fn() { Sql.query(db, "select 1e999 as big", []) }), SqlError.Failed("SQLite: the column big holds inf, which is not a Float"))
    Test.assert_equal(failure(fn() { Sql.query(db, "select cast(x'ff' as text) as bad", []) }), SqlError.Failed("SQLite: the column bad holds text that is not UTF-8"))
  })
}
"#,
    );
    assert_passed(&run, 6);
}

// Rejects a transaction that does not commit, a failed one that keeps its
// writes, a savepoint whose rollback undoes the outer transaction too, two
// savepoints at one depth at once, statements that run on after SQLite ended
// the transaction, a deferred begin (whose later write cannot wait for the
// lock), or a file database not in write-ahead mode.
#[test]
fn transactions_and_savepoints() {
    let run = run_tests(
        "transactions",
        r#"
test "a transaction commits, and a failed one leaves nothing" {
  database(fn(db) {
    users(db)
    Sql.transaction(db, fn(tx) {
      Sql.execute(tx, "insert into users (name) values ('kept')", [])
    })
    let failed = failure(fn() {
      Sql.transaction(db, fn(tx) {
        Sql.execute(tx, "insert into users (name) values ('lost')", [])
        Sql.execute(tx, "insert into users (email) values ('no name')", [])
      })
    })
    Test.assert_equal(failed, SqlError.ConstraintFailed("users.name", "NOT NULL constraint failed: users.name"))
    Test.assert_equal(names(db), ["kept"])
  })
}

test "a failed savepoint undoes only its own writes" {
  database(fn(db) {
    users(db)
    Sql.transaction(db, fn(tx) {
      Sql.execute(tx, "insert into users (name) values ('outer')", [])
      let _ = Result.capture(fn() {
        Sql.transaction(tx, fn(inner) {
          Sql.execute(inner, "insert into users (name) values ('inner')", [])
          Sql.execute(inner, "insert into users (email) values ('no name')", [])
        })
      })
      Sql.transaction(tx, fn(inner) {
        Sql.execute(inner, "insert into users (name) values ('second')", [])
      })
    })
    Test.assert_equal(names(db), ["outer", "second"])
  })
}

test "two savepoints at once on one transaction are refused" {
  database(fn(db) {
    users(db)
    Sql.transaction(db, fn(tx) {
      let first = Task.start(fn() {
        Sql.transaction(tx, fn(inner) {
          Sql.execute(inner, "insert into users (name) values ('first')", [])
          Clock.sleep(Duration.seconds(1))
        })
      })
      Clock.sleep(Duration.milliseconds(10))
      let second = failure(fn() {
        Sql.transaction(tx, fn(inner) { Sql.execute(inner, "insert into users (name) values ('second')", []) })
      })
      Test.assert_equal(second, SqlError.Failed("SQLite: a nested transaction is already open on this transaction; nested transactions on one transaction run one after another"))
      Task.result(first)
      Sql.transaction(tx, fn(inner) { Sql.execute(inner, "insert into users (name) values ('third')", []) })
    })
    Test.assert_equal(names(db), ["first", "third"])
  })
}

test "a transaction SQLite ended by itself takes no more statements" {
  database(fn(db) {
    users(db)
    let ended = failure(fn() {
      Sql.transaction(db, fn(tx) {
        Sql.execute(tx, "insert into users (name, email) values ('a', 'x@x')", [])
        let _ = Result.capture(fn() { Sql.execute(tx, "insert or rollback into users (name, email) values ('b', 'x@x')", []) })
        Sql.execute(tx, "insert into users (name) values ('after')", [])
      })
    })
    Test.assert_equal(ended, SqlError.TransactionEnded)
    Test.assert_equal(names(db), [])
    Sql.execute(db, "insert into users (name) values ('later')", [])
    Test.assert_equal(names(db), ["later"])
  })
}

test "a transaction that reads before it writes is not cut off by another writer" {
  database(fn(db) {
    users(db)
    let reader = Task.start(fn() {
      Sql.transaction(db, fn(tx) {
        let before = List.length(names(tx))
        Clock.sleep(Duration.seconds(1))
        Sql.execute(tx, "insert into users (name) values ('counted ${before}')", [])
      })
    })
    Clock.sleep(Duration.milliseconds(10))
    Sql.execute(db, "insert into users (name) values ('other')", [])
    Task.result(reader)
    Test.assert_equal(names(db), ["counted 0", "other"])
  })
}

test "a commit that fails leaves nothing behind for the connection's next use" {
  database(fn(db) {
    users(db)
    Sql.execute(db, "create table posts (id integer primary key, user_id integer references users(id) deferrable initially deferred)", [])
    let failed = failure(fn() {
      Sql.transaction(db, fn(tx) {
        Sql.execute(tx, "insert into posts (user_id) values (7)", [])
      })
    })
    Test.assert_equal(failed, SqlError.ConstraintFailed("FOREIGN KEY constraint failed", "FOREIGN KEY constraint failed"))
    Sql.transaction(db, fn(tx) {
      Sql.execute(tx, "insert into users (name) values ('next')", [])
    })
    Test.assert_equal(names(db), ["next"])
    Test.assert_equal(Sql.int(Sql.one(db, "select count(*) as n from posts", []), "n"), 0)
  })
}

test "a file database uses write-ahead logging" {
  database(fn(db) {
    Test.assert_equal(Sql.text(Sql.one(db, "pragma journal_mode", []), "journal_mode"), "wal")
    Test.assert_equal(Sql.int(Sql.one(db, "pragma foreign_keys", []), "foreign_keys"), 1)
  })
}
"#,
    );
    assert_passed(&run, 7);
}

// Rejects a statement that fails at once while another connection holds the
// write lock, one that blocks the other tasks while it waits (the holder
// could then never commit), or one that waits forever.
#[test]
fn a_locked_database_waits_then_conflicts() {
    let run = run_tests(
        "locked",
        r#"
test "a write waits for the transaction holding the lock" {
  database(fn(db) {
    users(db)
    let start = Clock.now()
    let holder = Task.start(fn() {
      Sql.transaction(db, fn(tx) {
        Sql.execute(tx, "insert into users (name) values ('first')", [])
        Clock.sleep(Duration.seconds(1))
      })
    })
    Clock.sleep(Duration.milliseconds(10))
    Sql.execute(db, "insert into users (name) values ('second')", [])
    Test.assert(Clock.since(start) >= Duration.seconds(1), "the write waited for the commit")
    Test.assert(Clock.since(start) < Duration.milliseconds(1200), "the write ran soon after the commit")
    Task.result(holder)
    Test.assert_equal(names(db), ["first", "second"])
  })
}

test "a lock held too long is a conflict" {
  database(fn(db) {
    users(db)
    let holder = Task.start(fn() {
      Sql.transaction(db, fn(tx) {
        Sql.execute(tx, "insert into users (name) values ('first')", [])
        Clock.sleep(Duration.seconds(10))
      })
    })
    Clock.sleep(Duration.milliseconds(10))
    let start = Clock.now()
    Test.assert_equal(failure(fn() { Sql.execute(db, "insert into users (name) values ('second')", []) }), SqlError.Conflict("database is locked"))
    Test.assert(Clock.since(start) >= Duration.seconds(5), "the write retried for 5 s")
    Task.result(holder)
  })
}
"#,
    );
    assert_passed(&run, 2);
}

// Rejects a driver that opens a file its `files` provider does not allow,
// one that lets a statement attach or write another file, or one that
// cannot open a file in a directory it does allow.
#[test]
fn the_files_provider_limits_the_targets() {
    let run = run_tests(
        "files",
        r#"
test "a target outside the provider's directory fails to connect" {
  Fs.temporary_directory(fn(directory) {
    let driver = Sqlite.driver() using { files: Fs.files_within(Path.join(directory, "inner")) }
    let target = Path.to_text(Path.join(directory, "outside.db"))
    let refused = failure(fn() { Sql.open(driver, target, Sql.defaults, fn(db) { Sql.execute(db, "select 1", []) }) })
    Test.assert_equal(refused, SqlError.ConnectFailed("SQLite: the files provider refuses ${target}"))
  })
}

test "a database in a missing directory fails to connect" {
  Fs.temporary_directory(fn(directory) {
    let refused = failure(fn() { within(directory, "missing/app.db", fn(db) { Sql.execute(db, "select 1", []) }) })
    Test.assert_equal(refused, SqlError.ConnectFailed("SQLite: the directory of ${Path.to_text(Path.join(directory, "missing/app.db"))} cannot be read"))
  })
}

test "a statement cannot reach another database file" {
  database(fn(db) {
    Test.assert_equal(failure(fn() { Sql.execute(db, "attach database '/tmp/chimi-attach.db' as other", []) }), SqlError.Rejected("SQLITE_AUTH", "not authorized"))
    Test.assert_equal(failure(fn() { Sql.execute(db, "vacuum into '/tmp/chimi-vacuum.db'", []) }), SqlError.Rejected("SQLITE_AUTH", "authorization denied"))
    Test.assert_equal(failure(fn() { Sql.execute(db, "pragma temp_store_directory = '/tmp'", []) }), SqlError.Rejected("SQLITE_AUTH", "not authorized"))
  })
}

test "data written through one pool is there for the next" {
  Fs.temporary_directory(fn(directory) {
    within(directory, "app.db", fn(db) {
      users(db)
      Sql.execute(db, "insert into users (name) values ('saved')", [])
    })
    Test.assert_equal(within(directory, "app.db", names), ["saved"])
  })
}
"#,
    );
    assert_passed(&run, 4);
}

// Rejects a vendored SQLite edited in place, or a file added or dropped
// without rerunning `scripts/vendor-sqlite.sh`: the vendored tree holds
// exactly the files `MANIFEST.sha256` lists, each with its checksum.
#[test]
fn the_vendored_sqlite_matches_its_manifest() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("runtime/native/bindings/sqlite/sqlite");
    let manifest = std::fs::read_to_string(root.join("MANIFEST.sha256")).unwrap();
    let listed: std::collections::BTreeSet<String> = manifest
        .lines()
        .map(|line| line.split_once("  ").expect("a sha256sum line").1.to_owned())
        .collect();
    let present: std::collections::BTreeSet<String> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name != "MANIFEST.sha256" && name != "PROVENANCE.md")
        .collect();
    assert_eq!(present, listed, "files differ from MANIFEST.sha256");
    // sha256sum on Linux, shasum on macOS.
    let check = Command::new("sha256sum")
        .args(["--check", "--quiet", "MANIFEST.sha256"])
        .current_dir(&root)
        .output()
        .or_else(|_| {
            Command::new("shasum")
                .args(["-a", "256", "--check", "--quiet", "MANIFEST.sha256"])
                .current_dir(&root)
                .output()
        })
        .expect("run sha256sum or shasum");
    assert!(
        check.status.success(),
        "{}{}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );
}
