use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::{SemanticId, TextRange};
use crate::source::SourceSnapshot;
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

#[derive(Clone, Debug, Eq, PartialEq)]
enum TokenKind {
    Struct,
    Pub,
    Fn,
    Identifier(String),
    Integer(i64),
    Text(String),
    LeftBrace,
    RightBrace,
    LeftParen,
    RightParen,
    Colon,
    Comma,
    End,
}

#[derive(Clone, Debug)]
struct Token {
    kind: TokenKind,
    range: TextRange,
}

pub(crate) fn lower(source: Arc<SourceSnapshot>) -> SemanticProgram {
    let (tokens, diagnostics) = lex(&source);
    Parser::new(source, tokens, diagnostics).parse()
}

fn lex(source: &SourceSnapshot) -> (Vec<Token>, Vec<Diagnostic>) {
    let text = source.text();
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut diagnostics = Vec::new();
    let mut offset = 0;

    while offset < bytes.len() {
        let start = offset;
        let byte = bytes[offset];
        match byte {
            b' ' | b'\t' | b'\r' | b'\n' => {
                offset += 1;
            }
            b'/' if offset + 1 < bytes.len() && bytes[offset + 1] == b'/' => {
                offset += 2;
                while offset < bytes.len() && bytes[offset] != b'\n' {
                    offset += 1;
                }
            }
            b'{' | b'}' | b'(' | b')' | b':' | b',' => {
                offset += 1;
                let kind = match byte {
                    b'{' => TokenKind::LeftBrace,
                    b'}' => TokenKind::RightBrace,
                    b'(' => TokenKind::LeftParen,
                    b')' => TokenKind::RightParen,
                    b':' => TokenKind::Colon,
                    b',' => TokenKind::Comma,
                    _ => unreachable!(),
                };
                tokens.push(Token {
                    kind,
                    range: TextRange::new(start, offset),
                });
            }
            b'0'..=b'9' => {
                offset += 1;
                while offset < bytes.len() && bytes[offset].is_ascii_digit() {
                    offset += 1;
                }
                let value = match text[start..offset].parse() {
                    Ok(value) => value,
                    Err(_) => {
                        diagnostics.push(Diagnostic::error(
                            DiagnosticStage::Syntax,
                            "S1002",
                            source.file_id(),
                            TextRange::new(start, offset),
                            "integer literal is outside the supported range",
                        ));
                        0
                    }
                };
                tokens.push(Token {
                    kind: TokenKind::Integer(value),
                    range: TextRange::new(start, offset),
                });
            }
            b'"' => {
                offset += 1;
                let content_start = offset;
                while offset < bytes.len() && bytes[offset] != b'"' && bytes[offset] != b'\n' {
                    offset += 1;
                }
                if offset < bytes.len() && bytes[offset] == b'"' {
                    let value = text[content_start..offset].to_owned();
                    offset += 1;
                    tokens.push(Token {
                        kind: TokenKind::Text(value),
                        range: TextRange::new(start, offset),
                    });
                } else {
                    diagnostics.push(Diagnostic::error(
                        DiagnosticStage::Syntax,
                        "S1001",
                        source.file_id(),
                        TextRange::new(start, offset),
                        "unterminated text literal",
                    ));
                }
            }
            _ if is_identifier_start(byte) => {
                offset += 1;
                while offset < bytes.len() && is_identifier_continue(bytes[offset]) {
                    offset += 1;
                }
                let word = &text[start..offset];
                let kind = match word {
                    "struct" => TokenKind::Struct,
                    "pub" => TokenKind::Pub,
                    "fn" => TokenKind::Fn,
                    _ => TokenKind::Identifier(word.to_owned()),
                };
                tokens.push(Token {
                    kind,
                    range: TextRange::new(start, offset),
                });
            }
            _ => {
                let character = text[start..]
                    .chars()
                    .next()
                    .expect("offset is before source end");
                offset += character.len_utf8();
                diagnostics.push(Diagnostic::error(
                    DiagnosticStage::Syntax,
                    "S1001",
                    source.file_id(),
                    TextRange::new(start, offset),
                    format!("unexpected source text `{character}`"),
                ));
            }
        }
    }

    tokens.push(Token {
        kind: TokenKind::End,
        range: TextRange::empty(text.len()),
    });
    (tokens, diagnostics)
}

fn is_identifier_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_lowercase() || byte.is_ascii_uppercase()
}

fn is_identifier_continue(byte: u8) -> bool {
    is_identifier_start(byte) || byte.is_ascii_digit()
}

struct Parser {
    source: Arc<SourceSnapshot>,
    tokens: Vec<Token>,
    position: usize,
    next_semantic_id: u32,
    diagnostics: Vec<Diagnostic>,
}

impl Parser {
    fn new(source: Arc<SourceSnapshot>, tokens: Vec<Token>, diagnostics: Vec<Diagnostic>) -> Self {
        Self {
            source,
            tokens,
            position: 0,
            next_semantic_id: 0,
            diagnostics,
        }
    }

    fn parse(mut self) -> SemanticProgram {
        let mut structure = None;
        let mut functions = Vec::new();
        while !self.at(&TokenKind::End) {
            if self.at(&TokenKind::Struct) {
                if let Some(declaration) = self.parse_struct() {
                    if structure.is_none() {
                        structure = Some(declaration);
                    } else {
                        self.syntax_error("M1 accepts exactly one struct declaration");
                    }
                }
            } else if self.at(&TokenKind::Pub) {
                if let Some(function) = self.parse_function() {
                    if functions.is_empty() {
                        functions.push(function);
                    } else {
                        self.syntax_error("M1 accepts exactly one function declaration");
                    }
                }
            } else {
                self.syntax_error("expected `struct` or `pub fn`");
                self.advance();
            }
        }

        SemanticProgram {
            source: self.source,
            structure,
            functions,
            diagnostics: self.diagnostics,
        }
    }

    fn parse_struct(&mut self) -> Option<StructDeclaration> {
        self.advance();
        let name = self.take_identifier("expected struct name")?;
        self.expect(TokenKind::LeftBrace, "expected `{` after struct name")?;
        let field_name = self.take_identifier("expected struct field name")?;
        self.expect(TokenKind::Colon, "expected `:` after struct field name")?;
        let field_type = self.take_identifier("expected struct field type")?;
        self.expect(TokenKind::RightBrace, "expected `}` after struct field")?;
        Some(StructDeclaration {
            semantic_id: self.allocate_semantic_id(),
            name,
            field_semantic_id: self.allocate_semantic_id(),
            field_name,
            field_type,
        })
    }

    fn parse_function(&mut self) -> Option<FunctionDeclaration> {
        self.advance();
        self.expect(TokenKind::Fn, "expected `fn` after `pub`")?;
        let name = self.take_identifier("expected function name")?;
        let function_semantic_id = self.allocate_semantic_id();
        self.expect(TokenKind::LeftParen, "expected `(` after function name")?;
        self.expect(TokenKind::RightParen, "M1 main does not accept parameters")?;
        self.expect(TokenKind::LeftBrace, "expected `{` before function body")?;
        let constructor = self.take_identifier("expected constructor expression")?;
        let construction_semantic_id = self.allocate_semantic_id();
        let construction_start = constructor.range.start() as usize;
        self.expect(TokenKind::LeftParen, "expected `(` after constructor")?;
        let field_name = self.take_identifier("expected constructor field name")?;
        self.expect(TokenKind::Colon, "expected `:` after constructor field")?;
        let value = self.take_literal()?;
        let body_end = self.current().range.end() as usize;
        self.expect(
            TokenKind::RightParen,
            "expected `)` after constructor field",
        )?;
        self.expect(TokenKind::RightBrace, "expected `}` after function body")?;

        Some(FunctionDeclaration {
            semantic_id: function_semantic_id,
            public: true,
            name,
            body: Construction {
                semantic_id: construction_semantic_id,
                range: TextRange::new(construction_start, body_end),
                constructor,
                field_name,
                value,
            },
        })
    }

    fn take_literal(&mut self) -> Option<Literal> {
        let token = self.current().clone();
        let literal = match token.kind {
            TokenKind::Integer(value) => Literal::Integer {
                semantic_id: self.allocate_semantic_id(),
                value,
                range: token.range,
            },
            TokenKind::Text(value) => Literal::Text {
                semantic_id: self.allocate_semantic_id(),
                value,
                range: token.range,
            },
            _ => {
                self.syntax_error("expected integer or text literal");
                return None;
            }
        };
        self.advance();
        Some(literal)
    }

    fn take_identifier(&mut self, message: &'static str) -> Option<Name> {
        let token = self.current().clone();
        let TokenKind::Identifier(text) = token.kind else {
            self.syntax_error(message);
            return None;
        };
        self.advance();
        Some(Name {
            text,
            range: token.range,
        })
    }

    fn expect(&mut self, expected: TokenKind, message: &'static str) -> Option<()> {
        if self.at(&expected) {
            self.advance();
            Some(())
        } else {
            self.syntax_error(message);
            None
        }
    }

    fn at(&self, expected: &TokenKind) -> bool {
        std::mem::discriminant(&self.current().kind) == std::mem::discriminant(expected)
    }

    fn current(&self) -> &Token {
        &self.tokens[self.position]
    }

    fn advance(&mut self) {
        if !self.at(&TokenKind::End) {
            self.position += 1;
        }
    }

    fn syntax_error(&mut self, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::error(
            DiagnosticStage::Syntax,
            "S1001",
            self.source.file_id(),
            self.current().range,
            message,
        ));
    }

    fn allocate_semantic_id(&mut self) -> SemanticId {
        let identifier = SemanticId::new(self.next_semantic_id);
        self.next_semantic_id += 1;
        identifier
    }
}
