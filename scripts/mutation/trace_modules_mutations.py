"""Mutants of the modules a trace records and the viewer's use of them (CHI-293).

Run: scripts/mutation/mutate.py scripts/mutation/trace_modules_mutations.py
"""

CRATE = "compiler/chimi"


def mutant(name, file, edits, targets=("trace_show",), filter=""):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("the native runtime writes no modules", "runtime/native/runtime.c", [(
        "    fputs(CHIMI_TRACE_MODULES, output);\n",
        "",
    )]),
    mutant("the interpreter writes no modules", "src/execution.rs", [(
        "                output.push_str(&self.trace_modules);",
        "                let _ = &self.trace_modules;",
    )]),
    mutant("the build embeds no modules", "src/backend/program_data.rs", [(
        "        c_string(&core.trace_modules)",
        "        c_string(\"\")",
    )]),
    mutant("a place is found by file id even with a module", "src/project.rs", [(
        "                PlaceModule::Path(module) => *by_path.get(modules.0.get(module)?)?,",
        "                PlaceModule::Path(_) => place.file_id as usize,",
    )]),
    mutant("a missing module is accepted", "src/project.rs", [(
        "                None => mismatch.missing.push(module.path.clone()),",
        "                None => {}",
    )]),
    mutant("a changed module is accepted", "src/project.rs", [(
        "                    mismatch.changed.push(module.path.clone());\n                    continue;",
        "                    matched.insert(module.path.clone(), module.path.clone());\n                    continue;",
    )]),
    mutant("an old trace is read without a warning", "src/trace_view.rs", [(
        "            None => found.unrecorded = true,",
        "            None => {}",
    )]),
    mutant("a program build fingerprints the whole text", "src/semantic.rs", [(
        "            self.program_fingerprint()\n        }",
        "            self.text_fingerprint()\n        }",
    )], targets=("trace_show", "cli"), filter=""),
    mutant("a test build leaves its tests out of the fingerprint", "src/semantic.rs", [(
        "        if self.declarations.iter().any(|declaration| declaration.test) {",
        "        if false {",
    )]),
    mutant("test items are not blanked", "src/source.rs", [(
        "        (start..stop).for_each(|_| mix(b' '));\n        offset = stop;",
        "        let _ = stop;",
    )]),
    mutant("the text after the last item is kept", "src/source.rs", [(
        "    let bytes = &text.as_bytes()[..end.min(text.len())];",
        "    let bytes = &text.as_bytes()[..text.len() + end * 0];",
    )], targets=("cli",), filter="build_output_is_identical"),
    mutant("the source is not checked", "src/main.rs", [(
        "    let modules = match project.match_trace(&recorded.modules) {",
        "    let modules = match Ok::<_, chimi::project::TraceMismatch>(\n        project.match_trace(&recorded.modules).unwrap_or_default(),\n    ) {",
    )]),
    mutant("the export reads places by file id", "src/trace_export.rs", [(
        "            .place(span.file_id as u32, span.source.0 as u32);",
        "            .place(span.file_id as u32, span.source.0 as u32);\n        let place = crate::trace_view::Place {\n            module: crate::trace_view::PlaceModule::Unrecorded,\n            ..place\n        };",
    )], targets=("trace_export", "trace_show")),
    mutant("a file id the trace has no module for is guessed", "src/project.rs", [(
        "                PlaceModule::Missing => return None,",
        "                PlaceModule::Missing => place.file_id as usize,",
    )]),
    mutant("a file built alone is not matched by its fingerprint", "src/project.rs", [(
        "                        (Some(path), None) => Some(path),",
        "                        (Some(_), None) => None,",
    )]),
    mutant("a fingerprint two modules share is matched anyway", "src/project.rs", [(
        "                        (Some(path), None) => Some(path),\n                        _ => None,",
        "                        (first, _) => first,",
    )]),
]
