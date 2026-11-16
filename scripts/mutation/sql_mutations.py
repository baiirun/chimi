"""Mutants of std/sql's driver-independent layer (CHI-258): reading rows by
column, one and first, the pool (lazy connections, reuse, resets, its size,
its wait, closing), and transactions (commit, rollback, savepoints). A mutant
that makes a wait hang shows as TIMEOUT, which counts as caught.

Run: scripts/mutation/mutate.py --timeout 300 scripts/mutation/sql_mutations.py
Results: scripts/mutation/sql_results.txt.
"""

CRATE = "compiler/chimi"

SQL = "std/sql.chimi"

SQL_TESTS = dict(targets=["sql"], filter="")


def mutant(name, old, new):
    return dict(name=name, file=SQL, edits=[(old, new)], **SQL_TESTS)


MUTATIONS = [
    # Reading rows.
    mutant("a missing column reads the first",
           "    None -> Failure(SqlError.NoColumn(name))\n  }\n}",
           "    None -> case List.first(row.values) {\n      Some(found) -> found\n      None -> Failure(SqlError.NoColumn(name))\n    }\n  }\n}"),
    mutant("a null reads as a wrong type",
           "    Value.Null -> Failure(SqlError.UnexpectedNull(column))\n",
           ""),
    mutant("a wrong type names the expected kind as found",
           "    _ -> Failure(SqlError.WrongType(column, expected, kind(found)))",
           "    _ -> Failure(SqlError.WrongType(column, expected, expected))"),
    mutant("nullable reads a null",
           "    Value.Null -> None\n    _ -> Some(read(row, name))",
           "    _ -> Some(read(row, name))"),
    mutant("one takes the first of many",
           "    [row] -> row\n    [] -> Failure(SqlError.NoRows)\n    _ -> Failure(SqlError.ManyRows)",
           "    [row, .._] -> row\n    [] -> Failure(SqlError.NoRows)"),
    mutant("execute gives the row count",
           "  run(db, sql, params).affected",
           "  List.length(run(db, sql, params).rows)"),
    # The pool.
    mutant("idle connections are never reused",
           "      ([waiter, ..rest], [connection, ..idle], _) -> case offer(waiter, Grant.Idle(connection)) {",
           "      ([waiter, ..rest], [connection, ..idle], False) -> case offer(waiter, Grant.Idle(connection)) {"),
    mutant("the pool has no size limit",
           "      ([_, .._], [], False) -> break state",
           "      ([waiter, ..rest], [], False) -> case offer(waiter, Grant.Permit) {\n        None -> Pool([], state.held + 1, rest)\n        Some(_) -> Pool([], state.held, rest)\n      }"),
    mutant("the pool's size is one less",
           "state.held + List.length(state.idle) < size",
           "state.held + List.length(state.idle) + 1 < size"),
    mutant("a slot is never freed",
           "      Receive.Received(Ask.Done) -> serve_waiting(Pool(pool.idle, pool.held - 1, pool.waiting), size)",
           "      Receive.Received(Ask.Done) -> serve_waiting(pool, size)"),
    mutant("the wait is half its limit",
           "    }, wait)\n  })",
           "    }, Duration.seconds(15))\n  })"),
    mutant("a timed-out wait reports the pool closed",
           "    Err(_) -> Failure(SqlError.PoolTimedOut(wait))",
           "    Err(_) -> Failure(SqlError.PoolClosed)"),
    mutant("a connection is reused without a reset",
           "    Task.timeout(fn() { connection.reset() }, limit)",
           "    Task.timeout(fn() { Void }, limit)"),
    mutant("a failed reset keeps the connection",
           "    Err(_) -> connection.close()\n  }\n}",
           "    Err(_) -> {\n      let _ = tell(asks, Ask.Return(connection))\n      Void\n    }\n  }\n}"),
    mutant("closing the pool leaves idle connections open",
           "      Receive.Received(Ask.Close) -> {\n        List.each(pool.idle, fn(connection) { connection.close() })\n        break Void",
           "      Receive.Received(Ask.Close) -> {\n        break Void"),
    mutant("a failed connect still takes a slot",
           "      let _ = tell(asks, Ask.Done)\n      Void\n    }, fn(grant) {",
           "      Void\n    }, fn(grant) {"),
    # Transactions.
    mutant("a transaction never commits",
           "    Ok(result) -> {\n      connection.commit()\n      result",
           "    Ok(result) -> {\n      result"),
    mutant("a failed body commits",
           "      let _ = Result.capture(fn() { connection.rollback() })",
           "      let _ = Result.capture(fn() { connection.commit() })"),
    mutant("a failed body's failure is swallowed",
           "      let _ = Result.capture(fn() { connection.rollback() })\n      Failure(error)",
           "      let _ = Result.capture(fn() { connection.rollback() })\n      Failure(SqlError.Failed(\"transaction failed\"))"),
    mutant("a nested transaction starts a new one",
           "    Place.Pinned(connection, depth) -> savepoint(connection, depth, body)",
           "    Place.Pinned(connection, _) -> outermost(connection, body)"),
    mutant("a failed savepoint is not undone",
           "let _ = Result.capture(fn() {\n          connection.run(\"ROLLBACK TO SAVEPOINT ${name}\", [])\n        })\n",
           "Void\n"),
    mutant("a cut-off savepoint counts as released",
           "    Success(Receive.Received(_)) -> True\n    Success(Receive.Drained) -> False",
           "    Success(Receive.Received(_)) -> True\n    Success(Receive.Drained) -> True"),
    mutant("a failed savepoint rollback replaces the failure",
           "let _ = Result.capture(fn() {\n          connection.run(\"ROLLBACK TO SAVEPOINT ${name}\", [])\n        })\n",
           "let _ = connection.run(\"ROLLBACK TO SAVEPOINT ${name}\", [])\n"),
    mutant("a connection returned while the pool closes stays open",
           "          Receive.Received(Ask.Return(connection)) -> {\n            connection.close()\n            left",
           "          Receive.Received(Ask.Return(connection)) -> {\n            left"),
    mutant("closing the pool ignores borrowers",
           "        break shut(keeper, pool.held)",
           "        break Void"),
    mutant("savepoints share one name",
           "  let name = \"chimi_${depth}\"",
           "  let name = \"chimi\""),
    mutant("a statement in a transaction takes its own connection",
           "    Place.Pinned(connection, _) -> connection.run(sql, params)\n    Place.Pooled(_, _, _, _) -> borrow(",
           "    Place.Pinned(connection, _) -> Failure(SqlError.PoolClosed)\n    Place.Pooled(_, _, _, _) -> borrow("),
]
