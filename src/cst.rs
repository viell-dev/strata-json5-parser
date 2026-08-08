//! The JSON5 CST surface: `strata-json-parser`'s generic tree instantiated with JSON5 kinds.
//!
//! Instead of mirroring the tree, this crate aliases the generic nodes with
//! [`Json5Token`] as the token kind and this crate's [`TriviaKind`] as the trivia kind. The
//! JSON5-only pieces live here:
//!
//! - [`TriviaKind`] carries `LineComment` and `BlockComment`: comments are retained trivia
//!   attached to the following token, exactly like whitespace runs. There is no BOM trivia
//!   kind — JSON5 classifies U+FEFF as ordinary whitespace, anywhere in the input.
//! - A [`Member`]'s name token can be an identifier (or one of the word tokens
//!   `true`/`false`/`null`/`Infinity`/`NaN`, which are valid `IdentifierName`s) as well as a
//!   string.
//! - [`Document`] wraps the generic document and carries the spec-mandated [`Warning`]s
//!   (unescaped U+2028/U+2029 in strings).
//!
//! Duplicate member names are retained verbatim, in source order. The [spec] merely observes
//! that software receiving an object with non-unique names behaves unpredictably; a
//! fidelity-preserving CST records what was written and leaves duplicate policy downstream, so
//! there is no option.
//!
//! [spec]: https://spec.json5.org/

use std::fmt;
use strata_json_parser::cst::{self, TriviaClass};
use strata_json5_rules::Json5Token;
use strata_lexer::{Position, SourceRef, SourceStore, Span};

/// Classification of a retained trivia piece.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TriviaKind {
    /// A run of JSON5 whitespace, including line terminators and any byte-order mark.
    Whitespace,
    /// A `//` comment (terminating line break excluded — it is whitespace).
    LineComment,
    /// A `/* ... */` comment, delimiters included.
    BlockComment,
}

impl TriviaClass for TriviaKind {
    #[inline]
    fn is_comment(self) -> bool {
        matches!(self, Self::LineComment | Self::BlockComment)
    }
}

/// A retained piece of grammatically insignificant JSON5 source content.
pub type Trivia = cst::Trivia<TriviaKind>;
/// A significant JSON5 token in the CST.
pub type SyntaxToken = cst::SyntaxToken<Json5Token, TriviaKind>;
/// A JSON5 value node.
pub type Value = cst::Value<Json5Token, TriviaKind>;
/// A JSON5 array node.
pub type ArrayNode = cst::ArrayNode<Json5Token, TriviaKind>;
/// One JSON5 array element.
pub type Element = cst::Element<Json5Token, TriviaKind>;
/// A JSON5 object node.
pub type ObjectNode = cst::ObjectNode<Json5Token, TriviaKind>;
/// One JSON5 object member.
pub type Member = cst::Member<Json5Token, TriviaKind>;

/// Categories of spec-mandated parse warnings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningKind {
    /// An unescaped U+2028 LINE SEPARATOR or U+2029 PARAGRAPH SEPARATOR inside a string.
    ///
    /// These are legal JSON5 string content, but the [spec] says parsers *should* produce a
    /// warning when they are found unescaped in strings (they break ECMAScript compatibility
    /// before ES2019). The parse succeeds and the warning is recorded on the document.
    ///
    /// [spec]: https://spec.json5.org/
    UnescapedLineSeparator {
        /// The offending character: U+2028 or U+2029.
        separator: char,
    },
}

/// A spec-mandated parse warning: the parse succeeded, but the input carries a construct the
/// JSON5 spec says parsers *should* warn about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    /// Position of the offending character.
    pub position: Position,
    /// What was found.
    pub kind: WarningKind,
}

impl fmt::Display for Warning {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            WarningKind::UnescapedLineSeparator { separator } => write!(
                formatter,
                "unescaped U+{:04X} in a string at line {}, column {} (byte {})",
                u32::from(separator),
                self.position.line(),
                self.position.column(),
                self.position.byte()
            ),
        }
    }
}

/// A parsed JSON5 document: the root value, trailing trivia, warnings, and the retained source.
///
/// This wraps the generic `strata-json-parser` document — JSON5 is the one JSON-family language
/// whose parses carry spec-mandated warnings, so the document gains a [`Document::warnings`]
/// accessor and its [`Document::into_parts`] hands the warnings to the resolver alongside the
/// tree. Everything else delegates: the document owns the [`SourceStore`] holding the original
/// bytes, so every span in the tree stays addressable — zero-copy — for as long as the document
/// lives. [`Document::source_ref`] hands out promotable handles that survive even store
/// reclamation (see `strata-lexer`'s retention layer); [`Document::bytes`] is the plain borrow
/// path.
#[derive(Debug)]
pub struct Document {
    inner: cst::Document<Json5Token, TriviaKind>,
    warnings: Vec<Warning>,
}

impl Document {
    /// Assembles a document from the generic parse result and its warnings. Used by the parser.
    pub(crate) const fn new(
        inner: cst::Document<Json5Token, TriviaKind>,
        warnings: Vec<Warning>,
    ) -> Self {
        Self { inner, warnings }
    }

    /// The root value. Any JSON5 value is a valid top-level document.
    pub const fn root(&self) -> &Value {
        self.inner.root()
    }

    /// Trivia between the end of the root value and the end of input.
    pub fn trailing_trivia(&self) -> &[Trivia] {
        self.inner.trailing_trivia()
    }

    /// The spec-mandated warnings recorded during the parse (see [`WarningKind`]), in source
    /// order. An empty slice means a warning-free document.
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// Borrows the raw source bytes covered by `span`, zero-copy.
    ///
    /// Returns `None` if the span is not (or no longer) backed by the retained source, e.g.
    /// after [`Document::source_mut`] was used to retire buffers.
    pub fn bytes(&self, span: Span) -> Option<&[u8]> {
        self.inner.bytes(span)
    }

    /// Borrows the raw lexeme bytes of `token`, zero-copy.
    pub fn token_bytes(&self, token: &SyntaxToken) -> Option<&[u8]> {
        self.inner.token_bytes(token)
    }

    /// The bytes of trivia `piece`, zero-copy.
    pub fn trivia_bytes(&self, piece: &Trivia) -> Option<&[u8]> {
        self.inner.trivia_bytes(piece)
    }

    /// Creates a promotable, owned-on-demand handle to the bytes covered by `span`.
    ///
    /// This is the resolver-facing path: the handle stays zero-copy while the store retains the
    /// buffer and degrades to a small owned copy if the store reclaims it.
    pub fn source_ref(&mut self, span: Span) -> Option<SourceRef> {
        self.inner.source_ref(span)
    }

    /// The retained source store.
    pub const fn source(&self) -> &SourceStore {
        self.inner.source()
    }

    /// Mutable access to the retained source store, e.g. for owner-side reclamation
    /// (`maintain` / `retire_all`).
    pub const fn source_mut(&mut self) -> &mut SourceStore {
        self.inner.source_mut()
    }

    /// Decomposes the document into the tree, its trailing trivia, the retained source, and the
    /// recorded warnings.
    ///
    /// This is the resolver boundary: a resolver takes ownership of the tree and the store,
    /// decodes what it needs, decides how long the retained source lives, and surfaces the
    /// warnings however its consumer expects.
    #[must_use]
    pub fn into_parts(self) -> (Value, Vec<Trivia>, SourceStore, Vec<Warning>) {
        let (root, trailing, source) = self.inner.into_parts();

        (root, trailing, source, self.warnings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warning_display_names_the_code_point() {
        let Ok(position) = Position::new(5, 2, 3) else {
            panic!("test positions must be one-based");
        };
        let warning = Warning {
            position,
            kind: WarningKind::UnescapedLineSeparator {
                separator: '\u{2028}',
            },
        };
        let message = warning.to_string();
        assert!(message.contains("U+2028"), "{message}");
        assert!(message.contains("line 2"), "{message}");
    }

    #[test]
    fn comment_trivia_kinds_classify_as_comments() {
        assert!(TriviaKind::LineComment.is_comment());
        assert!(TriviaKind::BlockComment.is_comment());
        assert!(!TriviaKind::Whitespace.is_comment());
        // JSON5 has no BOM trivia: U+FEFF is ordinary whitespace.
        assert!(!TriviaKind::Whitespace.is_bom());
        assert!(!TriviaKind::LineComment.is_bom());
        assert!(!TriviaKind::BlockComment.is_bom());
    }
}
