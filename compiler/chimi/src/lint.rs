//! `chimi lint`: convention and maintainability findings, separate from the
//! compiler's correctness checks (design-philosophy.md: "The compiler should
//! enforce central correctness guarantees. The linter should guide style,
//! conventions, and higher-level maintainability").
//!
//! Every rule reads the name resolution of a project that type-checks. A
//! finding is a warning with stage `lint` and the rule's name as its code.
//! A name that starts with `_` is unused on purpose, so no rule reports it.

use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::doc::{ItemNodes, kind_name};
use crate::ids::{SymbolId, TextRange};
use crate::operations::RUNNER_PROVIDERS;
use crate::resolution::{
    Namespace, ProjectReference, ProjectSymbolKind, ReferenceResolution, ResolvedProject,
};
use crate::semantic::{
    ImportBinding, ModuleDeclaration, ModuleDeclarationKind, RequirementTermUse, SemanticModule,
    is_generated_name,
};
use crate::typing::TypedProject;
use std::collections::{HashMap, HashSet};

/// An import whose alias or member no code in the module names.
pub(crate) const UNUSED_IMPORT: &str = "unused-import";
/// A private item nothing outside dead code references.
pub(crate) const UNUSED_PRIVATE: &str = "unused-private";
/// A `let` or pattern binding its scope never reads.
pub(crate) const UNUSED_BINDING: &str = "unused-binding";
/// A public item with no `///` doc comment.
pub(crate) const MISSING_DOC: &str = "missing-doc";
/// A written needs entry of a private function, or of `main`, that its body
/// never reaches.
pub(crate) const UNUSED_NEED: &str = "unused-need";

/// The findings for every module `include` accepts, in module order, then
/// source order.
pub(crate) fn lint(
    typed: &TypedProject,
    include: impl Fn(crate::compiler::ModuleOrigin) -> bool,
) -> Vec<Diagnostic> {
    let resolved = &typed.resolved;
    let used = used_symbols(resolved);
    let mut findings = Vec::new();
    for module in &resolved.semantic.modules {
        if !include(module.origin) {
            continue;
        }
        let mut module_findings = unused_imports(resolved, module);
        module_findings.extend(unused_private_items(resolved, module));
        module_findings.extend(unused_bindings(resolved, module, &used));
        module_findings.extend(unused_needs(typed, module));
        module_findings.extend(missing_docs(module));
        module_findings.sort_by_key(|diagnostic| diagnostic.range().start());
        findings.extend(module_findings);
    }
    findings
}

/// Every symbol some reference resolves to.
fn used_symbols(resolved: &ResolvedProject) -> HashSet<SymbolId> {
    resolved
        .references
        .iter()
        .filter_map(referenced_symbol)
        .collect()
}

fn referenced_symbol(reference: &ProjectReference) -> Option<SymbolId> {
    match reference.resolution {
        ReferenceResolution::Resolved(id) | ReferenceResolution::Inaccessible(id) => Some(id),
        _ => None,
    }
}

fn is_intentionally_unused(name: &str) -> bool {
    name.starts_with('_') || is_generated_name(name)
}

fn finding(
    module: &SemanticModule,
    code: &'static str,
    range: crate::ids::TextRange,
    message: String,
) -> Diagnostic {
    Diagnostic::warning(
        DiagnosticStage::Lint,
        code,
        module.source.file_id(),
        range,
        message,
    )
}

fn unused_imports(resolved: &ResolvedProject, module: &SemanticModule) -> Vec<Diagnostic> {
    let file_id = module.source.file_id();
    let references: Vec<_> = resolved
        .references
        .iter()
        .filter(|reference| reference.file_id == file_id)
        .collect();
    let aliases: Vec<&str> = module
        .imports
        .iter()
        .filter_map(|import| match &import.binding {
            ImportBinding::Alias(alias) => Some(alias.text.as_str()),
            _ => None,
        })
        .collect();
    let mut findings = Vec::new();
    for import in &module.imports {
        let line = ImportLine::of(module, import.range);
        match &import.binding {
            ImportBinding::Alias(alias) => {
                // A qualified type, `Alias.Type`, is one type reference with
                // the path as its text, not a module reference.
                let qualified = format!("{}.", alias.text);
                let used = references.iter().any(|reference| {
                    (reference.namespace == Namespace::Module && reference.text == alias.text)
                        || reference.text.starts_with(&qualified)
                });
                if !used && !is_intentionally_unused(&alias.text) {
                    let unused = finding(
                        module,
                        UNUSED_IMPORT,
                        alias.range,
                        format!("the module alias {} is never used", alias.text),
                    );
                    findings.push(line.with_fix(module, unused, |line| line.alias = None));
                }
            }
            ImportBinding::Members(members) => {
                for member in members {
                    // `Error.Variant` names the imported enum `Error`, unless a
                    // module alias of the same name takes the qualified path,
                    // as it does for types.
                    let qualified = format!("{}.", member.text);
                    let shadowed = aliases.contains(&member.text.as_str());
                    let used = references.iter().any(|reference| {
                        reference.namespace != Namespace::Module
                            && (reference.text == member.text
                                || (!shadowed && reference.text.starts_with(&qualified)))
                    });
                    if !used && !is_intentionally_unused(&member.text) {
                        let unused = finding(
                            module,
                            UNUSED_IMPORT,
                            member.range,
                            format!("the imported name {} is never used", member.text),
                        );
                        findings.push(line.with_fix(module, unused, |line| {
                            line.members.retain(|name| name != &member.text);
                        }));
                    }
                }
            }
            ImportBinding::Missing => {}
        }
    }
    findings
}

/// One `import` line as its parts: a line with both an alias and members
/// lowers to two imports over the same range.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ImportLine {
    range: TextRange,
    path: String,
    alias: Option<String>,
    members: Vec<String>,
}

impl ImportLine {
    fn of(module: &SemanticModule, range: TextRange) -> Self {
        let mut line = Self {
            range,
            path: String::new(),
            alias: None,
            members: Vec::new(),
        };
        for import in module.imports.iter().filter(|import| import.range == range) {
            line.path.clone_from(&import.path);
            match &import.binding {
                ImportBinding::Alias(alias) => line.alias = Some(alias.text.clone()),
                ImportBinding::Members(members) => {
                    line.members = members.iter().map(|member| member.text.clone()).collect();
                }
                ImportBinding::Missing => {}
            }
        }
        line
    }

    /// The canonical text of the line, or `None` when nothing is left to
    /// import.
    fn render(&self) -> Option<String> {
        if self.alias.is_none() && self.members.is_empty() {
            return None;
        }
        let mut text = format!("import {}", self.path);
        if let Some(alias) = &self.alias {
            text.push_str(" as ");
            text.push_str(alias);
        }
        if !self.members.is_empty() {
            text.push_str(".{");
            text.push_str(&self.members.join(", "));
            text.push('}');
        }
        Some(text)
    }

    /// `finding` with an edit that rewrites the line without the part
    /// `remove` takes out, or deletes the line when nothing is left. A line
    /// written other than in the formatter's one-line form (wrapped, or with
    /// a comment) gets no edit, since rewriting it would drop what it holds.
    fn with_fix(
        &self,
        module: &SemanticModule,
        finding: Diagnostic,
        remove: impl FnOnce(&mut Self),
    ) -> Diagnostic {
        let range = self.range;
        let text = module.source.text();
        let written = &text[range.start() as usize..range.end() as usize];
        let rest = &text[range.end() as usize..];
        let line_end = rest.find('\n').unwrap_or(rest.len());
        // Anything after the import on its line, a comment included, would be
        // left behind or lost, so such a line gets no edit either.
        if self.render().as_deref() != Some(written) || !rest[..line_end].trim().is_empty() {
            return finding;
        }
        let mut kept = self.clone();
        remove(&mut kept);
        match kept.render() {
            Some(replacement) => finding.with_fix(range, replacement),
            None => {
                let end = range.end() as usize + line_end + usize::from(line_end < rest.len());
                finding.with_fix(TextRange::new(range.start() as usize, end), "")
            }
        }
    }
}

/// A private item is used when a root reaches it: a reference from a public
/// item, a test, an item kept on purpose (a `_` name, a runner provider), or
/// the module's top level, then whatever a used private item references in
/// turn. So a private function only it calls, or one only other dead code
/// calls, is reported too.
fn unused_private_items(resolved: &ResolvedProject, module: &SemanticModule) -> Vec<Diagnostic> {
    let private: Vec<&ModuleDeclaration> = module
        .declarations
        .iter()
        .filter(|declaration| !declaration.public)
        .collect();
    let owner = private_owners(resolved, module, &private);
    let nodes = ItemNodes::new(module);
    let ranges: Vec<Option<TextRange>> = private
        .iter()
        .map(|declaration| nodes.item_range(declaration))
        .collect();
    let enclosing = |offset: u32| {
        ranges.iter().position(|range| {
            range.is_some_and(|range| range.start() <= offset && offset < range.end())
        })
    };

    let mut reached = vec![false; private.len()];
    // Referenced by something other than itself, reached or not.
    let mut referenced = vec![false; private.len()];
    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); private.len()];
    let mut pending: Vec<usize> = (0..private.len())
        .filter(|&index| !is_reportable(module, private[index]))
        .collect();
    let file_id = module.source.file_id();
    for reference in resolved
        .references
        .iter()
        .filter(|reference| reference.file_id == file_id)
    {
        let Some(&target) = referenced_symbol(reference).and_then(|id| owner.get(&id)) else {
            continue;
        };
        match enclosing(reference.range.start()) {
            Some(source) if source == target => continue,
            Some(source) => edges[source].push(target),
            None => pending.push(target),
        }
        referenced[target] = true;
    }
    while let Some(index) = pending.pop() {
        if !std::mem::replace(&mut reached[index], true) {
            pending.extend(edges[index].iter().copied());
        }
    }

    private
        .iter()
        .enumerate()
        .filter(|&(index, declaration)| !reached[index] && is_reportable(module, declaration))
        .map(|(index, declaration)| {
            let what = private_kind_name(declaration.kind);
            let message = if referenced[index] {
                format!(
                    "the private {what} {} is used only by code that is never used; remove it or make it `pub`",
                    declaration.name.text
                )
            } else {
                format!(
                    "the private {what} {} is never used; remove it or make it `pub`",
                    declaration.name.text
                )
            };
            finding(module, UNUSED_PRIVATE, declaration.name.range, message)
        })
        .collect()
}

/// Each symbol a private declaration owns, mapped to its index in `private`:
/// the item itself, a struct's constructor, and an enum's variants.
fn private_owners(
    resolved: &ResolvedProject,
    module: &SemanticModule,
    private: &[&ModuleDeclaration],
) -> HashMap<SymbolId, usize> {
    let mut owner = HashMap::new();
    for (index, declaration) in private.iter().enumerate() {
        let origins = std::iter::once(declaration.semantic_id).chain(
            declaration
                .variants
                .iter()
                .map(|variant| variant.semantic_id),
        );
        for origin in origins {
            for &symbol in resolved.source_symbols(module.id, origin) {
                owner.insert(symbol, index);
            }
        }
    }
    owner
}

/// Whether `unused-private` may report this private declaration. A test, a
/// `_` name, a runner provider and a `native type` are kept on purpose, so
/// what they reference counts as used.
fn is_reportable(module: &SemanticModule, declaration: &ModuleDeclaration) -> bool {
    !declaration.test
        && declaration.kind != ModuleDeclarationKind::NativeType
        && !is_intentionally_unused(&declaration.name.text)
        && !is_runner_provider(&module.path, &declaration.name.text)
}

fn private_kind_name(kind: ModuleDeclarationKind) -> &'static str {
    match kind {
        ModuleDeclarationKind::Struct => "struct",
        ModuleDeclarationKind::Enum => "enum",
        ModuleDeclarationKind::Interface => "interface",
        ModuleDeclarationKind::Function => "function",
        ModuleDeclarationKind::Constant => "constant",
        ModuleDeclarationKind::NativeType => "native type",
        ModuleDeclarationKind::TypeAlias => "type alias",
        ModuleDeclarationKind::ProcessName => "process name",
        ModuleDeclarationKind::NeedsGroup => "needs group",
    }
}

/// A private `host` function the runner binds to `main`'s needs by name
/// (operations.rs `RUNNER_PROVIDERS`), so no source references it.
fn is_runner_provider(module_path: &str, name: &str) -> bool {
    RUNNER_PROVIDERS
        .iter()
        .any(|runner| runner.provider.split_once('.') == Some((module_path, name)))
}

fn unused_bindings(
    resolved: &ResolvedProject,
    module: &SemanticModule,
    used: &HashSet<SymbolId>,
) -> Vec<Diagnostic> {
    // The `_` name the fix would give a binding may already be taken, and two
    // locals of one name in one scope are an error. Scopes are not tracked
    // here, so a name bound anywhere in the module gets no fix.
    let bound: HashSet<&str> = resolved
        .symbols
        .iter()
        .filter(|symbol| symbol.module_id == Some(module.id) && is_local(symbol.kind))
        .map(|symbol| symbol.name.as_str())
        .collect();
    resolved
        .symbols
        .iter()
        .filter(|symbol| {
            symbol.module_id == Some(module.id)
                && matches!(
                    symbol.kind,
                    ProjectSymbolKind::LocalBinding | ProjectSymbolKind::PatternBinding
                )
                && !is_intentionally_unused(&symbol.name)
                && !used.contains(&symbol.id)
        })
        .filter_map(|symbol| {
            let range = symbol.name_range?;
            let unused = finding(
                module,
                UNUSED_BINDING,
                range,
                format!(
                    "{} is never used; remove it, or name it _{} to keep it",
                    symbol.name, symbol.name
                ),
            );
            Some(if bound.contains(format!("_{}", symbol.name).as_str()) {
                unused
            } else {
                unused.with_fix(TextRange::empty(range.start() as usize), "_")
            })
        })
        .collect()
}

fn is_local(kind: ProjectSymbolKind) -> bool {
    matches!(
        kind,
        ProjectSymbolKind::LocalBinding
            | ProjectSymbolKind::PatternBinding
            | ProjectSymbolKind::Parameter
    )
}

/// A private function, or `main`, that writes a needs entry its body never
/// reaches makes every caller supply it for nothing. A public function's
/// written entries are its published contract (executable-kernel.md: a clause
/// may list a need the body does not reach), so only private ones and `main`
/// are checked. An entry a group or a row parameter brings is not checked: the
/// group is shared, and the row binds whatever its caller has.
fn unused_needs(typed: &TypedProject, module: &SemanticModule) -> Vec<Diagnostic> {
    let resolved = &typed.resolved;
    let mut findings = Vec::new();
    for declaration in &module.declarations {
        let Some(signature) = &declaration.function else {
            continue;
        };
        let is_main = declaration.public && declaration.name.text == "main";
        if declaration.kind != ModuleDeclarationKind::Function
            || declaration.body.is_none()
            || (declaration.public && !is_main)
        {
            continue;
        }
        let Some(reached) = resolved
            .source_symbols(module.id, declaration.semantic_id)
            .iter()
            .find_map(|symbol| typed.reached_needs.get(symbol))
        else {
            continue;
        };
        let reached = &typed.needs_rows[reached.raw() as usize];
        for term in &signature.requirements.terms {
            let RequirementTermUse::Field { name, .. } = term else {
                continue;
            };
            let is_reached = reached
                .entries
                .iter()
                .any(|need| need.lexical_name == name.text && !need.explained);
            if !is_reached && !is_intentionally_unused(&name.text) {
                findings.push(finding(
                    module,
                    UNUSED_NEED,
                    name.range,
                    format!(
                        "{} never reaches the need {}; remove it from the needs clause",
                        declaration.name.text, name.text
                    ),
                ));
            }
        }
    }
    findings
}

/// A public item a reader of `chimi doc` would see with no explanation.
/// `main` is exempt: the entry point is the program, not an API.
fn missing_docs(module: &SemanticModule) -> Vec<Diagnostic> {
    let nodes = ItemNodes::new(module);
    module
        .declarations
        .iter()
        .filter(|declaration| {
            declaration.public
                && declaration.name.text != "main"
                && !is_intentionally_unused(&declaration.name.text)
                && nodes.doc(declaration).is_none()
        })
        .map(|declaration| {
            finding(
                module,
                MISSING_DOC,
                declaration.name.range,
                format!(
                    "the public {} {} has no doc comment; add a `///` line saying what it is for",
                    kind_name(declaration.kind),
                    declaration.name.text
                ),
            )
        })
        .collect()
}
