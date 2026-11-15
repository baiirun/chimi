use chimi::Compiler;
use chimi::backend::native::build_application;
use chimi::execution::{ExecutionConfig, ExecutionOutcome};
use chimi::project::{LoadedProject, StandardTree};
use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

const VERSION: &str = concat!("chimi ", env!("CARGO_PKG_VERSION"), "-bootstrap");
const USAGE: &str = concat!(
    "usage: chimi --version | chimi format <path> | chimi inspect <path> <byte-offset> | ",
    "chimi inspect --root <project-dir> <entry-module> <module> <byte-offset> | ",
    "chimi check [--json] <path> | chimi run <path> [-- <argument>...] | ",
    "chimi build <path> [-o <executable>] | ",
    "chimi check [--json] --root <project-dir> <entry-module> | chimi run --root ",
    "<project-dir> <entry-module> [-- <argument>...] | chimi build --root <project-dir> <entry-module> ",
    "[-o <executable>]; a compiler developer may put --std-dir <directory> before ",
    "inspect, check, run, or build"
);

#[derive(Debug, Eq, PartialEq)]
enum Input {
    File { path: PathBuf },
    Project { root: PathBuf, entry: String },
}

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Version,
    Format {
        path: PathBuf,
    },
    Inspect {
        input: Input,
        module_path: String,
        offset: usize,
    },
    Check {
        input: Input,
        output: DiagnosticOutput,
    },
    Run {
        input: Input,
        /// Everything after `--`: the `List<Text>` passed to `main`.
        arguments: Vec<String>,
    },
    Build {
        input: Input,
        output: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiagnosticOutput {
    Human,
    Json,
}

#[derive(Debug, Eq, PartialEq)]
struct UsageError {
    message: &'static str,
}

/// A command and the standard tree directory that replaces the embedded one.
#[derive(Debug, Eq, PartialEq)]
struct Invocation {
    std_dir: Option<PathBuf>,
    command: Command,
}

/// Reads a leading `--std-dir <path>`, then the command. Only commands that
/// load a program accept it.
fn parse_invocation(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<Invocation, UsageError> {
    let mut arguments = arguments.into_iter().peekable();
    let std_dir = if arguments
        .next_if(|argument| argument == "--std-dir")
        .is_some()
    {
        let Some(path) = arguments.next() else {
            return Err(UsageError {
                message: "--std-dir requires a directory",
            });
        };
        Some(PathBuf::from(path))
    } else {
        None
    };
    let command = parse_command(arguments)?;
    if std_dir.is_some() && matches!(command, Command::Version | Command::Format { .. }) {
        return Err(UsageError {
            message: "--std-dir applies only to commands that load a program",
        });
    }
    Ok(Invocation { std_dir, command })
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

    if command == "format" {
        let Some(path) = arguments.next() else {
            return Err(UsageError {
                message: "format requires a source path",
            });
        };
        return if arguments.next().is_none() {
            Ok(Command::Format {
                path: PathBuf::from(path),
            })
        } else {
            Err(UsageError {
                message: "format received unexpected arguments",
            })
        };
    }

    if command == "inspect" {
        let Some(first) = arguments.next() else {
            return Err(UsageError {
                message: "inspect requires a source path and byte offset",
            });
        };
        let (input, module_path, offset) = if first == "--root" {
            let Some(root) = arguments.next() else {
                return Err(UsageError {
                    message: "inspect --root requires a project directory, entry module, module, and byte offset",
                });
            };
            let Some(entry) = arguments.next().and_then(|value| value.into_string().ok()) else {
                return Err(UsageError {
                    message: "inspect --root requires a project directory, entry module, module, and byte offset",
                });
            };
            let Some(module_path) = arguments.next().and_then(|value| value.into_string().ok())
            else {
                return Err(UsageError {
                    message: "inspect --root requires a project directory, entry module, module, and byte offset",
                });
            };
            let Some(offset) = arguments.next().and_then(parse_offset) else {
                return Err(UsageError {
                    message: "inspection offset must be a non-negative decimal byte offset",
                });
            };
            (
                Input::Project {
                    root: PathBuf::from(root),
                    entry,
                },
                module_path,
                offset,
            )
        } else {
            let Some(offset) = arguments.next().and_then(parse_offset) else {
                return Err(UsageError {
                    message: "inspection offset must be a non-negative decimal byte offset",
                });
            };
            (
                Input::File {
                    path: PathBuf::from(first),
                },
                "main".to_owned(),
                offset,
            )
        };
        return if arguments.next().is_none() {
            Ok(Command::Inspect {
                input,
                module_path,
                offset,
            })
        } else {
            Err(UsageError {
                message: "inspect received unexpected arguments",
            })
        };
    }

    if command == "check" || command == "run" || command == "build" {
        let Some(mut first) = arguments.next() else {
            return Err(UsageError {
                message: "check, run, and build require a source path or project root",
            });
        };
        let output = if command == "check" && first == "--json" {
            let Some(path_or_root) = arguments.next() else {
                return Err(UsageError {
                    message: "--json requires a source path or project root",
                });
            };
            first = path_or_root;
            DiagnosticOutput::Json
        } else {
            DiagnosticOutput::Human
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
        if command == "build" {
            let output = match arguments.next() {
                None => None,
                Some(flag) if flag == "-o" => {
                    let Some(path) = arguments.next() else {
                        return Err(UsageError {
                            message: "-o requires an executable path",
                        });
                    };
                    Some(PathBuf::from(path))
                }
                Some(_) => {
                    return Err(UsageError {
                        message: "build received unexpected arguments",
                    });
                }
            };
            return if arguments.next().is_none() {
                Ok(Command::Build { input, output })
            } else {
                Err(UsageError {
                    message: "build received unexpected arguments",
                })
            };
        }
        if command == "run" {
            let application_arguments = match arguments.next() {
                None => Vec::new(),
                Some(separator) if separator == "--" => {
                    let Ok(values) = arguments
                        .map(OsString::into_string)
                        .collect::<Result<Vec<_>, _>>()
                    else {
                        return Err(UsageError {
                            message: "application arguments must be valid UTF-8",
                        });
                    };
                    values
                }
                Some(_) => {
                    return Err(UsageError {
                        message: "run received unexpected arguments; pass application arguments after --",
                    });
                }
            };
            return Ok(Command::Run {
                input,
                arguments: application_arguments,
            });
        }
        return if arguments.next().is_some() {
            Err(UsageError {
                message: "check and run received unexpected arguments",
            })
        } else {
            Ok(Command::Check { input, output })
        };
    }

    Err(UsageError {
        message: "unknown command",
    })
}

fn parse_offset(value: OsString) -> Option<usize> {
    value.into_string().ok()?.parse().ok()
}

fn execute(Invocation { std_dir, command }: Invocation) -> ExitCode {
    let standard = match std_dir.map(StandardTree::directory).transpose() {
        Ok(standard) => standard.unwrap_or_default(),
        Err(error) => {
            eprintln!("chimi: {error}");
            return ExitCode::FAILURE;
        }
    };
    match command {
        Command::Version => {
            println!("{VERSION}");
            ExitCode::SUCCESS
        }
        Command::Format { path } => format(path),
        Command::Inspect {
            input,
            module_path,
            offset,
        } => inspect(input, &standard, &module_path, offset),
        Command::Check { input, output } => check(input, &standard, output),
        Command::Run { input, arguments } => run(input, &standard, arguments),
        Command::Build { input, output } => build(input, &standard, output),
    }
}

fn inspect(input: Input, standard: &StandardTree, module_path: &str, offset: usize) -> ExitCode {
    let Some(project) = load_project(input, standard) else {
        return ExitCode::FAILURE;
    };
    match Compiler::inspect_project(project.sources(), module_path, offset) {
        Ok(report) => {
            print!("{}", report.render_json());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("chimi: {error}");
            ExitCode::FAILURE
        }
    }
}

fn format(path: PathBuf) -> ExitCode {
    let source = match chimi::source::SourceSnapshot::read(&path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("chimi: {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let report = Compiler::format(source);
    print!("{}", report.text());
    if report.is_blocked() {
        eprint!("{}", report.render_diagnostics());
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn check(input: Input, standard: &StandardTree, output: DiagnosticOutput) -> ExitCode {
    let Some(project) = load_project(input, standard) else {
        return ExitCode::FAILURE;
    };
    match Compiler::verify_project(project.sources()) {
        Ok(verification) => {
            match output {
                DiagnosticOutput::Human => {
                    eprint!("{}", project.render_diagnostics(&verification.warnings));
                }
                DiagnosticOutput::Json => {
                    print!(
                        "{}",
                        project.render_diagnostics_json(&verification.warnings)
                    );
                }
            }
            ExitCode::SUCCESS
        }
        Err(diagnostics) => {
            match output {
                DiagnosticOutput::Human => {
                    eprint!("{}", project.render_diagnostics(&diagnostics));
                }
                DiagnosticOutput::Json => {
                    print!("{}", project.render_diagnostics_json(&diagnostics));
                }
            }
            ExitCode::FAILURE
        }
    }
}

fn run(input: Input, standard: &StandardTree, arguments: Vec<String>) -> ExitCode {
    let Some(project) = load_project(input, standard) else {
        return ExitCode::FAILURE;
    };
    let program = match Compiler::prepare_backend(project.sources()) {
        Ok(program) => program,
        Err(diagnostics) => {
            eprint!("{}", project.render_diagnostics(&diagnostics));
            return ExitCode::FAILURE;
        }
    };
    // As the native runner does: a zero-input `main` rejects arguments rather
    // than dropping them, with the runner-configuration status 64.
    if !arguments.is_empty() && !program.entry_accepts_arguments() {
        eprintln!("chimi: program entry does not accept application arguments");
        return ExitCode::from(64);
    }
    let config = ExecutionConfig::for_process().with_application_arguments(arguments);
    let report = program.execute_oracle(config);
    print!("{}", report.render_json_lines());
    outcome_exit_code(&report.outcome)
}

/// The process status a native runner exits with for the same outcome, per
/// runtime-output-channels.md, so `chimi run` and a native build agree.
fn outcome_exit_code(outcome: &ExecutionOutcome) -> ExitCode {
    match outcome {
        ExecutionOutcome::Success(_) => ExitCode::SUCCESS,
        ExecutionOutcome::ExpectedFailure(_) => ExitCode::from(1),
        ExecutionOutcome::Defect(_) => ExitCode::from(70),
        ExecutionOutcome::Cancellation(_) => ExitCode::from(130),
    }
}

fn build(input: Input, standard: &StandardTree, output: Option<PathBuf>) -> ExitCode {
    let output = output.unwrap_or_else(|| default_executable_path(&input));
    let artifact_directory = artifact_directory(&output);
    let Some(project) = load_project(input, standard) else {
        return ExitCode::FAILURE;
    };
    let program = match Compiler::prepare_backend(project.sources()) {
        Ok(program) => program,
        Err(diagnostics) => {
            eprint!("{}", project.render_diagnostics(&diagnostics));
            return ExitCode::FAILURE;
        }
    };
    match build_application(&program, standard, &output, &artifact_directory) {
        Ok(report) => {
            print!("{}", report.render_json());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("chimi: build failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn default_executable_path(input: &Input) -> PathBuf {
    let name = match input {
        Input::File { path } => path.file_stem().and_then(|name| name.to_str()),
        Input::Project { entry, .. } => entry.rsplit('/').next(),
    }
    .filter(|name| !name.is_empty())
    .unwrap_or("chimi-program");
    PathBuf::from(name)
}

fn artifact_directory(output: &std::path::Path) -> PathBuf {
    let name = output
        .file_name()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| std::ffi::OsStr::new("chimi-program"));
    PathBuf::from("target/chimi").join(name)
}

fn load_project(input: Input, standard: &StandardTree) -> Option<LoadedProject> {
    let loaded = match input {
        Input::File { path } => LoadedProject::single_file(path, standard),
        Input::Project { root, entry } => LoadedProject::load(root, entry, standard),
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
    match parse_invocation(env::args_os().skip(1)) {
        Ok(invocation) => execute(invocation),
        Err(error) => {
            eprintln!("chimi: {}\n{USAGE}", error.message);
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, DiagnosticOutput, Input, UsageError, parse_command};
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
                },
                output: DiagnosticOutput::Human,
            })
        );
        assert_eq!(
            parse_command(arguments(&["check"])),
            Err(UsageError {
                message: "check, run, and build require a source path or project root"
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
                },
                arguments: Vec::new(),
            })
        );
        assert_eq!(
            parse_command(arguments(&["run", "program.chimi", "--", "a", "--", "-o"])),
            Ok(Command::Run {
                input: Input::File {
                    path: PathBuf::from("program.chimi")
                },
                arguments: vec!["a".to_owned(), "--".to_owned(), "-o".to_owned()],
            })
        );
        assert_eq!(
            parse_command(arguments(&["run", "program.chimi", "a"])),
            Err(UsageError {
                message: "run received unexpected arguments; pass application arguments after --"
            })
        );
    }

    #[test]
    fn parses_structured_check_for_files_and_projects() {
        assert_eq!(
            parse_command(arguments(&["check", "--json", "program.chimi"])),
            Ok(Command::Check {
                input: Input::File {
                    path: PathBuf::from("program.chimi")
                },
                output: DiagnosticOutput::Json,
            })
        );
        assert_eq!(
            parse_command(arguments(&[
                "check", "--json", "--root", "project", "app/main"
            ])),
            Ok(Command::Check {
                input: Input::Project {
                    root: PathBuf::from("project"),
                    entry: "app/main".to_owned(),
                },
                output: DiagnosticOutput::Json,
            })
        );
        assert_eq!(
            parse_command(arguments(&["check", "--json"])),
            Err(UsageError {
                message: "--json requires a source path or project root"
            })
        );
    }

    #[test]
    fn parses_one_format_path() {
        assert_eq!(
            parse_command(arguments(&["format", "program.chimi"])),
            Ok(Command::Format {
                path: PathBuf::from("program.chimi")
            })
        );
        assert_eq!(
            parse_command(arguments(&["format"])),
            Err(UsageError {
                message: "format requires a source path"
            })
        );
    }

    #[test]
    fn parses_file_and_project_inspection_offsets() {
        assert_eq!(
            parse_command(arguments(&["inspect", "program.chimi", "42"])),
            Ok(Command::Inspect {
                input: Input::File {
                    path: PathBuf::from("program.chimi")
                },
                module_path: "main".to_owned(),
                offset: 42,
            })
        );
        assert_eq!(
            parse_command(arguments(&[
                "inspect",
                "--root",
                "project",
                "app/main",
                "lib/value",
                "17"
            ])),
            Ok(Command::Inspect {
                input: Input::Project {
                    root: PathBuf::from("project"),
                    entry: "app/main".to_owned(),
                },
                module_path: "lib/value".to_owned(),
                offset: 17,
            })
        );
        assert_eq!(
            parse_command(arguments(&["inspect", "program.chimi", "not-a-number"])),
            Err(UsageError {
                message: "inspection offset must be a non-negative decimal byte offset"
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
                },
                arguments: Vec::new(),
            })
        );
        assert_eq!(
            parse_command(arguments(&["run", "--root", "project", "app/main", "--"])),
            Ok(Command::Run {
                input: Input::Project {
                    root: PathBuf::from("project"),
                    entry: "app/main".to_owned()
                },
                arguments: Vec::new(),
            })
        );
        assert_eq!(
            parse_command(arguments(&[
                "build", "--root", "project", "app/main", "-o", "dist/app"
            ])),
            Ok(Command::Build {
                input: Input::Project {
                    root: PathBuf::from("project"),
                    entry: "app/main".to_owned()
                },
                output: Some(PathBuf::from("dist/app")),
            })
        );
    }

    #[test]
    fn parses_build_output_policy() {
        assert_eq!(
            parse_command(arguments(&["build", "program.chimi"])),
            Ok(Command::Build {
                input: Input::File {
                    path: PathBuf::from("program.chimi")
                },
                output: None,
            })
        );
        assert_eq!(
            parse_command(arguments(&["build", "program.chimi", "-o", "dist/program"])),
            Ok(Command::Build {
                input: Input::File {
                    path: PathBuf::from("program.chimi")
                },
                output: Some(PathBuf::from("dist/program")),
            })
        );
        assert_eq!(
            parse_command(arguments(&["build", "program.chimi", "-o"])),
            Err(UsageError {
                message: "-o requires an executable path"
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
