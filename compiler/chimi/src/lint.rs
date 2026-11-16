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
use crate::ids::SymbolId;
use crate::operations::RUNNER_PROVIDERS;
use crate::resolution::{Namespace, ProjectSymbolKind, ReferenceResolution, ResolvedProject};
use crate::semantic::{ImportBinding, ModuleDeclarationKind, SemanticModule, is_generated_name};
use std::collections::HashSet;

/// An import whose alias or member no code in the module names.
pub(crate) const UNUSED_IMPORT: &str = "unused-import";
/// A private function or constant nothing references.
pub(crate) const UNUSED_PRIVATE: &str = "unused-private";
/// A `let` or pattern binding its scope never reads.
pub(crate) const UNUSED_BINDING: &str = "unused-binding";
/// A public item with no `///` doc comment.
pub(crate) const MISSING_DOC: &str = "missing-doc";

/// The findings for every module `include` accepts, in module order, then
/// source order.
pub(crate) fn lint(resolved: &ResolvedProject, include: impl Fn(&str) -> bool) -> Vec<Diagnostic> {
    let used = used_symbols(resolved);
    let mut findings = Vec::new();
    for module in &resolved.semantic.modules {
        if !include(&module.path) {
            continue;
        }
        let mut module_findings = unused_imports(resolved, module);
        module_findings.extend(unused_private_items(resolved, module, &used));
        module_findings.extend(unused_bindings(resolved, module, &used));
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
        .filter_map(|reference| match reference.resolution {
            ReferenceResolution::Resolved(id) | ReferenceResolution::Inaccessible(id) => Some(id),
            _ => None,
        })
        .collect()
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
                    findings.push(finding(
                        module,
                        UNUSED_IMPORT,
                        alias.range,
                        format!("the module alias {} is never used", alias.text),
                    ));
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
                        findings.push(finding(
                            module,
                            UNUSED_IMPORT,
                            member.range,
                            format!("the imported name {} is never used", member.text),
                        ));
                    }
                }
            }
            ImportBinding::Missing => {}
        }
    }
    findings
}

fn unused_private_items(
    resolved: &ResolvedProject,
    module: &SemanticModule,
    used: &HashSet<SymbolId>,
) -> Vec<Diagnostic> {
    module
        .declarations
        .iter()
        .filter(|declaration| {
            !declaration.public
                && !declaration.test
                && matches!(
                    declaration.kind,
                    ModuleDeclarationKind::Function | ModuleDeclarationKind::Constant
                )
                && !is_intentionally_unused(&declaration.name.text)
                && !is_runner_provider(&module.path, &declaration.name.text)
        })
        .filter(|declaration| {
            !resolved.symbols.iter().any(|symbol| {
                symbol.module_id == Some(module.id)
                    && symbol.semantic_id == Some(declaration.semantic_id)
                    && used.contains(&symbol.id)
            })
        })
        .map(|declaration| {
            let what = match declaration.kind {
                ModuleDeclarationKind::Function => "function",
                _ => "constant",
            };
            finding(
                module,
                UNUSED_PRIVATE,
                declaration.name.range,
                format!(
                    "the private {what} {} is never used; remove it or make it `pub`",
                    declaration.name.text
                ),
            )
        })
        .collect()
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
            Some(finding(
                module,
                UNUSED_BINDING,
                symbol.name_range?,
                format!(
                    "{} is never used; remove it, or name it _{} to keep it",
                    symbol.name, symbol.name
                ),
            ))
        })
        .collect()
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
