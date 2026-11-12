//! Static scenarios: compile a fixture and compare its diagnostics with
//! `expected.diagnostics`, and, where the fixture has one, the rendered standard
//! contracts with `expected.contracts`.
//!
//! A fixture compiles when `Compiler::verify_project` succeeds, as `chimi check`
//! decides. Only errors are compared; warnings do not stop a build. Each expected
//! line is `<code> <module>.chimi:<line>:<column>`, optionally followed by a space
//! and a substring the diagnostic's message must contain.

use super::Options;
use super::catalogue::Scenario;
use super::fixture;
use crate::Compiler;
use crate::project::LoadedProject;

const DIAGNOSTICS: &str = "expected.diagnostics";
const CONTRACTS: &str = "expected.contracts";

pub(super) fn check(scenario: &Scenario, options: &Options) -> Result<(), String> {
    let expected = fixture::expected_lines(scenario, DIAGNOSTICS)?;
    let project = LoadedProject::load(fixture::root(scenario), "app/main")
        .map_err(|error| format!("load fixture: {error}"))?;
    let diagnostics = match Compiler::verify_project(project.sources()) {
        Ok(_) => Vec::new(),
        Err(diagnostics) => diagnostics,
    };
    let mut actual = project
        .diagnostic_positions(&diagnostics)
        .into_iter()
        .zip(&diagnostics)
        .map(|((module, line, column), diagnostic)| {
            (
                (module, line, column, diagnostic.code()),
                diagnostic.message(),
            )
        })
        .collect::<Vec<_>>();
    actual.sort();
    let rendered = actual
        .iter()
        .map(|((module, line, column, code), _)| format!("{code} {module}.chimi:{line}:{column}"))
        .collect::<Vec<_>>();

    let mut positions = Vec::with_capacity(expected.len());
    let mut substrings = Vec::with_capacity(expected.len());
    for (index, line) in expected.iter().enumerate() {
        let mut columns = line.splitn(3, ' ');
        let (Some(code), Some(location)) = (columns.next(), columns.next()) else {
            return Err(format!(
                "{DIAGNOSTICS} line {}: `{line}` is not `<code> <location>`",
                index + 1
            ));
        };
        positions.push(format!("{code} {location}"));
        substrings.push(columns.next());
    }
    if options.print_actual {
        let messages = actual
            .iter()
            .zip(&rendered)
            .map(|((_, message), line)| format!("{line} {message}"))
            .collect::<Vec<_>>();
        fixture::print_actual(scenario, DIAGNOSTICS, &expected, &messages);
    }
    fixture::compare(DIAGNOSTICS, &positions, &rendered)?;
    for (index, substring) in substrings.into_iter().enumerate() {
        let message = actual[index].1;
        if let Some(substring) = substring
            && !message.contains(substring)
        {
            return Err(format!(
                "{DIAGNOSTICS} line {}: message `{message}` does not contain `{substring}`",
                index + 1
            ));
        }
    }
    check_contracts(scenario, &project, options)
}

/// Compares `expected.contracts`, when present: each line is
/// `<Module>.<function>: <contract>` as the compiler renders it. A name whose module
/// part is a project module path (it contains `/`, as in `app/main.serving`) is that
/// project function's inferred contract; any other name is a standard function.
fn check_contracts(
    scenario: &Scenario,
    project: &LoadedProject,
    options: &Options,
) -> Result<(), String> {
    if !fixture::root(scenario).join(CONTRACTS).exists() {
        return Ok(());
    }
    let expected = fixture::expected_lines(scenario, CONTRACTS)?;
    let report = Compiler::type_project(project.sources());
    let actual = expected
        .iter()
        .map(|line| {
            let name = line
                .split_once(": ")
                .map_or(line.as_str(), |(name, _)| name);
            let contract = match name.rsplit_once('.') {
                Some((module, function)) if module.contains('/') => {
                    report.contract(module, function)
                }
                _ => report.standard_contract(name),
            }
            .unwrap_or_else(|| "<no such function>".to_owned());
            format!("{name}: {contract}")
        })
        .collect::<Vec<_>>();
    if options.print_actual {
        fixture::print_actual(scenario, CONTRACTS, &expected, &actual);
    }
    fixture::compare(CONTRACTS, &expected, &actual)
}
