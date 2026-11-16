"""Mutants of `chimi std test` (Phase 5, CHI-208): the standard tree's
doc-comment examples run as tests, reported under the documented module.

Run: scripts/mutation/mutate.py scripts/mutation/std_examples_mutations.py
"""

CRATE = "compiler/chimi"

P = "src/project.rs"
M = "src/main.rs"


def mutant(name, file, edits, targets=("testing",), filter="std_test"):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("std test reads project examples", P, [(
        "        self.add_examples_of(standard, true)",
        "        self.add_examples_of(standard, false)",
    )]),
    mutant("project test reads std examples", P, [(
        "        self.add_examples_of(standard, false)",
        "        self.add_examples_of(standard, true)",
    )], targets=("testing",), filter="doc_comment_examples"),
    mutant("std examples keep the std/ prefix", P, [(
        "                let prefix = if standard_modules { \"example/\" } else { \"\" };",
        "                let prefix = \"\";",
    )]),
    mutant("std test accepts a path", M, [(
        "        (Some(_), _) => Err(UsageError {\n            message: \"std test takes options only; it runs the standard tree's examples\",\n        }),",
        "        (Some(_), options) => Ok(Command::StdTest { options }),",
    )]),
    mutant("std test reports example modules", M, [(
        "    suite.rename_modules(examples);",
        "    let _ = examples;",
    )]),
]
