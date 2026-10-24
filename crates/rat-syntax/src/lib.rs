//! Dialect front ends: ACME, KickAssembler and rat. Each lowers source
//! text to the shared statement IR in `rat_core::ast`.

use rat_core::lexer::{lex, LexConfig};
use rat_core::{Dialect, Diagnostic, Expr, FileId, Instr, Stmt, StmtKind, Tok, Token};

mod acme;
pub mod cursor;
pub mod expr;
mod kick;

use cursor::{Cur, PResult};

/// Result of parsing one file.
#[derive(Debug, Default)]
pub struct ParsedFile {
    pub stmts: Vec<Stmt>,
    /// Names declared inside Kick `.zp { }` blocks.
    pub zp_names: Vec<String>,
}

pub fn lex_config(d: Dialect) -> &'static LexConfig {
    match d {
        Dialect::Acme => &LexConfig::ACME,
        Dialect::Kick => &LexConfig::KICK,
        Dialect::Rat => &LexConfig::RAT,
    }
}

/// In-file dialect switch: the first significant line is `.syntax x` or
/// `!syntax x`.
pub fn syntax_directive(src: &str) -> Option<Dialect> {
    for line in src.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with(';') || l.starts_with("//") || l.starts_with("/*") || l.starts_with('*') && !l.starts_with("*=") {
            continue;
        }
        let rest = l.strip_prefix(".syntax").or_else(|| l.strip_prefix("!syntax"))?;
        let word: String = rest.trim().chars().take_while(|c| c.is_alphanumeric()).collect();
        return Dialect::parse(&word);
    }
    None
}

/// Dialect of a file: in-file directive, then `forced` (CLI flag for the
/// main file), then extension, then `inherited`.
pub fn detect_dialect(src: &str, path: &str, forced: Option<Dialect>, inherited: Dialect) -> Dialect {
    syntax_directive(src)
        .or(forced)
        .or_else(|| Dialect::from_extension(path))
        .unwrap_or(inherited)
}

/// Platform lookup used while parsing to follow `.platform` CPU changes.
pub type PlatformCpu = fn(&str) -> Option<&'static str>;

pub struct Options {
    pub dialect: Dialect,
    /// CPU active where the file starts (decides what is a mnemonic).
    pub cpu: &'static dyn rat_cpu::Cpu,
    pub platform_cpu: PlatformCpu,
}

/// Parse a whole file. Errors are collected; parsing resumes at the next
/// line.
pub fn parse(file: FileId, src: &str, opts: &Options) -> (ParsedFile, Vec<Diagnostic>) {
    let toks = match lex(file, src, lex_config(opts.dialect)) {
        Ok(t) => t,
        Err(d) => return (ParsedFile::default(), vec![d]),
    };
    let mut p = Parser {
        c: Cur::new(&toks),
        src,
        dialect: opts.dialect,
        cpu: opts.cpu,
        platform_cpu: opts.platform_cpu,
        diags: Vec::new(),
        zp_names: Vec::new(),
        in_zp: false,
    };
    let stmts = match opts.dialect {
        Dialect::Acme => acme::file(&mut p),
        _ => kick::file(&mut p),
    };
    (
        ParsedFile {
            stmts,
            zp_names: p.zp_names,
        },
        p.diags,
    )
}

/// Parse a complete token slice as one expression (used for operands).
pub fn parse_expr_tokens(d: Dialect, toks: &[Token]) -> PResult<Expr> {
    let mut v = toks.to_vec();
    let end = v.last().map_or(rat_core::Span::default(), |t| t.span);
    v.push(Token { tok: Tok::Newline, span: end, space_before: true });
    v.push(Token { tok: Tok::Eof, span: end, space_before: true });
    let mut c = Cur::new(&v);
    let e = expr::parse_expr(&mut c, d)?;
    if !c.at_eol() {
        return Err(c.error(format!("unexpected {} in expression", c.peek().tok)));
    }
    Ok(e)
}

/// Parse an expression from source text (CLI `-D`, REST defines).
pub fn parse_expr_str(d: Dialect, src: &str) -> PResult<Expr> {
    let toks = lex(u32::MAX, src, lex_config(d))?;
    let n = toks.len().saturating_sub(2);
    parse_expr_tokens(d, &toks[..n])
}

pub(crate) struct Parser<'a> {
    pub c: Cur<'a>,
    pub src: &'a str,
    pub dialect: Dialect,
    pub cpu: &'static dyn rat_cpu::Cpu,
    pub platform_cpu: PlatformCpu,
    pub diags: Vec<Diagnostic>,
    pub zp_names: Vec<String>,
    pub in_zp: bool,
}

impl Parser<'_> {
    pub fn expr(&mut self) -> PResult<Expr> {
        expr::parse_expr(&mut self.c, self.dialect)
    }

    /// Comma-separated expressions up to the end of the statement.
    pub fn expr_list(&mut self) -> PResult<Vec<Expr>> {
        let mut v = Vec::new();
        if self.stmt_end() {
            return Ok(v);
        }
        loop {
            v.push(self.expr()?);
            if !self.c.eat(",") {
                return Ok(v);
            }
        }
    }

    /// End of a statement: line end, `}` or (ACME) `:`.
    pub fn stmt_end(&self) -> bool {
        let t = self.c.peek();
        t.is_end()
            || t.is_punct("}")
            || (self.dialect == Dialect::Acme && t.is_punct(":"))
            || (self.dialect == Dialect::Kick && t.is_punct(";"))
    }

    /// Operand tokens of an instruction up to the end of the statement.
    pub fn operand_tokens(&mut self) -> Vec<Token> {
        let mut v = Vec::new();
        let mut depth = 0i32;
        loop {
            let t = self.c.peek();
            if t.is_end() {
                break;
            }
            if depth == 0
                && (t.is_punct("}")
                    || (self.dialect == Dialect::Acme && t.is_punct(":"))
                    || (self.dialect == Dialect::Kick && t.is_punct(";"))
                    || (self.dialect != Dialect::Acme && t.is_ident("else") && !v.is_empty()))
            {
                break;
            }
            if t.is_punct("(") || t.is_punct("[") || t.is_punct("{") {
                depth += 1;
            } else if t.is_punct(")") || t.is_punct("]") || t.is_punct("}") {
                depth -= 1;
            }
            v.push(t.clone());
            self.c.next();
        }
        v
    }

    /// Is `name` an instruction of the CPU active at this point?
    pub fn is_mnemonic(&self, name: &str) -> bool {
        self.cpu.has_mnemonic(name)
    }

    /// Track static CPU switches so later lines classify mnemonics right.
    pub fn set_cpu(&mut self, name: &str) {
        if let Some(c) = rat_cpu::by_name(name) {
            self.cpu = c;
        }
    }

    pub fn set_platform(&mut self, name: &str) {
        if let Some(cpu) = (self.platform_cpu)(name) {
            self.set_cpu(cpu);
        }
    }

    pub fn instr(&mut self, mnemonic: String, suffix: Option<String>, start: usize) -> Stmt {
        let operand = self.operand_tokens();
        Stmt::new(
            StmtKind::Instr(Instr {
                label: None,
                mnemonic,
                suffix,
                operand,
                dialect: self.dialect,
            }),
            self.c.span_from(start),
        )
    }

    /// Raw source text from the current token to the end of the line
    /// (ACME `!hex`).
    pub fn rest_of_line_text(&mut self) -> String {
        let start = self.c.span().start as usize;
        let mut end = start;
        while !self.stmt_end() {
            end = self.c.peek().span.end as usize;
            self.c.next();
        }
        self.src.get(start..end).unwrap_or("").to_owned()
    }

    pub fn error(&mut self, d: Diagnostic) {
        self.diags.push(d);
    }

    /// Recover after an error: skip to the next line.
    pub fn recover(&mut self) {
        while !self.c.at_eol() {
            if self.c.is("}") {
                return;
            }
            self.c.next();
        }
    }
}

/// Parse `key = value` / positional args in `[ ... ]` (Kick segment
/// parameters).
pub(crate) fn bracket_args(p: &mut Parser<'_>) -> PResult<Vec<rat_core::Arg>> {
    let mut v = Vec::new();
    p.c.expect("[")?;
    loop {
        p.c.skip_newlines();
        if p.c.eat("]") {
            return Ok(v);
        }
        v.push(arg(p)?);
        p.c.skip_newlines();
        if !p.c.eat(",") {
            p.c.skip_newlines();
            p.c.expect("]")?;
            return Ok(v);
        }
    }
}

/// One `key=value` or positional argument.
pub(crate) fn arg(p: &mut Parser<'_>) -> PResult<rat_core::Arg> {
    if let Tok::Ident(k) = &p.c.peek().tok {
        if p.c.is_at(1, "=") {
            let k = k.clone();
            p.c.next();
            p.c.next();
            let value = p.expr()?;
            return Ok(rat_core::Arg { key: Some(k), value });
        }
    }
    Ok(rat_core::Arg { key: None, value: p.expr()? })
}

/// Comma-separated `key=value`/positional args up to the statement end.
pub(crate) fn arg_list(p: &mut Parser<'_>) -> PResult<Vec<rat_core::Arg>> {
    let mut v = Vec::new();
    if p.stmt_end() || p.c.is("{") {
        return Ok(v);
    }
    if p.c.is("[") {
        return bracket_args(p);
    }
    loop {
        v.push(arg(p)?);
        if !p.c.eat(",") {
            return Ok(v);
        }
    }
}
