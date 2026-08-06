use chimi::diagnostic::Diagnostic;
use chimi::execution::ExecutionConfig;
use chimi::source::SourceSnapshot;
use chimi::{Compiler, ProjectSource};
use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const VERSION: &str = concat!("chimi ", env!("CARGO_PKG_VERSION"), "-bootstrap");
const USAGE: &str = "usage: chimi --version | chimi check <path> | chimi run <path>";

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Version,
    Check { path: PathBuf },
    Run { path: PathBuf },
}

#[derive(Debug, Eq, PartialEq)]
struct UsageError {
    message: &'static str,
}

fn parse_command(arguments: impl IntoIterator<Item = OsString>) -> Result<Command, UsageError> {
    let mut arguments = arguments.into_iter();
    let Some(command) = arguments.next() else {
        return Err(UsageError {
            message: "missing command",
        });
    };

    if command == "--version" {
        return if arguments.next().is_none() {
            Ok(Command::Version)
        } else {
            Err(UsageError {
                message: "--version does not accept arguments",
            })
        };
    }

    if command == "check" || command == "run" {
        let Some(path) = arguments.next() else {
            return Err(UsageError {
                message: "check and run require a source path",
            });
        };
        let path = PathBuf::from(path);
        return if arguments.next().is_none() {
            if command == "check" {
                Ok(Command::Check { path })
            } else {
                Ok(Command::Run { path })
            }
        } else {
            Err(UsageError {
                message: "check and run accept exactly one source path",
            })
        };
    }

    Err(UsageError {
        message: "unknown command",
    })
}

fn execute(command: Command) -> ExitCode {
    match command {
        Command::Version => {
            println!("{VERSION}");
            ExitCode::SUCCESS
        }
        Command::Check { path } => check(path),
        Command::Run { path } => run(path),
    }
}

fn check(path: PathBuf) -> ExitCode {
    let Some(source) = read_source(&path) else {
        return ExitCode::FAILURE;
    };
    let diagnostic_source = source.clone();
    match Compiler::verify_project(vec![single_module(source)]) {
        Ok(_) => ExitCode::SUCCESS,
        Err(diagnostics) => {
            eprint!("{}", render_diagnostics(&diagnostic_source, &diagnostics));
            ExitCode::FAILURE
        }
    }
}

fn run(path: PathBuf) -> ExitCode {
    let Some(source) = read_source(&path) else {
        return ExitCode::FAILURE;
    };
    let diagnostic_source = source.clone();
    match Compiler::run_project(vec![single_module(source)], ExecutionConfig::for_process()) {
        Ok(report) => {
            print!("{}", report.render_json_lines());
            ExitCode::SUCCESS
        }
        Err(diagnostics) => {
            eprint!("{}", render_diagnostics(&diagnostic_source, &diagnostics));
            ExitCode::FAILURE
        }
    }
}

fn single_module(source: SourceSnapshot) -> ProjectSource {
    ProjectSource::new("main", source).expect("the CLI single-module path is valid")
}

fn render_diagnostics(source: &SourceSnapshot, diagnostics: &[Diagnostic]) -> String {
    let mut output = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.render(source))
        .collect::<Vec<_>>()
        .join("\n");
    if !output.is_empty() {
        output.push('\n');
    }
    output
}

fn read_source(path: &Path) -> Option<SourceSnapshot> {
    match SourceSnapshot::read(path) {
        Ok(source) => Some(source),
        Err(error) => {
            eprintln!("chimi: cannot read {}: {error}", path.display());
            None
        }
    }
}

fn main() -> ExitCode {
    match parse_command(env::args_os().skip(1)) {
        Ok(command) => execute(command),
        Err(error) => {
            eprintln!("chimi: {}\n{USAGE}", error.message);
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, UsageError, parse_command};
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn arguments(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_version_without_additional_arguments() {
        assert_eq!(
            parse_command(arguments(&["--version"])),
            Ok(Command::Version)
        );
        assert_eq!(
            parse_command(arguments(&["--version", "unexpected"])),
            Err(UsageError {
                message: "--version does not accept arguments"
            })
        );
    }

    #[test]
    fn parses_one_check_path() {
        assert_eq!(
            parse_command(arguments(&["check", "program.chimi"])),
            Ok(Command::Check {
                path: PathBuf::from("program.chimi")
            })
        );
        assert_eq!(
            parse_command(arguments(&["check"])),
            Err(UsageError {
                message: "check and run require a source path"
            })
        );
        assert_eq!(
            parse_command(arguments(&["check", "one.chimi", "two.chimi"])),
            Err(UsageError {
                message: "check and run accept exactly one source path"
            })
        );
        assert_eq!(
            parse_command(arguments(&["run", "program.chimi"])),
            Ok(Command::Run {
                path: PathBuf::from("program.chimi")
            })
        );
    }

    #[test]
    fn rejects_missing_and_unknown_commands() {
        assert_eq!(
            parse_command(arguments(&[])),
            Err(UsageError {
                message: "missing command"
            })
        );
        assert_eq!(
            parse_command(arguments(&["unknown"])),
            Err(UsageError {
                message: "unknown command"
            })
        );
    }
}
