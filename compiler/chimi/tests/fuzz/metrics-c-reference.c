/* The C std/metrics aggregate that runtime.c used before it moved to Rust
   (runtime/chimi-runtime/src/metrics.rs, CHI-307), kept as the oracle for
   differential fuzzing: the metrics target records the same measurements in
   both and checks that they write the same document. It is included after
   runtime.c and float-c-reference.c; the code is the shipped code with its
   names prefixed, without the lock, plus a reset between inputs. */

#define REFERENCE_CHIMI_METRIC_SERIES_PER_NAME 1000
#define REFERENCE_CHIMI_METRIC_BOUND_COUNT 15

static const double reference_metric_bounds[REFERENCE_CHIMI_METRIC_BOUND_COUNT] = {
  0.0, 0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
};

typedef struct {
  char *name;
  size_t name_length;
  char *labels;
  size_t labels_length;
  int64_t kind;
  /* A count's total, a gauge's last value, an observation's count and sum. */
  int64_t total;
  double last;
  uint64_t count;
  double sum;
  uint64_t buckets[REFERENCE_CHIMI_METRIC_BOUND_COUNT + 1];
} reference_MetricSeries;

typedef struct {
  char *name;
  size_t name_length;
  /* The kind the name was first recorded with, and its label sets so far. */
  int64_t kind;
  size_t series;
} reference_MetricName;

/* Open-addressed tables of index + 1 into the arrays, 0 for an empty slot,
   kept at most half full. */
typedef struct {
  size_t *slots;
  size_t capacity;
} reference_MetricTable;

static reference_MetricSeries *reference_metric_series;
static size_t reference_metric_series_count, reference_metric_series_capacity;
static reference_MetricName *reference_metric_names;
static size_t reference_metric_name_count, reference_metric_name_capacity;
static reference_MetricTable reference_metric_series_table, reference_metric_name_table;
static uint64_t reference_metric_dropped_label_sets, reference_metric_dropped_kind_conflicts;

static uint64_t reference_metric_hash(const char *name, size_t name_length, const char *labels, size_t labels_length) {
  uint64_t hash = UINT64_C(14695981039346656037);
  for (size_t i = 0; i < name_length; ++i) hash = (hash ^ (unsigned char)name[i]) * UINT64_C(1099511628211);
  hash = (hash ^ 0xff) * UINT64_C(1099511628211);
  for (size_t i = 0; i < labels_length; ++i) hash = (hash ^ (unsigned char)labels[i]) * UINT64_C(1099511628211);
  return hash;
}

static char *reference_metric_copy(const char *bytes, size_t length) {
  char *copy = malloc(length ? length : 1);
  if (!copy) exhausted_memory();
  memcpy(copy, bytes, length);
  return copy;
}

static bool reference_metric_series_is(const reference_MetricSeries *series, const char *name, size_t name_length, const char *labels,
                             size_t labels_length) {
  return series->name_length == name_length && series->labels_length == labels_length &&
         memcmp(series->name, name, name_length) == 0 && memcmp(series->labels, labels, labels_length) == 0;
}

/* The slot holding the entry `matches` accepts, or the empty slot it goes in. */
static size_t *reference_metric_slot(reference_MetricTable *table, uint64_t hash, bool (*matches)(size_t, const void *),
                           const void *key) {
  size_t mask = table->capacity - 1;
  for (size_t at = (size_t)hash & mask;; at = (at + 1) & mask)
    if (table->slots[at] == 0 || matches(table->slots[at] - 1, key)) return &table->slots[at];
}

typedef struct {
  const char *name;
  size_t name_length;
  const char *labels;
  size_t labels_length;
} reference_MetricKey;

static bool reference_metric_series_matches(size_t index, const void *key) {
  const reference_MetricKey *metric = key;
  return reference_metric_series_is(&reference_metric_series[index], metric->name, metric->name_length, metric->labels,
                          metric->labels_length);
}

static bool reference_metric_name_matches(size_t index, const void *key) {
  const reference_MetricKey *metric = key;
  return reference_metric_names[index].name_length == metric->name_length &&
         memcmp(reference_metric_names[index].name, metric->name, metric->name_length) == 0;
}

/* Makes room for one more entry, rehashing every entry `hash_of` keys. */
static void reference_metric_table_reserve(reference_MetricTable *table, size_t count, uint64_t (*hash_of)(size_t)) {
  if ((count + 1) * 2 <= table->capacity) return;
  size_t capacity = table->capacity ? table->capacity * 2 : 64;
  size_t *slots = calloc(capacity, sizeof(*slots));
  if (!slots) exhausted_memory();
  for (size_t index = 0; index < count; ++index) {
    size_t at = (size_t)hash_of(index) & (capacity - 1);
    while (slots[at] != 0) at = (at + 1) & (capacity - 1);
    slots[at] = index + 1;
  }
  free(table->slots);
  table->slots = slots;
  table->capacity = capacity;
}

static uint64_t reference_metric_series_hash(size_t index) {
  const reference_MetricSeries *series = &reference_metric_series[index];
  return reference_metric_hash(series->name, series->name_length, series->labels, series->labels_length);
}

static uint64_t reference_metric_name_hash(size_t index) {
  return reference_metric_hash(reference_metric_names[index].name, reference_metric_names[index].name_length, "", 0);
}

/* Adds one measurement; the caller holds metrics_lock. */
static void reference_metrics_record(int64_t kind, const char *name, size_t name_length, double value, const char *labels,
                           size_t labels_length) {
  reference_MetricKey key = {name, name_length, labels, labels_length};
  reference_metric_table_reserve(&reference_metric_name_table, reference_metric_name_count, reference_metric_name_hash);
  size_t *name_slot = reference_metric_slot(&reference_metric_name_table, reference_metric_hash(name, name_length, "", 0), reference_metric_name_matches, &key);
  if (*name_slot == 0) {
    if (reference_metric_name_count == reference_metric_name_capacity) {
      reference_metric_name_capacity = reference_metric_name_capacity ? reference_metric_name_capacity * 2 : 16;
      reference_metric_names = realloc(reference_metric_names, reference_metric_name_capacity * sizeof(*reference_metric_names));
      if (!reference_metric_names) exhausted_memory();
    }
    reference_metric_names[reference_metric_name_count] = (reference_MetricName){reference_metric_copy(name, name_length), name_length, kind, 0};
    *name_slot = ++reference_metric_name_count;
  }
  reference_MetricName *metric = &reference_metric_names[*name_slot - 1];
  if (metric->kind != kind) {
    ++reference_metric_dropped_kind_conflicts;
    return;
  }
  reference_metric_table_reserve(&reference_metric_series_table, reference_metric_series_count, reference_metric_series_hash);
  uint64_t hash = reference_metric_hash(name, name_length, labels, labels_length);
  size_t *series_slot = reference_metric_slot(&reference_metric_series_table, hash, reference_metric_series_matches, &key);
  if (*series_slot == 0) {
    if (metric->series == REFERENCE_CHIMI_METRIC_SERIES_PER_NAME) {
      ++reference_metric_dropped_label_sets;
      return;
    }
    ++metric->series;
    if (reference_metric_series_count == reference_metric_series_capacity) {
      reference_metric_series_capacity = reference_metric_series_capacity ? reference_metric_series_capacity * 2 : 16;
      reference_metric_series = realloc(reference_metric_series, reference_metric_series_capacity * sizeof(*reference_metric_series));
      if (!reference_metric_series) exhausted_memory();
    }
    reference_metric_series[reference_metric_series_count] = (reference_MetricSeries){
      .name = reference_metric_copy(name, name_length), .name_length = name_length,
      .labels = reference_metric_copy(labels, labels_length), .labels_length = labels_length, .kind = kind,
    };
    *series_slot = ++reference_metric_series_count;
  }
  reference_MetricSeries *series = &reference_metric_series[*series_slot - 1];
  switch (kind) {
    case 0: {
      /* A count's value is a whole, non-negative number (std/metrics.count). */
      int64_t by = value >= 9223372036854775807.0 ? INT64_MAX : (int64_t)value;
      series->total = series->total > INT64_MAX - by ? INT64_MAX : series->total + by;
      break;
    }
    case 1: series->last = value; break;
    default: {
      ++series->count;
      /* A sum past the largest Float saturates there, so it stays finite. */
      double sum = series->sum + value;
      series->sum = isfinite(sum) ? sum : sum > 0 ? DBL_MAX : -DBL_MAX;
      size_t bucket = 0;
      while (bucket < REFERENCE_CHIMI_METRIC_BOUND_COUNT && !(value <= reference_metric_bounds[bucket])) ++bucket;
      ++series->buckets[bucket];
      break;
    }
  }
}

/* Name order, then label order, comparing bytes as Rust's String does. */
static int reference_metric_bytes_order(const char *left, size_t left_length, const char *right, size_t right_length) {
  int order = memcmp(left, right, left_length < right_length ? left_length : right_length);
  if (order != 0) return order;
  return left_length < right_length ? -1 : left_length > right_length ? 1 : 0;
}

static int reference_metric_series_order(const void *left, const void *right) {
  const reference_MetricSeries *a = &reference_metric_series[*(const size_t *)left], *b = &reference_metric_series[*(const size_t *)right];
  int order = reference_metric_bytes_order(a->name, a->name_length, b->name, b->name_length);
  return order != 0 ? order : reference_metric_bytes_order(a->labels, a->labels_length, b->labels, b->labels_length);
}

/* Forgets every measurement, for the next input. */
static void reference_metrics_reset(void) {
  for (size_t index = 0; index < reference_metric_series_count; ++index) {
    free(reference_metric_series[index].name);
    free(reference_metric_series[index].labels);
  }
  for (size_t index = 0; index < reference_metric_name_count; ++index) free(reference_metric_names[index].name);
  free(reference_metric_series);
  free(reference_metric_names);
  free(reference_metric_series_table.slots);
  free(reference_metric_name_table.slots);
  reference_metric_series = NULL;
  reference_metric_names = NULL;
  reference_metric_series_table = (reference_MetricTable){0};
  reference_metric_name_table = (reference_MetricTable){0};
  reference_metric_series_count = reference_metric_series_capacity = 0;
  reference_metric_name_count = reference_metric_name_capacity = 0;
  reference_metric_dropped_label_sets = reference_metric_dropped_kind_conflicts = 0;
}

/* The metrics document, as write_metrics_document wrote it, in a malloc'd
   NUL-terminated string. */
static char *reference_metrics_document(int64_t started, int64_t ended) {
  char *bytes = NULL;
  size_t length = 0;
  FILE *output = open_memstream(&bytes, &length);
  if (!output) abort();
  size_t *order = malloc((reference_metric_series_count ? reference_metric_series_count : 1) * sizeof(*order));
  if (!order) abort();
  for (size_t index = 0; index < reference_metric_series_count; ++index) order[index] = index;
  qsort(order, reference_metric_series_count, sizeof(*order), reference_metric_series_order);
  fputs("{\"schema\":\"chimi.metrics.v1\",\"metrics\":[", output);
  char number[40];
  for (size_t index = 0; index < reference_metric_series_count; ++index) {
    const reference_MetricSeries *series = &reference_metric_series[order[index]];
    if (index > 0) fputc(',', output);
    fputs("{\"name\":", output);
    fwrite(series->name, 1, series->name_length, output);
    fputs(",\"labels\":", output);
    fwrite(series->labels, 1, series->labels_length, output);
    switch (series->kind) {
      case 0: fprintf(output, ",\"kind\":\"count\",\"value\":%" PRId64 "}", series->total); break;
      case 1:
        reference_float_text(series->last, number);
        fprintf(output, ",\"kind\":\"gauge\",\"value\":%s}", number);
        break;
      default:
        reference_float_text(series->sum, number);
        fprintf(output, ",\"kind\":\"observe\",\"count\":%" PRIu64 ",\"sum\":%s,\"bounds\":[", series->count, number);
        for (size_t bound = 0; bound < REFERENCE_CHIMI_METRIC_BOUND_COUNT; ++bound) {
          reference_float_text(reference_metric_bounds[bound], number);
          fprintf(output, "%s%s", bound ? "," : "", number);
        }
        fputs("],\"bucket_counts\":[", output);
        for (size_t bucket = 0; bucket <= REFERENCE_CHIMI_METRIC_BOUND_COUNT; ++bucket)
          fprintf(output, "%s%" PRIu64, bucket ? "," : "", series->buckets[bucket]);
        fputs("]}", output);
        break;
    }
  }
  fprintf(output, "],\"dropped\":{\"label_sets\":%" PRIu64 ",\"kind_conflicts\":%" PRIu64
          "},\"start_unix_ns\":%" PRId64 ",\"time_unix_ns\":%" PRId64 "}\n",
          reference_metric_dropped_label_sets, reference_metric_dropped_kind_conflicts, started, ended);
  free(order);
  if (fclose(output) != 0) abort();
  return bytes;
}
