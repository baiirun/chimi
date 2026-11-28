"""Mutants of the runner's argument parsing in Rust (CHI-305 step 5, phase 8, twelfth PR).

Run: scripts/mutation/mutate.py scripts/mutation/startup_mutations.py
Results: scripts/mutation/startup_results.txt.
The carrier, clock, signal, log, metrics and profile settings' mutants are in
their own lists, retargeted to startup.rs.
"""

CRATE = "compiler/chimi"

STARTUP = "../../runtime/chimi-runtime/src/startup.rs"


def m(name, old, new):
    return dict(name=name, file=STARTUP, edits=[(old, new)], targets=["cli"],
                filter="native_runner_settings_are_checked_before_the_program_runs")


MUTATIONS = [
    m("a missing -- is accepted",
      "            if !found_boundary {",
      "            if false {"),
    m("an unknown setting is ignored",
      "                    return refuse(&[b\"unknown Chimi runtime argument: \", text.to_bytes()]);",
      "                    return 0;"),
    m("--test takes the index one past the last test",
      "index >= u64::from(CHIMI_TEST_COUNT)",
      "index > u64::from(CHIMI_TEST_COUNT)"),
    m("--max-line-bytes takes 0",
      "if overflowed || !read || !rest.is_empty() || bytes == 0 || bytes > u64::from(u32::MAX) {",
      "if overflowed || !read || !rest.is_empty() || bytes > u64::from(u32::MAX) {"),
    m("--random-seed takes a minus sign",
      " || value.to_bytes().first() == Some(&b'-') {",
      " {"),
    m("a signal past the largest instant is accepted",
      "    if amount > i64::MAX as u64 / nanoseconds {",
      "    if false {"),
    m("an empty --random-seed is seed 0",
      "if overflowed || !read || !rest.is_empty() || value.to_bytes().first() == Some(&b'-') {",
      "if overflowed || !rest.is_empty() || value.to_bytes().first() == Some(&b'-') {"),
    m("the signal bound ignores the unit",
      "    if amount > i64::MAX as u64 / nanoseconds {",
      "    if amount > i64::MAX as u64 {"),
    m("a 65th user signal is taken",
      "} else if runtime.injected_user_signal_count == MAX_INJECTED_SIGNALS {",
      "} else if runtime.injected_user_signal_count > MAX_INJECTED_SIGNALS {"),
    m("a character device is one file with itself",
      "    left.st_dev == right.st_dev && left.st_ino == right.st_ino && (left.st_mode & libc::S_IFMT) != libc::S_IFCHR",
      "    left.st_dev == right.st_dev && left.st_ino == right.st_ino"),
    m("a character device metrics file clashes with the log",
      "                && (metrics.st_mode & libc::S_IFMT) != libc::S_IFCHR\n",
      "\n"),
    m("telemetry and outcome may be one file",
      "        if same(telemetry, outcome) {",
      "        if false {"),
]
