//! JSON5 (json5.org) parser for the Strata parser ecosystem.
//!
//! `strata-json5-parser` is a strict [JSON5](https://spec.json5.org/) parser composed on
//! `strata-json-parser`: a byte stream in, a lossless, zero-copy concrete syntax tree (CST) out.
//! Every token keeps its absolute byte span, trivia — whitespace runs, `//` and `/* */`
//! comments, blank lines — is retained instead of discarded, and string/number content stays as
//! raw source bytes. Decoding (escape resolution, numeric conversion) belongs to a downstream
//! resolver or deserializer, served by this crate's [`decode`] module.
//!
//! The language is JSON5 exactly: every JSON5 extension (unquoted `IdentifierName` member names,
//! single-quoted strings, line continuations and the extra escapes, hex numbers,
//! `Infinity`/`NaN`, explicit `+`, leading/trailing decimal points, trailing commas, comments,
//! the expanded whitespace set with the byte-order mark as whitespace) is a mandatory language
//! feature, not an option — and nothing beyond JSON5 is accepted, per the ecosystem's
//! strict-by-default policy. The one caveat is Unicode identifier classification, which
//! approximates the ES 5.1 category sets using `std` (exact for ASCII and `\uXXXX` escapes); see
//! `strata-json5-rules`' identifier rule for the precise deviations.
//!
//! The crate does not mirror the JSON parser; it instantiates it. The token rules come from
//! `strata-json5-rules`, the tree is `strata-json-parser`'s generic CST
//! ([`strata_json_parser::cst`]) with `K = `[`Json5Token`] and `T = `[`TriviaKind`], and the
//! grammar is the shared driver ([`strata_json_parser::grammar`]) behind a private JSON5
//! [`Syntax`](strata_json_parser::grammar::Syntax) implementation. What remains here is the
//! public front: [`Json5Parser`], strict via [`Json5Parser::new`], with a builder for the
//! spec-permitted nesting-depth limit and the lower-layer replacement hooks; the [`Document`]
//! wrapper carrying the spec-mandated [`Warning`]s (unescaped U+2028/U+2029 in strings); the
//! JSON5 error type; and the [`decode`] module — JSON5-shaped string unescaping, member-name
//! decoding, and number conversion built on the JSON parser's shared decode primitives.
//!
//! # Example
//!
//! ```
//! use strata_json5_parser::{Json5Parser, Json5Token, Value};
//!
//! # fn main() -> Result<(), strata_json5_parser::Json5Error> {
//! let document = Json5Parser::new().parse_str(
//!     "{\n  // package metadata\n  name: 'strata', versions: [0x1, .5, +Infinity,],\n}",
//! )?;
//!
//! let Value::Object(object) = document.root() else {
//!     panic!("expected an object");
//! };
//! let member = object.members().first().unwrap();
//!
//! // Unquoted member name, with its comment retained as leading trivia.
//! assert_eq!(member.name().kind, Json5Token::Identifier);
//! assert_eq!(document.token_bytes(member.name()), Some(&b"name"[..]));
//! assert_eq!(member.name().leading().len(), 3); // whitespace, comment, whitespace
//!
//! // Zero-copy: raw source bytes, quotes included, no decoding.
//! assert_eq!(
//!     document.token_bytes(member.value().first_token()),
//!     Some(&b"'strata'"[..])
//! );
//! # Ok(())
//! # }
//! ```

mod cst;
pub mod decode;
mod parse;

pub use cst::{
    ArrayNode, Document, Element, Member, ObjectNode, SyntaxToken, Trivia, TriviaKind, Value,
    Warning, WarningKind,
};
pub use parse::{DEFAULT_MAX_DEPTH, Json5Error, Json5ErrorKind, Json5Parser, Json5ParserBuilder};

// Re-exported so downstream code can name the kinds carried inside the CST and share or extend
// rule sets without depending on `strata-json5-rules` (or `strata-json-rules`) directly.
// `SharedRuleSet` is kind-generic upstream, so it wraps JSON5 rule sets too.
pub use strata_json5_rules::{
    BlockCommentRule, IdentifierRule, Json5Token, JsonToken, LineCommentRule, LiteralRule,
    NumberRule, PunctRule, SharedRuleSet, StrictRuleSet, StringRule, WhitespaceRule,
    is_json5_whitespace, strict_rule_set, strict_scanner,
};

/// Compile-checks the README examples alongside the doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
mod readme {}
