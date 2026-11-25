//! The `std/text` operations that need Unicode data, shared by the
//! interpreter and the native runtime (through [`crate::text_abi`]).

use alloc::string::String;

/// Unicode `White_Space`, the characters `Text.trim_start` and `Text.trim_end`
/// remove.
pub fn is_white_space(character: char) -> bool {
    matches!(
        character,
        '\u{9}'..='\u{d}'
            | ' '
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

pub fn trim_start(text: &str) -> &str {
    text.trim_start_matches(is_white_space)
}

pub fn trim_end(text: &str) -> &str {
    text.trim_end_matches(is_white_space)
}

/// `Text.lowercase`: the Unicode default case conversion, Final_Sigma
/// included. Mappings may change the length.
pub fn lowercase(text: &str) -> String {
    text.to_lowercase()
}

/// `Text.uppercase`: the Unicode default case conversion (`ß` becomes `SS`).
pub fn uppercase(text: &str) -> String {
    text.to_uppercase()
}
