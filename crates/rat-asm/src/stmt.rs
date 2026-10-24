//! Statement execution.

use std::rc::Rc;

use rat_core::*;

use crate::engine::{Engine, Fatal, Flow, OutReq, SymKind, R};
use crate::memory::SegParams;
use crate::value::*;

const MAX_LOOP: u64 = 1_000_000;

/// Kick pseudocommand argument types.
pub const AT_ABSOLUTE: i64 = 1;
pub const AT_ABSOLUTEX: i64 = 2;
pub const AT_ABSOLUTEY: i64 = 3;
pub const AT_IMMEDIATE: i64 = 4;
pub const AT_INDIRECT: i64 = 5;
pub const AT_IZEROPAGEX: i64 = 6;
pub const AT_IZEROPAGEY: i64 = 7;
pub const AT_NONE: i64 = 8;

fn tok(t: Tok, span: Span) -> Token {
    Token { tok: t, span, space_before: false }
}

/// Tokens for a pseudocommand argument (type + value).
pub fn cmdarg_tokens(c: &CmdArg, span: Span) -> Vec<Token> {
    let v = match &c.value {
        Value::Undef => tok(Tok::Ident("\u{1}undef".into()), span),
        v => tok(Tok::Int(v.as_i64().unwrap_or(0)), span),
    };
    let p = |s: &'static str| tok(Tok::Punct(s), span);
    let id = |s: &str| tok(Tok::Ident(s.into()), span);
    match c.kind {
        AT_IMMEDIATE => vec![p("#"), v],
        AT_ABSOLUTEX => vec![v, p(","), id("x")],
        AT_ABSOLUTEY => vec![v, p(","), id("y")],
        AT_INDIRECT => vec![p("("), v, p(")")],
        AT_IZEROPAGEX => vec![p("("), v, p(","), id("x"), p(")")],
        AT_IZEROPAGEY => vec![p("("), v, p(")"), p(","), id("y")],
        AT_NONE => vec![],
        _ => vec![v],
    }
}

fn arg<'a>(params: &'a [Arg], key: &str) -> Option<&'a Expr> {
    params.iter().find(|a| a.key.as_deref().is_some_and(|k| k.eq_ignore_ascii_case(key))).map(|a| &a.value)
}

fn pos_arg(params: &[Arg], i: usize) -> Option<&Expr> {
    params.iter().filter(|a| a.key.is_none()).nth(i).map(|a| &a.value)
}

fn split_list(s: &str) -> Vec<String> {
    s.split(',').map(|x| x.trim().to_owned()).filter(|x| !x.is_empty()).collect()
}

impl Engine<'_> {
    /// Assemble a block into a throw-away segment at the current PC and
    /// return its bytes (`.assert` with code).
    pub fn assemble_scratch(&mut self, body: &[Stmt]) -> Result<Vec<u8>, Fatal> {
        let saved = self.cur_seg.clone();
        let pc = self.pc();
        let seg = "\u{1}scratch".to_string();
        self.segments.shift_remove(&seg);
        self.ensure_segment(&seg, SegParams::default());
        self.cur_seg = seg.clone();
        self.seg().org(pc, None);
        let log = self.emit_log.len();
        let r = self.exec_scoped(body, None);
        let bytes = self.emit_log[log..].to_vec();
        self.emit_log.truncate(log);
        self.cur_seg = saved;
        self.segments.shift_remove(&seg);
        r.map(|_| bytes)
    }

    fn seg_params(&mut self, params: &[Arg]) -> SegParams {
        let mut p = SegParams::default();
        for a in params {
            let Some(k) = a.key.as_deref() else { continue };
            let k = k.to_ascii_lowercase();
            let Some(v) = self.eval_or_err(&a.value) else { continue };
            let int = || v.as_i64().map(|n| n as u32);
            let s = || v.to_string();
            match k.as_str() {
                "start" => p.start = int(),
                "startafter" => p.start_after = Some(s()),
                "min" => p.min = int(),
                "max" => p.max = int(),
                "fill" => {
                    if v.truthy() == Some(true) {
                        p.fill.get_or_insert(0);
                    }
                }
                "fillbyte" => p.fill = int().map(|n| n as u8),
                "allowoverlap" => p.allow_overlap = v.truthy() == Some(true),
                "segments" => p.segments = split_list(&s()),
                "outprg" => p.out_prg = Some(s()),
                "outbin" => p.out_bin = Some(s()),
                "virtual" => p.virtual_ = v.truthy() == Some(true),
                "hide" => p.hide = v.truthy() == Some(true),
                "align" => p.align = int(),
                "prgfiles" => {
                    for f in split_list(&s()) {
                        match self.read_file(&f, a.value.span) {
                            Ok((_, d)) if d.len() >= 2 => p.extra.push(rat_output::Chunk { addr: u32::from(u16::from_le_bytes([d[0], d[1]])), data: d[2..].to_vec() }),
                            Ok(_) => self.err(format!("{f}: too short for a PRG"), a.value.span),
                            Err(d) => self.diags.push(d),
                        }
                    }
                }
                "sidfiles" => {
                    for f in split_list(&s()) {
                        match self.read_file(&f, a.value.span).map_err(|d| d.message).and_then(|(_, d)| rat_platform::cbm::parse_sid(&d)) {
                            Ok(sid) => p.extra.push(rat_output::Chunk { addr: u32::from(sid.location), data: sid.data }),
                            Err(m) => self.err(m, a.value.span),
                        }
                    }
                }
                "modify" | "_start" | "marg1" | "marg2" | "marg3" | "marg4" | "marg5" => {
                    self.warn(format!("segment modifier parameter `{k}` is not supported by rat"), a.value.span)
                }
                "memblock" | "dest" => {}
                _ => self.warn(format!("unknown segment parameter `{k}`"), a.value.span),
            }
        }
        p
    }

    /// Switch to a segment, starting it if this is its first use.
    pub fn switch_segment(&mut self, name: &str, memblock: Option<String>, span: Span) {
        if !self.segments.contains_key(name) {
            self.ensure_segment(name, SegParams::default());
        }
        self.cur_seg = name.to_owned();
        let (started, start, after) = {
            let s = &self.segments[name];
            (s.started, s.params.start, s.params.start_after.clone())
        };
        if !started {
            let pc = if let Some(a) = after {
                match self.segments.get(&a) {
                    Some(o) => o.end(),
                    None => {
                        self.err(format!("unknown segment `{a}` in startAfter"), span);
                        0
                    }
                }
            } else {
                start.unwrap_or(0)
            };
            let align = self.segments[name].params.align;
            let pc = match align {
                Some(a) if a > 0 => pc.div_ceil(a) * a,
                _ => pc,
            };
            self.seg().org(pc, memblock);
        } else if memblock.is_some() {
            let pc = self.real_pc();
            self.seg().org(pc, memblock);
        }
    }

    /// Call a macro with evaluated arguments.
    pub fn call_macro(&mut self, def: &Rc<MacroDef>, args: Vec<Value>, refs: Vec<Option<Name>>, label: Option<&Name>, span: Span) -> R {
        if self.depth > 256 {
            return Err(Fatal(Diagnostic::error(format!("macro recursion too deep in `{}`", def.name), Some(span))));
        }
        let n = def.params.len();
        if args.len() > n && !def.variadic || args.len() < def.params.iter().filter(|p| p.default.is_none()).count().min(n) && !def.variadic {
            self.err(format!("`{}` takes {n} argument(s), got {}", def.name, args.len()), span);
            return Ok(Flow::Normal);
        }
        self.depth += 1;
        let sid = self.new_scope(label.map(|l| l.text.as_str()));
        self.scope_stack.push(sid);
        let saved_zone = self.zone;
        if def.dialect == Dialect::Acme {
            self.zone_counter += 1;
            self.zone = self.zone_counter;
        }
        self.dialect_stack.push(def.dialect);
        let mut args = args;
        for (i, p) in def.params.iter().enumerate() {
            let v = if def.variadic && i == n - 1 {
                Value::list(if args.len() >= i { args.split_off(i) } else { Vec::new() })
            } else if i < args.len() {
                args[i].clone()
            } else if let Some(d) = &p.default {
                self.eval_or_err(d).unwrap_or_default()
            } else {
                Value::Undef
            };
            if let Some(local) = p.name.strip_prefix('.') {
                // ACME `.param`: local to the macro's zone
                self.define(&Name { kind: NameKind::ZoneLocal, text: local.to_owned() }, v, SymKind::Set, span);
            } else {
                self.define_in(sid, &p.name, v, SymKind::Var, span);
            }
        }
        self.hoist(&def.body);
        let r = self.exec_block(&def.body);
        // by-reference parameters write back (ACME `~name`)
        for (p, r) in def.params.iter().zip(&refs) {
            if let (true, Some(target)) = (p.by_ref, r) {
                let v = match p.name.strip_prefix('.') {
                    Some(l) => self.lookup(&Name { kind: NameKind::ZoneLocal, text: l.to_owned() }).unwrap_or_default(),
                    None => self.scopes[sid].syms.get(&p.name).map(|s| s.value.clone()).unwrap_or_default(),
                };
                self.dialect_stack.push(Dialect::Acme);
                self.scope_stack.pop();
                self.define(target, v, SymKind::Set, span);
                self.scope_stack.push(sid);
                self.dialect_stack.pop();
            }
        }
        self.dialect_stack.pop();
        self.zone = saved_zone;
        self.scope_stack.pop();
        self.depth -= 1;
        match r? {
            Flow::Eof => Ok(Flow::Normal),
            _ => Ok(Flow::Normal),
        }
    }

    /// Call a user function and return its value.
    pub fn call_function(&mut self, def: &Rc<MacroDef>, args: Vec<Value>, span: Span) -> Result<Value, Diagnostic> {
        if self.depth > 256 {
            return Err(Diagnostic::error(format!("recursion too deep in `{}`", def.name), Some(span)));
        }
        self.depth += 1;
        let sid = self.new_scope(None);
        self.scope_stack.push(sid);
        self.dialect_stack.push(def.dialect);
        for (i, p) in def.params.iter().enumerate() {
            let v = match args.get(i) {
                Some(v) => v.clone(),
                None => match &p.default {
                    Some(d) => self.eval(d)?,
                    None => Value::Undef,
                },
            };
            self.define_in(sid, &p.name, v, SymKind::Var, span);
        }
        self.in_function += 1;
        let r = if let Some(e) = &def.expr_body {
            self.eval(e).map(Flow::Return)
        } else {
            self.exec_block(&def.body).map_err(|f| f.0)
        };
        self.in_function -= 1;
        self.dialect_stack.pop();
        self.scope_stack.pop();
        self.depth -= 1;
        Ok(match r? {
            Flow::Return(v) => v,
            _ => Value::Null,
        })
    }

    /// Kick pseudocommand: operand split at `:`, each part becomes a
    /// CmdArgument.
    fn call_pseudo(&mut self, def: &Rc<MacroDef>, ins: &Instr, span: Span) -> R {
        let toks = &ins.operand;
        let mut groups: Vec<&[Token]> = Vec::new();
        if !toks.is_empty() {
            let mut depth = 0;
            let mut start = 0;
            for (i, t) in toks.iter().enumerate() {
                if t.is_punct("(") || t.is_punct("[") {
                    depth += 1;
                } else if t.is_punct(")") || t.is_punct("]") {
                    depth -= 1;
                } else if depth == 0 && t.is_punct(":") {
                    groups.push(&toks[start..i]);
                    start = i + 1;
                }
            }
            groups.push(&toks[start..]);
        }
        let mut args = Vec::new();
        for g in groups {
            args.push(self.cmd_arg(g, ins.dialect, span));
        }
        let n = def.params.len();
        while args.len() < n {
            args.push(Value::CmdArg(Rc::new(CmdArg { kind: AT_NONE, value: Value::Null })));
        }
        self.call_macro(def, args, Vec::new(), ins.label.as_ref(), span)
    }

    fn cmd_arg(&mut self, g: &[Token], d: Dialect, span: Span) -> Value {
        let n = g.len();
        let is = |i: usize, p: &str| g.get(i).is_some_and(|t| t.is_punct(p));
        let ident = |i: usize, s: &str| g.get(i).is_some_and(|t| t.is_ident(s));
        let (kind, range) = if n == 0 {
            (AT_NONE, 0..0)
        } else if is(0, "#") {
            (AT_IMMEDIATE, 1..n)
        } else if n >= 5 && is(0, "(") && is(n - 3, ")") && is(n - 2, ",") && ident(n - 1, "y") {
            (AT_IZEROPAGEY, 1..n - 3)
        } else if n >= 5 && is(0, "(") && is(n - 1, ")") && is(n - 3, ",") && ident(n - 2, "x") {
            (AT_IZEROPAGEX, 1..n - 3)
        } else if n >= 3 && is(0, "(") && is(n - 1, ")") && rat_cpu::matching(g, 0) == Some(n - 1) {
            (AT_INDIRECT, 1..n - 1)
        } else if n >= 3 && is(n - 2, ",") && ident(n - 1, "x") {
            (AT_ABSOLUTEX, 0..n - 2)
        } else if n >= 3 && is(n - 2, ",") && ident(n - 1, "y") {
            (AT_ABSOLUTEY, 0..n - 2)
        } else {
            (AT_ABSOLUTE, 0..n)
        };
        let value = if range.is_empty() {
            Value::Null
        } else {
            match rat_syntax::parse_expr_tokens(d, &g[range]) {
                Ok(e) => self.eval_or_err(&e).unwrap_or_default(),
                Err(e) => {
                    self.diags.push(e);
                    Value::Undef
                }
            }
        };
        let _ = span;
        Value::CmdArg(Rc::new(CmdArg { kind, value }))
    }

    fn run_loop_body(&mut self, body: &[Stmt], iters: &mut u64, span: Span) -> Result<Option<Flow>, Fatal> {
        *iters += 1;
        if *iters > MAX_LOOP {
            return Err(Fatal(Diagnostic::error("loop runs too long (more than 1000000 iterations)", Some(span))));
        }
        match self.exec_block(body)? {
            Flow::Normal => Ok(None),
            f => Ok(Some(f)),
        }
    }

    fn include(&mut self, path: &str, once: bool, span: Span) -> R {
        let full = match self.resolve(path) {
            Some(p) => p,
            None => {
                self.err(format!("source file not found: {path}"), span);
                return Ok(Flow::Normal);
            }
        };
        if once && self.once.contains(&full) {
            return Ok(Flow::Normal);
        }
        let l = match self.load(&full, None, None) {
            Ok(l) => l,
            Err(mut d) => {
                d.span = Some(span);
                self.diags.push(d);
                return Ok(Flow::Normal);
            }
        };
        self.exec_file(l)?;
        Ok(Flow::Normal)
    }

    fn platform_dir(&mut self, ns: Option<&str>, name: &str, args: &[Arg], body: Option<&Block>, span: Span) -> R {
        let plat = match ns {
            Some(n) => match rat_platform::by_namespace(n) {
                Some(p) => p,
                None => {
                    self.err(format!("unknown directive `.{n}.{name}`"), span);
                    return Ok(Flow::Normal);
                }
            },
            None => self.platform,
        };
        if !plat.has_directive(name) {
            // is it a directive of some other platform?
            let owners: Vec<&str> = rat_platform::PLATFORMS.iter().filter(|p| p.has_directive(name)).map(|p| p.id).collect();
            if owners.is_empty() {
                self.err(format!("unknown directive `{name}`"), span);
            } else {
                self.err(
                    format!("`{name}` is a {} directive; it needs `.platform {}` (current platform: {})", owners.join("/"), owners[0], self.platform.id),
                    span,
                );
            }
            return Ok(Flow::Normal);
        }
        crate::platform::directive(self, plat, name, args, body, span)
    }

    fn disk(&mut self, params: &[Arg], files: &[Vec<Arg>], span: Span) {
        if self.platform.basic_start.is_none() {
            self.err(format!("`.disk` needs a Commodore platform (current platform: {})", self.platform.id), span);
            return;
        }
        let get = |e: &mut Engine<'_>, ps: &[Arg], k: &str| arg(ps, k).and_then(|x| e.eval_str(x));
        let name = get(self, params, "filename").unwrap_or_else(|| "disk.d64".into());
        let disk_name = get(self, params, "name").unwrap_or_else(|| "RAT".into());
        let id = get(self, params, "id").unwrap_or_else(|| "01".into());
        let mut out = Vec::new();
        for f in files {
            let fname = get(self, f, "name").unwrap_or_default();
            let kind = get(self, f, "type").unwrap_or_else(|| "prg".into());
            let mut segs = get(self, f, "segments").map(|s| split_list(&s)).unwrap_or_default();
            // prgFiles / sidFiles: an anonymous segment holding those files
            let mut extra = Vec::new();
            for key in ["prgFiles", "sidFiles"] {
                if let Some(x) = arg(f, key) {
                    extra.push(Arg { key: Some(key.to_owned()), value: x.clone() });
                }
            }
            if !extra.is_empty() {
                let seg = format!("\u{1}disk:{fname}");
                let p = self.seg_params(&extra);
                self.segments.shift_remove(&seg);
                self.ensure_segment(&seg, p);
                segs.push(seg);
            }
            out.push((fname, kind, segs));
        }
        self.outputs.push(OutReq::Disk { name, disk_name, id, files: out, span });
    }
}

pub(crate) fn exec(e: &mut Engine<'_>, s: &Stmt) -> R {
    let span = s.span;
    match &s.kind {
        StmtKind::Label(n) => {
            let pc = i64::from(e.pc());
            e.define(n, Value::Int(pc), SymKind::Label, span);
        }
        StmtKind::AnonLabel(key) => {
            let pc = i64::from(e.pc());
            e.anon.entry(key.clone()).or_default().push(pc);
        }
        StmtKind::Assign { name, value, kind } => {
            let v = e.eval_or_err(value).unwrap_or_default();
            let k = match kind {
                AssignKind::Const => SymKind::Const,
                AssignKind::Var => SymKind::Var,
                AssignKind::Label => SymKind::Label,
                AssignKind::Set => SymKind::Set,
            };
            if v.is_undef() {
                e.unresolved = true;
            }
            e.define(name, v, k, span);
        }
        StmtKind::Eval(x) => {
            e.eval_or_err(x);
        }
        StmtKind::Instr(ins) => {
            if e.in_function > 0 {
                e.err("functions cannot emit code", span);
                return Ok(Flow::Normal);
            }
            let forced = ins.suffix.as_deref() == Some("\u{1}macro");
            if forced || !e.cpu.has_mnemonic(&ins.mnemonic) {
                // pseudocommand, or a macro called without parentheses
                match e.lookup(&Name::plain(ins.mnemonic.clone())) {
                    Some(Value::Macro(m)) if m.kind == MacroKind::Pseudo => return e.call_pseudo(&m, ins, span),
                    Some(Value::Macro(m)) if forced => return e.call_pseudo(&m, ins, span),
                    Some(Value::Macro(m)) if m.kind == MacroKind::Macro => {
                        let mut args = Vec::new();
                        for r in rat_cpu::commas(&ins.operand) {
                            match rat_syntax::parse_expr_tokens(ins.dialect, &ins.operand[r]) {
                                Ok(x) => args.push(e.eval_or_err(&x).unwrap_or_default()),
                                Err(d) => e.diags.push(d),
                            }
                        }
                        return e.call_macro(&m, args, Vec::new(), None, span);
                    }
                    _ => {
                        let other: Vec<&str> = rat_cpu::all().into_iter().filter(|c| c.has_mnemonic(&ins.mnemonic)).map(|c| c.info().id).collect();
                        if other.is_empty() {
                            e.err(format!("unknown instruction or macro `{}`", ins.mnemonic), span);
                        } else {
                            e.err(format!("`{}` is not a {} instruction (it is one on {})", ins.mnemonic, e.cpu.info().id, other.join(", ")), span);
                        }
                        return Ok(Flow::Normal);
                    }
                }
            }
            return e.assemble_instr(s, ins);
        }
        StmtKind::Data { size, endian, items } => {
            let en = e.data_endian(*endian);
            for it in items {
                let v = e.eval_or_err(it).unwrap_or_default();
                e.emit_value(&v, *size, en, it.span);
            }
        }
        StmtKind::Text { conv, xor, items } => {
            let saved = e.encoding.clone();
            match conv {
                TextConv::Current => {}
                TextConv::Raw => e.encoding = Some("ascii".into()),
                TextConv::Petscii => e.encoding = Some("petscii_mixed".into()),
                TextConv::Screen => e.encoding = Some("screencode_mixed".into()),
                TextConv::Named(n) => e.encoding = crate::encoding::canonical(n).map(String::from),
            }
            let saved_tab = if matches!(conv, TextConv::Current) { None } else { e.convtab.take() };
            let x = match xor {
                Some(x) => e.eval_int(x).unwrap_or(0) as u8,
                None => 0,
            };
            let old_xor = e.xor;
            e.xor ^= x;
            for it in items {
                let v = e.eval_or_err(it).unwrap_or_default();
                match v {
                    Value::Str(_) | Value::List(_) | Value::Undef => e.emit_value(&v, 1, Endian::Little, it.span),
                    v => {
                        // numbers are emitted as is
                        e.emit_value(&v, 1, Endian::Little, it.span)
                    }
                }
            }
            e.xor = old_xor;
            if saved_tab.is_some() {
                e.convtab = saved_tab;
            }
            e.encoding = saved;
        }
        StmtKind::Hex(b) => e.emit(b),
        StmtKind::Fill { count, value, size, index_var } => {
            let Some(n) = e.eval_int(count) else { return Ok(Flow::Normal) };
            if !(0..=0x100_0000).contains(&n) {
                e.err(format!("bad fill count {n}"), span);
                return Ok(Flow::Normal);
            }
            let en = e.data_endian(None);
            match (value, index_var) {
                (None, _) => {
                    let b = e.initmem;
                    e.emit(&vec![b; n as usize * *size as usize]);
                }
                (Some(v), None) => {
                    let val = e.eval_or_err(v).unwrap_or_default();
                    for _ in 0..n {
                        e.emit_value(&val, *size, en, v.span);
                    }
                }
                (Some(v), Some(var)) => {
                    let sid = e.new_scope(None);
                    e.scope_stack.push(sid);
                    for i in 0..n {
                        e.define_in(sid, var, Value::Int(i), SymKind::Var, span);
                        let val = e.eval_or_err(v).unwrap_or_default();
                        e.emit_value(&val, *size, en, v.span);
                    }
                    e.scope_stack.pop();
                }
            }
        }
        StmtKind::LoHiFill { name, count, value, index_var } => {
            let Some(n) = e.eval_int(count) else { return Ok(Flow::Normal) };
            let mut vals = Vec::new();
            let sid = e.new_scope(None);
            e.scope_stack.push(sid);
            for i in 0..n {
                e.define_in(sid, index_var, Value::Int(i), SymKind::Var, span);
                vals.push(e.eval_or_err(value).and_then(|v| v.as_i64()).unwrap_or(0));
            }
            e.scope_stack.pop();
            let lo = i64::from(e.pc());
            let lo_bytes: Vec<u8> = vals.iter().map(|v| *v as u8).collect();
            let hi_bytes: Vec<u8> = vals.iter().map(|v| (*v >> 8) as u8).collect();
            if let Some(nm) = name {
                let scope = e.new_scope(Some(&nm.text));
                e.define_in(scope, "lo", Value::Int(lo), SymKind::Label, span);
                e.define_in(scope, "hi", Value::Int(lo + n), SymKind::Label, span);
            }
            e.emit(&lo_bytes);
            e.emit(&hi_bytes);
        }
        StmtKind::Skip(n) => {
            if let Some(n) = e.eval_int(n) {
                e.seg().skip(n as u32);
            }
        }
        StmtKind::Align { kind, fill } => {
            let f = match fill {
                Some(f) => e.eval_int(f).unwrap_or(0) as u8,
                None if e.dialect() == Dialect::Acme && e.cpu.endian() == Endian::Little && !e.cpu.has_mnemonic("ld") => 0xea,
                None => 0,
            };
            let pc = i64::from(e.pc());
            let pad = match kind {
                AlignKind::Multiple(m) => {
                    let Some(m) = e.eval_int(m) else { return Ok(Flow::Normal) };
                    if m <= 0 {
                        e.err("alignment must be positive", span);
                        return Ok(Flow::Normal);
                    }
                    (m - pc.rem_euclid(m)) % m
                }
                AlignKind::Mask { and, equal } => {
                    let (Some(a), Some(q)) = (e.eval_int(and), e.eval_int(equal)) else { return Ok(Flow::Normal) };
                    let mut k = 0;
                    while (pc + k) & a != q {
                        k += 1;
                        if k > 0x10000 {
                            e.err("alignment can never be satisfied", span);
                            return Ok(Flow::Normal);
                        }
                    }
                    k
                }
            };
            e.emit(&vec![f; pad as usize]);
        }
        StmtKind::Org { addr, name, overlay, invisible } => {
            let Some(a) = e.eval_int(addr) else {
                e.seg().org(0, None);
                return Ok(Flow::Normal);
            };
            let nm = name.as_ref().and_then(|n| e.eval_str(n));
            if *invisible {
                e.ensure_segment("\u{1}invisible", SegParams { virtual_: true, ..Default::default() });
                e.cur_seg = "\u{1}invisible".into();
            } else if e.cur_seg == "\u{1}invisible" {
                e.cur_seg = "Default".into();
            }
            if *overlay {
                e.seg().params.allow_overlap = true;
            }
            e.pc_offsets.clear();
            e.seg().org(a as u32, nm);
        }
        StmtKind::PseudoPc { addr, body } => {
            let a = e.eval_int(addr).unwrap_or(0);
            let off = a - i64::from(e.real_pc());
            e.pc_offsets.push(off);
            if let Some(b) = body {
                let r = e.exec_block(b);
                e.pc_offsets.pop();
                return r;
            }
        }
        StmtKind::RealPc => {
            if e.pc_offsets.pop().is_none() {
                e.warn("`realpc` without `pseudopc`", span);
            }
        }
        StmtKind::Include { path, once } => {
            if let Some(p) = e.eval_str(path) {
                return e.include(&p, *once, span);
            }
        }
        StmtKind::ImportOnce => {
            if let Some(f) = e.file_stack.last() {
                let p = f.path.clone();
                if !e.once.insert(p) {
                    return Ok(Flow::Eof);
                }
            }
        }
        StmtKind::Binary { path, size, offset, kind } => {
            let Some(p) = e.eval_str(path) else { return Ok(Flow::Normal) };
            let data = match e.read_file(&p, span) {
                Ok((_, d)) => d,
                Err(d) => {
                    e.diags.push(d);
                    return Ok(Flow::Normal);
                }
            };
            let mut off = offset.as_ref().and_then(|o| e.eval_int(o)).unwrap_or(0).max(0) as usize;
            if *kind == BinaryKind::C64 {
                off += 2;
            }
            let end = match size {
                Some(sz) => off + e.eval_int(sz).unwrap_or(0).max(0) as usize,
                None => data.len(),
            };
            if off > data.len() || end > data.len() {
                e.err(format!("{p}: offset/size beyond the file ({} bytes)", data.len()), span);
                return Ok(Flow::Normal);
            }
            let bytes = data[off..end].to_vec();
            if *kind == BinaryKind::Text {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                let v = Value::str(text);
                e.emit_value(&v, 1, Endian::Little, span);
            } else {
                e.emit(&bytes);
            }
        }
        StmtKind::If { cond, then, else_ } => match e.eval_cond(cond) {
            Some(true) => return e.exec_block(then),
            Some(false) => {
                if let Some(b) = else_ {
                    return e.exec_block(b);
                }
            }
            None => {}
        },
        StmtKind::IfDef { name, negate, then, else_ } => {
            let known = e.lookup(name).is_some_and(|v| !v.is_undef());
            if known != *negate {
                return e.exec_block(then);
            } else if let Some(b) = else_ {
                return e.exec_block(b);
            }
        }
        StmtKind::ForC { init, cond, step, body } => {
            let sid = e.new_scope(None);
            e.scope_stack.push(sid);
            let r = (|| -> R {
                e.exec_block(init)?;
                let mut iters = 0;
                loop {
                    if let Some(c) = cond {
                        match e.eval_cond(c) {
                            Some(true) => {}
                            _ => break,
                        }
                    }
                    let isid = e.new_scope(None);
                    e.scope_stack.push(isid);
                    let f = e.run_loop_body(body, &mut iters, span);
                    e.scope_stack.pop();
                    if let Some(f) = f? {
                        return Ok(f);
                    }
                    for x in step {
                        e.eval_or_err(x);
                    }
                }
                Ok(Flow::Normal)
            })();
            e.scope_stack.pop();
            return r;
        }
        StmtKind::ForRange { var, from, to, step, inclusive, body } => {
            let (Some(a), Some(b)) = (e.eval_int(from), e.eval_int(to)) else { return Ok(Flow::Normal) };
            let st = match step {
                Some(x) => e.eval_int(x).unwrap_or(1),
                None => {
                    if b < a && *inclusive {
                        -1
                    } else {
                        1
                    }
                }
            };
            if st == 0 {
                e.err("loop step is 0", span);
                return Ok(Flow::Normal);
            }
            let mut i = a;
            let mut iters = 0;
            let acme = e.dialect() == Dialect::Acme;
            loop {
                let more = if st > 0 {
                    if *inclusive { i <= b } else { i < b }
                } else if *inclusive {
                    i >= b
                } else {
                    i > b
                };
                if !more {
                    break;
                }
                if acme {
                    // ACME loop variables are ordinary (global) symbols
                    e.define(&Name::plain(var.clone()), Value::Int(i), SymKind::Set, span);
                    if let Some(f) = e.run_loop_body(body, &mut iters, span)? {
                        return Ok(f);
                    }
                } else {
                    let sid = e.new_scope(None);
                    e.define_in(sid, var, Value::Int(i), SymKind::Var, span);
                    e.scope_stack.push(sid);
                    let f = e.run_loop_body(body, &mut iters, span);
                    e.scope_stack.pop();
                    if let Some(f) = f? {
                        return Ok(f);
                    }
                }
                i += st;
            }
        }
        StmtKind::ForIn { var, list, body } => {
            let Some(v) = e.eval_or_err(list) else { return Ok(Flow::Normal) };
            let items: Vec<Value> = match &v {
                Value::List(l) => l.borrow().clone(),
                Value::Str(s) => s.chars().map(|c| Value::str(c.to_string())).collect(),
                Value::Hash(h) => h.borrow().keys().map(|k| match k {
                    HKey::Str(s) => Value::str(s.as_str()),
                }).collect(),
                Value::Undef => {
                    e.unresolved = true;
                    return Ok(Flow::Normal);
                }
                o => {
                    e.err(format!("cannot iterate over a {}", o.type_name()), span);
                    return Ok(Flow::Normal);
                }
            };
            let mut iters = 0;
            for it in items {
                let sid = e.new_scope(None);
                e.define_in(sid, var, it, SymKind::Var, span);
                e.scope_stack.push(sid);
                let f = e.run_loop_body(body, &mut iters, span);
                e.scope_stack.pop();
                if let Some(f) = f? {
                    return Ok(f);
                }
            }
        }
        StmtKind::While { cond, body } => {
            let mut iters = 0;
            while e.eval_cond(cond) == Some(true) {
                if let Some(f) = e.run_loop_body(body, &mut iters, span)? {
                    return Ok(f);
                }
            }
        }
        StmtKind::Do { pre, body, post } => {
            let mut iters = 0;
            let test = |e: &mut Engine<'_>, c: &Option<(bool, Expr)>| -> bool {
                match c {
                    None => true,
                    Some((until, x)) => match e.eval_cond(x) {
                        Some(b) => b != *until,
                        None => false,
                    },
                }
            };
            loop {
                if !test(e, pre) {
                    break;
                }
                if let Some(f) = e.run_loop_body(body, &mut iters, span)? {
                    return Ok(f);
                }
                if post.is_none() && pre.is_none() {
                    break;
                }
                if !test(e, post) {
                    break;
                }
            }
        }
        StmtKind::Scope { name, body } => {
            if let Some(n) = name {
                let pc = i64::from(e.pc());
                e.define(n, Value::Int(pc), SymKind::Label, span);
            }
            return e.exec_scoped(body, name.as_ref().map(|n| n.text.as_str()));
        }
        StmtKind::Zone { name: _, body } => {
            e.zone_counter += 1;
            match body {
                Some(b) => {
                    let saved = e.zone;
                    e.zone = e.zone_counter;
                    let r = e.exec_block(b);
                    e.zone = saved;
                    return r;
                }
                None => e.zone = e.zone_counter,
            }
        }
        StmtKind::Namespace { name, body, file: _ } => {
            let existing = e.scopes[e.cur_scope()].named.get(name).copied();
            let sid = match existing {
                Some(s) => s,
                None => e.new_scope(Some(name)),
            };
            e.scope_stack.push(sid);
            match body {
                Some(b) => {
                    e.hoist(b);
                    let r = e.exec_block(b);
                    e.scope_stack.pop();
                    return r;
                }
                None => {
                    // rest of the file: the scope stays until the file ends
                    let depth = e.scope_stack.len();
                    let _ = depth;
                }
            }
        }
        StmtKind::MacroDef(m) => match m.name.strip_prefix('@') {
            Some(root) => e.define_in(crate::engine::ROOT, root, Value::Macro(m.clone()), SymKind::Def, span),
            None => {
                let n = Name::plain(m.name.clone());
                e.define(&n, Value::Macro(m.clone()), SymKind::Def, span);
            }
        },
        StmtKind::Call { name, args, label } => {
            let callee = match e.resolve_callee(name) {
                Ok(c) => c,
                Err(d) => {
                    e.diags.push(d);
                    return Ok(Flow::Normal);
                }
            };
            match callee {
                Value::Macro(m) if m.kind != MacroKind::Function => {
                    let mut vals = Vec::new();
                    let mut refs = Vec::new();
                    for (i, a) in args.iter().enumerate() {
                        let by_ref = m.params.get(i).is_some_and(|p| p.by_ref);
                        refs.push(if by_ref {
                            match &a.kind {
                                ExprKind::Ident(n) => Some(n.clone()),
                                _ => None,
                            }
                        } else {
                            None
                        });
                        vals.push(e.eval_or_err(a).unwrap_or_default());
                    }
                    return e.call_macro(&m, vals, refs, label.as_ref(), span);
                }
                Value::Undef => {
                    e.unresolved = true;
                    e.undefined.push((expr_name(name), span));
                }
                _ => {
                    // function or builtin called as a statement
                    let call = Expr::new(ExprKind::Call(Box::new(name.clone()), args.clone()), span);
                    e.eval_or_err(&call);
                }
            }
        }
        StmtKind::PseudoCall { .. } => {}
        StmtKind::Return(v) => {
            let val = match v {
                Some(x) => e.eval_or_err(x).unwrap_or_default(),
                None => Value::Null,
            };
            return Ok(Flow::Return(val));
        }
        StmtKind::Print { expr, .. } => {
            if let Some(v) = e.eval_or_err(expr) {
                e.prints.push(v.to_string());
            }
        }
        StmtKind::Message { level, args } => {
            let mut text = String::new();
            for a in args {
                if let Some(v) = e.eval_or_err(a) {
                    text.push_str(&v.to_string());
                }
            }
            match level {
                MsgLevel::Warn => e.warn(text, span),
                MsgLevel::Error => e.err(text, span),
                MsgLevel::Serious => return Err(Fatal(Diagnostic::error(text, Some(span)))),
            }
        }
        StmtKind::ErrorIf { cond, msg } => {
            if e.eval_cond(cond) == Some(true) {
                let m = e.eval_str(msg).unwrap_or_default();
                e.err(m, span);
            }
        }
        StmtKind::Assert { desc, actual, expected } => {
            let a = e.eval_or_err(actual).unwrap_or_default();
            let d = desc.as_ref().and_then(|x| e.eval_str(x)).unwrap_or_else(|| "assertion".into());
            if a.is_undef() {
                e.unresolved = true;
            } else {
                let ok = match expected {
                    Some(x) => {
                        let b = e.eval_or_err(x).unwrap_or_default();
                        b.is_undef() || a == b
                    }
                    None => a.truthy() == Some(true),
                };
                if !ok {
                    e.err(format!("assert failed: {d} (got {a})"), span);
                }
            }
        }
        StmtKind::AssertCode { desc, actual, expected } => {
            let a = e.assemble_scratch(actual)?;
            let b = e.assemble_scratch(expected)?;
            if a != b {
                let d = e.eval_str(desc).unwrap_or_default();
                let hx = |v: &[u8]| v.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ");
                e.err(format!("assert failed: {d} (got {}, expected {})", hx(&a), hx(&b)), span);
            }
        }
        StmtKind::AssertError { desc, body } => {
            let before = e.diags.len();
            let r = e.exec_block(body);
            let had = e.diags[before..].iter().any(Diagnostic::is_error) || r.is_err();
            e.diags.truncate(before);
            if !had {
                let d = e.eval_str(desc).unwrap_or_default();
                e.err(format!("assert failed: {d} (expected an error)"), span);
            }
        }
        StmtKind::Cpu(name) => {
            if let Some(rest) = name.strip_prefix('+') {
                e.cpu_stack.push(e.cpu);
                set_cpu(e, rest, span);
            } else if name == "-" {
                if let Some(c) = e.cpu_stack.pop() {
                    e.cpu = c;
                }
            } else {
                set_cpu(e, name, span);
            }
        }
        StmtKind::Platform { name, body } => {
            let Some(p) = rat_platform::by_name(name) else {
                e.err(format!("unknown platform `{name}`"), span);
                return Ok(Flow::Normal);
            };
            match body {
                Some(b) => {
                    let (sp, sc) = (e.platform, e.cpu);
                    e.set_platform(p, span)?;
                    let r = e.exec_block(b);
                    e.platform = sp;
                    e.cpu = sc;
                    return r;
                }
                None => {
                    e.platform_explicit = true;
                    e.set_platform(p, span)?;
                }
            }
        }
        StmtKind::PlatformDir { ns, name, args, body } => {
            return e.platform_dir(ns.as_deref(), name, args, body.as_ref(), span);
        }
        StmtKind::SegmentDef { name, params } => {
            let p = e.seg_params(params);
            if e.segments.contains_key(name) && e.segments[name].started {
                e.err(format!("segment `{name}` is already defined"), span);
            } else {
                e.segments.shift_remove(name);
                e.ensure_segment(name, p);
            }
        }
        StmtKind::Segment { name, params, body } => {
            let memblock = arg(params, "memblock").and_then(|x| e.eval_str(x));
            let others: Vec<Arg> = params.iter().filter(|a| a.key.as_deref() != Some("memblock")).cloned().collect();
            if !others.is_empty() && !e.segments.contains_key(name) {
                let p = e.seg_params(&others);
                e.ensure_segment(name, p);
            }
            match body {
                Some(b) => {
                    let saved = e.cur_seg.clone();
                    let saved_off = std::mem::take(&mut e.pc_offsets);
                    e.switch_segment(name, memblock, span);
                    let r = e.exec_block(b);
                    e.cur_seg = saved;
                    e.pc_offsets = saved_off;
                    return r;
                }
                None => {
                    e.pc_offsets.clear();
                    e.switch_segment(name, memblock, span);
                }
            }
        }
        StmtKind::SegmentOut { params } => {
            let segs = arg(params, "segments").and_then(|x| e.eval_str(x)).map(|s| split_list(&s)).unwrap_or_default();
            let mut chunks = Vec::new();
            for sname in &segs {
                match e.segments.get(sname) {
                    Some(sg) => chunks.extend(sg.chunks()),
                    None => {
                        e.unresolved = true;
                    }
                }
            }
            let data = rat_output::bin(&chunks, 0);
            e.emit(&data);
        }
        StmtKind::File { params, body } => {
            let base = {
                let m = &e.opts.main;
                let f = m.rsplit(['/', '\\']).next().unwrap_or(m);
                f.rsplit_once('.').map_or(f, |x| x.0).to_owned()
            };
            let name = arg(params, "name").or_else(|| pos_arg(params, 0)).and_then(|x| e.eval_str(x)).map(|n| n.replace("%o", &base));
            let segs = arg(params, "segments").and_then(|x| e.eval_str(x)).map(|s| split_list(&s));
            let format = arg(params, "format").or_else(|| arg(params, "type")).and_then(|x| e.eval_str(x));
            match (name, body) {
                (Some(n), Some(b)) => {
                    let seg = format!("\u{1}file:{n}");
                    e.ensure_segment(&seg, SegParams::default());
                    let saved = e.cur_seg.clone();
                    e.switch_segment(&seg, None, span);
                    let r = e.exec_block(b);
                    e.cur_seg = saved;
                    e.outputs.push(OutReq::File { name: n, segments: vec![seg], format, span });
                    return r;
                }
                (Some(n), None) => e.outputs.push(OutReq::File { name: n, segments: segs.unwrap_or_else(|| vec!["Default".into()]), format, span }),
                _ => e.err("`.file` needs a name", span),
            }
        }
        StmtKind::Disk { params, files } => e.disk(params, files, span),
        StmtKind::MemBlock(n) => {
            if let Some(n) = e.eval_str(n) {
                let pc = e.real_pc();
                e.seg().org(pc, Some(n));
            }
        }
        StmtKind::Output { file, format } => {
            if let Some(f) = e.eval_str(file) {
                e.main_output = Some((f, format.clone()));
            }
        }
        StmtKind::Encoding(x) => {
            let Some(n) = e.eval_str(x) else { return Ok(Flow::Normal) };
            if n == "\0pop" {
                e.encoding = e.enc_stack.pop().flatten();
                return Ok(Flow::Normal);
            }
            if e.dialect() == Dialect::Acme && matches!(&s.kind, StmtKind::Encoding(_)) {
                e.enc_stack.push(e.encoding.clone());
            }
            match crate::encoding::canonical(&n) {
                Some(c) => {
                    e.encoding = Some(c.into());
                    e.convtab = None;
                }
                None => e.err(format!("unknown encoding `{n}`"), span),
            }
        }
        StmtKind::ConvTabFile(x) => {
            let Some(p) = e.eval_str(x) else { return Ok(Flow::Normal) };
            match e.read_file(&p, span) {
                Ok((_, d)) if d.len() >= 256 => {
                    let mut t = [0u8; 256];
                    t.copy_from_slice(&d[..256]);
                    e.convtab = Some(Box::new(t));
                }
                Ok(_) => e.err("conversion table files need 256 bytes", span),
                Err(d) => e.diags.push(d),
            }
        }
        StmtKind::Xor { value, body } => {
            let x = e.eval_int(value).unwrap_or(0) as u8;
            match body {
                Some(b) => {
                    let old = e.xor;
                    e.xor ^= x;
                    let r = e.exec_block(b);
                    e.xor = old;
                    return r;
                }
                None => e.xor = x,
            }
        }
        StmtKind::InitMem(v) => e.initmem = e.eval_int(v).unwrap_or(0) as u8,
        StmtKind::CpuFlag(f) => match f.as_str() {
            "a8" => e.cpu_state.m16 = false,
            "a16" => e.cpu_state.m16 = true,
            "i8" => e.cpu_state.x16 = false,
            "i16" => e.cpu_state.x16 = true,
            "push" => e.cpu_state_stack.push(e.cpu_state.clone()),
            "pop" => {
                if let Some(s) = e.cpu_state_stack.pop() {
                    e.cpu_state = s;
                }
            }
            "smart" => {}
            _ => e.err(format!("unknown CPU flag `{f}`"), span),
        },
        StmtKind::SetDp(v) => {
            let d = e.eval_int(v);
            e.cpu_state.dp = d.map(|d| d as u32);
        }
        StmtKind::Endian(x) => e.endian = *x,
        StmtKind::Struct { name, fields } => {
            let def = Rc::new(StructDef { name: name.clone(), fields: fields.clone() });
            e.define(&Name::plain(name.clone()), Value::StructDef(def), SymKind::Def, span);
        }
        StmtKind::Enum { items } => {
            let mut next = 0i64;
            for (n, v) in items {
                if let Some(x) = v {
                    next = e.eval_int(x).unwrap_or(next);
                }
                e.define(&Name::plain(n.clone()), Value::Int(next), SymKind::Const, span);
                next += 1;
            }
        }
        StmtKind::PpDefine(n) => {
            e.pp_defines.insert(n.clone());
        }
        StmtKind::PpUndef(n) => {
            e.pp_defines.remove(n);
        }
        StmtKind::PpIf { branches, else_ } => {
            for (c, b) in branches {
                if e.eval_pp(c) {
                    return e.exec_block(b);
                }
            }
            if let Some(b) = else_ {
                return e.exec_block(b);
            }
        }
        StmtKind::Break(_) => {
            let pc = e.pc();
            e.breaks.push(pc);
        }
        StmtKind::Watch { args } => {
            let mut parts = Vec::new();
            for a in args {
                if let Some(v) = e.eval_or_err(a) {
                    parts.push(match v.as_i64() {
                        Some(n) => format!("{n:04x}"),
                        None => v.to_string(),
                    });
                }
            }
            e.watches.push(parts.join(" "));
        }
        StmtKind::SymbolList(f) => {
            if let Some(f) = e.eval_str(f) {
                e.symbol_lists.push(f);
            }
        }
        StmtKind::Eof => return Ok(Flow::Eof),
        StmtKind::ZpBlock(b) => return e.exec_block(b),
        StmtKind::Group(b) => return e.exec_block(b),
        StmtKind::Syntax(_) | StmtKind::Nop => {}
    }
    Ok(Flow::Normal)
}

fn expr_name(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Ident(n) => n.text.clone(),
        ExprKind::Field(b, f) => format!("{}.{f}", expr_name(b)),
        _ => "?".into(),
    }
}

fn set_cpu(e: &mut Engine<'_>, name: &str, span: Span) {
    // Kick names: _6502, _6502NoIllegals, _65c02, dtv
    match rat_cpu::by_name(name) {
        Some(c) => e.cpu = c,
        None => e.err(format!("unknown CPU `{name}` (supported: {})", rat_cpu::CPU_IDS.join(", ")), span),
    }
}
