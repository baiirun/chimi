//! The debug format of `std/debug` (G10) on the reference interpreter.
//!
//! The native runtime implements the same format in `debug_render` (runtime.c); the
//! language tests run one program on both backends to keep them identical. The format
//! renders each value like the declaration or literal that describes it, never shows a
//! function's captures or a capability's contents, and gives equal values equal text.

use super::{RuntimeCallable, RuntimeValue};
use crate::executable::CoreProgram;
use crate::numeric::render_float;
use crate::standard::{StandardConstructor, StandardSymbol};

/// Renders `value` on one line, or with one field, element, or entry per line and
/// two-space indentation when `indented`.
pub(super) fn render(core: &CoreProgram, value: &RuntimeValue, indented: bool) -> String {
    let mut renderer = Renderer {
        core,
        indented,
        depth: 0,
        output: String::new(),
    };
    renderer.value(value);
    renderer.output
}

struct Renderer<'core> {
    core: &'core CoreProgram,
    indented: bool,
    depth: usize,
    output: String,
}

/// One part of a composite value, with the label written before it, if any.
struct Part<'value> {
    label: Option<String>,
    value: &'value RuntimeValue,
}

impl Renderer<'_> {
    fn value(&mut self, value: &RuntimeValue) {
        match value {
            RuntimeValue::Void => self.output.push_str("Void"),
            RuntimeValue::Integer(value) => self.output.push_str(&value.to_string()),
            // Equal values render equally, and `-0.0 == 0.0`.
            RuntimeValue::Float(value) => {
                self.output
                    .push_str(&render_float(if *value == 0.0 { 0.0 } else { *value }))
            }
            RuntimeValue::Text(text) => self.output.push_str(&quote(text)),
            RuntimeValue::Boolean(value) => {
                self.output.push_str(if *value { "True" } else { "False" })
            }
            RuntimeValue::List(values) => {
                let parts = values.iter().map(|value| Part { label: None, value });
                self.composite("", "[", "]", false, parts.collect());
            }
            RuntimeValue::Tuple(values) => {
                let parts = values.iter().map(|value| Part { label: None, value });
                self.composite("", "(", ")", false, parts.collect());
            }
            RuntimeValue::Record(fields) => {
                let mut fields = fields.iter().collect::<Vec<_>>();
                fields.sort_by(|left, right| left.name.cmp(&right.name));
                let parts = fields.into_iter().map(|field| Part {
                    label: Some(format!("{}: ", field.name)),
                    value: &field.value,
                });
                self.composite("", "{", "}", true, parts.collect());
            }
            RuntimeValue::Constructed {
                constructor,
                type_name,
                variant_name,
                fields,
            } => {
                let prelude = matches!(
                    self.core.symbol(*constructor).standard,
                    Some(StandardSymbol::Constructor(
                        StandardConstructor::OptionSome | StandardConstructor::OptionNone
                    ))
                );
                let head = match variant_name {
                    Some(variant) if prelude => variant.clone(),
                    Some(variant) => format!("{type_name}.{variant}"),
                    None => format!("{type_name} "),
                };
                if variant_name.is_none() {
                    let parts = fields.iter().map(|field| Part {
                        label: Some(format!("{}: ", field.name)),
                        value: &field.value,
                    });
                    self.composite(&head, "{", "}", true, parts.collect());
                } else if fields.is_empty() {
                    self.output.push_str(&head);
                } else {
                    let named = fields.len() > 1;
                    let parts = fields.iter().map(|field| Part {
                        label: named.then(|| format!("{}: ", field.name)),
                        value: &field.value,
                    });
                    self.composite(&head, "(", ")", false, parts.collect());
                }
            }
            RuntimeValue::Map(map) => {
                let entries = map.entries();
                let keys = entries
                    .iter()
                    // A key renders on one line even in the indented format.
                    .map(|(key, _)| format!("{}: ", render(self.core, key, false)))
                    .collect::<Vec<_>>();
                let parts = keys
                    .into_iter()
                    .zip(&entries)
                    .map(|(label, (_, value))| Part {
                        label: Some(label),
                        value,
                    });
                self.composite("Map ", "{", "}", true, parts.collect());
            }
            RuntimeValue::JsonNumber(text) => {
                self.output.push_str("Json.Number(");
                self.output.push_str(&normalize_json_number(text));
                self.output.push(')');
            }
            RuntimeValue::Callable(callable) => {
                let name = match callable {
                    RuntimeCallable::Declared { symbol, .. }
                    | RuntimeCallable::Constructor { symbol, .. } => {
                        Some(self.core.symbol(*symbol).name.clone())
                    }
                    RuntimeCallable::Dependency { name, .. } => Some(name.clone()),
                    RuntimeCallable::Intrinsic(function) => Some(function.name().to_owned()),
                    RuntimeCallable::Closure { .. } | RuntimeCallable::Host(_) => None,
                };
                match name {
                    Some(name) => self.output.push_str(&format!("<fn {name}>")),
                    None => self.output.push_str("<fn>"),
                }
            }
        }
    }

    /// Writes `head`, `open`, the parts separated by `, ` (or one per line when
    /// indented, each followed by `,`), and `close`. `spaced` puts a space inside the
    /// delimiters on one line, as in `{ cents: 1 }`; an empty composite is `open close`
    /// with nothing between, as in `[]` or `Map {}`.
    fn composite(&mut self, head: &str, open: &str, close: &str, spaced: bool, parts: Vec<Part>) {
        self.output.push_str(head);
        self.output.push_str(open);
        if parts.is_empty() {
            self.output.push_str(close);
            return;
        }
        self.depth += 1;
        for (index, part) in parts.iter().enumerate() {
            if self.indented {
                self.output.push('\n');
                self.output.push_str(&"  ".repeat(self.depth));
            } else if index > 0 {
                self.output.push_str(", ");
            } else if spaced {
                self.output.push(' ');
            }
            if let Some(label) = &part.label {
                self.output.push_str(label);
            }
            self.value(part.value);
            if self.indented {
                self.output.push(',');
            }
        }
        self.depth -= 1;
        if self.indented {
            self.output.push('\n');
            self.output.push_str(&"  ".repeat(self.depth));
        } else if spaced {
            self.output.push(' ');
        }
        self.output.push_str(close);
    }
}

/// `text` as a Chimi text literal: quoted, with `"`, `\`, control characters, and the
/// `${` that would start an interpolation escaped.
fn quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            '$' if characters.peek() == Some(&'{') => quoted.push_str("\\$"),
            control if control.is_ascii_control() => {
                quoted.push_str(&format!("\\u{{{:x}}}", u32::from(control)));
            }
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

/// The canonical text of a JSON number, so equal numbers such as `1.0` and `1` render
/// alike: an integral value within 2^53 as an integer, any other as the shortest
/// `Float` text.
fn normalize_json_number(text: &str) -> String {
    if let Ok(integer) = text.parse::<i64>() {
        return integer.to_string();
    }
    match text.parse::<f64>() {
        Ok(value)
            if value.is_finite()
                && value.fract() == 0.0
                && value.abs() < 9_007_199_254_740_992.0 =>
        {
            format!("{}", value as i64)
        }
        Ok(value) if value.is_finite() => render_float(if value == 0.0 { 0.0 } else { value }),
        _ => text.to_owned(),
    }
}
