//! JSON5 number features: hex integers, leading/trailing decimal points, explicit plus signs,
//! and `Infinity`/`NaN` with and without signs.

use strata_json5_parser::{Document, Json5Error, Json5Parser, Json5Token, Value};

fn parse(input: &str) -> Result<Document, Json5Error> {
    Json5Parser::new().parse_str(input)
}

/// Parses a lone number document, asserting the raw lexeme round-trips.
fn assert_number(input: &str) -> Json5Token {
    let document = match parse(input) {
        Ok(document) => document,
        Err(error) => panic!("expected {input:?} to parse: {error}"),
    };
    let Value::Number(token) = document.root() else {
        panic!("expected a number root for {input:?}");
    };

    assert_eq!(
        document.token_bytes(token),
        Some(input.as_bytes()),
        "input {input:?}"
    );
    token.kind
}

#[test]
fn hex_integers_parse() {
    for input in ["0x0", "0xdecaf", "0XABCDEF", "0x1f", "-0x1F", "+0xff"] {
        assert_eq!(assert_number(input), Json5Token::Number, "input {input:?}");
    }
}

#[test]
fn leading_and_trailing_decimal_points_parse() {
    for input in [".5", "-.5", "+.5", "1.", "-1.", "5.e2", ".5e-1", "0.", ".0"] {
        assert_eq!(assert_number(input), Json5Token::Number, "input {input:?}");
    }
}

#[test]
fn explicit_plus_signs_parse() {
    for input in ["+1", "+0", "+1.5e3", "+0x10"] {
        assert_eq!(assert_number(input), Json5Token::Number, "input {input:?}");
    }
}

#[test]
fn infinity_and_nan_parse_as_numbers() {
    // Bare words keep their classified kinds; signed forms are ordinary number tokens.
    assert_eq!(assert_number("Infinity"), Json5Token::Infinity);
    assert_eq!(assert_number("NaN"), Json5Token::NaN);

    for input in ["+Infinity", "-Infinity", "+NaN", "-NaN"] {
        assert_eq!(assert_number(input), Json5Token::Number, "input {input:?}");
    }
}

#[test]
fn json_numbers_still_parse() {
    for input in ["0", "-0", "42", "3.14", "1e10", "2.5E-3", "1e+5"] {
        assert_eq!(assert_number(input), Json5Token::Number, "input {input:?}");
    }
}

#[test]
fn numbers_work_as_member_values_and_elements() -> Result<(), Json5Error> {
    let document = parse("{ a: 0xC0FFEE, b: [.5, 2., +3, -Infinity, NaN,] }")?;
    let Value::Object(object) = document.root() else {
        panic!("expected an object");
    };
    let Value::Array(array) = object
        .members()
        .get(1)
        .unwrap_or_else(|| panic!("member b"))
        .value()
    else {
        panic!("expected an array");
    };

    assert_eq!(array.elements().len(), 5);

    Ok(())
}
