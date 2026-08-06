use std::fmt::Write;

pub(crate) fn write_json_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                write!(output, "\\u{:04x}", character as u32)
                    .expect("writing JSON to a string cannot fail");
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

#[cfg(test)]
mod tests {
    use super::write_json_string;

    #[test]
    fn strings_escape_json_control_characters() {
        let mut output = String::new();
        write_json_string(&mut output, "quote\" slash\\ line\n\t\u{1}");

        assert_eq!(output, "\"quote\\\" slash\\\\ line\\n\\t\\u0001\"");
    }
}
