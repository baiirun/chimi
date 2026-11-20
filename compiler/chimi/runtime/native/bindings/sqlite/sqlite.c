/* std/sqlite: the binding over the vendored SQLite amalgamation
   (sqlite/PROVENANCE.md). A database and a prepared statement are handles.
   No call here waits: there is no busy handler, so a locked database answers
   SQLITE_BUSY at once and std/sqlite.chimi retries after a parked wait.
   Several tasks may share one connection, so each call that reads the
   connection's error message or change count holds the connection's mutex
   for the call and the read together. */
#include "chimi_binding.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "sqlite/sqlite3.h"

/* Failure codes below SQLite's own (which are all positive). */
enum {
  SQLITE_BINDING_CLOSED = -1,
  SQLITE_BINDING_MANY_STATEMENTS = -2,
  SQLITE_BINDING_NOT_UTF8 = -3,
  SQLITE_BINDING_BAD_FLOAT = -4,
};

/* What a column value is, in the tuples `native_row` returns. */
enum {
  VALUE_NULL = 0,
  VALUE_INTEGER = 1,
  VALUE_FLOAT = 2,
  VALUE_TEXT = 3,
  VALUE_BLOB = 4,
};

static void release_database(void *state);
static void release_statement(void *state);
static const ChimiBindingType DATABASE_TYPE = {"SqliteDb", release_database};
static const ChimiBindingType STATEMENT_TYPE = {"SqliteStatement", release_statement};

/* An open connection, and how many transaction levels std/sqlite.chimi has
   open on it: 0 outside a transaction, 1 in one, n + 1 in savepoint n. */
typedef struct {
  sqlite3 *db;
  int64_t depth;
} Database;

/* A prepared statement, and the connection's total change count when it was
   prepared, so a statement that wrote nothing reports 0 changes. */
typedef struct {
  sqlite3_stmt *statement;
  sqlite3_int64 changes_before;
} Prepared;

static void release_database(void *state) {
  Database *database = state;
  sqlite3_close_v2(database->db);
  free(database);
}

static void release_statement(void *state) {
  Prepared *prepared = state;
  sqlite3_finalize(prepared->statement);
  free(prepared);
}

static sqlite3 *database_argument(ChimiBindingCall *call, uint32_t index) {
  Database *database = chimi_binding_handle(call, index, &DATABASE_TYPE);
  return database ? database->db : NULL;
}

static sqlite3_stmt *statement_argument(ChimiBindingCall *call, uint32_t index) {
  Prepared *prepared = chimi_binding_handle(call, index, &STATEMENT_TYPE);
  return prepared ? prepared->statement : NULL;
}


static ChimiBindingResult *closed(ChimiBindingCall *call) {
  return chimi_binding_failure(call, SQLITE_BINDING_CLOSED, "the database is closed");
}

/* The connection's failure, read while the caller holds its mutex. */
static ChimiBindingResult *database_failure(ChimiBindingCall *call, sqlite3 *db, int code) {
  return chimi_binding_failure(call, code, sqlite3_errmsg(db));
}

static ChimiBindingResult *ok_void(ChimiBindingCall *call) {
  return chimi_binding_ok(call, chimi_binding_void(call));
}

/* native_version() -> Text */
ChimiBindingResult *chimi_sqlite_version(ChimiBindingCall *call) {
  const char *version = sqlite3_libversion();
  return chimi_binding_success(call, chimi_binding_make_text(call, version, strlen(version)));
}

/* Whether the pragma `name` moves SQLite's files to a directory a statement
   names, past the files provider. */
static int moves_files(const char *name) {
  return name && (sqlite3_stricmp(name, "temp_store_directory") == 0
                  || sqlite3_stricmp(name, "data_store_directory") == 0);
}

/* Refuses ATTACH (which VACUUM INTO also asks for) and the pragmas that move
   SQLite's files, so a statement cannot open a file in a directory the files
   provider did not check. */
static int refuse_attach(void *unused, int action, const char *a, const char *b, const char *c,
                         const char *d) {
  (void)unused;
  (void)b;
  (void)c;
  (void)d;
  if (action == SQLITE_ATTACH) return SQLITE_DENY;
  if (action == SQLITE_PRAGMA && moves_files(a)) return SQLITE_DENY;
  return SQLITE_OK;
}

/* native_open(target: Text) -> Result<SqliteDb, (Int, Text)>: opens or
   creates the database at a path or `file:` URI. */
ChimiBindingResult *chimi_sqlite_open(ChimiBindingCall *call) {
  const char *target;
  size_t length;
  if (!chimi_binding_text(call, 0, &target, &length)) {
    return chimi_binding_defect(call, "std/sqlite: open takes a target");
  }
  if (strlen(target) != length) {
    return chimi_binding_failure(call, SQLITE_CANTOPEN, "the target holds a NUL character");
  }
  sqlite3 *db = NULL;
  int flags = SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_FULLMUTEX
              | SQLITE_OPEN_EXRESCODE;
  int code = sqlite3_open_v2(target, &db, flags, NULL);
  if (code != SQLITE_OK) {
    char message[512];
    snprintf(message, sizeof message, "%s", db ? sqlite3_errmsg(db) : sqlite3_errstr(code));
    sqlite3_close_v2(db);
    return chimi_binding_failure(call, code, message);
  }
  /* Another database file may never be reached from this one: the files
     provider checked only the target's directory. */
  sqlite3_limit(db, SQLITE_LIMIT_ATTACHED, 0);
  sqlite3_set_authorizer(db, refuse_attach, NULL);
  Database *database = malloc(sizeof *database);
  if (!database) {
    sqlite3_close_v2(db);
    return chimi_binding_failure(call, SQLITE_NOMEM, "out of memory");
  }
  database->db = db;
  database->depth = 0;
  ChimiBindingValue *handle = chimi_binding_make_handle(call, &DATABASE_TYPE, database);
  if (!handle) {
    return chimi_binding_failure(call, SQLITE_CANTOPEN, "too many open databases");
  }
  return chimi_binding_ok(call, handle);
}

/* native_close(db: SqliteDb) -> Void. Statements still open keep the
   connection until they are finalized. */
ChimiBindingResult *chimi_sqlite_close(ChimiBindingCall *call) {
  chimi_binding_close_handle(call, 0, &DATABASE_TYPE);
  return chimi_binding_success(call, chimi_binding_void(call));
}

/* native_autocommit(db: SqliteDb) -> Result<Bool, (Int, Text)>: whether no
   transaction is open. */
ChimiBindingResult *chimi_sqlite_autocommit(ChimiBindingCall *call) {
  sqlite3 *db = database_argument(call, 0);
  if (!db) return closed(call);
  return chimi_binding_ok(call, chimi_binding_make_boolean(call, sqlite3_get_autocommit(db)));
}

/* native_depth(db: SqliteDb) -> Result<Int, (Int, Text)>: the transaction
   levels open, as native_set_depth last set them. */
ChimiBindingResult *chimi_sqlite_depth(ChimiBindingCall *call) {
  Database *database = chimi_binding_handle(call, 0, &DATABASE_TYPE);
  if (!database) return closed(call);
  return chimi_binding_ok(call, chimi_binding_make_integer(call, database->depth));
}

/* native_set_depth(db: SqliteDb, depth: Int) -> Result<Void, (Int, Text)> */
ChimiBindingResult *chimi_sqlite_set_depth(ChimiBindingCall *call) {
  int64_t depth;
  Database *database = chimi_binding_handle(call, 0, &DATABASE_TYPE);
  if (!chimi_binding_integer(call, 1, &depth)) {
    return chimi_binding_defect(call, "std/sqlite: set_depth takes an Int");
  }
  if (!database) return closed(call);
  database->depth = depth;
  return ok_void(call);
}

/* Whether `rest` holds only whitespace and SQL comments. */
static int only_blank(const char *rest, const char *end) {
  while (rest < end) {
    if (*rest == ' ' || *rest == '\t' || *rest == '\n' || *rest == '\r' || *rest == '\f'
        || *rest == ';') {
      rest++;
    } else if (rest + 1 < end && rest[0] == '-' && rest[1] == '-') {
      while (rest < end && *rest != '\n') rest++;
    } else if (rest + 1 < end && rest[0] == '/' && rest[1] == '*') {
      rest += 2;
      while (rest + 1 < end && !(rest[0] == '*' && rest[1] == '/')) rest++;
      rest = rest + 1 < end ? rest + 2 : end;
    } else {
      return 0;
    }
  }
  return 1;
}

/* native_prepare(db: SqliteDb, sql: Text) -> Result<Option<SqliteStatement>, (Int, Text)>:
   `None` for a text with no statement in it. A text with a second statement
   after the first fails. */
ChimiBindingResult *chimi_sqlite_prepare(ChimiBindingCall *call) {
  const char *sql;
  size_t length;
  if (!chimi_binding_text(call, 1, &sql, &length)) {
    return chimi_binding_defect(call, "std/sqlite: prepare takes a statement");
  }
  sqlite3 *db = database_argument(call, 0);
  if (!db) return closed(call);
  sqlite3_mutex_enter(sqlite3_db_mutex(db));
  sqlite3_int64 changes_before = sqlite3_total_changes64(db);
  sqlite3_stmt *statement = NULL;
  const char *tail = NULL;
  int code = sqlite3_prepare_v2(db, sql, (int)length, &statement, &tail);
  if (code != SQLITE_OK) {
    ChimiBindingResult *result = database_failure(call, db, code);
    sqlite3_mutex_leave(sqlite3_db_mutex(db));
    return result;
  }
  sqlite3_mutex_leave(sqlite3_db_mutex(db));
  if (tail && !only_blank(tail, sql + length)) {
    sqlite3_finalize(statement);
    return chimi_binding_failure(call, SQLITE_BINDING_MANY_STATEMENTS,
                                 "the text holds more than one statement");
  }
  if (!statement) return chimi_binding_ok(call, chimi_binding_none(call));
  Prepared *prepared = malloc(sizeof *prepared);
  if (!prepared) {
    sqlite3_finalize(statement);
    return chimi_binding_failure(call, SQLITE_NOMEM, "out of memory");
  }
  prepared->statement = statement;
  prepared->changes_before = changes_before;
  ChimiBindingValue *handle = chimi_binding_make_handle(call, &STATEMENT_TYPE, prepared);
  if (!handle) {
    return chimi_binding_failure(call, SQLITE_NOMEM, "too many open statements");
  }
  return chimi_binding_ok(call, chimi_binding_some(call, handle));
}

/* native_finalize(statement: SqliteStatement) -> Void */
ChimiBindingResult *chimi_sqlite_finalize(ChimiBindingCall *call) {
  chimi_binding_close_handle(call, 0, &STATEMENT_TYPE);
  return chimi_binding_success(call, chimi_binding_void(call));
}

/* native_parameter_count(statement: SqliteStatement) -> Int */
ChimiBindingResult *chimi_sqlite_parameter_count(ChimiBindingCall *call) {
  sqlite3_stmt *statement = statement_argument(call, 0);
  if (!statement) return chimi_binding_defect(call, "std/sqlite: the statement is finalized");
  return chimi_binding_success(call,
                               chimi_binding_make_integer(call, sqlite3_bind_parameter_count(statement)));
}

/* The statement argument and the 1-based parameter index after it, for the
   bind calls; NULL after a defect result is stored in `*result`. */
static sqlite3_stmt *bind_target(ChimiBindingCall *call, int *index, ChimiBindingResult **result) {
  int64_t position;
  sqlite3_stmt *statement = statement_argument(call, 0);
  if (!statement || !chimi_binding_integer(call, 1, &position) || position < 1
      || position > sqlite3_bind_parameter_count(statement)) {
    *result = chimi_binding_defect(call, "std/sqlite: a bind outside the statement's parameters");
    return NULL;
  }
  *index = (int)position;
  return statement;
}

static ChimiBindingResult *bound(ChimiBindingCall *call, sqlite3_stmt *statement, int code) {
  if (code == SQLITE_OK) return ok_void(call);
  return chimi_binding_failure(call, code, sqlite3_errstr(code));
}

/* native_bind_null(statement, index) -> Result<Void, (Int, Text)> */
ChimiBindingResult *chimi_sqlite_bind_null(ChimiBindingCall *call) {
  int index;
  ChimiBindingResult *result;
  sqlite3_stmt *statement = bind_target(call, &index, &result);
  if (!statement) return result;
  return bound(call, statement, sqlite3_bind_null(statement, index));
}

/* native_bind_integer(statement, index, value: Int) -> Result<Void, (Int, Text)> */
ChimiBindingResult *chimi_sqlite_bind_integer(ChimiBindingCall *call) {
  int index;
  int64_t value;
  ChimiBindingResult *result;
  sqlite3_stmt *statement = bind_target(call, &index, &result);
  if (!statement) return result;
  if (!chimi_binding_integer(call, 2, &value)) {
    return chimi_binding_defect(call, "std/sqlite: bind_integer takes an Int");
  }
  return bound(call, statement, sqlite3_bind_int64(statement, index, value));
}

/* native_bind_float(statement, index, value: Text) -> Result<Void, (Int, Text)>:
   the value as `Float.to_text` writes it, which `strtod` reads back exactly. */
ChimiBindingResult *chimi_sqlite_bind_float(ChimiBindingCall *call) {
  int index;
  const char *text;
  size_t length;
  ChimiBindingResult *result;
  sqlite3_stmt *statement = bind_target(call, &index, &result);
  if (!statement) return result;
  if (!chimi_binding_text(call, 2, &text, &length)) {
    return chimi_binding_defect(call, "std/sqlite: bind_float takes a Text");
  }
  char *end;
  double value = strtod(text, &end);
  if (end != text + length) {
    return chimi_binding_failure(call, SQLITE_BINDING_BAD_FLOAT, "a Float that does not parse");
  }
  return bound(call, statement, sqlite3_bind_double(statement, index, value));
}

/* native_bind_text(statement, index, value: Text) -> Result<Void, (Int, Text)> */
ChimiBindingResult *chimi_sqlite_bind_text(ChimiBindingCall *call) {
  int index;
  const char *text;
  size_t length;
  ChimiBindingResult *result;
  sqlite3_stmt *statement = bind_target(call, &index, &result);
  if (!statement) return result;
  if (!chimi_binding_text(call, 2, &text, &length)) {
    return chimi_binding_defect(call, "std/sqlite: bind_text takes a Text");
  }
  return bound(call, statement,
               sqlite3_bind_text64(statement, index, text, length, SQLITE_TRANSIENT, SQLITE_UTF8));
}

/* native_bind_bytes(statement, index, value: Bytes) -> Result<Void, (Int, Text)> */
ChimiBindingResult *chimi_sqlite_bind_bytes(ChimiBindingCall *call) {
  int index;
  const unsigned char *bytes;
  size_t length;
  ChimiBindingResult *result;
  sqlite3_stmt *statement = bind_target(call, &index, &result);
  if (!statement) return result;
  if (!chimi_binding_bytes(call, 2, &bytes, &length)) {
    return chimi_binding_defect(call, "std/sqlite: bind_bytes takes Bytes");
  }
  /* A zero-length blob is still a blob, not NULL. */
  static const unsigned char empty[1] = {0};
  return bound(call, statement,
               sqlite3_bind_blob64(statement, index, length ? bytes : empty, length,
                                   SQLITE_TRANSIENT));
}

/* native_step(statement: SqliteStatement) -> Result<Option<Int>, (Int, Text)>:
   `None` when a row is ready, or `Some(changed)` when the statement is done,
   with the rows a write changed (0 for a statement that only reads). */
ChimiBindingResult *chimi_sqlite_step(ChimiBindingCall *call) {
  Prepared *prepared = chimi_binding_handle(call, 0, &STATEMENT_TYPE);
  if (!prepared) return chimi_binding_defect(call, "std/sqlite: the statement is finalized");
  sqlite3_stmt *statement = prepared->statement;
  sqlite3 *db = sqlite3_db_handle(statement);
  sqlite3_mutex_enter(sqlite3_db_mutex(db));
  int code = sqlite3_step(statement);
  ChimiBindingResult *result;
  if (code == SQLITE_ROW) {
    result = chimi_binding_ok(call, chimi_binding_none(call));
  } else if (code == SQLITE_DONE) {
    /* sqlite3_changes64 keeps the count of the last INSERT, UPDATE or DELETE,
       so a statement that changed nothing (a read, CREATE TABLE) reports 0. */
    int64_t changed =
        sqlite3_total_changes64(db) == prepared->changes_before ? 0 : sqlite3_changes64(db);
    result = chimi_binding_ok(call, chimi_binding_some(call, chimi_binding_make_integer(call, changed)));
  } else {
    result = database_failure(call, db, sqlite3_extended_errcode(db));
  }
  sqlite3_mutex_leave(sqlite3_db_mutex(db));
  return result;
}

static ChimiBindingValue *text_value(ChimiBindingCall *call, const char *text) {
  if (!text) text = "";
  return chimi_binding_make_text(call, text, strlen(text));
}

/* native_columns(statement: SqliteStatement) -> List<(Text, Text)>: each
   column's name and declared type ("" for an expression). */
ChimiBindingResult *chimi_sqlite_columns(ChimiBindingCall *call) {
  sqlite3_stmt *statement = statement_argument(call, 0);
  if (!statement) return chimi_binding_defect(call, "std/sqlite: the statement is finalized");
  int count = sqlite3_column_count(statement);
  ChimiBindingValue **columns = calloc(count ? (size_t)count : 1, sizeof *columns);
  if (!columns) return chimi_binding_defect(call, "std/sqlite: out of memory");
  for (int i = 0; i < count; i++) {
    ChimiBindingValue *pair[2] = {text_value(call, sqlite3_column_name(statement, i)),
                                  text_value(call, sqlite3_column_decltype(statement, i))};
    columns[i] = chimi_binding_make_tuple(call, pair, 2);
  }
  ChimiBindingValue *list = chimi_binding_make_list(call, columns, (uint32_t)count);
  free(columns);
  return chimi_binding_success(call, list);
}

/* Whether `bytes` is valid UTF-8 (RFC 3629: no overlongs, no surrogates). */
static int valid_utf8(const unsigned char *bytes, size_t length) {
  size_t i = 0;
  while (i < length) {
    unsigned char lead = bytes[i];
    size_t extra;
    uint32_t point;
    if (lead < 0x80) {
      i++;
      continue;
    } else if (lead >= 0xC2 && lead <= 0xDF) {
      extra = 1;
      point = lead & 0x1F;
    } else if (lead >= 0xE0 && lead <= 0xEF) {
      extra = 2;
      point = lead & 0x0F;
    } else if (lead >= 0xF0 && lead <= 0xF4) {
      extra = 3;
      point = lead & 0x07;
    } else {
      return 0;
    }
    if (i + extra >= length) return 0;
    for (size_t k = 1; k <= extra; k++) {
      if ((bytes[i + k] & 0xC0) != 0x80) return 0;
      point = (point << 6) | (bytes[i + k] & 0x3F);
    }
    if ((extra == 2 && point < 0x800) || (extra == 3 && (point < 0x10000 || point > 0x10FFFF))
        || (point >= 0xD800 && point <= 0xDFFF)) {
      return 0;
    }
    i += extra + 1;
  }
  return 1;
}

/* The shortest `%.Ng` text that reads back as `value`. */
static void float_text(double value, char *out, size_t size) {
  for (int precision = 15; precision <= 17; precision++) {
    snprintf(out, size, "%.*g", precision, value);
    if (strtod(out, NULL) == value) return;
  }
}

/* native_row(statement: SqliteStatement) -> Result<List<(Int, Int, Text, Bytes)>, (Int, Text)>:
   the current row's values as (kind, integer, text, bytes), where kind is
   0 null, 1 integer, 2 float (as text), 3 text, 4 blob. A text value that is
   not UTF-8 fails. */
ChimiBindingResult *chimi_sqlite_row(ChimiBindingCall *call) {
  sqlite3_stmt *statement = statement_argument(call, 0);
  if (!statement) return chimi_binding_defect(call, "std/sqlite: the statement is finalized");
  int count = sqlite3_data_count(statement);
  ChimiBindingValue **values = calloc(count ? (size_t)count : 1, sizeof *values);
  if (!values) return chimi_binding_defect(call, "std/sqlite: out of memory");
  for (int i = 0; i < count; i++) {
    int kind = VALUE_NULL;
    int64_t integer = 0;
    const char *text = "";
    size_t text_length = 0;
    const unsigned char *bytes = NULL;
    size_t bytes_length = 0;
    char number[32];
    switch (sqlite3_column_type(statement, i)) {
    case SQLITE_INTEGER:
      kind = VALUE_INTEGER;
      integer = sqlite3_column_int64(statement, i);
      break;
    case SQLITE_FLOAT:
      kind = VALUE_FLOAT;
      float_text(sqlite3_column_double(statement, i), number, sizeof number);
      text = number;
      text_length = strlen(number);
      break;
    case SQLITE_TEXT:
      kind = VALUE_TEXT;
      text = (const char *)sqlite3_column_text(statement, i);
      text_length = (size_t)sqlite3_column_bytes(statement, i);
      if (!text) text = "";
      if (!valid_utf8((const unsigned char *)text, text_length)) {
        free(values);
        return chimi_binding_failure(call, SQLITE_BINDING_NOT_UTF8, sqlite3_column_name(statement, i));
      }
      break;
    case SQLITE_BLOB:
      kind = VALUE_BLOB;
      bytes = sqlite3_column_blob(statement, i);
      bytes_length = (size_t)sqlite3_column_bytes(statement, i);
      break;
    default:
      break;
    }
    ChimiBindingValue *items[4] = {
        chimi_binding_make_integer(call, kind),
        chimi_binding_make_integer(call, integer),
        chimi_binding_make_text(call, text, text_length),
        chimi_binding_make_bytes(call, bytes ? bytes : (const unsigned char *)"", bytes_length),
    };
    values[i] = chimi_binding_make_tuple(call, items, 4);
  }
  ChimiBindingValue *list = chimi_binding_make_list(call, values, (uint32_t)count);
  free(values);
  return chimi_binding_ok(call, list);
}
