#ifndef CHIMI_NATIVE_AOT_RUNTIME_H
#define CHIMI_NATIVE_AOT_RUNTIME_H

#include <stdint.h>

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
  CHIMI_SYMBOL_OTHER,
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
extern const char *CHIMI_STRINGS[];
extern const uint32_t CHIMI_IDS[];
extern const ChimiOrigin CHIMI_ORIGINS[];
extern const ChimiNode CHIMI_NODES[];
extern const uint32_t CHIMI_NODE_COUNT;
extern const ChimiSymbol CHIMI_SYMBOLS[];
extern const uint32_t CHIMI_SYMBOL_COUNT;
extern const ChimiFunction CHIMI_FUNCTIONS[];
extern const uint32_t CHIMI_FUNCTION_COUNT;
extern const ChimiConstructor CHIMI_CONSTRUCTORS[];
extern const ChimiNamedNode CHIMI_NAMED_NODES[];
extern const ChimiStep CHIMI_STEPS[];
extern const ChimiArm CHIMI_ARMS[];
extern const ChimiPattern CHIMI_PATTERNS[];
extern ChimiNodeFn CHIMI_NODE_FUNCTIONS[];

int32_t chimi_rt_main(ChimiNodeFn entry, int32_t argc, const char **argv);

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

#endif
