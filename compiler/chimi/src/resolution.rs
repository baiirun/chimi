use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::{FileId, ModuleId, SemanticId, SymbolId};
use crate::semantic::{
    ImportBinding, ModuleBlockItem, ModuleCaseArm, ModuleDeclarationKind, ModuleExpression, Name,
    Pattern, RequirementRowUse, RequirementTermUse, SemanticModule, SemanticProgram,
    SemanticProject, StructDeclaration, TypeUse,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SymbolKind {
    Struct,
    Field,
    Function,
}

#[derive(Clone, Debug)]
pub(crate) struct Symbol {
    pub(crate) id: SymbolId,
    pub(crate) kind: SymbolKind,
    pub(crate) name: String,
    pub(crate) semantic_id: SemanticId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Resolution {
    Resolved(SymbolId),
    Unresolved,
}

#[derive(Debug)]
pub(crate) struct ResolvedProgram {
    pub(crate) semantic: Arc<SemanticProgram>,
    pub(crate) symbols: Vec<Symbol>,
    pub(crate) constructor_resolution: Option<Resolution>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

impl ResolvedProgram {
    pub(crate) fn symbol(&self, symbol_id: SymbolId) -> &Symbol {
        self.symbols
            .get(symbol_id.raw() as usize)
            .filter(|symbol| symbol.id == symbol_id)
            .expect("resolved symbol ID belongs to this snapshot")
    }

    pub(crate) fn structure(&self) -> Option<&StructDeclaration> {
        self.semantic.structure.as_ref()
    }
}

pub(crate) fn resolve(semantic: SemanticProgram) -> ResolvedProgram {
    let semantic = Arc::new(semantic);
    let mut symbols = Vec::new();
    if let Some(structure) = &semantic.structure {
        symbols.push(Symbol {
            id: SymbolId::new(symbols.len() as u32),
            kind: SymbolKind::Struct,
            name: structure.name.text.clone(),
            semantic_id: structure.semantic_id,
        });
        symbols.push(Symbol {
            id: SymbolId::new(symbols.len() as u32),
            kind: SymbolKind::Field,
            name: structure.field_name.text.clone(),
            semantic_id: structure.field_semantic_id,
        });
    }
    for function in &semantic.functions {
        symbols.push(Symbol {
            id: SymbolId::new(symbols.len() as u32),
            kind: SymbolKind::Function,
            name: function.name.text.clone(),
            semantic_id: function.semantic_id,
        });
    }

    let mut diagnostics = semantic.diagnostics.clone();
    if let Some(structure) = &semantic.structure
        && structure.field_type.text != "Int"
    {
        diagnostics.push(Diagnostic::error(
            DiagnosticStage::Name,
            "N3002",
            semantic.source.file_id(),
            structure.field_type.range,
            format!("unresolved type `{}`", structure.field_type.text),
        ));
    }

    let constructor_resolution = semantic.functions.first().map(|function| {
        let resolved = semantic
            .structure
            .as_ref()
            .is_some_and(|structure| structure.name.text == function.body.constructor.text);
        if resolved {
            Resolution::Resolved(SymbolId::new(0))
        } else {
            diagnostics.push(Diagnostic::error(
                DiagnosticStage::Name,
                "N3001",
                semantic.source.file_id(),
                function.body.constructor.range,
                format!(
                    "unresolved constructor `{}`",
                    function.body.constructor.text
                ),
            ));
            Resolution::Unresolved
        }
    });

    ResolvedProgram {
        semantic,
        symbols,
        constructor_resolution,
        diagnostics,
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum Namespace {
    Module,
    Type,
    Value,
}

impl Namespace {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Module => "module",
            Self::Type => "type",
            Self::Value => "value",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProjectSymbolKind {
    BuiltinModule,
    BuiltinType,
    BuiltinValue,
    Module,
    Struct,
    StructConstructor,
    Enum,
    EnumConstructor,
    Interface,
    Function,
    ModuleBinding,
    GenericParameter,
    Parameter,
    Requirement,
    LocalBinding,
    PatternBinding,
}

impl ProjectSymbolKind {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::BuiltinModule => "builtin module",
            Self::BuiltinType => "builtin type",
            Self::BuiltinValue => "builtin value",
            Self::Module => "module",
            Self::Struct => "struct",
            Self::StructConstructor => "struct constructor",
            Self::Enum => "enum",
            Self::EnumConstructor => "enum constructor",
            Self::Interface => "interface",
            Self::Function => "function",
            Self::ModuleBinding => "module binding",
            Self::GenericParameter => "generic parameter",
            Self::Parameter => "parameter",
            Self::Requirement => "requirement",
            Self::LocalBinding => "local binding",
            Self::PatternBinding => "pattern binding",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectSymbol {
    pub(crate) id: SymbolId,
    pub(crate) module_id: Option<ModuleId>,
    pub(crate) semantic_id: Option<SemanticId>,
    pub(crate) namespace: Namespace,
    pub(crate) kind: ProjectSymbolKind,
    pub(crate) name: String,
    pub(crate) public: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReferenceResolution {
    Resolved(SymbolId),
    Unresolved,
    Ambiguous,
    Inaccessible(SymbolId),
    DeferredToType,
}

impl ReferenceResolution {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Resolved(_) => "resolved",
            Self::Unresolved => "unresolved",
            Self::Ambiguous => "ambiguous",
            Self::Inaccessible(_) => "inaccessible",
            Self::DeferredToType => "deferred-to-type",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectReference {
    pub(crate) file_id: FileId,
    pub(crate) semantic_id: SemanticId,
    pub(crate) namespace: Namespace,
    pub(crate) text: String,
    pub(crate) range: crate::ids::TextRange,
    pub(crate) resolution: ReferenceResolution,
}

#[derive(Debug)]
pub(crate) struct ResolvedProject {
    pub(crate) semantic: Arc<SemanticProject>,
    pub(crate) dependency_order: Vec<ModuleId>,
    pub(crate) symbols: Vec<ProjectSymbol>,
    pub(crate) references: Vec<ProjectReference>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    symbols_by_origin: HashMap<(ModuleId, SemanticId), Vec<SymbolId>>,
    references_by_origin: HashMap<(FileId, SemanticId, Namespace), ReferenceResolution>,
}

impl ResolvedProject {
    pub(crate) fn symbol(&self, id: SymbolId) -> &ProjectSymbol {
        self.symbols
            .get(id.raw() as usize)
            .filter(|symbol| symbol.id == id)
            .expect("project symbol ID belongs to this immutable snapshot")
    }

    pub(crate) fn source_symbol(
        &self,
        module_id: ModuleId,
        semantic_id: SemanticId,
        kind: ProjectSymbolKind,
        name: &str,
    ) -> Option<SymbolId> {
        self.source_symbols(module_id, semantic_id)
            .iter()
            .copied()
            .find(|symbol| {
                let symbol = self.symbol(*symbol);
                symbol.kind == kind && symbol.name == name
            })
    }

    pub(crate) fn source_symbols(
        &self,
        module_id: ModuleId,
        semantic_id: SemanticId,
    ) -> &[SymbolId] {
        self.symbols_by_origin
            .get(&(module_id, semantic_id))
            .map_or(&[], Vec::as_slice)
    }

    pub(crate) fn reference_resolution(
        &self,
        module_id: ModuleId,
        semantic_id: SemanticId,
        namespace: Namespace,
    ) -> Option<ReferenceResolution> {
        let file_id = self.semantic.modules[module_id.raw() as usize]
            .source
            .file_id();
        self.references_by_origin
            .get(&(file_id, semantic_id, namespace))
            .copied()
    }
}

#[derive(Default)]
struct ModuleIndex {
    types: HashMap<String, Vec<SymbolId>>,
    values: HashMap<String, Vec<SymbolId>>,
    aliases: HashMap<String, AliasBinding>,
    imported_types: HashMap<String, Vec<SymbolId>>,
    imported_values: HashMap<String, Vec<SymbolId>>,
    ambiguous_types: HashSet<String>,
    ambiguous_values: HashSet<String>,
}

#[derive(Clone, Copy)]
enum AliasBinding {
    Module(ModuleId),
    Ambiguous,
}

struct ProjectResolver {
    semantic: Arc<SemanticProject>,
    symbols: Vec<ProjectSymbol>,
    module_paths: HashMap<String, ModuleId>,
    modules: Vec<ModuleIndex>,
    builtin_types: HashMap<String, SymbolId>,
    builtin_modules: HashMap<String, (SymbolId, HashMap<String, SymbolId>)>,
    prelude_values: HashMap<String, SymbolId>,
    references: Vec<ProjectReference>,
    diagnostics: Vec<Diagnostic>,
}

pub(crate) fn resolve_project(semantic: SemanticProject) -> ResolvedProject {
    let semantic = Arc::new(semantic);
    let mut resolver = ProjectResolver::new(semantic.clone());
    resolver.index_modules();
    let dependency_order = resolver.dependency_order();
    resolver.bind_imports();
    resolver.resolve_bodies();
    resolver.references.sort_by_key(|reference| {
        (
            reference.file_id.raw(),
            reference.range.start(),
            reference.semantic_id.raw(),
        )
    });
    resolver
        .diagnostics
        .sort_by_key(|diagnostic| (diagnostic.file_id().raw(), diagnostic.range().start()));
    let mut symbols_by_origin: HashMap<(ModuleId, SemanticId), Vec<SymbolId>> = HashMap::new();
    for symbol in &resolver.symbols {
        if let (Some(module_id), Some(semantic_id)) = (symbol.module_id, symbol.semantic_id) {
            symbols_by_origin
                .entry((module_id, semantic_id))
                .or_default()
                .push(symbol.id);
        }
    }
    let mut references_by_origin = HashMap::new();
    for reference in &resolver.references {
        let prior = references_by_origin.insert(
            (
                reference.file_id,
                reference.semantic_id,
                reference.namespace,
            ),
            reference.resolution,
        );
        debug_assert!(prior.is_none(), "one semantic name owns one resolution");
    }
    ResolvedProject {
        semantic,
        dependency_order,
        symbols: resolver.symbols,
        references: resolver.references,
        diagnostics: resolver.diagnostics,
        symbols_by_origin,
        references_by_origin,
    }
}

impl ProjectResolver {
    fn new(semantic: Arc<SemanticProject>) -> Self {
        let mut resolver = Self {
            modules: (0..semantic.modules.len())
                .map(|_| ModuleIndex::default())
                .collect(),
            semantic,
            symbols: Vec::new(),
            module_paths: HashMap::new(),
            builtin_types: HashMap::new(),
            builtin_modules: HashMap::new(),
            prelude_values: HashMap::new(),
            references: Vec::new(),
            diagnostics: Vec::new(),
        };
        resolver.index_builtins();
        resolver
    }

    fn index_builtins(&mut self) {
        for name in ["Bool", "Int", "List", "Option", "Text", "Void"] {
            let id = self.add_symbol(
                None,
                None,
                Namespace::Type,
                ProjectSymbolKind::BuiltinType,
                name,
                true,
            );
            self.builtin_types.insert(name.to_owned(), id);
        }
        for module_name in ["Int", "Text"] {
            let module = self.add_symbol(
                None,
                None,
                Namespace::Module,
                ProjectSymbolKind::BuiltinModule,
                module_name,
                true,
            );
            let member_name = if module_name == "Int" {
                "to_text"
            } else {
                "append"
            };
            let member = self.add_symbol(
                None,
                None,
                Namespace::Value,
                ProjectSymbolKind::BuiltinValue,
                member_name,
                true,
            );
            self.builtin_modules.insert(
                module_name.to_owned(),
                (module, HashMap::from([(member_name.to_owned(), member)])),
            );
        }
        for name in ["Failure", "False", "Success", "True"] {
            let id = self.add_symbol(
                None,
                None,
                Namespace::Value,
                ProjectSymbolKind::BuiltinValue,
                name,
                true,
            );
            self.prelude_values.insert(name.to_owned(), id);
        }
    }

    fn index_modules(&mut self) {
        for module_index in 0..self.semantic.modules.len() {
            let module_id = self.semantic.modules[module_index].id;
            let path = self.semantic.modules[module_index].path.clone();
            if self.module_paths.insert(path.clone(), module_id).is_some() {
                let module = &self.semantic.modules[module_index];
                self.diagnostics.push(name_error(
                    module,
                    "E_DUPLICATE_MODULE",
                    crate::ids::TextRange::empty(0),
                    format!("module path {path} occurs more than once"),
                ));
            }
            self.add_symbol(
                Some(module_id),
                None,
                Namespace::Module,
                ProjectSymbolKind::Module,
                &path,
                true,
            );
        }

        for module_index in 0..self.semantic.modules.len() {
            let declarations = self.semantic.modules[module_index].declarations.clone();
            for declaration in declarations {
                self.index_declaration(module_index, &declaration);
            }
            self.diagnostics
                .extend(self.semantic.modules[module_index].diagnostics.clone());
        }
    }

    fn dependency_order(&self) -> Vec<ModuleId> {
        fn visit(
            resolver: &ProjectResolver,
            module_id: ModuleId,
            visiting: &mut HashSet<ModuleId>,
            visited: &mut HashSet<ModuleId>,
            order: &mut Vec<ModuleId>,
        ) {
            if visited.contains(&module_id) || !visiting.insert(module_id) {
                return;
            }
            let module = &resolver.semantic.modules[module_id.raw() as usize];
            let mut dependencies = module
                .imports
                .iter()
                .filter_map(|import| resolver.module_paths.get(&import.path).copied())
                .collect::<Vec<_>>();
            dependencies.sort_unstable();
            dependencies.dedup();
            for dependency in dependencies {
                visit(resolver, dependency, visiting, visited, order);
            }
            visiting.remove(&module_id);
            visited.insert(module_id);
            order.push(module_id);
        }

        let mut visiting = HashSet::new();
        let mut visited = HashSet::new();
        let mut order = Vec::with_capacity(self.semantic.modules.len());
        for module in &self.semantic.modules {
            visit(self, module.id, &mut visiting, &mut visited, &mut order);
        }
        order
    }

    fn index_declaration(
        &mut self,
        module_index: usize,
        declaration: &crate::semantic::ModuleDeclaration,
    ) {
        let module_id = self.semantic.modules[module_index].id;
        let (namespace, kind) = match declaration.kind {
            ModuleDeclarationKind::Struct => (Namespace::Type, ProjectSymbolKind::Struct),
            ModuleDeclarationKind::Enum => (Namespace::Type, ProjectSymbolKind::Enum),
            ModuleDeclarationKind::Interface => (Namespace::Type, ProjectSymbolKind::Interface),
            ModuleDeclarationKind::Function => (Namespace::Value, ProjectSymbolKind::Function),
            ModuleDeclarationKind::Binding => (Namespace::Value, ProjectSymbolKind::ModuleBinding),
        };
        let id = self.add_symbol(
            Some(module_id),
            Some(declaration.semantic_id),
            namespace,
            kind,
            &declaration.name.text,
            declaration.public,
        );
        self.add_to_namespace(module_index, namespace, &declaration.name, id);

        if declaration.kind == ModuleDeclarationKind::Struct {
            let constructor = self.add_symbol(
                Some(module_id),
                Some(declaration.semantic_id),
                Namespace::Value,
                ProjectSymbolKind::StructConstructor,
                &declaration.name.text,
                false,
            );
            self.add_to_namespace(
                module_index,
                Namespace::Value,
                &declaration.name,
                constructor,
            );
        }
        if declaration.kind == ModuleDeclarationKind::Enum {
            for variant in &declaration.variants {
                let constructor = self.add_symbol(
                    Some(module_id),
                    Some(variant.semantic_id),
                    Namespace::Value,
                    ProjectSymbolKind::EnumConstructor,
                    &variant.name.text,
                    declaration.public,
                );
                self.add_to_namespace(module_index, Namespace::Value, &variant.name, constructor);
            }
        }
    }

    fn add_to_namespace(
        &mut self,
        module_index: usize,
        namespace: Namespace,
        name: &Name,
        id: SymbolId,
    ) {
        let table = match namespace {
            Namespace::Type => &mut self.modules[module_index].types,
            Namespace::Value => &mut self.modules[module_index].values,
            Namespace::Module => unreachable!("module declarations are project indexed"),
        };
        let existing = table.entry(name.text.clone()).or_default();
        if !existing.is_empty() {
            let module = &self.semantic.modules[module_index];
            self.diagnostics.push(name_error(
                module,
                "E_DUPLICATE_SYMBOL",
                name.range,
                format!("duplicate {:?} name `{}`", namespace, name.text),
            ));
        }
        existing.push(id);
    }

    fn bind_imports(&mut self) {
        for module_index in 0..self.semantic.modules.len() {
            let imports = self.semantic.modules[module_index].imports.clone();
            for import in imports {
                let Some(&target) = self.module_paths.get(&import.path) else {
                    let module = &self.semantic.modules[module_index];
                    self.diagnostics.push(name_error(
                        module,
                        "E_UNRESOLVED_MODULE",
                        import.range,
                        format!("cannot find module {}", import.path),
                    ));
                    continue;
                };
                match import.binding {
                    ImportBinding::Alias(alias) => {
                        self.bind_alias(module_index, target, alias, import.range)
                    }
                    ImportBinding::Members(members) => {
                        for member in members {
                            self.bind_member(module_index, target, &member);
                        }
                    }
                    ImportBinding::Missing => {
                        let module = &self.semantic.modules[module_index];
                        self.diagnostics.push(name_error(
                            module,
                            "E_MISSING_MODULE_ALIAS",
                            import.range,
                            "module imports require an explicit alias",
                        ));
                    }
                }
            }
        }
    }

    fn bind_alias(
        &mut self,
        module_index: usize,
        target: ModuleId,
        alias: Name,
        range: crate::ids::TextRange,
    ) {
        let prior = self.modules[module_index].aliases.get(&alias.text).copied();
        if prior.is_some() {
            self.modules[module_index]
                .aliases
                .insert(alias.text.clone(), AliasBinding::Ambiguous);
            let module = &self.semantic.modules[module_index];
            self.diagnostics.push(name_error(
                module,
                "E_IMPORT_BINDING_CONFLICT",
                range,
                format!("module alias {} is bound more than once", alias.text),
            ));
        } else {
            self.modules[module_index]
                .aliases
                .insert(alias.text, AliasBinding::Module(target));
        }
    }

    fn bind_member(&mut self, module_index: usize, target: ModuleId, member: &Name) {
        let target_index = target.raw() as usize;
        let types = self.modules[target_index]
            .types
            .get(&member.text)
            .cloned()
            .unwrap_or_default();
        let values = self.modules[target_index]
            .values
            .get(&member.text)
            .cloned()
            .unwrap_or_default();
        let mut bound = false;
        for id in types {
            if self.symbols[id.raw() as usize].public {
                bound = true;
                self.import_symbol(module_index, Namespace::Type, member, id);
            }
        }
        for id in values {
            let symbol = &self.symbols[id.raw() as usize];
            if symbol.kind == ProjectSymbolKind::Function {
                bound = true;
                let module = &self.semantic.modules[module_index];
                self.diagnostics.push(name_error(
                    module,
                    "E_UNQUALIFIED_FUNCTION_IMPORT",
                    member.range,
                    format!(
                        "functions remain module-qualified; call {}.{}",
                        module_name(&self.semantic.modules[target_index].path),
                        member.text
                    ),
                ));
                if !self.modules[module_index].values.contains_key(&member.text)
                    && !self.modules[module_index]
                        .imported_values
                        .contains_key(&member.text)
                {
                    self.modules[module_index]
                        .imported_values
                        .insert(member.text.clone(), vec![id]);
                }
            } else if symbol.kind == ProjectSymbolKind::EnumConstructor && symbol.public {
                bound = true;
                self.import_symbol(module_index, Namespace::Value, member, id);
            }
        }
        if !bound {
            let module = &self.semantic.modules[module_index];
            self.diagnostics.push(name_error(
                module,
                "E_UNRESOLVED_IMPORT_MEMBER",
                member.range,
                format!(
                    "{} has no public type or constructor named {}",
                    module_name(&self.semantic.modules[target_index].path),
                    member.text
                ),
            ));
        }
    }

    fn import_symbol(
        &mut self,
        module_index: usize,
        namespace: Namespace,
        member: &Name,
        id: SymbolId,
    ) {
        let local_conflict = match namespace {
            Namespace::Type => self.modules[module_index].types.contains_key(&member.text),
            Namespace::Value => self.modules[module_index].values.contains_key(&member.text),
            Namespace::Module => false,
        };
        let imports = match namespace {
            Namespace::Type => &mut self.modules[module_index].imported_types,
            Namespace::Value => &mut self.modules[module_index].imported_values,
            Namespace::Module => unreachable!(),
        };
        if local_conflict || imports.contains_key(&member.text) {
            match namespace {
                Namespace::Type => {
                    self.modules[module_index]
                        .ambiguous_types
                        .insert(member.text.clone());
                }
                Namespace::Value => {
                    self.modules[module_index]
                        .ambiguous_values
                        .insert(member.text.clone());
                }
                Namespace::Module => unreachable!(),
            }
            let module = &self.semantic.modules[module_index];
            self.diagnostics.push(name_error(
                module,
                "E_IMPORT_MEMBER_CONFLICT",
                member.range,
                format!("import conflicts with existing name {}", member.text),
            ));
        } else {
            imports.insert(member.text.clone(), vec![id]);
        }
    }

    fn resolve_bodies(&mut self) {
        for module_index in 0..self.semantic.modules.len() {
            let declarations = self.semantic.modules[module_index].declarations.clone();
            for declaration in declarations {
                self.resolve_declaration(module_index, &declaration);
            }
        }
    }

    fn resolve_declaration(
        &mut self,
        module_index: usize,
        declaration: &crate::semantic::ModuleDeclaration,
    ) {
        let module_id = self.semantic.modules[module_index].id;
        let mut generics = HashMap::new();
        for name in &declaration.generic_parameters {
            if generics.contains_key(&name.text) {
                let module = &self.semantic.modules[module_index];
                self.diagnostics.push(name_error(
                    module,
                    "E_DUPLICATE_GENERIC_PARAMETER",
                    name.range,
                    format!("generic parameter {} is declared more than once", name.text),
                ));
                continue;
            }
            let id = self.add_symbol(
                Some(module_id),
                Some(declaration.semantic_id),
                Namespace::Type,
                ProjectSymbolKind::GenericParameter,
                &name.text,
                false,
            );
            generics.insert(name.text.clone(), id);
        }
        for field in &declaration.fields {
            self.resolve_type(module_index, &field.type_use, &generics);
        }
        for variant in &declaration.variants {
            for field in &variant.fields {
                self.resolve_type(module_index, &field.type_use, &generics);
            }
        }
        let mut scope = Scope::default();
        if let Some(signature) = &declaration.function {
            for parameter in &signature.parameters {
                if let Some(type_use) = &parameter.type_use {
                    self.resolve_type(module_index, type_use, &generics);
                }
                self.define_local(
                    module_index,
                    parameter.semantic_id,
                    &parameter.name,
                    ProjectSymbolKind::Parameter,
                    &mut scope,
                );
            }
            if let Some(success) = &signature.success {
                self.resolve_type(module_index, success, &generics);
            }
            self.resolve_requirement_row(
                module_index,
                &signature.requirements,
                &generics,
                Some(&mut scope),
            );
        }
        if let Some(body) = &declaration.body {
            self.resolve_expression(module_index, body, &generics, &mut scope);
        }
    }

    fn resolve_type(
        &mut self,
        module_index: usize,
        type_use: &TypeUse,
        generics: &HashMap<String, SymbolId>,
    ) {
        let TypeUse::Named {
            semantic_id,
            path,
            arguments,
        } = type_use
        else {
            let TypeUse::Function {
                inputs,
                success,
                failures,
                requirements,
                ..
            } = type_use
            else {
                unreachable!()
            };
            for input in inputs {
                self.resolve_type(module_index, input, generics);
            }
            self.resolve_type(module_index, success, generics);
            if let Some(failures) = failures {
                self.resolve_type(module_index, failures, generics);
            }
            self.resolve_requirement_row(module_index, requirements, generics, None);
            return;
        };
        if let Some(first) = path.first() {
            let resolution = if path.len() == 1 {
                if let Some(id) = generics.get(&first.text).copied() {
                    ReferenceResolution::Resolved(id)
                } else if self.modules[module_index]
                    .ambiguous_types
                    .contains(&first.text)
                {
                    ReferenceResolution::Ambiguous
                } else {
                    resolve_local(
                        self.modules[module_index].types.get(&first.text),
                        self.modules[module_index].imported_types.get(&first.text),
                        self.builtin_types.get(&first.text).copied(),
                    )
                }
            } else {
                self.resolve_qualified_type(module_index, path)
            };
            let display = path
                .iter()
                .map(|part| part.text.as_str())
                .collect::<Vec<_>>()
                .join(".");
            let name = path.last().expect("nonempty path");
            self.references.push(ProjectReference {
                file_id: self.semantic.modules[module_index].source.file_id(),
                semantic_id: *semantic_id,
                namespace: Namespace::Type,
                text: display.clone(),
                range: name.range,
                resolution,
            });
            if resolution == ReferenceResolution::Unresolved {
                let module = &self.semantic.modules[module_index];
                self.diagnostics.push(name_error(
                    module,
                    "E_UNRESOLVED_TYPE",
                    name.range,
                    format!("cannot resolve type {display}"),
                ));
            } else if resolution == ReferenceResolution::Ambiguous {
                let module = &self.semantic.modules[module_index];
                self.diagnostics.push(name_error(
                    module,
                    "E_AMBIGUOUS_TYPE",
                    name.range,
                    format!("multiple types are named {display}"),
                ));
            } else if matches!(resolution, ReferenceResolution::Inaccessible(_)) {
                let module = &self.semantic.modules[module_index];
                self.diagnostics.push(name_error(
                    module,
                    "E_PRIVATE_SYMBOL",
                    name.range,
                    format!("type {display} is module-private"),
                ));
            }
        }
        for argument in arguments {
            self.resolve_type(module_index, argument, generics);
        }
    }

    fn resolve_qualified_type(&self, module_index: usize, path: &[Name]) -> ReferenceResolution {
        let alias = &path[0].text;
        let name = &path.last().expect("qualified type has a member").text;
        match self.modules[module_index].aliases.get(alias) {
            Some(AliasBinding::Ambiguous) => ReferenceResolution::Ambiguous,
            Some(AliasBinding::Module(target)) => {
                let ids = self.modules[target.raw() as usize].types.get(name);
                resolve_external(ids, &self.symbols)
            }
            None => ReferenceResolution::Unresolved,
        }
    }

    fn resolve_requirement_row(
        &mut self,
        module_index: usize,
        row: &RequirementRowUse,
        generics: &HashMap<String, SymbolId>,
        mut scope: Option<&mut Scope>,
    ) {
        for term in &row.terms {
            match term {
                RequirementTermUse::Variable { semantic_id, name } => {
                    let resolution = generics
                        .get(&name.text)
                        .copied()
                        .map(ReferenceResolution::Resolved)
                        .unwrap_or(ReferenceResolution::Unresolved);
                    self.reference(
                        module_index,
                        *semantic_id,
                        Namespace::Type,
                        name,
                        resolution,
                    );
                    if resolution == ReferenceResolution::Unresolved {
                        let module = &self.semantic.modules[module_index];
                        self.diagnostics.push(name_error(
                            module,
                            "E_UNRESOLVED_TYPE",
                            name.range,
                            format!("cannot resolve generic row {}", name.text),
                        ));
                    }
                }
                RequirementTermUse::Field {
                    semantic_id,
                    name,
                    interface,
                } => {
                    self.resolve_type(module_index, interface, generics);
                    if let Some(scope) = scope.as_deref_mut() {
                        self.define_local(
                            module_index,
                            *semantic_id,
                            name,
                            ProjectSymbolKind::Requirement,
                            scope,
                        );
                    } else {
                        self.add_symbol(
                            Some(self.semantic.modules[module_index].id),
                            Some(*semantic_id),
                            Namespace::Value,
                            ProjectSymbolKind::Requirement,
                            &name.text,
                            false,
                        );
                    }
                }
            }
        }
    }

    fn resolve_expression(
        &mut self,
        module_index: usize,
        expression: &ModuleExpression,
        generics: &HashMap<String, SymbolId>,
        scope: &mut Scope,
    ) {
        match expression {
            ModuleExpression::Literal(_) => {}
            ModuleExpression::Name { semantic_id, name } => {
                self.resolve_value_name(module_index, *semantic_id, name, scope);
            }
            ModuleExpression::Select {
                semantic_id,
                receiver,
                member,
                range,
            } => {
                if let ModuleExpression::Name {
                    semantic_id: base_id,
                    name: base,
                } = receiver.as_ref()
                    && self.resolve_module_select(
                        module_index,
                        *base_id,
                        *semantic_id,
                        base,
                        member,
                        *range,
                    )
                {
                    return;
                }
                self.resolve_expression(module_index, receiver, generics, scope);
                self.reference(
                    module_index,
                    *semantic_id,
                    Namespace::Value,
                    member,
                    ReferenceResolution::DeferredToType,
                );
            }
            ModuleExpression::Call {
                callee, arguments, ..
            } => {
                self.resolve_expression(module_index, callee, generics, scope);
                for argument in arguments {
                    self.resolve_expression(module_index, &argument.value, generics, scope);
                }
            }
            ModuleExpression::List { elements, .. } => {
                for element in elements {
                    self.resolve_expression(module_index, element, generics, scope);
                }
            }
            ModuleExpression::Record { fields, .. } => {
                for field in fields {
                    self.resolve_expression(module_index, &field.value, generics, scope);
                }
            }
            ModuleExpression::Unary { operand, .. } => {
                self.resolve_expression(module_index, operand, generics, scope);
            }
            ModuleExpression::Binary { left, right, .. } => {
                self.resolve_expression(module_index, left, generics, scope);
                self.resolve_expression(module_index, right, generics, scope);
            }
            ModuleExpression::Group { expression, .. } => {
                self.resolve_expression(module_index, expression, generics, scope);
            }
            ModuleExpression::Using {
                expression,
                providers,
                ..
            } => {
                self.resolve_expression(module_index, expression, generics, scope);
                for provider in providers {
                    self.resolve_expression(module_index, &provider.value, generics, scope);
                }
            }
            ModuleExpression::Sequence { expressions, .. } => {
                for child in expressions {
                    self.resolve_expression(module_index, child, generics, scope);
                }
            }
            ModuleExpression::Block { items, .. } => {
                let mut block = Scope::child(scope);
                for item in items {
                    match item {
                        ModuleBlockItem::Binding { name, value } => {
                            self.resolve_expression(module_index, value, generics, &mut block);
                            self.define_local(
                                module_index,
                                value.semantic_id(),
                                name,
                                ProjectSymbolKind::LocalBinding,
                                &mut block,
                            );
                        }
                        ModuleBlockItem::Expression(value) => {
                            self.resolve_expression(module_index, value, generics, &mut block);
                        }
                    }
                }
            }
            ModuleExpression::Function {
                signature, body, ..
            } => {
                let mut function = Scope::child(scope);
                for parameter in &signature.parameters {
                    if let Some(type_use) = &parameter.type_use {
                        self.resolve_type(module_index, type_use, generics);
                    }
                    self.define_local(
                        module_index,
                        parameter.semantic_id,
                        &parameter.name,
                        ProjectSymbolKind::Parameter,
                        &mut function,
                    );
                }
                if let Some(success) = &signature.success {
                    self.resolve_type(module_index, success, generics);
                }
                self.resolve_requirement_row(
                    module_index,
                    &signature.requirements,
                    generics,
                    Some(&mut function),
                );
                self.resolve_expression(module_index, body, generics, &mut function);
            }
            ModuleExpression::Case { subjects, arms, .. } => {
                for subject in subjects {
                    self.resolve_expression(module_index, subject, generics, scope);
                }
                for arm in arms {
                    self.resolve_case_arm(module_index, arm, generics, scope);
                }
            }
            ModuleExpression::Empty { .. } => {}
        }
    }

    fn resolve_case_arm(
        &mut self,
        module_index: usize,
        arm: &ModuleCaseArm,
        generics: &HashMap<String, SymbolId>,
        parent: &Scope,
    ) {
        let mut scope = Scope::child(parent);
        for pattern in &arm.patterns {
            self.resolve_pattern(module_index, pattern, &mut scope);
        }
        if let Some(guard) = &arm.guard {
            self.resolve_expression(module_index, guard, generics, &mut scope);
        }
        self.resolve_expression(module_index, &arm.body, generics, &mut scope);
    }

    fn resolve_pattern(&mut self, module_index: usize, pattern: &Pattern, scope: &mut Scope) {
        if let Some(name) = &pattern.name {
            if pattern.applied || starts_upper(&name.text) {
                let resolution = if self.modules[module_index]
                    .ambiguous_values
                    .contains(&name.text)
                {
                    ReferenceResolution::Ambiguous
                } else {
                    resolve_local(
                        self.modules[module_index].values.get(&name.text),
                        self.modules[module_index].imported_values.get(&name.text),
                        self.prelude_values.get(&name.text).copied(),
                    )
                };
                self.reference(
                    module_index,
                    pattern.semantic_id,
                    Namespace::Value,
                    name,
                    resolution,
                );
                if resolution == ReferenceResolution::Unresolved {
                    let module = &self.semantic.modules[module_index];
                    self.diagnostics.push(name_error(
                        module,
                        "E_UNRESOLVED_CONSTRUCTOR",
                        name.range,
                        format!("cannot resolve constructor {}", name.text),
                    ));
                } else if resolution == ReferenceResolution::Ambiguous {
                    let module = &self.semantic.modules[module_index];
                    self.diagnostics.push(name_error(
                        module,
                        "E_AMBIGUOUS_CONSTRUCTOR",
                        name.range,
                        format!("multiple constructors are named {}", name.text),
                    ));
                }
            } else {
                self.define_local(
                    module_index,
                    pattern.semantic_id,
                    name,
                    ProjectSymbolKind::PatternBinding,
                    scope,
                );
            }
        }
        for argument in &pattern.arguments {
            self.resolve_pattern(module_index, argument, scope);
        }
    }

    fn resolve_module_select(
        &mut self,
        module_index: usize,
        base_semantic_id: SemanticId,
        member_semantic_id: SemanticId,
        base: &Name,
        member: &Name,
        expression_range: crate::ids::TextRange,
    ) -> bool {
        if let Some(binding) = self.modules[module_index].aliases.get(&base.text).copied() {
            match binding {
                AliasBinding::Ambiguous => {
                    self.reference(
                        module_index,
                        base_semantic_id,
                        Namespace::Module,
                        base,
                        ReferenceResolution::Ambiguous,
                    );
                    self.reference(
                        module_index,
                        member_semantic_id,
                        Namespace::Value,
                        member,
                        ReferenceResolution::Ambiguous,
                    );
                }
                AliasBinding::Module(target) => {
                    let module_symbol = self
                        .symbols
                        .iter()
                        .find(|symbol| {
                            symbol.kind == ProjectSymbolKind::Module
                                && symbol.module_id == Some(target)
                        })
                        .expect("every module has a symbol")
                        .id;
                    self.reference(
                        module_index,
                        base_semantic_id,
                        Namespace::Module,
                        base,
                        ReferenceResolution::Resolved(module_symbol),
                    );
                    let resolution = resolve_external(
                        self.modules[target.raw() as usize].values.get(&member.text),
                        &self.symbols,
                    );
                    self.reference(
                        module_index,
                        member_semantic_id,
                        Namespace::Value,
                        member,
                        resolution,
                    );
                    self.external_diagnostic(
                        module_index,
                        target,
                        member,
                        expression_range,
                        resolution,
                    );
                }
            }
            return true;
        }
        if let Some((module, members)) = self.builtin_modules.get(&base.text) {
            let module = *module;
            let member_resolution = members
                .get(&member.text)
                .copied()
                .map(ReferenceResolution::Resolved)
                .unwrap_or(ReferenceResolution::Unresolved);
            self.reference(
                module_index,
                base_semantic_id,
                Namespace::Module,
                base,
                ReferenceResolution::Resolved(module),
            );
            self.reference(
                module_index,
                member_semantic_id,
                Namespace::Value,
                member,
                member_resolution,
            );
            if member_resolution == ReferenceResolution::Unresolved {
                let current = &self.semantic.modules[module_index];
                self.diagnostics.push(name_error(
                    current,
                    "E_UNRESOLVED_MEMBER",
                    member.range,
                    format!("{} has no member {}", base.text, member.text),
                ));
            }
            return true;
        }
        false
    }

    fn external_diagnostic(
        &mut self,
        module_index: usize,
        target: ModuleId,
        member: &Name,
        expression_range: crate::ids::TextRange,
        resolution: ReferenceResolution,
    ) {
        let current = &self.semantic.modules[module_index];
        match resolution {
            ReferenceResolution::Unresolved => self.diagnostics.push(name_error(
                current,
                "E_UNRESOLVED_MEMBER",
                member.range,
                format!(
                    "{} has no value named {}",
                    module_name(&self.semantic.modules[target.raw() as usize].path),
                    member.text
                ),
            )),
            ReferenceResolution::Ambiguous => self.diagnostics.push(name_error(
                current,
                "E_AMBIGUOUS_MEMBER",
                member.range,
                format!(
                    "{} has multiple members named {}",
                    module_name(&self.semantic.modules[target.raw() as usize].path),
                    member.text
                ),
            )),
            ReferenceResolution::Inaccessible(id) => {
                let kind = self.symbols[id.raw() as usize].kind;
                let code = if kind == ProjectSymbolKind::StructConstructor {
                    "E_PRIVATE_STRUCT_CONSTRUCTOR"
                } else {
                    "E_PRIVATE_SYMBOL"
                };
                self.diagnostics.push(name_error(
                    current,
                    code,
                    if kind == ProjectSymbolKind::StructConstructor {
                        expression_range
                    } else {
                        member.range
                    },
                    format!(
                        "{}.{} is module-private",
                        module_name(&self.semantic.modules[target.raw() as usize].path),
                        member.text
                    ),
                ));
            }
            ReferenceResolution::Resolved(_) | ReferenceResolution::DeferredToType => {}
        }
    }

    fn resolve_value_name(
        &mut self,
        module_index: usize,
        semantic_id: SemanticId,
        name: &Name,
        scope: &Scope,
    ) {
        let resolution = if let Some(id) = scope.lookup(&name.text) {
            ReferenceResolution::Resolved(id)
        } else if self.modules[module_index]
            .ambiguous_values
            .contains(&name.text)
        {
            ReferenceResolution::Ambiguous
        } else {
            resolve_local(
                self.modules[module_index].values.get(&name.text),
                self.modules[module_index].imported_values.get(&name.text),
                self.prelude_values.get(&name.text).copied(),
            )
        };
        self.reference(
            module_index,
            semantic_id,
            Namespace::Value,
            name,
            resolution,
        );
        if resolution == ReferenceResolution::Unresolved {
            let module = &self.semantic.modules[module_index];
            self.diagnostics.push(name_error(
                module,
                "E_UNRESOLVED_VALUE",
                name.range,
                format!("cannot resolve value {}", name.text),
            ));
        } else if resolution == ReferenceResolution::Ambiguous {
            let module = &self.semantic.modules[module_index];
            self.diagnostics.push(name_error(
                module,
                "E_AMBIGUOUS_VALUE",
                name.range,
                format!("multiple values are named {}", name.text),
            ));
        }
    }

    fn define_local(
        &mut self,
        module_index: usize,
        semantic_id: SemanticId,
        name: &Name,
        kind: ProjectSymbolKind,
        scope: &mut Scope,
    ) {
        if name.text == "_" {
            return;
        }
        if scope.values.contains_key(&name.text) {
            let module = &self.semantic.modules[module_index];
            self.diagnostics.push(name_error(
                module,
                "E_DUPLICATE_LOCAL",
                name.range,
                format!("duplicate local name {}", name.text),
            ));
            return;
        }
        let id = self.add_symbol(
            Some(self.semantic.modules[module_index].id),
            Some(semantic_id),
            Namespace::Value,
            kind,
            &name.text,
            false,
        );
        scope.values.insert(name.text.clone(), id);
    }

    fn reference(
        &mut self,
        module_index: usize,
        semantic_id: SemanticId,
        namespace: Namespace,
        name: &Name,
        resolution: ReferenceResolution,
    ) {
        self.references.push(ProjectReference {
            file_id: self.semantic.modules[module_index].source.file_id(),
            semantic_id,
            namespace,
            text: name.text.clone(),
            range: name.range,
            resolution,
        });
    }

    fn add_symbol(
        &mut self,
        module_id: Option<ModuleId>,
        semantic_id: Option<SemanticId>,
        namespace: Namespace,
        kind: ProjectSymbolKind,
        name: &str,
        public: bool,
    ) -> SymbolId {
        let id = SymbolId::new(self.symbols.len() as u32);
        self.symbols.push(ProjectSymbol {
            id,
            module_id,
            semantic_id,
            namespace,
            kind,
            name: name.to_owned(),
            public,
        });
        id
    }
}

#[derive(Default)]
struct Scope {
    parents: Vec<HashMap<String, SymbolId>>,
    values: HashMap<String, SymbolId>,
}

impl Scope {
    fn child(parent: &Self) -> Self {
        let mut parents = parent.parents.clone();
        parents.push(parent.values.clone());
        Self {
            parents,
            values: HashMap::new(),
        }
    }

    fn lookup(&self, name: &str) -> Option<SymbolId> {
        self.values.get(name).copied().or_else(|| {
            self.parents
                .iter()
                .rev()
                .find_map(|scope| scope.get(name).copied())
        })
    }
}

fn resolve_local(
    local: Option<&Vec<SymbolId>>,
    imported: Option<&Vec<SymbolId>>,
    builtin: Option<SymbolId>,
) -> ReferenceResolution {
    match (local, imported) {
        (Some(values), _) if values.len() > 1 => ReferenceResolution::Ambiguous,
        (Some(values), _) => ReferenceResolution::Resolved(values[0]),
        (None, Some(values)) if values.len() > 1 => ReferenceResolution::Ambiguous,
        (None, Some(values)) => ReferenceResolution::Resolved(values[0]),
        (None, None) => builtin
            .map(ReferenceResolution::Resolved)
            .unwrap_or(ReferenceResolution::Unresolved),
    }
}

fn resolve_external(
    values: Option<&Vec<SymbolId>>,
    symbols: &[ProjectSymbol],
) -> ReferenceResolution {
    let Some(values) = values else {
        return ReferenceResolution::Unresolved;
    };
    if values.len() != 1 {
        return ReferenceResolution::Ambiguous;
    }
    let id = values[0];
    if symbols[id.raw() as usize].public {
        ReferenceResolution::Resolved(id)
    } else {
        ReferenceResolution::Inaccessible(id)
    }
}

fn starts_upper(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

fn module_name(path: &str) -> String {
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .split('_')
        .map(|part| {
            let mut characters = part.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(characters).collect()
            })
        })
        .collect()
}

fn name_error(
    module: &SemanticModule,
    code: &'static str,
    range: crate::ids::TextRange,
    message: impl Into<String>,
) -> Diagnostic {
    Diagnostic::error(
        DiagnosticStage::Name,
        code,
        module.source.file_id(),
        range,
        message,
    )
}
