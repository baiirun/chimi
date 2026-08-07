"""Pure throwaway state model for the trace-data surface decision."""

from __future__ import annotations

from dataclasses import dataclass, replace
import json
from typing import Any


@dataclass(frozen=True)
class Summary:
    records: int = 0
    events: int = 0
    operations: tuple[tuple[str, int], ...] = ()
    dependency_calls: tuple[tuple[str, int], ...] = ()
    terminal_outcomes: tuple[tuple[str, int], ...] = ()
    execution_id: str | None = None
    computation: str | None = None
    last_sequence: int | None = None
    terminal_seen: bool = False
    peak_line_bytes: int = 0


@dataclass(frozen=True)
class Transition:
    line: str
    state: Summary
    error: str | None = None


@dataclass(frozen=True)
class Scenario:
    title: str
    question: str
    source: str
    lines: tuple[str, ...]
    note: str


def update_count(
    entries: tuple[tuple[str, int], ...], key: str
) -> tuple[tuple[str, int], ...]:
    counts = dict(entries)
    counts[key] = counts.get(key, 0) + 1
    return tuple(sorted(counts.items(), key=lambda entry: entry[0].encode("utf-8")))


def accept_line(state: Summary, line: str, line_number: int) -> Summary:
    size = len(line.encode("utf-8"))
    if line == "":
        raise ValueError(f"line {line_number}: blank record")
    if state.terminal_seen:
        raise ValueError(f"line {line_number}: record follows terminal outcome")
    try:
        value = json.loads(line, object_pairs_hook=_unique_object)
    except (json.JSONDecodeError, ValueError) as error:
        raise ValueError(f"line {line_number}: {error}") from error
    if not isinstance(value, dict):
        raise ValueError(f"line {line_number}: record must be a JSON object")

    next_state = replace(
        state,
        records=state.records + 1,
        peak_line_bytes=max(state.peak_line_bytes, size),
    )
    if "event" in value:
        return _accept_event(next_state, value, line_number)
    if "outcome" in value:
        outcome = value["outcome"]
        if outcome not in {"success", "expected_failure", "defect", "cancellation"}:
            raise ValueError(f"line {line_number}: unknown terminal outcome")
        return replace(
            next_state,
            terminal_outcomes=update_count(next_state.terminal_outcomes, outcome),
            terminal_seen=True,
        )
    raise ValueError(f"line {line_number}: unknown record shape")


def run(lines: tuple[str, ...]) -> tuple[Transition, ...]:
    state = Summary()
    transitions: list[Transition] = []
    for line_number, line in enumerate(lines, start=1):
        try:
            state = accept_line(state, line, line_number)
            transitions.append(Transition(line=line, state=state))
        except ValueError as error:
            transitions.append(Transition(line=line, state=state, error=str(error)))
            break
    return tuple(transitions)


def render_summary(state: Summary) -> str:
    document: dict[str, Any] = {
        "schema": "chimi.trace-summary.v1",
        "records": state.records,
        "events": state.events,
        "operations": dict(state.operations),
        "dependency_calls": dict(state.dependency_calls),
        "terminal_outcomes": dict(state.terminal_outcomes),
    }
    if state.execution_id is not None:
        document["execution_id"] = state.execution_id
    if state.computation is not None:
        document["computation"] = state.computation
    return json.dumps(document, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def _accept_event(state: Summary, value: dict[str, Any], line_number: int) -> Summary:
    sequence = value.get("sequence")
    if not isinstance(sequence, int) or isinstance(sequence, bool):
        raise ValueError(f"line {line_number}: event sequence must be an integer")
    if state.last_sequence is not None and sequence <= state.last_sequence:
        raise ValueError(f"line {line_number}: event sequence must increase")
    execution_id = _consistent_optional(
        state.execution_id, value.get("execution_id"), "execution_id", line_number
    )
    computation = _consistent_optional(
        state.computation, value.get("computation"), "computation", line_number
    )
    operations = state.operations
    dependencies = state.dependency_calls
    if value["event"] == "operation":
        operation = value.get("operation")
        if not isinstance(operation, str):
            raise ValueError(f"line {line_number}: operation name must be text")
        operations = update_count(operations, operation)
    if value["event"] == "dependency_call":
        dependency = value.get("dependency")
        method = value.get("method")
        if not isinstance(dependency, str) or not isinstance(method, str):
            raise ValueError(f"line {line_number}: dependency call needs text names")
        dependencies = update_count(dependencies, f"{dependency}.{method}")
    return replace(
        state,
        events=state.events + 1,
        operations=operations,
        dependency_calls=dependencies,
        execution_id=execution_id,
        computation=computation,
        last_sequence=sequence,
    )


def _consistent_optional(
    current: str | None, incoming: Any, field: str, line_number: int
) -> str | None:
    if incoming is None:
        return current
    if not isinstance(incoming, str):
        raise ValueError(f"line {line_number}: {field} must be text or JSON null")
    if current is not None and incoming != current:
        raise ValueError(f"line {line_number}: mixed {field}")
    return incoming


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate object key {key!r}")
        result[key] = value
    return result


SCENARIOS: tuple[Scenario, ...] = (
    Scenario(
        title="External JSON is a closed value",
        question="Can JSON null stay distinct from Chimi absence?",
        source="""import std/json as Json
import std/json.{Value}

case Json.decode(line) {
  Value.Null -> Failure(TraceError.InvalidShape)
  Value.Object(fields) -> decode_record(fields)
  _ -> Failure(TraceError.InvalidShape)
}""",
        lines=('null',),
        note="Value.Null is external data. Option<T> still represents absence in Chimi APIs.",
    ),
    Scenario(
        title="Domain decoding stays ordinary",
        question="Do we need derives, reflection, or typed-decoder syntax?",
        source="""fn decode_event(fields: Map<Text, Value>) {
  case Map.get(fields, \"event\") {
    Some(Value.Text(name)) -> Event(name: name)
    Some(_) -> Failure(TraceError.InvalidField)
    None -> Failure(TraceError.MissingField)
  }
}""",
        lines=(
            '{"event":"operation","sequence":1,"operation":"constant",'
            '"execution_id":null}',
        ),
        note="Json.decode owns syntax; patterns and functions own the trace schema.",
    ),
    Scenario(
        title="Immutable keyed aggregation",
        question="Can one update API count dynamic keys without general mutation?",
        source="""fn increment(counts: Map<Text, Int>, key: Text) {
  Map.update(counts, key, fn(current) {
    case current {
      Some(count) -> count + 1
      None -> 1
    }
  })
}""",
        lines=(
            '{"event":"operation","sequence":1,"operation":"call"}',
            '{"event":"operation","sequence":2,"operation":"constant"}',
            '{"event":"operation","sequence":3,"operation":"call"}',
        ),
        note="The old map remains valid; absence is an Option passed to the updater.",
    ),
    Scenario(
        title="Producer-owned streaming processing",
        question="Do lines require a general iterator abstraction?",
        source="""FileSystem.process_utf8_lines(path, Summary.empty(), fn(summary, line) {
  let record = line |> Json.decode |> TraceRecord.decode
  Summary.add(summary, record)
})""",
        lines=(
            '{"event":"operation","sequence":1,"operation":"call"}',
            '{"outcome":"success"}',
        ),
        note="Only the running state and current bounded line survive each callback.",
    ),
    Scenario(
        title="Deterministic keyed output",
        question="Can output ignore insertion order and host locale?",
        source="""let operations = Map.map_values(summary.operations, Json.int)
Json.encode(Json.object(operations))""",
        lines=(
            '{"event":"operation","sequence":1,"operation":"zeta"}',
            '{"event":"operation","sequence":2,"operation":"alpha"}',
            '{"outcome":"success"}',
        ),
        note="JSON object encoding uses ascending UTF-8 byte order.",
    ),
    Scenario(
        title="Invalid transitions stop processing",
        question="Can expected failures prevent a plausible partial report?",
        source="""case summarize(path) {
  Success(summary) -> Console.write_line(Json.encode(summary))
  Failure(error) -> Console.write_error(TraceError.display(error))
}""",
        lines=(
            '{"event":"operation","sequence":2,"operation":"call"}',
            '{"event":"operation","sequence":1,"operation":"constant"}',
            '{"outcome":"success"}',
        ),
        note="Failed processing returns no Summary; the shell never receives partial output.",
    ),
)
