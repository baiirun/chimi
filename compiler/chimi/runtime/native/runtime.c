/* Chimi native runtime ABI version 1. */
#include "runtime.h"

#include <inttypes.h>
#include <limits.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

typedef enum { VALUE_INTEGER, VALUE_TEXT, VALUE_BOOLEAN, VALUE_LIST, VALUE_RECORD,
               VALUE_CONSTRUCTED, VALUE_CALLABLE } ValueKind;
typedef enum { CALL_DECLARED, CALL_CLOSURE, CALL_DEPENDENCY } CallableKind;
typedef enum { CONTROL_SUCCESS, CONTROL_FAILURE, CONTROL_DEFECT, CONTROL_CANCEL } ControlKind;

typedef struct ChimiValue ChimiValue;
typedef struct { const char *name; ChimiValue *value; } RuntimeField;

typedef struct ChimiCallable {
  CallableKind kind;
  uint32_t symbol;
  uint32_t body;
  uint32_t parameters_offset;
  uint32_t parameters_length;
  ChimiEnvironment *environment;
  struct ChimiCallable *inner;
  uint32_t requirement;
  uint32_t interface_symbol;
  uint32_t method;
  const char *name;
} ChimiCallable;

struct ChimiValue {
  ValueKind kind;
  union {
    int64_t integer;
    const char *text;
    bool boolean;
    struct { ChimiValue **items; uint32_t length; } list;
    struct { RuntimeField *fields; uint32_t length; } record;
    struct { uint32_t constructor; const char *type_name; const char *variant_name;
             RuntimeField *fields; uint32_t length; } constructed;
    ChimiCallable *callable;
  } as;
};

struct ChimiEnvironment {
  ChimiValue **locals;
  ChimiValue **requirements;
};

struct ChimiControl {
  ControlKind kind;
  ChimiValue *value;
  const char *message;
  int64_t node;
};

struct ChimiRuntime {
  const char *execution_id;
  bool full_trace;
  bool cancel;
  bool silent;
  uint64_t next_sequence;
  uint64_t next_span;
  bool *active_bindings;
};

static void *allocate(size_t count, size_t size) {
  void *value = calloc(count == 0 ? 1 : count, size);
  if (value == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); exit(70); }
  return value;
}

static ChimiValue *new_value(ValueKind kind) {
  ChimiValue *value = allocate(1, sizeof(*value)); value->kind = kind; return value;
}
static ChimiControl *new_control(ControlKind kind, ChimiValue *value, const char *message, int64_t node) {
  ChimiControl *control = allocate(1, sizeof(*control));
  control->kind = kind; control->value = value; control->message = message; control->node = node;
  return control;
}
static ChimiControl *success(ChimiValue *value) { return new_control(CONTROL_SUCCESS, value, NULL, -1); }
static ChimiControl *failure(ChimiValue *value) { return new_control(CONTROL_FAILURE, value, NULL, -1); }
static ChimiControl *defect(const char *message, int64_t node) { return new_control(CONTROL_DEFECT, NULL, message, node); }
static ChimiControl *cancelled(void) { return new_control(CONTROL_CANCEL, NULL, NULL, -1); }

static ChimiEnvironment *new_environment(void) {
  ChimiEnvironment *environment = allocate(1, sizeof(*environment));
  environment->locals = allocate(CHIMI_SYMBOL_COUNT, sizeof(*environment->locals));
  environment->requirements = allocate(CHIMI_SYMBOL_COUNT, sizeof(*environment->requirements));
  return environment;
}
static ChimiEnvironment *clone_environment(const ChimiEnvironment *source) {
  ChimiEnvironment *environment = new_environment();
  memcpy(environment->locals, source->locals, CHIMI_SYMBOL_COUNT * sizeof(*environment->locals));
  memcpy(environment->requirements, source->requirements,
         CHIMI_SYMBOL_COUNT * sizeof(*environment->requirements));
  return environment;
}

static const ChimiFunction *find_function(uint32_t symbol) {
  for (uint32_t index = 0; index < CHIMI_FUNCTION_COUNT; ++index)
    if (CHIMI_FUNCTIONS[index].symbol == symbol) return &CHIMI_FUNCTIONS[index];
  return NULL;
}

static ChimiControl *run_node(uint32_t node, ChimiRuntime *runtime,
                              ChimiEnvironment *environment, uint64_t parent_span) {
  if (runtime->cancel) return cancelled();
  return CHIMI_NODE_FUNCTIONS[node](runtime, environment, parent_span);
}

static const char *control_name(const ChimiControl *control) {
  switch (control->kind) {
    case CONTROL_SUCCESS: return "success";
    case CONTROL_FAILURE: return "expected_failure";
    case CONTROL_DEFECT: return "defect";
    case CONTROL_CANCEL: return "cancellation";
  }
  return "defect";
}

static void json_string(const char *value) {
  putchar('"');
  for (const unsigned char *cursor = (const unsigned char *)value; *cursor; ++cursor) {
    switch (*cursor) {
      case '"': fputs("\\\"", stdout); break;
      case '\\': fputs("\\\\", stdout); break;
      case '\n': fputs("\\n", stdout); break;
      case '\r': fputs("\\r", stdout); break;
      case '\t': fputs("\\t", stdout); break;
      default:
        if (*cursor < 0x20) printf("\\u%04x", *cursor); else putchar(*cursor);
    }
  }
  putchar('"');
}

static void json_value(const ChimiValue *value);
static void json_fields(const RuntimeField *fields, uint32_t length) {
  putchar('{');
  for (uint32_t index = 0; index < length; ++index) {
    if (index) putchar(','); json_string(fields[index].name); putchar(':'); json_value(fields[index].value);
  }
  putchar('}');
}
static void json_value(const ChimiValue *value) {
  switch (value->kind) {
    case VALUE_INTEGER: printf("%" PRId64, value->as.integer); break;
    case VALUE_TEXT: json_string(value->as.text); break;
    case VALUE_BOOLEAN: fputs(value->as.boolean ? "true" : "false", stdout); break;
    case VALUE_LIST:
      putchar('['); for (uint32_t i = 0; i < value->as.list.length; ++i) {
        if (i) putchar(','); json_value(value->as.list.items[i]);
      } putchar(']'); break;
    case VALUE_RECORD: json_fields(value->as.record.fields, value->as.record.length); break;
    case VALUE_CONSTRUCTED:
      fputs("{\"type\":", stdout); json_string(value->as.constructed.type_name);
      if (value->as.constructed.variant_name) {
        fputs(",\"variant\":", stdout); json_string(value->as.constructed.variant_name);
      }
      fputs(",\"fields\":", stdout);
      json_fields(value->as.constructed.fields, value->as.constructed.length); putchar('}'); break;
    case VALUE_CALLABLE:
      fputs("{\"callable\":", stdout); json_string(value->as.callable->name); putchar('}'); break;
  }
}

static void emit_event(ChimiRuntime *runtime, const char *event, uint32_t node_id,
                       uint64_t span, int64_t parent, const char *operation,
                       const ChimiControl *outcome) {
  const ChimiNode *node = &CHIMI_NODES[node_id];
  const ChimiOrigin *origin = &CHIMI_ORIGINS[node->origin];
  fputs("{\"event\":", stdout); json_string(event);
  printf(",\"sequence\":%" PRIu64 ",\"computation\":", runtime->next_sequence++);
  json_string(CHIMI_COMPUTATION); fputs(",\"execution_id\":", stdout); json_string(runtime->execution_id);
  printf(",\"span_id\":%" PRIu64 ",\"parent_span_id\":", span);
  if (parent < 0) fputs("null", stdout); else printf("%" PRId64, parent);
  printf(",\"core_node_id\":%u,\"origin_id\":%u,\"semantic_id\":%u,\"file_id\":%u,"
         "\"source_start\":%u,\"source_end\":%u,\"operation\":",
         node_id, node->origin, origin->semantic_id, origin->file_id, origin->start, origin->end);
  json_string(operation); fputs(",\"attributes\":{", stdout);
  if (outcome) { fputs("\"outcome\":", stdout); json_string(control_name(outcome)); }
  fputs("}}\n", stdout);
}

static void emit_dependency(ChimiRuntime *runtime, uint32_t node_id, uint64_t span,
                            uint64_t parent, const ChimiCallable *callable) {
  const ChimiNode *node = &CHIMI_NODES[node_id]; const ChimiOrigin *origin = &CHIMI_ORIGINS[node->origin];
  fputs("{\"event\":\"dependency_called\",\"sequence\":", stdout);
  printf("%" PRIu64 ",\"computation\":", runtime->next_sequence++); json_string(CHIMI_COMPUTATION);
  fputs(",\"execution_id\":", stdout); json_string(runtime->execution_id);
  printf(",\"span_id\":%" PRIu64 ",\"parent_span_id\":%" PRIu64
         ",\"core_node_id\":%u,\"origin_id\":%u,\"semantic_id\":%u,\"file_id\":%u,"
         "\"source_start\":%u,\"source_end\":%u,\"operation\":\"dependency_call\",\"attributes\":{",
         span, parent, node_id, node->origin, origin->semantic_id, origin->file_id, origin->start, origin->end);
  fputs("\"requirement\":", stdout); json_string(CHIMI_STRINGS[CHIMI_SYMBOLS[callable->requirement].name]);
  printf(",\"requirement_symbol_id\":%u,\"interface_symbol_id\":%u,\"method\":",
         callable->requirement, callable->interface_symbol); json_string(callable->name);
  printf(",\"method_semantic_id\":%u}}\n", callable->method);
}

static uint64_t begin_node(ChimiRuntime *runtime, uint32_t node, uint64_t parent, const char *operation) {
  if (runtime->silent) return parent;
  uint64_t span = runtime->next_span++;
  if (runtime->full_trace) emit_event(runtime, "operation_started", node, span, (int64_t)parent, operation, NULL);
  return span;
}
static ChimiControl *finish_node(ChimiRuntime *runtime, uint32_t node, uint64_t parent,
                                 uint64_t span, const char *operation, ChimiControl *control) {
  if (runtime->full_trace) emit_event(runtime, "operation_completed", node, span, (int64_t)parent, operation, control);
  return control;
}

static ChimiValue *integer_value(int64_t raw) { ChimiValue *v = new_value(VALUE_INTEGER); v->as.integer = raw; return v; }
static ChimiValue *text_value(const char *raw) { ChimiValue *v = new_value(VALUE_TEXT); v->as.text = raw; return v; }
static ChimiValue *boolean_value(bool raw) { ChimiValue *v = new_value(VALUE_BOOLEAN); v->as.boolean = raw; return v; }
static ChimiValue *callable_value(ChimiCallable *raw) { ChimiValue *v = new_value(VALUE_CALLABLE); v->as.callable = raw; return v; }

static ChimiValue *select_value(ChimiValue *value, const char *name) {
  RuntimeField *fields = NULL; uint32_t length = 0;
  if (value->kind == VALUE_RECORD) { fields = value->as.record.fields; length = value->as.record.length; }
  else if (value->kind == VALUE_CONSTRUCTED) { fields = value->as.constructed.fields; length = value->as.constructed.length; }
  for (uint32_t index = 0; index < length; ++index)
    if (strcmp(fields[index].name, name) == 0) return fields[index].value;
  return NULL;
}

static ChimiControl *invoke(ChimiRuntime *runtime, ChimiCallable *callable, ChimiValue **arguments,
                            uint32_t argument_count, uint64_t span, uint32_t call_node,
                            ChimiEnvironment *call_environment) {
  if (callable->kind == CALL_DEPENDENCY) {
    if (!runtime->silent) {
      uint64_t dependency_span = runtime->next_span++;
      emit_dependency(runtime, call_node, dependency_span, span, callable);
    }
    return invoke(runtime, callable->inner, arguments, argument_count, span, call_node, call_environment);
  }
  ChimiEnvironment *environment = clone_environment(callable->environment);
  for (uint32_t symbol = 0; symbol < CHIMI_SYMBOL_COUNT; ++symbol)
    if (call_environment->requirements[symbol] != NULL)
      environment->requirements[symbol] = call_environment->requirements[symbol];
  uint32_t count = callable->parameters_length < argument_count ? callable->parameters_length : argument_count;
  for (uint32_t index = 0; index < count; ++index)
    environment->locals[CHIMI_IDS[callable->parameters_offset + index]] = arguments[index];
  return run_node(callable->body, runtime, environment, span);
}

static ChimiControl *eval_reference(ChimiRuntime *runtime, uint32_t node_id,
                                    ChimiEnvironment *environment, uint64_t span) {
  uint32_t symbol = CHIMI_NODES[node_id].a; const ChimiSymbol *shape = &CHIMI_SYMBOLS[symbol];
  switch (shape->kind) {
    case CHIMI_SYMBOL_FUNCTION: {
      const ChimiFunction *function = find_function(symbol);
      ChimiCallable *callable = allocate(1, sizeof(*callable)); callable->kind = CALL_DECLARED;
      callable->symbol = symbol; callable->body = function->body;
      callable->parameters_offset = function->parameters_offset; callable->parameters_length = function->parameters_length;
      callable->environment = clone_environment(environment); callable->name = "function";
      return success(callable_value(callable));
    }
    case CHIMI_SYMBOL_BINDING: {
      if (runtime->active_bindings[symbol]) return defect("module binding is cyclic", -1);
      runtime->active_bindings[symbol] = true;
      ChimiControl *control = run_node(shape->binding, runtime, environment, span);
      runtime->active_bindings[symbol] = false; return control;
    }
    case CHIMI_SYMBOL_REQUIREMENT:
      if (environment->requirements[symbol]) return success(environment->requirements[symbol]);
      return defect("closed core has no provider", -1);
    case CHIMI_SYMBOL_LOCAL:
      if (environment->locals[symbol]) return success(environment->locals[symbol]);
      return defect("verified local is not bound", -1);
    default: return defect("symbol is not a runtime value", -1);
  }
}

#define NODE_WRAPPER(NAME, OPERATION, ...) \
  ChimiControl *chimi_rt_##NAME(ChimiRuntime *runtime, uint32_t node_id, ChimiEnvironment *incoming, uint64_t parent) { \
    uint64_t span = begin_node(runtime, node_id, parent, OPERATION); ChimiEnvironment *environment = clone_environment(incoming); \
    (void)environment; \
    ChimiControl *result = NULL; do { __VA_ARGS__ } while (0); return finish_node(runtime, node_id, parent, span, OPERATION, result); \
  }
#define RUN(CHILD) run_node((CHILD), runtime, environment, span)
#define REQUIRE_SUCCESS(CONTROL, TARGET) do { ChimiControl *_control = (CONTROL); \
  if (_control->kind != CONTROL_SUCCESS) { result = _control; break; } (TARGET) = _control->value; } while (0)

NODE_WRAPPER(constant, "constant", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiValue *value;
  if (node->a == CHIMI_LITERAL_INTEGER) value = integer_value(node->integer);
  else if (node->a == CHIMI_LITERAL_TEXT) value = text_value(CHIMI_STRINGS[node->b]);
  else value = boolean_value(node->integer != 0); result = success(value);
})
NODE_WRAPPER(reference, "reference", { result = eval_reference(runtime, node_id, environment, span); })
NODE_WRAPPER(select, "select", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiControl *control = RUN(node->a);
  if (control->kind != CONTROL_SUCCESS) { result = control; break; }
  ChimiValue *value = select_value(control->value, CHIMI_STRINGS[node->b]);
  result = value ? success(value) : defect("verified value has no field", node_id);
})
NODE_WRAPPER(dependency_method, "dependency_method", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiValue *provider = environment->requirements[node->a];
  if (!provider) { result = defect("closed core has no provider", node_id); break; }
  ChimiValue *method = select_value(provider, CHIMI_STRINGS[node->d]);
  if (!method || method->kind != VALUE_CALLABLE) { result = defect("provider has no callable field", node_id); break; }
  ChimiCallable *callable = allocate(1, sizeof(*callable)); callable->kind = CALL_DEPENDENCY;
  callable->inner = method->as.callable; callable->requirement = node->a; callable->interface_symbol = node->b;
  callable->method = node->c; callable->name = CHIMI_STRINGS[node->d]; result = success(callable_value(callable));
})

NODE_WRAPPER(construct, "construct", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; const ChimiSymbol *symbol = &CHIMI_SYMBOLS[node->a];
  const ChimiConstructor *shape = &CHIMI_CONSTRUCTORS[symbol->constructor];
  ChimiValue **values = allocate(node->length, sizeof(*values));
  bool stopped = false;
  for (uint32_t index = 0; index < node->length; ++index) {
    ChimiControl *control = RUN(CHIMI_NAMED_NODES[node->offset + index].node);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    values[index] = control->value;
  }
  if (stopped) break;
  if (shape->outcome_success >= 0) { result = shape->outcome_success ? success(values[0]) : failure(values[0]); break; }
  ChimiValue *value = new_value(VALUE_CONSTRUCTED); value->as.constructed.constructor = node->a;
  value->as.constructed.type_name = CHIMI_STRINGS[shape->type_name];
  value->as.constructed.variant_name = shape->variant_name == UINT32_MAX ? NULL : CHIMI_STRINGS[shape->variant_name];
  value->as.constructed.length = shape->fields_length;
  value->as.constructed.fields = allocate(shape->fields_length, sizeof(*value->as.constructed.fields));
  for (uint32_t field = 0; field < shape->fields_length; ++field) {
    const char *name = CHIMI_STRINGS[CHIMI_IDS[shape->fields_offset + field]]; uint32_t selected = field;
    for (uint32_t argument = 0; argument < node->length; ++argument) {
      uint32_t label = CHIMI_NAMED_NODES[node->offset + argument].name;
      if (label != UINT32_MAX && strcmp(CHIMI_STRINGS[label], name) == 0) { selected = argument; break; }
    }
    value->as.constructed.fields[field] = (RuntimeField){name, values[selected]};
  }
  result = success(value);
})

NODE_WRAPPER(call, "call", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiControl *callee = RUN(node->a);
  if (callee->kind != CONTROL_SUCCESS) { result = callee; break; }
  if (callee->value->kind != VALUE_CALLABLE) { result = defect("verified call target is not callable", node_id); break; }
  ChimiValue **arguments = allocate(node->length, sizeof(*arguments)); bool stopped = false;
  for (uint32_t index = 0; index < node->length; ++index) {
    ChimiControl *control = RUN(CHIMI_NAMED_NODES[node->offset + index].node);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    arguments[index] = control->value;
  }
  if (!stopped) result = invoke(runtime, callee->value->as.callable, arguments, node->length, span, node_id, environment);
})

NODE_WRAPPER(list, "list", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiValue *value = new_value(VALUE_LIST);
  value->as.list.length = node->length; value->as.list.items = allocate(node->length, sizeof(*value->as.list.items));
  bool stopped = false; for (uint32_t i = 0; i < node->length; ++i) {
    ChimiControl *control = RUN(CHIMI_IDS[node->offset + i]);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    value->as.list.items[i] = control->value;
  } if (!stopped) result = success(value);
})

NODE_WRAPPER(record, "record", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiValue *value = new_value(VALUE_RECORD);
  value->as.record.length = node->length; value->as.record.fields = allocate(node->length, sizeof(*value->as.record.fields));
  bool stopped = false; for (uint32_t i = 0; i < node->length; ++i) {
    const ChimiNamedNode *field = &CHIMI_NAMED_NODES[node->offset + i]; ChimiControl *control = RUN(field->node);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    value->as.record.fields[i] = (RuntimeField){CHIMI_STRINGS[field->name], control->value};
  } if (!stopped) result = success(value);
})

NODE_WRAPPER(unary, "unary", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiControl *control = RUN(node->a);
  if (control->kind != CONTROL_SUCCESS) { result = control; break; }
  if (control->value->kind != VALUE_INTEGER) { result = defect("verified unary operand is not an integer", node_id); break; }
  if (control->value->as.integer == INT64_MIN) { result = defect("integer negation overflow", node_id); break; }
  result = success(integer_value(-control->value->as.integer));
})

NODE_WRAPPER(binary, "binary", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiControl *left = RUN(node->b);
  if (left->kind != CONTROL_SUCCESS) { result = left; break; }
  ChimiControl *right = RUN(node->c); if (right->kind != CONTROL_SUCCESS) { result = right; break; }
  if (node->a == CHIMI_BINARY_PIPE) {
    if (right->value->kind != VALUE_CALLABLE) { result = defect("verified pipeline target is not callable", node_id); break; }
    ChimiValue *arguments[] = {left->value}; result = invoke(runtime, right->value->as.callable, arguments, 1, span, node_id, environment); break;
  }
  if (left->value->kind != VALUE_INTEGER || right->value->kind != VALUE_INTEGER) {
    result = defect("verified binary operands are not integers", node_id); break;
  }
  int64_t l = left->value->as.integer, r = right->value->as.integer, output = 0; bool overflow = false;
  switch (node->a) {
    case CHIMI_BINARY_ADD: overflow = __builtin_add_overflow(l, r, &output); break;
    case CHIMI_BINARY_SUBTRACT: overflow = __builtin_sub_overflow(l, r, &output); break;
    case CHIMI_BINARY_MULTIPLY: overflow = __builtin_mul_overflow(l, r, &output); break;
    case CHIMI_BINARY_DIVIDE:
      if (r == 0) { result = defect("integer division by zero", node_id); break; }
      if (l == INT64_MIN && r == -1) overflow = true; else output = l / r; break;
    case CHIMI_BINARY_LESS: result = success(boolean_value(l < r)); break;
    case CHIMI_BINARY_LESS_EQUAL: result = success(boolean_value(l <= r)); break;
    case CHIMI_BINARY_GREATER: result = success(boolean_value(l > r)); break;
    case CHIMI_BINARY_GREATER_EQUAL: result = success(boolean_value(l >= r)); break;
  }
  if (result) break;
  result = overflow ? defect("integer arithmetic overflow", node_id) : success(integer_value(output));
})

NODE_WRAPPER(identity, "identity", { result = RUN(CHIMI_NODES[node_id].a); })

NODE_WRAPPER(provision, "provision", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; bool stopped = false;
  for (uint32_t i = 0; i < node->length; ++i) {
    const ChimiNamedNode *provider = &CHIMI_NAMED_NODES[node->offset + i]; ChimiControl *control = RUN(provider->node);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    const char *name = CHIMI_STRINGS[provider->name];
    for (uint32_t symbol = 0; symbol < CHIMI_SYMBOL_COUNT; ++symbol)
      if (CHIMI_SYMBOLS[symbol].kind == CHIMI_SYMBOL_REQUIREMENT &&
          strcmp(CHIMI_STRINGS[CHIMI_SYMBOLS[symbol].name], name) == 0) environment->requirements[symbol] = control->value;
  } if (!stopped) result = RUN(node->a);
})

NODE_WRAPPER(sequence, "sequence", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiValue *last = NULL; bool stopped = false;
  for (uint32_t i = 0; i < node->length; ++i) {
    const ChimiStep *step = &CHIMI_STEPS[node->offset + i]; ChimiControl *control = RUN(step->node);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    last = control->value; if (step->kind == 0) environment->locals[step->symbol] = last;
  } if (!stopped) result = success(last);
})

NODE_WRAPPER(closure, "closure", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiCallable *callable = allocate(1, sizeof(*callable));
  callable->kind = CALL_CLOSURE; callable->body = node->a; callable->parameters_offset = node->offset;
  callable->parameters_length = node->length; callable->environment = clone_environment(environment);
  callable->name = "closure"; result = success(callable_value(callable));
})

typedef enum { MATCH_PLAIN, MATCH_SUCCESS, MATCH_FAILURE } MatchKind;
typedef struct { MatchKind kind; ChimiValue *value; } MatchValue;

static bool literal_matches(const ChimiPattern *pattern, const ChimiValue *value) {
  if (pattern->a == CHIMI_LITERAL_INTEGER) return value->kind == VALUE_INTEGER && value->as.integer == pattern->integer;
  if (pattern->a == CHIMI_LITERAL_TEXT) return value->kind == VALUE_TEXT && strcmp(value->as.text, CHIMI_STRINGS[pattern->offset]) == 0;
  return value->kind == VALUE_BOOLEAN && value->as.boolean == (pattern->integer != 0);
}
static bool match_plain(uint32_t pattern_id, ChimiValue *value, ChimiEnvironment *environment) {
  const ChimiPattern *pattern = &CHIMI_PATTERNS[pattern_id];
  switch (pattern->kind) {
    case CHIMI_PATTERN_WILDCARD: return true;
    case CHIMI_PATTERN_BINDING: environment->locals[pattern->a] = value; return true;
    case CHIMI_PATTERN_LITERAL: return literal_matches(pattern, value);
    case CHIMI_PATTERN_CONSTRUCTOR:
      if (value->kind != VALUE_CONSTRUCTED || value->as.constructed.constructor != pattern->a ||
          value->as.constructed.length != pattern->length) return false;
      for (uint32_t i = 0; i < pattern->length; ++i)
        if (!match_plain(CHIMI_IDS[pattern->offset + i], value->as.constructed.fields[i].value, environment)) return false;
      return true;
    case CHIMI_PATTERN_OUTCOME: return false;
  }
  return false;
}
static bool match_value(uint32_t pattern_id, MatchValue value, ChimiEnvironment *environment) {
  const ChimiPattern *pattern = &CHIMI_PATTERNS[pattern_id];
  if (pattern->kind == CHIMI_PATTERN_OUTCOME) {
    MatchKind expected = pattern->a ? MATCH_SUCCESS : MATCH_FAILURE;
    return value.kind == expected && match_plain(pattern->offset, value.value, environment);
  }
  return value.kind == MATCH_PLAIN && match_plain(pattern_id, value.value, environment);
}

NODE_WRAPPER(match, "match", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; MatchValue *values = allocate(node->length, sizeof(*values)); bool stopped = false;
  for (uint32_t subject = 0; subject < node->length; ++subject) {
    bool contextual = false;
    for (uint32_t arm_index = 0; arm_index < node->b; ++arm_index) {
      const ChimiArm *arm = &CHIMI_ARMS[node->a + arm_index];
      if (CHIMI_PATTERNS[CHIMI_IDS[arm->patterns_offset + subject]].kind == CHIMI_PATTERN_OUTCOME) contextual = true;
    }
    ChimiControl *control = RUN(CHIMI_IDS[node->offset + subject]);
    if (contextual && control->kind == CONTROL_SUCCESS) values[subject] = (MatchValue){MATCH_SUCCESS, control->value};
    else if (contextual && control->kind == CONTROL_FAILURE) values[subject] = (MatchValue){MATCH_FAILURE, control->value};
    else if (!contextual && control->kind == CONTROL_SUCCESS) values[subject] = (MatchValue){MATCH_PLAIN, control->value};
    else { result = control; stopped = true; break; }
  }
  if (stopped) break;
  for (uint32_t arm_index = 0; arm_index < node->b; ++arm_index) {
    const ChimiArm *arm = &CHIMI_ARMS[node->a + arm_index]; ChimiEnvironment *arm_environment = clone_environment(environment); bool matched = true;
    for (uint32_t subject = 0; subject < node->length; ++subject)
      if (!match_value(CHIMI_IDS[arm->patterns_offset + subject], values[subject], arm_environment)) { matched = false; break; }
    if (!matched) continue;
    if (arm->guard != UINT32_MAX) {
      ChimiControl *guard = run_node(arm->guard, runtime, arm_environment, span);
      if (guard->kind != CONTROL_SUCCESS) { result = guard; stopped = true; break; }
      if (guard->value->kind != VALUE_BOOLEAN) { result = defect("verified case guard is not Boolean", node_id); stopped = true; break; }
      if (!guard->value->as.boolean) continue;
    }
    result = run_node(arm->body, runtime, arm_environment, span); stopped = true; break;
  }
  if (!stopped) {
    for (uint32_t i = 0; i < node->length; ++i) if (values[i].kind == MATCH_FAILURE) { result = failure(values[i].value); stopped = true; break; }
    if (!stopped) result = defect("verified exhaustive match had no matching arm", node_id);
  }
})

static void print_outcome(const ChimiControl *control) {
  switch (control->kind) {
    case CONTROL_SUCCESS: fputs("{\"outcome\":\"success\",\"value\":", stdout); json_value(control->value); break;
    case CONTROL_FAILURE: fputs("{\"outcome\":\"expected_failure\",\"error\":", stdout); json_value(control->value); break;
    case CONTROL_DEFECT:
      fputs("{\"outcome\":\"defect\",\"message\":", stdout); json_string(control->message);
      if (control->node < 0) fputs(",\"core_node_id\":null,\"origin_id\":null", stdout);
      else printf(",\"core_node_id\":%" PRId64 ",\"origin_id\":%u", control->node, CHIMI_NODES[control->node].origin);
      break;
    case CONTROL_CANCEL:
      fputs("{\"outcome\":\"cancellation\",\"reason\":\"requested before the next semantic operation\"", stdout); break;
  }
  fputs("}\n", stdout);
}

int32_t chimi_rt_main(ChimiNodeFn entry, int32_t argc, const char **argv) {
  struct timespec identity_time;
  clock_gettime(CLOCK_REALTIME, &identity_time);
  char generated_execution_id[96];
  snprintf(generated_execution_id, sizeof(generated_execution_id),
           "execution-%ld-%" PRId64 "-%ld", (long)getpid(),
           (int64_t)identity_time.tv_sec, identity_time.tv_nsec);
  ChimiRuntime runtime = {.execution_id = generated_execution_id, .next_span = 2};
  uint32_t benchmark_repeats = 0;
  runtime.active_bindings = allocate(CHIMI_SYMBOL_COUNT, sizeof(*runtime.active_bindings));
  for (int32_t index = 1; index < argc; ++index) {
    if (strcmp(argv[index], "--trace=full") == 0) { runtime.full_trace = true; runtime.silent = false; }
    else if (strcmp(argv[index], "--trace=boundaries") == 0) { runtime.full_trace = false; runtime.silent = false; }
    else if (strcmp(argv[index], "--trace=off") == 0) { runtime.full_trace = false; runtime.silent = true; }
    else if (strncmp(argv[index], "--execution-id=", 15) == 0) runtime.execution_id = argv[index] + 15;
    else if (strcmp(argv[index], "--cancel") == 0) runtime.cancel = true;
    else if (strncmp(argv[index], "--benchmark-repeats=", 20) == 0)
      benchmark_repeats = (uint32_t)strtoul(argv[index] + 20, NULL, 10);
    else { fprintf(stderr, "unknown Chimi runtime argument: %s\n", argv[index]); return 64; }
  }
  uint32_t entry_node = 0;
  for (uint32_t i = 0; i < CHIMI_FUNCTION_COUNT; ++i)
    if (CHIMI_NODE_FUNCTIONS[CHIMI_FUNCTIONS[i].body] == entry) { entry_node = CHIMI_FUNCTIONS[i].body; break; }
  if (benchmark_repeats > 0) {
    struct timespec started, finished;
    clock_gettime(CLOCK_MONOTONIC, &started);
    for (uint32_t repeat = 0; repeat < benchmark_repeats; ++repeat) {
      runtime.next_sequence = 0;
      runtime.next_span = 2;
      if (!runtime.silent) emit_event(&runtime, "computation_started", entry_node, 1, -1, "computation", NULL);
      ChimiControl *control = runtime.cancel ? cancelled() : entry(&runtime, new_environment(), 1);
      if (control == NULL) return 70;
      if (!runtime.silent) emit_event(&runtime, "computation_completed", entry_node, 1, -1, "computation", control);
    }
    fflush(stdout);
    clock_gettime(CLOCK_MONOTONIC, &finished);
    int64_t elapsed = (int64_t)(finished.tv_sec - started.tv_sec) * INT64_C(1000000000)
                    + (int64_t)finished.tv_nsec - (int64_t)started.tv_nsec;
    fprintf(stderr, "%" PRIu64 "\n", elapsed / benchmark_repeats);
    return 0;
  }
  if (!runtime.silent) emit_event(&runtime, "computation_started", entry_node, 1, -1, "computation", NULL);
  ChimiControl *control = runtime.cancel ? cancelled() : entry(&runtime, new_environment(), 1);
  if (!runtime.silent) emit_event(&runtime, "computation_completed", entry_node, 1, -1, "computation", control);
  print_outcome(control);
  return 0;
}
