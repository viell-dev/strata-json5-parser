//! The strict JSON5 parser front: token rules in, CST out.
//!
//! [`Json5Parser::new`] is the strict path — and, unlike JSON, strict JSON5 needs no tolerance
//! switches: every JSON5 extension (identifier names, single quotes, comments, trailing commas,
//! hex numbers, ...) is a mandatory language feature, and a leading byte-order mark is ordinary
//! whitespace, so there is no `allow_bom`. The [spec]'s only parser latitude is the
//! implementation limit on nesting depth ([`Json5ParserBuilder::max_depth`], mirroring
//! `strata-json-parser`'s option and default) and the *should*-level warning for unescaped
//! U+2028/U+2029 in strings, which is always recorded on the parsed [`Document`] rather than
//! optioned. The spec's permission to "accept non-JSON5 forms or extensions" is deliberately not
//! exercised: per the ecosystem's strict-by-default policy, this parser accepts exactly JSON5.
//!
//! The grammar itself is `strata-json-parser`'s shared driver
//! ([`strata_json_parser::grammar`]); this module implements the JSON5
//! [`Syntax`] over it — comments and the expanded whitespace set become trivia at classification
//! time, identifier tokens are admitted in member-name position, `Infinity`/`NaN` take the number
//! role, and trailing commas are always allowed — and owns the JSON5-specific error type, the
//! post-parse warnings pass, and the public parser front.
//!
//! [spec]: https://spec.json5.org/

use crate::cst::{Document, TriviaKind, Warning, WarningKind};
use std::error::Error;
use std::fmt;
use std::io::{Cursor, Read};
use strata_json_parser::cst;
use strata_json_parser::grammar::{self, Lexed, Punct, Syntax, ValueRole};
use strata_json5_rules::{Json5Token, JsonToken, StrictRuleSet, strict_rule_set};
use strata_lexer::{LexError, LexerConfig, MatcherSet, Position, Token};

/// The default nesting-depth limit, shared with `strata-json-parser`.
///
/// The [spec] permits implementations to set limits on the maximum depth of nesting; this crate
/// uses the same default as `strata-json-parser` (128). Override it with
/// [`Json5ParserBuilder::max_depth`]. The limit also bounds the parser's recursion, so raising
/// it substantially trades stack headroom for depth.
///
/// [spec]: https://spec.json5.org/
pub const DEFAULT_MAX_DEPTH: usize = strata_json_parser::DEFAULT_MAX_DEPTH;

/// A parse failure, carrying the position at which it was observed.
#[derive(Debug)]
pub struct Json5Error {
    /// Position of the offending byte or token start.
    pub position: Position,
    /// What went wrong.
    pub kind: Json5ErrorKind,
}

/// Categories of parse failures.
#[derive(Debug)]
pub enum Json5ErrorKind {
    /// The lexer failed: I/O, an unrecognized byte, or a token-shape violation (invalid string
    /// escapes, unescaped line terminators in strings, number-grammar violations, unterminated
    /// strings or block comments, invalid UTF-8) reported at the offending byte.
    Lex(LexError),
    /// A token of an unexpected kind where the grammar required something else.
    Expected {
        /// What the grammar required here.
        expected: &'static str,
        /// The token kind actually found.
        found: Json5Token,
    },
    /// The input ended where the grammar required more.
    UnexpectedEof {
        /// What the grammar required here.
        expected: &'static str,
    },
    /// A significant token followed the complete top-level value.
    TrailingContent {
        /// The token kind found after the value.
        found: Json5Token,
    },
    /// Nesting exceeded the configured depth limit (a spec-permitted implementation limit).
    DepthLimitExceeded {
        /// The configured limit.
        limit: usize,
    },
}

impl fmt::Display for Json5ErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lex(error) => write!(formatter, "{error}"),
            Self::Expected { expected, found } => {
                write!(formatter, "expected {expected}, found {}", found.describe())
            }
            Self::UnexpectedEof { expected } => {
                write!(formatter, "unexpected end of input, expected {expected}")
            }
            Self::TrailingContent { found } => write!(
                formatter,
                "unexpected {} after the top-level value",
                found.describe()
            ),
            Self::DepthLimitExceeded { limit } => {
                write!(formatter, "nesting depth limit of {limit} exceeded")
            }
        }
    }
}

impl fmt::Display for Json5Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            // The lexer's message already carries its byte position.
            Json5ErrorKind::Lex(error) => write!(formatter, "{error}"),
            kind => write!(
                formatter,
                "{kind} at line {}, column {} (byte {})",
                self.position.line(),
                self.position.column(),
                self.position.byte()
            ),
        }
    }
}

impl Error for Json5Error {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.kind {
            Json5ErrorKind::Lex(error) => Some(error),
            _ => None,
        }
    }
}

impl From<LexError> for Json5Error {
    fn from(error: LexError) -> Self {
        Self {
            position: error.position,
            kind: Json5ErrorKind::Lex(error),
        }
    }
}

/// The JSON5 [`Syntax`]: one parse's classification policy over [`Json5Token`].
///
/// This is the whole superset, expressed as classification: comments and the expanded whitespace
/// set are trivia, `Infinity`/`NaN` play the number role, identifiers (and the word tokens,
/// which are valid ECMAScript 5.1 `IdentifierName`s) may name members, and trailing commas are
/// always permitted — mandatory language features, not options.
#[derive(Debug, Clone, Copy)]
struct Json5Syntax;

impl Syntax for Json5Syntax {
    type Lex = Json5Token;
    type Kind = Json5Token;
    type Trivia = TriviaKind;
    type Error = Json5Error;

    fn eof_lex(&self) -> Json5Token {
        Json5Token::Eof
    }

    fn classify(
        &self,
        token: &Token<Json5Token>,
    ) -> Result<Lexed<Json5Token, TriviaKind>, Json5Error> {
        match token.kind {
            Json5Token::Whitespace => Ok(Lexed::Trivia(TriviaKind::Whitespace)),
            Json5Token::LineComment => Ok(Lexed::Trivia(TriviaKind::LineComment)),
            Json5Token::BlockComment => Ok(Lexed::Trivia(TriviaKind::BlockComment)),
            Json5Token::Eof => Ok(Lexed::Eof),
            kind => Ok(Lexed::Significant(kind)),
        }
    }

    fn value_role(&self, kind: Json5Token) -> Option<ValueRole> {
        match kind {
            Json5Token::Json(JsonToken::LeftBrace) => Some(ValueRole::ObjectOpen),
            Json5Token::Json(JsonToken::LeftBracket) => Some(ValueRole::ArrayOpen),
            Json5Token::String => Some(ValueRole::String),
            Json5Token::Number | Json5Token::Infinity | Json5Token::NaN => Some(ValueRole::Number),
            Json5Token::Json(JsonToken::True) => Some(ValueRole::True),
            Json5Token::Json(JsonToken::False) => Some(ValueRole::False),
            Json5Token::Json(JsonToken::Null) => Some(ValueRole::Null),
            _ => None,
        }
    }

    fn punct(&self, kind: Json5Token) -> Option<Punct> {
        match kind {
            Json5Token::Json(JsonToken::Colon) => Some(Punct::Colon),
            Json5Token::Json(JsonToken::Comma) => Some(Punct::Comma),
            Json5Token::Json(JsonToken::RightBrace) => Some(Punct::ObjectClose),
            Json5Token::Json(JsonToken::RightBracket) => Some(Punct::ArrayClose),
            _ => None,
        }
    }

    /// The [spec] allows a string or an ECMAScript 5.1 `IdentifierName`. `IdentifierName`
    /// (unlike `Identifier`) includes reserved words, so the classified word tokens — `true`,
    /// `false`, `null`, `Infinity`, `NaN` — are valid names too.
    ///
    /// [spec]: https://spec.json5.org/
    fn is_member_name(&self, kind: Json5Token) -> bool {
        matches!(
            kind,
            Json5Token::Identifier
                | Json5Token::String
                | Json5Token::Infinity
                | Json5Token::NaN
                | Json5Token::Json(JsonToken::True | JsonToken::False | JsonToken::Null)
        )
    }

    fn value_expectation(&self) -> &'static str {
        "a JSON5 value"
    }

    fn member_name_expectation(&self) -> &'static str {
        "an object member name (an identifier or a string)"
    }

    /// JSON5: one trailing comma is allowed in a non-empty object or array — a mandatory
    /// language feature, not an option.
    fn allow_trailing_commas(&self) -> bool {
        true
    }

    fn expected(
        &self,
        position: Position,
        expected: &'static str,
        found: Json5Token,
    ) -> Json5Error {
        Json5Error {
            position,
            kind: Json5ErrorKind::Expected { expected, found },
        }
    }

    fn unexpected_eof(&self, position: Position, expected: &'static str) -> Json5Error {
        Json5Error {
            position,
            kind: Json5ErrorKind::UnexpectedEof { expected },
        }
    }

    fn trailing_content(&self, position: Position, found: Json5Token) -> Json5Error {
        Json5Error {
            position,
            kind: Json5ErrorKind::TrailingContent { found },
        }
    }

    fn depth_exceeded(&self, position: Position, limit: usize) -> Json5Error {
        Json5Error {
            position,
            kind: Json5ErrorKind::DepthLimitExceeded { limit },
        }
    }
}

/// A reusable, strict JSON5 parser.
///
/// `M` is the token rule set handed to the lexer for every parse; it defaults to the strict
/// JSON5 rules. The parser clones the rules into a fresh scanner per [`Json5Parser::parse`]
/// call, so one configured parser can serve many inputs.
#[derive(Debug, Clone)]
pub struct Json5Parser<M = StrictRuleSet> {
    rules: M,
    max_depth: usize,
    lexer_config: LexerConfig,
}

impl Json5Parser<StrictRuleSet> {
    /// Creates a strict JSON5 parser with [`DEFAULT_MAX_DEPTH`] nesting.
    pub fn new() -> Self {
        Self {
            rules: strict_rule_set(),
            max_depth: DEFAULT_MAX_DEPTH,
            lexer_config: LexerConfig::new(),
        }
    }

    /// Starts a builder for the spec-permitted depth limit or for replacing lower layers.
    pub fn builder() -> Json5ParserBuilder<StrictRuleSet> {
        Json5ParserBuilder::new()
    }
}

impl Default for Json5Parser<StrictRuleSet> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M> Json5Parser<M>
where
    M: MatcherSet<Kind = Json5Token> + Clone,
{
    /// Parses a complete JSON5 text from `reader` into a [`Document`].
    ///
    /// Any JSON5 value is a valid top-level document. The entire input must be consumed:
    /// trailing content other than whitespace and comments is an error. On success the consumed
    /// source bytes are retained in the document's source store, keeping the CST zero-copy, and
    /// the spec-mandated warnings are recorded on the document ([`Document::warnings`]).
    pub fn parse<R: Read>(&self, reader: R) -> Result<Document, Json5Error> {
        let inner = grammar::parse_document(
            reader,
            self.rules.clone(),
            &Json5Syntax,
            self.max_depth,
            self.lexer_config,
        )?;
        let warnings = collect_warnings(&inner);

        Ok(Document::new(inner, warnings))
    }

    /// Parses a complete JSON5 text from a string slice.
    pub fn parse_str(&self, input: &str) -> Result<Document, Json5Error> {
        self.parse(Cursor::new(input.as_bytes()))
    }

    /// Parses a complete JSON5 text from a byte slice. The bytes must be UTF-8; violations
    /// inside strings, comments, and identifiers are lexical errors, and any other non-ASCII
    /// byte outside JSON5 whitespace matches no rule.
    pub fn parse_bytes(&self, input: &[u8]) -> Result<Document, Json5Error> {
        self.parse(Cursor::new(input))
    }

    /// The token rule set handed to the lexer at each parse.
    pub const fn rules(&self) -> &M {
        &self.rules
    }

    /// The configured nesting-depth limit.
    pub const fn max_depth(&self) -> usize {
        self.max_depth
    }
}

/// Builder for [`Json5Parser`], strict by default (there is no other mode: the only knobs are
/// the spec-permitted depth limit and the lower-layer replacement hooks).
#[derive(Debug, Clone)]
pub struct Json5ParserBuilder<M = StrictRuleSet> {
    rules: M,
    max_depth: usize,
    lexer_config: LexerConfig,
}

impl Json5ParserBuilder<StrictRuleSet> {
    /// Starts from the strict defaults.
    pub fn new() -> Self {
        Self {
            rules: strict_rule_set(),
            max_depth: DEFAULT_MAX_DEPTH,
            lexer_config: LexerConfig::new(),
        }
    }
}

impl Default for Json5ParserBuilder<StrictRuleSet> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M> Json5ParserBuilder<M> {
    /// Replaces the token rule set handed to the lexer.
    ///
    /// This is the rules-layer composition point: tests can mock the rules, and adjusted rule
    /// sets can be swapped in without touching the grammar driver. The set must still produce
    /// [`Json5Token`] kinds; a superset *language* instead implements its own
    /// [`Syntax`](strata_json_parser::grammar::Syntax) over its own kinds, reusing the rule
    /// structs from `strata-json5-rules` via
    /// [`MatcherExt::map_kind`](strata_lexer::MatcherExt::map_kind) — exactly as that crate
    /// reuses `strata-json-rules`'s.
    pub fn with_rules<N>(self, rules: N) -> Json5ParserBuilder<N>
    where
        N: MatcherSet<Kind = Json5Token> + Clone,
    {
        Json5ParserBuilder {
            rules,
            max_depth: self.max_depth,
            lexer_config: self.lexer_config,
        }
    }

    /// Replaces the lexer configuration (reader capacities, position tracking, ...).
    ///
    /// The parser always disables buffer compaction (`compact_after = None`) at parse time: the
    /// consumed bytes must survive until they are parked into the document's source store.
    #[must_use]
    pub const fn with_lexer_config(mut self, config: LexerConfig) -> Self {
        self.lexer_config = config;
        self
    }

    /// Sets the nesting-depth limit.
    ///
    /// The [spec] permits implementations to limit the maximum depth of nesting. The limit
    /// counts nested arrays/objects — with a limit of 1, `[1]` parses and `[[1]]` does not. It
    /// also bounds the parser's recursion depth; see [`DEFAULT_MAX_DEPTH`].
    ///
    /// [spec]: https://spec.json5.org/
    #[must_use]
    pub const fn max_depth(mut self, limit: usize) -> Self {
        self.max_depth = limit;
        self
    }

    /// Finishes the builder.
    pub fn build(self) -> Json5Parser<M>
    where
        M: MatcherSet<Kind = Json5Token> + Clone,
    {
        Json5Parser {
            rules: self.rules,
            max_depth: self.max_depth,
            lexer_config: self.lexer_config,
        }
    }
}

/// Walks every string token of the finished tree and records the spec-mandated warnings for
/// unescaped U+2028/U+2029 (see [`WarningKind::UnescapedLineSeparator`]).
fn collect_warnings(document: &cst::Document<Json5Token, TriviaKind>) -> Vec<Warning> {
    let mut warnings = Vec::new();

    document.root().for_each_token(&mut |token| {
        if token.kind == Json5Token::String {
            if let Some(bytes) = document.token_bytes(token) {
                scan_string_warnings(bytes, token.start(), &mut warnings);
            }
        }
    });

    warnings
}

/// Scans one string lexeme (quotes included, already validated by the string rule) for raw
/// U+2028/U+2029 characters, escape-aware: a separator directly preceded by a backslash is a
/// line continuation, not an unescaped occurrence.
fn scan_string_warnings(bytes: &[u8], start: Position, warnings: &mut Vec<Warning>) {
    let Ok(text) = core::str::from_utf8(bytes) else {
        // The string rule already validated the lexeme; nothing to scan otherwise.
        return;
    };

    let mut escaped = false;

    for (index, ch) in text.char_indices() {
        if escaped {
            // The character after a backslash is escape content — including a second backslash
            // (`\\`), which does *not* escape what follows it.
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if matches!(ch, '\u{2028}' | '\u{2029}') {
            warnings.push(Warning {
                position: advance_position(start, bytes.get(..index).unwrap_or_default()),
                kind: WarningKind::UnescapedLineSeparator { separator: ch },
            });
        }
    }
}

/// Advances `start` over `bytes`, mirroring the lexer core's tracking exactly: the column
/// advances one per byte, `\r`, `\n`, and `\r\n` each count as one line break, and U+2028/U+2029
/// do **not** advance the line (the core is byte-oriented; line/column are diagnostics).
fn advance_position(start: Position, bytes: &[u8]) -> Position {
    let mut byte = start.byte();
    let mut line = start.line();
    let mut column = start.column();
    let mut previous_was_cr = false;

    for &next in bytes {
        byte = byte.saturating_add(1);

        match next {
            b'\r' => {
                line = line.saturating_add(1);
                column = 1;
                previous_was_cr = true;
            }
            b'\n' if previous_was_cr => {
                previous_was_cr = false;
            }
            b'\n' => {
                line = line.saturating_add(1);
                column = 1;
                previous_was_cr = false;
            }
            _ => {
                column = column.saturating_add(1);
                previous_was_cr = false;
            }
        }
    }

    // `line` and `column` start one-based (they come from a valid `Position`) and are only ever
    // increased or reset to one above, so `Position::new` cannot observe a zero component; the
    // fallback merely keeps the impossible error arm total without panicking.
    Position::new(byte, line, column)
        .unwrap_or_else(|_| start.advanced_by(u64::try_from(bytes.len()).unwrap_or(u64::MAX)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_matches_the_builder_defaults() {
        let parser = Json5Parser::new();
        assert_eq!(parser.max_depth(), DEFAULT_MAX_DEPTH);

        let built = Json5Parser::builder().build();
        assert_eq!(built.max_depth(), DEFAULT_MAX_DEPTH);
    }

    #[test]
    fn builder_sets_the_depth_limit() {
        let parser = Json5Parser::builder().max_depth(4).build();
        assert_eq!(parser.max_depth(), 4);
    }

    #[test]
    fn error_display_includes_position() {
        let Err(error) = Json5Parser::new().parse_str("[1 2]") else {
            panic!("expected a parse error");
        };
        let message = error.to_string();
        assert!(message.contains("expected ',' or ']'"), "{message}");
        assert!(message.contains("line 1"), "{message}");
        assert!(message.contains("byte 3"), "{message}");
    }

    #[test]
    fn member_name_error_names_the_expectation() {
        let Err(error) = Json5Parser::new().parse_str("{1: 2}") else {
            panic!("expected a parse error");
        };
        let message = error.to_string();
        assert!(message.contains("member name"), "{message}");
        assert!(message.contains("found a number"), "{message}");
    }

    #[test]
    fn depth_limit_counts_containers() {
        let parser = Json5Parser::builder().max_depth(1).build();
        assert!(parser.parse_str("[1]").is_ok());

        let Err(error) = parser.parse_str("[[1]]") else {
            panic!("expected a depth error");
        };
        assert!(matches!(
            error.kind,
            Json5ErrorKind::DepthLimitExceeded { limit: 1 }
        ));
    }

    #[test]
    fn warning_scan_is_escape_aware() {
        let mut warnings = Vec::new();
        // `'a<U+2028>b'` — raw separator: one warning.
        let raw = "'a\u{2028}b'";
        scan_string_warnings(raw.as_bytes(), Position::START, &mut warnings);
        assert_eq!(warnings.len(), 1);
        assert_eq!(
            warnings.first().map(|warning| warning.position.byte()),
            Some(2)
        );

        // `'a\<U+2028>b'` — line continuation: no warning.
        let mut warnings = Vec::new();
        let continued = "'a\\\u{2028}b'";
        scan_string_warnings(continued.as_bytes(), Position::START, &mut warnings);
        assert!(warnings.is_empty());

        // `'\\<U+2028>'` — escaped backslash, then a *raw* separator: one warning.
        let mut warnings = Vec::new();
        let double = "'\\\\\u{2028}'";
        scan_string_warnings(double.as_bytes(), Position::START, &mut warnings);
        assert_eq!(warnings.len(), 1);
    }
}
