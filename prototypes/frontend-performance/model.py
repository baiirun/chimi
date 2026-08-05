"""Pure workload model for the disposable frontend performance prototype."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any


COMMON_INVARIANTS = (
    "all source bytes are represented by the lossless CST",
    "parsing consumes the complete input and never aborts",
    "every diagnostic and missing marker has a valid source location",
    "repeated runs produce structurally equivalent output and diagnostics",
)


@dataclass(frozen=True)
class Workload:
    id: str
    title: str
    question: str
    phases: tuple[str, ...]
    file_count: int
    declarations_per_file: int
    mutation: str
    metrics: tuple[str, ...]
    invariants: tuple[str, ...]
    target_behavior: str


@dataclass(frozen=True)
class Edit:
    path: str
    offset: int
    delete_bytes: int
    insert_text: str
    description: str


@dataclass(frozen=True)
class BenchmarkCase:
    workload: Workload
    before: dict[str, str]
    after: dict[str, str]
    edit: Edit | None

    @property
    def input_bytes(self) -> int:
        return sum(len(source.encode("utf-8")) for source in self.before.values())

    @property
    def input_lines(self) -> int:
        return sum(source.count("\n") + (1 if source else 0) for source in self.before.values())

    @property
    def edited_bytes(self) -> int:
        return sum(len(source.encode("utf-8")) for source in self.after.values())


WORKLOADS = (
    Workload(
        id="cold.small-project",
        title="Cold small-project check",
        question="How much fixed startup cost precedes useful diagnostics?",
        phases=("process startup", "lex", "parse", "lower", "check"),
        file_count=4,
        declarations_per_file=12,
        mutation="none",
        metrics=("wall time to first diagnostic", "total wall time", "peak RSS"),
        invariants=COMMON_INVARIANTS,
        target_behavior="Record cold p50 and p95; set the absolute budget after the native prototype and benchmark machine exist.",
    ),
    Workload(
        id="syntax.typical-valid",
        title="Typical valid file",
        question="Is full lossless lexing and parsing comfortably inside an editor frame?",
        phases=("lex", "parse", "green CST build"),
        file_count=1,
        declarations_per_file=250,
        mutation="none",
        metrics=("p50/p95 wall time", "MiB/s", "allocated bytes", "peak live bytes"),
        invariants=COMMON_INVARIANTS,
        target_behavior="p95 syntax availability <= 16 ms on the recorded reference machine.",
    ),
    Workload(
        id="syntax.large-valid",
        title="Large valid file",
        question="Does frontend cost remain linear when one file is unusually large?",
        phases=("lex", "parse", "green CST build", "CST traversal"),
        file_count=1,
        declarations_per_file=5_000,
        mutation="none",
        metrics=("p50/p95 wall time", "MiB/s", "bytes allocated per source byte"),
        invariants=COMMON_INVARIANTS,
        target_behavior="Linear growth from the typical-file case; establish throughput floor from measured native baselines.",
    ),
    Workload(
        id="syntax.malformed-brace",
        title="Malformed record/block boundary",
        question="Does brace lookahead recover predictably when a record field loses its colon?",
        phases=("lex", "parse", "green CST build", "diagnostics"),
        file_count=1,
        declarations_per_file=500,
        mutation="remove_record_colon",
        metrics=("p50/p95 wall time", "diagnostic count", "recovered bytes", "peak RSS"),
        invariants=COMMON_INVARIANTS
        + ("diagnostics remain bounded by damaged regions",),
        target_behavior="No catastrophic slowdown; malformed p95 <= 2x the corresponding valid full parse.",
    ),
    Workload(
        id="syntax.malformed-pipeline",
        title="Incomplete pipeline",
        question="Can an unfinished pipeline preserve the surrounding block and later declarations?",
        phases=("lex", "parse", "green CST build", "diagnostics"),
        file_count=1,
        declarations_per_file=500,
        mutation="remove_pipeline_transform",
        metrics=("p50/p95 wall time", "diagnostic count", "recovered bytes"),
        invariants=COMMON_INVARIANTS
        + ("declarations after the damaged pipeline remain structurally available",),
        target_behavior="No catastrophic slowdown; one local edit must not produce an unbounded diagnostic cascade.",
    ),
    Workload(
        id="editor.prefix-edit",
        title="Edit at file prefix",
        question="What is the worst full-file reparse cost when every later offset moves?",
        phases=("text edit", "reparse", "syntax diagnostics"),
        file_count=1,
        declarations_per_file=500,
        mutation="insert_prefix_space",
        metrics=("edit-to-syntax p50/p95", "reused-node ratio", "invalidated bytes"),
        invariants=COMMON_INVARIANTS
        + ("full and incremental outputs are structurally equivalent",),
        target_behavior="p95 edit-to-syntax <= 16 ms for the generated editor-sized file.",
    ),
    Workload(
        id="editor.middle-brace-edit",
        title="Edit at record/block ambiguity",
        question="How much syntax and diagnostic state changes when a middle-of-file colon is deleted?",
        phases=("text edit", "reparse", "syntax diagnostics"),
        file_count=1,
        declarations_per_file=500,
        mutation="remove_middle_record_colon",
        metrics=("edit-to-syntax p50/p95", "reused-node ratio", "changed diagnostics", "invalidated bytes"),
        invariants=COMMON_INVARIANTS
        + ("unchanged diagnostics retain stable identity where possible",),
        target_behavior="p95 edit-to-syntax <= 16 ms and invalidation remains delimiter-bounded when incrementality exists.",
    ),
    Workload(
        id="cst.project-storage",
        title="Lossless CST storage",
        question="What memory multiple does the retained green CST impose on a project?",
        phases=("lex", "parse", "retain green roots", "traverse"),
        file_count=64,
        declarations_per_file=150,
        mutation="none",
        metrics=("retained bytes", "bytes per source byte", "peak RSS", "traversal wall time"),
        invariants=COMMON_INVARIANTS,
        target_behavior="Measure explicit-trivia and compact-trivia representations before setting a memory ceiling.",
    ),
    Workload(
        id="semantics.local-edit",
        title="Local semantic edit",
        question="Can a leaf-module edit update names and types without rechecking unrelated modules?",
        phases=("text edit", "reparse", "lower", "resolve", "type check", "diagnostics"),
        file_count=256,
        declarations_per_file=20,
        mutation="change_leaf_literal",
        metrics=("edit-to-semantic p50/p95", "rechecked modules", "invalidated queries", "peak RSS"),
        invariants=COMMON_INVARIANTS
        + ("incremental and clean project checks produce equivalent diagnostics",),
        target_behavior="p95 local edit-to-semantic diagnostics <= 100 ms; unrelated modules are not rechecked.",
    ),
    Workload(
        id="semantics.fanout-edit",
        title="High-fanout semantic edit",
        question="How does checking scale when a widely imported public contract changes?",
        phases=("text edit", "reparse", "lower", "resolve", "type check", "diagnostics"),
        file_count=256,
        declarations_per_file=20,
        mutation="change_shared_parameter_type",
        metrics=("edit-to-semantic p50/p95", "rechecked modules", "invalidated queries", "diagnostic count"),
        invariants=COMMON_INVARIANTS
        + ("incremental and clean project checks produce equivalent diagnostics",),
        target_behavior="Work scales with the affected dependency closure and never exceeds a clean full-project check.",
    ),
)


def generate_module(index: int, declarations: int, imports_shared: bool) -> str:
    lines = ["// PROTOTYPE GENERATED WORKLOAD — DO NOT EDIT"]
    if imports_shared and index > 0:
        lines.extend(("import generated/module_0000", ""))

    for item in range(declarations):
        suffix = f"{index:04d}_{item:04d}"
        lines.extend(
            (
                f"struct Value_{suffix} {{",
                "  amount: Int",
                "}",
                "",
                f"pub fn increment_{suffix}(value: Int) -> Int {{",
                "  value + 1",
                "}",
                "",
                f"let configuration_{suffix} = {{",
                "  retry_limit: 3,",
                "  tracing: True,",
                "}",
                "",
                f"let transformed_{suffix} = 1",
                f"|> increment_{suffix}",
                "",
            )
        )

    if imports_shared and index > 0:
        lines.extend(
            (
                f"fn use_shared_{index:04d}(value: Int) -> Int {{",
                "  module_0000.increment_0000_0000(value)",
                "}",
                "",
            )
        )
    return "\n".join(lines)


def generate_project(workload: Workload) -> dict[str, str]:
    imports_shared = workload.id.startswith("semantics.")
    return {
        f"generated/module_{index:04d}.chimi": generate_module(
            index,
            workload.declarations_per_file,
            imports_shared,
        )
        for index in range(workload.file_count)
    }


def apply_mutation(project: dict[str, str], mutation: str) -> tuple[dict[str, str], Edit | None]:
    if mutation == "none":
        return dict(project), None

    paths = sorted(project)
    if mutation in {"change_leaf_literal"}:
        path = paths[-1]
    else:
        path = paths[0]
    source = project[path]

    if mutation == "insert_prefix_space":
        offset, delete_bytes, insert_text = 0, 0, " "
        description = "insert one byte before the first token"
    elif mutation == "remove_record_colon":
        offset = source.index("retry_limit:") + len("retry_limit")
        delete_bytes, insert_text = 1, ""
        description = "delete the first record-field colon"
    elif mutation == "remove_middle_record_colon":
        needle = "retry_limit:"
        occurrences = [i for i in range(len(source)) if source.startswith(needle, i)]
        offset = occurrences[len(occurrences) // 2] + len("retry_limit")
        delete_bytes, insert_text = 1, ""
        description = "delete a middle record-field colon"
    elif mutation == "remove_pipeline_transform":
        marker = "|> increment_"
        start = source.index(marker) + len("|> ")
        end = source.index("\n", start)
        offset, delete_bytes, insert_text = start, end - start, ""
        description = "delete the transform after a pipeline operator"
    elif mutation == "change_leaf_literal":
        marker = "value + 1"
        offset = source.index(marker) + len("value + ")
        delete_bytes, insert_text = 1, "2"
        description = "change one implementation literal in a leaf module"
    elif mutation == "change_shared_parameter_type":
        marker = "increment_0000_0000(value: Int)"
        offset = source.index(marker) + len("increment_0000_0000(value: ")
        delete_bytes, insert_text = len("Int"), "Text"
        description = "change a widely called public function parameter type"
    else:
        raise ValueError(f"unknown mutation: {mutation}")

    changed = source[:offset] + insert_text + source[offset + delete_bytes :]
    after = dict(project)
    after[path] = changed
    return after, Edit(path, offset, delete_bytes, insert_text, description)


def build_case(workload: Workload) -> BenchmarkCase:
    before = generate_project(workload)
    after, edit = apply_mutation(before, workload.mutation)
    return BenchmarkCase(workload, before, after, edit)


def result_template(case: BenchmarkCase) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "workload_id": case.workload.id,
        "implementation": {
            "name": None,
            "revision": None,
            "mode": "release",
        },
        "environment": {
            "os": None,
            "architecture": None,
            "cpu": None,
            "logical_cores": None,
            "memory_bytes": None,
        },
        "input": {
            "files": len(case.before),
            "bytes": case.input_bytes,
            "lines": case.input_lines,
            "edit": None
            if case.edit is None
            else {
                "path": case.edit.path,
                "offset": case.edit.offset,
                "delete_bytes": case.edit.delete_bytes,
                "insert_bytes": len(case.edit.insert_text.encode("utf-8")),
            },
        },
        "sampling": {
            "warmup_iterations": None,
            "measured_iterations": None,
            "wall_time_ns": [],
        },
        "memory": {
            "allocated_bytes": None,
            "peak_live_bytes": None,
            "peak_rss_bytes": None,
            "retained_cst_bytes": None,
        },
        "incremental": {
            "reused_nodes": None,
            "total_nodes": None,
            "invalidated_bytes": None,
            "rechecked_modules": None,
            "invalidated_queries": None,
        },
        "outcome": {
            "diagnostic_count": None,
            "recovered_bytes": None,
            "invariants": {invariant: None for invariant in case.workload.invariants},
        },
    }


def source_preview(case: BenchmarkCase, lines: int = 8) -> str:
    path = sorted(case.before)[0]
    return "\n".join(case.before[path].splitlines()[:lines])


def edit_preview(case: BenchmarkCase, radius: int = 36) -> tuple[str, str]:
    assert case.edit is not None
    before = case.before[case.edit.path]
    after = case.after[case.edit.path]
    start = max(0, case.edit.offset - radius)
    before_end = min(len(before), case.edit.offset + case.edit.delete_bytes + radius)
    after_end = min(len(after), case.edit.offset + len(case.edit.insert_text) + radius)
    return before[start:before_end].replace("\n", "\\n"), after[start:after_end].replace("\n", "\\n")
