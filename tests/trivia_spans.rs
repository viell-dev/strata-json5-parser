//! Source fidelity: byte-for-byte reconstruction, trivia retention (comments included), span
//! correctness, and zero-copy access.

use strata_json5_parser::{
    Document, Json5Error, Json5Parser, Json5Token, Trivia, TriviaKind, Value,
};
use strata_lexer::Span;

fn parse(input: &str) -> Result<Document, Json5Error> {
    Json5Parser::new().parse_str(input)
}

/// Collects every retained span — leading trivia and lexemes, in source order — for the whole
/// document.
fn all_spans(document: &Document) -> Vec<Span> {
    let mut spans = Vec::new();

    document.root().for_each_token(&mut |token| {
        for trivia in token.leading() {
            spans.push(trivia.span());
        }

        spans.push(token.span());
    });

    for trivia in document.trailing_trivia() {
        spans.push(trivia.span());
    }

    spans
}

/// Rebuilds the source text from the retained spans.
fn reconstruct(document: &Document) -> Option<Vec<u8>> {
    let mut out = Vec::new();

    for span in all_spans(document) {
        out.extend_from_slice(document.bytes(span)?);
    }

    Some(out)
}

fn assert_reconstructs(input: &str) {
    let document = match parse(input) {
        Ok(document) => document,
        Err(error) => panic!("expected {input:?} to parse: {error}"),
    };

    assert_eq!(
        reconstruct(&document).as_deref(),
        Some(input.as_bytes()),
        "input {input:?}"
    );
}

#[test]
fn kitchen_sink_document_reconstructs_byte_for_byte() {
    // Every JSON5 feature in one source: BOM, comments, expanded whitespace, identifier and
    // string names, single quotes, continuations, escapes, hex, signs, bare words, trailing
    // commas, and a trailing comment without a newline.
    let input = concat!(
        "\u{FEFF}// header comment\r\n",
        "{\u{A0}\n",
        "  unquoted: 'single\\'',\n\n",
        "  $escaped\\u0041: \"double\\u2028quote\",\u{2028}",
        "  /* block\n     comment */ nums: [0xC0FFEE, .5, 2., +3e1, -Infinity, NaN,],\u{3000}\n",
        "  true: null,\n",
        "  'cont': 'a\\\r\nb',\n",
        "}\u{2029} // done"
    );

    assert_reconstructs(input);
}

#[test]
fn simple_documents_reconstruct() {
    for input in [
        "1",
        "  null  ",
        "\u{FEFF}[]",
        "{a:1}",
        "[1,]",
        "// c\n0",
        "'\u{2028}'",
    ] {
        assert_reconstructs(input);
    }
}

#[test]
fn spans_and_positions_are_exact() -> Result<(), Json5Error> {
    let document = parse("{ key: 'v' }")?;
    let Value::Object(object) = document.root() else {
        panic!("expected an object");
    };
    let member = object
        .members()
        .first()
        .unwrap_or_else(|| panic!("one member"));

    assert_eq!(member.name().span(), Span::from_start_len(2, 3));
    assert_eq!(member.name().start().column(), 3);
    assert_eq!(member.colon().span(), Span::from_start_len(5, 1));
    assert_eq!(member.value().span(), Span::from_start_len(7, 3));
    assert_eq!(object.close().span(), Span::from_start_len(11, 1));

    Ok(())
}

#[test]
fn comment_trivia_attaches_to_the_following_token() -> Result<(), Json5Error> {
    let document = parse("[1, /* why */ 2]")?;
    let Value::Array(array) = document.root() else {
        panic!("expected an array");
    };
    let second = array
        .elements()
        .get(1)
        .unwrap_or_else(|| panic!("second element"));
    let leading = second.value().first_token().leading();
    let kinds: Vec<_> = leading
        .iter()
        .map(strata_json_parser::cst::Trivia::kind)
        .collect();

    assert_eq!(
        kinds,
        vec![
            TriviaKind::Whitespace,
            TriviaKind::BlockComment,
            TriviaKind::Whitespace,
        ]
    );

    Ok(())
}

#[test]
fn blank_lines_are_observable_through_trivia() -> Result<(), Json5Error> {
    let document = parse("{\n  a: 1,\n\n\n  b: 2\n}")?;
    let Value::Object(object) = document.root() else {
        panic!("expected an object");
    };
    let b_leading = object
        .members()
        .get(1)
        .unwrap_or_else(|| panic!("member b"))
        .name()
        .leading();
    let blank_lines: u64 = b_leading.iter().map(Trivia::blank_lines).sum();

    assert_eq!(blank_lines, 2);

    Ok(())
}

#[test]
fn zero_copy_access_returns_raw_slices() -> Result<(), Json5Error> {
    let document = parse("{ name: 'strata\\u0021' }")?;
    let Value::Object(object) = document.root() else {
        panic!("expected an object");
    };
    let member = object
        .members()
        .first()
        .unwrap_or_else(|| panic!("one member"));

    // Nothing is decoded: the escape stays raw, the quotes stay on.
    assert_eq!(document.token_bytes(member.name()), Some(&b"name"[..]));
    assert_eq!(
        document.token_bytes(member.value().first_token()),
        Some(&b"'strata\\u0021'"[..])
    );

    Ok(())
}

#[test]
fn source_refs_survive_store_reclamation() -> Result<(), Json5Error> {
    let mut document = parse("[ 'keep me' ]")?;
    let span = match document.root() {
        Value::Array(array) => array
            .elements()
            .first()
            .unwrap_or_else(|| panic!("element"))
            .value()
            .span(),
        other => panic!("expected an array, got {other:?}"),
    };

    let source_ref = document
        .source_ref(span)
        .unwrap_or_else(|| panic!("source ref"));
    document.source_mut().retire_all();

    // The promotable handle still resolves after the buffers are gone.
    assert_eq!(source_ref.resolve(), b"'keep me'".to_vec());
    // Plain span lookups no longer do.
    assert_eq!(document.bytes(span), None);

    Ok(())
}

#[test]
fn token_kinds_are_preserved_in_the_tree() -> Result<(), Json5Error> {
    let document = parse("{ a: Infinity, 'b': NaN }")?;
    let Value::Object(object) = document.root() else {
        panic!("expected an object");
    };

    let first = object
        .members()
        .first()
        .unwrap_or_else(|| panic!("member a"));
    assert_eq!(first.name().kind, Json5Token::Identifier);
    assert_eq!(first.value().first_token().kind, Json5Token::Infinity);

    let second = object
        .members()
        .get(1)
        .unwrap_or_else(|| panic!("member b"));
    assert_eq!(second.name().kind, Json5Token::String);
    assert_eq!(second.value().first_token().kind, Json5Token::NaN);

    Ok(())
}
