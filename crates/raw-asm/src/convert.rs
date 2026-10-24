//! Dialect converter: parse in one dialect, print the shared IR in
//! another. Comments are carried over by line. Constructs a target cannot
//! express produce a warning and a `TODO` comment; constructs only raw
//! understands are written with raw's extension directives (noted as
//! warnings) so the result still assembles with raw.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use raw_core::*;

pub struct Converted {
    pub text: String,
    pub warnings: Vec<String>,
}

/// Convert source `text` (file `path`) to dialect `to`.
pub fn convert(path: &str, text: &str, from: Option<Dialect>, to: Dialect) -> Result<Converted, String> {
    let from = raw_syntax::detect_dialect(text, path, from, Dialect::Raw);
    let cpu_name = match from {
        Dialect::Raw => "6502",
        _ => "6510",
    };
    let cpu = raw_cpu::by_name(cpu_name).unwrap();
    let opts = raw_syntax::Options { dialect: from, cpu, platform_cpu: raw_platform::platform_cpu };
    let (parsed, errs) = raw_syntax::parse(0, text, &opts);
    if errs.iter().any(Diagnostic::is_error) {
        let mut map = SourceMap::new();
        map.add(path, text);
        return Err(errs.iter().map(|d| d.render(&map, false)).collect());
    }
    let mut p = Printer::new(text, from, to);
    p.cpu = cpu;
    p.prescan(&parsed.stmts);
    p.anon_defs.clear();
    if from == Dialect::Acme && to != Dialect::Acme {
        p.zone = 0;
    }
    p.header(&parsed.stmts);
    p.block(&parsed.stmts, 0);
    p.flush_comments(usize::MAX);
    Ok(Converted { text: p.out, warnings: p.warnings })
}

/// Per-line comments of the source (line number -> comment text).
fn comments(src: &str, d: Dialect) -> (HashMap<usize, String>, HashSet<usize>) {
    let mut map = HashMap::new();
    let mut full = HashSet::new();
    let starts: &[&str] = match d {
        Dialect::Acme => &[";"],
        Dialect::Kick => &["//"],
        Dialect::Raw => &[";", "//"],
    };
    let mut in_block = false;
    for (i, line) in src.lines().enumerate() {
        let n = i + 1;
        let b = line.as_bytes();
        let mut k = 0;
        let mut in_str = false;
        if in_block {
            let text = line.trim().trim_end_matches("*/").trim().to_owned();
            if !text.is_empty() {
                map.insert(n, text);
                full.insert(n);
            }
            if line.contains("*/") {
                in_block = false;
            }
            continue;
        }
        while k < b.len() {
            let c = b[k];
            if c == b'"' {
                in_str = !in_str;
            } else if !in_str {
                if d != Dialect::Acme && line[k..].starts_with("/*") {
                    let rest = &line[k + 2..];
                    let text = rest.split("*/").next().unwrap_or("").trim().to_owned();
                    if !rest.contains("*/") {
                        in_block = true;
                    }
                    if !text.is_empty() {
                        map.insert(n, text);
                    }
                    if line[..k].trim().is_empty() {
                        full.insert(n);
                    }
                    break;
                }
                if let Some(s) = starts.iter().find(|s| line[k..].starts_with(**s)) {
                    let text = line[k + s.len()..].trim().to_owned();
                    map.insert(n, text);
                    if line[..k].trim().is_empty() {
                        full.insert(n);
                    }
                    break;
                }
            }
            k += 1;
        }
    }
    (map, full)
}

struct Printer<'a> {
    src: &'a str,
    from: Dialect,
    to: Dialect,
    out: String,
    warnings: Vec<String>,
    comments: HashMap<usize, String>,
    full_comments: HashSet<usize>,
    last_line: usize,
    line_starts: Vec<usize>,
    /// Anonymous label family -> definitions seen so far.
    anon_defs: HashMap<String, usize>,
    /// Definition ordinal of every anonymous family, in order.
    anon_total: HashMap<String, usize>,
    zone: u32,
    cheap: u32,
    /// Labels local to the block being printed (scopes, macros).
    locals: Vec<HashSet<String>>,
    /// Named scopes / namespaces (for `a.b` flattening in ACME).
    named_scopes: HashSet<String>,
    /// Prefix for labels of the named scope being printed (ACME target).
    prefix: Vec<String>,
    in_macro: u32,
    /// CPU at the statement being printed (static tracking).
    cpu: &'static dyn raw_cpu::Cpu,
    /// Encoding line written before the first real statement.
    pending_header: Option<String>,
    /// Text encoding in effect (canonical name, as the source sees it).
    cur_enc: String,
}

const TODO: &str = "TODO(raw convert)";

impl<'a> Printer<'a> {
    fn new(src: &'a str, from: Dialect, to: Dialect) -> Printer<'a> {
        let (comments, full_comments) = comments(src, from);
        let mut line_starts = vec![0];
        line_starts.extend(src.bytes().enumerate().filter(|b| b.1 == b'\n').map(|b| b.0 + 1));
        Printer {
            src,
            from,
            to,
            out: String::new(),
            warnings: Vec::new(),
            comments,
            full_comments,
            last_line: 0,
            line_starts,
            anon_defs: HashMap::new(),
            anon_total: HashMap::new(),
            zone: 0,
            cheap: 0,
            locals: Vec::new(),
            named_scopes: HashSet::new(),
            prefix: Vec::new(),
            in_macro: 0,
            cpu: raw_cpu::by_name("6502").unwrap(),
            pending_header: None,
            cur_enc: "ascii".into(),
        }
    }

    fn line_of(&self, span: Span) -> usize {
        match self.line_starts.binary_search(&(span.start as usize)) {
            Ok(i) => i + 1,
            Err(i) => i,
        }
    }

    fn cmt(&self) -> &'static str {
        match self.to {
            Dialect::Acme => ";",
            Dialect::Kick => "//",
            Dialect::Raw => ";",
        }
    }

    fn warn(&mut self, msg: impl Into<String>) {
        let m = msg.into();
        if !self.warnings.contains(&m) {
            self.warnings.push(m);
        }
    }

    fn header(&mut self, stmts: &[Stmt]) {
        let c = self.cmt();
        let _ = writeln!(self.out, "{c} converted from {} to {} by raw", self.from.name(), self.to.name());
        if self.to == Dialect::Raw {
            self.out.push_str(".syntax raw\n");
        }
        // implied platform: Kick and ACME sources are C64 sources
        let explicit = stmts.iter().find_map(|s| match &s.kind {
            StmtKind::Platform { name, body: None } => Some(name.clone()),
            _ => None,
        });
        let src_platform = explicit.clone().unwrap_or_else(|| if self.from == Dialect::Raw { "generic".into() } else { "c64".into() });
        if explicit.is_none() && src_platform != "generic" && self.to == Dialect::Raw {
            let _ = writeln!(self.out, ".platform {src_platform}");
        }
        if let Some(p) = raw_platform::by_name(&src_platform) {
            if explicit.is_none() {
                self.cpu = raw_cpu::by_name(p.cpu).unwrap_or(self.cpu);
            }
            let enc = |d: Dialect| -> &'static str {
                match d {
                    Dialect::Kick => "screencode_mixed",
                    Dialect::Acme => "ascii",
                    Dialect::Raw => p.encoding,
                }
            };
            let (src_enc, dst_enc) = (enc(self.from), enc(self.to));
            self.cur_enc = src_enc.to_owned();
            if src_enc != dst_enc {
                let line = match self.to {
                    Dialect::Acme => format!(
                        "!convtab {}",
                        match src_enc {
                            "petscii_mixed" | "petscii_upper" => "pet",
                            "screencode_mixed" | "screencode_upper" => "scr",
                            _ => "raw",
                        }
                    ),
                    _ => format!(".encoding \"{src_enc}\""),
                };
                self.pending_header = Some(line);
            }
        }
    }

    /// Emit full-line comments up to (excluding) `line`.
    fn flush_comments(&mut self, line: usize) {
        let mut keys: Vec<usize> = self.comments.keys().copied().filter(|&l| l > self.last_line && l < line && self.full_comments.contains(&l)).collect();
        keys.sort();
        for k in keys {
            let c = self.cmt();
            let text = self.comments.remove(&k).unwrap();
            let _ = writeln!(self.out, "{c} {text}");
        }
        // blank-line structure
        if line != usize::MAX && line > self.last_line + 1 && self.last_line > 0 {
            let gap = self.src.lines().skip(self.last_line).take(line - self.last_line - 1).filter(|l| l.trim().is_empty()).count();
            if gap > 0 {
                self.out.push('\n');
            }
        }
    }

    fn emit(&mut self, indent: usize, text: &str, span: Span) {
        let line = self.line_of(span);
        if line > self.last_line {
            self.flush_comments(line);
        }
        let pad = "    ".repeat(indent);
        let mut t = format!("{pad}{text}");
        if line > self.last_line {
            if let Some(c) = self.comments.remove(&line) {
                if !self.full_comments.contains(&line) {
                    let w = t.chars().count();
                    let col = if w < 40 { 40 - w } else { 1 };
                    let _ = write!(t, "{:col$}{} {c}", "", self.cmt());
                }
            }
            self.last_line = line;
        }
        self.out.push_str(&t);
        self.out.push('\n');
    }

    fn todo(&mut self, indent: usize, what: &str, span: Span) {
        self.warn(format!("{what} has no {} equivalent", self.to.name()));
        let src = self.src.get(span.start as usize..span.end as usize).unwrap_or("").replace('\n', " ");
        let c = self.cmt();
        self.emit(indent, &format!("{c} {TODO}: {what}: {src}"), span);
    }

    // ------------------------------------------------------------ names

    /// Pass over the tree: anonymous label counts, named scopes.
    fn prescan(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            match &s.kind {
                StmtKind::AnonLabel(k) => *self.anon_total.entry(k.clone()).or_default() += 1,
                StmtKind::Scope { name: Some(n), body } => {
                    self.named_scopes.insert(n.text.clone());
                    self.prescan(body);
                }
                StmtKind::Namespace { name, body, .. } => {
                    self.named_scopes.insert(name.clone());
                    if let Some(b) = body {
                        self.prescan(b);
                    }
                }
                _ => {
                    for b in children(&s.kind) {
                        self.prescan(b);
                    }
                }
            }
        }
    }

    /// Does the target dialect natively use this anonymous family?
    fn anon_native(&self, key: &str) -> bool {
        let kick_key = key.starts_with('!');
        match self.to {
            Dialect::Kick => kick_key,
            _ => !kick_key,
        }
    }

    /// Kick family for an ACME/raw `-`/`+` family.
    fn kick_family(key: &str) -> String {
        let n = key.len();
        if key.starts_with('-') {
            format!("!b{}", if n > 1 { n.to_string() } else { String::new() })
        } else {
            format!("!f{}", if n > 1 { n.to_string() } else { String::new() })
        }
    }

    fn anon_name(key: &str, idx: usize) -> String {
        let k: String = key
            .chars()
            .map(|c| match c {
                '-' => 'b',
                '+' => 'f',
                '!' => 'k',
                c if c.is_alphanumeric() => c,
                _ => '_',
            })
            .collect();
        format!("anon_{k}_{idx}")
    }

    fn anon_label(&mut self, key: &str) -> String {
        let idx = *self.anon_defs.get(key).unwrap_or(&0);
        *self.anon_defs.entry(key.to_owned()).or_default() += 1;
        if self.anon_native(key) {
            return match self.to {
                Dialect::Kick => format!("{key}:"),
                Dialect::Raw => key.to_owned(),
                Dialect::Acme => key.to_owned(),
            };
        }
        if self.to == Dialect::Kick && !key.starts_with('!') {
            return format!("{}:", Self::kick_family(key));
        }
        let n = Self::anon_name(key, idx);
        match self.to {
            Dialect::Acme => format!(".{n}"),
            Dialect::Kick => format!("{n}:"),
            Dialect::Raw => format!("{n}:"),
        }
    }

    fn anon_ref(&mut self, key: &str, offset: i32) -> String {
        if self.anon_native(key) {
            if key.starts_with('!') {
                let sign = if offset < 0 { "-" } else { "+" };
                return format!("{key}{}", sign.repeat(offset.unsigned_abs() as usize));
            }
            return key.to_owned();
        }
        if self.to == Dialect::Kick && !key.starts_with('!') {
            let fam = Self::kick_family(key);
            return format!("{fam}{}", if offset < 0 { "-" } else { "+" });
        }
        let seen = *self.anon_defs.get(key).unwrap_or(&0) as i64;
        let idx = if offset < 0 { seen + i64::from(offset) } else { seen + i64::from(offset) - 1 };
        if idx < 0 || idx as usize >= *self.anon_total.get(key).unwrap_or(&0) {
            self.warn(format!("anonymous label reference `{key}` has no target"));
        }
        let n = Self::anon_name(key, idx.max(0) as usize);
        if self.to == Dialect::Acme {
            format!(".{n}")
        } else {
            n
        }
    }

    fn is_local(&self, n: &str) -> bool {
        self.locals.last().is_some_and(|l| l.contains(n))
    }

    /// Name of a symbol in the target dialect.
    fn name(&mut self, n: &Name) -> String {
        match (n.kind, self.to) {
            (NameKind::Plain, Dialect::Acme) => {
                if self.is_local(&n.text) {
                    format!(".{}", n.text)
                } else {
                    n.text.clone()
                }
            }
            (NameKind::Plain, _) => n.text.clone(),
            (NameKind::ZoneLocal, Dialect::Acme) => format!(".{}", n.text),
            (NameKind::ZoneLocal, _) if self.in_macro > 0 => n.text.clone(),
            (NameKind::ZoneLocal, _) => format!("{}__z{}", n.text, self.zone),
            (NameKind::Cheap, Dialect::Acme) => format!("@{}", n.text),
            (NameKind::Cheap, Dialect::Raw) => format!("@{}", n.text),
            (NameKind::Cheap, Dialect::Kick) => format!("{}__c{}", n.text, self.cheap),
            (NameKind::Root, Dialect::Kick) => format!("@{}", n.text),
            (NameKind::Root, _) => n.text.clone(),
        }
    }

    /// Label definition text (records cheap regions).
    fn label_def(&mut self, n: &Name) -> String {
        let s = self.name(n);
        if n.kind == NameKind::Plain {
            self.cheap += 1;
        }
        let full = if self.to == Dialect::Acme && !self.prefix.is_empty() && n.kind == NameKind::Plain && !self.is_local(&n.text) {
            format!("{}_{s}", self.prefix.join("_"))
        } else {
            s
        };
        match self.to {
            Dialect::Acme if self.cpu.has_mnemonic(&full) => format!("{full}:"),
            Dialect::Acme => full,
            _ => format!("{full}:"),
        }
    }

    /// Byte order a data directive will use in the target.
    fn eff_endian(&self, size: u8, endian: Option<Endian>) -> Option<Endian> {
        match (endian, self.to) {
            (None, Dialect::Acme) if size > 1 => Some(self.cpu.endian()),
            (None, Dialect::Kick) if size > 1 && self.cpu.endian() == Endian::Big => Some(Endian::Big),
            (Some(Endian::Little), Dialect::Kick) => None,
            (e, _) => e,
        }
    }

    // ------------------------------------------------------- expressions

    fn num_text(&self, e: &Expr, v: i64) -> String {
        let raw = self.src.get(e.span.start as usize..e.span.end as usize).unwrap_or("");
        let ok_everywhere = raw.starts_with('$') || (raw.starts_with('%') && !raw.contains(['.', '#'])) || (!raw.is_empty() && raw.bytes().all(|b| b.is_ascii_digit()));
        if !raw.is_empty() && ok_everywhere && e.span.end > e.span.start {
            return raw.to_owned();
        }
        if v < 0 {
            return v.to_string();
        }
        if raw.starts_with('%') || raw.starts_with("0b") {
            return format!("%{v:b}");
        }
        if v > 9 {
            format!("${v:x}")
        } else {
            v.to_string()
        }
    }

    fn quote(&self, s: &str) -> String {
        let needs_escape = s.chars().any(|c| c == '"' || c == '\\' || (c as u32) < 0x20);
        match self.to {
            Dialect::Kick if needs_escape => {
                let mut o = String::from("@\"");
                for c in s.chars() {
                    match c {
                        '"' => o.push_str("\\\""),
                        '\\' => o.push_str("\\\\"),
                        '\n' => o.push_str("\\n"),
                        '\r' => o.push_str("\\r"),
                        '\t' => o.push_str("\\t"),
                        c if (c as u32) < 0x20 => {
                            let _ = write!(o, "\\${:02x}", c as u32);
                        }
                        c => o.push(c),
                    }
                }
                o.push('"');
                o
            }
            Dialect::Kick => format!("\"{s}\""),
            _ => {
                let mut o = String::from("\"");
                for c in s.chars() {
                    match c {
                        '"' => o.push_str("\\\""),
                        '\\' => o.push_str("\\\\"),
                        '\n' => o.push_str("\\n"),
                        '\r' => o.push_str("\\r"),
                        '\t' => o.push_str("\\t"),
                        '\0' => o.push_str("\\0"),
                        c => o.push(c),
                    }
                }
                o.push('"');
                o
            }
        }
    }

    /// Precedence of a binary operator in the target (higher binds tighter).
    fn prec(&self, op: BinOp) -> u8 {
        if self.to == Dialect::Acme {
            match op {
                BinOp::BitOr | BinOp::LOr => 3,
                BinOp::BitXor => 4,
                BinOp::BitAnd | BinOp::LAnd => 5,
                BinOp::Eq | BinOp::Ne => 6,
                BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 7,
                BinOp::Shl | BinOp::Shr | BinOp::Lsr => 8,
                BinOp::Add | BinOp::Sub => 9,
                BinOp::Mul | BinOp::Div | BinOp::FDiv | BinOp::IntDiv | BinOp::Mod => 10,
                BinOp::Pow => 12,
            }
        } else {
            match op {
                BinOp::LOr => 1,
                BinOp::LAnd => 2,
                BinOp::BitOr => 3,
                BinOp::BitXor => 4,
                BinOp::BitAnd => 5,
                BinOp::Eq | BinOp::Ne => 6,
                BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 7,
                BinOp::Shl | BinOp::Shr | BinOp::Lsr => 8,
                BinOp::Add | BinOp::Sub => 9,
                BinOp::Mul | BinOp::Div | BinOp::FDiv | BinOp::IntDiv | BinOp::Mod => 10,
                BinOp::Pow => 11,
            }
        }
    }

    fn expr(&mut self, e: &Expr) -> String {
        self.expr_p(e, 0)
    }

    /// Print with parentheses when the context binds tighter than `min`.
    fn expr_p(&mut self, e: &Expr, min: u8) -> String {
        let to = self.to;
        let (open, close) = ("(", ")");
        match &e.kind {
            ExprKind::Int(v) => self.num_text(e, *v),
            ExprKind::Float(v) => format!("{v}"),
            ExprKind::Str(s) => self.quote(s),
            ExprKind::Char(c) => match c {
                '\'' | '\\' if to != Dialect::Kick => format!("'\\{c}'"),
                c => format!("'{c}'"),
            },
            ExprKind::Bool(b) => match to {
                Dialect::Acme => (if *b { "1" } else { "0" }).into(),
                _ => b.to_string(),
            },
            ExprKind::Null => (if to == Dialect::Kick { "null" } else { "0" }).into(),
            ExprKind::Pc => "*".into(),
            ExprKind::Ident(n) => self.name(n),
            ExprKind::Defined(n) => {
                let nm = self.name(n);
                if to == Dialect::Raw {
                    format!("defined({nm})")
                } else {
                    self.warn("`defined()` in expressions");
                    format!("defined({nm})")
                }
            }
            ExprKind::Anon { key, offset } => self.anon_ref(key, *offset),
            ExprKind::Unary(op, x) => {
                let inner = self.expr_p(x, 13);
                let s = match (op, to) {
                    (UnOp::Neg, _) => format!("-{inner}"),
                    (UnOp::Pos, _) => inner,
                    (UnOp::Not, Dialect::Acme) => format!("({} = 0)", self.expr_p(x, 7)),
                    (UnOp::Not, _) => format!("!{inner}"),
                    (UnOp::BitNot, Dialect::Acme) => format!("!{inner}"),
                    (UnOp::BitNot, _) => format!("~{inner}"),
                    (UnOp::Lo, _) => {
                        let t = format!("<{}", self.expr_p(x, 1));
                        return wrap(t, min > 0);
                    }
                    (UnOp::Hi, _) => {
                        let t = format!(">{}", self.expr_p(x, 1));
                        return wrap(t, min > 0);
                    }
                    (UnOp::Bank, Dialect::Acme) => {
                        let t = format!("^{}", self.expr_p(x, 1));
                        return wrap(t, min > 0);
                    }
                    (UnOp::Bank, Dialect::Raw) => format!("bank({})", self.expr(x)),
                    (UnOp::Bank, Dialect::Kick) => format!("(({} >> 16) & $ff)", self.expr_p(x, 8)),
                };
                s
            }
            ExprKind::Binary(op, a, b) => {
                let p = self.prec(*op);
                // functions instead of operators
                let func = match (op, to) {
                    (BinOp::Pow, Dialect::Kick) => Some("pow"),
                    (BinOp::Mod, Dialect::Kick) => Some("mod"),
                    _ => None,
                };
                if let Some(f) = func {
                    return format!("{f}({}, {})", self.expr(a), self.expr(b));
                }
                if to == Dialect::Kick && matches!(op, BinOp::Div | BinOp::IntDiv) && self.from != Dialect::Kick {
                    return format!("floor({} / {})", self.expr_p(a, 10), self.expr_p(b, 11));
                }
                let sym = match (op, to) {
                    (BinOp::Add, _) => "+",
                    (BinOp::Sub, _) => "-",
                    (BinOp::Mul, _) => "*",
                    (BinOp::Div | BinOp::FDiv, _) => "/",
                    (BinOp::IntDiv, Dialect::Acme) => "DIV",
                    (BinOp::IntDiv, _) => "/",
                    (BinOp::Mod, _) => "%",
                    (BinOp::Pow, Dialect::Acme) => "^",
                    (BinOp::Pow, _) => "**",
                    (BinOp::Shl, _) => "<<",
                    (BinOp::Shr, _) => ">>",
                    (BinOp::Lsr, Dialect::Kick) => ">>",
                    (BinOp::Lsr, _) => ">>>",
                    (BinOp::BitAnd, _) => "&",
                    (BinOp::BitOr, _) => "|",
                    (BinOp::BitXor, Dialect::Acme) => "XOR",
                    (BinOp::BitXor, _) => "^",
                    (BinOp::Eq, Dialect::Acme) => "=",
                    (BinOp::Eq, _) => "==",
                    (BinOp::Ne, _) => "!=",
                    (BinOp::Lt, _) => "<",
                    (BinOp::Le, _) => "<=",
                    (BinOp::Gt, _) => ">",
                    (BinOp::Ge, _) => ">=",
                    (BinOp::LAnd, Dialect::Acme) => "&",
                    (BinOp::LAnd, _) => "&&",
                    (BinOp::LOr, Dialect::Acme) => "|",
                    (BinOp::LOr, _) => "||",
                };
                if to == Dialect::Acme && matches!(op, BinOp::LAnd | BinOp::LOr) {
                    self.warn("logical && / || become bitwise & / | in ACME");
                }
                let right_assoc = *op == BinOp::Pow;
                let l = self.expr_p(a, if right_assoc { p + 1 } else { p });
                let r = self.expr_p(b, if right_assoc { p } else { p + 1 });
                let s = format!("{l} {sym} {r}");
                if p < min {
                    format!("{open}{s}{close}")
                } else {
                    s
                }
            }
            ExprKind::Ternary(c, a, b) => {
                if to == Dialect::Acme {
                    self.warn("`?:` has no ACME equivalent");
                }
                let s = format!("{} ? {} : {}", self.expr_p(c, 1), self.expr(a), self.expr(b));
                if min > 0 {
                    format!("{open}{s}{close}")
                } else {
                    s
                }
            }
            ExprKind::Call(f, args) => {
                let fname = match &f.kind {
                    ExprKind::Ident(n) if n.text == "\u{0}str" => {
                        // ACME `!print a, b` concatenation helper
                        let a = args.first().map(|x| self.expr_p(x, 9)).unwrap_or_default();
                        return a;
                    }
                    _ => self.expr_p(f, 14),
                };
                let a: Vec<String> = args.iter().map(|x| self.expr(x)).collect();
                format!("{fname}({})", a.join(", "))
            }
            ExprKind::Field(base, f) => {
                if to == Dialect::Acme {
                    if let Some(b) = base.as_ident() {
                        if self.named_scopes.contains(b) {
                            return format!("{b}_{f}");
                        }
                    }
                    self.warn("member access `a.b` has no ACME equivalent");
                }
                format!("{}.{f}", self.expr_p(base, 14))
            }
            ExprKind::Index(base, i) => match to {
                Dialect::Kick => format!("{}.get({})", self.expr_p(base, 14), self.expr(i)),
                _ => format!("{}[{}]", self.expr_p(base, 14), self.expr(i)),
            },
            ExprKind::List(items) => {
                let a: Vec<String> = items.iter().map(|x| self.expr(x)).collect();
                match to {
                    Dialect::Kick => format!("List().add({})", a.join(", ")),
                    _ => format!("[{}]", a.join(", ")),
                }
            }
            ExprKind::Assign(op, t, v) => {
                let sym = match op {
                    AssignOp::Set => "=",
                    AssignOp::Add => "+=",
                    AssignOp::Sub => "-=",
                    AssignOp::Mul => "*=",
                    AssignOp::Div => "/=",
                };
                format!("{} {sym} {}", self.expr(t), self.expr(v))
            }
            ExprKind::IncDec { target, inc, post } => {
                let t = self.expr(target);
                let o = if *inc { "++" } else { "--" };
                if *post {
                    format!("{t}{o}")
                } else {
                    format!("{o}{t}")
                }
            }
        }
    }


    fn list(&mut self, items: &[Expr]) -> String {
        items.iter().map(|x| self.expr(x)).collect::<Vec<_>>().join(", ")
    }

    // ---------------------------------------------------------- operands

    fn tok_text(&self, t: &Token) -> String {
        match &t.tok {
            Tok::Int(_) | Tok::Float(_) => {
                let e = Expr::new(ExprKind::Int(0), t.span);
                let v = if let Tok::Int(v) = t.tok { v } else { 0 };
                if let Tok::Float(f) = t.tok {
                    return f.to_string();
                }
                self.num_text(&e, v)
            }
            Tok::Str(s) => self.quote(s),
            Tok::Char(c) => format!("'{c}'"),
            Tok::Ident(s) => s.clone(),
            Tok::Punct(p) => (*p).to_owned(),
            _ => String::new(),
        }
    }

    /// Rewrite operand tokens: local names, anonymous references,
    /// `a.b` for ACME, `==` / `=`.
    fn operand(&mut self, toks: &[Token]) -> String {
        let mut out = String::new();
        let mut i = 0;
        let n = toks.len();
        let term = |k: usize| k >= n || toks[k].is_punct(",") || toks[k].is_punct(")") || toks[k].is_punct("]");
        let start_pos = |k: usize| k == 0 || ["(", "[", ",", "#", "<", ">"].iter().any(|p| toks[k - 1].is_punct(p));
        while i < n {
            let t = &toks[i];
            let mut piece = None;
            let mut adv = 1;
            // ACME/raw `.local` / `@cheap`
            if (t.is_punct(".") || t.is_punct("@")) && i + 1 < n && !toks[i + 1].space_before && start_pos(i) || (t.is_punct(".") || t.is_punct("@")) && i + 1 < n && !toks[i + 1].space_before && i > 0 && toks[i].space_before {
                if let Tok::Ident(s) = &toks[i + 1].tok {
                    if self.from != Dialect::Kick {
                        let kind = if t.is_punct("@") || self.from == Dialect::Raw { NameKind::Cheap } else { NameKind::ZoneLocal };
                        piece = Some(self.name(&Name { kind, text: s.clone() }));
                        adv = 2;
                    }
                }
            }
            // anonymous references
            if piece.is_none() && self.from != Dialect::Kick && start_pos(i) {
                let sign = if t.is_punct("-") || t.is_punct("--") {
                    Some('-')
                } else if t.is_punct("+") || t.is_punct("++") {
                    Some('+')
                } else {
                    None
                };
                if let Some(sign) = sign {
                    let mut k = i;
                    let mut key = String::new();
                    while k < n {
                        match &toks[k].tok {
                            Tok::Punct(p) if p.chars().all(|c| c == sign) && (k == i || !toks[k].space_before) => {
                                key.push_str(p);
                                k += 1;
                            }
                            _ => break,
                        }
                    }
                    if term(k) {
                        piece = Some(self.anon_ref(&key, if sign == '-' { -1 } else { 1 }));
                        adv = k - i;
                    }
                }
            }
            if piece.is_none() && self.from == Dialect::Kick && t.is_punct("!") && i + 1 < n && !toks[i + 1].space_before {
                let mut k = i + 1;
                let mut key = "!".to_string();
                if let Tok::Ident(s) = &toks[k].tok {
                    key.push_str(s);
                    k += 1;
                }
                let mut cnt = 0i32;
                let mut sign = 0;
                while k < n && !toks[k].space_before {
                    let (s, w) = match toks[k].tok {
                        Tok::Punct("+") => (1, 1),
                        Tok::Punct("++") => (1, 2),
                        Tok::Punct("-") => (-1, 1),
                        Tok::Punct("--") => (-1, 2),
                        _ => break,
                    };
                    sign = s;
                    cnt += w;
                    k += 1;
                }
                if cnt > 0 {
                    piece = Some(self.anon_ref(&key, sign * cnt));
                    adv = k - i;
                }
            }
            // a.b scope access for ACME
            if piece.is_none() && self.to == Dialect::Acme && i + 2 < n && toks[i + 1].is_punct(".") && !toks[i + 1].space_before && !toks[i + 2].space_before {
                if let (Tok::Ident(a), Tok::Ident(b)) = (&t.tok, &toks[i + 2].tok) {
                    if self.named_scopes.contains(a) {
                        piece = Some(format!("{a}_{b}"));
                        adv = 3;
                    }
                }
            }
            if piece.is_none() {
                if let Tok::Ident(s) = &t.tok {
                    if self.to == Dialect::Acme && self.is_local(s) {
                        piece = Some(format!(".{s}"));
                    }
                }
            }
            if piece.is_none() {
                piece = Some(match (&t.tok, self.to, self.from) {
                    (Tok::Punct("=="), Dialect::Acme, _) => "=".into(),
                    (Tok::Punct("="), Dialect::Kick | Dialect::Raw, Dialect::Acme) => "==".into(),
                    _ => self.tok_text(t),
                });
            }
            if i > 0 && t.space_before {
                out.push(' ');
            }
            out.push_str(&piece.unwrap());
            i += adv;
        }
        out
    }

    fn suffix(&self, m: &str, sfx: Option<&str>) -> String {
        let Some(s) = sfx else { return String::new() };
        let id = self.cpu.info().id;
        let is65 = matches!(id, "6502" | "6502ni" | "6510" | "2a03" | "6507" | "65816") && self.cpu.has_mnemonic(m);
        let l = s.to_ascii_lowercase();
        if is65 {
            let size = match l.as_str() {
                "z" | "zp" | "b" | "1" => 1,
                "a" | "abs" | "w" | "2" => 2,
                _ => 3,
            };
            return match (self.to, size) {
                (Dialect::Acme, n) => format!("+{n}"),
                (_, 1) => ".z".into(),
                (_, 2) => ".a".into(),
                _ => ".l".into(),
            };
        }
        format!(".{s}")
    }

    // ------------------------------------------------------- statements

    fn block(&mut self, stmts: &[Stmt], ind: usize) {
        for (k, s) in stmts.iter().enumerate() {
            // `lbl: macro()` / `lbl: pseudo args`: the call prints the label
            if let (StmtKind::Label(n), Some(next)) = (&s.kind, stmts.get(k + 1)) {
                let carried = match &next.kind {
                    StmtKind::Call { label: Some(l), .. } => l == n,
                    StmtKind::Instr(i) => i.label.as_ref() == Some(n),
                    _ => false,
                };
                if carried && self.to != Dialect::Acme {
                    continue;
                }
            }
            if self.pending_header.is_some() && !matches!(s.kind, StmtKind::Platform { body: None, .. } | StmtKind::Syntax(_) | StmtKind::Nop) {
                let h = self.pending_header.take().unwrap();
                self.out.push_str(&h);
                self.out.push('\n');
            }
            self.stmt(s, ind);
        }
    }

    fn scoped(&mut self, stmts: &[Stmt], ind: usize) {
        let mut labels = HashSet::new();
        collect_labels(stmts, &mut labels);
        self.locals.push(labels);
        self.block(stmts, ind);
        self.locals.pop();
    }

    fn open_block(&mut self, head: &str, body: &[Stmt], ind: usize, span: Span, scoped: bool) {
        self.emit(ind, &format!("{head} {{"), span);
        if scoped {
            self.scoped(body, ind + 1);
        } else {
            self.block(body, ind + 1);
        }
        self.out.push_str(&"    ".repeat(ind));
        self.out.push_str("}\n");
    }

    fn d(&self, kick: &str, raw: &str, acme: &str) -> String {
        match self.to {
            Dialect::Kick => kick.to_owned(),
            Dialect::Raw => raw.to_owned(),
            Dialect::Acme => acme.to_owned(),
        }
    }

    fn args(&mut self, args: &[Arg]) -> String {
        args.iter()
            .map(|a| match &a.key {
                Some(k) => format!("{k}={}", self.expr(&a.value)),
                None => self.expr(&a.value),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn data_dir(&self, size: u8, endian: Option<Endian>) -> Option<String> {
        let endian = self.eff_endian(size, endian);
        let be = endian == Some(Endian::Big);
        Some(match (self.to, size, be) {
            (Dialect::Acme, 1, _) => "!byte".into(),
            (Dialect::Acme, 2, false) => "!word".into(),
            (Dialect::Acme, 2, true) => "!be16".into(),
            (Dialect::Acme, 3, false) => "!24".into(),
            (Dialect::Acme, 3, true) => "!be24".into(),
            (Dialect::Acme, 4, false) => "!32".into(),
            (Dialect::Acme, 4, true) => "!be32".into(),
            (Dialect::Kick, 1, _) => ".byte".into(),
            (Dialect::Kick, 2, false) => ".word".into(),
            (Dialect::Kick, 4, false) => ".dword".into(),
            (Dialect::Raw, 1, _) => ".byte".into(),
            (Dialect::Raw, 2, false) if endian.is_none() => ".word".into(),
            (Dialect::Raw, 2, false) => ".le16".into(),
            (Dialect::Raw, 2, true) => ".be16".into(),
            (Dialect::Raw, 3, false) => ".le24".into(),
            (Dialect::Raw, 3, true) => ".be24".into(),
            (Dialect::Raw, 4, false) if endian.is_none() => ".long".into(),
            (Dialect::Raw, 4, false) => ".le32".into(),
            (Dialect::Raw, 4, true) => ".be32".into(),
            (Dialect::Raw, 8, _) => ".quad".into(),
            _ => return None,
        })
    }

    fn cpu_name(&self, id: &str) -> String {
        let c = raw_cpu::canonical_id(id).unwrap_or("6502");
        match self.to {
            Dialect::Acme => match c {
                "6502ni" => "6502".into(),
                "6502" | "6510" | "2a03" | "6507" => "6510".into(),
                o => o.into(),
            },
            Dialect::Kick => match c {
                "6502ni" => "_6502NoIllegals".into(),
                "6502" | "6510" | "2a03" | "6507" => "_6502".into(),
                o => o.into(),
            },
            Dialect::Raw => c.into(),
        }
    }

    fn stmt(&mut self, s: &Stmt, ind: usize) {
        let sp = s.span;
        let to = self.to;
        match &s.kind {
            StmtKind::Label(n) => {
                let t = self.label_def(n);
                self.emit(ind, &t, sp);
            }
            StmtKind::AnonLabel(k) => {
                let t = self.anon_label(k);
                self.emit(ind, &t, sp);
            }
            StmtKind::Assign { name, value, kind } => {
                let n = self.name(name);
                let v = self.expr(value);
                let t = match (kind, to) {
                    (AssignKind::Const, Dialect::Acme) | (AssignKind::Label, Dialect::Acme) => format!("{n} = {v}"),
                    (_, Dialect::Acme) => format!("!set {n} = {v}"),
                    (AssignKind::Const, _) => format!(".const {n} = {v}"),
                    (AssignKind::Var | AssignKind::Set, _) => format!(".var {n} = {v}"),
                    (AssignKind::Label, Dialect::Kick) => format!(".label {n} = {v}"),
                    (AssignKind::Label, _) => format!("{n} = {v}"),
                };
                self.emit(ind, &t, sp);
            }
            StmtKind::Eval(e) => {
                if to == Dialect::Acme {
                    match &e.kind {
                        ExprKind::Assign(AssignOp::Set, t, v) => {
                            let t = self.expr(t);
                            let v = self.expr(v);
                            self.emit(ind, &format!("!set {t} = {v}"), sp);
                        }
                        ExprKind::IncDec { target, inc, .. } => {
                            let t = self.expr(target);
                            self.emit(ind, &format!("!set {t} = {t} {} 1", if *inc { "+" } else { "-" }), sp);
                        }
                        _ => self.todo(ind, "`.eval`", sp),
                    }
                } else {
                    let t = format!(".eval {}", self.expr(e));
                    self.emit(ind, &t, sp);
                }
            }
            StmtKind::Instr(i) => {
                if let (Some(l), Dialect::Kick | Dialect::Raw) = (&i.label, to) {
                    let t = self.label_def(l);
                    self.emit(ind, &t, sp);
                    self.out.pop();
                    self.out.push(' ');
                    let op = self.operand(&i.operand);
                    let line = if op.is_empty() { i.mnemonic.clone() } else { format!("{} {op}", i.mnemonic) };
                    self.out.push_str(&line);
                    self.out.push('\n');
                    return;
                }
                let op = self.operand(&i.operand);
                let sfx = self.suffix(&i.mnemonic, i.suffix.as_deref());
                let t = if op.is_empty() { format!("{}{sfx}", i.mnemonic) } else { format!("{}{sfx} {op}", i.mnemonic) };
                self.emit(ind + 1, &t, sp);
            }
            StmtKind::Data { size, endian, items } => match self.data_dir(*size, *endian) {
                Some(d) => {
                    let l = self.list(items);
                    self.emit(ind + 1, &format!("{d} {l}"), sp);
                }
                None => {
                    // big-endian or odd sizes for Kick: spell out the bytes
                    let mut parts = Vec::new();
                    for it in items {
                        let x = self.expr_p(it, 8);
                        let mut bytes: Vec<String> = (0..*size).map(|k| if k == 0 { format!("<{x}") } else { format!("({x} >> {}) & $ff", k * 8) }).collect();
                        if self.eff_endian(*size, *endian) == Some(Endian::Big) {
                            bytes.reverse();
                        }
                        parts.extend(bytes);
                    }
                    let b = self.d(".byte", ".byte", "!byte");
                    self.emit(ind + 1, &format!("{b} {}", parts.join(", ")), sp);
                }
            },
            StmtKind::Text { conv, xor, items } => {
                if xor.is_some() && to != Dialect::Acme {
                    self.todo(ind, "`!scrxor`", sp);
                    return;
                }
                let l = self.list(items);
                let t = match (conv, to) {
                    (TextConv::Current, Dialect::Acme) => format!("!text {l}"),
                    (TextConv::Current, _) => format!(".text {l}"),
                    (TextConv::Petscii, Dialect::Acme) => format!("!pet {l}"),
                    (TextConv::Screen, Dialect::Acme) if xor.is_some() => format!("!scrxor {}, {l}", self.expr(xor.as_ref().unwrap())),
                    (TextConv::Screen, Dialect::Acme) => format!("!scr {l}"),
                    (TextConv::Raw, Dialect::Acme) => format!("!raw {l}"),
                    (TextConv::Petscii, Dialect::Raw) => format!(".petscii {l}"),
                    (TextConv::Screen, Dialect::Raw) => format!(".screen {l}"),
                    (TextConv::Raw, Dialect::Raw) => {
                        let pad = "    ".repeat(ind + 1);
                        format!(".encoding \"ascii\"\n{pad}.text {l}\n{pad}.encoding \"{}\"", self.cur_enc)
                    }
                    (c, Dialect::Kick) => {
                        let enc = match c {
                            TextConv::Petscii => "petscii_mixed",
                            TextConv::Screen => "screencode_mixed",
                            TextConv::Raw => "ascii",
                            TextConv::Named(n) => n.as_str(),
                            TextConv::Current => unreachable!(),
                        };
                        let pad = "    ".repeat(ind + 1);
                        format!(".encoding \"{enc}\"\n{pad}.text {l}\n{pad}.encoding \"{}\"", self.cur_enc)
                    }
                    (TextConv::Named(n), _) => format!(".text {l} ; encoding {n}"),
                };
                self.emit(ind + 1, &t, sp);
            }
            StmtKind::Hex(b) => {
                let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
                let t = match to {
                    Dialect::Acme => format!("!hex {hex}"),
                    Dialect::Raw => format!(".hex {hex}"),
                    Dialect::Kick => format!(".byte {}", b.iter().map(|x| format!("${x:02x}")).collect::<Vec<_>>().join(", ")),
                };
                self.emit(ind + 1, &t, sp);
            }
            StmtKind::Fill { count, value, size, index_var } => {
                let c = self.expr(count);
                let uses_i = index_var.is_some() && value.as_ref().is_some_and(|v| mentions(v, index_var.as_deref().unwrap()));
                let v = value.as_ref().map(|v| self.expr(v)).unwrap_or_else(|| "0".into());
                let t = match to {
                    Dialect::Acme if uses_i || *size != 1 => {
                        let d = self.data_dir(*size, None).unwrap_or_else(|| "!byte".into());
                        format!("!for i, 0, {c} - 1 {{ {d} {v} }}")
                    }
                    Dialect::Acme => format!("!fill {c}, {v}"),
                    _ if *size == 2 => format!(".fillword {c}, {v}"),
                    _ => format!(".fill {c}, {v}"),
                };
                self.emit(ind + 1, &t, sp);
            }
            StmtKind::LoHiFill { name, count, value, index_var } => {
                let c = self.expr(count);
                let v = self.expr(value);
                if to == Dialect::Acme {
                    let n = name.as_ref().map(|n| n.text.clone()).unwrap_or_else(|| "lohi".into());
                    self.named_scopes.insert(n.clone());
                    self.emit(ind, &format!("{n}_lo !for {index_var}, 0, {c} - 1 {{ !byte <({v}) }}"), sp);
                    self.emit(ind, &format!("{n}_hi !for {index_var}, 0, {c} - 1 {{ !byte >({v}) }}"), sp);
                } else {
                    self.emit(ind + 1, &format!(".lohifill {c}, {v}"), sp);
                }
            }
            StmtKind::Skip(n) => {
                let x = self.expr(n);
                let t = self.d(&format!("* = * + {x}"), &format!("* = * + {x}"), &format!("!skip {x}"));
                self.emit(ind + 1, &t, sp);
            }
            StmtKind::Align { kind, fill } => {
                let f = fill.as_ref().map(|f| self.expr(f));
                let t = match (kind, to) {
                    (AlignKind::Mask { and, equal }, Dialect::Acme) => {
                        let (a, q) = (self.expr(and), self.expr(equal));
                        match f {
                            Some(f) => format!("!align {a}, {q}, {f}"),
                            None => format!("!align {a}, {q}"),
                        }
                    }
                    (AlignKind::Mask { and, equal }, _) => {
                        let (a, q) = (self.expr_p(and, 11), self.expr_p(equal, 11));
                        let is_zero = matches!(equal.kind, ExprKind::Int(0));
                        if let (ExprKind::Int(m), true) = (&and.kind, is_zero) {
                            if (m + 1).count_ones() == 1 {
                                match f {
                                    Some(f) => format!(".align {}, {f}", m + 1),
                                    None => format!(".align {}", m + 1),
                                }
                            } else {
                                format!(".fill ({q} - (* & {a})) & {a}, {}", f.unwrap_or_else(|| "0".into()))
                            }
                        } else {
                            format!(".fill ({q} - (* & {a})) & {a}, {}", f.unwrap_or_else(|| "0".into()))
                        }
                    }
                    (AlignKind::Multiple(n), Dialect::Acme) => {
                        let x = self.expr(n);
                        match &n.kind {
                            ExprKind::Int(v) if v.count_ones() == 1 => format!("!align {}, 0{}", v - 1, f.map(|f| format!(", {f}")).unwrap_or_default()),
                            _ => format!("!fill ({x} - (* % {x})) % {x}{}", f.map(|f| format!(", {f}")).unwrap_or_default()),
                        }
                    }
                    (AlignKind::Multiple(n), _) => {
                        let x = self.expr(n);
                        match f {
                            Some(f) => format!(".align {x}, {f}"),
                            None => format!(".align {x}"),
                        }
                    }
                };
                self.emit(ind + 1, &t, sp);
            }
            StmtKind::Org { addr, name, overlay, invisible } => {
                let a = self.expr(addr);
                let t = match to {
                    Dialect::Acme => {
                        let mut t = format!("* = {a}");
                        if *overlay {
                            t.push_str(", overlay");
                        }
                        if *invisible {
                            t.push_str(", invisible");
                        }
                        t
                    }
                    Dialect::Kick => match name {
                        Some(n) => format!("* = {a} {}", self.expr(n)),
                        None => format!("* = {a}"),
                    },
                    Dialect::Raw => match name {
                        Some(n) => format!(".org {a} {}", self.expr(n)),
                        None => format!(".org {a}"),
                    },
                };
                let t = if *invisible && to != Dialect::Acme { format!("{t} virtual") } else { t };
                if *overlay && to != Dialect::Acme {
                    self.warn("`*=` overlay");
                }
                self.emit(ind, &t, sp);
            }
            StmtKind::PseudoPc { addr, body } => {
                let a = self.expr(addr);
                let head = self.d(&format!(".pseudopc {a}"), &format!(".pseudopc {a}"), &format!("!pseudopc {a}"));
                match body {
                    Some(b) => self.open_block(&head, b, ind, sp, false),
                    None => {
                        if to == Dialect::Kick {
                            self.warn("`!pseudopc` without a block (raw extension in Kick output)");
                        }
                        self.emit(ind, &head, sp);
                    }
                }
            }
            StmtKind::RealPc => {
                let t = self.d(".realpc", ".realpc", "!realpc");
                if to == Dialect::Kick {
                    self.warn("`!realpc` (raw extension in Kick output)");
                }
                self.emit(ind, &t, sp);
            }
            StmtKind::Include { path, .. } => {
                let p = self.expr(path);
                let t = self.d(&format!("#import {p}"), &format!(".include {p}"), &format!("!source {p}"));
                self.emit(ind, &t, sp);
            }
            StmtKind::ImportOnce => match to {
                Dialect::Acme => self.todo(ind, "`#importonce`", sp),
                _ => {
                    let t = self.d("#importonce", ".once", "");
                    self.emit(ind, &t, sp);
                }
            },
            StmtKind::Binary { path, size, offset, kind } => {
                let p = self.expr(path);
                let mut off = offset.as_ref().map(|o| self.expr(o));
                if *kind == BinaryKind::C64 && to != Dialect::Kick {
                    off = Some(match off {
                        Some(o) => format!("{o} + 2"),
                        None => "2".into(),
                    });
                }
                let sz = size.as_ref().map(|s| self.expr(s));
                let t = match to {
                    Dialect::Acme => match (sz, off) {
                        (None, None) => format!("!binary {p}"),
                        (Some(s), None) => format!("!binary {p}, {s}"),
                        (s, Some(o)) => format!("!binary {p}, {}, {o}", s.unwrap_or_default()),
                    },
                    _ => {
                        let head = match (to, kind) {
                            (Dialect::Kick, BinaryKind::C64) => format!(".import c64 {p}"),
                            (Dialect::Kick, BinaryKind::Text) => format!(".import text {p}"),
                            (Dialect::Kick, _) => format!(".import binary {p}"),
                            _ => format!(".incbin {p}"),
                        };
                        match (off, sz) {
                            (None, None) => head,
                            (Some(o), None) => format!("{head}, {o}"),
                            (o, Some(s)) => format!("{head}, {}, {s}", o.unwrap_or_else(|| "0".into())),
                        }
                    }
                };
                self.emit(ind + 1, &t, sp);
            }
            StmtKind::If { cond, then, else_ } => self.if_stmt(cond, then, else_.as_deref(), ind, sp, false),
            StmtKind::IfDef { name, negate, then, else_ } => {
                let n = self.name(name);
                let head = match to {
                    Dialect::Acme => format!("!if{}def {n}", if *negate { "n" } else { "" }),
                    _ => {
                        if to == Dialect::Kick {
                            self.warn("`!ifdef` (raw extension `.ifdef` in Kick output)");
                        }
                        format!(".if{}def {n}", if *negate { "n" } else { "" })
                    }
                };
                self.emit(ind, &format!("{head} {{"), sp);
                self.block(then, ind + 1);
                self.close_else(else_.as_deref(), ind);
            }
            StmtKind::ForC { init, cond, step, body } => {
                if to != Dialect::Kick {
                    // `var i = a; i < b; i++` -> !for i, a, b - 1
                    if let (Some(StmtKind::Assign { name, value, .. }), Some(c), [st]) = (init.first().map(|x| &x.kind), cond, step.as_slice()) {
                        if let ExprKind::Binary(op @ (BinOp::Lt | BinOp::Le), a, b) = &c.kind {
                            if a.as_ident() == Some(name.text.as_str()) && matches!(&st.kind, ExprKind::IncDec { inc: true, .. }) {
                                let from = self.expr(value);
                                if to == Dialect::Raw {
                                    let range = if *op == BinOp::Lt { ".." } else { "..=" };
                                    let end = self.expr(b);
                                    self.open_block(&format!(".for {} in {from}{range}{end}", name.text), body, ind, sp, true);
                                    return;
                                }
                                let to_s = if *op == BinOp::Lt { format!("{} - 1", self.expr_p(b, 9)) } else { self.expr(b) };
                                self.open_block(&format!("!for {}, {from}, {to_s}", name.text), body, ind, sp, true);
                                return;
                            }
                        }
                    }
                    if to == Dialect::Raw {
                        // general form: init, then .while with the steps at the end
                        self.emit(ind, "{", sp);
                        for st in init {
                            self.stmt(st, ind + 1);
                        }
                        let c = cond.as_ref().map(|c| self.expr(c)).unwrap_or_else(|| "true".into());
                        self.emit(ind + 1, &format!(".while {c} {{"), sp);
                        self.scoped(body, ind + 2);
                        for x in step {
                            let t = format!(".eval {}", self.expr(x));
                            self.out.push_str(&format!("{}{t}\n", "    ".repeat(ind + 2)));
                        }
                        self.out.push_str(&format!("{}}}\n{}}}\n", "    ".repeat(ind + 1), "    ".repeat(ind)));
                        return;
                    }
                    self.todo(ind, "C-style `.for`", sp);
                    return;
                }
                let i: Vec<String> = init
                    .iter()
                    .map(|s| match &s.kind {
                        StmtKind::Assign { name, value, .. } => format!("var {} = {}", name.text, self.expr(value)),
                        StmtKind::Eval(e) => self.expr(e),
                        _ => String::new(),
                    })
                    .collect();
                let c = cond.as_ref().map(|c| self.expr(c)).unwrap_or_default();
                let st: Vec<String> = step.iter().map(|x| self.expr(x)).collect();
                self.open_block(&format!(".for ({}; {c}; {})", i.join(", "), st.join(", ")), body, ind, sp, true);
            }
            StmtKind::ForRange { var, from, to: end, step, inclusive, body } => {
                let (a, b) = (self.expr(from), self.expr(end));
                let st = step.as_ref().map(|s| self.expr(s));
                let head = match to {
                    Dialect::Acme => {
                        if st.is_some() {
                            self.warn("`.for` with a step (ACME has none)");
                        }
                        let last = if *inclusive { b } else { format!("{} - 1", self.expr_p(end, 9)) };
                        if var.starts_with('\0') {
                            format!("!for rep__, {a}, {last}")
                        } else {
                            format!("!for {var}, {a}, {last}")
                        }
                    }
                    Dialect::Kick => {
                        let v = if var.starts_with('\0') { "rep__" } else { var.as_str() };
                        let cmp = if *inclusive { "<=" } else { "<" };
                        let inc = match st {
                            Some(s) => format!("{v} += {s}"),
                            None => format!("{v}++"),
                        };
                        format!(".for (var {v} = {a}; {v} {cmp} {b}; {inc})")
                    }
                    Dialect::Raw => {
                        let v = if var.starts_with('\0') { "rep__" } else { var.as_str() };
                        let range = if *inclusive { "..=" } else { ".." };
                        match st {
                            Some(s) => format!(".for {v} in {a}{range}{b} step {s}"),
                            None => format!(".for {v} in {a}{range}{b}"),
                        }
                    }
                };
                self.open_block(&head, body, ind, sp, true);
            }
            StmtKind::ForIn { var, list, body } => {
                let l = self.expr(list);
                match to {
                    Dialect::Acme => self.open_block(&format!("!for {var} in {l}"), body, ind, sp, true),
                    Dialect::Raw => self.open_block(&format!(".for {var} in {l}"), body, ind, sp, true),
                    Dialect::Kick => {
                        self.emit(ind, &format!(".for (var i__ = 0; i__ < {l}.size(); i__++) {{"), sp);
                        self.emit(ind + 1, &format!(".var {var} = {l}.get(i__)"), sp);
                        self.scoped(body, ind + 1);
                        self.out.push_str(&"    ".repeat(ind));
                        self.out.push_str("}\n");
                    }
                }
            }
            StmtKind::While { cond, body } => {
                let c = self.expr(cond);
                let head = self.d(&format!(".while ({c})"), &format!(".while {c}"), &format!("!while {c}"));
                self.open_block(&head, body, ind, sp, true);
            }
            StmtKind::Do { pre, body, post } => {
                if to == Dialect::Acme {
                    let p = pre.as_ref().map(|(u, c)| format!(" {} {}", if *u { "until" } else { "while" }, self.expr(c))).unwrap_or_default();
                    self.emit(ind, &format!("!do{p} {{"), sp);
                    self.block(body, ind + 1);
                    let q = post.as_ref().map(|(u, c)| format!(" {} {}", if *u { "until" } else { "while" }, self.expr(c))).unwrap_or_default();
                    self.out.push_str(&format!("{}}}{q}\n", "    ".repeat(ind)));
                } else if let (Some((until, c)), None) = (pre, post) {
                    let cs = if *until { format!("!({})", self.expr(c)) } else { self.expr(c) };
                    let head = self.d(&format!(".while ({cs})"), &format!(".while {cs}"), "");
                    self.open_block(&head, body, ind, sp, true);
                } else {
                    self.todo(ind, "`!do` with a post condition", sp);
                }
            }
            StmtKind::Scope { name, body } => {
                if to == Dialect::Acme {
                    match name {
                        Some(n) => {
                            self.emit(ind, &n.text.clone(), sp);
                            self.prefix.push(n.text.clone());
                            self.emit(ind, &format!("!zone {} {{", n.text), sp);
                            self.block(body, ind + 1);
                            self.prefix.pop();
                        }
                        None => {
                            self.emit(ind, "!zone {", sp);
                            self.scoped(body, ind + 1);
                        }
                    }
                    self.out.push_str(&format!("{}}}\n", "    ".repeat(ind)));
                } else {
                    let head = match name {
                        Some(n) => format!("{}:", n.text),
                        None => String::new(),
                    };
                    self.emit(ind, format!("{head} {{").trim_start(), sp);
                    self.block(body, ind + 1);
                    self.out.push_str(&format!("{}}}\n", "    ".repeat(ind)));
                }
            }
            StmtKind::Zone { name, body } => {
                self.zone += 1;
                match to {
                    Dialect::Acme => {
                        let head = match name {
                            Some(n) => format!("!zone {n}"),
                            None => "!zone".into(),
                        };
                        match body {
                            Some(b) => self.open_block(&head, b, ind, sp, false),
                            None => self.emit(ind, &head, sp),
                        }
                    }
                    _ => match body {
                        Some(b) => {
                            self.emit(ind, "{", sp);
                            self.block(b, ind + 1);
                            self.out.push_str(&format!("{}}}\n", "    ".repeat(ind)));
                        }
                        None => {
                            let c = self.cmt();
                            self.emit(ind, &format!("{c} zone {}", name.clone().unwrap_or_default()), sp);
                        }
                    },
                }
            }
            StmtKind::Namespace { name, body, file } => match to {
                Dialect::Acme => {
                    self.prefix.push(name.clone());
                    if let Some(b) = body {
                        self.block(b, ind);
                        self.prefix.pop();
                    }
                }
                _ => {
                    let kw = if *file { ".filenamespace" } else { ".namespace" };
                    match body {
                        Some(b) => self.open_block(&format!("{kw} {name}"), b, ind, sp, false),
                        None => self.emit(ind, &format!("{kw} {name}"), sp),
                    }
                }
            },
            StmtKind::MacroDef(m) => self.macro_def(m, ind, sp),
            StmtKind::Call { name, args, label } => {
                let n = self.expr_p(name, 14);
                let a = self.list(args);
                if let (Some(l), Dialect::Kick | Dialect::Raw) = (label, to) {
                    let t = self.label_def(l);
                    self.emit(ind, &format!("{t} {n}({a})"), sp);
                    return;
                }
                let t = match to {
                    Dialect::Acme => {
                        if a.is_empty() {
                            format!("+{n}")
                        } else {
                            format!("+{n} {a}")
                        }
                    }
                    _ => format!("{n}({a})"),
                };
                self.emit(ind + 1, &t, sp);
            }
            StmtKind::PseudoCall { .. } => {}
            StmtKind::Return(v) => {
                let t = match v {
                    Some(x) => format!(".return {}", self.expr(x)),
                    None => ".return".into(),
                };
                if to == Dialect::Acme {
                    self.todo(ind, "`.return`", sp);
                } else {
                    self.emit(ind + 1, &t, sp);
                }
            }
            StmtKind::Print { expr, now } => {
                let e = self.expr(expr);
                let t = match to {
                    Dialect::Acme => {
                        self.warn("`.print` becomes `!warn` in ACME");
                        format!("!warn {e}")
                    }
                    _ => format!(".print{} {e}", if *now { "now" } else { "" }),
                };
                self.emit(ind, &t, sp);
            }
            StmtKind::Message { level, args } => {
                let t = match to {
                    Dialect::Acme => {
                        let kw = match level {
                            MsgLevel::Warn => "!warn",
                            MsgLevel::Error => "!error",
                            MsgLevel::Serious => "!serious",
                        };
                        format!("{kw} {}", self.list(args))
                    }
                    _ => {
                        let joined = args.iter().map(|a| self.expr_p(a, 9)).collect::<Vec<_>>().join(" + ");
                        match (level, to) {
                            (MsgLevel::Warn, Dialect::Kick) => format!(".print \"warning: \" + {joined}"),
                            (MsgLevel::Warn, _) => format!(".warn {joined}"),
                            _ => format!(".error {joined}"),
                        }
                    }
                };
                self.emit(ind, &t, sp);
            }
            StmtKind::ErrorIf { cond, msg } => {
                let (c, m) = (self.expr(cond), self.expr(msg));
                let t = match to {
                    Dialect::Acme => format!("!if {c} {{ !error {m} }}"),
                    _ => format!(".errorif {c}, {m}"),
                };
                self.emit(ind, &t, sp);
            }
            StmtKind::Assert { desc, actual, expected } => {
                let a = self.expr(actual);
                let d = desc.as_ref().map(|d| self.expr(d)).unwrap_or_else(|| self.quote("assertion"));
                let t = match (to, expected) {
                    (Dialect::Acme, Some(e)) => format!("!if ({a}) != ({}) {{ !error {d} }}", self.expr(e)),
                    (Dialect::Acme, None) => format!("!if ({a}) = 0 {{ !error {d} }}"),
                    (_, Some(e)) => format!(".assert {d}, {a}, {}", self.expr(e)),
                    (Dialect::Kick, None) => format!(".errorif !({a}), {d}"),
                    (_, None) => format!(".assert {a}, {d}"),
                };
                self.emit(ind, &t, sp);
            }
            StmtKind::AssertCode { desc, actual, expected } => {
                if to == Dialect::Acme {
                    self.todo(ind, "`.assert` with code", sp);
                } else {
                    let d = self.expr(desc);
                    self.emit(ind, &format!(".assert {d}, {{"), sp);
                    self.block(actual, ind + 1);
                    self.out.push_str(&format!("{}}}, {{\n", "    ".repeat(ind)));
                    self.block(expected, ind + 1);
                    self.out.push_str(&format!("{}}}\n", "    ".repeat(ind)));
                }
            }
            StmtKind::AssertError { desc, body } => {
                if to == Dialect::Acme {
                    self.todo(ind, "`.asserterror`", sp);
                } else {
                    let d = self.expr(desc);
                    self.open_block(&format!(".asserterror {d},"), body, ind, sp, false);
                }
            }
            StmtKind::Cpu(c) => {
                if c == "-" {
                    if to == Dialect::Acme {
                        self.out.push_str(&format!("{}}}\n", "    ".repeat(ind)));
                    }
                    return;
                }
                let (block, name) = match c.strip_prefix('+') {
                    Some(n) => (true, n),
                    None => (false, c.as_str()),
                };
                if let Some(cp) = raw_cpu::by_name(name) {
                    self.cpu = cp;
                }
                let n = self.cpu_name(name);
                let t = match to {
                    Dialect::Acme if block => format!("!cpu {n} {{"),
                    Dialect::Acme => format!("!cpu {n}"),
                    _ => format!(".cpu {n}"),
                };
                if to == Dialect::Kick && !n.starts_with('_') {
                    self.warn(format!("CPU `{n}` (raw extension in Kick output)"));
                }
                self.emit(ind, &t, sp);
            }
            StmtKind::Platform { name, body } => {
                if let Some(cp) = raw_platform::by_name(name).and_then(|p| raw_cpu::by_name(p.cpu)) {
                    self.cpu = cp;
                }
                let kw = self.d(".platform", ".platform", "!platform");
                match body {
                    Some(b) => self.open_block(&format!("{kw} {name}"), b, ind, sp, false),
                    None => {
                        if to != Dialect::Raw {
                            self.warn("`.platform` (raw extension)");
                        }
                        self.emit(ind, &format!("{kw} {name}"), sp);
                    }
                }
            }
            StmtKind::PlatformDir { ns, name, args, body } => {
                let a = self.args(args);
                let n = match ns {
                    Some(ns) => format!("{ns}.{name}"),
                    None => name.clone(),
                };
                let lead = if to == Dialect::Acme { "!" } else { "." };
                let head = if a.is_empty() { format!("{lead}{n}") } else { format!("{lead}{n} {a}") };
                match body {
                    Some(b) => {
                        let saved = self.cpu;
                        if name == "spc" {
                            self.cpu = raw_cpu::by_name("spc700").unwrap();
                        }
                        self.open_block(&head, b, ind, sp, false);
                        self.cpu = saved;
                    }
                    None => self.emit(ind, &head, sp),
                }
                if to != Dialect::Raw {
                    self.warn(format!("platform directive `{n}` (raw extension)"));
                }
            }
            StmtKind::SegmentDef { name, params } => {
                if to == Dialect::Acme {
                    self.todo(ind, "`.segmentdef`", sp);
                } else {
                    let p = self.args(params);
                    let t = if p.is_empty() { format!(".segmentdef {name}") } else { format!(".segmentdef {name} [{p}]") };
                    self.emit(ind, &t, sp);
                }
            }
            StmtKind::Segment { name, params, body } => {
                if to == Dialect::Acme {
                    if let Some(b) = body {
                        self.todo(ind, "`.segment` block", sp);
                        self.block(b, ind);
                    } else {
                        self.todo(ind, "`.segment`", sp);
                    }
                    return;
                }
                let p = self.args(params);
                let head = if p.is_empty() { format!(".segment {name}") } else { format!(".segment {name} [{p}]") };
                match body {
                    Some(b) => self.open_block(&head, b, ind, sp, false),
                    None => self.emit(ind, &head, sp),
                }
            }
            StmtKind::SegmentOut { params } | StmtKind::File { params, body: None } => {
                if to == Dialect::Acme {
                    self.todo(ind, "segment output", sp);
                    return;
                }
                let kw = if matches!(s.kind, StmtKind::SegmentOut { .. }) { ".segmentout" } else { ".file" };
                let p = self.args(params);
                self.emit(ind, &format!("{kw} [{p}]"), sp);
            }
            StmtKind::File { params, body: Some(b) } => {
                if to == Dialect::Acme {
                    self.todo(ind, "`.file`", sp);
                    return;
                }
                let p = self.args(params);
                self.open_block(&format!(".file [{p}]"), b, ind, sp, false);
            }
            StmtKind::Disk { params, files } => {
                if to == Dialect::Acme {
                    self.todo(ind, "`.disk`", sp);
                    return;
                }
                let p = self.args(params);
                self.emit(ind, &format!(".disk [{p}] {{"), sp);
                for f in files {
                    let fa = self.args(f);
                    self.out.push_str(&format!("{}[{fa}],\n", "    ".repeat(ind + 1)));
                }
                self.out.push_str(&format!("{}}}\n", "    ".repeat(ind)));
            }
            StmtKind::MemBlock(n) => {
                if to == Dialect::Acme {
                    self.todo(ind, "`.memblock`", sp);
                } else {
                    let t = format!(".memblock {}", self.expr(n));
                    self.emit(ind, &t, sp);
                }
            }
            StmtKind::Output { file, format } => {
                let f = self.expr(file);
                let t = match (to, format) {
                    (Dialect::Acme, Some(fm)) => format!("!to {f}, {fm}"),
                    (Dialect::Acme, None) => format!("!to {f}"),
                    (_, Some(fm)) => format!(".output {f}, {fm}"),
                    (_, None) => format!(".output {f}"),
                };
                if to == Dialect::Kick {
                    self.warn("`!to` (raw extension `.output` in Kick output)");
                }
                self.emit(ind, &t, sp);
            }
            StmtKind::Encoding(e) => {
                if let ExprKind::Str(n) = &e.kind {
                    if n == "\0pop" {
                        if to == Dialect::Acme {
                            self.out.push_str(&format!("{}}}\n", "    ".repeat(ind)));
                        }
                        return;
                    }
                    let canon = crate::encoding::canonical(n).unwrap_or("ascii");
                    self.cur_enc = canon.to_owned();
                    let t = match to {
                        Dialect::Acme => format!(
                            "!convtab {}",
                            match canon {
                                "petscii_mixed" | "petscii_upper" => "pet",
                                "screencode_mixed" | "screencode_upper" => "scr",
                                _ => "raw",
                            }
                        ),
                        _ => format!(".encoding \"{canon}\""),
                    };
                    self.emit(ind, &t, sp);
                } else {
                    let t = format!("{} {}", self.d(".encoding", ".encoding", "!convtab"), self.expr(e));
                    self.emit(ind, &t, sp);
                }
            }
            StmtKind::ConvTabFile(f) => {
                if to == Dialect::Acme {
                    let t = format!("!convtab {}", self.expr(f));
                    self.emit(ind, &t, sp);
                } else {
                    self.todo(ind, "conversion table file", sp);
                }
            }
            StmtKind::Xor { value, body } => {
                if to != Dialect::Acme {
                    self.todo(ind, "`!xor`", sp);
                    if let Some(b) = body {
                        self.block(b, ind);
                    }
                    return;
                }
                let v = self.expr(value);
                match body {
                    Some(b) => self.open_block(&format!("!xor {v}"), b, ind, sp, false),
                    None => self.emit(ind, &format!("!xor {v}"), sp),
                }
            }
            StmtKind::InitMem(v) => {
                if to == Dialect::Acme {
                    let t = format!("!initmem {}", self.expr(v));
                    self.emit(ind, &t, sp);
                } else {
                    self.todo(ind, "`!initmem`", sp);
                }
            }
            StmtKind::CpuFlag(f) => {
                let t = match (to, f.as_str()) {
                    (_, "push" | "pop") => return,
                    (Dialect::Acme, "a16") => "!al".to_owned(),
                    (Dialect::Acme, "a8") => "!as".to_owned(),
                    (Dialect::Acme, "i16") => "!rl".to_owned(),
                    (Dialect::Acme, "i8") => "!rs".to_owned(),
                    (_, f) => format!(".{f}"),
                };
                self.emit(ind, &t, sp);
            }
            StmtKind::SetDp(v) => {
                let t = format!("{} {}", self.d(".setdp", ".setdp", "!setdp"), self.expr(v));
                self.emit(ind, &t, sp);
            }
            StmtKind::Endian(e) => {
                let w = match e {
                    Some(Endian::Big) => "be",
                    Some(Endian::Little) => "le",
                    None => "cpu",
                };
                let t = format!("{} {w}", self.d(".endian", ".endian", "!endian"));
                self.emit(ind, &t, sp);
            }
            StmtKind::Struct { name, fields } => {
                if to == Dialect::Acme {
                    self.todo(ind, "`.struct`", sp);
                } else {
                    self.emit(ind, &format!(".struct {name} {{{}}}", fields.join(", ")), sp);
                }
            }
            StmtKind::Enum { items } => {
                if to == Dialect::Acme {
                    let mut next = String::from("0");
                    for (n, v) in items {
                        if let Some(v) = v {
                            next = self.expr(v);
                        }
                        self.emit(ind, &format!("{n} = {next}"), sp);
                        next = format!("{n} + 1");
                    }
                } else {
                    let parts: Vec<String> = items
                        .iter()
                        .map(|(n, v)| match v {
                            Some(v) => format!("{n} = {}", self.expr(v)),
                            None => n.clone(),
                        })
                        .collect();
                    self.emit(ind, &format!(".enum {{{}}}", parts.join(", ")), sp);
                }
            }
            StmtKind::PpDefine(n) => {
                let t = self.d(&format!("#define {n}"), &format!("#define {n}"), &format!("{n} = 1"));
                self.emit(ind, &t, sp);
            }
            StmtKind::PpUndef(n) => {
                if to == Dialect::Acme {
                    self.todo(ind, "`#undef`", sp);
                } else {
                    self.emit(ind, &format!("#undef {n}"), sp);
                }
            }
            StmtKind::PpIf { branches, else_ } => {
                if to == Dialect::Acme {
                    for (i, (c, b)) in branches.iter().enumerate() {
                        let cs = match c.as_ident() {
                            Some(n) => format!("!ifdef {n} {{"),
                            None => format!("!if {} {{", self.expr(c)),
                        };
                        let head = if i == 0 { cs } else { format!("}} else {{ {cs}") };
                        self.emit(ind, &head, sp);
                        self.block(b, ind + 1);
                    }
                    if let Some(e) = else_ {
                        self.emit(ind, "} else {", sp);
                        self.block(e, ind + 1);
                    }
                    let closes = "}".repeat(branches.len());
                    self.out.push_str(&format!("{}{closes}\n", "    ".repeat(ind)));
                    return;
                }
                for (i, (c, b)) in branches.iter().enumerate() {
                    let kw = if i == 0 { "#if" } else { "#elif" };
                    let t = format!("{kw} {}", self.expr(c));
                    self.emit(0, &t, sp);
                    self.block(b, ind);
                }
                if let Some(e) = else_ {
                    self.out.push_str("#else\n");
                    self.block(e, ind);
                }
                self.out.push_str("#endif\n");
            }
            StmtKind::Break(c) => {
                if to == Dialect::Acme {
                    self.todo(ind, "`.break`", sp);
                } else {
                    let t = match c {
                        Some(c) => format!(".break {}", self.expr(c)),
                        None => ".break".into(),
                    };
                    self.emit(ind, &t, sp);
                }
            }
            StmtKind::Watch { args } => {
                if to == Dialect::Acme {
                    self.todo(ind, "`.watch`", sp);
                } else {
                    let t = format!(".watch {}", self.list(args));
                    self.emit(ind, &t, sp);
                }
            }
            StmtKind::SymbolList(f) => {
                if to == Dialect::Acme {
                    let t = format!("!sl {}", self.expr(f));
                    self.emit(ind, &t, sp);
                } else {
                    self.todo(ind, "`!sl` (use `raw build --labels` instead)", sp);
                }
            }
            StmtKind::Eof => {
                if to == Dialect::Kick {
                    self.todo(ind, "`!eof`", sp);
                } else {
                    let t = self.d("", ".eof", "!eof");
                    self.emit(ind, &t, sp);
                }
            }
            StmtKind::ZpBlock(b) => {
                if to == Dialect::Acme {
                    self.block(b, ind);
                } else {
                    self.open_block(".zp", b, ind, sp, false);
                }
            }
            StmtKind::Group(b) => self.block(b, ind),
            StmtKind::Syntax(_) | StmtKind::Nop => {}
        }
    }

    fn close_else(&mut self, else_: Option<&[Stmt]>, ind: usize) {
        let pad = "    ".repeat(ind);
        match else_ {
            Some(e) => {
                if let ([one], true) = (e, true) {
                    if let StmtKind::If { cond, then, else_ } = &one.kind {
                        self.out.push_str(&pad);
                        self.out.push('}');
                        self.if_stmt(cond, then, else_.as_deref(), ind, one.span, true);
                        return;
                    }
                }
                let kw = self.d("} else {", "} .else {", "} else {");
                self.out.push_str(&format!("{pad}{kw}\n"));
                self.block(e, ind + 1);
                self.out.push_str(&format!("{pad}}}\n"));
            }
            None => self.out.push_str(&format!("{pad}}}\n")),
        }
    }

    fn if_stmt(&mut self, cond: &Expr, then: &[Stmt], else_: Option<&[Stmt]>, ind: usize, sp: Span, chained: bool) {
        let c = self.expr(cond);
        let head = match (self.to, chained) {
            (Dialect::Acme, false) => format!("!if {c} {{"),
            (Dialect::Acme, true) => format!(" else if {c} {{"),
            (Dialect::Kick, false) => format!(".if ({c}) {{"),
            (Dialect::Kick, true) => format!(" else .if ({c}) {{"),
            (Dialect::Raw, false) => format!(".if {c} {{"),
            (Dialect::Raw, true) => format!(" .elif {c} {{"),
        };
        if chained {
            self.out.push_str(&head);
            self.out.push('\n');
        } else {
            self.emit(ind, &head, sp);
        }
        self.block(then, ind + 1);
        self.close_else(else_, ind);
    }

    fn macro_def(&mut self, m: &MacroDef, ind: usize, sp: Span) {
        let to = self.to;
        let pnames: Vec<String> = m.params.iter().map(|p| p.name.trim_start_matches('.').to_owned()).collect();
        let mut labels = HashSet::new();
        collect_labels(&m.body, &mut labels);
        if to == Dialect::Acme {
            labels.extend(pnames.iter().cloned());
        }
        self.in_macro += 1;
        self.locals.push(labels);
        match (m.kind, to) {
            (MacroKind::Macro, Dialect::Acme) => {
                let ps: Vec<String> = m.params.iter().zip(&pnames).map(|(p, n)| format!("{}.{n}", if p.by_ref { "~" } else { "" })).collect();
                self.emit(ind, &format!("!macro {} {} {{", m.name, ps.join(", ")).replace("  {", " {"), sp);
                self.block(&m.body, ind + 1);
                self.out.push_str(&format!("{}}}\n", "    ".repeat(ind)));
            }
            (MacroKind::Macro, _) => {
                if m.params.iter().any(|p| p.by_ref) {
                    self.warn("by-reference macro parameters (`~name`)");
                }
                let ps: Vec<String> = m
                    .params
                    .iter()
                    .zip(&pnames)
                    .map(|(p, n)| match &p.default {
                        Some(d) if to == Dialect::Raw => format!("{n} = {}", self.expr(d)),
                        _ => n.clone(),
                    })
                    .collect();
                self.emit(ind, &format!(".macro {}({}) {{", m.name, ps.join(", ")), sp);
                self.block(&m.body, ind + 1);
                self.out.push_str(&format!("{}}}\n", "    ".repeat(ind)));
            }
            (MacroKind::Pseudo, Dialect::Acme) => {
                self.locals.pop();
                self.in_macro -= 1;
                self.todo(ind, "`.pseudocommand`", sp);
                return;
            }
            (MacroKind::Pseudo, _) => {
                self.emit(ind, &format!(".pseudocommand {} {} {{", m.name, pnames.join(" : ")), sp);
                self.block(&m.body, ind + 1);
                self.out.push_str(&format!("{}}}\n", "    ".repeat(ind)));
            }
            (MacroKind::Function, Dialect::Acme) => {
                self.locals.pop();
                self.in_macro -= 1;
                self.todo(ind, "`.function`", sp);
                return;
            }
            (MacroKind::Function, _) => {
                let kw = if to == Dialect::Kick { ".function" } else { ".fn" };
                match &m.expr_body {
                    Some(e) if to == Dialect::Raw => {
                        let x = self.expr(e);
                        self.emit(ind, &format!("{kw} {}({}) = {x}", m.name, pnames.join(", ")), sp);
                    }
                    Some(e) => {
                        let x = self.expr(e);
                        self.emit(ind, &format!("{kw} {}({}) {{ .return {x} }}", m.name, pnames.join(", ")), sp);
                    }
                    None => {
                        self.emit(ind, &format!("{kw} {}({}) {{", m.name, pnames.join(", ")), sp);
                        self.block(&m.body, ind + 1);
                        self.out.push_str(&format!("{}}}\n", "    ".repeat(ind)));
                    }
                }
            }
        }
        self.locals.pop();
        self.in_macro -= 1;
    }
}

/// Labels defined directly in a block (not in nested scopes).
fn collect_labels(stmts: &[Stmt], out: &mut HashSet<String>) {
    for s in stmts {
        match &s.kind {
            StmtKind::Label(n) if n.kind == NameKind::Plain => {
                out.insert(n.text.clone());
            }
            StmtKind::Group(b) | StmtKind::ZpBlock(b) => collect_labels(b, out),
            StmtKind::If { then, else_, .. } => {
                collect_labels(then, out);
                if let Some(e) = else_ {
                    collect_labels(e, out);
                }
            }
            _ => {}
        }
    }
}

fn children(k: &StmtKind) -> Vec<&Block> {
    match k {
        StmtKind::If { then, else_, .. } | StmtKind::IfDef { then, else_, .. } => {
            let mut v = vec![then];
            if let Some(e) = else_ {
                v.push(e);
            }
            v
        }
        StmtKind::ForC { body, .. }
        | StmtKind::ForRange { body, .. }
        | StmtKind::ForIn { body, .. }
        | StmtKind::While { body, .. }
        | StmtKind::Do { body, .. }
        | StmtKind::AssertError { body, .. }
        | StmtKind::ZpBlock(body)
        | StmtKind::Group(body) => vec![body],
        StmtKind::Scope { body, .. } => vec![body],
        StmtKind::Zone { body: Some(b), .. }
        | StmtKind::PseudoPc { body: Some(b), .. }
        | StmtKind::Platform { body: Some(b), .. }
        | StmtKind::Segment { body: Some(b), .. }
        | StmtKind::File { body: Some(b), .. }
        | StmtKind::Xor { body: Some(b), .. }
        | StmtKind::PlatformDir { body: Some(b), .. } => vec![b],
        StmtKind::MacroDef(m) => vec![&m.body],
        StmtKind::PpIf { branches, else_ } => {
            let mut v: Vec<&Block> = branches.iter().map(|b| &b.1).collect();
            if let Some(e) = else_ {
                v.push(e);
            }
            v
        }
        _ => Vec::new(),
    }
}

/// Does `e` reference the name `var`?
fn mentions(e: &Expr, var: &str) -> bool {
    match &e.kind {
        ExprKind::Ident(n) => n.text == var,
        ExprKind::Unary(_, x) | ExprKind::Field(x, _) => mentions(x, var),
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) | ExprKind::Assign(_, a, b) => mentions(a, var) || mentions(b, var),
        ExprKind::Ternary(a, b, c) => mentions(a, var) || mentions(b, var) || mentions(c, var),
        ExprKind::Call(f, args) => mentions(f, var) || args.iter().any(|x| mentions(x, var)),
        ExprKind::List(items) => items.iter().any(|x| mentions(x, var)),
        ExprKind::IncDec { target, .. } => mentions(target, var),
        _ => false,
    }
}

fn wrap(s: String, yes: bool) -> String {
    if yes {
        format!("({s})")
    } else {
        s
    }
}
