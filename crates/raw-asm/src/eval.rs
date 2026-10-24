//! Expression evaluation, built-in functions and methods.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use raw_core::*;

use crate::engine::{Engine, SymKind};
use crate::stmt::*;
use crate::value::*;

type E = Result<Value, Diagnostic>;

fn err(msg: impl Into<String>, span: Span) -> Diagnostic {
    Diagnostic::error(msg, Some(span))
}

fn num(v: &Value, span: Span) -> Result<f64, Diagnostic> {
    v.as_f64().ok_or_else(|| err(format!("expected a number, found {}", v.type_name()), span))
}

fn int(v: &Value, span: Span) -> Result<i64, Diagnostic> {
    v.as_i64().ok_or_else(|| err(format!("expected a number, found {}", v.type_name()), span))
}

fn mat_mul(a: &[[f64; 4]; 4], b: &[[f64; 4]; 4]) -> [[f64; 4]; 4] {
    let mut m = [[0.0; 4]; 4];
    for (r, row) in m.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            *cell = (0..4).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    m
}

fn identity() -> [[f64; 4]; 4] {
    let mut m = [[0.0; 4]; 4];
    for (i, row) in m.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    m
}

fn radix_string(v: i64, radix: u32, min: usize) -> String {
    let s = match radix {
        2 => format!("{v:b}"),
        8 => format!("{v:o}"),
        16 => format!("{v:x}"),
        _ => v.to_string(),
    };
    format!("{s:0>min$}")
}

/// KickAssembler LoadBinary templates.
const BF_C64FILE: &str = "C64FILE";
const BF_BITMAP_SINGLECOLOR: &str = "C64FILE, ScreenRam=$0000, Bitmap=$0400";
const BF_KOALA: &str = "C64FILE, Bitmap=$0000, ScreenRam=$1f40, ColorRam=$2328, BackgroundColor=$2710";
const BF_FLI: &str = "C64FILE, ColorRam=$0000, ScreenRam=$0400, Bitmap=$2400";
const BF_DOODLE: &str = "C64FILE, ColorRam=$0000, Bitmap=$0400";

impl Engine<'_> {
    pub fn eval(&mut self, e: &Expr) -> E {
        let span = e.span;
        Ok(match &e.kind {
            ExprKind::Int(v) => Value::Int(*v),
            ExprKind::Float(v) => Value::Float(*v),
            ExprKind::Str(s) => Value::str(s.as_str()),
            ExprKind::Char(c) => Value::Int(self.encode_char(*c)),
            ExprKind::Bool(b) => Value::Bool(*b),
            ExprKind::Null => Value::Null,
            ExprKind::Pc => Value::Int(i64::from(self.pc())),
            ExprKind::Ident(n) => self.ident(n, span),
            ExprKind::Defined(n) => Value::Bool(self.lookup(n).is_some_and(|v| !v.is_undef())),
            ExprKind::Anon { key, offset } => self.anon_ref(key, *offset, span)?,
            ExprKind::Unary(op, x) => {
                let v = self.eval(x)?;
                self.unary(*op, v, span)?
            }
            ExprKind::Binary(BinOp::LAnd, a, b) => {
                let va = self.eval(a)?;
                match va.truthy() {
                    None => Value::Undef,
                    Some(false) => Value::Bool(false),
                    Some(true) => match self.eval(b)?.truthy() {
                        None => Value::Undef,
                        Some(x) => Value::Bool(x),
                    },
                }
            }
            ExprKind::Binary(BinOp::LOr, a, b) => {
                let va = self.eval(a)?;
                match va.truthy() {
                    None => Value::Undef,
                    Some(true) => Value::Bool(true),
                    Some(false) => match self.eval(b)?.truthy() {
                        None => Value::Undef,
                        Some(x) => Value::Bool(x),
                    },
                }
            }
            ExprKind::Binary(op, a, b) => {
                let va = self.eval(a)?;
                let vb = self.eval(b)?;
                // "World" + '!' appends the character
                if *op == BinOp::Add {
                    match (&a.kind, &b.kind, &va, &vb) {
                        (_, ExprKind::Char(c), Value::Str(s), _) => return Ok(Value::str(format!("{s}{c}"))),
                        (ExprKind::Char(c), _, _, Value::Str(s)) => return Ok(Value::str(format!("{c}{s}"))),
                        _ => {}
                    }
                }
                binop(*op, &va, &vb, span)?
            }
            ExprKind::Ternary(c, a, b) => match self.eval(c)?.truthy() {
                None => Value::Undef,
                Some(true) => self.eval(a)?,
                Some(false) => self.eval(b)?,
            },
            ExprKind::List(items) => {
                let mut v = Vec::new();
                for x in items {
                    v.push(self.eval(x)?);
                }
                Value::list(v)
            }
            ExprKind::Index(base, idx) => {
                let b = self.eval(base)?;
                let i = self.eval(idx)?;
                index(&b, &i, span)?
            }
            ExprKind::Field(base, name) => self.field(base, name, span)?,
            ExprKind::Call(callee, args) => self.call(callee, args, span)?,
            ExprKind::Assign(op, target, value) => {
                let v = self.eval(value)?;
                let new = if *op == AssignOp::Set {
                    v
                } else {
                    let old = self.eval(target)?;
                    let bop = match op {
                        AssignOp::Add => BinOp::Add,
                        AssignOp::Sub => BinOp::Sub,
                        AssignOp::Mul => BinOp::Mul,
                        _ => BinOp::FDiv,
                    };
                    binop(bop, &old, &v, span)?
                };
                self.store(target, new.clone(), span)?;
                new
            }
            ExprKind::IncDec { target, inc, post } => {
                let old = self.eval(target)?;
                let new = binop(if *inc { BinOp::Add } else { BinOp::Sub }, &old, &Value::Int(1), span)?;
                self.store(target, new.clone(), span)?;
                if *post {
                    old
                } else {
                    new
                }
            }
        })
    }

    fn ident(&mut self, n: &Name, span: Span) -> Value {
        if let Some(v) = self.lookup(n) {
            if v.is_undef() {
                self.unresolved = true;
            }
            return v;
        }
        if n.kind == NameKind::Plain {
            if let Some(v) = constant(&n.text) {
                return v;
            }
            if let Some(v) = self.platform_constant(&n.text) {
                return v;
            }
            if n.text == "\u{1}undef" {
                self.unresolved = true;
                return Value::Undef;
            }
            if let Some(sid) = self.find_named_scope(&n.text) {
                return Value::Scope(sid);
            }
            if self.zp_names.contains(&n.text) {
                self.zp_hint = true;
            }
        }
        self.unresolved = true;
        self.undefined.push((n.text.clone(), span));
        Value::Undef
    }

    fn anon_ref(&mut self, key: &str, offset: i32, span: Span) -> E {
        let cur = self.anon.get(key).map_or(0, Vec::len);
        if offset < 0 {
            let back = (-offset) as usize;
            if back <= cur {
                return Ok(Value::Int(self.anon[key][cur - back]));
            }
            return Err(err(format!("no previous anonymous label `{key}`"), span));
        }
        let idx = cur + offset as usize - 1;
        match self.prev_anon.get(key).and_then(|v| v.get(idx)) {
            Some(&v) => {
                self.forward_used = true;
                Ok(Value::Int(v))
            }
            None => {
                self.unresolved = true;
                self.undefined.push((format!("anonymous label `{key}` ahead"), span));
                Ok(Value::Undef)
            }
        }
    }

    fn unary(&mut self, op: UnOp, v: Value, span: Span) -> E {
        if v.is_undef() {
            return Ok(Value::Undef);
        }
        Ok(match op {
            UnOp::Neg => match v {
                Value::Int(n) => Value::Int(-n),
                Value::Vector(a) => Value::Vector([-a[0], -a[1], -a[2]]),
                v => Value::num(-num(&v, span)?),
            },
            UnOp::Pos => v,
            UnOp::Not => Value::Bool(!v.truthy().unwrap_or(false)),
            UnOp::BitNot => Value::Int(!int(&v, span)?),
            UnOp::Lo => Value::Int(int(&v, span)? & 0xff),
            UnOp::Hi => Value::Int((int(&v, span)? >> 8) & 0xff),
            UnOp::Bank => Value::Int((int(&v, span)? >> 16) & 0xff),
        })
    }

    /// Store into a variable, struct field or list element.
    fn store(&mut self, target: &Expr, v: Value, span: Span) -> Result<(), Diagnostic> {
        match &target.kind {
            ExprKind::Ident(n) => self.assign_var(n, v, span),
            ExprKind::Field(base, f) => match self.eval(base)? {
                Value::Struct(s) => {
                    let i = s.def.fields.iter().position(|x| x == f).ok_or_else(|| err(format!("struct {} has no field `{f}`", s.def.name), span))?;
                    s.values.borrow_mut()[i] = v;
                    Ok(())
                }
                o => Err(err(format!("cannot assign a field of a {}", o.type_name()), span)),
            },
            ExprKind::Index(base, idx) => {
                let b = self.eval(base)?;
                let i = self.eval(idx)?;
                match b {
                    Value::List(l) => {
                        let i = int(&i, span)?;
                        let mut l = l.borrow_mut();
                        let n = l.len();
                        let slot = l.get_mut(i as usize).ok_or_else(|| err(format!("index {i} out of range (size {n})"), span))?;
                        *slot = v;
                        Ok(())
                    }
                    Value::Hash(h) => {
                        h.borrow_mut().insert(i.hkey(), v);
                        Ok(())
                    }
                    o => Err(err(format!("cannot index a {}", o.type_name()), span)),
                }
            }
            _ => Err(err("cannot assign to this expression", span)),
        }
    }

    fn field(&mut self, base: &Expr, name: &str, span: Span) -> E {
        // scope path: label.local, namespace.symbol
        if let Some(b) = base.as_ident() {
            if self.lookup(&Name::plain(b)).is_none() || matches!(self.lookup(&Name::plain(b)), Some(Value::Int(_))) {
                if let Some(sid) = self.find_named_scope(b) {
                    return Ok(self.member_or_undef(sid, name, span));
                }
            }
        }
        let v = self.eval(base)?;
        Ok(match v {
            Value::Undef => Value::Undef,
            Value::Scope(sid) => self.member_or_undef(sid, name, span),
            Value::Struct(s) => {
                let i = s.def.fields.iter().position(|x| x == name).ok_or_else(|| err(format!("struct {} has no field `{name}`", s.def.name), span))?;
                s.values.borrow()[i].clone()
            }
            Value::Sid(s) => match name.to_ascii_lowercase().as_str() {
                "header" => Value::str(s.header.as_str()),
                "version" => Value::Int(i64::from(s.version)),
                "location" => Value::Int(i64::from(s.location)),
                "init" => Value::Int(i64::from(s.init)),
                "play" => Value::Int(i64::from(s.play)),
                "songs" => Value::Int(i64::from(s.songs)),
                "startsong" => Value::Int(i64::from(s.start_song)),
                "speed" => Value::Int(i64::from(s.speed)),
                "name" => Value::str(s.name.as_str()),
                "author" => Value::str(s.author.as_str()),
                "copyright" => Value::str(s.copyright.as_str()),
                "flags" => Value::Int(i64::from(s.flags)),
                "startpage" => Value::Int(i64::from(s.start_page)),
                "pagelength" => Value::Int(i64::from(s.page_length)),
                "size" => Value::Int(s.data.len() as i64),
                _ => return Err(err(format!("SID files have no field `{name}`"), span)),
            },
            Value::Picture(p) => match name.to_ascii_lowercase().as_str() {
                "width" => Value::Int(i64::from(p.pic.width)),
                "height" => Value::Int(i64::from(p.pic.height)),
                _ => return Err(err(format!("pictures have no field `{name}`"), span)),
            },
            o => return Err(err(format!("a {} has no field `{name}`", o.type_name()), span)),
        })
    }

    fn member_or_undef(&mut self, sid: usize, name: &str, span: Span) -> Value {
        match self.scope_member(sid, name) {
            Some(v) => v,
            None => {
                self.unresolved = true;
                self.undefined.push((name.to_owned(), span));
                Value::Undef
            }
        }
    }

    /// What a call statement or expression refers to.
    pub fn resolve_callee(&mut self, e: &Expr) -> E {
        match &e.kind {
            ExprKind::Ident(n) => Ok(self.lookup(n).unwrap_or(Value::Null)),
            ExprKind::Field(base, name) => {
                if let Some(b) = base.as_ident() {
                    if let Some(sid) = self.find_named_scope(b) {
                        return Ok(self.scope_member(sid, name).unwrap_or(Value::Null));
                    }
                }
                Ok(Value::Null)
            }
            _ => Ok(Value::Null),
        }
    }

    fn eval_args(&mut self, args: &[Expr]) -> Result<Vec<Value>, Diagnostic> {
        args.iter().map(|a| self.eval(a)).collect()
    }

    fn call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> E {
        match &callee.kind {
            ExprKind::Ident(n) => {
                if n.text == "defined" && args.len() == 1 {
                    if let ExprKind::Ident(x) = &args[0].kind {
                        return Ok(Value::Bool(self.lookup(x).is_some_and(|v| !v.is_undef())));
                    }
                }
                if let Some(v) = self.lookup(n) {
                    return self.call_value(v, &n.text, args, span);
                }
                let a = self.eval_args(args)?;
                if let Some(v) = self.builtin(&n.text, &a, span)? {
                    return Ok(v);
                }
                if let Some(v) = self.platform_function(None, &n.text, &a, span)? {
                    return Ok(v);
                }
                if self.pass == 1 {
                    self.unresolved = true;
                    return Ok(Value::Undef);
                }
                Err(err(format!("unknown function `{}`", n.text), span))
            }
            ExprKind::Field(base, m) => {
                if let Some(b) = base.as_ident() {
                    let shadow = self.lookup(&Name::plain(b));
                    if shadow.is_none() || matches!(shadow, Some(Value::Int(_))) {
                        if let Some(sid) = self.find_named_scope(b) {
                            if let Some(v) = self.scope_member(sid, m) {
                                return self.call_value(v, m, args, span);
                            }
                        }
                        if shadow.is_none() {
                            if let Some(p) = raw_platform::by_namespace(b) {
                                let a = self.eval_args(args)?;
                                if let Some(v) = self.platform_function(Some(p), m, &a, span)? {
                                    return Ok(v);
                                }
                                return Err(err(format!("platform {} has no function `{m}`", p.id), span));
                            }
                        }
                    }
                }
                let obj = self.eval(base)?;
                let a = self.eval_args(args)?;
                self.method(obj, m, a, span)
            }
            _ => Err(err("cannot call this expression", span)),
        }
    }

    fn call_value(&mut self, v: Value, name: &str, args: &[Expr], span: Span) -> E {
        match v {
            Value::Macro(m) if m.kind == MacroKind::Function => {
                let a = self.eval_args(args)?;
                self.call_function(&m, a, span)
            }
            Value::Macro(m) => Err(err(format!("`{}` is a {}, not a function", name, Value::Macro(m).type_name()), span)),
            Value::StructDef(d) => {
                let mut a = self.eval_args(args)?;
                if !a.is_empty() && a.len() != d.fields.len() {
                    return Err(err(format!("struct {} has {} fields", d.name, d.fields.len()), span));
                }
                a.resize(d.fields.len(), Value::Null);
                Ok(Value::Struct(Rc::new(StructVal { def: d, values: RefCell::new(a) })))
            }
            Value::Undef => Ok(Value::Undef),
            o => {
                // a plain symbol shadows a builtin of the same name only when callable
                let a = self.eval_args(args)?;
                if let Some(v) = self.builtin(name, &a, span)? {
                    return Ok(v);
                }
                Err(err(format!("`{name}` is a {}, not a function", o.type_name()), span))
            }
        }
    }

    fn file_bytes(&mut self, v: &Value, span: Span) -> Result<Vec<u8>, Diagnostic> {
        let Value::Str(p) = v else { return Err(err("expected a file name", span)) };
        self.read_file(p, span).map(|x| x.1)
    }

    /// Platform functions (`LoadSid`, `LoadPicture`); only on platforms
    /// that have them, or namespaced (`c64.LoadSid`).
    fn platform_function(&mut self, ns: Option<&'static raw_platform::Platform>, name: &str, a: &[Value], span: Span) -> Result<Option<Value>, Diagnostic> {
        let lname = name.to_ascii_lowercase();
        if !matches!(lname.as_str(), "loadsid" | "loadpicture") {
            return Ok(None);
        }
        let plat = ns.unwrap_or(self.platform);
        if !plat.has_function(name) {
            let owners: Vec<&str> = raw_platform::PLATFORMS.iter().filter(|p| p.has_function(name)).map(|p| p.id).collect();
            return Err(err(
                format!("`{name}` is a Commodore platform function; it needs `.platform {}` (current platform: {})", owners.first().unwrap_or(&"c64"), self.platform.id),
                span,
            ));
        }
        let bytes = self.file_bytes(a.first().unwrap_or(&Value::Null), span)?;
        Ok(Some(match lname.as_str() {
            "loadsid" => Value::Sid(Rc::new(raw_platform::cbm::parse_sid(&bytes).map_err(|m| err(m, span))?)),
            _ => {
                let pic = raw_platform::cbm::Picture::decode(&bytes).map_err(|m| err(m, span))?;
                let mut table = Vec::new();
                if let Some(Value::List(l)) = a.get(1) {
                    for c in l.borrow().iter() {
                        table.push(c.as_i64().unwrap_or(0) as u32);
                    }
                }
                Value::Picture(Rc::new(PictureVal { pic, table: RefCell::new(table) }))
            }
        }))
    }

    /// Built-in functions shared by all dialects.
    fn builtin(&mut self, name: &str, a: &[Value], span: Span) -> Result<Option<Value>, Diagnostic> {
        let f = |i: usize| -> Result<f64, Diagnostic> { num(a.get(i).unwrap_or(&Value::Null), span) };
        let i = |k: usize| -> Result<i64, Diagnostic> { int(a.get(k).unwrap_or(&Value::Null), span) };
        if a.iter().any(Value::is_undef) && !matches!(name, "List" | "Hashtable" | "\u{0}str") {
            return Ok(if is_builtin(name) { Some(Value::Undef) } else { None });
        }
        let lname = name.to_ascii_lowercase();
        let v = match lname.as_str() {
            "\0str" => Value::str(a.first().map(|v| v.to_string()).unwrap_or_default()),
            "abs" => match a.first() {
                Some(Value::Int(n)) => Value::Int(n.abs()),
                _ => Value::num(f(0)?.abs()),
            },
            "acos" | "arccos" => Value::Float(f(0)?.acos()),
            "asin" | "arcsin" => Value::Float(f(0)?.asin()),
            "atan" | "arctan" => Value::Float(f(0)?.atan()),
            "atan2" => Value::Float(f(0)?.atan2(f(1)?)),
            "cbrt" => Value::Float(f(0)?.cbrt()),
            "ceil" => Value::num(f(0)?.ceil()),
            "cos" => Value::Float(f(0)?.cos()),
            "cosh" => Value::Float(f(0)?.cosh()),
            "exp" => Value::Float(f(0)?.exp()),
            "expm1" => Value::Float(f(0)?.exp_m1()),
            "floor" => Value::num(f(0)?.floor()),
            "hypot" => Value::Float(f(0)?.hypot(f(1)?)),
            "ieeeremainder" => {
                let (x, y) = (f(0)?, f(1)?);
                Value::Float(x - (x / y).round() * y)
            }
            "log" => Value::Float(f(0)?.ln()),
            "log10" => Value::Float(f(0)?.log10()),
            "log1p" => Value::Float(f(0)?.ln_1p()),
            "max" => {
                let mut m = f(0)?;
                for k in 1..a.len() {
                    m = m.max(f(k)?);
                }
                Value::num(m)
            }
            "min" => {
                let mut m = f(0)?;
                for k in 1..a.len() {
                    m = m.min(f(k)?);
                }
                Value::num(m)
            }
            "mod" => Value::Int(i(0)?.checked_rem(i(1)?).ok_or_else(|| err("division by zero", span))?),
            "pow" => Value::num(f(0)?.powf(f(1)?)),
            "random" => {
                // a sequence that restarts every pass, so passes agree
                let mut x = self.rng;
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                self.rng = x;
                Value::Float((x >> 11) as f64 / (1u64 << 53) as f64)
            }
            // Java rounding: halves go up
            "round" => Value::num((f(0)? + 0.5).floor()),
            "signum" => Value::num(f(0)?.signum()),
            "sin" => Value::Float(f(0)?.sin()),
            "sinh" => Value::Float(f(0)?.sinh()),
            "sqrt" => Value::Float(f(0)?.sqrt()),
            "tan" => Value::Float(f(0)?.tan()),
            "tanh" => Value::Float(f(0)?.tanh()),
            "todegrees" => Value::Float(f(0)?.to_degrees()),
            "toradians" => Value::Float(f(0)?.to_radians()),
            "int" => Value::Int(f(0)?.trunc() as i64),
            "float" => Value::Float(f(0)?),
            "asmcommandsize" => Value::Int(raw_cpu::mos6502::opcode_size(i(0)? as u8) as i64),
            "getpath" => Value::str(self.file_stack.last().map(|f| f.path.clone()).unwrap_or_default()),
            "getfilename" => Value::str(self.file_stack.last().map(|f| f.path.rsplit(['/', '\\']).next().unwrap_or("").to_owned()).unwrap_or_default()),
            "getnamespace" => Value::str(self.namespace_name()),
            "tointstring" | "tostring" | "str" => match a.first() {
                Some(Value::Str(s)) => Value::Str(s.clone()),
                Some(v) if v.as_i64().is_some() && lname == "tointstring" => {
                    let w = a.get(1).and_then(Value::as_i64).unwrap_or(0) as usize;
                    Value::str(format!("{:>w$}", i(0)?))
                }
                Some(v) => Value::str(v.to_string()),
                None => Value::str(""),
            },
            "tobinarystring" => Value::str(radix_string(i(0)?, 2, a.get(1).and_then(Value::as_i64).unwrap_or(0) as usize)),
            "tooctalstring" => Value::str(radix_string(i(0)?, 8, a.get(1).and_then(Value::as_i64).unwrap_or(0) as usize)),
            "tohexstring" => Value::str(radix_string(i(0)?, 16, a.get(1).and_then(Value::as_i64).unwrap_or(0) as usize)),
            "lo" | "lobyte" => Value::Int(i(0)? & 0xff),
            "hi" | "hibyte" => Value::Int((i(0)? >> 8) & 0xff),
            "bank" | "bankbyte" => Value::Int((i(0)? >> 16) & 0xff),
            "is_number" | "isnumber" => Value::Bool(matches!(a.first(), Some(Value::Int(_) | Value::Float(_)))),
            "is_list" | "islist" => Value::Bool(matches!(a.first(), Some(Value::List(_)))),
            "is_string" | "isstring" => Value::Bool(matches!(a.first(), Some(Value::Str(_)))),
            "len" | "size" => match a.first() {
                Some(Value::List(l)) => Value::Int(l.borrow().len() as i64),
                Some(Value::Str(s)) => Value::Int(s.chars().count() as i64),
                _ => return Err(err("len() needs a list or string", span)),
            },
            "address" | "addr" => a.first().cloned().unwrap_or(Value::Null),
            "list" => {
                let n = a.first().and_then(Value::as_i64).unwrap_or(0).max(0) as usize;
                Value::list(vec![Value::Null; n])
            }
            "hashtable" => Value::Hash(Rc::new(RefCell::new(IndexMap::new()))),
            "vector" => Value::Vector([f(0)?, f(1)?, f(2)?]),
            "matrix" => Value::Matrix(Rc::new(RefCell::new(identity()))),
            "rotationmatrix" => {
                let (x, y, z) = (f(0)?, f(1)?, f(2)?);
                let rx = [[1.0, 0.0, 0.0, 0.0], [0.0, x.cos(), -x.sin(), 0.0], [0.0, x.sin(), x.cos(), 0.0], [0.0, 0.0, 0.0, 1.0]];
                let ry = [[y.cos(), 0.0, y.sin(), 0.0], [0.0, 1.0, 0.0, 0.0], [-y.sin(), 0.0, y.cos(), 0.0], [0.0, 0.0, 0.0, 1.0]];
                let rz = [[z.cos(), -z.sin(), 0.0, 0.0], [z.sin(), z.cos(), 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
                Value::Matrix(Rc::new(RefCell::new(mat_mul(&mat_mul(&rx, &ry), &rz))))
            }
            "scalematrix" => {
                let mut m = identity();
                m[0][0] = f(0)?;
                m[1][1] = f(1)?;
                m[2][2] = f(2)?;
                Value::Matrix(Rc::new(RefCell::new(m)))
            }
            "movematrix" => {
                let mut m = identity();
                m[0][3] = f(0)?;
                m[1][3] = f(1)?;
                m[2][3] = f(2)?;
                Value::Matrix(Rc::new(RefCell::new(m)))
            }
            "perspectivematrix" => {
                let mut m = identity();
                m[3][2] = 1.0 / f(0)?;
                m[3][3] = 0.0;
                Value::Matrix(Rc::new(RefCell::new(m)))
            }
            "cmdargument" => Value::CmdArg(Rc::new(CmdArg { kind: i(0)?, value: a.get(1).cloned().unwrap_or(Value::Null) })),
            "loadbinary" => {
                let data = self.file_bytes(a.first().unwrap_or(&Value::Null), span)?;
                let template = match a.get(1) {
                    Some(Value::Str(s)) => s.to_string(),
                    _ => String::new(),
                };
                let c64 = template.split(',').any(|p| p.trim().eq_ignore_ascii_case(BF_C64FILE));
                let data = if c64 { data.get(2..).unwrap_or(&[]).to_vec() } else { data };
                let mut secs: Vec<(String, usize)> = Vec::new();
                for part in template.split(',').filter(|p| !p.trim().is_empty() && !p.trim().eq_ignore_ascii_case(BF_C64FILE)) {
                    let (n, o) = part.split_once('=').ok_or_else(|| err(format!("bad template part `{part}`"), span))?;
                    let o = o.trim();
                    let off = if let Some(h) = o.strip_prefix('$') {
                        usize::from_str_radix(h, 16)
                    } else if let Some(h) = o.strip_prefix("0x") {
                        usize::from_str_radix(h, 16)
                    } else {
                        o.parse::<usize>()
                    }
                    .map_err(|_| err(format!("bad template offset `{o}`"), span))?;
                    secs.push((n.trim().to_owned(), off));
                }
                let mut sorted = secs.clone();
                sorted.sort_by_key(|s| s.1);
                let sections = secs
                    .iter()
                    .map(|(n, o)| {
                        let end = sorted.iter().map(|s| s.1).filter(|&x| x > *o).min().unwrap_or(data.len());
                        (n.clone(), *o, end.saturating_sub(*o))
                    })
                    .collect();
                Value::Bin(Rc::new(BinFile { data, sections }))
            }
            _ => return Ok(None),
        };
        Ok(Some(v))
    }

    fn method(&mut self, obj: Value, m: &str, a: Vec<Value>, span: Span) -> E {
        let lm = m.to_ascii_lowercase();
        let ai = |k: usize| -> Result<i64, Diagnostic> { int(a.get(k).unwrap_or(&Value::Null), span) };
        if obj.is_undef() {
            return Ok(Value::Undef);
        }
        Ok(match (&obj, lm.as_str()) {
            (Value::List(l), "get") => {
                let k = ai(0)?;
                let l = l.borrow();
                l.get(k as usize).cloned().ok_or_else(|| err(format!("index {k} out of range (size {})", l.len()), span))?
            }
            (Value::List(l), "set") => {
                let k = ai(0)? as usize;
                let mut l = l.borrow_mut();
                if k >= l.len() {
                    return Err(err(format!("index {k} out of range (size {})", l.len()), span));
                }
                l[k] = a.get(1).cloned().unwrap_or(Value::Null);
                obj.clone()
            }
            (Value::List(l), "add") => {
                l.borrow_mut().extend(a.iter().cloned());
                obj.clone()
            }
            (Value::List(l), "addall") => {
                if let Some(Value::List(o)) = a.first() {
                    let items = o.borrow().clone();
                    l.borrow_mut().extend(items);
                }
                obj.clone()
            }
            (Value::List(l), "size") => Value::Int(l.borrow().len() as i64),
            (Value::List(l), "remove") => {
                let k = ai(0)? as usize;
                let mut l = l.borrow_mut();
                if k >= l.len() {
                    return Err(err(format!("index {k} out of range"), span));
                }
                l.remove(k)
            }
            (Value::List(l), "reverse") => {
                l.borrow_mut().reverse();
                obj.clone()
            }
            (Value::List(l), "sort") => {
                l.borrow_mut().sort_by(|x, y| x.as_f64().unwrap_or(0.0).partial_cmp(&y.as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
                obj.clone()
            }
            (Value::List(l), "shuffle") => {
                let mut v = l.borrow_mut();
                let n = v.len();
                let mut x = 0x9e37_79b9_7f4a_7c15u64;
                for k in (1..n).rev() {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    v.swap(k, (x % (k as u64 + 1)) as usize);
                }
                drop(v);
                obj.clone()
            }
            (Value::List(_) | Value::Hash(_), "lock") => obj.clone(),
            (Value::Hash(h), "put") => {
                // put(k, v, k2, v2, ...)
                for pair in a.chunks(2) {
                    h.borrow_mut().insert(pair[0].hkey(), pair.get(1).cloned().unwrap_or(Value::Null));
                }
                obj.clone()
            }
            (Value::Hash(h), "get") => {
                let k = a.first().cloned().unwrap_or(Value::Null).hkey();
                h.borrow().get(&k).cloned().unwrap_or(Value::Null)
            }
            (Value::Hash(h), "containskey") => Value::Bool(h.borrow().contains_key(&a.first().cloned().unwrap_or(Value::Null).hkey())),
            (Value::Hash(h), "remove") => h.borrow_mut().shift_remove(&a.first().cloned().unwrap_or(Value::Null).hkey()).unwrap_or(Value::Null),
            (Value::Hash(h), "keys") => Value::list(
                h.borrow()
                    .keys()
                    .map(|k| match k {
                        HKey::Str(s) => Value::str(s.as_str()),
                    })
                    .collect(),
            ),
            (Value::Hash(h), "size") => Value::Int(h.borrow().len() as i64),
            (Value::Str(s), "charat") => Value::str(s.chars().nth(ai(0)? as usize).map(|c| c.to_string()).unwrap_or_default()),
            (Value::Str(s), "size" | "length") => Value::Int(s.chars().count() as i64),
            (Value::Str(s), "substring") => {
                let chars: Vec<char> = s.chars().collect();
                let a0 = (ai(0)?.max(0) as usize).min(chars.len());
                let b0 = a.get(1).and_then(Value::as_i64).map_or(chars.len(), |b| (b.max(0) as usize).min(chars.len()));
                Value::str(chars[a0..b0.max(a0)].iter().collect::<String>())
            }
            (Value::Str(s), "tolowercase") => Value::str(s.to_lowercase()),
            (Value::Str(s), "touppercase") => Value::str(s.to_uppercase()),
            (Value::Str(s), "asnumber") if !a.is_empty() => {
                let radix = ai(0)? as u32;
                Value::Int(i64::from_str_radix(s.trim(), radix).map_err(|_| err(format!("`{s}` is not a base-{radix} number"), span))?)
            }
            (Value::Str(s), "asnumber") => {
                let t = s.trim();
                let v = if let Some(h) = t.strip_prefix('$') {
                    i64::from_str_radix(h, 16).ok().map(Value::Int)
                } else {
                    t.parse::<i64>().ok().map(Value::Int).or_else(|| t.parse::<f64>().ok().map(Value::Float))
                };
                v.ok_or_else(|| err(format!("`{t}` is not a number"), span))?
            }
            (Value::Str(s), "asboolean") => Value::Bool(s.trim().eq_ignore_ascii_case("true")),
            (Value::Str(s), "string" | "tostring") => Value::Str(s.clone()),
            (Value::Struct(s), "getstructname") => Value::str(s.def.name.as_str()),
            (Value::Struct(s), "getnooffields") => Value::Int(s.def.fields.len() as i64),
            (Value::Struct(s), "getfieldnames") => Value::list(s.def.fields.iter().map(|f| Value::str(f.as_str())).collect()),
            (Value::Struct(s), "get") => {
                let k = match a.first() {
                    Some(Value::Str(n)) => s.def.fields.iter().position(|f| **f == **n).ok_or_else(|| err(format!("struct {} has no field `{n}`", s.def.name), span))?,
                    _ => ai(0)? as usize,
                };
                s.values.borrow().get(k).cloned().unwrap_or(Value::Null)
            }
            (Value::Struct(s), "set") => {
                let k = match a.first() {
                    Some(Value::Str(n)) => s.def.fields.iter().position(|f| **f == **n).ok_or_else(|| err(format!("struct {} has no field `{n}`", s.def.name), span))?,
                    _ => ai(0)? as usize,
                };
                if let Some(slot) = s.values.borrow_mut().get_mut(k) {
                    *slot = a.get(1).cloned().unwrap_or(Value::Null);
                }
                obj.clone()
            }
            (Value::CmdArg(c), "gettype") => Value::Int(c.kind),
            (Value::CmdArg(c), "getvalue") => c.value.clone(),
            (Value::Sid(s), "getdata") => Value::Int(i64::from(*s.data.get(ai(0)? as usize).ok_or_else(|| err("SID data index out of range", span))?)),
            (Value::Sid(_), _) => self.field(&Expr::new(ExprKind::Null, span), m, span).unwrap_or(Value::Null),
            (Value::Picture(p), "getpixel") => Value::Int(i64::from(p.pic.pixel(ai(0)?, ai(1)?).map_err(|e| err(e, span))?)),
            (Value::Picture(p), "getsinglecolorbyte") => {
                Value::Int(i64::from(p.pic.singlecolor_byte(ai(0)?, ai(1)?, &mut p.table.borrow_mut()).map_err(|e| err(e, span))?))
            }
            (Value::Picture(p), "getmulticolorbyte") => {
                Value::Int(i64::from(p.pic.multicolor_byte(ai(0)?, ai(1)?, &mut p.table.borrow_mut()).map_err(|e| err(e, span))?))
            }
            (Value::Bin(b), "getsize") => Value::Int(b.data.len() as i64),
            (Value::Bin(b), "get") => Value::Int(i64::from(*b.data.get(ai(0)? as usize).ok_or_else(|| err("index out of range", span))? as i8)),
            (Value::Bin(b), "uget") => Value::Int(i64::from(*b.data.get(ai(0)? as usize).ok_or_else(|| err("index out of range", span))?)),
            (Value::Bin(b), _) => {
                // template sections: getBitmap(i), getBitmapSize()
                let unsigned = lm.starts_with("uget");
                let lm2 = lm.trim_start_matches('u');
                let (sec, size) = if let Some(n) = lm2.strip_prefix("get").and_then(|r| r.strip_suffix("size")) {
                    (n.to_owned(), true)
                } else if let Some(n) = lm2.strip_prefix("get") {
                    (n.to_owned(), false)
                } else {
                    return Err(err(format!("binary files have no method `{m}`"), span));
                };
                let (_, off, len) = b
                    .sections
                    .iter()
                    .find(|s| s.0.eq_ignore_ascii_case(&sec))
                    .ok_or_else(|| err(format!("binary template has no section `{sec}`"), span))?;
                if size {
                    Value::Int(*len as i64)
                } else {
                    let k = if a.is_empty() { 0 } else { ai(0)? as usize };
                    let byte = *b.data.get(off + k).ok_or_else(|| err("index out of range", span))?;
                    Value::Int(if unsigned { i64::from(byte) } else { i64::from(byte as i8) })
                }
            }
            (Value::Vector(v), "get") => Value::Float(v[(ai(0)? as usize).min(2)]),
            (Value::Vector(v), "getx") => Value::Float(v[0]),
            (Value::Vector(v), "gety") => Value::Float(v[1]),
            (Value::Vector(v), "getz") => Value::Float(v[2]),
            (Value::Vector(v), "x") => match a.first() {
                Some(Value::Vector(w)) => Value::Vector([v[1] * w[2] - v[2] * w[1], v[2] * w[0] - v[0] * w[2], v[0] * w[1] - v[1] * w[0]]),
                _ => return Err(err("X() needs a vector", span)),
            },
            (Value::Matrix(mx), "get") => Value::Float(mx.borrow()[(ai(0)? as usize).min(3)][(ai(1)? as usize).min(3)]),
            (Value::Matrix(mx), "set") => {
                let v = num(a.get(2).unwrap_or(&Value::Null), span)?;
                mx.borrow_mut()[(ai(0)? as usize).min(3)][(ai(1)? as usize).min(3)] = v;
                obj.clone()
            }
            (o, "string" | "tostring") => Value::str(o.to_string()),
            (Value::Int(_) | Value::Float(_), "number") => obj.clone(),
            (Value::Str(s), "number") if s.chars().count() == 1 => Value::Int(s.chars().next().unwrap() as i64),
            (o, _) => return Err(err(format!("a {} has no method `{m}`", o.type_name()), span)),
        })
    }

    /// Preprocessor condition: names are true when `#define`d.
    pub fn eval_pp(&mut self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Ident(n) => self.pp_defines.contains(&n.text) || self.opts.defines.iter().any(|(k, _)| *k == n.text),
            ExprKind::Bool(b) => *b,
            ExprKind::Int(v) => *v != 0,
            ExprKind::Unary(UnOp::Not, x) => !self.eval_pp(x),
            ExprKind::Binary(BinOp::LAnd, a, b) => self.eval_pp(a) && self.eval_pp(b),
            ExprKind::Binary(BinOp::LOr, a, b) => self.eval_pp(a) || self.eval_pp(b),
            ExprKind::Binary(BinOp::Eq, a, b) => self.eval_pp(a) == self.eval_pp(b),
            ExprKind::Binary(BinOp::Ne, a, b) => self.eval_pp(a) != self.eval_pp(b),
            _ => self.eval_cond(e).unwrap_or(false),
        }
    }

    pub fn define_globals(&mut self) {
        let mut h = IndexMap::new();
        for (k, v) in &self.opts.defines {
            h.insert(HKey::Str(k.clone()), Value::str(v.as_str()));
        }
        let span = Span::default();
        self.define_in(0, "cmdLineVars", Value::Hash(Rc::new(RefCell::new(h))), SymKind::Const, span);
        // KickAssembler keeps `-D` values in cmdLineVars and #define only
        if self.dialect() == Dialect::Kick {
            return;
        }
        let defs = self.opts.defines.clone();
        for (k, v) in defs {
            let d = self.dialect();
            let val = match raw_syntax::parse_expr_str(d, &v) {
                Ok(x) => self.eval(&x).unwrap_or_else(|_| Value::str(v.as_str())),
                Err(_) => Value::str(v.as_str()),
            };
            let val = if v.is_empty() { Value::Bool(true) } else { val };
            self.define_in(0, &k, val, SymKind::Const, span);
        }
    }
}

fn is_builtin(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "abs" | "acos" | "asin" | "atan" | "atan2" | "cbrt" | "ceil" | "cos" | "cosh" | "exp" | "expm1" | "floor" | "hypot" | "ieeeremainder"
            | "log" | "log10" | "log1p" | "max" | "min" | "mod" | "pow" | "round" | "signum" | "sin" | "sinh" | "sqrt" | "tan" | "tanh"
            | "todegrees" | "toradians" | "int" | "float" | "tointstring" | "tobinarystring" | "tooctalstring" | "tohexstring" | "lo"
            | "hi" | "bank" | "lobyte" | "hibyte" | "bankbyte" | "arcsin" | "arccos" | "arctan" | "len" | "size" | "address" | "addr"
            | "vector" | "rotationmatrix" | "scalematrix" | "movematrix" | "perspectivematrix" | "cmdargument" | "str" | "tostring"
    )
}

fn constant(name: &str) -> Option<Value> {
    Some(match name {
        "PI" => Value::Float(std::f64::consts::PI),
        "E" => Value::Float(std::f64::consts::E),
        "AT_ABSOLUTE" => Value::Int(AT_ABSOLUTE),
        "AT_ABSOLUTEX" => Value::Int(AT_ABSOLUTEX),
        "AT_ABSOLUTEY" => Value::Int(AT_ABSOLUTEY),
        "AT_IMMEDIATE" => Value::Int(AT_IMMEDIATE),
        "AT_INDIRECT" => Value::Int(AT_INDIRECT),
        "AT_IZEROPAGEX" => Value::Int(AT_IZEROPAGEX),
        "AT_IZEROPAGEY" => Value::Int(AT_IZEROPAGEY),
        "AT_NONE" => Value::Int(AT_NONE),
        "BF_C64FILE" => Value::str(BF_C64FILE),
        "BF_BITMAP_SINGLECOLOR" => Value::str(BF_BITMAP_SINGLECOLOR),
        "BF_KOALA" => Value::str(BF_KOALA),
        "BF_FLI" => Value::str(BF_FLI),
        "BF_DOODLE" => Value::str(BF_DOODLE),
        _ => return None,
    })
}

fn index(b: &Value, i: &Value, span: Span) -> E {
    if b.is_undef() || i.is_undef() {
        return Ok(Value::Undef);
    }
    match b {
        Value::List(l) => {
            let k = int(i, span)?;
            let l = l.borrow();
            l.get(k as usize).cloned().ok_or_else(|| err(format!("index {k} out of range (size {})", l.len()), span))
        }
        Value::Str(s) => {
            let k = int(i, span)?;
            s.chars().nth(k as usize).map(|c| Value::Int(c as i64)).ok_or_else(|| err("string index out of range", span))
        }
        Value::Hash(h) => Ok(h.borrow().get(&i.hkey()).cloned().unwrap_or(Value::Null)),
        o => Err(err(format!("cannot index a {}", o.type_name()), span)),
    }
}

pub(crate) fn binop(op: BinOp, a: &Value, b: &Value, span: Span) -> E {
    use Value::*;
    if a.is_undef() || b.is_undef() {
        // string concatenation with an unresolved part stays unresolved
        return Ok(Undef);
    }
    match op {
        BinOp::Eq => return Ok(Bool(a == b)),
        BinOp::Ne => return Ok(Bool(a != b)),
        _ => {}
    }
    // strings
    if op == BinOp::Add && (matches!(a, Str(_)) || matches!(b, Str(_))) {
        return Ok(Value::str(format!("{a}{b}")));
    }
    if let (Str(x), Str(y)) = (a, b) {
        return Ok(Bool(match op {
            BinOp::Lt => x < y,
            BinOp::Le => x <= y,
            BinOp::Gt => x > y,
            BinOp::Ge => x >= y,
            _ => return Err(err("invalid operation on strings", span)),
        }));
    }
    if let (List(x), List(y)) = (a, b) {
        if op == BinOp::Add {
            let mut v = x.borrow().clone();
            v.extend(y.borrow().iter().cloned());
            return Ok(Value::list(v));
        }
    }
    // vectors and matrices
    match (a, b, op) {
        (Vector(x), Vector(y), BinOp::Add) => return Ok(Vector([x[0] + y[0], x[1] + y[1], x[2] + y[2]])),
        (Vector(x), Vector(y), BinOp::Sub) => return Ok(Vector([x[0] - y[0], x[1] - y[1], x[2] - y[2]])),
        (Vector(x), Vector(y), BinOp::Mul) => return Ok(Float(x[0] * y[0] + x[1] * y[1] + x[2] * y[2])),
        (Vector(x), n, BinOp::Mul) | (n, Vector(x), BinOp::Mul) if n.as_f64().is_some() => {
            let k = n.as_f64().unwrap();
            return Ok(Vector([x[0] * k, x[1] * k, x[2] * k]));
        }
        (Vector(x), n, BinOp::FDiv | BinOp::Div) if n.as_f64().is_some() => {
            let k = n.as_f64().unwrap();
            return Ok(Vector([x[0] / k, x[1] / k, x[2] / k]));
        }
        (Matrix(m), Vector(v), BinOp::Mul) => {
            let m = *m.borrow();
            let p = [v[0], v[1], v[2], 1.0];
            let r: Vec<f64> = (0..4).map(|row| (0..4).map(|k| m[row][k] * p[k]).sum()).collect();
            let w = if r[3] == 0.0 { 1.0 } else { r[3] };
            return Ok(Vector([r[0] / w, r[1] / w, r[2] / w]));
        }
        (Matrix(x), Matrix(y), BinOp::Mul) => return Ok(Matrix(Rc::new(RefCell::new(mat_mul(&x.borrow(), &y.borrow()))))),
        _ => {}
    }
    let both_int = matches!((a, b), (Int(_) | Bool(_), Int(_) | Bool(_)));
    if both_int || matches!(op, BinOp::Shl | BinOp::Shr | BinOp::Lsr | BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::IntDiv | BinOp::Mod) && a.as_i64().is_some() && b.as_i64().is_some() {
        let (x, y) = (int(a, span)?, int(b, span)?);
        let shift = |y: i64| -> Result<u32, Diagnostic> {
            if (0..64).contains(&y) {
                Ok(y as u32)
            } else {
                Err(err(format!("bad shift count {y}"), span))
            }
        };
        return Ok(match op {
            BinOp::Add => Int(x.wrapping_add(y)),
            BinOp::Sub => Int(x.wrapping_sub(y)),
            BinOp::Mul => Int(x.wrapping_mul(y)),
            BinOp::Div | BinOp::IntDiv => {
                if y == 0 {
                    return Err(err("division by zero", span));
                }
                Int(x.wrapping_div(y))
            }
            BinOp::FDiv => {
                if y == 0 {
                    return Err(err("division by zero", span));
                }
                Value::num(x as f64 / y as f64)
            }
            BinOp::Mod => {
                if y == 0 {
                    return Err(err("division by zero", span));
                }
                Int(x.wrapping_rem(y))
            }
            BinOp::Pow => {
                if y >= 0 {
                    Int(x.wrapping_pow(y.min(u32::MAX as i64) as u32))
                } else {
                    Float((x as f64).powf(y as f64))
                }
            }
            BinOp::Shl => Int(x.wrapping_shl(shift(y)?)),
            BinOp::Shr => Int(x >> shift(y)?),
            BinOp::Lsr => Int(((x as u64) >> shift(y)?) as i64),
            BinOp::BitAnd => Int(x & y),
            BinOp::BitOr => Int(x | y),
            BinOp::BitXor => Int(x ^ y),
            BinOp::Lt => Bool(x < y),
            BinOp::Le => Bool(x <= y),
            BinOp::Gt => Bool(x > y),
            BinOp::Ge => Bool(x >= y),
            BinOp::Eq | BinOp::Ne | BinOp::LAnd | BinOp::LOr => unreachable!(),
        });
    }
    let (x, y) = (num(a, span)?, num(b, span)?);
    Ok(match op {
        BinOp::Add => Value::num(x + y),
        BinOp::Sub => Value::num(x - y),
        BinOp::Mul => Value::num(x * y),
        BinOp::Div | BinOp::FDiv | BinOp::IntDiv => {
            if y == 0.0 {
                return Err(err("division by zero", span));
            }
            if op == BinOp::IntDiv { Int((x / y).trunc() as i64) } else { Value::num(x / y) }
        }
        BinOp::Mod => Value::num(x % y),
        BinOp::Pow => Value::num(x.powf(y)),
        BinOp::Lt => Bool(x < y),
        BinOp::Le => Bool(x <= y),
        BinOp::Gt => Bool(x > y),
        BinOp::Ge => Bool(x >= y),
        _ => return Err(err("invalid operation on numbers", span)),
    })
}

const COLOURS: &[(&str, i64)] = &[
    ("BLACK", 0),
    ("WHITE", 1),
    ("RED", 2),
    ("CYAN", 3),
    ("PURPLE", 4),
    ("GREEN", 5),
    ("BLUE", 6),
    ("YELLOW", 7),
    ("ORANGE", 8),
    ("BROWN", 9),
    ("LIGHT_RED", 10),
    ("DARK_GRAY", 11),
    ("DARK_GREY", 11),
    ("GRAY", 12),
    ("GREY", 12),
    ("LIGHT_GREEN", 13),
    ("LIGHT_BLUE", 14),
    ("LIGHT_GRAY", 15),
    ("LIGHT_GREY", 15),
];

impl Engine<'_> {
    /// KickAssembler colour constants (C64 platforms) and opcode constants
    /// (`LDA_IMM`, `RTS`; 65xx CPUs).
    fn platform_constant(&self, name: &str) -> Option<Value> {
        if self.platform.basic_start.is_some() {
            if let Some((_, v)) = COLOURS.iter().find(|(n, _)| *n == name) {
                return Some(Value::Int(*v));
            }
        }
        let id = self.cpu.info().id;
        if matches!(id, "6502" | "6502ni" | "6510" | "2a03" | "6507") && name.bytes().next().is_some_and(|b| b.is_ascii_uppercase()) {
            return raw_cpu::mos6502::opcode_constant(name, id != "6502ni").map(|op| Value::Int(i64::from(op)));
        }
        None
    }

    /// Name of the innermost named scope (`getNamespace()`).
    fn namespace_name(&self) -> String {
        let cur = self.cur_scope();
        for s in &self.scopes {
            for (n, &c) in &s.named {
                if c == cur {
                    return n.clone();
                }
            }
        }
        String::new()
    }
}
