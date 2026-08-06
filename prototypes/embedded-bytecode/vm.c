/* PROTOTYPE: disposable embedded-bytecode dispatcher for the M8 comparison. */
#include "runtime.h"

#include <stdint.h>
#include <stdlib.h>

extern const uint8_t CHIMI_OPCODES[];
extern const uint32_t CHIMI_ENTRY_NODE;

enum {
  CHIMI_OPCODE_CONSTANT,
  CHIMI_OPCODE_REFERENCE,
  CHIMI_OPCODE_SELECT,
  CHIMI_OPCODE_DEPENDENCY_METHOD,
  CHIMI_OPCODE_CONSTRUCT,
  CHIMI_OPCODE_CALL,
  CHIMI_OPCODE_LIST,
  CHIMI_OPCODE_RECORD,
  CHIMI_OPCODE_UNARY,
  CHIMI_OPCODE_BINARY,
  CHIMI_OPCODE_IDENTITY,
  CHIMI_OPCODE_PROVISION,
  CHIMI_OPCODE_SEQUENCE,
  CHIMI_OPCODE_CLOSURE,
  CHIMI_OPCODE_MATCH,
};

ChimiControl *chimi_bytecode_eval(uint32_t node, ChimiRuntime *runtime,
                                  ChimiEnvironment *environment,
                                  uint64_t parent_span) {
  switch (CHIMI_OPCODES[node]) {
  case CHIMI_OPCODE_CONSTANT:
    return chimi_rt_constant(runtime, node, environment, parent_span);
  case CHIMI_OPCODE_REFERENCE:
    return chimi_rt_reference(runtime, node, environment, parent_span);
  case CHIMI_OPCODE_SELECT:
    return chimi_rt_select(runtime, node, environment, parent_span);
  case CHIMI_OPCODE_DEPENDENCY_METHOD:
    return chimi_rt_dependency_method(runtime, node, environment, parent_span);
  case CHIMI_OPCODE_CONSTRUCT:
    return chimi_rt_construct(runtime, node, environment, parent_span);
  case CHIMI_OPCODE_CALL:
    return chimi_rt_call(runtime, node, environment, parent_span);
  case CHIMI_OPCODE_LIST:
    return chimi_rt_list(runtime, node, environment, parent_span);
  case CHIMI_OPCODE_RECORD:
    return chimi_rt_record(runtime, node, environment, parent_span);
  case CHIMI_OPCODE_UNARY:
    return chimi_rt_unary(runtime, node, environment, parent_span);
  case CHIMI_OPCODE_BINARY:
    return chimi_rt_binary(runtime, node, environment, parent_span);
  case CHIMI_OPCODE_IDENTITY:
    return chimi_rt_identity(runtime, node, environment, parent_span);
  case CHIMI_OPCODE_PROVISION:
    return chimi_rt_provision(runtime, node, environment, parent_span);
  case CHIMI_OPCODE_SEQUENCE:
    return chimi_rt_sequence(runtime, node, environment, parent_span);
  case CHIMI_OPCODE_CLOSURE:
    return chimi_rt_closure(runtime, node, environment, parent_span);
  case CHIMI_OPCODE_MATCH:
    return chimi_rt_match(runtime, node, environment, parent_span);
  default:
    abort();
  }
}

int main(int argc, const char **argv) {
  return chimi_rt_main(CHIMI_NODE_FUNCTIONS[CHIMI_ENTRY_NODE], argc, argv);
}
