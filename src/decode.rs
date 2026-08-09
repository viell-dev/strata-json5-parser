//! Lexeme decoding for resolver layers: string unescaping, member-name decoding, and number
//! conversion.
//!
//! The parser itself never decodes — the CST keeps raw spans. This module owns the inverse of
//! the token shapes `strata-json5-rules` validates, mirroring `strata-json-parser`'s [`decode`]
//! module in role and style: resolver crates share one implementation instead of each rewriting
//! escape walking and number classification. The pieces JSON5 shares with JSON — the
//! lone-surrogate policy, the error type, the UTF-16 pairing decoder, and the [`Number`]
//! classification — are reused from `strata-json-parser` and re-exported here, so a JSON5
//! resolver needs no direct `strata-json-parser` dependency.
//!
//! The functions are total over arbitrary input: tokens produced by this crate's parser have
//! already had their shape validated and cannot fail with [`ErrorKind::MalformedToken`], but the
//! functions are public and re-check shape instead of trusting their caller.
//!
//! [`decode`]: strata_json_parser::decode

use strata_json_parser::decode::{LoneSurrogate, Utf16Decoder, Utf16Step};

pub use strata_json_parser::decode::{Error, ErrorKind, LoneSurrogates, Number};

/// The written shape of an object member name, derivable from the raw lexeme's first byte.
///
/// The CST records member names as tokens whose kind distinguishes strings from identifiers;
/// this enum lets a resolver holding only the raw bytes recover the shape (e.g. to preserve the
/// author's quoting style) before handing them to [`decode_member_name`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NameShape {
    /// A double-quoted string name.
    DoubleQuoted,
    /// A single-quoted string name.
    SingleQuoted,
    /// An unquoted `IdentifierName` (including the reserved and number words).
    Identifier,
}

/// Classifies a raw member-name lexeme by its first byte.
///
/// Quote bytes cannot begin an identifier, so the first byte decides: `"` is
/// [`NameShape::DoubleQuoted`], `'` is [`NameShape::SingleQuoted`], and anything else (including
/// an empty slice) is [`NameShape::Identifier`]. The classification is purely syntactic; the
/// bytes are not otherwise validated.
pub const fn member_name_shape(raw: &[u8]) -> NameShape {
    match raw.first() {
        Some(b'"') => NameShape::DoubleQuoted,
        Some(b'\'') => NameShape::SingleQuoted,
        _ => NameShape::Identifier,
    }
}

/// Decodes a raw JSON5 string lexeme — both quotes included, single- or double-quoted — into
/// its owned text.
///
/// Handles the full ECMAScript 5.1 escape repertoire the string rule admits:
///
/// - the single-escape characters `\' \" \\ \b \f \n \r \t \v`;
/// - `\0` (U+0000), malformed when followed by a decimal digit;
/// - `\xHH` and `\uHHHH` hex escapes, the latter with UTF-16 surrogate-pair combination via the
///   shared [`Utf16Decoder`] — lone surrogates resolve per `policy`, and a pair split by a line
///   continuation still combines (the continuation contributes nothing, leaving the code units
///   adjacent, exactly as in ECMAScript);
/// - line continuations — a backslash before LF, CR, CRLF, U+2028, or U+2029 — which produce no
///   output;
/// - any other escaped character (a `NonEscapeCharacter`, e.g. `\q`, `\/`, or a multibyte
///   character) escaping itself. Decimal digits `1`–`9` after a backslash have no production and
///   are malformed.
///
/// Errors carry the byte offset inside `raw` of the backslash of the offending escape.
pub fn unescape_string(raw: &[u8], policy: LoneSurrogates) -> Result<String, Error> {
    let text = validated_quoted(raw)?;
    let inner_end = raw.len().saturating_sub(1);
    let mut walker = EscapeWalker::new(text, policy);
    let mut index = 1; // past the opening quote

    while index < inner_end {
        if raw.get(index).copied() != Some(b'\\') {
            index = index.saturating_add(1);
            continue;
        }

        walker.take_run_until(index)?;

        let escape_offset = index;
        let malformed = Error {
            offset: escape_offset,
            kind: ErrorKind::MalformedToken,
        };
        let escaped = text
            .get(index.saturating_add(1)..)
            .and_then(|rest| rest.chars().next())
            .ok_or(malformed)?;

        match escaped {
            // LineContinuation: no output. CR followed by LF is one terminator sequence.
            '\r' => {
                index = index.saturating_add(2);

                if raw.get(index).copied() == Some(b'\n') {
                    index = index.saturating_add(1);
                }
            }
            '\n' | '\u{2028}' | '\u{2029}' => {
                index = index.saturating_add(1).saturating_add(escaped.len_utf8());
            }
            // \0 — the ES 5.1 EscapeSequence production `0 [lookahead not DecimalDigit]`.
            '0' => {
                index = index.saturating_add(2);

                if matches!(raw.get(index), Some(b'0'..=b'9')) {
                    return Err(malformed);
                }

                walker.push_char('\u{0}')?;
            }
            // Decimal digits 1-9 are EscapeCharacters with no production: always malformed.
            '1'..='9' => return Err(malformed),
            'x' => {
                let value = parse_hex(raw, index.saturating_add(2), 2).ok_or(malformed)?;
                let ch = char::from_u32(value).ok_or(malformed)?;
                index = index.saturating_add(4);
                walker.push_char(ch)?;
            }
            'u' => {
                let value = parse_hex(raw, index.saturating_add(2), 4).ok_or(malformed)?;
                let unit = u16::try_from(value).map_err(|_| malformed)?;
                index = index.saturating_add(6);
                walker.push_unit(unit, escape_offset)?;
            }
            // SingleEscapeCharacter or NonEscapeCharacter: one source character either way.
            other => {
                index = index.saturating_add(1).saturating_add(other.len_utf8());
                walker.push_char(simple_escape(other))?;
            }
        }

        if index > inner_end {
            // The escape swallowed the closing quote (e.g. `'\'`): the lexeme is not a
            // well-formed string token.
            return Err(malformed);
        }

        walker.resume_run_at(index);
    }

    walker.take_run_until(inner_end)?;
    walker.finish()
}

/// Maps a single-escape character to its value; every other character escapes itself
/// (`NonEscapeCharacter`).
const fn simple_escape(ch: char) -> char {
    match ch {
        'b' => '\u{08}',
        'f' => '\u{0C}',
        'n' => '\n',
        'r' => '\r',
        't' => '\t',
        'v' => '\u{0B}',
        other => other,
    }
}

/// Decodes a raw JSON5 member-name lexeme into its owned text.
///
/// Quoted names (either quote kind, see [`member_name_shape`]) route through
/// [`unescape_string`]. Identifier names decode their `\uHHHH` escapes — `\u0061bc` and `abc`
/// denote the same name — where each escape stands for exactly one character: ES 5.1 identifier
/// escapes carry no surrogate pairing, so an escape denoting a surrogate code unit is
/// [`ErrorKind::MalformedToken`] (the identifier rule never produces one), and `policy` applies
/// only to the quoted route. Identifier characters are not otherwise re-validated against the
/// `IdentifierName` character classes.
#[expect(
    clippy::module_name_repetitions,
    reason = "the resolver-facing API imports the function directly, where the `decode_` verb \
              carries the meaning; `decode::member_name` would read as an accessor"
)]
pub fn decode_member_name(raw: &[u8], policy: LoneSurrogates) -> Result<String, Error> {
    match member_name_shape(raw) {
        NameShape::DoubleQuoted | NameShape::SingleQuoted => unescape_string(raw, policy),
        NameShape::Identifier => decode_identifier(raw),
    }
}

/// Decodes an unquoted identifier lexeme, resolving `\uHHHH` escapes.
fn decode_identifier(raw: &[u8]) -> Result<String, Error> {
    if raw.is_empty() {
        return Err(Error {
            offset: 0,
            kind: ErrorKind::MalformedToken,
        });
    }

    let text = validated_utf8(raw)?;
    let mut out = String::with_capacity(text.len());
    let mut index = 0;

    while index < raw.len() {
        if raw.get(index).copied() == Some(b'\\') {
            let malformed = Error {
                offset: index,
                kind: ErrorKind::MalformedToken,
            };

            if raw.get(index.saturating_add(1)).copied() != Some(b'u') {
                return Err(malformed);
            }

            let value = parse_hex(raw, index.saturating_add(2), 4).ok_or(malformed)?;
            // Surrogate code units do not denote scalar values; `char::from_u32` rejects them.
            out.push(char::from_u32(value).ok_or(malformed)?);
            index = index.saturating_add(6);
            continue;
        }

        let run_start = index;

        while index < raw.len() && raw.get(index).copied() != Some(b'\\') {
            index = index.saturating_add(1);
        }

        // Backslash bytes never occur inside multibyte UTF-8 sequences, so both bounds sit on
        // character boundaries.
        let run = text.get(run_start..index).ok_or(Error {
            offset: run_start,
            kind: ErrorKind::MalformedToken,
        })?;
        out.push_str(run);
    }

    Ok(out)
}

/// The written form of a JSON5 number lexeme — author-intent fidelity for the resolved model.
///
/// The classification says how the author spelled the number; the converted value lives in
/// [`Number`] and the raw bytes stay reachable through the CST span, so nothing needs to be
/// re-stringified. Signs do not affect the classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NumberFormat {
    /// Decimal digits only: `42`, `+42`, `-17`.
    Integer,
    /// A decimal point and no exponent, including the leading/trailing-point forms: `3.25`,
    /// `.5`, `5.`.
    Decimal,
    /// An exponent part: `1.23e+5`, `1E9`.
    Exponential,
    /// A hexadecimal integer: `0x1F`, `-0X10`.
    Hex,
    /// The `Infinity` word, either sign.
    Infinity,
    /// The `NaN` word, either sign.
    NaN,
}

/// A decoded JSON5 number lexeme: the converted value and the written form it came from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecodedNumber {
    /// The converted value, integer-preserving where it fits.
    pub value: Number,
    /// The written form of the lexeme.
    pub format: NumberFormat,
}

/// Converts a raw JSON5 number lexeme into a [`DecodedNumber`].
///
/// Handles every shape the number rule admits, plus the bare `Infinity`/`NaN` words (which lex
/// as their own token kinds but are number values):
///
/// - An optional leading `+` or `-` on every form.
/// - `Infinity` and `NaN` convert to `f64` infinities and NaN, the sign preserved (a negative
///   NaN sets the sign bit).
/// - Hex integers (`0x`/`0X`) convert by magnitude: non-negative values fitting `u64` are
///   [`Number::U64`], negative values fitting `i64` are [`Number::I64`], and anything larger is
///   an `f64` approximation — which may round, and beyond `f64` range becomes infinite.
/// - Decimal forms keep integer identity where the value is integral and unsigned-decimal
///   shaped (`+42` is `U64(42)`); fractions, exponents, and leading/trailing decimal points
///   (`.5`, `5.`) convert to `f64`.
/// - `-0` and `-0x0` convert to `F64(-0.0)`, preserving the sign.
///
/// The written form is classified alongside; the raw text stays available through the CST for
/// consumers needing arbitrary precision.
pub fn parse_number(raw: &[u8]) -> Result<DecodedNumber, Error> {
    let text = validated_utf8(raw)?;
    let malformed = Error {
        offset: 0,
        kind: ErrorKind::MalformedToken,
    };

    let (negative, magnitude) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };

    match magnitude {
        "Infinity" => {
            let value = Number::F64(if negative {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            });

            return Ok(DecodedNumber {
                value,
                format: NumberFormat::Infinity,
            });
        }
        "NaN" => {
            return Ok(DecodedNumber {
                value: Number::F64(if negative { -f64::NAN } else { f64::NAN }),
                format: NumberFormat::NaN,
            });
        }
        _ => {}
    }

    if let Some(hex) = magnitude
        .strip_prefix("0x")
        .or_else(|| magnitude.strip_prefix("0X"))
    {
        let value = parse_hex_number(hex, negative).ok_or(malformed)?;

        return Ok(DecodedNumber {
            value,
            format: NumberFormat::Hex,
        });
    }

    if magnitude
        .bytes()
        .any(|byte| matches!(byte, b'.' | b'e' | b'E'))
    {
        let format = if magnitude.bytes().any(|byte| matches!(byte, b'e' | b'E')) {
            NumberFormat::Exponential
        } else {
            NumberFormat::Decimal
        };
        let value = text
            .parse::<f64>()
            .map(Number::F64)
            .map_err(|_| malformed)?;

        return Ok(DecodedNumber { value, format });
    }

    if magnitude.is_empty() || !magnitude.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(malformed);
    }

    let format = NumberFormat::Integer;

    if negative {
        let value = match text.parse::<i64>() {
            // `-0` preserves its sign as a float; parsing the text keeps the minus.
            Ok(0) => text
                .parse::<f64>()
                .map(Number::F64)
                .map_err(|_| malformed)?,
            Ok(value) => Number::I64(value),
            // Negative magnitude beyond i64: fall back to f64.
            Err(_) => text
                .parse::<f64>()
                .map(Number::F64)
                .map_err(|_| malformed)?,
        };

        return Ok(DecodedNumber { value, format });
    }

    let value = match magnitude.parse::<u64>() {
        Ok(value) => Number::U64(value),
        // Magnitude beyond u64: fall back to f64.
        Err(_) => magnitude
            .parse::<f64>()
            .map(Number::F64)
            .map_err(|_| malformed)?,
    };

    Ok(DecodedNumber { value, format })
}

/// The magnitude of `i64::MIN`, the one negative value whose magnitude does not fit `i64`.
const I64_MIN_MAGNITUDE: u128 = 1 << 63;

/// Converts validated-shape hex digits into a [`Number`] by magnitude; `None` if the digits are
/// empty or not hexadecimal.
fn parse_hex_number(digits: &str, negative: bool) -> Option<Number> {
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }

    if let Ok(magnitude) = u128::from_str_radix(digits, 16) {
        return Some(classify_hex_magnitude(magnitude, negative));
    }

    // Magnitude beyond u128: fold an f64 approximation directly from the digits.
    let approximation = digits.bytes().fold(0.0_f64, |value, byte| {
        let digit = char::from(byte).to_digit(16).unwrap_or_default();
        value * 16.0 + f64::from(digit)
    });

    Some(Number::F64(if negative {
        -approximation
    } else {
        approximation
    }))
}

/// Classifies a hex magnitude that fits `u128`, integer-preserving where the value fits.
fn classify_hex_magnitude(magnitude: u128, negative: bool) -> Number {
    #[expect(
        clippy::cast_precision_loss,
        clippy::as_conversions,
        reason = "documented rounding approximation for magnitudes beyond 64 bits"
    )]
    let approximate = |magnitude: u128| magnitude as f64;

    if negative {
        if magnitude == 0 {
            return Number::F64(-0.0);
        }

        if let Ok(value) = i64::try_from(magnitude) {
            if let Some(negated) = value.checked_neg() {
                return Number::I64(negated);
            }
        }

        if magnitude == I64_MIN_MAGNITUDE {
            return Number::I64(i64::MIN);
        }

        return Number::F64(-approximate(magnitude));
    }

    match u64::try_from(magnitude) {
        Ok(value) => Number::U64(value),
        Err(_) => Number::F64(approximate(magnitude)),
    }
}

/// The escape-walking state shared between literal runs and decoded escapes.
///
/// Mirrors the walker inside `strata-json-parser`'s decode module: literal content and completed
/// escape characters flush a held high surrogate, while `\uHHHH` units feed the shared pairing
/// decoder. Line continuations bypass the walker entirely — they produce no output and do not
/// break a pending pair.
struct EscapeWalker<'text> {
    text: &'text str,
    policy: LoneSurrogates,
    out: String,
    decoder: Utf16Decoder,
    /// Offset of the `\` that produced the held high surrogate, when one is held.
    pending_offset: usize,
    run_start: usize,
}

impl<'text> EscapeWalker<'text> {
    fn new(text: &'text str, policy: LoneSurrogates) -> Self {
        Self {
            text,
            policy,
            out: String::with_capacity(text.len()),
            decoder: Utf16Decoder::new(),
            pending_offset: 0,
            run_start: 1,
        }
    }

    /// Emits the literal run `[run_start, end)`, flushing any held surrogate first.
    fn take_run_until(&mut self, end: usize) -> Result<(), Error> {
        if self.run_start >= end {
            return Ok(());
        }

        self.flush_decoder()?;

        let slice = self.text.get(self.run_start..end).ok_or(Error {
            offset: self.run_start,
            kind: ErrorKind::MalformedToken,
        })?;
        self.out.push_str(slice);
        self.run_start = end;

        Ok(())
    }

    /// Restarts literal-run tracking after an escape.
    fn resume_run_at(&mut self, index: usize) {
        self.run_start = index;
    }

    /// Emits one decoded escape character, flushing any held surrogate first.
    fn push_char(&mut self, ch: char) -> Result<(), Error> {
        self.flush_decoder()?;
        self.out.push(ch);

        Ok(())
    }

    /// Feeds one `\uHHHH` code unit from the escape at `escape_offset`.
    fn push_unit(&mut self, unit: u16, escape_offset: usize) -> Result<(), Error> {
        match self.decoder.push(unit, self.policy) {
            Ok(Utf16Step::None) => {
                self.pending_offset = escape_offset;
            }
            Ok(Utf16Step::One(ch)) => self.out.push(ch),
            Ok(Utf16Step::Two(first, second)) => {
                self.out.push(first);
                self.out.push(second);
            }
            Err(LoneSurrogate { unit, was_pending }) => {
                return Err(Error {
                    offset: if was_pending {
                        self.pending_offset
                    } else {
                        escape_offset
                    },
                    kind: ErrorKind::LoneSurrogate { unit },
                });
            }
        }

        if self.decoder.has_pending() {
            self.pending_offset = escape_offset;
        }

        Ok(())
    }

    /// Flushes a held surrogate, blaming its own escape on error.
    fn flush_decoder(&mut self) -> Result<(), Error> {
        match self.decoder.flush(self.policy) {
            Ok(Some(ch)) => {
                self.out.push(ch);
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(LoneSurrogate { unit, .. }) => Err(Error {
                offset: self.pending_offset,
                kind: ErrorKind::LoneSurrogate { unit },
            }),
        }
    }

    /// Finishes decoding, flushing any held surrogate.
    fn finish(mut self) -> Result<String, Error> {
        self.flush_decoder()?;

        Ok(self.out)
    }
}

/// Validates the quoting shape and UTF-8 of a raw string lexeme, returning it as text.
fn validated_quoted(raw: &[u8]) -> Result<&str, Error> {
    let Some(&quote @ (b'"' | b'\'')) = raw.first() else {
        return Err(Error {
            offset: 0,
            kind: ErrorKind::MalformedToken,
        });
    };

    if raw.len() < 2 || raw.last() != Some(&quote) {
        return Err(Error {
            offset: 0,
            kind: ErrorKind::MalformedToken,
        });
    }

    validated_utf8(raw)
}

/// Validates UTF-8, blaming the first invalid byte.
fn validated_utf8(raw: &[u8]) -> Result<&str, Error> {
    core::str::from_utf8(raw).map_err(|error| Error {
        offset: error.valid_up_to(),
        kind: ErrorKind::MalformedToken,
    })
}

/// Parses `digits` ASCII hex digits at `index` into their value (at most eight digits fit).
fn parse_hex(raw: &[u8], index: usize, digits: usize) -> Option<u32> {
    let slice = raw.get(index..index.saturating_add(digits))?;
    let mut value: u32 = 0;

    for &byte in slice {
        let digit = char::from(byte).to_digit(16)?;
        value = value.saturating_mul(16).saturating_add(digit);
    }

    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unescape(raw: &str) -> Result<String, Error> {
        unescape_string(raw.as_bytes(), LoneSurrogates::Error)
    }

    fn unescape_replacing(raw: &str) -> Result<String, Error> {
        unescape_string(raw.as_bytes(), LoneSurrogates::Replace)
    }

    #[test]
    fn number_formats_classify_the_written_form() -> Result<(), Error> {
        assert_eq!(parse_number(b"42")?.format, NumberFormat::Integer);
        assert_eq!(parse_number(b"+42")?.format, NumberFormat::Integer);
        assert_eq!(parse_number(b".5")?.format, NumberFormat::Decimal);
        assert_eq!(parse_number(b"5.")?.format, NumberFormat::Decimal);
        assert_eq!(parse_number(b"1.23e+5")?.format, NumberFormat::Exponential);
        assert_eq!(parse_number(b"-0x10")?.format, NumberFormat::Hex);
        assert_eq!(parse_number(b"-Infinity")?.format, NumberFormat::Infinity);
        assert_eq!(parse_number(b"NaN")?.format, NumberFormat::NaN);

        Ok(())
    }

    #[test]
    fn plain_strings_decode_verbatim_in_both_quote_kinds() -> Result<(), Error> {
        assert_eq!(unescape(r#""hello""#)?, "hello");
        assert_eq!(unescape("'hello'")?, "hello");
        assert_eq!(unescape(r#""""#)?, "");
        assert_eq!(unescape("''")?, "");
        assert_eq!(unescape("\"caf\u{E9} \u{1F600}\"")?, "caf\u{E9} \u{1F600}");
        // The other quote kind is ordinary content.
        assert_eq!(unescape("'say \"hi\"'")?, "say \"hi\"");
        assert_eq!(unescape("\"it's\"")?, "it's");

        Ok(())
    }

    #[test]
    fn json_escapes_decode() -> Result<(), Error> {
        assert_eq!(
            unescape(r#""\"\\\/\b\f\n\r\t""#)?,
            "\"\\/\u{08}\u{0C}\n\r\t"
        );

        Ok(())
    }

    #[test]
    fn json5_escapes_decode() -> Result<(), Error> {
        assert_eq!(unescape(r"'\''")?, "'");
        assert_eq!(unescape(r"'\v'")?, "\u{0B}");
        assert_eq!(unescape(r"'\0'")?, "\u{0}");
        assert_eq!(unescape(r"'\x41'")?, "A");
        assert_eq!(unescape(r"'\xe9'")?, "\u{E9}");
        assert_eq!(unescape(r"'a\0b'")?, "a\u{0}b");

        Ok(())
    }

    #[test]
    fn non_escape_characters_escape_themselves() -> Result<(), Error> {
        assert_eq!(unescape(r"'\q'")?, "q");
        assert_eq!(unescape(r"'\/'")?, "/");
        assert_eq!(unescape("'\\\u{20AC}'")?, "\u{20AC}");
        assert_eq!(unescape("\"\\'\"")?, "'");

        Ok(())
    }

    #[test]
    fn unicode_escapes_decode() -> Result<(), Error> {
        assert_eq!(unescape(r#""\u0041\u00e9\u20AC""#)?, "A\u{E9}\u{20AC}");
        assert_eq!(unescape(r#""\u0000""#)?, "\u{0}");

        Ok(())
    }

    #[test]
    fn line_continuations_produce_no_output() -> Result<(), Error> {
        for terminator in ["\n", "\r", "\r\n", "\u{2028}", "\u{2029}"] {
            let raw = format!("'a\\{terminator}b'");
            assert_eq!(
                unescape_string(raw.as_bytes(), LoneSurrogates::Error)?,
                "ab",
                "terminator {terminator:?}"
            );
        }

        // CRLF is one terminator sequence, not a continuation plus a raw LF.
        assert_eq!(unescape("'\\\r\n'")?, "");

        Ok(())
    }

    #[test]
    fn surrogate_pairs_combine() -> Result<(), Error> {
        assert_eq!(unescape(r#""\ud83d\ude00""#)?, "\u{1F600}");
        assert_eq!(unescape(r"'x\uD83D\uDE00y'")?, "x\u{1F600}y");

        Ok(())
    }

    #[test]
    fn surrogate_pairs_combine_across_a_line_continuation() -> Result<(), Error> {
        // The continuation contributes nothing, leaving the code units adjacent — the pair still
        // combines, exactly as in ECMAScript.
        assert_eq!(unescape("'\\uD83D\\\n\\uDE00'")?, "\u{1F600}");

        Ok(())
    }

    #[test]
    fn lone_surrogates_error_with_offsets() {
        // Bytes: 0 the quote, 1 the backslash of the lone escape.
        let Err(error) = unescape(r"'\ud800'") else {
            panic!("expected a lone surrogate error");
        };
        assert_eq!(error.kind, ErrorKind::LoneSurrogate { unit: 0xD800 });
        assert_eq!(error.offset, 1);

        // A lone low surrogate errors at its own escape.
        let Err(error) = unescape(r"'a\udc00'") else {
            panic!("expected a lone surrogate error");
        };
        assert_eq!(error.kind, ErrorKind::LoneSurrogate { unit: 0xDC00 });
        assert_eq!(error.offset, 2);

        // A high surrogate followed by literal content is blamed at the high's escape.
        let Err(error) = unescape(r"'\ud800abc'") else {
            panic!("expected a lone surrogate error");
        };
        assert_eq!(error.kind, ErrorKind::LoneSurrogate { unit: 0xD800 });
        assert_eq!(error.offset, 1);

        // A high surrogate followed by a completed non-`\u` escape.
        let Err(error) = unescape(r"'\ud800\x41'") else {
            panic!("expected a lone surrogate error");
        };
        assert_eq!(error.kind, ErrorKind::LoneSurrogate { unit: 0xD800 });
        assert_eq!(error.offset, 1);
    }

    #[test]
    fn lone_surrogates_replace_under_policy() -> Result<(), Error> {
        assert_eq!(unescape_replacing(r"'\ud800'")?, "\u{FFFD}");
        assert_eq!(unescape_replacing(r"'\udc00'")?, "\u{FFFD}");
        assert_eq!(unescape_replacing(r"'\ud800abc'")?, "\u{FFFD}abc");
        assert_eq!(unescape_replacing(r"'\ud800\u0041'")?, "\u{FFFD}A");
        assert_eq!(
            unescape_replacing(r"'\ud800\ud83d\ude00'")?,
            "\u{FFFD}\u{1F600}"
        );

        Ok(())
    }

    #[test]
    fn digit_escapes_are_malformed() {
        for raw in [r"'\1'", r"'\9'", r"'\09'", r"'a\5b'"] {
            let Err(error) = unescape(raw) else {
                panic!("expected a malformed-token error for {raw:?}");
            };
            assert_eq!(error.kind, ErrorKind::MalformedToken, "raw {raw:?}");
        }

        // The offset blames the backslash.
        let Err(error) = unescape(r"'\09'") else {
            panic!("expected a malformed-token error");
        };
        assert_eq!(error.offset, 1);
    }

    #[test]
    fn malformed_hex_escapes_are_rejected_at_the_backslash() {
        for raw in [r"'\x4'", r"'\xg1'", r"'\u12'", r"'\u12G4'"] {
            let Err(error) = unescape(raw) else {
                panic!("expected a malformed-token error for {raw:?}");
            };
            assert_eq!(error.kind, ErrorKind::MalformedToken, "raw {raw:?}");
            assert_eq!(error.offset, 1, "raw {raw:?}");
        }
    }

    #[test]
    fn malformed_shapes_are_rejected() {
        for raw in [
            &b"hello"[..],
            b"'open",
            b"\"open",
            b"x'",
            b"'",
            b"\"",
            b"'ab\"",
            // The escape swallows the closing quote.
            b"'\\'",
        ] {
            let Err(error) = unescape_string(raw, LoneSurrogates::Error) else {
                panic!("expected a malformed-token error for {raw:?}");
            };
            assert_eq!(error.kind, ErrorKind::MalformedToken, "raw {raw:?}");
        }
    }

    #[test]
    fn invalid_utf8_is_rejected_at_its_byte() {
        let Err(error) = unescape_string(b"'a\xFFb'", LoneSurrogates::Error) else {
            panic!("expected a malformed-token error");
        };
        assert_eq!(error.kind, ErrorKind::MalformedToken);
        assert_eq!(error.offset, 2);
    }

    #[test]
    fn member_name_shapes_classify_by_first_byte() {
        assert_eq!(member_name_shape(b"\"key\""), NameShape::DoubleQuoted);
        assert_eq!(member_name_shape(b"'key'"), NameShape::SingleQuoted);
        assert_eq!(member_name_shape(b"key"), NameShape::Identifier);
        assert_eq!(member_name_shape(br"\u0061bc"), NameShape::Identifier);
        assert_eq!(member_name_shape(b""), NameShape::Identifier);
    }

    #[test]
    fn identifier_names_decode() -> Result<(), Error> {
        let decode = |raw: &str| decode_member_name(raw.as_bytes(), LoneSurrogates::Error);

        assert_eq!(decode("abc")?, "abc");
        assert_eq!(decode("$_x9")?, "$_x9");
        assert_eq!(decode("\u{00E9}t\u{00E9}")?, "\u{00E9}t\u{00E9}");
        // `\u0061bc` and `abc` denote the same name.
        assert_eq!(decode(r"\u0061bc")?, "abc");
        assert_eq!(decode(r"ab\u0063")?, "abc");
        assert_eq!(decode(r"a\u0031")?, "a1");
        assert_eq!(decode(r"Infinit\u0079")?, "Infinity");

        Ok(())
    }

    #[test]
    fn quoted_names_route_through_string_unescaping() -> Result<(), Error> {
        assert_eq!(decode_member_name(b"'a b'", LoneSurrogates::Error)?, "a b");
        assert_eq!(
            decode_member_name(br#""\u0041""#, LoneSurrogates::Error)?,
            "A"
        );
        // The policy applies on the quoted route.
        assert_eq!(
            decode_member_name(br"'\uD800'", LoneSurrogates::Replace)?,
            "\u{FFFD}"
        );

        Ok(())
    }

    #[test]
    fn malformed_identifier_escapes_are_rejected() {
        let decode = |raw: &[u8]| decode_member_name(raw, LoneSurrogates::Error);

        for raw in [
            &br"a\x41"[..],
            br"a\u12",
            br"a\uZZZZ",
            br"a\",
            // Identifier escapes carry no surrogate pairing: a surrogate unit is malformed.
            br"a\uD800",
            br"a\uD83D\uDE00",
            b"",
        ] {
            let Err(error) = decode(raw) else {
                panic!("expected a malformed-token error for {raw:?}");
            };
            assert_eq!(error.kind, ErrorKind::MalformedToken, "raw {raw:?}");
        }

        // The offset blames the backslash.
        let Err(error) = decode(br"ab\uD800") else {
            panic!("expected a malformed-token error");
        };
        assert_eq!(error.offset, 2);
    }

    #[test]
    fn json_numbers_still_classify() -> Result<(), Error> {
        assert_eq!(parse_number(b"0")?.value, Number::U64(0));
        assert_eq!(parse_number(b"42")?.value, Number::U64(42));
        assert_eq!(
            parse_number(b"18446744073709551615")?.value,
            Number::U64(u64::MAX)
        );
        assert_eq!(parse_number(b"-17")?.value, Number::I64(-17));
        assert_eq!(
            parse_number(b"-9223372036854775808")?.value,
            Number::I64(i64::MIN)
        );
        assert_eq!(parse_number(b"3.25")?.value, Number::F64(3.25));
        assert_eq!(parse_number(b"1e10")?.value, Number::F64(1e10));
        assert_eq!(parse_number(b"2.5E-3")?.value, Number::F64(2.5e-3));

        Ok(())
    }

    #[test]
    fn explicit_plus_signs_classify_like_their_unsigned_forms() -> Result<(), Error> {
        assert_eq!(parse_number(b"+42")?.value, Number::U64(42));
        assert_eq!(parse_number(b"+0")?.value, Number::U64(0));
        assert_eq!(parse_number(b"+1.5e3")?.value, Number::F64(1.5e3));
        assert_eq!(parse_number(b"+0x10")?.value, Number::U64(16));

        Ok(())
    }

    #[test]
    fn leading_and_trailing_decimal_points_convert() -> Result<(), Error> {
        assert_eq!(parse_number(b".5")?.value, Number::F64(0.5));
        assert_eq!(parse_number(b"-.5")?.value, Number::F64(-0.5));
        assert_eq!(parse_number(b"5.")?.value, Number::F64(5.0));
        assert_eq!(parse_number(b"5.e2")?.value, Number::F64(500.0));
        assert_eq!(parse_number(b".5e-1")?.value, Number::F64(0.05));

        Ok(())
    }

    #[test]
    fn infinity_and_nan_convert_with_their_signs() -> Result<(), Error> {
        assert_eq!(parse_number(b"Infinity")?.value, Number::F64(f64::INFINITY));
        assert_eq!(
            parse_number(b"+Infinity")?.value,
            Number::F64(f64::INFINITY)
        );
        assert_eq!(
            parse_number(b"-Infinity")?.value,
            Number::F64(f64::NEG_INFINITY)
        );

        for (raw, negative) in [(&b"NaN"[..], false), (b"+NaN", false), (b"-NaN", true)] {
            let Number::F64(value) = parse_number(raw)?.value else {
                panic!("expected an f64 NaN for {raw:?}");
            };
            assert!(value.is_nan(), "raw {raw:?}");
            assert_eq!(value.is_sign_negative(), negative, "raw {raw:?}");
        }

        Ok(())
    }

    #[test]
    fn hex_integers_preserve_integer_identity() -> Result<(), Error> {
        assert_eq!(parse_number(b"0x0")?.value, Number::U64(0));
        assert_eq!(parse_number(b"0xdecaf")?.value, Number::U64(0x000D_ECAF));
        assert_eq!(parse_number(b"0XABCDEF")?.value, Number::U64(0x00AB_CDEF));
        assert_eq!(
            parse_number(b"0xFFFFFFFFFFFFFFFF")?.value,
            Number::U64(u64::MAX)
        );
        assert_eq!(parse_number(b"-0x1F")?.value, Number::I64(-31));
        assert_eq!(
            parse_number(b"-0x7FFFFFFFFFFFFFFF")?.value,
            Number::I64(i64::MIN.saturating_add(1))
        );
        assert_eq!(
            parse_number(b"-0x8000000000000000")?.value,
            Number::I64(i64::MIN)
        );

        Ok(())
    }

    #[test]
    fn oversized_hex_magnitudes_approximate_as_f64() -> Result<(), Error> {
        // 2^64: one past u64::MAX.
        assert_eq!(
            parse_number(b"0x10000000000000000")?.value,
            Number::F64(18_446_744_073_709_551_616.0)
        );
        // One past i64::MIN's magnitude.
        assert_eq!(
            parse_number(b"-0x8000000000000001")?.value,
            Number::F64(-9_223_372_036_854_775_809.0)
        );
        // Beyond u128: still a finite approximation (2^128).
        assert_eq!(
            parse_number(b"0x100000000000000000000000000000000")?.value,
            Number::F64(340_282_366_920_938_463_463_374_607_431_768_211_456.0)
        );

        Ok(())
    }

    #[test]
    fn negative_zeros_preserve_their_sign() -> Result<(), Error> {
        for raw in [&b"-0"[..], b"-0x0", b"-0X00"] {
            let Number::F64(value) = parse_number(raw)?.value else {
                panic!("expected -0 to convert as a float for {raw:?}");
            };
            assert_eq!(value.to_bits(), (-0.0_f64).to_bits(), "raw {raw:?}");
        }

        Ok(())
    }

    #[test]
    fn malformed_numbers_are_rejected() {
        for raw in [
            &b""[..],
            b"-",
            b"+",
            b".",
            b"+.",
            b"abc",
            b"0x",
            b"0X",
            b"0xg",
            b"0x1p3",
            b"1e",
            b"1e+",
            b".e3",
            b"+-1",
            b"nan",
            b"inf",
            b"Infinity5",
        ] {
            let Err(error) = parse_number(raw) else {
                panic!("expected a malformed-token error for {raw:?}");
            };
            assert_eq!(error.kind, ErrorKind::MalformedToken, "raw {raw:?}");
        }
    }
}
