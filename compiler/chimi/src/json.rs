use std::collections::HashSet;
use std::fmt::Write;

const MAX_JSON_DEPTH: usize = 128;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum JsonData {
    Null,
    Boolean(bool),
    Number(String),
    Text(String),
    Array(Vec<JsonData>),
    Object(Vec<(String, JsonData)>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct JsonDecodeError {
    pub(crate) message: String,
    pub(crate) offset: usize,
}

pub(crate) fn decode_json(source: &str) -> Result<JsonData, JsonDecodeError> {
    JsonParser {
        source,
        bytes: source.as_bytes(),
        offset: 0,
    }
    .parse()
}

pub(crate) fn write_json_data(output: &mut String, value: &JsonData) {
    match value {
        JsonData::Null => output.push_str("null"),
        JsonData::Boolean(value) => output.push_str(if *value { "true" } else { "false" }),
        JsonData::Number(value) => output.push_str(value),
        JsonData::Text(value) => write_json_string(output, value),
        JsonData::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_json_data(output, value);
            }
            output.push(']');
        }
        JsonData::Object(fields) => {
            let mut fields = fields.iter().collect::<Vec<_>>();
            fields.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
            output.push('{');
            for (index, (name, value)) in fields.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_json_string(output, name);
                output.push(':');
                write_json_data(output, value);
            }
            output.push('}');
        }
    }
}

pub(crate) fn write_json_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{8}' => output.push_str("\\b"),
            '\u{c}' => output.push_str("\\f"),
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

struct JsonParser<'source> {
    source: &'source str,
    bytes: &'source [u8],
    offset: usize,
}

impl JsonParser<'_> {
    fn parse(mut self) -> Result<JsonData, JsonDecodeError> {
        self.skip_whitespace();
        let value = self.value(0)?;
        self.skip_whitespace();
        if self.offset != self.bytes.len() {
            return Err(self.error("unexpected characters after JSON value"));
        }
        Ok(value)
    }

    fn value(&mut self, depth: usize) -> Result<JsonData, JsonDecodeError> {
        if depth > MAX_JSON_DEPTH {
            return Err(self.error("JSON nesting exceeds the configured limit"));
        }
        match self.peek() {
            Some(b'n') => {
                self.keyword(b"null")?;
                Ok(JsonData::Null)
            }
            Some(b't') => {
                self.keyword(b"true")?;
                Ok(JsonData::Boolean(true))
            }
            Some(b'f') => {
                self.keyword(b"false")?;
                Ok(JsonData::Boolean(false))
            }
            Some(b'"') => self.string().map(JsonData::Text),
            Some(b'[') => self.array(depth + 1),
            Some(b'{') => self.object(depth + 1),
            Some(b'-' | b'0'..=b'9') => self.number().map(JsonData::Number),
            Some(_) => Err(self.error("expected a JSON value")),
            None => Err(self.error("expected a JSON value")),
        }
    }

    fn keyword(&mut self, keyword: &[u8]) -> Result<(), JsonDecodeError> {
        if self.bytes.get(self.offset..self.offset + keyword.len()) == Some(keyword) {
            self.offset += keyword.len();
            Ok(())
        } else {
            Err(self.error("invalid JSON literal"))
        }
    }

    fn array(&mut self, depth: usize) -> Result<JsonData, JsonDecodeError> {
        self.offset += 1;
        self.skip_whitespace();
        let mut values = Vec::new();
        if self.take(b']') {
            return Ok(JsonData::Array(values));
        }
        loop {
            values.push(self.value(depth)?);
            self.skip_whitespace();
            if self.take(b']') {
                return Ok(JsonData::Array(values));
            }
            if !self.take(b',') {
                return Err(self.error("expected `,` or `]` in JSON array"));
            }
            self.skip_whitespace();
        }
    }

    fn object(&mut self, depth: usize) -> Result<JsonData, JsonDecodeError> {
        self.offset += 1;
        self.skip_whitespace();
        let mut fields = Vec::new();
        let mut names = HashSet::new();
        if self.take(b'}') {
            return Ok(JsonData::Object(fields));
        }
        loop {
            if self.peek() != Some(b'"') {
                return Err(self.error("expected a string key in JSON object"));
            }
            let name_offset = self.offset;
            let name = self.string()?;
            if !names.insert(name.clone()) {
                return Err(JsonDecodeError {
                    message: format!("duplicate JSON object key `{name}`"),
                    offset: name_offset,
                });
            }
            self.skip_whitespace();
            if !self.take(b':') {
                return Err(self.error("expected `:` after JSON object key"));
            }
            self.skip_whitespace();
            let value = self.value(depth)?;
            fields.push((name, value));
            self.skip_whitespace();
            if self.take(b'}') {
                return Ok(JsonData::Object(fields));
            }
            if !self.take(b',') {
                return Err(self.error("expected `,` or `}` in JSON object"));
            }
            self.skip_whitespace();
        }
    }

    fn number(&mut self) -> Result<String, JsonDecodeError> {
        let start = self.offset;
        self.take(b'-');
        match self.peek() {
            Some(b'0') => {
                self.offset += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(self.error("JSON numbers cannot contain leading zeroes"));
                }
            }
            Some(b'1'..=b'9') => self.digits(),
            _ => return Err(self.error("expected digits in JSON number")),
        }
        if self.take(b'.') {
            let fraction = self.offset;
            self.digits();
            if self.offset == fraction {
                return Err(self.error("expected digits after JSON decimal point"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.offset += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.offset += 1;
            }
            let exponent = self.offset;
            self.digits();
            if self.offset == exponent {
                return Err(self.error("expected digits in JSON exponent"));
            }
        }
        Ok(self.source[start..self.offset].to_owned())
    }

    fn digits(&mut self) {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.offset += 1;
        }
    }

    fn string(&mut self) -> Result<String, JsonDecodeError> {
        debug_assert_eq!(self.peek(), Some(b'"'));
        self.offset += 1;
        let mut value = String::new();
        loop {
            match self.peek() {
                Some(b'"') => {
                    self.offset += 1;
                    return Ok(value);
                }
                Some(b'\\') => {
                    self.offset += 1;
                    self.escape(&mut value)?;
                }
                Some(0x00..=0x1f) => {
                    return Err(self.error("unescaped control character in JSON string"));
                }
                Some(_) => {
                    let character = self.source[self.offset..]
                        .chars()
                        .next()
                        .expect("offset remains on a UTF-8 boundary");
                    self.offset += character.len_utf8();
                    value.push(character);
                }
                None => return Err(self.error("unterminated JSON string")),
            }
        }
    }

    fn escape(&mut self, output: &mut String) -> Result<(), JsonDecodeError> {
        let Some(escape) = self.peek() else {
            return Err(self.error("unterminated JSON escape"));
        };
        self.offset += 1;
        match escape {
            b'"' => output.push('"'),
            b'\\' => output.push('\\'),
            b'/' => output.push('/'),
            b'b' => output.push('\u{8}'),
            b'f' => output.push('\u{c}'),
            b'n' => output.push('\n'),
            b'r' => output.push('\r'),
            b't' => output.push('\t'),
            b'u' => {
                let first = self.hex_quad()?;
                let scalar = if (0xd800..=0xdbff).contains(&first) {
                    if !self.take(b'\\') || !self.take(b'u') {
                        return Err(
                            self.error("high surrogate must be followed by a low surrogate")
                        );
                    }
                    let second = self.hex_quad()?;
                    if !(0xdc00..=0xdfff).contains(&second) {
                        return Err(
                            self.error("high surrogate must be followed by a low surrogate")
                        );
                    }
                    0x10000 + ((u32::from(first) - 0xd800) << 10) + (u32::from(second) - 0xdc00)
                } else if (0xdc00..=0xdfff).contains(&first) {
                    return Err(self.error("isolated low surrogate in JSON string"));
                } else {
                    u32::from(first)
                };
                output.push(char::from_u32(scalar).expect("validated JSON scalar"));
            }
            _ => return Err(self.error("invalid JSON escape")),
        }
        Ok(())
    }

    fn hex_quad(&mut self) -> Result<u16, JsonDecodeError> {
        let start = self.offset;
        let end = start.saturating_add(4);
        let Some(bytes) = self.bytes.get(start..end) else {
            return Err(self.error("incomplete JSON Unicode escape"));
        };
        let mut value = 0_u16;
        for byte in bytes {
            value = value
                .checked_mul(16)
                .expect("four hexadecimal digits fit in u16");
            value += match byte {
                b'0'..=b'9' => u16::from(byte - b'0'),
                b'a'..=b'f' => u16::from(byte - b'a' + 10),
                b'A'..=b'F' => u16::from(byte - b'A' + 10),
                _ => return Err(self.error("invalid JSON Unicode escape")),
            };
        }
        self.offset = end;
        Ok(value)
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.offset += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.offset).copied()
    }

    fn take(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.offset += 1;
            true
        } else {
            false
        }
    }

    fn error(&self, message: &str) -> JsonDecodeError {
        JsonDecodeError {
            message: message.to_owned(),
            offset: self.offset,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{JsonData, decode_json, write_json_data, write_json_string};

    #[test]
    fn strings_escape_json_control_characters() {
        let mut output = String::new();
        write_json_string(&mut output, "quote\" slash\\ line\n\t\u{1}");

        assert_eq!(output, "\"quote\\\" slash\\\\ line\\n\\t\\u0001\"");
    }

    #[test]
    fn decode_rejects_duplicate_keys_and_invalid_numbers() {
        assert!(decode_json("{\"a\":1,\"a\":2}").is_err());
        for source in ["01", "1.", "1e", "-"] {
            assert!(decode_json(source).is_err(), "{source}");
        }
    }

    #[test]
    fn object_encoding_sorts_utf8_keys_recursively() {
        let value = JsonData::Object(vec![
            ("z".to_owned(), JsonData::Null),
            (
                "a".to_owned(),
                JsonData::Object(vec![
                    ("β".to_owned(), JsonData::Boolean(true)),
                    ("b".to_owned(), JsonData::Number("2".to_owned())),
                ]),
            ),
        ]);
        let mut output = String::new();
        write_json_data(&mut output, &value);
        assert_eq!(output, "{\"a\":{\"b\":2,\"β\":true},\"z\":null}");
    }

    #[test]
    fn unicode_surrogate_pairs_decode_to_one_scalar() {
        assert_eq!(
            decode_json("\"\\ud83d\\udca5\"").unwrap(),
            JsonData::Text("💥".to_owned())
        );
    }

    #[test]
    fn current_runtime_event_and_outcome_envelopes_decode_as_objects() {
        let records = [
            r#"{"schema":"chimi.runtime.v1","event":"computation_started","sequence":0,"computation":"main","execution_id":"run-1","span_id":1,"parent_span_id":null,"core_node_id":0,"origin_id":0,"semantic_id":0,"file_id":0,"source_start":0,"source_end":1,"operation":"computation","attributes":{}}"#,
            r#"{"schema":"chimi.runtime.v1","event":"operation_started","sequence":1,"computation":"main","execution_id":"run-1","span_id":2,"parent_span_id":1,"core_node_id":0,"origin_id":0,"semantic_id":0,"file_id":0,"source_start":0,"source_end":1,"operation":"call","attributes":{}}"#,
            r#"{"schema":"chimi.runtime.v1","event":"dependency_called","sequence":2,"computation":"main","execution_id":"run-1","span_id":3,"parent_span_id":2,"core_node_id":0,"origin_id":0,"semantic_id":0,"file_id":0,"source_start":0,"source_end":1,"operation":"dependency_call","attributes":{"requirement":"payment","requirement_symbol_id":1,"interface_symbol_id":2,"method":"charge","method_semantic_id":3}}"#,
            r#"{"schema":"chimi.runtime.v1","event":"operation_completed","sequence":3,"computation":"main","execution_id":"run-1","span_id":2,"parent_span_id":1,"core_node_id":0,"origin_id":0,"semantic_id":0,"file_id":0,"source_start":0,"source_end":1,"operation":"call","attributes":{"outcome":"success"}}"#,
            r#"{"schema":"chimi.runtime.v1","outcome":"success","value":42}"#,
            r#"{"schema":"chimi.runtime.v1","outcome":"expected_failure","error":{"type":"PaymentError","variant":"Declined","fields":{}}}"#,
            r#"{"schema":"chimi.runtime.v1","outcome":"defect","message":"overflow","core_node_id":4,"origin_id":2}"#,
            r#"{"schema":"chimi.runtime.v1","outcome":"cancellation","reason":"requested before the next semantic operation"}"#,
        ];

        for record in records {
            assert!(matches!(decode_json(record), Ok(JsonData::Object(_))));
        }
    }

    #[test]
    fn decode_enforces_the_nesting_limit() {
        let source = format!("{}null{}", "[".repeat(129), "]".repeat(129));
        let error = decode_json(&source).unwrap_err();

        assert!(error.message.contains("nesting"));
    }
}
