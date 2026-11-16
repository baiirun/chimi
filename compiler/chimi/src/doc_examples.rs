//! Runnable examples in doc comments (testing.md, "Documentation examples").
//!
//! A fenced ```` ```chimi ```` block in the `///` comment directly before an
//! item is an example. Each one becomes its own test module: the block's
//! leading `import` lines are its imports, and the rest is the body of a test
//! named `<item> example <n>`. The module keeps the documented file's layout,
//! with every other line blank and each example line's `///` replaced by
//! spaces, so a diagnostic in an example points at the line and column of the
//! doc comment it came from.

/// One example, as the text of the test module that runs it.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct DocExample {
    /// The documented item's name, as `unwrap_or`.
    pub(crate) item: String,
    /// The module's text: as many lines as the documented file.
    pub(crate) text: String,
}

/// The examples in `text`, in source order.
pub(crate) fn examples(text: &str) -> Vec<DocExample> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut examples = Vec::new();
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if doc_content(lines[index]).is_none() {
            index += 1;
            continue;
        }
        let start = index;
        while index < lines.len() && doc_content(lines[index]).is_some() {
            index += 1;
        }
        let Some(item) = lines.get(index).and_then(|line| item_name(line)) else {
            continue;
        };
        for block in fenced_blocks(&lines, start, index) {
            let ordinal = match counts.iter_mut().find(|(name, _)| *name == item) {
                Some((_, count)) => {
                    *count += 1;
                    *count
                }
                None => {
                    counts.push((item.clone(), 1));
                    1
                }
            };
            examples.push(DocExample {
                text: example_module(&lines, &block, &format!("{item} example {ordinal}")),
                item: item.clone(),
            });
        }
    }
    examples
}

/// The lines of one ```` ```chimi ```` block: the opening fence, the code, and
/// the closing fence.
struct Block {
    open: usize,
    code: std::ops::Range<usize>,
    close: usize,
}

/// The text after `///` (and one space), when `line` is a doc comment line.
fn doc_content(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix("///")?;
    if rest.starts_with('/') {
        return None;
    }
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

/// The ```` ```chimi ```` blocks in doc lines `start..end`. A block without a
/// closing fence is not an example.
fn fenced_blocks(lines: &[&str], start: usize, end: usize) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut open: Option<usize> = None;
    for index in start..end {
        let content = doc_content(lines[index]).unwrap_or_default().trim_end();
        match open {
            None if content.starts_with("```") => open = Some(index),
            Some(fence) if content == "```" => {
                if doc_content(lines[fence]).unwrap_or_default().trim_end() == "```chimi" {
                    blocks.push(Block {
                        open: fence,
                        code: fence + 1..index,
                        close: index,
                    });
                }
                open = None;
            }
            _ => {}
        }
    }
    blocks
}

/// The name an item line declares: `fn`, `struct`, `enum`, `interface`,
/// `const`, `process name`, or a native `type`, after `pub` and `native`.
fn item_name(line: &str) -> Option<String> {
    let mut words = line.trim_start();
    for prefix in ["pub ", "native "] {
        if let Some(rest) = words.strip_prefix(prefix) {
            words = rest.trim_start();
        }
    }
    let rest = ["fn ", "struct ", "enum ", "interface ", "const ", "process name ", "type "]
        .iter()
        .find_map(|keyword| words.strip_prefix(keyword))?
        .trim_start();
    let name: String = rest
        .chars()
        .take_while(|character| *character == '_' || character.is_ascii_alphanumeric())
        .collect();
    (!name.is_empty()).then_some(name)
}

/// The test module for `block`, laid out line for line like the documented
/// file. The test opens on the blank line after the imports, or on the
/// opening fence when there are none, or else at the start of the first body
/// line, and closes on the closing fence.
fn example_module(lines: &[&str], block: &Block, name: &str) -> String {
    let mut output: Vec<String> = vec![String::new(); lines.len()];
    for index in block.code.clone() {
        output[index] = code_line(lines[index]);
    }
    let code = |index: usize| doc_content(lines[index]).unwrap_or_default().trim();
    let body = block
        .code
        .clone()
        .find(|&index| !code(index).is_empty() && !code(index).starts_with("import "));
    let imports = block
        .code
        .clone()
        .take_while(|&index| Some(index) != body)
        .any(|index| code(index).starts_with("import "));
    let header = format!("test \"{}\" {{", name.replace('\\', "\\\\").replace('"', "\\\""));
    let opening = if imports {
        body.and_then(|body| (block.code.start..body).rev().find(|&index| code(index).is_empty()))
    } else {
        Some(block.open)
    };
    match (opening, body) {
        (Some(line), _) => output[line] = header,
        (None, Some(body)) => output[body] = format!("{header} {}", code(body)),
        (None, None) => output[block.close] = header.clone(),
    }
    // An example with no statement still needs a body.
    output[block.close].push_str(if body.is_some() { "}" } else { "Void }" });
    output.join("\n")
}

/// A doc line with its `///` (and the space after it) replaced by spaces, so
/// the code keeps its column.
fn code_line(line: &str) -> String {
    let indent = line.len() - line.trim_start().len();
    let rest = &line.trim_start()[3..];
    let (space, code) = match rest.strip_prefix(' ') {
        Some(code) => (1, code),
        None => (0, rest),
    };
    format!("{}{code}", " ".repeat(indent + 3 + space))
}

#[cfg(test)]
mod tests {
    use super::examples;

    #[test]
    fn an_example_keeps_the_documented_layout() {
        let text = concat!(
            "/// Doubles `value`.\n",
            "///\n",
            "/// ```chimi\n",
            "/// import app/math as Math\n",
            "/// import std/test as Test\n",
            "///\n",
            "/// Test.assert_equal(Math.double(2), 4)\n",
            "/// ```\n",
            "pub fn double(value: Int) -> Int {\n",
            "  value * 2\n",
            "}\n",
        );
        let found = examples(text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].item, "double");
        assert_eq!(
            found[0].text,
            concat!(
                "\n",
                "\n",
                "\n",
                "    import app/math as Math\n",
                "    import std/test as Test\n",
                "test \"double example 1\" {\n",
                "    Test.assert_equal(Math.double(2), 4)\n",
                "}\n",
                "\n",
                "\n",
                "\n",
            )
        );
    }

    #[test]
    fn only_chimi_blocks_before_an_item_are_examples() {
        let text = concat!(
            "/// A module comment.\n",
            "/// ```chimi\n",
            "/// 1\n",
            "/// ```\n",
            "\n",
            "/// ```text\n",
            "/// not code\n",
            "/// ```\n",
            "/// ```chimi fragment\n",
            "/// part of something\n",
            "/// ```\n",
            "/// ```chimi\n",
            "/// first\n",
            "/// ```\n",
            "/// ```chimi\n",
            "/// second\n",
            "/// ```\n",
            "pub native fn run<T>(value: T) -> T\n",
            "/// ```chimi\n",
            "/// import std/test as Test\n",
            "/// ```\n",
            "pub struct Point {\n",
            "  x: Int\n",
            "}\n",
            "//// not a doc comment\n",
            "/// ```chimi\n",
            "/// unclosed\n",
            "type Id = Int\n",
        );
        let found = examples(text);
        let names: Vec<&str> = found.iter().map(|example| example.item.as_str()).collect();
        assert_eq!(names, ["run", "run", "Point"]);
        assert!(found[2].text.contains("    import std/test as Test\ntest \"Point example 1\" {Void }"), "{}", found[2].text);
        assert!(found[0].text.contains("test \"run example 1\" {\n    first\n}"));
        assert!(found[1].text.contains("test \"run example 2\" {\n    second\n}"));
    }

    #[test]
    fn a_doc_run_before_a_blank_line_documents_nothing() {
        let text = "/// ```chimi\n/// 1\n/// ```\n\npub fn after() -> Int {\n  1\n}\n";
        assert_eq!(examples(text), []);
    }

    #[test]
    fn a_body_right_after_the_imports_opens_on_its_own_line() {
        let text = concat!(
            "/// ```chimi\n",
            "/// import std/test as Test\n",
            "/// Test.assert(True, \"holds\")\n",
            "/// ```\n",
            "fn check() -> Void {\n",
            "  Void\n",
            "}\n",
        );
        let found = examples(text);
        assert!(
            found[0]
                .text
                .contains("    import std/test as Test\ntest \"check example 1\" { Test.assert(True, \"holds\")\n}"),
            "{}",
            found[0].text
        );
    }
}
