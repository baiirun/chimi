//! Generates the Unicode tables both backends use, and fails when a
//! checked-in copy is stale. Regenerate with
//! `CHIMI_UPDATE_UNICODE_TABLES=1 cargo test -p chimi --test unicode_tables`.
//!
//! - Case tables for `Text.lowercase` and `Text.uppercase` in `runtime.c`. The
//!   interpreter calls Rust's `str::to_lowercase` and `str::to_uppercase`, so
//!   the tables are derived from those functions, and a toolchain whose Unicode
//!   version moves fails here.
//! - The grapheme cluster property table, in `src/graphemes/table.rs` and in
//!   `runtime.c`, from the vendored files in `compiler/chimi/unicode/`, whose
//!   version must equal the toolchain's so case and segmentation agree.
//! - `std-tests/text/grapheme_break_test`, the official `GraphemeBreakTest.txt`
//!   cases as a program both backends run, with its expected outcome.

use std::path::{Path, PathBuf};

const CASE_BEGIN: &str = "/* BEGIN GENERATED unicode case tables";
const CASE_END: &str = "/* END GENERATED unicode case tables */";
const GRAPHEME_BEGIN: &str = "/* BEGIN GENERATED grapheme table";
const GRAPHEME_END: &str = "/* END GENERATED grapheme table */";

fn manifest_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn unicode_version() -> String {
    let (major, minor, update) = char::UNICODE_VERSION;
    format!("{major}.{minor}.{update}")
}

// Case tables.

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

fn case_region() -> String {
    format!(
        "{CASE_BEGIN} (tests/unicode_tables.rs, Unicode {}) */\n{}{}{}{CASE_END}",
        unicode_version(),
        mapping_table("lowercase_mappings", char::to_lowercase),
        mapping_table("uppercase_mappings", char::to_uppercase),
        sigma_context_table(),
    )
}

// Grapheme cluster properties.

/// Grapheme_Cluster_Break values as the data files spell them, with the
/// variant names of `src/graphemes.rs` and `runtime.c`.
const BREAKS: &[(&str, &str, &str)] = &[
    ("Other", "Other", "GB_OTHER"),
    ("CR", "Cr", "GB_CR"),
    ("LF", "Lf", "GB_LF"),
    ("Control", "Control", "GB_CONTROL"),
    ("Extend", "Extend", "GB_EXTEND"),
    ("ZWJ", "Zwj", "GB_ZWJ"),
    (
        "Regional_Indicator",
        "RegionalIndicator",
        "GB_REGIONAL_INDICATOR",
    ),
    ("Prepend", "Prepend", "GB_PREPEND"),
    ("SpacingMark", "SpacingMark", "GB_SPACING_MARK"),
    ("L", "L", "GB_L"),
    ("V", "V", "GB_V"),
    ("T", "T", "GB_T"),
    ("LV", "Lv", "GB_LV"),
    ("LVT", "Lvt", "GB_LVT"),
];

/// Indic_Conjunct_Break values, the same way.
const CONJUNCTS: &[(&str, &str, &str)] = &[
    ("None", "None", "INCB_NONE"),
    ("Linker", "Linker", "INCB_LINKER"),
    ("Consonant", "Consonant", "INCB_CONSONANT"),
    ("Extend", "Extend", "INCB_EXTEND"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
struct GraphemeProperties {
    grapheme_break: usize,
    conjunct: usize,
    pictographic: bool,
}

const DEFAULT_PROPERTIES: GraphemeProperties = GraphemeProperties {
    grapheme_break: 0,
    conjunct: 0,
    pictographic: false,
};

fn read_unicode_file(name: &str) -> String {
    std::fs::read_to_string(manifest_path(&format!("unicode/{name}")))
        .unwrap_or_else(|error| panic!("read unicode/{name}: {error}"))
}

/// The `(first, last, fields)` rows of a UCD data file, fields trimmed.
fn data_rows(text: &str) -> Vec<(u32, u32, Vec<&str>)> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let data = line.split('#').next().unwrap_or("").trim();
        if data.is_empty() {
            continue;
        }
        let mut fields = data.split(';').map(str::trim);
        let codes = fields.next().expect("a code point field");
        let (first, last) = codes.split_once("..").unwrap_or((codes, codes));
        let parse = |code: &str| u32::from_str_radix(code, 16).expect("a hexadecimal code point");
        rows.push((parse(first), parse(last), fields.collect()));
    }
    rows
}

fn position(values: &[(&str, &str, &str)], name: &str) -> usize {
    values
        .iter()
        .position(|(data_name, ..)| *data_name == name)
        .unwrap_or_else(|| panic!("unknown property value {name}"))
}

fn grapheme_ranges() -> Vec<(u32, u32, GraphemeProperties)> {
    let version = unicode_version();
    let break_file = read_unicode_file("GraphemeBreakProperty.txt");
    let emoji_file = read_unicode_file("emoji-data.txt");
    let conjunct_file = read_unicode_file("DerivedCoreProperties-InCB.txt");
    for (name, text) in [
        ("GraphemeBreakProperty", &break_file),
        ("DerivedCoreProperties", &conjunct_file),
    ] {
        assert!(
            text.starts_with(&format!("# {name}-{version}.txt")),
            "unicode/ holds another Unicode version than the toolchain's {version}"
        );
    }
    let mut properties = vec![DEFAULT_PROPERTIES; 0x110000];
    for (first, last, fields) in data_rows(&break_file) {
        for code in first..=last {
            properties[code as usize].grapheme_break = position(BREAKS, fields[0]);
        }
    }
    for (first, last, fields) in data_rows(&emoji_file) {
        if fields[0] == "Extended_Pictographic" {
            for code in first..=last {
                properties[code as usize].pictographic = true;
            }
        }
    }
    for (first, last, fields) in data_rows(&conjunct_file) {
        assert_eq!(fields[0], "InCB", "the excerpt holds only InCB rows");
        for code in first..=last {
            properties[code as usize].conjunct = position(CONJUNCTS, fields[1]);
        }
    }
    let mut ranges: Vec<(u32, u32, GraphemeProperties)> = Vec::new();
    for (code, property) in (0u32..).zip(properties) {
        if property == DEFAULT_PROPERTIES {
            continue;
        }
        match ranges.last_mut() {
            Some((_, last, previous)) if *last + 1 == code && *previous == property => *last = code,
            _ => ranges.push((code, code, property)),
        }
    }
    ranges
}

fn grapheme_rust_table(ranges: &[(u32, u32, GraphemeProperties)]) -> String {
    let mut table = format!(
        "// Generated by tests/unicode_tables.rs from compiler/chimi/unicode/ (Unicode {}).\n\
         // Ranges in ascending order; a code point in none of them is Other, None, and not\n\
         // Extended_Pictographic.\n\n\
         use super::{{Break, Conjunct}};\n\n\
         #[rustfmt::skip]\n\
         pub(super) const RANGES: &[(u32, u32, Break, Conjunct, bool)] = &[\n",
        unicode_version()
    );
    for (first, last, property) in ranges {
        table.push_str(&format!(
            "    (0x{first:x}, 0x{last:x}, Break::{}, Conjunct::{}, {}),\n",
            BREAKS[property.grapheme_break].1,
            CONJUNCTS[property.conjunct].1,
            property.pictographic
        ));
    }
    table.push_str("];\n");
    table
}

fn grapheme_c_region(ranges: &[(u32, u32, GraphemeProperties)]) -> String {
    let mut region = format!(
        "{GRAPHEME_BEGIN} (tests/unicode_tables.rs, Unicode {}) */\n\
         static const GraphemeRange grapheme_ranges[] = {{\n",
        unicode_version()
    );
    let entries: Vec<String> = ranges
        .iter()
        .map(|(first, last, property)| {
            format!(
                "{{0x{first:x}, 0x{last:x}, {}, {}, {}}}",
                BREAKS[property.grapheme_break].2,
                CONJUNCTS[property.conjunct].2,
                u8::from(property.pictographic)
            )
        })
        .collect();
    for line in entries.chunks(3) {
        region.push_str(&format!("  {},\n", line.join(", ")));
    }
    region.push_str(&format!("}};\n{GRAPHEME_END}"));
    region
}

// The official break test as a std-tests program.

/// The program and its expected outcome record. Each line of the result is
/// one case, as the UTF-8 byte length of each grapheme; the expectation comes
/// from the `÷` marks of the test file, not from either implementation.
fn grapheme_break_test_program() -> (String, String) {
    let mut calls = Vec::new();
    let mut expected_lines = Vec::new();
    for line in read_unicode_file("GraphemeBreakTest.txt").lines() {
        let case = line.split('#').next().unwrap_or("").trim();
        if case.is_empty() {
            continue;
        }
        let mut literal = String::new();
        let mut lengths = Vec::new();
        let mut cluster = 0;
        let mut has_nul = false;
        for token in case.split_whitespace() {
            match token {
                "÷" if cluster > 0 => {
                    lengths.push(cluster.to_string());
                    cluster = 0;
                }
                "÷" | "×" => {}
                code => {
                    let code = u32::from_str_radix(code, 16).expect("a hexadecimal code point");
                    has_nul |= code == 0;
                    cluster += char::from_u32(code).expect("a scalar value").len_utf8();
                    literal.push_str(&format!("\\u{{{code:x}}}"));
                }
            }
        }
        if has_nul {
            continue;
        }
        calls.push(format!("    lengths(\"{literal}\"),\n"));
        expected_lines.push(lengths.join(" "));
    }
    let program = format!(
        "// Generated by tests/unicode_tables.rs from unicode/GraphemeBreakTest.txt\n\
         // (Unicode {}). Each line of the result is one official case, as the UTF-8\n\
         // byte length of each grapheme. Cases with U+0000 are left out: a native\n\
         // Text cannot hold it.\n\
         import std/int as Int\n\
         import std/list as List\n\
         import std/text as Text\n\
         \n\
         fn lengths(text: Text) -> Text {{\n\
         \x20 Text.join(List.map(Text.graphemes(text), fn(grapheme: Text) {{ Int.to_text(Text.byte_length(grapheme)) }}), \" \")\n\
         }}\n\
         \n\
         pub fn main() -> Text {{\n\
         \x20 let cases = [\n{}  ]\n\
         \x20 Text.join(cases, \"\\n\")\n\
         }}\n",
        unicode_version(),
        calls.concat(),
    );
    let expected = format!(
        "{{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":\"{}\"}}\n",
        expected_lines.join("\\n")
    );
    (program, expected)
}

// Checking and updating.

fn replace_region(source: &str, begin: &str, end: &str, region: &str) -> String {
    let start = source
        .find(begin)
        .unwrap_or_else(|| panic!("runtime.c has no region {begin}"));
    let stop = source[start..]
        .find(end)
        .expect("the generated region ends")
        + start
        + end.len();
    format!("{}{region}{}", &source[..start], &source[stop..])
}

#[test]
fn generated_unicode_files_are_current() {
    let ranges = grapheme_ranges();
    let runtime_path = manifest_path("runtime/native/runtime.c");
    let runtime = std::fs::read_to_string(&runtime_path).expect("read runtime.c");
    let runtime_expected = replace_region(&runtime, CASE_BEGIN, CASE_END, &case_region());
    let runtime_expected = replace_region(
        &runtime_expected,
        GRAPHEME_BEGIN,
        GRAPHEME_END,
        &grapheme_c_region(&ranges),
    );
    let (program, expected_outcome) = grapheme_break_test_program();
    let outputs = [
        (runtime_path, runtime_expected),
        (
            manifest_path("src/graphemes/table.rs"),
            grapheme_rust_table(&ranges),
        ),
        (
            manifest_path("std-tests/text/grapheme_break_test.chimi"),
            program,
        ),
        (
            manifest_path("std-tests/text/grapheme_break_test.expected"),
            expected_outcome,
        ),
    ];
    let update = std::env::var_os("CHIMI_UPDATE_UNICODE_TABLES").is_some();
    let mut stale = Vec::new();
    for (path, expected) in outputs {
        if std::fs::read_to_string(&path).ok().as_deref() == Some(expected.as_str()) {
            continue;
        }
        if update {
            std::fs::write(&path, expected).expect("write a generated file");
        } else {
            stale.push(path.display().to_string());
        }
    }
    assert!(
        stale.is_empty(),
        "stale generated Unicode files: {}; regenerate them with \
         CHIMI_UPDATE_UNICODE_TABLES=1 cargo test -p chimi --test unicode_tables",
        stale.join(", ")
    );
}
