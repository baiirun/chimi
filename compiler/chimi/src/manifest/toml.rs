//! The part of TOML that `chimi.toml` and `chimi.lock` use: `#` comments,
//! `[table]`, dotted `[table.name]` and `[[array]]` headers, bare keys, and
//! values that are basic
//! strings, arrays of strings (which may span lines), or inline tables of
//! strings. Anything else is an error with its line, so a file the compiler
//! cannot read fully is never read in part.

use std::fmt;

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Document {
    /// Keys written before any header.
    pub(crate) root: Table,
    pub(crate) tables: Vec<Table>,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Table {
    pub(crate) name: String,
    /// True for a `[[name]]` header, one element of an array of tables.
    pub(crate) array_element: bool,
    pub(crate) line: usize,
    pub(crate) entries: Vec<Entry>,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Entry {
    pub(crate) key: String,
    pub(crate) line: usize,
    pub(crate) value: Value,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Value {
    String(String),
    Array(Vec<String>),
    InlineTable(Vec<(String, String)>),
}

impl Value {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::String(_) => "a string",
            Self::Array(_) => "an array",
            Self::InlineTable(_) => "an inline table",
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct SyntaxError {
    pub(crate) line: usize,
    pub(crate) message: String,
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "line {}: {}", self.line, self.message)
    }
}

pub(crate) fn parse(text: &str) -> Result<Document, SyntaxError> {
    let mut parser = Parser {
        characters: text.strip_prefix('\u{feff}').unwrap_or(text).chars().collect(),
        position: 0,
        line: 1,
    };
    parser.document()
}

struct Parser {
    characters: Vec<char>,
    position: usize,
    line: usize,
}

impl Parser {
    fn document(&mut self) -> Result<Document, SyntaxError> {
        let mut root = Table {
            name: String::new(),
            array_element: false,
            line: 1,
            entries: Vec::new(),
        };
        let mut tables: Vec<Table> = Vec::new();
        loop {
            self.skip_blank_lines();
            let Some(character) = self.peek() else { break };
            if character == '[' {
                let table = self.header()?;
                let repeated = tables
                    .iter()
                    .any(|prior| prior.name == table.name && !(prior.array_element && table.array_element));
                if repeated {
                    return Err(self.error_at(table.line, format!("table [{}] is defined twice", table.name)));
                }
                tables.push(table);
            } else {
                let entry = self.entry()?;
                let table = tables.last_mut().unwrap_or(&mut root);
                if table.entries.iter().any(|prior| prior.key == entry.key) {
                    return Err(self.error_at(entry.line, format!("key `{}` is defined twice", entry.key)));
                }
                table.entries.push(entry);
            }
            self.end_of_line()?;
        }
        Ok(Document { root, tables })
    }

    fn header(&mut self) -> Result<Table, SyntaxError> {
        let line = self.line;
        self.expect('[')?;
        let array_element = self.eat('[');
        self.skip_spaces();
        // A dotted name, as `[git.json_tools]`, is kept whole.
        let mut name = self.bare_key()?;
        while self.eat('.') {
            name.push('.');
            name.push_str(&self.bare_key()?);
        }
        self.skip_spaces();
        self.expect(']')?;
        if array_element {
            self.expect(']')?;
        }
        Ok(Table {
            name,
            array_element,
            line,
            entries: Vec::new(),
        })
    }

    fn entry(&mut self) -> Result<Entry, SyntaxError> {
        let line = self.line;
        let key = self.bare_key()?;
        self.skip_spaces();
        self.expect('=')?;
        self.skip_spaces();
        let value = match self.peek() {
            Some('"') => Value::String(self.string()?),
            Some('[') => Value::Array(self.array()?),
            Some('{') => Value::InlineTable(self.inline_table()?),
            _ => return Err(self.error("expected a string, an array or an inline table")),
        };
        Ok(Entry { key, line, value })
    }

    fn array(&mut self) -> Result<Vec<String>, SyntaxError> {
        self.expect('[')?;
        let mut items = Vec::new();
        loop {
            self.skip_blank_lines();
            if self.eat(']') {
                return Ok(items);
            }
            items.push(self.string()?);
            self.skip_blank_lines();
            if self.eat(']') {
                return Ok(items);
            }
            self.expect(',')?;
        }
    }

    fn inline_table(&mut self) -> Result<Vec<(String, String)>, SyntaxError> {
        self.expect('{')?;
        let mut entries: Vec<(String, String)> = Vec::new();
        self.skip_spaces();
        if self.eat('}') {
            return Ok(entries);
        }
        loop {
            self.skip_spaces();
            let key = self.bare_key()?;
            if entries.iter().any(|(prior, _)| *prior == key) {
                return Err(self.error(format!("key `{key}` is defined twice")));
            }
            self.skip_spaces();
            self.expect('=')?;
            self.skip_spaces();
            let value = self.string()?;
            entries.push((key, value));
            self.skip_spaces();
            if self.eat('}') {
                return Ok(entries);
            }
            self.expect(',')?;
        }
    }

    fn bare_key(&mut self) -> Result<String, SyntaxError> {
        let start = self.position;
        while self
            .peek()
            .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_' || character == '-')
        {
            self.position += 1;
        }
        if start == self.position {
            return Err(self.error("expected a key"));
        }
        Ok(self.characters[start..self.position].iter().collect())
    }

    fn string(&mut self) -> Result<String, SyntaxError> {
        self.expect('"')?;
        let mut text = String::new();
        loop {
            // Checked before consuming, so the error names the string's line.
            let character = match self.peek() {
                None | Some('\n') => return Err(self.error("unterminated string")),
                Some(_) => self.next().expect("peeked a character"),
            };
            match character {
                '"' => return Ok(text),
                '\\' => text.push(self.escape()?),
                character if character.is_control() && character != '\t' => {
                    return Err(self.error("control character in a string"));
                }
                character => text.push(character),
            }
        }
    }

    fn escape(&mut self) -> Result<char, SyntaxError> {
        match self.next() {
            Some('"') => Ok('"'),
            Some('\\') => Ok('\\'),
            Some('n') => Ok('\n'),
            Some('t') => Ok('\t'),
            Some('r') => Ok('\r'),
            Some('u') => {
                let digits: String = (0..4).filter_map(|_| self.next()).collect();
                u32::from_str_radix(&digits, 16)
                    .ok()
                    .filter(|_| digits.len() == 4)
                    .and_then(char::from_u32)
                    .ok_or_else(|| self.error("invalid \\u escape"))
            }
            _ => Err(self.error("unknown escape in a string")),
        }
    }

    /// After a header or an entry, only spaces and a comment may follow on
    /// the line.
    fn end_of_line(&mut self) -> Result<(), SyntaxError> {
        self.skip_spaces();
        self.skip_comment();
        match self.peek() {
            None => Ok(()),
            Some('\n') => {
                self.next();
                Ok(())
            }
            Some('\r') if self.characters.get(self.position + 1) == Some(&'\n') => {
                self.position += 2;
                self.line += 1;
                Ok(())
            }
            Some(_) => Err(self.error("expected the end of the line")),
        }
    }

    fn skip_blank_lines(&mut self) {
        loop {
            self.skip_spaces();
            self.skip_comment();
            match self.peek() {
                Some('\n') => {
                    self.next();
                }
                Some('\r') if self.characters.get(self.position + 1) == Some(&'\n') => {
                    self.position += 2;
                    self.line += 1;
                }
                _ => return,
            }
        }
    }

    fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.position += 1;
        }
    }

    fn skip_comment(&mut self) {
        if self.peek() == Some('#') {
            while self.peek().is_some_and(|character| character != '\n' && character != '\r') {
                self.position += 1;
            }
        }
    }

    fn peek(&self) -> Option<char> {
        self.characters.get(self.position).copied()
    }

    fn next(&mut self) -> Option<char> {
        let character = self.peek()?;
        self.position += 1;
        if character == '\n' {
            self.line += 1;
        }
        Some(character)
    }

    fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.next();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: char) -> Result<(), SyntaxError> {
        if self.eat(expected) {
            Ok(())
        } else {
            Err(self.error(format!("expected `{expected}`")))
        }
    }

    fn error(&self, message: impl Into<String>) -> SyntaxError {
        self.error_at(self.line, message)
    }

    fn error_at(&self, line: usize, message: impl Into<String>) -> SyntaxError {
        SyntaxError {
            line,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Value, parse};

    #[test]
    fn reads_tables_entries_and_every_value_form() {
        let document = parse(concat!(
            "version = \"1\" # trailing comment\n",
            "\n",
            "[package]\n",
            "name = \"shop\"\n",
            "[dependencies]\n",
            "money = { path = \"../money\" }\n",
            "list = [\n  \"a\",\n  \"b\\u00e9\", # comment\n]\n",
            "[[git]]\nname = \"x\"\n[[git]]\nname = \"y\"\n",
            "[git.json_tools]\nurl = \"u\"\n",
        ))
        .unwrap();
        assert_eq!(document.root.entries[0].value, Value::String("1".into()));
        let names: Vec<_> = document.tables.iter().map(|table| table.name.as_str()).collect();
        assert_eq!(names, ["package", "dependencies", "git", "git", "git.json_tools"]);
        assert_eq!(
            document.tables[1].entries[0].value,
            Value::InlineTable(vec![("path".into(), "../money".into())])
        );
        assert_eq!(
            document.tables[1].entries[1].value,
            Value::Array(vec!["a".into(), "bé".into()])
        );
        assert_eq!(document.tables[1].entries[1].line, 7);
        assert!(document.tables[2].array_element);
    }

    #[test]
    fn rejects_what_it_cannot_read_with_the_line() {
        let cases = [
            ("name = shop\n", 1, "expected a string, an array or an inline table"),
            ("[package]\nname = \"a\"\nname = \"b\"\n", 3, "key `name` is defined twice"),
            ("[package]\n[package]\n", 2, "table [package] is defined twice"),
            ("name = \"a\" extra\n", 1, "expected the end of the line"),
            ("name = \"a\n", 1, "unterminated string"),
            ("x = 1\n", 1, "expected a string, an array or an inline table"),
            ("x = { a = \"1\", a = \"2\" }\n", 1, "key `a` is defined twice"),
            ("x = \"\\q\"\n", 1, "unknown escape in a string"),
        ];
        for (text, line, message) in cases {
            let error = parse(text).expect_err(text);
            assert_eq!((error.line, error.message.as_str()), (line, message), "{text}");
        }
    }

    #[test]
    fn skips_a_byte_order_mark() {
        let document = parse("\u{feff}[package]\nname = \"a\"\n").unwrap();
        assert_eq!(document.tables[0].name, "package");
    }

    #[test]
    fn reads_crlf_line_ends() {
        let document = parse("[package]\r\nname = \"a\"\r\n").unwrap();
        assert_eq!(document.tables[0].entries[0].line, 2);
    }
}
