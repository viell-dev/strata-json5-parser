//! The spec-permitted depth limit and the lower-layer builder hooks (`with_rules`,
//! `with_lexer_config`).

use strata_json5_parser::{
    BlockCommentRule, DEFAULT_MAX_DEPTH, IdentifierRule, Json5Error, Json5ErrorKind, Json5Parser,
    Json5Token, JsonToken, LineCommentRule, LiteralRule, NumberRule, PunctRule, SharedRuleSet,
    StringRule, Value, WhitespaceRule,
};
use strata_lexer::{LexerConfig, MatcherExt, MatcherVec};

#[test]
fn default_depth_limit_matches_strata_json_parser() {
    assert_eq!(DEFAULT_MAX_DEPTH, strata_json_parser::DEFAULT_MAX_DEPTH);
    assert_eq!(Json5Parser::new().max_depth(), DEFAULT_MAX_DEPTH);
}

#[test]
fn depth_limit_is_enforced_at_the_boundary() -> Result<(), Json5Error> {
    let parser = Json5Parser::builder().max_depth(2).build();

    parser.parse_str("[{ a: 1 }]")?;

    let Err(error) = parser.parse_str("[[[1]]]") else {
        panic!("expected a depth error");
    };
    assert!(matches!(
        error.kind,
        Json5ErrorKind::DepthLimitExceeded { limit: 2 }
    ));
    // Reported at the offending open bracket.
    assert_eq!(error.position.byte(), 2);

    Ok(())
}

#[test]
fn deep_documents_parse_up_to_the_default_limit() -> Result<(), Json5Error> {
    let mut input = String::new();
    for _ in 0..DEFAULT_MAX_DEPTH {
        input.push('[');
    }
    input.push('1');
    for _ in 0..DEFAULT_MAX_DEPTH {
        input.push(']');
    }

    Json5Parser::new().parse_str(&input)?;

    let overflowing = format!("[{input}]");
    assert!(matches!(
        Json5Parser::new().parse_str(&overflowing),
        Err(Json5Error {
            kind: Json5ErrorKind::DepthLimitExceeded { .. },
            ..
        })
    ));

    Ok(())
}

#[test]
fn with_lexer_config_accepts_tiny_buffers() -> Result<(), Json5Error> {
    // A 2-byte initial buffer forces growth across a long token.
    let config = LexerConfig::new().with_reader_initial_capacity(2);
    let parser = Json5Parser::builder().with_lexer_config(config).build();

    let long = format!("'{}'", "x".repeat(300));
    let document = parser.parse_str(&long)?;
    assert!(matches!(document.root(), Value::String(_)));

    Ok(())
}

#[test]
fn with_rules_accepts_a_shared_runtime_rule_set() -> Result<(), Json5Error> {
    // The same strict grammar assembled as a runtime `MatcherVec`, shared via `SharedRuleSet`
    // (the parser clones its rules per parse). This is the mocking / adjusted-rules seam.
    let wrap: fn(JsonToken) -> Json5Token = Json5Token::Json;
    let rules: MatcherVec<Json5Token> = vec![
        WhitespaceRule.boxed(),
        PunctRule.map_kind(wrap).boxed(),
        LiteralRule.map_kind(wrap).boxed(),
        IdentifierRule.boxed(),
        StringRule.boxed(),
        NumberRule.boxed(),
        LineCommentRule.boxed(),
        BlockCommentRule.boxed(),
    ];
    let parser = Json5Parser::builder()
        .with_rules(SharedRuleSet::new(rules))
        .build();

    // Two parses from one parser: the shared set clones cheaply.
    parser.parse_str("{ a: 0x1, } // ok")?;
    let document = parser.parse_str("[Infinity, 'two']")?;
    assert!(matches!(document.root(), Value::Array(_)));

    Ok(())
}
