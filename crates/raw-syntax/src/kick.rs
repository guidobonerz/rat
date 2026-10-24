//! KickAssembler and raw front end. Both use `.` directives, `label:`,
//! `{ }` blocks and C-like expressions; raw adds its own directive names
//! (`.org`, `.include`, `.incbin`, `.fn`, `.for i in a..b`, `.elif`,
//! `dc.b` ...). Each dialect accepts the other's directives.

use std::rc::Rc;

use raw_core::*;

use crate::cursor::PResult;
use crate::{arg_list, bracket_args, Parser};

pub(crate) fn file(p: &mut Parser<'_>) -> Vec<Stmt> {
    items(p, End::Eof)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum End {
    Eof,
    Brace,
    /// `#elif`, `#else`, `#endif` (left unconsumed).
    PpBranch,
}

fn pp_word(p: &Parser<'_>) -> Option<String> {
    if p.c.is("#") && p.c.glued_at(1) {
        if let Tok::Ident(s) = &p.c.peek_at(1).tok {
            return Some(s.to_ascii_lowercase());
        }
    }
    None
}

fn items(p: &mut Parser<'_>, end: End) -> Vec<Stmt> {
    let mut out = Vec::new();
    loop {
        while matches!(p.c.peek().tok, Tok::Newline) || p.c.is(";") {
            p.c.next();
        }
        if p.c.at_eof() {
            if end == End::Brace {
                let d = p.c.error("missing `}`");
                p.error(d);
            }
            if end == End::PpBranch {
                let d = p.c.error("missing `#endif`");
                p.error(d);
            }
            return out;
        }
        if p.c.is("}") {
            if end == End::Brace {
                p.c.next();
                return out;
            }
            let d = p.c.error("unexpected `}`");
            p.error(d);
            p.c.next();
            continue;
        }
        if end == End::PpBranch && matches!(pp_word(p).as_deref(), Some("elif" | "else" | "endif")) {
            return out;
        }
        match statement(p, &mut out) {
            Ok(()) => {
                if !p.stmt_end() && !p.c.is(";") {
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
    Ok(items(p, End::Brace))
}

fn opt_block(p: &mut Parser<'_>) -> PResult<Option<Block>> {
    if p.c.is("{") {
        Ok(Some(block(p)?))
    } else {
        Ok(None)
    }
}

/// A block or a single statement (Kick `.if (c) .byte 1`).
fn body(p: &mut Parser<'_>) -> PResult<Block> {
    if p.c.is("{") {
        return block(p);
    }
    if p.c.at_eol() {
        let save = p.c.pos;
        p.c.skip_newlines();
        if p.c.is("{") {
            return block(p);
        }
        if p.c.at_eof() || p.c.is("}") {
            p.c.pos = save;
            return Err(p.c.error("expected a statement or `{`"));
        }
    }
    let mut v = Vec::new();
    statement(p, &mut v)?;
    Ok(v)
}

/// Raw word text (`.cpu _6502NoIllegals`, `.platform c64`).
fn word_text(p: &mut Parser<'_>) -> String {
    if let Tok::Str(s) = &p.c.peek().tok {
        let s = s.clone();
        p.c.next();
        return s;
    }
    let start = p.c.span().start as usize;
    let mut end = start;
    while !p.stmt_end() && !p.c.is("{") && !p.c.is(",") {
        end = p.c.peek().span.end as usize;
        p.c.next();
    }
    p.src.get(start..end).unwrap_or("").trim().to_owned()
}

fn is_label_colon(p: &Parser<'_>, at: usize) -> bool {
    // `name:` but not `name::`
    p.c.is_at(at, ":") && !p.c.is_at(at + 1, ":")
}

fn statement(p: &mut Parser<'_>, out: &mut Vec<Stmt>) -> PResult<()> {
    let start = p.c.pos;
    let sp = p.c.span();
    if let Some(w) = pp_word(p) {
        return preprocessor(p, w, out);
    }
    if p.c.is("*=") || (p.c.is("*") && p.c.is_at(1, "=")) {
        if p.c.is("*") {
            p.c.next();
        }
        p.c.next();
        let addr = p.expr()?;
        let name = if matches!(p.c.peek().tok, Tok::Str(_)) { Some(p.expr()?) } else { None };
        let invisible = p.c.eat_ident("virtual");
        out.push(Stmt::new(StmtKind::Org { addr, name, overlay: false, invisible }, p.c.span_from(start)));
        return Ok(());
    }
    if p.c.is(".") && p.c.glued_at(1) && !(p.dialect == Dialect::Raw && is_label_colon(p, 2)) {
        if let Tok::Ident(name) = &p.c.peek_at(1).tok {
            let name = name.to_ascii_lowercase();
            p.c.next();
            p.c.next();
            return directive(p, name, start, out);
        }
    }
    // Kick multi labels `!name:` / `!:`
    if p.c.is("!") && p.c.glued_at(1) {
        if p.c.is_at(1, ":") {
            p.c.next();
            p.c.next();
            out.push(Stmt::new(StmtKind::AnonLabel("!".into()), sp));
            return rest_of_line(p, out);
        }
        if let Tok::Ident(n) = &p.c.peek_at(1).tok {
            if p.c.is_at(2, ":") {
                let key = format!("!{n}");
                p.c.next();
                p.c.next();
                p.c.next();
                out.push(Stmt::new(StmtKind::AnonLabel(key), sp));
                return rest_of_line(p, out);
            }
        }
    }
    // raw anonymous labels `-` `+` at the start of a statement
    if p.dialect == Dialect::Raw {
        if let Tok::Punct(first @ ("-" | "--" | "+" | "++")) = p.c.peek().tok {
            let sign = first.chars().next().unwrap();
            let mut key = String::new();
            while let Tok::Punct(q) = p.c.peek().tok {
                if !q.chars().all(|ch| ch == sign) || (!key.is_empty() && !p.c.glued_at(0)) {
                    break;
                }
                key.push_str(q);
                p.c.next();
            }
            p.c.eat(":");
            out.push(Stmt::new(StmtKind::AnonLabel(key), sp));
            return rest_of_line(p, out);
        }
        // `@local:` / `.local:` cheap label
        if (p.c.is("@") || p.c.is(".")) && p.c.glued_at(1) && is_label_colon(p, 2) {
            if let Tok::Ident(n) = &p.c.peek_at(1).tok {
                let name = Name { kind: NameKind::Cheap, text: n.clone() };
                p.c.next();
                p.c.next();
                p.c.next();
                out.push(Stmt::new(StmtKind::Label(name), sp));
                return rest_of_line(p, out);
            }
        }
    }
    // Kick `@label:` defines in the root scope
    if p.dialect == Dialect::Kick && p.c.is("@") && p.c.glued_at(1) && is_label_colon(p, 2) {
        if let Tok::Ident(n) = &p.c.peek_at(1).tok {
            let name = Name { kind: NameKind::Root, text: n.clone() };
            p.c.next();
            p.c.next();
            p.c.next();
            out.push(Stmt::new(StmtKind::Label(name), sp));
            return rest_of_line(p, out);
        }
    }
    if p.c.is("{") {
        let b = block(p)?;
        out.push(Stmt::new(StmtKind::Scope { name: None, body: b }, p.c.span_from(start)));
        return Ok(());
    }
    // old Kick macro call `:Name(args)`
    if p.c.is(":") && p.c.glued_at(1) && matches!(p.c.peek_at(1).tok, Tok::Ident(_)) {
        p.c.next();
        if p.c.is_at(1, "(") && p.c.glued_at(1) {
            return call_stmt(p, start, None, out);
        }
        // `:adc #1 : $10` calls the pseudocommand, not the mnemonic
        let name = p.c.ident()?;
        out.push(p.instr(name, Some("\u{1}macro".into()), start));
        return Ok(());
    }
    let Tok::Ident(name) = &p.c.peek().tok else {
        return Err(p.c.error(format!("unexpected {}", p.c.peek().tok)));
    };
    let name = name.clone();
    // label
    if is_label_colon(p, 1) {
        p.c.next();
        p.c.next();
        let lname = Name::plain(name.clone());
        if p.c.is("{") {
            let b = block(p)?;
            out.push(Stmt::new(StmtKind::Scope { name: Some(lname), body: b }, p.c.span_from(start)));
            return Ok(());
        }
        out.push(Stmt::new(StmtKind::Label(lname.clone()), sp));
        if p.stmt_end() {
            return Ok(());
        }
        // `table: .lohifill ...`
        if p.c.is(".") && p.c.peek_at(1).is_ident("lohifill") {
            let s = p.c.pos;
            p.c.next();
            p.c.next();
            let count = p.expr()?;
            p.c.expect(",")?;
            let value = p.expr()?;
            out.push(Stmt::new(StmtKind::LoHiFill { name: Some(lname), count, value, index_var: "i".into() }, p.c.span_from(s)));
            return Ok(());
        }
        // `lbl: macro()` names the macro's scope
        if let Tok::Ident(n) = &p.c.peek().tok {
            if !p.is_mnemonic(n) && p.c.is_at(1, "(") && p.c.glued_at(1) {
                let s = p.c.pos;
                return call_stmt(p, s, Some(lname), out);
            }
        }
        // `lbl: :macro()`
        if p.c.is(":") && p.c.glued_at(1) && p.c.is_at(2, "(") {
            let s = p.c.pos;
            p.c.next();
            return call_stmt(p, s, Some(lname), out);
        }
        // `lbl: .if (...) { label: }` -> `lbl.label`
        if p.c.is(".") && ["if", "for", "while"].iter().any(|w| p.c.peek_at(1).is_ident(w)) {
            let s = p.c.pos;
            let mut inner = Vec::new();
            statement(p, &mut inner)?;
            out.pop();
            out.push(Stmt::new(StmtKind::Scope { name: Some(lname), body: inner }, p.c.span_from(s)));
            return Ok(());
        }
        let before = out.len();
        statement(p, out)?;
        // `lbl: pseudocommand args` -> `lbl.x`
        if out.len() == before + 1 {
            if let StmtKind::Instr(i) = &mut out[before].kind {
                if !p.is_mnemonic(&i.mnemonic) {
                    i.label = Some(lname);
                }
            }
        }
        return Ok(());
    }
    if p.c.is_at(1, "=") {
        p.c.next();
        p.c.next();
        let value = p.expr()?;
        out.push(Stmt::new(StmtKind::Assign { name: Name::plain(name), value, kind: AssignKind::Label }, p.c.span_from(start)));
        return Ok(());
    }
    // 68k-style data: dc.b / ds.w
    if p.dialect == Dialect::Raw && (name.eq_ignore_ascii_case("dc") || name.eq_ignore_ascii_case("ds")) && p.c.is_at(1, ".") && p.c.glued_at(1) {
        p.c.next();
        p.c.next();
        let sz = p.c.ident()?.to_ascii_lowercase();
        return directive(p, format!("{}.{sz}", name.to_ascii_lowercase()), start, out);
    }
    if p.is_mnemonic(&name) {
        p.c.next();
        let suffix = mnemonic_suffix(p);
        out.push(p.instr(name, suffix, start));
        return Ok(());
    }
    // macro / function call statement `name(args)` or `ns.name(args)`
    let mut k = 1;
    while p.c.is_at(k, ".") && p.c.glued_at(k) && matches!(p.c.peek_at(k + 1).tok, Tok::Ident(_)) {
        k += 2;
    }
    if p.c.is_at(k, "(") && p.c.glued_at(k) {
        return call_stmt(p, start, None, out);
    }
    // pseudocommand (or an instruction of a CPU selected at run time)
    p.c.next();
    let suffix = mnemonic_suffix(p);
    out.push(p.instr(name, suffix, start));
    Ok(())
}

/// `lda.z`, `move.l`, `jmp.abs`.
fn mnemonic_suffix(p: &mut Parser<'_>) -> Option<String> {
    if p.c.is(".") && p.c.glued_at(0) && p.c.glued_at(1) {
        if let Tok::Ident(s) = &p.c.peek_at(1).tok {
            let s = s.clone();
            p.c.next();
            p.c.next();
            return Some(s);
        }
    }
    None
}

fn rest_of_line(p: &mut Parser<'_>, out: &mut Vec<Stmt>) -> PResult<()> {
    if p.stmt_end() || p.c.is(";") {
        return Ok(());
    }
    statement(p, out)
}

fn call_stmt(p: &mut Parser<'_>, start: usize, label: Option<Name>, out: &mut Vec<Stmt>) -> PResult<()> {
    let e = p.expr()?;
    let ExprKind::Call(callee, args) = e.kind else {
        return Err(Diagnostic::error("expected a macro call", Some(e.span)));
    };
    out.push(Stmt::new(StmtKind::Call { name: *callee, args, label }, p.c.span_from(start)));
    Ok(())
}

fn preprocessor(p: &mut Parser<'_>, w: String, out: &mut Vec<Stmt>) -> PResult<()> {
    let start = p.c.pos;
    p.c.next();
    p.c.next();
    let kind = match w.as_str() {
        "import" | "include" => StmtKind::Include { path: p.expr()?, once: false },
        "importif" => {
            let cond = p.expr()?;
            let path = p.expr()?;
            let sp = p.c.span_from(start);
            StmtKind::PpIf { branches: vec![(cond, vec![Stmt::new(StmtKind::Include { path, once: false }, sp)])], else_: None }
        }
        "importonce" => StmtKind::ImportOnce,
        "define" => StmtKind::PpDefine(p.c.ident()?),
        "undef" => StmtKind::PpUndef(p.c.ident()?),
        "if" | "ifdef" => {
            let mut branches = Vec::new();
            let mut cond = p.expr()?;
            let mut else_ = None;
            loop {
                let b = items(p, End::PpBranch);
                match pp_word(p).as_deref() {
                    Some("elif") => {
                        p.c.next();
                        p.c.next();
                        branches.push((cond, b));
                        cond = p.expr()?;
                    }
                    Some("else") => {
                        p.c.next();
                        p.c.next();
                        branches.push((cond, b));
                        let e = items(p, End::PpBranch);
                        else_ = Some(e);
                        if pp_word(p).as_deref() == Some("endif") {
                            p.c.next();
                            p.c.next();
                        }
                        break;
                    }
                    Some("endif") => {
                        p.c.next();
                        p.c.next();
                        branches.push((cond, b));
                        break;
                    }
                    _ => {
                        branches.push((cond, b));
                        break;
                    }
                }
            }
            StmtKind::PpIf { branches, else_ }
        }
        _ => return Err(p.c.error(format!("unknown preprocessor directive `#{w}`"))),
    };
    out.push(Stmt::new(kind, p.c.span_from(start)));
    Ok(())
}

fn data(p: &mut Parser<'_>, size: u8, endian: Option<Endian>) -> PResult<StmtKind> {
    Ok(StmtKind::Data { size, endian, items: p.expr_list()? })
}

fn params(p: &mut Parser<'_>, sep: &str, close: Option<&str>) -> PResult<(Vec<Param>, bool)> {
    let mut v = Vec::new();
    let mut variadic = false;
    loop {
        if let Some(c) = close {
            if p.c.is(c) {
                break;
            }
        } else if p.c.is("{") || p.stmt_end() {
            break;
        }
        if p.c.is("..") && p.c.is_at(1, ".") {
            p.c.next();
            p.c.next();
            variadic = true;
            continue;
        }
        let name = p.c.ident()?;
        let default = if p.c.eat("=") { Some(p.expr()?) } else { None };
        v.push(Param { name, by_ref: false, default });
        if !p.c.eat(sep) {
            break;
        }
    }
    if let Some(c) = close {
        p.c.expect(c)?;
    }
    Ok((v, variadic))
}

fn if_stmt(p: &mut Parser<'_>, start: usize) -> PResult<StmtKind> {
    let cond = p.expr()?;
    let then = body(p)?;
    // look past line breaks for `else` / `.else` / `.elif`
    let save = p.c.pos;
    p.c.skip_newlines();
    let dot = p.c.is(".") && p.c.glued_at(1);
    let w = if dot { p.c.peek_at(1) } else { p.c.peek() };
    let word = w.ident().map(|s| s.to_ascii_lowercase());
    let else_ = match word.as_deref() {
        Some("else") => {
            if dot {
                p.c.next();
            }
            p.c.next();
            // `else if` / `else .if`
            if p.c.is_ident("if") || (p.c.is(".") && p.c.peek_at(1).is_ident("if")) {
                if p.c.is(".") {
                    p.c.next();
                }
                p.c.next();
                let s = p.c.pos;
                let k = if_stmt(p, s)?;
                Some(vec![Stmt::new(k, p.c.span_from(s))])
            } else {
                Some(body(p)?)
            }
        }
        Some("elif") | Some("elseif") if dot => {
            p.c.next();
            p.c.next();
            let s = p.c.pos;
            let k = if_stmt(p, s)?;
            Some(vec![Stmt::new(k, p.c.span_from(s))])
        }
        _ => {
            p.c.pos = save;
            None
        }
    };
    let _ = start;
    Ok(StmtKind::If { cond, then, else_ })
}

/// `var i = 0` / `.var i = 0` / plain expression inside `.for ( ... )`.
fn for_init(p: &mut Parser<'_>) -> PResult<Vec<Stmt>> {
    let mut v = Vec::new();
    if p.c.is(";") {
        return Ok(v);
    }
    loop {
        let s = p.c.pos;
        p.c.eat(".");
        if p.c.eat_ident("var") || p.c.eat_ident("const") {
            let name = p.c.ident()?;
            p.c.expect("=")?;
            let value = p.expr()?;
            v.push(Stmt::new(StmtKind::Assign { name: Name::plain(name), value, kind: AssignKind::Var }, p.c.span_from(s)));
        } else {
            let e = p.expr()?;
            v.push(Stmt::new(StmtKind::Eval(e), p.c.span_from(s)));
        }
        if !p.c.eat(",") {
            return Ok(v);
        }
    }
}

fn for_stmt(p: &mut Parser<'_>) -> PResult<StmtKind> {
    // raw: `.for i in a..b` / `.for i in list`
    if let Tok::Ident(var) = &p.c.peek().tok {
        if p.c.peek_at(1).is_ident("in") {
            let var = var.clone();
            p.c.next();
            p.c.next();
            let from = p.expr()?;
            if p.c.is("..") || p.c.is("..=") {
                let inclusive = p.c.is("..=");
                p.c.next();
                let to = p.expr()?;
                let step = if p.c.eat_ident("step") { Some(p.expr()?) } else { None };
                return Ok(StmtKind::ForRange { var, from, to, step, inclusive, body: body(p)? });
            }
            return Ok(StmtKind::ForIn { var, list: from, body: body(p)? });
        }
    }
    p.c.expect("(")?;
    let init = for_init(p)?;
    p.c.expect(";")?;
    let cond = if p.c.is(";") { None } else { Some(p.expr()?) };
    p.c.expect(";")?;
    let mut step = Vec::new();
    while !p.c.is(")") {
        step.push(p.expr()?);
        if !p.c.eat(",") {
            break;
        }
    }
    p.c.expect(")")?;
    Ok(StmtKind::ForC { init, cond, step, body: body(p)? })
}

fn macro_def(p: &mut Parser<'_>, kind: MacroKind, start: usize) -> PResult<StmtKind> {
    let root = p.c.eat("@");
    let name = if root { format!("@{}", p.c.ident()?) } else { p.c.ident()? };
    let (params, variadic) = if kind == MacroKind::Pseudo {
        params(p, ":", None)?
    } else if p.c.eat("(") {
        params(p, ",", Some(")"))?
    } else {
        (Vec::new(), false)
    };
    let mut expr_body = None;
    let body = if kind == MacroKind::Function && p.c.eat("=") {
        expr_body = Some(p.expr()?);
        Vec::new()
    } else {
        block(p)?
    };
    Ok(StmtKind::MacroDef(Rc::new(MacroDef {
        name,
        params,
        variadic,
        body,
        kind,
        dialect: p.dialect,
        span: p.c.span_from(start),
        expr_body,
    })))
}

fn directive(p: &mut Parser<'_>, name: String, start: usize, out: &mut Vec<Stmt>) -> PResult<()> {
    let span = p.c.span_from(start);
    let kick = p.dialect == Dialect::Kick;
    let be = Some(Endian::Big);
    let le = Some(Endian::Little);
    let kind = match name.as_str() {
        "byte" | "by" | "b" | "db" | "dc.b" | "byt" => data(p, 1, None)?,
        "word" | "wo" | "w" | "dc.w" | "addr" => data(p, 2, None)?,
        "dw" if kick => data(p, 4, None)?,
        "dw" => data(p, 2, None)?,
        "dword" | "long" | "dl" | "dc.l" | "dd" => data(p, 4, None)?,
        "quad" | "dq" => data(p, 8, None)?,
        "le16" => data(p, 2, le)?,
        "be16" => data(p, 2, be)?,
        "le24" | "faraddr" => data(p, 3, le)?,
        "be24" => data(p, 3, be)?,
        "le32" => data(p, 4, le)?,
        "be32" => data(p, 4, be)?,
        "ds.b" | "ds.w" | "ds.l" | "ds" | "res" | "dsb" => {
            let size = match name.as_str() {
                "ds.w" => 2,
                "ds.l" => 4,
                _ => 1,
            };
            let count = p.expr()?;
            let value = if p.c.eat(",") { Some(p.expr()?) } else { None };
            StmtKind::Fill { count, value, size, index_var: None }
        }
        "text" | "te" | "ascii" | "str" => StmtKind::Text { conv: TextConv::Current, xor: None, items: p.expr_list()? },
        "asciiz" | "cstr" => {
            let items = p.expr_list()?;
            let mut v = vec![Stmt::new(StmtKind::Text { conv: TextConv::Current, xor: None, items }, span)];
            v.push(Stmt::new(StmtKind::Data { size: 1, endian: None, items: vec![Expr::int(0, span)] }, span));
            StmtKind::Group(v)
        }
        "petscii" | "pet" => StmtKind::Text { conv: TextConv::Petscii, xor: None, items: p.expr_list()? },
        "screen" | "scr" => StmtKind::Text { conv: TextConv::Screen, xor: None, items: p.expr_list()? },
        "hex" => {
            let text = p.rest_of_line_text();
            let digits: Vec<u8> = text.bytes().filter(|b| b.is_ascii_hexdigit()).collect();
            if !digits.len().is_multiple_of(2) {
                return Err(Diagnostic::error("`.hex` needs pairs of hex digits", Some(span)));
            }
            StmtKind::Hex(digits.chunks(2).map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap()).collect())
        }
        "fill" | "fillword" => {
            let count = p.expr()?;
            p.c.expect(",")?;
            let value = p.expr()?;
            StmtKind::Fill { count, value: Some(value), size: if name == "fillword" { 2 } else { 1 }, index_var: Some("i".into()) }
        }
        "lohifill" => {
            let count = p.expr()?;
            p.c.expect(",")?;
            let value = p.expr()?;
            StmtKind::LoHiFill { name: None, count, value, index_var: "i".into() }
        }
        "align" => {
            let n = p.expr()?;
            let fill = if p.c.eat(",") { Some(p.expr()?) } else { None };
            StmtKind::Align { kind: AlignKind::Multiple(n), fill }
        }
        "even" => StmtKind::Align { kind: AlignKind::Multiple(Expr::int(2, span)), fill: None },
        "pc" | "org" => {
            p.c.eat("=");
            let addr = p.expr()?;
            let name = if matches!(p.c.peek().tok, Tok::Str(_)) { Some(p.expr()?) } else { None };
            let invisible = p.c.eat_ident("virtual");
            StmtKind::Org { addr, name, overlay: false, invisible }
        }
        "pseudopc" | "rorg" | "phase" => {
            let addr = p.expr()?;
            StmtKind::PseudoPc { addr, body: opt_block(p)? }
        }
        "realpc" | "dephase" => StmtKind::RealPc,
        "zp" => {
            p.in_zp = true;
            let b = block(p)?;
            p.in_zp = false;
            for s in &b {
                if let StmtKind::Label(n) = &s.kind {
                    p.zp_names.push(n.text.clone());
                }
                if let StmtKind::Assign { name, .. } = &s.kind {
                    p.zp_names.push(name.text.clone());
                }
            }
            StmtKind::ZpBlock(b)
        }
        "import" => {
            let what = p.c.ident()?.to_ascii_lowercase();
            let path = p.expr()?;
            match what.as_str() {
                "source" => StmtKind::Include { path, once: false },
                "binary" | "c64" | "text" => {
                    let kind = match what.as_str() {
                        "c64" => BinaryKind::C64,
                        "text" => BinaryKind::Text,
                        _ => BinaryKind::Raw,
                    };
                    let mut offset = None;
                    let mut size = None;
                    if p.c.eat(",") {
                        offset = Some(p.expr()?);
                        if p.c.eat(",") {
                            size = Some(p.expr()?);
                        }
                    }
                    StmtKind::Binary { path, size, offset, kind }
                }
                _ => return Err(Diagnostic::error(format!("unknown import kind `{what}`"), Some(span))),
            }
        }
        "include" | "source" => StmtKind::Include { path: p.expr()?, once: false },
        "incbin" | "binary" => {
            let path = p.expr()?;
            let mut offset = None;
            let mut size = None;
            if p.c.eat(",") {
                offset = Some(p.expr()?);
                if p.c.eat(",") {
                    size = Some(p.expr()?);
                }
            }
            StmtKind::Binary { path, size, offset, kind: BinaryKind::Raw }
        }
        "importonce" | "once" => StmtKind::ImportOnce,
        "var" | "const" | "label" | "set" | "equ" => {
            let kind = match name.as_str() {
                "var" | "set" => AssignKind::Var,
                "const" | "equ" => AssignKind::Const,
                _ => AssignKind::Label,
            };
            let mut v = Vec::new();
            loop {
                let s = p.c.pos;
                let root = p.c.eat("@");
                let n = p.c.ident()?;
                p.c.expect("=")?;
                let value = p.expr()?;
                let name = if root { Name { kind: NameKind::Root, text: n } } else { Name::plain(n) };
                v.push(Stmt::new(StmtKind::Assign { name, value, kind }, p.c.span_from(s)));
                if !p.c.eat(",") {
                    break;
                }
            }
            // `.label x = $1000 { .label y = ... }`: x also names a scope
            if v.len() == 1 && p.c.is("{") {
                if let StmtKind::Assign { name, .. } = &v[0].kind {
                    let n = name.text.clone();
                    let b = block(p)?;
                    v.push(Stmt::new(StmtKind::Namespace { name: n, body: Some(b), file: false }, span));
                }
            }
            if v.len() == 1 {
                v.pop().unwrap().kind
            } else {
                StmtKind::Group(v)
            }
        }
        "eval" => StmtKind::Eval(p.expr()?),
        "print" => StmtKind::Print { expr: p.expr()?, now: false },
        "printnow" => StmtKind::Print { expr: p.expr()?, now: true },
        "error" => StmtKind::Message { level: MsgLevel::Error, args: p.expr_list()? },
        "warn" | "warning" => StmtKind::Message { level: MsgLevel::Warn, args: p.expr_list()? },
        "errorif" => {
            let cond = p.expr()?;
            p.c.expect(",")?;
            StmtKind::ErrorIf { cond, msg: p.expr()? }
        }
        "assert" if {
            // `.assert "d", { a }, { b }`
            let save = p.c.pos;
            let is_block = p.expr().is_ok() && p.c.eat(",") && {
                p.c.skip_newlines();
                p.c.is("{")
            };
            p.c.pos = save;
            is_block
        } =>
        {
            let desc = p.expr()?;
            p.c.expect(",")?;
            let actual = block(p)?;
            p.c.skip_newlines();
            p.c.expect(",")?;
            let expected = block(p)?;
            StmtKind::AssertCode { desc, actual, expected }
        }
        "assert" => {
            let first = p.expr()?;
            if p.c.eat(",") {
                let second = p.expr()?;
                if p.c.eat(",") {
                    // Kick: .assert "desc", actual, expected
                    let expected = p.expr()?;
                    StmtKind::Assert { desc: Some(first), actual: second, expected: Some(expected) }
                } else {
                    // raw: .assert cond, "message"
                    StmtKind::Assert { desc: Some(second), actual: first, expected: None }
                }
            } else {
                StmtKind::Assert { desc: None, actual: first, expected: None }
            }
        }
        "asserterror" => {
            let desc = p.expr()?;
            p.c.expect(",")?;
            p.c.skip_newlines();
            let b = if p.c.is("{") {
                block(p)?
            } else {
                // expression form: `.asserterror "d", 20/false`
                let s = p.c.pos;
                let e = p.expr()?;
                vec![Stmt::new(StmtKind::Eval(e), p.c.span_from(s))]
            };
            StmtKind::AssertError { desc, body: b }
        }
        "if" => if_stmt(p, start)?,
        "ifdef" | "ifndef" => {
            let n = p.c.ident()?;
            let then = body(p)?;
            let save = p.c.pos;
            p.c.skip_newlines();
            let else_ = if (p.c.is(".") && p.c.peek_at(1).is_ident("else")) || p.c.is_ident("else") {
                if p.c.is(".") {
                    p.c.next();
                }
                p.c.next();
                Some(body(p)?)
            } else {
                p.c.pos = save;
                None
            };
            StmtKind::IfDef { name: Name::plain(n), negate: name == "ifndef", then, else_ }
        }
        "for" => for_stmt(p)?,
        "while" => {
            let cond = p.expr()?;
            StmtKind::While { cond, body: body(p)? }
        }
        "repeat" | "rept" => {
            let count = p.expr()?;
            let var = if p.c.eat(",") { p.c.ident()? } else { "\0rep".into() };
            StmtKind::ForRange { var, from: Expr::int(0, span), to: count, step: None, inclusive: false, body: block(p)? }
        }
        "macro" | "mac" => macro_def(p, MacroKind::Macro, start)?,
        "function" | "fn" | "func" => macro_def(p, MacroKind::Function, start)?,
        "pseudocommand" => macro_def(p, MacroKind::Pseudo, start)?,
        "return" => StmtKind::Return(if p.stmt_end() { None } else { Some(p.expr()?) }),
        "struct" => {
            let n = p.c.ident()?;
            p.c.expect("{")?;
            let mut fields = Vec::new();
            loop {
                p.c.skip_newlines();
                if p.c.eat("}") {
                    break;
                }
                fields.push(p.c.ident()?);
                p.c.skip_newlines();
                if !p.c.eat(",") {
                    p.c.skip_newlines();
                    p.c.expect("}")?;
                    break;
                }
            }
            StmtKind::Struct { name: n, fields }
        }
        "enum" => {
            // optional name (ignored, Kick enums are flat)
            if let Tok::Ident(_) = p.c.peek().tok {
                p.c.next();
            }
            p.c.expect("{")?;
            let mut items = Vec::new();
            loop {
                p.c.skip_newlines();
                if p.c.eat("}") {
                    break;
                }
                let n = p.c.ident()?;
                let v = if p.c.eat("=") { Some(p.expr()?) } else { None };
                items.push((n, v));
                p.c.skip_newlines();
                if !p.c.eat(",") {
                    p.c.skip_newlines();
                    p.c.expect("}")?;
                    break;
                }
            }
            StmtKind::Enum { items }
        }
        "namespace" | "scope" | "proc" => {
            let n = p.c.ident()?;
            let b = opt_block(p)?;
            if name == "namespace" {
                StmtKind::Namespace { name: n, body: b, file: false }
            } else {
                StmtKind::Scope { name: Some(Name::plain(n)), body: b.unwrap_or_default() }
            }
        }
        "filenamespace" => StmtKind::Namespace { name: p.c.ident()?, body: None, file: true },
        "segmentdef" => {
            let n = p.c.ident()?;
            let params = if p.c.is("[") { bracket_args(p)? } else { Vec::new() };
            StmtKind::SegmentDef { name: n, params }
        }
        "segment" => {
            let n = p.c.ident()?;
            let mut params = if p.c.is("[") { bracket_args(p)? } else { Vec::new() };
            if matches!(p.c.peek().tok, Tok::Str(_)) {
                params.push(Arg { key: Some("memblock".into()), value: p.expr()? });
            }
            StmtKind::Segment { name: n, params, body: opt_block(p)? }
        }
        "segmentout" => StmtKind::SegmentOut { params: if p.c.is("[") { bracket_args(p)? } else { Vec::new() } },
        "file" => {
            let params = if p.c.is("[") { bracket_args(p)? } else { arg_list(p)? };
            StmtKind::File { params, body: opt_block(p)? }
        }
        "disk" => {
            let params = if p.c.is("[") { bracket_args(p)? } else { Vec::new() };
            p.c.skip_newlines();
            p.c.expect("{")?;
            let mut files = Vec::new();
            loop {
                p.c.skip_newlines();
                if p.c.eat("}") {
                    break;
                }
                files.push(bracket_args(p)?);
                p.c.skip_newlines();
                p.c.eat(",");
            }
            StmtKind::Disk { params, files }
        }
        "memblock" => StmtKind::MemBlock(p.expr()?),
        "encoding" | "charset" | "convtab" => StmtKind::Encoding(p.expr()?),
        "cpu" | "setcpu" | "p816" | "p02" => {
            let c = match name.as_str() {
                "p816" => "65816".to_string(),
                "p02" => "6502".to_string(),
                _ => word_text(p),
            };
            p.set_cpu(&c);
            StmtKind::Cpu(c)
        }
        "platform" | "target" => {
            let n = word_text(p);
            p.set_platform(&n);
            let b = opt_block(p)?;
            StmtKind::Platform { name: n, body: b }
        }
        "syntax" => StmtKind::Syntax(word_text(p)),
        "a8" | "a16" | "i8" | "i16" | "smart" => StmtKind::CpuFlag(name.clone()),
        "al" => StmtKind::CpuFlag("a16".into()),
        "as" => StmtKind::CpuFlag("a8".into()),
        "il" | "xl" => StmtKind::CpuFlag("i16".into()),
        "is" | "xs" => StmtKind::CpuFlag("i8".into()),
        "setdp" | "dp" | "direct" => StmtKind::SetDp(p.expr()?),
        "endian" => {
            let w = word_text(p).to_ascii_lowercase();
            StmtKind::Endian(match w.as_str() {
                "le" | "little" => Some(Endian::Little),
                "be" | "big" => Some(Endian::Big),
                _ => None,
            })
        }
        "le" => StmtKind::Endian(Some(Endian::Little)),
        "be" => StmtKind::Endian(Some(Endian::Big)),
        "output" | "outfile" => {
            let file = p.expr()?;
            let format = if p.c.eat(",") { Some(word_text(p).to_ascii_lowercase()) } else { None };
            StmtKind::Output { file, format }
        }
        "break" => StmtKind::Break(if p.stmt_end() { None } else { Some(p.expr()?) }),
        "watch" => {
            let mut args = Vec::new();
            loop {
                if p.c.is(",") {
                    args.push(Expr::new(ExprKind::Null, p.c.span()));
                } else if !p.stmt_end() {
                    args.push(p.expr()?);
                }
                if !p.c.eat(",") {
                    break;
                }
            }
            StmtKind::Watch { args }
        }
        "eof" | "end" => StmtKind::Eof,
        "plugin" | "define" | "modify" | "filemodify" | "createfile" => {
            return Err(Diagnostic::error(format!("`.{name}` is not supported by raw"), Some(span)));
        }
        _ => {
            // platform directive, optionally namespaced: .c64.basic, .xex.run
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
