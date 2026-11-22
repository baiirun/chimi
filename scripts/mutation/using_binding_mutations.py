"""Mutants of a provider `using` fixes on a function value (CHI-294): typing
records the bound names, and core lowering wraps the value so its own
providers serve every call.

Run: scripts/mutation/mutate.py scripts/mutation/using_binding_mutations.py
"""

CRATE = "compiler/chimi"

TYPING = "src/typing.rs"
EXECUTABLE = "src/executable.rs"
EXECUTION = "src/execution.rs"
RUNTIME = "runtime/native/runtime.c"

FILTER = "function_value"


def mutant(name, file, edits, filter=FILTER, targets=("language",)):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("typing binds nothing", TYPING, [(
        "        if bound_names.is_empty() || open_row {\n            self.using_bindings",
        "        if true {\n            self.using_bindings",
    )]),
    mutant("typing binds every supplied name, evaluation needs too", TYPING, [(
        "        let bound_names = self.needs_rows[latent_needs.raw() as usize]\n            .entries\n            .iter()\n            .filter(|need| supplied_names.contains(&need.lexical_name))\n            .map(|need| need.lexical_name.clone())\n            .collect::<Vec<_>>();",
        "        let bound_names = supplied_names.clone();",
    )], filter="using"),
    mutant("the wrapper supplies no provider at the call", EXECUTABLE, [(
        "            .filter(|(name, _, _)| bound.contains(name))",
        "            .filter(|(name, _, _)| !bound.contains(name))",
    )]),
    mutant("the wrapper calls the value without providing", EXECUTABLE, [(
        "            CoreOperation::Provision {\n                expression: call,\n                providers: rebound,\n            },",
        "            {\n                let _ = rebound;\n                CoreOperation::Identity(call)\n            },",
    )]),
    mutant("the wrapper does not capture the value", EXECUTABLE, [(
        "        captures.push(function);\n",
        "",
    )]),
    mutant("the wrapper drops the last argument", EXECUTABLE, [(
        "            .map(|_| self.synthetic_local(\"argument\"))\n            .collect::<Vec<_>>();",
        "            .map(|_| self.synthetic_local(\"argument\"))\n            .skip(1)\n            .collect::<Vec<_>>();",
    )]),
    mutant("the value itself is evaluated without its providers", EXECUTABLE, [(
        "            CoreOperation::Provision {\n                expression: value,\n                providers: provided,\n            },",
        "            {\n                let _ = provided;\n                CoreOperation::Identity(value)\n            },",
    )]),
    mutant("a row parameter does not stop the binding", TYPING, [(
        "        if !bound_names.is_empty() && open_row {",
        "        if false {",
    )], filter="row_parameter"),
    mutant("the interpreter evaluates a provider after the earlier ones", EXECUTION, [(
        "                    provided.push((provider.name, value));",
        "                    environment.requirements.insert(provider.name, value);",
    )], filter="outside_its_own_using"),
    mutant("native evaluates a provider after the earlier ones", RUNTIME, [(
        "    ChimiControl *control = run_node(provider->node, runtime, incoming, span);",
        "    ChimiControl *control = RUN(provider->node);",
    )], filter="outside_its_own_using"),
]
