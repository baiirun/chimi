"""Pure validation model for the disposable Chimi kernel validator."""

from dataclasses import dataclass
from enum import Enum


class Evidence(Enum):
    CONNECTED = "connected"
    INDEPENDENT = "independent"
    DEFERRED = "deferred"


@dataclass(frozen=True)
class Stage:
    name: str
    input: str
    output: str
    command: tuple[str, ...]
    expected_summary: str


@dataclass(frozen=True)
class Seam:
    source: str
    target: str
    evidence: Evidence
    detail: str


@dataclass(frozen=True)
class ProbeResult:
    stage: Stage
    passed: bool
    summary: str


@dataclass(frozen=True)
class Verdict:
    probes_pass: bool
    coherent: bool
    specification_ready: bool
    end_to_end_compiler: bool
    blockers: tuple[str, ...]


STAGES = (
    Stage(
        "surface grammar",
        ".chimi source",
        "syntax acceptance",
        ("python3", "prototypes/grammar/main.py", "--all"),
        "20/20 fixtures match the draft grammar expectation",
    ),
    Stage(
        "lossless parser",
        ".chimi source",
        "lossless CST + syntax diagnostics",
        ("go", "-C", "prototypes/lossless-parser", "run", ".", "--check"),
        "24/24 fixtures satisfy lossless and syntax-diagnostic expectations",
    ),
    Stage(
        "semantic lowering",
        "lossless CST",
        "source-mapped semantic file",
        ("go", "-C", "prototypes/semantic-lowering", "run", ".", "--check"),
        "24/24 fixtures satisfy lowering-stage expectations",
    ),
    Stage(
        "name resolution",
        "source-mapped semantic modules",
        "stable symbols + name diagnostics",
        ("go", "-C", "prototypes/name-resolution", "run", ".", "--check"),
        "10/10 scenarios satisfy name-resolution expectations",
    ),
    Stage(
        "type contracts",
        "source-shaped contract scenarios",
        "success/error/needs contracts",
        ("go", "-C", "prototypes/type-contracts", "run", ".", "--check"),
        "17/17 scenarios satisfy type-contract expectations",
    ),
    Stage(
        "observable runner",
        "hand-built executable nodes",
        "outcomes + structured events",
        ("go", "-C", "prototypes/runner", "run", ".", "--check"),
        "10/10 scenarios satisfy executable semantics",
    ),
    Stage(
        "connected typing",
        "resolved Rust semantic project",
        "typed callable contracts",
        (
            "cargo",
            "test",
            "--quiet",
            "-p",
            "chimi",
            "--lib",
            "compiler::tests::calls_infer_failure_and_needs_channels_from_function_bodies",
            "--",
            "--exact",
        ),
        "test result: ok. 1 passed",
    ),
    Stage(
        "connected execution",
        "typed Rust project",
        "verified core + execution report",
        (
            "cargo",
            "test",
            "--quiet",
            "-p",
            "chimi",
            "--lib",
            "compiler::tests::project_core_executes_the_dependency_forcing_program_from_source",
            "--",
            "--exact",
        ),
        "test result: ok. 1 passed",
    ),
    Stage(
        "tooling seams",
        "lossless CST + semantic file",
        "formatting + diagnostics + inspection",
        ("go", "-C", "prototypes/tooling-seams", "run", ".", "--check"),
        "4/4 tooling scenarios satisfy the proposed seams",
    ),
)


SEAMS = (
    Seam(
        "source",
        "CST",
        Evidence.CONNECTED,
        "The lossless parser reads the golden corpus and preserves every byte.",
    ),
    Seam(
        "CST",
        "semantic representation",
        Evidence.CONNECTED,
        "The lowerer directly consumes parser GreenNode values.",
    ),
    Seam(
        "semantic representation",
        "resolved names",
        Evidence.CONNECTED,
        "The resolver parses and lowers real module fixtures before resolving them.",
    ),
    Seam(
        "resolved names",
        "typed contracts",
        Evidence.CONNECTED,
        "The Rust checker consumes the resolver's project and infers callable success, failure, and needs channels.",
    ),
    Seam(
        "typed contracts",
        "executable core",
        Evidence.CONNECTED,
        "The Rust compiler seals a typed project, lowers its verified root, and executes the resulting core.",
    ),
    Seam(
        "CST + semantics",
        "developer tooling",
        Evidence.CONNECTED,
        "Formatting and inspection directly consume the parser and lowerer outputs.",
    ),
    Seam(
        "frontend",
        "incremental performance",
        Evidence.DEFERRED,
        "Workloads and local phase measurements exist, but no incremental semantic engine is connected.",
    ),
    Seam(
        "executable core",
        "durable production runtime",
        Evidence.DEFERRED,
        "Scheduling, supervision, durability, recovery, and a permanent backend remain out of kernel scope.",
    ),
)


def evaluate(results: tuple[ProbeResult, ...]) -> Verdict:
    probes_pass = len(results) == len(STAGES) and all(result.passed for result in results)
    required = {
        ("source", "CST"),
        ("CST", "semantic representation"),
        ("semantic representation", "resolved names"),
        ("CST + semantics", "developer tooling"),
    }
    connected = {
        (seam.source, seam.target)
        for seam in SEAMS
        if seam.evidence is Evidence.CONNECTED
    }
    coherent = probes_pass and required <= connected
    compiler_required = {
        ("resolved names", "typed contracts"),
        ("typed contracts", "executable core"),
    }
    blockers = tuple(
        f"{seam.source} -> {seam.target}: {seam.detail}"
        for seam in SEAMS
        if seam.evidence is Evidence.INDEPENDENT
    )
    return Verdict(
        probes_pass=probes_pass,
        coherent=coherent,
        specification_ready=coherent,
        end_to_end_compiler=coherent and compiler_required <= connected and not blockers,
        blockers=blockers,
    )
