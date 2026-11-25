//! Generic interface members (G11): `fn<A>(..)` as an interface member's type.
//!
//! A member such as `wrap: fn<A>(A) -> List<A>` is used at any instantiation,
//! so its provider must be as generic. Running programs go through both
//! execution paths; rejected programs assert their diagnostic codes.

#[path = "support/interpreter.rs"]
mod interpreter;

use chimi::source::SourceSnapshot;
use chimi::{Compiler, CompilerSession, ProjectSource};
use std::path::PathBuf;
use std::process::Command;

fn scratch_directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "chimi-generic-members-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    if path.exists() {
        std::fs::remove_dir_all(&path).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn last_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .last()
        .unwrap_or_default()
        .to_owned()
}

/// Runs `source` on the interpreter and as a native executable, asserts the
/// two outcomes agree and the formatter keeps the source stable, and returns
/// the outcome without its schema prefix.
fn run_both(name: &str, source: &str) -> String {
    let scratch = scratch_directory(name);
    let path = scratch.join("main.chimi");
    std::fs::write(&path, source).unwrap();
    let interpreted = interpreter::traced(
        Command::new(env!("CARGO_BIN_EXE_chimi"))
            .arg("run")
            .arg(&path),
    )
    .expect("run the reference interpreter");
    let interpreted_outcome = last_line(&interpreted.stdout);
    let executable = scratch.join("program");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .arg("build")
        .arg(&path)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("build the native executable");
    assert!(
        build.status.success(),
        "native build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let outcome_path = scratch.join("outcome.jsonl");
    Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!(
            "--telemetry-file={}",
            scratch.join("telemetry.jsonl").display()
        ))
        .arg(format!("--outcome-file={}", outcome_path.display()))
        .arg("--execution-id=generic-members")
        .arg("--")
        .output()
        .expect("execute the native program");
    let native_outcome = std::fs::read_to_string(&outcome_path)
        .expect("native outcome file")
        .trim_end()
        .to_owned();
    assert_eq!(
        interpreted_outcome, native_outcome,
        "execution paths disagree for {name}"
    );
    assert_formatting_is_stable(source);
    assert_inspection_survives_every_offset(source);
    std::fs::remove_dir_all(scratch).unwrap();
    interpreted_outcome
        .strip_prefix("{\"schema\":\"chimi.runtime.v1\",")
        .and_then(|rest| rest.strip_suffix('}'))
        .unwrap_or_else(|| panic!("unexpected outcome line: {interpreted_outcome}"))
        .to_owned()
}

fn assert_formatting_is_stable(source: &str) {
    let first = Compiler::format(SourceSnapshot::from_text("main.chimi", source));
    let second = Compiler::format(SourceSnapshot::from_text("main.chimi", first.text()));
    assert_eq!(first.text(), second.text(), "formatting is not stable");
    assert_eq!(
        check_codes(first.text()),
        check_codes(source),
        "formatting changed the diagnostics"
    );
}

/// Inspects every character boundary, as an editor would, so member generics
/// have facts wherever a cursor can land.
fn assert_inspection_survives_every_offset(source: &str) {
    let module = || {
        ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
            .expect("valid module path")
    };
    let report = CompilerSession::new().analyze_project(vec![module()]);
    for offset in (0..=source.len()).filter(|&offset| source.is_char_boundary(offset)) {
        let _ = report.inspect("main", offset);
    }
    let _ = Compiler::inspect_project(vec![module()], "main", source.len());
}

fn check_codes(source: &str) -> Vec<String> {
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    Compiler::type_project(vec![module])
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code().to_owned())
        .collect()
}

fn check_messages(source: &str) -> Vec<String> {
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    Compiler::type_project(vec![module])
        .diagnostics()
        .iter()
        .map(|diagnostic| format!("{} {}", diagnostic.code(), diagnostic.message()))
        .collect()
}

const BOXER: &str = concat!(
    "import std/list as List\n",
    "interface Boxer { wrap: fn<A>(A) -> List<A> }\n",
    "fn sizes() -> Int needs { boxer: Boxer } {\n",
    "  List.length(boxer.wrap(1)) * 10 + List.length(boxer.wrap(\"x\"))\n",
    "}\n",
);

#[test]
fn a_generic_member_is_called_at_two_types_in_one_body() {
    let outcome = run_both(
        "two-instantiations",
        &format!(
            "{BOXER}fn pair<T>(value: T) -> List<T> {{ [value, value] }}\n\
             pub fn main() -> Int {{ sizes() using {{ boxer: {{ wrap: pair }} }} }}\n"
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":22");
}

#[test]
fn a_provider_fixed_to_one_type_does_not_fit_a_generic_member() {
    let source = format!(
        "{BOXER}fn ints(value: Int) -> List<Int> {{ [value] }}\n\
         pub fn main() -> Int {{ sizes() using {{ boxer: {{ wrap: ints }} }} }}\n"
    );
    assert_eq!(check_codes(&source), ["T4213"]);
}

// An unannotated lambda's parameter type exists before the check, so binding
// it to the member's `A` would let `A` escape; the provider must be generic.
#[test]
fn a_lambda_provider_is_rejected_with_a_hint_to_declare_a_generic_function() {
    let source = format!(
        "{BOXER}pub fn main() -> Int {{ sizes() using {{ boxer: {{ wrap: fn(x) {{ [x] }} }} }} }}\n"
    );
    let messages = check_messages(&source);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].starts_with("T4213") && messages[0].contains("not generic"),
        "{messages:?}"
    );
}

// The member promises nothing about `A`, so a provider that compares two `A`
// values would run `==` on values, such as functions, that have no equality.
#[test]
fn a_provider_that_compares_member_values_does_not_fit() {
    let source = concat!(
        "interface Eq { same: fn<A>(A, A) -> Bool }\n",
        "fn same<T>(left: T, right: T) -> Bool { left == right }\n",
        "fn use_it() -> Bool needs { eq: Eq } { eq.same(1, 1) }\n",
        "pub fn main() -> Bool { use_it() using { eq: { same: same } } }\n",
    );
    let messages = check_messages(source);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].starts_with("T4213") && messages[0].contains("compares or orders"),
        "{messages:?}"
    );
}

#[test]
fn a_provider_whose_generics_line_up_differently_does_not_fit() {
    let source = concat!(
        "interface Mapper { apply: fn<A, B>(A, fn(A) -> B) -> B }\n",
        "fn use_it() -> Int needs { m: Mapper } { m.apply(2, fn(x: Int) { x * 10 }) }\n",
        "fn same_type<A>(value: A, f: fn(A) -> A) -> A { f(value) }\n",
        "pub fn main() -> Int { use_it() using { m: { apply: same_type } } }\n",
    );
    assert_eq!(check_codes(source), ["T4213"]);
}

#[test]
fn a_provider_may_name_its_generics_in_another_order() {
    let outcome = run_both(
        "generic-order",
        concat!(
            "interface Mapper { apply: fn<A, B>(A, fn(A) -> B) -> B }\n",
            "fn use_it() -> Int needs { m: Mapper } { m.apply(2, fn(x: Int) { x * 10 }) }\n",
            "fn backwards<B, A>(value: A, f: fn(A) -> B) -> B { f(value) }\n",
            "pub fn main() -> Int { use_it() using { m: { apply: backwards } } }\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":20");
}

const STORE: &str = concat!(
    "interface Store<K> { get: fn<A>(K, A) -> A }\n",
    "fn keep<K, A>(key: K, fallback: A) -> A { fallback }\n",
);

// The interface's own `K` is fixed by the need's type while the member's `A`
// stays open per call.
#[test]
fn an_interface_argument_is_fixed_while_the_member_generic_varies() {
    let outcome = run_both(
        "interface-and-member-generics",
        &format!(
            "{STORE}fn use_it() -> Int needs {{ store: Store<Text> }} {{\n\
             \x20 case store.get(\"flag\", True) {{\n\
             \x20   True -> store.get(\"count\", 7)\n\
             \x20   False -> 0\n\
             \x20 }}\n\
             }}\n\
             pub fn main() -> Int {{ use_it() using {{ store: {{ get: keep }} }} }}\n"
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":7");
}

#[test]
fn a_call_must_pass_the_interface_argument_type() {
    let source = format!(
        "{STORE}fn use_it() -> Int needs {{ store: Store<Text> }} {{ store.get(42, 1) }}\n\
         pub fn main() -> Int {{ 1 }}\n"
    );
    assert_eq!(check_codes(&source), ["T4203"]);
}

#[test]
fn a_member_value_taken_from_a_need_still_fits_the_member() {
    let outcome = run_both(
        "forwarded-member",
        &format!(
            "{STORE}fn inner() -> Int needs {{ store: Store<Text> }} {{ store.get(\"a\", 5) }}\n\
             fn outer() -> Int needs {{ store: Store<Text> }} {{\n\
             \x20 let get = store.get\n\
             \x20 inner() using {{ store: {{ get: get }} }}\n\
             }}\n\
             pub fn main() -> Int {{ outer() using {{ store: {{ get: keep }} }} }}\n"
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":5");
}

// Replacing a generic member through an update would let `inner` call the
// Int-only function with Text.
#[test]
fn an_update_cannot_replace_a_generic_member_with_a_fixed_one() {
    let source = format!(
        "{STORE}fn inner() -> Text needs {{ store: Store<Text> }} {{ store.get(\"a\", \"text\") }}\n\
         fn ints(key: Text, fallback: Int) -> Int {{ fallback + 1 }}\n\
         fn outer() -> Text needs {{ store: Store<Text> }} {{\n\
         \x20 let broken = {{ ..store, get: ints }}\n\
         \x20 inner() using {{ store: broken }}\n\
         }}\n\
         pub fn main() -> Text {{ outer() using {{ store: {{ get: keep }} }} }}\n"
    );
    assert_eq!(check_codes(&source), ["T4203"]);
}

const LINE_COUNT: &str = concat!(
    "import std/io as IO\n",
    "import std/io.{FileSystem, Path}\n",
    "enum Boom { Bang }\n",
    "fn count_lines() -> Int needs { files: FileSystem } {\n",
    "  files.process_utf8_lines(IO.path(\"input.txt\"), 0, fn(count: Int, line: Text) { count + 1 })\n",
    "}\n",
);

// The ticket's motivating case: a test fake for the standard `FileSystem`,
// whose member is `fn<A, F, R>(Path, A, fn(A, Text) -> A errors F needs R) ->
// A errors F | FileError | Utf8Error needs R`.
#[test]
fn a_user_fake_stands_in_for_the_file_system() {
    let outcome = run_both(
        "file-system-fake",
        &format!(
            "{LINE_COUNT}fn fake_lines<A, F, R>(path: Path, initial: A, step: fn(A, Text) -> A errors F needs R) -> A {{\n\
             \x20 step(step(step(initial, \"one\"), \"two\"), \"three\")\n\
             }}\n\
             pub fn main() -> Int {{ count_lines() using {{ files: {{ process_utf8_lines: fake_lines }} }} }}\n"
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":3");
}

#[test]
fn a_fake_may_not_raise_failures_the_member_does_not_declare() {
    let source = format!(
        "{LINE_COUNT}fn fake_lines<A, F, R>(path: Path, initial: A, step: fn(A, Text) -> A errors F needs R) -> A {{\n\
         \x20 Failure(Boom.Bang)\n\
         }}\n\
         pub fn main() -> Int {{ count_lines() using {{ files: {{ process_utf8_lines: fake_lines }} }} }}\n"
    );
    assert_eq!(check_codes(&source), ["T4213"]);
}

#[test]
fn a_generic_function_type_is_rejected_outside_an_interface_member() {
    for declaration in [
        "fn apply(f: fn<A>(A) -> A) -> Int { 1 }\n",
        "struct Holder { f: fn<A>(A) -> A }\n",
        "interface Nested { run: fn(fn<A>(A) -> A) -> Int }\n",
        "interface Returned { make: fn() -> fn<A>(A) -> A }\n",
    ] {
        let source = format!("{declaration}pub fn main() -> Int {{ 1 }}\n");
        assert_eq!(check_codes(&source), ["T4360"], "{declaration}");
    }
}

#[test]
fn a_member_generic_may_not_reuse_the_interface_generic_name() {
    let source = concat!(
        "interface Store<K> { put: fn<K>(K) -> Int }\n",
        "pub fn main() -> Int { 1 }\n",
    );
    assert_eq!(check_codes(source), ["E_DUPLICATE_GENERIC_PARAMETER"]);
}

#[test]
fn the_formatter_writes_member_generics_in_canonical_form() {
    let formatted = Compiler::format(SourceSnapshot::from_text(
        "main.chimi",
        "interface Boxer {\n  wrap: fn< A ,B >( A ) -> B\n}\n",
    ));
    assert_eq!(
        formatted.text(),
        "interface Boxer {\n  wrap: fn<A, B>(A) -> B\n}\n"
    );
}

const RUNNER: &str = concat!(
    "interface Runner { run: fn<F>(fn() -> Int errors F) -> Int errors F }\n",
    "fn total() -> Int needs { runner: Runner } {\n",
    "  runner.run(fn() { 20 }) + runner.run(fn() { 1 })\n",
    "}\n",
);

// `F` appears only in failure rows, so the check must hold row parameters
// rigid as it does value parameters.
#[test]
fn a_member_generic_used_only_in_failure_rows_is_provided_by_a_row_generic_function() {
    let outcome = run_both(
        "row-only-member-generic",
        &format!(
            "{RUNNER}fn call<E>(operation: fn() -> Int errors E) -> Int {{ operation() }}\n\
             pub fn main() -> Int {{ total() using {{ runner: {{ run: call }} }} }}\n"
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":21");
}

#[test]
fn a_provider_that_cannot_fail_through_its_callback_does_not_fit_a_row_generic_member() {
    let source = format!(
        "{RUNNER}fn call(operation: fn() -> Int) -> Int {{ operation() }}\n\
         pub fn main() -> Int {{ total() using {{ runner: {{ run: call }} }} }}\n"
    );
    assert_eq!(check_codes(&source), ["T4213"]);
}

// Each member's `A` is its own parameter, so a provider for one says nothing
// about the other.
#[test]
fn two_members_may_each_name_a_generic_a() {
    let outcome = run_both(
        "members-share-a-name",
        concat!(
            "import std/list as List\n",
            "interface Shelf { keep: fn<A>(A) -> A, count: fn<A>(List<A>) -> Int }\n",
            "fn use_it() -> Int needs { shelf: Shelf } {\n",
            "  shelf.keep(3) + shelf.count([\"a\", \"b\"]) + shelf.count([True])\n",
            "}\n",
            "fn same<T>(value: T) -> T { value }\n",
            "fn size<T>(items: List<T>) -> Int { List.length(items) }\n",
            "pub fn main() -> Int { use_it() using { shelf: { keep: same, count: size } } }\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":6");
}

// A reference to an unannotated function is instantiated where it is named,
// so it is no more generic than a lambda.
#[test]
fn an_unannotated_function_reference_does_not_fit_a_generic_member() {
    let source = concat!(
        "interface Boxer { wrap: fn<A>(A) -> A }\n",
        "fn same(value) { value }\n",
        "fn use_it() -> Int needs { boxer: Boxer } { boxer.wrap(1) }\n",
        "pub fn main() -> Int { use_it() using { boxer: { wrap: same } } }\n",
    );
    assert_eq!(check_codes(source), ["T4213"]);
}

// A case or list that mixes a generic function with a fixed one may hold the
// fixed one, so the join must have the fixed type, not the generic one.
#[test]
fn joining_a_generic_function_with_a_fixed_one_gives_the_fixed_type() {
    let joins = [
        "  let pick = case False {\n    True -> keep\n    False -> ints\n  }\n",
        "  let pick = case [keep, ints] {\n    [_, second] -> second\n    _ -> ints\n  }\n",
        "  let pick = apply(keep, fn(unused) { ints })\n",
    ];
    for join in joins {
        let source = format!(
            "{STORE}fn ints(key: Text, fallback: Int) -> Int {{ fallback + 1 }}\n\
             fn apply<T>(value: T, f: fn(T) -> T) -> T {{ f(value) }}\n\
             pub fn main() -> Text {{\n{join}  pick(\"k\", \"text\")\n}}\n"
        );
        assert!(check_codes(&source).contains(&"T4203".to_owned()), "{join}");
        let provided = format!(
            "{STORE}fn ints(key: Text, fallback: Int) -> Int {{ fallback + 1 }}\n\
             fn apply<T>(value: T, f: fn(T) -> T) -> T {{ f(value) }}\n\
             fn inner() -> Text needs {{ store: Store<Text> }} {{ store.get(\"a\", \"text\") }}\n\
             pub fn main() -> Text {{\n{join}  inner() using {{ store: {{ get: pick }} }}\n}}\n"
        );
        assert_eq!(check_codes(&provided), ["T4213"], "{join}");
    }
}

// `N` appears only in needs rows; the member's own generics must be in scope
// there as they are in value positions.
#[test]
fn a_member_generic_may_be_a_needs_row() {
    let outcome = run_both(
        "needs-row-member-generic",
        concat!(
            "interface Log { line: fn(Text) -> Int }\n",
            "interface Runner { run: fn<N>(fn() -> Int needs N) -> Int needs N }\n",
            "fn call<N>(operation: fn() -> Int needs N) -> Int { operation() }\n",
            "fn use_it() -> Int needs { runner: Runner, log: Log } {\n",
            "  runner.run(fn() { log.line(\"x\") })\n",
            "}\n",
            "fn four(text: Text) -> Int { 4 }\n",
            "pub fn main() -> Int {\n",
            "  use_it() using { runner: { run: call }, log: { line: four } }\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":4");
}
