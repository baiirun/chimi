use crate::diagnostic::Diagnostic;
use crate::source::SourceSnapshot;
use crate::syntax::{NodeKind, ParseResult, SyntaxToken, TokenKind, parse};
use std::sync::Arc;

#[derive(Debug)]
pub struct FormatReport {
    source: Arc<SourceSnapshot>,
    formatted: String,
    diagnostics: Vec<Diagnostic>,
}

impl FormatReport {
    pub(crate) fn new(source: SourceSnapshot) -> Self {
        let source = Arc::new(source);
        let parsed = parse(&source);
        if !parsed.diagnostics().is_empty() {
            return Self {
                formatted: source.text().to_owned(),
                diagnostics: parsed.diagnostics().to_vec(),
                source,
            };
        }

        // Joining split imports edits the source, so the result is parsed again.
        let joined = join_split_imports(source.text(), &parsed)
            .map(|text| parse(&SourceSnapshot::from_text(source.path(), text)));
        let parsed = joined.as_ref().unwrap_or(&parsed);
        let formatted = crate::doc_examples::format_examples(&format_parse(parsed), |code| {
            let example = SourceSnapshot::from_text(source.path(), code.to_owned());
            let parsed = parse(&example);
            parsed.diagnostics().is_empty().then(|| format_parse(&parsed))
        });
        Self {
            formatted,
            diagnostics: Vec::new(),
            source,
        }
    }

    pub fn text(&self) -> &str {
        &self.formatted
    }

    pub fn is_changed(&self) -> bool {
        self.formatted != self.source.text()
    }

    pub fn is_blocked(&self) -> bool {
        !self.diagnostics.is_empty()
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn render_diagnostics(&self) -> String {
        let mut output = self
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.render(&self.source))
            .collect::<Vec<_>>()
            .join("\n");
        if !output.is_empty() {
            output.push('\n');
        }
        output
    }
}

/// One file-scope import line, as the import join sees it.
struct ImportLine {
    path: String,
    alias: Option<String>,
    members: Option<Vec<String>>,
    /// The import's own text, from `import` to its last token.
    text: std::ops::Range<usize>,
    /// The whole line or lines it fills, with the newline that ends it.
    lines: std::ops::Range<usize>,
    /// False when a comment or other code shares its lines.
    joinable: bool,
}

/// Joins `import p as A` with `import p.{T}` into `import p as A.{T}`
/// (decisions.md 2026-11-22), at the first of the lines. A path is joined only
/// when it has exactly one alias-only import, at least one member-only import,
/// no combined import already, no member named twice (the conflict stays
/// visible), and no comment from its first import line to its last. Returns
/// `None` when nothing is joined.
fn join_split_imports(source: &str, parsed: &ParseResult) -> Option<String> {
    let lines: Vec<ImportLine> = parsed
        .root()
        .child_nodes()
        .into_iter()
        .filter(|node| node.kind() == NodeKind::ImportDeclaration)
        .filter_map(|node| import_line(source, &node))
        .collect();
    let comments: Vec<usize> = parsed
        .root()
        .descendant_tokens()
        .into_iter()
        .filter(|token| token.kind() == TokenKind::Comment)
        .map(|token| token.range().start() as usize)
        .collect();
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let mut seen_paths: Vec<&str> = Vec::new();
    for line in &lines {
        if seen_paths.contains(&line.path.as_str()) {
            continue;
        }
        seen_paths.push(&line.path);
        let same_path: Vec<&ImportLine> =
            lines.iter().filter(|other| other.path == line.path).collect();
        let aliases: Vec<&&ImportLine> = same_path
            .iter()
            .filter(|other| other.alias.is_some())
            .collect();
        let member_lines: Vec<&&ImportLine> =
            same_path.iter().filter(|other| other.members.is_some()).collect();
        let [alias_line] = aliases.as_slice() else {
            continue;
        };
        if member_lines.is_empty()
            || alias_line.members.is_some()
            || same_path.iter().any(|other| !other.joinable)
        {
            continue;
        }
        let span = same_path[0].lines.start..same_path[same_path.len() - 1].lines.end;
        if comments.iter().any(|offset| span.contains(offset)) {
            continue;
        }
        let members: Vec<&str> = member_lines
            .iter()
            .flat_map(|other| other.members.iter().flatten())
            .map(String::as_str)
            .collect();
        if (1..members.len()).any(|index| members[..index].contains(&members[index])) {
            continue;
        }
        let alias = alias_line.alias.as_deref().unwrap_or_default();
        let joined = format!("import {} as {alias}.{{{}}}", line.path, members.join(", "));
        edits.push((same_path[0].text.clone(), joined));
        for other in &same_path[1..] {
            edits.push((other.lines.clone(), String::new()));
        }
    }
    if edits.is_empty() {
        return None;
    }
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut text = source.to_owned();
    for (range, replacement) in edits {
        text.replace_range(range, &replacement);
    }
    Some(text)
}

fn import_line(source: &str, node: &crate::syntax::SyntaxNode<'_>) -> Option<ImportLine> {
    let tokens = node.descendant_tokens();
    if tokens.iter().any(|token| token.is_missing()) {
        return None;
    }
    let significant: Vec<_> = tokens
        .iter()
        .filter(|token| {
            !matches!(
                token.kind(),
                TokenKind::Whitespace | TokenKind::Newline | TokenKind::End
            )
        })
        .collect();
    let start = significant.first()?.range().start() as usize;
    let end = significant.last()?.range().end() as usize;
    let line_start = source[..start].rfind('\n').map_or(0, |index| index + 1);
    let line_end = source[end..]
        .find('\n')
        .map_or(source.len(), |index| end + index + 1);
    // Another item or a trailing comment on the same line keeps it as written.
    let joinable = source[line_start..start].trim().is_empty()
        && source[end..line_end].trim().is_empty()
        && !tokens.iter().any(|token| token.kind() == TokenKind::Comment);
    let mut path = Vec::new();
    let mut alias = None;
    let mut members = None;
    for child in node.child_nodes() {
        let names: Vec<String> = child
            .descendant_tokens()
            .into_iter()
            .filter(|token| token.kind() == TokenKind::Identifier)
            .map(|token| token.text().to_owned())
            .collect();
        match child.kind() {
            NodeKind::ImportAlias => alias = names.into_iter().next(),
            NodeKind::ImportMembers => members = Some(names),
            _ => {}
        }
    }
    for token in node.child_tokens() {
        if token.kind() == TokenKind::Identifier {
            path.push(token.text().to_owned());
        }
    }
    Some(ImportLine {
        path: path.join("/"),
        alias,
        members,
        text: start..end,
        lines: line_start..line_end,
        joinable,
    })
}

fn format_parse(parsed: &ParseResult) -> String {
    let tokens = parsed.root().descendant_tokens();
    let clause_ends = clause_group_joints(parsed);
    let mut writer = Writer::default();
    let mut previous = None;
    let mut saw_newline = false;
    // Set after a clause that the next clause of the same function follows, so no
    // blank line separates them; cleared by the next significant token.
    let mut joining_clauses = false;
    // Open `${` interpolations. A text literal cannot span lines, so every token
    // inside one stays on the current line: braces and `|>` are written inline.
    let mut interpolation_depth = 0_usize;

    for (index, token) in tokens.iter().enumerate() {
        let kind = token.kind();
        if matches!(kind, TokenKind::End | TokenKind::Whitespace) || token.is_missing() {
            continue;
        }
        match kind {
            TokenKind::LeftParen
            | TokenKind::LeftBracket
            | TokenKind::LeftBrace
            | TokenKind::InterpolationOpen => writer.open_group(kind),
            TokenKind::RightParen
            | TokenKind::RightBracket
            | TokenKind::RightBrace
            | TokenKind::InterpolationClose => writer.close_group(),
            _ => {}
        }
        let next = next_significant(&tokens, index + 1);
        match kind {
            TokenKind::InterpolationOpen => interpolation_depth += 1,
            TokenKind::InterpolationClose => {
                interpolation_depth = interpolation_depth.saturating_sub(1);
            }
            _ => {}
        }
        if interpolation_depth > 0 {
            match kind {
                TokenKind::LeftBrace => {
                    if !matches!(
                        previous,
                        Some(
                            TokenKind::InterpolationOpen
                                | TokenKind::LeftParen
                                | TokenKind::LeftBracket
                        )
                    ) {
                        writer.space();
                    }
                    writer.write("{ ");
                    previous = Some(kind);
                    saw_newline = false;
                    continue;
                }
                TokenKind::RightBrace => {
                    writer.trim_space();
                    writer.write(" }");
                    previous = Some(kind);
                    saw_newline = false;
                    continue;
                }
                TokenKind::Pipe => {
                    writer.space();
                    writer.write("|> ");
                    previous = Some(kind);
                    saw_newline = false;
                    continue;
                }
                _ => {}
            }
        }
        // A member import is written tight on one line: `import std/io.{A, B}`.
        if token_has_ancestor(token, &[NodeKind::ImportMembers]) {
            match kind {
                TokenKind::Newline => {
                    previous = Some(kind);
                    continue;
                }
                TokenKind::Comma if next == Some(TokenKind::RightBrace) => continue,
                TokenKind::LeftBrace | TokenKind::RightBrace => {
                    writer.trim_space();
                    writer.write(token.text());
                    previous = Some(kind);
                    saw_newline = false;
                    continue;
                }
                TokenKind::Identifier if previous == Some(TokenKind::LeftBrace) => {
                    writer.write(token.text());
                    previous = Some(kind);
                    saw_newline = false;
                    continue;
                }
                _ => {}
            }
        }
        // A record type is written on one line, like the type it sits in, and so
        // are a short record or record update and a type alias.
        if is_record_type_syntax(token)
            || in_short_record(token)
            || in_short_lambda(token)
            || token_has_ancestor(token, &[NodeKind::TypeAliasDeclaration])
        {
            match kind {
                TokenKind::Newline => {
                    writer.space();
                    previous = Some(kind);
                    continue;
                }
                // A trailing comma separates nothing on one line.
                TokenKind::Comma if next == Some(TokenKind::RightBrace) => continue,
                TokenKind::LeftBrace => {
                    if !matches!(
                        previous,
                        Some(TokenKind::LeftParen | TokenKind::LeftBracket | TokenKind::Less)
                    ) {
                        writer.space();
                    }
                    writer.write("{ ");
                    previous = Some(kind);
                    saw_newline = false;
                    continue;
                }
                TokenKind::RightBrace => {
                    writer.trim_space();
                    // A struct without fields is written `Marker {}`.
                    if writer.line().ends_with('{') {
                        writer.write("}");
                    } else {
                        writer.write(" }");
                    }
                    previous = Some(kind);
                    saw_newline = false;
                    continue;
                }
                _ => {}
            }
        }
        match kind {
            TokenKind::Newline => {
                if saw_newline && !joining_clauses {
                    writer.blank_line();
                } else if !should_join_line(previous, next_significant_token(&tokens, index + 1))
                    || writer.line_is_empty()
                {
                    writer.flush();
                } else {
                    writer.space();
                }
                saw_newline = true;
                continue;
            }
            TokenKind::Comment => {
                if !writer.line_is_empty() {
                    writer.space();
                }
                writer.write(token.text());
                writer.flush();
            }
            TokenKind::LeftBrace => {
                if !matches!(
                    previous,
                    Some(TokenKind::LeftParen | TokenKind::LeftBracket)
                ) {
                    writer.space();
                }
                writer.write("{");
                writer.flush();
                writer.indent += 1;
                writer.brace_depth += 1;
            }
            TokenKind::RightBrace => {
                writer.flush();
                writer.indent = writer.indent.saturating_sub(1);
                writer.brace_depth = writer.brace_depth.saturating_sub(1);
                writer.write("}");
                let continues_requirement =
                    next == Some(TokenKind::LeftBrace) && is_requirement_syntax(token);
                let continues_header = next_significant_token(&tokens, index + 1)
                    .is_some_and(|next| opens_header_body(&next));
                // A struct pattern's `}` continues its arm, clause or binding.
                let ends_pattern = token.parent().kind() == NodeKind::Pattern;
                if !continues_requirement
                    && !continues_header
                    && !ends_pattern
                    && !matches!(
                        next,
                        Some(TokenKind::Comma | TokenKind::RightParen | TokenKind::Using)
                    )
                {
                    writer.flush();
                    if writer.brace_depth == 0 {
                        if clause_ends.contains(&token.range().start()) {
                            joining_clauses = true;
                        } else {
                            writer.blank_line();
                        }
                    }
                }
            }
            TokenKind::Comma => {
                writer.trim_space();
                writer.write(", ");
            }
            TokenKind::Colon => {
                writer.trim_space();
                writer.write(": ");
            }
            // Division spaces like the other arithmetic operators; a `/` in an
            // import path such as `std/json` stays tight.
            TokenKind::Slash
                if token_has_ancestor(token, &[NodeKind::MultiplicativeExpression]) =>
            {
                writer.space();
                writer.write("/ ");
            }
            TokenKind::Dot | TokenKind::Slash => {
                writer.trim_space();
                writer.write(token.text());
            }
            TokenKind::LeftParen | TokenKind::LeftBracket => {
                // Only a call, index, or parameter list attaches to what precedes
                // it (`f(x)`, `items[0]`, `fn(x)`). A grouping or list literal
                // after an operator or keyword keeps its space: `x = (1)`,
                // `not (ready)`, `case (a) {`.
                if matches!(
                    previous,
                    Some(
                        TokenKind::Identifier
                            | TokenKind::RightParen
                            | TokenKind::RightBracket
                            | TokenKind::Fn
                    )
                ) {
                    writer.trim_space();
                } else if previous.is_some_and(word_kind) {
                    writer.space();
                }
                writer.write(token.text());
            }
            TokenKind::RightParen | TokenKind::RightBracket | TokenKind::InterpolationClose => {
                writer.trim_space();
                writer.write(token.text());
            }
            TokenKind::Pipe => {
                writer.flush();
                writer.write("|> ");
            }
            TokenKind::Less | TokenKind::Greater if is_type_syntax(token) => {
                writer.trim_space();
                writer.write(token.text());
            }
            // `|` joins failure-set members; `+` was the old spelling after
            // `errors` and is rewritten.
            TokenKind::Bar | TokenKind::Plus if token.parent().kind() == NodeKind::UnionType => {
                writer.space();
                writer.write("| ");
            }
            TokenKind::Minus
                if token_has_ancestor(token, &[NodeKind::UnaryExpression, NodeKind::Pattern]) =>
            {
                writer.write("-");
            }
            TokenKind::Arrow
            | TokenKind::Equal
            | TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Percent
            | TokenKind::Less
            | TokenKind::LessEqual
            | TokenKind::Greater
            | TokenKind::GreaterEqual
            | TokenKind::EqualEqual
            | TokenKind::BangEqual
            | TokenKind::And
            | TokenKind::Or => {
                writer.space();
                writer.write(token.text());
                writer.write(" ");
            }
            // A range is written without spaces, like `0..count`.
            TokenKind::DotDot | TokenKind::DotDotEqual
                if token_has_ancestor(token, &[NodeKind::RangeExpression])
                    && !token_has_ancestor(token, &[NodeKind::Pattern]) =>
            {
                writer.trim_space();
                writer.write(token.text());
            }
            TokenKind::Not => {
                if needs_space(previous, kind, writer.line()) {
                    writer.space();
                }
                writer.write("not ");
            }
            _ => {
                if needs_space(previous, kind, writer.line()) {
                    writer.space();
                }
                writer.write(token.text());
            }
        }
        saw_newline = false;
        if kind != TokenKind::Comment {
            previous = Some(kind);
            if kind != TokenKind::RightBrace {
                joining_clauses = false;
            }
        }
    }

    writer.finish()
}

/// The start offsets of each `}` that closes a function clause directly followed by
/// another clause of the same function. The formatter keeps a clause group together.
fn clause_group_joints(parsed: &ParseResult) -> Vec<u32> {
    let declarations = parsed.root().child_nodes();
    let function_name = |node: &crate::syntax::SyntaxNode<'_>| {
        (node.kind() == NodeKind::FunctionDeclaration)
            .then(|| {
                node.child_tokens()
                    .into_iter()
                    .skip_while(|token| token.kind() != TokenKind::Fn)
                    .find(|token| token.kind() == TokenKind::Identifier)
                    .map(|token| token.text().to_owned())
            })
            .flatten()
    };
    declarations
        .windows(2)
        .filter(|pair| {
            function_name(&pair[0]).is_some() && function_name(&pair[0]) == function_name(&pair[1])
        })
        .filter_map(|pair| {
            pair[0]
                .descendant_tokens()
                .into_iter()
                .rev()
                .find(|token| token.kind() == TokenKind::RightBrace && !token.is_missing())
                .map(|token| token.range().start())
        })
        .collect()
}

/// Whether the line break before `next` is soft: a continued `needs` clause, or the
/// body brace of a header split across lines. A `{` that starts a block item is
/// never joined, because the line break is what ends the item before it.
fn should_join_line(previous: Option<TokenKind>, next: Option<SyntaxToken<'_>>) -> bool {
    let Some(next) = next else {
        return false;
    };
    next.kind() == TokenKind::Needs
        || next.kind() == TokenKind::LeftBrace
            && matches!(
                previous,
                Some(TokenKind::RightParen | TokenKind::RightBrace)
            )
            && !starts_block_item(&next)
}

fn starts_block_item(token: &SyntaxToken<'_>) -> bool {
    let mut node = Some(token.parent());
    while let Some(current) = node {
        if current.kind() == NodeKind::BlockItem {
            return current
                .descendant_tokens()
                .iter()
                .find(|candidate| {
                    !matches!(
                        candidate.kind(),
                        TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment
                    )
                })
                .is_some_and(|first| first.range() == token.range());
        }
        node = current.parent();
    }
    false
}

fn is_requirement_syntax(token: &SyntaxToken<'_>) -> bool {
    token_has_ancestor(
        token,
        &[NodeKind::RequirementClause, NodeKind::RequirementRecord],
    )
}

fn is_record_type_syntax(token: &SyntaxToken<'_>) -> bool {
    token_has_ancestor(token, &[NodeKind::RecordType])
}

/// The widest record or record update the formatter writes on one line, measured
/// from its `{` to its `}` with one space between tokens.
const SHORT_RECORD_WIDTH: usize = 60;

/// Whether `token` lies in a record literal or update written on one line: one at
/// most `SHORT_RECORD_WIDTH` wide that holds no comment, pipeline, or brace other
/// than another record's or a record type's.
fn in_short_record(token: &SyntaxToken<'_>) -> bool {
    let mut node = Some(token.parent());
    while let Some(current) = node {
        if (current.kind() == NodeKind::RecordExpression || is_struct_pattern(&current))
            && is_short_record(&current)
        {
            return true;
        }
        node = current.parent();
    }
    false
}

/// Whether `token` lies in a function expression written on one line in the
/// source, such as `fn(value: Int) -> Int { value + 10 }`: one without a line
/// break, comment, or pipeline, at most `SHORT_RECORD_WIDTH` wide. The formatter
/// keeps it on one line instead of expanding its body.
fn in_short_lambda(token: &SyntaxToken<'_>) -> bool {
    let mut node = Some(token.parent());
    while let Some(current) = node {
        if current.kind() == NodeKind::FunctionExpression && is_short_lambda(&current) {
            return true;
        }
        node = current.parent();
    }
    false
}

fn is_short_lambda(function: &crate::syntax::SyntaxNode<'_>) -> bool {
    let mut width = 0_usize;
    for token in function.descendant_tokens() {
        match token.kind() {
            TokenKind::Whitespace => continue,
            TokenKind::Newline | TokenKind::Comment | TokenKind::Pipe => return false,
            _ => {}
        }
        width += token.text().len() + 1;
        if width > SHORT_RECORD_WIDTH + 1 {
            return false;
        }
    }
    true
}

/// Whether `node` is a struct pattern, `Name { field: pattern, .. }`.
fn is_struct_pattern(node: &crate::syntax::SyntaxNode<'_>) -> bool {
    node.kind() == NodeKind::Pattern
        && node
            .child_tokens()
            .iter()
            .any(|token| token.kind() == TokenKind::LeftBrace)
}

fn is_short_record(record: &crate::syntax::SyntaxNode<'_>) -> bool {
    let mut width = 0_usize;
    for token in record.descendant_tokens() {
        match token.kind() {
            TokenKind::Whitespace | TokenKind::Newline => continue,
            TokenKind::Comment | TokenKind::Pipe => return false,
            TokenKind::LeftBrace | TokenKind::RightBrace
                if !matches!(
                    token.parent().kind(),
                    NodeKind::RecordExpression | NodeKind::RecordType | NodeKind::Pattern
                ) && !in_short_lambda(&token) =>
            {
                return false;
            }
            _ => {}
        }
        width += token.text().len() + 1;
        if width > SHORT_RECORD_WIDTH + 1 {
            return false;
        }
    }
    true
}

fn is_type_syntax(token: &SyntaxToken<'_>) -> bool {
    token_has_ancestor(
        token,
        &[
            NodeKind::Type,
            NodeKind::FunctionType,
            NodeKind::RecordType,
            NodeKind::TypeArguments,
            NodeKind::GenericParameters,
            NodeKind::GenericParameter,
        ],
    )
}

fn token_has_ancestor(token: &SyntaxToken<'_>, kinds: &[NodeKind]) -> bool {
    let mut node = Some(token.parent());
    while let Some(current) = node {
        if kinds.contains(&current.kind()) {
            return true;
        }
        node = current.parent();
    }
    false
}

/// Whether `token` is the `{` that opens a `loop` body or a `case` arm list. Such a
/// brace must stay on the line of the header it follows: after a record header such
/// as `loop s = { a: 1 } {`, a line break would end the loop expression.
fn opens_header_body(token: &SyntaxToken<'_>) -> bool {
    if token.kind() != TokenKind::LeftBrace {
        return false;
    }
    let parent = token.parent();
    match parent.kind() {
        NodeKind::CaseExpression => true,
        NodeKind::BlockExpression => parent
            .parent()
            .is_some_and(|owner| owner.kind() == NodeKind::LoopExpression),
        _ => false,
    }
}

fn next_significant_token<'tree>(
    tokens: &[SyntaxToken<'tree>],
    start: usize,
) -> Option<SyntaxToken<'tree>> {
    tokens[start..]
        .iter()
        .find(|token| {
            !matches!(
                token.kind(),
                TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment
            )
        })
        .cloned()
}

fn next_significant(tokens: &[SyntaxToken<'_>], start: usize) -> Option<TokenKind> {
    tokens[start..]
        .iter()
        .find(|token| {
            !matches!(
                token.kind(),
                TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment
            )
        })
        .map(SyntaxToken::kind)
}

fn needs_space(previous: Option<TokenKind>, current: TokenKind, line: &str) -> bool {
    if line.is_empty() || line.ends_with(' ') {
        return false;
    }
    let Some(previous) = previous else {
        return false;
    };
    if matches!(
        previous,
        TokenKind::Dot | TokenKind::Slash | TokenKind::LeftParen | TokenKind::LeftBracket
    ) {
        return false;
    }
    // A type argument list's `>` is written tight, so the word after it, as in
    // `fn(T) -> Option<U> errors E`, needs its own space.
    // A record type's `}` before a signature's `errors` or `needs`, as in
    // `-> { status: Status } needs { ... }`, needs one too, as does a struct pattern's
    // `}` before a guard or an alias, as in `Money { cents: c, .. } if c > 0`.
    (word_kind(previous) || previous == TokenKind::Greater) && word_kind(current)
        || matches!(previous, TokenKind::RightParen | TokenKind::RightBracket)
        || previous == TokenKind::RightBrace
            && matches!(current, TokenKind::Errors | TokenKind::Needs | TokenKind::If | TokenKind::As)
}

const fn word_kind(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Import
            | TokenKind::As
            | TokenKind::Struct
            | TokenKind::Enum
            | TokenKind::Interface
            | TokenKind::Pub
            | TokenKind::Fn
            | TokenKind::Let
            | TokenKind::Const
            | TokenKind::Case
            | TokenKind::Loop
            | TokenKind::Break
            | TokenKind::If
            | TokenKind::Errors
            | TokenKind::Needs
            | TokenKind::Using
            | TokenKind::And
            | TokenKind::Or
            | TokenKind::Not
            | TokenKind::Void
            | TokenKind::True
            | TokenKind::False
            | TokenKind::Identifier
            | TokenKind::Integer
            | TokenKind::Float
            | TokenKind::Text
            | TokenKind::TextStart
            | TokenKind::TextEnd
    )
}

struct OpenGroup {
    id: usize,
    kind: TokenKind,
    /// Whether a line break inside this `(` or `[` indented its contents; the
    /// matching close then starts its own line one level out.
    indented: bool,
}

#[derive(Default)]
struct Writer {
    output: String,
    line: String,
    indent: usize,
    brace_depth: usize,
    /// Every open bracket, innermost last. A line that ends inside a `(` or `[`
    /// (its last token was written while that bracket was innermost) indents
    /// the rest of the bracket's contents one level; braces indent themselves.
    groups: Vec<OpenGroup>,
    next_group: usize,
    /// The innermost open bracket when the current line was last written to.
    line_group: Option<usize>,
}

impl Writer {
    fn open_group(&mut self, kind: TokenKind) {
        self.groups.push(OpenGroup {
            id: self.next_group,
            kind,
            indented: false,
        });
        self.next_group += 1;
    }

    /// Closes the innermost bracket. An indented `(` or `[` puts its close on a
    /// line of its own, one level out.
    fn close_group(&mut self) {
        if self.groups.last().is_some_and(|group| group.indented) {
            self.flush();
            self.indent = self.indent.saturating_sub(1);
        }
        self.groups.pop();
    }

    fn finish(mut self) -> String {
        self.flush();
        while self.output.ends_with("\n\n") {
            self.output.pop();
        }
        if !self.output.ends_with('\n') {
            self.output.push('\n');
        }
        self.output
    }

    fn line(&self) -> &str {
        &self.line
    }

    fn line_is_empty(&self) -> bool {
        self.line.is_empty()
    }

    fn write(&mut self, text: &str) {
        self.line.push_str(text);
        self.line_group = self.groups.last().map(|group| group.id);
    }

    fn trim_space(&mut self) {
        self.line.truncate(self.line.trim_end().len());
    }

    fn space(&mut self) {
        if !self.line.is_empty() && !self.line.ends_with(' ') {
            self.line.push(' ');
        }
    }

    fn flush(&mut self) {
        let value = self.line.trim();
        if value.is_empty() {
            self.line.clear();
            return;
        }
        self.output.push_str(&"  ".repeat(self.indent));
        self.output.push_str(value);
        self.output.push('\n');
        self.line.clear();
        let line_group = self.line_group.take();
        if let Some(group) = self.groups.last_mut()
            && Some(group.id) == line_group
            && matches!(group.kind, TokenKind::LeftParen | TokenKind::LeftBracket)
            && !group.indented
        {
            group.indented = true;
            self.indent += 1;
        }
    }

    fn blank_line(&mut self) {
        if !self.output.is_empty() && !self.output.ends_with("\n\n") {
            self.output.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FormatReport;
    use crate::source::SourceSnapshot;

    fn format(source: &str) -> FormatReport {
        FormatReport::new(SourceSnapshot::from_text("format.chimi", source))
    }

    /// Example code in a doc comment is formatted as the module it runs as;
    /// an example that does not parse, and a non-Chimi or fragment block, keep
    /// their text.
    #[test]
    fn doc_comment_examples_are_formatted() {
        let source = concat!(
            "/// Adds.\n",
            "///\n",
            "/// ```chimi\n",
            "/// import std/test as Test\n",
            "/// Test.assert_equal(  add(1,2) ,3 )\n",
            "///     let x=add(2,2)\n",
            "/// Test.assert_equal(x,4)\n",
            "/// ```\n",
            "///\n",
            "/// ```chimi\n",
            "/// add(  1,\n",
            "/// ```\n",
            "///\n",
            "/// ```text\n",
            "/// add(  1,2)\n",
            "/// ```\n",
            "///\n",
            "/// ```chimi fragment\n",
            "/// add(  1,2)\n",
            "/// ```\n",
            "pub fn add(left:Int,right:Int)->Int{left+right}\n",
        );
        let first = format(source);
        assert!(!first.is_blocked());
        assert_eq!(
            first.text(),
            concat!(
                "/// Adds.\n",
                "///\n",
                "/// ```chimi\n",
                "/// import std/test as Test\n",
                "///\n",
                "/// Test.assert_equal(add(1, 2), 3)\n",
                "/// let x = add(2, 2)\n",
                "/// Test.assert_equal(x, 4)\n",
                "/// ```\n",
                "///\n",
                "/// ```chimi\n",
                "/// add(  1,\n",
                "/// ```\n",
                "///\n",
                "/// ```text\n",
                "/// add(  1,2)\n",
                "/// ```\n",
                "///\n",
                "/// ```chimi fragment\n",
                "/// add(  1,2)\n",
                "/// ```\n",
                "pub fn add(left: Int, right: Int) -> Int {\n",
                "  left + right\n",
                "}\n",
            )
        );
        assert_eq!(format(first.text()).text(), first.text());
        let examples = crate::doc_examples::examples(first.text());
        assert_eq!(examples.len(), 2, "both chimi blocks are still examples");
        assert_eq!(
            examples[0].text.lines().count(),
            first.text().lines().count(),
            "an example keeps the documented file's layout"
        );
    }

    /// An example without imports is formatted as a body alone, and one that
    /// closes the test early to declare its own items keeps its text.
    #[test]
    fn doc_comment_examples_without_imports_or_with_items() {
        let source = concat!(
            "/// ```chimi\n",
            "/// let x=add(1,2)\n",
            "/// ```\n",
            "///\n",
            "/// ```chimi\n",
            "/// let a=1\n",
            "/// }\n",
            "/// fn g()->Int{2}\n",
            "/// test \"t\" {\n",
            "/// let b=2\n",
            "/// ```\n",
            "pub fn add(left: Int, right: Int) -> Int {\n",
            "  left + right\n",
            "}\n",
        );
        let formatted = format(source);
        assert!(!formatted.is_blocked());
        assert!(formatted.text().starts_with("/// ```chimi\n/// let x = add(1, 2)\n/// ```\n"));
        assert!(formatted.text().contains("/// let a=1\n/// }\n/// fn g()->Int{2}\n"));
    }

    /// A record return type keeps a space before the signature's `needs`.
    #[test]
    fn a_record_return_type_is_spaced_from_needs() {
        let source = "fn f() -> { a: Int } needs { x: X } {\n  { a: 1 }\n}\n";
        let formatted = format(source);
        assert!(!formatted.is_blocked());
        assert!(formatted.text().starts_with("fn f() -> { a: Int } needs {"), "{}", formatted.text());
        assert_eq!(format(formatted.text()).text(), formatted.text());
    }

    #[test]
    fn canonical_format_is_idempotent_and_preserves_comments() {
        let source = concat!(
            "// comments survive canonical formatting\n",
            "struct   Money{cents:Int}\n",
            "fn add(left:Money,right:Money)->Money{Money(left.cents+right.cents)}\n",
        );
        let first = format(source);
        assert!(!first.is_blocked());
        assert!(first.is_changed());
        assert!(
            first
                .text()
                .contains("// comments survive canonical formatting")
        );

        let second = format(first.text());
        assert!(!second.is_blocked());
        assert!(!second.is_changed());
        assert_eq!(second.text(), first.text());
    }

    /// The embedded standard tree is kept in canonical format, which covers
    /// `native fn` headers and `errors` rows after type arguments.
    #[test]
    fn embedded_standard_modules_are_canonically_formatted() {
        for (path, text) in crate::standard_tree::EMBEDDED_MODULES {
            let report = format(text);
            assert!(
                !report.is_blocked(),
                "{path}: {}",
                report.render_diagnostics()
            );
            assert_eq!(report.text(), *text, "{path} is not in canonical format");
        }
    }

    #[test]
    fn test_items_format_like_functions_without_parameters() {
        let report =
            format("fn double(v:Int)->Int{v*2}\ntest   \"doubles\"{let test=double(2)\nVoid}\n");
        assert!(!report.is_blocked(), "{}", report.render_diagnostics());
        assert_eq!(
            report.text(),
            concat!(
                "fn double(v: Int) -> Int {\n  v * 2\n}\n\n",
                "test \"doubles\" {\n  let test = double(2)\n  Void\n}\n",
            )
        );
        assert!(!format(report.text()).is_changed());
    }

    #[test]
    fn malformed_source_is_returned_byte_for_byte_with_syntax_diagnostics() {
        let source = "pub fn main() {\n  Answer(value 42)\n}\n";
        let report = format(source);

        assert!(report.is_blocked());
        assert!(!report.is_changed());
        assert_eq!(report.text(), source);
        assert!(!report.diagnostics().is_empty());
        assert!(
            report
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.stage() == crate::diagnostic::DiagnosticStage::Syntax)
        );
    }

    #[test]
    fn arithmetic_operators_space_canonically_but_import_paths_stay_tight() {
        let source = concat!(
            "import std/float as Float\n",
            "fn f(a: Int, b: Float) -> Int { 20/2/5 + a%3*-a }\n",
        );
        let report = format(source);
        assert!(!report.is_blocked(), "{}", report.render_diagnostics());
        assert!(report.text().starts_with("import std/float as Float\n"));
        assert!(
            report.text().contains("  20 / 2 / 5 + a % 3 * -a\n"),
            "{}",
            report.text()
        );
    }

    #[test]
    fn adjacent_blocks_remain_separate_expressions() {
        let source = "fn f() -> Int {\n  { 1 }\n  { 2 }\n}\n";
        let report = format(source);

        assert!(!report.is_blocked(), "{}", report.render_diagnostics());
        assert!(report.text().contains("}\n  {"), "{}", report.text());
        let reparsed = format(report.text());
        assert!(!reparsed.is_blocked(), "{}", reparsed.render_diagnostics());
    }

    #[test]
    fn a_short_record_or_update_stays_on_one_line() {
        let source = concat!(
            "fn f(base: Config) -> Int {\n",
            "  let updated = {\n    ..base,\n    name: \"b\",\n    jobs: 2,\n  }\n",
            "  let point = {x:1, y: { z: 2 }}\n",
            "  g(Holder({ y: 20 }), [{ a: 1 }])\n",
            "  { ..updated, jobs: 3 }.jobs\n",
            "}\n",
        );
        let report = format(source);
        assert!(!report.is_blocked(), "{}", report.render_diagnostics());
        let text = report.text();
        assert!(
            text.contains("  let updated = { ..base, name: \"b\", jobs: 2 }\n"),
            "{text}"
        );
        assert!(
            text.contains("  let point = { x: 1, y: { z: 2 } }\n"),
            "{text}"
        );
        assert!(
            text.contains("  g(Holder({ y: 20 }), [{ a: 1 }])\n"),
            "{text}"
        );
        assert!(text.contains("  { ..updated, jobs: 3 }.jobs\n"), "{text}");
        assert_eq!(format(text).text(), text);
    }

    #[test]
    fn a_record_that_is_long_or_holds_a_block_keeps_its_braces_on_their_own_lines() {
        let long = format!("const c = {{ first: \"{}\", second: 2 }}\n", "x".repeat(70));
        let report = format(&long);
        assert!(report.text().contains("{\n  first: "), "{}", report.text());
        // A one-line function stays inline; one whose body spans lines is a block.
        let provider = "const fake = { read: fn(path: Text) {\n  40\n} }\n";
        let report = format(provider);
        assert!(
            report.text().starts_with("const fake = {\n"),
            "{}",
            report.text()
        );
        assert_eq!(format(report.text()).text(), report.text());
    }

    #[test]
    fn a_record_type_stays_on_the_line_of_its_declaration() {
        let source = concat!(
            "fn sum(point: {\n  x: Int,\n  y: Int\n}, items: List<{ a: Int }>) -> { total: Int } {\n",
            "  { total: point.x + point.y }\n",
            "}\n",
        );
        let report = format(source);
        assert!(!report.is_blocked(), "{}", report.render_diagnostics());
        assert!(
            report.text().starts_with(
                "fn sum(point: { x: Int, y: Int }, items: List<{ a: Int }>) -> { total: Int } {\n"
            ),
            "{}",
            report.text()
        );
        assert_eq!(format(report.text()).text(), report.text());
    }

    #[test]
    fn contents_of_a_multi_line_call_or_list_indent_one_level() {
        let source = concat!(
            "fn f(\n",
            "a: Int,\n",
            "b: Int,\n",
            ") -> Int {\n",
            "  g(1,\n",
            "  2)\n",
            "  h([\n",
            "  (1, 2),\n",
            "  (3, 4),\n",
            "  ])\n",
            "}\n",
        );
        let report = format(source);
        assert!(!report.is_blocked(), "{}", report.render_diagnostics());
        assert_eq!(
            report.text(),
            concat!(
                "fn f(\n",
                "  a: Int,\n",
                "  b: Int,\n",
                ") -> Int {\n",
                "  g(1,\n",
                "    2\n",
                "  )\n",
                "  h([\n",
                "    (1, 2),\n",
                "    (3, 4),\n",
                "  ])\n",
                "}\n",
            )
        );
        assert_eq!(format(report.text()).text(), report.text());
    }

    #[test]
    fn a_one_line_function_argument_stays_on_one_line() {
        let source = concat!(
            "fn f(items: List<Int>) -> List<Int> {\n",
            "  let add = { apply: fn(value: Int) -> Int { value + 10 } }\n",
            "  List.map(items, fn(item: Int) -> Int { item * 2 })\n",
            "}\n",
        );
        let report = format(source);
        assert!(!report.is_blocked(), "{}", report.render_diagnostics());
        assert_eq!(report.text(), source);
    }

    #[test]
    fn a_block_argument_opens_right_after_the_parenthesis() {
        let long = "x".repeat(70);
        let source = format!("fn f() -> Text {{\n  render({{ first: \"{long}\" }})\n}}\n");
        let report = format(&source);
        assert!(!report.is_blocked(), "{}", report.render_diagnostics());
        assert!(report.text().contains("  render({\n"), "{}", report.text());
    }

    /// The repository's programs are the formatter's showcase: each must already
    /// be in canonical form.
    #[test]
    fn every_repository_program_is_canonically_formatted() {
        fn chimi_files(directory: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    chimi_files(&path, found);
                } else if path
                    .extension()
                    .is_some_and(|extension| extension == "chimi")
                {
                    found.push(path);
                }
            }
        }
        let programs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs");
        let mut files = Vec::new();
        chimi_files(&programs, &mut files);
        assert!(
            !files.is_empty(),
            "no programs under {}",
            programs.display()
        );
        for path in files {
            let source = std::fs::read_to_string(&path).unwrap();
            let report = format(&source);
            assert!(!report.is_blocked(), "{}", report.render_diagnostics());
            assert_eq!(
                report.text(),
                source,
                "{} is not canonically formatted",
                path.display()
            );
        }
    }

    #[test]
    fn type_arguments_unary_minus_and_comparisons_use_distinct_spacing() {
        let source = concat!(
            "struct Box { values: List<Int> }\n",
            "const negative=-100\n",
            "const compared=negative<0\n",
        );
        let report = format(source);

        assert!(!report.is_blocked(), "{}", report.render_diagnostics());
        assert!(report.text().contains("values: List<Int>"));
        assert!(report.text().contains("negative = -100"));
        assert!(report.text().contains("negative < 0"));
    }

    #[test]
    fn accepted_golden_corpus_is_idempotent_and_preserves_comment_text() {
        let cases = [
            include_str!("../../../prototypes/golden/valid/composition.chimi"),
            include_str!("../../../prototypes/golden/valid/dependencies_and_execution.chimi"),
            include_str!("../../../prototypes/golden/valid/expressions_and_matching.chimi"),
            include_str!("../../../prototypes/golden/valid/functions_as_values.chimi"),
            include_str!("../../../prototypes/golden/valid/values_and_variants.chimi"),
            include_str!("../../../prototypes/golden/valid/worker_forcing_example.chimi"),
            include_str!("../../../prototypes/golden/valid/modules/money.chimi"),
            include_str!("../../../prototypes/golden/valid/modules/use_money.chimi"),
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
            let first = format(source);
            assert!(!first.is_blocked(), "{}", first.render_diagnostics());
            let second = format(first.text());
            assert!(!second.is_blocked(), "{}", second.render_diagnostics());
            assert_eq!(second.text(), first.text());

            let source_comments = source
                .lines()
                .map(str::trim_start)
                .filter(|line| line.starts_with("//"))
                .collect::<Vec<_>>();
            let formatted_comments = first
                .text()
                .lines()
                .map(str::trim_start)
                .filter(|line| line.starts_with("//"))
                .collect::<Vec<_>>();
            assert_eq!(formatted_comments, source_comments);
        }
    }
}
