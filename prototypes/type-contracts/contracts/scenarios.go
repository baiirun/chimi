package contracts

func Scenarios() ([]Scenario, Catalog) {
	text := Named("Text")
	integer := Named("Int")
	money := Named("Money")
	receipt := Named("Receipt")
	reservation := Named("Reservation")
	order := Named("Order")
	confirmation := Named("Confirmation")
	done := Named("Done")
	job := Named("Job")
	boolType := Named("Bool")
	voidType := Named("Void")
	duration := Named("Duration")

	negative := Failure{Enum: "MoneyError", Variant: "NegativeAmount"}
	declined := Failure{Enum: "PaymentError", Variant: "Declined"}
	gateway := Failure{Enum: "PaymentError", Variant: "GatewayUnavailable"}
	unavailable := Failure{Enum: "InventoryError", Variant: "Unavailable"}
	temporary := Failure{Enum: "JobError", Variant: "TemporaryFailure"}
	permanent := Failure{Enum: "JobError", Variant: "PermanentFailure"}

	payment := Interface("Payment", map[string]Type{
		"charge": Function([]Type{money}, Contract{Success: receipt, Failures: []Failure{declined, gateway}}),
	})
	inventory := Interface("Inventory", map[string]Type{
		"reserve": Function([]Type{Named("List<Item>")}, Contract{Success: reservation, Failures: []Failure{unavailable}}),
	})
	clock := Interface("Clock", map[string]Type{
		"sleep": Function([]Type{duration}, Contract{Success: voidType}),
	})
	mailer := Interface("Mailer", map[string]Type{
		"send": Function([]Type{job}, Contract{Success: done, Failures: []Failure{temporary, permanent}}),
	})

	paymentRequirement := Requirement{Name: "payment", Interface: payment}
	inventoryRequirement := Requirement{Name: "inventory", Interface: inventory}
	clockRequirement := Requirement{Name: "clock", Interface: clock}
	mailerRequirement := Requirement{Name: "mailer", Interface: mailer}

	fromCents := Function([]Type{integer}, Contract{Success: money, Failures: []Failure{negative}})
	reserve := Function([]Type{Named("List<Item>")}, Contract{Success: reservation, Failures: []Failure{unavailable}, Requirements: []Requirement{inventoryRequirement}})
	charge := Function([]Type{money}, Contract{Success: receipt, Failures: []Failure{declined, gateway}, Requirements: []Requirement{paymentRequirement}})
	toText := Function([]Type{integer}, Contract{Success: text})

	catalog := Catalog{
		"MoneyError":     {"NegativeAmount"},
		"PaymentError":   {"Declined", "GatewayUnavailable"},
		"InventoryError": {"Unavailable"},
		"JobError":       {"TemporaryFailure", "PermanentFailure"},
	}
	accountState := EnumShape{Name: "AccountState", Variants: []EnumVariant{
		{Name: "Active"},
		{Name: "Suspended", Fields: []Type{text}},
	}}
	moneyStruct := Type{Kind: "struct", Name: "Money", Fields: map[string]Type{"cents": integer}}

	scenarios := []Scenario{
		{
			Name:     "pure inference",
			Question: "Does an ordinary call infer a normal function without failure or requirement noise?",
			Source:   "fn normalize(input: Text) {\n  Text.lowercase(input)\n}",
			Expected: "fn(Text) -> Text",
			Infer: func() Inference {
				lowercase := Function([]Type{text}, Contract{Success: text})
				return Call(lowercase, Pure(text))
			},
		},
		{
			Name:     "automatic failure bubbling",
			Question: "Does an unhandled expected failure join the caller contract without propagation syntax?",
			Source:   "fn display(cents: Int) {\n  let money = Money.from_cents(cents)\n  Int.to_text(Money.cents(money))\n}",
			Expected: "fn(Int) -> Text errors MoneyError",
			Infer: func() Inference {
				created := Call(fromCents, Pure(integer))
				converted := Call(toText, Inference{Contract: Contract{Success: integer, Failures: created.Contract.Failures}})
				return Sequence(created, converted)
			},
		},
		{
			Name:     "sequential channel union",
			Question: "Do unrelated failures and dependencies compose without a wrapper type?",
			Source:   "fn submit(cart: Cart) needs { inventory: Inventory, payment: Payment } {\n  let reservation = inventory.reserve(cart.items)\n  let receipt = payment.charge(cart.total)\n  Order(reservation: reservation, receipt: receipt)\n}",
			Expected: "fn(Cart) -> Order errors { InventoryError, PaymentError } needs { inventory: Inventory, payment: Payment }",
			Infer: func() Inference {
				reserved := Call(reserve, Pure(Named("List<Item>")))
				charged := Call(charge, Pure(money))
				return Sequence(reserved, charged, Pure(order))
			},
		},
		{
			Name:     "selective recovery",
			Question: "When one failure variant is handled, does every unmatched variant bubble automatically?",
			Source:   "case payment.charge(total) {\n  Failure(Declined(reason)) -> fallback_receipt(reason)\n  Success(receipt) -> receipt\n}",
			Expected: "Receipt errors PaymentError.GatewayUnavailable needs { payment: Payment }",
			Infer: func() Inference {
				return Handle(Call(charge, Pure(money)), []Failure{declined}, Pure(receipt))
			},
		},
		{
			Name:     "higher-order preservation",
			Question: "Can a function transform success while preserving a caller-defined operation's channels?",
			Source:   "let configured = map_success(submit, confirm)",
			Expected: "Confirmation errors { InventoryError, PaymentError } needs { inventory: Inventory, payment: Payment }",
			Infer: func() Inference {
				operation := Inference{Contract: Contract{Success: order, Failures: []Failure{unavailable, declined, gateway}, Requirements: []Requirement{inventoryRequirement, paymentRequirement}}}
				confirm := Function([]Type{order}, Contract{Success: confirmation})
				return MapSuccess(operation, confirm)
			},
		},
		{
			Name:     "pipeable retry",
			Question: "Does Retry.with behave as a unary operation transform and preserve the final failure channel?",
			Source:   "let process_job = handle\n  |> Retry.with(policy: policy, delay: delay)",
			Expected: "Done errors JobError needs { clock: Clock, mailer: Mailer }",
			Infer: func() Inference {
				operation := Inference{Contract: Contract{Success: done, Failures: []Failure{temporary, permanent}, Requirements: []Requirement{mailerRequirement}}}
				return Retry(operation, []Failure{temporary}, []Requirement{clockRequirement})
			},
		},
		{
			Name:     "partial using boundary",
			Question: "Can a caller discharge only some needs before reaching the runtime root?",
			Source:   "let checkout = submit(cart) using {\n  payment: fake_payment,\n}",
			Expected: "Order errors { InventoryError, PaymentError } needs { inventory: Inventory }",
			Infer: func() Inference {
				operation := Inference{Contract: Contract{Success: order, Failures: []Failure{unavailable, declined, gateway}, Requirements: []Requirement{paymentRequirement, inventoryRequirement}}}
				provider := Record(map[string]Type{"charge": Function([]Type{money}, Contract{Success: receipt})})
				return Provide(operation, map[string]Type{"payment": provider})
			},
		},
		{
			Name:     "closed runtime root",
			Question: "Does supplying structurally compatible dependencies remove requirements before execution?",
			Source:   "pub fn main() {\n  program using { payment: fake_payment, inventory: fake_inventory }\n}",
			Expected: "Order errors { InventoryError, PaymentError }",
			Infer: func() Inference {
				operation := Inference{Contract: Contract{Success: order, Failures: []Failure{unavailable, declined, gateway}, Requirements: []Requirement{paymentRequirement, inventoryRequirement}}}
				paymentProvider := Record(map[string]Type{"charge": Function([]Type{money}, Contract{Success: receipt})})
				inventoryProvider := Record(map[string]Type{"reserve": Function([]Type{Named("List<Item>")}, Contract{Success: reservation})})
				partiallyConfigured := Provide(operation, map[string]Type{"payment": paymentProvider})
				return Run(Provide(partiallyConfigured, map[string]Type{"inventory": inventoryProvider}))
			},
		},
		{
			Name:        "missing runtime dependency",
			Question:    "Does execution reject a computation whose requirement row is still open?",
			Source:      "pub fn main() {\n  program using { payment: fake_payment }\n}",
			Expected:    "Order needs { inventory: Inventory }",
			ExpectedIDs: []string{"E_MISSING_RUNTIME_DEPENDENCY"},
			Infer: func() Inference {
				operation := Inference{Contract: Contract{Success: order, Requirements: []Requirement{paymentRequirement, inventoryRequirement}}}
				providers := map[string]Type{"payment": Record(map[string]Type{"charge": Function([]Type{money}, Contract{Success: receipt})})}
				return Run(Provide(operation, providers))
			},
		},
		{
			Name:        "provider shape mismatch",
			Question:    "Does a provider satisfy the consumer-defined interface structurally?",
			Source:      "let invalid_payment = { charge: fn(reference: Text) { Receipt(id: reference) } }\npub fn main() { program using { payment: invalid_payment } }",
			Expected:    "Receipt errors PaymentError needs { payment: Payment }",
			ExpectedIDs: []string{"E_REQUIREMENT_SHAPE_MISMATCH"},
			Infer: func() Inference {
				operation := Inference{Contract: Contract{Success: receipt, Failures: []Failure{declined, gateway}, Requirements: []Requirement{paymentRequirement}}}
				invalid := Record(map[string]Type{"charge": Function([]Type{text}, Contract{Success: receipt})})
				return Run(Provide(operation, map[string]Type{"payment": invalid}))
			},
		},
		{
			Name:        "ordinary case exhaustiveness",
			Question:    "Must an ordinary closed enum case cover every possible value?",
			Source:      "case state {\n  Active -> \"active\"\n}",
			Expected:    "Text",
			ExpectedIDs: []string{"E_CASE_NOT_EXHAUSTIVE"},
			Infer: func() Inference {
				return CheckCase(accountState, []PatternArm{{Variant: "Active"}}, text)
			},
		},
		{
			Name:     "ordinary case fallback",
			Question: "Does an unguarded catch-all make an ordinary enum case exhaustive?",
			Source:   "case state {\n  Active -> \"active\"\n  _ -> \"other\"\n}",
			Expected: "Text",
			Infer: func() Inference {
				return CheckCase(accountState, []PatternArm{{Variant: "Active"}, {CatchAll: true}}, text)
			},
		},
		{
			Name:        "guarded fallback coverage",
			Question:    "Does a guarded catch-all leave values for which its guard is false uncovered?",
			Source:      "case state {\n  Active -> \"active\"\n  other if should_inspect(other) -> inspect(other)\n}",
			Expected:    "Text",
			ExpectedIDs: []string{"E_CASE_NOT_EXHAUSTIVE"},
			Infer: func() Inference {
				return CheckCase(accountState, []PatternArm{{Variant: "Active"}, {CatchAll: true, Guarded: true}}, text)
			},
		},
		{
			Name:        "unreachable fallback arm",
			Question:    "Does an unguarded catch-all make every later arm unreachable?",
			Source:      "case state {\n  _ -> \"other\"\n  Active -> \"active\"\n}",
			Expected:    "Text",
			ExpectedIDs: []string{"E_UNREACHABLE_CASE_ARM"},
			Infer: func() Inference {
				return CheckCase(accountState, []PatternArm{{CatchAll: true}, {Variant: "Active"}}, text)
			},
		},
		{
			Name:        "variant pattern arity",
			Question:    "Does destructuring a variant enforce its declared payload shape?",
			Source:      "case state {\n  Suspended(reason, extra) -> reason\n  _ -> \"active\"\n}",
			Expected:    "Text",
			ExpectedIDs: []string{"E_PATTERN_ARITY"},
			Infer: func() Inference {
				return CheckCase(accountState, []PatternArm{{Variant: "Suspended", BindingCount: 2}, {CatchAll: true}}, text)
			},
		},
		{
			Name:        "struct field typing",
			Question:    "Does nominal struct construction validate field names and values?",
			Source:      "let invalid = Money(cents: \"100\")",
			Expected:    "Money",
			ExpectedIDs: []string{"E_FIELD_TYPE_MISMATCH"},
			Infer: func() Inference {
				return Construct(moneyStruct, map[string]Inference{"cents": Pure(text)})
			},
		},
		{
			Name:        "pipeline type mismatch",
			Question:    "Is every unary function pipeable only when its input type matches?",
			Source:      "let invalid = message |> Money.add_tax(rate)",
			Expected:    "Money",
			ExpectedIDs: []string{"E_TYPE_MISMATCH"},
			Infer: func() Inference {
				addTax := Function([]Type{money}, Contract{Success: money})
				return Call(addTax, Pure(text))
			},
		},
	}

	_ = boolType
	return scenarios, catalog
}
