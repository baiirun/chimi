use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::{SemanticId, TextRange};
use crate::source::SourceSnapshot;
use crate::syntax::{self, NodeKind, SyntaxNode, SyntaxToken, TokenKind};
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

    for node in parsed.root().child_nodes() {
        match node.kind() {
            NodeKind::StructDeclaration if lowerer.structure.is_none() => {
                lowerer.structure = lowerer.lower_struct(node);
            }
            NodeKind::StructDeclaration => {
                lowerer.lowering_error(
                    node,
                    "additional struct declarations are not lowered by the executable subset",
                );
            }
            NodeKind::FunctionDeclaration if lowerer.functions.is_empty() => {
                if let Some(function) = lowerer.lower_function(node) {
                    lowerer.functions.push(function);
                }
            }
            NodeKind::FunctionDeclaration => {
                lowerer.lowering_error(
                    node,
                    "additional function declarations are not lowered by the executable subset",
                );
            }
            NodeKind::ImportDeclaration
            | NodeKind::EnumDeclaration
            | NodeKind::InterfaceDeclaration => {
                lowerer.lowering_error(
                    node,
                    "declaration is parsed but not lowered by the executable subset",
                );
            }
            _ => {}
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

    fn lower_struct(&mut self, node: SyntaxNode<'_>) -> Option<StructDeclaration> {
        let tokens = significant_tokens(node);
        let [_, name, _, field_name, _, field_type, _] = tokens.as_slice() else {
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
            name: name_from(name),
            field_semantic_id: self.allocate_semantic_id(),
            field_name: name_from(field_name),
            field_type: name_from(field_type),
        })
    }

    fn lower_function(&mut self, node: SyntaxNode<'_>) -> Option<FunctionDeclaration> {
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
        let function_name = name_from(&tokens[2]);
        let function_semantic_id = self.allocate_semantic_id();
        let body = self.lower_construction(&tokens[6..12], node)?;

        Some(FunctionDeclaration {
            semantic_id: function_semantic_id,
            public: true,
            name: function_name,
            body,
        })
    }

    fn lower_construction(
        &mut self,
        tokens: &[SyntaxToken<'_>],
        owner: SyntaxNode<'_>,
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

    fn lowering_error(&mut self, node: SyntaxNode<'_>, message: &'static str) {
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

fn significant_tokens(node: SyntaxNode<'_>) -> Vec<SyntaxToken<'_>> {
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
