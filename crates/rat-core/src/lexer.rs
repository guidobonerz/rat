//! Configurable lexer shared by all dialects.

use crate::diag::Diagnostic;
use crate::span::{FileId, Span};
use crate::token::{Tok, Token};

/// How string literals treat backslashes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrEscapes {
    /// `"..."` has backslash escapes.
    Always,
    /// Only `@"..."` has escapes (KickAssembler).
    AtPrefix,
}

#[derive(Clone, Debug)]
pub struct LexConfig {
    pub line_comments: &'static [&'static str],
    pub block_comments: bool,
    pub escapes: StrEscapes,
    /// `&777` is octal when an operand is expected (ACME).
    pub amp_octal: bool,
    /// `%..##..##` style binary with `.`/`#` digits (ACME).
    pub binary_dots: bool,
}

impl LexConfig {
    pub const ACME: LexConfig = LexConfig {
        line_comments: &[";"],
        block_comments: false,
        escapes: StrEscapes::Always,
        amp_octal: true,
        binary_dots: true,
    };
    pub const KICK: LexConfig = LexConfig {
        line_comments: &["//"],
        block_comments: true,
        escapes: StrEscapes::AtPrefix,
        amp_octal: false,
        binary_dots: false,
    };
    pub const RAT: LexConfig = LexConfig {
        line_comments: &[";", "//"],
        block_comments: true,
        escapes: StrEscapes::Always,
        amp_octal: false,
        binary_dots: false,
    };
}

const PUNCTS: &[&str] = &[
    ">>>", "..=", "<<", ">>", "<=", ">=", "==", "!=", "<>", "><", "&&", "||", "++", "--", "+=",
    "-=", "*=", "/=", "**", "..", "::", "+", "-", "*", "/", "%", "&", "|", "^", "~", "!", "<",
    ">", "=", "(", ")", "[", "]", "{", "}", ",", ":", ";", "#", ".", "@", "?", "\\", "`", "$",
];

pub fn lex(file: FileId, src: &str, cfg: &LexConfig) -> Result<Vec<Token>, Diagnostic> {
    Lexer {
        file,
        src,
        b: src.as_bytes(),
        pos: 0,
        cfg,
        out: Vec::new(),
    }
    .run()
}

struct Lexer<'a> {
    file: FileId,
    src: &'a str,
    b: &'a [u8],
    pos: usize,
    cfg: &'a LexConfig,
    out: Vec<Token>,
}

impl Lexer<'_> {
    fn span(&self, start: usize) -> Span {
        Span::new(self.file, start, self.pos)
    }

    fn err(&self, msg: impl Into<String>, start: usize) -> Diagnostic {
        Diagnostic::error(msg, Some(Span::new(self.file, start, self.pos.max(start + 1))))
    }

    fn peek(&self, n: usize) -> u8 {
        self.b.get(self.pos + n).copied().unwrap_or(0)
    }

    /// The previous token ends an operand, so `%`, `&`, `*` are operators.
    fn after_operand(&self) -> bool {
        match self.out.last().map(|t| &t.tok) {
            Some(Tok::Ident(_) | Tok::Int(_) | Tok::Float(_) | Tok::Str(_) | Tok::Char(_)) => true,
            Some(Tok::Punct(p)) => matches!(*p, ")" | "]"),
            _ => false,
        }
    }

    fn push(&mut self, tok: Tok, start: usize, space: bool) {
        let span = self.span(start);
        self.out.push(Token {
            tok,
            span,
            space_before: space,
        });
    }

    fn run(mut self) -> Result<Vec<Token>, Diagnostic> {
        let mut space = true;
        'outer: while self.pos < self.b.len() {
            let c = self.b[self.pos];
            let start = self.pos;
            if c == b'\n' {
                self.pos += 1;
                self.push(Tok::Newline, start, space);
                space = true;
                continue;
            }
            if c == b' ' || c == b'\t' || c == b'\r' || c == 0x0c {
                self.pos += 1;
                space = true;
                continue;
            }
            for lc in self.cfg.line_comments {
                if self.src[self.pos..].starts_with(lc) {
                    while self.pos < self.b.len() && self.b[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                    space = true;
                    continue 'outer;
                }
            }
            if self.cfg.block_comments && c == b'/' && self.peek(1) == b'*' {
                let s = self.pos;
                self.pos += 2;
                loop {
                    if self.pos >= self.b.len() {
                        return Err(self.err("unterminated block comment", s));
                    }
                    if self.b[self.pos] == b'*' && self.peek(1) == b'/' {
                        self.pos += 2;
                        break;
                    }
                    // keep line structure: a newline inside a comment still ends the line
                    if self.b[self.pos] == b'\n' {
                        let p = self.pos;
                        self.pos += 1;
                        self.push(Tok::Newline, p, true);
                        continue;
                    }
                    self.pos += 1;
                }
                space = true;
                continue;
            }
            // strings
            if c == b'"' || (c == b'@' && self.peek(1) == b'"') {
                let escapes = match self.cfg.escapes {
                    StrEscapes::Always => true,
                    StrEscapes::AtPrefix => c == b'@',
                };
                if c == b'@' {
                    self.pos += 1;
                }
                let s = self.string(b'"', escapes, start)?;
                self.push(Tok::Str(s), start, space);
                space = false;
                continue;
            }
            if c == b'\'' {
                if let Some(ch) = self.char_lit()? {
                    self.push(Tok::Char(ch), start, space);
                    space = false;
                    continue;
                }
            }
            if c.is_ascii_digit() {
                let tok = self.number(start)?;
                self.push(tok, start, space);
                space = false;
                continue;
            }
            if c == b'$' && self.peek(1).is_ascii_hexdigit() {
                self.pos += 1;
                let v = self.radix(16, start)?;
                self.push(Tok::Int(v), start, space);
                space = false;
                continue;
            }
            if c == b'%' && (!self.after_operand() || (space && self.peek(1) != b' ')) {
                let d = self.peek(1);
                if d == b'0' || d == b'1' || (self.cfg.binary_dots && (d == b'.' || d == b'#')) {
                    self.pos += 1;
                    let mut v: i64 = 0;
                    while self.pos < self.b.len() {
                        let ch = self.b[self.pos];
                        let bit = match ch {
                            b'0' => 0,
                            b'1' => 1,
                            b'.' if self.cfg.binary_dots => 0,
                            b'#' if self.cfg.binary_dots => 1,
                            b'_' => {
                                self.pos += 1;
                                continue;
                            }
                            _ => break,
                        };
                        v = v.wrapping_shl(1) | bit;
                        self.pos += 1;
                    }
                    self.push(Tok::Int(v), start, space);
                    space = false;
                    continue;
                }
            }
            if c == b'&' && self.cfg.amp_octal && !self.after_operand() && (b'0'..=b'7').contains(&self.peek(1)) {
                self.pos += 1;
                let v = self.radix(8, start)?;
                self.push(Tok::Int(v), start, space);
                space = false;
                continue;
            }
            if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
                while self.pos < self.b.len() {
                    let ch = self.b[self.pos];
                    if ch.is_ascii_alphanumeric() || ch == b'_' || ch >= 0x80 {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                let mut word = self.src[start..self.pos].to_owned();
                // Z80 shadow register pair `af'`
                if word.eq_ignore_ascii_case("af") && self.peek(0) == b'\'' {
                    self.pos += 1;
                    word.push('\'');
                }
                self.push(Tok::Ident(word), start, space);
                space = false;
                continue;
            }
            for p in PUNCTS {
                if self.src[self.pos..].starts_with(p) {
                    self.pos += p.len();
                    self.push(Tok::Punct(p), start, space);
                    space = false;
                    continue 'outer;
                }
            }
            let ch = self.src[self.pos..].chars().next().unwrap_or('?');
            self.pos += ch.len_utf8();
            return Err(self.err(format!("unexpected character `{ch}`"), start));
        }
        let end = self.pos;
        self.out.push(Token {
            tok: Tok::Newline,
            span: Span::new(self.file, end, end),
            space_before: true,
        });
        self.out.push(Token {
            tok: Tok::Eof,
            span: Span::new(self.file, end, end),
            space_before: true,
        });
        Ok(self.out)
    }

    fn radix(&mut self, radix: u32, start: usize) -> Result<i64, Diagnostic> {
        let mut v: i64 = 0;
        let mut any = false;
        while self.pos < self.b.len() {
            let ch = self.b[self.pos] as char;
            if ch == '_' && any {
                self.pos += 1;
                continue;
            }
            let Some(d) = ch.to_digit(radix) else { break };
            v = v.wrapping_mul(i64::from(radix)).wrapping_add(i64::from(d));
            any = true;
            self.pos += 1;
        }
        if !any {
            return Err(self.err("missing digits", start));
        }
        Ok(v)
    }

    fn number(&mut self, start: usize) -> Result<Tok, Diagnostic> {
        if self.b[self.pos] == b'0' {
            let radix = match self.peek(1) {
                b'x' | b'X' if self.peek(2).is_ascii_hexdigit() => Some(16),
                b'b' | b'B' if matches!(self.peek(2), b'0' | b'1') => Some(2),
                b'o' | b'O' if (b'0'..=b'7').contains(&self.peek(2)) => Some(8),
                _ => None,
            };
            if let Some(r) = radix {
                self.pos += 2;
                return Ok(Tok::Int(self.radix(r, start)?));
            }
        }
        while self.pos < self.b.len() && (self.b[self.pos].is_ascii_digit() || self.b[self.pos] == b'_') {
            self.pos += 1;
        }
        let mut float = false;
        if self.peek(0) == b'.' && self.peek(1).is_ascii_digit() {
            float = true;
            self.pos += 1;
            while self.pos < self.b.len() && self.b[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        if matches!(self.peek(0), b'e' | b'E')
            && (self.peek(1).is_ascii_digit()
                || (matches!(self.peek(1), b'+' | b'-') && self.peek(2).is_ascii_digit()))
            && float
        {
            self.pos += 2;
            while self.pos < self.b.len() && self.b[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        let text: String = self.src[start..self.pos].chars().filter(|&c| c != '_').collect();
        if float {
            text.parse::<f64>()
                .map(Tok::Float)
                .map_err(|_| self.err("bad number", start))
        } else {
            text.parse::<i64>()
                .map(Tok::Int)
                .or_else(|_| text.parse::<f64>().map(Tok::Float))
                .map_err(|_| self.err("bad number", start))
        }
    }

    fn escape(&mut self, start: usize) -> Result<char, Diagnostic> {
        let c = self.peek(0);
        self.pos += 1;
        Ok(match c {
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'0' => '\0',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'\\' => '\\',
            b'"' => '"',
            b'\'' => '\'',
            b'$' | b'x' => {
                let s = self.pos;
                let mut v = 0u32;
                for _ in 0..2 {
                    let Some(d) = (self.peek(0) as char).to_digit(16) else {
                        break;
                    };
                    v = v * 16 + d;
                    self.pos += 1;
                }
                if self.pos == s {
                    return Err(self.err("bad hex escape", start));
                }
                char::from_u32(v).unwrap_or('?')
            }
            0 => return Err(self.err("unterminated string", start)),
            _ => {
                self.pos -= 1;
                let ch = self.src[self.pos..].chars().next().unwrap_or('?');
                self.pos += ch.len_utf8();
                ch
            }
        })
    }

    fn string(&mut self, quote: u8, escapes: bool, start: usize) -> Result<String, Diagnostic> {
        self.pos += 1;
        let mut s = String::new();
        loop {
            if self.pos >= self.b.len() || self.b[self.pos] == b'\n' {
                return Err(self.err("unterminated string", start));
            }
            let c = self.b[self.pos];
            if c == quote {
                self.pos += 1;
                return Ok(s);
            }
            if c == b'\\' && escapes {
                self.pos += 1;
                s.push(self.escape(start)?);
                continue;
            }
            let ch = self.src[self.pos..].chars().next().unwrap_or('?');
            self.pos += ch.len_utf8();
            s.push(ch);
        }
    }

    /// `'a'` or `'\n'`; `None` if this quote does not start a char literal.
    fn char_lit(&mut self) -> Result<Option<char>, Diagnostic> {
        let start = self.pos;
        let rest = &self.src[self.pos + 1..];
        let mut it = rest.chars();
        let Some(first) = it.next() else {
            return Ok(None);
        };
        if first == '\\' && self.cfg.escapes == StrEscapes::Always {
            self.pos += 2;
            let ch = self.escape(start)?;
            if self.peek(0) != b'\'' {
                return Err(self.err("unterminated character literal", start));
            }
            self.pos += 1;
            return Ok(Some(ch));
        }
        if first == '\n' {
            return Ok(None);
        }
        if it.next() == Some('\'') {
            self.pos += 1 + first.len_utf8() + 1;
            return Ok(Some(first));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str, cfg: &LexConfig) -> Vec<Tok> {
        lex(0, src, cfg).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn numbers() {
        assert_eq!(
            toks("$ff %101 0x10 12 1.5 'a'", &LexConfig::RAT)[..6],
            [
                Tok::Int(255),
                Tok::Int(5),
                Tok::Int(16),
                Tok::Int(12),
                Tok::Float(1.5),
                Tok::Char('a')
            ]
        );
        assert_eq!(toks("%..##", &LexConfig::ACME)[0], Tok::Int(3));
        assert_eq!(toks("&17", &LexConfig::ACME)[0], Tok::Int(15));
        assert_eq!(toks("a % 2", &LexConfig::RAT)[1], Tok::Punct("%"));
    }

    #[test]
    fn comments_and_strings() {
        assert_eq!(
            toks("lda #1 ; hi\n", &LexConfig::ACME),
            [
                Tok::Ident("lda".into()),
                Tok::Punct("#"),
                Tok::Int(1),
                Tok::Newline,
                Tok::Newline,
                Tok::Eof
            ]
        );
        assert_eq!(toks("\"a\\n\"", &LexConfig::KICK)[0], Tok::Str("a\\n".into()));
        assert_eq!(toks("@\"a\\n\"", &LexConfig::KICK)[0], Tok::Str("a\n".into()));
        assert_eq!(toks("ex af,af'", &LexConfig::RAT)[3], Tok::Ident("af'".into()));
    }
}
