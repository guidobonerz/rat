//! ACME front end: `!` pseudo-ops, labels without colons, `.local` and
//! `@cheap` names, anonymous `-`/`+` labels, `+macro` calls, `:` as
//! statement separator.

use std::rc::Rc;

use rat_core::*;

use crate::cursor::PResult;
use crate::expr::acme_name;
use crate::{arg_list, Parser};

pub(crate) fn file(p: &mut Parser<'_>) -> Vec<Stmt> {
    items(p, false)
}

fn items(p: &mut Parser<'_>, in_block: bool) -> Vec<Stmt> {
    let mut out = Vec::new();
    loop {
        while p.c.eat(":") || matches!(p.c.peek().tok, Tok::Newline) {
            p.c.next_if_newline();
        }
        if p.c.at_eof() {
            if in_block {
                let d = p.c.error("missing `}`");
                p.error(d);
            }
            return out;
        }
        if p.c.is("}") {
            if in_block {
                p.c.next();
                return out;
            }
            let d = p.c.error("unexpected `}`");
            p.error(d);
            p.c.next();
            continue;
        }
        match statement(p, &mut out) {
            Ok(()) => {
                if !p.stmt_end() {
                    let d = p.c.error(format!("unexpected {}", p.c.peek().tok));
                    p.error(d);
                    p.recover();
                }
            }
            Err(d) => {
                p.error(d);
                p.recover();
            }
        }
    }
}

fn block(p: &mut Parser<'_>) -> PResult<Block> {
    p.c.skip_newlines();
    p.c.expect("{")?;
    Ok(items(p, true))
}

fn opt_block(p: &mut Parser<'_>) -> PResult<Option<Block>> {
    if p.c.is("{") {
        Ok(Some(block(p)?))
    } else {
        Ok(None)
    }
}

fn str_expr(s: &str, span: Span) -> Expr {
    Expr::string(s, span)
}

/// Raw text of a pseudo-op argument (`!cpu 65c02`, `!source <lib/x.a>`).
fn word_text(p: &mut Parser<'_>) -> String {
    let start = p.c.span().start as usize;
    let mut end = start;
    while !p.stmt_end() && !p.c.is("{") && !p.c.is(",") {
        end = p.c.peek().span.end as usize;
        p.c.next();
    }
    p.src.get(start..end).unwrap_or("").trim().to_owned()
}

fn statement(p: &mut Parser<'_>, out: &mut Vec<Stmt>) -> PResult<()> {
    let start = p.c.pos;
    let sp = p.c.span();
    // *= addr [, overlay] [, invisible]
    if p.c.is("*=") || (p.c.is("*") && p.c.is_at(1, "=")) {
        if p.c.is("*") {
            p.c.next();
        }
        p.c.next();
        let addr = p.expr()?;
        let (mut overlay, mut invisible) = (false, false);
        while p.c.eat(",") {
            let w = p.c.ident()?.to_ascii_lowercase();
            match w.as_str() {
                "overlay" => overlay = true,
                "invisible" => invisible = true,
                _ => return Err(p.c.error(format!("unknown `*=` keyword `{w}`"))),
            }
        }
        out.push(Stmt::new(StmtKind::Org { addr, name: None, overlay, invisible }, p.c.span_from(start)));
        return Ok(());
    }
    // +macro call
    if p.c.is("+") && p.c.glued_at(1) && matches!(p.c.peek_at(1).tok, Tok::Ident(_)) {
        p.c.next();
        let name = p.c.ident()?;
        let span = p.c.span_from(start);
        let mut args = Vec::new();
        if !p.stmt_end() {
            loop {
                if p.c.eat("~") {
                    let (n, s) = acme_name(&mut p.c).ok_or_else(|| p.c.error("expected a name after `~`"))?;
                    args.push(Expr::new(ExprKind::Ident(n), s));
                } else {
                    args.push(p.expr()?);
                }
                if !p.c.eat(",") {
                    break;
                }
            }
        }
        out.push(Stmt::new(StmtKind::Call { name: Expr::ident(name, span), args, label: None }, p.c.span_from(start)));
        return Ok(());
    }
    // anonymous labels `-`, `--`, `+`, `++`
    if let Tok::Punct(first @ ("-" | "--" | "+" | "++")) = p.c.peek().tok {
        let sign = first.chars().next().unwrap();
        let mut key = String::new();
        loop {
            match &p.c.peek().tok {
                Tok::Punct(q) if q.chars().all(|ch| ch == sign) && (key.is_empty() || p.c.glued_at(0)) => {
                    key.push_str(q);
                    p.c.next();
                }
                _ => break,
            }
        }
        out.push(Stmt::new(StmtKind::AnonLabel(key), sp));
        if !p.stmt_end() {
            statement(p, out)?;
        }
        return Ok(());
    }
    // pseudo op
    if p.c.is("!") && p.c.glued_at(1) && matches!(p.c.peek_at(1).tok, Tok::Ident(_) | Tok::Int(_)) {
        p.c.next();
        let name = match &p.c.next().tok {
            Tok::Ident(s) => s.to_ascii_lowercase(),
            Tok::Int(v) => v.to_string(),
            _ => unreachable!(),
        };
        return pseudo(p, name, start, out);
    }
    if let Some((name, nspan)) = acme_name(&mut p.c) {
        if p.c.is("=") {
            p.c.next();
            let value = p.expr()?;
            out.push(Stmt::new(StmtKind::Assign { name, value, kind: AssignKind::Label }, p.c.span_from(start)));
            return Ok(());
        }
        // `name:` is always a label, even when it spells a mnemonic
        let colon = p.c.is(":") && p.c.glued_at(0);
        if name.kind == NameKind::Plain && !colon && p.is_mnemonic(&name.text) {
            // `lda+2 $12`: forced operand size
            let mut suffix = None;
            if p.c.is("+") && p.c.glued_at(0) && p.c.glued_at(1) {
                if let Tok::Int(n @ 1..=3) = p.c.peek_at(1).tok {
                    suffix = Some(n.to_string());
                    p.c.next();
                    p.c.next();
                }
            }
            if suffix.is_none() && p.c.is(".") && p.c.glued_at(0) && p.c.glued_at(1) {
                if let Tok::Ident(sx) = &p.c.peek_at(1).tok {
                    suffix = Some(sx.clone());
                    p.c.next();
                    p.c.next();
                }
            }
            out.push(p.instr(name.text, suffix, start));
            return Ok(());
        }
        out.push(Stmt::new(StmtKind::Label(name), nspan));
        p.c.eat(":");
        if !p.stmt_end() {
            statement(p, out)?;
        }
        return Ok(());
    }
    Err(p.c.error(format!("unexpected {}", p.c.peek().tok)))
}

fn push_pop(kind_push: StmtKind, body: Block, kind_pop: StmtKind, span: Span) -> Stmt {
    let mut v = vec![Stmt::new(kind_push, span)];
    v.extend(body);
    v.push(Stmt::new(kind_pop, span));
    Stmt::new(StmtKind::Group(v), span)
}

fn data(p: &mut Parser<'_>, size: u8, endian: Option<Endian>) -> PResult<StmtKind> {
    Ok(StmtKind::Data { size, endian, items: p.expr_list()? })
}

fn pseudo(p: &mut Parser<'_>, name: String, start: usize, out: &mut Vec<Stmt>) -> PResult<()> {
    let le = Some(Endian::Little);
    let be = Some(Endian::Big);
    let span = p.c.span_from(start);
    let kind = match name.as_str() {
        "byte" | "by" | "db" | "8" | "08" => data(p, 1, le)?,
        "word" | "wo" | "dw" | "16" | "le16" => data(p, 2, le)?,
        "be16" => data(p, 2, be)?,
        "24" | "le24" => data(p, 3, le)?,
        "be24" => data(p, 3, be)?,
        "32" | "le32" => data(p, 4, le)?,
        "be32" => data(p, 4, be)?,
        "hex" | "h" => {
            let text = p.rest_of_line_text();
            let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace() && *b != b',').collect();
            if !digits.len().is_multiple_of(2) || !digits.iter().all(u8::is_ascii_hexdigit) {
                return Err(Diagnostic::error("`!hex` needs pairs of hex digits", Some(span)));
            }
            let bytes = digits
                .chunks(2)
                .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
                .collect();
            StmtKind::Hex(bytes)
        }
        "fill" | "fi" => {
            let count = p.expr()?;
            let value = if p.c.eat(",") { Some(p.expr()?) } else { None };
            StmtKind::Fill { count, value, size: 1, index_var: None }
        }
        "skip" => StmtKind::Skip(p.expr()?),
        "align" => {
            let and = p.expr()?;
            p.c.expect(",")?;
            let equal = p.expr()?;
            let fill = if p.c.eat(",") { Some(p.expr()?) } else { None };
            StmtKind::Align { kind: AlignKind::Mask { and, equal }, fill }
        }
        "text" | "tx" => StmtKind::Text { conv: TextConv::Current, xor: None, items: p.expr_list()? },
        "pet" => StmtKind::Text { conv: TextConv::Petscii, xor: None, items: p.expr_list()? },
        "raw" => StmtKind::Text { conv: TextConv::Raw, xor: None, items: p.expr_list()? },
        "scr" => StmtKind::Text { conv: TextConv::Screen, xor: None, items: p.expr_list()? },
        "scrxor" => {
            let x = p.expr()?;
            p.c.expect(",")?;
            StmtKind::Text { conv: TextConv::Screen, xor: Some(x), items: p.expr_list()? }
        }
        "convtab" | "ct" | "cbm" => {
            let enc = if name == "cbm" {
                StmtKind::Encoding(str_expr("pet", span))
            } else if matches!(p.c.peek().tok, Tok::Str(_)) {
                StmtKind::ConvTabFile(p.expr()?)
            } else {
                let w = p.c.ident()?.to_ascii_lowercase();
                StmtKind::Encoding(str_expr(&w, span))
            };
            if let Some(b) = opt_block(p)? {
                out.push(push_pop(enc, b, StmtKind::Encoding(str_expr("\0pop", span)), span));
                return Ok(());
            }
            enc
        }
        "source" | "src" => {
            let path = if matches!(p.c.peek().tok, Tok::Str(_)) {
                p.expr()?
            } else {
                // <library/file.a>
                let t = word_text(p);
                str_expr(t.trim_start_matches('<').trim_end_matches('>'), span)
            };
            StmtKind::Include { path, once: false }
        }
        "binary" | "bin" | "bi" => {
            let path = p.expr()?;
            let mut size = None;
            let mut offset = None;
            if p.c.eat(",") {
                if !p.c.is(",") {
                    size = Some(p.expr()?);
                }
                if p.c.eat(",") {
                    offset = Some(p.expr()?);
                }
            }
            StmtKind::Binary { path, size, offset, kind: BinaryKind::Raw }
        }
        "to" | "output" => {
            let file = p.expr()?;
            let format = if p.c.eat(",") { Some(p.c.ident()?.to_ascii_lowercase()) } else { None };
            StmtKind::Output { file, format }
        }
        "sl" | "symbollist" => StmtKind::SymbolList(p.expr()?),
        "cpu" => {
            let cpu = word_text(p);
            let prev = p.cpu.info().id;
            p.set_cpu(&cpu);
            if let Some(b) = opt_block(p)? {
                p.set_cpu(prev);
                out.push(push_pop(StmtKind::Cpu(format!("+{cpu}")), b, StmtKind::Cpu("-".into()), span));
                return Ok(());
            }
            StmtKind::Cpu(cpu)
        }
        "platform" => {
            let name = word_text(p);
            p.set_platform(&name);
            StmtKind::Platform { name, body: opt_block(p)? }
        }
        "syntax" => StmtKind::Syntax(word_text(p)),
        "zone" | "zn" => {
            let name = if let Tok::Ident(s) = &p.c.peek().tok {
                let s = s.clone();
                p.c.next();
                Some(s)
            } else {
                None
            };
            StmtKind::Zone { name, body: opt_block(p)? }
        }
        "if" => if_chain(p)?,
        "ifdef" | "ifndef" => {
            let (n, _) = acme_name(&mut p.c).ok_or_else(|| p.c.error("expected a name"))?;
            let negate = name == "ifndef";
            let (then, else_) = if p.c.is("{") {
                let then = block(p)?;
                (then, else_part(p)?)
            } else {
                // old style: `!ifdef name statement`
                let mut v = Vec::new();
                statement(p, &mut v)?;
                (v, None)
            };
            StmtKind::IfDef { name: n, negate, then, else_ }
        }
        "for" => {
            let (var, _) = acme_name(&mut p.c).ok_or_else(|| p.c.error("expected a loop variable"))?;
            if p.c.eat_ident("in") {
                let list = p.expr()?;
                StmtKind::ForIn { var: var.text, list, body: block(p)? }
            } else {
                p.c.expect(",")?;
                let a = p.expr()?;
                let (from, to) = if p.c.eat(",") {
                    (a, p.expr()?)
                } else {
                    (Expr::int(1, span), a)
                };
                StmtKind::ForRange { var: var.text, from, to, step: None, inclusive: true, body: block(p)? }
            }
        }
        "set" => {
            let (n, _) = acme_name(&mut p.c).ok_or_else(|| p.c.error("expected a name"))?;
            p.c.expect("=")?;
            StmtKind::Assign { name: n, value: p.expr()?, kind: AssignKind::Set }
        }
        "do" => {
            let pre = loop_cond(p)?;
            let body = block(p)?;
            let post = loop_cond(p)?;
            StmtKind::Do { pre, body, post }
        }
        "while" => {
            let cond = p.expr()?;
            StmtKind::While { cond, body: block(p)? }
        }
        "macro" => {
            let mname = p.c.ident()?;
            let mut params = Vec::new();
            while !p.c.is("{") && !p.stmt_end() {
                let by_ref = p.c.eat("~");
                let (n, _) = acme_name(&mut p.c).ok_or_else(|| p.c.error("expected a parameter name"))?;
                let pname = if n.kind == NameKind::ZoneLocal { format!(".{}", n.text) } else { n.text };
                params.push(Param { name: pname, by_ref, default: None });
                if !p.c.eat(",") {
                    break;
                }
            }
            let body = block(p)?;
            StmtKind::MacroDef(Rc::new(MacroDef {
                name: mname,
                params,
                variadic: false,
                body,
                kind: MacroKind::Macro,
                dialect: Dialect::Acme,
                span,
                expr_body: None,
            }))
        }
        "eof" | "endoffile" => StmtKind::Eof,
        "warn" => StmtKind::Message { level: MsgLevel::Warn, args: p.expr_list()? },
        "error" => StmtKind::Message { level: MsgLevel::Error, args: p.expr_list()? },
        "serious" => StmtKind::Message { level: MsgLevel::Serious, args: p.expr_list()? },
        "print" | "info" => StmtKind::Print { expr: concat(p.expr_list()?, span), now: false },
        "pseudopc" => {
            let addr = p.expr()?;
            StmtKind::PseudoPc { addr, body: opt_block(p)? }
        }
        "realpc" => StmtKind::RealPc,
        "initmem" => StmtKind::InitMem(p.expr()?),
        "xor" => {
            let value = p.expr()?;
            StmtKind::Xor { value, body: opt_block(p)? }
        }
        "address" | "addr" => match opt_block(p)? {
            Some(b) => StmtKind::Group(b),
            None => StmtKind::Nop,
        },
        "al" | "as" | "rl" | "rs" | "a8" | "a16" | "i8" | "i16" => {
            let flag = match name.as_str() {
                "al" | "a16" => "a16",
                "as" | "a8" => "a8",
                "rl" | "i16" => "i16",
                _ => "i8",
            };
            if let Some(b) = opt_block(p)? {
                let mut v = vec![Stmt::new(StmtKind::CpuFlag("push".into()), span), Stmt::new(StmtKind::CpuFlag(flag.into()), span)];
                v.extend(b);
                v.push(Stmt::new(StmtKind::CpuFlag("pop".into()), span));
                StmtKind::Group(v)
            } else {
                StmtKind::CpuFlag(flag.into())
            }
        }
        "setdp" => StmtKind::SetDp(p.expr()?),
        "endian" => {
            let w = p.c.ident()?.to_ascii_lowercase();
            StmtKind::Endian(match w.as_str() {
                "le" | "little" => Some(Endian::Little),
                "be" | "big" => Some(Endian::Big),
                _ => None,
            })
        }
        "even" => StmtKind::Align { kind: AlignKind::Multiple(Expr::int(2, span)), fill: None },
        "assert" => {
            let actual = p.expr()?;
            let desc = if p.c.eat(",") { Some(p.expr()?) } else { None };
            StmtKind::Assert { desc, actual, expected: None }
        }
        _ => {
            // platform directive, optionally namespaced: !c64.basic ...
            let mut ns = None;
            let mut dname = name.clone();
            if p.c.is(".") && p.c.glued_at(0) && p.c.glued_at(1) {
                p.c.next();
                ns = Some(dname);
                dname = p.c.ident()?.to_ascii_lowercase();
            }
            let args = arg_list(p)?;
            // `.snes.spc { }` holds SPC700 code
            let saved = p.cpu;
            if dname == "spc" {
                p.set_cpu("spc700");
            }
            let body = opt_block(p);
            p.cpu = saved;
            let body = body?;
            StmtKind::PlatformDir { ns, name: dname, args, body }
        }
    };
    out.push(Stmt::new(kind, p.c.span_from(start)));
    Ok(())
}

/// `!print a, b` prints the items one after the other.
fn concat(items: Vec<Expr>, span: Span) -> Expr {
    let mut it = items.into_iter();
    let Some(mut e) = it.next() else { return Expr::string("", span) };
    for x in it {
        let s = e.span.to(x.span);
        let as_str = Expr::new(ExprKind::Call(Box::new(Expr::ident("\0str", s)), vec![x]), s);
        e = Expr::new(ExprKind::Binary(BinOp::Add, Box::new(Expr::new(ExprKind::Call(Box::new(Expr::ident("\0str", s)), vec![e]), s)), Box::new(as_str)), s);
    }
    e
}

fn loop_cond(p: &mut Parser<'_>) -> PResult<Option<(bool, Expr)>> {
    if p.c.eat_ident("while") {
        return Ok(Some((false, p.expr()?)));
    }
    if p.c.eat_ident("until") {
        return Ok(Some((true, p.expr()?)));
    }
    Ok(None)
}

fn if_chain(p: &mut Parser<'_>) -> PResult<StmtKind> {
    let cond = p.expr()?;
    let then = block(p)?;
    let else_ = else_part(p)?;
    Ok(StmtKind::If { cond, then, else_ })
}

fn else_part(p: &mut Parser<'_>) -> PResult<Option<Block>> {
    if !p.c.is_ident("else") {
        return Ok(None);
    }
    let sp = p.c.span();
    p.c.next();
    if p.c.eat_ident("if") {
        let k = if_chain(p)?;
        return Ok(Some(vec![Stmt::new(k, sp)]));
    }
    Ok(Some(block(p)?))
}

impl crate::cursor::Cur<'_> {
    pub(crate) fn next_if_newline(&mut self) {
        if matches!(self.peek().tok, Tok::Newline) {
            self.next();
        }
    }
}
