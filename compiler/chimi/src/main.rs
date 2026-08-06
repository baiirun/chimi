use chimi::Compiler;
use chimi::execution::ExecutionConfig;
use chimi::project::LoadedProject;
use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

const VERSION: &str = concat!("chimi ", env!("CARGO_PKG_VERSION"), "-bootstrap");
const USAGE: &str = concat!(
    "usage: chimi --version | chimi check <path> | chimi run <path> | ",
    "chimi check --root <project-dir> <entry-module> | ",
    "chimi run --root <project-dir> <entry-module>"
);

#[derive(Debug, Eq, PartialEq)]
enum Input {
    File { path: PathBuf },
    Project { root: PathBuf, entry: String },
}

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Version,
    Check { input: Input },
    Run { input: Input },
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
        let Some(first) = arguments.next() else {
            return Err(UsageError {
                message: "check and run require a source path or project root",
            });
        };
        let input = if first == "--root" {
            let Some(root) = arguments.next() else {
                return Err(UsageError {
                    message: "--root requires a project directory and entry module",
                });
            };
            let Some(entry) = arguments.next() else {
                return Err(UsageError {
                    message: "--root requires a project directory and entry module",
                });
            };
            let Ok(entry) = entry.into_string() else {
                return Err(UsageError {
                    message: "entry module must be valid UTF-8",
                });
            };
            Input::Project {
                root: PathBuf::from(root),
                entry,
            }
        } else {
            Input::File {
                path: PathBuf::from(first),
            }
        };
        return if arguments.next().is_some() {
            Err(UsageError {
                message: "check and run received unexpected arguments",
            })
        } else if command == "check" {
            Ok(Command::Check { input })
        } else {
            Ok(Command::Run { input })
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
        Command::Check { input } => check(input),
        Command::Run { input } => run(input),
    }
}

fn check(input: Input) -> ExitCode {
    let Some(project) = load_project(input) else {
        return ExitCode::FAILURE;
    };
    match Compiler::verify_project(project.sources()) {
        Ok(_) => ExitCode::SUCCESS,
        Err(diagnostics) => {
            eprint!("{}", project.render_diagnostics(&diagnostics));
            ExitCode::FAILURE
        }
    }
}

fn run(input: Input) -> ExitCode {
    let Some(project) = load_project(input) else {
        return ExitCode::FAILURE;
    };
    match Compiler::run_project(project.sources(), ExecutionConfig::for_process()) {
        Ok(report) => {
            print!("{}", report.render_json_lines());
            ExitCode::SUCCESS
        }
        Err(diagnostics) => {
            eprint!("{}", project.render_diagnostics(&diagnostics));
            ExitCode::FAILURE
        }
    }
}

fn load_project(input: Input) -> Option<LoadedProject> {
    let loaded = match input {
        Input::File { path } => LoadedProject::single_file(path),
        Input::Project { root, entry } => LoadedProject::load(root, entry),
    };
    match loaded {
        Ok(project) => Some(project),
        Err(error) => {
            eprintln!("chimi: {error}");
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
    use super::{Command, Input, UsageError, parse_command};
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
                input: Input::File {
                    path: PathBuf::from("program.chimi")
                }
            })
        );
        assert_eq!(
            parse_command(arguments(&["check"])),
            Err(UsageError {
                message: "check and run require a source path or project root"
            })
        );
        assert_eq!(
            parse_command(arguments(&["check", "one.chimi", "two.chimi"])),
            Err(UsageError {
                message: "check and run received unexpected arguments"
            })
        );
        assert_eq!(
            parse_command(arguments(&["run", "program.chimi"])),
            Ok(Command::Run {
                input: Input::File {
                    path: PathBuf::from("program.chimi")
                }
            })
        );
    }

    #[test]
    fn parses_an_explicit_project_root_and_entry_module() {
        assert_eq!(
            parse_command(arguments(&["run", "--root", "project", "app/main"])),
            Ok(Command::Run {
                input: Input::Project {
                    root: PathBuf::from("project"),
                    entry: "app/main".to_owned()
                }
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
