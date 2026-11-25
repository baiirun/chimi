/* What every fuzz harness here shares, part one: the canonical output a
   target writes, and FUZZ_CHECK for its invariants. Define FUZZ_NAME (the
   harness's name, for messages) before including it, and include
   fuzz-main.h at the end of the harness. */
#ifndef CHIMI_FUZZ_OUTPUT_H
#define CHIMI_FUZZ_OUTPUT_H

#include <inttypes.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* The input that broke an invariant, set while a target runs. */
static const uint8_t *fuzz_input;
static size_t fuzz_input_size;

#define FUZZ_CHECK(condition, ...)                                                   \
  do {                                                                               \
    if (!(condition)) {                                                              \
      fprintf(stderr, FUZZ_NAME ": invariant failed at %s:%d: %s: ", __FILE__,     \
              __LINE__, #condition);                                                 \
      fprintf(stderr, __VA_ARGS__);                                                  \
      fprintf(stderr, "\ninput (%zu bytes):", fuzz_input_size);                      \
      for (size_t at = 0; at < fuzz_input_size; ++at) fprintf(stderr, " %02x", fuzz_input[at]); \
      fputc('\n', stderr);                                                           \
      abort();                                                                       \
    }                                                                                \
  } while (0)

/* The canonical output, in plain malloc memory so it outlives the input's
   allocation scope. */
typedef struct { char *data; size_t length, capacity; } FuzzOutput;
static void out_bytes(FuzzOutput *out, const char *bytes, size_t length) {
  if (out->length + length + 1 > out->capacity) {
    size_t capacity = out->capacity ? out->capacity : 256;
    while (capacity < out->length + length + 1) capacity *= 2;
    out->data = realloc(out->data, capacity);
    if (!out->data) abort();
    out->capacity = capacity;
  }
  memcpy(out->data + out->length, bytes, length);
  out->length += length;
  out->data[out->length] = '\0';
}
static void out_word(FuzzOutput *out, const char *word) { out_bytes(out, word, strlen(word)); }
static void out_integer(FuzzOutput *out, int64_t value) {
  char text[24];
  snprintf(text, sizeof(text), "%" PRId64, value);
  out_word(out, text);
}
/* `bytes` quoted, with `"`, `\` and every byte outside printable ASCII escaped,
   so an output line holds no tab or newline of its own. */
static void out_quoted(FuzzOutput *out, const char *bytes, size_t length) {
  out_bytes(out, "\"", 1);
  for (size_t at = 0; at < length; ++at) {
    unsigned char byte = (unsigned char)bytes[at];
    if (byte == '"' || byte == '\\') {
      char escaped[2] = {'\\', (char)byte};
      out_bytes(out, escaped, 2);
    } else if (byte >= 0x20 && byte < 0x7f) {
      out_bytes(out, (const char *)&byte, 1);
    } else {
      char escaped[5];
      snprintf(escaped, sizeof(escaped), "\\x%02x", byte);
      out_bytes(out, escaped, 4);
    }
  }
  out_bytes(out, "\"", 1);
}

/* A target turns one input into `out` and checks its invariants. */
typedef void (*FuzzTarget)(const uint8_t *, size_t, FuzzOutput *);
typedef struct { const char *name; FuzzTarget run; } FuzzTargetEntry;

#endif
