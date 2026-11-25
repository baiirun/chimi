/* What every fuzz harness here shares, part two: the replay driver and the
   libFuzzer entry points. Include it at the end of the harness, after
   `FUZZ_TARGETS`, its table of targets, and `fz_run_one`, which runs one
   target on one input (setting `fuzz_input` for FUZZ_CHECK) and frees
   whatever the input allocated, except `out->data`, which the caller frees.

   Built two ways:
     cc ... HARNESS.c                         the replay driver:
         HARNESS TARGET CORPUS                prints the corpus file again with
                                              each output recomputed
         HARNESS TARGET --files FILE...       prints one corpus line per file
         HARNESS --unpack CORPUS DIR          writes each input to DIR/<line>
     clang -fsanitize=fuzzer -DCHIMI_FUZZ_LIBFUZZER ... HARNESS.c
         CHIMI_FUZZ_TARGET=TARGET HARNESS DIR...    libFuzzer

   A corpus file holds one input per line: the input quoted as outputs are
   (out_quoted), a tab, and the output the target printed for it when the
   file was last blessed. */
#ifndef CHIMI_FUZZ_MAIN_H
#define CHIMI_FUZZ_MAIN_H

static FuzzTarget fz_find_target(const char *name) {
  size_t count = sizeof(FUZZ_TARGETS) / sizeof(FUZZ_TARGETS[0]);
  for (size_t index = 0; name && index < count; ++index)
    if (strcmp(FUZZ_TARGETS[index].name, name) == 0) return FUZZ_TARGETS[index].run;
  fprintf(stderr, FUZZ_NAME ": unknown target `%s`; targets:", name ? name : "(none)");
  for (size_t index = 0; index < count; ++index) fprintf(stderr, " %s", FUZZ_TARGETS[index].name);
  fputc('\n', stderr);
  return NULL;
}

#ifdef CHIMI_FUZZ_LIBFUZZER
static FuzzTarget libfuzzer_target;
int LLVMFuzzerInitialize(int *argc, char ***argv) {
  (void)argc; (void)argv;
  libfuzzer_target = fz_find_target(getenv("CHIMI_FUZZ_TARGET"));
  if (!libfuzzer_target) exit(2);
  return 0;
}
int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
  FuzzOutput out = {0};
  fz_run_one(libfuzzer_target, data, size, &out);
  free(out.data);
  return 0;
}
#else
static uint8_t *fz_read_file(const char *path, size_t *size) {
  FILE *file = fopen(path, "rb");
  if (!file) return NULL;
  uint8_t *data = NULL;
  size_t length = 0, capacity = 0;
  for (;;) {
    if (length == capacity) {
      capacity = capacity ? capacity * 2 : 4096;
      data = realloc(data, capacity);
      if (!data) abort();
    }
    size_t read = fread(data + length, 1, capacity - length, file);
    length += read;
    if (read == 0) break;
  }
  bool failed = ferror(file);
  fclose(file);
  if (failed) { free(data); return NULL; }
  *size = length;
  return data;
}
static int fz_hex(char digit) {
  if (digit >= '0' && digit <= '9') return digit - '0';
  if (digit >= 'a' && digit <= 'f') return digit - 'a' + 10;
  return -1;
}
/* The input quoted at the start of `line` (out_quoted's form), into `input`;
   false when the line is not in that form. */
static bool fz_unquote(const char *line, size_t length, FuzzOutput *input) {
  if (length < 2 || line[0] != '"') return false;
  for (size_t at = 1; at < length; ++at) {
    char next = line[at];
    if (next == '"') return at + 1 == length || line[at + 1] == '\t';
    if (next != '\\') { out_bytes(input, &next, 1); continue; }
    if (++at >= length) return false;
    if (line[at] == '"' || line[at] == '\\') { out_bytes(input, &line[at], 1); continue; }
    if (line[at] != 'x' || at + 2 >= length) return false;
    int high = fz_hex(line[at + 1]), low = fz_hex(line[at + 2]);
    if (high < 0 || low < 0) return false;
    char byte = (char)(high << 4 | low);
    out_bytes(input, &byte, 1);
    at += 2;
  }
  return false;
}
static void fz_print_line(FuzzTarget target, const uint8_t *data, size_t size) {
  FuzzOutput quoted = {0}, out = {0};
  out_quoted(&quoted, (const char *)data, size);
  fz_run_one(target, data, size, &out);
  printf("%s\t%s\n", quoted.data, out.data ? out.data : "");
  free(quoted.data);
  free(out.data);
}
/* Calls `each` with every input of the corpus file `path`, in order; 2 when
   the file cannot be read or a line is malformed. */
static int fz_each_input(const char *path, void (*each)(void *, const uint8_t *, size_t, size_t), void *context) {
  size_t size = 0;
  uint8_t *corpus = fz_read_file(path, &size);
  if (!corpus) {
    fprintf(stderr, FUZZ_NAME ": cannot read %s\n", path);
    return 2;
  }
  size_t number = 0;
  for (size_t start = 0; start < size; ++number) {
    const uint8_t *newline = memchr(corpus + start, '\n', size - start);
    size_t end = newline ? (size_t)(newline - corpus) : size;
    FuzzOutput input = {0};
    if (!fz_unquote((const char *)corpus + start, end - start, &input)) {
      fprintf(stderr, FUZZ_NAME ": %s:%zu: not a quoted input\n", path, number + 1);
      free(input.data);
      free(corpus);
      return 2;
    }
    each(context, (const uint8_t *)(input.data ? input.data : ""), input.length, number);
    free(input.data);
    start = end + 1;
  }
  free(corpus);
  return 0;
}
static void fz_replay_input(void *context, const uint8_t *data, size_t size, size_t number) {
  (void)number;
  fz_print_line((FuzzTarget)context, data, size);
}
static void fz_unpack_input(void *context, const uint8_t *data, size_t size, size_t number) {
  char path[4096];
  snprintf(path, sizeof(path), "%s/%06zu", (const char *)context, number + 1);
  FILE *file = fopen(path, "wb");
  if (!file || fwrite(data, 1, size, file) != size || fclose(file) != 0) {
    fprintf(stderr, FUZZ_NAME ": cannot write %s\n", path);
    exit(2);
  }
}
int main(int argc, char **argv) {
  if (argc == 4 && strcmp(argv[1], "--unpack") == 0) return fz_each_input(argv[2], fz_unpack_input, argv[3]);
  if (argc < 3) {
    fputs("usage: " FUZZ_NAME " TARGET CORPUS | TARGET --files FILE... | --unpack CORPUS DIR\n", stderr);
    return 2;
  }
  FuzzTarget target = fz_find_target(argv[1]);
  if (!target) return 2;
  if (strcmp(argv[2], "--files") != 0) {
    if (argc != 3) {
      fputs(FUZZ_NAME ": replay takes one corpus file\n", stderr);
      return 2;
    }
    return fz_each_input(argv[2], fz_replay_input, (void *)target);
  }
  for (int index = 3; index < argc; ++index) {
    size_t size = 0;
    uint8_t *data = fz_read_file(argv[index], &size);
    if (!data) {
      fprintf(stderr, FUZZ_NAME ": cannot read %s\n", argv[index]);
      return 2;
    }
    fz_print_line(target, data, size);
    free(data);
  }
  return 0;
}
#endif

#endif
