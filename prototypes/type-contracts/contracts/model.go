// Package contracts is a pure, disposable probe for Chimi's inferred callable contracts.
package contracts

import (
	"fmt"
	"sort"
	"strings"
)

type Type struct {
	Kind   string
	Name   string
	Inputs []Type
	Output *Contract
	Fields map[string]Type
}

type Failure struct {
	Enum    string
	Variant string
}

func (failure Failure) ID() string {
	return failure.Enum + "." + failure.Variant
}

type Requirement struct {
	Name      string
	Interface Type
}

type Contract struct {
	Success      Type
	Failures     []Failure
	Requirements []Requirement
}

type Diagnostic struct {
	Code    string
	Message string
}

type Inference struct {
	Contract       Contract
	Diagnostics    []Diagnostic
	Trace          []string
	ExplainedNeeds map[string]bool
}

type Catalog map[string][]string

type Scenario struct {
	Name        string
	Question    string
	Source      string
	Expected    string
	ExpectedIDs []string
	Infer       func() Inference
}

type EnumVariant struct {
	Name   string
	Fields []Type
}

type EnumShape struct {
	Name     string
	Variants []EnumVariant
}

type PatternArm struct {
	Variant      string
	BindingCount int
	CatchAll     bool
	Guarded      bool
}

func Named(name string) Type {
	return Type{Kind: "named", Name: name}
}

func Function(inputs []Type, output Contract) Type {
	copy := output
	return Type{Kind: "function", Inputs: inputs, Output: &copy}
}

func Record(fields map[string]Type) Type {
	return Type{Kind: "record", Fields: fields}
}

func Interface(name string, fields map[string]Type) Type {
	return Type{Kind: "interface", Name: name, Fields: fields}
}

func Pure(success Type) Inference {
	return Inference{Contract: Contract{Success: success}}
}

func Construct(target Type, fields map[string]Inference) Inference {
	result := Pure(target)
	for _, name := range sortedInferenceNames(fields) {
		field := fields[name]
		result.Diagnostics = append(result.Diagnostics, field.Diagnostics...)
		result.Contract.Failures = unionFailures(result.Contract.Failures, field.Contract.Failures)
		result.Contract.Requirements, result.Diagnostics = unionRequirements(result.Contract.Requirements, field.Contract.Requirements, result.Diagnostics)
		expected, found := target.Fields[name]
		if !found {
			result.Diagnostics = append(result.Diagnostics, Diagnostic{Code: "E_UNKNOWN_STRUCT_FIELD", Message: target.Name + " has no field " + name})
			continue
		}
		if !sameType(field.Contract.Success, expected) {
			result.Diagnostics = append(result.Diagnostics, Diagnostic{Code: "E_FIELD_TYPE_MISMATCH", Message: target.Name + "." + name + " has type " + FormatType(field.Contract.Success, nil) + "; expected " + FormatType(expected, nil)})
		}
	}
	for _, name := range sortedFieldNames(target.Fields) {
		if _, found := fields[name]; !found {
			result.Diagnostics = append(result.Diagnostics, Diagnostic{Code: "E_MISSING_STRUCT_FIELD", Message: target.Name + " construction is missing field " + name})
		}
	}
	result.Trace = append(result.Trace, "struct construction validates every field and preserves channels from field expressions")
	return result
}

func CheckCase(subject EnumShape, arms []PatternArm, success Type) Inference {
	result := Pure(success)
	variants := map[string]EnumVariant{}
	for _, variant := range subject.Variants {
		variants[variant.Name] = variant
	}
	covered := map[string]bool{}
	allCovered := false
	for index, arm := range arms {
		if allCovered {
			result.Diagnostics = append(result.Diagnostics, Diagnostic{Code: "E_UNREACHABLE_CASE_ARM", Message: fmt.Sprintf("case arm %d is unreachable after a catch-all", index+1)})
			continue
		}
		if arm.CatchAll {
			if !arm.Guarded {
				allCovered = true
				result.Trace = append(result.Trace, "unguarded catch-all closes the remaining variant set")
			} else {
				result.Trace = append(result.Trace, "guarded catch-all does not prove exhaustiveness")
			}
			continue
		}
		variant, found := variants[arm.Variant]
		if !found {
			result.Diagnostics = append(result.Diagnostics, Diagnostic{Code: "E_UNKNOWN_PATTERN_VARIANT", Message: subject.Name + " has no variant " + arm.Variant})
			continue
		}
		if arm.BindingCount != len(variant.Fields) {
			result.Diagnostics = append(result.Diagnostics, Diagnostic{Code: "E_PATTERN_ARITY", Message: fmt.Sprintf("%s.%s carries %d values; pattern binds %d", subject.Name, arm.Variant, len(variant.Fields), arm.BindingCount)})
		}
		if !arm.Guarded {
			if covered[arm.Variant] {
				result.Diagnostics = append(result.Diagnostics, Diagnostic{Code: "E_UNREACHABLE_CASE_ARM", Message: arm.Variant + " is already handled"})
			}
			covered[arm.Variant] = true
		}
	}
	if !allCovered {
		missing := []string{}
		for _, variant := range subject.Variants {
			if !covered[variant.Name] {
				missing = append(missing, variant.Name)
			}
		}
		if len(missing) > 0 {
			result.Diagnostics = append(result.Diagnostics, Diagnostic{Code: "E_CASE_NOT_EXHAUSTIVE", Message: "case does not handle " + strings.Join(missing, ", ")})
		}
	}
	return result
}

func Call(callable Type, arguments ...Inference) Inference {
	result := Inference{}
	for _, argument := range arguments {
		result.Diagnostics = append(result.Diagnostics, argument.Diagnostics...)
		result.Contract.Failures = unionFailures(result.Contract.Failures, argument.Contract.Failures)
		result.Contract.Requirements, result.Diagnostics = unionRequirements(result.Contract.Requirements, argument.Contract.Requirements, result.Diagnostics)
	}
	if callable.Kind != "function" || callable.Output == nil {
		result.Diagnostics = append(result.Diagnostics, Diagnostic{Code: "E_NOT_CALLABLE", Message: "value is not callable"})
		result.Contract.Success = Named("Invalid")
		return result
	}
	if len(arguments) != len(callable.Inputs) {
		result.Diagnostics = append(result.Diagnostics, Diagnostic{Code: "E_ARGUMENT_COUNT", Message: fmt.Sprintf("expected %d arguments; received %d", len(callable.Inputs), len(arguments))})
	}
	limit := len(arguments)
	if len(callable.Inputs) < limit {
		limit = len(callable.Inputs)
	}
	for index := 0; index < limit; index++ {
		if !sameType(arguments[index].Contract.Success, callable.Inputs[index]) {
			result.Diagnostics = append(result.Diagnostics, Diagnostic{
				Code:    "E_TYPE_MISMATCH",
				Message: fmt.Sprintf("argument %d has type %s; expected %s", index+1, FormatType(arguments[index].Contract.Success, nil), FormatType(callable.Inputs[index], nil)),
			})
		}
	}
	result.Contract.Success = callable.Output.Success
	result.Contract.Failures = unionFailures(result.Contract.Failures, callable.Output.Failures)
	result.Contract.Requirements, result.Diagnostics = unionRequirements(result.Contract.Requirements, callable.Output.Requirements, result.Diagnostics)
	result.Trace = append(result.Trace, fmt.Sprintf("call returns %s and bubbles its unhandled channels", FormatType(result.Contract.Success, nil)))
	return result
}

func Sequence(parts ...Inference) Inference {
	result := Pure(Named("Void"))
	for _, part := range parts {
		result.Contract.Success = part.Contract.Success
		result.Contract.Failures = unionFailures(result.Contract.Failures, part.Contract.Failures)
		result.Contract.Requirements, result.Diagnostics = unionRequirements(result.Contract.Requirements, part.Contract.Requirements, result.Diagnostics)
		result.Diagnostics = append(result.Diagnostics, part.Diagnostics...)
		result.Trace = append(result.Trace, part.Trace...)
	}
	result.Trace = append(result.Trace, "sequence unions failures and requirements; its final expression supplies success")
	return result
}

func Handle(subject Inference, handled []Failure, recovery Inference) Inference {
	result := Inference{Contract: subject.Contract, Diagnostics: append([]Diagnostic{}, subject.Diagnostics...), Trace: append([]string{}, subject.Trace...)}
	known := map[string]bool{}
	for _, failure := range subject.Contract.Failures {
		known[failure.ID()] = true
	}
	removed := map[string]bool{}
	for _, failure := range handled {
		if !known[failure.ID()] {
			result.Diagnostics = append(result.Diagnostics, Diagnostic{Code: "E_UNKNOWN_HANDLED_FAILURE", Message: "cannot handle absent failure " + failure.ID()})
			continue
		}
		removed[failure.ID()] = true
	}
	remaining := []Failure{}
	for _, failure := range subject.Contract.Failures {
		if !removed[failure.ID()] {
			remaining = append(remaining, failure)
		}
	}
	if !sameType(subject.Contract.Success, recovery.Contract.Success) {
		result.Diagnostics = append(result.Diagnostics, Diagnostic{Code: "E_CASE_RESULT_MISMATCH", Message: "success and recovery arms produce incompatible values"})
	}
	result.Contract.Failures = unionFailures(remaining, recovery.Contract.Failures)
	result.Contract.Requirements, result.Diagnostics = unionRequirements(subject.Contract.Requirements, recovery.Contract.Requirements, result.Diagnostics)
	result.Diagnostics = append(result.Diagnostics, recovery.Diagnostics...)
	result.Trace = append(result.Trace, fmt.Sprintf("outcome case handles %s", joinFailureIDs(handled)))
	if len(remaining) > 0 {
		result.Trace = append(result.Trace, fmt.Sprintf("unmatched failures bubble automatically: %s", joinFailureIDs(remaining)))
	}
	return result
}

func MapSuccess(operation Inference, transform Type) Inference {
	if transform.Kind != "function" || transform.Output == nil || len(transform.Inputs) != 1 {
		return Inference{Contract: operation.Contract, Diagnostics: []Diagnostic{{Code: "E_TRANSFORM_SHAPE", Message: "map transform must be unary"}}}
	}
	if !sameType(operation.Contract.Success, transform.Inputs[0]) {
		return Inference{Contract: operation.Contract, Diagnostics: []Diagnostic{{Code: "E_TYPE_MISMATCH", Message: "map transform input does not match operation success"}}}
	}
	result := operation
	result.Contract.Success = transform.Output.Success
	result.Contract.Failures = unionFailures(result.Contract.Failures, transform.Output.Failures)
	result.Contract.Requirements, result.Diagnostics = unionRequirements(result.Contract.Requirements, transform.Output.Requirements, result.Diagnostics)
	result.Trace = append(result.Trace, "map_success changes success and preserves the operation's unknown failure and requirement rows")
	return result
}

func Retry(operation Inference, selected []Failure, policyRequirements []Requirement) Inference {
	result := operation
	known := map[string]bool{}
	for _, failure := range operation.Contract.Failures {
		known[failure.ID()] = true
	}
	for _, failure := range selected {
		if !known[failure.ID()] {
			result.Diagnostics = append(result.Diagnostics, Diagnostic{Code: "E_RETRY_FAILURE_NOT_PRESENT", Message: "retry policy names absent failure " + failure.ID()})
		}
	}
	result.Contract.Requirements, result.Diagnostics = unionRequirements(result.Contract.Requirements, policyRequirements, result.Diagnostics)
	result.Trace = append(result.Trace, fmt.Sprintf("Retry.with may rerun %s; it preserves every final failure", joinFailureIDs(selected)))
	result.Trace = append(result.Trace, "retry policy requirements join the operation requirements")
	return result
}

func Provide(operation Inference, providers map[string]Type) Inference {
	result := operation
	result.ExplainedNeeds = cloneNames(operation.ExplainedNeeds)
	remaining := []Requirement{}
	for _, requirement := range operation.Contract.Requirements {
		provider, found := providers[requirement.Name]
		if !found {
			remaining = append(remaining, requirement)
			continue
		}
		if mismatch := conformanceMismatch(provider, requirement.Interface, requirement.Name); mismatch != "" {
			result.Diagnostics = append(result.Diagnostics, Diagnostic{Code: "E_REQUIREMENT_SHAPE_MISMATCH", Message: mismatch})
			result.ExplainedNeeds[requirement.Name] = true
			remaining = append(remaining, requirement)
			continue
		}
		result.Trace = append(result.Trace, fmt.Sprintf("provider discharges %s: %s", requirement.Name, requirement.Interface.Name))
	}
	result.Contract.Requirements = remaining
	return result
}

func Run(operation Inference) Inference {
	result := operation
	for _, requirement := range operation.Contract.Requirements {
		if operation.ExplainedNeeds[requirement.Name] {
			continue
		}
		result.Diagnostics = append(result.Diagnostics, Diagnostic{
			Code:    "E_MISSING_RUNTIME_DEPENDENCY",
			Message: fmt.Sprintf("program needs %s: %s", requirement.Name, requirement.Interface.Name),
		})
	}
	if len(operation.Contract.Requirements) == 0 {
		result.Trace = append(result.Trace, "runtime root is closed: no unresolved requirements")
	}
	return result
}

func cloneNames(source map[string]bool) map[string]bool {
	result := map[string]bool{}
	for name, value := range source {
		result[name] = value
	}
	return result
}

func FormatContract(contract Contract, catalog Catalog) string {
	result := FormatType(contract.Success, catalog)
	if len(contract.Failures) > 0 {
		result += " errors " + formatFailureRow(contract.Failures, catalog)
	}
	if len(contract.Requirements) > 0 {
		parts := []string{}
		for _, requirement := range sortedRequirements(contract.Requirements) {
			parts = append(parts, requirement.Name+": "+FormatType(requirement.Interface, catalog))
		}
		result += " needs { " + strings.Join(parts, ", ") + " }"
	}
	return result
}

func FormatFunction(inputs []Type, contract Contract, catalog Catalog) string {
	parts := make([]string, len(inputs))
	for index, input := range inputs {
		parts[index] = FormatType(input, catalog)
	}
	return "fn(" + strings.Join(parts, ", ") + ") -> " + FormatContract(contract, catalog)
}

func FormatType(value Type, catalog Catalog) string {
	switch value.Kind {
	case "function":
		if value.Output == nil {
			return "fn(?)"
		}
		return FormatFunction(value.Inputs, *value.Output, catalog)
	case "record":
		keys := sortedFieldNames(value.Fields)
		parts := make([]string, len(keys))
		for index, key := range keys {
			parts[index] = key + ": " + FormatType(value.Fields[key], catalog)
		}
		return "{ " + strings.Join(parts, ", ") + " }"
	default:
		return value.Name
	}
}

func DiagnosticIDs(value Inference) []string {
	ids := make([]string, len(value.Diagnostics))
	for index, diagnostic := range value.Diagnostics {
		ids[index] = diagnostic.Code
	}
	sort.Strings(ids)
	return ids
}

func sameType(left, right Type) bool {
	if left.Kind != right.Kind || left.Name != right.Name || len(left.Inputs) != len(right.Inputs) || len(left.Fields) != len(right.Fields) {
		return false
	}
	for index := range left.Inputs {
		if !sameType(left.Inputs[index], right.Inputs[index]) {
			return false
		}
	}
	if (left.Output == nil) != (right.Output == nil) {
		return false
	}
	if left.Output != nil && !sameContract(*left.Output, *right.Output) {
		return false
	}
	for name, field := range left.Fields {
		other, exists := right.Fields[name]
		if !exists || !sameType(field, other) {
			return false
		}
	}
	return true
}

func sameContract(left, right Contract) bool {
	return sameType(left.Success, right.Success) && joinFailureIDs(left.Failures) == joinFailureIDs(right.Failures) && formatRequirements(left.Requirements) == formatRequirements(right.Requirements)
}

func unionFailures(left, right []Failure) []Failure {
	byID := map[string]Failure{}
	for _, failure := range append(append([]Failure{}, left...), right...) {
		byID[failure.ID()] = failure
	}
	ids := make([]string, 0, len(byID))
	for id := range byID {
		ids = append(ids, id)
	}
	sort.Strings(ids)
	result := make([]Failure, len(ids))
	for index, id := range ids {
		result[index] = byID[id]
	}
	return result
}

func unionRequirements(left, right []Requirement, diagnostics []Diagnostic) ([]Requirement, []Diagnostic) {
	byName := map[string]Requirement{}
	for _, requirement := range append(append([]Requirement{}, left...), right...) {
		if existing, found := byName[requirement.Name]; found && !sameType(existing.Interface, requirement.Interface) {
			diagnostics = append(diagnostics, Diagnostic{Code: "E_REQUIREMENT_NAME_CONFLICT", Message: requirement.Name + " refers to incompatible interfaces"})
			continue
		}
		byName[requirement.Name] = requirement
	}
	result := make([]Requirement, 0, len(byName))
	for _, requirement := range byName {
		result = append(result, requirement)
	}
	return sortedRequirements(result), diagnostics
}

func sortedRequirements(values []Requirement) []Requirement {
	result := append([]Requirement{}, values...)
	sort.Slice(result, func(i, j int) bool { return result[i].Name < result[j].Name })
	return result
}

func sortedFieldNames(fields map[string]Type) []string {
	result := make([]string, 0, len(fields))
	for name := range fields {
		result = append(result, name)
	}
	sort.Strings(result)
	return result
}

func sortedInferenceNames(fields map[string]Inference) []string {
	result := make([]string, 0, len(fields))
	for name := range fields {
		result = append(result, name)
	}
	sort.Strings(result)
	return result
}

func formatRequirements(values []Requirement) string {
	parts := []string{}
	for _, value := range sortedRequirements(values) {
		parts = append(parts, value.Name+":"+FormatType(value.Interface, nil))
	}
	return strings.Join(parts, ",")
}

func formatFailureRow(values []Failure, catalog Catalog) string {
	byEnum := map[string][]string{}
	for _, failure := range unionFailures(nil, values) {
		byEnum[failure.Enum] = append(byEnum[failure.Enum], failure.Variant)
	}
	entries := []string{}
	for enum, variants := range byEnum {
		sort.Strings(variants)
		all := append([]string{}, catalog[enum]...)
		sort.Strings(all)
		if len(all) > 0 && strings.Join(all, ",") == strings.Join(variants, ",") {
			entries = append(entries, enum)
			continue
		}
		for _, variant := range variants {
			entries = append(entries, enum+"."+variant)
		}
	}
	sort.Strings(entries)
	if len(entries) == 1 {
		return entries[0]
	}
	return "{ " + strings.Join(entries, ", ") + " }"
}

func joinFailureIDs(values []Failure) string {
	ids := []string{}
	for _, failure := range unionFailures(nil, values) {
		ids = append(ids, failure.ID())
	}
	return strings.Join(ids, ", ")
}

func conformanceMismatch(provider, target Type, requirementName string) string {
	if provider.Kind != "record" || target.Kind != "interface" {
		return requirementName + " must be a record satisfying interface " + target.Name
	}
	for _, fieldName := range sortedFieldNames(target.Fields) {
		expected := target.Fields[fieldName]
		actual, found := provider.Fields[fieldName]
		if !found {
			return requirementName + "." + fieldName + " is missing; expected " + FormatType(expected, nil)
		}
		if !providerTypeCompatible(actual, expected) {
			return requirementName + "." + fieldName + " has type " + FormatType(actual, nil) + "; expected " + FormatType(expected, nil)
		}
	}
	return ""
}

func providerTypeCompatible(actual, expected Type) bool {
	if actual.Kind != "function" || expected.Kind != "function" {
		return sameType(actual, expected)
	}
	if len(actual.Inputs) != len(expected.Inputs) || actual.Output == nil || expected.Output == nil {
		return false
	}
	for index := range actual.Inputs {
		if !sameType(actual.Inputs[index], expected.Inputs[index]) {
			return false
		}
	}
	if !sameType(actual.Output.Success, expected.Output.Success) {
		return false
	}
	if !failureSubset(actual.Output.Failures, expected.Output.Failures) {
		return false
	}
	return requirementSubset(actual.Output.Requirements, expected.Output.Requirements)
}

func failureSubset(actual, expected []Failure) bool {
	allowed := map[string]bool{}
	for _, failure := range expected {
		allowed[failure.ID()] = true
	}
	for _, failure := range actual {
		if !allowed[failure.ID()] {
			return false
		}
	}
	return true
}

func requirementSubset(actual, expected []Requirement) bool {
	allowed := map[string]Type{}
	for _, requirement := range expected {
		allowed[requirement.Name] = requirement.Interface
	}
	for _, requirement := range actual {
		interfaceType, found := allowed[requirement.Name]
		if !found || !sameType(requirement.Interface, interfaceType) {
			return false
		}
	}
	return true
}
