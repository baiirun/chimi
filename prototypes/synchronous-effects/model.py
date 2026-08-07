"""Pure state model for the synchronous-effects prototype."""

from dataclasses import dataclass, replace


@dataclass(frozen=True)
class Operation:
    name: str
    failures: tuple[str, ...] = ()
    needs: tuple[tuple[str, str], ...] = ()
    effects: tuple[str, ...] = ()


@dataclass(frozen=True)
class CallableShape:
    inputs: tuple[str, ...]
    success: str
    failures: tuple[str, ...] = ()
    needs: tuple[tuple[str, str], ...] = ()
    effects: tuple[str, ...] = ()


@dataclass(frozen=True)
class PrototypeState:
    callable: CallableShape
    runner_configuration: tuple[str, ...]
    last_action: str


FILES = Operation(
    "FileSystem.fold_utf8_lines",
    failures=("FileError",),
    needs=(("files", "FileSystem"),),
    effects=("io",),
)
CONSOLE = Operation(
    "Console.write_line",
    failures=("ConsoleError",),
    needs=(("console", "Console"),),
    effects=("io",),
)


def initial_state() -> PrototypeState:
    return PrototypeState(
        callable=CallableShape(
            inputs=("List<Text>",),
            success="Void",
            failures=("UsageError",),
        ),
        runner_configuration=("trace", "execution_id", "cancellation", "limits"),
        last_action="reset to pure argument validation",
    )


def _union(left: tuple[str, ...], right: tuple[str, ...]) -> tuple[str, ...]:
    return tuple(sorted(set(left) | set(right)))


def _union_needs(
    left: tuple[tuple[str, str], ...], right: tuple[tuple[str, str], ...]
) -> tuple[tuple[str, str], ...]:
    combined = dict(left)
    for name, interface in right:
        existing = combined.get(name)
        if existing is not None and existing != interface:
            raise ValueError(f"need {name} cannot be both {existing} and {interface}")
        combined[name] = interface
    return tuple(sorted(combined.items()))


def call(state: PrototypeState, operation: Operation) -> PrototypeState:
    shape = state.callable
    return replace(
        state,
        callable=replace(
            shape,
            failures=_union(shape.failures, operation.failures),
            needs=_union_needs(shape.needs, operation.needs),
            effects=_union(shape.effects, operation.effects),
        ),
        last_action=f"called {operation.name}; unioned failures, needs, and effects",
    )


def provision(state: PrototypeState) -> PrototypeState:
    supplied = {"files", "console"}
    shape = state.callable
    return replace(
        state,
        callable=replace(
            shape,
            needs=tuple(need for need in shape.needs if need[0] not in supplied),
        ),
        last_action="provided files and console; effects and failures did not change",
    )


def recover(state: PrototypeState) -> PrototypeState:
    shape = state.callable
    recoverable = next(
        (failure for failure in ("FileError", "ConsoleError", "UsageError") if failure in shape.failures),
        None,
    )
    if recoverable is None:
        return replace(state, last_action="no expected failure remains to recover")
    return replace(
        state,
        callable=replace(
            shape,
            failures=tuple(failure for failure in shape.failures if failure != recoverable),
        ),
        last_action=f"recovered {recoverable}; needs and effects did not change",
    )


def guard_diagnostic(state: PrototypeState) -> str:
    shape = state.callable
    exposed = []
    if shape.failures:
        exposed.append("expected failures " + ", ".join(shape.failures))
    if shape.needs:
        exposed.append("dependency needs " + ", ".join(name for name, _ in shape.needs))
    if shape.effects:
        exposed.append("effects " + ", ".join(shape.effects))
    if not exposed:
        return "guard accepted: callable is pure and total"
    return "E_EFFECTFUL_GUARD: guard exposes " + "; ".join(exposed)
