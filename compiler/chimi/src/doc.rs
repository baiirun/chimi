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
use crate::occurrences::symbol_at;
use crate::resolution::ProjectSymbolKind;
use crate::json::write_json_string_escaping_controls;
use crate::semantic::{
    ImportBinding, ModuleDeclaration, ModuleDeclarationKind, SemanticModule, is_generated_name,
};
use crate::syntax::{NodeKind, SyntaxNode, TokenKind, is_comment_or_text_at};
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
    let nodes = ItemNodes::new(module);
    let items = module
        .declarations
        .iter()
        // Test items are private, so this leaves them out too.
        .filter(|declaration| declaration.public)
        .map(|declaration| ItemDocumentation {
            kind: kind_name(declaration.kind),
            name: declaration.name.text.clone(),
            signature: signature(typed, &nodes, declaration),
            doc: nodes.doc(declaration),
        })
        .collect();
    Some(ModuleDocumentation {
        module_path: module.path.clone(),
        summary: module_comment(text),
        items,
    })
}

/// The public surface of the module that `alias` names in `module_path`,
/// unless `offset` is inside a comment or a text literal there.
pub(crate) fn alias_members(
    typed: &TypedProject,
    module_path: &str,
    alias: &str,
    offset: usize,
) -> Option<ModuleDocumentation> {
    let module = typed
        .resolved
        .semantic
        .modules
        .iter()
        .find(|module| module.path == module_path)?;
    if is_comment_or_text_at(&module.source, offset) {
        return None;
    }
    let imported = module.imports.iter().find_map(|import| match &import.binding {
        ImportBinding::Alias(name) if name.text == alias => Some(import.path.as_str()),
        _ => None,
    })?;
    document_module(typed, imported)
}

/// The `///` doc comment of the item named at `offset` in `module_path`, at
/// a reference to it or at its declaration, in any module.
pub(crate) fn doc_comment_at(typed: &TypedProject, module_path: &str, offset: usize) -> Option<String> {
    let resolved = &typed.resolved;
    let module = resolved
        .semantic
        .modules
        .iter()
        .find(|module| module.path == module_path)?;
    let symbol = resolved.symbol(symbol_at(resolved, module, offset)?);
    // A type parameter or a local shares its function's semantic id, but has
    // no doc comment of its own.
    if !matches!(
        symbol.kind,
        ProjectSymbolKind::Struct
            | ProjectSymbolKind::StructConstructor
            | ProjectSymbolKind::Enum
            | ProjectSymbolKind::Interface
            | ProjectSymbolKind::NativeType
            | ProjectSymbolKind::TypeAlias
            | ProjectSymbolKind::Function
            | ProjectSymbolKind::Constant
            | ProjectSymbolKind::ProcessName
    ) {
        return None;
    }
    let declaring = &resolved.semantic.modules[symbol.module_id?.raw() as usize];
    let semantic_id = symbol.semantic_id?;
    let declaration = declaring
        .declarations
        .iter()
        .find(|declaration| declaration.semantic_id == semantic_id)?;
    ItemNodes::new(declaring).doc(declaration)
}


pub(crate) const fn kind_name(kind: ModuleDeclarationKind) -> &'static str {
    match kind {
        ModuleDeclarationKind::Struct => "struct",
        ModuleDeclarationKind::Enum => "enum",
        ModuleDeclarationKind::Interface => "interface",
        ModuleDeclarationKind::Function => "fn",
        ModuleDeclarationKind::Constant => "const",
        ModuleDeclarationKind::NativeType | ModuleDeclarationKind::TypeAlias => "type",
        ModuleDeclarationKind::ProcessName => "process name",
        ModuleDeclarationKind::NeedsGroup => "needs",
    }
}


/// The item's doc comment. Only an item that begins its line has one, so the
/// second of two items on one line does not take the first one's.
fn item_doc(text: &str, start: usize) -> Option<String> {
    let line_start = text[..start].rfind('\n').map_or(0, |newline| newline + 1);
    text[line_start..start]
        .trim()
        .is_empty()
        .then(|| doc_comment_before(text, line_start))
        .flatten()
}

fn signature(typed: &TypedProject, nodes: &ItemNodes<'_>, declaration: &ModuleDeclaration) -> String {
    let module = nodes.module;
    let text = module.source.text();
    let start = nodes.item_start(declaration);
    let declared = nodes
        .node(declaration)
        .map_or(
            &text[start..declaration.name.range.end() as usize],
            |node| &text[start..(node.range().end() as usize).max(start)],
        )
        .trim_end()
        .replace("\r\n", "\n");
    let declared = declared.as_str();
    match declaration.kind {
        ModuleDeclarationKind::Function => typed
            .contract(&module.path, &declaration.name.text)
            .and_then(|contract| {
                let header = header_before_parameters(declared)?;
                let parameters = declaration.function.as_ref()?.parameters.iter();
                let written = nodes
                    .node(declaration)
                    .map(|node| WrittenTypes::of(&node, text))
                    .unwrap_or_default();
                let inputs = parameters
                    .zip(&contract.inputs)
                    .enumerate()
                    .map(|(index, (parameter, input))| {
                        // A clause group's parameters are patterns, not names.
                        if is_generated_name(&parameter.name.text) {
                            typed.format_type(*input)
                        } else {
                            // A written type keeps the name it was written with,
                            // so a parameter of type `Settings` is not spelled out.
                            let shown = written
                                .parameters
                                .get(index)
                                .cloned()
                                .flatten()
                                .unwrap_or_else(|| typed.format_type(*input));
                            format!("{}: {shown}", parameter.name.text)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let result = match &written.success {
                    Some(success) => {
                        format!("{success}{}", typed.format_contract_rows(contract))
                    }
                    None => typed.format_contract_result(contract),
                };
                Some(format!("{header}({inputs}) -> {result}"))
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
/// A module's top-level syntax nodes in source order, gathered once so each
/// declaration's node is a binary search rather than a scan.
pub(crate) struct ItemNodes<'tree> {
    module: &'tree SemanticModule,
    nodes: Vec<SyntaxNode<'tree>>,
}

impl<'tree> ItemNodes<'tree> {
    pub(crate) fn new(module: &'tree SemanticModule) -> Self {
        Self {
            module,
            nodes: module.syntax.root().child_nodes(),
        }
    }

    /// The `///` doc comment of `declaration`, if it has one.
    pub(crate) fn doc(&self, declaration: &ModuleDeclaration) -> Option<String> {
        item_doc(self.module.source.text(), self.item_start(declaration))
    }

    /// The source range of the declaration's whole item, comments before it
    /// excluded.
    pub(crate) fn item_range(&self, declaration: &ModuleDeclaration) -> Option<crate::ids::TextRange> {
        self.node(declaration).map(|node| node.range())
    }

    fn node(&self, declaration: &ModuleDeclaration) -> Option<SyntaxNode<'tree>> {
        let name = declaration.name.range;
        let after = self
            .nodes
            .partition_point(|node| node.range().start() <= name.start());
        let node = self.nodes.get(after.checked_sub(1)?)?.clone();
        (name.end() <= node.range().end()).then_some(node)
    }

    /// The byte offset where the declaration's own text starts: its item node
    /// without the comments and blank space before it.
    fn item_start(&self, declaration: &ModuleDeclaration) -> usize {
        let text = self.module.source.text();
        let name = declaration.name.range.start() as usize;
        let Some(node) = self.node(declaration) else {
            return text[..name].rfind('\n').map_or(0, |newline| newline + 1);
        };
        let mut start = node.range().start() as usize;
        loop {
            let rest = &text[start..name];
            let trimmed = rest.trim_start();
            let skipped = rest.len() - trimmed.len();
            match trimmed.find('\n') {
                Some(newline) if trimmed.starts_with("//") => start += skipped + newline + 1,
                _ => return start + skipped,
            }
        }
    }
}

/// `pub fn name<T>` from a function declaration: everything before the
/// parameter list's `(`.
/// The types a function declaration writes, as written: one entry per
/// parameter, `None` where the parameter has no type, and the success type
/// after `->`. Whitespace runs collapse to one space.
#[derive(Default)]
struct WrittenTypes {
    parameters: Vec<Option<String>>,
    success: Option<String>,
}

impl WrittenTypes {
    fn of(function: &SyntaxNode<'_>, text: &str) -> Self {
        // A comment inside a written type cannot be kept on one line; such a
        // type is shown as the checker formats it.
        let written = |node: &SyntaxNode<'_>| {
            if node
                .descendant_tokens()
                .iter()
                .any(|token| token.kind() == TokenKind::Comment)
            {
                return None;
            }
            let range = node.range();
            text[range.start() as usize..range.end() as usize]
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .into()
        };
        let type_of = |node: &SyntaxNode<'_>| {
            node.child_nodes()
                .into_iter()
                .find(|child| child.kind() == NodeKind::Type)
                .and_then(|child| written(&child))
        };
        let children = function.child_nodes();
        let parameters = children
            .iter()
            .find(|child| child.kind() == NodeKind::ValueParameters)
            .map(|parameters| {
                parameters
                    .child_nodes()
                    .into_iter()
                    .filter(|parameter| parameter.kind() == NodeKind::ValueParameter)
                    .map(|parameter| type_of(&parameter))
                    .collect()
            })
            .unwrap_or_default();
        let success = children
            .iter()
            .find(|child| child.kind() == NodeKind::ImplementationContract)
            .filter(|contract| {
                contract
                    .child_tokens()
                    .iter()
                    .any(|token| token.kind() == TokenKind::Arrow)
            })
            .and_then(type_of);
        Self {
            parameters,
            success,
        }
    }
}

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
