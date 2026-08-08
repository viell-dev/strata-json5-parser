//! Comments in every grammatical position and the expanded JSON5 whitespace set, including the
//! byte-order mark and the Unicode `Zs` space separators.

use strata_json5_parser::{Document, Json5Error, Json5Parser, TriviaKind, Value};

fn parse(input: &str) -> Result<Document, Json5Error> {
    Json5Parser::new().parse_str(input)
}

#[test]
fn comments_are_legal_in_every_inter_token_position() -> Result<(), Json5Error> {
    let input = "/*a*/ { /*b*/ key /*c*/ : /*d*/ [ /*e*/ 1 /*f*/ , /*g*/ ] /*h*/ , /*i*/ } // j";
    let document = parse(input)?;
    assert!(matches!(document.root(), Value::Object(_)));

    Ok(())
}

#[test]
fn line_comments_terminate_at_every_terminator_kind() -> Result<(), Json5Error> {
    for terminator in ["\n", "\r", "\r\n", "\u{2028}", "\u{2029}"] {
        let input = format!("// note{terminator}null");
        let document = parse(&input)?;
        assert!(
            matches!(document.root(), Value::Null(_)),
            "terminator {terminator:?}"
        );
    }

    Ok(())
}

#[test]
fn line_comment_may_end_the_document_without_a_newline() -> Result<(), Json5Error> {
    let document = parse("1 // trailing note")?;
    let kinds: Vec<_> = document
        .trailing_trivia()
        .iter()
        .map(strata_json_parser::cst::Trivia::kind)
        .collect();

    assert_eq!(kinds, vec![TriviaKind::Whitespace, TriviaKind::LineComment]);

    Ok(())
}

#[test]
fn comments_attach_as_leading_trivia_with_their_kinds() -> Result<(), Json5Error> {
    let document = parse("// header\n/* block */ 42")?;
    let leading = document.root().first_token().leading();
    let kinds: Vec<_> = leading
        .iter()
        .map(strata_json_parser::cst::Trivia::kind)
        .collect();

    assert_eq!(
        kinds,
        vec![
            TriviaKind::LineComment,
            TriviaKind::Whitespace,
            TriviaKind::BlockComment,
            TriviaKind::Whitespace,
        ]
    );
    // The comment text is retained verbatim.
    let comment = leading.first().unwrap_or_else(|| panic!("line comment"));
    assert_eq!(document.bytes(comment.span()), Some(&b"// header"[..]));

    Ok(())
}

#[test]
fn leading_bom_is_plain_whitespace_trivia() -> Result<(), Json5Error> {
    // No option required: U+FEFF is JSON5 whitespace.
    let document = parse("\u{FEFF}1")?;
    let leading = document.root().first_token().leading();

    assert_eq!(leading.len(), 1);
    assert_eq!(
        leading.first().map(strata_json_parser::cst::Trivia::kind),
        Some(TriviaKind::Whitespace)
    );

    Ok(())
}

#[test]
fn interior_and_trailing_boms_are_whitespace_too() -> Result<(), Json5Error> {
    let document = parse("[1,\u{FEFF}2]\u{FEFF}")?;
    assert!(matches!(document.root(), Value::Array(_)));
    assert_eq!(document.trailing_trivia().len(), 1);

    Ok(())
}

#[test]
fn expanded_whitespace_separates_tokens() -> Result<(), Json5Error> {
    // VT, FF, NBSP, LS, PS, ogham space mark, en/em/thin spaces, narrow NBSP, math space,
    // ideographic space — all between significant tokens.
    let input = "[\u{0B}1,\u{0C}2,\u{A0}3,\u{2028}4,\u{2029}5,\u{1680}6,\u{2003}7,\u{2009}8,\u{202F}9,\u{205F}10,\u{3000}11]";
    let document = parse(input)?;
    let Value::Array(array) = document.root() else {
        panic!("expected an array");
    };

    assert_eq!(array.elements().len(), 11);

    Ok(())
}

#[test]
fn line_terminators_are_ordinary_whitespace_between_tokens() -> Result<(), Json5Error> {
    let document = parse("{\r\n  a: 1,\u{2028}  b: 2\r}")?;
    let Value::Object(object) = document.root() else {
        panic!("expected an object");
    };

    assert_eq!(object.members().len(), 2);

    Ok(())
}
