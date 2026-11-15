//! Generates the case tables `runtime.c` uses for `Text.lowercase` and
//! `Text.uppercase`, and fails when the checked-in copy is stale.
//!
//! The interpreter calls Rust's `str::to_lowercase` and `str::to_uppercase`; the
//! tables are derived from those same functions, so a toolchain whose Unicode
//! version differs fails here until the tables are regenerated with
//! `CHIMI_UPDATE_UNICODE_TABLES=1 cargo test -p chimi --test unicode_case_tables`.

use std::path::Path;

const BEGIN: &str = "/* BEGIN GENERATED unicode case tables";
const END: &str = "/* END GENERATED unicode case tables */";

/// Where a character stands in Rust's Final_Sigma test, which skips
/// `Case_Ignorable` characters and then asks whether the next one is `Cased`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SigmaContext {
    Cased,
    Ignorable,
}

impl SigmaContext {
    const fn c_name(self) -> &'static str {
        match self {
            Self::Cased => "SIGMA_CONTEXT_CASED",
            Self::Ignorable => "SIGMA_CONTEXT_IGNORABLE",
        }
    }

    /// Probes `str::to_lowercase`, the oracle the interpreter uses, instead of
    /// reading the Unicode property files the toolchain does not expose.
    fn of(character: char) -> Option<Self> {
        let final_sigma = |text: String| text.to_lowercase().ends_with('ς');
        // Only a Cased, non-ignorable character makes a lone `Σ` after it final.
        if final_sigma(format!("{character}Σ")) {
            Some(Self::Cased)
        } else if final_sigma(format!("A{character}Σ")) {
            Some(Self::Ignorable)
        } else {
            None
        }
    }
}

fn characters() -> impl Iterator<Item = char> {
    (0..=0x10ffff).filter_map(char::from_u32)
}

fn mapping_table<I: Iterator<Item = char>>(name: &str, map: impl Fn(char) -> I) -> String {
    let mut entries = Vec::new();
    for character in characters() {
        let mapped: Vec<char> = map(character).collect();
        if mapped != [character] {
            assert!(
                mapped.len() <= 3,
                "{character:?} maps to more than three characters"
            );
            let codes: Vec<String> = mapped
                .iter()
                .map(|c| format!("0x{:x}", u32::from(*c)))
                .collect();
            entries.push(format!(
                "{{0x{:x}, {{{}}}}}",
                u32::from(character),
                codes.join(", ")
            ));
        }
    }
    let mut table = format!("static const CaseMapping {name}[] = {{\n");
    for line in entries.chunks(6) {
        table.push_str(&format!("  {},\n", line.join(", ")));
    }
    table.push_str("};\n");
    table
}

fn sigma_context_table() -> String {
    let mut ranges: Vec<(u32, u32, SigmaContext)> = Vec::new();
    for character in characters() {
        let Some(context) = SigmaContext::of(character) else {
            continue;
        };
        let code = u32::from(character);
        match ranges.last_mut() {
            Some((_, last, previous)) if *last + 1 == code && *previous == context => *last = code,
            _ => ranges.push((code, code, context)),
        }
    }
    let entries: Vec<String> = ranges
        .iter()
        .map(|(first, last, context)| format!("{{0x{first:x}, 0x{last:x}, {}}}", context.c_name()))
        .collect();
    let mut table = String::from("static const SigmaContextRange sigma_context_ranges[] = {\n");
    for line in entries.chunks(3) {
        table.push_str(&format!("  {},\n", line.join(", ")));
    }
    table.push_str("};\n");
    table
}

fn generated_region() -> String {
    let (major, minor, update) = char::UNICODE_VERSION;
    format!(
        "{BEGIN} (tests/unicode_case_tables.rs, Unicode {major}.{minor}.{update}) */\n\
         {lowercase}{uppercase}{sigma}{END}",
        lowercase = mapping_table("lowercase_mappings", char::to_lowercase),
        uppercase = mapping_table("uppercase_mappings", char::to_uppercase),
        sigma = sigma_context_table(),
    )
}

#[test]
fn runtime_case_tables_match_the_toolchain_case_mappings() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/native/runtime.c");
    let source = std::fs::read_to_string(&path).expect("read runtime.c");
    let start = source
        .find(BEGIN)
        .expect("runtime.c has the generated case tables");
    let end = source[start..]
        .find(END)
        .expect("the generated region ends")
        + start
        + END.len();
    let expected = generated_region();
    if source[start..end] == expected {
        return;
    }
    if std::env::var_os("CHIMI_UPDATE_UNICODE_TABLES").is_some() {
        let updated = format!("{}{expected}{}", &source[..start], &source[end..]);
        std::fs::write(&path, updated).expect("write runtime.c");
        return;
    }
    panic!(
        "the case tables in runtime.c are stale; regenerate them with \
         CHIMI_UPDATE_UNICODE_TABLES=1 cargo test -p chimi --test unicode_case_tables"
    );
}
