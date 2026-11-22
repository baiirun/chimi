"""Mutants of the native build's artifact-directory lock (CHI-299).

Run: scripts/mutation/mutate.py scripts/mutation/build_lock_mutations.py
"""

CRATE = "compiler/chimi"

NATIVE = "src/backend/native.rs"


def mutant(name, edits):
    return dict(name=name, file=NATIVE, edits=edits, targets=["build_lock"], filter="")


MUTATIONS = [
    mutant("an application build takes no lock", [(
        "    let _lock = lock_artifact_directory(artifact_directory)?;\n",
        "",
    )]),
    mutant("the lock is released before the build", [(
        "    let _lock = lock_artifact_directory(artifact_directory)?;\n",
        "    let _ = lock_artifact_directory(artifact_directory)?;\n",
    )]),
    mutant("the lock is shared", [(
        "    lock.lock()\n",
        "    lock.lock_shared()\n",
    )]),
]
