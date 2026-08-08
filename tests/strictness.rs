//! Strictness: JSON5 grants no latitude beyond the spec, so everything JSON5 still forbids must
//! fail — with a position — even where other "JSON5-ish" parsers are lenient.

use strata_json5_parser::{Json5Error, Json5ErrorKind, Json5Parser};
use strata_lexer::LexErrorKind;

fn parse_error(input: &str) -> Json5Error {
    match Json5Parser::new().parse_str(input) {
        Err(error) => error,
        Ok(_) => panic!("expected {input:?} to be rejected"),
    }
}

fn assert_lex_error(input: &str, kind: fn(&LexErrorKind) -> bool, byte: u64) {
    let error = parse_error(input);
    match &error.kind {
        Json5ErrorKind::Lex(lex) => {
            assert!(kind(&lex.kind), "input {input:?}: {lex:?}");
            assert_eq!(lex.position.byte(), byte, "input {input:?}");
        }
        other => panic!("expected a lexical error for {input:?}, got {other:?}"),
    }
}

fn is_unrecognized(kind: &LexErrorKind) -> bool {
    matches!(kind, LexErrorKind::Unrecognized)
}

fn is_unterminated(kind: &LexErrorKind) -> bool {
    matches!(kind, LexErrorKind::UnterminatedDelimiter)
}

#[test]
fn leading_zeros_are_rejected() {
    assert_lex_error("08", is_unrecognized, 1);
    assert_lex_error("01", is_unrecognized, 1);
    assert_lex_error("-01", is_unrecognized, 2);
}

#[test]
fn hex_floats_are_rejected() {
    // The number token ends at `0x1`; the adjacent `.5` number is a parse error.
    let error = parse_error("0x1.5");
    assert!(matches!(error.kind, Json5ErrorKind::TrailingContent { .. }));
    assert_eq!(error.position.byte(), 3);

    let error = parse_error("[0x1.5]");
    assert!(matches!(error.kind, Json5ErrorKind::Expected { .. }));
    assert_eq!(error.position.byte(), 4);
}

#[test]
fn hex_exponents_and_signs_inside_are_rejected() {
    assert_lex_error("0x", is_unrecognized, 2);
    let error = parse_error("0x1p3");
    assert!(matches!(error.kind, Json5ErrorKind::TrailingContent { .. }));
}

#[test]
fn bare_signs_and_dots_are_rejected() {
    for input in ["+", "-", ".", "+.", "-."] {
        assert_lex_error(input, is_unrecognized, 0);
    }
}

#[test]
fn double_signs_are_rejected() {
    assert_lex_error("+-1", is_unrecognized, 0);
    assert_lex_error("++1", is_unrecognized, 0);
}

#[test]
fn signs_must_touch_their_number() {
    // `+ 1` — the sign alone matches nothing.
    assert_lex_error("+ 1", is_unrecognized, 0);
}

#[test]
fn empty_fractions_still_need_context() {
    for input in ["1e", "1e+", ".e3"] {
        let error = parse_error(input);
        assert!(
            matches!(error.kind, Json5ErrorKind::Lex(_)),
            "input {input:?}: {error:?}"
        );
    }
}

#[test]
fn lone_commas_in_containers_are_rejected() {
    // `[,]` — JSON5 has trailing commas, not elisions.
    let error = parse_error("[,]");
    assert!(matches!(
        error.kind,
        Json5ErrorKind::Expected {
            expected: "a JSON5 value",
            ..
        }
    ));
    assert_eq!(error.position.byte(), 1);

    let error = parse_error("{,}");
    assert!(matches!(error.kind, Json5ErrorKind::Expected { .. }));
}

#[test]
fn double_commas_are_rejected() {
    let error = parse_error("[1,,2]");
    assert!(matches!(error.kind, Json5ErrorKind::Expected { .. }));
    assert_eq!(error.position.byte(), 3);

    let error = parse_error("{a: 1,,}");
    assert!(matches!(error.kind, Json5ErrorKind::Expected { .. }));
}

#[test]
fn double_trailing_commas_are_rejected() {
    let error = parse_error("[1,2,,]");
    assert!(matches!(error.kind, Json5ErrorKind::Expected { .. }));
}

#[test]
fn identifiers_are_not_values() {
    for input in ["[a]", "{k: v}", "truex", "Infinityx", "nan"] {
        let error = parse_error(input);
        assert!(
            matches!(
                error.kind,
                Json5ErrorKind::Expected { .. } | Json5ErrorKind::TrailingContent { .. }
            ),
            "input {input:?}: {error:?}"
        );
    }
}

#[test]
fn uppercase_literals_are_rejected_as_values() {
    // `TRUE` / `NULL` are identifiers (member names at best), not literals.
    let error = parse_error("TRUE");
    assert!(matches!(error.kind, Json5ErrorKind::Expected { .. }));
}

#[test]
fn unterminated_constructs_are_rejected() {
    assert_lex_error("'open", is_unterminated, 0);
    assert_lex_error("\"open", is_unterminated, 0);
    assert_lex_error("'trailing\\", is_unterminated, 0);
    assert_lex_error("/* open", is_unterminated, 0);
    assert_lex_error("[1, /* open", is_unterminated, 4);
}

#[test]
fn invalid_escapes_are_rejected() {
    // Digit escapes 1-9 and \0 followed by a digit, per ES 5.1.
    assert_lex_error("'\\1'", is_unrecognized, 1);
    assert_lex_error("'\\09'", is_unrecognized, 1);
    // Truncated hex escapes.
    assert_lex_error("'\\x4'", is_unrecognized, 1);
    assert_lex_error("'\\u12'", is_unrecognized, 1);
    assert_lex_error("'\\u12G4'", is_unrecognized, 1);
}

#[test]
fn unescaped_line_terminators_in_strings_are_rejected() {
    assert_lex_error("'a\nb'", is_unrecognized, 2);
    assert_lex_error("'a\rb'", is_unrecognized, 2);
}

#[test]
fn json_structural_errors_remain_errors() {
    for input in [
        "{a}", "{a:}", "[1 2]", "{'a' 1}", "[1] 2", "{} {}", ":", "}",
    ] {
        let error = parse_error(input);
        assert!(
            !matches!(error.kind, Json5ErrorKind::DepthLimitExceeded { .. }),
            "input {input:?}"
        );
    }
}

#[test]
fn empty_and_trivia_only_inputs_are_rejected() {
    for input in ["", "   ", "// only a comment", "/* only */", "\u{FEFF}"] {
        let error = parse_error(input);
        assert!(
            matches!(error.kind, Json5ErrorKind::UnexpectedEof { .. }),
            "input {input:?}: {error:?}"
        );
    }
}

#[test]
fn nested_block_comments_are_rejected() {
    let error = parse_error("/* outer /* inner */ */ 1");
    assert!(matches!(error.kind, Json5ErrorKind::Lex(_)));
}

#[test]
fn error_positions_carry_line_and_column() {
    let error = parse_error("{\n  a: 1,\n  b @\n}");
    assert_eq!(error.position.line(), 3);
    // Bytes: line 3 starts at byte 10; `b` at column 3, ` ` 4, `@` at column 5.
    assert_eq!(error.position.column(), 5);
}
