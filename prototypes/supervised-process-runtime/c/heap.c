#include "heap.h"

#include <stdio.h>
#include <stdlib.h>

#define UNIMPLEMENTED() \
  do { fprintf(stderr, "unimplemented: %s\n", __func__); abort(); } while (0)

ChimiHeap *chimi_heap_create(uint64_t owner) { (void)owner; UNIMPLEMENTED(); }
void chimi_heap_destroy(ChimiHeap *heap) { (void)heap; UNIMPLEMENTED(); }
ChimiHeap *chimi_heap_switch(ChimiHeap *heap) { (void)heap; UNIMPLEMENTED(); }

ChimiValue *chimi_value_int(int64_t value) { (void)value; UNIMPLEMENTED(); }
ChimiValue *chimi_value_text(const char *bytes, size_t length) {
  (void)bytes; (void)length; UNIMPLEMENTED();
}
ChimiValue *chimi_value_items(ChimiValue *const *items, uint32_t length) {
  (void)items; (void)length; UNIMPLEMENTED();
}

void chimi_region_enter(void) { UNIMPLEMENTED(); }
ChimiHeapStatus chimi_region_pass(ChimiValue **state) { (void)state; UNIMPLEMENTED(); }
void chimi_region_exit(void) { UNIMPLEMENTED(); }

ChimiHeapStatus chimi_fragment_copy(const ChimiValue *root, ChimiFragment **out) {
  (void)root; (void)out; UNIMPLEMENTED();
}
ChimiValue *chimi_fragment_adopt(ChimiFragment *fragment) { (void)fragment; UNIMPLEMENTED(); }
void chimi_fragment_free(ChimiFragment *fragment) { (void)fragment; UNIMPLEMENTED(); }

uint64_t chimi_value_owner(const ChimiValue *value) { (void)value; UNIMPLEMENTED(); }
ChimiHeapStats chimi_heap_stats(const ChimiHeap *heap) { (void)heap; UNIMPLEMENTED(); }
ChimiChunkStats chimi_chunk_stats(void) { UNIMPLEMENTED(); }
