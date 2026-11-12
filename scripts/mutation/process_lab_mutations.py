"""Mutants of the supervised-process conformance lab (compiler/chimi/src/process_lab/).

Run: scripts/mutation/mutate.py scripts/mutation/process_lab_mutations.py
Each mutant reintroduces a projector, catalogue, or harness bug that would let
a wrong runtime pass a transcript, or let the lab pass a stage it has not
reached. Results: compiler/chimi/tests/process-lab/MUTATION-RESULTS.txt.
"""

CRATE = "compiler/chimi"

T = "src/process_lab/transcript.rs"
C = "src/process_lab/catalogue.rs"
M = "src/process_lab/mod.rs"


def lib(name, file, edits, filter):
    return dict(name=name, file=file, edits=edits, targets=["lib"], filter=filter)


def lab(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["process_lab"], filter="")


MUTATIONS = [
    lib("supervisor_started kept before U9", T,
        [('            _ => return Ok(None),\n        };\n        let time', '            "supervisor_started" => format!("supervisor={}", self.required_identity("supervisor_id", \'S\')?),\n            _ => return Ok(None),\n        };\n        let time')],
        "process_lab::transcript"),
    lib("clock advances printed", T,
        [('            now = time;\n            None\n', '            now = time;\n            Some(String::new())\n')],
        "process_lab::transcript"),
    lib("time not tracked from advances", T,
        [('            now = time;\n', '            let _ = time;\n')],
        "process_lab::transcript"),
    lib("sequence order unchecked", T,
        [('        if last_sequence.is_some_and(|last| event.sequence <= last) {', '        if last_sequence.is_some_and(|last| event.sequence < last) {')],
        "process_lab::transcript"),
    lib("event time not checked against the clock", T,
        [('        if time != now {', '        if time != now && false {')],
        "process_lab::transcript"),
    lib("unknown cause word accepted", T,
        [('            Some(JsonData::Text(word)) if allowed.contains(&word.as_str()) => Ok(word.clone()),', '            Some(JsonData::Text(word)) => Ok(word.clone()),')],
        "process_lab::transcript"),
    lib("named send without a name accepted", T,
        [('                if kind == "named" {\n                    fields.push_str(&format!(" name={}", self.required_label()?));', '                if kind == "named" {\n                    fields.push_str(&format!(" name={}", self.label()?.unwrap_or_default()));')],
        "process_lab::transcript"),
    lib("root escalation parent printed as -", T,
        [('                        "root".to_owned()\n', '                        parent.clone()\n')],
        "process_lab::transcript"),
    lib("boundaries keep sends", T,
        [('        .filter(|line| line.split(\' \').nth(1) != Some(PROCESS_SEND))', '        .filter(|_| true)')],
        "process_lab::transcript"),
    lib("duration keeps trailing zeros", T,
        [("    format!(\"{whole}.{}\", fraction.trim_end_matches('0'))", '    format!("{whole}.{fraction}")')],
        "process_lab::transcript"),
    lib("duration drops the zero minute", T,
        [('    if minutes > 0 || hours > 0 && seconds > 0 {', '    if minutes > 0 {')],
        "process_lab::transcript"),
    lib("extra events ignored", T,
        [('        } else if extra_events.contains(&event.name.as_str()) {', '        } else if false {')],
        "process_lab::transcript"),
    lib("catalogue unlock typo", C,
        [('    fixture("RR02", "removal-during-replacement-start", U7),', '    fixture("RR02", "removal-during-replacement-start", U6),')],
        "process_lab::catalogue"),
    lib("catalogue extra event dropped", C,
        [('    fixture("SD05", "member-children-end-before-next-member", U8).with_events(TASK_TERMINAL),', '    fixture("SD05", "member-children-end-before-next-member", U8),')],
        "process_lab::catalogue"),
    lab("locked scenarios run", M,
        [('    if scenario.unlock > options.through {', '    if false {')]),
    lab("unlocked failures pass", M,
        [('        Err(detail) => Status::Fail { detail },', '        Err(_) => Status::Pass,')]),
]
