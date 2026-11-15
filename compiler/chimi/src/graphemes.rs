//! Extended grapheme clusters (Unicode UAX #29), the unit `Text.graphemes`,
//! `Text.grapheme_count`, and the `Text.pad_*` functions count in.
//!
//! `runtime/native/runtime.c` implements the same rules (`grapheme_end`) over the
//! same generated table; `tests/unicode_tables.rs` generates both copies from
//! the vendored files in `compiler/chimi/unicode/`.

mod table;

/// The Grapheme_Cluster_Break property.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Break {
    Other,
    Cr,
    Lf,
    Control,
    Extend,
    Zwj,
    RegionalIndicator,
    Prepend,
    SpacingMark,
    L,
    V,
    T,
    Lv,
    Lvt,
}

/// The Indic_Conjunct_Break property, which rule GB9c reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Conjunct {
    None,
    Linker,
    Consonant,
    Extend,
}

#[derive(Clone, Copy)]
struct Properties {
    grapheme_break: Break,
    conjunct: Conjunct,
    pictographic: bool,
}

fn properties(character: char) -> Properties {
    let code = u32::from(character);
    let index = table::RANGES.partition_point(|&(_, last, ..)| last < code);
    match table::RANGES.get(index) {
        Some(&(first, _, grapheme_break, conjunct, pictographic)) if first <= code => Properties {
            grapheme_break,
            conjunct,
            pictographic,
        },
        _ => Properties {
            grapheme_break: Break::Other,
            conjunct: Conjunct::None,
            pictographic: false,
        },
    }
}

/// How far a cluster has matched the GB9c conjunct sequence
/// `Consonant [Extend Linker]* Linker [Extend Linker]*`.
#[derive(Clone, Copy, Eq, PartialEq)]
enum ConjunctRun {
    None,
    Consonant,
    Linked,
}

/// What the rules need to know about the cluster read so far.
struct Cluster {
    previous: Break,
    /// The cluster ends in `Extended_Pictographic Extend*` (GB11).
    pictographic_run: bool,
    /// The cluster ends in `Extended_Pictographic Extend* ZWJ` (GB11).
    pictographic_joiner: bool,
    /// The cluster ends in an odd number of regional indicators (GB12, GB13).
    odd_regional_indicators: bool,
    conjunct: ConjunctRun,
}

impl Cluster {
    fn start(first: Properties) -> Self {
        Self {
            previous: first.grapheme_break,
            pictographic_run: first.pictographic,
            pictographic_joiner: false,
            odd_regional_indicators: first.grapheme_break == Break::RegionalIndicator,
            conjunct: match first.conjunct {
                Conjunct::Consonant => ConjunctRun::Consonant,
                _ => ConjunctRun::None,
            },
        }
    }

    /// The rules GB3 to GB999, in their order of precedence.
    fn breaks_before(&self, next: Properties) -> bool {
        use Break::*;
        match (self.previous, next.grapheme_break) {
            (Cr, Lf) => false,
            (Cr | Lf | Control, _) | (_, Cr | Lf | Control) => true,
            (L, L | V | Lv | Lvt) | (Lv | V, V | T) | (Lvt | T, T) => false,
            (_, Extend | Zwj | SpacingMark) | (Prepend, _) => false,
            _ if self.conjunct == ConjunctRun::Linked && next.conjunct == Conjunct::Consonant => {
                false
            }
            (Zwj, _) if self.pictographic_joiner && next.pictographic => false,
            (RegionalIndicator, RegionalIndicator) => !self.odd_regional_indicators,
            _ => true,
        }
    }

    fn extend(&mut self, next: Properties) {
        self.pictographic_joiner = next.grapheme_break == Break::Zwj && self.pictographic_run;
        self.pictographic_run =
            next.pictographic || (self.pictographic_run && next.grapheme_break == Break::Extend);
        self.odd_regional_indicators =
            next.grapheme_break == Break::RegionalIndicator && !self.odd_regional_indicators;
        self.conjunct = match (next.conjunct, self.conjunct) {
            (Conjunct::Consonant, _) => ConjunctRun::Consonant,
            (_, ConjunctRun::None) => ConjunctRun::None,
            (Conjunct::Linker, _) => ConjunctRun::Linked,
            (Conjunct::Extend, run) => run,
            (Conjunct::None, _) => ConjunctRun::None,
        };
        self.previous = next.grapheme_break;
    }
}

/// Returns the byte offset where the grapheme cluster that starts at `start`
/// ends, or `text.len()` when `start` is the end.
///
/// Requires: `start` is a character boundary of `text`. A cluster boundary
/// depends only on the text from the cluster's start, so callers walk clusters
/// one at a time.
pub(crate) fn grapheme_end(text: &str, start: usize) -> usize {
    let mut characters = text[start..].char_indices();
    let Some((_, first)) = characters.next() else {
        return start;
    };
    let mut cluster = Cluster::start(properties(first));
    for (offset, character) in characters {
        let next = properties(character);
        if cluster.breaks_before(next) {
            return start + offset;
        }
        cluster.extend(next);
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::grapheme_end;

    /// Every case of the official `GraphemeBreakTest.txt`: `÷` marks a
    /// boundary, `×` none, between hexadecimal code points.
    #[test]
    fn clusters_match_the_unicode_grapheme_break_test() {
        let cases = include_str!("../unicode/GraphemeBreakTest.txt");
        let mut failures = Vec::new();
        let mut checked = 0;
        for line in cases.lines() {
            let case = line.split('#').next().unwrap_or("").trim();
            if case.is_empty() {
                continue;
            }
            let mut text = String::new();
            let mut expected = Vec::new();
            for token in case.split_whitespace() {
                match token {
                    "÷" => expected.push(text.len()),
                    "×" => {}
                    code => {
                        text.push(char::from_u32(u32::from_str_radix(code, 16).unwrap()).unwrap())
                    }
                }
            }
            let mut boundaries = vec![0];
            while let Some(&start) = boundaries.last().filter(|&&start| start < text.len()) {
                boundaries.push(grapheme_end(&text, start));
            }
            if boundaries != expected {
                failures.push(format!("{case}: got byte boundaries {boundaries:?}"));
            }
            checked += 1;
        }
        assert!(checked > 700, "read only {checked} cases");
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
