//! Expression grammars: ACME (its own operator table) and the C-like one
//! shared by KickAssembler and raw.

use raw_core::{AssignOp, BinOp, Dialect, Expr, ExprKind, Name, NameKind, Tok, UnOp};

use crate::cursor::{Cur, PResult};

pub fn parse_expr(c: &mut Cur<'_>, d: Dialect) -> PResult<Expr> {
    match d {
        Dialect::Acme => acme_lohi(c),
        _ => CExpr { d }.assign(c),
    }
}

/// Tokens that end an expression.
fn terminator(c: &Cur<'_>) -> bool {
    let t = c.peek();
    t.is_end()
        || [",", ")", "]", "}", ":", "{", ";"]
            .iter()
            .any(|p| t.is_punct(p))
}

/// Anonymous label reference (`-`, `--`, `+`, `++`) at a primary position:
/// a run of glued `-`/`+` followed by a terminator.
fn anon_ref(c: &mut Cur<'_>) -> Option<Expr> {
    let first = c.peek();
    let sign = if first.is_punct("-") || first.is_punct("--") {
        '-'
    } else if first.is_punct("+") || first.is_punct("++") {
        '+'
    } else {
        return None;
    };
    let mut n = 0;
    let mut k = 0;
    loop {
        let t = c.peek_at(k);
        if k > 0 && t.space_before {
            break;
        }
        let w = match &t.tok {
            Tok::Punct(p) if p.chars().all(|ch| ch == sign) => p.len(),
            _ => break,
        };
        n += w;
        k += 1;
    }
    // must be followed by a terminator
    let save = c.pos;
    c.pos += k;
    if !terminator(c) {
        c.pos = save;
        return None;
    }
    let span = c.toks[save].span.to(c.toks[c.pos - 1].span);
    let key: String = std::iter::repeat_n(sign, n).collect();
    let offset = if sign == '-' { -1 } else { 1 };
    Some(Expr::new(ExprKind::Anon { key, offset }, span))
}

fn bin(op: BinOp, a: Expr, b: Expr) -> Expr {
    let span = a.span.to(b.span);
    Expr::new(ExprKind::Binary(op, Box::new(a), Box::new(b)), span)
}

fn un(op: UnOp, e: Expr, start: raw_core::Span) -> Expr {
    let span = start.to(e.span);
    Expr::new(ExprKind::Unary(op, Box::new(e)), span)
}

fn literal(c: &mut Cur<'_>) -> Option<Expr> {
    let t = c.peek();
    let k = match &t.tok {
        Tok::Int(v) => ExprKind::Int(*v),
        Tok::Float(v) => ExprKind::Float(*v),
        Tok::Str(s) => ExprKind::Str(s.clone()),
        Tok::Char(ch) => ExprKind::Char(*ch),
        _ => return None,
    };
    c.next();
    Some(Expr::new(k, t.span))
}

fn args(c: &mut Cur<'_>, close: &str, mut item: impl FnMut(&mut Cur<'_>) -> PResult<Expr>) -> PResult<Vec<Expr>> {
    let mut v = Vec::new();
    c.skip_newlines();
    if c.eat(close) {
        return Ok(v);
    }
    loop {
        c.skip_newlines();
        v.push(item(c)?);
        c.skip_newlines();
        if c.eat(",") {
            continue;
        }
        c.expect(close)?;
        return Ok(v);
    }
}

// ---------------------------------------------------------------- ACME

fn acme_word(c: &Cur<'_>, w: &str) -> bool {
    c.peek().is_ident(w)
}

fn acme_lohi(c: &mut Cur<'_>) -> PResult<Expr> {
    let start = c.span();
    let op = if c.is("<") {
        Some(UnOp::Lo)
    } else if c.is(">") {
        Some(UnOp::Hi)
    } else if c.is("^") {
        Some(UnOp::Bank)
    } else {
        None
    };
    if let Some(op) = op {
        c.next();
        let e = acme_lohi(c)?;
        return Ok(un(op, e, start));
    }
    acme_or(c)
}

fn acme_or(c: &mut Cur<'_>) -> PResult<Expr> {
    let mut a = acme_xor(c)?;
    while c.is("|") || acme_word(c, "or") {
        c.next();
        a = bin(BinOp::BitOr, a, acme_xor(c)?);
    }
    Ok(a)
}

fn acme_xor(c: &mut Cur<'_>) -> PResult<Expr> {
    let mut a = acme_and(c)?;
    while acme_word(c, "xor") || acme_word(c, "eor") {
        c.next();
        a = bin(BinOp::BitXor, a, acme_and(c)?);
    }
    Ok(a)
}

fn acme_and(c: &mut Cur<'_>) -> PResult<Expr> {
    let mut a = acme_eq(c)?;
    while c.is("&") || acme_word(c, "and") {
        c.next();
        a = bin(BinOp::BitAnd, a, acme_eq(c)?);
    }
    Ok(a)
}

fn acme_eq(c: &mut Cur<'_>) -> PResult<Expr> {
    let mut a = acme_cmp(c)?;
    loop {
        let op = if c.is("=") || c.is("==") {
            BinOp::Eq
        } else if c.is("!=") || c.is("<>") || c.is("><") {
            BinOp::Ne
        } else {
            return Ok(a);
        };
        c.next();
        a = bin(op, a, acme_cmp(c)?);
    }
}

fn acme_cmp(c: &mut Cur<'_>) -> PResult<Expr> {
    let mut a = acme_shift(c)?;
    loop {
        let op = if c.is("<=") {
            BinOp::Le
        } else if c.is(">=") {
            BinOp::Ge
        } else if c.is("<") {
            BinOp::Lt
        } else if c.is(">") {
            BinOp::Gt
        } else {
            return Ok(a);
        };
        c.next();
        a = bin(op, a, acme_shift(c)?);
    }
}

fn acme_shift(c: &mut Cur<'_>) -> PResult<Expr> {
    let mut a = acme_add(c)?;
    loop {
        let op = if c.is("<<") || acme_word(c, "asl") || acme_word(c, "lsl") {
            BinOp::Shl
        } else if c.is(">>") || acme_word(c, "asr") {
            BinOp::Shr
        } else if c.is(">>>") || acme_word(c, "lsr") {
            BinOp::Lsr
        } else {
            return Ok(a);
        };
        c.next();
        a = bin(op, a, acme_add(c)?);
    }
}

fn acme_add(c: &mut Cur<'_>) -> PResult<Expr> {
    let mut a = acme_mul(c)?;
    loop {
        let op = if c.is("+") {
            BinOp::Add
        } else if c.is("-") {
            BinOp::Sub
        } else {
            return Ok(a);
        };
        c.next();
        a = bin(op, a, acme_mul(c)?);
    }
}

fn acme_mul(c: &mut Cur<'_>) -> PResult<Expr> {
    let mut a = acme_neg(c)?;
    loop {
        let op = if c.is("*") {
            BinOp::Mul
        } else if c.is("/") {
            BinOp::Div
        } else if acme_word(c, "div") {
            BinOp::IntDiv
        } else if c.is("%") || acme_word(c, "mod") {
            BinOp::Mod
        } else {
            return Ok(a);
        };
        c.next();
        a = bin(op, a, acme_neg(c)?);
    }
}

fn acme_neg(c: &mut Cur<'_>) -> PResult<Expr> {
    if let Some(e) = anon_ref(c) {
        return Ok(e);
    }
    let start = c.span();
    if c.eat("-") {
        let e = acme_neg(c)?;
        return Ok(un(UnOp::Neg, e, start));
    }
    if c.eat("+") {
        return acme_neg(c);
    }
    acme_pow(c)
}

fn acme_pow(c: &mut Cur<'_>) -> PResult<Expr> {
    let a = acme_not(c)?;
    if c.is("^") {
        c.next();
        let b = acme_neg(c)?;
        return Ok(bin(BinOp::Pow, a, b));
    }
    Ok(a)
}

fn acme_not(c: &mut Cur<'_>) -> PResult<Expr> {
    let start = c.span();
    if c.eat("!") || acme_word(c, "not") && {
        c.next();
        true
    } {
        let e = acme_not(c)?;
        return Ok(un(UnOp::BitNot, e, start));
    }
    acme_postfix(c)
}

fn acme_postfix(c: &mut Cur<'_>) -> PResult<Expr> {
    let mut e = acme_primary(c)?;
    while c.is("[") && c.glued_at(0) {
        c.next();
        let i = acme_lohi(c)?;
        c.expect("]")?;
        let span = e.span.to(i.span);
        e = Expr::new(ExprKind::Index(Box::new(e), Box::new(i)), span);
    }
    Ok(e)
}

/// ACME name: `name`, `.local`, `@cheap`.
pub fn acme_name(c: &mut Cur<'_>) -> Option<(Name, raw_core::Span)> {
    let t = c.peek();
    if (t.is_punct(".") || t.is_punct("@")) && c.glued_at(1) {
        if let Tok::Ident(s) = &c.peek_at(1).tok {
            let kind = if t.is_punct(".") { NameKind::ZoneLocal } else { NameKind::Cheap };
            let span = t.span.to(c.peek_at(1).span);
            let name = Name { kind, text: s.clone() };
            c.next();
            c.next();
            return Some((name, span));
        }
    }
    if let Tok::Ident(s) = &t.tok {
        c.next();
        return Some((Name::plain(s.clone()), t.span));
    }
    None
}

fn acme_primary(c: &mut Cur<'_>) -> PResult<Expr> {
    if let Some(e) = literal(c) {
        return Ok(e);
    }
    let t = c.peek();
    if t.is_punct("*") {
        c.next();
        return Ok(Expr::new(ExprKind::Pc, t.span));
    }
    if t.is_punct("(") {
        c.next();
        let e = acme_lohi(c)?;
        c.expect(")")?;
        return Ok(e);
    }
    if t.is_punct("[") {
        c.next();
        let items = args(c, "]", acme_lohi)?;
        return Ok(Expr::new(ExprKind::List(items), t.span.to(c.toks[c.pos - 1].span)));
    }
    if let Some((name, span)) = acme_name(c) {
        if name.kind == NameKind::Plain && c.is("(") && c.glued_at(0) {
            c.next();
            let a = args(c, ")", acme_lohi)?;
            let callee = Expr::new(ExprKind::Ident(name), span);
            return Ok(Expr::new(ExprKind::Call(Box::new(callee), a), span.to(c.toks[c.pos - 1].span)));
        }
        return Ok(Expr::new(ExprKind::Ident(name), span));
    }
    Err(c.error(format!("expected an expression, found {}", t.tok)))
}

// ------------------------------------------------------- Kick and raw

struct CExpr {
    d: Dialect,
}

impl CExpr {
    fn assign(&self, c: &mut Cur<'_>) -> PResult<Expr> {
        let lhs = self.ternary(c)?;
        let op = if c.is("=") {
            AssignOp::Set
        } else if c.is("+=") {
            AssignOp::Add
        } else if c.is("-=") {
            AssignOp::Sub
        } else if c.is("*=") {
            AssignOp::Mul
        } else if c.is("/=") {
            AssignOp::Div
        } else {
            return Ok(lhs);
        };
        c.next();
        let rhs = self.assign(c)?;
        let span = lhs.span.to(rhs.span);
        Ok(Expr::new(ExprKind::Assign(op, Box::new(lhs), Box::new(rhs)), span))
    }

    fn ternary(&self, c: &mut Cur<'_>) -> PResult<Expr> {
        let start = c.span();
        // `<expr` / `>expr`: low byte / high byte of the whole expression
        if (c.is("<") || c.is(">")) && !matches!(c.peek_at(1).tok, Tok::Newline | Tok::Eof) {
            let op = if c.is("<") { UnOp::Lo } else { UnOp::Hi };
            c.next();
            let e = self.ternary(c)?;
            return Ok(un(op, e, start));
        }
        let cond = self.binary(c, 1)?;
        if c.eat("?") {
            let a = self.assign(c)?;
            c.expect(":")?;
            let b = self.assign(c)?;
            let span = cond.span.to(b.span);
            return Ok(Expr::new(ExprKind::Ternary(Box::new(cond), Box::new(a), Box::new(b)), span));
        }
        Ok(cond)
    }

    fn binop(&self, c: &Cur<'_>) -> Option<(u8, BinOp)> {
        let t = c.peek();
        let Tok::Punct(p) = t.tok else { return None };
        Some(match p {
            "||" => (1, BinOp::LOr),
            "&&" => (2, BinOp::LAnd),
            "|" => (3, BinOp::BitOr),
            "^" => (4, BinOp::BitXor),
            "&" => (5, BinOp::BitAnd),
            "==" => (6, BinOp::Eq),
            "!=" | "<>" => (6, BinOp::Ne),
            "<" => (7, BinOp::Lt),
            "<=" => (7, BinOp::Le),
            ">" => (7, BinOp::Gt),
            ">=" => (7, BinOp::Ge),
            "<<" => (8, BinOp::Shl),
            ">>" => (8, BinOp::Shr),
            ">>>" => (8, BinOp::Lsr),
            "+" => (9, BinOp::Add),
            "-" => (9, BinOp::Sub),
            "*" => (10, BinOp::Mul),
            "/" => (10, if self.d == Dialect::Kick { BinOp::FDiv } else { BinOp::Div }),
            "%" => (10, BinOp::Mod),
            "**" if self.d == Dialect::Raw => (11, BinOp::Pow),
            _ => return None,
        })
    }

    /// Precedence climbing; `**` (11) is right associative.
    fn binary(&self, c: &mut Cur<'_>, min: u8) -> PResult<Expr> {
        let mut a = self.unary(c)?;
        while let Some((prec, op)) = self.binop(c) {
            if prec < min.max(1) {
                break;
            }
            c.next();
            let b = self.binary(c, if prec == 11 { prec } else { prec + 1 })?;
            a = bin(op, a, b);
        }
        Ok(a)
    }

    fn unary(&self, c: &mut Cur<'_>) -> PResult<Expr> {
        if self.d == Dialect::Raw {
            if let Some(e) = anon_ref(c) {
                return Ok(e);
            }
        }
        let start = c.span();
        let op = if c.is("-") {
            Some(UnOp::Neg)
        } else if c.is("+") {
            Some(UnOp::Pos)
        } else if c.is("!") && !self.kick_anon_ahead(c) {
            Some(UnOp::Not)
        } else if c.is("~") {
            Some(UnOp::BitNot)
        } else if c.is("<") {
            Some(UnOp::Lo)
        } else if c.is(">") {
            Some(UnOp::Hi)
        } else {
            None
        };
        if let Some(op) = op {
            c.next();
            let e = self.unary(c)?;
            return Ok(un(op, e, start));
        }
        if c.is("++") || c.is("--") {
            let inc = c.is("++");
            c.next();
            let e = self.unary(c)?;
            let span = start.to(e.span);
            return Ok(Expr::new(ExprKind::IncDec { target: Box::new(e), inc, post: false }, span));
        }
        self.postfix(c)
    }

    /// Kick `!+`, `!-`, `!name+` anonymous references.
    fn kick_anon_ahead(&self, c: &Cur<'_>) -> bool {
        if self.d != Dialect::Kick || !c.glued_at(1) {
            return false;
        }
        let n1 = c.peek_at(1);
        let pm = |t: &raw_core::Token| t.is_punct("+") || t.is_punct("-") || t.is_punct("++") || t.is_punct("--");
        pm(n1) || (matches!(n1.tok, Tok::Ident(_)) && c.glued_at(2) && pm(c.peek_at(2)))
    }

    fn kick_anon(&self, c: &mut Cur<'_>) -> Option<Expr> {
        if !(c.is("!") && self.kick_anon_ahead(c)) {
            return None;
        }
        let start = c.span();
        c.next();
        let mut key = "!".to_string();
        if let Tok::Ident(s) = &c.peek().tok {
            key.push_str(s);
            c.next();
        }
        let mut n: i32 = 0;
        let mut sign = 0;
        while c.glued_at(0) {
            let t = c.peek();
            let (s, w) = match &t.tok {
                Tok::Punct("+") => (1, 1),
                Tok::Punct("++") => (1, 2),
                Tok::Punct("-") => (-1, 1),
                Tok::Punct("--") => (-1, 2),
                _ => break,
            };
            if sign != 0 && s != sign {
                break;
            }
            sign = s;
            n += w;
            c.next();
        }
        let span = start.to(c.toks[c.pos - 1].span);
        Some(Expr::new(ExprKind::Anon { key, offset: sign * n }, span))
    }

    fn postfix(&self, c: &mut Cur<'_>) -> PResult<Expr> {
        let mut e = self.primary(c)?;
        loop {
            if c.is("(") && c.glued_at(0) {
                c.next();
                let a = args(c, ")", |c| self.assign(c))?;
                let span = e.span.to(c.toks[c.pos - 1].span);
                e = Expr::new(ExprKind::Call(Box::new(e), a), span);
            } else if c.is(".") && c.glued_at(0) && c.glued_at(1) && matches!(c.peek_at(1).tok, Tok::Ident(_)) {
                c.next();
                let name = c.ident()?;
                let span = e.span.to(c.toks[c.pos - 1].span);
                e = Expr::new(ExprKind::Field(Box::new(e), name), span);
            } else if c.is("[") && c.glued_at(0) && self.d == Dialect::Raw {
                c.next();
                let i = self.assign(c)?;
                c.expect("]")?;
                let span = e.span.to(c.toks[c.pos - 1].span);
                e = Expr::new(ExprKind::Index(Box::new(e), Box::new(i)), span);
            } else if (c.is("++") || c.is("--")) && c.glued_at(0) && matches!(e.kind, ExprKind::Ident(_) | ExprKind::Field(..) | ExprKind::Index(..)) {
                let inc = c.is("++");
                c.next();
                let span = e.span.to(c.toks[c.pos - 1].span);
                e = Expr::new(ExprKind::IncDec { target: Box::new(e), inc, post: true }, span);
            } else {
                return Ok(e);
            }
        }
    }

    fn primary(&self, c: &mut Cur<'_>) -> PResult<Expr> {
        if let Some(e) = literal(c) {
            return Ok(e);
        }
        if let Some(e) = self.kick_anon(c) {
            return Ok(e);
        }
        let t = c.peek();
        if t.is_punct("*") || t.is_punct("$") {
            c.next();
            return Ok(Expr::new(ExprKind::Pc, t.span));
        }
        if t.is_punct("(") {
            c.next();
            c.skip_newlines();
            let e = self.assign(c)?;
            c.skip_newlines();
            c.expect(")")?;
            return Ok(e);
        }
        if t.is_punct("[") {
            c.next();
            if self.d == Dialect::Kick {
                let e = self.assign(c)?;
                if c.eat("]") {
                    return Ok(e);
                }
                // `[a, b, ...]`: a list (`.fill 4, [$10, $20]`)
                let mut items = vec![e];
                while c.eat(",") {
                    items.push(self.assign(c)?);
                }
                c.expect("]")?;
                return Ok(Expr::new(ExprKind::List(items), t.span.to(c.toks[c.pos - 1].span)));
            }
            let items = args(c, "]", |c| self.assign(c))?;
            return Ok(Expr::new(ExprKind::List(items), t.span.to(c.toks[c.pos - 1].span)));
        }
        if t.is_punct("@") && self.d == Dialect::Kick && c.glued_at(1) {
            if let Tok::Ident(s) = &c.peek_at(1).tok {
                c.next();
                c.next();
                let span = t.span.to(c.toks[c.pos - 1].span);
                return Ok(Expr::new(ExprKind::Ident(Name { kind: NameKind::Root, text: s.clone() }), span));
            }
        }
        if (t.is_punct("@") || t.is_punct(".")) && self.d == Dialect::Raw && c.glued_at(1) {
            if let Tok::Ident(s) = &c.peek_at(1).tok {
                c.next();
                c.next();
                let span = t.span.to(c.toks[c.pos - 1].span);
                return Ok(Expr::new(ExprKind::Ident(Name { kind: NameKind::Cheap, text: s.clone() }), span));
            }
        }
        if let Tok::Ident(s) = &t.tok {
            c.next();
            let k = match s.as_str() {
                "true" => ExprKind::Bool(true),
                "false" => ExprKind::Bool(false),
                "null" => ExprKind::Null,
                _ => ExprKind::Ident(Name::plain(s.clone())),
            };
            return Ok(Expr::new(k, t.span));
        }
        Err(c.error(format!("expected an expression, found {}", t.tok)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use raw_core::lexer::{lex, LexConfig};

    fn p(src: &str, d: Dialect) -> ExprKind {
        let cfg = match d {
            Dialect::Acme => LexConfig::ACME,
            Dialect::Kick => LexConfig::KICK,
            Dialect::Raw => LexConfig::RAW,
        };
        let toks = lex(0, src, &cfg).unwrap();
        let mut c = Cur::new(&toks);
        parse_expr(&mut c, d).unwrap().kind
    }

    #[test]
    fn precedence() {
        let ExprKind::Binary(BinOp::Add, _, b) = p("1+2*3", Dialect::Kick) else { panic!() };
        assert!(matches!(b.kind, ExprKind::Binary(BinOp::Mul, ..)));
        assert!(matches!(p("<x+1", Dialect::Kick), ExprKind::Unary(UnOp::Lo, _)));
        assert!(matches!(p("<x+1", Dialect::Acme), ExprKind::Unary(UnOp::Lo, _)));
        assert!(matches!(p("a = 2", Dialect::Acme), ExprKind::Binary(BinOp::Eq, ..)));
        assert!(matches!(p("-", Dialect::Acme), ExprKind::Anon { offset: -1, .. }));
        assert!(matches!(p("!+", Dialect::Kick), ExprKind::Anon { offset: 1, .. }));
        assert!(matches!(p("!x", Dialect::Kick), ExprKind::Unary(UnOp::Not, _)));
        assert!(matches!(p("list.get(1)", Dialect::Kick), ExprKind::Call(..)));
        assert!(matches!(p("i++", Dialect::Kick), ExprKind::IncDec { post: true, .. }));
    }
}
