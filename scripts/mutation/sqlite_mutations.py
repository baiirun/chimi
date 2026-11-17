"""Mutants of std/sqlite, the SQLite driver for std/sql (CHI-258): opening
and the files check, connection setup, transactions and savepoints, the lock
retry, binding and reading values, and mapping failures, in the Chimi module
and its C binding. A mutant that makes a wait hang shows as TIMEOUT, which
counts as caught.

Run: scripts/mutation/mutate.py --timeout 300 scripts/mutation/sqlite_mutations.py
Results: scripts/mutation/sqlite_results.txt.
"""

CRATE = "compiler/chimi"

MODULE = "std/sqlite.chimi"
BINDING = "runtime/native/bindings/sqlite/sqlite.c"

TESTS = dict(targets=["sqlite"], filter="")


def mutant(name, old, new, file=MODULE):
    return dict(name=name, file=file, edits=[(old, new)], **TESTS)


def c_mutant(name, old, new):
    return mutant(name, old, new, file=BINDING)


MUTATIONS = [
    # Opening.
    mutant("the files provider is not asked",
           "    False -> allowed(given.files, target)\n",
           "    False -> Void\n"),
    mutant("a refusal is reported as a missing directory",
           "    Err(FsError.PermissionDenied(_)) -> Failure(SqlError.ConnectFailed(\"SQLite: the files provider refuses ${target}\"))\n",
           ""),
    mutant("foreign keys stay off",
           "    statement(db, \"pragma foreign_keys = on\")\n",
           "    Void\n"),
    mutant("a file keeps the rollback journal",
           "        statement(db, \"pragma journal_mode = wal\")\n",
           ""),
    # Transactions.
    mutant("a transaction begins deferred",
           "    True -> \"begin immediate\"",
           "    True -> \"begin\""),
    mutant("a savepoint commit commits everything",
           "    False -> \"release chimi_${depth}\"\n  }\n}\n\n// A savepoint's",
           "    False -> \"commit\"\n  }\n}\n\n// A savepoint's"),
    mutant("a savepoint rollback rolls back the transaction",
           "        statement(db, \"rollback to chimi_${depth}\")\n",
           "        statement(db, \"rollback\")\n"),
    mutant("a savepoint rolled back is left open",
           "        Ok(_) -> statement(db, \"release chimi_${depth}\")\n",
           "        Ok(_) -> Void\n"),
    mutant("a reset never rolls back",
           "    False -> statement(db, \"rollback\")\n  }\n}\n\nfn statement",
           "    False -> Void\n  }\n}\n\nfn statement"),
    # The lock retry.
    mutant("a locked statement fails at once",
           "      Try.Locked(message) -> case waited >= busy_limit {",
           "      Try.Locked(message) -> case True {"),
    mutant("a locked statement retries forever",
           "      Try.Locked(message) -> case waited >= busy_limit {",
           "      Try.Locked(message) -> case False {"),
    mutant("the wait between tries never grows",
           "          (next, waited + pause)",
           "          (1, waited + pause)"),
    mutant("a lock is not recognised",
           "  code > 0 and (primary == 5 or primary == 6)",
           "  code > 0 and primary == 6"),
    # Statements.
    mutant("the parameter count is not checked",
           "  case given == expected {\n    True -> Void",
           "  case True {\n    True -> Void"),
    mutant("parameters bind one place off",
           "    sql(bind(handle, index + 1, param))",
           "    sql(bind(handle, index + 2, param))"),
    mutant("a statement's rows are dropped",
           "          Ok(Some(changed)) -> break Try.Ran(Sql.reply(names, rows, changed))",
           "          Ok(Some(changed)) -> break Try.Ran(Sql.reply(names, [], changed))"),
    mutant("an empty text fails",
           "    Ok(None) -> Try.Ran(Sql.done())",
           "    Ok(None) -> Failure(SqlError.Failed(\"empty\"))"),
    # Values.
    mutant("a Bool is stored the wrong way round",
           "      True -> 1\n      False -> 0",
           "      True -> 0\n      False -> 1"),
    mutant("a BOOLEAN column reads as Int",
           "      (True, _) -> Value.Bool(integer == 1)\n",
           ""),
    mutant("any 0 or 1 reads as Bool",
           "  let boolean = declared_type == \"BOOLEAN\" or declared_type == \"BOOL\"",
           "  let boolean = True"),
    mutant("a decimal column reads as text",
           "      True -> case Result.capture(fn() { Decimal.parse(text) }) {",
           "      True -> case Result.capture(fn() { Failure(SqlError.NoRows) }) {"),
    mutant("a NUMERIC column reads as a number",
           "  let decimal = Text.starts_with(declared_type, \"DECIMAL\") or Text.starts_with(declared_type, \"NUMERIC\")",
           "  let decimal = Text.starts_with(declared_type, \"DECIMAL\")"),
    mutant("a blob reads as null",
           "    4 -> Value.Bytes(bytes)\n",
           ""),
    mutant("an infinity reads as zero",
           "    Err(_) -> Failure(SqlError.Failed(\"SQLite: the column ${name} holds ${text}, which is not a Float\"))",
           "    Err(_) -> Value.Float(0.0)"),
    # Failures.
    mutant("a constraint is named by its whole message",
           "    Some((_, named)) -> named",
           "    Some(_) -> message"),
    mutant("a constraint failure is a plain failure",
           "    19 -> Failure(SqlError.ConstraintFailed(constraint(message), message))\n",
           ""),
    mutant("a syntax error is a plain failure",
           "    1 -> Failure(SqlError.Rejected(\"SQLITE_ERROR\", message))\n",
           ""),
    mutant("a lock after the retries is a plain failure",
           "    5 -> Failure(SqlError.Conflict(message))\n",
           ""),
    mutant("extended codes are not reduced",
           "    _ -> by_primary(code % 256, message)",
           "    _ -> by_primary(code, message)"),
    # The binding.
    c_mutant("a second statement is run silently",
             "  if (tail && !only_blank(tail, sql + length)) {",
             "  if (0) {"),
    c_mutant("a comment after the statement is a second statement",
             "    } else if (rest + 1 < end && rest[0] == '-' && rest[1] == '-') {",
             "    } else if (0) {"),
    c_mutant("a read counts the last write's changes",
             "    int64_t changed = sqlite3_stmt_readonly(statement) ? 0 : sqlite3_changes64(db);",
             "    int64_t changed = sqlite3_changes64(db);"),
    c_mutant("an empty blob is bound as null",
             "               sqlite3_bind_blob64(statement, index, length ? bytes : empty, length,",
             "               sqlite3_bind_blob64(statement, index, length ? bytes : NULL, length,"),
    c_mutant("a float loses digits",
             "  for (int precision = 15; precision <= 17; precision++) {",
             "  for (int precision = 6; precision <= 6; precision++) {"),
    c_mutant("text that is not UTF-8 is passed through",
             "      if (!valid_utf8((const unsigned char *)text, text_length)) {",
             "      if (0) {"),
    c_mutant("the declared type is never given",
             "                                  text_value(call, sqlite3_column_decltype(statement, i))};",
             "                                  text_value(call, NULL)};"),
]
