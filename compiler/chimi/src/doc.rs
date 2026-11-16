//! `chimi doc`: a module's public surface, as the compiler understands it
//! (CONTEXT.md, "Compiler-Derived Documentation").
//!
//! Each public item shows its declaration and its doc comment. A function shows
//! the contract the compiler derived for it, with the parameter names its source
//! writes, so failures and needs the source leaves to inference are visible. Other
//! items show their declaration as written, since their fields, variants and
//! members are their contract. Items keep source order; private items and tests
//! are left out.

use crate::doc_examples::{doc_comment_before, module_comment};
use crate::json::write_json_string_escaping_controls;
use crate::semantic::{ModuleDeclaration, ModuleDeclarationKind, SemanticModule};
use crate::syntax::SyntaxNode;
use crate::typing::TypedProject;

/// The public surface of one module.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModuleDocumentation {
    module_path: String,
    /// The `///` run at the top of the file that documents no item.
    summary: Option<String>,
    items: Vec<ItemDocumentation>,
}

/// One public item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemDocumentation {
    kind: &'static str,
    name: String,
    /// The declaration without its body, such as `pub fn length(text: Text) -> Int`.
    signature: String,
    /// The doc comment's text, with each line's `///` removed.
    doc: Option<String>,
}

impl ModuleDocumentation {
    pub fn module_path(&self) -> &str {
        &self.module_path
    }

    pub fn summary(&self) -> Option<&str> {
        self.summary.as_deref()
    }

    pub fn items(&self) -> &[ItemDocumentation] {
        &self.items
    }

    /// The terminal form, like `go doc`: the module's summary, then each item's
    /// signature with its doc comment indented under it.
    pub fn render_text(&self) -> String {
        let mut output = format!("module {}\n", self.module_path);
        if let Some(summary) = &self.summary {
            output.push('\n');
            output.push_str(summary);
            output.push('\n');
        }
        for item in &self.items {
            output.push('\n');
            output.push_str(&item.signature);
            output.push('\n');
            if let Some(doc) = &item.doc {
                for line in doc.lines() {
                    if line.is_empty() {
                        output.push('\n');
                    } else {
                        output.push_str("    ");
                        output.push_str(line);
                        output.push('\n');
                    }
                }
            }
        }
        output
    }

    pub fn render_json(&self) -> String {
        let mut output = String::from("{\"module\":");
        write_json_string_escaping_controls(&mut output, &self.module_path);
        output.push_str(",\"summary\":");
        write_optional(&mut output, self.summary.as_deref());
        output.push_str(",\"items\":[");
        for (index, item) in self.items.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            output.push_str("{\"kind\":");
            write_json_string_escaping_controls(&mut output, item.kind);
            output.push_str(",\"name\":");
            write_json_string_escaping_controls(&mut output, &item.name);
            output.push_str(",\"signature\":");
            write_json_string_escaping_controls(&mut output, &item.signature);
            output.push_str(",\"doc\":");
            write_optional(&mut output, item.doc.as_deref());
            output.push('}');
        }
        output.push_str("]}\n");
        output
    }
}

impl ItemDocumentation {
    pub const fn kind(&self) -> &'static str {
        self.kind
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn signature(&self) -> &str {
        &self.signature
    }

    pub fn doc(&self) -> Option<&str> {
        self.doc.as_deref()
    }
}

fn write_optional(output: &mut String, value: Option<&str>) {
    match value {
        Some(value) => write_json_string_escaping_controls(output, value),
        None => output.push_str("null"),
    }
}

/// The documentation of `module_path`, or `None` when the project has no such
/// module.
pub(crate) fn document_module(
    typed: &TypedProject,
    module_path: &str,
) -> Option<ModuleDocumentation> {
    let module = typed
        .resolved
        .semantic
        .modules
        .iter()
        .find(|module| module.path == module_path)?;
    let text = module.source.text();
    let items = module
        .declarations
        .iter()
        .filter(|declaration| declaration.public && !declaration.test)
        .map(|declaration| ItemDocumentation {
            kind: kind_name(declaration.kind),
            name: declaration.name.text.clone(),
            signature: signature(typed, module, declaration),
            doc: doc_comment_before(text, item_line_start(text, declaration)),
        })
        .collect();
    Some(ModuleDocumentation {
        module_path: module.path.clone(),
        summary: module_comment(text),
        items,
    })
}

const fn kind_name(kind: ModuleDeclarationKind) -> &'static str {
    match kind {
        ModuleDeclarationKind::Struct => "struct",
        ModuleDeclarationKind::Enum => "enum",
        ModuleDeclarationKind::Interface => "interface",
        ModuleDeclarationKind::Function => "fn",
        ModuleDeclarationKind::Constant => "const",
        ModuleDeclarationKind::NativeType => "type",
        ModuleDeclarationKind::ProcessName => "process name",
    }
}

/// The byte offset where the line holding the declaration's name starts: an
/// item's keywords and name share its first line.
fn item_line_start(text: &str, declaration: &ModuleDeclaration) -> usize {
    let name = declaration.name.range.start() as usize;
    text[..name].rfind('\n').map_or(0, |newline| newline + 1)
}

fn signature(
    typed: &TypedProject,
    module: &SemanticModule,
    declaration: &ModuleDeclaration,
) -> String {
    let text = module.source.text();
    let start = item_line_start(text, declaration);
    let declared = declaration_node(module, declaration)
        .map_or(
            &text[start..declaration.name.range.end() as usize],
            |node| &text[start..(node.range().end() as usize).max(start)],
        )
        .trim_end();
    match declaration.kind {
        ModuleDeclarationKind::Function => typed
            .contract(&module.path, &declaration.name.text)
            .and_then(|contract| {
                let header = header_before_parameters(declared)?;
                let parameters = declaration.function.as_ref()?.parameters.iter();
                let inputs = parameters
                    .zip(&contract.inputs)
                    .map(|(parameter, input)| {
                        format!("{}: {}", parameter.name.text, typed.format_type(*input))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(format!(
                    "{header}({inputs}) -> {}",
                    typed.format_contract_result(contract)
                ))
            })
            .unwrap_or_else(|| body_removed(declared).to_owned()),
        ModuleDeclarationKind::Constant => match declared.split_once('\n') {
            // A long initializer is not the constant's contract; its type is.
            Some(_) => declared
                .split_once(" =")
                .map_or(declared, |(head, _)| head)
                .trim_end()
                .to_owned(),
            None => declared.to_owned(),
        },
        _ => declared.to_owned(),
    }
}

/// The module item whose range holds the declaration's name.
fn declaration_node<'tree>(
    module: &'tree SemanticModule,
    declaration: &ModuleDeclaration,
) -> Option<SyntaxNode<'tree>> {
    let name = declaration.name.range;
    module
        .syntax
        .root()
        .child_nodes()
        .into_iter()
        .find(|node| node.range().start() <= name.start() && name.end() <= node.range().end())
}

/// `pub fn name<T>` from a function declaration: everything before the
/// parameter list's `(`.
fn header_before_parameters(declared: &str) -> Option<&str> {
    let open = declared.find('(')?;
    Some(declared[..open].trim_end())
}

/// A function declaration as written, without its `{ ... }` body.
fn body_removed(declared: &str) -> &str {
    declared
        .find(" {")
        .map_or(declared, |brace| &declared[..brace])
        .trim_end()
}
