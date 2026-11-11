/* Chimi native runtime ABI version 11. */
#include "runtime.h"
#include "chimi_fiber.h"

#include <inttypes.h>
#include <math.h>
#include <stdarg.h>
#include <errno.h>
#include <limits.h>
#include <stddef.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

// Ordinary binaries keep the native runtime but intentionally omit the fiber
// archive. Concurrency builds force-load the archive, whose strong definitions
// replace these unreachable fallbacks.
__attribute__((weak)) ChimiFiberCommand chimi_fiber_request(
  void *request, const ChimiRootFrame *roots) {
  (void)request;
  (void)roots;
  abort();
}

__attribute__((weak)) ChimiSchedulerOutcome chimi_scheduler_execute(
  ChimiFiberEntry entry, void *context, size_t stack_bytes, uint32_t trace_mode,
  ChimiSchedulerTraceSink trace_sink, void *trace_context) {
  (void)entry;
  (void)context;
  (void)stack_bytes;
  (void)trace_mode;
  (void)trace_sink;
  (void)trace_context;
  return (ChimiSchedulerOutcome){.state = CHIMI_FIBER_DEFECTED};
}

typedef enum { VALUE_VOID, VALUE_INTEGER, VALUE_FLOAT, VALUE_TEXT, VALUE_BOOLEAN, VALUE_LIST, VALUE_RECORD,
               VALUE_CONSTRUCTED, VALUE_CALLABLE, VALUE_MAP, VALUE_JSON_NUMBER, VALUE_TASK,
               VALUE_CHANNEL_SENDER, VALUE_CHANNEL_RECEIVER,
               VALUE_CHANNEL_COMPLETION } ValueKind;
/* CALL_CONSTRUCTOR is a constructor with fields named as a value; calling it constructs. */
typedef enum { CALL_DECLARED, CALL_CLOSURE, CALL_DEPENDENCY, CALL_HOST, CALL_INTRINSIC,
               CALL_CONSTRUCTOR } CallableKind;
typedef enum { HOST_PROCESS_UTF8_LINES, HOST_WRITE_LINE, HOST_WRITE_ERROR } HostFunction;
/* CONTROL_BREAK carries the target loop's state symbol in `node` and its value in `value`.
   Verified programs keep it inside one function, so it never reaches a call or root. */
typedef enum { CONTROL_SUCCESS, CONTROL_FAILURE, CONTROL_DEFECT, CONTROL_CANCEL, CONTROL_BREAK } ControlKind;

typedef struct ChimiValue ChimiValue;
typedef struct ChimiTaskContext ChimiTaskContext;
typedef struct { const char *name; ChimiValue *value; } RuntimeField;
typedef struct { uint64_t channel; uint64_t owner; uint64_t token; } RuntimeCapability;
typedef struct MapNode MapNode;
typedef struct ListNode ListNode;

/* One node of a persistent vector: a 32-way radix trie whose leaves (height 0)
   hold values and whose branches hold child nodes. Trie leaves are always full;
   a list's tail leaf holds its last 1..32 values. Nodes are immutable once a
   list value refers to them, so every version shares all untouched nodes. */
struct ListNode {
  uint32_t height;
  uint32_t count;
  void *slots[];
};

/* A persistent vector of `length` values. `root` is NULL until it outgrows one
   leaf; `tail` is NULL only when empty. */
typedef struct { ListNode *root; ListNode *tail; uint32_t length; } ListVector;

/* Keys are any equatable value, kept in the internal structural order. */
struct MapNode {
  ChimiValue *key;
  ChimiValue *value;
  MapNode *left;
  MapNode *right;
  uint16_t height;
};

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
  uint32_t intrinsic;
  HostFunction host;
} ChimiCallable;

struct ChimiValue {
  ValueKind kind;
  union {
    int64_t integer;
    double floating; /* always finite */
    const char *text;
    bool boolean;
    /* A List is the window [start, start + length) of a shared vector, so
       slicing is O(1) amortized; see list_slice for when a window copies. */
    struct { ListVector vector; uint32_t start; uint32_t length; } list;
    struct { RuntimeField *fields; uint32_t length; } record;
    struct { uint32_t constructor; const char *type_name; const char *variant_name;
             RuntimeField *fields; uint32_t length; } constructed;
    ChimiCallable *callable;
    MapNode *map;
    const char *json_number;
    uint64_t task;
    RuntimeCapability capability;
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

struct ChimiTaskContext {
  ChimiRuntime *runtime;
  ChimiEnvironment *environment;
  ChimiCallable *callable;
  ChimiNodeFn entry;
  uint64_t span;
};

typedef enum {
  ALLOCATION_BYTES,
  ALLOCATION_VALUE,
  ALLOCATION_CONTROL,
  ALLOCATION_ENVIRONMENT,
  ALLOCATION_VALUE_POINTERS,
  ALLOCATION_FIELDS,
  ALLOCATION_CALLABLE,
  ALLOCATION_MAP_NODE,
  ALLOCATION_LIST_NODE,
  ALLOCATION_KIND_COUNT,
} AllocationKind;

struct ChimiRuntime {
  const char *execution_id;
  bool full_trace;
  bool cancel;
  bool silent;
  FILE *telemetry;
  FILE *outcome;
  uint32_t max_line_bytes;
  uint64_t next_sequence;
  uint64_t next_span;
  size_t scoped_total_allocated_bytes;
  size_t scoped_allocated_bytes_by_kind[ALLOCATION_KIND_COUNT];
  size_t scoped_peak_live_bytes;
  size_t scoped_retained_bytes;
  uint64_t scoped_collections;
};

typedef struct AllocationHeader AllocationHeader;
typedef struct AllocationScope AllocationScope;
struct AllocationHeader {
  AllocationHeader *next;
  size_t bytes;
  uint64_t mark_epoch;
  AllocationKind kind;
  /* The scope whose list holds this allocation; NULL outside every scope. */
  AllocationScope *scope;
};

struct AllocationScope {
  AllocationScope *parent;
  AllocationHeader *allocations;
  size_t live_bytes;
  size_t peak_live_bytes;
  size_t total_allocated_bytes;
  size_t allocated_bytes_by_kind[ALLOCATION_KIND_COUNT];
  uint64_t collections;
};

static AllocationScope *current_allocation_scope;
/* The scope allocation_scope_collect is marking. Marking stops at allocations outside
   it: they were made before the scope began and values never change, so they cannot
   reach the scope's allocations, and traversing them would make every collection cost
   the size of long-lived data such as a large list a loop drains. */
static AllocationScope *collecting_scope;
static uint64_t next_mark_epoch = 1;

static void *allocate(size_t count, size_t size) {
  if (count == 0) count = 1;
  if (size != 0 && count > SIZE_MAX / size) {
    fputs("Chimi runtime allocation overflow\n", stderr); exit(70);
  }
  size_t bytes = count * size;
  if (bytes > SIZE_MAX - sizeof(AllocationHeader)) {
    fputs("Chimi runtime allocation overflow\n", stderr); exit(70);
  }
  AllocationHeader *header = calloc(1, sizeof(*header) + bytes);
  if (header == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); exit(70); }
  header->bytes = bytes;
  header->scope = current_allocation_scope;
  if (current_allocation_scope) {
    header->next = current_allocation_scope->allocations;
    current_allocation_scope->allocations = header;
    current_allocation_scope->live_bytes += bytes;
    current_allocation_scope->total_allocated_bytes += bytes;
    current_allocation_scope->allocated_bytes_by_kind[ALLOCATION_BYTES] += bytes;
    if (current_allocation_scope->live_bytes > current_allocation_scope->peak_live_bytes)
      current_allocation_scope->peak_live_bytes = current_allocation_scope->live_bytes;
  }
  return header + 1;
}

static AllocationHeader *allocation_header(const void *value) {
  return value ? (AllocationHeader *)value - 1 : NULL;
}

static void allocation_kind(void *value, AllocationKind kind) {
  AllocationHeader *header = allocation_header(value);
  if (current_allocation_scope && header->kind != kind) {
    current_allocation_scope->allocated_bytes_by_kind[header->kind] -= header->bytes;
    current_allocation_scope->allocated_bytes_by_kind[kind] += header->bytes;
  }
  header->kind = kind;
}

static void *reallocate_bytes(void *value, size_t old_bytes, size_t new_bytes) {
  void *replacement = allocate(new_bytes, 1);
  if (value) memcpy(replacement, value, old_bytes < new_bytes ? old_bytes : new_bytes);
  return replacement;
}

static ChimiValue *new_value(ValueKind kind) {
  ChimiValue *value = allocate(1, sizeof(*value));
  allocation_kind(value, ALLOCATION_VALUE); value->kind = kind; return value;
}
static ChimiControl *new_control(ControlKind kind, ChimiValue *value, const char *message, int64_t node) {
  ChimiControl *control = allocate(1, sizeof(*control));
  allocation_kind(control, ALLOCATION_CONTROL);
  control->kind = kind; control->value = value; control->message = message; control->node = node;
  return control;
}
static ChimiControl *success(ChimiValue *value) { return new_control(CONTROL_SUCCESS, value, NULL, -1); }
static ChimiControl *failure(ChimiValue *value) { return new_control(CONTROL_FAILURE, value, NULL, -1); }
static ChimiControl *defect(const char *message, int64_t node) { return new_control(CONTROL_DEFECT, NULL, message, node); }
static ChimiControl *cancelled(void) { return new_control(CONTROL_CANCEL, NULL, NULL, -1); }

static ChimiValue **new_value_slots(uint32_t count) {
  if (count == 0) return NULL;
  ChimiValue **slots = allocate(count, sizeof(*slots));
  allocation_kind(slots, ALLOCATION_VALUE_POINTERS);
  return slots;
}
static ChimiValue **clone_value_slots(ChimiValue **source, uint32_t count) {
  ChimiValue **slots = new_value_slots(count);
  if (count != 0) memcpy(slots, source, count * sizeof(*slots));
  return slots;
}
static ChimiEnvironment *new_environment(void) {
  ChimiEnvironment *environment = allocate(1, sizeof(*environment));
  allocation_kind(environment, ALLOCATION_ENVIRONMENT);
  environment->locals = new_value_slots(CHIMI_LOCAL_SLOT_COUNT);
  environment->requirements = new_value_slots(CHIMI_REQUIREMENT_SLOT_COUNT);
  return environment;
}
static ChimiEnvironment *clone_environment(const ChimiEnvironment *source) {
  ChimiEnvironment *environment = allocate(1, sizeof(*environment));
  allocation_kind(environment, ALLOCATION_ENVIRONMENT);
  environment->locals = clone_value_slots(source->locals, CHIMI_LOCAL_SLOT_COUNT);
  environment->requirements = clone_value_slots(source->requirements, CHIMI_REQUIREMENT_SLOT_COUNT);
  return environment;
}
// Locals and requirements are independently immutable outside the boundaries
// that bind them. Copy only the table a boundary mutates; precise marking keeps
// the other table alive through the shared environment pointer.
static ChimiEnvironment *clone_local_environment(const ChimiEnvironment *source) {
  ChimiEnvironment *environment = allocate(1, sizeof(*environment));
  allocation_kind(environment, ALLOCATION_ENVIRONMENT);
  environment->locals = clone_value_slots(source->locals, CHIMI_LOCAL_SLOT_COUNT);
  environment->requirements = source->requirements;
  return environment;
}
static ChimiEnvironment *clone_requirement_environment(const ChimiEnvironment *source) {
  ChimiEnvironment *environment = allocate(1, sizeof(*environment));
  allocation_kind(environment, ALLOCATION_ENVIRONMENT);
  environment->locals = source->locals;
  environment->requirements = clone_value_slots(source->requirements, CHIMI_REQUIREMENT_SLOT_COUNT);
  return environment;
}
static ChimiEnvironment *capture_requirements(const ChimiEnvironment *source) {
  ChimiEnvironment *environment = allocate(1, sizeof(*environment));
  allocation_kind(environment, ALLOCATION_ENVIRONMENT);
  environment->locals = NULL;
  environment->requirements = clone_value_slots(source->requirements, CHIMI_REQUIREMENT_SLOT_COUNT);
  return environment;
}
static ChimiEnvironment *capture_closure_environment(const ChimiEnvironment *source,
                                                     const ChimiNode *node) {
  ChimiEnvironment *environment = allocate(1, sizeof(*environment));
  allocation_kind(environment, ALLOCATION_ENVIRONMENT);
  environment->locals = new_value_slots(CHIMI_LOCAL_SLOT_COUNT);
  environment->requirements = clone_value_slots(source->requirements, CHIMI_REQUIREMENT_SLOT_COUNT);
  for (uint32_t index = 0; index < node->c; ++index) {
    uint32_t symbol = CHIMI_IDS[node->b + index];
    uint32_t slot = CHIMI_SYMBOLS[symbol].environment_slot;
    environment->locals[slot] = source->locals[slot];
  }
  return environment;
}
static uint32_t environment_slot(uint32_t symbol) {
  return CHIMI_SYMBOLS[symbol].environment_slot;
}

enum { LIST_BITS = 5, LIST_WIDTH = 1 << LIST_BITS, LIST_MASK = LIST_WIDTH - 1 };

static ListNode *list_node(uint32_t height, uint32_t count) {
  ListNode *node = allocate(1, sizeof(*node) + (size_t)count * sizeof(node->slots[0]));
  allocation_kind(node, ALLOCATION_LIST_NODE);
  node->height = height; node->count = count;
  return node;
}
static ListNode *list_node_copy(const ListNode *source, uint32_t count) {
  ListNode *node = list_node(source->height, count);
  uint32_t kept = source->count < count ? source->count : count;
  memcpy(node->slots, source->slots, kept * sizeof(node->slots[0]));
  return node;
}
/* Index of the first element stored in the tail rather than the trie. */
static uint32_t vector_tail_offset(uint32_t length) {
  return length == 0 ? 0 : ((length - 1) >> LIST_BITS) << LIST_BITS;
}
/* The leaf holding element `index`; the caller reads slot `index & LIST_MASK`. */
static ListNode *vector_leaf(ListVector vector, uint32_t index) {
  if (index >= vector_tail_offset(vector.length)) return vector.tail;
  ListNode *node = vector.root;
  for (uint32_t height = node->height; height > 0; --height)
    node = node->slots[(index >> (height * LIST_BITS)) & LIST_MASK];
  return node;
}
static ListNode *vector_new_path(uint32_t height, ListNode *leaf) {
  if (height == 0) return leaf;
  ListNode *node = list_node(height, 1);
  node->slots[0] = vector_new_path(height - 1, leaf);
  return node;
}
/* Copies the path to the leaf that starts at element `first_index` and attaches `leaf`. */
static ListNode *vector_push_leaf(const ListNode *node, uint32_t first_index, ListNode *leaf) {
  uint32_t slot = (first_index >> (node->height * LIST_BITS)) & LIST_MASK;
  ListNode *copy = list_node_copy(node, slot + 1);
  if (node->height == 1) copy->slots[slot] = leaf;
  else if (slot < node->count) copy->slots[slot] = vector_push_leaf(node->slots[slot], first_index, leaf);
  else copy->slots[slot] = vector_new_path(node->height - 1, leaf);
  return copy;
}
/* Amortized O(1): copies at most one 32-slot tail plus one root-to-leaf path. */
static ListVector vector_push(ListVector vector, ChimiValue *item) {
  if (vector.length == UINT32_MAX) { fputs("Chimi runtime list length overflow\n", stderr); exit(70); }
  uint32_t tail_length = vector.length - vector_tail_offset(vector.length);
  if (vector.length == 0 || tail_length < LIST_WIDTH) {
    ListNode *next = vector.length == 0 ? list_node(0, 1) : list_node_copy(vector.tail, tail_length + 1);
    next->slots[tail_length] = item;
    return (ListVector){vector.root, next, vector.length + 1};
  }
  ListNode *root = vector.root;
  uint32_t first_index = vector.length - LIST_WIDTH;
  if (!root) {
    root = vector_new_path(1, vector.tail);
  } else if ((uint64_t)(first_index >> LIST_BITS) == (uint64_t)1 << (root->height * LIST_BITS)) {
    ListNode *grown = list_node(root->height + 1, 2);
    grown->slots[0] = root; grown->slots[1] = vector_new_path(root->height, vector.tail);
    root = grown;
  } else {
    root = vector_push_leaf(root, first_index, vector.tail);
  }
  ListNode *next = list_node(0, 1);
  next->slots[0] = item;
  return (ListVector){root, next, vector.length + 1};
}
static ListNode *vector_set_node(const ListNode *node, uint32_t index, ChimiValue *item) {
  ListNode *copy = list_node_copy(node, node->count);
  if (node->height == 0) copy->slots[index & LIST_MASK] = item;
  else {
    uint32_t slot = (index >> (node->height * LIST_BITS)) & LIST_MASK;
    copy->slots[slot] = vector_set_node(node->slots[slot], index, item);
  }
  return copy;
}
/* O(log32 n): copies the path to element `index`, which must exist. */
static ListVector vector_set(ListVector vector, uint32_t index, ChimiValue *item) {
  if (index >= vector_tail_offset(vector.length))
    return (ListVector){vector.root, vector_set_node(vector.tail, index, item), vector.length};
  return (ListVector){vector_set_node(vector.root, index, item), vector.tail, vector.length};
}
/* Builds a vector from `length` values in O(length) without intermediate versions. */
static ListVector vector_from_values(ChimiValue *const *values, uint32_t length) {
  if (length == 0) return (ListVector){NULL, NULL, 0};
  uint32_t tail_offset = vector_tail_offset(length);
  ListNode *tail = list_node(0, length - tail_offset);
  memcpy(tail->slots, values + tail_offset, tail->count * sizeof(tail->slots[0]));
  uint32_t level_count = tail_offset >> LIST_BITS;
  if (level_count == 0) return (ListVector){NULL, tail, length};
  ListNode **level = malloc(level_count * sizeof(*level));
  if (!level) { fputs("Chimi runtime exhausted memory\n", stderr); exit(70); }
  for (uint32_t index = 0; index < level_count; ++index) {
    level[index] = list_node(0, LIST_WIDTH);
    memcpy(level[index]->slots, values + ((size_t)index << LIST_BITS), sizeof(level[index]->slots[0]) * LIST_WIDTH);
  }
  uint32_t height = 0;
  while (level_count > 1 || height == 0) {
    ++height;
    uint32_t parent_count = (level_count + LIST_MASK) >> LIST_BITS;
    for (uint32_t parent = 0; parent < parent_count; ++parent) {
      uint32_t first = parent << LIST_BITS;
      uint32_t count = level_count - first < LIST_WIDTH ? level_count - first : LIST_WIDTH;
      ListNode *node = list_node(height, count);
      memcpy(node->slots, level + first, count * sizeof(node->slots[0]));
      level[parent] = node;
    }
    level_count = parent_count;
  }
  ListNode *root = level[0];
  free(level);
  return (ListVector){root, tail, length};
}

static ChimiValue *list_value(ListVector vector, uint32_t start, uint32_t length) {
  ChimiValue *value = new_value(VALUE_LIST);
  value->as.list.vector = vector; value->as.list.start = start; value->as.list.length = length;
  return value;
}
static ChimiValue *list_from_values(ChimiValue *const *values, uint32_t length) {
  return list_value(vector_from_values(values, length), 0, length);
}
static uint32_t list_length(const ChimiValue *list) { return list->as.list.length; }
static ChimiValue *list_get(const ChimiValue *list, uint32_t index) {
  if (index >= list->as.list.length) return NULL;
  index += list->as.list.start;
  return vector_leaf(list->as.list.vector, index)->slots[index & LIST_MASK];
}
/* list_push is reached from Chimi once the standard-library mechanism (G1)
   exposes it; until then only tests call it. */

/* Returns a new list with `item` appended; `list` is unchanged. A window that
   ends before its vector does writes slot `end` into a path copy instead, so
   the shared vector every other window sees is untouched. */
__attribute__((unused)) static ChimiValue *list_push(const ChimiValue *list, ChimiValue *item) {
  ListVector vector = list->as.list.vector;
  uint32_t end = list->as.list.start + list->as.list.length;
  vector = end == vector.length ? vector_push(vector, item) : vector_set(vector, end, item);
  return list_value(vector, list->as.list.start, list->as.list.length + 1);
}
/* The innermost scope among a vector's root and tail. Live scopes form a stack, so a
   later allocation sits in the same scope as an earlier one or in a nested one; this
   scope therefore holds the vector's youngest node, and its elements are no younger.
   NULL (outside every scope) encloses every scope. */
static AllocationScope *vector_scope(ListVector vector) {
  AllocationScope *root = vector.root ? allocation_header(vector.root)->scope : NULL;
  AllocationScope *tail = vector.tail ? allocation_header(vector.tail)->scope : NULL;
  if (!root) return tail;
  for (AllocationScope *scope = tail; scope; scope = scope->parent)
    if (scope == root) return tail;
  return root;
}
/* A window shorter than 1/LIST_RETAINED_FRACTION of its vector is copied. */
enum { LIST_RETAINED_FRACTION = 4 };
/* Window [from, to) of `list`, or NULL when the range is reversed or past the end.
   The window shares the vector in O(1) unless it is shorter than a quarter of it;
   then it copies its elements into a fresh vector so a small window does not keep a
   large vector alive. Draining n elements one at a time copies fewer than n/3 in
   total (n/4 + n/16 + ...), because each copy of m elements follows at least 3m
   dropped ones. The interpreter's PersistentList::slice applies the same rule. */
static ChimiValue *list_slice(const ChimiValue *list, uint32_t from, uint32_t to) {
  if (from > to || to > list->as.list.length) return NULL;
  if (from == to) return list_value((ListVector){NULL, NULL, 0}, 0, 0);
  uint32_t length = to - from;
  if ((uint64_t)length * LIST_RETAINED_FRACTION < list->as.list.vector.length) {
    ChimiValue **values = malloc((size_t)length * sizeof(*values));
    if (!values) { fputs("Chimi runtime exhausted memory\n", stderr); exit(70); }
    for (uint32_t index = 0; index < length; ++index) values[index] = list_get(list, from + index);
    /* The copy joins the scope of the vector it replaces. Its elements are no younger
       than that vector, so the copy breaks no scope invariant, and a loop draining a
       list built before the loop does not re-mark each copy on every pass. */
    AllocationScope *current = current_allocation_scope;
    current_allocation_scope = vector_scope(list->as.list.vector);
    ChimiValue *copy = list_from_values(values, length);
    current_allocation_scope = current;
    free(values);
    return copy;
  }
  return list_value(list->as.list.vector, list->as.list.start + from, length);
}

static void mark_value(ChimiValue *value, uint64_t epoch);

static bool mark_allocation(void *value, uint64_t epoch, AllocationKind expected_kind) {
  if (!value) return false;
  AllocationHeader *header = allocation_header(value);
  if (header->kind != expected_kind) {
    fputs("Chimi runtime allocation graph is inconsistent\n", stderr); exit(70);
  }
  if (header->scope != collecting_scope || header->mark_epoch == epoch) return false;
  header->mark_epoch = epoch;
  return true;
}

static void mark_value_pointers(ChimiValue **values, uint64_t epoch) {
  if (!values || !mark_allocation(values, epoch, ALLOCATION_VALUE_POINTERS)) return;
  AllocationHeader *header = allocation_header(values);
  size_t length = header->bytes / sizeof(*values);
  for (size_t index = 0; index < length; ++index) mark_value(values[index], epoch);
}

static void mark_fields(RuntimeField *fields, uint64_t epoch) {
  if (!fields || !mark_allocation(fields, epoch, ALLOCATION_FIELDS)) return;
  AllocationHeader *header = allocation_header(fields);
  size_t length = header->bytes / sizeof(*fields);
  for (size_t index = 0; index < length; ++index) mark_value(fields[index].value, epoch);
}

static void mark_environment(ChimiEnvironment *environment, uint64_t epoch) {
  if (!environment || !mark_allocation(environment, epoch, ALLOCATION_ENVIRONMENT)) return;
  mark_value_pointers(environment->locals, epoch);
  mark_value_pointers(environment->requirements, epoch);
}

static void mark_callable(ChimiCallable *callable, uint64_t epoch) {
  if (!callable || !mark_allocation(callable, epoch, ALLOCATION_CALLABLE)) return;
  mark_environment(callable->environment, epoch);
  mark_callable(callable->inner, epoch);
}

static void mark_list_node(ListNode *node, uint64_t epoch) {
  if (!node || !mark_allocation(node, epoch, ALLOCATION_LIST_NODE)) return;
  for (uint32_t index = 0; index < node->count; ++index) {
    if (node->height == 0) mark_value(node->slots[index], epoch);
    else mark_list_node(node->slots[index], epoch);
  }
}

static void mark_map(MapNode *node, uint64_t epoch) {
  if (!node || !mark_allocation(node, epoch, ALLOCATION_MAP_NODE)) return;
  mark_value(node->key, epoch);
  mark_value(node->value, epoch);
  mark_map(node->left, epoch);
  mark_map(node->right, epoch);
}

static void mark_value(ChimiValue *value, uint64_t epoch) {
  if (!value || !mark_allocation(value, epoch, ALLOCATION_VALUE)) return;
  switch (value->kind) {
    case VALUE_TEXT: mark_allocation((void *)value->as.text, epoch, ALLOCATION_BYTES); break;
    case VALUE_LIST:
      mark_list_node(value->as.list.vector.root, epoch);
      mark_list_node(value->as.list.vector.tail, epoch);
      break;
    case VALUE_RECORD: mark_fields(value->as.record.fields, epoch); break;
    case VALUE_CONSTRUCTED: mark_fields(value->as.constructed.fields, epoch); break;
    case VALUE_CALLABLE: mark_callable(value->as.callable, epoch); break;
    case VALUE_MAP: mark_map(value->as.map, epoch); break;
    case VALUE_JSON_NUMBER:
      mark_allocation((void *)value->as.json_number, epoch, ALLOCATION_BYTES); break;
    case VALUE_TASK:
    case VALUE_CHANNEL_SENDER:
    case VALUE_CHANNEL_RECEIVER:
    case VALUE_CHANNEL_COMPLETION: break;
    case VALUE_VOID:
    case VALUE_INTEGER:
    case VALUE_FLOAT:
    case VALUE_BOOLEAN: break;
  }
}

static void mark_control(ChimiControl *control, uint64_t epoch) {
  if (!control || !mark_allocation(control, epoch, ALLOCATION_CONTROL)) return;
  mark_value(control->value, epoch);
}

static void allocation_scope_begin(AllocationScope *scope) {
  memset(scope, 0, sizeof(*scope));
  scope->parent = current_allocation_scope;
  current_allocation_scope = scope;
}

static void allocation_scope_collect(AllocationScope *scope, ChimiValue *value,
                                     ChimiControl *control) {
  uint64_t epoch = next_mark_epoch++;
  if (next_mark_epoch == 0) {
    fputs("Chimi runtime allocation mark epoch exhausted\n", stderr); exit(70);
  }
  collecting_scope = scope;
  mark_value(value, epoch);
  mark_control(control, epoch);
  collecting_scope = NULL;
  AllocationHeader **cursor = &scope->allocations;
  while (*cursor) {
    AllocationHeader *header = *cursor;
    if (header->mark_epoch == epoch) {
      cursor = &header->next;
    } else {
      *cursor = header->next;
      scope->live_bytes -= header->bytes;
      free(header);
    }
  }
  ++scope->collections;
}

static void allocation_scope_end(AllocationScope *scope) {
  current_allocation_scope = scope->parent;
  for (AllocationHeader *header = scope->allocations; header; header = header->next)
    header->scope = scope->parent;
  if (!scope->parent || !scope->allocations) return;
  size_t nested_peak = scope->parent->live_bytes + scope->peak_live_bytes;
  if (nested_peak > scope->parent->peak_live_bytes)
    scope->parent->peak_live_bytes = nested_peak;
  AllocationHeader *tail = scope->allocations;
  while (tail->next) tail = tail->next;
  tail->next = scope->parent->allocations;
  scope->parent->allocations = scope->allocations;
  scope->parent->live_bytes += scope->live_bytes;
  scope->parent->total_allocated_bytes += scope->total_allocated_bytes;
  for (uint32_t kind = 0; kind < ALLOCATION_KIND_COUNT; ++kind)
    scope->parent->allocated_bytes_by_kind[kind] += scope->allocated_bytes_by_kind[kind];
  scope->parent->collections += scope->collections;
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
    case CONTROL_BREAK: return "break";
  }
  return "defect";
}

static void json_string(FILE *output, const char *value) {
  fputc('"', output);
  for (const unsigned char *cursor = (const unsigned char *)value; *cursor; ++cursor) {
    switch (*cursor) {
      case '"': fputs("\\\"", output); break;
      case '\\': fputs("\\\\", output); break;
      case '\n': fputs("\\n", output); break;
      case '\r': fputs("\\r", output); break;
      case '\t': fputs("\\t", output); break;
      default:
        if (*cursor < 0x20) fprintf(output, "\\u%04x", *cursor); else fputc(*cursor, output);
    }
  }
  fputc('"', output);
}

static void json_value(FILE *output, const ChimiValue *value);
static void float_text(double value, char *output);
static void json_fields(FILE *output, const RuntimeField *fields, uint32_t length) {
  fputc('{', output);
  for (uint32_t index = 0; index < length; ++index) {
    if (index) fputc(',', output);
    json_string(output, fields[index].name);
    fputc(':', output);
    json_value(output, fields[index].value);
  }
  fputc('}', output);
}
static void json_value(FILE *output, const ChimiValue *value) {
  switch (value->kind) {
    case VALUE_VOID: fputs("{\"type\":\"Void\"}", output); break;
    case VALUE_INTEGER: fprintf(output, "%" PRId64, value->as.integer); break;
    case VALUE_FLOAT: { char text[40]; float_text(value->as.floating, text); fputs(text, output); break; }
    case VALUE_TEXT: json_string(output, value->as.text); break;
    case VALUE_BOOLEAN: fputs(value->as.boolean ? "true" : "false", output); break;
    case VALUE_LIST:
      fputc('[', output); for (uint32_t i = 0; i < list_length(value); ++i) {
        if (i) fputc(',', output); json_value(output, list_get(value, i));
      } fputc(']', output); break;
    case VALUE_RECORD: json_fields(output, value->as.record.fields, value->as.record.length); break;
    case VALUE_CONSTRUCTED:
      fputs("{\"type\":", output); json_string(output, value->as.constructed.type_name);
      if (value->as.constructed.variant_name) {
        fputs(",\"variant\":", output); json_string(output, value->as.constructed.variant_name);
      }
      fputs(",\"fields\":", output);
      json_fields(output, value->as.constructed.fields, value->as.constructed.length);
      fputc('}', output); break;
    case VALUE_CALLABLE:
      fputs("{\"callable\":", output); json_string(output, value->as.callable->name);
      fputc('}', output); break;
    case VALUE_MAP: fputs("{\"map\":\"opaque\"}", output); break;
    case VALUE_JSON_NUMBER: json_string(output, value->as.json_number); break;
    case VALUE_TASK: fprintf(output, "{\"task\":%" PRIu64 "}", value->as.task); break;
    case VALUE_CHANNEL_SENDER:
    case VALUE_CHANNEL_RECEIVER:
    case VALUE_CHANNEL_COMPLETION:
      fprintf(output, "{\"channel\":%" PRIu64 "}", value->as.capability.channel); break;
  }
}

static void emit_event(ChimiRuntime *runtime, const char *event, uint32_t node_id,
                       uint64_t span, int64_t parent, const char *operation,
                       const ChimiControl *outcome) {
  const ChimiNode *node = &CHIMI_NODES[node_id];
  const ChimiOrigin *origin = &CHIMI_ORIGINS[node->origin];
  FILE *output = runtime->telemetry;
  fputs("{\"schema\":\"chimi.runtime.v1\",\"event\":", output); json_string(output, event);
  fprintf(output, ",\"sequence\":%" PRIu64 ",\"computation\":", runtime->next_sequence++);
  json_string(output, CHIMI_COMPUTATION); fputs(",\"execution_id\":", output);
  json_string(output, runtime->execution_id);
  fprintf(output, ",\"span_id\":%" PRIu64 ",\"parent_span_id\":", span);
  if (parent < 0) fputs("null", output); else fprintf(output, "%" PRId64, parent);
  fprintf(output, ",\"core_node_id\":%u,\"origin_id\":%u,\"semantic_id\":%u,\"file_id\":%u,"
         "\"source_start\":%u,\"source_end\":%u,\"operation\":",
         node_id, node->origin, origin->semantic_id, origin->file_id, origin->start, origin->end);
  json_string(output, operation); fputs(",\"attributes\":{", output);
  if (outcome) { fputs("\"outcome\":", output); json_string(output, control_name(outcome)); }
  fputs("}}\n", output);
}

static void emit_dependency(ChimiRuntime *runtime, uint32_t node_id, uint64_t span,
                            uint64_t parent, const ChimiCallable *callable) {
  const ChimiNode *node = &CHIMI_NODES[node_id]; const ChimiOrigin *origin = &CHIMI_ORIGINS[node->origin];
  FILE *output = runtime->telemetry;
  fputs("{\"schema\":\"chimi.runtime.v1\",\"event\":\"dependency_called\",\"sequence\":", output);
  fprintf(output, "%" PRIu64 ",\"computation\":", runtime->next_sequence++);
  json_string(output, CHIMI_COMPUTATION);
  fputs(",\"execution_id\":", output); json_string(output, runtime->execution_id);
  fprintf(output, ",\"span_id\":%" PRIu64 ",\"parent_span_id\":%" PRIu64
         ",\"core_node_id\":%u,\"origin_id\":%u,\"semantic_id\":%u,\"file_id\":%u,"
         "\"source_start\":%u,\"source_end\":%u,\"operation\":\"dependency_call\",\"attributes\":{",
         span, parent, node_id, node->origin, origin->semantic_id, origin->file_id, origin->start, origin->end);
  fputs("\"requirement\":", output);
  json_string(output, CHIMI_STRINGS[CHIMI_SYMBOLS[callable->requirement].name]);
  fprintf(output, ",\"requirement_symbol_id\":%u,\"interface_symbol_id\":%u,\"method\":",
          callable->requirement, callable->interface_symbol); json_string(output, callable->name);
  fprintf(output, ",\"method_semantic_id\":%u}}\n", callable->method);
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
static ChimiValue *void_value(void) { return new_value(VALUE_VOID); }
static char *copy_text(const char *raw) {
  size_t length = strlen(raw);
  char *copy = allocate(length + 1, 1);
  memcpy(copy, raw, length);
  return copy;
}
static ChimiValue *text_value(const char *raw) { ChimiValue *v = new_value(VALUE_TEXT); v->as.text = copy_text(raw); return v; }
static ChimiValue *boolean_value(bool raw) { ChimiValue *v = new_value(VALUE_BOOLEAN); v->as.boolean = raw; return v; }
static ChimiValue *float_value(double raw) { ChimiValue *v = new_value(VALUE_FLOAT); v->as.floating = raw; return v; }

/* The numeric model. It mirrors compiler/chimi/src/numeric.rs value for value and
   message for message, because both execution paths must report identical outcomes. */

/* A defect message that outlives the collector: a defect ends its unit of work. */
static const char *numeric_message(const char *format, ...) {
  va_list arguments; va_start(arguments, format);
  char *message = malloc(256);
  if (!message) { fputs("Chimi runtime allocation failed\n", stderr); exit(70); }
  vsnprintf(message, 256, format, arguments); va_end(arguments);
  return message;
}

/* Canonical Float text: the shortest digits that parse back to the same value,
   positional for decimal exponents -5..15 and scientific otherwise, always with a
   `.` or an `e`. `output` holds at least 40 bytes. */
/* True when `significand` x 10^`exponent` (decimal) parses back to `magnitude`. */
static bool decimal_round_trips(uint64_t significand, int exponent, double magnitude) {
  char text[48]; snprintf(text, sizeof(text), "%" PRIu64 "e%d", significand, exponent);
  return strtod(text, NULL) == magnitude;
}

/* The shortest significand (and its decimal exponent) that parses back to
   `magnitude`, chosen like Rust's `{:e}`: at the shortest length, the candidate
   closest to the value, and the larger one on an exact tie. The correctly rounded
   candidate can miss the round-trip interval at a power of two, where the interval
   below the value is half as wide, so its neighbors are tried too. */
static void shortest_digits(double magnitude, char *digits, int *exponent) {
  for (int precision = 0; precision <= 16; ++precision) {
    char scientific[40]; snprintf(scientific, sizeof(scientific), "%.*e", precision, magnitude);
    uint64_t nearest = 0; const char *cursor = scientific;
    for (; *cursor != 'e'; ++cursor) if (*cursor != '.') nearest = nearest * 10 + (uint64_t)(*cursor - '0');
    int point = atoi(cursor + 1), scale = point - precision;
    uint64_t smallest = 1; for (int digit = 0; digit < precision; ++digit) smallest *= 10;
    uint64_t chosen = 0; bool found = false;
    if (decimal_round_trips(nearest, scale, magnitude)) {
      chosen = nearest; found = true;
      if (decimal_round_trips(nearest + 1, scale, magnitude)) {
        /* Both neighbors fit only on a tie: the exact value is `nearest`5000... */
        char exact[800]; snprintf(exact, sizeof(exact), "%.770e", magnitude);
        char wanted[24]; snprintf(wanted, sizeof(wanted), "%" PRIu64, nearest);
        char actual[800]; size_t length = 0;
        for (const char *c = exact; *c != 'e'; ++c) if (*c != '.') actual[length++] = *c;
        actual[length] = '\0';
        size_t prefix = strlen(wanted);
        bool tie = strncmp(actual, wanted, prefix) == 0 && actual[prefix] == '5' &&
                   strspn(actual + prefix + 1, "0") == length - prefix - 1;
        if (tie) chosen = nearest + 1;
      }
    } else if (decimal_round_trips(nearest + 1, scale, magnitude)) {
      chosen = nearest + 1; found = true;
    } else if (nearest > smallest && decimal_round_trips(nearest - 1, scale, magnitude)) {
      chosen = nearest - 1; found = true;
    }
    if (!found) continue;
    snprintf(digits, 24, "%" PRIu64, chosen);
    size_t count = strlen(digits);
    /* A carry such as 99 + 1 adds a digit; drop the zero it leaves. */
    *exponent = scale + (int)count - 1;
    while (count > 1 && digits[count - 1] == '0') digits[--count] = '\0';
    return;
  }
  /* 17 significant digits always round-trip. */
  char scientific[40]; snprintf(scientific, sizeof(scientific), "%.16e", magnitude);
  size_t count = 0; const char *cursor = scientific;
  for (; *cursor != 'e'; ++cursor) if (*cursor != '.') digits[count++] = *cursor;
  digits[count] = '\0'; *exponent = atoi(cursor + 1);
  while (count > 1 && digits[count - 1] == '0') digits[--count] = '\0';
}

static void float_text(double value, char *output) {
  const char *sign = signbit(value) ? "-" : "";
  if (value == 0) { snprintf(output, 40, "%s0.0", sign); return; }
  char digits[24]; int exponent = 0;
  shortest_digits(fabs(value), digits, &exponent);
  size_t count = strlen(digits);
  if (exponent >= -5 && exponent < 16) {
    if (exponent < 0) {
      snprintf(output, 40, "%s0.%.*s%s", sign, -exponent - 1, "0000", digits);
    } else if (count <= (size_t)exponent + 1) {
      snprintf(output, 40, "%s%s%.*s.0", sign, digits, (int)((size_t)exponent + 1 - count),
               "000000000000000");
    } else {
      snprintf(output, 40, "%s%.*s.%s", sign, exponent + 1, digits, digits + exponent + 1);
    }
  } else if (count == 1) {
    snprintf(output, 40, "%s%se%d", sign, digits, exponent);
  } else {
    snprintf(output, 40, "%s%c.%se%d", sign, digits[0], digits + 1, exponent);
  }
}

static const char *binary_symbol(uint32_t operator) {
  switch (operator) {
    case CHIMI_BINARY_ADD: return "+";
    case CHIMI_BINARY_SUBTRACT: return "-";
    case CHIMI_BINARY_MULTIPLY: return "*";
    case CHIMI_BINARY_DIVIDE: return "/";
    case CHIMI_BINARY_REMAINDER: return "%";
    default: return "?";
  }
}

static bool is_arithmetic(uint32_t operator) {
  return operator == CHIMI_BINARY_ADD || operator == CHIMI_BINARY_SUBTRACT ||
         operator == CHIMI_BINARY_MULTIPLY || operator == CHIMI_BINARY_DIVIDE ||
         operator == CHIMI_BINARY_REMAINDER;
}

/* Stores the exact result and returns true, or returns false on overflow or a zero divisor. */
static bool integer_result(uint32_t operator, int64_t l, int64_t r, int64_t *output) {
  switch (operator) {
    case CHIMI_BINARY_ADD: return !__builtin_add_overflow(l, r, output);
    case CHIMI_BINARY_SUBTRACT: return !__builtin_sub_overflow(l, r, output);
    case CHIMI_BINARY_MULTIPLY: return !__builtin_mul_overflow(l, r, output);
    case CHIMI_BINARY_DIVIDE:
      if (r == 0) return false;
      if (r == -1) return !__builtin_sub_overflow((int64_t)0, l, output);
      *output = l / r; return true;
    case CHIMI_BINARY_REMAINDER:
      if (r == 0) return false;
      /* INT64_MIN / -1 overflows, but INT64_MIN % -1 is exactly 0. */
      *output = r == -1 ? 0 : l % r; return true;
    default: return false;
  }
}

static ChimiControl *integer_arithmetic(uint32_t operator, int64_t l, int64_t r, int64_t node) {
  const char *symbol = binary_symbol(operator); int64_t output = 0;
  if (integer_result(operator, l, r, &output)) return success(integer_value(output));
  const char *format = r == 0 && (operator == CHIMI_BINARY_DIVIDE || operator == CHIMI_BINARY_REMAINDER)
    ? "integer division by zero in `%s`: %" PRId64 " %s %" PRId64
    : "integer overflow in `%s`: %" PRId64 " %s %" PRId64;
  return defect(numeric_message(format, symbol, l, symbol, r), node);
}

static ChimiControl *float_arithmetic(uint32_t operator, double l, double r, int64_t node) {
  const char *symbol = binary_symbol(operator); double output = 0;
  char left_text[40], right_text[40]; float_text(l, left_text); float_text(r, right_text);
  switch (operator) {
    case CHIMI_BINARY_ADD: output = l + r; break;
    case CHIMI_BINARY_SUBTRACT: output = l - r; break;
    case CHIMI_BINARY_MULTIPLY: output = l * r; break;
    case CHIMI_BINARY_DIVIDE:
    case CHIMI_BINARY_REMAINDER:
      if (r == 0) {
        return defect(numeric_message("float division by zero in `%s`: %s %s %s",
                                      symbol, left_text, symbol, right_text), node);
      }
      output = operator == CHIMI_BINARY_DIVIDE ? l / r : fmod(l, r);
      break;
  }
  if (!isfinite(output)) {
    return defect(numeric_message("float `%s` has no finite result: %s %s %s",
                                  symbol, left_text, symbol, right_text), node);
  }
  return success(float_value(output));
}
static ChimiValue *callable_value(ChimiCallable *raw) { ChimiValue *v = new_value(VALUE_CALLABLE); v->as.callable = raw; return v; }
static ChimiValue *map_value(MapNode *raw) { ChimiValue *v = new_value(VALUE_MAP); v->as.map = raw; return v; }
static ChimiValue *json_number_value(const char *raw) { ChimiValue *v = new_value(VALUE_JSON_NUMBER); v->as.json_number = copy_text(raw); return v; }
static ChimiValue *task_value(uint64_t raw) { ChimiValue *v = new_value(VALUE_TASK); v->as.task = raw; return v; }
static ChimiValue *channel_capability_value(ValueKind kind, uint64_t channel,
                                            uint64_t owner, uint64_t token) {
  ChimiValue *value = new_value(kind);
  value->as.capability = (RuntimeCapability){channel, owner, token};
  return value;
}
static ChimiValue *channel_bundle_value(uint64_t channel, uint64_t owner, uint64_t token) {
  ChimiValue *value = new_value(VALUE_RECORD);
  value->as.record.length = 3;
  value->as.record.fields = allocate(3, sizeof(*value->as.record.fields));
  allocation_kind(value->as.record.fields, ALLOCATION_FIELDS);
  value->as.record.fields[0] = (RuntimeField){
    "completion", channel_capability_value(VALUE_CHANNEL_COMPLETION, channel, owner, token)
  };
  value->as.record.fields[1] = (RuntimeField){
    "receiver", channel_capability_value(VALUE_CHANNEL_RECEIVER, channel, owner, token)
  };
  value->as.record.fields[2] = (RuntimeField){
    "sender", channel_capability_value(VALUE_CHANNEL_SENDER, channel, 0, 0)
  };
  return value;
}

typedef struct {
  ChimiValue **visited;
  size_t length;
  size_t capacity;
  uint64_t child;
  bool cancelled;
  bool allocation_failed;
} CapabilityTransfer;

static bool transfer_captured_value(CapabilityTransfer *transfer, ChimiValue *value,
                                    uint32_t depth);

static bool remember_transfer_value(CapabilityTransfer *transfer, ChimiValue *value) {
  for (size_t index = 0; index < transfer->length; ++index)
    if (transfer->visited[index] == value) return false;
  if (transfer->length == transfer->capacity) {
    size_t capacity = transfer->capacity ? transfer->capacity * 2 : 16;
    ChimiValue **visited = realloc(transfer->visited, capacity * sizeof(*visited));
    if (!visited) {
      transfer->allocation_failed = true;
      return false;
    }
    transfer->visited = visited;
    transfer->capacity = capacity;
  }
  transfer->visited[transfer->length++] = value;
  return true;
}

static bool transfer_captured_environment(CapabilityTransfer *transfer,
                                          ChimiEnvironment *environment,
                                          uint32_t depth) {
  if (!environment) return true;
  if (environment->locals)
    for (uint32_t slot = 0; slot < CHIMI_LOCAL_SLOT_COUNT; ++slot)
      if (!transfer_captured_value(transfer, environment->locals[slot], depth + 1)) return false;
  if (environment->requirements)
    for (uint32_t slot = 0; slot < CHIMI_REQUIREMENT_SLOT_COUNT; ++slot)
      if (!transfer_captured_value(transfer, environment->requirements[slot], depth + 1)) return false;
  return true;
}

static bool transfer_captured_callable(CapabilityTransfer *transfer, ChimiCallable *callable,
                                       uint32_t depth) {
  if (!callable) return true;
  if (!transfer_captured_environment(transfer, callable->environment, depth + 1)) return false;
  return transfer_captured_callable(transfer, callable->inner, depth + 1);
}

static bool transfer_captured_map(CapabilityTransfer *transfer, MapNode *node,
                                  uint32_t depth) {
  if (!node) return true;
  return transfer_captured_value(transfer, node->value, depth + 1) &&
         transfer_captured_map(transfer, node->left, depth + 1) &&
         transfer_captured_map(transfer, node->right, depth + 1);
}

static bool transfer_captured_capability(CapabilityTransfer *transfer, ChimiValue *value,
                                         uint32_t request_kind) {
  ChimiSchedulerRequest request = {
    .kind = request_kind,
    .task = transfer->child,
    .channel = value->as.capability.channel,
    .owner = value->as.capability.owner,
    .token = value->as.capability.token,
  };
  ChimiFiberCommand command = chimi_fiber_request(&request, NULL);
  if (command == CHIMI_FIBER_CANCEL) {
    transfer->cancelled = true;
    return false;
  }
  if (request.status != CHIMI_RESPONSE_OK) return false;
  value->as.capability.owner = request.owner;
  value->as.capability.token = request.token;
  return true;
}

static bool transfer_captured_value(CapabilityTransfer *transfer, ChimiValue *value,
                                    uint32_t depth) {
  if (!value || depth > 1024) return depth <= 1024;
  if (!remember_transfer_value(transfer, value)) return !transfer->allocation_failed;
  switch (value->kind) {
    case VALUE_CHANNEL_RECEIVER:
      return transfer_captured_capability(
        transfer, value, CHIMI_REQUEST_CHANNEL_TRANSFER_RECEIVER);
    case VALUE_CHANNEL_COMPLETION:
      return transfer_captured_capability(
        transfer, value, CHIMI_REQUEST_CHANNEL_TRANSFER_COMPLETION);
    case VALUE_LIST:
      for (uint32_t index = 0; index < list_length(value); ++index)
        if (!transfer_captured_value(transfer, list_get(value, index), depth + 1)) return false;
      return true;
    case VALUE_RECORD:
      for (uint32_t index = 0; index < value->as.record.length; ++index)
        if (!transfer_captured_value(
              transfer, value->as.record.fields[index].value, depth + 1)) return false;
      return true;
    case VALUE_CONSTRUCTED:
      for (uint32_t index = 0; index < value->as.constructed.length; ++index)
        if (!transfer_captured_value(
              transfer, value->as.constructed.fields[index].value, depth + 1)) return false;
      return true;
    case VALUE_CALLABLE:
      return transfer_captured_callable(transfer, value->as.callable, depth + 1);
    case VALUE_MAP:
      return transfer_captured_map(transfer, value->as.map, depth + 1);
    case VALUE_VOID:
    case VALUE_INTEGER:
    case VALUE_FLOAT:
    case VALUE_TEXT:
    case VALUE_BOOLEAN:
    case VALUE_JSON_NUMBER:
    case VALUE_TASK:
    case VALUE_CHANNEL_SENDER: return true;
  }
  return false;
}

static ChimiValue *application_arguments(int32_t count, const char **argv) {
  ChimiValue **items = new_value_slots((uint32_t)count);
  for (int32_t argument = 0; argument < count; ++argument) items[argument] = text_value(argv[argument]);
  return list_from_values(items, (uint32_t)count);
}

int32_t chimi_rt_full_trace(const ChimiRuntime *runtime) { return runtime->full_trace; }
ChimiControl *chimi_rt_direct_integer_success(ChimiRuntime *runtime, uint32_t node_count,
                                               int64_t raw) {
  if (!runtime->silent) runtime->next_span += node_count;
  return success(integer_value(raw));
}

static ChimiValue *select_value(ChimiValue *value, const char *name) {
  RuntimeField *fields = NULL; uint32_t length = 0;
  if (value->kind == VALUE_RECORD) { fields = value->as.record.fields; length = value->as.record.length; }
  else if (value->kind == VALUE_CONSTRUCTED) { fields = value->as.constructed.fields; length = value->as.constructed.length; }
  for (uint32_t index = 0; index < length; ++index)
    if (strcmp(fields[index].name, name) == 0) return fields[index].value;
  return NULL;
}

static uint16_t map_height(const MapNode *node) { return node ? node->height : 0; }
static uint16_t map_max(uint16_t left, uint16_t right) { return left > right ? left : right; }
static int chimi_value_compare(const ChimiValue *left_root, const ChimiValue *right_root);
static MapNode *map_node(ChimiValue *key, ChimiValue *value, MapNode *left, MapNode *right) {
  MapNode *node = allocate(1, sizeof(*node));
  allocation_kind(node, ALLOCATION_MAP_NODE);
  node->key = key; node->value = value; node->left = left; node->right = right;
  node->height = (uint16_t)(1 + map_max(map_height(left), map_height(right)));
  return node;
}
static MapNode *map_rotate_left(MapNode *node) {
  MapNode *right = node->right;
  MapNode *left = map_node(node->key, node->value, node->left, right->left);
  return map_node(right->key, right->value, left, right->right);
}
static MapNode *map_rotate_right(MapNode *node) {
  MapNode *left = node->left;
  MapNode *right = map_node(node->key, node->value, left->right, node->right);
  return map_node(left->key, left->value, left->left, right);
}
static MapNode *map_balance(MapNode *node) {
  int balance = (int)map_height(node->left) - (int)map_height(node->right);
  if (balance > 1) {
    if (map_height(node->left->right) > map_height(node->left->left))
      node = map_node(node->key, node->value, map_rotate_left(node->left), node->right);
    return map_rotate_right(node);
  }
  if (balance < -1) {
    if (map_height(node->right->left) > map_height(node->right->right))
      node = map_node(node->key, node->value, node->left, map_rotate_right(node->right));
    return map_rotate_left(node);
  }
  return node;
}
static MapNode *map_put_node(MapNode *node, ChimiValue *key, ChimiValue *value) {
  if (!node) return map_node(key, value, NULL, NULL);
  int order = chimi_value_compare(key, node->key);
  if (order == 0) return map_node(key, value, node->left, node->right);
  if (order < 0) return map_balance(map_node(node->key, node->value,
                                            map_put_node(node->left, key, value), node->right));
  return map_balance(map_node(node->key, node->value, node->left,
                              map_put_node(node->right, key, value)));
}
static ChimiValue *map_get_node(MapNode *node, const ChimiValue *key) {
  while (node) {
    int order = chimi_value_compare(key, node->key);
    if (order == 0) return node->value;
    node = order < 0 ? node->left : node->right;
  }
  return NULL;
}

/* Structural order: the one comparison behind `==`, `<`, `Order.compare`, and Map
   keys. For ordered types it is the source-level order; for types that are only
   equatable (records, maps, Json.Value) it is an internal total order consistent
   with equality. It mirrors execution/structural.rs. The walk keeps its own work
   stack on the heap, so nesting depth never costs host stack. */
typedef struct { const ChimiValue *left; const ChimiValue *right; int decided; } CompareStep;
typedef struct { CompareStep *steps; size_t length; size_t capacity; } CompareStack;

static void compare_push(CompareStack *stack, const ChimiValue *left, const ChimiValue *right, int decided) {
  if (stack->length == stack->capacity) {
    size_t capacity = stack->capacity ? stack->capacity * 2 : 64;
    CompareStep *steps = realloc(stack->steps, capacity * sizeof(*steps));
    if (steps == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); exit(70); }
    stack->steps = steps; stack->capacity = capacity;
  }
  stack->steps[stack->length++] = (CompareStep){left, right, decided};
}
/* A step with no values decides the comparison when every step above it was equal,
   such as the length rule after the shared prefix of two lists. */
static void compare_decide(CompareStack *stack, int decided) { compare_push(stack, NULL, NULL, decided); }
static int order_of(int64_t left, int64_t right) { return (left > right) - (left < right); }

/* A JSON number as sign x 0.digits x 10^exponent with no leading or trailing zeros
   in digits; zero has sign 0. Written exponents clamp at 2^60, like json.rs. */
typedef struct { int sign; const char *digits; size_t length; int64_t exponent; char *owned; } CanonicalNumber;
static CanonicalNumber canonical_number(const char *text) {
  CanonicalNumber number = {0};
  bool negative = *text == '-'; if (negative) ++text;
  size_t mantissa = strcspn(text, "eE"); int64_t exponent = 0;
  if (text[mantissa]) {
    const char *written = text + mantissa + 1; bool negative_exponent = *written == '-';
    if (*written == '-' || *written == '+') ++written;
    for (; *written; ++written) {
      exponent = exponent * 10 + (*written - '0');
      if (exponent > ((int64_t)1 << 60)) exponent = (int64_t)1 << 60;
    }
    if (negative_exponent) exponent = -exponent;
  }
  size_t point = strcspn(text, ".eE"); if (point > mantissa) point = mantissa;
  number.owned = malloc(mantissa + 1);
  if (number.owned == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); exit(70); }
  size_t length = 0;
  for (size_t index = 0; index < mantissa; ++index)
    if (text[index] != '.') number.owned[length++] = text[index];
  size_t leading = 0; while (leading < length && number.owned[leading] == '0') ++leading;
  while (length > leading && number.owned[length - 1] == '0') --length;
  if (length == leading) return number;
  number.sign = negative ? -1 : 1; number.digits = number.owned + leading; number.length = length - leading;
  number.exponent = (int64_t)point - (int64_t)leading + exponent;
  return number;
}
static int json_number_compare(const char *left_text, const char *right_text) {
  CanonicalNumber left = canonical_number(left_text), right = canonical_number(right_text);
  int order = order_of(left.sign, right.sign);
  if (order == 0 && left.sign != 0) {
    order = order_of(left.exponent, right.exponent);
    if (order == 0) {
      size_t shared = left.length < right.length ? left.length : right.length;
      int digits = memcmp(left.digits, right.digits, shared);
      order = digits != 0 ? (digits > 0) - (digits < 0) : order_of((int64_t)left.length, (int64_t)right.length);
    }
    if (left.sign < 0) order = -order;
  }
  free(left.owned); free(right.owned);
  return order;
}

static int field_name_order(const void *left, const void *right) {
  return strcmp((*(const RuntimeField *const *)left)->name, (*(const RuntimeField *const *)right)->name);
}
/* Records have no declaration order; name order is a fixed order both sides share. */
static const RuntimeField **fields_by_name(const RuntimeField *fields, uint32_t length) {
  const RuntimeField **sorted = malloc((length ? length : 1) * sizeof(*sorted));
  if (sorted == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); exit(70); }
  for (uint32_t index = 0; index < length; ++index) sorted[index] = &fields[index];
  qsort(sorted, length, sizeof(*sorted), field_name_order);
  return sorted;
}

typedef struct { const MapNode **nodes; size_t length; } MapEntries;
/* In key order, so equal maps yield equal sequences whatever their insertion history.
   The tree is balanced, so the pending stack never exceeds its height. */
static MapEntries map_entries(const MapNode *root) {
  size_t capacity = 16, length = 0, pending_length = 0;
  const MapNode **nodes = malloc(capacity * sizeof(*nodes)); const MapNode *pending[64];
  if (nodes == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); exit(70); }
  const MapNode *node = root;
  for (;;) {
    while (node) { pending[pending_length++] = node; node = node->left; }
    if (pending_length == 0) break;
    node = pending[--pending_length];
    if (length == capacity) {
      capacity *= 2; const MapNode **grown = realloc(nodes, capacity * sizeof(*nodes));
      if (grown == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); exit(70); }
      nodes = grown;
    }
    nodes[length++] = node; node = node->right;
  }
  return (MapEntries){nodes, length};
}

static uint32_t constructor_ordinal(uint32_t symbol) {
  return CHIMI_CONSTRUCTORS[CHIMI_SYMBOLS[symbol].constructor].ordinal;
}

/* Requires: both values have the same equatable type. Returns -1, 0, or 1. */
static int chimi_value_compare(const ChimiValue *left_root, const ChimiValue *right_root) {
  CompareStack stack = {0}; int order = 0;
  compare_push(&stack, left_root, right_root, 0);
  while (order == 0 && stack.length != 0) {
    CompareStep step = stack.steps[--stack.length];
    if (step.left == NULL) { order = step.decided; continue; }
    const ChimiValue *left = step.left, *right = step.right;
    if (left == right) continue;
    if (left->kind != right->kind) { order = order_of(left->kind, right->kind); continue; }
    switch (left->kind) {
      case VALUE_VOID: break;
      case VALUE_INTEGER: order = order_of(left->as.integer, right->as.integer); break;
      /* Finite, so total; -0.0 and 0.0 are equal. */
      case VALUE_FLOAT: order = (left->as.floating > right->as.floating) - (left->as.floating < right->as.floating); break;
      case VALUE_BOOLEAN: order = order_of(left->as.boolean, right->as.boolean); break;
      case VALUE_TEXT: { int bytes = strcmp(left->as.text, right->as.text); order = (bytes > 0) - (bytes < 0); break; }
      case VALUE_JSON_NUMBER: order = json_number_compare(left->as.json_number, right->as.json_number); break;
      case VALUE_LIST: {
        uint32_t left_length = list_length(left), right_length = list_length(right);
        uint32_t shared = left_length < right_length ? left_length : right_length;
        compare_decide(&stack, order_of(left_length, right_length));
        for (uint32_t index = shared; index-- > 0;)
          compare_push(&stack, list_get(left, index), list_get(right, index), 0);
        break;
      }
      case VALUE_RECORD: {
        uint32_t length = left->as.record.length < right->as.record.length ? left->as.record.length : right->as.record.length;
        const RuntimeField **left_fields = fields_by_name(left->as.record.fields, length);
        const RuntimeField **right_fields = fields_by_name(right->as.record.fields, length);
        for (uint32_t index = length; index-- > 0;)
          compare_push(&stack, left_fields[index]->value, right_fields[index]->value, 0);
        free(left_fields); free(right_fields);
        break;
      }
      case VALUE_CONSTRUCTED: {
        order = order_of(constructor_ordinal(left->as.constructed.constructor),
                         constructor_ordinal(right->as.constructed.constructor));
        if (order != 0) break;
        uint32_t length = left->as.constructed.length;
        for (uint32_t index = length; index-- > 0;)
          compare_push(&stack, left->as.constructed.fields[index].value, right->as.constructed.fields[index].value, 0);
        break;
      }
      case VALUE_MAP: {
        MapEntries left_entries = map_entries(left->as.map), right_entries = map_entries(right->as.map);
        size_t shared = left_entries.length < right_entries.length ? left_entries.length : right_entries.length;
        compare_decide(&stack, order_of((int64_t)left_entries.length, (int64_t)right_entries.length));
        for (size_t index = shared; index-- > 0;) {
          compare_push(&stack, left_entries.nodes[index]->value, right_entries.nodes[index]->value, 0);
          compare_push(&stack, left_entries.nodes[index]->key, right_entries.nodes[index]->key, 0);
        }
        free(left_entries.nodes); free(right_entries.nodes);
        break;
      }
      /* Typing admits no comparison of these; identity keeps the order total. */
      case VALUE_CALLABLE: case VALUE_TASK: case VALUE_CHANNEL_SENDER:
      case VALUE_CHANNEL_RECEIVER: case VALUE_CHANNEL_COMPLETION:
        order = order_of((int64_t)(uintptr_t)left, (int64_t)(uintptr_t)right); break;
    }
  }
  free(stack.steps);
  return order;
}

static uint32_t standard_constructor_symbol(uint32_t standard_constructor) {
  for (uint32_t symbol = 0; symbol < CHIMI_SYMBOL_COUNT; ++symbol)
    if (CHIMI_SYMBOLS[symbol].standard_constructor == standard_constructor) return symbol;
  return UINT32_MAX;
}
static ChimiValue *standard_value(uint32_t standard_constructor, ChimiValue **values,
                                  uint32_t value_count) {
  uint32_t symbol = standard_constructor_symbol(standard_constructor);
  if (symbol == UINT32_MAX) return NULL;
  const ChimiConstructor *shape = &CHIMI_CONSTRUCTORS[CHIMI_SYMBOLS[symbol].constructor];
  if (shape->fields_length != value_count) return NULL;
  ChimiValue *value = new_value(VALUE_CONSTRUCTED);
  value->as.constructed.constructor = symbol;
  value->as.constructed.type_name = CHIMI_STRINGS[shape->type_name];
  value->as.constructed.variant_name = shape->variant_name == UINT32_MAX ? NULL : CHIMI_STRINGS[shape->variant_name];
  value->as.constructed.length = value_count;
  value->as.constructed.fields = allocate(value_count, sizeof(*value->as.constructed.fields));
  allocation_kind(value->as.constructed.fields, ALLOCATION_FIELDS);
  for (uint32_t field = 0; field < value_count; ++field)
    value->as.constructed.fields[field] = (RuntimeField){
      CHIMI_STRINGS[CHIMI_IDS[shape->fields_offset + field]], values[field]
    };
  return value;
}

typedef struct { char *data; size_t length; size_t capacity; } TextBuffer;
static void buffer_reserve(TextBuffer *buffer, size_t extra) {
  if (buffer->length + extra + 1 <= buffer->capacity) return;
  size_t capacity = buffer->capacity ? buffer->capacity : 64;
  while (capacity < buffer->length + extra + 1) capacity *= 2;
  char *data = reallocate_bytes(buffer->data, buffer->capacity, capacity);
  buffer->data = data; buffer->capacity = capacity;
}
static void buffer_bytes(TextBuffer *buffer, const char *value, size_t length) {
  buffer_reserve(buffer, length); memcpy(buffer->data + buffer->length, value, length);
  buffer->length += length; buffer->data[buffer->length] = '\0';
}
static void buffer_char(TextBuffer *buffer, char value) { buffer_bytes(buffer, &value, 1); }
static void buffer_json_string(TextBuffer *buffer, const char *value) {
  buffer_char(buffer, '"');
  for (const unsigned char *cursor = (const unsigned char *)value; *cursor; ++cursor) {
    switch (*cursor) {
      case '"': buffer_bytes(buffer, "\\\"", 2); break;
      case '\\': buffer_bytes(buffer, "\\\\", 2); break;
      case '\b': buffer_bytes(buffer, "\\b", 2); break;
      case '\f': buffer_bytes(buffer, "\\f", 2); break;
      case '\n': buffer_bytes(buffer, "\\n", 2); break;
      case '\r': buffer_bytes(buffer, "\\r", 2); break;
      case '\t': buffer_bytes(buffer, "\\t", 2); break;
      default:
        if (*cursor < 0x20) {
          char escaped[7]; snprintf(escaped, sizeof(escaped), "\\u%04x", *cursor);
          buffer_bytes(buffer, escaped, 6);
        } else buffer_char(buffer, (char)*cursor);
    }
  }
  buffer_char(buffer, '"');
}

static bool encode_json_value(TextBuffer *buffer, const ChimiValue *value);
static bool encode_json_map(TextBuffer *buffer, const MapNode *node, bool *first) {
  if (!node) return true;
  if (!encode_json_map(buffer, node->left, first)) return false;
  if (!*first) buffer_char(buffer, ','); *first = false;
  if (node->key->kind != VALUE_TEXT) return false;
  buffer_json_string(buffer, node->key->as.text); buffer_char(buffer, ':');
  if (!encode_json_value(buffer, node->value)) return false;
  return encode_json_map(buffer, node->right, first);
}
static bool encode_json_value(TextBuffer *buffer, const ChimiValue *value) {
  if (value->kind != VALUE_CONSTRUCTED) return false;
  uint32_t kind = CHIMI_SYMBOLS[value->as.constructed.constructor].standard_constructor;
  ChimiValue *field = value->as.constructed.length ? value->as.constructed.fields[0].value : NULL;
  switch (kind) {
    case CHIMI_CONSTRUCTOR_JSON_NULL: buffer_bytes(buffer, "null", 4); return true;
    case CHIMI_CONSTRUCTOR_JSON_BOOLEAN:
      if (!field || field->kind != VALUE_BOOLEAN) return false;
      buffer_bytes(buffer, field->as.boolean ? "true" : "false", field->as.boolean ? 4 : 5); return true;
    case CHIMI_CONSTRUCTOR_JSON_NUMBER:
      if (!field || field->kind != VALUE_JSON_NUMBER) return false;
      buffer_bytes(buffer, field->as.json_number, strlen(field->as.json_number)); return true;
    case CHIMI_CONSTRUCTOR_JSON_TEXT:
      if (!field || field->kind != VALUE_TEXT) return false;
      buffer_json_string(buffer, field->as.text); return true;
    case CHIMI_CONSTRUCTOR_JSON_ARRAY:
      if (!field || field->kind != VALUE_LIST) return false;
      buffer_char(buffer, '[');
      for (uint32_t index = 0; index < list_length(field); ++index) {
        if (index) buffer_char(buffer, ',');
        if (!encode_json_value(buffer, list_get(field, index))) return false;
      }
      buffer_char(buffer, ']'); return true;
    case CHIMI_CONSTRUCTOR_JSON_OBJECT: {
      if (!field || field->kind != VALUE_MAP) return false;
      buffer_char(buffer, '{'); bool first = true;
      if (!encode_json_map(buffer, field->as.map, &first)) return false;
      buffer_char(buffer, '}'); return true;
    }
    default: return false;
  }
}

typedef struct { const char *source; size_t length; size_t offset; const char *message; } JsonParser;
static void json_skip_space(JsonParser *parser) {
  while (parser->offset < parser->length &&
         strchr(" \t\r\n", parser->source[parser->offset])) ++parser->offset;
}
static bool json_hex(char value, uint32_t *digit) {
  if (value >= '0' && value <= '9') *digit = (uint32_t)(value - '0');
  else if (value >= 'a' && value <= 'f') *digit = (uint32_t)(value - 'a' + 10);
  else if (value >= 'A' && value <= 'F') *digit = (uint32_t)(value - 'A' + 10);
  else return false;
  return true;
}
static bool json_code_unit(JsonParser *parser, uint32_t *value) {
  if (parser->offset + 4 > parser->length) return false;
  *value = 0;
  for (uint32_t index = 0; index < 4; ++index) {
    uint32_t digit;
    if (!json_hex(parser->source[parser->offset++], &digit)) return false;
    *value = *value * 16 + digit;
  }
  return true;
}
static void buffer_utf8(TextBuffer *buffer, uint32_t value) {
  if (value <= 0x7f) buffer_char(buffer, (char)value);
  else if (value <= 0x7ff) {
    buffer_char(buffer, (char)(0xc0 | value >> 6)); buffer_char(buffer, (char)(0x80 | (value & 0x3f)));
  } else if (value <= 0xffff) {
    buffer_char(buffer, (char)(0xe0 | value >> 12)); buffer_char(buffer, (char)(0x80 | ((value >> 6) & 0x3f)));
    buffer_char(buffer, (char)(0x80 | (value & 0x3f)));
  } else {
    buffer_char(buffer, (char)(0xf0 | value >> 18)); buffer_char(buffer, (char)(0x80 | ((value >> 12) & 0x3f)));
    buffer_char(buffer, (char)(0x80 | ((value >> 6) & 0x3f))); buffer_char(buffer, (char)(0x80 | (value & 0x3f)));
  }
}
static char *json_parse_string(JsonParser *parser) {
  if (parser->offset >= parser->length || parser->source[parser->offset++] != '"') return NULL;
  TextBuffer buffer = {0};
  while (parser->offset < parser->length) {
    unsigned char value = (unsigned char)parser->source[parser->offset++];
    if (value == '"') return buffer.data ? buffer.data : allocate(1, 1);
    if (value < 0x20) { parser->message = "unescaped control character"; return NULL; }
    if (value != '\\') { buffer_char(&buffer, (char)value); continue; }
    if (parser->offset >= parser->length) break;
    char escaped = parser->source[parser->offset++];
    switch (escaped) {
      case '"': case '\\': case '/': buffer_char(&buffer, escaped); break;
      case 'b': buffer_char(&buffer, '\b'); break; case 'f': buffer_char(&buffer, '\f'); break;
      case 'n': buffer_char(&buffer, '\n'); break; case 'r': buffer_char(&buffer, '\r'); break;
      case 't': buffer_char(&buffer, '\t'); break;
      case 'u': {
        uint32_t code;
        if (!json_code_unit(parser, &code)) { parser->message = "invalid Unicode escape"; return NULL; }
        if (code >= 0xd800 && code <= 0xdbff) {
          if (parser->offset + 2 > parser->length || parser->source[parser->offset] != '\\' ||
              parser->source[parser->offset + 1] != 'u') { parser->message = "missing low surrogate"; return NULL; }
          parser->offset += 2; uint32_t low;
          if (!json_code_unit(parser, &low) || low < 0xdc00 || low > 0xdfff) {
            parser->message = "invalid low surrogate"; return NULL;
          }
          code = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
        } else if (code >= 0xdc00 && code <= 0xdfff) {
          parser->message = "unexpected low surrogate"; return NULL;
        }
        buffer_utf8(&buffer, code); break;
      }
      default: parser->message = "invalid string escape"; return NULL;
    }
  }
  parser->message = "unterminated string"; return NULL;
}

static ChimiValue *json_parse_value(JsonParser *parser, uint32_t depth);
static ChimiValue *json_parse_array(JsonParser *parser, uint32_t depth) {
  ++parser->offset; json_skip_space(parser);
  ChimiValue **items = NULL; uint32_t length = 0, capacity = 0;
  if (parser->offset < parser->length && parser->source[parser->offset] == ']') ++parser->offset;
  else for (;;) {
    ChimiValue *item = json_parse_value(parser, depth + 1); if (!item) return NULL;
    if (length == capacity) {
      uint32_t old_capacity = capacity;
      capacity = capacity ? capacity * 2 : 4;
      items = reallocate_bytes(items, old_capacity * sizeof(*items), capacity * sizeof(*items));
      allocation_kind(items, ALLOCATION_VALUE_POINTERS);
    }
    items[length++] = item; json_skip_space(parser);
    if (parser->offset >= parser->length) { parser->message = "unterminated array"; return NULL; }
    char separator = parser->source[parser->offset++]; if (separator == ']') break;
    if (separator != ',') { parser->message = "expected comma in array"; return NULL; }
    json_skip_space(parser);
  }
  ChimiValue *fields[] = {list_from_values(items, length)};
  return standard_value(CHIMI_CONSTRUCTOR_JSON_ARRAY, fields, 1);
}
static ChimiValue *json_parse_object(JsonParser *parser, uint32_t depth) {
  ++parser->offset; json_skip_space(parser); MapNode *map = NULL;
  if (parser->offset < parser->length && parser->source[parser->offset] == '}') ++parser->offset;
  else for (;;) {
    if (parser->offset >= parser->length || parser->source[parser->offset] != '"') {
      parser->message = "expected object key"; return NULL;
    }
    char *key_text = json_parse_string(parser); if (!key_text) return NULL; json_skip_space(parser);
    ChimiValue *key = text_value(key_text);
    if (parser->offset >= parser->length || parser->source[parser->offset++] != ':') {
      parser->message = "expected colon after object key"; return NULL;
    }
    if (map_get_node(map, key)) { parser->message = "duplicate object key"; return NULL; }
    ChimiValue *item = json_parse_value(parser, depth + 1); if (!item) return NULL;
    map = map_put_node(map, key, item); json_skip_space(parser);
    if (parser->offset >= parser->length) { parser->message = "unterminated object"; return NULL; }
    char separator = parser->source[parser->offset++]; if (separator == '}') break;
    if (separator != ',') { parser->message = "expected comma in object"; return NULL; }
    json_skip_space(parser);
  }
  ChimiValue *fields_value = map_value(map); ChimiValue *fields[] = {fields_value};
  return standard_value(CHIMI_CONSTRUCTOR_JSON_OBJECT, fields, 1);
}
static ChimiValue *json_parse_number(JsonParser *parser) {
  size_t start = parser->offset;
  if (parser->source[parser->offset] == '-') ++parser->offset;
  if (parser->offset >= parser->length) { parser->message = "invalid number"; return NULL; }
  if (parser->source[parser->offset] == '0') ++parser->offset;
  else if (parser->source[parser->offset] >= '1' && parser->source[parser->offset] <= '9')
    while (parser->offset < parser->length && parser->source[parser->offset] >= '0' && parser->source[parser->offset] <= '9') ++parser->offset;
  else { parser->message = "invalid number"; return NULL; }
  if (parser->offset < parser->length && parser->source[parser->offset] == '.') {
    ++parser->offset; size_t digits = parser->offset;
    while (parser->offset < parser->length && parser->source[parser->offset] >= '0' && parser->source[parser->offset] <= '9') ++parser->offset;
    if (digits == parser->offset) { parser->message = "invalid fraction"; return NULL; }
  }
  if (parser->offset < parser->length && (parser->source[parser->offset] == 'e' || parser->source[parser->offset] == 'E')) {
    ++parser->offset; if (parser->offset < parser->length && (parser->source[parser->offset] == '+' || parser->source[parser->offset] == '-')) ++parser->offset;
    size_t digits = parser->offset;
    while (parser->offset < parser->length && parser->source[parser->offset] >= '0' && parser->source[parser->offset] <= '9') ++parser->offset;
    if (digits == parser->offset) { parser->message = "invalid exponent"; return NULL; }
  }
  size_t length = parser->offset - start; char *number = allocate(length + 1, 1);
  memcpy(number, parser->source + start, length); ChimiValue *raw = json_number_value(number);
  ChimiValue *fields[] = {raw}; return standard_value(CHIMI_CONSTRUCTOR_JSON_NUMBER, fields, 1);
}
static ChimiValue *json_parse_value(JsonParser *parser, uint32_t depth) {
  if (depth > 128) { parser->message = "JSON nesting limit exceeded"; return NULL; }
  json_skip_space(parser); if (parser->offset >= parser->length) { parser->message = "expected JSON value"; return NULL; }
  char value = parser->source[parser->offset];
  if (value == 'n' && parser->length - parser->offset >= 4 && memcmp(parser->source + parser->offset, "null", 4) == 0) {
    parser->offset += 4; return standard_value(CHIMI_CONSTRUCTOR_JSON_NULL, NULL, 0);
  }
  if ((value == 't' && parser->length - parser->offset >= 4 && memcmp(parser->source + parser->offset, "true", 4) == 0) ||
      (value == 'f' && parser->length - parser->offset >= 5 && memcmp(parser->source + parser->offset, "false", 5) == 0)) {
    bool raw = value == 't'; parser->offset += raw ? 4 : 5; ChimiValue *boolean = boolean_value(raw);
    ChimiValue *fields[] = {boolean}; return standard_value(CHIMI_CONSTRUCTOR_JSON_BOOLEAN, fields, 1);
  }
  if (value == '"') { char *text = json_parse_string(parser); if (!text) return NULL;
    ChimiValue *raw = text_value(text); ChimiValue *fields[] = {raw}; return standard_value(CHIMI_CONSTRUCTOR_JSON_TEXT, fields, 1); }
  if (value == '[') return json_parse_array(parser, depth);
  if (value == '{') return json_parse_object(parser, depth);
  if (value == '-' || (value >= '0' && value <= '9')) return json_parse_number(parser);
  parser->message = "expected JSON value"; return NULL;
}

static ChimiControl *invoke(ChimiRuntime *runtime, ChimiCallable *callable, ChimiValue **arguments,
                            uint32_t argument_count, uint64_t span, uint32_t call_node,
                            ChimiEnvironment *call_environment);

static ChimiControl *invoke_intrinsic(ChimiRuntime *runtime, uint32_t intrinsic,
                                      ChimiValue **arguments, uint32_t argument_count,
                                      uint64_t span, uint32_t call_node,
                                      ChimiEnvironment *environment);

static char *copy_bytes(const unsigned char *source, size_t length) {
  char *copy = allocate(length + 1, 1);
  memcpy(copy, source, length);
  return copy;
}

static bool valid_utf8(const unsigned char *text, size_t length) {
  size_t offset = 0;
  while (offset < length) {
    unsigned char first = text[offset++];
    if (first <= 0x7f) {
      if (first == 0) return false;
      continue;
    }
    if (first >= 0xc2 && first <= 0xdf) {
      if (offset >= length || (text[offset++] & 0xc0) != 0x80) return false;
      continue;
    }
    if (first >= 0xe0 && first <= 0xef) {
      if (offset + 1 >= length) return false;
      unsigned char second = text[offset++], third = text[offset++];
      if ((third & 0xc0) != 0x80) return false;
      if (first == 0xe0) { if (second < 0xa0 || second > 0xbf) return false; }
      else if (first == 0xed) { if (second < 0x80 || second > 0x9f) return false; }
      else if ((second & 0xc0) != 0x80) return false;
      continue;
    }
    if (first >= 0xf0 && first <= 0xf4) {
      if (offset + 2 >= length) return false;
      unsigned char second = text[offset++], third = text[offset++], fourth = text[offset++];
      if ((third & 0xc0) != 0x80 || (fourth & 0xc0) != 0x80) return false;
      if (first == 0xf0) { if (second < 0x90 || second > 0xbf) return false; }
      else if (first == 0xf4) { if (second < 0x80 || second > 0x8f) return false; }
      else if ((second & 0xc0) != 0x80) return false;
      continue;
    }
    return false;
  }
  return true;
}

static ChimiControl *host_file_message(uint32_t constructor, const char *path,
                                       const char *message) {
  ChimiValue *fields[] = {
    text_value(path),
    text_value(message ? message : "unknown I/O error"),
  };
  return failure(standard_value(constructor, fields, 2));
}

static ChimiControl *invoke_line_callback(ChimiRuntime *runtime, ChimiCallable *callback,
                                          ChimiValue *state, unsigned char *bytes, size_t length,
                                          uint64_t line, const char *path, uint64_t span,
                                          uint32_t call_node, ChimiEnvironment *environment) {
  if (!valid_utf8(bytes, length)) {
    ChimiValue *fields[] = {text_value(path), integer_value((int64_t)line)};
    return failure(standard_value(CHIMI_CONSTRUCTOR_UTF8_INVALID, fields, 2));
  }
  ChimiValue *arguments[] = {state, text_value(copy_bytes(bytes, length))};
  return invoke(runtime, callback, arguments, 2, span, call_node, environment);
}

/* Ends a reclaiming scope. Only an outermost scope reports to the runtime, because a
   nested scope's statistics already merge into its parent. */
static void end_measured_allocation_scope(ChimiRuntime *runtime, AllocationScope *scope) {
  bool top_scope = scope->parent == NULL;
  allocation_scope_end(scope);
  if (!top_scope) return;
  runtime->scoped_total_allocated_bytes += scope->total_allocated_bytes;
  for (uint32_t kind = 0; kind < ALLOCATION_KIND_COUNT; ++kind)
    runtime->scoped_allocated_bytes_by_kind[kind] += scope->allocated_bytes_by_kind[kind];
  if (scope->peak_live_bytes > runtime->scoped_peak_live_bytes)
    runtime->scoped_peak_live_bytes = scope->peak_live_bytes;
  runtime->scoped_retained_bytes += scope->live_bytes;
  runtime->scoped_collections += scope->collections;
}

static ChimiControl *process_utf8_lines(ChimiRuntime *runtime, ChimiValue **arguments,
                                        uint32_t argument_count, uint64_t span,
                                        uint32_t call_node, ChimiEnvironment *environment) {
  if (argument_count != 3 || arguments[0]->kind != VALUE_TEXT ||
      arguments[2]->kind != VALUE_CALLABLE)
    return defect("verified host function has invalid runtime arguments", call_node);
  if (runtime->cancel) return cancelled();
  const char *path = arguments[0]->as.text;
  bool owns_file = strcmp(path, "-") != 0;
  FILE *input = owns_file ? fopen(path, "rb") : stdin;
  if (!input) return host_file_message(CHIMI_CONSTRUCTOR_FILE_OPEN, path, strerror(errno));
  unsigned char *line = allocate((size_t)runtime->max_line_bytes + 1, 1);
  size_t length = 0;
  uint64_t line_number = 1;
  ChimiValue *state = arguments[1];
  ChimiControl *control = NULL;
  AllocationScope allocation_scope;
  allocation_scope_begin(&allocation_scope);
  for (;;) {
    if (runtime->cancel) {
      control = cancelled();
      allocation_scope_collect(&allocation_scope, NULL, control);
      break;
    }
    errno = 0;
    int byte = fgetc(input);
    if (byte == EOF) {
      if (ferror(input)) {
        control = runtime->cancel && errno == EINTR
          ? cancelled()
          : host_file_message(CHIMI_CONSTRUCTOR_FILE_READ, path, strerror(errno));
        allocation_scope_collect(&allocation_scope, NULL, control);
      } else if (length > 0) {
        control = invoke_line_callback(runtime, arguments[2]->as.callable, state, line, length,
                                       line_number, path, span, call_node, environment);
        allocation_scope_collect(&allocation_scope, NULL, control);
      } else {
        control = success(state);
        allocation_scope_collect(&allocation_scope, NULL, control);
      }
      break;
    }
    if (byte == '\n') {
      size_t content_length = length > 0 && line[length - 1] == '\r' ? length - 1 : length;
      control = invoke_line_callback(runtime, arguments[2]->as.callable, state, line,
                                     content_length, line_number, path, span, call_node,
                                     environment);
      if (control->kind != CONTROL_SUCCESS) {
        allocation_scope_collect(&allocation_scope, NULL, control);
        break;
      }
      state = control->value;
      allocation_scope_collect(&allocation_scope, state, NULL);
      length = 0;
      ++line_number;
      continue;
    }
    if (length == runtime->max_line_bytes) {
      ChimiValue *fields[] = {text_value(path), integer_value(runtime->max_line_bytes)};
      control = failure(standard_value(CHIMI_CONSTRUCTOR_FILE_LINE_TOO_LONG, fields, 2));
      allocation_scope_collect(&allocation_scope, NULL, control);
      break;
    }
    line[length++] = (unsigned char)byte;
  }
  if (owns_file && fclose(input) != 0 &&
      (control->kind == CONTROL_SUCCESS || control->kind == CONTROL_FAILURE)) {
    control = host_file_message(CHIMI_CONSTRUCTOR_FILE_CLOSE, path, strerror(errno));
    allocation_scope_collect(&allocation_scope, NULL, control);
  }
  end_measured_allocation_scope(runtime, &allocation_scope);
  return control;
}

static ChimiControl *invoke_host(ChimiRuntime *runtime, HostFunction function,
                                 ChimiValue **arguments, uint32_t argument_count,
                                 uint64_t span, uint32_t call_node,
                                 ChimiEnvironment *environment) {
  if (function == HOST_PROCESS_UTF8_LINES)
    return process_utf8_lines(runtime, arguments, argument_count, span, call_node, environment);
  if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT)
    return defect("verified host function has invalid runtime arguments", call_node);
  if (runtime->cancel) return cancelled();
  FILE *output = function == HOST_WRITE_LINE ? stdout : stderr;
  if (fputs(arguments[0]->as.text, output) < 0 || fputc('\n', output) == EOF || fflush(output) != 0) {
    const char *stream = function == HOST_WRITE_LINE ? "stdout" : "stderr";
    ChimiValue *fields[] = {text_value(stream), text_value(strerror(errno))};
    return failure(standard_value(CHIMI_CONSTRUCTOR_CONSOLE_WRITE_FAILED, fields, 2));
  }
  return runtime->cancel ? cancelled() : success(void_value());
}

static ChimiControl *construct_value(uint32_t symbol, ChimiValue **values);

static ChimiControl *invoke(ChimiRuntime *runtime, ChimiCallable *callable, ChimiValue **arguments,
                            uint32_t argument_count, uint64_t span, uint32_t call_node,
                            ChimiEnvironment *call_environment) {
  if (callable->kind == CALL_CONSTRUCTOR) return construct_value(callable->symbol, arguments);
  if (callable->kind == CALL_DEPENDENCY) {
    if (!runtime->silent) {
      uint64_t dependency_span = runtime->next_span++;
      emit_dependency(runtime, call_node, dependency_span, span, callable);
    }
    return invoke(runtime, callable->inner, arguments, argument_count, span, call_node, call_environment);
  }
  if (callable->kind == CALL_INTRINSIC)
    return invoke_intrinsic(runtime, callable->intrinsic, arguments, argument_count,
                            span, call_node, call_environment);
  if (callable->kind == CALL_HOST)
    return invoke_host(runtime, callable->host, arguments, argument_count,
                       span, call_node, call_environment);
  ChimiEnvironment *environment;
  if (callable->kind == CALL_DECLARED) {
    // Declared functions capture provisioned requirements, but never local
    // bindings. Build a clean local table for parameters while retaining the
    // requirement snapshot taken when the function became a value.
    environment = new_environment();
    if (CHIMI_REQUIREMENT_SLOT_COUNT != 0)
      memcpy(environment->requirements, callable->environment->requirements,
             CHIMI_REQUIREMENT_SLOT_COUNT * sizeof(*environment->requirements));
  } else {
    environment = clone_environment(callable->environment);
  }
  for (uint32_t slot = 0; slot < CHIMI_REQUIREMENT_SLOT_COUNT; ++slot)
    if (call_environment->requirements[slot] != NULL)
      environment->requirements[slot] = call_environment->requirements[slot];
  uint32_t count = callable->parameters_length < argument_count ? callable->parameters_length : argument_count;
  for (uint32_t index = 0; index < count; ++index)
    environment->locals[environment_slot(CHIMI_IDS[callable->parameters_offset + index])] = arguments[index];
  return run_node(callable->body, runtime, environment, span);
}

static ChimiFiberResult task_fiber_result(ChimiControl *control) {
  if (control->kind == CONTROL_CANCEL)
    return (ChimiFiberResult){.state = CHIMI_FIBER_CANCELLED};
  if (control->kind == CONTROL_DEFECT)
    return (ChimiFiberResult){
      .state = CHIMI_FIBER_DEFECTED,
      .defect_code = (int64_t)(intptr_t)control,
    };
  return (ChimiFiberResult){
    .state = CHIMI_FIBER_SUCCEEDED,
    .value = (int64_t)(intptr_t)control,
  };
}

static ChimiFiberResult chimi_task_entry(void *raw_context) {
  ChimiTaskContext *context = raw_context;
  ChimiControl *control;
  if (context->entry)
    control = context->entry(context->runtime, context->environment, context->span);
  else
    control = invoke(context->runtime, context->callable, NULL, 0, context->span,
                     UINT32_MAX, context->environment);
  return task_fiber_result(control);
}

static ChimiControl *map_values_node(ChimiRuntime *runtime, MapNode *node,
                                     ChimiCallable *callback, MapNode **output,
                                     uint64_t span, uint32_t call_node,
                                     ChimiEnvironment *environment) {
  if (!node) return success(NULL);
  ChimiControl *control = map_values_node(runtime, node->left, callback, output,
                                         span, call_node, environment);
  if (control->kind != CONTROL_SUCCESS) return control;
  ChimiValue *arguments[] = {node->value};
  control = invoke(runtime, callback, arguments, 1, span, call_node, environment);
  if (control->kind != CONTROL_SUCCESS) return control;
  *output = map_put_node(*output, node->key, control->value);
  return map_values_node(runtime, node->right, callback, output,
                         span, call_node, environment);
}

/* `digits [. digits] [(e|E) [+|-] digits]`, as numeric.rs `is_decimal_number`. */
static bool is_decimal_number(const char *text) {
  const char *cursor = text;
  #define CHIMI_DIGITS() do { const char *start = cursor; \
    while (*cursor >= '0' && *cursor <= '9') ++cursor; if (cursor == start) return false; } while (0)
  CHIMI_DIGITS();
  if (*cursor == '.') { ++cursor; CHIMI_DIGITS(); }
  if (*cursor == 'e' || *cursor == 'E') {
    ++cursor; if (*cursor == '+' || *cursor == '-') ++cursor;
    CHIMI_DIGITS();
  }
  #undef CHIMI_DIGITS
  return *cursor == '\0';
}

/* `Int.parse`; returns 0 on success or the failure constructor. */
static uint32_t parse_int_text(const char *text, int64_t *output) {
  if (*text == '\0') return CHIMI_CONSTRUCTOR_INT_PARSE_EMPTY;
  const char *digits = *text == '-' ? text + 1 : text;
  if (*digits == '\0') return CHIMI_CONSTRUCTOR_INT_PARSE_INVALID_DIGIT;
  for (const char *cursor = digits; *cursor; ++cursor)
    if (*cursor < '0' || *cursor > '9') return CHIMI_CONSTRUCTOR_INT_PARSE_INVALID_DIGIT;
  errno = 0; *output = strtoll(text, NULL, 10);
  return errno == ERANGE ? CHIMI_CONSTRUCTOR_INT_PARSE_OUT_OF_RANGE : 0;
}

/* `Float.parse`; returns 0 on success or the failure constructor. */
static uint32_t parse_float_text(const char *text, double *output) {
  if (*text == '\0') return CHIMI_CONSTRUCTOR_FLOAT_PARSE_EMPTY;
  if (!is_decimal_number(*text == '-' ? text + 1 : text)) return CHIMI_CONSTRUCTOR_FLOAT_PARSE_INVALID;
  *output = strtod(text, NULL);
  return isfinite(*output) ? 0 : CHIMI_CONSTRUCTOR_FLOAT_PARSE_OUT_OF_RANGE;
}

/* The debug format of `std/debug` (G10). It MUST match the interpreter's renderer in
   execution/debug.rs byte for byte; the language tests compare both. Captures and
   capability contents are never shown. */
typedef struct { TextBuffer *buffer; bool indented; uint32_t depth; } DebugRenderer;

static void debug_text(TextBuffer *buffer, const char *value) { buffer_bytes(buffer, value, strlen(value)); }

static void debug_quote(TextBuffer *buffer, const char *text) {
  buffer_char(buffer, '"');
  for (const unsigned char *cursor = (const unsigned char *)text; *cursor; ++cursor) {
    switch (*cursor) {
      case '"': debug_text(buffer, "\\\""); break;
      case '\\': debug_text(buffer, "\\\\"); break;
      case '\n': debug_text(buffer, "\\n"); break;
      case '\r': debug_text(buffer, "\\r"); break;
      case '\t': debug_text(buffer, "\\t"); break;
      case '$':
        if (cursor[1] == '{') debug_text(buffer, "\\$"); else buffer_char(buffer, '$');
        break;
      default:
        if (*cursor < 0x20 || *cursor == 0x7f) {
          char escaped[16]; snprintf(escaped, sizeof(escaped), "\\u{%x}", *cursor);
          debug_text(buffer, escaped);
        } else {
          buffer_char(buffer, (char)*cursor);
        }
    }
  }
  buffer_char(buffer, '"');
}

/* An integral JSON number within 2^53 as an integer, any other as the shortest Float. */
static void debug_json_number(TextBuffer *buffer, const char *text) {
  char *end = NULL; errno = 0;
  long long integer = strtoll(text, &end, 10);
  char output[48];
  if (errno == 0 && end != text && *end == '\0') {
    snprintf(output, sizeof(output), "%lld", integer); debug_text(buffer, output); return;
  }
  errno = 0; double value = strtod(text, &end);
  if (end == text || *end != '\0' || !isfinite(value)) { debug_text(buffer, text); return; }
  if (value == trunc(value) && fabs(value) < 9007199254740992.0) {
    snprintf(output, sizeof(output), "%lld", (long long)value); debug_text(buffer, output); return;
  }
  float_text(value == 0.0 ? 0.0 : value, output); debug_text(buffer, output);
}

static void debug_value(DebugRenderer *renderer, const ChimiValue *value);

static void debug_line(DebugRenderer *renderer) {
  buffer_char(renderer->buffer, '\n');
  for (uint32_t i = 0; i < renderer->depth; ++i) debug_text(renderer->buffer, "  ");
}

/* Between parts: `, ` on one line, or a line break and indentation. Before the first
   part on one line, a space when `spaced`. */
static void debug_separator(DebugRenderer *renderer, uint32_t index, bool spaced) {
  if (renderer->indented) debug_line(renderer);
  else if (index > 0) debug_text(renderer->buffer, ", ");
  else if (spaced) buffer_char(renderer->buffer, ' ');
}
static void debug_part_end(DebugRenderer *renderer) {
  if (renderer->indented) buffer_char(renderer->buffer, ',');
}
static void debug_close(DebugRenderer *renderer, const char *close, bool spaced) {
  if (renderer->indented) debug_line(renderer);
  else if (spaced) buffer_char(renderer->buffer, ' ');
  debug_text(renderer->buffer, close);
}

/* Named parts in the given order, as `name: value`; `labels` false drops the names. */
static void debug_fields(DebugRenderer *renderer, const char *open, const char *close, bool spaced,
                         const RuntimeField *fields, const uint32_t *order, uint32_t length,
                         bool labels) {
  debug_text(renderer->buffer, open);
  if (length == 0) { debug_text(renderer->buffer, close); return; }
  ++renderer->depth;
  for (uint32_t i = 0; i < length; ++i) {
    const RuntimeField *field = &fields[order ? order[i] : i];
    debug_separator(renderer, i, spaced);
    if (labels) { debug_text(renderer->buffer, field->name); debug_text(renderer->buffer, ": "); }
    debug_value(renderer, field->value);
    debug_part_end(renderer);
  }
  --renderer->depth;
  debug_close(renderer, close, spaced);
}

static void debug_map(DebugRenderer *renderer, const MapNode *node, uint32_t *index) {
  if (!node) return;
  debug_map(renderer, node->left, index);
  debug_separator(renderer, (*index)++, true);
  /* A key renders on one line even in the indented format, as the interpreter does. */
  bool indented = renderer->indented; uint32_t depth = renderer->depth;
  renderer->indented = false;
  debug_value(renderer, node->key);
  renderer->indented = indented; renderer->depth = depth;
  debug_text(renderer->buffer, ": ");
  debug_value(renderer, node->value);
  debug_part_end(renderer);
  debug_map(renderer, node->right, index);
}

static void debug_value(DebugRenderer *renderer, const ChimiValue *value) {
  TextBuffer *buffer = renderer->buffer;
  char scalar[48];
  switch (value->kind) {
    case VALUE_VOID: debug_text(buffer, "Void"); return;
    case VALUE_INTEGER:
      snprintf(scalar, sizeof(scalar), "%" PRId64, value->as.integer); debug_text(buffer, scalar); return;
    case VALUE_FLOAT:
      float_text(value->as.floating == 0.0 ? 0.0 : value->as.floating, scalar);
      debug_text(buffer, scalar); return;
    case VALUE_TEXT: debug_quote(buffer, value->as.text); return;
    case VALUE_BOOLEAN: debug_text(buffer, value->as.boolean ? "True" : "False"); return;
    case VALUE_LIST: {
      debug_text(buffer, "[");
      uint32_t length = value->as.list.length;
      if (length == 0) { debug_text(buffer, "]"); return; }
      ++renderer->depth;
      for (uint32_t i = 0; i < length; ++i) {
        debug_separator(renderer, i, false);
        debug_value(renderer, list_get(value, i));
        debug_part_end(renderer);
      }
      --renderer->depth;
      debug_close(renderer, "]", false);
      return;
    }
    case VALUE_RECORD: {
      uint32_t length = value->as.record.length;
      uint32_t *order = allocate(length, sizeof(*order));
      for (uint32_t i = 0; i < length; ++i) {
        uint32_t j = i;
        while (j > 0 && strcmp(value->as.record.fields[order[j - 1]].name,
                               value->as.record.fields[i].name) > 0) {
          order[j] = order[j - 1]; --j;
        }
        order[j] = i;
      }
      debug_fields(renderer, "{", "}", true, value->as.record.fields, order, length, true);
      return;
    }
    case VALUE_CONSTRUCTED: {
      uint32_t standard = CHIMI_SYMBOLS[value->as.constructed.constructor].standard_constructor;
      bool prelude = standard == CHIMI_CONSTRUCTOR_OPTION_SOME || standard == CHIMI_CONSTRUCTOR_OPTION_NONE;
      const char *variant = value->as.constructed.variant_name;
      uint32_t length = value->as.constructed.length;
      if (!prelude) debug_text(buffer, value->as.constructed.type_name);
      if (!variant) {
        debug_fields(renderer, " {", "}", true, value->as.constructed.fields, NULL, length, true);
        return;
      }
      if (!prelude) buffer_char(buffer, '.');
      debug_text(buffer, variant);
      if (length > 0)
        debug_fields(renderer, "(", ")", false, value->as.constructed.fields, NULL, length, length > 1);
      return;
    }
    case VALUE_MAP: {
      debug_text(buffer, "Map {");
      if (!value->as.map) { debug_text(buffer, "}"); return; }
      uint32_t index = 0;
      ++renderer->depth;
      debug_map(renderer, value->as.map, &index);
      --renderer->depth;
      debug_close(renderer, "}", true);
      return;
    }
    case VALUE_JSON_NUMBER:
      debug_text(buffer, "Json.Number(");
      debug_json_number(buffer, value->as.json_number);
      debug_text(buffer, ")");
      return;
    case VALUE_CALLABLE: {
      const ChimiCallable *callable = value->as.callable;
      const char *name = NULL;
      switch (callable->kind) {
        case CALL_DECLARED: name = CHIMI_STRINGS[CHIMI_SYMBOLS[callable->symbol].name]; break;
        case CALL_DEPENDENCY: case CALL_INTRINSIC: case CALL_CONSTRUCTOR: name = callable->name; break;
        case CALL_CLOSURE: case CALL_HOST: break;
      }
      debug_text(buffer, "<fn");
      if (name) { buffer_char(buffer, ' '); debug_text(buffer, name); }
      debug_text(buffer, ">");
      return;
    }
    case VALUE_TASK: debug_text(buffer, "<Task>"); return;
    case VALUE_CHANNEL_SENDER: debug_text(buffer, "<Sender>"); return;
    case VALUE_CHANNEL_RECEIVER: debug_text(buffer, "<Receiver>"); return;
    case VALUE_CHANNEL_COMPLETION: debug_text(buffer, "<Completion>"); return;
  }
}

static ChimiValue *debug_render(const ChimiValue *value, bool indented) {
  TextBuffer buffer = {0};
  DebugRenderer renderer = {&buffer, indented, 0};
  debug_value(&renderer, value);
  return text_value(buffer.data);
}

static ChimiControl *intrinsic_shape_defect(uint32_t call_node) {
  return defect("verified standard function has invalid runtime arguments", call_node);
}

static ChimiControl *invoke_intrinsic(ChimiRuntime *runtime, uint32_t intrinsic,
                                      ChimiValue **arguments, uint32_t argument_count,
                                      uint64_t span, uint32_t call_node,
                                      ChimiEnvironment *environment) {
  switch (intrinsic) {
    case CHIMI_INTRINSIC_IO_PATH:
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT)
        return intrinsic_shape_defect(call_node);
      return success(arguments[0]);
    case CHIMI_INTRINSIC_LIST_FIRST: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_LIST)
        return intrinsic_shape_defect(call_node);
      if (list_length(arguments[0]) == 0)
        return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_NONE, NULL, 0));
      ChimiValue *fields[] = {list_get(arguments[0], 0)};
      return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_SOME, fields, 1));
    }
    case CHIMI_INTRINSIC_LIST_LENGTH:
      if (argument_count != 1 || arguments[0]->kind != VALUE_LIST)
        return intrinsic_shape_defect(call_node);
      return success(integer_value((int64_t)list_length(arguments[0])));
    case CHIMI_INTRINSIC_ORDER_COMPARE: {
      if (argument_count != 2) return intrinsic_shape_defect(call_node);
      int order = chimi_value_compare(arguments[0], arguments[1]);
      uint32_t constructor = order < 0 ? CHIMI_CONSTRUCTOR_ORDERING_LESS
                           : order > 0 ? CHIMI_CONSTRUCTOR_ORDERING_GREATER : CHIMI_CONSTRUCTOR_ORDERING_EQUAL;
      return success(standard_value(constructor, NULL, 0));
    }
    /* Both return the left operand when the two are equal. */
    case CHIMI_INTRINSIC_ORDER_MIN:
      if (argument_count != 2) return intrinsic_shape_defect(call_node);
      return success(chimi_value_compare(arguments[0], arguments[1]) > 0 ? arguments[1] : arguments[0]);
    case CHIMI_INTRINSIC_ORDER_MAX:
      if (argument_count != 2) return intrinsic_shape_defect(call_node);
      return success(chimi_value_compare(arguments[0], arguments[1]) < 0 ? arguments[1] : arguments[0]);
    case CHIMI_INTRINSIC_MAP_EMPTY:
      return argument_count == 0 ? success(map_value(NULL)) : intrinsic_shape_defect(call_node);
    case CHIMI_INTRINSIC_MAP_GET: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_MAP)
        return intrinsic_shape_defect(call_node);
      ChimiValue *found = map_get_node(arguments[0]->as.map, arguments[1]);
      if (!found) return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_NONE, NULL, 0));
      ChimiValue *fields[] = {found};
      return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_SOME, fields, 1));
    }
    case CHIMI_INTRINSIC_MAP_PUT: {
      if (argument_count != 3 || arguments[0]->kind != VALUE_MAP)
        return intrinsic_shape_defect(call_node);
      return success(map_value(map_put_node(arguments[0]->as.map, arguments[1], arguments[2])));
    }
    case CHIMI_INTRINSIC_MAP_UPDATE: {
      if (argument_count != 3 || arguments[0]->kind != VALUE_MAP ||
          arguments[2]->kind != VALUE_CALLABLE) return intrinsic_shape_defect(call_node);
      ChimiValue *found = map_get_node(arguments[0]->as.map, arguments[1]);
      ChimiValue *current;
      if (found) { ChimiValue *fields[] = {found}; current = standard_value(CHIMI_CONSTRUCTOR_OPTION_SOME, fields, 1); }
      else current = standard_value(CHIMI_CONSTRUCTOR_OPTION_NONE, NULL, 0);
      ChimiValue *callback_arguments[] = {current};
      ChimiControl *control = invoke(runtime, arguments[2]->as.callable, callback_arguments, 1,
                                     span, call_node, environment);
      if (control->kind != CONTROL_SUCCESS) return control;
      return success(map_value(map_put_node(arguments[0]->as.map, arguments[1], control->value)));
    }
    case CHIMI_INTRINSIC_MAP_MAP_VALUES: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_MAP || arguments[1]->kind != VALUE_CALLABLE)
        return intrinsic_shape_defect(call_node);
      MapNode *output = NULL;
      ChimiControl *control = map_values_node(runtime, arguments[0]->as.map, arguments[1]->as.callable,
                                              &output, span, call_node, environment);
      return control->kind == CONTROL_SUCCESS ? success(map_value(output)) : control;
    }
    case CHIMI_INTRINSIC_JSON_DECODE: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) return intrinsic_shape_defect(call_node);
      JsonParser parser = {arguments[0]->as.text, strlen(arguments[0]->as.text), 0, NULL};
      ChimiValue *value = json_parse_value(&parser, 0); json_skip_space(&parser);
      if (value && parser.offset == parser.length) return success(value);
      if (!parser.message) parser.message = "unexpected trailing JSON input";
      ChimiValue *fields[] = {text_value(parser.message), integer_value((int64_t)parser.offset)};
      return failure(standard_value(CHIMI_CONSTRUCTOR_DECODE_INVALID_JSON, fields, 2));
    }
    case CHIMI_INTRINSIC_JSON_ENCODE: {
      if (argument_count != 1) return intrinsic_shape_defect(call_node);
      TextBuffer buffer = {0};
      if (!encode_json_value(&buffer, arguments[0])) return intrinsic_shape_defect(call_node);
      if (!buffer.data) buffer.data = allocate(1, 1);
      return success(text_value(buffer.data));
    }
    case CHIMI_INTRINSIC_JSON_BOOLEAN: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_BOOLEAN) return intrinsic_shape_defect(call_node);
      ChimiValue *fields[] = {arguments[0]}; return success(standard_value(CHIMI_CONSTRUCTOR_JSON_BOOLEAN, fields, 1));
    }
    case CHIMI_INTRINSIC_JSON_INT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_INTEGER) return intrinsic_shape_defect(call_node);
      char *number = allocate(32, 1); snprintf(number, 32, "%" PRId64, arguments[0]->as.integer);
      ChimiValue *raw = json_number_value(number); ChimiValue *fields[] = {raw};
      return success(standard_value(CHIMI_CONSTRUCTOR_JSON_NUMBER, fields, 1));
    }
    case CHIMI_INTRINSIC_JSON_TEXT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) return intrinsic_shape_defect(call_node);
      ChimiValue *fields[] = {arguments[0]}; return success(standard_value(CHIMI_CONSTRUCTOR_JSON_TEXT, fields, 1));
    }
    case CHIMI_INTRINSIC_JSON_ARRAY: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_LIST) return intrinsic_shape_defect(call_node);
      ChimiValue *fields[] = {arguments[0]}; return success(standard_value(CHIMI_CONSTRUCTOR_JSON_ARRAY, fields, 1));
    }
    case CHIMI_INTRINSIC_JSON_OBJECT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_MAP) return intrinsic_shape_defect(call_node);
      ChimiValue *fields[] = {arguments[0]}; return success(standard_value(CHIMI_CONSTRUCTOR_JSON_OBJECT, fields, 1));
    }
    case CHIMI_INTRINSIC_JSON_NUMBER_TO_INT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_JSON_NUMBER) return intrinsic_shape_defect(call_node);
      const char *number = arguments[0]->as.json_number;
      if (strpbrk(number, ".eE")) {
        ChimiValue *fields[] = {arguments[0]};
        return failure(standard_value(CHIMI_CONSTRUCTOR_NUMBER_NOT_INTEGER, fields, 1));
      }
      errno = 0; char *end = NULL; int64_t value = strtoll(number, &end, 10);
      if (errno == ERANGE || !end || *end) {
        ChimiValue *fields[] = {arguments[0]};
        return failure(standard_value(CHIMI_CONSTRUCTOR_NUMBER_OUT_OF_RANGE, fields, 1));
      }
      return success(integer_value(value));
    }
    case CHIMI_INTRINSIC_TEXT_APPEND: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_TEXT ||
          arguments[1]->kind != VALUE_TEXT) return intrinsic_shape_defect(call_node);
      size_t left = strlen(arguments[0]->as.text), right = strlen(arguments[1]->as.text);
      char *output = allocate(left + right + 1, 1);
      memcpy(output, arguments[0]->as.text, left);
      memcpy(output + left, arguments[1]->as.text, right);
      return success(text_value(output));
    }
    case CHIMI_INTRINSIC_TEXT_EQUAL:
      if (argument_count != 2 || arguments[0]->kind != VALUE_TEXT ||
          arguments[1]->kind != VALUE_TEXT) return intrinsic_shape_defect(call_node);
      return success(boolean_value(strcmp(arguments[0]->as.text, arguments[1]->as.text) == 0));
    case CHIMI_INTRINSIC_INT_PARSE: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) return intrinsic_shape_defect(call_node);
      int64_t value = 0; uint32_t error = parse_int_text(arguments[0]->as.text, &value);
      return error ? failure(standard_value(error, NULL, 0)) : success(integer_value(value));
    }
    case CHIMI_INTRINSIC_INT_TO_TEXT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_INTEGER) return intrinsic_shape_defect(call_node);
      char text[24]; snprintf(text, sizeof(text), "%" PRId64, arguments[0]->as.integer);
      return success(text_value(text));
    }
    case CHIMI_INTRINSIC_INT_ADD_CHECKED:
    case CHIMI_INTRINSIC_INT_SUBTRACT_CHECKED:
    case CHIMI_INTRINSIC_INT_MULTIPLY_CHECKED:
    case CHIMI_INTRINSIC_INT_DIVIDE_CHECKED:
    case CHIMI_INTRINSIC_INT_REMAINDER_CHECKED: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_INTEGER ||
          arguments[1]->kind != VALUE_INTEGER) return intrinsic_shape_defect(call_node);
      static const uint32_t operators[] = {CHIMI_BINARY_ADD, CHIMI_BINARY_SUBTRACT, CHIMI_BINARY_MULTIPLY,
                                           CHIMI_BINARY_DIVIDE, CHIMI_BINARY_REMAINDER};
      int64_t value = 0;
      if (!integer_result(operators[intrinsic - CHIMI_INTRINSIC_INT_ADD_CHECKED],
                          arguments[0]->as.integer, arguments[1]->as.integer, &value))
        return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_NONE, NULL, 0));
      ChimiValue *fields[] = {integer_value(value)};
      return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_SOME, fields, 1));
    }
    case CHIMI_INTRINSIC_INT_MODULO: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_INTEGER ||
          arguments[1]->kind != VALUE_INTEGER) return intrinsic_shape_defect(call_node);
      int64_t value = arguments[0]->as.integer, modulus = arguments[1]->as.integer;
      if (modulus == 0) {
        return defect(numeric_message("integer division by zero in `Int.modulo`: Int.modulo(%" PRId64
                                      ", %" PRId64 ")", value, modulus), call_node);
      }
      int64_t remainder = modulus == -1 ? 0 : value % modulus;
      if (remainder != 0 && (remainder < 0) != (modulus < 0)) remainder += modulus;
      return success(integer_value(remainder));
    }
    case CHIMI_INTRINSIC_FLOAT_FROM_INT:
      if (argument_count != 1 || arguments[0]->kind != VALUE_INTEGER) return intrinsic_shape_defect(call_node);
      return success(float_value((double)arguments[0]->as.integer));
    case CHIMI_INTRINSIC_FLOAT_TRUNCATE:
    case CHIMI_INTRINSIC_FLOAT_FLOOR:
    case CHIMI_INTRINSIC_FLOAT_CEILING:
    case CHIMI_INTRINSIC_FLOAT_ROUND: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_FLOAT) return intrinsic_shape_defect(call_node);
      double value = arguments[0]->as.floating;
      switch (intrinsic) {
        case CHIMI_INTRINSIC_FLOAT_TRUNCATE: value = trunc(value); break;
        case CHIMI_INTRINSIC_FLOAT_FLOOR: value = floor(value); break;
        case CHIMI_INTRINSIC_FLOAT_CEILING: value = ceil(value); break;
        default: value = round(value); break; /* half away from zero */
      }
      return success(float_value(value));
    }
    case CHIMI_INTRINSIC_FLOAT_TO_INT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_FLOAT) return intrinsic_shape_defect(call_node);
      double value = arguments[0]->as.floating;
      if (value != trunc(value)) return failure(standard_value(CHIMI_CONSTRUCTOR_FLOAT_NOT_INTEGER, NULL, 0));
      /* -2^63 is exactly representable; 2^63 is the first value above Int. */
      if (!(value >= -9223372036854775808.0 && value < 9223372036854775808.0))
        return failure(standard_value(CHIMI_CONSTRUCTOR_FLOAT_OUT_OF_RANGE, NULL, 0));
      return success(integer_value((int64_t)value));
    }
    case CHIMI_INTRINSIC_DEBUG_RENDER:
    case CHIMI_INTRINSIC_DEBUG_RENDER_INDENTED:
      if (argument_count != 1) return intrinsic_shape_defect(call_node);
      return success(debug_render(arguments[0], intrinsic == CHIMI_INTRINSIC_DEBUG_RENDER_INDENTED));
    case CHIMI_INTRINSIC_FLOAT_TO_TEXT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_FLOAT) return intrinsic_shape_defect(call_node);
      char text[40]; float_text(arguments[0]->as.floating, text);
      return success(text_value(text));
    }
    case CHIMI_INTRINSIC_FLOAT_PARSE: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) return intrinsic_shape_defect(call_node);
      double value = 0; uint32_t error = parse_float_text(arguments[0]->as.text, &value);
      return error ? failure(standard_value(error, NULL, 0)) : success(float_value(value));
    }
    case CHIMI_INTRINSIC_TASK_START: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_CALLABLE)
        return intrinsic_shape_defect(call_node);
      ChimiTaskContext *context = allocate(1, sizeof(*context));
      context->runtime = runtime;
      context->environment = capture_requirements(environment);
      context->callable = arguments[0]->as.callable;
      context->entry = NULL;
      context->span = span;
      ChimiSchedulerRequest request = {
        .kind = CHIMI_REQUEST_TASK_START,
        .entry = chimi_task_entry,
        .context = context,
      };
      ChimiFiberCommand command = chimi_fiber_request(&request, NULL);
      if (command == CHIMI_FIBER_CANCEL) return cancelled();
      if (request.status != CHIMI_RESPONSE_OK)
        return defect("Task.start scheduler request failed", call_node);
      CapabilityTransfer transfer = {.child = request.task};
      bool transferred = transfer_captured_value(&transfer, arguments[0], 0);
      free(transfer.visited);
      if (!transferred) {
        if (transfer.cancelled) return cancelled();
        return defect("Task.start could not transfer an exclusive capture", call_node);
      }
      return success(task_value(request.task));
    }
    case CHIMI_INTRINSIC_TASK_RESULT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TASK)
        return intrinsic_shape_defect(call_node);
      for (;;) {
        ChimiSchedulerRequest request = {
          .kind = CHIMI_REQUEST_TASK_RESULT,
          .task = arguments[0]->as.task,
        };
        ChimiFiberCommand command = chimi_fiber_request(&request, NULL);
        if (command == CHIMI_FIBER_CANCEL) return cancelled();
        if (request.status == CHIMI_RESPONSE_PENDING) continue;
        if (request.status == CHIMI_RESPONSE_SUCCEEDED)
          return (ChimiControl *)(intptr_t)request.value;
        if (request.status == CHIMI_RESPONSE_CANCELLED) return cancelled();
        if (request.status == CHIMI_RESPONSE_DEFECTED && request.value != 0)
          return (ChimiControl *)(intptr_t)request.value;
        if (request.status == CHIMI_RESPONSE_STACK_OVERFLOW)
          return defect("Task stack overflow", call_node);
        return defect("Task.result scheduler request failed", call_node);
      }
    }
    case CHIMI_INTRINSIC_TASK_CANCEL: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TASK)
        return intrinsic_shape_defect(call_node);
      ChimiSchedulerRequest request = {
        .kind = CHIMI_REQUEST_TASK_CANCEL,
        .task = arguments[0]->as.task,
      };
      ChimiFiberCommand command = chimi_fiber_request(&request, NULL);
      if (command == CHIMI_FIBER_CANCEL) return cancelled();
      return request.status == CHIMI_RESPONSE_OK
        ? success(void_value())
        : defect("Task.cancel scheduler request failed", call_node);
    }
    case CHIMI_INTRINSIC_CHANNEL_BOUNDED: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_INTEGER ||
          arguments[0]->as.integer < 0)
        return intrinsic_shape_defect(call_node);
      ChimiSchedulerRequest request = {
        .kind = CHIMI_REQUEST_CHANNEL_BOUNDED,
        .value = arguments[0]->as.integer,
      };
      ChimiFiberCommand command = chimi_fiber_request(&request, NULL);
      if (command == CHIMI_FIBER_CANCEL) return cancelled();
      return request.status == CHIMI_RESPONSE_OK
        ? success(channel_bundle_value(request.channel, request.owner, request.token))
        : defect("Channel.bounded scheduler request failed", call_node);
    }
    case CHIMI_INTRINSIC_CHANNEL_SEND: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_CHANNEL_SENDER)
        return intrinsic_shape_defect(call_node);
      for (;;) {
        ChimiSchedulerRequest request = {
          .kind = CHIMI_REQUEST_CHANNEL_SEND,
          .channel = arguments[0]->as.capability.channel,
          .value = (int64_t)(intptr_t)arguments[1],
        };
        ChimiFiberCommand command = chimi_fiber_request(&request, NULL);
        if (command == CHIMI_FIBER_CANCEL) return cancelled();
        if (request.status == CHIMI_RESPONSE_PENDING) continue;
        if (request.status == CHIMI_RESPONSE_OK) return success(void_value());
        ChimiValue *recovered = (ChimiValue *)(intptr_t)request.value;
        ChimiValue *fields[] = {recovered};
        if (request.status == CHIMI_RESPONSE_CLOSED)
          return failure(standard_value(CHIMI_CONSTRUCTOR_CHANNEL_SEND_CLOSED, fields, 1));
        if (request.status == CHIMI_RESPONSE_ABORTED)
          return failure(standard_value(CHIMI_CONSTRUCTOR_CHANNEL_SEND_ABORTED, fields, 1));
        return defect("Channel.send scheduler request failed", call_node);
      }
    }
    case CHIMI_INTRINSIC_CHANNEL_RECEIVE: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_CHANNEL_RECEIVER)
        return intrinsic_shape_defect(call_node);
      RuntimeCapability capability = arguments[0]->as.capability;
      for (;;) {
        ChimiSchedulerRequest request = {
          .kind = CHIMI_REQUEST_CHANNEL_RECEIVE,
          .channel = capability.channel,
          .owner = capability.owner,
          .token = capability.token,
        };
        ChimiFiberCommand command = chimi_fiber_request(&request, NULL);
        if (command == CHIMI_FIBER_CANCEL) return cancelled();
        if (request.status == CHIMI_RESPONSE_PENDING) continue;
        if (request.status == CHIMI_RESPONSE_SUCCEEDED) {
          ChimiValue *fields[] = {(ChimiValue *)(intptr_t)request.value};
          return success(standard_value(CHIMI_CONSTRUCTOR_CHANNEL_RECEIVED, fields, 1));
        }
        if (request.status == CHIMI_RESPONSE_DRAINED)
          return success(standard_value(CHIMI_CONSTRUCTOR_CHANNEL_DRAINED, NULL, 0));
        if (request.status == CHIMI_RESPONSE_ABORTED)
          return failure(standard_value(CHIMI_CONSTRUCTOR_CHANNEL_ABORTED, NULL, 0));
        return defect("Channel.receive scheduler request failed", call_node);
      }
    }
    case CHIMI_INTRINSIC_CHANNEL_CLOSE: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_CHANNEL_COMPLETION)
        return intrinsic_shape_defect(call_node);
      RuntimeCapability capability = arguments[0]->as.capability;
      ChimiSchedulerRequest request = {
        .kind = CHIMI_REQUEST_CHANNEL_CLOSE,
        .channel = capability.channel,
        .owner = capability.owner,
        .token = capability.token,
      };
      ChimiFiberCommand command = chimi_fiber_request(&request, NULL);
      if (command == CHIMI_FIBER_CANCEL) return cancelled();
      return request.status == CHIMI_RESPONSE_OK
        ? success(void_value())
        : defect("Channel.close scheduler request failed", call_node);
    }
    case CHIMI_INTRINSIC_CHANNEL_ABORT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_CHANNEL_RECEIVER)
        return intrinsic_shape_defect(call_node);
      RuntimeCapability capability = arguments[0]->as.capability;
      ChimiSchedulerRequest request = {
        .kind = CHIMI_REQUEST_CHANNEL_ABORT,
        .channel = capability.channel,
        .owner = capability.owner,
        .token = capability.token,
      };
      ChimiFiberCommand command = chimi_fiber_request(&request, NULL);
      if (command == CHIMI_FIBER_CANCEL) return cancelled();
      return request.status == CHIMI_RESPONSE_OK
        ? success(void_value())
        : defect("Channel.abort scheduler request failed", call_node);
    }
  }
  return defect("unknown standard intrinsic", call_node);
}

/* Module constants, decoded from CHIMI_CONSTANT_VALUES before the entry runs. They are
   allocated outside every allocation scope, so no collection frees them, and values are
   immutable, so every task shares them. Decoding cannot fail and emits no events. */
static ChimiValue **constant_values;

static ChimiValue *decode_constant(const ChimiConstantValue *raw) {
  const uint32_t *ids = &CHIMI_IDS[raw->offset];
  switch (raw->kind) {
    case CHIMI_CONSTANT_VOID: return void_value();
    case CHIMI_CONSTANT_INTEGER: return integer_value(raw->integer);
    case CHIMI_CONSTANT_FLOAT: {
      uint64_t bits = (uint64_t)raw->integer; double value; memcpy(&value, &bits, sizeof(value));
      return float_value(value);
    }
    case CHIMI_CONSTANT_TEXT: return text_value(CHIMI_STRINGS[raw->a]);
    case CHIMI_CONSTANT_BOOLEAN: return boolean_value(raw->integer != 0);
    case CHIMI_CONSTANT_JSON_NUMBER: return json_number_value(CHIMI_STRINGS[raw->a]);
    case CHIMI_CONSTANT_LIST: {
      ChimiValue **items = allocate(raw->length, sizeof(*items));
      for (uint32_t i = 0; i < raw->length; ++i) items[i] = constant_values[ids[i]];
      return list_from_values(items, raw->length);
    }
    case CHIMI_CONSTANT_RECORD: {
      ChimiValue *value = new_value(VALUE_RECORD);
      value->as.record.length = raw->length;
      value->as.record.fields = allocate(raw->length, sizeof(*value->as.record.fields));
      allocation_kind(value->as.record.fields, ALLOCATION_FIELDS);
      for (uint32_t i = 0; i < raw->length; ++i)
        value->as.record.fields[i] = (RuntimeField){CHIMI_STRINGS[ids[2 * i]], constant_values[ids[2 * i + 1]]};
      return value;
    }
    case CHIMI_CONSTANT_CONSTRUCTED: {
      ChimiValue **fields = allocate(raw->length, sizeof(*fields));
      for (uint32_t i = 0; i < raw->length; ++i) fields[i] = constant_values[ids[i]];
      return construct_value(raw->a, fields)->value;
    }
    case CHIMI_CONSTANT_MAP: {
      MapNode *map = NULL;
      for (uint32_t i = 0; i < raw->length; ++i)
        map = map_put_node(map, constant_values[ids[2 * i]], constant_values[ids[2 * i + 1]]);
      return map_value(map);
    }
    default: break;
  }
  ChimiCallable *callable = allocate(1, sizeof(*callable));
  allocation_kind(callable, ALLOCATION_CALLABLE);
  const ChimiSymbol *shape = &CHIMI_SYMBOLS[raw->a];
  switch (raw->kind) {
    case CHIMI_CONSTANT_DECLARED: {
      const ChimiFunction *function = find_function(raw->a);
      callable->kind = CALL_DECLARED; callable->symbol = raw->a; callable->body = function->body;
      callable->parameters_offset = function->parameters_offset;
      callable->parameters_length = function->parameters_length;
      callable->environment = capture_requirements(new_environment()); callable->name = "function";
      break;
    }
    case CHIMI_CONSTANT_INTRINSIC:
      callable->kind = CALL_INTRINSIC; callable->intrinsic = shape->intrinsic;
      callable->name = CHIMI_STRINGS[shape->name];
      break;
    case CHIMI_CONSTANT_CONSTRUCTOR:
      callable->kind = CALL_CONSTRUCTOR; callable->symbol = raw->a; callable->name = CHIMI_STRINGS[shape->name];
      break;
    case CHIMI_CONSTANT_CLOSURE: {
      const ChimiNode *node = &CHIMI_NODES[raw->a];
      ChimiEnvironment *environment = new_environment();
      for (uint32_t i = 0; i < raw->length; ++i)
        environment->locals[environment_slot(ids[2 * i])] = constant_values[ids[2 * i + 1]];
      callable->kind = CALL_CLOSURE; callable->body = node->a;
      callable->parameters_offset = node->offset; callable->parameters_length = node->length;
      callable->environment = environment; callable->name = "closure";
      break;
    }
    default: abort();
  }
  return callable_value(callable);
}

static void decode_constants(void) {
  constant_values = allocate(CHIMI_CONSTANT_VALUE_COUNT, sizeof(*constant_values));
  for (uint32_t i = 0; i < CHIMI_CONSTANT_VALUE_COUNT; ++i)
    constant_values[i] = decode_constant(&CHIMI_CONSTANT_VALUES[i]);
}

static ChimiControl *eval_reference(ChimiRuntime *runtime, uint32_t node_id,
                                    ChimiEnvironment *environment, uint64_t span) {
  uint32_t symbol = CHIMI_NODES[node_id].a; const ChimiSymbol *shape = &CHIMI_SYMBOLS[symbol];
  switch (shape->kind) {
    case CHIMI_SYMBOL_FUNCTION: {
      const ChimiFunction *function = find_function(symbol);
      ChimiCallable *callable = allocate(1, sizeof(*callable));
      allocation_kind(callable, ALLOCATION_CALLABLE); callable->kind = CALL_DECLARED;
      callable->symbol = symbol; callable->body = function->body;
      callable->parameters_offset = function->parameters_offset; callable->parameters_length = function->parameters_length;
      callable->environment = capture_requirements(environment); callable->name = "function";
      return success(callable_value(callable));
    }
    case CHIMI_SYMBOL_CONSTANT:
      return success(constant_values[shape->constant]);
    case CHIMI_SYMBOL_REQUIREMENT:
      if (environment->requirements[shape->environment_slot])
        return success(environment->requirements[shape->environment_slot]);
      return defect("closed core has no provider", -1);
    case CHIMI_SYMBOL_LOCAL:
      if (environment->locals[shape->environment_slot])
        return success(environment->locals[shape->environment_slot]);
      return defect("verified local is not bound", -1);
    case CHIMI_SYMBOL_CONSTRUCTOR: {
      ChimiCallable *callable = allocate(1, sizeof(*callable));
      allocation_kind(callable, ALLOCATION_CALLABLE); callable->kind = CALL_CONSTRUCTOR;
      callable->symbol = symbol; callable->name = CHIMI_STRINGS[shape->name];
      return success(callable_value(callable));
    }
    case CHIMI_SYMBOL_INTRINSIC: {
      ChimiCallable *callable = allocate(1, sizeof(*callable));
      allocation_kind(callable, ALLOCATION_CALLABLE); callable->kind = CALL_INTRINSIC;
      callable->intrinsic = shape->intrinsic; callable->name = CHIMI_STRINGS[shape->name];
      return success(callable_value(callable));
    }
    default: return defect("symbol is not a runtime value", -1);
  }
}

/* Core nodes share their incoming environment because ordinary evaluation only reads it.
   Sequence and provision clone before binding locals or requirements, and match arms clone
   before binding patterns. Any new node that mutates either table must establish the same
   isolation boundary instead of writing through this shared pointer. */
#define NODE_WRAPPER(NAME, OPERATION, ...) \
  ChimiControl *chimi_rt_##NAME(ChimiRuntime *runtime, uint32_t node_id, ChimiEnvironment *incoming, uint64_t parent) { \
    uint64_t span = begin_node(runtime, node_id, parent, OPERATION); ChimiEnvironment *environment = incoming; \
    (void)environment; \
    ChimiControl *result = NULL; do { __VA_ARGS__ } while (0); return finish_node(runtime, node_id, parent, span, OPERATION, result); \
  }
#define RUN(CHILD) run_node((CHILD), runtime, environment, span)
#define REQUIRE_SUCCESS(CONTROL, TARGET) do { ChimiControl *_control = (CONTROL); \
  if (_control->kind != CONTROL_SUCCESS) { result = _control; break; } (TARGET) = _control->value; } while (0)

NODE_WRAPPER(constant, "constant", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiValue *value;
  if (node->a == CHIMI_LITERAL_INTEGER) value = integer_value(node->integer);
  else if (node->a == CHIMI_LITERAL_FLOAT) {
    double raw; memcpy(&raw, &node->integer, sizeof(raw)); value = float_value(raw);
  } else if (node->a == CHIMI_LITERAL_TEXT) value = text_value(CHIMI_STRINGS[node->b]);
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
  const ChimiNode *node = &CHIMI_NODES[node_id];
  ChimiValue *provider = environment->requirements[environment_slot(node->a)];
  ChimiCallable *inner = NULL;
  if (!provider) {
    uint32_t standard_type = CHIMI_SYMBOLS[node->b].standard_type;
    inner = allocate(1, sizeof(*inner)); allocation_kind(inner, ALLOCATION_CALLABLE);
    inner->kind = CALL_HOST;
    if (standard_type == CHIMI_TYPE_FILE_SYSTEM && node->c == 0)
      inner->host = HOST_PROCESS_UTF8_LINES;
    else if (standard_type == CHIMI_TYPE_CONSOLE && node->c == 0)
      inner->host = HOST_WRITE_LINE;
    else if (standard_type == CHIMI_TYPE_CONSOLE && node->c == 1)
      inner->host = HOST_WRITE_ERROR;
    else { result = defect("closed core has no provider", node_id); break; }
  } else {
    ChimiValue *method = select_value(provider, CHIMI_STRINGS[node->d]);
    if (!method || method->kind != VALUE_CALLABLE) {
      result = defect("provider has no callable field", node_id); break;
    }
    inner = method->as.callable;
  }
  ChimiCallable *callable = allocate(1, sizeof(*callable));
  allocation_kind(callable, ALLOCATION_CALLABLE); callable->kind = CALL_DEPENDENCY;
  callable->inner = inner; callable->requirement = node->a; callable->interface_symbol = node->b;
  callable->method = node->c; callable->name = CHIMI_STRINGS[node->d]; result = success(callable_value(callable));
})

static ChimiControl *construct_value(uint32_t symbol, ChimiValue **values) {
  const ChimiConstructor *shape = &CHIMI_CONSTRUCTORS[CHIMI_SYMBOLS[symbol].constructor];
  if (shape->outcome_success >= 0) return shape->outcome_success ? success(values[0]) : failure(values[0]);
  ChimiValue *value = new_value(VALUE_CONSTRUCTED); value->as.constructed.constructor = symbol;
  value->as.constructed.type_name = CHIMI_STRINGS[shape->type_name];
  value->as.constructed.variant_name = shape->variant_name == UINT32_MAX ? NULL : CHIMI_STRINGS[shape->variant_name];
  value->as.constructed.length = shape->fields_length;
  value->as.constructed.fields = allocate(shape->fields_length, sizeof(*value->as.constructed.fields));
  allocation_kind(value->as.constructed.fields, ALLOCATION_FIELDS);
  for (uint32_t field = 0; field < shape->fields_length; ++field) {
    const char *name = CHIMI_STRINGS[CHIMI_IDS[shape->fields_offset + field]];
    value->as.constructed.fields[field] = (RuntimeField){name, values[field]};
  }
  return success(value);
}

NODE_WRAPPER(construct, "construct", {
  const ChimiNode *node = &CHIMI_NODES[node_id];
  ChimiValue **values = allocate(node->length, sizeof(*values));
  bool stopped = false;
  for (uint32_t index = 0; index < node->length; ++index) {
    ChimiControl *control = RUN(CHIMI_IDS[node->offset + index]);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    values[index] = control->value;
  }
  if (stopped) break;
  result = construct_value(node->a, values);
})

NODE_WRAPPER(call, "call", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiControl *callee = RUN(node->a);
  if (callee->kind != CONTROL_SUCCESS) { result = callee; break; }
  if (callee->value->kind != VALUE_CALLABLE) { result = defect("verified call target is not callable", node_id); break; }
  ChimiValue **arguments = allocate(node->length, sizeof(*arguments)); bool stopped = false;
  for (uint32_t index = 0; index < node->length; ++index) {
    ChimiControl *control = RUN(CHIMI_IDS[node->offset + index]);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    arguments[index] = control->value;
  }
  if (!stopped) result = invoke(runtime, callee->value->as.callable, arguments, node->length, span, node_id, environment);
})

NODE_WRAPPER(list, "list", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiValue **items = new_value_slots(node->length);
  bool stopped = false; for (uint32_t i = 0; i < node->length; ++i) {
    ChimiControl *control = RUN(CHIMI_IDS[node->offset + i]);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    items[i] = control->value;
  } if (!stopped) result = success(list_from_values(items, node->length));
})

NODE_WRAPPER(record, "record", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiValue *value = new_value(VALUE_RECORD);
  value->as.record.length = node->length; value->as.record.fields = allocate(node->length, sizeof(*value->as.record.fields));
  allocation_kind(value->as.record.fields, ALLOCATION_FIELDS);
  bool stopped = false; for (uint32_t i = 0; i < node->length; ++i) {
    const ChimiNamedNode *field = &CHIMI_NAMED_NODES[node->offset + i]; ChimiControl *control = RUN(field->node);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    value->as.record.fields[i] = (RuntimeField){CHIMI_STRINGS[field->name], control->value};
  } if (!stopped) result = success(value);
})

NODE_WRAPPER(unary, "unary", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiControl *control = RUN(node->a);
  if (control->kind != CONTROL_SUCCESS) { result = control; break; }
  if (control->value->kind == VALUE_FLOAT) { result = success(float_value(-control->value->as.floating)); break; }
  if (control->value->kind != VALUE_INTEGER) { result = defect("verified unary operand is not an integer", node_id); break; }
  if (control->value->as.integer == INT64_MIN) {
    result = defect(numeric_message("integer overflow in unary `-`: -(%" PRId64 ")", INT64_MIN), node_id); break;
  }
  result = success(integer_value(-control->value->as.integer));
})

NODE_WRAPPER(binary, "binary", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiControl *left = RUN(node->b);
  if (left->kind != CONTROL_SUCCESS) { result = left; break; }
  ChimiControl *right = RUN(node->c); if (right->kind != CONTROL_SUCCESS) { result = right; break; }
  if (is_arithmetic(node->a)) {
    if (left->value->kind == VALUE_INTEGER && right->value->kind == VALUE_INTEGER)
      result = integer_arithmetic(node->a, left->value->as.integer, right->value->as.integer, node_id);
    else if (left->value->kind == VALUE_FLOAT && right->value->kind == VALUE_FLOAT)
      result = float_arithmetic(node->a, left->value->as.floating, right->value->as.floating, node_id);
    else
      result = defect("verified binary operands are not numbers", node_id);
    break;
  }
  int order = chimi_value_compare(left->value, right->value);
  switch (node->a) {
    case CHIMI_BINARY_EQUAL: result = success(boolean_value(order == 0)); break;
    case CHIMI_BINARY_LESS: result = success(boolean_value(order < 0)); break;
    case CHIMI_BINARY_LESS_EQUAL: result = success(boolean_value(order <= 0)); break;
    case CHIMI_BINARY_GREATER: result = success(boolean_value(order > 0)); break;
    case CHIMI_BINARY_GREATER_EQUAL: result = success(boolean_value(order >= 0)); break;
    default: result = defect("verified binary operator is unknown", node_id); break;
  }
})

NODE_WRAPPER(identity, "identity", { result = RUN(CHIMI_NODES[node_id].a); })

NODE_WRAPPER(provision, "provision", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; bool stopped = false;
  environment = clone_requirement_environment(incoming);
  for (uint32_t i = 0; i < node->length; ++i) {
    const ChimiNamedNode *provider = &CHIMI_NAMED_NODES[node->offset + i]; ChimiControl *control = RUN(provider->node);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    const char *name = CHIMI_STRINGS[provider->name];
    for (uint32_t symbol = 0; symbol < CHIMI_SYMBOL_COUNT; ++symbol)
      if (CHIMI_SYMBOLS[symbol].kind == CHIMI_SYMBOL_REQUIREMENT &&
          strcmp(CHIMI_STRINGS[CHIMI_SYMBOLS[symbol].name], name) == 0)
        environment->requirements[CHIMI_SYMBOLS[symbol].environment_slot] = control->value;
  } if (!stopped) result = RUN(node->a);
})

NODE_WRAPPER(sequence, "sequence", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiValue *last = NULL; bool stopped = false;
  environment = clone_local_environment(incoming);
  for (uint32_t i = 0; i < node->length; ++i) {
    const ChimiStep *step = &CHIMI_STEPS[node->offset + i]; ChimiControl *control = RUN(step->node);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    last = control->value;
    if (step->kind == 0) environment->locals[environment_slot(step->symbol)] = last;
  } if (!stopped) result = success(last);
})

NODE_WRAPPER(closure, "closure", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiCallable *callable = allocate(1, sizeof(*callable));
  allocation_kind(callable, ALLOCATION_CALLABLE);
  callable->kind = CALL_CLOSURE; callable->body = node->a; callable->parameters_offset = node->offset;
  callable->parameters_length = node->length;
  callable->environment = capture_closure_environment(environment, node);
  callable->name = "closure"; result = success(callable_value(callable));
})

typedef enum { MATCH_PLAIN, MATCH_SUCCESS, MATCH_FAILURE } MatchKind;
typedef struct { MatchKind kind; ChimiValue *value; } MatchValue;

static bool literal_matches(const ChimiPattern *pattern, const ChimiValue *value) {
  if (pattern->a == CHIMI_LITERAL_INTEGER) return value->kind == VALUE_INTEGER && value->as.integer == pattern->integer;
  if (pattern->a == CHIMI_LITERAL_TEXT) return value->kind == VALUE_TEXT && strcmp(value->as.text, CHIMI_STRINGS[pattern->offset]) == 0;
  return value->kind == VALUE_BOOLEAN && value->as.boolean == (pattern->integer != 0);
}
/* The binding pass of a match. `view_failed` records a list view that could not be
   taken, which the match reports as a defect. */
typedef struct { ChimiEnvironment *environment; bool view_failed; } PatternBinder;

static bool match_plain(uint32_t pattern_id, ChimiValue *value, PatternBinder *binder);
/* Checks the length first, then reads each fixed element by index. In the binding
   pass a named rest receives the view between the prefix and the suffix. */
static bool match_list(const ChimiPattern *pattern, ChimiValue *value, PatternBinder *binder) {
  if (value->kind != VALUE_LIST) return false;
  uint32_t length = list_length(value), prefix = pattern->a, has_rest = pattern->integer != 0;
  uint32_t suffix = pattern->length - prefix - has_rest;
  if (has_rest ? length < prefix + suffix : length != prefix) return false;
  uint32_t suffix_start = length - suffix;
  for (uint32_t index = 0; index < prefix; ++index)
    if (!match_plain(CHIMI_IDS[pattern->offset + index], list_get(value, index), binder)) return false;
  for (uint32_t index = 0; index < suffix; ++index)
    if (!match_plain(CHIMI_IDS[pattern->offset + prefix + has_rest + index],
                     list_get(value, suffix_start + index), binder))
      return false;
  if (!has_rest || !binder) return true;
  uint32_t rest_id = CHIMI_IDS[pattern->offset + prefix];
  if (CHIMI_PATTERNS[rest_id].kind == CHIMI_PATTERN_WILDCARD) return true;
  ChimiValue *view = list_slice(value, prefix, suffix_start);
  if (!view) { binder->view_failed = true; return false; }
  return match_plain(rest_id, view, binder);
}
static bool match_plain(uint32_t pattern_id, ChimiValue *value, PatternBinder *binder) {
  const ChimiPattern *pattern = &CHIMI_PATTERNS[pattern_id];
  switch (pattern->kind) {
    case CHIMI_PATTERN_WILDCARD: return true;
    case CHIMI_PATTERN_BINDING:
      if (binder) binder->environment->locals[environment_slot(pattern->a)] = value;
      return true;
    case CHIMI_PATTERN_LITERAL: return literal_matches(pattern, value);
    case CHIMI_PATTERN_CONSTRUCTOR:
      if (value->kind != VALUE_CONSTRUCTED || value->as.constructed.constructor != pattern->a ||
          value->as.constructed.length != pattern->length) return false;
      for (uint32_t i = 0; i < pattern->length; ++i)
        if (!match_plain(CHIMI_IDS[pattern->offset + i], value->as.constructed.fields[i].value, binder)) return false;
      return true;
    case CHIMI_PATTERN_OUTCOME: return false;
    case CHIMI_PATTERN_LIST: return match_list(pattern, value, binder);
  }
  return false;
}
static bool match_value(uint32_t pattern_id, MatchValue value, PatternBinder *binder) {
  const ChimiPattern *pattern = &CHIMI_PATTERNS[pattern_id];
  if (pattern->kind == CHIMI_PATTERN_OUTCOME) {
    MatchKind expected = pattern->a ? MATCH_SUCCESS : MATCH_FAILURE;
    return value.kind == expected && match_plain(pattern->offset, value.value, binder);
  }
  return value.kind == MATCH_PLAIN && match_plain(pattern_id, value.value, binder);
}
static bool pattern_binds(uint32_t pattern_id) {
  const ChimiPattern *pattern = &CHIMI_PATTERNS[pattern_id];
  if (pattern->kind == CHIMI_PATTERN_BINDING) return true;
  if (pattern->kind == CHIMI_PATTERN_OUTCOME) return pattern_binds(pattern->offset);
  if (pattern->kind != CHIMI_PATTERN_CONSTRUCTOR && pattern->kind != CHIMI_PATTERN_LIST) return false;
  for (uint32_t index = 0; index < pattern->length; ++index)
    if (pattern_binds(CHIMI_IDS[pattern->offset + index])) return true;
  return false;
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
    const ChimiArm *arm = &CHIMI_ARMS[node->a + arm_index]; bool matched = true;
    for (uint32_t subject = 0; subject < node->length; ++subject)
      if (!match_value(CHIMI_IDS[arm->patterns_offset + subject], values[subject], NULL)) { matched = false; break; }
    if (!matched) continue;
    bool binds = false;
    for (uint32_t subject = 0; subject < node->length; ++subject)
      if (pattern_binds(CHIMI_IDS[arm->patterns_offset + subject])) { binds = true; break; }
    ChimiEnvironment *arm_environment = binds ? clone_local_environment(environment) : environment;
    PatternBinder binder = {arm_environment, false};
    for (uint32_t subject = 0; binds && subject < node->length; ++subject)
      if (!match_value(CHIMI_IDS[arm->patterns_offset + subject], values[subject], &binder)) {
        result = defect(binder.view_failed ? "verified list pattern took a view outside its list"
                                           : "verified pattern changed while binding", node_id);
        stopped = true; break;
      }
    if (stopped) break;
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

/* Runs passes in a C loop so stack use is independent of the pass count. The loop owns
   one cloned environment and rebinds only its state slot between passes; that is safe
   because closures copy captured slots and binding nodes clone before writing. Each
   pass checks cancellation first and runs under this node's span, so pass operations
   are siblings.

   Without a scheduler, a pass's allocations are reclaimed once it ends, keeping only
   what the next state reaches. Values are immutable and the loop environment lives in
   the enclosing scope, so the state is the only path from one pass to the next. With a
   scheduler this is unsafe: Channel.send hands the scheduler an unmarked pointer, and
   fibers share one current scope (prog show ts-da83af). */
NODE_WRAPPER(loop, "loop", {
  const ChimiNode *node = &CHIMI_NODES[node_id];
  ChimiControl *initial = RUN(node->b);
  if (initial->kind != CONTROL_SUCCESS) { result = initial; break; }
  environment = clone_local_environment(incoming);
  uint32_t state_slot = environment_slot(node->a);
  ChimiValue *state = initial->value;
  bool reclaims = !CHIMI_REQUIRES_SCHEDULER;
  AllocationScope passes;
  if (reclaims) allocation_scope_begin(&passes);
  for (;;) {
    if (runtime->cancel) { result = cancelled(); break; }
    environment->locals[state_slot] = state;
    ChimiControl *pass = RUN(node->c);
    if (pass->kind == CONTROL_SUCCESS) {
      state = pass->value;
      if (reclaims) allocation_scope_collect(&passes, state, NULL);
      continue;
    }
    if (pass->kind == CONTROL_BREAK && pass->node == (int64_t)node->a) {
      result = success(pass->value);
    } else {
      result = pass;
    }
    break;
  }
  if (reclaims) {
    allocation_scope_collect(&passes, NULL, result);
    end_measured_allocation_scope(runtime, &passes);
  }
})
NODE_WRAPPER(break, "break", {
  const ChimiNode *node = &CHIMI_NODES[node_id];
  ChimiValue *value = void_value();
  if (node->b != UINT32_MAX) {
    ChimiControl *control = RUN(node->b);
    if (control->kind != CONTROL_SUCCESS) { result = control; break; }
    value = control->value;
  }
  result = new_control(CONTROL_BREAK, value, NULL, node->a);
})

static void print_outcome(FILE *output, const ChimiControl *control) {
  fputs("{\"schema\":\"chimi.runtime.v1\",", output);
  switch (control->kind) {
    case CONTROL_SUCCESS:
      fputs("\"outcome\":\"success\",\"value\":", output); json_value(output, control->value); break;
    case CONTROL_FAILURE:
      fputs("\"outcome\":\"expected_failure\",\"error\":", output); json_value(output, control->value); break;
    case CONTROL_DEFECT:
      fputs("\"outcome\":\"defect\",\"message\":", output); json_string(output, control->message);
      if (control->node < 0) fputs(",\"core_node_id\":null,\"origin_id\":null", output);
      else fprintf(output, ",\"core_node_id\":%" PRId64 ",\"origin_id\":%u",
                   control->node, CHIMI_NODES[control->node].origin);
      break;
    case CONTROL_CANCEL:
      fputs("\"outcome\":\"cancellation\",\"reason\":\"requested before the next semantic operation\"", output); break;
  }
  fputs("}\n", output);
}

static bool close_output(FILE *output) {
  if (output == NULL) return false;
  bool failed = fflush(output) != 0;
  if (fclose(output) != 0) failed = true;
  return failed;
}

static bool write_memory_report(const char *path, const ChimiRuntime *runtime) {
  if (!path) return false;
  FILE *output = fopen(path, "wx");
  if (!output) {
    fprintf(stderr, "cannot open memory report file %s: %s\n", path, strerror(errno));
    return true;
  }
  fprintf(output,
          "{\"schema\":\"chimi.runtime-memory.v1\",\"scoped_total_allocated_bytes\":%zu,"
          "\"scoped_allocated_bytes_by_kind\":{\"bytes\":%zu,\"values\":%zu,"
          "\"controls\":%zu,\"environments\":%zu,\"value_pointers\":%zu,"
          "\"fields\":%zu,\"callables\":%zu,\"map_nodes\":%zu,\"list_nodes\":%zu},"
          "\"scoped_peak_live_bytes\":%zu,\"scoped_retained_bytes\":%zu,"
          "\"scoped_collections\":%" PRIu64 "}\n",
          runtime->scoped_total_allocated_bytes,
          runtime->scoped_allocated_bytes_by_kind[ALLOCATION_BYTES],
          runtime->scoped_allocated_bytes_by_kind[ALLOCATION_VALUE],
          runtime->scoped_allocated_bytes_by_kind[ALLOCATION_CONTROL],
          runtime->scoped_allocated_bytes_by_kind[ALLOCATION_ENVIRONMENT],
          runtime->scoped_allocated_bytes_by_kind[ALLOCATION_VALUE_POINTERS],
          runtime->scoped_allocated_bytes_by_kind[ALLOCATION_FIELDS],
          runtime->scoped_allocated_bytes_by_kind[ALLOCATION_CALLABLE],
          runtime->scoped_allocated_bytes_by_kind[ALLOCATION_MAP_NODE],
          runtime->scoped_allocated_bytes_by_kind[ALLOCATION_LIST_NODE],
          runtime->scoped_peak_live_bytes,
          runtime->scoped_retained_bytes,
          runtime->scoped_collections);
  return close_output(output);
}

static const char *scheduler_event_name(const ChimiSchedulerTraceEvent *event) {
  static const char *task_names[] = {
    "task_started", "task_parked", "task_woken", "task_cancellation_requested",
    "task_cleanup_completed", "task_terminal",
  };
  static const char *channel_names[] = {
    "channel_created", "channel_send_accepted", "channel_send_parked",
    "channel_send_rejected_closed", "channel_send_rejected_aborted",
    "channel_send_cancelled", "channel_receive_delivered", "channel_receive_parked",
    "channel_receive_cancelled", "channel_closed", "channel_drained",
    "channel_aborted", "channel_receiver_transferred", "channel_completion_transferred",
  };
  if (event->family == CHIMI_SCHEDULER_TRACE_TASK &&
      event->kind > 0 && event->kind <= sizeof(task_names) / sizeof(*task_names))
    return task_names[event->kind - 1];
  if (event->family == CHIMI_SCHEDULER_TRACE_CHANNEL &&
      event->kind > 0 && event->kind <= sizeof(channel_names) / sizeof(*channel_names))
    return channel_names[event->kind - 1];
  return "scheduler_event";
}

static void emit_scheduler_event(void *context, const ChimiSchedulerTraceEvent *event) {
  ChimiRuntime *runtime = context;
  FILE *output = runtime->telemetry;
  fputs("{\"schema\":\"chimi.runtime.v1\",\"event\":", output);
  json_string(output, scheduler_event_name(event));
  fprintf(output, ",\"sequence\":%" PRIu64 ",\"computation\":", runtime->next_sequence++);
  json_string(output, CHIMI_COMPUTATION);
  fputs(",\"execution_id\":", output);
  json_string(output, runtime->execution_id);
  fputs(",\"task_id\":", output);
  if (event->task == 0) fputs("null", output); else fprintf(output, "%" PRIu64, event->task);
  fputs(",\"parent_task_id\":", output);
  if (event->parent == 0) fputs("null", output); else fprintf(output, "%" PRIu64, event->parent);
  fputs(",\"cause_task_id\":", output);
  if (event->cause == 0) fputs("null", output); else fprintf(output, "%" PRIu64, event->cause);
  fputs(",\"channel_id\":", output);
  if (event->channel == 0) fputs("null", output); else fprintf(output, "%" PRIu64, event->channel);
  fprintf(output,
          ",\"attributes\":{\"scheduler_sequence\":%" PRIu64
          ",\"capacity\":%" PRIu64 ",\"queue_depth\":%" PRIu64
          ",\"waiter_depth\":%" PRIu64 ",\"discard_count\":%" PRIu64 "}}\n",
          event->sequence, event->capacity, event->queue_depth,
          event->waiter_depth, event->discard_count);
}

static ChimiControl *execute_program_entry(ChimiRuntime *runtime, ChimiNodeFn entry,
                                           ChimiEnvironment *environment) {
  if (!CHIMI_REQUIRES_SCHEDULER) return entry(runtime, environment, 1);
  ChimiTaskContext context = {
    .runtime = runtime,
    .environment = environment,
    .entry = entry,
    .span = 1,
  };
  uint32_t trace_mode = runtime->silent ? 0 : (runtime->full_trace ? 2 : 1);
  ChimiSchedulerOutcome outcome = chimi_scheduler_execute(
    chimi_task_entry, &context, 64u * 1024u, trace_mode,
    emit_scheduler_event, runtime);
  if (outcome.state == CHIMI_FIBER_SUCCEEDED)
    return (ChimiControl *)(intptr_t)outcome.value;
  if (outcome.state == CHIMI_FIBER_CANCELLED) return cancelled();
  if (outcome.state == CHIMI_FIBER_DEFECTED && outcome.defect_code != 0)
    return (ChimiControl *)(intptr_t)outcome.defect_code;
  if (outcome.state == CHIMI_FIBER_STACK_OVERFLOW)
    return defect("Task stack overflow", -1);
  return defect("Task scheduler failed to produce a terminal root", -1);
}

int32_t chimi_rt_main(ChimiNodeFn entry, int32_t argc, const char **argv) {
  struct timespec identity_time;
  clock_gettime(CLOCK_REALTIME, &identity_time);
  char generated_execution_id[96];
  snprintf(generated_execution_id, sizeof(generated_execution_id),
           "execution-%ld-%" PRId64 "-%ld", (long)getpid(),
           (int64_t)identity_time.tv_sec, identity_time.tv_nsec);
  ChimiRuntime runtime = {
    .execution_id = generated_execution_id,
    .silent = true,
    .max_line_bytes = 1024 * 1024,
    .next_span = 2,
  };
  uint32_t benchmark_repeats = 0;
  const char *telemetry_path = NULL;
  const char *outcome_path = NULL;
  const char *memory_report_path = NULL;
  decode_constants();
  int32_t index = 1;
  if (index < argc && strcmp(argv[index], "--chimi-runner") == 0) {
    ++index;
    bool found_boundary = false;
    for (; index < argc; ++index) {
      if (strcmp(argv[index], "--") == 0) {
        found_boundary = true;
        ++index;
        break;
      }
    if (strcmp(argv[index], "--trace=full") == 0) { runtime.full_trace = true; runtime.silent = false; }
    else if (strcmp(argv[index], "--trace=boundaries") == 0) { runtime.full_trace = false; runtime.silent = false; }
    else if (strcmp(argv[index], "--trace=off") == 0) { runtime.full_trace = false; runtime.silent = true; }
    else if (strncmp(argv[index], "--telemetry-file=", 17) == 0) telemetry_path = argv[index] + 17;
    else if (strncmp(argv[index], "--outcome-file=", 15) == 0) outcome_path = argv[index] + 15;
    else if (strncmp(argv[index], "--memory-report-file=", 21) == 0) memory_report_path = argv[index] + 21;
    else if (strncmp(argv[index], "--execution-id=", 15) == 0) runtime.execution_id = argv[index] + 15;
    else if (strcmp(argv[index], "--cancel") == 0) runtime.cancel = true;
    else if (strncmp(argv[index], "--max-line-bytes=", 17) == 0) {
      errno = 0;
      char *end = NULL;
      unsigned long value = strtoul(argv[index] + 17, &end, 10);
      if (errno == ERANGE || end == argv[index] + 17 || *end != '\0' ||
          value == 0 || value > UINT32_MAX) {
        fputs("--max-line-bytes requires an integer from 1 through 4294967295\n", stderr);
        return 64;
      }
      runtime.max_line_bytes = (uint32_t)value;
    }
    else if (strncmp(argv[index], "--benchmark-repeats=", 20) == 0)
      benchmark_repeats = (uint32_t)strtoul(argv[index] + 20, NULL, 10);
    else { fprintf(stderr, "unknown Chimi runtime argument: %s\n", argv[index]); return 64; }
    }
    if (!found_boundary) {
      fputs("Chimi runner configuration must end with --\n", stderr);
      return 64;
    }
  }
  if (!runtime.silent && telemetry_path == NULL) {
    fputs("boundary or full tracing requires --telemetry-file\n", stderr);
    return 64;
  }
  if (telemetry_path && outcome_path && strcmp(telemetry_path, outcome_path) == 0) {
    fputs("telemetry and runner outcome require distinct files\n", stderr);
    return 64;
  }
  if ((memory_report_path && telemetry_path && strcmp(memory_report_path, telemetry_path) == 0) ||
      (memory_report_path && outcome_path && strcmp(memory_report_path, outcome_path) == 0)) {
    fputs("memory report, telemetry, and runner outcome require distinct files\n", stderr);
    return 64;
  }
  if (telemetry_path) {
    runtime.telemetry = fopen(telemetry_path, strcmp(telemetry_path, "/dev/null") == 0 ? "w" : "wx");
    if (!runtime.telemetry) {
      fprintf(stderr, "cannot open telemetry file %s: %s\n", telemetry_path, strerror(errno));
      return 74;
    }
  }
  if (outcome_path) {
    runtime.outcome = fopen(outcome_path, "wx");
    if (!runtime.outcome) {
      fprintf(stderr, "cannot open outcome file %s: %s\n", outcome_path, strerror(errno));
      if (runtime.telemetry) fclose(runtime.telemetry);
      return 74;
    }
  }
  uint32_t entry_node = 0;
  const ChimiFunction *entry_function = NULL;
  for (uint32_t i = 0; i < CHIMI_FUNCTION_COUNT; ++i)
    if (CHIMI_NODE_FUNCTIONS[CHIMI_FUNCTIONS[i].body] == entry) {
      entry_node = CHIMI_FUNCTIONS[i].body;
      entry_function = &CHIMI_FUNCTIONS[i];
      break;
    }
  if (entry_function == NULL) return 70;
  if (entry_function->parameters_length == 0 && index < argc) {
    fputs("program entry does not accept application arguments\n", stderr);
    return 64;
  }
  if (benchmark_repeats > 0) {
    struct timespec started, finished;
    clock_gettime(CLOCK_MONOTONIC, &started);
    for (uint32_t repeat = 0; repeat < benchmark_repeats; ++repeat) {
      runtime.next_sequence = 0;
      runtime.next_span = 2;
      if (!runtime.silent) emit_event(&runtime, "computation_started", entry_node, 1, -1, "computation", NULL);
      ChimiEnvironment *environment = new_environment();
      if (entry_function->parameters_length == 1) {
        ChimiValue *arguments = application_arguments(argc - index, argv + index);
        environment->locals[environment_slot(CHIMI_IDS[entry_function->parameters_offset])] = arguments;
      }
      ChimiControl *control = runtime.cancel
        ? cancelled()
        : execute_program_entry(&runtime, entry, environment);
      if (control == NULL) return 70;
      if (!runtime.silent) emit_event(&runtime, "computation_completed", entry_node, 1, -1, "computation", control);
    }
    bool write_failed = write_memory_report(memory_report_path, &runtime);
    write_failed = close_output(runtime.telemetry) || write_failed;
    runtime.telemetry = NULL;
    write_failed = close_output(runtime.outcome) || write_failed;
    runtime.outcome = NULL;
    clock_gettime(CLOCK_MONOTONIC, &finished);
    int64_t elapsed = (int64_t)(finished.tv_sec - started.tv_sec) * INT64_C(1000000000)
                    + (int64_t)finished.tv_nsec - (int64_t)started.tv_nsec;
    fprintf(stderr, "%" PRIu64 "\n", elapsed / benchmark_repeats);
    return write_failed ? 74 : 0;
  }
  if (!runtime.silent) emit_event(&runtime, "computation_started", entry_node, 1, -1, "computation", NULL);
  ChimiEnvironment *environment = new_environment();
  if (entry_function->parameters_length == 1) {
    ChimiValue *arguments = application_arguments(argc - index, argv + index);
    environment->locals[environment_slot(CHIMI_IDS[entry_function->parameters_offset])] = arguments;
  }
  ChimiControl *control = runtime.cancel
    ? cancelled()
    : execute_program_entry(&runtime, entry, environment);
  if (!runtime.silent) emit_event(&runtime, "computation_completed", entry_node, 1, -1, "computation", control);
  if (runtime.outcome) print_outcome(runtime.outcome, control);
  bool write_failed = write_memory_report(memory_report_path, &runtime);
  write_failed = close_output(runtime.telemetry) || write_failed;
  write_failed = close_output(runtime.outcome) || write_failed;
  if (write_failed) return 74;
  switch (control->kind) {
    case CONTROL_SUCCESS: return 0;
    case CONTROL_FAILURE: return 1;
    case CONTROL_DEFECT: return 70;
    case CONTROL_CANCEL: return 130;
  }
  return 70;
}
