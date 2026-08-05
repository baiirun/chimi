package runtime

type Scenario struct {
	Name           string
	Question       string
	Source         string
	Program        Program
	Config         Config
	Expected       string
	ExpectedCodes  []string
	RequiredEvents []string
	ForbiddenPairs []string
}

func Scenarios() []Scenario {
	done := Struct("Done")
	receipt := Struct("Receipt", Field{Name: "id", Value: Text("receipt-1")})
	order := Struct("Order", Field{Name: "id", Value: Text("order-1")})
	temporary := Enum("JobError", "TemporaryFailure", Field{Name: "reason", Value: Text("timeout")})
	permanent := Enum("JobError", "PermanentFailure", Field{Name: "reason", Value: Text("invalid address")})
	declined := Enum("PaymentError", "Declined", Field{Name: "reason", Value: Text("card declined")})

	all := TraceConfig{Mode: "all"}
	config := func(traceID string, providers map[string]Provider) Config {
		return Config{TraceID: traceID, Providers: providers, Trace: all, CancelBefore: map[string]string{}}
	}
	provider := func(origin, operation string, outcomes ...Outcome) Provider {
		return Provider{Origin: origin, Operations: map[string][]Outcome{operation: outcomes}}
	}
	retrySource := "ReceiptJobs.handle(job) |> Retry.with(policy: policy, delay: delay)"
	retryCall := At(Dependency("send", "mailer", "send"), "worker.chimi", 0, len("ReceiptJobs.handle(job)"))
	retryExpr := At(Retrying("retry", retryCall,
		RetryPolicy{MaximumAttempts: 3, RetryFailures: []string{"JobError.TemporaryFailure"}, Delay: "1s"},
	), "worker.chimi", 0, len(retrySource))

	return []Scenario{
		{
			Name:     "left-to-right block",
			Question: "Do block steps evaluate left to right and return only the final value?",
			Source:   "let value = {\n  let first = 1\n  first + 1\n}",
			Program:  Program{Name: "block", Root: Sequence("block", Pure("first", Int(1)), Pure("last", Int(2)))},
			Config:   config("trace-block", nil),
			Expected: "Success(2)",
			RequiredEvents: []string{
				"computation.started", "sequence.started", "value.produced", "value.produced", "sequence.completed", "computation.completed",
			},
		},
		{
			Name:     "dependency success",
			Question: "Does a dependency call produce a child span and a successful value?",
			Source:   "payment.charge(total)",
			Program:  Program{Name: "charge", Root: Dependency("charge", "payment", "charge"), Needs: []string{"payment"}},
			Config: config("trace-charge", map[string]Provider{
				"payment": provider("using", "charge", Success(receipt)),
			}),
			Expected: "Success(Receipt(id: \"receipt-1\"))",
			RequiredEvents: []string{
				"computation.started", "dependency.started", "dependency.completed", "computation.completed",
			},
		},
		{
			Name:     "failure short circuit",
			Question: "Does an expected failure skip every remaining block expression?",
			Source:   "{\n  payment.charge(total)\n  send_receipt()\n  Order(id: \"order-1\")\n}",
			Program: Program{Name: "checkout", Needs: []string{"payment"}, Root: Sequence("checkout",
				Dependency("charge", "payment", "charge"),
				Pure("send_receipt", done),
				Pure("order", order),
			)},
			Config: config("trace-failure", map[string]Provider{
				"payment": provider("runtime", "charge", ExpectedFailure(declined)),
			}),
			Expected: "ExpectedFailure(Declined(reason: \"card declined\"))",
			RequiredEvents: []string{
				"dependency.failed", "sequence.short_circuited", "computation.failed",
			},
			ForbiddenPairs: []string{"value.produced@send_receipt", "value.produced@order"},
		},
		{
			Name:     "handled expected failure",
			Question: "Can a matching failure case recover to an ordinary success value?",
			Source:   "case payment.charge(total) {\n  Failure(Declined(_)) -> fallback_receipt\n  Success(receipt) -> receipt\n}",
			Program: Program{Name: "recover", Needs: []string{"payment"}, Root: Handle("recover",
				Dependency("charge", "payment", "charge"),
				Handler{FailureID: "PaymentError.Declined", Body: Pure("fallback", receipt)},
			)},
			Config: config("trace-recovery", map[string]Provider{
				"payment": provider("using", "charge", ExpectedFailure(declined)),
			}),
			Expected: "Success(Receipt(id: \"receipt-1\"))",
			RequiredEvents: []string{
				"dependency.failed", "failure.handled", "value.produced", "computation.completed",
			},
		},
		{
			Name:     "retry succeeds",
			Question: "Does retry rerun a selected expected failure and preserve one causal trace?",
			Source:   retrySource,
			Program:  Program{Name: "retry-job", Needs: []string{"mailer", "clock"}, Root: retryExpr},
			Config: config("trace-retry", map[string]Provider{
				"mailer": provider("using", "send", ExpectedFailure(temporary), Success(done)),
				"clock":  provider("runtime", "sleep", Success(Void())),
			}),
			Expected: "Success(Done())",
			RequiredEvents: []string{
				"attempt.started", "dependency.failed", "attempt.failed", "retry.scheduled", "attempt.started", "dependency.completed", "attempt.succeeded", "retry.completed", "computation.completed",
			},
		},
		{
			Name:     "retry skips permanent failure",
			Question: "Does retry immediately return an expected failure excluded by its policy?",
			Source:   "operation |> Retry.with(policy: retry_temporary)",
			Program: Program{Name: "permanent", Needs: []string{"mailer", "clock"}, Root: Retrying("retry",
				Dependency("send", "mailer", "send"),
				RetryPolicy{MaximumAttempts: 3, RetryFailures: []string{"JobError.TemporaryFailure"}, Delay: "1s"},
			)},
			Config: config("trace-permanent", map[string]Provider{
				"mailer": provider("using", "send", ExpectedFailure(permanent)),
				"clock":  provider("runtime", "sleep", Success(Void())),
			}),
			Expected: "ExpectedFailure(PermanentFailure(reason: \"invalid address\"))",
			RequiredEvents: []string{
				"attempt.failed", "retry.not_selected", "computation.failed",
			},
		},
		{
			Name:     "retry exhausted",
			Question: "Does exhausted retry return the final ordinary failure value?",
			Source:   "operation |> Retry.with(policy: two_attempts)",
			Program: Program{Name: "exhausted", Needs: []string{"mailer", "clock"}, Root: Retrying("retry",
				Dependency("send", "mailer", "send"),
				RetryPolicy{MaximumAttempts: 2, RetryFailures: []string{"JobError.TemporaryFailure"}, Delay: "1s"},
			)},
			Config: config("trace-exhausted", map[string]Provider{
				"mailer": provider("using", "send", ExpectedFailure(temporary), ExpectedFailure(temporary)),
				"clock":  provider("runtime", "sleep", Success(Void())),
			}),
			Expected: "ExpectedFailure(TemporaryFailure(reason: \"timeout\"))",
			RequiredEvents: []string{
				"retry.scheduled", "attempt.failed", "retry.exhausted", "computation.failed",
			},
		},
		{
			Name:     "defect is not expected failure",
			Question: "Does a broken invariant bypass expected-failure recovery and retry?",
			Source:   "operation_with_broken_invariant()",
			Program:  Program{Name: "defect", Root: DefectExpr("broken", "impossible empty scheduler queue")},
			Config:   config("trace-defect", nil),
			Expected: "Defect(\"impossible empty scheduler queue\")",
			RequiredEvents: []string{
				"defect.raised", "computation.defected",
			},
		},
		{
			Name:     "cooperative cancellation",
			Question: "Does cancellation stop at a semantic boundary and remain distinct from failure?",
			Source:   "mailer.send(receipt)",
			Program:  Program{Name: "cancel", Root: Dependency("send", "mailer", "send"), Needs: []string{"mailer"}},
			Config: Config{TraceID: "trace-cancel", Trace: all, Providers: map[string]Provider{
				"mailer": provider("using", "send", Success(done)),
			}, CancelBefore: map[string]string{"send": "parent scope cancelled"}},
			Expected: "Cancelled(\"parent scope cancelled\")",
			RequiredEvents: []string{
				"cancellation.observed", "computation.cancelled",
			},
			ForbiddenPairs: []string{"dependency.started@send"},
		},
		{
			Name:     "runner configuration rejected",
			Question: "Does the runner refuse to begin when a defensive provider check fails?",
			Source:   "// embedding host attempted to invoke an open root",
			Program:  Program{Name: "missing", Root: Dependency("charge", "payment", "charge"), Needs: []string{"payment"}},
			Config:   config("trace-missing", nil),
			Expected: "",
			ExpectedCodes: []string{
				"E_RUNNER_MISSING_PROVIDER",
			},
		},
	}
}
