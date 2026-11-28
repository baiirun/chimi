"""Mutants of R4 step 3 PR 5d-4: the C runner's `--carriers=<n>`.

Run: scripts/mutation/mutate.py --timeout 600 scripts/mutation/carrier_flag_mutations.py
Results: scripts/mutation/carrier_flag_results.txt.
Each mutant runs tests/carriers.rs.
"""

CRATE = "compiler/chimi"

C = "runtime/native/runtime.c"
STARTUP = "../../runtime/chimi-runtime/src/startup.rs"


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
    dict(m("several carriers run under virtual time",
           "            if runtime.carriers > 1 && runtime.virtual_clock {",
           "            if runtime.carriers > 1 && runtime.virtual_clock && false {"), file=STARTUP),
    dict(m("0 carriers is accepted",
           "if count == 0 || !rest.is_empty() || count > u64::from(CARRIERS_MAX) {",
           "if !rest.is_empty() || count > u64::from(CARRIERS_MAX) {"), file=STARTUP),
    dict(m("the upper bound is off by one",
           "if count == 0 || !rest.is_empty() || count > u64::from(CARRIERS_MAX) {",
           "if count == 0 || !rest.is_empty() || count >= u64::from(CARRIERS_MAX) {"), file=STARTUP),
    dict(m("no upper bound",
           "if count == 0 || !rest.is_empty() || count > u64::from(CARRIERS_MAX) {",
           "if count == 0 || !rest.is_empty() {"), file=STARTUP),
    dict(m("trailing text is accepted",
           "if count == 0 || !rest.is_empty() || count > u64::from(CARRIERS_MAX) {",
           "if count == 0 || count > u64::from(CARRIERS_MAX) {"), file=STARTUP),
    dict(m("a leading sign or space is accepted",
           "let (count, rest) = if value.to_bytes().first().is_some_and(u8::is_ascii_digit) {",
           "let (count, rest) = if true {"), file=STARTUP),
    # The review's crash: a stop forcing a body that never ran, from another
    # carrier, resumed it there while its record named its old home.
    dict(name="a fiber that never ran keeps its old home",
         file="../../runtime/chimi-fiber/src/scheduler.rs",
         edits=[("        if !record.ran {\n            record.carrier = self.current;\n        }\n", "")],
         targets=["carriers"], filter="a_stop_that_forces"),
]
