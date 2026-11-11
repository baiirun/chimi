#ifndef CHIMI_NATIVE_RUNTIME_H
#define CHIMI_NATIVE_RUNTIME_H

#include <stdint.h>

#define CHIMI_RUNTIME_ABI_VERSION 9

typedef struct ChimiRuntime ChimiRuntime;
typedef struct ChimiEnvironment ChimiEnvironment;
typedef struct ChimiControl ChimiControl;
typedef ChimiControl *(*ChimiNodeFn)(ChimiRuntime *, ChimiEnvironment *, uint64_t);

enum {
  CHIMI_SYMBOL_FUNCTION,
  CHIMI_SYMBOL_BINDING,
  CHIMI_SYMBOL_REQUIREMENT,
  CHIMI_SYMBOL_LOCAL,
  CHIMI_SYMBOL_CONSTRUCTOR,
  CHIMI_SYMBOL_INTRINSIC,
  CHIMI_SYMBOL_OTHER,
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
  CHIMI_TYPE_NUMBER_ERROR,
  CHIMI_TYPE_OPTION,
  CHIMI_TYPE_CHANNEL_SENDER,
  CHIMI_TYPE_CHANNEL_RECEIVER,
  CHIMI_TYPE_CHANNEL_COMPLETION,
  CHIMI_TYPE_CHANNEL_SEND_ERROR,
  CHIMI_TYPE_CHANNEL_RECEIVE,
  CHIMI_TYPE_CHANNEL_ERROR,
  CHIMI_TYPE_TASK,
};

enum {
  CHIMI_INTRINSIC_IO_PATH,
  CHIMI_INTRINSIC_LIST_FIRST,
  CHIMI_INTRINSIC_LIST_LENGTH,
  CHIMI_INTRINSIC_MAP_EMPTY,
  CHIMI_INTRINSIC_MAP_GET,
  CHIMI_INTRINSIC_MAP_PUT,
  CHIMI_INTRINSIC_MAP_UPDATE,
  CHIMI_INTRINSIC_MAP_MAP_VALUES,
  CHIMI_INTRINSIC_JSON_DECODE,
  CHIMI_INTRINSIC_JSON_ENCODE,
  CHIMI_INTRINSIC_JSON_BOOLEAN,
  CHIMI_INTRINSIC_JSON_INT,
  CHIMI_INTRINSIC_JSON_TEXT,
  CHIMI_INTRINSIC_JSON_ARRAY,
  CHIMI_INTRINSIC_JSON_OBJECT,
  CHIMI_INTRINSIC_JSON_NUMBER_TO_INT,
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
  CHIMI_CONSTRUCTOR_NUMBER_NOT_INTEGER,
  CHIMI_CONSTRUCTOR_NUMBER_OUT_OF_RANGE,
  CHIMI_CONSTRUCTOR_CHANNEL_SEND_CLOSED,
  CHIMI_CONSTRUCTOR_CHANNEL_SEND_ABORTED,
  CHIMI_CONSTRUCTOR_CHANNEL_RECEIVED,
  CHIMI_CONSTRUCTOR_CHANNEL_DRAINED,
  CHIMI_CONSTRUCTOR_CHANNEL_ABORTED,
};

enum {
  CHIMI_LITERAL_INTEGER,
  CHIMI_LITERAL_TEXT,
  CHIMI_LITERAL_BOOLEAN,
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
  CHIMI_BINARY_PIPE,
};

enum {
  CHIMI_PATTERN_WILDCARD,
  CHIMI_PATTERN_BINDING,
  CHIMI_PATTERN_LITERAL,
  CHIMI_PATTERN_CONSTRUCTOR,
  CHIMI_PATTERN_OUTCOME,
};

typedef struct {
  uint32_t file_id;
  uint32_t semantic_id;
  uint32_t start;
  uint32_t end;
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
  uint32_t binding;
  uint32_t constructor;
  uint32_t intrinsic;
  uint32_t standard_constructor;
  uint32_t standard_type;
  uint32_t environment_slot;
} ChimiSymbol;

typedef struct {
  uint32_t symbol;
  uint32_t body;
  uint32_t parameters_offset;
  uint32_t parameters_length;
} ChimiFunction;

typedef struct {
  uint32_t type_name;
  uint32_t variant_name;
  uint32_t fields_offset;
  uint32_t fields_length;
  int32_t outcome_success;
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

typedef struct {
  uint32_t patterns_offset;
  uint32_t patterns_length;
  uint32_t guard;
  uint32_t body;
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
extern const uint32_t CHIMI_IDS[];
extern const ChimiOrigin CHIMI_ORIGINS[];
extern const ChimiNode CHIMI_NODES[];
extern const uint32_t CHIMI_NODE_COUNT;
extern const ChimiSymbol CHIMI_SYMBOLS[];
extern const uint32_t CHIMI_SYMBOL_COUNT;
extern const uint32_t CHIMI_LOCAL_SLOT_COUNT;
extern const uint32_t CHIMI_REQUIREMENT_SLOT_COUNT;
extern const ChimiFunction CHIMI_FUNCTIONS[];
extern const uint32_t CHIMI_FUNCTION_COUNT;
extern const ChimiConstructor CHIMI_CONSTRUCTORS[];
extern const ChimiNamedNode CHIMI_NAMED_NODES[];
extern const ChimiStep CHIMI_STEPS[];
extern const ChimiArm CHIMI_ARMS[];
extern const ChimiPattern CHIMI_PATTERNS[];
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

#endif
