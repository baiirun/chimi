use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::{SyntaxId, TextRange};
use crate::source::SourceSnapshot;
use std::sync::Arc;

mod typed;
pub(crate) use typed::{
    DeclarationSyntax, FileSyntax, FunctionDeclarationSyntax, StructDeclarationSyntax,
    TypedSyntaxNode,
};

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
    Let,
    Const,
    Case,
    Loop,
    Break,
    If,
    Errors,
    Needs,
    Using,
    And,
    Or,
    Not,
    Void,
    True,
    False,
    Identifier,
    Integer,
    Float,
    Text,
    /// `"` and the text before an interpolated literal's first `${`.
    TextStart,
    /// The text between an interpolation's `}` and the next `${`.
    TextMiddle,
    /// The text after an interpolated literal's last `}`, with the closing `"`.
    TextEnd,
    /// `${`, which opens an interpolation inside a text literal.
    InterpolationOpen,
    /// The `}` that closes an interpolation.
    InterpolationClose,
    LeftBrace,
    RightBrace,
    LeftParen,
    RightParen,
    LeftBracket,
    RightBracket,
    Colon,
    Comma,
    Dot,
    /// `..`: the half-open range operator, or the rest marker inside a list pattern.
    DotDot,
    /// `..=`, the inclusive range operator.
    DotDotEqual,
    Slash,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Plus,
    Minus,
    Star,
    Percent,
    Equal,
    EqualEqual,
    BangEqual,
    Arrow,
    Pipe,
    /// `&&`, recognized only to report `S1004`; the parser treats it as `and`.
    AmpersandAmpersand,
    /// `||`, recognized only to report `S1004`; the parser treats it as `or`.
    BarBar,
    /// A `!` not followed by `=`, recognized only to report `S1004`; the parser treats it as `not`.
    Bang,
    Invalid,
    InvalidText,
    End,
}

impl TokenKind {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Whitespace => "whitespace",
            Self::Newline => "newline",
            Self::Comment => "comment",
            Self::Import => "import",
            Self::As => "as",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Interface => "interface",
            Self::Pub => "pub",
            Self::Fn => "fn",
            Self::Let => "let",
            Self::Const => "const",
            Self::Case => "case",
            Self::Loop => "loop",
            Self::Break => "break",
            Self::If => "if",
            Self::Errors => "errors",
            Self::Needs => "needs",
            Self::Using => "using",
            Self::And => "and",
            Self::Or => "or",
            Self::Not => "not",
            Self::Void => "void",
            Self::True => "true",
            Self::False => "false",
            Self::Identifier => "identifier",
            Self::Integer => "integer",
            Self::Float => "float",
            Self::Text => "text",
            Self::TextStart => "text_start",
            Self::TextMiddle => "text_middle",
            Self::TextEnd => "text_end",
            Self::InterpolationOpen => "interpolation_open",
            Self::InterpolationClose => "interpolation_close",
            Self::LeftBrace => "left_brace",
            Self::RightBrace => "right_brace",
            Self::LeftParen => "left_paren",
            Self::RightParen => "right_paren",
            Self::LeftBracket => "left_bracket",
            Self::RightBracket => "right_bracket",
            Self::Colon => "colon",
            Self::Comma => "comma",
            Self::Dot => "dot",
            Self::DotDot => "dot_dot",
            Self::DotDotEqual => "dot_dot_equal",
            Self::Slash => "slash",
            Self::Less => "less",
            Self::LessEqual => "less_equal",
            Self::Greater => "greater",
            Self::GreaterEqual => "greater_equal",
            Self::Plus => "plus",
            Self::Minus => "minus",
            Self::Star => "star",
            Self::Percent => "percent",
            Self::Equal => "equal",
            Self::EqualEqual => "equal_equal",
            Self::BangEqual => "bang_equal",
            Self::Arrow => "arrow",
            Self::Pipe => "pipe",
            Self::AmpersandAmpersand => "ampersand_ampersand",
            Self::BarBar => "bar_bar",
            Self::Bang => "bang",
            Self::Invalid => "invalid",
            Self::InvalidText => "invalid_text",
            Self::End => "end",
        }
    }

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
            Self::Let => "`let`",
            Self::Const => "`const`",
            Self::Case => "`case`",
            Self::Loop => "`loop`",
            Self::Break => "`break`",
            Self::If => "`if`",
            Self::Errors => "`errors`",
            Self::Needs => "`needs`",
            Self::Using => "`using`",
            Self::And => "`and`",
            Self::Or => "`or`",
            Self::Not => "`not`",
            Self::Void => "`Void`",
            Self::True => "`True`",
            Self::False => "`False`",
            Self::Identifier => "identifier",
            Self::Integer => "integer literal",
            Self::Float => "float literal",
            Self::Text | Self::TextStart => "text literal",
            Self::TextMiddle | Self::TextEnd => "the rest of the text literal",
            Self::InterpolationOpen => "`${`",
            Self::InterpolationClose => "`}` closing the interpolation",
            Self::LeftBrace => "`{`",
            Self::RightBrace => "`}`",
            Self::LeftParen => "`(`",
            Self::RightParen => "`)`",
            Self::LeftBracket => "`[`",
            Self::RightBracket => "`]`",
            Self::Colon => "`:`",
            Self::Comma => "`,`",
            Self::Dot => "`.`",
            Self::DotDot => "`..`",
            Self::DotDotEqual => "`..=`",
            Self::Slash => "`/`",
            Self::Less => "`<`",
            Self::LessEqual => "`<=`",
            Self::Greater => "`>`",
            Self::GreaterEqual => "`>=`",
            Self::Plus => "`+`",
            Self::Minus => "`-`",
            Self::Star => "`*`",
            Self::Percent => "`%`",
            Self::Equal => "`=`",
            Self::EqualEqual => "`==`",
            Self::BangEqual => "`!=`",
            Self::Arrow => "`->`",
            Self::Pipe => "`|>`",
            Self::AmpersandAmpersand => "`&&`",
            Self::BarBar => "`||`",
            Self::Bang => "`!`",
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
    /// `(T1, T2, ...)` inside a `Type`.
    TupleType,
    TypeArguments,
    RequirementClause,
    RequirementRecord,
    LetBinding,
    ConstDeclaration,
    /// A text literal with `${...}` interpolations: text chunk tokens around
    /// `InterpolationOpen` expression `InterpolationClose` groups.
    InterpolatedText,
    UsingExpression,
    OrExpression,
    AndExpression,
    NotExpression,
    PipelineExpression,
    ComparisonExpression,
    AdditiveExpression,
    MultiplicativeExpression,
    UnaryExpression,
    PostfixExpression,
    Literal,
    NameReference,
    GroupedExpression,
    /// `(e1, e2, ...)`: two or more comma-separated elements in parentheses.
    TupleExpression,
    /// `start..end` or `start..=end`.
    RangeExpression,
    ListExpression,
    RecordExpression,
    RecordField,
    BlockExpression,
    BlockItem,
    CaseExpression,
    CaseArm,
    /// `if <guard>` between a function clause's parameters or contract and its body.
    ClauseGuard,
    /// `..base` leading a record update `{ ..base, field: value }`.
    RecordSpread,
    LoopExpression,
    BreakExpression,
    Pattern,
    FunctionExpression,
    CallSuffix,
    Argument,
    MemberAccess,
    EmptyBraceExpression,
    ErrorIfExpression,
    Error,
}

impl NodeKind {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::ImportDeclaration => "import_declaration",
            Self::ImportAlias => "import_alias",
            Self::ImportMembers => "import_members",
            Self::ImportMember => "import_member",
            Self::StructDeclaration => "struct_declaration",
            Self::EnumDeclaration => "enum_declaration",
            Self::InterfaceDeclaration => "interface_declaration",
            Self::Field => "field",
            Self::Variant => "variant",
            Self::NamedTypeField => "named_type_field",
            Self::GenericParameters => "generic_parameters",
            Self::GenericParameter => "generic_parameter",
            Self::FunctionDeclaration => "function_declaration",
            Self::ValueParameters => "value_parameters",
            Self::ValueParameter => "value_parameter",
            Self::ImplementationContract => "implementation_contract",
            Self::Type => "type",
            Self::FunctionType => "function_type",
            Self::TupleType => "tuple_type",
            Self::TypeArguments => "type_arguments",
            Self::RequirementClause => "requirement_clause",
            Self::RequirementRecord => "requirement_record",
            Self::LetBinding => "let_binding",
            Self::ConstDeclaration => "const_declaration",
            Self::InterpolatedText => "interpolated_text",
            Self::UsingExpression => "using_expression",
            Self::OrExpression => "or_expression",
            Self::AndExpression => "and_expression",
            Self::NotExpression => "not_expression",
            Self::PipelineExpression => "pipeline_expression",
            Self::ComparisonExpression => "comparison_expression",
            Self::AdditiveExpression => "additive_expression",
            Self::MultiplicativeExpression => "multiplicative_expression",
            Self::UnaryExpression => "unary_expression",
            Self::PostfixExpression => "postfix_expression",
            Self::Literal => "literal",
            Self::NameReference => "name_reference",
            Self::GroupedExpression => "grouped_expression",
            Self::TupleExpression => "tuple_expression",
            Self::RangeExpression => "range_expression",
            Self::ListExpression => "list_expression",
            Self::RecordExpression => "record_expression",
            Self::RecordField => "record_field",
            Self::BlockExpression => "block_expression",
            Self::BlockItem => "block_item",
            Self::CaseExpression => "case_expression",
            Self::CaseArm => "case_arm",
            Self::ClauseGuard => "clause_guard",
            Self::RecordSpread => "record_spread",
            Self::LoopExpression => "loop_expression",
            Self::BreakExpression => "break_expression",
            Self::Pattern => "pattern",
            Self::FunctionExpression => "function_expression",
            Self::CallSuffix => "call_suffix",
            Self::Argument => "argument",
            Self::MemberAccess => "member_access",
            Self::EmptyBraceExpression => "empty_brace_expression",
            Self::ErrorIfExpression => "error_if_expression",
            Self::Error => "error",
        }
    }
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
    element_count: u32,
}

impl GreenNode {
    #[cfg(any(test, feature = "frontend-benchmarks"))]
    pub(crate) fn text(&self) -> String {
        let mut text = String::with_capacity(self.text_len as usize);
        self.write_text(&mut text);
        text
    }

    #[cfg(any(test, feature = "frontend-benchmarks"))]
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
            id: SyntaxId::new(0),
            ancestors: Vec::new(),
        }
    }

    pub(crate) fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Resolves an identity created by this parse snapshot.
    #[allow(dead_code)]
    pub(crate) fn element(&self, id: SyntaxId) -> Option<SyntaxElement<'_>> {
        find_element(self.root(), id)
    }

    #[cfg(any(test, feature = "frontend-benchmarks"))]
    fn round_trip(&self) -> String {
        self.root.text()
    }
}

#[cfg(any(test, feature = "frontend-benchmarks"))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct SyntaxStatistics {
    pub(crate) node_count: usize,
    pub(crate) token_count: usize,
    pub(crate) missing_token_count: usize,
}

#[cfg(any(test, feature = "frontend-benchmarks"))]
pub(crate) fn validate_parse(
    parsed: &ParseResult,
    source: &SourceSnapshot,
) -> Result<SyntaxStatistics, String> {
    use std::collections::BTreeSet;

    if parsed.round_trip() != source.text() {
        return Err("CST leaves do not reproduce the source".to_owned());
    }
    for diagnostic in parsed.diagnostics() {
        if diagnostic.range().start() > diagnostic.range().end()
            || diagnostic.range().end() as usize > source.text().len()
        {
            return Err("diagnostic range lies outside the source".to_owned());
        }
    }

    fn visit(
        parsed: &ParseResult,
        node: &SyntaxNode<'_>,
        source_len: usize,
        ids: &mut BTreeSet<SyntaxId>,
        statistics: &mut SyntaxStatistics,
    ) -> Result<(), String> {
        if node.range().end() as usize > source_len || !ids.insert(node.id()) {
            return Err("node range or SyntaxId is invalid".to_owned());
        }
        if !matches!(parsed.element(node.id()), Some(SyntaxElement::Node(found)) if found.id() == node.id())
        {
            return Err("node SyntaxId does not resolve to the same node".to_owned());
        }
        statistics.node_count += 1;

        for token in node.child_tokens() {
            if token.range().start() > token.range().end()
                || token.range().end() as usize > source_len
                || !ids.insert(token.id())
            {
                return Err("token range or SyntaxId is invalid".to_owned());
            }
            if token.parent().id() != node.id()
                || !matches!(parsed.element(token.id()), Some(SyntaxElement::Token(found)) if found.id() == token.id())
            {
                return Err("token cursor context or SyntaxId resolution is invalid".to_owned());
            }
            statistics.token_count += 1;
            statistics.missing_token_count += usize::from(token.is_missing());
        }
        for child in node.child_nodes() {
            if child.parent().map(|parent| parent.id()) != Some(node.id()) {
                return Err("node parent navigation is invalid".to_owned());
            }
            visit(parsed, &child, source_len, ids, statistics)?;
        }
        Ok(())
    }

    let root = parsed.root();
    if root.range() != TextRange::new(0, source.text().len()) || root.parent().is_some() {
        return Err("root cursor range or parent is invalid".to_owned());
    }
    let mut ids = BTreeSet::new();
    let mut statistics = SyntaxStatistics {
        node_count: 0,
        token_count: 0,
        missing_token_count: 0,
    };
    visit(
        parsed,
        &root,
        source.text().len(),
        &mut ids,
        &mut statistics,
    )?;
    if statistics.node_count + statistics.token_count != parsed.root.element_count as usize {
        return Err("green subtree element count is inconsistent".to_owned());
    }
    Ok(statistics)
}

#[derive(Clone, Debug)]
pub(crate) struct SyntaxNode<'tree> {
    green: &'tree GreenNode,
    offset: usize,
    id: SyntaxId,
    ancestors: Vec<SyntaxAncestor<'tree>>,
}

// These cursor frames are consumed by formatter/editor tooling in later milestones.
// The compiler currently exercises the same cursor through typed lowering.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
struct SyntaxAncestor<'tree> {
    green: &'tree GreenNode,
    offset: usize,
    id: SyntaxId,
}

impl<'tree> SyntaxNode<'tree> {
    pub(crate) const fn kind(&self) -> NodeKind {
        self.green.kind
    }

    #[allow(dead_code)]
    pub(crate) const fn id(&self) -> SyntaxId {
        self.id
    }

    #[allow(dead_code)]
    pub(crate) fn range(&self) -> TextRange {
        TextRange::new(self.offset, self.offset + self.green.text_len as usize)
    }

    #[allow(dead_code)]
    pub(crate) fn parent(&self) -> Option<Self> {
        let (&parent, remaining) = self.ancestors.split_last()?;
        Some(Self {
            green: parent.green,
            offset: parent.offset,
            id: parent.id,
            ancestors: remaining.to_vec(),
        })
    }

    pub(crate) fn child_nodes(&self) -> Vec<Self> {
        let mut offset = self.offset;
        let mut next_id = self.id.raw() + 1;
        let mut nodes = Vec::new();
        for child in &self.green.children {
            match child {
                GreenElement::Node(node) => {
                    let mut ancestors = self.ancestors.clone();
                    ancestors.push(SyntaxAncestor {
                        green: self.green,
                        offset: self.offset,
                        id: self.id,
                    });
                    nodes.push(Self {
                        green: node,
                        offset,
                        id: SyntaxId::new(next_id),
                        ancestors,
                    });
                    offset += node.text_len as usize;
                    next_id += node.element_count;
                }
                GreenElement::Token(token) => {
                    offset += token.text.len();
                    next_id += 1;
                }
            }
        }
        nodes
    }

    pub(crate) fn child_tokens(&self) -> Vec<SyntaxToken<'tree>> {
        let mut offset = self.offset;
        let mut next_id = self.id.raw() + 1;
        let mut tokens = Vec::new();
        for child in &self.green.children {
            match child {
                GreenElement::Node(node) => {
                    offset += node.text_len as usize;
                    next_id += node.element_count;
                }
                GreenElement::Token(token) => {
                    let end = offset + token.text.len();
                    tokens.push(SyntaxToken {
                        green: token,
                        range: TextRange::new(offset, end),
                        id: SyntaxId::new(next_id),
                        parent: self.clone(),
                    });
                    offset = end;
                    next_id += 1;
                }
            }
        }
        tokens
    }

    pub(crate) fn descendant_tokens(&self) -> Vec<SyntaxToken<'tree>> {
        let mut tokens = Vec::new();
        collect_tokens(self.clone(), &mut tokens);
        tokens
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SyntaxToken<'tree> {
    green: &'tree GreenToken,
    range: TextRange,
    #[allow(dead_code)]
    id: SyntaxId,
    #[allow(dead_code)]
    parent: SyntaxNode<'tree>,
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

    #[allow(dead_code)]
    pub(crate) const fn id(&self) -> SyntaxId {
        self.id
    }

    #[allow(dead_code)]
    pub(crate) fn parent(&self) -> SyntaxNode<'_> {
        self.parent.clone()
    }

    pub(crate) const fn is_missing(&self) -> bool {
        self.green.missing
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) enum SyntaxElement<'tree> {
    Node(SyntaxNode<'tree>),
    Token(SyntaxToken<'tree>),
}

impl SyntaxElement<'_> {
    #[allow(dead_code)]
    pub(crate) const fn id(&self) -> SyntaxId {
        match self {
            Self::Node(node) => node.id(),
            Self::Token(token) => token.id(),
        }
    }
}

fn find_element<'tree>(node: SyntaxNode<'tree>, wanted: SyntaxId) -> Option<SyntaxElement<'tree>> {
    if node.id == wanted {
        return Some(SyntaxElement::Node(node));
    }

    let mut offset = node.offset;
    let mut next_id = node.id.raw() + 1;
    for child in &node.green.children {
        match child {
            GreenElement::Node(child) => {
                let child_id_end = next_id + child.element_count;
                if wanted.raw() >= next_id && wanted.raw() < child_id_end {
                    let mut ancestors = node.ancestors.clone();
                    ancestors.push(SyntaxAncestor {
                        green: node.green,
                        offset: node.offset,
                        id: node.id,
                    });
                    return find_element(
                        SyntaxNode {
                            green: child,
                            offset,
                            id: SyntaxId::new(next_id),
                            ancestors,
                        },
                        wanted,
                    );
                }
                offset += child.text_len as usize;
                next_id = child_id_end;
            }
            GreenElement::Token(token) => {
                if wanted.raw() == next_id {
                    let end = offset + token.text.len();
                    return Some(SyntaxElement::Token(SyntaxToken {
                        green: token,
                        range: TextRange::new(offset, end),
                        id: SyntaxId::new(next_id),
                        parent: node.clone(),
                    }));
                }
                offset += token.text.len();
                next_id += 1;
            }
        }
    }
    None
}

fn collect_tokens<'tree>(node: SyntaxNode<'tree>, output: &mut Vec<SyntaxToken<'tree>>) {
    let mut offset = node.offset;
    let mut next_id = node.id.raw() + 1;
    for child in &node.green.children {
        match child {
            GreenElement::Node(child) => {
                let mut ancestors = node.ancestors.clone();
                ancestors.push(SyntaxAncestor {
                    green: node.green,
                    offset: node.offset,
                    id: node.id,
                });
                collect_tokens(
                    SyntaxNode {
                        green: child,
                        offset,
                        id: SyntaxId::new(next_id),
                        ancestors,
                    },
                    output,
                );
                offset += child.text_len as usize;
                next_id += child.element_count;
            }
            GreenElement::Token(token) => {
                let end = offset + token.text.len();
                output.push(SyntaxToken {
                    green: token,
                    range: TextRange::new(offset, end),
                    id: SyntaxId::new(next_id),
                    parent: node.clone(),
                });
                offset = end;
                next_id += 1;
            }
        }
    }
}

#[derive(Clone, Debug)]
enum Event {
    Tombstone,
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
    // One entry per open interpolation: the `{` nesting inside it, so only the `}` at
    // depth zero closes it.
    let mut interpolations: Vec<u32> = Vec::new();

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
            b'\n' | b'\r' if !interpolations.is_empty() => {
                interpolations.clear();
                diagnostics.push(Diagnostic::error(
                    DiagnosticStage::Syntax,
                    "S1101",
                    source.file_id(),
                    TextRange::empty(start),
                    "an interpolation must close with `}` on the same line",
                ));
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
            b'|' if offset + 1 < bytes.len() && bytes[offset + 1] == b'>' => {
                offset += 2;
                emit(TokenKind::Pipe, start, offset, &mut tokens);
            }
            b'&' if offset + 1 < bytes.len() && bytes[offset + 1] == b'&' => {
                offset += 2;
                diagnostics.push(symbolic_logic_diagnostic(source, start, offset, "and"));
                emit(TokenKind::AmpersandAmpersand, start, offset, &mut tokens);
            }
            b'|' if offset + 1 < bytes.len() && bytes[offset + 1] == b'|' => {
                offset += 2;
                diagnostics.push(symbolic_logic_diagnostic(source, start, offset, "or"));
                emit(TokenKind::BarBar, start, offset, &mut tokens);
            }
            b'!' if offset + 1 < bytes.len() && bytes[offset + 1] == b'=' => {
                offset += 2;
                emit(TokenKind::BangEqual, start, offset, &mut tokens);
            }
            b'=' if offset + 1 < bytes.len() && bytes[offset + 1] == b'=' => {
                offset += 2;
                emit(TokenKind::EqualEqual, start, offset, &mut tokens);
            }
            // Only a `!` without `=` is the symbolic `not`.
            b'!' if offset + 1 >= bytes.len() || bytes[offset + 1] != b'=' => {
                offset += 1;
                diagnostics.push(symbolic_logic_diagnostic(source, start, offset, "not"));
                emit(TokenKind::Bang, start, offset, &mut tokens);
            }
            b'.' if offset + 1 < bytes.len() && bytes[offset + 1] == b'.' => {
                if offset + 2 < bytes.len() && bytes[offset + 2] == b'=' {
                    offset += 3;
                    emit(TokenKind::DotDotEqual, start, offset, &mut tokens);
                } else {
                    offset += 2;
                    emit(TokenKind::DotDot, start, offset, &mut tokens);
                }
            }
            b'<' if offset + 1 < bytes.len() && bytes[offset + 1] == b'=' => {
                offset += 2;
                emit(TokenKind::LessEqual, start, offset, &mut tokens);
            }
            b'>' if offset + 1 < bytes.len() && bytes[offset + 1] == b'=' => {
                offset += 2;
                emit(TokenKind::GreaterEqual, start, offset, &mut tokens);
            }
            b'}' if interpolations.last() == Some(&0) => {
                interpolations.pop();
                offset += 1;
                emit(TokenKind::InterpolationClose, start, offset, &mut tokens);
                offset = lex_text_chunk(
                    source,
                    offset,
                    false,
                    &mut tokens,
                    &mut diagnostics,
                    &mut interpolations,
                );
            }
            b'{' | b'}' | b'(' | b')' | b'[' | b']' | b':' | b',' | b'.' | b'/' | b'<' | b'>'
            | b'+' | b'-' | b'*' | b'%' | b'=' => {
                if let Some(depth) = interpolations.last_mut() {
                    match bytes[offset] {
                        b'{' => *depth += 1,
                        b'}' => *depth -= 1,
                        _ => {}
                    }
                }
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
                    b'-' => TokenKind::Minus,
                    b'*' => TokenKind::Star,
                    b'%' => TokenKind::Percent,
                    b'=' => TokenKind::Equal,
                    _ => unreachable!(),
                };
                offset += 1;
                emit(kind, start, offset, &mut tokens);
            }
            b'0'..=b'9' => {
                let (kind, end, problem) = lex_number(bytes, start);
                offset = end;
                if let Some(problem) = problem {
                    diagnostics.push(Diagnostic::error(
                        DiagnosticStage::Syntax,
                        "S1001",
                        source.file_id(),
                        TextRange::new(start, offset),
                        problem,
                    ));
                }
                emit(kind, start, offset, &mut tokens);
            }
            // `_1` reads as a misplaced digit separator, not an identifier.
            b'_' if offset + 1 < bytes.len() && bytes[offset + 1].is_ascii_digit() => {
                let (kind, end, _) = lex_number(bytes, start + 1);
                offset = end;
                diagnostics.push(Diagnostic::error(
                    DiagnosticStage::Syntax,
                    "S1001",
                    source.file_id(),
                    TextRange::new(start, offset),
                    "`_` may appear only between two digits of a number",
                ));
                emit(kind, start, offset, &mut tokens);
            }
            b'"' => {
                offset = lex_text_chunk(
                    source,
                    offset,
                    true,
                    &mut tokens,
                    &mut diagnostics,
                    &mut interpolations,
                );
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
                    "let" => TokenKind::Let,
                    "const" => TokenKind::Const,
                    "case" => TokenKind::Case,
                    "loop" => TokenKind::Loop,
                    "break" => TokenKind::Break,
                    "if" => TokenKind::If,
                    "errors" => TokenKind::Errors,
                    "needs" => TokenKind::Needs,
                    "using" => TokenKind::Using,
                    "and" => TokenKind::And,
                    "or" => TokenKind::Or,
                    "not" => TokenKind::Not,
                    "Void" => TokenKind::Void,
                    "True" => TokenKind::True,
                    "False" => TokenKind::False,
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

    if !interpolations.is_empty() {
        diagnostics.push(Diagnostic::error(
            DiagnosticStage::Syntax,
            "S1101",
            source.file_id(),
            TextRange::empty(text.len()),
            "an interpolation must close with `}` on the same line",
        ));
    }
    tokens.push(LexedToken {
        kind: TokenKind::End,
        text: Arc::from(""),
        range: TextRange::empty(text.len()),
    });
    (tokens, diagnostics)
}

/// Lexes one chunk of a text literal starting at `start`: the opening `"` and what
/// follows when `opening`, otherwise the text right after an interpolation's `}`.
///
/// Ensures: emits `Text` for a literal with no interpolation; otherwise a chunk token
/// (`TextStart`, `TextMiddle`, or `TextEnd`), followed by `InterpolationOpen` and a new
/// `interpolations` entry when the chunk ends at `${`. An escape such as `\$` never
/// opens an interpolation. Returns the offset after the emitted tokens.
fn lex_text_chunk(
    source: &SourceSnapshot,
    start: usize,
    opening: bool,
    tokens: &mut Vec<LexedToken>,
    diagnostics: &mut Vec<Diagnostic>,
    interpolations: &mut Vec<u32>,
) -> usize {
    let text = source.text();
    let bytes = text.as_bytes();
    let mut emit = |kind: TokenKind, from: usize, to: usize| {
        tokens.push(LexedToken {
            kind,
            text: Arc::from(&text[from..to]),
            range: TextRange::new(from, to),
        });
    };
    let mut offset = if opening { start + 1 } else { start };
    while offset < bytes.len() {
        match bytes[offset] {
            b'\\' => match text_escape(&text[offset + 1..]) {
                // A backslash at a line end leaves the literal unterminated.
                TextEscape::Truncated => offset += 1,
                TextEscape::Valid { length, .. } => offset += 1 + length,
                TextEscape::Invalid { length, problem } => {
                    diagnostics.push(Diagnostic::error(
                        DiagnosticStage::Syntax,
                        "S1001",
                        source.file_id(),
                        TextRange::new(offset, offset + 1 + length),
                        problem,
                    ));
                    offset += 1 + length;
                }
            },
            b'"' => {
                offset += 1;
                let kind = if opening {
                    TokenKind::Text
                } else {
                    TokenKind::TextEnd
                };
                emit(kind, start, offset);
                return offset;
            }
            b'$' if bytes.get(offset + 1) == Some(&b'{') => {
                let kind = if opening {
                    TokenKind::TextStart
                } else {
                    TokenKind::TextMiddle
                };
                emit(kind, start, offset);
                emit(TokenKind::InterpolationOpen, offset, offset + 2);
                interpolations.push(0);
                return offset + 2;
            }
            b'\n' | b'\r' => break,
            _ => {
                offset += text[offset..]
                    .chars()
                    .next()
                    .expect("offset is before source end")
                    .len_utf8();
            }
        }
    }
    diagnostics.push(Diagnostic::error(
        DiagnosticStage::Syntax,
        "S1001",
        source.file_id(),
        TextRange::new(start, offset),
        "unterminated text literal",
    ));
    emit(TokenKind::InvalidText, start, offset);
    offset
}

/// One escape sequence in a text literal, read from the text after its backslash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextEscape {
    /// `length` bytes after the backslash denote `character`.
    Valid { character: char, length: usize },
    /// `length` bytes after the backslash are a malformed escape.
    Invalid {
        length: usize,
        problem: &'static str,
    },
    /// The backslash ends the line or the source.
    Truncated,
}

/// Reads the escape whose backslash precedes `rest`.
///
/// The escapes are `\"`, `\\`, `\n`, `\r`, `\t`, `\$`, and `\u{X}` with one to six
/// hex digits naming a Unicode scalar value. A malformed escape never extends past
/// the line or a closing quote, so the literal still ends where it appears to.
pub(crate) fn text_escape(rest: &str) -> TextEscape {
    let Some(first) = rest.chars().next() else {
        return TextEscape::Truncated;
    };
    let character = match first {
        '"' => '"',
        '\\' => '\\',
        'n' => '\n',
        'r' => '\r',
        't' => '\t',
        '$' => '$',
        '\n' | '\r' => return TextEscape::Truncated,
        'u' => return unicode_escape(rest),
        other => {
            return TextEscape::Invalid {
                length: other.len_utf8(),
                problem: "unknown escape; text literals accept `\\\"`, `\\\\`, `\\n`, `\\r`, `\\t`, `\\$`, and `\\u{...}`",
            };
        }
    };
    TextEscape::Valid {
        character,
        length: 1,
    }
}

/// Reads `u{X}` at the start of `rest`.
fn unicode_escape(rest: &str) -> TextEscape {
    const MALFORMED: &str = "a Unicode escape is `\\u{` followed by one to six hex digits and `}`";
    let bytes = rest.as_bytes();
    if bytes.get(1) != Some(&b'{') {
        return TextEscape::Invalid {
            length: 1,
            problem: MALFORMED,
        };
    }
    let digits_end = 2 + bytes[2..]
        .iter()
        .take_while(|byte| byte.is_ascii_hexdigit())
        .count();
    let digits = &rest[2..digits_end];
    if bytes.get(digits_end) != Some(&b'}') || digits.is_empty() || digits.len() > 6 {
        return TextEscape::Invalid {
            length: digits_end,
            problem: MALFORMED,
        };
    }
    let length = digits_end + 1;
    match u32::from_str_radix(digits, 16)
        .ok()
        .and_then(char::from_u32)
    {
        Some(character) => TextEscape::Valid { character, length },
        None => TextEscape::Invalid {
            length,
            problem: "a Unicode escape must name a Unicode scalar value: not a surrogate and at most 10FFFF",
        },
    }
}

/// Lexes the number literal that starts with the digit at `start`.
///
/// Ensures: returns the token kind, the end offset, and a problem message when the
/// literal is malformed. A `.` starts a fraction only when a digit follows it, so
/// `0..10` and `point.x` are untouched; `e` starts an exponent only when digits follow.
fn lex_number(bytes: &[u8], start: usize) -> (TokenKind, usize, Option<&'static str>) {
    const SEPARATOR: &str = "`_` may appear only between two digits of a number";
    let mut problem = None;
    // Consumes digits accepted by `is_digit` and `_` separators; reports a misplaced `_`.
    let digits =
        |offset: &mut usize, problem: &mut Option<&'static str>, is_digit: fn(u8) -> bool| {
            let first = *offset;
            while *offset < bytes.len() && (is_digit(bytes[*offset]) || bytes[*offset] == b'_') {
                *offset += 1;
            }
            let run = &bytes[first..*offset];
            if run.first() == Some(&b'_')
                || run.last() == Some(&b'_')
                || run.windows(2).any(|pair| pair == b"__")
            {
                problem.get_or_insert(SEPARATOR);
            }
            run.iter().any(|byte| *byte != b'_')
        };
    let mut offset = start;
    let radix_digit: Option<fn(u8) -> bool> = match bytes.get(start + 1) {
        Some(b'x') if bytes[start] == b'0' => Some(|byte: u8| byte.is_ascii_hexdigit()),
        Some(b'b') if bytes[start] == b'0' => Some(|byte: u8| matches!(byte, b'0' | b'1')),
        _ => None,
    };
    if let Some(is_digit) = radix_digit {
        offset += 2;
        let has_digits = digits(&mut offset, &mut problem, is_digit);
        // Letters and digits the radix does not allow still belong to this literal.
        let valid_end = offset;
        while offset < bytes.len() && is_identifier_continue(bytes[offset]) {
            offset += 1;
        }
        if !has_digits || offset != valid_end {
            problem = Some(if bytes[start + 1] == b'x' {
                "a hexadecimal literal needs digits 0-9 and a-f after `0x`"
            } else {
                "a binary literal needs digits 0 and 1 after `0b`"
            });
        }
        return (TokenKind::Integer, offset, problem);
    }
    digits(&mut offset, &mut problem, |byte| byte.is_ascii_digit());
    // `0_17` is as much a leading zero as `017`: any digit after the first counts.
    if bytes[start] == b'0' && bytes[start + 1..offset].iter().any(u8::is_ascii_digit) {
        problem
            .get_or_insert("a decimal literal cannot start with `0`; Chimi has no octal literals");
    }
    let mut kind = TokenKind::Integer;
    if offset + 1 < bytes.len() && bytes[offset] == b'.' && bytes[offset + 1].is_ascii_digit() {
        offset += 1;
        digits(&mut offset, &mut problem, |byte| byte.is_ascii_digit());
        kind = TokenKind::Float;
    }
    if offset < bytes.len() && matches!(bytes[offset], b'e' | b'E') {
        let sign = usize::from(matches!(bytes.get(offset + 1), Some(b'+' | b'-')));
        if bytes
            .get(offset + 1 + sign)
            .is_some_and(|byte| byte.is_ascii_digit())
        {
            offset += 1 + sign;
            digits(&mut offset, &mut problem, |byte| byte.is_ascii_digit());
            kind = TokenKind::Float;
        }
    }
    (kind, offset, problem)
}

/// Reports `&&`, `||`, or `!` at `start..end` with an edit to the keyword `word`.
///
/// The replacement adds the spaces a keyword needs where the symbol touched its
/// neighbors, so `a&&b` becomes `a and b` and `!ready` becomes `not ready`.
fn symbolic_logic_diagnostic(
    source: &SourceSnapshot,
    start: usize,
    end: usize,
    word: &str,
) -> Diagnostic {
    let bytes = source.text().as_bytes();
    let touches = |byte: Option<&u8>| byte.is_some_and(|byte| !byte.is_ascii_whitespace());
    // `not` is prefix, so it only needs a space where it would otherwise join
    // a preceding word, as in `if!ready`; the binary keywords need one after
    // any non-space neighbor.
    let prefix = start > 0
        && if word == "not" {
            is_identifier_continue(bytes[start - 1])
        } else {
            touches(bytes.get(start - 1))
        };
    let suffix = touches(bytes.get(end));
    let replacement = format!(
        "{}{word}{}",
        if prefix { " " } else { "" },
        if suffix { " " } else { "" }
    );
    Diagnostic::error(
        DiagnosticStage::Syntax,
        "S1004",
        source.file_id(),
        TextRange::new(start, end),
        format!("Chimi spells this `{word}`"),
    )
    .with_fix(TextRange::new(start, end), replacement)
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
                TokenKind::Pub | TokenKind::Struct | TokenKind::Enum | TokenKind::Interface => {
                    self.parse_declaration()
                }
                TokenKind::Fn if self.fn_starts_declaration() => self.parse_declaration(),
                TokenKind::Const => self.parse_const_declaration(),
                TokenKind::Let => self.parse_let_binding(),
                _ => {
                    self.error_at_current(
                        "expected an import, a declaration, or a `const` at file scope",
                    );
                    self.parse_expression();
                }
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
            Some((NodeKind::ConstDeclaration, TokenKind::Const)) => self.parse_const_declaration(),
            _ => self.error_and_bump(
                "expected `struct`, `enum`, `interface`, `fn`, or `const` after `pub`",
            ),
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
            TokenKind::Const => Some((NodeKind::ConstDeclaration, TokenKind::Const)),
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
        if self.at(TokenKind::If) {
            self.start(NodeKind::ClauseGuard);
            self.bump();
            self.parse_expression();
            self.finish();
        }
        self.skip_newlines();
        self.parse_block();
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

    /// Parses one parameter. A typed name, or a lowercase name, keeps the plain
    /// `ValueParameter` shape; any other form, such as `None` or `Circle(r)`, is a
    /// clause pattern, parsed as a `Pattern` child of the `ValueParameter`.
    fn parse_value_parameter(&mut self) {
        self.start(NodeKind::ValueParameter);
        let lowercase = !self
            .current()
            .text
            .starts_with(|character: char| character.is_uppercase());
        let plain = self.at(TokenKind::Identifier)
            && match self.kind_at(self.position + 1) {
                TokenKind::Colon => !self.parameter_type_is_aliased(),
                TokenKind::Comma | TokenKind::RightParen | TokenKind::Newline => lowercase,
                _ => false,
            };
        if plain {
            self.bump();
            if self.accept(TokenKind::Colon) {
                self.skip_newlines();
                self.parse_type();
            }
        } else {
            self.parse_pattern();
        }
        self.finish();
    }

    /// Whether the typed parameter starting at the current name continues with `as`,
    /// which makes it an alias pattern rather than a plain parameter.
    fn parameter_type_is_aliased(&self) -> bool {
        let mut position = self.position + 2;
        let mut depth = 0_u32;
        loop {
            match self.kind_at(position) {
                TokenKind::Less | TokenKind::LeftParen => depth += 1,
                TokenKind::Greater | TokenKind::RightParen if depth > 0 => depth -= 1,
                TokenKind::As if depth == 0 => return true,
                TokenKind::Comma | TokenKind::RightParen | TokenKind::End if depth == 0 => {
                    return false;
                }
                TokenKind::LeftBrace | TokenKind::End => return false,
                _ => {}
            }
            position += 1;
        }
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
        } else if self.at(TokenKind::LeftParen) {
            self.parse_tuple_type();
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

    /// Parses `(T1, T2, ...)`. Ensures: `()` and `(T)` are `S1005`; one `TupleType` node.
    fn parse_tuple_type(&mut self) {
        self.start(NodeKind::TupleType);
        let open = self.current().range;
        self.bump();
        self.skip_newlines();
        let elements = if self.at(TokenKind::RightParen) {
            0
        } else {
            self.parse_type();
            self.parse_remaining_elements(Self::parse_type)
        };
        if elements < 2 {
            let message = if elements == 0 {
                "`()` is not a type; use `Void`"
            } else {
                "a tuple type needs at least two elements"
            };
            let range = TextRange::new(open.start() as usize, self.current().range.end() as usize);
            self.syntax_error("S1005", range, message);
        }
        self.skip_newlines();
        self.expect(TokenKind::RightParen);
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
        // The contract may continue on the next line, but a line break that is
        // not followed by `errors` or `needs` ends the type, so it can still
        // separate fields and parameters.
        if self.accept_after_newlines(TokenKind::Errors) {
            self.skip_newlines();
            self.parse_type();
        }
        if self.at_after_newlines(TokenKind::Needs) {
            self.skip_newlines();
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

    /// Parses `[pub] const <name> [: <type>] = <expression>`.
    /// Ensures: one `ConstDeclaration` node. Inside a block the node is still built,
    /// so later stages see the initializer, and `S1103` reports the misplaced keyword.
    fn parse_const_declaration(&mut self) {
        self.start(NodeKind::ConstDeclaration);
        self.parse_visibility();
        self.expect(TokenKind::Const);
        self.expect(TokenKind::Identifier);
        if self.accept(TokenKind::Colon) {
            self.parse_type();
        }
        self.expect(TokenKind::Equal);
        self.parse_expression();
        self.finish();
    }

    /// Parses `let <name> = <value>`, or `let <pattern> = <value>` for any other
    /// binding form, such as `let (low, high) = pair` or `let count: Int = 0`.
    fn parse_let_binding(&mut self) {
        self.start(NodeKind::LetBinding);
        self.expect(TokenKind::Let);
        self.parse_binding_target();
        self.expect(TokenKind::Equal);
        self.parse_expression();
        self.finish();
    }

    fn parse_expression(&mut self) {
        self.parse_using();
    }

    fn parse_using(&mut self) {
        let marker = self.placeholder();
        self.parse_or();
        let mut wrapped = false;
        while self.accept_after_newlines(TokenKind::Using) {
            if !wrapped {
                self.complete(marker, NodeKind::UsingExpression);
                wrapped = true;
            }
            self.skip_newlines();
            self.parse_record();
        }
        if wrapped {
            self.finish();
        }
    }

    fn parse_or(&mut self) {
        self.parse_left_associative(
            NodeKind::OrExpression,
            &[TokenKind::Or, TokenKind::BarBar],
            Self::parse_and,
        );
    }

    fn parse_and(&mut self) {
        self.parse_left_associative(
            NodeKind::AndExpression,
            &[TokenKind::And, TokenKind::AmpersandAmpersand],
            Self::parse_not,
        );
    }

    /// Parses `operand (operator operand)*` as one flat `kind` node.
    ///
    /// A line break may come before each operator, as before `+` and `|>`.
    fn parse_left_associative(
        &mut self,
        kind: NodeKind,
        operators: &[TokenKind],
        operand: fn(&mut Self),
    ) {
        let marker = self.placeholder();
        operand(self);
        let mut wrapped = false;
        while self.after_any_newlines(operators) {
            if !wrapped {
                self.complete(marker, kind);
                wrapped = true;
            }
            self.skip_newlines();
            self.bump();
            self.skip_newlines();
            operand(self);
        }
        if wrapped {
            self.finish();
        }
    }

    fn parse_not(&mut self) {
        if self.at(TokenKind::Not) || self.at(TokenKind::Bang) {
            self.start(NodeKind::NotExpression);
            self.bump();
            self.skip_newlines();
            self.parse_not();
            self.finish();
        } else {
            self.parse_pipeline();
        }
    }

    fn parse_pipeline(&mut self) {
        let marker = self.placeholder();
        self.parse_comparison();
        let mut wrapped = false;
        while self.accept_after_newlines(TokenKind::Pipe) {
            if !wrapped {
                self.complete(marker, NodeKind::PipelineExpression);
                wrapped = true;
            }
            if self.pipeline_right_hand_side_is_missing() {
                self.missing(TokenKind::Identifier, "pipeline transformation");
                break;
            }
            self.skip_newlines();
            self.parse_comparison();
        }
        if wrapped {
            self.finish();
        }
    }

    fn pipeline_right_hand_side_is_missing(&self) -> bool {
        let mut position = self.position;
        while self.kind_at(position) == TokenKind::Newline {
            position += 1;
        }
        matches!(
            self.kind_at(position),
            TokenKind::End
                | TokenKind::RightBrace
                | TokenKind::RightBracket
                | TokenKind::RightParen
                | TokenKind::Comma
                | TokenKind::Pub
                | TokenKind::Import
                | TokenKind::Struct
                | TokenKind::Enum
                | TokenKind::Interface
                | TokenKind::Fn
                | TokenKind::Let
                | TokenKind::Const
        )
    }

    /// Equality and ordering operators are non-associative: one operator per
    /// comparison. A second one, as in `a < b < c`, is `S1006`; the parser still
    /// consumes it so the rest of the expression recovers.
    fn parse_comparison(&mut self) {
        const COMPARISONS: [TokenKind; 6] = [
            TokenKind::Less,
            TokenKind::LessEqual,
            TokenKind::Greater,
            TokenKind::GreaterEqual,
            TokenKind::EqualEqual,
            TokenKind::BangEqual,
        ];
        let marker = self.placeholder();
        self.parse_range();
        let mut wrapped = false;
        while self.after_any_newlines(&COMPARISONS) {
            self.skip_newlines();
            if wrapped {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticStage::Syntax,
                    "S1006",
                    self.source.file_id(),
                    self.current().range,
                    "comparisons do not chain; write `a < b and b < c`",
                ));
            } else {
                self.complete(marker, NodeKind::ComparisonExpression);
                wrapped = true;
            }
            self.bump();
            self.skip_newlines();
            self.parse_range();
        }
        if wrapped {
            self.finish();
        }
    }

    /// Ranges sit between comparison and additive operators and do not chain: a
    /// second `..` or `..=`, as in `a..b..c`, is `S1006`; it is still consumed so the
    /// rest of the expression recovers.
    fn parse_range(&mut self) {
        const RANGES: [TokenKind; 2] = [TokenKind::DotDot, TokenKind::DotDotEqual];
        let marker = self.placeholder();
        self.parse_additive();
        let mut wrapped = false;
        while self.after_any_newlines(&RANGES) {
            self.skip_newlines();
            if wrapped {
                self.syntax_error(
                    "S1006",
                    self.current().range,
                    "ranges do not chain; write one range, such as `a..c`",
                );
            } else {
                self.complete(marker, NodeKind::RangeExpression);
                wrapped = true;
            }
            self.bump();
            self.skip_newlines();
            self.parse_additive();
        }
        if wrapped {
            self.finish();
        }
    }

    fn parse_additive(&mut self) {
        let marker = self.placeholder();
        self.parse_multiplicative();
        let mut wrapped = false;
        while self.after_any_newlines(&[TokenKind::Plus, TokenKind::Minus])
            && !self.next_line_starts_negative_arm()
        {
            if !wrapped {
                self.complete(marker, NodeKind::AdditiveExpression);
                wrapped = true;
            }
            self.skip_newlines();
            self.bump();
            self.skip_newlines();
            self.parse_multiplicative();
        }
        if wrapped {
            self.finish();
        }
    }

    fn parse_multiplicative(&mut self) {
        let marker = self.placeholder();
        self.parse_unary();
        let mut wrapped = false;
        while self.after_any_newlines(&[TokenKind::Star, TokenKind::Slash, TokenKind::Percent]) {
            if !wrapped {
                self.complete(marker, NodeKind::MultiplicativeExpression);
                wrapped = true;
            }
            self.skip_newlines();
            self.bump();
            self.skip_newlines();
            self.parse_unary();
        }
        if wrapped {
            self.finish();
        }
    }

    /// Whether a line break comes next and the following line is a case arm whose
    /// first pattern is a negative number: `-`, a number, then `->` later on that
    /// line outside brackets. Such a line starts a new arm instead of continuing a
    /// subtraction, because `- 1 ... ->` could never complete the expression.
    fn next_line_starts_negative_arm(&self) -> bool {
        let mut position = self.position;
        if self.kind_at(position) != TokenKind::Newline {
            return false;
        }
        while self.kind_at(position) == TokenKind::Newline {
            position += 1;
        }
        if self.kind_at(position) != TokenKind::Minus
            || !matches!(
                self.kind_at(position + 1),
                TokenKind::Integer | TokenKind::Float
            )
        {
            return false;
        }
        let mut depth = 0_u32;
        position += 2;
        loop {
            match self.kind_at(position) {
                TokenKind::LeftParen | TokenKind::LeftBracket | TokenKind::LeftBrace => {
                    depth += 1;
                }
                TokenKind::RightParen | TokenKind::RightBracket | TokenKind::RightBrace
                    if depth > 0 =>
                {
                    depth -= 1;
                }
                TokenKind::Arrow if depth == 0 => return true,
                TokenKind::Newline if depth == 0 => return false,
                TokenKind::RightParen
                | TokenKind::RightBracket
                | TokenKind::RightBrace
                | TokenKind::End => return false,
                _ => {}
            }
            position += 1;
        }
    }

    fn after_any_newlines(&self, kinds: &[TokenKind]) -> bool {
        let mut position = self.position;
        while self.kind_at(position) == TokenKind::Newline {
            position += 1;
        }
        kinds.contains(&self.kind_at(position))
    }

    fn parse_unary(&mut self) {
        let marker = self.placeholder();
        let wrapped = if self.accept(TokenKind::Minus) {
            self.complete(marker, NodeKind::UnaryExpression);
            self.skip_newlines();
            true
        } else {
            false
        };
        self.parse_postfix();
        if wrapped {
            self.finish();
        }
    }

    fn parse_postfix(&mut self) {
        let marker = self.placeholder();
        self.parse_primary();
        let mut wrapped = false;
        loop {
            if self.accept(TokenKind::Dot) {
                if !wrapped {
                    self.complete(marker, NodeKind::PostfixExpression);
                    wrapped = true;
                }
                self.start(NodeKind::MemberAccess);
                self.expect(TokenKind::Identifier);
                self.finish();
            } else if self.at(TokenKind::LeftParen) {
                if !wrapped {
                    self.complete(marker, NodeKind::PostfixExpression);
                    wrapped = true;
                }
                self.parse_call_suffix();
            } else {
                break;
            }
        }
        if wrapped {
            self.finish();
        }
    }

    /// Parses `"text ${expression} text"` from its `TextStart` token.
    /// Ensures: one `InterpolatedText` node; `${}` reports `S1101` and the node still
    /// closes, and a chunk the lexer could not finish ends the node at that point.
    fn parse_interpolated_text(&mut self) {
        self.start(NodeKind::InterpolatedText);
        self.bump();
        loop {
            self.expect(TokenKind::InterpolationOpen);
            if self.at(TokenKind::InterpolationClose) {
                let range = self.current().range;
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticStage::Syntax,
                    "S1101",
                    self.source.file_id(),
                    range,
                    "an interpolation needs an expression between `${` and `}`",
                ));
            } else {
                self.parse_expression();
            }
            if !self.expect(TokenKind::InterpolationClose) {
                break;
            }
            if self.accept(TokenKind::TextMiddle) {
                continue;
            }
            self.expect(TokenKind::TextEnd);
            break;
        }
        self.finish();
    }

    fn parse_primary(&mut self) {
        match self.kind() {
            TokenKind::Integer
            | TokenKind::Float
            | TokenKind::Text
            | TokenKind::True
            | TokenKind::False
            | TokenKind::Void => {
                self.start(NodeKind::Literal);
                self.bump();
                self.finish();
            }
            TokenKind::Identifier => {
                self.start(NodeKind::NameReference);
                self.bump();
                self.finish();
            }
            TokenKind::TextStart => self.parse_interpolated_text(),
            TokenKind::LeftParen => self.parse_parenthesized(),
            TokenKind::LeftBracket => self.parse_list(),
            TokenKind::LeftBrace => self.parse_brace_expression(),
            TokenKind::Case => self.parse_case(),
            TokenKind::Loop => self.parse_loop(),
            TokenKind::Break => self.parse_break(),
            TokenKind::Fn => self.parse_function_expression(),
            TokenKind::If => self.parse_unsupported_if(),
            _ => self.missing(TokenKind::Identifier, "expression"),
        }
    }

    /// Parses `(e)` as a `GroupedExpression` and `(e1, e2, ...)` as a `TupleExpression`.
    /// Ensures: `()` and the one-element `(e,)` are `S1005` and still close their node.
    fn parse_parenthesized(&mut self) {
        let marker = self.placeholder();
        let open = self.current().range;
        self.bump();
        self.skip_newlines();
        if self.at(TokenKind::RightParen) {
            self.complete(marker, NodeKind::GroupedExpression);
            let range = TextRange::new(open.start() as usize, self.current().range.end() as usize);
            self.syntax_error("S1005", range, "`()` is not a value; use `Void`");
            self.bump();
            self.finish();
            return;
        }
        self.parse_expression();
        let tuple = self.at_after_newlines(TokenKind::Comma);
        self.complete(
            marker,
            if tuple {
                NodeKind::TupleExpression
            } else {
                NodeKind::GroupedExpression
            },
        );
        let elements = self.parse_remaining_elements(Self::parse_expression);
        if tuple && elements < 2 {
            self.syntax_error(
                "S1005",
                TextRange::new(open.start() as usize, self.current().range.end() as usize),
                "a tuple needs at least two elements",
            );
        }
        self.skip_newlines();
        self.expect(TokenKind::RightParen);
        self.finish();
    }

    /// After a first element, parses `, element` repeatedly up to `)`, allowing a
    /// trailing comma. Returns the element count, the first one included.
    fn parse_remaining_elements(&mut self, element: fn(&mut Self)) -> usize {
        let mut count = 1;
        while self.accept_after_newlines(TokenKind::Comma) {
            self.skip_newlines();
            if self.at(TokenKind::RightParen) {
                break;
            }
            element(self);
            count += 1;
        }
        count
    }

    fn syntax_error(&mut self, code: &'static str, range: TextRange, message: &str) {
        self.diagnostics.push(Diagnostic::error(
            DiagnosticStage::Syntax,
            code,
            self.source.file_id(),
            range,
            message,
        ));
    }

    fn parse_call_suffix(&mut self) {
        self.start(NodeKind::CallSuffix);
        self.expect(TokenKind::LeftParen);
        self.skip_newlines();
        if self.at(TokenKind::DotDot) {
            self.reject_positional_update();
        }
        if !self.at(TokenKind::RightParen) {
            self.parse_argument();
            while self.accept(TokenKind::Comma) {
                self.skip_newlines();
                if self.at(TokenKind::RightParen) {
                    break;
                }
                self.parse_argument();
            }
        }
        self.skip_newlines();
        self.expect(TokenKind::RightParen);
        self.finish();
    }

    /// The source ranges of the comma-separated parts of a bracketed list whose
    /// first part starts at significant `position`, and the closing bracket's
    /// position (or the end of input).
    fn bracketed_parts(&self, mut position: usize) -> (Vec<TextRange>, usize) {
        let mut parts = Vec::new();
        let mut current: Option<TextRange> = None;
        let mut depth = 0_u32;
        loop {
            let kind = self.kind_at(position);
            match kind {
                TokenKind::LeftParen | TokenKind::LeftBracket | TokenKind::LeftBrace => {
                    depth += 1;
                }
                TokenKind::RightParen | TokenKind::RightBracket | TokenKind::RightBrace
                    if depth > 0 =>
                {
                    depth -= 1;
                }
                TokenKind::RightParen
                | TokenKind::RightBracket
                | TokenKind::RightBrace
                | TokenKind::End => break,
                TokenKind::Comma if depth == 0 => {
                    parts.extend(current.take());
                    position += 1;
                    continue;
                }
                _ => {}
            }
            if kind != TokenKind::Newline {
                let range = self.tokens[self.significant[position]].range;
                current = Some(current.map_or(range, |current| {
                    TextRange::new(current.start() as usize, range.end() as usize)
                }));
            }
            position += 1;
        }
        parts.extend(current);
        (parts, position)
    }

    fn source_slice(&self, range: TextRange) -> &str {
        &self.source.text()[range.start() as usize..range.end() as usize]
    }

    /// Reports `Config(..base, jobs)` as `S1001`. Updates name the fields they change,
    /// so the message suggests the braced form, labelling each plain-name argument with
    /// its own name, and when every argument is a plain name the fix rewrites the call
    /// to that form. The `..` is consumed so `base` parses as an ordinary argument;
    /// lowering then drops the call, so it adds no type errors.
    fn reject_positional_update(&mut self) {
        let start = self.current().range.start();
        let (parts, close) = self.bracketed_parts(self.position + 1);
        let parts = parts
            .into_iter()
            .map(|range| self.source_slice(range).to_owned())
            .collect::<Vec<_>>();
        let mut suggestion = format!("{{ ..{}", parts.first().map_or("", String::as_str));
        let mut all_names = true;
        for part in parts.iter().skip(1) {
            let is_name = part
                .chars()
                .next()
                .is_some_and(|first| first.is_ascii_lowercase())
                && part
                    .chars()
                    .all(|character| character.is_alphanumeric() || character == '_');
            if is_name {
                suggestion.push_str(&format!(", {part}: {part}"));
            } else {
                all_names = false;
                suggestion.push_str(&format!(", field: {part}"));
            }
        }
        suggestion.push_str(" }");
        let range = TextRange::new(start as usize, self.current().range.end() as usize);
        let mut diagnostic = Diagnostic::error(
            DiagnosticStage::Syntax,
            "S1001",
            self.source.file_id(),
            range,
            format!(
                "a record update names the fields it changes and is not a call; write `{suggestion}`"
            ),
        );
        // The callee is the name (or dotted path) right before `(`.
        let callee_end = self.position.saturating_sub(2);
        let mut callee = callee_end;
        while callee >= 2
            && self.kind_at(callee - 1) == TokenKind::Dot
            && self.kind_at(callee - 2) == TokenKind::Identifier
        {
            callee -= 2;
        }
        if all_names
            && parts.len() > 1
            && self.kind_at(self.position.saturating_sub(1)) == TokenKind::LeftParen
            && self.kind_at(callee_end) == TokenKind::Identifier
            && self.kind_at(close) == TokenKind::RightParen
        {
            let call = TextRange::new(
                self.tokens[self.significant[callee]].range.start() as usize,
                self.tokens[self.significant[close]].range.end() as usize,
            );
            diagnostic = diagnostic.with_fix(call, suggestion);
        }
        self.diagnostics.push(diagnostic);
        self.bump();
    }

    fn parse_argument(&mut self) {
        self.start(NodeKind::Argument);
        if self.at(TokenKind::Identifier) && self.kind_at(self.position + 1) == TokenKind::Colon {
            self.bump();
            self.bump();
        }
        self.parse_expression();
        self.finish();
    }

    fn parse_list(&mut self) {
        self.start(NodeKind::ListExpression);
        self.bump();
        self.skip_newlines();
        if !self.at(TokenKind::RightBracket) {
            self.parse_expression();
            while self.accept(TokenKind::Comma) {
                self.skip_newlines();
                if self.at(TokenKind::RightBracket) {
                    break;
                }
                self.parse_expression();
            }
        }
        self.skip_newlines();
        self.expect(TokenKind::RightBracket);
        self.finish();
    }

    fn parse_brace_expression(&mut self) {
        let mut position = self.position + 1;
        while self.kind_at(position) == TokenKind::Newline {
            position += 1;
        }
        if self.kind_at(position) == TokenKind::RightBrace {
            self.start(NodeKind::EmptyBraceExpression);
            self.bump();
            self.skip_newlines();
            self.error_at_current("empty braces are invalid");
            self.bump();
            self.finish();
        } else if self.kind_at(position) == TokenKind::DotDot || self.looks_like_record(position) {
            self.parse_record();
        } else {
            self.parse_block();
        }
    }

    fn looks_like_record(&self, position: usize) -> bool {
        if self.kind_at(position) != TokenKind::Identifier {
            return false;
        }
        if self.kind_at(position + 1) == TokenKind::Colon {
            return true;
        }
        let mut scan = position + 1;
        let mut depth = 0_u32;
        loop {
            match self.kind_at(scan) {
                TokenKind::LeftParen | TokenKind::LeftBracket | TokenKind::LeftBrace => depth += 1,
                TokenKind::RightParen | TokenKind::RightBracket if depth > 0 => depth -= 1,
                TokenKind::RightBrace if depth > 0 => depth -= 1,
                TokenKind::Comma if depth == 0 => return true,
                TokenKind::Newline | TokenKind::RightBrace if depth == 0 => return false,
                TokenKind::End => return false,
                _ => {}
            }
            scan += 1;
        }
    }

    /// Parses a record literal or a record update `{ ..base, field: value }`.
    /// Ensures: a leading `..base` is a `RecordSpread` child; a spread anywhere else is
    /// `S1001` and is still parsed as a `RecordSpread` so lowering sees the base.
    fn parse_record(&mut self) {
        self.start(NodeKind::RecordExpression);
        let open = self.position;
        self.expect(TokenKind::LeftBrace);
        self.skip_newlines();
        let mut spread_fix = self.spread_first_fix(open);
        if self.at(TokenKind::DotDot) {
            self.parse_record_spread();
        } else {
            self.parse_record_field();
        }
        while self.accept(TokenKind::Comma) {
            self.skip_newlines();
            if self.at(TokenKind::RightBrace) {
                break;
            }
            if self.at(TokenKind::DotDot) {
                let mut diagnostic = Diagnostic::error(
                    DiagnosticStage::Syntax,
                    "S1001",
                    self.source.file_id(),
                    self.current().range,
                    "`..base` must come first in a record update and appear once; move it to the start",
                );
                if let Some((range, replacement)) = spread_fix.take() {
                    diagnostic = diagnostic.with_fix(range, replacement);
                }
                self.diagnostics.push(diagnostic);
                self.parse_record_spread();
            } else {
                self.parse_record_field();
            }
        }
        self.skip_newlines();
        self.expect(TokenKind::RightBrace);
        self.finish();
    }

    /// For a record whose `{` is at significant `open` and that holds exactly one
    /// `..base`, not first: the edit that rewrites the record with the spread first
    /// and the other parts in their order.
    fn spread_first_fix(&self, open: usize) -> Option<(TextRange, String)> {
        let (parts, close) = self.bracketed_parts(open + 1);
        if self.kind_at(close) != TokenKind::RightBrace {
            return None;
        }
        let parts = parts
            .into_iter()
            .map(|range| self.source_slice(range))
            .collect::<Vec<_>>();
        let spreads = parts.iter().filter(|part| part.starts_with("..")).count();
        let spread = parts.iter().position(|part| part.starts_with(".."))?;
        if spreads != 1 || spread == 0 {
            return None;
        }
        let ordered = std::iter::once(parts[spread])
            .chain(
                parts
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index != spread)
                    .map(|(_, part)| *part),
            )
            .collect::<Vec<_>>();
        let range = TextRange::new(
            self.tokens[self.significant[open]].range.start() as usize,
            self.tokens[self.significant[close]].range.end() as usize,
        );
        Some((range, format!("{{ {} }}", ordered.join(", "))))
    }

    fn parse_record_spread(&mut self) {
        self.start(NodeKind::RecordSpread);
        self.bump();
        self.parse_expression();
        self.finish();
    }

    fn parse_record_field(&mut self) {
        self.start(NodeKind::RecordField);
        self.expect(TokenKind::Identifier);
        self.expect(TokenKind::Colon);
        self.parse_expression();
        self.finish();
    }

    fn parse_block(&mut self) {
        self.start(NodeKind::BlockExpression);
        self.expect(TokenKind::LeftBrace);
        self.skip_newlines();
        if self.at(TokenKind::RightBrace) {
            self.error_at_current("empty braces are invalid");
            self.bump();
            self.finish();
            return;
        }
        while !self.at(TokenKind::RightBrace) && !self.at(TokenKind::End) {
            let before = self.position;
            self.start(NodeKind::BlockItem);
            if self.at(TokenKind::Let) {
                self.parse_let_binding();
            } else if self.at(TokenKind::Const) {
                let range = self.current().range;
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticStage::Syntax,
                    "S1103",
                    self.source.file_id(),
                    range,
                    "`const` declares a module constant and is valid only at module level; use `let` inside a block",
                ));
                self.parse_const_declaration();
            } else {
                self.parse_expression();
            }
            self.finish();
            if self.position == before {
                self.error_and_bump("parser made no progress in block");
            }
            if !self.at(TokenKind::RightBrace) && !self.skip_newlines() {
                self.error_and_bump("expected a line break between block items");
            }
        }
        self.expect(TokenKind::RightBrace);
        self.finish();
    }

    /// Parses `loop <name> = <initial> <block>`.
    /// Ensures: one `LoopExpression` node whose children are the state identifier token,
    /// the initial expression, and a `BlockExpression` body, recovering like `case`.
    fn parse_loop(&mut self) {
        self.start(NodeKind::LoopExpression);
        self.bump();
        self.parse_binding_target();
        self.expect(TokenKind::Equal);
        self.skip_newlines();
        self.parse_expression();
        self.parse_block();
        self.finish();
    }

    /// The target of `let` or a `loop` state: a plain name stays a direct identifier
    /// token, and every other form is a `Pattern` child.
    fn parse_binding_target(&mut self) {
        if self.at(TokenKind::Identifier) && self.kind_at(self.position + 1) == TokenKind::Equal {
            self.bump();
        } else {
            self.parse_pattern();
        }
    }

    /// Parses `break` with an optional value.
    /// Ensures: the value is parsed only when the next significant token can start an
    /// expression on the same line; `break` before a newline, `}`, `,`, or `)` is bare.
    fn parse_break(&mut self) {
        self.start(NodeKind::BreakExpression);
        self.bump();
        if !matches!(
            self.kind(),
            TokenKind::Newline
                | TokenKind::RightBrace
                | TokenKind::Comma
                | TokenKind::RightParen
                | TokenKind::End
        ) {
            self.parse_expression();
        }
        self.finish();
    }

    fn parse_case(&mut self) {
        self.start(NodeKind::CaseExpression);
        self.bump();
        self.parse_expression();
        while self.accept(TokenKind::Comma) {
            self.skip_newlines();
            self.parse_expression();
        }
        self.skip_newlines();
        self.expect(TokenKind::LeftBrace);
        self.skip_newlines();
        while !self.at(TokenKind::RightBrace) && !self.at(TokenKind::End) {
            let before = self.position;
            self.start(NodeKind::CaseArm);
            self.parse_pattern();
            while self.accept(TokenKind::Comma) {
                self.skip_newlines();
                self.parse_pattern();
            }
            if self.accept(TokenKind::If) {
                self.parse_expression();
            }
            self.skip_newlines();
            self.expect(TokenKind::Arrow);
            self.skip_newlines();
            self.parse_expression();
            self.finish();
            if self.position == before {
                self.error_and_bump("parser made no progress in case expression");
            }
            if !self.at(TokenKind::RightBrace) && !self.skip_newlines() {
                self.error_and_bump("expected a line break between case arms");
            }
        }
        self.expect(TokenKind::RightBrace);
        self.finish();
    }

    /// Parses one pattern. `as name` binds less tightly than every other form, so an
    /// alias wraps the complete pattern before it in an outer `Pattern` node.
    fn parse_pattern(&mut self) {
        let marker = self.placeholder();
        self.parse_unaliased_pattern();
        if self.at(TokenKind::As) {
            self.complete(marker, NodeKind::Pattern);
            self.bump();
            self.expect(TokenKind::Identifier);
            self.finish();
        }
    }

    fn parse_unaliased_pattern(&mut self) {
        self.start(NodeKind::Pattern);
        match self.kind() {
            TokenKind::Integer
            | TokenKind::Float
            | TokenKind::Text
            | TokenKind::True
            | TokenKind::False
            | TokenKind::Void => {
                self.bump();
            }
            TokenKind::Minus
                if matches!(
                    self.kind_at(self.position + 1),
                    TokenKind::Integer | TokenKind::Float
                ) =>
            {
                self.bump();
                self.bump();
            }
            TokenKind::Identifier => {
                let lowercase = !self
                    .current()
                    .text
                    .starts_with(|character: char| character.is_uppercase());
                self.bump();
                if lowercase && self.at(TokenKind::Colon) {
                    self.parse_pattern_type();
                } else {
                    while self.accept(TokenKind::Dot) {
                        self.expect(TokenKind::Identifier);
                    }
                    if self.accept(TokenKind::LeftParen) {
                        self.skip_newlines();
                        if !self.at(TokenKind::RightParen) {
                            self.parse_pattern();
                            while self.accept(TokenKind::Comma) {
                                self.skip_newlines();
                                self.parse_pattern();
                            }
                        }
                        self.skip_newlines();
                        self.expect(TokenKind::RightParen);
                    }
                }
            }
            TokenKind::LeftBracket => self.parse_list_pattern_elements(),
            // Consumed whole so one diagnostic replaces a cascade of token errors.
            TokenKind::TextStart => {
                let start = self.current().range.start() as usize;
                self.parse_interpolated_text();
                let end = self.tokens[self.significant[self.position - 1]].range.end() as usize;
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticStage::Syntax,
                    "S1101",
                    self.source.file_id(),
                    TextRange::new(start, end),
                    "a text pattern cannot interpolate; bind the text and compare it in a guard",
                ));
            }
            TokenKind::LeftParen => self.parse_tuple_pattern_elements(),
            _ => self.missing(TokenKind::Identifier, "pattern"),
        }
        self.finish();
    }

    /// Parses `: Type` after a binding name. A colon in a pattern always introduces a
    /// type; a colon followed by anything else is a labelled pattern, which is `S1001`
    /// because patterns are positional. The labelled pattern is still parsed so the
    /// rest of the pattern recovers.
    fn parse_pattern_type(&mut self) {
        self.bump();
        if matches!(
            self.kind(),
            TokenKind::Identifier | TokenKind::Void | TokenKind::Fn
        ) {
            self.parse_type();
        } else {
            self.error_at_current(
                "patterns are positional, as construction is; a colon in a pattern introduces a type, not a field label",
            );
            self.parse_pattern();
        }
    }

    /// Parses the bracketed elements of a list pattern such as `[first, ..rest]`.
    /// Ensures: each element is a `Pattern` node; a rest element is a `Pattern` node that
    /// starts with `..` and holds at most one binding name. The parser accepts any number
    /// of rest elements so lowering can report `L2021` with the whole pattern in view.
    fn parse_list_pattern_elements(&mut self) {
        self.bump();
        self.skip_newlines();
        while !self.at(TokenKind::RightBracket) && !self.at(TokenKind::End) {
            if self.at(TokenKind::DotDot) {
                self.start(NodeKind::Pattern);
                self.bump();
                self.accept(TokenKind::Identifier);
                self.finish();
            } else {
                let before = self.position;
                self.parse_pattern();
                if self.position == before {
                    break;
                }
            }
            if !self.accept(TokenKind::Comma) {
                break;
            }
            self.skip_newlines();
        }
        self.skip_newlines();
        self.expect(TokenKind::RightBracket);
    }

    /// Parses the parenthesized elements of a tuple pattern `(p1, p2, ...)`.
    /// Ensures: each element is a `Pattern` child; `(p)` groups one pattern, and `()`
    /// and `(p,)` are `S1005`.
    fn parse_tuple_pattern_elements(&mut self) {
        let open = self.current().range;
        self.bump();
        self.skip_newlines();
        let (elements, trailing) = if self.at(TokenKind::RightParen) {
            (0, false)
        } else {
            self.parse_pattern();
            let trailing = self.at_after_newlines(TokenKind::Comma);
            (self.parse_remaining_elements(Self::parse_pattern), trailing)
        };
        if elements == 0 || (elements == 1 && trailing) {
            let message = if elements == 0 {
                "`()` is not a pattern; use `Void`"
            } else {
                "a tuple pattern needs at least two elements"
            };
            let range = TextRange::new(open.start() as usize, self.current().range.end() as usize);
            self.syntax_error("S1005", range, message);
        }
        self.skip_newlines();
        self.expect(TokenKind::RightParen);
    }

    fn parse_function_expression(&mut self) {
        self.start(NodeKind::FunctionExpression);
        self.bump();
        self.skip_newlines();
        self.parse_value_parameters();
        self.parse_implementation_contract();
        self.skip_newlines();
        self.parse_block();
        self.finish();
    }

    fn parse_unsupported_if(&mut self) {
        self.start(NodeKind::ErrorIfExpression);
        self.error_at_current("standalone if is not an expression; use case");
        self.bump();
        let mut depth = 0_u32;
        let mut saw_brace = false;
        while !self.at(TokenKind::End) {
            if self.at(TokenKind::LeftBrace) {
                saw_brace = true;
                depth += 1;
                self.bump();
            } else if self.at(TokenKind::RightBrace) {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                self.bump();
                if saw_brace && depth == 0 {
                    if self.at(TokenKind::Identifier) && self.current().text.as_ref() == "else" {
                        self.bump();
                        saw_brace = false;
                    } else {
                        break;
                    }
                }
            } else {
                self.bump();
            }
        }
        self.finish();
    }

    fn placeholder(&mut self) -> usize {
        let marker = self.events.len();
        self.events.push(Event::Tombstone);
        marker
    }

    fn complete(&mut self, marker: usize, kind: NodeKind) {
        debug_assert!(matches!(self.events[marker], Event::Tombstone));
        self.events[marker] = Event::Start(kind);
    }

    fn fn_starts_declaration(&self) -> bool {
        let mut position = self.position + 1;
        while self.kind_at(position) == TokenKind::Newline {
            position += 1;
        }
        self.kind_at(position) == TokenKind::Identifier
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
                | TokenKind::Let
                | TokenKind::Const
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

    fn at_after_newlines(&self, kind: TokenKind) -> bool {
        let mut position = self.position;
        while self.kind_at(position) == TokenKind::Newline {
            position += 1;
        }
        self.kind_at(position) == kind
    }

    fn accept_after_newlines(&mut self, kind: TokenKind) -> bool {
        if !self.at_after_newlines(kind) {
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
        element_count: u32,
    }

    let mut stack: Vec<OpenNode> = Vec::new();
    let mut root = None;
    for event in events {
        match *event {
            Event::Tombstone => {}
            Event::Start(kind) => stack.push(OpenNode {
                kind,
                children: Vec::new(),
                text_len: 0,
                element_count: 1,
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
                open.element_count = open
                    .element_count
                    .checked_add(1)
                    .expect("syntax element counts fit in u32");
            }
            Event::Missing(kind) => {
                let open = stack
                    .last_mut()
                    .expect("missing tokens belong to an open node");
                open.children.push(GreenElement::Token(GreenToken {
                    kind,
                    text: Arc::from(""),
                    missing: true,
                }));
                open.element_count = open
                    .element_count
                    .checked_add(1)
                    .expect("syntax element counts fit in u32");
            }
            Event::Finish => {
                let open = stack.pop().expect("finish closes an open node");
                let node = Arc::new(GreenNode {
                    kind: open.kind,
                    children: open.children.into_boxed_slice(),
                    text_len: open.text_len,
                    element_count: open.element_count,
                });
                if let Some(parent) = stack.last_mut() {
                    parent.text_len = parent
                        .text_len
                        .checked_add(node.text_len)
                        .expect("syntax node lengths fit in the source offset type");
                    parent.element_count = parent
                        .element_count
                        .checked_add(node.element_count)
                        .expect("syntax element counts fit in u32");
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
    use super::typed::{BlockExpressionSyntax, PostfixExpressionSyntax};
    use super::{
        DeclarationSyntax, FileSyntax, NodeKind, SyntaxElement, SyntaxNode, TypedSyntaxNode, parse,
    };
    use crate::diagnostic::DiagnosticStage;
    use crate::source::SourceSnapshot;
    use std::collections::BTreeSet;

    fn token_kinds(text: &str) -> Vec<super::TokenKind> {
        let (tokens, diagnostics) = super::lex(&SourceSnapshot::from_text("lex.chimi", text));
        assert!(diagnostics.is_empty(), "{text}: {diagnostics:?}");
        tokens.into_iter().map(|token| token.kind).collect()
    }

    // N11: a `.` starts a fraction only when a digit follows it.
    #[test]
    fn a_dot_after_digits_starts_a_fraction_only_before_a_digit() {
        use super::TokenKind::{Dot, DotDot, End, Float, Identifier, Integer};
        assert_eq!(token_kinds("0..10"), [Integer, DotDot, Integer, End]);
        assert_eq!(token_kinds("1.x"), [Integer, Dot, Identifier, End]);
        assert_eq!(token_kinds("1.5"), [Float, End]);
        assert_eq!(
            token_kinds("1e9 2E-3"),
            [Float, super::TokenKind::Whitespace, Float, End]
        );
        assert_eq!(token_kinds("2em"), [Integer, Identifier, End]);
    }

    fn contains_node(node: &SyntaxNode<'_>, kind: NodeKind) -> bool {
        node.kind() == kind
            || node
                .child_nodes()
                .into_iter()
                .any(|child| contains_node(&child, kind))
    }

    fn find_node<'tree>(node: &SyntaxNode<'tree>, kind: NodeKind) -> Option<SyntaxNode<'tree>> {
        if node.kind() == kind {
            return Some(node.clone());
        }
        node.child_nodes()
            .into_iter()
            .find_map(|child| find_node(&child, kind))
    }

    fn collect_ids(node: &SyntaxNode<'_>, ids: &mut Vec<u32>) {
        ids.push(node.id().raw());
        ids.extend(
            node.child_tokens()
                .into_iter()
                .map(|token| token.id().raw()),
        );
        for child in node.child_nodes() {
            collect_ids(&child, ids);
        }
    }

    #[test]
    fn valid_source_round_trips_with_every_trivia_byte() {
        let source = "// answer\r\nstruct Answer {\n  value: Int // cents\n}\n\npub fn main() {\n  Answer(42)\n}\n";
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
        assert_eq!(missing[0].parent().kind(), NodeKind::BlockExpression);
    }

    #[test]
    fn invalid_unicode_is_preserved_and_parsing_terminates() {
        let source = "struct Answer { value: Int } pub fn main() { Answer(λ) }";
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

    #[test]
    fn effect_clauses_are_not_language_syntax() {
        let source = "fn main() effects { io } { 1 }\n";
        let result = parse(&SourceSnapshot::from_text("effects.chimi", source));

        assert_eq!(result.round_trip(), source);
        assert!(!result.diagnostics().is_empty());
        assert!(
            result
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.stage() == DiagnosticStage::Syntax)
        );
    }

    #[test]
    fn accepted_expression_corpus_round_trips_without_syntax_diagnostics() {
        let cases = [
            (
                "composition.chimi",
                include_str!("../../../prototypes/golden/valid/composition.chimi"),
            ),
            (
                "dependencies-and-execution.chimi",
                include_str!("../../../prototypes/golden/valid/dependencies_and_execution.chimi"),
            ),
            (
                "expressions-and-matching.chimi",
                include_str!("../../../prototypes/golden/valid/expressions_and_matching.chimi"),
            ),
            (
                "functions-as-values.chimi",
                include_str!("../../../prototypes/golden/valid/functions_as_values.chimi"),
            ),
            (
                "values-and-variants.chimi",
                include_str!("../../../prototypes/golden/valid/values_and_variants.chimi"),
            ),
            (
                "worker-forcing-example.chimi",
                include_str!("../../../prototypes/golden/valid/worker_forcing_example.chimi"),
            ),
            (
                "money.chimi",
                include_str!("../../../prototypes/golden/valid/modules/money.chimi"),
            ),
            (
                "use-money.chimi",
                include_str!("../../../prototypes/golden/valid/modules/use_money.chimi"),
            ),
        ];

        for (path, source) in cases {
            let snapshot = SourceSnapshot::from_text(path, source);
            let result = parse(&snapshot);

            assert_eq!(result.round_trip(), source, "fixture {path}");
            assert_eq!(
                result.diagnostics(),
                [],
                "fixture {path}: {:?}",
                result.diagnostics()
            );
        }
    }

    #[test]
    fn focused_expression_recovery_corpus_round_trips_and_diagnoses() {
        let cases = [
            (
                "incomplete-pipeline.chimi",
                include_str!("../../../prototypes/lossless-parser/cases/incomplete_pipeline.chimi"),
            ),
            (
                "missing-closing-brace.chimi",
                include_str!(
                    "../../../prototypes/lossless-parser/cases/missing_closing_brace.chimi"
                ),
            ),
            (
                "missing-record-colon.chimi",
                include_str!(
                    "../../../prototypes/lossless-parser/cases/missing_record_colon.chimi"
                ),
            ),
            (
                "missing-record-value.chimi",
                include_str!(
                    "../../../prototypes/lossless-parser/cases/missing_record_value.chimi"
                ),
            ),
        ];

        for (path, source) in cases {
            let snapshot = SourceSnapshot::from_text(path, source);
            let result = parse(&snapshot);

            assert_eq!(result.round_trip(), source, "fixture {path}");
            assert!(!result.diagnostics().is_empty(), "fixture {path}");
        }
    }

    #[test]
    fn semantic_negative_corpus_remains_syntax_valid() {
        let cases = [
            include_str!("../../../prototypes/golden/invalid/ignored_value.chimi"),
            include_str!("../../../prototypes/golden/invalid/interface_shape_mismatch.chimi"),
            include_str!("../../../prototypes/golden/invalid/missing_dependency.chimi"),
            include_str!("../../../prototypes/golden/invalid/non_exhaustive_case.chimi"),
            include_str!("../../../prototypes/golden/invalid/non_exhaustive_outcome_case.chimi"),
            include_str!("../../../prototypes/golden/invalid/observable_case_guard.chimi"),
            include_str!("../../../prototypes/golden/invalid/private_struct_construction.chimi"),
            include_str!("../../../prototypes/golden/invalid/private_struct_field_access.chimi"),
        ];

        for source in cases {
            let snapshot = SourceSnapshot::from_text("semantic-negative.chimi", source);
            let result = parse(&snapshot);

            assert_eq!(result.round_trip(), source);
            assert_eq!(result.diagnostics(), []);
        }
    }

    #[test]
    fn syntax_negative_corpus_round_trips_with_diagnostics() {
        let cases = [
            include_str!("../../../prototypes/golden/invalid/empty_braces.chimi"),
            include_str!("../../../prototypes/golden/invalid/implicit_module_alias.chimi"),
            include_str!("../../../prototypes/golden/invalid/standalone_if.chimi"),
            include_str!("../../../prototypes/golden/invalid/empty_tuple.chimi"),
        ];

        for source in cases {
            let snapshot = SourceSnapshot::from_text("syntax-negative.chimi", source);
            let result = parse(&snapshot);

            assert_eq!(result.round_trip(), source);
            assert!(!result.diagnostics().is_empty());
        }
    }

    #[test]
    fn expression_tree_preserves_precedence_and_composition_boundaries() {
        let source = concat!(
            "const configured = operation |> retry using { clock: system_clock }\n",
            "const calculated = 1 + 2 * 3\n",
        );
        let snapshot = SourceSnapshot::from_text("precedence.chimi", source);
        let result = parse(&snapshot);
        let root = result.root();

        assert_eq!(result.diagnostics(), []);
        // `using` applies to the whole pipeline, and `*` groups inside `+`.
        let using = find_node(&root, NodeKind::UsingExpression).expect("using node");
        let pipeline = find_node(&using, NodeKind::PipelineExpression).expect("pipeline in using");
        assert!(!contains_node(&pipeline, NodeKind::UsingExpression));
        assert!(!contains_node(&pipeline, NodeKind::RecordExpression));
        let additive = find_node(&root, NodeKind::AdditiveExpression).expect("additive node");
        let product =
            find_node(&additive, NodeKind::MultiplicativeExpression).expect("product in sum");
        assert_eq!(
            source[product.range().start() as usize..product.range().end() as usize].trim(),
            "2 * 3"
        );
    }

    #[test]
    fn cursor_computes_absolute_ranges_parents_and_snapshot_ids() {
        let source = "// λ\r\nstruct Answer {\n  value: Int\n}\n";
        let snapshot = SourceSnapshot::from_text("cursor.chimi", source);
        let result = parse(&snapshot);
        let root = result.root();
        let structure = find_node(&root, NodeKind::StructDeclaration).expect("struct node");
        let field = find_node(&structure, NodeKind::Field).expect("field node");
        let field_start = source.find("  value: Int").expect("field text");

        assert_eq!(root.id().raw(), 0);
        assert_eq!(root.range(), crate::ids::TextRange::new(0, source.len()));
        assert_eq!(field.range().start() as usize, field_start);
        assert_eq!(field.parent().expect("field parent").id(), structure.id());
        let field_name = field
            .child_tokens()
            .into_iter()
            .find(|token| token.text() == "value")
            .expect("field name");
        assert_eq!(
            field_name.range().start() as usize,
            source.find("value: Int").unwrap()
        );

        let name = structure
            .child_tokens()
            .into_iter()
            .find(|token| token.text() == "Answer")
            .expect("struct name");
        assert_eq!(name.parent().id(), structure.id());
        assert_eq!(
            name.range().start() as usize,
            source.find("Answer").unwrap()
        );
        assert!(matches!(
            result.element(structure.id()),
            Some(SyntaxElement::Node(node)) if node.id() == structure.id()
        ));
        assert!(matches!(
            result.element(name.id()),
            Some(SyntaxElement::Token(token)) if token.text() == "Answer"
        ));
        assert!(
            result
                .element(crate::ids::SyntaxId::new(u32::MAX))
                .is_none()
        );

        let mut ids = Vec::new();
        collect_ids(&root, &mut ids);
        assert_eq!(
            ids.iter().copied().collect::<BTreeSet<_>>().len(),
            ids.len()
        );
    }

    #[test]
    fn snapshot_ids_are_deterministic_for_identical_parse_trees() {
        let source = "struct Answer { value: Int }\npub fn main() { Answer(42) }\n";
        let first = parse(&SourceSnapshot::from_text("first.chimi", source));
        let second = parse(&SourceSnapshot::from_text("second.chimi", source));
        let mut first_ids = Vec::new();
        let mut second_ids = Vec::new();
        collect_ids(&first.root(), &mut first_ids);
        collect_ids(&second.root(), &mut second_ids);

        assert_eq!(first_ids, second_ids);
    }

    #[test]
    fn typed_facade_exposes_declarations_and_expression_shapes() {
        let source = "struct Answer { value: Int }\npub fn main() { Answer(42) }\n";
        let result = parse(&SourceSnapshot::from_text("typed.chimi", source));
        let file = FileSyntax::cast(result.root()).expect("file root");
        let declarations = file.declarations();

        let DeclarationSyntax::Struct(structure) = &declarations[0] else {
            panic!("first declaration should be a struct");
        };
        assert_eq!(
            structure.name_token().expect("struct name").text(),
            "Answer"
        );
        assert_eq!(
            structure.fields()[0]
                .name_token()
                .expect("field name")
                .text(),
            "value"
        );

        let DeclarationSyntax::Function(function) = &declarations[1] else {
            panic!("second declaration should be a function");
        };
        assert!(function.is_public());
        assert_eq!(function.name_token().expect("function name").text(), "main");
        let body = function.body().expect("function body");
        assert!(BlockExpressionSyntax::cast(body.syntax().clone()).is_some());
        let postfix = find_node(body.syntax(), NodeKind::PostfixExpression)
            .and_then(PostfixExpressionSyntax::cast)
            .expect("constructor call");
        assert_eq!(postfix.call_suffixes().len(), 1);
    }

    #[test]
    fn typed_accessors_tolerate_missing_declaration_children() {
        let source = "struct { value: Int }\npub fn () {}\n";
        let result = parse(&SourceSnapshot::from_text("incomplete.chimi", source));
        let file = FileSyntax::cast(result.root()).expect("file root");
        let declarations = file.declarations();

        let DeclarationSyntax::Struct(structure) = &declarations[0] else {
            panic!("first declaration should remain structurally available");
        };
        assert!(structure.name_token().is_none());

        let DeclarationSyntax::Function(function) = &declarations[1] else {
            panic!("second declaration should remain structurally available");
        };
        assert!(function.name_token().is_none());
        assert!(function.body().is_some());
    }

    #[test]
    fn fixed_seed_malformed_inputs_preserve_frontend_invariants() {
        const FRAGMENTS: &[&str] = &[
            " ",
            "\n",
            "\r\n",
            "// comment\n",
            "struct",
            "enum",
            "interface",
            "fn",
            "let",
            "case",
            "if",
            "errors",
            "needs",
            "using",
            "Void",
            "name",
            "123",
            "\"text\"",
            "{",
            "}",
            "(",
            ")",
            "[",
            "]",
            ":",
            ",",
            ".",
            "/",
            "<",
            ">",
            "+",
            "-",
            "*",
            "=",
            "->",
            "|>",
            "λ",
            "💥",
            "@",
            "\"unterminated",
            "\"a ${",
            "${",
            "} b\"",
            "\\$",
            "const",
        ];

        let mut state = 0x5eed_f00d_cafe_babe_u64;
        for case_index in 0..2_048 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let fragment_count = (state as usize % 96) + 1;
            let mut source = String::new();
            for _ in 0..fragment_count {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                source.push_str(FRAGMENTS[state as usize % FRAGMENTS.len()]);
            }

            let snapshot = SourceSnapshot::from_text(format!("fuzz-{case_index}.chimi"), source);
            let parsed = parse(&snapshot);
            let statistics = super::validate_parse(&parsed, &snapshot)
                .unwrap_or_else(|error| panic!("case {case_index}: {error}"));

            assert!(statistics.node_count > 0, "case {case_index}");
            assert!(
                parsed.diagnostics().len() <= (statistics.token_count + 1) * 4,
                "case {case_index} produced {} diagnostics for {} tokens: {:?}",
                parsed.diagnostics().len(),
                statistics.token_count,
                snapshot.text(),
            );
        }
    }
}
