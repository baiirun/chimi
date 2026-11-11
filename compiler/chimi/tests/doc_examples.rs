//! Every ```chimi block in `README.md` and `docs/**/*.md` is compiled by the
//! real compiler, so the documentation cannot drift from what the compiler
//! accepts.
//!
//! A block's expected status comes from an HTML comment on the line before its
//! fence (blank lines between are allowed):
//!
//! - `<!-- chimi: check -->`: must type-check with no diagnostics. A module
//!   without `pub fn main()` is checked as a library; one with it is also
//!   sealed as a program.
//! - `<!-- chimi: fails E1234 T5678 -->`: must fail with exactly this set of
//!   diagnostic codes.
//! - `<!-- chimi: run VALUE -->`: must run to success and produce `VALUE`,
//!   written as the runner's JSON outcome value (`25`, `"text"`, `true`).
//! - `<!-- chimi: pending TICKET... -->`: needs features that are not
//!   implemented yet, named by one or more `prog` tickets. It is compiled
//!   anyway, and the suite fails once it compiles cleanly, so the annotation
//!   flips to `check` when the features land.
//! - `<!-- chimi: fragment -->`: not a complete module (an excerpt, a sketch of
//!   a shape, or pseudo-code). Skipped.
//!
//! An unannotated block takes the default from `doc_examples.toml`, or `check`
//! when the manifest names no default for its document. The manifest also
//! accepts `unreviewed`: the block is compiled and its outcome reported, but it
//! never fails the suite. It exists for documents this harness may not annotate
//! yet; annotating such a block in the document overrides it.

use chimi::execution::{ExecutionConfig, ExecutionOutcome, TraceMode};
use chimi::source::SourceSnapshot;
use chimi::{Compiler, ProjectSource};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const MISSING_ENTRY_CODE: &str = "E5001";
const PANIC_CODE: &str = "compiler-panic";

#[derive(Clone, Debug, Eq, PartialEq)]
enum Expectation {
    Check,
    Fails(Vec<String>),
    Run(String),
    Pending(Vec<String>),
    Fragment,
    Unreviewed,
}

#[derive(Debug, Eq, PartialEq)]
struct Block {
    line: usize,
    annotation: Option<Result<Expectation, String>>,
    source: String,
}

fn parse_expectation(text: &str) -> Result<Expectation, String> {
    let mut words = text.split_whitespace();
    let kind = words.next().unwrap_or_default();
    let rest = words.collect::<Vec<_>>();
    match (kind, rest.as_slice()) {
        ("check", []) => Ok(Expectation::Check),
        ("fragment", []) => Ok(Expectation::Fragment),
        ("unreviewed", []) => Ok(Expectation::Unreviewed),
        ("pending", tickets) if !tickets.is_empty() => Ok(Expectation::Pending(
            tickets.iter().map(|ticket| (*ticket).to_owned()).collect(),
        )),
        ("fails", codes) if !codes.is_empty() => Ok(Expectation::Fails(code_set(
            codes.iter().map(|code| (*code).to_owned()),
        ))),
        ("run", value) if !value.is_empty() => Ok(Expectation::Run(value.join(" "))),
        _ => Err(format!("unrecognized chimi annotation `{text}`")),
    }
}

fn code_set(codes: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut codes = codes.into_iter().collect::<Vec<_>>();
    codes.sort();
    codes.dedup();
    codes
}

fn annotation_text(line: &str) -> Option<&str> {
    line.trim()
        .strip_prefix("<!--")?
        .strip_suffix("-->")?
        .trim()
        .strip_prefix("chimi:")
        .map(str::trim)
}

/// Extracts ```chimi blocks with the annotation that precedes each fence.
/// Fence indentation (inside list items) is stripped from the block body.
fn extract_blocks(markdown: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut lines = markdown.lines().enumerate();
    let mut annotation = None;
    while let Some((index, line)) = lines.next() {
        let trimmed = line.trim_start();
        let Some(info) = trimmed.strip_prefix("```") else {
            if let Some(text) = annotation_text(line) {
                annotation = Some(parse_expectation(text));
            } else if !line.trim().is_empty() {
                annotation = None;
            }
            continue;
        };
        let indent = line.len() - trimmed.len();
        let mut body = String::new();
        for (_, inner) in lines.by_ref() {
            if inner.trim() == "```" {
                break;
            }
            body.push_str(inner.get(indent..).unwrap_or(inner.trim_start()));
            body.push('\n');
        }
        if info.trim() == "chimi" {
            blocks.push(Block {
                line: index + 1,
                annotation: annotation.take(),
                source: body,
            });
        }
        annotation = None;
    }
    blocks
}

/// Default expectations keyed by document path or directory prefix, from the
/// `[defaults]` table of `doc_examples.toml`. The longest matching key wins.
fn parse_manifest(text: &str) -> BTreeMap<String, Expectation> {
    let mut defaults = BTreeMap::new();
    let mut in_defaults = false;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_defaults = line == "[defaults]";
            continue;
        }
        assert!(
            in_defaults,
            "doc_examples.toml: entry outside [defaults]: {line}"
        );
        let (key, value) = line.split_once('=').unwrap_or_else(|| {
            panic!("doc_examples.toml: expected `\"path\" = \"status\"`: {line}")
        });
        let unquote = |part: &str| part.trim().trim_matches('"').to_owned();
        let expectation = parse_expectation(&unquote(value))
            .unwrap_or_else(|error| panic!("doc_examples.toml: {error}"));
        defaults.insert(unquote(key), expectation);
    }
    defaults
}

fn default_for(defaults: &BTreeMap<String, Expectation>, document: &str) -> Expectation {
    defaults
        .iter()
        .filter(|(key, _)| document.starts_with(key.as_str()))
        .max_by_key(|(key, _)| key.len())
        .map_or(Expectation::Check, |(_, expectation)| expectation.clone())
}

enum Compilation {
    Clean,
    Diagnostics {
        codes: Vec<String>,
        rendered: String,
    },
}

fn project(document: &str, source: &str) -> Vec<ProjectSource> {
    vec![
        ProjectSource::new("main", SourceSnapshot::from_text(document, source))
            .expect("`main` is a valid module path"),
    ]
}

/// A compiler panic is reported as the pseudo-code `compiler-panic`, so one
/// crashing example does not hide the rest. It fails the example under every
/// expectation, `pending` and unreviewed included.
fn compile(document: &str, source: &str) -> Compilation {
    std::panic::catch_unwind(|| compile_unguarded(document, source)).unwrap_or_else(|payload| {
        Compilation::Diagnostics {
            codes: vec![PANIC_CODE.to_owned()],
            rendered: panic_message(payload.as_ref()),
        }
    })
}

fn is_panic(codes: &[String]) -> bool {
    codes.iter().any(|code| code == PANIC_CODE)
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .map_or_else(
            || "compiler panicked".to_owned(),
            |message| format!("compiler panicked: {message}"),
        )
}

fn compile_unguarded(document: &str, source: &str) -> Compilation {
    let typed = Compiler::type_project(project(document, source));
    if !typed.is_success() {
        return Compilation::Diagnostics {
            codes: code_set(typed.diagnostics().iter().map(|d| d.code().to_owned())),
            rendered: typed.render_diagnostics(),
        };
    }
    match Compiler::verify_project(project(document, source)) {
        Ok(_) => Compilation::Clean,
        Err(diagnostics) if diagnostics.iter().all(|d| d.code() == MISSING_ENTRY_CODE) => {
            Compilation::Clean
        }
        Err(diagnostics) => Compilation::Diagnostics {
            codes: code_set(diagnostics.iter().map(|d| d.code().to_owned())),
            rendered: diagnostics
                .iter()
                .map(|d| format!("[{}] {}", d.code(), d.message()))
                .collect::<Vec<_>>()
                .join("\n"),
        },
    }
}

fn run(document: &str, source: &str) -> Result<String, String> {
    std::panic::catch_unwind(|| run_unguarded(document, source))
        .unwrap_or_else(|payload| Err(panic_message(payload.as_ref())))
}

fn run_unguarded(document: &str, source: &str) -> Result<String, String> {
    let config = ExecutionConfig::new("doc-example")
        .expect("valid execution id")
        .with_trace_mode(TraceMode::Off);
    let report =
        Compiler::run_project(project(document, source), config).map_err(|diagnostics| {
            diagnostics
                .iter()
                .map(|d| format!("[{}] {}", d.code(), d.message()))
                .collect::<Vec<_>>()
                .join("\n")
        })?;
    let line = report.render_outcome_json_line();
    match &report.outcome {
        ExecutionOutcome::Success(_) => Ok(line
            .trim_end()
            .strip_prefix(r#"{"schema":"chimi.runtime.v1","outcome":"success","value":"#)
            .and_then(|rest| rest.strip_suffix('}'))
            .unwrap_or_else(|| panic!("unexpected success outcome line: {line}"))
            .to_owned()),
        _ => Err(line),
    }
}

/// Returns why the block disagrees with its expectation, if it does.
fn evaluate(document: &str, block: &Block, expectation: &Expectation) -> Option<String> {
    match expectation {
        Expectation::Fragment => None,
        Expectation::Check => match compile(document, &block.source) {
            Compilation::Clean => None,
            Compilation::Diagnostics { rendered, .. } => {
                Some(format!("expected a clean check, got:\n{rendered}"))
            }
        },
        Expectation::Fails(expected) => match compile(document, &block.source) {
            Compilation::Clean => Some(format!("expected {expected:?}, but it compiles cleanly")),
            Compilation::Diagnostics { codes, rendered } if is_panic(&codes) => Some(rendered),
            Compilation::Diagnostics { codes, .. } if &codes == expected => None,
            Compilation::Diagnostics { codes, rendered } => {
                Some(format!("expected {expected:?}, got {codes:?}:\n{rendered}"))
            }
        },
        Expectation::Run(expected) => match run(document, &block.source) {
            Ok(value) if &value == expected => None,
            Ok(value) => Some(format!("expected run value {expected}, got {value}")),
            Err(failure) => Some(format!("expected run value {expected}, got:\n{failure}")),
        },
        Expectation::Pending(tickets) => match compile(document, &block.source) {
            Compilation::Clean => Some(format!(
                "pending on {}, but it now compiles cleanly; annotate it `check`",
                tickets.join(" ")
            )),
            Compilation::Diagnostics { codes, rendered } if is_panic(&codes) => Some(rendered),
            Compilation::Diagnostics { .. } => None,
        },
        Expectation::Unreviewed => None,
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn markdown_documents(root: &Path) -> Vec<PathBuf> {
    fn walk(directory: &Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "md") {
                found.push(path);
            }
        }
    }
    let mut found = vec![root.join("README.md")];
    walk(&root.join("docs"), &mut found);
    found.sort();
    found
}

#[derive(Default)]
struct Tally {
    checked: usize,
    failing_as_expected: usize,
    ran: usize,
    fragments: usize,
    pending: BTreeMap<String, usize>,
    unreviewed_clean: Vec<String>,
    unreviewed_failing: Vec<String>,
}

#[test]
fn documentation_examples_match_their_annotations() {
    let root = repository_root();
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/doc_examples.toml");
    let defaults = parse_manifest(&std::fs::read_to_string(manifest_path).unwrap());
    let mut per_document = BTreeMap::<String, Tally>::new();
    let mut failures = Vec::new();

    for path in markdown_documents(&root) {
        let document = path
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let blocks = extract_blocks(&std::fs::read_to_string(&path).unwrap());
        if blocks.is_empty() {
            continue;
        }
        let tally = per_document.entry(document.clone()).or_default();
        for block in &blocks {
            let location = format!("{document}:{}", block.line);
            let expectation = match &block.annotation {
                Some(Ok(expectation)) => expectation.clone(),
                Some(Err(error)) => {
                    failures.push(format!("{location}: {error}"));
                    continue;
                }
                None => default_for(&defaults, &document),
            };
            if let Some(failure) = evaluate(&document, block, &expectation) {
                failures.push(format!("{location}: {failure}"));
            }
            match expectation {
                Expectation::Check => tally.checked += 1,
                Expectation::Fails(_) => tally.failing_as_expected += 1,
                Expectation::Run(_) => tally.ran += 1,
                Expectation::Fragment => tally.fragments += 1,
                Expectation::Pending(tickets) => {
                    *tally.pending.entry(tickets.join(" ")).or_default() += 1;
                }
                Expectation::Unreviewed => match compile(&document, &block.source) {
                    Compilation::Clean => tally.unreviewed_clean.push(location),
                    // Not reviewing an example never excuses a compiler crash.
                    Compilation::Diagnostics { codes, rendered } if is_panic(&codes) => {
                        failures.push(format!("{location}: {rendered}"));
                    }
                    Compilation::Diagnostics { codes, .. } => tally
                        .unreviewed_failing
                        .push(format!("{location} {}", codes.join(" "))),
                },
            }
        }
    }

    let summary = render_summary(&per_document);
    println!("{summary}");
    assert!(
        failures.is_empty(),
        "{} documentation example(s) disagree with their annotations:\n\n{}\n\n{summary}",
        failures.len(),
        failures.join("\n\n"),
    );
}

fn render_summary(per_document: &BTreeMap<String, Tally>) -> String {
    let mut output = String::from("documentation examples:\n");
    let mut pending_total = BTreeMap::<&str, usize>::new();
    let mut unreviewed = Vec::new();
    for (document, tally) in per_document {
        let pending = tally.pending.values().sum::<usize>();
        writeln!(
            output,
            "  {document}: check {}, fails {}, run {}, pending {pending}, fragment {}, unreviewed {}",
            tally.checked,
            tally.failing_as_expected,
            tally.ran,
            tally.fragments,
            tally.unreviewed_clean.len() + tally.unreviewed_failing.len(),
        )
        .unwrap();
        for (ticket, count) in &tally.pending {
            *pending_total.entry(ticket).or_default() += count;
        }
        unreviewed.extend(
            tally
                .unreviewed_clean
                .iter()
                .map(|location| format!("{location} compiles")),
        );
        unreviewed.extend(tally.unreviewed_failing.iter().cloned());
    }
    if !pending_total.is_empty() {
        output.push_str("pending by ticket:\n");
        for (ticket, count) in pending_total {
            writeln!(output, "  {ticket}: {count}").unwrap();
        }
    }
    if !unreviewed.is_empty() {
        output.push_str("unreviewed (not enforced):\n");
        for line in unreviewed {
            writeln!(output, "  {line}").unwrap();
        }
    }
    output
}

#[test]
fn extraction_pairs_each_fence_with_the_annotation_directly_above_it() {
    let markdown = concat!(
        "<!-- chimi: fails T4203 E1000 T4203 -->\n",
        "\n",
        "```chimi\n",
        "fn a() {}\n",
        "```\n",
        "<!-- chimi: check -->\n",
        "Intervening prose detaches the annotation.\n",
        "```chimi\n",
        "fn b() {}\n",
        "```\n",
        "- item\n",
        "  <!-- chimi: pending ts-123 ts-456 -->\n",
        "  ```chimi\n",
        "  fn c() {\n",
        "    1\n",
        "  }\n",
        "  ```\n",
        "<!-- chimi: check -->\n",
        "```text\n",
        "```chimi\n",
        "```\n",
        "<!-- chimi: maybe -->\n",
        "```chimi\n",
        "```\n",
    );
    let blocks = extract_blocks(markdown);
    let annotations = blocks
        .iter()
        .map(|block| block.annotation.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        annotations,
        vec![
            Some(Ok(Expectation::Fails(vec!["E1000".into(), "T4203".into()]))),
            None,
            Some(Ok(Expectation::Pending(vec![
                "ts-123".into(),
                "ts-456".into()
            ]))),
            Some(Err("unrecognized chimi annotation `maybe`".into())),
        ],
    );
    assert_eq!(blocks[2].source, "fn c() {\n  1\n}\n");
    assert_eq!(
        blocks.iter().map(|block| block.line).collect::<Vec<_>>(),
        vec![3, 8, 13, 23]
    );
}

#[test]
fn manifest_defaults_prefer_the_most_specific_path() {
    let defaults = parse_manifest(concat!(
        "# comment\n",
        "[defaults]\n",
        "\"docs/plans/\" = \"fragment\"\n",
        "\"docs/plans/a.md\" = \"unreviewed\"\n",
    ));
    assert_eq!(
        default_for(&defaults, "docs/plans/a.md"),
        Expectation::Unreviewed
    );
    assert_eq!(
        default_for(&defaults, "docs/plans/b.md"),
        Expectation::Fragment
    );
    assert_eq!(default_for(&defaults, "docs/spec.md"), Expectation::Check);
}

#[test]
fn pending_blocks_fail_once_they_compile() {
    let block = Block {
        line: 1,
        annotation: None,
        source: "pub fn main() -> Int { 25 }\n".into(),
    };
    let pending = Expectation::Pending(vec!["ts-000000".into()]);
    assert!(evaluate("example.md", &block, &pending).is_some());
    assert_eq!(
        evaluate("example.md", &block, &Expectation::Run("25".into())),
        None
    );
    assert_eq!(evaluate("example.md", &block, &Expectation::Check), None);
}
