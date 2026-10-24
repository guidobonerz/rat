//! Tokens.

use std::fmt;

use crate::span::Span;

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    Int(i64),
    Float(f64),
    Str(String),
    Char(char),
    Punct(&'static str),
    Newline,
    Eof,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
    /// Whitespace (or a comment) directly precedes this token.
    pub space_before: bool,
}

impl Token {
    pub fn is_punct(&self, p: &str) -> bool {
        matches!(&self.tok, Tok::Punct(q) if *q == p)
    }

    pub fn ident(&self) -> Option<&str> {
        match &self.tok {
            Tok::Ident(s) => Some(s),
            _ => None,
        }
    }

    /// Identifier, case-insensitively equal to `name`.
    pub fn is_ident(&self, name: &str) -> bool {
        self.ident().is_some_and(|s| s.eq_ignore_ascii_case(name))
    }

    pub fn is_end(&self) -> bool {
        matches!(self.tok, Tok::Newline | Tok::Eof)
    }
}

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tok::Ident(s) => f.write_str(s),
            Tok::Int(v) => write!(f, "{v}"),
            Tok::Float(v) => write!(f, "{v}"),
            Tok::Str(s) => write!(f, "{s:?}"),
            Tok::Char(c) => write!(f, "'{c}'"),
            Tok::Punct(p) => f.write_str(p),
            Tok::Newline => f.write_str("end of line"),
            Tok::Eof => f.write_str("end of file"),
        }
    }
}

/// Render a token slice back to approximate source text.
pub fn tokens_to_string(toks: &[Token]) -> String {
    let mut s = String::new();
    for (i, t) in toks.iter().enumerate() {
        if i > 0 && t.space_before {
            s.push(' ');
        }
        s.push_str(&t.tok.to_string());
    }
    s
}
