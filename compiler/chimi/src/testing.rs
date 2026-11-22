//! `chimi test`: discovery, execution, and reports for `test "name" { ... }`
//! items (docs/specification/testing.md).
//!
//! A [`TestSuite`] is one build of a project's tests: every test is a root of a
//! single Core program. Each test then runs as its own execution, on the
//! interpreter in a thread of this process or natively as one process of a
//! shared test executable (`<exe> --test=<k>`), and its raw outcome is
//! classified into the kinds the spec names. Reports list tests in suite order
//! however many run at once.

use crate::backend::BackendProgram;
use crate::compiler::{ProjectSource, resolve_project_sources};
use crate::diagnostic::Diagnostic;
use crate::executable::{lower, seal_tests};
use crate::execution::core_operations::{ASSERT_EQUAL_HEADING, ASSERT_NOT_EQUAL_HEADING};
use crate::execution::{ExecutionConfig, ExecutionOutcome, TraceMode};
use crate::json::write_json_string;
use crate::line_index::{LineIndex, PositionEncoding};
use crate::typing::check_project;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// A `--time-limit` value in the `Duration` text form (`60s`, `1m30s`); `None`
/// unless it is positive.
/// Whether `text` is a `--clock-start` the native runtime accepts: RFC 3339
/// with an offset, years 1678 through 2262, and a result that fits Unix
/// nanoseconds in an `i64`. Mirrors `parse_timestamp` in runtime.c, so a bad
/// value is a usage error (exit 2) rather than a failure in every test.
pub fn valid_clock_start(text: &str) -> bool {
    clock_start_nanoseconds(text).is_some()
}

fn clock_start_nanoseconds(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    let separator = |at: &mut usize, allowed: &[u8]| -> Option<()> {
        let byte = *bytes.get(*at)?;
        allowed.contains(&byte).then(|| *at += 1)
    };
    let number = |at: &mut usize, count: usize| -> Option<i64> {
        let slice = bytes.get(*at..*at + count)?;
        if !slice.iter().all(u8::is_ascii_digit) {
            return None;
        }
        *at += count;
        Some(
            slice
                .iter()
                .fold(0, |value, digit| value * 10 + i64::from(digit - b'0')),
        )
    };
    let mut at = 0;
    let year = number(&mut at, 4)?;
    separator(&mut at, b"-")?;
    let month = number(&mut at, 2)?;
    separator(&mut at, b"-")?;
    let day = number(&mut at, 2)?;
    separator(&mut at, b"Tt")?;
    let hour = number(&mut at, 2)?;
    separator(&mut at, b":")?;
    let minute = number(&mut at, 2)?;
    separator(&mut at, b":")?;
    let second = number(&mut at, 2)?;
    let mut fraction = 0;
    if separator(&mut at, b".").is_some() {
        let start = at;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        let count = at - start;
        if count == 0 || count > 9 {
            return None;
        }
        fraction = number(&mut { start }, count)? * 10_i64.pow(u32::try_from(9 - count).ok()?);
    }
    let offset = match bytes.get(at)? {
        b'Z' | b'z' => {
            at += 1;
            0
        }
        sign @ (b'+' | b'-') => {
            let sign = if *sign == b'-' { -1 } else { 1 };
            at += 1;
            let hours = number(&mut at, 2)?;
            separator(&mut at, b":")?;
            let minutes = number(&mut at, 2)?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            sign * (hours * 3600 + minutes * 60)
        }
        _ => return None,
    };
    if at != bytes.len() {
        return None;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1678..=2262).contains(&year)
        || !(1..=12).contains(&month)
        || day < 1
        || day > month_days[usize::try_from(month - 1).ok()?]
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let seconds =
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset;
    seconds.checked_mul(1_000_000_000)?.checked_add(fraction)
}

/// Days since 1970-01-01 in the proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

pub fn parse_time_limit(text: &str) -> Option<Duration> {
    let nanoseconds = crate::duration::parse(text).ok()?;
    u64::try_from(nanoseconds)
        .ok()
        .filter(|nanoseconds| *nanoseconds > 0)
        .map(Duration::from_nanos)
}

/// One test, identified by its module path and name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestCase {
    pub module: String,
    pub name: String,
    /// The 1-based line of the test's name.
    pub line: u32,
}

impl TestCase {
    /// `app/checkout "charges the total once"`, the form reports and filters use.
    pub fn identity(&self) -> String {
        format!("{} \"{}\"", self.module, self.name)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceLocation {
    pub path: String,
    pub line: u32,
    pub column: u32,
}

impl std::fmt::Display for SourceLocation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}:{}:{}", self.path, self.line, self.column)
    }
}

/// Why a test failed (testing.md, Test outcomes).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureKind {
    Assertion,
    Failure,
    Defect,
    Cancelled,
    Deadlock,
    TimeLimit,
}

impl FailureKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Assertion => "assertion",
            Self::Failure => "failure",
            Self::Defect => "defect",
            Self::Cancelled => "cancelled",
            Self::Deadlock => "deadlock",
            Self::TimeLimit => "time_limit",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestFailure {
    pub kind: FailureKind,
    pub message: String,
    pub location: Option<SourceLocation>,
    /// The two renderings of a failed `assert_equal` or `assert_not_equal`.
    pub comparison: Option<(String, String)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestResult {
    pub failure: Option<TestFailure>,
    /// What the test wrote to its console.
    pub output: String,
    pub wall_ns: u64,
    /// Whether the test drew from its `random` provider, so a failure report
    /// names the run seed that replays it.
    pub random_read: bool,
    /// The test's telemetry file, when the run traced and the test wrote one.
    pub trace: Option<std::path::PathBuf>,
}

/// An execution's outcome before classification, from either backend.
#[derive(Clone, Debug)]
enum RawOutcome {
    Success,
    ExpectedFailure { rendering: String },
    Defect { message: String, node: Option<u32> },
    Cancelled,
    TimeLimit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backend {
    Native,
    Interpreter,
}

#[derive(Clone, Debug)]
pub struct RunOptions {
    pub backend: Backend,
    /// Substrings of a test's identity; empty selects every test.
    pub filters: Vec<String>,
    pub virtual_clock: bool,
    pub clock_start: Option<String>,
    /// The run seed (`--seed`, default 0); each test's `random` provider is
    /// seeded from it and the test's identity.
    pub seed: u64,
    pub time_limit: Duration,
    pub jobs: usize,
    /// `--trace=boundaries|full`; `None` under `--trace=off`.
    pub trace: Option<TraceOutput>,
}

/// Where a traced run writes each test's `chimi.runtime.v1` telemetry: one
/// JSON Lines file per test in `directory`.
#[derive(Clone, Debug)]
pub struct TraceOutput {
    pub level: TraceLevel,
    pub directory: std::path::PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceLevel {
    Boundaries,
    Full,
}

impl TraceLevel {
    const fn mode(self) -> TraceMode {
        match self {
            Self::Boundaries => TraceMode::Boundaries,
            Self::Full => TraceMode::Full,
        }
    }
}

pub struct TestSuite {
    program: BackendProgram,
    cases: Vec<TestCase>,
    /// By origin id.
    locations: Vec<SourceLocation>,
    /// Core call nodes whose callee is a `std/test` assertion: a defect raised
    /// at one of them is an assertion failure.
    assertion_calls: HashSet<u32>,
}

impl TestSuite {
    /// Reports each test in `renamed`'s first module (a doc example's own
    /// module) under its second, the module the example documents.
    pub fn rename_modules(&mut self, renamed: &[(String, String)]) {
        for case in &mut self.cases {
            if let Some((_, documented)) = renamed.iter().find(|(path, _)| *path == case.module) {
                case.module.clone_from(documented);
            }
        }
    }

    /// Type-checks the project and lowers every test item to one program.
    ///
    /// Ensures: `Err` holds the project's diagnostics, or `E5009` when it has
    /// no test. Cases are in module path order, then declaration order.
    pub fn prepare(sources: Vec<ProjectSource>) -> Result<Self, Vec<Diagnostic>> {
        let root = seal_tests(check_project(resolve_project_sources(sources)))?;
        let typed = Arc::clone(&root.typed);
        let core = lower(root)?;
        let modules = &typed.resolved.semantic.modules;
        let line_indexes = modules
            .iter()
            .map(|module| LineIndex::new(Arc::from(module.source.text())))
            .collect::<Vec<_>>();
        let position = |file: usize, offset: u32| {
            let position = line_indexes[file].position(offset, PositionEncoding::Utf16);
            (position.line + 1, position.character + 1)
        };
        let cases = core
            .tests
            .iter()
            .map(|test| {
                let symbol = typed.resolved.symbol(*test);
                let module_id = symbol.module_id.expect("a test belongs to a module");
                let module = &modules[module_id.raw() as usize];
                let declaration = module
                    .declarations
                    .iter()
                    .find(|declaration| Some(declaration.semantic_id) == symbol.semantic_id)
                    .expect("a test symbol has its declaration");
                TestCase {
                    module: module.path.clone(),
                    name: crate::semantic::test_name_of(&symbol.name)
                        .expect("a test symbol has a test name")
                        .to_owned(),
                    line: position(module_id.raw() as usize, declaration.name.range.start()).0,
                }
            })
            .collect();
        let file_modules = |file_id: u32| {
            modules
                .iter()
                .position(|module| module.source.file_id().raw() == file_id)
                .expect("an origin's file belongs to a loaded module")
        };
        let locations = core
            .origins
            .iter()
            .map(|origin| {
                let file = file_modules(origin.file_id.raw());
                let (line, column) = position(file, origin.range.start());
                SourceLocation {
                    path: modules[file].source.path().display().to_string(),
                    line,
                    column,
                }
            })
            .collect();
        let assertion_calls = core.assertion_calls();
        Ok(Self {
            program: BackendProgram::new(core),
            cases,
            locations,
            assertion_calls,
        })
    }

    pub fn cases(&self) -> &[TestCase] {
        &self.cases
    }

    /// The indexes of the cases `filters` selects, in suite order.
    pub fn select(&self, filters: &[String]) -> Vec<usize> {
        (0..self.cases.len())
            .filter(|index| {
                let case = &self.cases[*index];
                // The spec's `<module> <name>`, and the quoted form reports print.
                let plain = format!("{} {}", case.module, case.name);
                let quoted = case.identity();
                filters.is_empty()
                    || filters
                        .iter()
                        .any(|filter| plain.contains(filter) || quoted.contains(filter))
            })
            .collect()
    }

    /// Runs `selected` on the interpreter, `jobs` at a time, each in its own
    /// thread and execution. Results are in `selected` order.
    pub fn run_interpreted(&self, selected: &[usize], options: &RunOptions) -> Vec<TestResult> {
        run_parallel(selected, options.jobs, |index| {
            let started = Instant::now();
            let (outcome, output, random_read) = self.interpret(index, options);
            self.result(index, options, outcome, output, random_read, started)
        })
    }

    /// The seed of test `index`'s `random` provider (testing.md, Randomness).
    fn random_seed(&self, index: usize, options: &RunOptions) -> u64 {
        let case = &self.cases[index];
        crate::random_source::test_seed(options.seed, &case.module, &case.name)
    }

    fn interpret(&self, index: usize, options: &RunOptions) -> (RawOutcome, String, bool) {
        let limit = options.time_limit;
        let seed = self.random_seed(index, options);
        let program = self.program.with_entry(self.program.core().tests[index]);
        let trace = options
            .trace
            .as_ref()
            .map(|trace| (trace.level.mode(), self.trace_path(index, trace)));
        let (sender, receiver) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name(format!("chimi-test-{index}"))
            .stack_size(256 * 1024 * 1024)
            .spawn(move || {
                let config = ExecutionConfig::new(Self::execution_id(index))
                    .expect("a generated execution id is valid")
                    .with_random_seed(seed)
                    .with_trace_mode(trace.as_ref().map_or(TraceMode::Off, |(mode, _)| *mode));
                let report = program.execute_oracle(config);
                let trace_error = trace.and_then(|(_, path)| {
                    std::fs::write(&path, report.render_telemetry_json_lines())
                        .err()
                        .map(|error| format!("could not write {}: {error}\n", path.display()))
                });
                let outcome = match report.outcome {
                    ExecutionOutcome::Success(_) => RawOutcome::Success,
                    ExecutionOutcome::ExpectedFailure(_) => RawOutcome::ExpectedFailure {
                        rendering: report.failure_rendering.unwrap_or_default(),
                    },
                    ExecutionOutcome::Defect(defect) => RawOutcome::Defect {
                        message: defect.message,
                        node: defect.core_node_id,
                    },
                    ExecutionOutcome::Cancellation(_) => RawOutcome::Cancelled,
                };
                let output = report.application_stdout
                    + &report.application_stderr
                    + &trace_error.unwrap_or_default();
                let _ = sender.send((outcome, output, report.random_read));
            });
        if spawned.is_err() {
            return (
                RawOutcome::Defect {
                    message: "the test runner could not start a thread".to_owned(),
                    node: None,
                },
                String::new(),
                false,
            );
        }
        // A test past its limit keeps its thread; the run ends without it.
        match receiver.recv_timeout(limit) {
            Ok(result) => result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                (RawOutcome::TimeLimit, String::new(), false)
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => (
                RawOutcome::Defect {
                    message: "the interpreter stopped without an outcome (internal error)"
                        .to_owned(),
                    node: None,
                },
                String::new(),
                false,
            ),
        }
    }

    fn result(
        &self,
        index: usize,
        options: &RunOptions,
        outcome: RawOutcome,
        output: String,
        random_read: bool,
        started: Instant,
    ) -> TestResult {
        TestResult {
            failure: self.classify(outcome),
            output,
            wall_ns: u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
            random_read,
            trace: options
                .trace
                .as_ref()
                .map(|trace| self.trace_path(index, trace))
                .filter(|path| path.is_file()),
        }
    }

    /// Test `index`'s telemetry file: `<index>-<identity>.jsonl`, the identity
    /// reduced to lowercase letters, digits, and `-` so the name is portable.
    fn trace_path(&self, index: usize, trace: &TraceOutput) -> std::path::PathBuf {
        let case = &self.cases[index];
        let mut name = format!("{index}-");
        let mut dash = true;
        for character in format!("{} {}", case.module, case.name).chars() {
            if character.is_ascii_alphanumeric() {
                name.push(character.to_ascii_lowercase());
                dash = false;
            } else if !dash {
                name.push('-');
                dash = true;
            }
            if name.len() >= 96 {
                break;
            }
        }
        let name = name.trim_end_matches('-');
        trace.directory.join(format!("{name}.jsonl"))
    }

    fn execution_id(index: usize) -> String {
        format!("test-{index}")
    }

    fn classify(&self, outcome: RawOutcome) -> Option<TestFailure> {
        let failure = |kind, message: String| TestFailure {
            kind,
            message,
            location: None,
            comparison: None,
        };
        Some(match outcome {
            RawOutcome::Success => return None,
            RawOutcome::ExpectedFailure { rendering } => failure(FailureKind::Failure, rendering),
            RawOutcome::Cancelled => {
                failure(FailureKind::Cancelled, "the test was cancelled".into())
            }
            RawOutcome::TimeLimit => failure(FailureKind::TimeLimit, String::new()),
            RawOutcome::Defect { message, node } => {
                let location = node.and_then(|node| {
                    let origin = self.program.core().nodes.get(node as usize)?.origin;
                    self.locations.get(origin.raw() as usize).cloned()
                });
                let assertion = node.is_some_and(|node| self.assertion_calls.contains(&node));
                let kind = if assertion {
                    FailureKind::Assertion
                } else if message.starts_with("virtual-time deadlock") {
                    FailureKind::Deadlock
                } else {
                    FailureKind::Defect
                };
                let comparison = assertion.then(|| comparison(&message)).flatten();
                TestFailure {
                    kind,
                    message: match &comparison {
                        Some(_) => message.lines().next().unwrap_or_default().to_owned(),
                        None => message,
                    },
                    location,
                    comparison,
                }
            }
        })
    }
}

#[cfg(feature = "native-backend")]
impl TestSuite {
    /// Builds the suite's test executable at `executable`.
    pub fn build_native(
        &self,
        standard: &crate::project::StandardTree,
        executable: &std::path::Path,
        artifact_directory: &std::path::Path,
    ) -> Result<(), String> {
        crate::backend::native::build_application(
            &self.program,
            standard,
            executable,
            artifact_directory,
            Vec::new(),
        )
        .map(|_| ())
    }

    /// Runs `selected` as processes of the executable `build_native` wrote, `jobs`
    /// at a time. Results are in `selected` order.
    pub fn run_native(
        &self,
        executable: &std::path::Path,
        scratch: &std::path::Path,
        selected: &[usize],
        options: &RunOptions,
    ) -> Vec<TestResult> {
        run_parallel(selected, options.jobs, |index| {
            let started = Instant::now();
            let (outcome, output, random_read) =
                self.run_process(executable, scratch, index, options);
            self.result(index, options, outcome, output, random_read, started)
        })
    }

    fn run_process(
        &self,
        executable: &std::path::Path,
        scratch: &std::path::Path,
        index: usize,
        options: &RunOptions,
    ) -> (RawOutcome, String, bool) {
        use std::process::{Command, Stdio};
        let outcome_path =
            scratch.join(format!("test-{}-{index}.outcome.json", std::process::id()));
        let _ = std::fs::remove_file(&outcome_path);
        let mut command = Command::new(executable);
        command.arg("--chimi-runner").arg(if options.virtual_clock {
            "--clock=virtual"
        } else {
            "--clock=real"
        });
        if let Some(start) = &options.clock_start {
            command.arg(format!("--clock-start={start}"));
        }
        command
            .arg(format!("--random-seed={}", self.random_seed(index, options)))
            .arg(format!("--outcome-file={}", outcome_path.display()))
            .arg(format!("--execution-id={}", Self::execution_id(index)))
            .arg(format!("--test={index}"));
        if let Some(trace) = &options.trace {
            command
                .arg(match trace.level {
                    TraceLevel::Boundaries => "--trace=boundaries",
                    TraceLevel::Full => "--trace=full",
                })
                .arg(format!(
                    "--telemetry-file={}",
                    self.trace_path(index, trace).display()
                ));
        }
        command
            .arg("--")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let runner_defect = |message: String| {
            (
                RawOutcome::Defect {
                    message,
                    node: None,
                },
                String::new(),
                false,
            )
        };
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => return runner_defect(format!("could not start the test: {error}")),
        };
        let stdout = child.stdout.take().map(read_in_background);
        let stderr = child.stderr.take().map(read_in_background);
        let deadline = Instant::now() + options.time_limit;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(2)),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return runner_defect(format!("could not wait for the test: {error}"));
                }
            }
        };
        // A process the test started may still hold the pipes open, so the
        // readers get a short grace period rather than an unbounded join.
        let grace = Instant::now() + Duration::from_secs(1);
        let collect = |reader: Option<std::sync::mpsc::Receiver<String>>| {
            reader
                .and_then(|reader| {
                    reader
                        .recv_timeout(grace.saturating_duration_since(Instant::now()))
                        .ok()
                })
                .unwrap_or_default()
        };
        let output = collect(stdout);
        let errors = collect(stderr);
        let Some(status) = status else {
            return (RawOutcome::TimeLimit, output + &errors, false);
        };
        match std::fs::read_to_string(&outcome_path)
            .ok()
            .and_then(|text| native_outcome(&text))
        {
            Some((outcome, random_read)) => (outcome, output + &errors, random_read),
            None => {
                let (outcome, ..) = runner_defect(format!(
                    "the test process ended without an outcome ({status}){}{}",
                    if errors.is_empty() { "" } else { ": " },
                    errors.trim_end()
                ));
                (outcome, output, false)
            }
        }
    }
}

#[cfg(feature = "native-backend")]
fn read_in_background(
    mut stream: impl std::io::Read + Send + 'static,
) -> std::sync::mpsc::Receiver<String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stream.read_to_end(&mut bytes);
        let _ = sender.send(String::from_utf8_lossy(&bytes).into_owned());
    });
    receiver
}

/// Reads a `chimi.runtime.v1` outcome record that a test process wrote, and
/// whether the test drew from its `random` provider.
#[cfg(feature = "native-backend")]
fn native_outcome(text: &str) -> Option<(RawOutcome, bool)> {
    use crate::json::{JsonData, decode_json};
    let JsonData::Object(fields) = decode_json(text.trim_end()).ok()? else {
        return None;
    };
    let field = |name: &str| {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    };
    let text_field = |name: &str| match field(name) {
        Some(JsonData::Text(text)) => Some(text.clone()),
        _ => None,
    };
    let random_read = matches!(field("random_read"), Some(JsonData::Boolean(true)));
    let outcome = match text_field("outcome")?.as_str() {
        "success" => Some(RawOutcome::Success),
        "expected_failure" => Some(RawOutcome::ExpectedFailure {
            rendering: text_field("rendering").unwrap_or_default(),
        }),
        "defect" => Some(RawOutcome::Defect {
            message: text_field("message").unwrap_or_default(),
            node: match field("core_node_id") {
                Some(JsonData::Number(number)) => number.parse().ok(),
                _ => None,
            },
        }),
        "cancellation" => Some(RawOutcome::Cancelled),
        _ => None,
    }?;
    Some((outcome, random_read))
}

/// The two renderings of a comparison assertion's defect message, which
/// `comparison_message` (and `runtime.c`) write as
/// `<heading>\nactual:\n<actual>\nexpected:\n<expected>`.
fn comparison(message: &str) -> Option<(String, String)> {
    let rest = [ASSERT_EQUAL_HEADING, ASSERT_NOT_EQUAL_HEADING]
        .iter()
        .find_map(|heading| message.strip_prefix(heading))?
        .strip_prefix("\nactual:\n")?;
    // A rendering never has a line `expected:` of its own: field names are
    // followed by a value, and text is quoted.
    let (actual, expected) = rest.split_once("\nexpected:\n")?;
    Some((actual.to_owned(), expected.to_owned()))
}

/// Runs `run` over `selected` on up to `jobs` threads; results keep `selected` order.
fn run_parallel(
    selected: &[usize],
    jobs: usize,
    run: impl Fn(usize) -> TestResult + Sync,
) -> Vec<TestResult> {
    let next = AtomicUsize::new(0);
    let mut results = vec![None; selected.len()];
    let (sender, receiver) = mpsc::channel();
    std::thread::scope(|scope| {
        for _ in 0..jobs.clamp(1, selected.len().max(1)) {
            let sender = sender.clone();
            let (next, run) = (&next, &run);
            scope.spawn(move || {
                loop {
                    let position = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&index) = selected.get(position) else {
                        break;
                    };
                    let _ = sender.send((position, run(index)));
                }
            });
        }
        drop(sender);
        for (position, result) in receiver {
            results[position] = Some(result);
        }
    });
    results
        .into_iter()
        .map(|result| result.expect("every selected test reports a result"))
        .collect()
}

/// The human report: one line per test, failure details under a failed one,
/// then the totals (testing.md, Failure reports).
pub fn render_report(
    suite: &TestSuite,
    selected: &[usize],
    results: &[TestResult],
    show_output: bool,
    seed: u64,
) -> String {
    let mut output = String::new();
    for (index, result) in selected.iter().zip(results) {
        let case = &suite.cases[*index];
        let verdict = if result.failure.is_some() {
            "FAILED"
        } else {
            "ok"
        };
        writeln!(output, "test {} ... {verdict}", case.identity()).unwrap();
        if let Some(failure) = &result.failure {
            render_failure(&mut output, failure);
            if result.random_read {
                writeln!(output, "  random seed: {seed}; replay with --seed={seed}").unwrap();
            }
        }
        if !result.output.is_empty() && (show_output || result.failure.is_some()) {
            output.push_str("  output:\n");
            indent(&mut output, &result.output, 4);
        }
        if let Some(trace) = result.trace.as_ref().filter(|_| result.failure.is_some()) {
            writeln!(output, "  trace: {}", trace.display()).unwrap();
        }
    }
    let failed = results
        .iter()
        .filter(|result| result.failure.is_some())
        .count();
    writeln!(
        output,
        "\n{} passed, {failed} failed",
        results.len() - failed
    )
    .unwrap();
    output
}

fn render_failure(output: &mut String, failure: &TestFailure) {
    let at = failure
        .location
        .as_ref()
        .map_or_else(String::new, |location| format!(" at {location}"));
    match failure.kind {
        FailureKind::Assertion => {
            writeln!(output, "  assertion failed{at}").unwrap();
            if failure.comparison.is_none() || !failure.message.starts_with("Test.assert_equal") {
                indent(output, &failure.message, 2);
            }
        }
        FailureKind::Failure => {
            output.push_str("  an expected failure escaped the test:\n");
            indent(output, &failure.message, 4);
        }
        FailureKind::Defect => {
            writeln!(output, "  defect{at}:").unwrap();
            indent(output, &failure.message, 4);
        }
        FailureKind::Deadlock => {
            writeln!(output, "  deadlock{at}:").unwrap();
            indent(output, &failure.message, 4);
        }
        FailureKind::Cancelled => output.push_str("  cancelled\n"),
        FailureKind::TimeLimit => output.push_str("  exceeded the time limit\n"),
    }
    if let Some((actual, expected)) = &failure.comparison {
        output.push_str("  actual:\n");
        indent(output, actual, 4);
        output.push_str("  expected:\n");
        indent(output, expected, 4);
        if actual == expected {
            if failure.message.starts_with("Test.assert_equal") {
                output.push_str("  the values differ but render the same\n");
            }
        } else if actual.contains('\n') || expected.contains('\n') {
            output.push_str("  diff:\n");
            indent(output, &line_diff(actual, expected), 4);
        }
    }
}

fn indent(output: &mut String, text: &str, width: usize) {
    for line in text.lines() {
        writeln!(output, "{:width$}{line}", "").unwrap();
    }
}

/// A line diff of `actual` against `expected`, as testing.md's example shows:
/// `-` for a line only `actual` has, `+` for one only `expected` has, two
/// spaces for a shared line. It aligns the longest common subsequence of
/// lines, so equal lines stay unmarked.
fn line_diff(actual: &str, expected: &str) -> String {
    let left = actual.lines().collect::<Vec<_>>();
    let right = expected.lines().collect::<Vec<_>>();
    let mut lengths = vec![vec![0_usize; right.len() + 1]; left.len() + 1];
    for i in (0..left.len()).rev() {
        for j in (0..right.len()).rev() {
            lengths[i][j] = if left[i] == right[j] {
                lengths[i + 1][j + 1] + 1
            } else {
                lengths[i + 1][j].max(lengths[i][j + 1])
            };
        }
    }
    let mut output = String::new();
    let (mut i, mut j) = (0, 0);
    while i < left.len() || j < right.len() {
        if i < left.len() && j < right.len() && left[i] == right[j] {
            writeln!(output, "  {}", left[i]).unwrap();
            i += 1;
            j += 1;
        } else if i < left.len() && (j == right.len() || lengths[i + 1][j] >= lengths[i][j + 1]) {
            writeln!(output, "- {}", left[i]).unwrap();
            i += 1;
        } else {
            writeln!(output, "+ {}", right[j]).unwrap();
            j += 1;
        }
    }
    output
}

/// The `--json` output: one `chimi.test.v1` record per test in report order,
/// then the `run_finished` record (testing.md, Machine-readable output).
pub fn render_json(
    suite: &TestSuite,
    selected: &[usize],
    results: &[TestResult],
    seed: u64,
) -> String {
    let mut output = String::new();
    for (index, result) in selected.iter().zip(results) {
        let case = &suite.cases[*index];
        output.push_str("{\"schema\":\"chimi.test.v1\",\"event\":\"test_finished\",\"module\":");
        write_json_string(&mut output, &case.module);
        output.push_str(",\"name\":");
        write_json_string(&mut output, &case.name);
        write!(output, ",\"line\":{}", case.line).unwrap();
        match &result.failure {
            None => output.push_str(",\"outcome\":\"passed\""),
            Some(failure) => {
                output.push_str(",\"outcome\":\"failed\",\"kind\":");
                write_json_string(&mut output, failure.kind.name());
                output.push_str(",\"message\":");
                write_json_string(&mut output, &failure.message);
                if let Some(location) = failure
                    .location
                    .as_ref()
                    .filter(|_| failure.kind == FailureKind::Assertion)
                {
                    output.push_str(",\"location\":{\"path\":");
                    write_json_string(&mut output, &location.path);
                    write!(
                        output,
                        ",\"line\":{},\"column\":{}}}",
                        location.line, location.column
                    )
                    .unwrap();
                }
                if let Some((actual, expected)) = &failure.comparison {
                    output.push_str(",\"actual\":");
                    write_json_string(&mut output, actual);
                    output.push_str(",\"expected\":");
                    write_json_string(&mut output, expected);
                }
            }
        }
        output.push_str(",\"output\":[");
        for (position, line) in result.output.lines().enumerate() {
            if position > 0 {
                output.push(',');
            }
            write_json_string(&mut output, line);
        }
        output.push(']');
        if let Some(trace) = &result.trace {
            output.push_str(",\"trace\":");
            write_json_string(&mut output, &trace.display().to_string());
        }
        writeln!(output, ",\"seed\":{seed},\"wall_ns\":{}}}", result.wall_ns).unwrap();
    }
    let failed = results
        .iter()
        .filter(|result| result.failure.is_some())
        .count();
    writeln!(
        output,
        "{{\"schema\":\"chimi.test.v1\",\"event\":\"run_finished\",\"passed\":{},\"failed\":{failed},\"seed\":{seed}}}",
        results.len() - failed
    )
    .unwrap();
    output
}

#[cfg(test)]
mod tests {
    use super::{clock_start_nanoseconds, comparison, line_diff};

    #[test]
    fn clock_start_accepts_what_the_native_runtime_accepts() {
        assert_eq!(clock_start_nanoseconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            clock_start_nanoseconds("2026-11-10T09:30:00.5+01:00"),
            Some(1_794_299_400_500_000_000)
        );
        assert_eq!(
            clock_start_nanoseconds("2024-02-29t00:00:00z"),
            Some(1_709_164_800_000_000_000)
        );
        for rejected in [
            "",
            "2026-11-10",
            "2026-11-10T09:30:00",
            "2026-11-10T09:30:00.Z",
            "2026-11-10T09:30:00.1234567890Z",
            "2026-11-10T24:00:00Z",
            "2026-11-10T09:30:00+24:00",
            "2025-02-29T00:00:00Z",
            "1677-12-31T23:59:59Z",
            "2262-04-12T00:00:00Z",
            "2026-11-10T09:30:00Zjunk",
        ] {
            assert_eq!(clock_start_nanoseconds(rejected), None, "{rejected}");
        }
    }

    #[test]
    fn line_diff_marks_only_the_lines_that_differ() {
        assert_eq!(
            line_diff("[\n  2500,\n  2500,\n]", "[\n  2500,\n]"),
            "  [\n    2500,\n-   2500,\n  ]\n"
        );
        assert_eq!(line_diff("a\nb", "a\nc"), "  a\n- b\n+ c\n");
    }

    #[test]
    fn comparison_messages_split_into_their_renderings() {
        assert_eq!(
            comparison("Test.assert_equal: the values differ\nactual:\n[\n  1,\n]\nexpected:\n[]"),
            Some(("[\n  1,\n]".to_owned(), "[]".to_owned()))
        );
        assert_eq!(comparison("Test.assert_equal(x)"), None);
    }
}
