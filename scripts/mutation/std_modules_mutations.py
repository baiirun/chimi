"""Mutants of the standard modules written in Chimi (ts-0ba63e): the native
cores of std/text, std/list, and std/option on both backends, the Chimi
source of std/text and std/list, the registry binding of native fns and
runtime-known declarations, std loading, and the diagnostics and runtime
behaviour the port changed.

Run: scripts/mutation/mutate.py scripts/mutation/std_modules_mutations.py
Each mutant must turn tests/std_modules.rs (the std-tests programs, run on
the interpreter and natively), the formatter's lib tests, or the native
memory tests in tests/cli.rs red.
"""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"
CORE = "src/execution/core_operations.rs"
SOURCE = "src/resolution/standard_source.rs"
TEXT = "std/text.chimi"
LIST = "std/list.chimi"


def std(name, file, edits, filter=""):
    return dict(name=name, file=file, edits=edits, targets=["std_modules"], filter=filter)


MUTATIONS = [
    # Native cores: each must agree with its expected outcome on both backends.
    std("native byte_slice skips the UTF-8 boundary check", RUNTIME, [(
        "      if (!utf8_boundary(text, length, from))",
        "      if (false)")], filter="standard_programs"),
    std("interpreter byte_slice skips the UTF-8 boundary check", CORE, [(
        "                    if !value.is_char_boundary(bound) {",
        "                    if bound > value.len() {")], filter="standard_programs"),
    std("native trim misses ideographic space", RUNTIME, [(
        "code == 0x205f || code == 0x3000;",
        "code == 0x205f;")], filter="standard_programs"),
    std("interpreter split accepts an empty separator", CORE, [(
        "                if separator.is_empty() {\n"
        "                    return defect(\"Text.split: separator must not be empty\".to_owned());",
        "                if separator.is_empty() && false {\n"
        "                    return defect(\"Text.split: separator must not be empty\".to_owned());")],
        filter="standard_programs"),
    std("native List.set accepts the index equal to the length", RUNTIME, [(
        "      if (index < 0 || (uint64_t)index >= list_length(list))",
        "      if (index < 0 || (uint64_t)index > list_length(list))")], filter="standard_programs"),
    std("native unwrap_or_fail fails on Some", RUNTIME, [(
        "      if (arguments[0]->as.constructed.length == 1) return",
        "      if (arguments[0]->as.constructed.length == 0) return")], filter="standard_programs"),
    std("native Debug.crash drops the caller's message", RUNTIME, [(
        "      return defect(copy_text(arguments[0]->as.text.bytes), call_node);",
        "      return defect(\"Debug.crash\", call_node);")], filter="standard_programs"),
    # Chimi source of the modules.
    std("Text.lines keeps a final empty line", TEXT, [(
        "    [..most, \"\"] -> most\n",
        "    [..most, \"\"] -> split(text, \"\\n\")\n")], filter="standard_programs"),
    std("List.sort_by merges the right half first, losing stability", LIST, [(
        "      merge(sort_by(take(list, middle), compare), sort_by(drop(list, middle), compare), compare)",
        "      merge(sort_by(drop(list, middle), compare), sort_by(take(list, middle), compare), compare)")],
        filter="standard_programs"),
    # Registry binding and loading.
    std("runtime-known declarations keep their source identity", SOURCE, [(
        "        self.standard_symbols.insert(symbol, identity);",
        "        return;")], filter="standard_programs"),
    std("a native fn outside the standard tree is accepted", SOURCE, [(
        "        if !self.is_standard_module(module_index) {\n"
        "            self.diagnostics.push(name_error(\n"
        "                &module,\n"
        "                \"N3101\",",
        "        if false {\n"
        "            self.diagnostics.push(name_error(\n"
        "                &module,\n"
        "                \"N3101\",")], filter="native_fn_outside"),
    std("a workspace std/ module is accepted", SOURCE, [(
        "        if module.path.starts_with(\"std/\") && !self.is_standard_module(module_index) {",
        "        if false {")], filter="under_std"),
    std("std imports of std modules are not followed", "src/standard_tree.rs", [(
        "        pending.extend(imports(&module));",
        "        let _ = imports(&module);")], filter="transitive_standard_imports"),
    std("a function named native is read as a native fn", "src/semantic.rs", [(
        "            .take_while(|token| token.kind() != TokenKind::Fn)\n",
        "")], filter="ordinary_name"),
    std("a standard requirement is reported inside std source", "src/typing.rs", [(
        "            .is_some_and(|origin| self.resolved.is_standard_module(origin.module_id))",
        "            .is_some_and(|_| false)")], filter="reported_at_the_call"),
    dict(name="the formatter joins `>` and a following word", file="src/formatter.rs",
         edits=[("    (word_kind(previous) || previous == TokenKind::Greater) && word_kind(current)",
                 "    word_kind(previous) && word_kind(current)")],
         targets=["lib"], filter="embedded_standard_modules"),
    # Native frames hold only the locals reachable code binds.
    dict(name="every program local takes a frame slot", file="src/executable.rs",
         edits=[("            || (symbol.kind == CoreSymbolKind::Local && !bound.contains(&symbol.id))",
                 "")],
         targets=["cli"], filter="native_draining_loop_allocates"),
]
