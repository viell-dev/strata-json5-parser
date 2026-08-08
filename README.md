# strata-json5-parser

JSON5 ([json5.org](https://json5.org)) parser for the Strata parser ecosystem, producing a
trivia-retaining, zero-copy CST by reusing `strata-json-parser`'s CST and grammar parts.

## Status

This crate is in initial development. The public API is not settled yet.

## Usage

```rust
use strata_json5_parser::{Json5Error, Json5Parser, Json5Token, TriviaKind, Value};

fn main() -> Result<(), Json5Error> {
    let document = Json5Parser::new()
        .parse_str("{\n  // editor width\n  columns: 100,\n}")?;

    let Value::Object(object) = document.root() else {
        panic!("expected an object root");
    };
    let [member] = object.members() else {
        panic!("expected one member");
    };

    // An unquoted member name; nothing is decoded — the CST keeps raw spans.
    assert_eq!(member.name().kind, Json5Token::Identifier);
    assert_eq!(document.token_bytes(member.name()), Some(&b"columns"[..]));

    // The comment is retained as trivia on the token it precedes.
    let Some(comment) = member.name().leading_comments().next() else {
        panic!("expected the leading comment");
    };
    assert_eq!(comment.kind(), TriviaKind::LineComment);
    assert_eq!(
        document.trivia_bytes(comment),
        Some(&b"// editor width"[..])
    );

    // The trailing comma is mandatory JSON5, and this input needs no
    // spec-mandated warnings (unescaped U+2028/U+2029 in strings).
    assert!(document.warnings().is_empty());

    Ok(())
}
```

Every JSON5 extension is a mandatory language feature, so the builder exposes only the
spec-permitted nesting-depth limit and the lower-layer swap hooks. The crate is
`strata-json-parser`'s generic CST and grammar instantiated for JSON5, and resolver layers
decode string, member-name, and number lexemes through this crate's `decode` module. See
[docs/strata-json5-parser.md](docs/strata-json5-parser.md) for the design reference.

## Minimum Supported Rust Version

The MSRV is **1.87.0**.

## License

Licensed under the Mozilla Public License, Version 2.0 (see [LICENSE](LICENSE)
or <https://www.mozilla.org/en-US/MPL/2.0/>).

MPL-2.0 is a file-level weak copyleft license: its obligations attach to this crate's files (in
source and compiled form) and to modifications of them, not to your own source files that merely
use this crate. Distributing a binary that includes this crate obliges you to tell recipients
that the crate is included and how to obtain its source; for an unmodified crate, pointing them
at the exact crates.io release is a reasonable way to do that. Example material (documentation
code examples, `examples/`, integration tests, and benchmarks) is additionally licensed under
[MIT-0](LICENSE-MIT-0), so it can be copied freely. See
[LICENSE-CLARIFICATION.md](LICENSE-CLARIFICATION.md) for the maintainers' full statement of
intent regarding normal Rust usage (linking, monomorphization, macro expansion), binary
distribution, and generated code, and [CONTRIBUTING.md](CONTRIBUTING.md) for the contribution
policy.
