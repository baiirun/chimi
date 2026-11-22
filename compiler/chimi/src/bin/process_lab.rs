//! `chimi-process-lab <kind>... [--through <stage>] [--scenario <ID>] [--backend <native|interpreter>] [--print-actual] [--baseline <report.json>] [--inject-pg05-regression <factor>] [--carriers <n>]`

use chimi::process_lab::catalogue::{self, Stage};
use chimi::process_lab::{Backend, Kind, Options, baseline_warnings, run};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: chimi-process-lab <static|core|runtime|heap|observability|forcing|performance|all>... [--through <stage>] [--scenario <ID>] [--backend <native|interpreter>] [--print-actual] [--baseline <report.json>] [--inject-pg05-regression <factor>] [--carriers <n>]";

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
/// failed. With `--baseline`, a gate more than 1.5 times its saved value
/// prints a warning to stderr, which does not fail the run.
fn lab() -> Result<bool, String> {
    let (kinds, options, baseline) = parse(std::env::args().skip(1))?;
    let baseline = baseline
        .map(|path| {
            std::fs::read_to_string(&path)
                .map_err(|error| format!("read baseline {}: {error}", path.display()))
        })
        .transpose()?;
    let mut passed = true;
    for kind in kinds {
        let report = run(kind, &options)?;
        passed &= !report.failed();
        print!("{}", report.render_json());
        if let Some(baseline) = &baseline
            && !report.gates.is_empty()
        {
            for warning in baseline_warnings(&report.gates, baseline)? {
                eprintln!("{warning}");
            }
        }
    }
    Ok(passed)
}

type Parsed = (Vec<Kind>, Options, Option<PathBuf>);

/// Kinds may be listed several at once (`static core runtime`), each run in
/// the order given; `all` is every kind in order.
fn parse(arguments: impl Iterator<Item = String>) -> Result<Parsed, String> {
    let mut kinds: Option<Vec<Kind>> = None;
    let mut baseline = None;
    let mut options = Options::default();
    let mut carriers_given = false;
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
            "--backend" => {
                let backend = value("--backend")?;
                options.backend = Backend::parse(&backend)
                    .ok_or_else(|| format!("unknown backend `{backend}`"))?;
            }
            "--print-actual" => options.print_actual = true,
            "--inject-pg05-regression" => {
                let factor = value("--inject-pg05-regression")?;
                options.injected_pg05_regression = Some(
                    factor
                        .parse::<f64>()
                        .map_err(|_| format!("`{factor}` is not a regression factor"))?,
                );
            }
            "--carriers" => {
                carriers_given = true;
                let count = value("--carriers")?;
                // Digits only, as the runner takes them: `parse` alone
                // would accept a leading `+`.
                options.carriers = Some(&count)
                    .filter(|count| count.bytes().all(|byte| byte.is_ascii_digit()))
                    .and_then(|count| count.parse::<u32>().ok())
                    .filter(|count| (1..=256).contains(count))
                    .ok_or_else(|| {
                        format!("`{count}` is not a carrier count from 1 through 256")
                    })?;
            }
            "--baseline" => baseline = Some(PathBuf::from(value("--baseline")?)),
            "all" if kinds.is_none() => kinds = Some(Kind::ALL.to_vec()),
            kind if !kind.starts_with('-') => {
                let kind =
                    Kind::parse(kind).ok_or_else(|| format!("unknown evidence kind `{kind}`"))?;
                match &mut kinds {
                    Some(listed) if !listed.contains(&kind) && listed.len() < Kind::ALL.len() => {
                        listed.push(kind)
                    }
                    Some(_) => return Err(format!("evidence kind `{}` repeated", kind.name())),
                    None => kinds = Some(vec![kind]),
                }
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
    // Only gates are compared with a baseline, and only PG05 takes an
    // injected regression: either flag on a run without them would be
    // ignored, so it is an error instead.
    let measures_gates = kinds.contains(&Kind::Performance);
    if baseline.is_some() && !measures_gates {
        return Err("--baseline compares performance gates; add the `performance` kind".into());
    }
    if options.injected_pg05_regression.is_some()
        && (!measures_gates
            || options
                .scenario
                .is_some_and(|scenario| scenario.id != "PG05"))
    {
        return Err(
            "--inject-pg05-regression applies to PG05; run `performance` or `--scenario PG05`"
                .into(),
        );
    }
    if carriers_given && !measures_gates {
        return Err("--carriers runs performance gates; add the `performance` kind".into());
    }
    // The saved baselines were measured on one carrier.
    if baseline.is_some() && options.carriers > 1 {
        return Err("--baseline compares one-carrier gates; leave out --carriers".into());
    }
    Ok((kinds, options, baseline))
}
