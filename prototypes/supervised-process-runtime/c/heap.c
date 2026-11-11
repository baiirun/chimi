/* PROTOTYPE: disposable evidence for `prog show ts-a6b43b`.
 *
 * Representation:
 * - Memory comes in chunks aligned to CHUNK_BYTES. A chunk header records the
 *   owner tag, the region, and the role of every value in it, so finding a
 *   value's owner is one mask and one load.
 * - An object larger than LARGE_OBJECT_BYTES gets its own aligned chunk with
 *   the value at the start, so the mask still finds the header. A collection
 *   promotes such an object by relinking its chunk, never by copying it.
 * - A value is laid out as its ChimiValue header followed inline by its text
 *   bytes or item array, so copying a value is one memcpy plus one pointer fix.
 * - An area is a set of chunks plus one bump chunk. Each bump chunk links to
 *   the bump chunk that replaced it, so a Cheney scan can follow new copies in
 *   allocation order. */
#include "heap.h"

#include <assert.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define CHUNK_BYTES ((size_t)64 * 1024)
#define CHUNK_MASK (~(uintptr_t)(CHUNK_BYTES - 1))
#define HEADER_BYTES ((size_t)64)
#define LARGE_OBJECT_BYTES (CHUNK_BYTES / 4)
/* Empty standard chunks kept per thread instead of returned to the allocator.
 * Without it every message pays an aligned 64 KiB allocation and release. */
#define CHUNK_CACHE_LIMIT 64

/* A from-space value whose copy lives at `as.items`. Never a valid kind. */
#define FORWARDED_KIND 0

typedef enum {
  ROLE_BASE = 1,
  ROLE_YOUNG = 2,
  ROLE_SURVIVOR = 3,
  /* Survivors being evacuated by a full collection. */
  ROLE_FROM = 4,
  ROLE_FRAGMENT = 5,
} ChunkRole;

typedef struct Region Region;
typedef struct Chunk Chunk;

struct Chunk {
  uint64_t owner;
  Region *region;
  Chunk *previous;
  Chunk *next;
  Chunk *successor;
  size_t size;
  size_t used;
  uint32_t role;
  uint32_t large;
};
_Static_assert(sizeof(Chunk) <= HEADER_BYTES, "chunk header must fit before values");

typedef struct {
  Chunk *chunks;
  Chunk *bump;
  /* Value bytes in this area's chunks. */
  size_t bytes;
} Area;

struct Region {
  Region *outer;
  Area young;
  Area survivors;
  size_t survivor_bytes_at_last_full;
};

struct ChimiHeap {
  uint64_t owner;
  Area base;
  Region *innermost;
  uint32_t open_regions;
  ChimiHeapStats stats;
};

struct ChimiFragment {
  Area area;
  ChimiValue *root;
};

static _Thread_local ChimiHeap *current_heap;
static _Thread_local ChimiChunkStats thread_chunk_stats;
static _Thread_local Chunk *chunk_cache;
static _Thread_local size_t chunk_cache_length;

static void fail(const char *message) {
  fprintf(stderr, "chimi heap: %s\n", message);
  abort();
}

static size_t align8(size_t bytes) { return (bytes + 7) & ~(size_t)7; }

static Chunk *chunk_of(const void *value) {
  return (Chunk *)((uintptr_t)value & CHUNK_MASK);
}

static size_t value_bytes(const ChimiValue *value) {
  switch (value->kind) {
    case CHIMI_VALUE_INT:
      return sizeof(ChimiValue);
    case CHIMI_VALUE_TEXT:
      return sizeof(ChimiValue) + align8((size_t)value->length + 1);
    case CHIMI_VALUE_ITEMS:
      return sizeof(ChimiValue) + sizeof(ChimiValue *) * value->length;
  }
  fail("value has no valid kind");
  return 0;
}

/* Points the inline payload pointer at the copy's own inline payload. */
static void fix_payload(ChimiValue *value) {
  if (value->kind == CHIMI_VALUE_TEXT) value->as.text = (char *)(value + 1);
  if (value->kind == CHIMI_VALUE_ITEMS) value->as.items = (ChimiValue **)(value + 1);
}

static Chunk *chunk_new(size_t value_bytes_needed, bool large, uint64_t owner,
                        Region *region, ChunkRole role) {
  size_t size = CHUNK_BYTES;
  Chunk *chunk = NULL;
  if (large) {
    size = (HEADER_BYTES + value_bytes_needed + CHUNK_BYTES - 1) & CHUNK_MASK;
  } else if (chunk_cache) {
    chunk = chunk_cache;
    chunk_cache = chunk->next;
    --chunk_cache_length;
  }
  if (!chunk) {
    void *memory = NULL;
    if (posix_memalign(&memory, CHUNK_BYTES, size) != 0) fail("out of memory");
    chunk = memory;
  }
  *chunk = (Chunk){
      .owner = owner,
      .region = region,
      .size = size,
      .used = HEADER_BYTES,
      .role = role,
      .large = large,
  };
  ++thread_chunk_stats.live_chunks;
  thread_chunk_stats.live_chunk_bytes += size;
  return chunk;
}

static void chunk_release(Chunk *chunk) {
  --thread_chunk_stats.live_chunks;
  thread_chunk_stats.live_chunk_bytes -= chunk->size;
  if (!chunk->large && chunk_cache_length < CHUNK_CACHE_LIMIT) {
    chunk->next = chunk_cache;
    chunk_cache = chunk;
    ++chunk_cache_length;
    return;
  }
  free(chunk);
}

static void area_push(Area *area, Chunk *chunk) {
  chunk->previous = NULL;
  chunk->next = area->chunks;
  if (area->chunks) area->chunks->previous = chunk;
  area->chunks = chunk;
  area->bytes += chunk->used - HEADER_BYTES;
}

static void area_unlink(Area *area, Chunk *chunk) {
  if (chunk->previous) chunk->previous->next = chunk->next;
  else area->chunks = chunk->next;
  if (chunk->next) chunk->next->previous = chunk->previous;
  if (area->bump == chunk) area->bump = NULL;
  area->bytes -= chunk->used - HEADER_BYTES;
}

static void area_release(Area *area) {
  Chunk *chunk = area->chunks;
  while (chunk) {
    Chunk *next = chunk->next;
    chunk_release(chunk);
    chunk = next;
  }
  *area = (Area){0};
}

/* Moves every chunk of `from` into `to`, retagged; `to` keeps its bump chunk. */
static void area_splice(Area *to, Area *from, uint64_t owner, Region *region,
                        ChunkRole role) {
  Chunk *chunk = from->chunks;
  while (chunk) {
    Chunk *next = chunk->next;
    chunk->owner = owner;
    chunk->region = region;
    chunk->role = role;
    chunk->successor = NULL;
    area_push(to, chunk);
    chunk = next;
  }
  *from = (Area){0};
}

static void area_ensure_bump(Area *area, uint64_t owner, Region *region, ChunkRole role) {
  if (area->bump) return;
  Chunk *chunk = chunk_new(0, false, owner, region, role);
  area_push(area, chunk);
  area->bump = chunk;
}

/* Returns `bytes` of value memory, from a dedicated chunk when the value is large. */
static void *area_alloc(Area *area, size_t bytes, uint64_t owner, Region *region,
                        ChunkRole role) {
  Chunk *chunk;
  if (bytes > LARGE_OBJECT_BYTES) {
    chunk = chunk_new(bytes, true, owner, region, role);
    area_push(area, chunk);
  } else {
    chunk = area->bump;
    if (!chunk || chunk->used + bytes > chunk->size) {
      Chunk *next = chunk_new(0, false, owner, region, role);
      area_push(area, next);
      if (chunk) chunk->successor = next;
      area->bump = next;
      chunk = next;
    }
  }
  void *memory = (char *)chunk + chunk->used;
  chunk->used += bytes;
  area->bytes += bytes;
  return memory;
}

/* Cheney scan over values copied into an area since `scan_start`, plus large
 * values that were promoted or copied whole and wait on a stack. */
typedef struct {
  Chunk *chunk;
  size_t offset;
  ChimiValue **large;
  size_t large_length;
  size_t large_capacity;
} Scan;

static Scan scan_start(const Area *area) {
  return (Scan){.chunk = area->bump, .offset = area->bump->used};
}

static void scan_push_large(Scan *scan, ChimiValue *value) {
  if (scan->large_length == scan->large_capacity) {
    scan->large_capacity = scan->large_capacity ? 2 * scan->large_capacity : 16;
    scan->large = realloc(scan->large, scan->large_capacity * sizeof(*scan->large));
    if (!scan->large) fail("out of memory");
  }
  scan->large[scan->large_length++] = value;
}

static ChimiValue *scan_next(Scan *scan) {
  for (;;) {
    if (scan->offset < scan->chunk->used) {
      ChimiValue *value = (ChimiValue *)((char *)scan->chunk + scan->offset);
      scan->offset += value_bytes(value);
      return value;
    }
    if (scan->chunk->successor) {
      scan->chunk = scan->chunk->successor;
      scan->offset = HEADER_BYTES;
      continue;
    }
    if (scan->large_length) return scan->large[--scan->large_length];
    return NULL;
  }
}

static void scan_finish(Scan *scan) { free(scan->large); }

static void heap_grew(ChimiHeap *heap, size_t bytes) {
  heap->stats.live_bytes += bytes;
  if (heap->stats.live_bytes > heap->stats.peak_live_bytes)
    heap->stats.peak_live_bytes = heap->stats.live_bytes;
}

static void heap_shrank(ChimiHeap *heap, size_t bytes) { heap->stats.live_bytes -= bytes; }

static ChimiHeap *require_current(void) {
  if (!current_heap) fail("no current heap");
  return current_heap;
}

static Area *allocation_area(ChimiHeap *heap, Region **region, ChunkRole *role) {
  *region = heap->innermost;
  *role = *region ? ROLE_YOUNG : ROLE_BASE;
  return *region ? &(*region)->young : &heap->base;
}

static ChimiValue *heap_alloc(size_t bytes) {
  ChimiHeap *heap = require_current();
  Region *region;
  ChunkRole role;
  Area *area = allocation_area(heap, &region, &role);
  ChimiValue *value = area_alloc(area, bytes, heap->owner, region, role);
  heap_grew(heap, bytes);
  return value;
}

ChimiHeap *chimi_heap_create(uint64_t owner) {
  if (owner == 0) fail("owner tag 0 is reserved for fragments");
  ChimiHeap *heap = calloc(1, sizeof(*heap));
  if (!heap) fail("out of memory");
  heap->owner = owner;
  area_ensure_bump(&heap->base, owner, NULL, ROLE_BASE);
  return heap;
}

void chimi_heap_destroy(ChimiHeap *heap) {
  if (!heap) return;
  if (current_heap == heap) current_heap = NULL;
  while (heap->innermost) {
    Region *region = heap->innermost;
    heap->innermost = region->outer;
    area_release(&region->young);
    area_release(&region->survivors);
    free(region);
  }
  area_release(&heap->base);
  free(heap);
}

ChimiHeap *chimi_heap_switch(ChimiHeap *heap) {
  ChimiHeap *previous = current_heap;
  current_heap = heap;
  return previous;
}

ChimiValue *chimi_value_int(int64_t integer) {
  ChimiValue *value = heap_alloc(sizeof(ChimiValue));
  value->kind = CHIMI_VALUE_INT;
  value->length = 0;
  value->as.integer = integer;
  return value;
}

ChimiValue *chimi_value_text(const char *bytes, size_t length) {
  if (length > UINT32_MAX) fail("text longer than 4 GiB");
  ChimiValue *value = heap_alloc(sizeof(ChimiValue) + align8(length + 1));
  value->kind = CHIMI_VALUE_TEXT;
  value->length = (uint32_t)length;
  fix_payload(value);
  if (length) memcpy(value->as.text, bytes, length);
  value->as.text[length] = '\0';
  return value;
}

ChimiValue *chimi_value_items(ChimiValue *const *items, uint32_t length) {
  ChimiValue *value = heap_alloc(sizeof(ChimiValue) + sizeof(ChimiValue *) * length);
  value->kind = CHIMI_VALUE_ITEMS;
  value->length = length;
  fix_payload(value);
  if (length) memcpy(value->as.items, items, sizeof(ChimiValue *) * length);
  return value;
}

void chimi_region_enter(void) {
  ChimiHeap *heap = require_current();
  Region *region = calloc(1, sizeof(*region));
  if (!region) fail("out of memory");
  region->outer = heap->innermost;
  heap->innermost = region;
  ++heap->open_regions;
}

typedef struct {
  ChimiHeap *heap;
  Region *region;
  Area from;
  Scan scan;
  ChimiHeapStatus status;
} Pass;

/* Moves `value` into the survivor area if this pass collects it; returns where
 * it lives now. Values outside the region, or survivors during a minor pass,
 * stay put and are not traversed: immutability means they cannot point to
 * anything this pass moves. */
static ChimiValue *evacuate(Pass *pass, ChimiValue *value) {
  Chunk *chunk = chunk_of(value);
  if (chunk->owner != pass->heap->owner) {
    pass->status = CHIMI_HEAP_FOREIGN_POINTER;
    return value;
  }
  if (chunk->region != pass->region ||
      (chunk->role != ROLE_YOUNG && chunk->role != ROLE_FROM))
    return value;
  if ((int)value->kind == FORWARDED_KIND) return (ChimiValue *)value->as.items;
  Region *region = pass->region;
  if (chunk->large) {
    area_unlink(chunk->role == ROLE_YOUNG ? &region->young : &pass->from, chunk);
    chunk->role = ROLE_SURVIVOR;
    area_push(&region->survivors, chunk);
    scan_push_large(&pass->scan, value);
    return value;
  }
  size_t bytes = value_bytes(value);
  ChimiValue *copy = area_alloc(&region->survivors, bytes, pass->heap->owner, region,
                                ROLE_SURVIVOR);
  memcpy(copy, value, bytes);
  fix_payload(copy);
  heap_grew(pass->heap, bytes);
  pass->heap->stats.copied_bytes += bytes;
  value->kind = FORWARDED_KIND;
  value->as.items = (ChimiValue **)copy;
  return copy;
}

ChimiHeapStatus chimi_region_pass(ChimiValue **state) {
  ChimiHeap *heap = require_current();
  Region *region = heap->innermost;
  if (!region) fail("chimi_region_pass outside a region");
  Pass pass = {.heap = heap, .region = region, .status = CHIMI_HEAP_OK};
  bool full = region->survivors.bytes > 0 &&
              region->survivors.bytes >= 2 * region->survivor_bytes_at_last_full;
  if (full) {
    pass.from = region->survivors;
    region->survivors = (Area){0};
    for (Chunk *chunk = pass.from.chunks; chunk; chunk = chunk->next) chunk->role = ROLE_FROM;
  }
  area_ensure_bump(&region->survivors, heap->owner, region, ROLE_SURVIVOR);
  pass.scan = scan_start(&region->survivors);

  *state = evacuate(&pass, *state);
  for (ChimiValue *value; (value = scan_next(&pass.scan));) {
    ++heap->stats.scanned_values;
    if (value->kind != CHIMI_VALUE_ITEMS) continue;
    for (uint32_t index = 0; index < value->length; ++index)
      value->as.items[index] = evacuate(&pass, value->as.items[index]);
  }
  scan_finish(&pass.scan);

  heap_shrank(heap, pass.from.bytes);
  area_release(&pass.from);
  heap_shrank(heap, region->young.bytes);
  Chunk *keep = region->young.bump;
  for (Chunk *chunk = region->young.chunks; chunk;) {
    Chunk *next = chunk->next;
    if (chunk != keep) chunk_release(chunk);
    chunk = next;
  }
  region->young = (Area){0};
  if (keep) {
    keep->used = HEADER_BYTES;
    keep->successor = NULL;
    area_push(&region->young, keep);
    region->young.bump = keep;
  }

  if (full) {
    region->survivor_bytes_at_last_full = region->survivors.bytes;
    ++heap->stats.full_collections;
  } else {
    ++heap->stats.minor_collections;
  }
  return pass.status;
}

void chimi_region_exit(void) {
  ChimiHeap *heap = require_current();
  Region *region = heap->innermost;
  if (!region) fail("chimi_region_exit outside a region");
  Region *outer = region->outer;
  Area *into = outer ? &outer->young : &heap->base;
  ChunkRole role = outer ? ROLE_YOUNG : ROLE_BASE;
  area_splice(into, &region->young, heap->owner, outer, role);
  area_splice(into, &region->survivors, heap->owner, outer, role);
  heap->innermost = outer;
  --heap->open_regions;
  free(region);
}

/* Open-addressing map from a source value to its copy in the fragment. */
typedef struct {
  const ChimiValue **keys;
  ChimiValue **values;
  size_t capacity;
  size_t length;
} CopyMap;

static size_t copy_map_slot(const CopyMap *map, const ChimiValue *key) {
  uint64_t hash = ((uintptr_t)key >> 4) * UINT64_C(0x9E3779B97F4A7C15);
  size_t slot = (size_t)(hash >> 32) & (map->capacity - 1);
  while (map->keys[slot] && map->keys[slot] != key) slot = (slot + 1) & (map->capacity - 1);
  return slot;
}

static void copy_map_grow(CopyMap *map) {
  CopyMap grown = {.capacity = map->capacity ? 2 * map->capacity : 64};
  grown.keys = calloc(grown.capacity, sizeof(*grown.keys));
  grown.values = calloc(grown.capacity, sizeof(*grown.values));
  if (!grown.keys || !grown.values) fail("out of memory");
  for (size_t index = 0; index < map->capacity; ++index) {
    if (!map->keys[index]) continue;
    size_t slot = copy_map_slot(&grown, map->keys[index]);
    grown.keys[slot] = map->keys[index];
    grown.values[slot] = map->values[index];
    ++grown.length;
  }
  free(map->keys);
  free(map->values);
  *map = grown;
}

typedef struct {
  uint64_t owner;
  ChimiFragment *fragment;
  CopyMap map;
  Scan scan;
  ChimiHeapStatus status;
} FragmentCopy;

static ChimiValue *copy_into_fragment(FragmentCopy *copy, const ChimiValue *value) {
  if (chunk_of(value)->owner != copy->owner) {
    copy->status = CHIMI_HEAP_FOREIGN_POINTER;
    return (ChimiValue *)value;
  }
  if (2 * (copy->map.length + 1) > copy->map.capacity) copy_map_grow(&copy->map);
  size_t slot = copy_map_slot(&copy->map, value);
  if (copy->map.keys[slot]) return copy->map.values[slot];
  size_t bytes = value_bytes(value);
  ChimiValue *target =
      area_alloc(&copy->fragment->area, bytes, 0, NULL, ROLE_FRAGMENT);
  memcpy(target, value, bytes);
  fix_payload(target);
  if (bytes > LARGE_OBJECT_BYTES) scan_push_large(&copy->scan, target);
  copy->map.keys[slot] = value;
  copy->map.values[slot] = target;
  ++copy->map.length;
  return target;
}

ChimiHeapStatus chimi_fragment_copy(const ChimiValue *root, ChimiFragment **out) {
  ChimiHeap *heap = require_current();
  *out = NULL;
  ChimiFragment *fragment = calloc(1, sizeof(*fragment));
  if (!fragment) fail("out of memory");
  area_ensure_bump(&fragment->area, 0, NULL, ROLE_FRAGMENT);
  FragmentCopy copy = {
      .owner = heap->owner,
      .fragment = fragment,
      .scan = scan_start(&fragment->area),
      .status = CHIMI_HEAP_OK,
  };
  fragment->root = copy_into_fragment(&copy, root);
  for (ChimiValue *value; copy.status == CHIMI_HEAP_OK && (value = scan_next(&copy.scan));) {
    if (value->kind != CHIMI_VALUE_ITEMS) continue;
    for (uint32_t index = 0; index < value->length && copy.status == CHIMI_HEAP_OK; ++index)
      value->as.items[index] = copy_into_fragment(&copy, value->as.items[index]);
  }
  scan_finish(&copy.scan);
  free(copy.map.keys);
  free(copy.map.values);
  if (copy.status != CHIMI_HEAP_OK) {
    chimi_fragment_free(fragment);
    return copy.status;
  }
  *out = fragment;
  return CHIMI_HEAP_OK;
}

ChimiValue *chimi_fragment_adopt(ChimiFragment *fragment) {
  ChimiHeap *heap = require_current();
  Region *region;
  ChunkRole role;
  Area *area = allocation_area(heap, &region, &role);
  size_t bytes = fragment->area.bytes;
  area_splice(area, &fragment->area, heap->owner, region, role);
  heap_grew(heap, bytes);
  ChimiValue *root = fragment->root;
  free(fragment);
  return root;
}

void chimi_fragment_free(ChimiFragment *fragment) {
  if (!fragment) return;
  area_release(&fragment->area);
  free(fragment);
}

uint64_t chimi_value_owner(const ChimiValue *value) { return chunk_of(value)->owner; }

ChimiHeapStats chimi_heap_stats(const ChimiHeap *heap) {
  ChimiHeapStats stats = heap->stats;
  stats.open_regions = heap->open_regions;
  return stats;
}

ChimiChunkStats chimi_chunk_stats(void) { return thread_chunk_stats; }
