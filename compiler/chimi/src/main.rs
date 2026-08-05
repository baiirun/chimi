use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

const VERSION: &str = concat!("chimi ", env!("CARGO_PKG_VERSION"), "-bootstrap");
const USAGE: &str = "usage: chimi --version | chimi check <path>";

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Version,
    Check { path: PathBuf },
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

    if command == "check" {
        let Some(path) = arguments.next() else {
            return Err(UsageError {
                message: "check requires a source path",
            });
        };
        return if arguments.next().is_none() {
            Ok(Command::Check {
                path: PathBuf::from(path),
            })
        } else {
            Err(UsageError {
                message: "check accepts exactly one source path",
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
        Command::Check { path } => {
            eprintln!("chimi: frontend unavailable for {}", path.display());
            ExitCode::FAILURE
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
                message: "check requires a source path"
            })
        );
        assert_eq!(
            parse_command(arguments(&["check", "one.chimi", "two.chimi"])),
            Err(UsageError {
                message: "check accepts exactly one source path"
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
            parse_command(arguments(&["run"])),
            Err(UsageError {
                message: "unknown command"
            })
        );
    }
}
