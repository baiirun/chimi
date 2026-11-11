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

        Self {
            formatted: format_parse(&parsed),
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

fn format_parse(parsed: &ParseResult) -> String {
    let tokens = parsed.root().descendant_tokens();
    let mut writer = Writer::default();
    let mut previous = None;
    let mut saw_newline = false;

    for (index, token) in tokens.iter().enumerate() {
        let kind = token.kind();
        if matches!(kind, TokenKind::End | TokenKind::Whitespace) || token.is_missing() {
            continue;
        }
        let next = next_significant(&tokens, index + 1);
        match kind {
            TokenKind::Newline => {
                if saw_newline {
                    writer.blank_line();
                } else if !should_join_line(previous, next) || writer.line_is_empty() {
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
                writer.space();
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
                if !continues_requirement
                    && !matches!(
                        next,
                        Some(TokenKind::Comma | TokenKind::RightParen | TokenKind::Using)
                    )
                {
                    writer.flush();
                    if writer.brace_depth == 0 {
                        writer.blank_line();
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
            TokenKind::RightParen | TokenKind::RightBracket => {
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
            TokenKind::Minus if token_has_ancestor(token, &[NodeKind::UnaryExpression]) => {
                writer.write("-");
            }
            TokenKind::Arrow
            | TokenKind::Equal
            | TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Less
            | TokenKind::LessEqual
            | TokenKind::Greater
            | TokenKind::GreaterEqual
            | TokenKind::And
            | TokenKind::Or => {
                writer.space();
                writer.write(token.text());
                writer.write(" ");
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
        }
    }

    writer.finish()
}

fn should_join_line(previous: Option<TokenKind>, next: Option<TokenKind>) -> bool {
    matches!(next, Some(TokenKind::Needs))
        || next == Some(TokenKind::LeftBrace)
            && matches!(
                previous,
                Some(TokenKind::RightParen | TokenKind::RightBrace)
            )
}

fn is_requirement_syntax(token: &SyntaxToken<'_>) -> bool {
    token_has_ancestor(
        token,
        &[NodeKind::RequirementClause, NodeKind::RequirementRecord],
    )
}

fn is_type_syntax(token: &SyntaxToken<'_>) -> bool {
    token_has_ancestor(
        token,
        &[
            NodeKind::Type,
            NodeKind::FunctionType,
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
    word_kind(previous) && word_kind(current)
        || matches!(previous, TokenKind::RightParen | TokenKind::RightBracket)
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
            | TokenKind::Text
    )
}

#[derive(Default)]
struct Writer {
    output: String,
    line: String,
    indent: usize,
    brace_depth: usize,
}

impl Writer {
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
    fn adjacent_blocks_remain_separate_expressions() {
        let source = "fn f() -> Int {\n  { 1 }\n  { 2 }\n}\n";
        let report = format(source);

        assert!(!report.is_blocked(), "{}", report.render_diagnostics());
        assert!(report.text().contains("}\n  {"), "{}", report.text());
        let reparsed = format(report.text());
        assert!(!reparsed.is_blocked(), "{}", reparsed.render_diagnostics());
    }

    #[test]
    fn type_arguments_unary_minus_and_comparisons_use_distinct_spacing() {
        let source = concat!(
            "struct Box { values: List<Int> }\n",
            "let negative=-100\n",
            "let compared=negative<0\n",
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
