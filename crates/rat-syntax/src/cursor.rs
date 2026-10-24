//! Token cursor shared by the parsers.

use rat_core::{Diagnostic, Span, Tok, Token};

pub struct Cur<'a> {
    pub toks: &'a [Token],
    pub pos: usize,
}

pub type PResult<T> = Result<T, Diagnostic>;

impl<'a> Cur<'a> {
    pub fn new(toks: &'a [Token]) -> Cur<'a> {
        Cur { toks, pos: 0 }
    }

    pub fn peek(&self) -> &'a Token {
        self.peek_at(0)
    }

    pub fn peek_at(&self, n: usize) -> &'a Token {
        let i = (self.pos + n).min(self.toks.len() - 1);
        &self.toks[i]
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> &'a Token {
        let t = self.peek();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }

    pub fn is(&self, p: &str) -> bool {
        self.peek().is_punct(p)
    }

    pub fn is_at(&self, n: usize, p: &str) -> bool {
        self.peek_at(n).is_punct(p)
    }

    /// Next token is `p` and directly follows the current one (no space).
    pub fn glued_at(&self, n: usize) -> bool {
        !self.peek_at(n).space_before
    }

    pub fn eat(&mut self, p: &str) -> bool {
        if self.is(p) {
            self.next();
            true
        } else {
            false
        }
    }

    pub fn expect(&mut self, p: &str) -> PResult<&'a Token> {
        if self.is(p) {
            Ok(self.next())
        } else {
            Err(self.error(format!("expected `{p}`, found {}", self.peek().tok)))
        }
    }

    pub fn ident(&mut self) -> PResult<String> {
        match &self.peek().tok {
            Tok::Ident(s) => {
                let s = s.clone();
                self.next();
                Ok(s)
            }
            t => Err(self.error(format!("expected a name, found {t}"))),
        }
    }

    pub fn is_ident(&self, name: &str) -> bool {
        self.peek().is_ident(name)
    }

    pub fn eat_ident(&mut self, name: &str) -> bool {
        if self.is_ident(name) {
            self.next();
            true
        } else {
            false
        }
    }

    pub fn at_eol(&self) -> bool {
        self.peek().is_end()
    }

    pub fn at_eof(&self) -> bool {
        matches!(self.peek().tok, Tok::Eof)
    }

    pub fn span(&self) -> Span {
        self.peek().span
    }

    /// Span from token index `start` to the previous token.
    pub fn span_from(&self, start: usize) -> Span {
        let a = self.toks[start.min(self.toks.len() - 1)].span;
        let b = self.toks[self.pos.saturating_sub(1).max(start).min(self.toks.len() - 1)].span;
        a.to(b)
    }

    pub fn error(&self, msg: impl Into<String>) -> Diagnostic {
        Diagnostic::error(msg, Some(self.span()))
    }

    pub fn skip_newlines(&mut self) {
        while matches!(self.peek().tok, Tok::Newline) {
            self.next();
        }
    }

    /// Skip to the start of the next line.
    pub fn skip_line(&mut self) {
        while !self.at_eol() {
            self.next();
        }
        if matches!(self.peek().tok, Tok::Newline) {
            self.next();
        }
    }
}
