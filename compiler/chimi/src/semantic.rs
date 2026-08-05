use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::{FileId, ModuleId, SemanticId, TextRange};
use crate::source::SourceSnapshot;
use crate::syntax::{
    self, DeclarationSyntax, FileSyntax, FunctionDeclarationSyntax, NodeKind,
    StructDeclarationSyntax, SyntaxNode, SyntaxToken, TokenKind, TypedSyntaxNode,
};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(crate) struct Name {
    pub(crate) text: String,
    pub(crate) range: TextRange,
}

#[derive(Clone, Debug)]
pub(crate) struct StructDeclaration {
    pub(crate) semantic_id: SemanticId,
    pub(crate) name: Name,
    pub(crate) field_semantic_id: SemanticId,
    pub(crate) field_name: Name,
    pub(crate) field_type: Name,
}

#[derive(Clone, Debug)]
pub(crate) struct FunctionDeclaration {
    pub(crate) semantic_id: SemanticId,
    pub(crate) public: bool,
    pub(crate) name: Name,
    pub(crate) body: Construction,
}

#[derive(Clone, Debug)]
pub(crate) struct Construction {
    pub(crate) semantic_id: SemanticId,
    pub(crate) range: TextRange,
    pub(crate) constructor: Name,
    pub(crate) field_name: Name,
    pub(crate) value: Literal,
}

#[derive(Clone, Debug)]
pub(crate) enum Literal {
    Integer {
        semantic_id: SemanticId,
        value: i64,
        range: TextRange,
    },
    Text {
        semantic_id: SemanticId,
        value: String,
        range: TextRange,
    },
}

impl Literal {
    pub(crate) const fn semantic_id(&self) -> SemanticId {
        match self {
            Self::Integer { semantic_id, .. } | Self::Text { semantic_id, .. } => *semantic_id,
        }
    }

    pub(crate) const fn range(&self) -> TextRange {
        match self {
            Self::Integer { range, .. } | Self::Text { range, .. } => *range,
        }
    }
}

#[derive(Debug)]
pub(crate) struct SemanticProgram {
    pub(crate) source: Arc<SourceSnapshot>,
    pub(crate) structure: Option<StructDeclaration>,
    pub(crate) functions: Vec<FunctionDeclaration>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

pub(crate) fn lower(source: Arc<SourceSnapshot>) -> SemanticProgram {
    let parsed = syntax::parse(&source);
    let mut lowerer = Lowerer::new(&source, parsed.diagnostics().to_vec());
    let file = FileSyntax::cast(parsed.root()).expect("the parser root is always a file");

    for declaration in file.declarations() {
        match declaration {
            DeclarationSyntax::Struct(declaration) if lowerer.structure.is_none() => {
                lowerer.structure = lowerer.lower_struct(&declaration);
            }
            DeclarationSyntax::Struct(declaration) => {
                lowerer.lowering_error(
                    declaration.syntax(),
                    "additional struct declarations are not lowered by the executable subset",
                );
            }
            DeclarationSyntax::Function(declaration) if lowerer.functions.is_empty() => {
                if let Some(function) = lowerer.lower_function(&declaration) {
                    lowerer.functions.push(function);
                }
            }
            DeclarationSyntax::Function(declaration) => {
                lowerer.lowering_error(
                    declaration.syntax(),
                    "additional function declarations are not lowered by the executable subset",
                );
            }
            DeclarationSyntax::Unsupported(declaration) => {
                lowerer.lowering_error(
                    &declaration,
                    "declaration is parsed but not lowered by the executable subset",
                );
            }
        }
    }

    let Lowerer {
        structure,
        functions,
        diagnostics,
        ..
    } = lowerer;
    SemanticProgram {
        source,
        structure,
        functions,
        diagnostics,
    }
}

struct Lowerer<'source> {
    source: &'source SourceSnapshot,
    next_semantic_id: u32,
    structure: Option<StructDeclaration>,
    functions: Vec<FunctionDeclaration>,
    diagnostics: Vec<Diagnostic>,
}

impl<'source> Lowerer<'source> {
    fn new(source: &'source SourceSnapshot, diagnostics: Vec<Diagnostic>) -> Self {
        Self {
            source,
            next_semantic_id: 0,
            structure: None,
            functions: Vec::new(),
            diagnostics,
        }
    }

    fn lower_struct(
        &mut self,
        declaration: &StructDeclarationSyntax<'_>,
    ) -> Option<StructDeclaration> {
        let node = declaration.syntax();
        let Some(name) = declaration.name_token() else {
            self.lowering_error(node, "incomplete struct declaration cannot be lowered");
            return None;
        };
        let Some(field) = declaration.fields().into_iter().next() else {
            self.lowering_error(node, "incomplete struct declaration cannot be lowered");
            return None;
        };
        let Some(field_name) = field.name_token() else {
            self.lowering_error(node, "incomplete struct declaration cannot be lowered");
            return None;
        };
        let tokens = significant_tokens(node);
        let [_, _, _, _, _, field_type, _] = tokens.as_slice() else {
            self.lowering_error(node, "incomplete struct declaration cannot be lowered");
            return None;
        };
        if !matches_kinds(
            &tokens,
            &[
                TokenKind::Struct,
                TokenKind::Identifier,
                TokenKind::LeftBrace,
                TokenKind::Identifier,
                TokenKind::Colon,
                TokenKind::Identifier,
                TokenKind::RightBrace,
            ],
        ) {
            self.lowering_error(node, "malformed struct declaration cannot be lowered");
            return None;
        }
        Some(StructDeclaration {
            semantic_id: self.allocate_semantic_id(),
            name: name_from(&name),
            field_semantic_id: self.allocate_semantic_id(),
            field_name: name_from(&field_name),
            field_type: name_from(field_type),
        })
    }

    fn lower_function(
        &mut self,
        declaration: &FunctionDeclarationSyntax<'_>,
    ) -> Option<FunctionDeclaration> {
        let node = declaration.syntax();
        let Some(function_name) = declaration.name_token() else {
            self.lowering_error(node, "incomplete function declaration cannot be lowered");
            return None;
        };
        let Some(body_syntax) = declaration.body() else {
            self.lowering_error(node, "incomplete function declaration cannot be lowered");
            return None;
        };
        let tokens = significant_tokens(node);
        if tokens.len() != 13
            || !matches_kinds(
                &tokens[..10],
                &[
                    TokenKind::Pub,
                    TokenKind::Fn,
                    TokenKind::Identifier,
                    TokenKind::LeftParen,
                    TokenKind::RightParen,
                    TokenKind::LeftBrace,
                    TokenKind::Identifier,
                    TokenKind::LeftParen,
                    TokenKind::Identifier,
                    TokenKind::Colon,
                ],
            )
            || !matches!(tokens[10].kind(), TokenKind::Integer | TokenKind::Text)
            || !matches_kinds(
                &tokens[11..],
                &[TokenKind::RightParen, TokenKind::RightBrace],
            )
        {
            self.lowering_error(
                node,
                "function is parsed but not part of the executable subset",
            );
            return None;
        }
        let function_semantic_id = self.allocate_semantic_id();
        let body = self.lower_construction(&tokens[6..12], body_syntax.syntax())?;

        Some(FunctionDeclaration {
            semantic_id: function_semantic_id,
            public: declaration.is_public(),
            name: name_from(&function_name),
            body,
        })
    }

    fn lower_construction(
        &mut self,
        tokens: &[SyntaxToken<'_>],
        owner: &SyntaxNode<'_>,
    ) -> Option<Construction> {
        let [constructor, _, field_name, _, value, right_paren] = tokens else {
            self.lowering_error(
                owner,
                "incomplete construction expression cannot be lowered",
            );
            return None;
        };
        if !matches_kinds(
            &tokens[..4],
            &[
                TokenKind::Identifier,
                TokenKind::LeftParen,
                TokenKind::Identifier,
                TokenKind::Colon,
            ],
        ) || right_paren.kind() != TokenKind::RightParen
        {
            self.lowering_error(owner, "malformed construction expression cannot be lowered");
            return None;
        }
        let construction_semantic_id = self.allocate_semantic_id();
        let literal_semantic_id = self.allocate_semantic_id();
        let literal = match value.kind() {
            TokenKind::Integer => match value.text().replace('_', "").parse() {
                Ok(integer) => Literal::Integer {
                    semantic_id: literal_semantic_id,
                    value: integer,
                    range: value.range(),
                },
                Err(_) => {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticStage::Syntax,
                        "S1002",
                        self.source.file_id(),
                        value.range(),
                        "integer literal is outside the supported range",
                    ));
                    return None;
                }
            },
            TokenKind::Text => Literal::Text {
                semantic_id: literal_semantic_id,
                value: value.text()[1..value.text().len() - 1].to_owned(),
                range: value.range(),
            },
            _ => {
                self.lowering_error(owner, "construction value is not a supported literal");
                return None;
            }
        };

        Some(Construction {
            semantic_id: construction_semantic_id,
            range: TextRange::new(
                constructor.range().start() as usize,
                right_paren.range().end() as usize,
            ),
            constructor: name_from(constructor),
            field_name: name_from(field_name),
            value: literal,
        })
    }

    fn lowering_error(&mut self, node: &SyntaxNode<'_>, message: &'static str) {
        if self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.stage() == DiagnosticStage::Syntax)
        {
            return;
        }
        let tokens = significant_tokens(node);
        let range = tokens.first().map_or_else(
            || TextRange::empty(self.source.text().len()),
            |token| token.range(),
        );
        self.diagnostics.push(Diagnostic::error(
            DiagnosticStage::Lowering,
            "L2001",
            self.source.file_id(),
            range,
            message,
        ));
    }

    fn allocate_semantic_id(&mut self) -> SemanticId {
        let identifier = SemanticId::new(self.next_semantic_id);
        self.next_semantic_id += 1;
        identifier
    }
}

fn significant_tokens<'tree>(node: &SyntaxNode<'tree>) -> Vec<SyntaxToken<'tree>> {
    node.descendant_tokens()
        .into_iter()
        .filter(|token| {
            !token.is_missing()
                && !matches!(
                    token.kind(),
                    TokenKind::Whitespace
                        | TokenKind::Newline
                        | TokenKind::Comment
                        | TokenKind::End
                )
        })
        .collect()
}

fn matches_kinds(tokens: &[SyntaxToken<'_>], kinds: &[TokenKind]) -> bool {
    tokens.len() == kinds.len()
        && tokens
            .iter()
            .zip(kinds)
            .all(|(token, kind)| token.kind() == *kind)
}

fn name_from(token: &SyntaxToken<'_>) -> Name {
    Name {
        text: token.text().to_owned(),
        range: token.range(),
    }
}

#[derive(Debug)]
pub(crate) struct ModuleSource {
    pub(crate) path: String,
    pub(crate) source: SourceSnapshot,
}

#[derive(Clone, Debug)]
pub(crate) struct ModuleImport {
    pub(crate) path: String,
    pub(crate) binding: ImportBinding,
    pub(crate) range: TextRange,
}

#[derive(Clone, Debug)]
pub(crate) enum ImportBinding {
    Alias(Name),
    Members(Vec<Name>),
    Missing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModuleDeclarationKind {
    Struct,
    Enum,
    Interface,
    Function,
    Binding,
}

#[derive(Clone, Debug)]
pub(crate) struct ModuleDeclaration {
    pub(crate) semantic_id: SemanticId,
    pub(crate) kind: ModuleDeclarationKind,
    pub(crate) public: bool,
    pub(crate) name: Name,
    pub(crate) generic_parameters: Vec<Name>,
    pub(crate) fields: Vec<TypedField>,
    pub(crate) variants: Vec<VariantDeclaration>,
    pub(crate) function: Option<FunctionSignature>,
    pub(crate) body: Option<ModuleExpression>,
}

#[derive(Clone, Debug)]
pub(crate) enum TypeUse {
    Named {
        semantic_id: SemanticId,
        path: Vec<Name>,
        arguments: Vec<TypeUse>,
    },
    Function {
        semantic_id: SemanticId,
        inputs: Vec<TypeUse>,
        success: Box<TypeUse>,
        failures: Option<Box<TypeUse>>,
        requirements: RequirementRowUse,
    },
}

impl TypeUse {
    pub(crate) const fn semantic_id(&self) -> SemanticId {
        match self {
            Self::Named { semantic_id, .. } | Self::Function { semantic_id, .. } => *semantic_id,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TypedField {
    pub(crate) semantic_id: SemanticId,
    pub(crate) name: Name,
    pub(crate) type_use: TypeUse,
}

#[derive(Clone, Debug)]
pub(crate) struct VariantDeclaration {
    pub(crate) semantic_id: SemanticId,
    pub(crate) name: Name,
    pub(crate) fields: Vec<TypedField>,
}

#[derive(Clone, Debug)]
pub(crate) struct ValueParameter {
    pub(crate) semantic_id: SemanticId,
    pub(crate) name: Name,
    pub(crate) type_use: Option<TypeUse>,
}

#[derive(Clone, Debug)]
pub(crate) struct FunctionSignature {
    pub(crate) parameters: Vec<ValueParameter>,
    pub(crate) success: Option<TypeUse>,
    pub(crate) requirements: RequirementRowUse,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RequirementRowUse {
    pub(crate) terms: Vec<RequirementTermUse>,
}

#[derive(Clone, Debug)]
pub(crate) enum RequirementTermUse {
    Variable {
        semantic_id: SemanticId,
        name: Name,
    },
    Field {
        semantic_id: SemanticId,
        name: Name,
        interface: TypeUse,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct Pattern {
    pub(crate) semantic_id: SemanticId,
    pub(crate) name: Option<Name>,
    pub(crate) applied: bool,
    pub(crate) arguments: Vec<Pattern>,
}

#[derive(Clone, Debug)]
pub(crate) enum ModuleExpression {
    Name {
        semantic_id: SemanticId,
        name: Name,
    },
    Select {
        semantic_id: SemanticId,
        receiver: Box<Self>,
        member: Name,
        range: TextRange,
    },
    Sequence {
        semantic_id: SemanticId,
        expressions: Vec<Self>,
    },
    Block {
        semantic_id: SemanticId,
        items: Vec<ModuleBlockItem>,
    },
    Function {
        semantic_id: SemanticId,
        signature: FunctionSignature,
        body: Box<Self>,
    },
    Case {
        semantic_id: SemanticId,
        subjects: Vec<Self>,
        arms: Vec<ModuleCaseArm>,
    },
    Empty {
        semantic_id: SemanticId,
    },
}

impl ModuleExpression {
    pub(crate) const fn semantic_id(&self) -> SemanticId {
        match self {
            Self::Name { semantic_id, .. }
            | Self::Select { semantic_id, .. }
            | Self::Sequence { semantic_id, .. }
            | Self::Block { semantic_id, .. }
            | Self::Function { semantic_id, .. }
            | Self::Case { semantic_id, .. }
            | Self::Empty { semantic_id } => *semantic_id,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum ModuleBlockItem {
    Binding { name: Name, value: ModuleExpression },
    Expression(ModuleExpression),
}

#[derive(Clone, Debug)]
pub(crate) struct ModuleCaseArm {
    pub(crate) patterns: Vec<Pattern>,
    pub(crate) guard: Option<ModuleExpression>,
    pub(crate) body: ModuleExpression,
}

#[derive(Debug)]
pub(crate) struct SemanticModule {
    pub(crate) id: ModuleId,
    pub(crate) path: String,
    pub(crate) source: Arc<SourceSnapshot>,
    pub(crate) imports: Vec<ModuleImport>,
    pub(crate) declarations: Vec<ModuleDeclaration>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

#[derive(Debug)]
pub(crate) struct SemanticProject {
    pub(crate) modules: Vec<SemanticModule>,
}

pub(crate) fn lower_project(mut inputs: Vec<ModuleSource>) -> SemanticProject {
    inputs.sort_by(|left, right| left.path.cmp(&right.path));
    let modules = inputs
        .into_iter()
        .enumerate()
        .map(|(index, mut input)| {
            input.source.assign_file_id(FileId::new(index as u32));
            ProjectLowerer::lower(ModuleId::new(index as u32), input.path, input.source)
        })
        .collect();
    SemanticProject { modules }
}

struct ProjectLowerer {
    next_semantic_id: u32,
}

impl ProjectLowerer {
    fn lower(id: ModuleId, path: String, source: SourceSnapshot) -> SemanticModule {
        let source = Arc::new(source);
        let parsed = syntax::parse(&source);
        let root = parsed.root();
        let mut lowerer = Self {
            next_semantic_id: 0,
        };
        let mut imports = Vec::new();
        let mut declarations = Vec::new();
        for node in root.child_nodes() {
            match node.kind() {
                NodeKind::ImportDeclaration => imports.push(lowerer.lower_import(&node)),
                NodeKind::StructDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::InterfaceDeclaration
                | NodeKind::FunctionDeclaration => {
                    if let Some(declaration) = lowerer.lower_module_declaration(&node) {
                        declarations.push(declaration);
                    }
                }
                NodeKind::LetBinding => {
                    if let Some(declaration) = lowerer.lower_module_binding(&node) {
                        declarations.push(declaration);
                    }
                }
                _ => {}
            }
        }
        SemanticModule {
            id,
            path,
            source,
            imports,
            declarations,
            diagnostics: parsed.diagnostics().to_vec(),
        }
    }

    fn lower_import(&mut self, node: &SyntaxNode<'_>) -> ModuleImport {
        let path = node
            .child_tokens()
            .into_iter()
            .filter(|token| token.kind() == TokenKind::Identifier && !token.is_missing())
            .map(|token| token.text().to_owned())
            .collect::<Vec<_>>()
            .join("/");
        let binding = node
            .child_nodes()
            .into_iter()
            .find_map(|child| match child.kind() {
                NodeKind::ImportAlias => direct_identifier(&child).map(ImportBinding::Alias),
                NodeKind::ImportMembers => Some(ImportBinding::Members(
                    child
                        .child_nodes()
                        .into_iter()
                        .filter(|member| member.kind() == NodeKind::ImportMember)
                        .filter_map(|member| direct_identifier(&member))
                        .collect(),
                )),
                _ => None,
            })
            .unwrap_or(ImportBinding::Missing);
        ModuleImport {
            path,
            binding,
            range: significant_range(node),
        }
    }

    fn lower_module_declaration(&mut self, node: &SyntaxNode<'_>) -> Option<ModuleDeclaration> {
        let (kind, keyword) = match node.kind() {
            NodeKind::StructDeclaration => (ModuleDeclarationKind::Struct, TokenKind::Struct),
            NodeKind::EnumDeclaration => (ModuleDeclarationKind::Enum, TokenKind::Enum),
            NodeKind::InterfaceDeclaration => {
                (ModuleDeclarationKind::Interface, TokenKind::Interface)
            }
            NodeKind::FunctionDeclaration => (ModuleDeclarationKind::Function, TokenKind::Fn),
            _ => return None,
        };
        let name = direct_token_after(node, keyword, TokenKind::Identifier)
            .map(|token| name_from(&token))?;
        let generic_parameters = direct_child(node, NodeKind::GenericParameters)
            .map(|generics| {
                generics
                    .child_nodes()
                    .into_iter()
                    .filter(|parameter| parameter.kind() == NodeKind::GenericParameter)
                    .filter_map(|parameter| direct_identifier(&parameter))
                    .collect()
            })
            .unwrap_or_default();
        let fields = node
            .child_nodes()
            .into_iter()
            .filter(|child| child.kind() == NodeKind::Field)
            .filter_map(|field| self.lower_typed_field(&field))
            .collect();
        let variants = node
            .child_nodes()
            .into_iter()
            .filter(|child| child.kind() == NodeKind::Variant)
            .filter_map(|variant| self.lower_variant(&variant))
            .collect();
        let function =
            (kind == ModuleDeclarationKind::Function).then(|| self.lower_function_signature(node));
        let body = if kind == ModuleDeclarationKind::Function {
            direct_child(node, NodeKind::BlockExpression).map(|body| self.lower_expression(&body))
        } else {
            None
        };
        Some(ModuleDeclaration {
            semantic_id: self.allocate_semantic_id(),
            kind,
            public: node
                .child_tokens()
                .iter()
                .any(|token| token.kind() == TokenKind::Pub),
            name,
            generic_parameters,
            fields,
            variants,
            function,
            body,
        })
    }

    fn lower_module_binding(&mut self, node: &SyntaxNode<'_>) -> Option<ModuleDeclaration> {
        let name = direct_identifier(node)?;
        let body = direct_expression_child(node).map(|value| self.lower_expression(&value));
        Some(ModuleDeclaration {
            semantic_id: self.allocate_semantic_id(),
            kind: ModuleDeclarationKind::Binding,
            public: false,
            name,
            generic_parameters: Vec::new(),
            fields: Vec::new(),
            variants: Vec::new(),
            function: None,
            body,
        })
    }

    fn lower_type(&mut self, node: &SyntaxNode<'_>) -> TypeUse {
        let semantic_id = self.allocate_semantic_id();
        if let Some(function) = direct_child(node, NodeKind::FunctionType) {
            let arrow_offset = function
                .child_tokens()
                .into_iter()
                .find(|token| token.kind() == TokenKind::Arrow)
                .map(|token| token.range().start())
                .unwrap_or(u32::MAX);
            let errors_offset = function
                .child_tokens()
                .into_iter()
                .find(|token| token.kind() == TokenKind::Errors)
                .map(|token| token.range().start())
                .unwrap_or(u32::MAX);
            let mut inputs = Vec::new();
            let mut success = None;
            let mut failures = None;
            for child in function.child_nodes() {
                if child.kind() != NodeKind::Type {
                    continue;
                }
                let lowered = self.lower_type(&child);
                if child.range().start() < arrow_offset {
                    inputs.push(lowered);
                } else if child.range().start() < errors_offset && success.is_none() {
                    success = Some(lowered);
                } else {
                    failures = Some(Box::new(lowered));
                }
            }
            let success = success.unwrap_or_else(|| TypeUse::Named {
                semantic_id: self.allocate_semantic_id(),
                path: Vec::new(),
                arguments: Vec::new(),
            });
            let requirements = direct_child(&function, NodeKind::RequirementClause)
                .map(|requirements| self.lower_requirement_row(&requirements))
                .unwrap_or_default();
            return TypeUse::Function {
                semantic_id,
                inputs,
                success: Box::new(success),
                failures,
                requirements,
            };
        }
        let path = node
            .child_tokens()
            .into_iter()
            .filter(|token| {
                matches!(token.kind(), TokenKind::Identifier | TokenKind::Void)
                    && !token.is_missing()
            })
            .map(|token| name_from(&token))
            .collect();
        let arguments = direct_child(node, NodeKind::TypeArguments)
            .map(|arguments| {
                arguments
                    .child_nodes()
                    .into_iter()
                    .filter(|child| child.kind() == NodeKind::Type)
                    .map(|child| self.lower_type(&child))
                    .collect()
            })
            .unwrap_or_default();
        TypeUse::Named {
            semantic_id,
            path,
            arguments,
        }
    }

    fn lower_typed_field(&mut self, node: &SyntaxNode<'_>) -> Option<TypedField> {
        let name = direct_identifier(node)?;
        let type_node = direct_child(node, NodeKind::Type)?;
        Some(TypedField {
            semantic_id: self.allocate_semantic_id(),
            name,
            type_use: self.lower_type(&type_node),
        })
    }

    fn lower_variant(&mut self, node: &SyntaxNode<'_>) -> Option<VariantDeclaration> {
        let name = direct_identifier(node)?;
        let fields = node
            .child_nodes()
            .into_iter()
            .filter(|child| child.kind() == NodeKind::NamedTypeField)
            .filter_map(|field| self.lower_typed_field(&field))
            .collect();
        Some(VariantDeclaration {
            semantic_id: self.allocate_semantic_id(),
            name,
            fields,
        })
    }

    fn lower_function_signature(&mut self, node: &SyntaxNode<'_>) -> FunctionSignature {
        let parameters = direct_child(node, NodeKind::ValueParameters)
            .map(|parameters| {
                parameters
                    .child_nodes()
                    .into_iter()
                    .filter(|child| child.kind() == NodeKind::ValueParameter)
                    .filter_map(|parameter| self.lower_value_parameter(&parameter))
                    .collect()
            })
            .unwrap_or_default();
        let contract = direct_child(node, NodeKind::ImplementationContract);
        let success = contract
            .as_ref()
            .and_then(|contract| direct_child(contract, NodeKind::Type))
            .map(|type_node| self.lower_type(&type_node));
        let requirements = contract
            .as_ref()
            .and_then(|contract| direct_child(contract, NodeKind::RequirementClause))
            .map(|requirements| self.lower_requirement_row(&requirements))
            .unwrap_or_default();
        FunctionSignature {
            parameters,
            success,
            requirements,
        }
    }

    fn lower_value_parameter(&mut self, node: &SyntaxNode<'_>) -> Option<ValueParameter> {
        Some(ValueParameter {
            semantic_id: self.allocate_semantic_id(),
            name: direct_identifier(node)?,
            type_use: direct_child(node, NodeKind::Type)
                .map(|type_node| self.lower_type(&type_node)),
        })
    }

    fn lower_requirement_row(&mut self, node: &SyntaxNode<'_>) -> RequirementRowUse {
        let mut terms = node
            .child_tokens()
            .into_iter()
            .filter(|token| token.kind() == TokenKind::Identifier && !token.is_missing())
            .map(|token| RequirementTermUse::Variable {
                semantic_id: self.allocate_semantic_id(),
                name: name_from(&token),
            })
            .collect::<Vec<_>>();
        for record in node
            .child_nodes()
            .into_iter()
            .filter(|child| child.kind() == NodeKind::RequirementRecord)
        {
            terms.extend(
                record
                    .child_nodes()
                    .into_iter()
                    .filter(|field| field.kind() == NodeKind::NamedTypeField)
                    .filter_map(|field| {
                        let name = direct_identifier(&field)?;
                        let interface = direct_child(&field, NodeKind::Type)?;
                        Some(RequirementTermUse::Field {
                            semantic_id: self.allocate_semantic_id(),
                            name,
                            interface: self.lower_type(&interface),
                        })
                    }),
            );
        }
        RequirementRowUse { terms }
    }

    fn lower_expression(&mut self, node: &SyntaxNode<'_>) -> ModuleExpression {
        let semantic_id = self.allocate_semantic_id();
        match node.kind() {
            NodeKind::NameReference => direct_identifier(node)
                .map_or(ModuleExpression::Empty { semantic_id }, |name| {
                    ModuleExpression::Name { semantic_id, name }
                }),
            NodeKind::PostfixExpression => self.lower_postfix(node, semantic_id),
            NodeKind::BlockExpression => ModuleExpression::Block {
                semantic_id,
                items: node
                    .child_nodes()
                    .into_iter()
                    .filter(|child| child.kind() == NodeKind::BlockItem)
                    .filter_map(|item| self.lower_block_item(&item))
                    .collect(),
            },
            NodeKind::FunctionExpression => {
                let signature = self.lower_function_signature(node);
                let body = direct_child(node, NodeKind::BlockExpression)
                    .map(|body| self.lower_expression(&body))
                    .unwrap_or(ModuleExpression::Empty { semantic_id });
                ModuleExpression::Function {
                    semantic_id,
                    signature,
                    body: Box::new(body),
                }
            }
            NodeKind::CaseExpression => self.lower_case(node, semantic_id),
            _ => ModuleExpression::Sequence {
                semantic_id,
                expressions: contained_expression_children(node)
                    .into_iter()
                    .map(|child| self.lower_expression(&child))
                    .collect(),
            },
        }
    }

    fn lower_postfix(
        &mut self,
        node: &SyntaxNode<'_>,
        semantic_id: SemanticId,
    ) -> ModuleExpression {
        let mut children = node.child_nodes().into_iter();
        let Some(base) = children.next() else {
            return ModuleExpression::Empty { semantic_id };
        };
        let mut expression = self.lower_expression(&base);
        let mut arguments = Vec::new();
        for suffix in children {
            match suffix.kind() {
                NodeKind::MemberAccess => {
                    if let Some(member) = direct_identifier(&suffix) {
                        expression = ModuleExpression::Select {
                            semantic_id: self.allocate_semantic_id(),
                            receiver: Box::new(expression),
                            member,
                            range: significant_range(node),
                        };
                    }
                }
                NodeKind::CallSuffix => {
                    arguments.extend(
                        suffix
                            .child_nodes()
                            .into_iter()
                            .filter(|argument| argument.kind() == NodeKind::Argument)
                            .filter_map(|argument| direct_expression_child(&argument))
                            .map(|argument| self.lower_expression(&argument)),
                    );
                }
                _ => {}
            }
        }
        if arguments.is_empty() {
            expression
        } else {
            let mut expressions = Vec::with_capacity(arguments.len() + 1);
            expressions.push(expression);
            expressions.extend(arguments);
            ModuleExpression::Sequence {
                semantic_id,
                expressions,
            }
        }
    }

    fn lower_block_item(&mut self, node: &SyntaxNode<'_>) -> Option<ModuleBlockItem> {
        let child = node.child_nodes().into_iter().next()?;
        if child.kind() == NodeKind::LetBinding {
            let name = direct_identifier(&child)?;
            let value = direct_expression_child(&child)
                .map(|value| self.lower_expression(&value))
                .unwrap_or(ModuleExpression::Empty {
                    semantic_id: self.allocate_semantic_id(),
                });
            Some(ModuleBlockItem::Binding { name, value })
        } else {
            Some(ModuleBlockItem::Expression(self.lower_expression(&child)))
        }
    }

    fn lower_case(&mut self, node: &SyntaxNode<'_>, semantic_id: SemanticId) -> ModuleExpression {
        let subjects = direct_expression_children(node)
            .into_iter()
            .map(|subject| self.lower_expression(&subject))
            .collect();
        let arms = node
            .child_nodes()
            .into_iter()
            .filter(|child| child.kind() == NodeKind::CaseArm)
            .map(|arm| {
                let patterns = arm
                    .child_nodes()
                    .into_iter()
                    .filter(|child| child.kind() == NodeKind::Pattern)
                    .map(|pattern| self.lower_pattern(&pattern))
                    .collect();
                let mut arm_expressions = direct_expression_children(&arm);
                let body = arm_expressions
                    .pop()
                    .map(|body| self.lower_expression(&body))
                    .unwrap_or(ModuleExpression::Empty {
                        semantic_id: self.allocate_semantic_id(),
                    });
                let guard = arm_expressions
                    .pop()
                    .map(|guard| self.lower_expression(&guard));
                ModuleCaseArm {
                    patterns,
                    guard,
                    body,
                }
            })
            .collect();
        ModuleExpression::Case {
            semantic_id,
            subjects,
            arms,
        }
    }

    fn lower_pattern(&mut self, node: &SyntaxNode<'_>) -> Pattern {
        let arguments = node
            .child_nodes()
            .into_iter()
            .filter(|child| child.kind() == NodeKind::Pattern)
            .map(|child| self.lower_pattern(&child))
            .collect();
        Pattern {
            semantic_id: self.allocate_semantic_id(),
            name: direct_identifier(node),
            applied: node
                .child_tokens()
                .iter()
                .any(|token| token.kind() == TokenKind::LeftParen),
            arguments,
        }
    }

    fn allocate_semantic_id(&mut self) -> SemanticId {
        let id = SemanticId::new(self.next_semantic_id);
        self.next_semantic_id += 1;
        id
    }
}

fn direct_child<'tree>(node: &SyntaxNode<'tree>, kind: NodeKind) -> Option<SyntaxNode<'tree>> {
    node.child_nodes()
        .into_iter()
        .find(|child| child.kind() == kind)
}

fn direct_identifier(node: &SyntaxNode<'_>) -> Option<Name> {
    node.child_tokens()
        .into_iter()
        .find(|token| token.kind() == TokenKind::Identifier && !token.is_missing())
        .map(|token| name_from(&token))
}

fn direct_token_after<'tree>(
    node: &SyntaxNode<'tree>,
    preceding: TokenKind,
    wanted: TokenKind,
) -> Option<SyntaxToken<'tree>> {
    let mut saw_preceding = false;
    node.child_tokens().into_iter().find(|token| {
        if token.kind() == preceding {
            saw_preceding = true;
            return false;
        }
        saw_preceding && token.kind() == wanted && !token.is_missing()
    })
}

fn significant_range(node: &SyntaxNode<'_>) -> TextRange {
    let tokens = significant_tokens(node);
    match (tokens.first(), tokens.last()) {
        (Some(first), Some(last)) => {
            TextRange::new(first.range().start() as usize, last.range().end() as usize)
        }
        _ => node.range(),
    }
}

fn is_expression_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::UsingExpression
            | NodeKind::PipelineExpression
            | NodeKind::ComparisonExpression
            | NodeKind::AdditiveExpression
            | NodeKind::MultiplicativeExpression
            | NodeKind::UnaryExpression
            | NodeKind::PostfixExpression
            | NodeKind::Literal
            | NodeKind::NameReference
            | NodeKind::GroupedExpression
            | NodeKind::ListExpression
            | NodeKind::RecordExpression
            | NodeKind::BlockExpression
            | NodeKind::CaseExpression
            | NodeKind::FunctionExpression
            | NodeKind::EmptyBraceExpression
            | NodeKind::ErrorIfExpression
            | NodeKind::Error
    )
}

fn direct_expression_child<'tree>(node: &SyntaxNode<'tree>) -> Option<SyntaxNode<'tree>> {
    node.child_nodes()
        .into_iter()
        .find(|child| is_expression_kind(child.kind()))
}

fn direct_expression_children<'tree>(node: &SyntaxNode<'tree>) -> Vec<SyntaxNode<'tree>> {
    node.child_nodes()
        .into_iter()
        .filter(|child| is_expression_kind(child.kind()))
        .collect()
}

fn contained_expression_children<'tree>(node: &SyntaxNode<'tree>) -> Vec<SyntaxNode<'tree>> {
    let mut expressions = Vec::new();
    for child in node.child_nodes() {
        if is_expression_kind(child.kind()) {
            expressions.push(child);
        } else {
            expressions.extend(contained_expression_children(&child));
        }
    }
    expressions
}
