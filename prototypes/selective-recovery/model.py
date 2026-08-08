"""PROTOTYPE: pure selective-recovery syntax comparison model."""

from dataclasses import dataclass


@dataclass(frozen=True)
class Example:
    title: str
    source: str
    inferred_failures: str
    observation: str


@dataclass(frozen=True)
class Form:
    name: str
    summary: str
    new_language_rule: str
    examples: tuple[Example, ...]


FORMS = (
    Form(
        name="Live outcome matching",
        summary="Match the operation where recovery owns execution.",
        new_language_rule="none",
        examples=(
            Example(
                title="Handle one failure at the call site",
                source="""case checkout() {
  Success(receipt) -> receipt
  Failure(Payment.Declined(reason)) -> alternate_payment(reason)
}""",
                inferred_failures="Inventory.OutOfStock + alternate_payment failures",
                observation="Unmatched live failures bubble without a forwarding arm.",
            ),
            Example(
                title="Compose a reusable thunk transformer",
                source="""fn recover_decline<E>(operation: fn() -> Receipt errors E) {
  fn() {
    case operation() {
      Success(receipt) -> receipt
      Failure(Payment.Declined(reason)) -> alternate_payment(reason)
    }
  }
}

let checkout_with_recovery = checkout |> recover_decline""",
                inferred_failures="E - Payment.Declined + alternate_payment failures",
                observation="The function remains ordinary, partial, and pipeable.",
            ),
            Example(
                title="Handle every failure",
                source="""case checkout() {
  Success(receipt) -> receipt
  Failure(Payment.Declined(reason)) -> alternate_payment(reason)
  Failure(Inventory.OutOfStock(item)) -> backorder(item)
}""",
                inferred_failures="recovery-arm failures only",
                observation="Removing every original variant closes the original failure set.",
            ),
            Example(
                title="Preserve an unrelated failure",
                source="""case checkout() {
  Success(receipt) -> receipt
  Failure(Payment.Declined(reason)) -> alternate_payment(reason)
}""",
                inferred_failures="Inventory.OutOfStock + alternate_payment failures",
                observation="No ordinary value is captured for the unmatched failure.",
            ),
        ),
    ),
    Form(
        name="Captured failure handler",
        summary="Pass an ordinary failure value to a reusable handler.",
        new_language_rule="none",
        examples=(
            Example(
                title="Handle one failure at the call site",
                source="""checkout
|> Recovery.with(fn(error) {
  case error {
    Payment.Declined(reason) -> alternate_payment(reason)
    _ -> Failure(error)
  }
})""",
                inferred_failures="Inventory.OutOfStock + alternate_payment failures",
                observation="Captured values require explicit unchanged forwarding.",
            ),
            Example(
                title="Compose a reusable handler",
                source="""fn recover_decline<E>(error: E) -> Receipt {
  case error {
    Payment.Declined(reason) -> alternate_payment(reason)
    _ -> Failure(error)
  }
}

let checkout_with_recovery = checkout |> Recovery.with(recover_decline)""",
                inferred_failures="E - Payment.Declined + alternate_payment failures",
                observation="The handler is reusable but depends on a recovery combinator.",
            ),
            Example(
                title="Handle every failure",
                source="""checkout
|> Recovery.with(fn(error) {
  case error {
    Payment.Declined(reason) -> alternate_payment(reason)
    Inventory.OutOfStock(item) -> backorder(item)
  }
})""",
                inferred_failures="recovery-arm failures only",
                observation="An exhaustive ordinary case needs no forwarding arm.",
            ),
            Example(
                title="Preserve an unrelated failure",
                source="""fn recover_decline<E>(error: E) -> Receipt {
  case error {
    Payment.Declined(reason) -> alternate_payment(reason)
    _ -> Failure(error)
  }
}""",
                inferred_failures="E - Payment.Declined + alternate_payment failures",
                observation="Every selective handler repeats the forwarding fallback.",
            ),
        ),
    ),
    Form(
        name="Contextual partial handler",
        summary="Let a recovery combinator reinterpret a partial function.",
        new_language_rule="unmatched function patterns forward the captured failure",
        examples=(
            Example(
                title="Handle one failure at the call site",
                source="""checkout
|> Recovery.with(fn(Payment.Declined(reason)) {
  alternate_payment(reason)
})""",
                inferred_failures="Inventory.OutOfStock + alternate_payment failures",
                observation="Concise, but the same function is partial outside Recovery.with.",
            ),
            Example(
                title="Compose a reusable handler",
                source="""fn recover_decline(Payment.Declined(reason)) -> Receipt {
  alternate_payment(reason)
}

let checkout_with_recovery = checkout |> Recovery.with(recover_decline)""",
                inferred_failures="original failures except Payment.Declined, plus arm failures",
                observation="The call site hides how unmatched inputs behave.",
            ),
            Example(
                title="Handle every failure",
                source="""checkout
|> Recovery.with(fn(error) {
  case error {
    Payment.Declined(reason) -> alternate_payment(reason)
    Inventory.OutOfStock(item) -> backorder(item)
  }
})""",
                inferred_failures="recovery-arm failures only",
                observation="The special rule adds nothing once matching is exhaustive.",
            ),
            Example(
                title="Preserve an unrelated failure",
                source="""fn recover_decline(Payment.Declined(reason)) -> Receipt {
  alternate_payment(reason)
}""",
                inferred_failures="original failures except Payment.Declined, plus arm failures",
                observation="Bubbling depends on the consumer's context, not the function alone.",
            ),
        ),
    ),
)


def render(form_index: int, example_index: int, decisions: tuple[str, ...]) -> str:
    form = FORMS[form_index]
    example = form.examples[example_index]
    decision = decisions[form_index] or "undecided"
    return "\n".join(
        (
            f"Form {form_index + 1}/{len(FORMS)}: {form.name}",
            f"Decision: {decision}",
            f"Summary: {form.summary}",
            f"New language rule: {form.new_language_rule}",
            "",
            f"Scenario {example_index + 1}/{len(form.examples)}: {example.title}",
            "",
            example.source,
            "",
            f"Inferred failures: {example.inferred_failures}",
            f"Observation: {example.observation}",
        )
    )
