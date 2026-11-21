"""Mutants of R4 step 3 PR 5d-3 (fiber ABI 29): the scheduler config's
carrier count and carrier-enter hook in `chimi_scheduler_run`.

Run: scripts/mutation/mutate.py --timeout 180 scripts/mutation/carrier_config_mutations.py
Results: scripts/mutation/carrier_config_results.txt.
"""

CRATE = "runtime/chimi-fiber"

P = "src/scheduler/process_abi.rs"


def m(name, old, new, file=P):
    return dict(name=name, file=file, edits=[(old, new)], targets=["lib"], filter="")


MUTATIONS = [
    m("several carriers run under virtual time",
      "    if carriers > 1 && clock == ClockMode::Virtual {",
      "    if carriers > 1 && clock == ClockMode::Virtual && false {"),
    m("0 carriers is taken literally",
      "    let carriers = config.carriers.max(1);",
      "    let carriers = config.carriers;"),
    m("the carrier count is ignored",
      "    let Ok(mut scheduler) = Scheduler::with_options_carriers(options, carriers) else {",
      "    let Ok(mut scheduler) = Scheduler::with_options_carriers(options, 1) else {"),
    m("the enter hook is never called",
      "                        if let Some(enter) = enter {\n                            enter.call(index);\n                        }",
      ""),
    m("the enter hook names the wrong carrier",
      "                            enter.call(index);",
      "                            enter.call(index - 1);"),
    m("the enter hook gets no context",
      "    let enter = config.carrier_enter.map(|hook| EnterHook {\n        hook,\n        context: config.carrier_context,\n    });",
      "    let enter = config.carrier_enter.map(|hook| EnterHook {\n        hook,\n        context: std::ptr::null_mut(),\n    });"),
    m("the enter hook runs after the carrier's first drive",
      "                        if let Some(enter) = enter {\n                            enter.call(index);\n                        }\n                        super::carrier::run_carrier(&mut Locked::new(kernel, Carrier(index)));",
      "                        super::carrier::run_carrier(&mut Locked::new(kernel, Carrier(index)));\n                        if let Some(enter) = enter {\n                            enter.call(index);\n                        }"),
]
