"""Mutants for building structs by field name (CHI-291): each reintroduces a
plausible bug in parsing, lowering or checking a struct literal."""

CRATE = "compiler/chimi"

MUTATIONS = [
    dict(
        name="fields are passed in written order, not declaration order",
        file="src/semantic.rs",
        edits=[("        if order.iter().enumerate().all(|(index, source)| index == *source) {", "        if true || order.iter().enumerate().all(|(index, source)| index == *source) {")],
        targets=["language"],
        filter="struct",
    ),
    dict(
        name="reordered field values are evaluated last field first",
        file="src/semantic.rs",
        edits=[("            .zip(written)\n            .map(|(name, (_, value))| ModuleBlockItem::Binding {", "            .zip(written)\n            .rev()\n            .map(|(name, (_, value))| ModuleBlockItem::Binding {")],
        targets=["language"],
        filter="struct",
    ),
    dict(
        name="a field named twice is not reported",
        file="src/semantic.rs",
        edits=[("            if written[..index].iter().any(", "            if false && written[..index].iter().any(")],
        targets=["language"],
        filter="struct",
    ),
    dict(
        name="an unknown field is not reported",
        file="src/semantic.rs",
        edits=[("            } else if !declared.contains(&name.text) {", "            } else if false && !declared.contains(&name.text) {")],
        targets=["language"],
        filter="struct",
    ),
    dict(
        name="a missing field is not reported",
        file="src/semantic.rs",
        edits=[("        if !missing.is_empty() {", "        if false && !missing.is_empty() {")],
        targets=["language"],
        filter="struct",
    ),
    dict(
        name="a spread in a struct literal is accepted",
        file="src/semantic.rs",
        edits=[("        if let Some(spread) = direct_child(fields, NodeKind::RecordSpread) {", "        if let Some(spread) = direct_child(fields, NodeKind::RecordSpread).filter(|_| false) {")],
        targets=["language"],
        filter="struct",
    ),
    dict(
        name="positional struct construction is accepted",
        file="src/typing.rs",
        edits=[("                if *form == CallForm::Positional\n", "                if false && *form == CallForm::Positional\n")],
        targets=["language"],
        filter="struct",
    ),
    dict(
        name="the positional rejection carries no fix",
        file="src/typing/struct_literals.rs",
        edits=[("        if written_as_call && !fields.is_empty() {", "        if false && written_as_call && !fields.is_empty() {")],
        targets=["language"],
        filter="struct",
    ),
    dict(
        name="a struct constructor is a function value again",
        file="src/typing.rs",
        edits=[("                } else if self.is_struct_constructor(symbol) {", "                } else if false && self.is_struct_constructor(symbol) {")],
        targets=["language"],
        filter="struct",
    ),
    dict(
        name="labelled struct calls get the positional advice",
        file="src/typing.rs",
        edits=[("        if callee_symbol.is_some_and(|symbol| self.is_struct_constructor(symbol)) {\n            let labelled", "        if false && callee_symbol.is_some_and(|symbol| self.is_struct_constructor(symbol)) {\n            let labelled")],
        targets=["lib"],
        filter="labelled_arguments",
    ),
    dict(
        name="braces after an enum variant or function are silent",
        file="src/typing/struct_literals.rs",
        edits=[("            if !own_struct {", "            if false && !own_struct {")],
        targets=["language"],
        filter="struct",
    ),
    dict(
        name="a case subject reads a struct literal",
        file="src/syntax.rs",
        edits=[("        self.with_struct_literals(false, |parser| {", "        self.with_struct_literals(true, |parser| {")],
        targets=["language"],
        filter="struct",
    ),
    dict(
        name="brackets in a case subject do not allow a struct literal again",
        file="src/syntax.rs",
        edits=[("            TokenKind::LeftParen => self.with_struct_literals(true, Self::parse_parenthesized),", "            TokenKind::LeftParen => self.parse_parenthesized(),")],
        targets=["language"],
        filter="struct",
    ),
]
