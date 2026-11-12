//! `chimi-process-lab <kind> [--through <stage>] [--scenario <ID>] [--print-actual]`

use chimi::process_lab::catalogue::{self, Stage};
use chimi::process_lab::{Kind, Options, run};
use std::process::ExitCode;

const USAGE: &str = "usage: chimi-process-lab <static|core|runtime|heap|observability|forcing|performance|all> [--through <stage>] [--scenario <ID>] [--print-actual]";

fn main() -> ExitCode {
    match lab() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("chimi-process-lab: {error}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// Prints one report line per kind; `Ok(false)` when an unlocked scenario
/// failed.
fn lab() -> Result<bool, String> {
    let (kinds, options) = parse(std::env::args().skip(1))?;
    let mut passed = true;
    for kind in kinds {
        let report = run(kind, &options)?;
        passed &= !report.failed();
        print!("{}", report.render_json());
    }
    Ok(passed)
}

fn parse(arguments: impl Iterator<Item = String>) -> Result<(Vec<Kind>, Options), String> {
    let mut kinds = None;
    let mut options = Options::default();
    let mut arguments = arguments;
    while let Some(argument) = arguments.next() {
        let mut value = |flag: &str| {
            arguments
                .next()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match argument.as_str() {
            "--through" => {
                let stage = value("--through")?;
                options.through =
                    Stage::parse(&stage).ok_or_else(|| format!("unknown stage `{stage}`"))?;
            }
            "--scenario" => {
                let id = value("--scenario")?;
                options.scenario =
                    Some(catalogue::find(&id).ok_or_else(|| format!("unknown scenario `{id}`"))?);
            }
            "--print-actual" => options.print_actual = true,
            "all" if kinds.is_none() => kinds = Some(Kind::ALL.to_vec()),
            kind if kinds.is_none() && !kind.starts_with('-') => {
                kinds = Some(vec![
                    Kind::parse(kind).ok_or_else(|| format!("unknown evidence kind `{kind}`"))?,
                ]);
            }
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    // `--scenario` runs that one scenario whatever its kind, so the kind may
    // be left out.
    let kinds = match (kinds, options.scenario) {
        (_, Some(scenario)) => vec![scenario.kind()],
        (Some(kinds), None) => kinds,
        (None, None) => return Err("expected an evidence kind".into()),
    };
    Ok((kinds, options))
}
