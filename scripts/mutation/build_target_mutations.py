"""Mutants of `chimi build --target` (Phase 9, T8): which targets build and
what a refused one says.

Run: scripts/mutation/mutate.py scripts/mutation/build_target_mutations.py
"""

CRATE = "compiler/chimi"

N = "src/backend/native.rs"
M = "src/main.rs"


def mutant(name, file, edits, targets=("build_target",), filter=""):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("the host is refused", N, [(
        "    if same_platform(&target, &host) {",
        "    if false {",
    )]),
    mutant("any target builds", N, [(
        "    if same_platform(&target, &host) {",
        "    if true {",
    )]),
    mutant("the target is not checked", M, [(
        "    if let Some(Err(error)) = target.map(check_build_target) {",
        "    if let Some(Err(error)) = None::<Option<Result<(), String>>>.flatten() {",
    )]),
    mutant("the vendor must match", N, [(
        "    left.architecture == right.architecture\n",
        "    left.vendor == right.vendor && left.architecture == right.architecture\n",
    )]),
    mutant("the environment is ignored", N, [(
        "        && left.environment == right.environment\n",
        "",
    )]),
]
