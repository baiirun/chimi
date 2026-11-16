#ifndef CHIMI_NATIVE_RUNTIME_H
#define CHIMI_NATIVE_RUNTIME_H

#include <stdint.h>

#define CHIMI_RUNTIME_ABI_VERSION 32

typedef struct ChimiRuntime ChimiRuntime;
typedef struct ChimiEnvironment ChimiEnvironment;
typedef struct ChimiControl ChimiControl;
typedef ChimiControl *(*ChimiNodeFn)(ChimiRuntime *, ChimiEnvironment *, uint64_t);

enum {
  CHIMI_SYMBOL_FUNCTION,
  CHIMI_SYMBOL_CONSTANT,
  CHIMI_SYMBOL_REQUIREMENT,
  CHIMI_SYMBOL_LOCAL,
  CHIMI_SYMBOL_CONSTRUCTOR,
  CHIMI_SYMBOL_INTRINSIC,
  CHIMI_SYMBOL_OTHER,
  /* A `process name`: a reference is its named address. The scheduler's name id
     is the symbol index plus 1, and the symbol's name is its trace label. */
  CHIMI_SYMBOL_PROCESS_NAME,
};

enum {
  CHIMI_TYPE_CONSOLE,
  CHIMI_TYPE_CONSOLE_ERROR,
  CHIMI_TYPE_FILE_ERROR,
  CHIMI_TYPE_FILE_SYSTEM,
  CHIMI_TYPE_MAP,
  CHIMI_TYPE_PATH,
  CHIMI_TYPE_UTF8_ERROR,
  CHIMI_TYPE_JSON_VALUE,
  CHIMI_TYPE_JSON_NUMBER,
  CHIMI_TYPE_DECODE_ERROR,
  CHIMI_TYPE_JSON_CONVERSION_ERROR,
  CHIMI_TYPE_OPTION,
  CHIMI_TYPE_CHANNEL_SENDER,
  CHIMI_TYPE_CHANNEL_RECEIVER,
  CHIMI_TYPE_CHANNEL_COMPLETION,
  CHIMI_TYPE_CHANNEL_SEND_ERROR,
  CHIMI_TYPE_CHANNEL_RECEIVE,
  CHIMI_TYPE_CHANNEL_ERROR,
  CHIMI_TYPE_TASK,
  CHIMI_TYPE_INT_PARSE_ERROR,
  CHIMI_TYPE_FLOAT_PARSE_ERROR,
  CHIMI_TYPE_FLOAT_CONVERSION_ERROR,
  CHIMI_TYPE_ORDERING,
  CHIMI_TYPE_DURATION,
  CHIMI_TYPE_DURATION_PARSE_ERROR,
  CHIMI_TYPE_PROCESS_ADDRESS,
  CHIMI_TYPE_PROCESS_INBOX,
  CHIMI_TYPE_PROCESS_SPEC,
  CHIMI_TYPE_PROCESS_SEND_ERROR,
  CHIMI_TYPE_SUPERVISOR_MEMBER,
  CHIMI_TYPE_SUPERVISOR_POLICY,
  CHIMI_TYPE_SUPERVISOR_RESTARTS,
  CHIMI_TYPE_SUPERVISOR_RESTART,
  CHIMI_TYPE_SUPERVISOR_RUNNING,
  CHIMI_TYPE_SUPERVISOR_MANAGED_PROCESS,
  CHIMI_TYPE_SUPERVISOR_MANAGED_SUPERVISOR,
  CHIMI_TYPE_SUPERVISOR_START_ERROR,
  CHIMI_TYPE_CLOCK,
  CHIMI_TYPE_SET,
  CHIMI_TYPE_DECIMAL,
  CHIMI_TYPE_BYTES,
  CHIMI_TYPE_INSTANT,
  CHIMI_TYPE_TIMESTAMP,
  CHIMI_TYPE_TIMEOUT,
  CHIMI_TYPE_CAUSE,
  CHIMI_TYPE_RESULT,
};

enum {
  CHIMI_INTRINSIC_IO_PATH,
  CHIMI_INTRINSIC_LIST_FIRST,
  CHIMI_INTRINSIC_LIST_LENGTH,
  CHIMI_INTRINSIC_MAP_EMPTY,
  CHIMI_INTRINSIC_MAP_GET,
  CHIMI_INTRINSIC_MAP_PUT,
  CHIMI_INTRINSIC_TEXT_APPEND,
  CHIMI_INTRINSIC_TEXT_EQUAL,
  CHIMI_INTRINSIC_TASK_START,
  CHIMI_INTRINSIC_TASK_RESULT,
  CHIMI_INTRINSIC_TASK_CANCEL,
  CHIMI_INTRINSIC_CHANNEL_SEND,
  CHIMI_INTRINSIC_CHANNEL_RECEIVE,
  CHIMI_INTRINSIC_CHANNEL_CLOSE,
  CHIMI_INTRINSIC_CHANNEL_ABORT,
  CHIMI_INTRINSIC_CHANNEL_BOUNDED,
  CHIMI_INTRINSIC_INT_PARSE,
  CHIMI_INTRINSIC_INT_TO_TEXT,
  CHIMI_INTRINSIC_INT_ADD_CHECKED,
  CHIMI_INTRINSIC_INT_SUBTRACT_CHECKED,
  CHIMI_INTRINSIC_INT_MULTIPLY_CHECKED,
  CHIMI_INTRINSIC_INT_DIVIDE_CHECKED,
  CHIMI_INTRINSIC_INT_REMAINDER_CHECKED,
  CHIMI_INTRINSIC_INT_MODULO,
  CHIMI_INTRINSIC_FLOAT_FROM_INT,
  CHIMI_INTRINSIC_FLOAT_TRUNCATE,
  CHIMI_INTRINSIC_FLOAT_FLOOR,
  CHIMI_INTRINSIC_FLOAT_CEILING,
  CHIMI_INTRINSIC_FLOAT_ROUND,
  CHIMI_INTRINSIC_FLOAT_TO_INT,
  CHIMI_INTRINSIC_FLOAT_TO_TEXT,
  CHIMI_INTRINSIC_FLOAT_PARSE,
  CHIMI_INTRINSIC_DEBUG_RENDER,
  CHIMI_INTRINSIC_DEBUG_RENDER_INDENTED,
  CHIMI_INTRINSIC_ORDER_COMPARE,
  CHIMI_INTRINSIC_ORDER_MIN,
  CHIMI_INTRINSIC_ORDER_MAX,
  CHIMI_INTRINSIC_RANGE_START,
  CHIMI_INTRINSIC_RANGE_END,
  CHIMI_INTRINSIC_RANGE_LENGTH,
  CHIMI_INTRINSIC_RANGE_CONTAINS,
  CHIMI_INTRINSIC_DURATION_NANOSECONDS,
  CHIMI_INTRINSIC_DURATION_MICROSECONDS,
  CHIMI_INTRINSIC_DURATION_MILLISECONDS,
  CHIMI_INTRINSIC_DURATION_SECONDS,
  CHIMI_INTRINSIC_DURATION_MINUTES,
  CHIMI_INTRINSIC_DURATION_HOURS,
  CHIMI_INTRINSIC_DURATION_TO_NANOSECONDS,
  CHIMI_INTRINSIC_DURATION_TO_MICROSECONDS,
  CHIMI_INTRINSIC_DURATION_TO_MILLISECONDS,
  CHIMI_INTRINSIC_DURATION_TO_SECONDS,
  CHIMI_INTRINSIC_DURATION_ADD,
  CHIMI_INTRINSIC_DURATION_SUBTRACT,
  CHIMI_INTRINSIC_DURATION_MULTIPLY,
  CHIMI_INTRINSIC_DURATION_DIVIDE,
  CHIMI_INTRINSIC_DURATION_NEGATE,
  CHIMI_INTRINSIC_DURATION_ABSOLUTE,
  CHIMI_INTRINSIC_DURATION_TO_TEXT,
  CHIMI_INTRINSIC_DURATION_PARSE,
  CHIMI_INTRINSIC_CLOCK_SLEEP,
  /* The rest of std/clock, then Task.timeout and Task.deadline. */
  CHIMI_INTRINSIC_CLOCK_NOW,
  CHIMI_INTRINSIC_CLOCK_TIMESTAMP,
  CHIMI_INTRINSIC_CLOCK_SINCE,
  CHIMI_INTRINSIC_CLOCK_SLEEP_UNTIL,
  CHIMI_INTRINSIC_CLOCK_ADD,
  CHIMI_INTRINSIC_CLOCK_BETWEEN,
  CHIMI_INTRINSIC_CLOCK_TIMESTAMP_TO_TEXT,
  CHIMI_INTRINSIC_CLOCK_UNIX_NANOSECONDS,
  CHIMI_INTRINSIC_CLOCK_FROM_UNIX_NANOSECONDS,
  CHIMI_INTRINSIC_CLOCK_TIMESTAMP_BETWEEN,
  CHIMI_INTRINSIC_CLOCK_STARTING_AT,
  CHIMI_INTRINSIC_TASK_TIMEOUT,
  CHIMI_INTRINSIC_TASK_DEADLINE,
  CHIMI_INTRINSIC_TASK_ALL,
  CHIMI_INTRINSIC_TASK_RESULTS,
  CHIMI_INTRINSIC_TASK_RACE,
  /* std/select. */
  CHIMI_INTRINSIC_SELECT_RECEIVE,
  CHIMI_INTRINSIC_SELECT_INBOX,
  CHIMI_INTRINSIC_SELECT_TASK,
  CHIMI_INTRINSIC_SELECT_AFTER,
  CHIMI_INTRINSIC_SELECT_AT,
  CHIMI_INTRINSIC_SELECT_FIRST,
  /* The process intrinsics follow StandardFunction's declaration order. */
  CHIMI_INTRINSIC_PROCESS_SPEC,
  CHIMI_INTRINSIC_PROCESS_REGISTER,
  CHIMI_INTRINSIC_PROCESS_RECEIVE,
  CHIMI_INTRINSIC_PROCESS_SEND,
  CHIMI_INTRINSIC_PROCESS_OWN_ADDRESS,
  CHIMI_INTRINSIC_PROCESS_QUEUED,
  CHIMI_INTRINSIC_PROCESS_CAPACITY,
  CHIMI_INTRINSIC_PROCESS_MONITOR,
  CHIMI_INTRINSIC_PROCESS_UNMONITOR,
  CHIMI_INTRINSIC_PROCESS_HANDOFF_SPEC,
  CHIMI_INTRINSIC_SUPERVISOR_PROCESS,
  CHIMI_INTRINSIC_SUPERVISOR_SUPERVISOR,
  CHIMI_INTRINSIC_SUPERVISOR_POLICY,
  CHIMI_INTRINSIC_SUPERVISOR_RESTARTS,
  CHIMI_INTRINSIC_SUPERVISOR_START,
  CHIMI_INTRINSIC_SUPERVISOR_START_PROCESS,
  CHIMI_INTRINSIC_SUPERVISOR_START_SUPERVISOR,
  CHIMI_INTRINSIC_SUPERVISOR_START_HANDOFF,
  CHIMI_INTRINSIC_SUPERVISOR_ADDRESS,
  CHIMI_INTRINSIC_SUPERVISOR_REMOVE_PROCESS,
  CHIMI_INTRINSIC_SUPERVISOR_REMOVE_SUPERVISOR,
  CHIMI_INTRINSIC_SUPERVISOR_STOP,
  CHIMI_INTRINSIC_SUPERVISOR_WAIT,
  /* The native core of the standard modules written in Chimi (operations.rs). */
  CHIMI_INTRINSIC_DEBUG_CRASH,
  CHIMI_INTRINSIC_OPTION_UNWRAP_OR_FAIL,
  CHIMI_INTRINSIC_TEXT_CONCAT,
  CHIMI_INTRINSIC_TEXT_BYTE_LENGTH,
  CHIMI_INTRINSIC_TEXT_BYTE_SLICE,
  CHIMI_INTRINSIC_TEXT_CONTAINS,
  CHIMI_INTRINSIC_TEXT_STARTS_WITH,
  CHIMI_INTRINSIC_TEXT_ENDS_WITH,
  CHIMI_INTRINSIC_TEXT_SPLIT,
  CHIMI_INTRINSIC_TEXT_REPLACE,
  CHIMI_INTRINSIC_TEXT_TRIM_START,
  CHIMI_INTRINSIC_TEXT_TRIM_END,
  CHIMI_INTRINSIC_TEXT_LOWERCASE,
  CHIMI_INTRINSIC_TEXT_UPPERCASE,
  CHIMI_INTRINSIC_TEXT_GRAPHEMES,
  CHIMI_INTRINSIC_LIST_GET,
  CHIMI_INTRINSIC_LIST_PUSH,
  CHIMI_INTRINSIC_LIST_SLICE,
  CHIMI_INTRINSIC_LIST_SET,
  /* The native cores of std/map, std/set, std/json, and std/decimal; each block is contiguous. */
  CHIMI_INTRINSIC_MAP_REMOVE,
  CHIMI_INTRINSIC_MAP_SIZE,
  CHIMI_INTRINSIC_MAP_REDUCE_ENTRIES,
  CHIMI_INTRINSIC_SET_EMPTY,
  CHIMI_INTRINSIC_SET_CONTAINS,
  CHIMI_INTRINSIC_SET_ADD,
  CHIMI_INTRINSIC_SET_REMOVE,
  CHIMI_INTRINSIC_SET_SIZE,
  CHIMI_INTRINSIC_SET_REDUCE_ITEMS,
  CHIMI_INTRINSIC_JSON_DECODE,
  CHIMI_INTRINSIC_JSON_ENCODE,
  CHIMI_INTRINSIC_JSON_INT,
  CHIMI_INTRINSIC_JSON_NUMBER_TO_INT,
  CHIMI_INTRINSIC_JSON_NUMBER_TO_DECIMAL,
  CHIMI_INTRINSIC_DECIMAL_SCALED,
  CHIMI_INTRINSIC_DECIMAL_READ,
  CHIMI_INTRINSIC_DECIMAL_TO_TEXT,
  CHIMI_INTRINSIC_DECIMAL_ADD_CHECKED,
  CHIMI_INTRINSIC_DECIMAL_SUBTRACT_CHECKED,
  CHIMI_INTRINSIC_DECIMAL_MULTIPLY_CHECKED,
  CHIMI_INTRINSIC_DECIMAL_DIVIDE_ROUNDED,
  CHIMI_INTRINSIC_DECIMAL_TO_FLOAT,
  /* The native core of std/bytes; contiguous. */
  CHIMI_INTRINSIC_BYTES_FROM,
  CHIMI_INTRINSIC_BYTES_LENGTH,
  CHIMI_INTRINSIC_BYTES_GET,
  CHIMI_INTRINSIC_BYTES_INDEX_OF,
  CHIMI_INTRINSIC_BYTES_CONCAT,
  CHIMI_INTRINSIC_BYTES_SLICE,
  CHIMI_INTRINSIC_BYTES_TO_LIST,
  CHIMI_INTRINSIC_BYTES_FROM_TEXT,
  CHIMI_INTRINSIC_BYTES_TO_TEXT_LOSSY,
  CHIMI_INTRINSIC_BYTES_VALID_UTF8_LENGTH,
  CHIMI_INTRINSIC_BYTES_READ_INT_CODE,
  CHIMI_INTRINSIC_BYTES_INT_BYTES,
  /* std/embed: lowering put the file's contents (text) or their hex (bytes) in the argument. */
  CHIMI_INTRINSIC_EMBED_TEXT,
  CHIMI_INTRINSIC_EMBED_BYTES,
  /* The native core of std/cause; contiguous. */
  CHIMI_INTRINSIC_CAUSE_CONTEXT,
  CHIMI_INTRINSIC_CAUSE_OF,
  CHIMI_INTRINSIC_CAUSE_KIND_CODE,
  CHIMI_INTRINSIC_CAUSE_SUMMARY,
  CHIMI_INTRINSIC_CAUSE_FRAMES,
  CHIMI_INTRINSIC_CAUSE_CAUSED_BY,
  CHIMI_INTRINSIC_CAUSE_SUPPRESSED,
  CHIMI_INTRINSIC_CAUSE_REASON_CODE,
  CHIMI_INTRINSIC_CAUSE_TO_TEXT,
  /* std/test assertions; contiguous. Each defects at its own call node. */
  CHIMI_INTRINSIC_TEST_ASSERT_EQUAL,
  CHIMI_INTRINSIC_TEST_ASSERT_NOT_EQUAL,
  CHIMI_INTRINSIC_TEST_ASSERT,
  CHIMI_INTRINSIC_TEST_FAIL,
};

enum {
  CHIMI_CONSTRUCTOR_CONSOLE_WRITE_FAILED,
  CHIMI_CONSTRUCTOR_FILE_CLOSE,
  CHIMI_CONSTRUCTOR_FILE_LINE_TOO_LONG,
  CHIMI_CONSTRUCTOR_FILE_OPEN,
  CHIMI_CONSTRUCTOR_FILE_READ,
  CHIMI_CONSTRUCTOR_UTF8_INVALID,
  CHIMI_CONSTRUCTOR_OPTION_SOME,
  CHIMI_CONSTRUCTOR_OPTION_NONE,
  CHIMI_CONSTRUCTOR_JSON_NULL,
  CHIMI_CONSTRUCTOR_JSON_BOOLEAN,
  CHIMI_CONSTRUCTOR_JSON_NUMBER,
  CHIMI_CONSTRUCTOR_JSON_TEXT,
  CHIMI_CONSTRUCTOR_JSON_ARRAY,
  CHIMI_CONSTRUCTOR_JSON_OBJECT,
  CHIMI_CONSTRUCTOR_DECODE_INVALID_JSON,
  CHIMI_CONSTRUCTOR_JSON_NOT_INTEGER,
  CHIMI_CONSTRUCTOR_JSON_OUT_OF_RANGE,
  CHIMI_CONSTRUCTOR_CHANNEL_SEND_CLOSED,
  CHIMI_CONSTRUCTOR_CHANNEL_SEND_ABORTED,
  CHIMI_CONSTRUCTOR_CHANNEL_RECEIVED,
  CHIMI_CONSTRUCTOR_CHANNEL_DRAINED,
  CHIMI_CONSTRUCTOR_CHANNEL_ABORTED,
  CHIMI_CONSTRUCTOR_INT_PARSE_EMPTY,
  CHIMI_CONSTRUCTOR_INT_PARSE_INVALID_DIGIT,
  CHIMI_CONSTRUCTOR_INT_PARSE_OUT_OF_RANGE,
  CHIMI_CONSTRUCTOR_FLOAT_PARSE_EMPTY,
  CHIMI_CONSTRUCTOR_FLOAT_PARSE_INVALID,
  CHIMI_CONSTRUCTOR_FLOAT_PARSE_OUT_OF_RANGE,
  CHIMI_CONSTRUCTOR_FLOAT_NOT_INTEGER,
  CHIMI_CONSTRUCTOR_FLOAT_OUT_OF_RANGE,
  CHIMI_CONSTRUCTOR_ORDERING_LESS,
  CHIMI_CONSTRUCTOR_ORDERING_EQUAL,
  CHIMI_CONSTRUCTOR_ORDERING_GREATER,
  CHIMI_CONSTRUCTOR_DURATION_PARSE_EMPTY,
  CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID,
  CHIMI_CONSTRUCTOR_DURATION_PARSE_OUT_OF_RANGE,
  CHIMI_CONSTRUCTOR_PROCESS_SEND_FULL,
  CHIMI_CONSTRUCTOR_PROCESS_SEND_UNAVAILABLE,
  CHIMI_CONSTRUCTOR_RESTART_NEVER,
  CHIMI_CONSTRUCTOR_RESTART_ON_DEFECT,
  CHIMI_CONSTRUCTOR_START_CAPACITY_REACHED,
  CHIMI_CONSTRUCTOR_START_NAME_TAKEN,
  CHIMI_CONSTRUCTOR_START_STOPPED,
  CHIMI_CONSTRUCTOR_TIMEOUT_TIMED_OUT,
  CHIMI_CONSTRUCTOR_DOWN_RETURNED,
  CHIMI_CONSTRUCTOR_DOWN_FAILED,
  CHIMI_CONSTRUCTOR_DOWN_DEFECTED,
  CHIMI_CONSTRUCTOR_DOWN_STOPPED,
  CHIMI_CONSTRUCTOR_DOWN_UNAVAILABLE,
  CHIMI_CONSTRUCTOR_RESULT_OK,
  CHIMI_CONSTRUCTOR_RESULT_ERR,
  CHIMI_CONSTRUCTOR_HANDOFF_CAPACITY_REACHED,
  CHIMI_CONSTRUCTOR_HANDOFF_STOPPED,
};

enum {
  CHIMI_LITERAL_INTEGER,
  CHIMI_LITERAL_TEXT,
  CHIMI_LITERAL_BOOLEAN,
  CHIMI_LITERAL_FLOAT,
  CHIMI_LITERAL_VOID,
};

enum {
  CHIMI_BINARY_ADD,
  CHIMI_BINARY_SUBTRACT,
  CHIMI_BINARY_MULTIPLY,
  CHIMI_BINARY_DIVIDE,
  CHIMI_BINARY_LESS,
  CHIMI_BINARY_LESS_EQUAL,
  CHIMI_BINARY_GREATER,
  CHIMI_BINARY_GREATER_EQUAL,
  CHIMI_BINARY_REMAINDER,
  CHIMI_BINARY_EQUAL,
  CHIMI_BINARY_RANGE,
  CHIMI_BINARY_RANGE_INCLUSIVE,
};

enum {
  CHIMI_PATTERN_WILDCARD,
  CHIMI_PATTERN_BINDING,
  CHIMI_PATTERN_LITERAL,
  CHIMI_PATTERN_CONSTRUCTOR,
  CHIMI_PATTERN_OUTCOME,
  /* `a` counts the prefix patterns; `integer` is 1 when a rest pattern follows them.
     The ids at `offset` are the prefix, then the rest pattern if present, then the
     suffix, `length` in all. */
  CHIMI_PATTERN_LIST,
  /* `a` is the binding symbol and `offset` the pattern id it aliases. */
  CHIMI_PATTERN_ALIAS,
  /* The ids at `offset` match the tuple's `length` elements in order. */
  CHIMI_PATTERN_TUPLE,
};

typedef struct {
  uint32_t file_id;
  uint32_t semantic_id;
  uint32_t start;
  uint32_t end;
  /* The string `module/path.chimi:line` that causes show for it. */
  uint32_t location;
} ChimiOrigin;

typedef struct {
  uint32_t origin;
  uint32_t a;
  uint32_t b;
  uint32_t c;
  uint32_t d;
  uint32_t offset;
  uint32_t length;
  int64_t integer;
} ChimiNode;

typedef struct {
  uint32_t kind;
  uint32_t name;
  uint32_t function;
  uint32_t constant; /* index of the constant's value in CHIMI_CONSTANT_VALUES */
  uint32_t constructor;
  uint32_t intrinsic;
  uint32_t standard_constructor;
  uint32_t standard_type;
  uint32_t environment_slot;
} ChimiSymbol;

/* One value of a module constant, computed by the compiler. Children precede their
   parent, so the runtime decodes the table front to back.
   TEXT and JSON_NUMBER: `a` is a string id. LIST and CONSTRUCTED: `length` value ids at
   `offset` in CHIMI_IDS, and `a` is the constructor symbol. RECORD and MAP: `length`
   (string id, value id) pairs at `offset`. DECLARED, INTRINSIC, and CONSTRUCTOR: `a` is
   the symbol. CLOSURE: `a` is the closure node and `length` (symbol, value id) capture
   pairs sit at `offset`. DECLARED and CLOSURE: `integer` (requirement name string id,
   value id) provider pairs follow the captures at `offset`. INTEGER, BOOLEAN, and FLOAT
   (IEEE bits) use `integer`. */
enum {
  CHIMI_CONSTANT_VOID,
  CHIMI_CONSTANT_INTEGER,
  CHIMI_CONSTANT_FLOAT,
  CHIMI_CONSTANT_TEXT,
  CHIMI_CONSTANT_BOOLEAN,
  CHIMI_CONSTANT_LIST,
  CHIMI_CONSTANT_RECORD,
  CHIMI_CONSTANT_CONSTRUCTED,
  CHIMI_CONSTANT_MAP,
  CHIMI_CONSTANT_JSON_NUMBER,
  CHIMI_CONSTANT_DECLARED,
  CHIMI_CONSTANT_INTRINSIC,
  CHIMI_CONSTANT_CONSTRUCTOR,
  CHIMI_CONSTANT_CLOSURE,
  /* `length` element value ids at `offset`. */
  CHIMI_CONSTANT_TUPLE,
  /* Two INTEGER value ids at `offset`: the start and the exclusive end. */
  CHIMI_CONSTANT_RANGE,
  /* `integer` is the nanosecond count. */
  CHIMI_CONSTANT_DURATION,
  /* `a` is the `process name` symbol of a named address. */
  CHIMI_CONSTANT_PROCESS_ADDRESS,
  /* A `Decimal`: `a` is the scale with the sign in bit 8; `offset` and `length` are
     the high and low halves of the magnitude's high 64 bits; `integer` holds its
     low 64 bits. */
  CHIMI_CONSTANT_DECIMAL,
  /* `length` item value ids at `offset`, in ascending order. */
  CHIMI_CONSTANT_SET,
  /* A `Bytes`: `a` is the id of its lowercase hex text, two digits per byte. */
  CHIMI_CONSTANT_BYTES,
};

typedef struct {
  uint32_t kind;
  uint32_t a;
  uint32_t offset;
  uint32_t length;
  int64_t integer;
} ChimiConstantValue;

typedef struct {
  uint32_t symbol;
  uint32_t body;
  uint32_t parameters_offset;
  uint32_t parameters_length;
  /* One environment slot per local of the body, closures included. */
  uint32_t local_count;
} ChimiFunction;

typedef struct {
  uint32_t type_name;
  uint32_t variant_name;
  uint32_t fields_offset;
  uint32_t fields_length;
  int32_t outcome_success;
  /* Position among the type's variants in declaration order; derived order follows it. */
  uint32_t ordinal;
} ChimiConstructor;

typedef struct {
  uint32_t name;
  uint32_t node;
} ChimiNamedNode;

typedef struct {
  uint32_t kind;
  uint32_t symbol;
  uint32_t node;
} ChimiStep;

/* When the arm matches, the cause of subject `cause_subject` is bound to the
   local symbol `cause_local` (both UINT32_MAX when the arm binds none). */
typedef struct {
  uint32_t patterns_offset;
  uint32_t patterns_length;
  uint32_t guard;
  uint32_t body;
  uint32_t cause_subject;
  uint32_t cause_local;
} ChimiArm;

typedef struct {
  uint32_t kind;
  uint32_t a;
  uint32_t offset;
  uint32_t length;
  int64_t integer;
} ChimiPattern;

extern const char *CHIMI_COMPUTATION;
extern const uint32_t CHIMI_REQUIRES_SCHEDULER;
extern const char *CHIMI_STRINGS[];
/* The byte length of each string, which a Text string may need: it can hold U+0000. */
extern const uint32_t CHIMI_STRING_LENGTHS[];
extern const uint32_t CHIMI_IDS[];
extern const ChimiOrigin CHIMI_ORIGINS[];
extern const ChimiNode CHIMI_NODES[];
extern const uint32_t CHIMI_NODE_COUNT;
extern const ChimiSymbol CHIMI_SYMBOLS[];
extern const uint32_t CHIMI_SYMBOL_COUNT;
extern const uint32_t CHIMI_REQUIREMENT_SLOT_COUNT;
/* The slot of the `clock` need's provider; see std/clock in runtime.c. */
extern const uint32_t CHIMI_CLOCK_REQUIREMENT_SLOT;
extern const ChimiFunction CHIMI_FUNCTIONS[];
extern const uint32_t CHIMI_FUNCTION_COUNT;
/* A test build's tests, as CHIMI_FUNCTIONS indexes in report order; `--test=<k>`
   runs the k-th instead of the entry. Empty for a program. */
extern const uint32_t CHIMI_TEST_FUNCTIONS[];
extern const uint32_t CHIMI_TEST_COUNT;
extern const ChimiConstructor CHIMI_CONSTRUCTORS[];
extern const ChimiNamedNode CHIMI_NAMED_NODES[];
extern const ChimiStep CHIMI_STEPS[];
extern const ChimiArm CHIMI_ARMS[];
extern const ChimiPattern CHIMI_PATTERNS[];
extern const ChimiConstantValue CHIMI_CONSTANT_VALUES[];
extern const uint32_t CHIMI_CONSTANT_VALUE_COUNT;
extern ChimiNodeFn CHIMI_NODE_FUNCTIONS[];

int32_t chimi_rt_main(ChimiNodeFn entry, int32_t argc, const char **argv);
int32_t chimi_rt_full_trace(const ChimiRuntime *);
ChimiControl *chimi_rt_direct_integer_success(ChimiRuntime *, uint32_t, int64_t);

ChimiControl *chimi_rt_constant(ChimiRuntime *, uint32_t, ChimiEnvironment *, uint64_t);
ChimiControl *chimi_rt_reference(ChimiRuntime *, uint32_t, ChimiEnvironment *, uint64_t);
ChimiControl *chimi_rt_select(ChimiRuntime *, uint32_t, ChimiEnvironment *, uint64_t);
ChimiControl *chimi_rt_dependency_method(ChimiRuntime *, uint32_t, ChimiEnvironment *, uint64_t);
ChimiControl *chimi_rt_construct(ChimiRuntime *, uint32_t, ChimiEnvironment *, uint64_t);
ChimiControl *chimi_rt_call(ChimiRuntime *, uint32_t, ChimiEnvironment *, uint64_t);
ChimiControl *chimi_rt_list(ChimiRuntime *, uint32_t, ChimiEnvironment *, uint64_t);
ChimiControl *chimi_rt_record(ChimiRuntime *, uint32_t, ChimiEnvironment *, uint64_t);
ChimiControl *chimi_rt_tuple(ChimiRuntime *, uint32_t, ChimiEnvironment *, uint64_t);
ChimiControl *chimi_rt_unary(ChimiRuntime *, uint32_t, ChimiEnvironment *, uint64_t);
ChimiControl *chimi_rt_binary(ChimiRuntime *, uint32_t, ChimiEnvironment *, uint64_t);
ChimiControl *chimi_rt_identity(ChimiRuntime *, uint32_t, ChimiEnvironment *, uint64_t);
ChimiControl *chimi_rt_provision(ChimiRuntime *, uint32_t, ChimiEnvironment *, uint64_t);
ChimiControl *chimi_rt_sequence(ChimiRuntime *, uint32_t, ChimiEnvironment *, uint64_t);
ChimiControl *chimi_rt_closure(ChimiRuntime *, uint32_t, ChimiEnvironment *, uint64_t);
ChimiControl *chimi_rt_match(ChimiRuntime *, uint32_t, ChimiEnvironment *, uint64_t);
/* Node layout: a = state symbol, b = initial node, c = body node. */
ChimiControl *chimi_rt_loop(ChimiRuntime *, uint32_t, ChimiEnvironment *, uint64_t);
/* Node layout: a = target loop state symbol, b = value node or UINT32_MAX for Void. */
ChimiControl *chimi_rt_break(ChimiRuntime *, uint32_t, ChimiEnvironment *, uint64_t);

/* One pass of a direct loop: the state in, the next state or the loop's value out. */
enum { CHIMI_DIRECT_PASS_CONTINUE = 0, CHIMI_DIRECT_PASS_BREAK = 1, CHIMI_DIRECT_PASS_FALLBACK = 2 };
typedef int32_t (*ChimiDirectPass)(int64_t, int64_t *);
/* A loop node whose passes the compiler lowered to `pass` (see runtime.c). */
ChimiControl *chimi_rt_direct_loop(ChimiRuntime *, uint32_t, ChimiEnvironment *, uint64_t,
                                   ChimiDirectPass);

#endif
