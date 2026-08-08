# strata-json5-parser

`strata-json5-parser` is the strict [JSON5](https://spec.json5.org/) parser for the Strata
parser ecosystem. Every JSON5 extension — unquoted `IdentifierName` member names, single-quoted
strings, line continuations and the extra escapes, hex numbers, `Infinity`/`NaN`, explicit `+`,
leading/trailing decimal points, trailing commas, `//` and `/* */` comments, the expanded
whitespace set with the byte-order mark as ordinary whitespace — is a mandatory language
feature, not an option, and nothing beyond JSON5 is accepted: the spec's permission to "accept
non-JSON5 forms or extensions" is deliberately unexercised, per the ecosystem's
strict-by-default policy. Comments are trivia — retained in the CST with spans, never affecting
the parsed data — string and number content stays raw in the tree, and decoding runs at the
resolver layer through this crate's own `decode` module (JSON5 escapes and numbers are wider
than JSON's, so unlike JSONC it cannot simply re-export the JSON one).

The crate sits at the parser stage of the per-language stack and is the consumer side of
`strata-json-parser`'s generic layer: it does not mirror or wrap the JSON parser, it
instantiates it —

```text
strata-json5-rules -> strata-json5-parser (this crate) -> strata-json5 (resolver)
                          ^ a Syntax impl + type aliases over
                            strata-json-parser's generic cst + grammar
```

- **Edition / MSRV:** Rust 2024, MSRV `1.87.0`.
- **License:** MPL-2.0.
- **Dependencies:** `strata-json-parser`, `strata-json5-rules`, `strata-lexer`.

## 1. Module and public-API map

| Module   | Public items |
|----------|--------------|
| `cst`    | `Document`, `TriviaKind`, `Warning`, `WarningKind`, CST aliases (`Value`, `ArrayNode`, `Element`, `ObjectNode`, `Member`, `SyntaxToken`, `Trivia`) |
| `parse`  | `Json5Parser<M>`, `Json5ParserBuilder<M>`, `Json5Error`, `Json5ErrorKind`, `DEFAULT_MAX_DEPTH` |
| `decode` | `unescape_string()`, `decode_member_name()`, `member_name_shape()`, `NameShape`, `parse_number()`; re-exports `Error`, `ErrorKind`, `LoneSurrogates`, `Number` from `strata-json-parser` |
| root     | everything above (`cst` and `parse` are private modules re-exported at the root; `decode` is public), plus the `strata-json5-rules` surface re-exported |

## 2. Instantiating the generic layer

`strata-json-parser` §2–3 parameterizes its CST over `(K, T)` — significant token kind and
trivia kind — and captures all grammar variance in the `Syntax` trait. This crate supplies the
JSON5 answers:

- **`Lex = Kind = Json5Token`.** Unlike JSONC — whose `classify` unwraps its lex kind so the
  tree holds plain `JsonToken`s — JSON5 *has* significant kinds JSON lacks (`Identifier`,
  `Infinity`, `NaN`, its own `String`/`Number`), so both `Syntax` type parameters are
  `Json5Token` and classification never rewraps: the aliases instantiate the generic tree as
  `cst::SyntaxToken<Json5Token, TriviaKind>` and likewise for `Value`, `ArrayNode`, `Element`,
  `ObjectNode`, `Member`, and `Trivia`. Reused JSON kinds still show through as
  `Json5Token::Json(...)` — with the strict rules exactly the six structural characters and
  `true`/`false`/`null`.
- **`T = TriviaKind`.** The crate's own trivia enum is `Whitespace`, `LineComment`, and
  `BlockComment` — deliberately *without* a `Bom` variant: U+FEFF is ordinary JSON5 whitespace,
  anywhere in the input, so a BOM is indistinguishable from (and lexed as part of) a whitespace
  run. It implements the upstream `TriviaClass` hook with `is_comment()` for the two comment
  kinds and the default always-false `is_bom()`, so the generic tree's comment conveniences
  (`SyntaxToken::leading_comments()`, `Document::trivia_bytes()`) work unchanged while the BOM
  conveniences are inert.
- **A private `Json5Syntax` implementing `Syntax`.** Its `classify` sends whitespace and both
  comment kinds to trivia and everything else through as significant; `value_role` gives
  `Infinity`/`NaN` the number role alongside `Number`; `is_member_name` is the JSON5 breadth —
  `Identifier`, `String`, and the word tokens `Infinity`, `NaN`, `Json(True/False/Null)`,
  because ES 5.1 `IdentifierName` (unlike `Identifier`) includes reserved words;
  `allow_trailing_commas` returns `true` unconditionally — one trailing comma in a non-empty
  object or array is a mandatory language feature, where JSONC gates the same driver tolerance
  behind an option. The error-constructor methods build `Json5Error`.

Everything else is inherited from `parse_document`: trivia folding (each piece attaches to the
next significant token, or to the document end), the nesting-depth limit, the whole-input
requirement, source parking into the document's `SourceStore`, and byte-for-byte reconstruction
(`tests/trivia_spans.rs` proves it over a kitchen-sink document — comments, a mid-document BOM,
and trailing commas included).

## 3. The `Document` wrapper and spec-mandated warnings

JSON5 is the one JSON-family language whose parses carry warnings: the spec says parsers
*should* produce a warning when U+2028 LINE SEPARATOR or U+2029 PARAGRAPH SEPARATOR is found
unescaped in a string (legal content, but it breaks ECMAScript compatibility before ES2019). So
this crate's `Document` is not a type alias: it wraps the generic document and adds
`warnings()`, a slice of `Warning { position, kind }` values in source order, with
`WarningKind::UnescapedLineSeparator { separator }` naming the offending character.
`into_parts()` widens to a four-tuple, handing the warnings to the resolver alongside the tree,
trailing trivia, and source store. Every other accessor (`root()`, `trailing_trivia()`,
`bytes()`, `token_bytes()`, `trivia_bytes()`, `source_ref()`, `source()`/`source_mut()`)
delegates.

The warnings are collected in a post-parse pass, not during the grammar walk: after
`parse_document` returns, the parser visits every string token of the finished tree (member
names included) and scans its retained lexeme, escape-aware — a separator directly preceded by
a backslash is a line continuation, and after `\\` the next character is *not* escape content,
so `'\\` + U+2028 + `'` still warns. Warning positions mirror the lexer core's byte-oriented
tracking (CR, LF, CRLF each count one line break; U+2028/U+2029 do not advance the diagnostic
line). The pass is always on: a spec SHOULD, not an option, and recording on the document keeps
the parse result a plain value — no callback plumbing through the generic driver.

## 4. The JSON5 front (`parse`)

`Json5Parser::new()` / `default()` is strict JSON5 — and the only JSON5 there is. `parse(impl
Read)`, `parse_str`, and `parse_bytes` each run one document to EOF; any JSON5 value is a valid
top-level document, and trailing content other than whitespace and comments is an error. The
parser clones its rule set into a fresh scanner per parse, so one parser serves many inputs.

`Json5Parser::builder()` exposes only the spec-permitted depth limit and the ecosystem's
layer-swap hooks — there are no tolerance switches, because every JSON5 extension is mandatory
and a BOM is whitespace (no `allow_bom`, unlike both sibling parsers):

- `max_depth(usize)` — the spec's "implementation may set limits on the maximum depth of
  nesting"; counts nested containers (`DEFAULT_MAX_DEPTH` = `strata_json_parser`'s 128) and
  also bounds recursion.
- `with_rules(set)` — any `MatcherSet<Kind = Json5Token> + Clone`; wrap non-`Clone` sets in
  `SharedRuleSet`. A superset *language* instead implements its own `Syntax` over its own
  kinds, reusing the rule structs from `strata-json5-rules`.
- `with_lexer_config(config)` — reader capacities, position tracking, ...; `compact_after` is
  always overridden to `None` at parse time so the consumed bytes survive into the store.

Duplicate member names are retained verbatim in source order (the spec merely observes that
receivers of such objects behave unpredictably); duplicate policy belongs to resolvers, so
there is no option.

Errors are `Json5Error { position, kind }` with one-based line/column plus absolute byte.
`Json5ErrorKind`: `Lex(LexError)` (unrecognized bytes, string/number shape violations,
unterminated strings and block comments, invalid UTF-8 — at the offending byte), `Expected`,
`UnexpectedEof`, `TrailingContent`, `DepthLimitExceeded`. There is no `UnexpectedBom` variant —
it has no meaning in JSON5 — and `found` kinds are `Json5Token`s.

## 5. Decoding (`decode`)

The parser never decodes; the resolver layer does, through this module. It mirrors
`strata-json-parser`'s `decode` in role and placement — the parser layer owns the inverse of
the shapes its rules validate, and resolvers must not depend on each other — but the content is
JSON5-shaped. The pieces shared with JSON (the `Utf16Decoder` surrogate pairing, the
`LoneSurrogates` policy, `Error`/`ErrorKind`, and the `Number` classification) are reused from
`strata-json-parser` and re-exported here, so a JSON5 resolver needs no direct dependency on
it. The functions are total over arbitrary input: parser-produced tokens cannot be malformed,
but the functions are public and re-check shape (`ErrorKind::MalformedToken`) instead of
trusting callers.

- `unescape_string(raw, policy)` — both quotes included, either quote kind. Handles the full
  ES 5.1 repertoire the string rule admits: the single-escape characters (JSON's set plus `\'`
  and `\v`), `\0` (malformed if a digit follows), `\xHH`, `\uHHHH` with UTF-16 surrogate-pair
  combination (a pair split by a line continuation still combines — the continuation
  contributes nothing, exactly as in ECMAScript), line continuations producing no output, and
  `NonEscapeCharacter` self-escaping (`\q` → `q`, covering JSON's `\/`). Digits `1`–`9` after a
  backslash are malformed. Lone surrogates resolve per `LoneSurrogates` (`Error`, the strict
  default, or `Replace` with U+FFFD); errors carry the byte offset of the offending escape's
  backslash.
- `decode_member_name(raw, policy)` and `member_name_shape(raw)` — member names have three
  written shapes (`NameShape`: double-quoted, single-quoted, identifier), decidable from the
  first byte since a quote cannot begin an identifier. Quoted names route through
  `unescape_string`; identifier names decode their `\uHHHH` escapes — `\u0061bc` and `abc`
  denote the same name — where each escape stands for exactly one character: ES 5.1 identifier
  escapes carry no surrogate pairing, so an escape denoting a surrogate code unit is malformed
  and `policy` applies only to the quoted route.
- `parse_number(raw)` — integer-preserving classification into the shared `Number`, over every
  shape the number rule admits plus the bare `Infinity`/`NaN` words (their own token kinds, but
  number values). As implemented: an optional `+` or `-` on every form; `Infinity`/`NaN`
  convert to `f64` with the sign preserved (`-NaN` sets the sign bit); hex integers convert by
  magnitude — non-negative fitting `u64` is `Number::U64`, negative fitting `i64` is `I64`
  (including `-0x8000000000000000` = `i64::MIN`), anything larger an `f64` approximation that
  may round and beyond `f64` range becomes infinite; decimal forms keep integer identity where
  the value is integral and digit-shaped (`+42` is `U64(42)`, negatives are `I64`, 64-bit
  overflow falls back to `f64`), while fractions, exponents, and leading/trailing decimal
  points (`.5`, `5.`) convert to `F64`; `-0` and `-0x0` convert to `F64(-0.0)`, preserving the
  sign. The raw text stays reachable through the CST for arbitrary-precision consumers.

## 6. Design choices

- **Mandatory extensions as classification, not options.** The whole superset lives in the
  `Syntax` impl (trivia classification, member-name breadth, the number role, always-on
  trailing commas); the builder consequently has nothing to toggle. This is the same shape as
  JSONC's front with the option surface collapsed to zero.
- **Warnings on the document.** The one spec SHOULD that is not a parse failure rides the
  result value, collected post-parse from the retained lexemes — no driver changes, no
  callbacks, resolvers receive it through `into_parts`.
- **Own kind in the tree.** Where JSONC reuses `JsonToken` as its tree kind (it adds no
  significant kinds), JSON5 keeps `Json5Token`: the written form of a member name or number
  (identifier vs. string; word vs. digits) is information the resolver needs, recorded in the
  kind.
- **A JSON5 `decode`, JSON primitives reused.** The escape walker and number classifier are
  JSON5-specific, but surrogate pairing, the lone-surrogate policy, the error type, and
  `Number` come from `strata-json-parser`, so the two decode modules cannot drift on shared
  semantics.

## 7. Quality bar

Same as the ecosystem: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D
warnings` under the strict lint set in `Cargo.toml`, and `cargo test` all clean. Lint exceptions
use `#[expect(lint, reason = "…")]` on the smallest existing scope.

## 8. Limitations and follow-up

- Unicode identifier classification is approximate outside ASCII — exact for ASCII and
  `\uXXXX` escapes; see `strata-json5-rules`' design reference §5 for the deviations.
- Single-buffer retention: the parser parks one consumed buffer after EOF; streaming/chunked
  DOM construction awaits the lexer's batch protocol.
- Serde glue (parser → structs directly) is a separate planned crate, not part of this one.
- The public API is not settled; this is an initial-development crate on an experimental branch.
- Package verification (`cargo package`) is blocked until the path dependencies are published.
