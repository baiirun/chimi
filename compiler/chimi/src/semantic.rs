use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::{FileId, ModuleId, SemanticId, TextRange};
use crate::source::SourceSnapshot;
use crate::syntax::{
    self, DeclarationSyntax, FileSyntax, FunctionDeclarationSyntax, NodeKind, ParseResult,
    StructDeclarationSyntax, SyntaxNode, SyntaxToken, TextEscape, TokenKind, TypedSyntaxNode,
};
use std::collections::HashSet;
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
    pub(crate) value: Literal,
}

#[derive(Clone, Debug)]
pub(crate) enum Literal {
    Integer {
        semantic_id: SemanticId,
        value: i64,
        range: TextRange,
    },
    /// A finite `Float`; lowering rejects literals that round to an infinity with `S1003`.
    Float {
        semantic_id: SemanticId,
        value: f64,
        range: TextRange,
    },
    Text {
        semantic_id: SemanticId,
        value: String,
        range: TextRange,
    },
    Boolean {
        semantic_id: SemanticId,
        value: bool,
        range: TextRange,
    },
    /// `Void`, the only value of the type `Void`.
    Void {
        semantic_id: SemanticId,
        range: TextRange,
    },
}

impl Literal {
    pub(crate) const fn semantic_id(&self) -> SemanticId {
        match self {
            Self::Integer { semantic_id, .. }
            | Self::Float { semantic_id, .. }
            | Self::Text { semantic_id, .. }
            | Self::Boolean { semantic_id, .. }
            | Self::Void { semantic_id, .. } => *semantic_id,
        }
    }

    pub(crate) const fn range(&self) -> TextRange {
        match self {
            Self::Integer { range, .. }
            | Self::Float { range, .. }
            | Self::Text { range, .. }
            | Self::Boolean { range, .. }
            | Self::Void { range, .. } => *range,
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
        if tokens.len() != 11
            || !matches_kinds(
                &tokens[..8],
                &[
                    TokenKind::Pub,
                    TokenKind::Fn,
                    TokenKind::Identifier,
                    TokenKind::LeftParen,
                    TokenKind::RightParen,
                    TokenKind::LeftBrace,
                    TokenKind::Identifier,
                    TokenKind::LeftParen,
                ],
            )
            || !matches!(tokens[8].kind(), TokenKind::Integer | TokenKind::Text)
            || !matches_kinds(
                &tokens[9..],
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
        let body = self.lower_construction(&tokens[6..10], body_syntax.syntax())?;

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
        let [constructor, left_paren, value, right_paren] = tokens else {
            self.lowering_error(
                owner,
                "incomplete construction expression cannot be lowered",
            );
            return None;
        };
        if constructor.kind() != TokenKind::Identifier
            || left_paren.kind() != TokenKind::LeftParen
            || right_paren.kind() != TokenKind::RightParen
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
                value: text_literal_value(value.text()),
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

/// A node's own tokens, without trivia or tokens the parser reported as missing.
fn significant_child_tokens<'tree>(node: &SyntaxNode<'tree>) -> Vec<SyntaxToken<'tree>> {
    node.child_tokens()
        .into_iter()
        .filter(|token| {
            !token.is_missing()
                && !matches!(
                    token.kind(),
                    TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment
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
    pub(crate) syntax: Option<Arc<ParseResult>>,
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
    /// A module constant, `[pub] const name [: Type] = initializer`, evaluated at build time.
    Constant,
    /// `[pub] native type Name<...>`: an opaque type with no fields or
    /// constructors, bound to a runtime type of the operation registry.
    NativeType,
    /// `[pub] process name <identifier>: <Message>`: a module-level
    /// `Process.Address<Message>` with named routing. `annotation` holds the message
    /// type; there is no initializer.
    ProcessName,
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
    /// The declared type of a constant; `None` for every other declaration.
    pub(crate) annotation: Option<TypeUse>,
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
    /// `(T1, T2, ...)`; the parser reports fewer than two elements as `S1005`.
    Tuple {
        semantic_id: SemanticId,
        elements: Vec<TypeUse>,
    },
    /// An anonymous record type, fields in source order.
    Record {
        semantic_id: SemanticId,
        fields: Vec<TypedField>,
    },
    /// `A | B | ...`: a failure-set type, members in source order.
    Union {
        semantic_id: SemanticId,
        members: Vec<TypeUse>,
    },
}

impl TypeUse {
    pub(crate) const fn semantic_id(&self) -> SemanticId {
        match self {
            Self::Named { semantic_id, .. }
            | Self::Function { semantic_id, .. }
            | Self::Tuple { semantic_id, .. }
            | Self::Record { semantic_id, .. }
            | Self::Union { semantic_id, .. } => *semantic_id,
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
    /// The declared failure row; only a `native fn` header writes one.
    pub(crate) failures: Option<TypeUse>,
    pub(crate) requirements: RequirementRowUse,
    /// `native fn`: no body; the operation registry supplies the implementation.
    pub(crate) native: bool,
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

/// One pattern form per variant, so that impossible shapes such as a literal with
/// payload patterns cannot be represented. Every variant carries the semantic id that
/// resolution and typing key their facts on.
#[derive(Clone, Debug)]
pub(crate) enum Pattern {
    /// `_`, or a pattern the parser already reported as missing.
    Wildcard {
        semantic_id: SemanticId,
        range: TextRange,
    },
    /// A lowercase name, which always introduces a binding.
    Binding {
        semantic_id: SemanticId,
        name: Name,
        range: TextRange,
    },
    Literal {
        semantic_id: SemanticId,
        literal: PatternLiteral,
        range: TextRange,
    },
    /// `None`, `Circle(radius)`, or `Value.Text(schema)`: a constructor path and its
    /// payload patterns by position.
    Constructor {
        semantic_id: SemanticId,
        path: Vec<Name>,
        arguments: Vec<Pattern>,
        range: TextRange,
    },
    /// `name: Type` or `_: Type`. Over an ordinary value the type is an annotation;
    /// over a failure-set value it selects the members of that type.
    Typed {
        semantic_id: SemanticId,
        /// `None` for the typed wildcard `_: Type`.
        name: Option<Name>,
        type_use: TypeUse,
        range: TextRange,
    },
    /// `pattern as name`: matches `pattern` and also binds the whole value to `name`.
    Alias {
        semantic_id: SemanticId,
        pattern: Box<Pattern>,
        name: Name,
        range: TextRange,
    },
    /// `(p1, ..., pn)`: a tuple of arity `n`, then its elements by position.
    Tuple {
        semantic_id: SemanticId,
        elements: Vec<Pattern>,
        range: TextRange,
    },
    /// `[p1, ..., pk, ..rest, q1, ..., qm]`. `rest` is `None` for an exact-length
    /// pattern; otherwise it is the `Binding` or `Wildcard` that receives the
    /// remaining elements, and `suffix` holds the patterns after it.
    List {
        semantic_id: SemanticId,
        prefix: Vec<Pattern>,
        rest: Option<Box<Pattern>>,
        suffix: Vec<Pattern>,
        /// A further `..` was reported as `L2021` and dropped, so this pattern does
        /// not mean what was written; coverage leaves it out.
        rejected_rest: bool,
        range: TextRange,
    },
}

impl Pattern {
    pub(crate) fn semantic_id(&self) -> SemanticId {
        match self {
            Self::Wildcard { semantic_id, .. }
            | Self::Binding { semantic_id, .. }
            | Self::Literal { semantic_id, .. }
            | Self::Constructor { semantic_id, .. }
            | Self::Typed { semantic_id, .. }
            | Self::Alias { semantic_id, .. }
            | Self::Tuple { semantic_id, .. }
            | Self::List { semantic_id, .. } => *semantic_id,
        }
    }

    pub(crate) fn range(&self) -> TextRange {
        match self {
            Self::Wildcard { range, .. }
            | Self::Binding { range, .. }
            | Self::Literal { range, .. }
            | Self::Constructor { range, .. }
            | Self::Typed { range, .. }
            | Self::Alias { range, .. }
            | Self::Tuple { range, .. }
            | Self::List { range, .. } => *range,
        }
    }

    /// The range a diagnostic about this pattern's head points at: a constructor's
    /// name, or the whole pattern for other forms.
    pub(crate) fn head_range(&self) -> TextRange {
        match self {
            Self::Constructor { path, range, .. } => path.last().map_or(*range, |name| name.range),
            other => other.range(),
        }
    }

    /// Payload patterns of a constructor pattern; empty for every other form.
    pub(crate) fn constructor_arguments(&self) -> &[Pattern] {
        match self {
            Self::Constructor { arguments, .. } => arguments,
            _ => &[],
        }
    }

    /// Direct sub-patterns in source order, for passes that visit every binding.
    /// Whether this pattern or one nested in it lost a `..` to `L2021`.
    pub(crate) fn has_rejected_rest(&self) -> bool {
        matches!(
            self,
            Self::List {
                rejected_rest: true,
                ..
            }
        ) || self.children().into_iter().any(Self::has_rejected_rest)
    }

    pub(crate) fn children(&self) -> Vec<&Pattern> {
        match self {
            Self::Wildcard { .. }
            | Self::Binding { .. }
            | Self::Literal { .. }
            | Self::Typed { .. } => Vec::new(),
            Self::Constructor { arguments, .. } => arguments.iter().collect(),
            Self::Tuple { elements, .. } => elements.iter().collect(),
            Self::Alias { pattern, .. } => vec![pattern],
            Self::List {
                prefix,
                rest,
                suffix,
                ..
            } => prefix.iter().chain(rest.as_deref()).chain(suffix).collect(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum PatternLiteral {
    Integer(i64),
    Text(String),
    Boolean(bool),
    /// `Void`, which matches the only value of its type.
    Void,
    /// `Float` has no literal patterns; typing rejects this with `T4313`.
    Float,
}

#[derive(Clone, Debug)]
pub(crate) enum ModuleExpression {
    Literal(Literal),
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
    Call {
        semantic_id: SemanticId,
        callee: Box<Self>,
        arguments: Vec<CallArgument>,
        range: TextRange,
    },
    List {
        semantic_id: SemanticId,
        elements: Vec<Self>,
        range: TextRange,
    },
    /// `(e1, e2, ...)`, evaluated left to right.
    Tuple {
        semantic_id: SemanticId,
        elements: Vec<Self>,
        range: TextRange,
    },
    Record {
        semantic_id: SemanticId,
        fields: Vec<RecordValueField>,
        range: TextRange,
    },
    /// `{ ..base, field: value }`: a copy of `base` with the named fields replaced.
    /// The base evaluates first, then the field values in source order.
    Update {
        semantic_id: SemanticId,
        base: Box<Self>,
        fields: Vec<RecordValueField>,
        range: TextRange,
    },
    Unary {
        semantic_id: SemanticId,
        operator: UnaryOperator,
        operand: Box<Self>,
        range: TextRange,
    },
    Binary {
        semantic_id: SemanticId,
        operator: BinaryOperator,
        left: Box<Self>,
        right: Box<Self>,
        range: TextRange,
    },
    Group {
        semantic_id: SemanticId,
        expression: Box<Self>,
        range: TextRange,
    },
    Using {
        semantic_id: SemanticId,
        expression: Box<Self>,
        providers: Vec<RecordValueField>,
        range: TextRange,
    },
    Sequence {
        semantic_id: SemanticId,
        expressions: Vec<Self>,
        range: TextRange,
    },
    Block {
        semantic_id: SemanticId,
        items: Vec<ModuleBlockItem>,
        range: TextRange,
    },
    Function {
        semantic_id: SemanticId,
        signature: FunctionSignature,
        body: Box<Self>,
        range: TextRange,
    },
    Case {
        semantic_id: SemanticId,
        subjects: Vec<Self>,
        arms: Vec<ModuleCaseArm>,
        range: TextRange,
        origin: CaseOrigin,
    },
    /// `loop state = initial { body }`. The body's value is the next pass's state.
    /// The state is a local binding defined at `state_id`, which also identifies the loop:
    /// resolution links each `Break` to it. It differs from `semantic_id` because a `let`
    /// whose value is this loop defines its own binding at `semantic_id`.
    Loop {
        semantic_id: SemanticId,
        state_id: SemanticId,
        state: Name,
        initial: Box<Self>,
        body: Box<Self>,
        range: TextRange,
    },
    /// `break` or `break value`. `None` means `Void`.
    Break {
        semantic_id: SemanticId,
        value: Option<Box<Self>>,
        range: TextRange,
    },
    Empty {
        semantic_id: SemanticId,
        range: TextRange,
    },
    /// `"cost: ${amount}"`: text chunks and interpolated expressions in source order.
    /// Always holds at least one expression part; chunks hold decoded text.
    Interpolation {
        semantic_id: SemanticId,
        parts: Vec<InterpolationPart>,
        range: TextRange,
    },
}

#[derive(Clone, Debug)]
pub(crate) enum InterpolationPart {
    Text(String),
    Expression(ModuleExpression),
}

/// The interpolated expressions of `parts`, left to right, which is their evaluation order.
pub(crate) fn interpolated_expressions(
    parts: &[InterpolationPart],
) -> impl Iterator<Item = &ModuleExpression> {
    parts.iter().filter_map(|part| match part {
        InterpolationPart::Expression(expression) => Some(expression),
        InterpolationPart::Text(_) => None,
    })
}

impl ModuleExpression {
    pub(crate) const fn semantic_id(&self) -> SemanticId {
        match self {
            Self::Literal(literal) => literal.semantic_id(),
            Self::Name { semantic_id, .. }
            | Self::Select { semantic_id, .. }
            | Self::Call { semantic_id, .. }
            | Self::List { semantic_id, .. }
            | Self::Tuple { semantic_id, .. }
            | Self::Record { semantic_id, .. }
            | Self::Update { semantic_id, .. }
            | Self::Unary { semantic_id, .. }
            | Self::Binary { semantic_id, .. }
            | Self::Group { semantic_id, .. }
            | Self::Using { semantic_id, .. }
            | Self::Sequence { semantic_id, .. }
            | Self::Block { semantic_id, .. }
            | Self::Function { semantic_id, .. }
            | Self::Case { semantic_id, .. }
            | Self::Loop { semantic_id, .. }
            | Self::Break { semantic_id, .. }
            | Self::Empty { semantic_id, .. }
            | Self::Interpolation { semantic_id, .. } => *semantic_id,
        }
    }

    pub(crate) fn range(&self) -> TextRange {
        match self {
            Self::Literal(literal) => literal.range(),
            Self::Name { name, .. } => name.range,
            Self::Select { range, .. }
            | Self::Call { range, .. }
            | Self::List { range, .. }
            | Self::Tuple { range, .. }
            | Self::Record { range, .. }
            | Self::Update { range, .. }
            | Self::Unary { range, .. }
            | Self::Binary { range, .. }
            | Self::Group { range, .. }
            | Self::Using { range, .. }
            | Self::Sequence { range, .. }
            | Self::Block { range, .. }
            | Self::Function { range, .. }
            | Self::Case { range, .. }
            | Self::Loop { range, .. }
            | Self::Break { range, .. }
            | Self::Empty { range, .. }
            | Self::Interpolation { range, .. } => *range,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CallArgument {
    pub(crate) label: Option<Name>,
    pub(crate) value: ModuleExpression,
}

#[derive(Clone, Debug)]
pub(crate) struct RecordValueField {
    pub(crate) name: Name,
    pub(crate) value: ModuleExpression,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UnaryOperator {
    Negate,
    /// Boolean negation. Core lowering turns it into a `Match`, so it never reaches Core.
    Not,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    /// Structural equality over an equatable type.
    Equal,
    /// `a != b`; Core lowers it to `not (a == b)`, so it never reaches Core.
    NotEqual,
    /// Short-circuit conjunction. Core lowering turns it into a `Match`, so it never reaches Core.
    And,
    /// Short-circuit disjunction. Core lowering turns it into a `Match`, so it never reaches Core.
    Or,
    /// `start..end`, the half-open `Range`.
    Range,
    /// `start..=end`, which means `start..(end + 1)` and defects when `end` is the
    /// largest `Int`.
    RangeInclusive,
}

#[derive(Clone, Debug)]
pub(crate) enum ModuleBlockItem {
    /// `let name = value` or `let name: Type = value`. Every other `let` pattern
    /// lowers to a `case` whose one arm holds the rest of the block.
    Binding {
        name: Name,
        type_use: Option<TypeUse>,
        value: ModuleExpression,
    },
    Expression(ModuleExpression),
}

/// Where a `case` came from. Clause groups lower to a `case` over compiler-generated
/// parameters; diagnostics use the origin to talk about clauses instead of arms.
#[derive(Clone, Debug)]
pub(crate) enum CaseOrigin {
    Source,
    /// A destructuring `let` or `loop` state: one arm whose pattern must be
    /// irrefutable.
    Binding {
        kind: BindingKind,
    },
    /// The body of the function `function`, one arm per clause in source order. Each
    /// arm's range is its clause head, `name(patterns)`.
    Clauses {
        function: Name,
        /// Another group of the same name appears later or earlier
        /// (`E_SEPARATED_FUNCTION_CLAUSES`), so this group alone is not checked
        /// for exhaustiveness.
        separated: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BindingKind {
    Let,
    LoopState,
}

impl BindingKind {
    pub(crate) const fn keyword(self) -> &'static str {
        match self {
            Self::Let => "let",
            Self::LoopState => "loop",
        }
    }
}

/// The prefix of compiler-generated names, such as clause-group parameters. Source
/// identifiers cannot contain it, so a generated name never collides with one.
pub(crate) const GENERATED_NAME_PREFIX: char = '#';

pub(crate) fn is_generated_name(text: &str) -> bool {
    text.starts_with(GENERATED_NAME_PREFIX)
}

#[derive(Clone, Debug)]
pub(crate) struct ModuleCaseArm {
    pub(crate) patterns: Vec<Pattern>,
    pub(crate) guard: Option<ModuleExpression>,
    pub(crate) body: ModuleExpression,
    pub(crate) range: TextRange,
}

#[derive(Debug)]
pub(crate) struct SemanticModule {
    pub(crate) id: ModuleId,
    pub(crate) path: String,
    pub(crate) source: Arc<SourceSnapshot>,
    pub(crate) syntax: Arc<ParseResult>,
    pub(crate) imports: Vec<ModuleImport>,
    pub(crate) declarations: Vec<ModuleDeclaration>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

#[derive(Debug)]
pub(crate) struct SemanticProject {
    pub(crate) modules: Vec<Arc<SemanticModule>>,
}

pub(crate) fn lower_project(mut inputs: Vec<ModuleSource>) -> SemanticProject {
    inputs.sort_by(|left, right| left.path.cmp(&right.path));
    let modules = inputs
        .into_iter()
        .enumerate()
        .map(|(index, input)| {
            let syntax = input
                .syntax
                .unwrap_or_else(|| Arc::new(syntax::parse(&input.source)));
            lower_module(
                ModuleId::new(index as u32),
                input.path,
                input.source,
                syntax,
            )
        })
        .collect();
    SemanticProject { modules }
}

pub(crate) fn lower_module(
    id: ModuleId,
    path: String,
    mut source: SourceSnapshot,
    syntax: Arc<ParseResult>,
) -> Arc<SemanticModule> {
    source.assign_file_id(FileId::new(id.raw()));
    Arc::new(ProjectLowerer::lower(id, path, source, syntax))
}

pub(crate) fn discover_imports(parsed: &ParseResult) -> Vec<ModuleImport> {
    let mut lowerer = ProjectLowerer {
        next_semantic_id: 0,
        diagnostics: Vec::new(),
    };
    parsed
        .root()
        .child_nodes()
        .into_iter()
        .filter(|node| node.kind() == NodeKind::ImportDeclaration)
        .map(|node| lowerer.lower_import(&node))
        .collect()
}

struct ProjectLowerer {
    next_semantic_id: u32,
    /// Literal diagnostics (`S1002`, `S1003`), with a placeholder file id until the module is done.
    diagnostics: Vec<Diagnostic>,
}

impl ProjectLowerer {
    fn lower(
        id: ModuleId,
        path: String,
        source: SourceSnapshot,
        parsed: Arc<ParseResult>,
    ) -> SemanticModule {
        let source = Arc::new(source);
        let root = parsed.root();
        let mut lowerer = Self {
            next_semantic_id: 0,
            diagnostics: Vec::new(),
        };
        let mut imports = Vec::new();
        let mut declarations = Vec::new();
        let nodes = root.child_nodes();
        let mut clause_groups = Vec::<(String, TextRange)>::new();
        let separated = separated_function_names(&nodes);
        let mut index = 0;
        while index < nodes.len() {
            let node = &nodes[index];
            index += 1;
            if node.kind() == NodeKind::FunctionDeclaration {
                let name = function_name(node);
                let mut group = vec![node.clone()];
                while let Some(next) = nodes.get(index)
                    && next.kind() == NodeKind::FunctionDeclaration
                    && name.is_some()
                    && function_name(next).map(|next| next.text)
                        == name.as_ref().map(|name| name.text.clone())
                {
                    group.push(next.clone());
                    index += 1;
                }
                if let Some(name) = &name
                    && let Some((_, first)) = clause_groups
                        .iter()
                        .find(|(existing, _)| *existing == name.text)
                {
                    lowerer.diagnostics.push(
                        Diagnostic::error(
                            DiagnosticStage::Name,
                            "E_SEPARATED_FUNCTION_CLAUSES",
                            FileId::new(0),
                            name.range,
                            format!(
                                "clauses of `{}` must be adjacent; move this clause next to the others",
                                name.text
                            ),
                        )
                        .with_label(FileId::new(0), *first, "the first clause group is here"),
                    );
                    continue;
                }
                if let Some(name) = &name {
                    clause_groups.push((name.text.clone(), name.range));
                }
                let is_separated = name
                    .as_ref()
                    .is_some_and(|name| separated.contains(&name.text));
                if let Some(declaration) = lowerer.lower_function_group(&group, is_separated) {
                    declarations.push(declaration);
                }
                continue;
            }
            let node = node.clone();
            match node.kind() {
                NodeKind::ImportDeclaration => imports.push(lowerer.lower_import(&node)),
                NodeKind::StructDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::InterfaceDeclaration
                | NodeKind::NativeTypeDeclaration => {
                    if let Some(declaration) = lowerer.lower_module_declaration(&node) {
                        declarations.push(declaration);
                    }
                }
                NodeKind::ConstDeclaration => {
                    if let Some(declaration) = lowerer.lower_constant(&node) {
                        declarations.push(declaration);
                    }
                }
                NodeKind::ProcessNameDeclaration => {
                    if let Some(declaration) = lowerer.lower_process_name(&node) {
                        declarations.push(declaration);
                    }
                }
                // Module-level `let` was replaced by `const` (`L2101`). It still lowers as
                // a constant so references to it resolve and report nothing further.
                NodeKind::LetBinding => {
                    if let Some(keyword) = node
                        .child_tokens()
                        .into_iter()
                        .find(|token| token.kind() == TokenKind::Let)
                    {
                        lowerer.diagnostics.push(
                            Diagnostic::error(
                                DiagnosticStage::Lowering,
                                "L2101",
                                FileId::new(0),
                                keyword.range(),
                                "a module-level value is a constant; declare it with `const`",
                            )
                            .with_fix(keyword.range(), "const"),
                        );
                    }
                    if let Some(declaration) = lowerer.lower_constant(&node) {
                        declarations.push(declaration);
                    }
                }
                _ => {}
            }
        }
        let diagnostics = parsed
            .diagnostics()
            .iter()
            .cloned()
            .chain(std::mem::take(&mut lowerer.diagnostics))
            .map(|diagnostic| diagnostic.with_file_id(source.file_id()))
            .collect();
        SemanticModule {
            id,
            path,
            source,
            syntax: parsed,
            imports,
            declarations,
            diagnostics,
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
            NodeKind::NativeTypeDeclaration => {
                (ModuleDeclarationKind::NativeType, TokenKind::Identifier)
            }
            _ => return None,
        };
        let name = if kind == ModuleDeclarationKind::NativeType {
            // `native` and `type` are identifiers; the name is the third.
            node.child_tokens()
                .into_iter()
                .filter(|token| token.kind() == TokenKind::Identifier && !token.is_missing())
                .nth(2)
        } else {
            direct_token_after(node, keyword, TokenKind::Identifier)
        }
        .map(|token| name_from(&token))?;
        let generic_parameters = self.lower_generic_parameters(node);
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
        let native = function.as_ref().is_some_and(|function| function.native);
        let body = if kind == ModuleDeclarationKind::Function && !native {
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
            annotation: None,
        })
    }

    /// Lowers adjacent same-named functions as one declaration. A group that is one
    /// plain function lowers as before. Otherwise the parameters become generated
    /// names and the body a `case` over them with one arm per clause, in source order.
    /// Header items come from the first clause; a later clause that repeats one is
    /// `L2020`, and a clause with a different parameter count is `T4280` and is left out.
    fn lower_function_group(
        &mut self,
        clauses: &[SyntaxNode<'_>],
        separated: bool,
    ) -> Option<ModuleDeclaration> {
        let first = clauses.first()?;
        if clauses.len() == 1 && !clause_has_patterns(first) {
            return self.lower_module_declaration(first);
        }
        let name = function_name(first)?;
        let generic_parameters = self.lower_generic_parameters(first);
        let mut signature = self.lower_function_signature(first);
        let arity = clause_parameters(first).len();
        let (parameters, subjects) = self.generated_parameters(arity, name.range);
        let mut arms = Vec::new();
        for (index, clause) in clauses.iter().enumerate() {
            if index > 0 {
                self.reject_repeated_header(clause);
            }
            let parameter_nodes = clause_parameters(clause);
            let head = clause_head_range(clause);
            if parameter_nodes.len() != arity {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticStage::Type,
                    "T4280",
                    FileId::new(0),
                    head,
                    format!(
                        "clauses of `{}` take {arity} parameters, but this clause takes {}; Chimi has no arity overloading",
                        name.text,
                        parameter_nodes.len()
                    ),
                ));
                continue;
            }
            arms.push(self.lower_clause_arm(clause, &parameter_nodes, head));
        }
        let last = clauses.last().map_or(name.range, significant_range);
        signature.parameters = parameters;
        let body = ModuleExpression::Case {
            semantic_id: self.allocate_semantic_id(),
            subjects,
            arms,
            range: TextRange::new(name.range.start() as usize, last.end() as usize),
            origin: CaseOrigin::Clauses {
                function: name.clone(),
                separated,
            },
        };
        Some(ModuleDeclaration {
            semantic_id: self.allocate_semantic_id(),
            kind: ModuleDeclarationKind::Function,
            public: first
                .child_tokens()
                .iter()
                .any(|token| token.kind() == TokenKind::Pub),
            name,
            generic_parameters,
            fields: Vec::new(),
            variants: Vec::new(),
            function: Some(signature),
            body: Some(body),
            annotation: None,
        })
    }

    /// `arity` generated parameters and the name expressions a clause `case` matches.
    /// They take the range `at`, because every executable expression needs a source
    /// range; inspection skips generated names.
    fn generated_parameters(
        &mut self,
        arity: usize,
        at: TextRange,
    ) -> (Vec<ValueParameter>, Vec<ModuleExpression>) {
        let parameters = (0..arity)
            .map(|index| ValueParameter {
                semantic_id: self.allocate_semantic_id(),
                name: Name {
                    text: format!("{GENERATED_NAME_PREFIX}{index}"),
                    range: at,
                },
                type_use: None,
            })
            .collect::<Vec<_>>();
        let subjects = parameters
            .iter()
            .map(|parameter| ModuleExpression::Name {
                semantic_id: self.allocate_semantic_id(),
                name: parameter.name.clone(),
            })
            .collect();
        (parameters, subjects)
    }

    fn lower_clause_arm(
        &mut self,
        clause: &SyntaxNode<'_>,
        parameters: &[SyntaxNode<'_>],
        head: TextRange,
    ) -> ModuleCaseArm {
        let patterns = parameters
            .iter()
            .map(|parameter| self.lower_parameter_pattern(parameter))
            .collect();
        let guard = direct_child(clause, NodeKind::ClauseGuard)
            .and_then(|guard| direct_expression_child(&guard))
            .map(|guard| self.lower_expression(&guard));
        let body = match direct_child(clause, NodeKind::BlockExpression) {
            Some(body) => self.lower_expression(&body),
            None => ModuleExpression::Empty {
                semantic_id: self.allocate_semantic_id(),
                range: head,
            },
        };
        ModuleCaseArm {
            patterns,
            guard,
            body,
            range: head,
        }
    }

    /// Reports each header item a later clause repeats (`L2020`), with a fix that
    /// removes it.
    fn reject_repeated_header(&mut self, clause: &SyntaxNode<'_>) {
        let mut repeated = Vec::new();
        if let Some(token) = clause
            .child_tokens()
            .into_iter()
            .find(|token| token.kind() == TokenKind::Pub)
        {
            repeated.push(("`pub`", token.range()));
        }
        if let Some(generics) = direct_child(clause, NodeKind::GenericParameters) {
            repeated.push(("generic parameters", significant_range(&generics)));
        }
        if let Some(contract) = direct_child(clause, NodeKind::ImplementationContract) {
            let arrow = contract
                .child_tokens()
                .into_iter()
                .find(|token| token.kind() == TokenKind::Arrow);
            if let (Some(arrow), Some(success)) = (arrow, direct_child(&contract, NodeKind::Type)) {
                repeated.push((
                    "the success annotation",
                    TextRange::new(
                        arrow.range().start() as usize,
                        significant_range(&success).end() as usize,
                    ),
                ));
            }
            if let Some(needs) = direct_child(&contract, NodeKind::RequirementClause) {
                repeated.push(("`needs`", significant_range(&needs)));
            }
        }
        for (item, range) in repeated {
            self.diagnostics.push(
                Diagnostic::error(
                    DiagnosticStage::Lowering,
                    "L2020",
                    FileId::new(0),
                    range,
                    format!(
                        "only the first clause states {item}; remove it here, or move it to the first clause"
                    ),
                )
                .with_fix(range, ""),
            );
        }
    }

    /// The pattern a clause parameter matches: its `Pattern` child, or the plain
    /// `name` or `name: Type` form as a binding or typed binding.
    fn lower_parameter_pattern(&mut self, parameter: &SyntaxNode<'_>) -> Pattern {
        if let Some(pattern) = direct_child(parameter, NodeKind::Pattern) {
            return self.lower_pattern(&pattern);
        }
        let semantic_id = self.allocate_semantic_id();
        let range = significant_range(parameter);
        let name = direct_identifier(parameter).filter(|name| name.text != "_");
        if let Some(type_node) = direct_child(parameter, NodeKind::Type) {
            return Pattern::Typed {
                semantic_id,
                name,
                type_use: self.lower_type(&type_node),
                range,
            };
        }
        match name {
            Some(name) => Pattern::Binding {
                semantic_id,
                name,
                range,
            },
            None => Pattern::Wildcard { semantic_id, range },
        }
    }

    fn lower_generic_parameters(&mut self, node: &SyntaxNode<'_>) -> Vec<Name> {
        direct_child(node, NodeKind::GenericParameters)
            .map(|generics| {
                generics
                    .child_nodes()
                    .into_iter()
                    .filter(|parameter| parameter.kind() == NodeKind::GenericParameter)
                    .filter_map(|parameter| direct_identifier(&parameter))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Lowers a `ConstDeclaration`, or a module-level `LetBinding` already reported as `L2101`.
    fn lower_constant(&mut self, node: &SyntaxNode<'_>) -> Option<ModuleDeclaration> {
        let name = direct_identifier(node)?;
        let annotation =
            direct_child(node, NodeKind::Type).map(|type_node| self.lower_type(&type_node));
        let body = direct_expression_child(node).map(|value| self.lower_expression(&value));
        Some(ModuleDeclaration {
            semantic_id: self.allocate_semantic_id(),
            kind: ModuleDeclarationKind::Constant,
            public: node
                .child_tokens()
                .iter()
                .any(|token| token.kind() == TokenKind::Pub),
            name,
            generic_parameters: Vec::new(),
            fields: Vec::new(),
            variants: Vec::new(),
            function: None,
            body,
            annotation,
        })
    }

    /// Lowers a `ProcessNameDeclaration`; the name is the identifier after the
    /// contextual words `process` and `name`.
    fn lower_process_name(&mut self, node: &SyntaxNode<'_>) -> Option<ModuleDeclaration> {
        let name = node
            .child_tokens()
            .into_iter()
            .filter(|token| token.kind() == TokenKind::Identifier && !token.is_missing())
            .nth(2)
            .map(|token| name_from(&token))?;
        let annotation =
            direct_child(node, NodeKind::Type).map(|type_node| self.lower_type(&type_node));
        Some(ModuleDeclaration {
            semantic_id: self.allocate_semantic_id(),
            kind: ModuleDeclarationKind::ProcessName,
            public: node
                .child_tokens()
                .iter()
                .any(|token| token.kind() == TokenKind::Pub),
            name,
            generic_parameters: Vec::new(),
            fields: Vec::new(),
            variants: Vec::new(),
            function: None,
            body: None,
            annotation,
        })
    }

    fn lower_type(&mut self, node: &SyntaxNode<'_>) -> TypeUse {
        let semantic_id = self.allocate_semantic_id();
        if let Some(union) = direct_child(node, NodeKind::UnionType) {
            return TypeUse::Union {
                semantic_id,
                members: union
                    .child_nodes()
                    .into_iter()
                    .filter(|child| child.kind() == NodeKind::Type)
                    .map(|child| self.lower_type(&child))
                    .collect(),
            };
        }
        if let Some(tuple) = direct_child(node, NodeKind::TupleType) {
            return TypeUse::Tuple {
                semantic_id,
                elements: tuple
                    .child_nodes()
                    .into_iter()
                    .filter(|child| child.kind() == NodeKind::Type)
                    .map(|child| self.lower_type(&child))
                    .collect(),
            };
        }
        if let Some(record) = direct_child(node, NodeKind::RecordType) {
            let fields = record
                .child_nodes()
                .into_iter()
                .filter(|child| child.kind() == NodeKind::NamedTypeField)
                .filter_map(|field| self.lower_typed_field(&field))
                .collect();
            return TypeUse::Record {
                semantic_id,
                fields,
            };
        }
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
        // `native` before `fn`; after it, `native` is the function's name.
        let native = node
            .child_tokens()
            .iter()
            .take_while(|token| token.kind() != TokenKind::Fn)
            .any(|token| token.kind() == TokenKind::Identifier && token.text() == "native");
        let has_success = contract.as_ref().is_some_and(|contract| {
            contract
                .child_tokens()
                .iter()
                .any(|token| token.kind() == TokenKind::Arrow)
        });
        let has_failures = contract.as_ref().is_some_and(|contract| {
            contract
                .child_tokens()
                .iter()
                .any(|token| token.kind() == TokenKind::Errors)
        });
        let mut types = contract
            .as_ref()
            .map(|contract| {
                contract
                    .child_nodes()
                    .into_iter()
                    .filter(|child| child.kind() == NodeKind::Type)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
            .into_iter();
        let success = has_success
            .then(|| types.next())
            .flatten()
            .map(|type_node| self.lower_type(&type_node));
        let failures = has_failures
            .then(|| types.next())
            .flatten()
            .map(|type_node| self.lower_type(&type_node));
        let requirements = contract
            .as_ref()
            .and_then(|contract| direct_child(contract, NodeKind::RequirementClause))
            .map(|requirements| self.lower_requirement_row(&requirements))
            .unwrap_or_default();
        FunctionSignature {
            parameters,
            success,
            failures,
            requirements,
            native,
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
            NodeKind::Literal => self.lower_module_literal(node, semantic_id),
            NodeKind::InterpolatedText => self.lower_interpolation(node, semantic_id),
            NodeKind::NameReference => direct_identifier(node).map_or(
                ModuleExpression::Empty {
                    semantic_id,
                    range: significant_range(node),
                },
                |name| ModuleExpression::Name { semantic_id, name },
            ),
            NodeKind::PostfixExpression => self.lower_postfix(node, semantic_id),
            NodeKind::ListExpression => ModuleExpression::List {
                semantic_id,
                elements: direct_expression_children(node)
                    .into_iter()
                    .map(|element| self.lower_expression(&element))
                    .collect(),
                range: significant_range(node),
            },
            NodeKind::TupleExpression => ModuleExpression::Tuple {
                semantic_id,
                elements: direct_expression_children(node)
                    .into_iter()
                    .map(|element| self.lower_expression(&element))
                    .collect(),
                range: significant_range(node),
            },
            NodeKind::RecordExpression if direct_child(node, NodeKind::RecordSpread).is_some() => {
                self.lower_update(node, semantic_id)
            }
            NodeKind::RecordExpression => ModuleExpression::Record {
                semantic_id,
                fields: node
                    .child_nodes()
                    .into_iter()
                    .filter(|child| child.kind() == NodeKind::RecordField)
                    .filter_map(|field| {
                        Some(RecordValueField {
                            name: direct_identifier(&field)?,
                            value: self.lower_expression(&direct_expression_child(&field)?),
                        })
                    })
                    .collect(),
                range: significant_range(node),
            },
            NodeKind::GroupedExpression => direct_expression_child(node).map_or(
                ModuleExpression::Empty {
                    semantic_id,
                    range: significant_range(node),
                },
                |expression| ModuleExpression::Group {
                    semantic_id,
                    expression: Box::new(self.lower_expression(&expression)),
                    range: significant_range(node),
                },
            ),
            NodeKind::UnaryExpression | NodeKind::NotExpression => direct_expression_child(node)
                .map_or(
                    ModuleExpression::Empty {
                        semantic_id,
                        range: significant_range(node),
                    },
                    |operand| {
                        if node.kind() == NodeKind::UnaryExpression
                            && operand.kind() == NodeKind::Literal
                            && operand
                                .child_tokens()
                                .iter()
                                .any(|token| token.kind() == TokenKind::Integer)
                        {
                            return self.lower_literal(
                                &operand,
                                semantic_id,
                                Some(significant_range(node)),
                            );
                        }
                        ModuleExpression::Unary {
                            semantic_id,
                            operator: if node.kind() == NodeKind::NotExpression {
                                UnaryOperator::Not
                            } else {
                                UnaryOperator::Negate
                            },
                            operand: Box::new(self.lower_expression(&operand)),
                            range: significant_range(node),
                        }
                    },
                ),
            NodeKind::AdditiveExpression
            | NodeKind::MultiplicativeExpression
            | NodeKind::ComparisonExpression
            | NodeKind::PipelineExpression
            | NodeKind::AndExpression
            | NodeKind::OrExpression
            | NodeKind::RangeExpression => self.lower_binary(node, semantic_id),
            NodeKind::UsingExpression => self.lower_using(node, semantic_id),
            NodeKind::BlockExpression => {
                let items = node
                    .child_nodes()
                    .into_iter()
                    .filter(|child| child.kind() == NodeKind::BlockItem)
                    .collect::<Vec<_>>();
                self.reject_trailing_let(&items);
                ModuleExpression::Block {
                    semantic_id,
                    items: self.lower_block_items(&items, significant_range(node)),
                    range: significant_range(node),
                }
            }
            NodeKind::FunctionExpression if clause_has_patterns(node) => {
                let mut signature = self.lower_function_signature(node);
                let parameter_nodes = clause_parameters(node);
                let range = significant_range(node);
                let head = direct_child(node, NodeKind::ValueParameters)
                    .map_or(range, |parameters| significant_range(&parameters));
                let (parameters, subjects) = self.generated_parameters(parameter_nodes.len(), head);
                let arm = self.lower_clause_arm(node, &parameter_nodes, head);
                signature.parameters = parameters;
                // A literal has no name; its coverage diagnostics point at the patterns.
                let patterns = match (parameter_nodes.first(), parameter_nodes.last()) {
                    (Some(first), Some(last)) => TextRange::new(
                        significant_range(first).start() as usize,
                        significant_range(last).end() as usize,
                    ),
                    _ => head,
                };
                let function = Name {
                    text: "fn".to_owned(),
                    range: patterns,
                };
                ModuleExpression::Function {
                    semantic_id,
                    signature,
                    body: Box::new(ModuleExpression::Case {
                        semantic_id: self.allocate_semantic_id(),
                        subjects,
                        arms: vec![arm],
                        range,
                        origin: CaseOrigin::Clauses {
                            function,
                            separated: false,
                        },
                    }),
                    range,
                }
            }
            NodeKind::FunctionExpression => {
                let signature = self.lower_function_signature(node);
                let body = direct_child(node, NodeKind::BlockExpression)
                    .map(|body| self.lower_expression(&body))
                    .unwrap_or(ModuleExpression::Empty {
                        semantic_id,
                        range: significant_range(node),
                    });
                ModuleExpression::Function {
                    semantic_id,
                    signature,
                    body: Box::new(body),
                    range: significant_range(node),
                }
            }
            NodeKind::CaseExpression => self.lower_case(node, semantic_id),
            NodeKind::LoopExpression => self.lower_loop(node, semantic_id),
            NodeKind::BreakExpression => self.lower_break(node, semantic_id),
            _ => ModuleExpression::Sequence {
                semantic_id,
                expressions: contained_expression_children(node)
                    .into_iter()
                    .map(|child| self.lower_expression(&child))
                    .collect(),
                range: significant_range(node),
            },
        }
    }

    /// Lowers `{ ..base, field: value }`. `{ ..base }` names no field and is `L2022`,
    /// with a fix that replaces it by `base`; it lowers to the base alone.
    fn lower_update(&mut self, node: &SyntaxNode<'_>, semantic_id: SemanticId) -> ModuleExpression {
        let range = significant_range(node);
        let spread = direct_child(node, NodeKind::RecordSpread).expect("an update has a spread");
        let base_node = direct_expression_child(&spread);
        let base = match &base_node {
            Some(base) => self.lower_expression(base),
            None => ModuleExpression::Empty {
                semantic_id: self.allocate_semantic_id(),
                range: significant_range(&spread),
            },
        };
        let fields = node
            .child_nodes()
            .into_iter()
            .filter(|child| child.kind() == NodeKind::RecordField)
            .filter_map(|field| {
                Some(RecordValueField {
                    name: direct_identifier(&field)?,
                    value: self.lower_expression(&direct_expression_child(&field)?),
                })
            })
            .collect::<Vec<_>>();
        if fields.is_empty() {
            let mut diagnostic = Diagnostic::error(
                DiagnosticStage::Lowering,
                "L2022",
                FileId::new(0),
                range,
                "an update must name at least one field; `{ ..base }` is `base`",
            );
            if let Some(base_node) = &base_node {
                let text = base_node
                    .descendant_tokens()
                    .iter()
                    .map(|token| token.text())
                    .collect::<String>();
                diagnostic = diagnostic.with_fix(range, text.trim());
            }
            self.diagnostics.push(diagnostic);
            return base;
        }
        ModuleExpression::Update {
            semantic_id,
            base: Box::new(base),
            fields,
            range,
        }
    }

    fn lower_postfix(
        &mut self,
        node: &SyntaxNode<'_>,
        semantic_id: SemanticId,
    ) -> ModuleExpression {
        let mut children = node.child_nodes().into_iter();
        let Some(base) = children.next() else {
            return ModuleExpression::Empty {
                semantic_id,
                range: significant_range(node),
            };
        };
        let mut expression = self.lower_expression(&base);
        let suffixes = children.collect::<Vec<_>>();
        let last_suffix = suffixes.len().saturating_sub(1);
        for (index, suffix) in suffixes.into_iter().enumerate() {
            let suffix_semantic_id = if index == last_suffix {
                semantic_id
            } else {
                self.allocate_semantic_id()
            };
            match suffix.kind() {
                NodeKind::MemberAccess => {
                    if let Some(member) = direct_identifier(&suffix) {
                        expression = ModuleExpression::Select {
                            semantic_id: suffix_semantic_id,
                            receiver: Box::new(expression),
                            member,
                            range: significant_range(node),
                        };
                    }
                }
                NodeKind::CallSuffix
                    if significant_child_tokens(&suffix)
                        .iter()
                        .any(|token| token.kind() == TokenKind::DotDot) =>
                {
                    // `Config(..base, jobs)` is already S1001; typing it as a call
                    // would only add argument errors.
                    expression = ModuleExpression::Empty {
                        semantic_id: suffix_semantic_id,
                        range: significant_range(node),
                    };
                }
                NodeKind::CallSuffix => {
                    let arguments = suffix
                        .child_nodes()
                        .into_iter()
                        .filter(|argument| argument.kind() == NodeKind::Argument)
                        .filter_map(|argument| {
                            Some(CallArgument {
                                label: direct_identifier(&argument),
                                value: self.lower_expression(&direct_expression_child(&argument)?),
                            })
                        })
                        .collect();
                    expression = ModuleExpression::Call {
                        semantic_id: suffix_semantic_id,
                        callee: Box::new(expression),
                        arguments,
                        range: significant_range(node),
                    };
                }
                _ => {}
            }
        }
        expression
    }

    fn lower_binary(&mut self, node: &SyntaxNode<'_>, semantic_id: SemanticId) -> ModuleExpression {
        let mut operands = direct_expression_children(node).into_iter();
        let Some(first) = operands.next() else {
            return ModuleExpression::Empty {
                semantic_id,
                range: significant_range(node),
            };
        };
        let operators = node
            .child_tokens()
            .into_iter()
            // `None` is a pipeline stage, which lowers to a call rather than an operator.
            .filter_map(|token| {
                let operator = match token.kind() {
                    TokenKind::Plus => Some(BinaryOperator::Add),
                    TokenKind::Minus => Some(BinaryOperator::Subtract),
                    TokenKind::Star => Some(BinaryOperator::Multiply),
                    TokenKind::Slash => Some(BinaryOperator::Divide),
                    TokenKind::Percent => Some(BinaryOperator::Remainder),
                    TokenKind::Less => Some(BinaryOperator::Less),
                    TokenKind::LessEqual => Some(BinaryOperator::LessEqual),
                    TokenKind::Greater => Some(BinaryOperator::Greater),
                    TokenKind::GreaterEqual => Some(BinaryOperator::GreaterEqual),
                    TokenKind::EqualEqual => Some(BinaryOperator::Equal),
                    TokenKind::BangEqual => Some(BinaryOperator::NotEqual),
                    TokenKind::DotDot => Some(BinaryOperator::Range),
                    TokenKind::DotDotEqual => Some(BinaryOperator::RangeInclusive),
                    TokenKind::Pipe => None,
                    TokenKind::And | TokenKind::AmpersandAmpersand => Some(BinaryOperator::And),
                    TokenKind::Or | TokenKind::BarBar => Some(BinaryOperator::Or),
                    _ => return None,
                };
                Some(operator)
            })
            .collect::<Vec<_>>();
        let mut operands = operands.collect::<Vec<_>>();
        let mut operators = operators;
        // A chained range was reported as `S1006`; only its first range lowers, so the
        // chain adds no type follow-ons.
        if node.kind() == NodeKind::RangeExpression {
            operators.truncate(1);
            operands.truncate(1);
        }
        let last_operator = operators.len().saturating_sub(1);
        let start = significant_range(&first).start();
        let mut left = self.lower_expression(&first);
        for (index, (operator, right_node)) in operators.into_iter().zip(operands).enumerate() {
            let semantic_id = if index == last_operator {
                semantic_id
            } else {
                self.allocate_semantic_id()
            };
            let right = self.lower_expression(&right_node);
            // Stage `index` of a chain spans the first operand through its own
            // right operand, so diagnostics and inspection name that stage only.
            let range = TextRange::new(
                start as usize,
                significant_range(&right_node).end() as usize,
            );
            left = match (operator, right) {
                (
                    None,
                    ModuleExpression::Call {
                        callee, arguments, ..
                    },
                ) => insert_pipeline_subject(semantic_id, left, *callee, arguments, range),
                // `x |> f` is exactly the call `f(x)`.
                (None, callee) => {
                    insert_pipeline_subject(semantic_id, left, callee, Vec::new(), range)
                }
                (Some(operator), right) => ModuleExpression::Binary {
                    semantic_id,
                    operator,
                    left: Box::new(left),
                    right: Box::new(right),
                    range,
                },
            };
        }
        left
    }

    fn lower_using(&mut self, node: &SyntaxNode<'_>, semantic_id: SemanticId) -> ModuleExpression {
        let mut children = direct_expression_children(node).into_iter();
        let Some(base) = children.next() else {
            return ModuleExpression::Empty {
                semantic_id,
                range: significant_range(node),
            };
        };
        let provider_records = children
            .filter(|child| child.kind() == NodeKind::RecordExpression)
            .collect::<Vec<_>>();
        let last_record = provider_records.len().saturating_sub(1);
        let mut expression = self.lower_expression(&base);
        for (index, record) in provider_records.into_iter().enumerate() {
            let providers = record
                .child_nodes()
                .into_iter()
                .filter(|child| child.kind() == NodeKind::RecordField)
                .filter_map(|field| {
                    Some(RecordValueField {
                        name: direct_identifier(&field)?,
                        value: self.lower_expression(&direct_expression_child(&field)?),
                    })
                })
                .collect();
            expression = ModuleExpression::Using {
                semantic_id: if index == last_record {
                    semantic_id
                } else {
                    self.allocate_semantic_id()
                },
                expression: Box::new(expression),
                providers,
                range: significant_range(node),
            };
        }
        expression
    }

    fn lower_module_literal(
        &mut self,
        node: &SyntaxNode<'_>,
        semantic_id: SemanticId,
    ) -> ModuleExpression {
        self.lower_literal(node, semantic_id, None)
    }

    /// Lowers a `Literal` node. `negation` is the range of a unary `-` applied directly
    /// to it, which folds into an integer literal before the range check (`S1002`), so
    /// `-9223372036854775808` is valid while `-(9223372036854775808)` is not.
    fn lower_literal(
        &mut self,
        node: &SyntaxNode<'_>,
        semantic_id: SemanticId,
        negation: Option<TextRange>,
    ) -> ModuleExpression {
        let Some(token) = node.child_tokens().into_iter().find(|token| {
            matches!(
                token.kind(),
                TokenKind::Integer
                    | TokenKind::Float
                    | TokenKind::Text
                    | TokenKind::True
                    | TokenKind::False
                    | TokenKind::Void
            )
        }) else {
            return ModuleExpression::Empty {
                semantic_id,
                range: significant_range(node),
            };
        };
        let literal = match token.kind() {
            TokenKind::Integer => Literal::Integer {
                semantic_id,
                value: self.integer_literal(&token, negation.is_some()),
                range: negation.unwrap_or(token.range()),
            },
            TokenKind::Float => Literal::Float {
                semantic_id,
                value: self.float_literal(&token),
                range: token.range(),
            },
            TokenKind::Text => Literal::Text {
                semantic_id,
                value: text_literal_value(token.text()),
                range: token.range(),
            },
            TokenKind::True | TokenKind::False => Literal::Boolean {
                semantic_id,
                value: token.kind() == TokenKind::True,
                range: token.range(),
            },
            TokenKind::Void => Literal::Void {
                semantic_id,
                range: token.range(),
            },
            _ => unreachable!("literal token was filtered above"),
        };
        ModuleExpression::Literal(literal)
    }

    /// The value of an integer literal token, negated when `negated`; reports `S1002`
    /// and yields 0 when the value is outside `Int`. Hexadecimal and binary literals
    /// are magnitudes, never two's complement bit patterns.
    fn integer_literal(&mut self, token: &SyntaxToken<'_>, negated: bool) -> i64 {
        let digits = token.text().replace('_', "");
        let magnitude = if let Some(hex) = digits.strip_prefix("0x") {
            u64::from_str_radix(hex, 16)
        } else if let Some(binary) = digits.strip_prefix("0b") {
            u64::from_str_radix(binary, 2)
        } else {
            digits.parse()
        };
        let magnitude = match magnitude {
            Ok(magnitude) => Some(i128::from(magnitude)),
            Err(error) if *error.kind() == std::num::IntErrorKind::PosOverflow => None,
            // The lexer already reported the malformed digits.
            Err(_) => return 0,
        };
        let value = magnitude
            .and_then(|magnitude| i64::try_from(if negated { -magnitude } else { magnitude }).ok());
        value.unwrap_or_else(|| {
            self.literal_error(
                "S1002",
                token.range(),
                "integer literal is outside the supported range",
            );
            0
        })
    }

    /// The value of a float literal token; reports `S1003` and yields 0 when it
    /// rounds to an infinity.
    fn float_literal(&mut self, token: &SyntaxToken<'_>) -> f64 {
        crate::numeric::float_literal_value(token.text()).unwrap_or_else(|| {
            self.literal_error(
                "S1003",
                token.range(),
                "float literal is outside the finite `Float` range",
            );
            0.0
        })
    }

    fn literal_error(&mut self, code: &'static str, range: TextRange, message: &str) {
        self.diagnostics.push(Diagnostic::error(
            DiagnosticStage::Syntax,
            code,
            FileId::new(0),
            range,
            message,
        ));
    }

    /// Lowers an `InterpolatedText` node into its decoded chunks and expressions.
    fn lower_interpolation(
        &mut self,
        node: &SyntaxNode<'_>,
        semantic_id: SemanticId,
    ) -> ModuleExpression {
        // Chunk tokens and expression nodes interleave; source order restores the sequence.
        let mut pieces = Vec::new();
        for token in node.child_tokens() {
            let text = token.text();
            let chunk = match token.kind() {
                TokenKind::TextStart => &text[1..],
                TokenKind::TextMiddle => text,
                TokenKind::TextEnd => &text[..text.len() - 1],
                _ => continue,
            };
            if !chunk.is_empty() {
                pieces.push((token.range().start(), Err(decode_text(chunk))));
            }
        }
        for child in node.child_nodes() {
            if is_expression_kind(child.kind()) {
                pieces.push((child.range().start(), Ok(child)));
            }
        }
        pieces.sort_by_key(|(start, _)| *start);
        let parts = pieces
            .into_iter()
            .map(|(_, piece)| match piece {
                Ok(child) => InterpolationPart::Expression(self.lower_expression(&child)),
                Err(text) => InterpolationPart::Text(text),
            })
            .collect::<Vec<_>>();
        if !parts
            .iter()
            .any(|part| matches!(part, InterpolationPart::Expression(_)))
        {
            // Only `${}` parts, already reported as `S1101`.
            return ModuleExpression::Empty {
                semantic_id,
                range: significant_range(node),
            };
        }
        ModuleExpression::Interpolation {
            semantic_id,
            parts,
            range: significant_range(node),
        }
    }

    /// `L2023`: a block's value is its final expression, so a block MUST NOT end with
    /// a `let`, plain or destructuring. The block still lowers as written.
    fn reject_trailing_let(&mut self, items: &[SyntaxNode<'_>]) {
        let Some(binding) = items
            .last()
            .and_then(|item| item.child_nodes().into_iter().next())
            .filter(|child| child.kind() == NodeKind::LetBinding)
        else {
            return;
        };
        self.diagnostics.push(Diagnostic::error(
            DiagnosticStage::Lowering,
            "L2023",
            FileId::new(0),
            significant_range(&binding),
            "a block ends with its value, but this block ends with a `let`; add the value after it, or use the bound expression as the value",
        ));
    }

    /// Lowers block items in order. A `let` whose pattern is not a plain or typed name
    /// becomes a `case` over its value with one arm, whose body is the rest of the block,
    /// so its bindings scope over exactly the items after it. `block` ends that body.
    fn lower_block_items(
        &mut self,
        nodes: &[SyntaxNode<'_>],
        block: TextRange,
    ) -> Vec<ModuleBlockItem> {
        let mut items = Vec::new();
        for (index, node) in nodes.iter().enumerate() {
            let Some(child) = node.child_nodes().into_iter().next() else {
                continue;
            };
            // A block `const` was already reported as `S1103`; it binds like `let`.
            if !matches!(
                child.kind(),
                NodeKind::LetBinding | NodeKind::ConstDeclaration
            ) {
                items.push(ModuleBlockItem::Expression(self.lower_expression(&child)));
                continue;
            }
            let value = direct_expression_child(&child)
                .map(|value| self.lower_expression(&value))
                .unwrap_or(ModuleExpression::Empty {
                    semantic_id: self.allocate_semantic_id(),
                    range: significant_range(&child),
                });
            let pattern = match direct_child(&child, NodeKind::Pattern) {
                Some(pattern) => self.lower_pattern(&pattern),
                None => match direct_identifier(&child) {
                    // `let _ = value` binds nothing; it lowers like any other
                    // non-binding pattern, to a `case` over the value.
                    Some(name) if name.text == "_" => Pattern::Wildcard {
                        semantic_id: self.allocate_semantic_id(),
                        range: name.range,
                    },
                    Some(name) => Pattern::Binding {
                        semantic_id: self.allocate_semantic_id(),
                        range: name.range,
                        name,
                    },
                    None => continue,
                },
            };
            match pattern {
                Pattern::Binding { name, .. } => items.push(ModuleBlockItem::Binding {
                    name,
                    type_use: None,
                    value,
                }),
                Pattern::Typed {
                    name: Some(name),
                    type_use,
                    ..
                } => items.push(ModuleBlockItem::Binding {
                    name,
                    type_use: Some(type_use),
                    value,
                }),
                pattern => {
                    let keyword = child
                        .child_tokens()
                        .into_iter()
                        .find(|token| token.kind() == TokenKind::Let)
                        .map_or(pattern.range(), |token| token.range());
                    let start = significant_range(&child).start() as usize;
                    let rest_range = TextRange::new(start, block.end() as usize);
                    let rest = self.lower_block_items(&nodes[index + 1..], block);
                    let body = if rest.is_empty() {
                        ModuleExpression::Literal(Literal::Void {
                            semantic_id: self.allocate_semantic_id(),
                            range: TextRange::empty(significant_range(&child).end() as usize),
                        })
                    } else {
                        ModuleExpression::Block {
                            semantic_id: self.allocate_semantic_id(),
                            items: rest,
                            range: rest_range,
                        }
                    };
                    let case = self.binding_case(value, pattern, body, BindingKind::Let, keyword);
                    items.push(ModuleBlockItem::Expression(case));
                    return items;
                }
            }
        }
        items
    }

    /// A one-arm `case` that matches `subject` against the irrefutable `pattern` of a
    /// `let` or `loop` state and evaluates `body` with its bindings.
    fn binding_case(
        &mut self,
        subject: ModuleExpression,
        pattern: Pattern,
        body: ModuleExpression,
        kind: BindingKind,
        keyword: TextRange,
    ) -> ModuleExpression {
        let arm_range = pattern.range();
        let range = TextRange::new(
            keyword.start().min(arm_range.start()) as usize,
            body.range().end().max(arm_range.end()) as usize,
        );
        ModuleExpression::Case {
            semantic_id: self.allocate_semantic_id(),
            subjects: vec![subject],
            arms: vec![ModuleCaseArm {
                patterns: vec![pattern],
                guard: None,
                body,
                range: arm_range,
            }],
            range,
            origin: CaseOrigin::Binding { kind },
        }
    }

    /// Ensures: missing state, initial, or body lower to `Empty`/placeholder parts so the
    /// node keeps its identity and diagnostics come from the parser, never a panic.
    fn lower_loop(&mut self, node: &SyntaxNode<'_>, semantic_id: SemanticId) -> ModuleExpression {
        let range = significant_range(node);
        let mut state = direct_identifier(node).unwrap_or_else(|| Name {
            text: "_".to_owned(),
            range: TextRange::empty(range.start() as usize),
        });
        let mut parts = direct_expression_children(node).into_iter();
        let lower_part = |part: Option<SyntaxNode<'_>>, lowerer: &mut Self| {
            part.map(|part| lowerer.lower_expression(&part))
                .unwrap_or_else(|| ModuleExpression::Empty {
                    semantic_id: lowerer.allocate_semantic_id(),
                    range,
                })
        };
        let state_id = self.allocate_semantic_id();
        let pattern = direct_child(node, NodeKind::Pattern)
            .map(|pattern| self.lower_pattern(&pattern))
            .filter(|pattern| !matches!(pattern, Pattern::Wildcard { .. }));
        let pattern = match pattern {
            Some(Pattern::Binding { name, .. }) => {
                state = name;
                None
            }
            other => other,
        };
        // A `loop _` or a destructuring state still needs a state binding, which
        // identifies the loop; the destructuring then matches it on every pass.
        if state.text == "_" || pattern.is_some() {
            state = Name {
                text: format!("{GENERATED_NAME_PREFIX}loop{}", state_id.raw()),
                range: pattern.as_ref().map_or(state.range, Pattern::range),
            };
        }
        let initial = lower_part(parts.next(), self);
        let mut body = lower_part(parts.next(), self);
        if let Some(pattern) = pattern {
            let keyword = node
                .child_tokens()
                .into_iter()
                .find(|token| token.kind() == TokenKind::Loop)
                .map_or(range, |token| token.range());
            let subject = ModuleExpression::Name {
                semantic_id: self.allocate_semantic_id(),
                name: state.clone(),
            };
            body = self.binding_case(subject, pattern, body, BindingKind::LoopState, keyword);
        }
        ModuleExpression::Loop {
            semantic_id,
            state_id,
            state,
            initial: Box::new(initial),
            body: Box::new(body),
            range,
        }
    }

    fn lower_break(&mut self, node: &SyntaxNode<'_>, semantic_id: SemanticId) -> ModuleExpression {
        ModuleExpression::Break {
            semantic_id,
            value: direct_expression_child(node)
                .map(|value| Box::new(self.lower_expression(&value))),
            range: significant_range(node),
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
                        range: significant_range(&arm),
                    });
                let guard = arm_expressions
                    .pop()
                    .map(|guard| self.lower_expression(&guard));
                ModuleCaseArm {
                    patterns,
                    guard,
                    body,
                    range: significant_range(&arm),
                }
            })
            .collect();
        ModuleExpression::Case {
            semantic_id,
            subjects,
            arms,
            range: significant_range(node),
            origin: CaseOrigin::Source,
        }
    }

    fn lower_pattern(&mut self, node: &SyntaxNode<'_>) -> Pattern {
        let semantic_id = self.allocate_semantic_id();
        let range = significant_range(node);
        let tokens = significant_child_tokens(node);
        let children = node
            .child_nodes()
            .into_iter()
            .filter(|child| child.kind() == NodeKind::Pattern)
            .collect::<Vec<_>>();
        let Some(first) = tokens.first() else {
            // An interpolated text pattern is already S1101. Lowering it as a text
            // literal keeps it from reading as a catch-all, which would add T4233.
            if let Some(text) = direct_child(node, NodeKind::InterpolatedText) {
                return Pattern::Literal {
                    semantic_id,
                    literal: PatternLiteral::Text(
                        text.descendant_tokens()
                            .iter()
                            .map(SyntaxToken::text)
                            .collect(),
                    ),
                    range: significant_range(&text),
                };
            }
            return Pattern::Wildcard { semantic_id, range };
        };
        let identifiers = tokens
            .iter()
            .filter(|token| token.kind() == TokenKind::Identifier)
            .map(name_from)
            .collect::<Vec<_>>();
        if tokens.iter().any(|token| token.kind() == TokenKind::As) {
            let inner = match children.first() {
                Some(inner) => self.lower_pattern(inner),
                None => Pattern::Wildcard {
                    semantic_id: self.allocate_semantic_id(),
                    range,
                },
            };
            return match direct_token_after(node, TokenKind::As, TokenKind::Identifier) {
                Some(name) => Pattern::Alias {
                    semantic_id,
                    pattern: Box::new(inner),
                    name: name_from(&name),
                    range,
                },
                None => inner,
            };
        }
        if tokens.iter().any(|token| token.kind() == TokenKind::Colon) {
            let name = identifiers.first().filter(|name| name.text != "_").cloned();
            return match (direct_child(node, NodeKind::Type), name) {
                (Some(type_node), name) => Pattern::Typed {
                    semantic_id,
                    name,
                    type_use: self.lower_type(&type_node),
                    range,
                },
                // A labelled pattern, already `S1001`.
                (None, Some(name)) => Pattern::Binding {
                    semantic_id,
                    name,
                    range,
                },
                (None, None) => Pattern::Wildcard { semantic_id, range },
            };
        }
        let literal = match first.kind() {
            TokenKind::Minus => match tokens.get(1) {
                Some(digits) if digits.kind() == TokenKind::Integer => {
                    PatternLiteral::Integer(self.integer_literal(digits, true))
                }
                _ => PatternLiteral::Float,
            },
            TokenKind::LeftBracket => {
                return self.lower_list_pattern(semantic_id, &children, range);
            }
            TokenKind::LeftParen => {
                // The parser rejects `()` and `(p,)` with `S1005`; they lower to what
                // they hold so typing adds no follow-on tuple-shape error.
                return match children.as_slice() {
                    [] => Pattern::Wildcard { semantic_id, range },
                    [only] => self.lower_pattern(only),
                    _ => Pattern::Tuple {
                        semantic_id,
                        elements: children
                            .iter()
                            .map(|child| self.lower_pattern(child))
                            .collect(),
                        range,
                    },
                };
            }
            TokenKind::DotDot => {
                return match identifiers.into_iter().next() {
                    Some(name) if name.text != "_" => Pattern::Binding {
                        semantic_id,
                        name,
                        range,
                    },
                    _ => Pattern::Wildcard { semantic_id, range },
                };
            }
            TokenKind::Integer => PatternLiteral::Integer(self.integer_literal(first, false)),
            TokenKind::Float => {
                // Typing rejects every Float pattern (T4313), but the range check is
                // a literal rule that applies in patterns too.
                self.float_literal(first);
                PatternLiteral::Float
            }
            TokenKind::Text => PatternLiteral::Text(text_literal_value(first.text())),
            TokenKind::True | TokenKind::False => {
                PatternLiteral::Boolean(first.kind() == TokenKind::True)
            }
            TokenKind::Void => PatternLiteral::Void,
            _ => {
                let applied = tokens
                    .iter()
                    .any(|token| token.kind() == TokenKind::LeftParen);
                return match identifiers.as_slice() {
                    [] => Pattern::Wildcard { semantic_id, range },
                    [name] if !applied && name.text == "_" => {
                        Pattern::Wildcard { semantic_id, range }
                    }
                    [name] if !applied && !name.text.starts_with(char::is_uppercase) => {
                        Pattern::Binding {
                            semantic_id,
                            name: name.clone(),
                            range,
                        }
                    }
                    _ => Pattern::Constructor {
                        semantic_id,
                        path: identifiers,
                        arguments: children
                            .iter()
                            .map(|child| self.lower_pattern(child))
                            .collect(),
                        range,
                    },
                };
            }
        };
        Pattern::Literal {
            semantic_id,
            literal,
            range,
        }
    }

    /// Splits list elements around the first rest element. A further rest is `L2021`;
    /// it is dropped so the elements after it still type-check as the suffix, and the
    /// pattern is marked so coverage does not read the remainder as written.
    fn lower_list_pattern(
        &mut self,
        semantic_id: SemanticId,
        elements: &[SyntaxNode<'_>],
        range: TextRange,
    ) -> Pattern {
        let mut prefix = Vec::new();
        let mut rest = None;
        let mut suffix = Vec::new();
        let mut rejected_rest = false;
        for element in elements {
            let is_rest = significant_child_tokens(element)
                .first()
                .is_some_and(|token| token.kind() == TokenKind::DotDot);
            if is_rest && rest.is_some() {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticStage::Lowering,
                    "L2021",
                    FileId::new(0),
                    significant_range(element),
                    "a list pattern can contain at most one `..`",
                ));
                rejected_rest = true;
                continue;
            }
            let lowered = self.lower_pattern(element);
            if is_rest {
                rest = Some(Box::new(lowered));
            } else if rest.is_some() {
                suffix.push(lowered);
            } else {
                prefix.push(lowered);
            }
        }
        Pattern::List {
            semantic_id,
            prefix,
            rest,
            suffix,
            rejected_rest,
            range,
        }
    }

    fn allocate_semantic_id(&mut self) -> SemanticId {
        let id = SemanticId::new(self.next_semantic_id);
        self.next_semantic_id += 1;
        id
    }
}

fn text_literal_value(source: &str) -> String {
    decode_text(&source[1..source.len() - 1])
}

/// The text a literal chunk denotes, with each escape decoded. The lexer has already
/// reported a malformed escape with `S1001`, so one is kept as written.
fn decode_text(chunk: &str) -> String {
    let mut decoded = String::with_capacity(chunk.len());
    let mut rest = chunk;
    while let Some(backslash) = rest.find('\\') {
        decoded.push_str(&rest[..backslash]);
        let after = &rest[backslash + 1..];
        rest = match syntax::text_escape(after) {
            TextEscape::Valid { character, length } => {
                decoded.push(character);
                &after[length..]
            }
            TextEscape::Invalid { length, .. } => {
                decoded.push('\\');
                decoded.push_str(&after[..length]);
                &after[length..]
            }
            TextEscape::Truncated => {
                decoded.push('\\');
                after
            }
        };
    }
    decoded.push_str(rest);
    decoded
}

fn function_name(node: &SyntaxNode<'_>) -> Option<Name> {
    direct_token_after(node, TokenKind::Fn, TokenKind::Identifier).map(|token| name_from(&token))
}

/// Function names declared by more than one run of adjacent declarations.
fn separated_function_names(nodes: &[SyntaxNode<'_>]) -> HashSet<String> {
    let mut seen = HashSet::new();
    let mut separated = HashSet::new();
    let mut previous: Option<String> = None;
    for node in nodes {
        let name = (node.kind() == NodeKind::FunctionDeclaration)
            .then(|| function_name(node))
            .flatten()
            .map(|name| name.text);
        if let Some(name) = &name
            && previous.as_ref() != Some(name)
            && !seen.insert(name.clone())
        {
            separated.insert(name.clone());
        }
        previous = name;
    }
    separated
}

fn clause_parameters<'tree>(node: &SyntaxNode<'tree>) -> Vec<SyntaxNode<'tree>> {
    direct_child(node, NodeKind::ValueParameters)
        .map(|parameters| {
            parameters
                .child_nodes()
                .into_iter()
                .filter(|child| child.kind() == NodeKind::ValueParameter)
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a function or anonymous function is written with clause syntax: a
/// parameter that is a pattern, or a guard.
fn clause_has_patterns(node: &SyntaxNode<'_>) -> bool {
    direct_child(node, NodeKind::ClauseGuard).is_some()
        || clause_parameters(node)
            .iter()
            .any(|parameter| direct_child(parameter, NodeKind::Pattern).is_some())
}

/// `name(patterns)`: where clause diagnostics point.
fn clause_head_range(node: &SyntaxNode<'_>) -> TextRange {
    let whole = significant_range(node);
    let start = function_name(node).map_or(whole.start(), |name| name.range.start());
    let end = direct_child(node, NodeKind::ValueParameters).map_or(whole.end(), |parameters| {
        significant_range(&parameters).end()
    });
    TextRange::new(start as usize, end as usize)
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
            | NodeKind::OrExpression
            | NodeKind::AndExpression
            | NodeKind::NotExpression
            | NodeKind::PipelineExpression
            | NodeKind::ComparisonExpression
            | NodeKind::AdditiveExpression
            | NodeKind::MultiplicativeExpression
            | NodeKind::UnaryExpression
            | NodeKind::PostfixExpression
            | NodeKind::Literal
            | NodeKind::InterpolatedText
            | NodeKind::NameReference
            | NodeKind::GroupedExpression
            | NodeKind::TupleExpression
            | NodeKind::RangeExpression
            | NodeKind::ListExpression
            | NodeKind::RecordExpression
            | NodeKind::BlockExpression
            | NodeKind::CaseExpression
            | NodeKind::LoopExpression
            | NodeKind::BreakExpression
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

/// Lowers `subject |> callee(arguments)` to `callee(subject, arguments)`.
///
/// The pipeline rule is syntactic: only a call written directly on the right of
/// `|>` receives the subject as its first argument. Any other right-hand side,
/// including a parenthesized call, is applied to the subject as a unary function.
fn insert_pipeline_subject(
    semantic_id: SemanticId,
    subject: ModuleExpression,
    callee: ModuleExpression,
    arguments: Vec<CallArgument>,
    range: TextRange,
) -> ModuleExpression {
    let mut piped = Vec::with_capacity(arguments.len() + 1);
    piped.push(CallArgument {
        label: None,
        value: subject,
    });
    piped.extend(arguments);
    ModuleExpression::Call {
        semantic_id,
        callee: Box::new(callee),
        arguments: piped,
        range,
    }
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
