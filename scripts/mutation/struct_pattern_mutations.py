"""Mutants for struct patterns by field name (CHI-291, second part)."""

CRATE = "compiler/chimi"

MUTATIONS = [
    dict(
        name="field patterns keep their written order",
        file="src/semantic.rs",
        edits=[("            .map(|field| written.iter().position(|(name, _)| name.text == *field))", "            .enumerate()\n                .map(|(index, _)| (index < written.len()).then_some(index))")],
        targets=["language"],
        filter="struct_pattern",
    ),
    dict(
        name="a missing field without rest is accepted",
        file="src/semantic.rs",
        edits=[("        if !missing.is_empty() && !rest {", "        if false && !missing.is_empty() && !rest {")],
        targets=["language"],
        filter="struct_pattern",
    ),
    dict(
        name="a rest before a field is accepted",
        file="src/semantic.rs",
        edits=[("        if rests.len() > 1\n            || rests", "        if false && rests.len() > 1\n            || false && rests")],
        targets=["language"],
        filter="struct_pattern",
    ),
    dict(
        name="a field named twice in a pattern is accepted",
        file="src/semantic.rs",
        edits=[("                    format!(\"field `{}` is named more than once\", name.text),\n                );\n                valid = false;\n            } else if !declared.contains(&name.text) {\n                self.type_error(\n                    \"T4205\",\n                    name.range,\n                    format!(\"`{struct_name}` has no field `{}`\", name.text),\n                );\n                valid = false;\n            }\n        }\n        let missing = declared\n            .iter()\n            .filter(|field| !written.iter().any(|(name, _)| name.text == **field))\n            .map(|field| format!(\"`{field}`\"))\n            .collect::<Vec<_>>();\n        if !missing.is_empty() && !rest {", "                    format!(\"field `{}` is named more than once\", name.text),\n                );\n            } else if !declared.contains(&name.text) {\n                self.type_error(\n                    \"T4205\",\n                    name.range,\n                    format!(\"`{struct_name}` has no field `{}`\", name.text),\n                );\n                valid = false;\n            }\n        }\n        let missing = declared\n            .iter()\n            .filter(|field| !written.iter().any(|(name, _)| name.text == **field))\n            .map(|field| format!(\"`{field}`\"))\n            .collect::<Vec<_>>();\n        if !missing.is_empty() && !rest {")],
        targets=["language"],
        filter="struct_pattern",
    ),
    dict(
        name="positional struct patterns are accepted",
        file="src/typing.rs",
        edits=[("                if *form == CallForm::Positional {\n                    self.reject_positional_struct_pattern(", "                if false && *form == CallForm::Positional {\n                    self.reject_positional_struct_pattern(")],
        targets=["language"],
        filter="struct_pattern",
    ),
    dict(
        name="braces after a variant pattern are silent",
        file="src/typing/struct_literals.rs",
        edits=[("            \"T4217\",\n            range,\n            format!(\n                \"`{name}` is an enum variant; a variant's payload is matched in parentheses", "            \"T4999\",\n            range,\n            format!(\n                \"`{name}` is an enum variant; a variant's payload is matched in parentheses")],
        targets=["language"],
        filter="struct_pattern",
    ),
    dict(
        name="the coverage witness prints a struct positionally",
        file="src/typing.rs",
        edits=[("                    MatchTag::EnumVariant(symbol) if self.is_struct_constructor(*symbol) => {", "                    MatchTag::EnumVariant(symbol) if false && self.is_struct_constructor(*symbol) => {")],
        targets=["language"],
        filter="refutable_field",
    ),
    dict(
        name="a struct pattern's closing brace ends the line",
        file="src/formatter.rs",
        edits=[("        let ends_pattern = token.parent().kind() == NodeKind::Pattern;", "        let ends_pattern = false;")],
        targets=["language"],
        filter="struct_pattern",
    ),
    dict(
        name="a short struct pattern is spread over lines",
        file="src/formatter.rs",
        edits=[("        if (current.kind() == NodeKind::RecordExpression || is_struct_pattern(&current))", "        if (current.kind() == NodeKind::RecordExpression)")],
        targets=["language"],
        filter="struct_pattern",
    ),
]
