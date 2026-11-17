use chimi::Compiler;
use chimi::backend::native::{build_application, check_build_target};
use chimi::execution::{ExecutionConfig, ExecutionOutcome, ExecutionReport, ProgramExit};
use chimi::project::{LoadedProject, StandardTree};
use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

const VERSION: &str = concat!("chimi ", env!("CARGO_PKG_VERSION"), "-bootstrap");
const USAGE: &str = concat!(
    "usage: chimi --version | chimi format <path> | chimi doc [--json] <path> | ",
    "chimi doc [--json] --root <project-dir> <module> | chimi doc [--json] std/<module> | ",
    "chimi doc --html <directory> (<path> | --root <project-dir> <module> | std) | ",
    "chimi trace show [--all] (<trace> | -) [<path> | <directory> | --root <project-dir> <entry-module>] | ",
    "chimi lint [--json] (<path> | <directory> | --root <project-dir> <module>) | chimi inspect <path> <byte-offset> | ",
    "chimi inspect --root <project-dir> <entry-module> <module> <byte-offset> | ",
    "chimi check [--json] <path> | chimi run <path> [-- <argument>...] | ",
    "chimi build <path> [-o <executable>] [--target <triple>] | ",
    "chimi check [--json] --root <project-dir> <entry-module> | chimi run --root ",
    "<project-dir> <entry-module> [-- <argument>...] | chimi build --root <project-dir> <entry-module> ",
    "[-o <executable>] [--target <triple>] | chimi test [<path> | --root <project-dir> <module>] [--filter=<text>] ",
    "[--backend=native|interpreter] [--clock=virtual|real] [--clock-start=<timestamp>] ",
    "[--seed=<n>] [--time-limit=<duration>] [--jobs=<n>] [--show-output] [--json] ",
    "[--trace=off|boundaries|full] | ",
    "chimi std check | chimi std test [<test option>...]; a compiler developer may put ",
    "--std-dir <directory> before inspect, check, run, build, test, std check, or std test"
);

#[derive(Debug, Eq, PartialEq)]
enum Input {
    File { path: PathBuf },
    Project { root: PathBuf, entry: String },
    /// Every module under `root` (`chimi test <directory>` only).
    Directory { root: PathBuf },
}

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Version,
    Format {
        path: PathBuf,
    },
    /// Reports convention findings in the project's own modules.
    Lint {
        input: Input,
        output: DiagnosticOutput,
    },
    /// Reads one run's telemetry after the fact.
    TraceShow {
        trace: PathBuf,
        /// The program the trace came from, to name source locations.
        source: Option<Input>,
        all: bool,
    },
    /// Prints one module's public surface.
    Doc {
        target: DocTarget,
        output: DocOutput,
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
        /// The target triple; `None` builds for the host.
        target: Option<String>,
    },
    /// Checks the whole standard tree against the operation registry.
    StdCheck,
    /// Runs the doc-comment examples of the standard tree as tests.
    StdTest {
        options: TestOptions,
    },
    Test {
        input: Input,
        options: TestOptions,
    },
}

/// What `chimi doc` writes.
#[derive(Debug, Eq, PartialEq)]
enum DocOutput {
    /// The module's surface on standard output, like `go doc`.
    Text,
    Json,
    /// Static pages for every module of the target's project, or of the
    /// standard tree for a `std/` target, written into `directory`.
    Html { directory: PathBuf },
}

/// The module `chimi doc` documents.
#[derive(Debug, Eq, PartialEq)]
enum DocTarget {
    /// The `main` module of a single file.
    File { path: PathBuf },
    /// `module` of the project rooted at `root`.
    Project { root: PathBuf, module: String },
    /// A module of the standard tree, such as `std/option`.
    Standard { module: String },
}

#[derive(Debug, Eq, PartialEq)]
struct TestOptions {
    filters: Vec<String>,
    backend: chimi::testing::Backend,
    virtual_clock: bool,
    clock_start: Option<String>,
    seed: u64,
    time_limit: std::time::Duration,
    jobs: Option<usize>,
    show_output: bool,
    json: bool,
    /// `None` is `--trace=off`.
    trace: Option<chimi::testing::TraceLevel>,
}

impl Default for TestOptions {
    fn default() -> Self {
        Self {
            filters: Vec::new(),
            backend: chimi::testing::Backend::Native,
            virtual_clock: true,
            clock_start: None,
            seed: 0,
            time_limit: std::time::Duration::from_secs(60),
            jobs: None,
            show_output: false,
            json: false,
            trace: None,
        }
    }
}

/// Parses `chimi test [<path> | --root <project> <module>] [options]`.
fn parse_test_command(arguments: impl Iterator<Item = OsString>) -> Result<Command, UsageError> {
    let (input, options) = parse_test_arguments(arguments)?;
    let Some(input) = input else {
        return Err(UsageError {
            message: "test requires a source path or project root",
        });
    };
    Ok(Command::Test { input, options })
}

/// Parses `chimi std test [options]`: the `chimi test` options, no input.
fn parse_std_test_command(
    arguments: impl Iterator<Item = OsString>,
) -> Result<Command, UsageError> {
    match parse_test_arguments(arguments)? {
        (None, options) => Ok(Command::StdTest { options }),
        (Some(_), _) => Err(UsageError {
            message: "std test takes options only; it runs the standard tree's examples",
        }),
    }
}

fn parse_test_arguments(
    arguments: impl Iterator<Item = OsString>,
) -> Result<(Option<Input>, TestOptions), UsageError> {
    let usage = |message| Err(UsageError { message });
    let mut options = TestOptions::default();
    let mut input = None;
    let mut arguments = arguments.peekable();
    while let Some(argument) = arguments.next() {
        let Ok(argument) = argument.into_string() else {
            return usage("test arguments must be valid UTF-8");
        };
        if argument == "--root" {
            let (Some(root), Some(entry)) = (arguments.next(), arguments.next()) else {
                return usage("--root requires a project directory and module");
            };
            let Ok(entry) = entry.into_string() else {
                return usage("entry module must be valid UTF-8");
            };
            if input.is_some() {
                return usage("test takes one source path or project root");
            }
            input = Some(Input::Project {
                root: PathBuf::from(root),
                entry,
            });
        } else if let Some(filter) = argument.strip_prefix("--filter=") {
            options.filters.push(filter.to_owned());
        } else if let Some(backend) = argument.strip_prefix("--backend=") {
            options.backend = match backend {
                "native" => chimi::testing::Backend::Native,
                "interpreter" => chimi::testing::Backend::Interpreter,
                _ => return usage("--backend must be native or interpreter"),
            };
        } else if let Some(clock) = argument.strip_prefix("--clock=") {
            options.virtual_clock = match clock {
                "virtual" => true,
                "real" => false,
                _ => return usage("--clock must be virtual or real"),
            };
        } else if let Some(start) = argument.strip_prefix("--clock-start=") {
            if !chimi::testing::valid_clock_start(start) {
                return usage(
                    "--clock-start requires an RFC 3339 timestamp from 1678 through 2262, such as 2026-11-10T09:30:00Z",
                );
            }
            options.clock_start = Some(start.to_owned());
        } else if let Some(limit) = argument.strip_prefix("--time-limit=") {
            let Some(limit) = chimi::testing::parse_time_limit(limit) else {
                return usage("--time-limit must be a positive duration such as 60s");
            };
            options.time_limit = limit;
        } else if let Some(seed) = argument.strip_prefix("--seed=") {
            let Ok(seed) = seed.parse::<u64>() else {
                return usage("--seed must be an integer from 0 through 18446744073709551615");
            };
            options.seed = seed;
        } else if let Some(jobs) = argument.strip_prefix("--jobs=") {
            match jobs.parse::<usize>() {
                Ok(jobs) if jobs > 0 => options.jobs = Some(jobs),
                _ => return usage("--jobs must be a positive integer"),
            }
        } else if argument == "--show-output" {
            options.show_output = true;
        } else if argument == "--json" {
            options.json = true;
        } else if let Some(trace) = argument.strip_prefix("--trace=") {
            options.trace = match trace {
                "off" => None,
                "boundaries" => Some(chimi::testing::TraceLevel::Boundaries),
                "full" => Some(chimi::testing::TraceLevel::Full),
                _ => return usage("--trace must be off, boundaries, or full"),
            };
        } else if argument.starts_with("--") {
            return usage("unknown test option");
        } else if input.is_some() {
            return usage("test takes one source path or project root");
        } else {
            let path = PathBuf::from(argument);
            input = Some(if path.is_dir() {
                Input::Directory { root: path }
            } else {
                Input::File { path }
            });
        }
    }
    if options.clock_start.is_some() && !options.virtual_clock {
        return usage("--clock-start requires --clock=virtual");
    }
    if options.clock_start.is_some() && options.backend == chimi::testing::Backend::Interpreter {
        return usage("--clock-start requires --backend=native");
    }
    Ok((input, options))
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

    if command == "doc" {
        return parse_doc_command(arguments);
    }

    if command == "lint" {
        return parse_lint_command(arguments);
    }

    if command == "trace" {
        return parse_trace_command(arguments);
    }

    if command == "std" {
        return match arguments.next() {
            Some(subcommand) if subcommand == "check" => match arguments.next() {
                None => Ok(Command::StdCheck),
                Some(_) => Err(UsageError {
                    message: "std takes one subcommand: check or test",
                }),
            },
            Some(subcommand) if subcommand == "test" => parse_std_test_command(arguments),
            _ => Err(UsageError {
                message: "std takes one subcommand: check or test",
            }),
        };
    }

    if command == "test" {
        return parse_test_command(arguments);
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
            let mut output = None;
            let mut target = None;
            while let Some(flag) = arguments.next() {
                let (slot, message) = if flag == "-o" && output.is_none() {
                    (&mut output, "-o requires an executable path")
                } else if flag == "--target" && target.is_none() {
                    (&mut target, "--target requires a target triple")
                } else {
                    return Err(UsageError {
                        message: "build received unexpected arguments",
                    });
                };
                let Some(value) = arguments.next() else {
                    return Err(UsageError { message });
                };
                *slot = Some(value);
            }
            let Some(target) = target.map(OsString::into_string).transpose().ok() else {
                return Err(UsageError {
                    message: "--target must be valid UTF-8",
                });
            };
            return Ok(Command::Build {
                input,
                output: output.map(PathBuf::from),
                target,
            });
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

/// Parses `chimi doc [--json | --html <directory>] (<path> | --root <project> <module> | std/<module>)`.
fn parse_doc_command(arguments: impl Iterator<Item = OsString>) -> Result<Command, UsageError> {
    let mut arguments = arguments.peekable();
    let output = if arguments.next_if(|argument| argument == "--json").is_some() {
        DocOutput::Json
    } else if arguments.next_if(|argument| argument == "--html").is_some() {
        let Some(directory) = arguments.next() else {
            return Err(UsageError {
                message: "doc --html requires an output directory",
            });
        };
        DocOutput::Html {
            directory: PathBuf::from(directory),
        }
    } else {
        DocOutput::Text
    };
    let target = match arguments.next() {
        None => {
            return Err(UsageError {
                message: "doc requires a source path, a project root and module, or a std/ module",
            });
        }
        Some(flag) if flag == "--root" => {
            let (Some(root), Some(module)) = (arguments.next(), arguments.next()) else {
                return Err(UsageError {
                    message: "doc --root requires a project directory and a module",
                });
            };
            let Ok(module) = module.into_string() else {
                return Err(UsageError {
                    message: "doc module paths must be valid UTF-8",
                });
            };
            DocTarget::Project {
                root: PathBuf::from(root),
                module,
            }
        }
        Some(argument) => match argument.to_str() {
            Some(module)
                if module == "std" || (module.starts_with("std/") && !module.ends_with(".chimi")) =>
            {
                DocTarget::Standard {
                    module: module.to_owned(),
                }
            }
            _ => DocTarget::File {
                path: PathBuf::from(argument),
            },
        },
    };
    if arguments.next().is_some() {
        return Err(UsageError {
            message: "doc received unexpected arguments",
        });
    }
    let names_one_std_module = matches!(&target, DocTarget::Standard { module } if module != "std");
    let names_all_of_std = matches!(&target, DocTarget::Standard { module } if module == "std");
    match output {
        DocOutput::Html { .. } if names_one_std_module => Err(UsageError {
            message: "doc --html writes the whole standard tree; name it as std",
        }),
        DocOutput::Text | DocOutput::Json if names_all_of_std => Err(UsageError {
            message: "doc requires a source path, a project root and module, or a std/ module",
        }),
        _ => Ok(Command::Doc { target, output }),
    }
}

/// Parses `chimi trace show [--all] <trace> [<path> | <directory> | --root <project> <entry>]`.
fn parse_trace_command(arguments: impl Iterator<Item = OsString>) -> Result<Command, UsageError> {
    let mut arguments = arguments.peekable();
    if arguments.next().is_none_or(|subcommand| subcommand != "show") {
        return Err(UsageError {
            message: "trace requires the show subcommand",
        });
    }
    let all = arguments.next_if(|argument| argument == "--all").is_some();
    let Some(trace) = arguments.next() else {
        return Err(UsageError {
            message: "trace show requires a trace file, or - for standard input",
        });
    };
    let source = match arguments.next() {
        None => None,
        Some(flag) if flag == "--root" => {
            let (Some(root), Some(entry)) = (arguments.next(), arguments.next()) else {
                return Err(UsageError {
                    message: "trace show --root requires a project directory and an entry module",
                });
            };
            let Ok(entry) = entry.into_string() else {
                return Err(UsageError {
                    message: "trace show module paths must be valid UTF-8",
                });
            };
            Some(Input::Project {
                root: PathBuf::from(root),
                entry,
            })
        }
        Some(path) => {
            let path = PathBuf::from(path);
            Some(if path.is_dir() {
                Input::Directory { root: path }
            } else {
                Input::File { path }
            })
        }
    };
    if arguments.next().is_some() {
        return Err(UsageError {
            message: "trace show received unexpected arguments",
        });
    }
    Ok(Command::TraceShow {
        trace: PathBuf::from(trace),
        source,
        all,
    })
}

/// Parses `chimi lint [--json] (<path> | <directory> | --root <project> <module>)`.
fn parse_lint_command(arguments: impl Iterator<Item = OsString>) -> Result<Command, UsageError> {
    let mut arguments = arguments.peekable();
    let output = if arguments.next_if(|argument| argument == "--json").is_some() {
        DiagnosticOutput::Json
    } else {
        DiagnosticOutput::Human
    };
    let input = match arguments.next() {
        None => {
            return Err(UsageError {
                message: "lint requires a source path, a directory, or a project root and module",
            });
        }
        Some(flag) if flag == "--root" => {
            let (Some(root), Some(entry)) = (arguments.next(), arguments.next()) else {
                return Err(UsageError {
                    message: "lint --root requires a project directory and a module",
                });
            };
            let Ok(entry) = entry.into_string() else {
                return Err(UsageError {
                    message: "lint module paths must be valid UTF-8",
                });
            };
            Input::Project {
                root: PathBuf::from(root),
                entry,
            }
        }
        Some(path) => {
            let path = PathBuf::from(path);
            if path.is_dir() {
                Input::Directory { root: path }
            } else {
                Input::File { path }
            }
        }
    };
    if arguments.next().is_some() {
        return Err(UsageError {
            message: "lint received unexpected arguments",
        });
    }
    Ok(Command::Lint { input, output })
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
        Command::Doc { target, output } => doc(target, &standard, output),
        Command::Lint { input, output } => lint(input, &standard, output),
        Command::TraceShow { trace, source, all } => trace_show(&trace, source, &standard, all),
        Command::Inspect {
            input,
            module_path,
            offset,
        } => inspect(input, &standard, &module_path, offset),
        Command::Check { input, output } => check(input, &standard, output),
        Command::Run { input, arguments } => run(input, &standard, arguments),
        Command::Build {
            input,
            output,
            target,
        } => build(input, &standard, output, target.as_deref()),
        Command::StdCheck => std_check(&standard),
        Command::StdTest { options } => std_test(&standard, options),
        Command::Test { input, options } => test(input, &standard, options),
    }
}

/// Exits 0 with no findings, 1 with findings or when the project does not
/// type-check, since a lint finding on code that does not compile would mislead.
fn lint(input: Input, standard: &StandardTree, output: DiagnosticOutput) -> ExitCode {
    let Some(project) = load_project(input, standard) else {
        return ExitCode::FAILURE;
    };
    let report = Compiler::type_project(project.sources());
    let findings = if report.is_success() {
        report.lint(false)
    } else {
        report.diagnostics().to_vec()
    };
    match output {
        DiagnosticOutput::Human => eprint!("{}", project.render_diagnostics(&findings)),
        DiagnosticOutput::Json => print!("{}", project.render_diagnostics_json(&findings)),
    }
    if findings.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn trace_show(
    trace: &std::path::Path,
    source: Option<Input>,
    standard: &StandardTree,
    all: bool,
) -> ExitCode {
    let text = if trace.as_os_str() == "-" {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).map(|_| text)
    } else {
        std::fs::read_to_string(trace)
    };
    let text = match text {
        Ok(text) => text,
        Err(error) => {
            eprintln!("chimi: cannot read {}: {error}", trace.display());
            return ExitCode::FAILURE;
        }
    };
    let with_source = source.is_some();
    let locator: Box<dyn Fn(u32, u32) -> Option<String>> = match source {
        None => Box::new(|_, _| None),
        Some(input) => {
            let Some(project) = load_project(input, standard) else {
                return ExitCode::FAILURE;
            };
            Box::new(project.source_locator())
        }
    };
    // A place the source cannot name means the source is not the program
    // the run was built from, so its other locations may be wrong too.
    let missed = std::cell::Cell::new(false);
    let locate = |file: u32, offset: u32| {
        let location = locator(file, offset);
        missed.set(missed.get() || location.is_none());
        location
    };
    match chimi::render_trace(&text, &locate, all) {
        Ok(report) => {
            // A closed pipe, as in `| head`, is not an error.
            let _ = std::io::Write::write_all(&mut std::io::stdout(), report.as_bytes());
            if with_source && missed.get() {
                eprintln!(
                    "chimi: warning: some places in the trace are not in the given source; \
                     give the same file, directory or project the run was built from"
                );
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("chimi: {}: {error}", trace.display());
            ExitCode::FAILURE
        }
    }
}

fn doc(target: DocTarget, standard: &StandardTree, output: DocOutput) -> ExitCode {
    let standard_target = matches!(target, DocTarget::Standard { .. });
    let (loaded, module, title) = match target {
        DocTarget::File { path } => {
            let title = path.file_stem().map_or_else(
                || "main".to_owned(),
                |stem| stem.to_string_lossy().into_owned(),
            );
            (LoadedProject::single_file(path, standard), "main".to_owned(), title)
        }
        DocTarget::Project { root, module } => {
            let title = root
                .canonicalize()
                .ok()
                .and_then(|root| root.file_name().map(|name| name.to_string_lossy().into_owned()))
                .unwrap_or_else(|| module.clone());
            (LoadedProject::load(root, module.clone(), standard), module, title)
        }
        DocTarget::Standard { module } => {
            (LoadedProject::standard_tree(standard), module, "std".to_owned())
        }
    };
    let project = match loaded {
        Ok(project) => project,
        Err(error) => {
            eprintln!("chimi: {error}");
            return ExitCode::FAILURE;
        }
    };
    let report = Compiler::type_project(project.sources());
    if !report.is_success() {
        eprint!("{}", project.render_diagnostics(report.diagnostics()));
        return ExitCode::FAILURE;
    }
    let rendered = match output {
        DocOutput::Html { directory } => {
            // A std target documents the standard tree; any other target only
            // its project's own modules.
            let modules = report.documentation_of(|path| {
                let in_std = path.starts_with("std/");
                if standard_target { in_std } else { !in_std }
            });
            return write_doc_site(&directory, &title, &modules);
        }
        DocOutput::Text => report.documentation(&module).map(|doc| doc.render_text()),
        DocOutput::Json => report.documentation(&module).map(|doc| doc.render_json()),
    };
    let Some(rendered) = rendered else {
        eprintln!("chimi: no module `{module}`");
        return ExitCode::FAILURE;
    };
    print!("{rendered}");
    ExitCode::SUCCESS
}

fn write_doc_site(
    directory: &std::path::Path,
    title: &str,
    modules: &[chimi::ModuleDocumentation],
) -> ExitCode {
    for page in chimi::render_site(title, modules) {
        let path = directory.join(&page.path);
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, page.html));
        if let Err(error) = written {
            eprintln!("chimi: cannot write {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

fn std_check(standard: &StandardTree) -> ExitCode {
    let tree = match LoadedProject::standard_tree(standard) {
        Ok(tree) => tree,
        Err(error) => {
            eprintln!("chimi: {error}");
            return ExitCode::FAILURE;
        }
    };
    let verification = Compiler::verify_standard_tree(tree.sources());
    eprint!("{}", tree.render_diagnostics(&verification.diagnostics));
    for mismatch in &verification.mismatches {
        eprintln!("chimi: {mismatch}");
    }
    if !verification.matches() {
        return ExitCode::FAILURE;
    }
    let location = standard.override_directory().map_or_else(
        || "embedded standard tree".to_owned(),
        |directory| format!("standard tree in {}", directory.display()),
    );
    println!(
        "chimi: the {location} matches the operation registry ({} modules)",
        tree.sources().len()
    );
    ExitCode::SUCCESS
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
    let Some(mut project) = load_project(input, standard) else {
        return ExitCode::FAILURE;
    };
    if let Err(error) = project.add_doc_examples(standard) {
        eprintln!("chimi: {error}");
        return ExitCode::FAILURE;
    }
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
    if let Some(refusal) = program.interpreter_refusal() {
        eprintln!("chimi: {refusal}");
        return ExitCode::FAILURE;
    }
    // As the native runner does: a zero-input `main` rejects arguments rather
    // than dropping them, with the runner-configuration status 64.
    if !arguments.is_empty() && !program.entry_accepts_arguments() {
        eprintln!("chimi: program entry does not accept application arguments");
        return ExitCode::from(64);
    }
    let config = ExecutionConfig::for_process().with_application_arguments(arguments);
    let report = program.execute_oracle(config);
    print!("{}", report.render_json_lines());
    exit_code(&report)
}

/// The status of `report`: its `std/cli.Exit`, applied after the outcome as
/// the native runner applies it, or else its outcome's.
fn exit_code(report: &ExecutionReport) -> ExitCode {
    match &report.program_exit {
        Some(ProgramExit::Code(code)) => match u8::try_from(*code) {
            Ok(code) => ExitCode::from(code),
            Err(_) => {
                eprintln!("Chimi runtime: Exit.Code({code}) is outside 0 to 255");
                ExitCode::from(70)
            }
        },
        Some(ProgramExit::Help(text)) => shown(&mut std::io::stdout(), text, 0),
        Some(ProgramExit::Usage(message)) => shown(&mut std::io::stderr(), message, 2),
        None => outcome_exit_code(&report.outcome),
    }
}

/// Writes `text` and a newline to `stream` and exits `status`, or 74 when the
/// write fails, as the native runner does.
fn shown(stream: &mut dyn std::io::Write, text: &str, status: u8) -> ExitCode {
    match writeln!(stream, "{text}").and_then(|()| stream.flush()) {
        Ok(()) => ExitCode::from(status),
        Err(_) => ExitCode::from(74),
    }
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

fn build(
    input: Input,
    standard: &StandardTree,
    output: Option<PathBuf>,
    target: Option<&str>,
) -> ExitCode {
    if let Some(Err(error)) = target.map(check_build_target) {
        eprintln!("chimi: build failed: {error}");
        return ExitCode::FAILURE;
    }
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

/// `chimi test`: exits 0 when every selected test passed, 1 when one failed,
/// and 2 when the run could not start (testing.md, Exit status).
fn test(input: Input, standard: &StandardTree, options: TestOptions) -> ExitCode {
    let artifact_directory = artifact_directory(&default_executable_path(&input)).join("test");
    let Some(mut project) = load_project(input, standard) else {
        return ExitCode::from(2);
    };
    match project.add_doc_examples(standard) {
        Ok(examples) => run_tests(&project, &examples, &artifact_directory, standard, options),
        Err(error) => {
            eprintln!("chimi: {error}");
            ExitCode::from(2)
        }
    }
}

/// `chimi std test`: the standard tree's doc-comment examples, run as
/// `chimi test` runs a project's.
fn std_test(standard: &StandardTree, options: TestOptions) -> ExitCode {
    let mut tree = match LoadedProject::standard_tree(standard) {
        Ok(tree) => tree,
        Err(error) => {
            eprintln!("chimi: {error}");
            return ExitCode::from(2);
        }
    };
    match tree.add_standard_doc_examples(standard) {
        Ok(examples) => {
            let artifact_directory = PathBuf::from("target/chimi/std-examples/test");
            run_tests(&tree, &examples, &artifact_directory, standard, options)
        }
        Err(error) => {
            eprintln!("chimi: {error}");
            ExitCode::from(2)
        }
    }
}

fn run_tests(
    project: &LoadedProject,
    examples: &[(String, String)],
    artifact_directory: &std::path::Path,
    standard: &StandardTree,
    options: TestOptions,
) -> ExitCode {
    use chimi::testing::{Backend, RunOptions, TestSuite, TraceOutput, render_json, render_report};
    let mut suite = match TestSuite::prepare(project.sources()) {
        Ok(suite) => suite,
        Err(diagnostics) => {
            eprint!("{}", project.render_diagnostics(&diagnostics));
            return ExitCode::from(2);
        }
    };
    suite.rename_modules(examples);
    let selected = suite.select(&options.filters);
    if selected.is_empty() {
        eprintln!("chimi: no test matches the filter");
        return ExitCode::from(2);
    }
    let trace = match options.trace {
        None => None,
        Some(level) => {
            let directory = artifact_directory.join("traces");
            // Traces from an earlier run would read as this run's.
            let cleared = match std::fs::remove_dir_all(&directory) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                _ => std::fs::create_dir_all(&directory),
            };
            if let Err(error) = cleared {
                eprintln!("chimi: could not prepare {}: {error}", directory.display());
                return ExitCode::from(2);
            }
            Some(TraceOutput { level, directory })
        }
    };
    let run = RunOptions {
        backend: options.backend,
        filters: options.filters,
        virtual_clock: options.virtual_clock,
        clock_start: options.clock_start,
        seed: options.seed,
        time_limit: options.time_limit,
        jobs: options.jobs.unwrap_or_else(|| {
            std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
        }),
        trace,
    };
    let results = match run.backend {
        Backend::Interpreter => suite.run_interpreted(&selected, &run),
        Backend::Native => {
            let executable = artifact_directory.join("tests");
            if let Err(error) = suite.build_native(standard, &executable, artifact_directory) {
                eprintln!("chimi: test build failed: {error}");
                return ExitCode::from(2);
            }
            suite.run_native(&executable, artifact_directory, &selected, &run)
        }
    };
    if options.json {
        print!("{}", render_json(&suite, &selected, &results, options.seed));
    } else {
        print!(
            "{}",
            render_report(&suite, &selected, &results, options.show_output, options.seed)
        );
    }
    if results.iter().all(|result| result.failure.is_none()) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn default_executable_path(input: &Input) -> PathBuf {
    let name = match input {
        Input::File { path } => path.file_stem().and_then(|name| name.to_str()),
        Input::Directory { root } => {
            let root = root.canonicalize().unwrap_or_else(|_| root.clone());
            return PathBuf::from(
                root.file_name()
                    .and_then(|name| name.to_str())
                    .filter(|name| !name.is_empty())
                    .unwrap_or("chimi-program"),
            );
        }
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
        Input::Directory { root } => LoadedProject::load_directory(root, standard),
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
    use super::{Command, DiagnosticOutput, DocOutput, DocTarget, Input, UsageError, parse_command};
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn arguments(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_doc_targets() {
        assert_eq!(
            parse_command(arguments(&["doc", "app.chimi"])),
            Ok(Command::Doc {
                target: DocTarget::File {
                    path: PathBuf::from("app.chimi")
                },
                output: DocOutput::Text,
            })
        );
        assert_eq!(
            parse_command(arguments(&["doc", "--json", "--root", "service", "app/orders"])),
            Ok(Command::Doc {
                target: DocTarget::Project {
                    root: PathBuf::from("service"),
                    module: "app/orders".to_owned(),
                },
                output: DocOutput::Json,
            })
        );
        assert_eq!(
            parse_command(arguments(&["doc", "std/option"])),
            Ok(Command::Doc {
                target: DocTarget::Standard {
                    module: "std/option".to_owned()
                },
                output: DocOutput::Text,
            })
        );
        assert_eq!(
            parse_command(arguments(&["doc", "--html", "site", "--root", "service", "app"])),
            Ok(Command::Doc {
                target: DocTarget::Project {
                    root: PathBuf::from("service"),
                    module: "app".to_owned(),
                },
                output: DocOutput::Html {
                    directory: PathBuf::from("site")
                },
            })
        );
        assert_eq!(
            parse_command(arguments(&["doc", "--html", "site", "std"])),
            Ok(Command::Doc {
                target: DocTarget::Standard {
                    module: "std".to_owned()
                },
                output: DocOutput::Html {
                    directory: PathBuf::from("site")
                },
            })
        );
        // A file under a `std` directory is still a file.
        assert_eq!(
            parse_command(arguments(&["doc", "std/option.chimi"])),
            Ok(Command::Doc {
                target: DocTarget::File {
                    path: PathBuf::from("std/option.chimi")
                },
                output: DocOutput::Text,
            })
        );
    }

    #[test]
    fn parses_trace_show() {
        assert_eq!(
            parse_command(arguments(&["trace", "show", "run.jsonl"])),
            Ok(Command::TraceShow {
                trace: PathBuf::from("run.jsonl"),
                source: None,
                all: false,
            })
        );
        assert_eq!(
            parse_command(arguments(&["trace", "show", "--all", "-", "--root", "svc", "app"])),
            Ok(Command::TraceShow {
                trace: PathBuf::from("-"),
                source: Some(Input::Project {
                    root: PathBuf::from("svc"),
                    entry: "app".to_owned(),
                }),
                all: true,
            })
        );
        for (command, message) in [
            (&["trace"][..], "trace requires the show subcommand"),
            (
                &["trace", "show"][..],
                "trace show requires a trace file, or - for standard input",
            ),
            (
                &["trace", "show", "a.jsonl", "a.chimi", "b"][..],
                "trace show received unexpected arguments",
            ),
        ] {
            assert_eq!(
                parse_command(arguments(command)),
                Err(UsageError { message }),
                "{command:?}"
            );
        }
    }

    #[test]
    fn rejects_malformed_doc_commands() {
        for (command, message) in [
            (
                &["doc"][..],
                "doc requires a source path, a project root and module, or a std/ module",
            ),
            (
                &["doc", "--root", "service"][..],
                "doc --root requires a project directory and a module",
            ),
            (&["doc", "a.chimi", "b.chimi"][..], "doc received unexpected arguments"),
            (&["doc", "--html"][..], "doc --html requires an output directory"),
            (
                &["doc", "--html", "site", "std/option"][..],
                "doc --html writes the whole standard tree; name it as std",
            ),
            (
                &["doc", "std"][..],
                "doc requires a source path, a project root and module, or a std/ module",
            ),
        ] {
            assert_eq!(
                parse_command(arguments(command)),
                Err(UsageError { message }),
                "{command:?}"
            );
        }
    }

    #[test]
    fn parses_lint_inputs() {
        assert_eq!(
            parse_command(arguments(&["lint", "--json", "--root", "service", "app/main"])),
            Ok(Command::Lint {
                input: Input::Project {
                    root: PathBuf::from("service"),
                    entry: "app/main".to_owned(),
                },
                output: DiagnosticOutput::Json,
            })
        );
        assert_eq!(
            parse_command(arguments(&["lint", "missing.chimi"])),
            Ok(Command::Lint {
                input: Input::File {
                    path: PathBuf::from("missing.chimi")
                },
                output: DiagnosticOutput::Human,
            })
        );
        let directory = std::env::temp_dir();
        assert_eq!(
            parse_command(vec![OsString::from("lint"), directory.clone().into_os_string()]),
            Ok(Command::Lint {
                input: Input::Directory { root: directory },
                output: DiagnosticOutput::Human,
            })
        );
        for (command, message) in [
            (
                &["lint"][..],
                "lint requires a source path, a directory, or a project root and module",
            ),
            (
                &["lint", "--root", "service"][..],
                "lint --root requires a project directory and a module",
            ),
            (&["lint", "a.chimi", "b.chimi"][..], "lint received unexpected arguments"),
        ] {
            assert_eq!(
                parse_command(arguments(command)),
                Err(UsageError { message }),
                "{command:?}"
            );
        }
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
                target: None,
            })
        );
    }

    #[test]
    fn parses_build_targets() {
        assert_eq!(
            parse_command(arguments(&[
                "build",
                "app.chimi",
                "--target",
                "aarch64-apple-darwin",
                "-o",
                "dist/app"
            ])),
            Ok(Command::Build {
                input: Input::File {
                    path: PathBuf::from("app.chimi")
                },
                output: Some(PathBuf::from("dist/app")),
                target: Some("aarch64-apple-darwin".to_owned()),
            })
        );
        for (command, message) in [
            (
                &["build", "app.chimi", "--target"][..],
                "--target requires a target triple",
            ),
            (
                &["build", "app.chimi", "--target", "a", "--target", "b"][..],
                "build received unexpected arguments",
            ),
            (
                &["build", "app.chimi", "-o", "a", "-o", "b"][..],
                "build received unexpected arguments",
            ),
        ] {
            assert_eq!(
                parse_command(arguments(command)),
                Err(UsageError { message }),
                "{command:?}"
            );
        }
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
                target: None,
            })
        );
        assert_eq!(
            parse_command(arguments(&["build", "program.chimi", "-o", "dist/program"])),
            Ok(Command::Build {
                input: Input::File {
                    path: PathBuf::from("program.chimi")
                },
                output: Some(PathBuf::from("dist/program")),
                target: None,
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
