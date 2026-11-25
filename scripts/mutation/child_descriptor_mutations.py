"""Mutants of the close-on-exec PR before R4 step 3 PR 5e: a child std/exec
starts holds only its three standard streams.

Run: scripts/mutation/mutate.py --timeout 600 scripts/mutation/child_descriptor_mutations.py
Results: scripts/mutation/child_descriptor_results.txt.
The macOS branch (POSIX_SPAWN_CLOEXEC_DEFAULT and addinherit_np) compiles
only on macOS, so these mutants cover the glibc branch; CI's macOS job runs
the same test against the macOS branch.
"""

CRATE = "compiler/chimi"

C = "../../runtime/chimi-runtime/src/binding/exec.rs"


def m(name, old, new):
    return dict(name=name, file=C, edits=[(old, new)], targets=["std_exec"], filter="a_child_holds_only")


MUTATIONS = [
    m('other descriptors stay open',
      '    if error == 0 {\n        error = close_other_descriptors(&mut spawn.actions);\n    }\n',
      ''),
    m('standard error is closed too',
      'addclosefrom_np(actions, libc::STDERR_FILENO + 1)',
      'addclosefrom_np(actions, libc::STDERR_FILENO)'),
    m('descriptors close before the streams are connected',
      '    for (fd, (mode, end)) in (0..).zip(modes.iter().zip(ends.iter())) {\n',
      '    if error == 0 {\n        error = close_other_descriptors(&mut spawn.actions);\n    }\n    for (fd, (mode, end)) in (0..).zip(modes.iter().zip(ends.iter())) {\n'),
]
