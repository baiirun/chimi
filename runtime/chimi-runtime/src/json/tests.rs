use super::*;
use alloc::string::String;

/// The tape as a compact line, to compare whole documents at once.
fn render(source: &str) -> String {
    let tape = decode(source.as_bytes()).expect("decodes");
    let mut out = String::new();
    for node in &tape.nodes {
        let text = |bytes: &[u8]| String::from_utf8(bytes.to_vec()).unwrap();
        let range = node.offset..node.offset + node.length;
        let part = match node.kind {
            NodeKind::Null => "null".into(),
            NodeKind::False => "false".into(),
            NodeKind::True => "true".into(),
            NodeKind::Number => format!("#{}", text(&source.as_bytes()[range])),
            NodeKind::SourceText => format!("s{:?}", text(&source.as_bytes()[range])),
            NodeKind::DecodedText => format!("d{:?}", text(&tape.decoded[range])),
            NodeKind::SourceKey => format!("k{:?}", text(&source.as_bytes()[range])),
            NodeKind::DecodedKey => format!("dk{:?}", text(&tape.decoded[range])),
            NodeKind::Array => format!("A{}", node.length),
            NodeKind::Object => format!("O{}", node.length),
        };
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&part);
    }
    out
}

fn error(source: &str) -> (usize, String) {
    let error = decode(source.as_bytes()).expect_err("fails");
    (
        error.offset,
        String::from_utf8(error.message.into_owned()).unwrap(),
    )
}

#[test]
fn a_document_is_its_values_in_post_order() {
    assert_eq!(
        render(r#" {"a": [1, -2.5e+3, true], "b\n": {}, "c": [[]], "d": null, "e": false} "#),
        r#"k"a" #1 #-2.5e+3 true A3 dk"b\n" O0 k"c" A0 A1 k"d" null k"e" false O5"#
    );
}

#[test]
fn escapes_decode_and_plain_strings_borrow_the_source() {
    assert_eq!(render(r#""plain é""#), r#"s"plain é""#);
    assert_eq!(
        render(r#""a\"\\\/\b\f\n\r\t\u00e9\ud83d\ude00z""#),
        "d\"a\\\"\\\\/\\u{8}\\u{c}\\n\\r\\té😀z\""
    );
    assert_eq!(render(r#""\u0000""#), "d\"\\0\"");
}

#[test]
fn nesting_stops_after_the_configured_depth() {
    let deepest = "[".repeat(MAX_DEPTH + 1) + &"]".repeat(MAX_DEPTH + 1);
    assert!(decode(deepest.as_bytes()).is_ok());
    let too_deep = "[".repeat(MAX_DEPTH + 2) + &"]".repeat(MAX_DEPTH + 2);
    assert_eq!(
        error(&too_deep),
        (
            MAX_DEPTH + 1,
            "JSON nesting exceeds the configured limit".into()
        )
    );
    let mut objects = String::new();
    for _ in 0..=MAX_DEPTH {
        objects.push_str("{\"k\": ");
    }
    objects.push_str(" 1");
    assert_eq!(
        error(&objects).1,
        "JSON nesting exceeds the configured limit"
    );
}

#[test]
fn errors_carry_the_interpreters_offset_and_message() {
    let cases: &[(&str, usize, &str)] = &[
        ("", 0, "expected a JSON value"),
        ("  ", 2, "expected a JSON value"),
        ("nul", 0, "invalid JSON literal"),
        ("x", 0, "expected a JSON value"),
        ("null x", 5, "unexpected characters after JSON value"),
        ("[1 2]", 3, "expected `,` or `]` in JSON array"),
        ("[1,]", 3, "expected a JSON value"),
        ("[1", 2, "expected `,` or `]` in JSON array"),
        ("{1:2}", 1, "expected a string key in JSON object"),
        ("{\"a\" 2}", 5, "expected `:` after JSON object key"),
        ("{\"a\":1,}", 7, "expected a string key in JSON object"),
        ("{\"a\":1 \"b\"}", 7, "expected `,` or `}` in JSON object"),
        ("{\"a\":1,\"a\":2}", 7, "duplicate JSON object key `a`"),
        (
            "{\"\\u0061\":1, \"a\":2}",
            13,
            "duplicate JSON object key `a`",
        ),
        ("-", 1, "expected digits in JSON number"),
        ("-x", 1, "expected digits in JSON number"),
        ("01", 1, "JSON numbers cannot contain leading zeroes"),
        ("1.", 2, "expected digits after JSON decimal point"),
        ("1e+", 3, "expected digits in JSON exponent"),
        ("\"abc", 4, "unterminated JSON string"),
        ("\"a\tb\"", 2, "unescaped control character in JSON string"),
        ("\"a\\", 3, "unterminated JSON escape"),
        ("\"\\x\"", 3, "invalid JSON escape"),
        ("\"\\u12\"", 3, "incomplete JSON Unicode escape"),
        ("\"\\u12x4\"", 3, "invalid JSON Unicode escape"),
        ("\"\\udc00\"", 7, "isolated low surrogate in JSON string"),
        (
            "\"\\ud800\"",
            7,
            "high surrogate must be followed by a low surrogate",
        ),
        (
            "\"\\ud800\\n\"",
            8,
            "high surrogate must be followed by a low surrogate",
        ),
        (
            "\"\\ud800\\u0041\"",
            13,
            "high surrogate must be followed by a low surrogate",
        ),
        ("\"\\ud800\\u00\"", 9, "incomplete JSON Unicode escape"),
    ];
    for &(source, offset, message) in cases {
        assert_eq!(error(source), (offset, message.into()), "{source:?}");
    }
}

#[test]
fn a_duplicate_key_is_quoted_whole_even_with_u0000() {
    assert_eq!(
        error("{\"a\\u0000b\":1,\"a\\u0000b\":2}"),
        (14, "duplicate JSON object key `a\0b`".into())
    );
}
