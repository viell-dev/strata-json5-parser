//! JSON5 string features: quote kinds, the extended escape repertoire, line continuations for
//! every terminator kind, and the spec-mandated U+2028/U+2029 warnings.

use strata_json5_parser::{Document, Json5Error, Json5Parser, Value, WarningKind};

fn parse(input: &str) -> Result<Document, Json5Error> {
    Json5Parser::new().parse_str(input)
}

/// Parses a lone string document and returns its raw lexeme bytes (quotes included).
fn raw_string(input: &str) -> Vec<u8> {
    let document = match parse(input) {
        Ok(document) => document,
        Err(error) => panic!("expected {input:?} to parse: {error}"),
    };
    let Value::String(token) = document.root() else {
        panic!("expected a string root for {input:?}");
    };

    document
        .token_bytes(token)
        .unwrap_or_else(|| panic!("retained bytes"))
        .to_vec()
}

#[test]
fn both_quote_kinds_parse_and_retain_their_quotes() {
    assert_eq!(raw_string("'single'"), b"'single'");
    assert_eq!(raw_string("\"double\""), b"\"double\"");
    assert_eq!(raw_string("'say \"hi\"'"), b"'say \"hi\"'");
    assert_eq!(raw_string("\"it's\""), b"\"it's\"");
}

#[test]
fn every_escape_parses() {
    // JSON escapes.
    raw_string("\"\\\" \\\\ \\/ \\b \\f \\n \\r \\t \\u0041\"");
    // JSON5 additions: \' \v \0 \xHH, and non-escape characters.
    raw_string("'\\' \\v \\0 \\x41 \\q \\\u{20AC}'");
    // Surrogate pair (combined downstream, retained raw here).
    raw_string("'\\uD83D\\uDE00'");
}

#[test]
fn line_continuations_parse_for_every_terminator_sequence() {
    for terminator in ["\n", "\r", "\r\n", "\u{2028}", "\u{2029}"] {
        let input = format!("'before\\{terminator}after'");
        let bytes = raw_string(&input);
        // One string token containing the continuation verbatim.
        assert_eq!(bytes, input.as_bytes(), "terminator {terminator:?}");
    }
}

#[test]
fn raw_line_and_paragraph_separators_parse_with_a_warning() -> Result<(), Json5Error> {
    for separator in ['\u{2028}', '\u{2029}'] {
        let input = format!("'ab{separator}cd'");
        let document = parse(&input)?;

        let [warning] = document.warnings() else {
            panic!("expected exactly one warning for {separator:?}");
        };
        assert_eq!(
            warning.kind,
            WarningKind::UnescapedLineSeparator { separator }
        );
        // Bytes: 0 `'`, 1 `a`, 2 `b`, 3..6 the separator.
        assert_eq!(warning.position.byte(), 3);
        assert_eq!(warning.position.line(), 1);
    }

    Ok(())
}

#[test]
fn escaped_separators_produce_no_warning() -> Result<(), Json5Error> {
    // As a line continuation.
    let document = parse("'a\\\u{2028}b'")?;
    assert!(document.warnings().is_empty());

    // As a \u escape.
    let document = parse("'a\\u2028b'")?;
    assert!(document.warnings().is_empty());

    Ok(())
}

#[test]
fn separator_after_escaped_backslash_still_warns() -> Result<(), Json5Error> {
    // `\\` then a raw separator: the separator is unescaped.
    let document = parse("'a\\\\\u{2029}b'")?;
    assert_eq!(document.warnings().len(), 1);

    Ok(())
}

#[test]
fn warnings_are_recorded_for_member_name_strings_too() -> Result<(), Json5Error> {
    let document = parse("{ 'k\u{2028}ey': 1 }")?;
    assert_eq!(document.warnings().len(), 1);

    Ok(())
}

#[test]
fn multiple_warnings_arrive_in_source_order() -> Result<(), Json5Error> {
    let document = parse("['a\u{2028}b', 'c\u{2029}d']")?;
    let warnings = document.warnings();

    assert_eq!(warnings.len(), 2);
    assert!(
        warnings.first().map(|warning| warning.position.byte())
            < warnings.get(1).map(|warning| warning.position.byte())
    );
    assert!(matches!(
        warnings.first().map(|warning| warning.kind),
        Some(WarningKind::UnescapedLineSeparator {
            separator: '\u{2028}'
        })
    ));

    Ok(())
}

#[test]
fn warning_positions_account_for_line_continuations() -> Result<(), Json5Error> {
    // A continuation with a real LF precedes the separator: the warning's diagnostic line
    // advances past the continuation.
    let document = parse("'a\\\nb\u{2028}c'")?;
    let [warning] = document.warnings() else {
        panic!("expected one warning");
    };

    assert_eq!(warning.position.byte(), 5);
    assert_eq!(warning.position.line(), 2);
    assert_eq!(warning.position.column(), 2);

    Ok(())
}

#[test]
fn clean_documents_have_no_warnings() -> Result<(), Json5Error> {
    let document = parse("{ a: 'plain', b: [1, 2] } // done")?;
    assert!(document.warnings().is_empty());

    Ok(())
}

#[test]
fn raw_control_characters_are_legal_string_content() {
    let bytes = raw_string("'tab\there'");
    assert_eq!(bytes, b"'tab\there'");
}
