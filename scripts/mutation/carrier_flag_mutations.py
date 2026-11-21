"""Mutants of R4 step 3 PR 5d-4: the C runner's `--carriers=<n>`.

Run: scripts/mutation/mutate.py --timeout 600 scripts/mutation/carrier_flag_mutations.py
Results: scripts/mutation/carrier_flag_results.txt.
Each mutant runs tests/carriers.rs.
"""

CRATE = "compiler/chimi"

C = "runtime/native/runtime.c"


def m(name, old, new):
    return dict(name=name, file=C, edits=[(old, new)], targets=["carriers"], filter="")


MUTATIONS = [
    m("the count never reaches the scheduler",
      "    .carriers = runtime->carriers,",
      "    .carriers = 1,"),
    m("carrier threads are not bound",
      "    .carrier_enter = enter_carrier_thread,",
      "    .carrier_enter = NULL,"),
    m("a carrier thread binds carrier 0",
      "  carrier_thread_enter(index);\n}",
      "  carrier_thread_enter(0);\n}"),
    m("the carriers are never created",
      "  carriers_create(runtime.carriers);",
      ""),
    m("several carriers run under virtual time",
      "    if (runtime.carriers > 1 && runtime.virtual_clock) {",
      "    if (runtime.carriers > 1 && runtime.virtual_clock && false) {"),
    m("0 carriers is accepted",
      "          value == 0 || value > CARRIERS_MAX) {",
      "          value > CARRIERS_MAX) {"),
    m("the upper bound is off by one",
      "          value == 0 || value > CARRIERS_MAX) {",
      "          value == 0 || value >= CARRIERS_MAX) {"),
    m("no upper bound",
      "          value == 0 || value > CARRIERS_MAX) {",
      "          value == 0) {"),
    m("trailing text is accepted",
      "      if (errno == ERANGE || end == argv[index] + 11 || *end != '\\0' || argv[index][11] == '-' ||",
      "      if (errno == ERANGE || end == argv[index] + 11 || argv[index][11] == '-' ||"),
    m("an empty count is accepted",
      "      if (errno == ERANGE || end == argv[index] + 11 || *end != '\\0' || argv[index][11] == '-' ||",
      "      if (errno == ERANGE || *end != '\\0' || argv[index][11] == '-' ||"),
    m("a negative count is accepted",
      "      if (errno == ERANGE || end == argv[index] + 11 || *end != '\\0' || argv[index][11] == '-' ||",
      "      if (errno == ERANGE || end == argv[index] + 11 || *end != '\\0' ||"),
    m("an overflowing count is accepted",
      "      if (errno == ERANGE || end == argv[index] + 11 || *end != '\\0' || argv[index][11] == '-' ||",
      "      if (end == argv[index] + 11 || *end != '\\0' || argv[index][11] == '-' ||"),
]
