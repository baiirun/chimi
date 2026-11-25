"""Mutants of `chimi lint`'s finished rule set (CHI-281): what counts as a
root for `unused-private`, which item kinds it covers, the `unused-need`
rule's scope, and the fixes for unused imports and bindings.

Run: scripts/mutation/mutate.py scripts/mutation/lint_reach_mutations.py
"""

CRATE = "compiler/chimi"

L = "src/lint.rs"
T = "src/typing.rs"


def mutant(name, file, edits, targets=("lint",), filter=""):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("any reference counts as a use", L, [(
        "            Some(source) => edges[source].push(target),",
        "            Some(_) => pending.push(target),",
    )]),
    mutant("self-recursion counts as a use", L, [(
        "            Some(source) if source == target => continue,",
        "            Some(source) if source == target => pending.push(target),",
    )]),
    mutant("reach does not pass on", L, [(
        "            pending.extend(edges[index].iter().copied());",
        "            let _ = &edges[index];",
    )]),
    mutant("kept items are not roots", L, [(
        "        .filter(|&index| !is_reportable(module, private[index]))",
        "        .filter(|_| false)",
    )]),
    mutant("underscore items are reported", L, [(
        "        && !is_intentionally_unused(&declaration.name.text)\n        && !is_runner_provider",
        "        && !is_runner_provider",
    )]),
    mutant("tests are reported", L, [(
        "    !declaration.test\n        && declaration.kind != ModuleDeclarationKind::NativeType",
        "    declaration.kind != ModuleDeclarationKind::NativeType",
    )]),
    mutant("enum variants do not use their enum", L, [(
        "                .map(|variant| variant.semantic_id),",
        "                .map(|variant| variant.semantic_id).take(0),",
    )]),
    mutant("used-by-dead-code message is never chosen", L, [(
        "        referenced[target] = true;",
        "",
    )]),
    mutant("public functions' needs are checked", L, [(
        "            || (declaration.public && !is_main)",
        "            || false",
    )]),
    mutant("a reached need is reported", L, [(
        "                .any(|need| need.lexical_name == name.text && !need.explained);",
        "                .any(|need| need.lexical_name == name.text && need.explained);",
    )]),
    mutant("reached needs include the written ones", T, [(
        "                self.reached_needs.insert(function_symbol, body_fact.needs);\n                inferred.needs = self.union_needs_rows([inferred.needs, body_fact.needs]);",
        "                inferred.needs = self.union_needs_rows([inferred.needs, body_fact.needs]);\n                self.reached_needs.insert(function_symbol, inferred.needs);",
    )]),
    mutant("an emptied import line keeps its newline", L, [(
        " + line_end + usize::from(line_end < rest.len());",
        " + line_end;",
    )]),
    mutant("a wrapped import line gets a fix", L, [(
        "        if self.render().as_deref() != Some(written) {",
        "        if false && self.render().as_deref() != Some(written) {",
    )]),
    mutant("removing a member drops the others", L, [(
        "                            line.members.retain(|name| name != &member.text);",
        "                            line.members.clear();",
    )]),
    mutant("the binding fix replaces the name", L, [(
        "                unused.with_fix(TextRange::empty(range.start() as usize), \"_\")",
        "                unused.with_fix(range, \"_\")",
    )]),
    mutant("a taken underscore name still gets a fix", L, [(
        "            Some(if bound.contains(format!(\"_{}\", symbol.name).as_str()) {",
        "            Some(if false {",
    )]),
    mutant("a trailing comment does not block the fix", L, [(
        " || !rest[..line_end].trim().is_empty() {",
        " {",
    )]),
]
