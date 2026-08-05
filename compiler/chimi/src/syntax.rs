use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::TextRange;
use crate::source::SourceSnapshot;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TokenKind {
    Whitespace,
    Newline,
    Comment,
    Import,
    As,
    Struct,
    Enum,
    Interface,
    Pub,
    Fn,
    Errors,
    Needs,
    Void,
    Identifier,
    Integer,
    Text,
    LeftBrace,
    RightBrace,
    LeftParen,
    RightParen,
    LeftBracket,
    RightBracket,
    Colon,
    Comma,
    Dot,
    Slash,
    Less,
    Greater,
    Plus,
    Arrow,
    Invalid,
    InvalidText,
    End,
}

impl TokenKind {
    const fn is_trivia(self) -> bool {
        matches!(self, Self::Whitespace | Self::Comment)
    }

    const fn expected_name(self) -> &'static str {
        match self {
            Self::Import => "`import`",
            Self::As => "`as`",
            Self::Struct => "`struct`",
            Self::Enum => "`enum`",
            Self::Interface => "`interface`",
            Self::Pub => "`pub`",
            Self::Fn => "`fn`",
            Self::Errors => "`errors`",
            Self::Needs => "`needs`",
            Self::Void => "`Void`",
            Self::Identifier => "identifier",
            Self::Integer => "integer literal",
            Self::Text => "text literal",
            Self::LeftBrace => "`{`",
            Self::RightBrace => "`}`",
            Self::LeftParen => "`(`",
            Self::RightParen => "`)`",
            Self::LeftBracket => "`[`",
            Self::RightBracket => "`]`",
            Self::Colon => "`:`",
            Self::Comma => "`,`",
            Self::Dot => "`.`",
            Self::Slash => "`/`",
            Self::Less => "`<`",
            Self::Greater => "`>`",
            Self::Plus => "`+`",
            Self::Arrow => "`->`",
            Self::Whitespace
            | Self::Newline
            | Self::Comment
            | Self::Invalid
            | Self::InvalidText
            | Self::End => "valid syntax",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NodeKind {
    File,
    ImportDeclaration,
    ImportAlias,
    ImportMembers,
    ImportMember,
    StructDeclaration,
    EnumDeclaration,
    InterfaceDeclaration,
    Field,
    Variant,
    NamedTypeField,
    GenericParameters,
    GenericParameter,
    FunctionDeclaration,
    ValueParameters,
    ValueParameter,
    ImplementationContract,
    Type,
    FunctionType,
    TypeArguments,
    RequirementClause,
    RequirementRecord,
    ConstructionExpression,
    Error,
}

#[derive(Clone, Debug)]
struct LexedToken {
    kind: TokenKind,
    text: Arc<str>,
    range: TextRange,
}

#[derive(Clone, Debug)]
pub(crate) struct GreenToken {
    kind: TokenKind,
    text: Arc<str>,
    missing: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum GreenElement {
    Node(Arc<GreenNode>),
    Token(GreenToken),
}

#[derive(Clone, Debug)]
pub(crate) struct GreenNode {
    kind: NodeKind,
    children: Box<[GreenElement]>,
    text_len: u32,
}

impl GreenNode {
    #[cfg(test)]
    pub(crate) fn text(&self) -> String {
        let mut text = String::with_capacity(self.text_len as usize);
        self.write_text(&mut text);
        text
    }

    #[cfg(test)]
    fn write_text(&self, output: &mut String) {
        for child in &self.children {
            match child {
                GreenElement::Node(node) => node.write_text(output),
                GreenElement::Token(token) => output.push_str(&token.text),
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct ParseResult {
    root: Arc<GreenNode>,
    diagnostics: Vec<Diagnostic>,
}

impl ParseResult {
    pub(crate) fn root(&self) -> SyntaxNode<'_> {
        SyntaxNode {
            green: &self.root,
            offset: 0,
        }
    }

    pub(crate) fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    #[cfg(test)]
    fn round_trip(&self) -> String {
        self.root.text()
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SyntaxNode<'tree> {
    green: &'tree GreenNode,
    offset: usize,
}

impl<'tree> SyntaxNode<'tree> {
    pub(crate) const fn kind(self) -> NodeKind {
        self.green.kind
    }

    pub(crate) fn child_nodes(self) -> Vec<Self> {
        let mut offset = self.offset;
        let mut nodes = Vec::new();
        for child in &self.green.children {
            match child {
                GreenElement::Node(node) => {
                    nodes.push(Self {
                        green: node,
                        offset,
                    });
                    offset += node.text_len as usize;
                }
                GreenElement::Token(token) => offset += token.text.len(),
            }
        }
        nodes
    }

    pub(crate) fn descendant_tokens(self) -> Vec<SyntaxToken<'tree>> {
        let mut tokens = Vec::new();
        collect_tokens(self.green, self.offset, &mut tokens);
        tokens
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SyntaxToken<'tree> {
    green: &'tree GreenToken,
    range: TextRange,
}

impl SyntaxToken<'_> {
    pub(crate) const fn kind(&self) -> TokenKind {
        self.green.kind
    }

    pub(crate) fn text(&self) -> &str {
        &self.green.text
    }

    pub(crate) const fn range(&self) -> TextRange {
        self.range
    }

    pub(crate) const fn is_missing(&self) -> bool {
        self.green.missing
    }
}

fn collect_tokens<'tree>(
    node: &'tree GreenNode,
    mut offset: usize,
    output: &mut Vec<SyntaxToken<'tree>>,
) {
    for child in &node.children {
        match child {
            GreenElement::Node(node) => {
                collect_tokens(node, offset, output);
                offset += node.text_len as usize;
            }
            GreenElement::Token(token) => {
                let end = offset + token.text.len();
                output.push(SyntaxToken {
                    green: token,
                    range: TextRange::new(offset, end),
                });
                offset = end;
            }
        }
    }
}

#[derive(Clone, Debug)]
enum Event {
    Start(NodeKind),
    Token(usize),
    Missing(TokenKind),
    Finish,
}

pub(crate) fn parse(source: &SourceSnapshot) -> ParseResult {
    let (tokens, diagnostics) = lex(source);
    Parser::new(source, tokens, diagnostics).parse()
}

fn lex(source: &SourceSnapshot) -> (Vec<LexedToken>, Vec<Diagnostic>) {
    let text = source.text();
    let bytes = text.as_bytes();
    let mut tokens = Vec::with_capacity(text.len() / 3);
    let mut diagnostics = Vec::new();
    let mut offset = 0;

    let emit = |kind: TokenKind, start: usize, end: usize, tokens: &mut Vec<LexedToken>| {
        tokens.push(LexedToken {
            kind,
            text: Arc::from(&text[start..end]),
            range: TextRange::new(start, end),
        });
    };

    while offset < bytes.len() {
        let start = offset;
        match bytes[offset] {
            b' ' | b'\t' => {
                offset += 1;
                while offset < bytes.len() && matches!(bytes[offset], b' ' | b'\t') {
                    offset += 1;
                }
                emit(TokenKind::Whitespace, start, offset, &mut tokens);
            }
            b'\n' => {
                offset += 1;
                emit(TokenKind::Newline, start, offset, &mut tokens);
            }
            b'\r' => {
                offset += 1;
                if offset < bytes.len() && bytes[offset] == b'\n' {
                    offset += 1;
                }
                emit(TokenKind::Newline, start, offset, &mut tokens);
            }
            b'/' if offset + 1 < bytes.len() && bytes[offset + 1] == b'/' => {
                offset += 2;
                while offset < bytes.len() && !matches!(bytes[offset], b'\n' | b'\r') {
                    offset += 1;
                }
                emit(TokenKind::Comment, start, offset, &mut tokens);
            }
            b'-' if offset + 1 < bytes.len() && bytes[offset + 1] == b'>' => {
                offset += 2;
                emit(TokenKind::Arrow, start, offset, &mut tokens);
            }
            b'{' | b'}' | b'(' | b')' | b'[' | b']' | b':' | b',' | b'.' | b'/' | b'<' | b'>'
            | b'+' => {
                let kind = match bytes[offset] {
                    b'{' => TokenKind::LeftBrace,
                    b'}' => TokenKind::RightBrace,
                    b'(' => TokenKind::LeftParen,
                    b')' => TokenKind::RightParen,
                    b'[' => TokenKind::LeftBracket,
                    b']' => TokenKind::RightBracket,
                    b':' => TokenKind::Colon,
                    b',' => TokenKind::Comma,
                    b'.' => TokenKind::Dot,
                    b'/' => TokenKind::Slash,
                    b'<' => TokenKind::Less,
                    b'>' => TokenKind::Greater,
                    b'+' => TokenKind::Plus,
                    _ => unreachable!(),
                };
                offset += 1;
                emit(kind, start, offset, &mut tokens);
            }
            b'0'..=b'9' => {
                offset += 1;
                while offset < bytes.len()
                    && (bytes[offset].is_ascii_digit() || bytes[offset] == b'_')
                {
                    offset += 1;
                }
                emit(TokenKind::Integer, start, offset, &mut tokens);
            }
            b'"' => {
                offset += 1;
                let mut closed = false;
                while offset < bytes.len() {
                    if bytes[offset] == b'\\' && offset + 1 < bytes.len() {
                        offset += 2;
                    } else if bytes[offset] == b'"' {
                        offset += 1;
                        closed = true;
                        break;
                    } else if matches!(bytes[offset], b'\n' | b'\r') {
                        break;
                    } else {
                        offset += text[offset..]
                            .chars()
                            .next()
                            .expect("offset is before source end")
                            .len_utf8();
                    }
                }
                let kind = if closed {
                    TokenKind::Text
                } else {
                    diagnostics.push(Diagnostic::error(
                        DiagnosticStage::Syntax,
                        "S1001",
                        source.file_id(),
                        TextRange::new(start, offset),
                        "unterminated text literal",
                    ));
                    TokenKind::InvalidText
                };
                emit(kind, start, offset, &mut tokens);
            }
            byte if is_identifier_start(byte) => {
                offset += 1;
                while offset < bytes.len() && is_identifier_continue(bytes[offset]) {
                    offset += 1;
                }
                let kind = match &text[start..offset] {
                    "import" => TokenKind::Import,
                    "as" => TokenKind::As,
                    "struct" => TokenKind::Struct,
                    "enum" => TokenKind::Enum,
                    "interface" => TokenKind::Interface,
                    "pub" => TokenKind::Pub,
                    "fn" => TokenKind::Fn,
                    "errors" => TokenKind::Errors,
                    "needs" => TokenKind::Needs,
                    "Void" => TokenKind::Void,
                    _ => TokenKind::Identifier,
                };
                emit(kind, start, offset, &mut tokens);
            }
            _ => {
                let character = text[offset..]
                    .chars()
                    .next()
                    .expect("offset is before source end");
                offset += character.len_utf8();
                emit(TokenKind::Invalid, start, offset, &mut tokens);
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

    tokens.push(LexedToken {
        kind: TokenKind::End,
        text: Arc::from(""),
        range: TextRange::empty(text.len()),
    });
    (tokens, diagnostics)
}

const fn is_identifier_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_lowercase() || byte.is_ascii_uppercase()
}

const fn is_identifier_continue(byte: u8) -> bool {
    is_identifier_start(byte) || byte.is_ascii_digit()
}

struct Parser<'source> {
    source: &'source SourceSnapshot,
    tokens: Vec<LexedToken>,
    significant: Vec<usize>,
    position: usize,
    next_raw: usize,
    events: Vec<Event>,
    diagnostics: Vec<Diagnostic>,
}

impl<'source> Parser<'source> {
    fn new(
        source: &'source SourceSnapshot,
        tokens: Vec<LexedToken>,
        diagnostics: Vec<Diagnostic>,
    ) -> Self {
        let significant = tokens
            .iter()
            .enumerate()
            .filter_map(|(index, token)| (!token.kind.is_trivia()).then_some(index))
            .collect();
        Self {
            source,
            tokens,
            significant,
            position: 0,
            next_raw: 0,
            events: Vec::new(),
            diagnostics,
        }
    }

    fn parse(mut self) -> ParseResult {
        self.start(NodeKind::File);
        self.skip_newlines();
        while !self.at(TokenKind::End) {
            let before = self.position;
            match self.kind() {
                TokenKind::Import => self.parse_import(),
                TokenKind::Pub
                | TokenKind::Struct
                | TokenKind::Enum
                | TokenKind::Interface
                | TokenKind::Fn => self.parse_declaration(),
                _ => self.error_and_bump("expected an import or declaration"),
            }
            if self.position == before {
                self.error_and_bump("parser made no progress at file scope");
            }
            if !self.at(TokenKind::End) && !self.skip_newlines() && !self.is_top_level_start() {
                self.error_and_bump("expected a line break between top-level items");
                self.skip_newlines();
            }
        }
        self.emit_remaining();
        self.finish();

        ParseResult {
            root: sink(&self.events, &self.tokens),
            diagnostics: self.diagnostics,
        }
    }

    fn parse_import(&mut self) {
        self.start(NodeKind::ImportDeclaration);
        self.bump();
        self.expect(TokenKind::Identifier);
        while self.accept(TokenKind::Slash) {
            self.expect(TokenKind::Identifier);
        }
        if self.at(TokenKind::As) {
            self.start(NodeKind::ImportAlias);
            self.bump();
            self.expect(TokenKind::Identifier);
            self.finish();
        } else if self.at(TokenKind::Dot) {
            self.start(NodeKind::ImportMembers);
            self.bump();
            self.expect(TokenKind::LeftBrace);
            self.skip_newlines();
            if self.at(TokenKind::RightBrace) {
                self.error_at_current("member imports must name at least one type or constructor");
            } else {
                self.parse_import_member();
                while self.accept(TokenKind::Comma) {
                    self.skip_newlines();
                    if self.at(TokenKind::RightBrace) {
                        break;
                    }
                    self.parse_import_member();
                }
            }
            self.skip_newlines();
            self.expect(TokenKind::RightBrace);
            self.finish();
        } else {
            self.error_at_current(
                "module imports require an explicit alias with `as` or an explicit member list",
            );
        }
        self.finish();
    }

    fn parse_import_member(&mut self) {
        self.start(NodeKind::ImportMember);
        self.expect(TokenKind::Identifier);
        self.finish();
    }

    fn parse_declaration(&mut self) {
        match self.declaration_kind() {
            Some((NodeKind::StructDeclaration, TokenKind::Struct)) => {
                self.parse_record_declaration(NodeKind::StructDeclaration, TokenKind::Struct)
            }
            Some((NodeKind::InterfaceDeclaration, TokenKind::Interface)) => {
                self.parse_record_declaration(NodeKind::InterfaceDeclaration, TokenKind::Interface)
            }
            Some((NodeKind::EnumDeclaration, TokenKind::Enum)) => self.parse_enum(),
            Some((NodeKind::FunctionDeclaration, TokenKind::Fn)) => self.parse_function(),
            _ => self.error_and_bump("expected `struct`, `enum`, `interface`, or `fn` after `pub`"),
        }
    }

    fn declaration_kind(&self) -> Option<(NodeKind, TokenKind)> {
        let mut index = self.position;
        if self.kind_at(index) == TokenKind::Pub {
            index += 1;
            while self.kind_at(index) == TokenKind::Newline {
                index += 1;
            }
        }
        match self.kind_at(index) {
            TokenKind::Struct => Some((NodeKind::StructDeclaration, TokenKind::Struct)),
            TokenKind::Enum => Some((NodeKind::EnumDeclaration, TokenKind::Enum)),
            TokenKind::Interface => Some((NodeKind::InterfaceDeclaration, TokenKind::Interface)),
            TokenKind::Fn => Some((NodeKind::FunctionDeclaration, TokenKind::Fn)),
            _ => None,
        }
    }

    fn parse_record_declaration(&mut self, node_kind: NodeKind, keyword: TokenKind) {
        self.start(node_kind);
        self.parse_visibility();
        self.expect(keyword);
        self.expect(TokenKind::Identifier);
        self.parse_optional_generic_parameters();
        self.skip_newlines();
        self.expect(TokenKind::LeftBrace);
        self.skip_newlines();
        while !self.at(TokenKind::RightBrace) && !self.at(TokenKind::End) {
            let before = self.position;
            self.parse_field();
            if self.accept(TokenKind::Comma) {
                self.skip_newlines();
            } else if !self.at(TokenKind::RightBrace) && !self.skip_newlines() {
                self.error_and_bump("expected a line break or comma between fields");
            }
            if self.position == before {
                self.error_and_bump("parser made no progress in declaration body");
            }
        }
        self.expect(TokenKind::RightBrace);
        self.finish();
    }

    fn parse_field(&mut self) {
        self.start(NodeKind::Field);
        self.expect(TokenKind::Identifier);
        self.skip_newlines();
        self.expect(TokenKind::Colon);
        self.skip_newlines();
        self.parse_type();
        self.finish();
    }

    fn parse_enum(&mut self) {
        self.start(NodeKind::EnumDeclaration);
        self.parse_visibility();
        self.expect(TokenKind::Enum);
        self.expect(TokenKind::Identifier);
        self.parse_optional_generic_parameters();
        self.skip_newlines();
        self.expect(TokenKind::LeftBrace);
        self.skip_newlines();
        while !self.at(TokenKind::RightBrace) && !self.at(TokenKind::End) {
            let before = self.position;
            self.start(NodeKind::Variant);
            self.expect(TokenKind::Identifier);
            if self.accept(TokenKind::LeftParen) {
                self.skip_newlines();
                if !self.at(TokenKind::RightParen) {
                    self.parse_named_type_field();
                    while self.accept(TokenKind::Comma) {
                        self.skip_newlines();
                        if self.at(TokenKind::RightParen) {
                            break;
                        }
                        self.parse_named_type_field();
                    }
                }
                self.skip_newlines();
                self.expect(TokenKind::RightParen);
            }
            self.finish();
            if self.accept(TokenKind::Comma) {
                self.skip_newlines();
            } else if !self.at(TokenKind::RightBrace) && !self.skip_newlines() {
                self.error_and_bump("expected a line break or comma between variants");
            }
            if self.position == before {
                self.error_and_bump("parser made no progress in enum body");
            }
        }
        self.expect(TokenKind::RightBrace);
        self.finish();
    }

    fn parse_optional_generic_parameters(&mut self) {
        if !self.at(TokenKind::Less) {
            return;
        }
        self.start(NodeKind::GenericParameters);
        self.bump();
        self.skip_newlines();
        self.parse_generic_parameter();
        while self.accept(TokenKind::Comma) {
            self.skip_newlines();
            self.parse_generic_parameter();
        }
        self.skip_newlines();
        self.expect(TokenKind::Greater);
        self.finish();
    }

    fn parse_generic_parameter(&mut self) {
        self.start(NodeKind::GenericParameter);
        self.expect(TokenKind::Identifier);
        self.finish();
    }

    fn parse_named_type_field(&mut self) {
        self.start(NodeKind::NamedTypeField);
        self.expect(TokenKind::Identifier);
        self.skip_newlines();
        self.expect(TokenKind::Colon);
        self.skip_newlines();
        self.parse_type();
        self.finish();
    }

    fn parse_function(&mut self) {
        self.start(NodeKind::FunctionDeclaration);
        self.parse_visibility();
        self.expect(TokenKind::Fn);
        self.expect(TokenKind::Identifier);
        self.parse_optional_generic_parameters();
        self.skip_newlines();
        self.parse_value_parameters();
        self.parse_implementation_contract();
        self.skip_newlines();
        self.parse_m1_function_body();
        self.finish();
    }

    fn parse_visibility(&mut self) {
        if self.accept(TokenKind::Pub) {
            self.skip_newlines();
        }
    }

    fn parse_value_parameters(&mut self) {
        self.start(NodeKind::ValueParameters);
        self.expect(TokenKind::LeftParen);
        self.skip_newlines();
        if !self.at(TokenKind::RightParen) {
            self.parse_value_parameter();
            while self.accept(TokenKind::Comma) {
                self.skip_newlines();
                if self.at(TokenKind::RightParen) {
                    break;
                }
                self.parse_value_parameter();
            }
        }
        self.skip_newlines();
        self.expect(TokenKind::RightParen);
        self.finish();
    }

    fn parse_value_parameter(&mut self) {
        self.start(NodeKind::ValueParameter);
        self.expect(TokenKind::Identifier);
        if self.accept(TokenKind::Colon) {
            self.skip_newlines();
            self.parse_type();
        }
        self.finish();
    }

    fn parse_implementation_contract(&mut self) {
        self.start(NodeKind::ImplementationContract);
        self.skip_newlines();
        if self.accept(TokenKind::Arrow) {
            self.skip_newlines();
            self.parse_type();
            self.skip_newlines();
        }
        if self.at(TokenKind::Needs) {
            self.parse_requirement_clause();
        }
        self.finish();
    }

    fn parse_type(&mut self) {
        self.start(NodeKind::Type);
        self.skip_newlines();
        if self.at(TokenKind::Fn) {
            self.parse_function_type();
        } else if self.accept(TokenKind::Identifier) || self.accept(TokenKind::Void) {
            while self.accept(TokenKind::Dot) {
                self.expect(TokenKind::Identifier);
            }
            self.parse_optional_type_arguments();
        } else {
            self.missing(TokenKind::Identifier, "type");
        }
        self.finish();
    }

    fn parse_function_type(&mut self) {
        self.start(NodeKind::FunctionType);
        self.bump();
        self.skip_newlines();
        self.expect(TokenKind::LeftParen);
        self.skip_newlines();
        if !self.at(TokenKind::RightParen) {
            self.parse_type();
            while self.accept(TokenKind::Comma) {
                self.skip_newlines();
                if self.at(TokenKind::RightParen) {
                    break;
                }
                self.parse_type();
            }
        }
        self.skip_newlines();
        self.expect(TokenKind::RightParen);
        self.skip_newlines();
        self.expect(TokenKind::Arrow);
        self.skip_newlines();
        self.parse_type();
        self.skip_newlines();
        if self.accept(TokenKind::Errors) {
            self.skip_newlines();
            self.parse_type();
            self.skip_newlines();
        }
        if self.at(TokenKind::Needs) {
            self.parse_requirement_clause();
        }
        self.finish();
    }

    fn parse_optional_type_arguments(&mut self) {
        if !self.at(TokenKind::Less) {
            return;
        }
        self.start(NodeKind::TypeArguments);
        self.bump();
        self.skip_newlines();
        self.parse_type();
        while self.accept(TokenKind::Comma) {
            self.skip_newlines();
            self.parse_type();
        }
        self.skip_newlines();
        self.expect(TokenKind::Greater);
        self.finish();
    }

    fn parse_requirement_clause(&mut self) {
        self.start(NodeKind::RequirementClause);
        self.expect(TokenKind::Needs);
        self.skip_newlines();
        self.parse_requirement_atom();
        while self.accept_after_newlines(TokenKind::Plus) {
            self.skip_newlines();
            self.parse_requirement_atom();
        }
        self.finish();
    }

    fn parse_requirement_atom(&mut self) {
        if self.accept(TokenKind::Identifier) {
            return;
        }
        self.start(NodeKind::RequirementRecord);
        self.expect(TokenKind::LeftBrace);
        self.skip_newlines();
        if self.at(TokenKind::RightBrace) {
            self.error_at_current("empty requirement records are invalid");
        } else {
            self.parse_named_type_field();
            while self.accept(TokenKind::Comma) {
                self.skip_newlines();
                if self.at(TokenKind::RightBrace) {
                    break;
                }
                self.parse_named_type_field();
            }
        }
        self.skip_newlines();
        self.expect(TokenKind::RightBrace);
        self.finish();
    }

    fn parse_m1_function_body(&mut self) {
        self.expect(TokenKind::LeftBrace);
        self.skip_newlines();
        self.start(NodeKind::ConstructionExpression);
        self.expect(TokenKind::Identifier);
        self.expect(TokenKind::LeftParen);
        self.expect(TokenKind::Identifier);
        self.expect(TokenKind::Colon);
        if matches!(self.kind(), TokenKind::Integer | TokenKind::Text) {
            self.bump();
        } else {
            self.missing(TokenKind::Integer, "integer or text literal");
        }
        self.expect(TokenKind::RightParen);
        self.finish();
        self.skip_newlines();
        self.expect(TokenKind::RightBrace);
    }

    fn is_top_level_start(&self) -> bool {
        matches!(
            self.kind(),
            TokenKind::Import
                | TokenKind::Pub
                | TokenKind::Struct
                | TokenKind::Enum
                | TokenKind::Interface
                | TokenKind::Fn
        )
    }

    fn kind(&self) -> TokenKind {
        self.current().kind
    }

    fn current(&self) -> &LexedToken {
        &self.tokens[self.significant[self.position]]
    }

    fn kind_at(&self, position: usize) -> TokenKind {
        self.significant
            .get(position)
            .map_or(TokenKind::End, |index| self.tokens[*index].kind)
    }

    fn at(&self, kind: TokenKind) -> bool {
        self.kind() == kind
    }

    fn accept(&mut self, kind: TokenKind) -> bool {
        if !self.at(kind) {
            return false;
        }
        self.bump();
        true
    }

    fn skip_newlines(&mut self) -> bool {
        let mut consumed = false;
        while self.accept(TokenKind::Newline) {
            consumed = true;
        }
        consumed
    }

    fn accept_after_newlines(&mut self, kind: TokenKind) -> bool {
        let mut position = self.position;
        while self.kind_at(position) == TokenKind::Newline {
            position += 1;
        }
        if self.kind_at(position) != kind {
            return false;
        }
        self.skip_newlines();
        self.accept(kind)
    }

    fn bump(&mut self) {
        let raw = self.significant[self.position];
        while self.next_raw <= raw {
            self.events.push(Event::Token(self.next_raw));
            self.next_raw += 1;
        }
        if self.position + 1 < self.significant.len() {
            self.position += 1;
        }
    }

    fn expect(&mut self, kind: TokenKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            self.missing(kind, kind.expected_name());
            false
        }
    }

    fn missing(&mut self, kind: TokenKind, expected: &str) {
        self.emit_trivia_before_current();
        self.diagnostics.push(Diagnostic::error(
            DiagnosticStage::Syntax,
            "S1001",
            self.source.file_id(),
            TextRange::empty(self.current().range.start() as usize),
            format!("expected {expected}"),
        ));
        self.events.push(Event::Missing(kind));
    }

    fn emit_trivia_before_current(&mut self) {
        let current_raw = self.significant[self.position];
        while self.next_raw < current_raw {
            self.events.push(Event::Token(self.next_raw));
            self.next_raw += 1;
        }
    }

    fn error_at_current(&mut self, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::error(
            DiagnosticStage::Syntax,
            "S1001",
            self.source.file_id(),
            self.current().range,
            message,
        ));
    }

    fn error_and_bump(&mut self, message: impl Into<String>) {
        self.start(NodeKind::Error);
        self.error_at_current(message);
        if !self.at(TokenKind::End) {
            self.bump();
        }
        self.finish();
    }

    fn start(&mut self, kind: NodeKind) {
        self.events.push(Event::Start(kind));
    }

    fn finish(&mut self) {
        self.events.push(Event::Finish);
    }

    fn emit_remaining(&mut self) {
        while self.next_raw < self.tokens.len() {
            self.events.push(Event::Token(self.next_raw));
            self.next_raw += 1;
        }
    }
}

fn sink(events: &[Event], tokens: &[LexedToken]) -> Arc<GreenNode> {
    struct OpenNode {
        kind: NodeKind,
        children: Vec<GreenElement>,
        text_len: u32,
    }

    let mut stack: Vec<OpenNode> = Vec::new();
    let mut root = None;
    for event in events {
        match *event {
            Event::Start(kind) => stack.push(OpenNode {
                kind,
                children: Vec::new(),
                text_len: 0,
            }),
            Event::Token(index) => {
                let token = &tokens[index];
                let open = stack.last_mut().expect("tokens belong to an open node");
                open.text_len = open
                    .text_len
                    .checked_add(
                        u32::try_from(token.text.len())
                            .expect("syntax token lengths fit in the source offset type"),
                    )
                    .expect("syntax node lengths fit in the source offset type");
                open.children.push(GreenElement::Token(GreenToken {
                    kind: token.kind,
                    text: token.text.clone(),
                    missing: false,
                }));
            }
            Event::Missing(kind) => {
                stack
                    .last_mut()
                    .expect("missing tokens belong to an open node")
                    .children
                    .push(GreenElement::Token(GreenToken {
                        kind,
                        text: Arc::from(""),
                        missing: true,
                    }));
            }
            Event::Finish => {
                let open = stack.pop().expect("finish closes an open node");
                let node = Arc::new(GreenNode {
                    kind: open.kind,
                    children: open.children.into_boxed_slice(),
                    text_len: open.text_len,
                });
                if let Some(parent) = stack.last_mut() {
                    parent.text_len = parent
                        .text_len
                        .checked_add(node.text_len)
                        .expect("syntax node lengths fit in the source offset type");
                    parent.children.push(GreenElement::Node(node));
                } else {
                    assert!(root.replace(node).is_none(), "event stream has one root");
                }
            }
        }
    }
    assert!(stack.is_empty(), "event stream closes every node");
    root.expect("event stream produces a root")
}

#[cfg(test)]
mod tests {
    use super::{NodeKind, parse};
    use crate::source::SourceSnapshot;

    #[test]
    fn valid_source_round_trips_with_every_trivia_byte() {
        let source = "// answer\r\nstruct Answer {\n  value: Int // cents\n}\n\npub fn main() {\n  Answer(value: 42)\n}\n";
        let snapshot = SourceSnapshot::from_text("round-trip.chimi", source);
        let result = parse(&snapshot);

        assert!(result.diagnostics().is_empty());
        assert_eq!(result.round_trip(), source);
        assert_eq!(result.root().kind(), NodeKind::File);
    }

    #[test]
    fn missing_closing_brace_is_zero_width_and_source_still_round_trips() {
        let source = include_str!("../tests/fixtures/missing_closing_brace.chimi");
        let snapshot = SourceSnapshot::from_text("missing-closing-brace.chimi", source);
        let result = parse(&snapshot);

        assert_eq!(result.round_trip(), source);
        assert_eq!(result.diagnostics().len(), 1);
        assert_eq!(result.diagnostics()[0].message(), "expected `}`");
        let missing = result
            .root()
            .descendant_tokens()
            .into_iter()
            .filter(|token| token.is_missing())
            .collect::<Vec<_>>();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].range().start() as usize, source.len());
    }

    #[test]
    fn invalid_unicode_is_preserved_and_parsing_terminates() {
        let source = "struct Answer { value: Int } pub fn main() { Answer(value: λ) }";
        let snapshot = SourceSnapshot::from_text("unicode.chimi", source);
        let result = parse(&snapshot);

        assert_eq!(result.round_trip(), source);
        assert!(!result.diagnostics().is_empty());
    }

    #[test]
    fn declarations_and_type_contracts_round_trip_without_diagnostics() {
        let source = include_str!("../tests/fixtures/declarations_and_types.chimi");
        let snapshot = SourceSnapshot::from_text("declarations-and-types.chimi", source);
        let result = parse(&snapshot);

        assert_eq!(result.round_trip(), source);
        assert_eq!(result.diagnostics(), []);
        let root_kinds = result
            .root()
            .child_nodes()
            .into_iter()
            .map(|node| node.kind())
            .collect::<Vec<_>>();
        assert_eq!(
            root_kinds,
            [
                NodeKind::ImportDeclaration,
                NodeKind::ImportDeclaration,
                NodeKind::StructDeclaration,
                NodeKind::EnumDeclaration,
                NodeKind::InterfaceDeclaration,
                NodeKind::FunctionDeclaration,
            ]
        );
    }

    #[test]
    fn missing_generic_delimiter_recovers_into_the_following_body() {
        let source = "struct Box<Value {\n  value: Value\n}\n";
        let snapshot = SourceSnapshot::from_text("missing-generic-close.chimi", source);
        let result = parse(&snapshot);

        assert_eq!(result.round_trip(), source);
        assert_eq!(result.diagnostics().len(), 1);
        assert_eq!(result.diagnostics()[0].message(), "expected `>`");
        assert_eq!(
            result
                .root()
                .descendant_tokens()
                .into_iter()
                .filter(|token| token.is_missing())
                .count(),
            1
        );
    }

    #[test]
    fn empty_import_and_requirement_shapes_are_rejected_without_losing_source() {
        let cases = [
            (
                "import modules/money.{}\n",
                "member imports must name at least one type or constructor",
            ),
            (
                "interface Service { call: fn() -> Void needs {} }\n",
                "empty requirement records are invalid",
            ),
        ];

        for (source, expected_message) in cases {
            let snapshot = SourceSnapshot::from_text("empty-shape.chimi", source);
            let result = parse(&snapshot);

            assert_eq!(result.round_trip(), source);
            assert_eq!(result.diagnostics().len(), 1);
            assert_eq!(result.diagnostics()[0].message(), expected_message);
        }
    }
}
