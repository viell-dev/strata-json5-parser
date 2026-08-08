//! Spec conformance for objects, arrays, and value shapes: identifier member names (including
//! escapes, `$`, `_`, and reserved words), trailing commas, and duplicate members.

use strata_json5_parser::{
    ArrayNode, Document, Json5Error, Json5Parser, Json5Token, JsonToken, ObjectNode, Value,
};

fn parse(input: &str) -> Result<Document, Json5Error> {
    Json5Parser::new().parse_str(input)
}

fn root_object(document: &Document) -> &ObjectNode {
    match document.root() {
        Value::Object(object) => object,
        other => panic!("expected an object root, got {other:?}"),
    }
}

fn root_array(document: &Document) -> &ArrayNode {
    match document.root() {
        Value::Array(array) => array,
        other => panic!("expected an array root, got {other:?}"),
    }
}

#[test]
fn empty_containers_parse() -> Result<(), Json5Error> {
    assert!(root_object(&parse("{}")?).members().is_empty());
    assert!(root_array(&parse("[]")?).elements().is_empty());

    Ok(())
}

#[test]
fn identifier_member_names_parse() -> Result<(), Json5Error> {
    let document = parse("{ name: 1, $loc: 2, _private: 3, x9: 4 }")?;
    let object = root_object(&document);
    let names: Vec<_> = object
        .members()
        .iter()
        .map(|member| {
            (
                member.name().kind,
                document.token_bytes(member.name()).unwrap_or_default(),
            )
        })
        .collect();

    assert_eq!(
        names,
        vec![
            (Json5Token::Identifier, &b"name"[..]),
            (Json5Token::Identifier, &b"$loc"[..]),
            (Json5Token::Identifier, &b"_private"[..]),
            (Json5Token::Identifier, &b"x9"[..]),
        ]
    );

    Ok(())
}

#[test]
fn escaped_identifier_member_names_parse() -> Result<(), Json5Error> {
    // abc == "abc" spelled with an escape; the raw bytes are retained undecoded.
    let document = parse("{ \\u0061bc: 1, ab\\u0063: 2 }")?;
    let object = root_object(&document);

    assert_eq!(object.members().len(), 2);
    let first = object
        .members()
        .first()
        .unwrap_or_else(|| panic!("first member"));
    assert_eq!(first.name().kind, Json5Token::Identifier);
    assert_eq!(document.token_bytes(first.name()), Some(&b"\\u0061bc"[..]));

    Ok(())
}

#[test]
fn unicode_identifier_member_names_parse() -> Result<(), Json5Error> {
    let document = parse("{ \u{00FC}ml\u{00E4}ut: 1, \u{4E2D}\u{6587}: 2 }")?;
    assert_eq!(root_object(&document).members().len(), 2);

    Ok(())
}

#[test]
fn reserved_words_are_valid_member_names() -> Result<(), Json5Error> {
    // IdentifierName (not Identifier) — reserved words and the number words included.
    let document = parse("{ true: 1, false: 2, null: 3, Infinity: 4, NaN: 5 }")?;
    let object = root_object(&document);
    let kinds: Vec<_> = object
        .members()
        .iter()
        .map(|member| member.name().kind)
        .collect();

    assert_eq!(
        kinds,
        vec![
            Json5Token::Json(JsonToken::True),
            Json5Token::Json(JsonToken::False),
            Json5Token::Json(JsonToken::Null),
            Json5Token::Infinity,
            Json5Token::NaN,
        ]
    );

    Ok(())
}

#[test]
fn string_member_names_parse_in_both_quote_kinds() -> Result<(), Json5Error> {
    let document = parse("{ \"a b\": 1, 'c d': 2 }")?;
    let object = root_object(&document);

    for member in object.members() {
        assert_eq!(member.name().kind, Json5Token::String);
    }
    assert_eq!(
        document.token_bytes(
            object
                .members()
                .get(1)
                .unwrap_or_else(|| panic!("second member"))
                .name()
        ),
        Some(&b"'c d'"[..])
    );

    Ok(())
}

#[test]
fn trailing_comma_in_object_is_retained() -> Result<(), Json5Error> {
    let document = parse("{ a: 1, b: 2, }")?;
    let object = root_object(&document);

    assert_eq!(object.members().len(), 2);
    // The trailing comma belongs to the last member, verbatim.
    let last = object
        .members()
        .last()
        .unwrap_or_else(|| panic!("last member"));
    assert!(last.comma().is_some());

    Ok(())
}

#[test]
fn trailing_comma_in_array_is_retained() -> Result<(), Json5Error> {
    let document = parse("[1, 2, 3,]")?;
    let array = root_array(&document);

    assert_eq!(array.elements().len(), 3);
    assert!(
        array
            .elements()
            .last()
            .unwrap_or_else(|| panic!("last element"))
            .comma()
            .is_some()
    );

    Ok(())
}

#[test]
fn duplicate_member_names_are_retained_in_source_order() -> Result<(), Json5Error> {
    // The spec only observes that receivers of non-unique names behave unpredictably; the CST
    // records all members verbatim and leaves the policy downstream.
    let document = parse("{ a: 1, a: 2, 'a': 3 }")?;
    let object = root_object(&document);

    assert_eq!(object.members().len(), 3);

    Ok(())
}

#[test]
fn every_value_kind_parses() -> Result<(), Json5Error> {
    let document = parse(
        "{ n: null, t: true, f: false, num: 1.5, hex: 0xFF, inf: Infinity, nan: NaN, \
         s: 'str', a: [1], o: { x: 1 } }",
    )?;
    let object = root_object(&document);
    let value_of = |index: usize| {
        object
            .members()
            .get(index)
            .unwrap_or_else(|| panic!("member"))
            .value()
    };

    assert!(matches!(value_of(0), Value::Null(_)));
    assert!(matches!(value_of(1), Value::Bool { value: true, .. }));
    assert!(matches!(value_of(2), Value::Bool { value: false, .. }));
    assert!(matches!(value_of(3), Value::Number(_)));
    assert!(matches!(value_of(4), Value::Number(_)));
    assert!(matches!(value_of(5), Value::Number(_)));
    assert!(matches!(value_of(6), Value::Number(_)));
    assert!(matches!(value_of(7), Value::String(_)));
    assert!(matches!(value_of(8), Value::Array(_)));
    assert!(matches!(value_of(9), Value::Object(_)));

    Ok(())
}

#[test]
fn any_value_is_a_valid_top_level_document() -> Result<(), Json5Error> {
    for input in [
        "null",
        "true",
        "42",
        "'lone'",
        "Infinity",
        "-Infinity",
        "NaN",
        "[]",
        "{}",
    ] {
        parse(input)?;
    }

    Ok(())
}

#[test]
fn nested_containers_parse() -> Result<(), Json5Error> {
    let document = parse("{ a: [ { b: [1, [2, { c: 3, }, ], ] } ] }")?;
    assert!(matches!(document.root(), Value::Object(_)));

    Ok(())
}
