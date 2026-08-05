// Package resolve contains the pure name and module resolution probe.
package resolve

import (
	"fmt"
	"sort"
	"strings"
	"unicode"

	"chimi-semantic-lowering-prototype/lower"
)

type Namespace string

const (
	ModuleNamespace Namespace = "module"
	TypeNamespace   Namespace = "type"
	ValueNamespace  Namespace = "value"
)

type ModuleInput struct {
	Path string
	File lower.File
}

type Symbol struct {
	ID        string
	Name      string
	Kind      string
	Namespace Namespace
	Module    string
	Public    bool
	Span      lower.Span
}

type Reference struct {
	Text      string
	Namespace Namespace
	Span      lower.Span
	SymbolID  string
	Status    string
}

type Diagnostic struct {
	Code    string
	Message string
	Span    lower.Span
}

type ModuleResult struct {
	Path        string
	Name        string
	Symbols     []Symbol
	References  []Reference
	Diagnostics []Diagnostic
}

type Result struct {
	Modules []ModuleResult
}

type module struct {
	input            ModuleInput
	name             string
	types            map[string][]Symbol
	values           map[string][]Symbol
	imports          map[string]*module
	ambiguousImports map[string]bool
	importedTypes    map[string][]Symbol
	importedValues   map[string][]Symbol
	references       []Reference
	diagnostics      []Diagnostic
}

type resolver struct {
	modules           map[string]*module
	builtinTypes      map[string]Symbol
	preludeValues     map[string]Symbol
	builtinModules    map[string]map[string]Symbol
	genericParameters map[string]Symbol
}

type scope struct {
	parent *scope
	values map[string]Symbol
}

func newScope(parent *scope) *scope {
	return &scope{parent: parent, values: map[string]Symbol{}}
}

func (scope *scope) lookup(name string) (Symbol, bool) {
	for current := scope; current != nil; current = current.parent {
		if symbol, ok := current.values[name]; ok {
			return symbol, true
		}
	}
	return Symbol{}, false
}

func Resolve(inputs []ModuleInput) Result {
	r := resolver{
		modules:           map[string]*module{},
		builtinTypes:      builtinTypes(),
		preludeValues:     preludeValues(),
		builtinModules:    builtinModules(),
		genericParameters: map[string]Symbol{},
	}
	for _, input := range inputs {
		r.modules[input.Path] = &module{
			input:            input,
			name:             moduleName(input.Path),
			types:            map[string][]Symbol{},
			values:           map[string][]Symbol{},
			imports:          map[string]*module{},
			ambiguousImports: map[string]bool{},
			importedTypes:    map[string][]Symbol{},
			importedValues:   map[string][]Symbol{},
		}
	}
	for _, current := range r.modules {
		r.indexModule(current)
	}
	for _, current := range r.modules {
		r.bindImports(current)
		r.resolveModule(current)
	}
	result := Result{}
	paths := make([]string, 0, len(r.modules))
	for path := range r.modules {
		paths = append(paths, path)
	}
	sort.Strings(paths)
	for _, path := range paths {
		current := r.modules[path]
		moduleResult := ModuleResult{
			Path:        path,
			Name:        current.name,
			References:  current.references,
			Diagnostics: current.diagnostics,
		}
		for _, symbols := range current.types {
			moduleResult.Symbols = append(moduleResult.Symbols, symbols...)
		}
		for _, symbols := range current.values {
			moduleResult.Symbols = append(moduleResult.Symbols, symbols...)
		}
		sort.Slice(moduleResult.Symbols, func(i, j int) bool { return moduleResult.Symbols[i].ID < moduleResult.Symbols[j].ID })
		sort.Slice(moduleResult.References, func(i, j int) bool {
			return moduleResult.References[i].Span.Start < moduleResult.References[j].Span.Start
		})
		sort.Slice(moduleResult.Diagnostics, func(i, j int) bool {
			return moduleResult.Diagnostics[i].Span.Start < moduleResult.Diagnostics[j].Span.Start
		})
		result.Modules = append(result.Modules, moduleResult)
	}
	return result
}

func moduleName(path string) string {
	parts := strings.Split(path, "/")
	base := parts[len(parts)-1]
	words := strings.Split(base, "_")
	for index, word := range words {
		if word == "" {
			continue
		}
		runes := []rune(word)
		runes[0] = unicode.ToUpper(runes[0])
		words[index] = string(runes)
	}
	return strings.Join(words, "")
}

func (r *resolver) indexModule(current *module) {
	for _, item := range current.input.File.Items {
		if item.Data != nil {
			declaration := item.Data
			r.addSymbol(current, Symbol{
				ID: current.input.Path + "::type::" + declaration.Name.Text, Name: declaration.Name.Text,
				Kind: declaration.Kind, Namespace: TypeNamespace, Module: current.input.Path,
				Public: declaration.Public, Span: declaration.Name.Span,
			})
			if declaration.Kind == "struct" {
				r.addSymbol(current, Symbol{
					ID: current.input.Path + "::constructor::" + declaration.Name.Text, Name: declaration.Name.Text,
					Kind: "struct constructor", Namespace: ValueNamespace, Module: current.input.Path,
					Public: false, Span: declaration.Name.Span,
				})
			}
			for _, variant := range declaration.Variants {
				r.addSymbol(current, Symbol{
					ID: current.input.Path + "::constructor::" + variant.Name.Text, Name: variant.Name.Text,
					Kind: "enum constructor", Namespace: ValueNamespace, Module: current.input.Path,
					Public: declaration.Public, Span: variant.Name.Span,
				})
			}
		}
		if item.Function != nil {
			function := item.Function
			r.addSymbol(current, Symbol{
				ID: current.input.Path + "::value::" + function.Name.Text, Name: function.Name.Text,
				Kind: "function", Namespace: ValueNamespace, Module: current.input.Path,
				Public: function.Public, Span: function.Name.Span,
			})
		}
		if item.Binding != nil {
			binding := item.Binding
			r.addSymbol(current, Symbol{
				ID: current.input.Path + "::value::" + binding.Name.Text, Name: binding.Name.Text,
				Kind: "module binding", Namespace: ValueNamespace, Module: current.input.Path,
				Public: false, Span: binding.Name.Span,
			})
		}
	}
}

func (r *resolver) addSymbol(current *module, symbol Symbol) {
	table := current.values
	if symbol.Namespace == TypeNamespace {
		table = current.types
	}
	if existing := table[symbol.Name]; len(existing) > 0 {
		current.diagnostics = append(current.diagnostics, Diagnostic{
			Code: "E_DUPLICATE_SYMBOL", Message: fmt.Sprintf("%s %q conflicts with %s", symbol.Namespace, symbol.Name, existing[0].Kind), Span: symbol.Span,
		})
	}
	table[symbol.Name] = append(table[symbol.Name], symbol)
}

func (r *resolver) bindImports(current *module) {
	for _, item := range current.input.File.Items {
		if item.Import == nil {
			continue
		}
		parts := []string{}
		for _, part := range item.Import.Path {
			parts = append(parts, part.Text)
		}
		path := strings.Join(parts, "/")
		target, exists := r.modules[path]
		if !exists {
			current.diagnostics = append(current.diagnostics, Diagnostic{
				Code: "E_UNRESOLVED_MODULE", Message: "cannot find module " + path, Span: item.Import.Span,
			})
			continue
		}
		if len(item.Import.Members) > 0 {
			r.bindImportMembers(current, target, item.Import.Members)
			continue
		}
		if item.Import.Alias == nil {
			current.diagnostics = append(current.diagnostics, Diagnostic{
				Code: "E_MISSING_MODULE_ALIAS", Message: "module imports require an explicit alias", Span: item.Import.Span,
			})
			continue
		}
		binding := item.Import.Alias.Text
		if _, exists := current.imports[binding]; exists || current.ambiguousImports[binding] {
			current.ambiguousImports[binding] = true
			delete(current.imports, binding)
			current.diagnostics = append(current.diagnostics, Diagnostic{
				Code: "E_IMPORT_BINDING_CONFLICT", Message: fmt.Sprintf("module alias %s is bound more than once", binding), Span: item.Import.Span,
			})
			continue
		}
		current.imports[binding] = target
	}
}

func (r *resolver) bindImportMembers(current, target *module, members []lower.LocatedName) {
	for _, member := range members {
		bound := false
		for _, symbol := range target.types[member.Text] {
			if !symbol.Public {
				continue
			}
			bound = true
			if len(current.types[member.Text]) > 0 || len(current.importedTypes[member.Text]) > 0 {
				current.diagnostics = append(current.diagnostics, Diagnostic{Code: "E_IMPORT_MEMBER_CONFLICT", Message: "type import conflicts with existing name " + member.Text, Span: member.Span})
			} else {
				current.importedTypes[member.Text] = append(current.importedTypes[member.Text], symbol)
			}
		}
		for _, symbol := range target.values[member.Text] {
			if symbol.Kind == "function" {
				current.diagnostics = append(current.diagnostics, Diagnostic{Code: "E_UNQUALIFIED_FUNCTION_IMPORT", Message: "functions remain module-qualified; call " + target.name + "." + member.Text, Span: member.Span})
				bound = true
				if len(current.values[member.Text]) == 0 && len(current.importedValues[member.Text]) == 0 {
					current.importedValues[member.Text] = append(current.importedValues[member.Text], symbol)
				}
				continue
			}
			if !strings.Contains(symbol.Kind, "constructor") || !symbol.Public {
				continue
			}
			bound = true
			if len(current.values[member.Text]) > 0 || len(current.importedValues[member.Text]) > 0 {
				current.diagnostics = append(current.diagnostics, Diagnostic{Code: "E_IMPORT_MEMBER_CONFLICT", Message: "constructor import conflicts with existing name " + member.Text, Span: member.Span})
			} else {
				current.importedValues[member.Text] = append(current.importedValues[member.Text], symbol)
			}
		}
		if !bound {
			current.diagnostics = append(current.diagnostics, Diagnostic{Code: "E_UNRESOLVED_IMPORT_MEMBER", Message: target.name + " has no public type or constructor named " + member.Text, Span: member.Span})
		}
	}
}

func (r *resolver) resolveModule(current *module) {
	moduleScope := newScope(nil)
	for _, item := range current.input.File.Items {
		if item.Data != nil {
			r.resolveData(current, item.Data)
		}
		if item.Function != nil {
			r.resolveFunction(current, item.Function, moduleScope)
		}
		if item.Binding != nil {
			r.resolveExpr(current, item.Binding.Value, moduleScope)
		}
		if item.Expression != nil {
			r.resolveExpr(current, *item.Expression, moduleScope)
		}
	}
}

func (r *resolver) resolveData(current *module, declaration *lower.DataDeclaration) {
	r.withGenericParameters(current, declaration.Name.Text, declaration.Parameters, func() {
		for _, field := range declaration.Fields {
			r.resolveType(current, field.Type)
		}
		for _, variant := range declaration.Variants {
			for _, field := range variant.Fields {
				r.resolveType(current, field.Type)
			}
		}
	})
}

func (r *resolver) resolveFunction(current *module, function *lower.Function, parent *scope) {
	r.withGenericParameters(current, function.Name.Text, function.GenericParameters, func() {
		functionScope := newScope(parent)
		for _, parameter := range function.Parameters {
			if parameter.Type != nil {
				r.resolveType(current, *parameter.Type)
			}
			r.defineLocal(current, functionScope, parameter.Name, "parameter")
		}
		if function.Return != nil {
			r.resolveType(current, *function.Return)
		}
		if function.Requirements != nil {
			r.resolveRequirements(current, function.Requirements, functionScope)
		}
		r.resolveExpr(current, function.Body, functionScope)
	})
}

func (r *resolver) withGenericParameters(current *module, owner string, names []lower.LocatedName, resolve func()) {
	previous := r.genericParameters
	r.genericParameters = map[string]Symbol{}
	for _, name := range names {
		if _, exists := r.genericParameters[name.Text]; exists {
			current.diagnostics = append(current.diagnostics, Diagnostic{Code: "E_DUPLICATE_GENERIC_PARAMETER", Message: "generic parameter " + name.Text + " is declared more than once", Span: name.Span})
			continue
		}
		r.genericParameters[name.Text] = Symbol{
			ID:   current.input.Path + "::generic::" + owner + "::" + name.Text,
			Name: name.Text, Kind: "generic parameter", Namespace: TypeNamespace,
			Module: current.input.Path, Span: name.Span,
		}
	}
	resolve()
	r.genericParameters = previous
}

func (r *resolver) resolveType(current *module, value lower.Type) {
	if value.Kind == "FunctionType" {
		for _, input := range value.Inputs {
			r.resolveType(current, input)
		}
		if value.Success != nil {
			r.resolveType(current, *value.Success)
		}
		if value.Errors != nil {
			r.resolveType(current, *value.Errors)
		}
		if value.Requirements != nil {
			r.resolveRequirements(current, value.Requirements, nil)
		}
		return
	}
	for _, argument := range value.Arguments {
		r.resolveType(current, argument)
	}
	if len(value.Path) == 0 {
		return
	}
	if len(value.Path) == 1 {
		name := value.Path[0]
		if symbol, ok := r.genericParameters[name.Text]; ok {
			r.reference(current, name.Text, TypeNamespace, name.Span, symbol, "resolved generic")
			return
		}
		if symbols := current.types[name.Text]; len(symbols) == 1 {
			r.reference(current, name.Text, TypeNamespace, name.Span, symbols[0], "resolved")
			return
		}
		if symbols := current.importedTypes[name.Text]; len(symbols) == 1 {
			r.reference(current, name.Text, TypeNamespace, name.Span, symbols[0], "resolved explicit import")
			return
		}
		if symbol, ok := r.builtinTypes[name.Text]; ok {
			r.reference(current, name.Text, TypeNamespace, name.Span, symbol, "resolved")
			return
		}
		current.diagnostics = append(current.diagnostics, Diagnostic{Code: "E_UNRESOLVED_TYPE", Message: "cannot resolve type " + name.Text, Span: name.Span})
		return
	}
	moduleName := value.Path[0]
	typeName := value.Path[len(value.Path)-1]
	target := r.importedModule(current, moduleName)
	if target == nil {
		return
	}
	r.moduleReference(current, moduleName, target)
	symbols := target.types[typeName.Text]
	r.resolveExternal(current, target, typeName.Text, TypeNamespace, typeName.Span, symbols)
}

func (r *resolver) resolveRequirements(current *module, requirements *lower.RequirementRow, targetScope *scope) {
	for _, term := range requirements.Terms {
		if term.Variable != nil {
			name := *term.Variable
			if symbol, ok := r.genericParameters[name.Text]; ok {
				r.reference(current, name.Text, TypeNamespace, name.Span, symbol, "resolved generic row")
			} else {
				current.diagnostics = append(current.diagnostics, Diagnostic{Code: "E_UNRESOLVED_TYPE", Message: "cannot resolve generic row " + name.Text, Span: name.Span})
			}
		}
		for _, field := range term.Fields {
			r.resolveType(current, field.Type)
			if targetScope != nil {
				r.defineLocal(current, targetScope, field.Name, "requirement")
			}
		}
	}
}

func (r *resolver) resolveExpr(current *module, expression lower.Expr, currentScope *scope) {
	switch expression.Kind {
	case "Name":
		if expression.Name != nil {
			r.resolveValueName(current, *expression.Name, currentScope)
		}
	case "Select":
		if expression.Receiver != nil && expression.Receiver.Kind == "Name" && expression.Receiver.Name != nil {
			base := *expression.Receiver.Name
			if current.ambiguousImports[base.Text] {
				current.references = append(current.references, Reference{Text: base.Text, Namespace: ModuleNamespace, Span: base.Span, Status: "ambiguous import binding"})
				return
			}
			if target := r.moduleBySourceName(current, base.Text); target != nil {
				r.moduleReference(current, base, target)
				if expression.Name != nil {
					r.resolveExternal(current, target, expression.Name.Text, ValueNamespace, expression.Name.Span, target.values[expression.Name.Text])
				}
				return
			}
			if exports, ok := r.builtinModules[base.Text]; ok {
				moduleSymbol := Symbol{ID: "builtin::module::" + base.Text, Name: base.Text, Kind: "builtin module", Namespace: ModuleNamespace, Public: true, Span: base.Span}
				r.reference(current, base.Text, ModuleNamespace, base.Span, moduleSymbol, "resolved")
				if expression.Name != nil {
					symbol, exists := exports[expression.Name.Text]
					if exists {
						r.reference(current, base.Text+"."+expression.Name.Text, ValueNamespace, expression.Name.Span, symbol, "resolved")
					} else {
						current.diagnostics = append(current.diagnostics, Diagnostic{Code: "E_UNRESOLVED_MEMBER", Message: base.Text + " has no member " + expression.Name.Text, Span: expression.Name.Span})
					}
				}
				return
			}
		}
		if expression.Receiver != nil {
			r.resolveExpr(current, *expression.Receiver, currentScope)
		}
		if expression.Name != nil {
			current.references = append(current.references, Reference{Text: "." + expression.Name.Text, Namespace: ValueNamespace, Span: expression.Name.Span, Status: "deferred to receiver type"})
		}
	case "Call":
		if expression.Callee != nil {
			r.resolveExpr(current, *expression.Callee, currentScope)
		}
		for _, argument := range expression.Arguments {
			r.resolveExpr(current, argument.Value, currentScope)
		}
	case "Function":
		functionScope := newScope(currentScope)
		for _, parameter := range expression.Parameters {
			if parameter.Type != nil {
				r.resolveType(current, *parameter.Type)
			}
			r.defineLocal(current, functionScope, parameter.Name, "parameter")
		}
		if expression.Return != nil {
			r.resolveType(current, *expression.Return)
		}
		if expression.Requires != nil {
			r.resolveRequirements(current, expression.Requires, functionScope)
		}
		if expression.Body != nil {
			r.resolveExpr(current, *expression.Body, functionScope)
		}
	case "Block":
		blockScope := newScope(currentScope)
		for _, step := range expression.Steps {
			if step.Binding != nil {
				r.resolveExpr(current, step.Binding.Value, blockScope)
				r.defineLocal(current, blockScope, step.Binding.Name, "let binding")
			}
			if step.Expression != nil {
				r.resolveExpr(current, *step.Expression, blockScope)
			}
		}
		if expression.Result != nil {
			r.resolveExpr(current, *expression.Result, blockScope)
		}
	case "Binary":
		if expression.Left != nil {
			r.resolveExpr(current, *expression.Left, currentScope)
		}
		if expression.Right != nil {
			r.resolveExpr(current, *expression.Right, currentScope)
		}
	case "Unary", "Invalid":
		if expression.Left != nil {
			r.resolveExpr(current, *expression.Left, currentScope)
		}
		if expression.Right != nil {
			r.resolveExpr(current, *expression.Right, currentScope)
		}
	case "List":
		for _, element := range expression.Elements {
			r.resolveExpr(current, element, currentScope)
		}
	case "Record", "Using":
		if expression.Receiver != nil {
			r.resolveExpr(current, *expression.Receiver, currentScope)
		}
		for _, field := range expression.Fields {
			r.resolveExpr(current, field.Value, currentScope)
		}
	case "Case":
		for _, subject := range expression.Subjects {
			r.resolveExpr(current, subject, currentScope)
		}
		for _, arm := range expression.Arms {
			armScope := newScope(currentScope)
			for _, pattern := range arm.Patterns {
				r.resolvePattern(current, pattern, armScope)
			}
			if arm.Guard != nil {
				r.resolveExpr(current, *arm.Guard, armScope)
			}
			r.resolveExpr(current, arm.Body, armScope)
		}
	}
}

func (r *resolver) resolvePattern(current *module, pattern lower.Pattern, targetScope *scope) {
	if pattern.Kind == "WildcardPattern" || pattern.Kind == "LiteralPattern" || pattern.Kind == "InvalidPattern" {
		return
	}
	if pattern.Name == nil {
		return
	}
	name := *pattern.Name
	if pattern.Kind == "AppliedPattern" || startsUpper(name.Text) {
		if symbol, ok := r.lookupConstructor(current, name.Text); ok {
			r.reference(current, name.Text, ValueNamespace, name.Span, symbol, "resolved constructor")
		} else {
			current.diagnostics = append(current.diagnostics, Diagnostic{Code: "E_UNRESOLVED_CONSTRUCTOR", Message: "cannot resolve constructor " + name.Text, Span: name.Span})
		}
		for _, argument := range pattern.Arguments {
			r.resolvePattern(current, argument, targetScope)
		}
		return
	}
	r.defineLocal(current, targetScope, name, "pattern binding")
}

func startsUpper(name string) bool {
	runes := []rune(name)
	return len(runes) > 0 && unicode.IsUpper(runes[0])
}

func (r *resolver) lookupConstructor(current *module, name string) (Symbol, bool) {
	if symbols := current.values[name]; len(symbols) == 1 && strings.Contains(symbols[0].Kind, "constructor") {
		return symbols[0], true
	}
	if symbols := current.importedValues[name]; len(symbols) == 1 && strings.Contains(symbols[0].Kind, "constructor") {
		return symbols[0], true
	}
	if symbol, ok := r.preludeValues[name]; ok && strings.Contains(symbol.Kind, "constructor") {
		return symbol, true
	}
	return Symbol{}, false
}

func (r *resolver) resolveValueName(current *module, name lower.LocatedName, currentScope *scope) {
	if currentScope != nil {
		if symbol, ok := currentScope.lookup(name.Text); ok {
			r.reference(current, name.Text, ValueNamespace, name.Span, symbol, "resolved local")
			return
		}
	}
	if symbols := current.values[name.Text]; len(symbols) == 1 {
		r.reference(current, name.Text, ValueNamespace, name.Span, symbols[0], "resolved")
		return
	} else if len(symbols) > 1 {
		current.diagnostics = append(current.diagnostics, Diagnostic{Code: "E_AMBIGUOUS_VALUE", Message: "multiple module values are named " + name.Text, Span: name.Span})
		return
	}
	if symbols := current.importedValues[name.Text]; len(symbols) == 1 {
		r.reference(current, name.Text, ValueNamespace, name.Span, symbols[0], "resolved explicit import")
		return
	}
	if symbol, ok := r.preludeValues[name.Text]; ok {
		r.reference(current, name.Text, ValueNamespace, name.Span, symbol, "resolved prelude")
		return
	}
	current.diagnostics = append(current.diagnostics, Diagnostic{Code: "E_UNRESOLVED_VALUE", Message: "cannot resolve value " + name.Text, Span: name.Span})
}

func (r *resolver) defineLocal(current *module, targetScope *scope, name lower.LocatedName, kind string) {
	if name.Text == "_" {
		return
	}
	if _, exists := targetScope.values[name.Text]; exists {
		current.diagnostics = append(current.diagnostics, Diagnostic{Code: "E_DUPLICATE_LOCAL", Message: "duplicate local name " + name.Text, Span: name.Span})
		return
	}
	targetScope.values[name.Text] = Symbol{
		ID:   current.input.Path + "::local@" + fmt.Sprint(name.Span.Start) + "::" + name.Text,
		Name: name.Text, Kind: kind, Namespace: ValueNamespace, Module: current.input.Path, Span: name.Span,
	}
}

func (r *resolver) moduleBySourceName(current *module, name string) *module {
	if current.ambiguousImports[name] {
		return nil
	}
	return current.imports[name]
}

func (r *resolver) importedModule(current *module, name lower.LocatedName) *module {
	if current.ambiguousImports[name.Text] {
		return nil
	}
	target := current.imports[name.Text]
	if target == nil {
		current.diagnostics = append(current.diagnostics, Diagnostic{Code: "E_UNRESOLVED_MODULE", Message: "no imported module named " + name.Text, Span: name.Span})
	}
	return target
}

func (r *resolver) moduleReference(current *module, name lower.LocatedName, target *module) {
	symbol := Symbol{ID: "module::" + target.input.Path, Name: name.Text, Kind: "module", Namespace: ModuleNamespace, Module: target.input.Path, Public: true, Span: name.Span}
	r.reference(current, name.Text, ModuleNamespace, name.Span, symbol, "resolved")
}

func (r *resolver) resolveExternal(current, target *module, name string, namespace Namespace, span lower.Span, symbols []Symbol) {
	if len(symbols) == 0 {
		current.diagnostics = append(current.diagnostics, Diagnostic{Code: "E_UNRESOLVED_MEMBER", Message: target.name + " has no " + string(namespace) + " named " + name, Span: span})
		return
	}
	if len(symbols) > 1 {
		current.diagnostics = append(current.diagnostics, Diagnostic{Code: "E_AMBIGUOUS_MEMBER", Message: target.name + " has multiple members named " + name, Span: span})
		return
	}
	symbol := symbols[0]
	if !symbol.Public {
		current.references = append(current.references, Reference{Text: target.name + "." + name, Namespace: namespace, Span: span, SymbolID: symbol.ID, Status: "inaccessible"})
		code := "E_PRIVATE_SYMBOL"
		if symbol.Kind == "struct constructor" {
			code = "E_PRIVATE_STRUCT_CONSTRUCTOR"
		}
		current.diagnostics = append(current.diagnostics, Diagnostic{Code: code, Message: target.name + "." + name + " is module-private", Span: span})
		return
	}
	r.reference(current, target.name+"."+name, namespace, span, symbol, "resolved external")
}

func (r *resolver) reference(current *module, text string, namespace Namespace, span lower.Span, symbol Symbol, status string) {
	current.references = append(current.references, Reference{Text: text, Namespace: namespace, Span: span, SymbolID: symbol.ID, Status: status})
}

func builtinTypes() map[string]Symbol {
	result := map[string]Symbol{}
	for _, name := range []string{"Bool", "Duration", "Int", "List", "Text", "Void"} {
		result[name] = Symbol{ID: "builtin::type::" + name, Name: name, Kind: "builtin type", Namespace: TypeNamespace, Public: true}
	}
	return result
}

func preludeValues() map[string]Symbol {
	result := map[string]Symbol{}
	for _, name := range []string{"False", "Failure", "Success", "True"} {
		result[name] = Symbol{ID: "builtin::constructor::" + name, Name: name, Kind: "builtin constructor", Namespace: ValueNamespace, Public: true}
	}
	return result
}

func builtinModules() map[string]map[string]Symbol {
	result := map[string]map[string]Symbol{}
	result["Int"] = map[string]Symbol{
		"to_text": {ID: "builtin::module::Int::to_text", Name: "to_text", Kind: "builtin function", Namespace: ValueNamespace, Public: true},
	}
	return result
}

func DiagnosticCodes(result Result) []string {
	codes := []string{}
	for _, module := range result.Modules {
		for _, diagnostic := range module.Diagnostics {
			codes = append(codes, diagnostic.Code)
		}
	}
	sort.Strings(codes)
	return codes
}

func Format(result Result) []string {
	lines := []string{}
	for _, module := range result.Modules {
		lines = append(lines, fmt.Sprintf("Module %s (%s)", module.Name, module.Path))
		lines = append(lines, "  symbols")
		for _, symbol := range module.Symbols {
			visibility := "private"
			if symbol.Public {
				visibility = "public"
			}
			lines = append(lines, fmt.Sprintf("    %-18s %-11s %-7s %s", symbol.Name, symbol.Namespace, visibility, symbol.ID))
		}
		lines = append(lines, "  references")
		for _, reference := range module.References {
			lines = append(lines, fmt.Sprintf("    @%-8s %-22s %-11s %-22s -> %s", spanText(reference.Span), reference.Text, reference.Namespace, reference.Status, reference.SymbolID))
		}
		if len(module.Diagnostics) > 0 {
			lines = append(lines, "  diagnostics")
			for _, diagnostic := range module.Diagnostics {
				lines = append(lines, fmt.Sprintf("    %s @%s %s", diagnostic.Code, spanText(diagnostic.Span), diagnostic.Message))
			}
		}
	}
	return lines
}

func spanText(span lower.Span) string {
	return fmt.Sprintf("%d..%d", span.Start, span.End)
}
