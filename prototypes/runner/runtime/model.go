// Package runtime is a pure, disposable interpreter for Chimi's first executable semantics.
package runtime

import (
	"fmt"
	"sort"
	"strconv"
	"strings"
)

type SourceSpan struct {
	File  string `json:"file"`
	Start int    `json:"start"`
	End   int    `json:"end"`
}

type Field struct {
	Name  string
	Value Value
}

type Value struct {
	Kind     string
	Type     string
	Name     string
	Text     string
	Integer  int
	Boolean  bool
	Fields   []Field
	Elements []Value
}

func Text(value string) Value {
	return Value{Kind: "Text", Type: "Text", Text: value}
}

func Int(value int) Value {
	return Value{Kind: "Int", Type: "Int", Integer: value}
}

func Bool(value bool) Value {
	return Value{Kind: "Bool", Type: "Bool", Boolean: value}
}

func Void() Value {
	return Value{Kind: "Void", Type: "Void"}
}

func Struct(typeName string, fields ...Field) Value {
	return Value{Kind: "Struct", Type: typeName, Fields: fields}
}

func Enum(typeName, variant string, fields ...Field) Value {
	return Value{Kind: "Enum", Type: typeName, Name: variant, Fields: fields}
}

func (value Value) FailureID() string {
	if value.Kind != "Enum" {
		return value.Type
	}
	return value.Type + "." + value.Name
}

func FormatValue(value Value) string {
	switch value.Kind {
	case "Text":
		return strconv.Quote(value.Text)
	case "Int":
		return strconv.Itoa(value.Integer)
	case "Bool":
		if value.Boolean {
			return "True"
		}
		return "False"
	case "Void":
		return "Void"
	case "Struct", "Enum", "Record":
		parts := make([]string, len(value.Fields))
		for index, field := range value.Fields {
			parts[index] = field.Name + ": " + FormatValue(field.Value)
		}
		name := value.Type
		if value.Kind == "Enum" {
			name = value.Name
		}
		return name + "(" + strings.Join(parts, ", ") + ")"
	case "List":
		parts := make([]string, len(value.Elements))
		for index, element := range value.Elements {
			parts[index] = FormatValue(element)
		}
		return "[" + strings.Join(parts, ", ") + "]"
	default:
		return value.Type
	}
}

type Outcome struct {
	Kind    string
	Value   Value
	Message string
}

func Success(value Value) Outcome {
	return Outcome{Kind: "Success", Value: value}
}

func ExpectedFailure(value Value) Outcome {
	return Outcome{Kind: "ExpectedFailure", Value: value}
}

func Defect(message string) Outcome {
	return Outcome{Kind: "Defect", Message: message}
}

func Cancelled(reason string) Outcome {
	return Outcome{Kind: "Cancelled", Message: reason}
}

func FormatOutcome(outcome Outcome) string {
	switch outcome.Kind {
	case "Success":
		return "Success(" + FormatValue(outcome.Value) + ")"
	case "ExpectedFailure":
		return "ExpectedFailure(" + FormatValue(outcome.Value) + ")"
	default:
		return outcome.Kind + "(" + strconv.Quote(outcome.Message) + ")"
	}
}

type Handler struct {
	FailureID string
	Body      *Expr
}

type RetryPolicy struct {
	MaximumAttempts int
	RetryFailures   []string
	Delay           string
}

type Expr struct {
	Kind      string
	ID        string
	Span      SourceSpan
	Value     Value
	Steps     []*Expr
	Need      string
	Operation string
	Input     *Expr
	Handlers  []Handler
	Retry     RetryPolicy
	Message   string
}

func Pure(id string, value Value) *Expr {
	return &Expr{Kind: "Pure", ID: id, Value: value}
}

func Sequence(id string, steps ...*Expr) *Expr {
	return &Expr{Kind: "Sequence", ID: id, Steps: steps}
}

func Dependency(id, need, operation string) *Expr {
	return &Expr{Kind: "Dependency", ID: id, Need: need, Operation: operation}
}

func Fail(id string, failure Value) *Expr {
	return &Expr{Kind: "Fail", ID: id, Value: failure}
}

func Handle(id string, input *Expr, handlers ...Handler) *Expr {
	return &Expr{Kind: "Handle", ID: id, Input: input, Handlers: handlers}
}

func Retrying(id string, input *Expr, policy RetryPolicy) *Expr {
	return &Expr{Kind: "Retry", ID: id, Input: input, Retry: policy}
}

func DefectExpr(id, message string) *Expr {
	return &Expr{Kind: "Defect", ID: id, Message: message}
}

func At(expression *Expr, file string, start, end int) *Expr {
	expression.Span = SourceSpan{File: file, Start: start, End: end}
	return expression
}

type Program struct {
	Name  string
	Root  *Expr
	Needs []string
}

type Provider struct {
	Origin     string
	Operations map[string][]Outcome
}

type TraceConfig struct {
	Mode string
}

type Config struct {
	TraceID      string
	Providers    map[string]Provider
	Trace        TraceConfig
	CancelBefore map[string]string
}

type Attribute struct {
	Key   string `json:"key"`
	Value string `json:"value"`
}

type Event struct {
	Sequence     int         `json:"sequence"`
	TraceID      string      `json:"trace_id"`
	Computation  string      `json:"computation"`
	SpanID       string      `json:"span_id"`
	ParentSpanID string      `json:"parent_span_id,omitempty"`
	Type         string      `json:"type"`
	NodeID       string      `json:"node_id,omitempty"`
	Source       SourceSpan  `json:"source"`
	Attributes   []Attribute `json:"attributes,omitempty"`
}

type Diagnostic struct {
	Code    string
	Message string
}

type Report struct {
	Outcome     Outcome
	Events      []Event
	Diagnostics []Diagnostic
}

func EventTypes(report Report) []string {
	result := make([]string, len(report.Events))
	for index, event := range report.Events {
		result[index] = event.Type
	}
	return result
}

func EventNodePairs(report Report) []string {
	result := make([]string, len(report.Events))
	for index, event := range report.Events {
		result[index] = event.Type + "@" + event.NodeID
	}
	return result
}

type runner struct {
	config      Config
	computation string
	events      []Event
	positions   map[string]int
	sequence    int
	span        int
}

func Run(program Program, config Config) Report {
	diagnostics := []Diagnostic{}
	for _, need := range sortedStrings(program.Needs) {
		if _, found := config.Providers[need]; !found {
			diagnostics = append(diagnostics, Diagnostic{Code: "E_RUNNER_MISSING_PROVIDER", Message: "runner configuration has no provider for " + need})
		}
	}
	if len(diagnostics) > 0 {
		return Report{Diagnostics: diagnostics}
	}
	state := runner{config: config, computation: program.Name, positions: map[string]int{}}
	rootSpan := state.nextSpan()
	state.emit(rootSpan, "", "computation.started", program.Root, nil)
	outcome := state.evaluate(program.Root, rootSpan)
	attributes := attrs("outcome", outcome.Kind)
	if outcome.Kind == "Success" || outcome.Kind == "ExpectedFailure" {
		attributes = append(attributes, Attribute{Key: "value", Value: FormatValue(outcome.Value)})
	} else {
		attributes = append(attributes, Attribute{Key: "message", Value: outcome.Message})
	}
	eventType := map[string]string{
		"Success":         "computation.completed",
		"ExpectedFailure": "computation.failed",
		"Defect":          "computation.defected",
		"Cancelled":       "computation.cancelled",
	}[outcome.Kind]
	state.emit(rootSpan, "", eventType, program.Root, attributes)
	return Report{Outcome: outcome, Events: state.events}
}

func (state *runner) evaluate(expression *Expr, parentSpan string) Outcome {
	if reason, cancelled := state.config.CancelBefore[expression.ID]; cancelled {
		span := state.nextSpan()
		state.emit(span, parentSpan, "cancellation.observed", expression, attrs("reason", reason))
		return Cancelled(reason)
	}
	switch expression.Kind {
	case "Pure":
		span := state.nextSpan()
		state.emit(span, parentSpan, "value.produced", expression, attrs("value", FormatValue(expression.Value)))
		return Success(expression.Value)
	case "Sequence":
		span := state.nextSpan()
		state.emit(span, parentSpan, "sequence.started", expression, nil)
		result := Success(Void())
		for _, step := range expression.Steps {
			result = state.evaluate(step, span)
			if result.Kind != "Success" {
				state.emit(span, parentSpan, "sequence.short_circuited", expression, attrs("outcome", result.Kind, "at", step.ID))
				return result
			}
		}
		state.emit(span, parentSpan, "sequence.completed", expression, attrs("value", FormatValue(result.Value)))
		return result
	case "Dependency":
		return state.evaluateDependency(expression, parentSpan)
	case "Fail":
		span := state.nextSpan()
		state.emit(span, parentSpan, "failure.raised", expression, attrs("failure", expression.Value.FailureID(), "value", FormatValue(expression.Value)))
		return ExpectedFailure(expression.Value)
	case "Handle":
		span := state.nextSpan()
		state.emit(span, parentSpan, "handler.started", expression, nil)
		outcome := state.evaluate(expression.Input, span)
		if outcome.Kind != "ExpectedFailure" {
			state.emit(span, parentSpan, "handler.passed_through", expression, attrs("outcome", outcome.Kind))
			return outcome
		}
		for _, handler := range expression.Handlers {
			if handler.FailureID == outcome.Value.FailureID() {
				state.emit(span, parentSpan, "failure.handled", expression, attrs("failure", handler.FailureID))
				return state.evaluate(handler.Body, span)
			}
		}
		state.emit(span, parentSpan, "failure.unhandled", expression, attrs("failure", outcome.Value.FailureID()))
		return outcome
	case "Retry":
		return state.evaluateRetry(expression, parentSpan)
	case "Defect":
		span := state.nextSpan()
		state.emit(span, parentSpan, "defect.raised", expression, attrs("message", expression.Message))
		return Defect(expression.Message)
	default:
		span := state.nextSpan()
		message := "unknown executable node " + expression.Kind
		state.emit(span, parentSpan, "defect.raised", expression, attrs("message", message))
		return Defect(message)
	}
}

func (state *runner) evaluateDependency(expression *Expr, parentSpan string) Outcome {
	span := state.nextSpan()
	provider := state.config.Providers[expression.Need]
	state.emit(span, parentSpan, "dependency.started", expression, attrs("need", expression.Need, "operation", expression.Operation, "provider_origin", provider.Origin))
	key := expression.Need + "." + expression.Operation
	position := state.positions[key]
	script := provider.Operations[expression.Operation]
	if position >= len(script) {
		outcome := Defect("provider script exhausted for " + key)
		state.emit(span, parentSpan, "dependency.defected", expression, attrs("message", outcome.Message))
		return outcome
	}
	state.positions[key] = position + 1
	outcome := script[position]
	eventType := map[string]string{
		"Success":         "dependency.completed",
		"ExpectedFailure": "dependency.failed",
		"Defect":          "dependency.defected",
		"Cancelled":       "dependency.cancelled",
	}[outcome.Kind]
	attributes := attrs("outcome", outcome.Kind)
	if outcome.Kind == "Success" || outcome.Kind == "ExpectedFailure" {
		attributes = append(attributes, Attribute{Key: "value", Value: FormatValue(outcome.Value)})
	} else {
		attributes = append(attributes, Attribute{Key: "message", Value: outcome.Message})
	}
	state.emit(span, parentSpan, eventType, expression, attributes)
	return outcome
}

func (state *runner) evaluateRetry(expression *Expr, parentSpan string) Outcome {
	span := state.nextSpan()
	state.emit(span, parentSpan, "retry.started", expression, attrs("maximum_attempts", strconv.Itoa(expression.Retry.MaximumAttempts)))
	for attempt := 1; attempt <= expression.Retry.MaximumAttempts; attempt++ {
		attemptSpan := state.nextSpan()
		state.emit(attemptSpan, span, "attempt.started", expression, attrs("attempt", strconv.Itoa(attempt)))
		outcome := state.evaluate(expression.Input, attemptSpan)
		switch outcome.Kind {
		case "Success":
			state.emit(attemptSpan, span, "attempt.succeeded", expression, attrs("attempt", strconv.Itoa(attempt)))
			state.emit(span, parentSpan, "retry.completed", expression, attrs("attempts", strconv.Itoa(attempt)))
			return outcome
		case "ExpectedFailure":
			state.emit(attemptSpan, span, "attempt.failed", expression, attrs("attempt", strconv.Itoa(attempt), "failure", outcome.Value.FailureID()))
			if !contains(expression.Retry.RetryFailures, outcome.Value.FailureID()) {
				state.emit(span, parentSpan, "retry.not_selected", expression, attrs("failure", outcome.Value.FailureID()))
				return outcome
			}
			if attempt == expression.Retry.MaximumAttempts {
				state.emit(span, parentSpan, "retry.exhausted", expression, attrs("attempts", strconv.Itoa(attempt), "failure", outcome.Value.FailureID()))
				return outcome
			}
			state.emit(span, parentSpan, "retry.scheduled", expression, attrs("after", expression.Retry.Delay, "next_attempt", strconv.Itoa(attempt+1)))
		case "Defect":
			state.emit(attemptSpan, span, "attempt.defected", expression, attrs("attempt", strconv.Itoa(attempt)))
			return outcome
		case "Cancelled":
			state.emit(attemptSpan, span, "attempt.cancelled", expression, attrs("attempt", strconv.Itoa(attempt)))
			return outcome
		}
	}
	return Defect("retry reached impossible state")
}

func (state *runner) nextSpan() string {
	state.span++
	return "s" + strconv.Itoa(state.span)
}

func (state *runner) emit(span, parent, eventType string, expression *Expr, attributes []Attribute) {
	if state.config.Trace.Mode != "all" {
		return
	}
	state.sequence++
	sort.Slice(attributes, func(i, j int) bool { return attributes[i].Key < attributes[j].Key })
	state.events = append(state.events, Event{
		Sequence: state.sequence, TraceID: state.config.TraceID, Computation: state.computation, SpanID: span,
		ParentSpanID: parent, Type: eventType, NodeID: expression.ID,
		Source: expression.Span, Attributes: attributes,
	})
}

func attrs(values ...string) []Attribute {
	result := []Attribute{}
	for index := 0; index+1 < len(values); index += 2 {
		result = append(result, Attribute{Key: values[index], Value: values[index+1]})
	}
	return result
}

func contains(values []string, target string) bool {
	for _, value := range values {
		if value == target {
			return true
		}
	}
	return false
}

func sortedStrings(values []string) []string {
	result := append([]string{}, values...)
	sort.Strings(result)
	return result
}

func FormatEvent(event Event) string {
	attributes := []string{}
	for _, attribute := range event.Attributes {
		attributes = append(attributes, attribute.Key+"="+strconv.Quote(attribute.Value))
	}
	line := fmt.Sprintf("%02d %-27s computation=%s span=%s", event.Sequence, event.Type, event.Computation, event.SpanID)
	if event.ParentSpanID != "" {
		line += " parent=" + event.ParentSpanID
	}
	if event.NodeID != "" {
		line += " node=" + event.NodeID
	}
	if event.Source.File != "" {
		line += fmt.Sprintf(" source=%s:%d..%d", event.Source.File, event.Source.Start, event.Source.End)
	}
	if len(attributes) > 0 {
		line += " " + strings.Join(attributes, " ")
	}
	return line
}
