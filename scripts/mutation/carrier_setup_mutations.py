"""Mutants of R4 step 3 PR 5c-3: a wake pipe that fails to open is a
scheduler error and leaves nothing open.

Run: scripts/mutation/mutate.py --timeout 120 scripts/mutation/carrier_setup_mutations.py
Results: scripts/mutation/carrier_setup_results.txt.
"""

CRATE = "runtime/chimi-fiber"

S = "src/scheduler.rs"
W = "src/scheduler/wake.rs"
ERROR = "a_kernel_whose_wake_pipe_fails_is_an_error"
CLOSED = "a_failed_pipe_closes_the_ones_opened_before_it"


def m(name, file, old, new, test):
    return dict(name=name, file=file, edits=[(old, new)], targets=["lib"], filter=test)


MUTATIONS = [
    m("a failed pipe panics", S,
      "            .map_err(|error| SchedulerError::CarrierSetupFailed(error.raw_os_error()))?;",
      "            .expect(\"a wake pipe per carrier\");", ERROR),
    m("a failed pipe loses its error number", S,
      "SchedulerError::CarrierSetupFailed(error.raw_os_error())",
      "SchedulerError::CarrierSetupFailed({ let _ = error; None })", ERROR),
    m("a failed set keeps the pipes opened before it", W,
      "            (0..carriers).map(|_| Pipe::open()).collect::<io::Result<_>>()?",
      "            {\n                let pipes: Vec<io::Result<Pipe>> = (0..carriers).map(|_| Pipe::open()).collect();\n                if pipes.iter().any(Result::is_err) {\n                    let error = io::Error::from_raw_os_error(libc::EMFILE);\n                    std::mem::forget(pipes);\n                    return Err(error);\n                }\n                pipes.into_iter().map(Result::unwrap).collect()\n            }", CLOSED),
    m("carriers are installed with one queue", S,
      "        self.ready = ready::ReadyQueues::new(carriers);\n        self.jobs = jobs::Jobs::new(carriers);\n        self.wakes = wakes;",
      "        self.ready = ready::ReadyQueues::new(1);\n        self.jobs = jobs::Jobs::new(carriers);\n        self.wakes = wakes;", ERROR),
    m("a pipe's ends are never closed", W,
      "            let _closed = sys::close(fd);",
      "            let _closed = { let _ = fd; true };", CLOSED),
    m("a pipe closes only its read end", W,
      "        for fd in [self.read, self.write] {",
      "        for fd in [self.read] {", CLOSED),
    m("a failed install changes the queues first", S,
      "        let wakes = wake::Wakes::new(carriers)\n            .map_err(|error| SchedulerError::CarrierSetupFailed(error.raw_os_error()))?;\n        self.ready = ready::ReadyQueues::new(carriers);",
      "        self.ready = ready::ReadyQueues::new(carriers);\n        let wakes = wake::Wakes::new(carriers)\n            .map_err(|error| SchedulerError::CarrierSetupFailed(error.raw_os_error()))?;", ERROR),
]
