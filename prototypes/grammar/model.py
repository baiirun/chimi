"""Pure logic for the disposable Chimi grammar prototype."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import re


KEYWORDS = {
    "pub": "KW_PUB",
    "import": "KW_IMPORT",
    "as": "KW_AS",
    "struct": "KW_STRUCT",
    "enum": "KW_ENUM",
    "interface": "KW_INTERFACE",
    "fn": "KW_FN",
    "let": "KW_LET",
    "case": "KW_CASE",
    "if": "KW_IF",
    "errors": "KW_ERRORS",
    "needs": "KW_NEEDS",
    "using": "KW_USING",
    "True": "TRUE",
    "False": "FALSE",
    "Void": "VOID",
}

MULTI_TOKENS = ("->", "|>", ">=", "<=")
SINGLE_TOKENS = set("(){}[],:.<>+-*/=")
TRIVIA = {"WHITESPACE", "COMMENT"}
LITERALS = {"INT", "TEXT", "TRUE", "FALSE"}


@dataclass(frozen=True)
class Token:
    kind: str
    text: str
    line: int
    column: int


@dataclass(frozen=True)
class Issue:
    message: str
    line: int
    column: int


@dataclass(frozen=True)
class FixtureAnalysis:
    path: Path
    source: str
    tokens: tuple[Token, ...]
    lexical_issues: tuple[Issue, ...]
    syntax_issue: Issue | None
    expected_syntax: str
    expected_compile: str
    expected_diagnostic: str | None
    constructs: tuple[str, ...]

    @property
    def syntax_result(self) -> str:
        return "failure" if self.syntax_issue else "success"

    @property
    def matches_expectation(self) -> bool:
        return not self.lexical_issues and self.syntax_result == self.expected_syntax

    @property
    def significant_tokens(self) -> tuple[Token, ...]:
        return tuple(token for token in self.tokens if token.kind not in TRIVIA)


class ParseFailure(Exception):
    def __init__(self, message: str, token: Token):
        super().__init__(message)
        self.issue = Issue(message, token.line, token.column)


def lex(source: str) -> tuple[tuple[Token, ...], tuple[Issue, ...]]:
    tokens: list[Token] = []
    issues: list[Issue] = []
    index = 0
    line = 1
    column = 1

    def emit(kind: str, text: str, start_line: int, start_column: int) -> None:
        tokens.append(Token(kind, text, start_line, start_column))

    while index < len(source):
        char = source[index]
        start_line = line
        start_column = column

        if char == "\n":
            emit("NEWLINE", char, start_line, start_column)
            index += 1
            line += 1
            column = 1
            continue

        if char in " \t\r":
            end = index + 1
            while end < len(source) and source[end] in " \t\r":
                end += 1
            text = source[index:end]
            emit("WHITESPACE", text, start_line, start_column)
            index = end
            column += len(text)
            continue

        if source.startswith("//", index):
            end = source.find("\n", index)
            if end == -1:
                end = len(source)
            text = source[index:end]
            emit("COMMENT", text, start_line, start_column)
            index = end
            column += len(text)
            continue

        if char == '"':
            end = index + 1
            escaped = False
            while end < len(source):
                current = source[end]
                if current == "\n":
                    break
                if escaped:
                    if current not in {'"', "\\", "n", "r", "t"}:
                        issues.append(
                            Issue(
                                f"unsupported escape \\{current}",
                                start_line,
                                start_column + (end - index) - 1,
                            )
                        )
                    escaped = False
                elif current == "\\":
                    escaped = True
                elif current == '"':
                    end += 1
                    break
                end += 1
            text = source[index:end]
            if not text.endswith('"') or len(text) == 1:
                issues.append(Issue("unterminated text literal", start_line, start_column))
            emit("TEXT", text, start_line, start_column)
            index = end
            column += len(text)
            continue

        matched = next((token for token in MULTI_TOKENS if source.startswith(token, index)), None)
        if matched:
            emit(matched, matched, start_line, start_column)
            index += len(matched)
            column += len(matched)
            continue

        if (char.isascii() and char.isalpha()) or char == "_":
            end = index + 1
            while end < len(source) and (
                (source[end].isascii() and source[end].isalnum()) or source[end] == "_"
            ):
                end += 1
            text = source[index:end]
            emit(KEYWORDS.get(text, "IDENT"), text, start_line, start_column)
            index = end
            column += len(text)
            continue

        if char.isdigit():
            end = index + 1
            while end < len(source) and (source[end].isdigit() or source[end] == "_"):
                end += 1
            text = source[index:end]
            if text.endswith("_") or "__" in text:
                issues.append(Issue("invalid underscore placement in integer", start_line, start_column))
            emit("INT", text, start_line, start_column)
            index = end
            column += len(text)
            continue

        if char in SINGLE_TOKENS:
            emit(char, char, start_line, start_column)
            index += 1
            column += 1
            continue

        issues.append(Issue(f"unrecognized character {char!r}", start_line, start_column))
        emit("ERROR", char, start_line, start_column)
        index += 1
        column += 1

    tokens.append(Token("EOF", "", line, column))
    issues.extend(_delimiter_issues(tokens))
    return tuple(tokens), tuple(issues)


def _delimiter_issues(tokens: list[Token]) -> list[Issue]:
    pairs = {")": "(", "]": "[", "}": "{"}
    stack: list[Token] = []
    issues: list[Issue] = []
    for token in tokens:
        if token.kind in {"(", "[", "{"}:
            stack.append(token)
        elif token.kind in pairs:
            if not stack or stack[-1].kind != pairs[token.kind]:
                issues.append(Issue(f"unmatched {token.text}", token.line, token.column))
            else:
                stack.pop()
    for token in stack:
        issues.append(Issue(f"unclosed {token.text}", token.line, token.column))
    return issues


class Parser:
    def __init__(self, tokens: tuple[Token, ...]):
        self.tokens = tuple(token for token in tokens if token.kind not in TRIVIA)
        self.position = 0

    @property
    def current(self) -> Token:
        return self.tokens[self.position]

    def at(self, kind: str) -> bool:
        return self.current.kind == kind

    def bump(self) -> Token:
        token = self.current
        if token.kind != "EOF":
            self.position += 1
        return token

    def accept(self, kind: str) -> Token | None:
        if self.at(kind):
            return self.bump()
        return None

    def expect(self, kind: str, message: str | None = None) -> Token:
        if not self.at(kind):
            expected = message or f"expected {kind}, found {self.current.text or 'end of file'}"
            raise ParseFailure(expected, self.current)
        return self.bump()

    def skip_newlines(self) -> int:
        count = 0
        while self.accept("NEWLINE"):
            count += 1
        return count

    def next_non_newline(self, offset: int = 1) -> Token:
        index = self.position + offset
        while index < len(self.tokens) and self.tokens[index].kind == "NEWLINE":
            index += 1
        return self.tokens[min(index, len(self.tokens) - 1)]

    def parse_file(self) -> None:
        self.skip_newlines()
        while not self.at("EOF"):
            self.parse_top_item()
            if self.at("EOF"):
                break
            if not self.skip_newlines():
                raise ParseFailure("expected a line break between top-level items", self.current)
        self.expect("EOF")

    def parse_top_item(self) -> None:
        if self.at("KW_IMPORT"):
            self.parse_import()
            return
        if self.at("KW_PUB") or self.at("KW_STRUCT") or self.at("KW_ENUM") or self.at("KW_INTERFACE"):
            self.parse_declaration()
            return
        if self.at("KW_FN") and self.next_non_newline().kind == "IDENT":
            self.parse_declaration()
            return
        if self.at("KW_LET"):
            self.parse_let()
            return
        self.parse_expression()

    def parse_import(self) -> None:
        self.expect("KW_IMPORT")
        self.expect("IDENT", "expected a module path after import")
        while self.accept("/"):
            self.expect("IDENT", "expected a module path segment")
        if self.accept("KW_AS"):
            self.expect("IDENT", "expected a module alias after as")
        elif self.accept("."):
            self.expect("{")
            self.skip_newlines()
            self.expect("IDENT", "expected an explicitly imported member")
            while self.accept(","):
                self.skip_newlines()
                if self.at("}"):
                    break
                self.expect("IDENT", "expected an explicitly imported member")
            self.skip_newlines()
            self.expect("}")
        else:
            raise ParseFailure(
                "module imports require an explicit alias with `as` or an explicit member list",
                self.current,
            )

    def parse_declaration(self) -> None:
        self.accept("KW_PUB")
        self.skip_newlines()
        if self.accept("KW_STRUCT"):
            self.parse_product_body()
        elif self.accept("KW_ENUM"):
            self.parse_enum_body()
        elif self.accept("KW_INTERFACE"):
            self.parse_product_body()
        elif self.at("KW_FN"):
            self.parse_function_declaration()
        else:
            raise ParseFailure("expected a declaration after pub", self.current)

    def parse_product_body(self) -> None:
        self.expect("IDENT", "expected a declaration name")
        self.parse_optional_generic_params()
        self.skip_newlines()
        self.expect("{")
        self.skip_newlines()
        while not self.at("}"):
            self.parse_named_type_field()
            if self.accept(","):
                self.skip_newlines()
            elif not self.at("}") and not self.skip_newlines():
                raise ParseFailure("expected a line break or comma between fields", self.current)
        self.expect("}")

    def parse_enum_body(self) -> None:
        self.expect("IDENT", "expected an enum name")
        self.parse_optional_generic_params()
        self.skip_newlines()
        self.expect("{")
        self.skip_newlines()
        while not self.at("}"):
            self.expect("IDENT", "expected an enum variant")
            if self.accept("("):
                self.skip_newlines()
                if not self.at(")"):
                    self.parse_named_type_field()
                    while self.accept(","):
                        self.skip_newlines()
                        if self.at(")"):
                            break
                        self.parse_named_type_field()
                self.skip_newlines()
                self.expect(")")
            if self.accept(","):
                self.skip_newlines()
            elif not self.at("}") and not self.skip_newlines():
                raise ParseFailure("expected a line break or comma between variants", self.current)
        self.expect("}")

    def parse_optional_generic_params(self) -> None:
        if not self.accept("<"):
            return
        self.skip_newlines()
        self.expect("IDENT", "expected a generic parameter name")
        while self.accept(","):
            self.skip_newlines()
            self.expect("IDENT", "expected a generic parameter name")
        self.skip_newlines()
        self.expect(">")

    def parse_named_type_field(self) -> None:
        self.expect("IDENT", "expected a field name")
        self.skip_newlines()
        self.expect(":")
        self.skip_newlines()
        self.parse_type()

    def parse_function_declaration(self) -> None:
        self.expect("KW_FN")
        self.expect("IDENT", "expected a function name")
        self.parse_optional_generic_params()
        self.skip_newlines()
        self.parse_value_params()
        self.parse_implementation_contract()
        self.skip_newlines()
        self.parse_block()

    def parse_implementation_contract(self) -> None:
        self.skip_newlines()
        if self.accept("->"):
            self.skip_newlines()
            self.parse_type()
            self.skip_newlines()
        if self.at("KW_NEEDS"):
            self.parse_requirement_clause()

    def parse_value_params(self) -> None:
        self.expect("(")
        self.skip_newlines()
        if not self.at(")"):
            self.parse_value_param()
            while self.accept(","):
                self.skip_newlines()
                if self.at(")"):
                    break
                self.parse_value_param()
        self.skip_newlines()
        self.expect(")")

    def parse_value_param(self) -> None:
        self.expect("IDENT", "expected a parameter name")
        if self.accept(":"):
            self.skip_newlines()
            self.parse_type()

    def parse_type(self) -> None:
        self.skip_newlines()
        if self.accept("KW_FN"):
            self.skip_newlines()
            self.expect("(")
            self.skip_newlines()
            if not self.at(")"):
                self.parse_type()
                while self.accept(","):
                    self.skip_newlines()
                    if self.at(")"):
                        break
                    self.parse_type()
            self.skip_newlines()
            self.expect(")")
            self.skip_newlines()
            self.expect("->")
            self.skip_newlines()
            self.parse_type()
            self.skip_newlines()
            if self.accept("KW_ERRORS"):
                self.skip_newlines()
                self.parse_type()
                self.skip_newlines()
            if self.at("KW_NEEDS"):
                self.parse_requirement_clause()
            return

        if self.accept("IDENT"):
            while self.accept("."):
                self.expect("IDENT", "expected a type name after '.'")
        elif not self.accept("VOID"):
            raise ParseFailure("expected a type", self.current)
        if self.accept("<"):
            self.skip_newlines()
            self.parse_type()
            while self.accept(","):
                self.skip_newlines()
                self.parse_type()
            self.skip_newlines()
            self.expect(">")

    def parse_requirement_clause(self) -> None:
        self.expect("KW_NEEDS")
        self.skip_newlines()
        self.parse_requirement_atom()
        while True:
            checkpoint = self.position
            self.skip_newlines()
            if not self.accept("+"):
                self.position = checkpoint
                break
            self.skip_newlines()
            self.parse_requirement_atom()

    def parse_requirement_atom(self) -> None:
        if self.accept("IDENT"):
            return
        self.expect("{", "expected a requirement row variable or record")
        self.skip_newlines()
        if self.at("}"):
            raise ParseFailure("empty requirement records are invalid", self.current)
        self.parse_named_type_field()
        while self.accept(","):
            self.skip_newlines()
            if self.at("}"):
                break
            self.parse_named_type_field()
        self.skip_newlines()
        self.expect("}")

    def parse_let(self) -> None:
        self.expect("KW_LET")
        self.expect("IDENT", "expected a binding name")
        self.expect("=")
        self.parse_expression()

    def parse_expression(self) -> None:
        self.parse_using()

    def parse_using(self) -> None:
        self.parse_pipeline()
        while True:
            checkpoint = self.position
            self.skip_newlines()
            if not self.accept("KW_USING"):
                self.position = checkpoint
                return
            self.skip_newlines()
            self.parse_record()

    def parse_pipeline(self) -> None:
        self.parse_comparison()
        while True:
            checkpoint = self.position
            self.skip_newlines()
            if not self.accept("|>"):
                self.position = checkpoint
                return
            self.skip_newlines()
            self.parse_comparison()

    def parse_case(self) -> None:
        self.expect("KW_CASE")
        self.parse_expression()
        while self.accept(","):
            self.skip_newlines()
            self.parse_expression()
        self.skip_newlines()
        self.expect("{")
        self.skip_newlines()
        while not self.at("}"):
            self.parse_pattern()
            while self.accept(","):
                self.skip_newlines()
                self.parse_pattern()
            if self.accept("KW_IF"):
                self.parse_expression()
            self.skip_newlines()
            self.expect("->")
            self.skip_newlines()
            self.parse_expression()
            if not self.at("}") and not self.skip_newlines():
                raise ParseFailure("expected a line break between case arms", self.current)
        self.expect("}")

    def parse_pattern(self) -> None:
        if self.current.kind in LITERALS:
            self.bump()
            return
        self.expect("IDENT", "expected a pattern")
        if self.accept("("):
            self.skip_newlines()
            if not self.at(")"):
                self.parse_pattern()
                while self.accept(","):
                    self.skip_newlines()
                    self.parse_pattern()
            self.skip_newlines()
            self.expect(")")

    def parse_function_expression(self) -> None:
        self.expect("KW_FN")
        self.skip_newlines()
        self.parse_value_params()
        self.parse_implementation_contract()
        self.skip_newlines()
        self.parse_block()

    def parse_comparison(self) -> None:
        self.parse_additive()
        self.parse_binary_tail({"<", "<=", ">", ">="}, self.parse_additive)

    def parse_additive(self) -> None:
        self.parse_multiplicative()
        self.parse_binary_tail({"+", "-"}, self.parse_multiplicative)

    def parse_multiplicative(self) -> None:
        self.parse_unary()
        self.parse_binary_tail({"*", "/"}, self.parse_unary)

    def parse_binary_tail(self, operators: set[str], operand) -> None:
        while True:
            checkpoint = self.position
            self.skip_newlines()
            if self.current.kind not in operators:
                self.position = checkpoint
                return
            self.bump()
            self.skip_newlines()
            operand()

    def parse_unary(self) -> None:
        if self.accept("-"):
            self.skip_newlines()
        self.parse_postfix()

    def parse_postfix(self) -> None:
        self.parse_primary()
        while True:
            if self.accept("."):
                self.expect("IDENT", "expected a member name after .")
            elif self.at("("):
                self.parse_call_suffix()
            else:
                return

    def parse_primary(self) -> None:
        if self.current.kind in LITERALS or self.at("IDENT"):
            self.bump()
            return
        if self.accept("("):
            self.skip_newlines()
            self.parse_expression()
            self.skip_newlines()
            self.expect(")")
            return
        if self.at("["):
            self.parse_list()
            return
        if self.at("{"):
            self.parse_brace_expression()
            return
        if self.at("KW_CASE"):
            self.parse_case()
            return
        if self.at("KW_FN"):
            self.parse_function_expression()
            return
        if self.at("VOID"):
            raise ParseFailure("Void is a type, not a value", self.current)
        if self.at("KW_IF"):
            raise ParseFailure("standalone if is not an expression; use case", self.current)
        raise ParseFailure(f"expected an expression, found {self.current.text or 'end of file'}", self.current)

    def parse_call_suffix(self) -> None:
        self.expect("(")
        self.skip_newlines()
        if not self.at(")"):
            self.parse_argument()
            while self.accept(","):
                self.skip_newlines()
                if self.at(")"):
                    break
                self.parse_argument()
        self.skip_newlines()
        self.expect(")")

    def parse_argument(self) -> None:
        if self.at("IDENT") and self.tokens[self.position + 1].kind == ":":
            self.bump()
            self.bump()
        self.parse_expression()

    def parse_list(self) -> None:
        self.expect("[")
        self.skip_newlines()
        if not self.at("]"):
            self.parse_expression()
            while self.accept(","):
                self.skip_newlines()
                if self.at("]"):
                    break
                self.parse_expression()
        self.skip_newlines()
        self.expect("]")

    def parse_brace_expression(self) -> None:
        checkpoint = self.position
        self.expect("{")
        self.skip_newlines()
        if self.at("}"):
            raise ParseFailure("empty braces are invalid", self.current)
        is_record = self.at("IDENT") and self.tokens[self.position + 1].kind == ":"
        self.position = checkpoint
        if is_record:
            self.parse_record()
        else:
            self.parse_block()

    def parse_record(self) -> None:
        self.expect("{")
        self.skip_newlines()
        self.parse_record_field()
        while self.accept(","):
            self.skip_newlines()
            if self.at("}"):
                break
            self.parse_record_field()
        self.skip_newlines()
        self.expect("}")

    def parse_record_field(self) -> None:
        self.expect("IDENT", "expected a record field name")
        self.expect(":")
        self.parse_expression()

    def parse_block(self) -> None:
        self.expect("{")
        self.skip_newlines()
        if self.at("}"):
            raise ParseFailure("empty braces are invalid", self.current)
        while not self.at("}"):
            if self.at("KW_LET"):
                self.parse_let()
            else:
                self.parse_expression()
            if not self.at("}") and not self.skip_newlines():
                raise ParseFailure("expected a line break between block items", self.current)
        self.expect("}")


def analyze_fixture(path: Path) -> FixtureAnalysis:
    source = path.read_text(encoding="utf-8")
    tokens, lexical_issues = lex(source)
    syntax_issue: Issue | None = None
    if not lexical_issues:
        try:
            Parser(tokens).parse_file()
        except ParseFailure as failure:
            syntax_issue = failure.issue

    expected_compile_match = re.search(r"^// EXPECT-COMPILE: (success|failure)$", source, re.MULTILINE)
    diagnostic_match = re.search(r"^// EXPECT-DIAGNOSTIC: ([A-Z0-9_]+)$", source, re.MULTILINE)
    expected_compile = expected_compile_match.group(1) if expected_compile_match else "unknown"
    expected_diagnostic = diagnostic_match.group(1) if diagnostic_match else None
    expected_syntax_match = re.search(
        r"^// EXPECT-SYNTAX: (success|failure)$", source, re.MULTILINE
    )
    expected_syntax = expected_syntax_match.group(1) if expected_syntax_match else "success"

    keyword_constructs = {
        "KW_IMPORT": "imports",
        "KW_STRUCT": "structs",
        "KW_ENUM": "enums",
        "KW_INTERFACE": "interfaces",
        "KW_FN": "functions",
        "KW_CASE": "case expressions",
        "KW_NEEDS": "requirements",
        "KW_USING": "dependency provision",
        "KW_ERRORS": "error contracts",
    }
    token_kinds = {token.kind for token in tokens}
    constructs = tuple(label for kind, label in keyword_constructs.items() if kind in token_kinds)

    return FixtureAnalysis(
        path=path,
        source=source,
        tokens=tokens,
        lexical_issues=lexical_issues,
        syntax_issue=syntax_issue,
        expected_syntax=expected_syntax,
        expected_compile=expected_compile,
        expected_diagnostic=expected_diagnostic,
        constructs=constructs,
    )


def analyze_corpus(root: Path) -> tuple[FixtureAnalysis, ...]:
    paths = sorted(root.rglob("*.chimi"), key=lambda path: str(path.relative_to(root)))
    return tuple(analyze_fixture(path) for path in paths)
