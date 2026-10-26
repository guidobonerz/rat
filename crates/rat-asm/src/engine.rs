//! The assembler: executes the statement IR once per pass until every
//! symbol and size is stable.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use indexmap::IndexMap;
use rat_core::*;
use rat_cpu::{Cpu, CpuState, Num, OperandCtx};
use rat_platform::Platform;

use crate::memory::{SegParams, Segment};
use crate::value::*;
use crate::{Options, Vfs};

pub(crate) enum Flow {
    Normal,
    Return(Value),
    Eof,
}

/// Aborts the pass (`!serious`, recursion limits).
pub(crate) struct Fatal(pub Diagnostic);

pub(crate) type R = Result<Flow, Fatal>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SymKind {
    Label,
    Const,
    Var,
    Set,
    Def,
}

#[derive(Clone, Debug)]
pub(crate) struct Sym {
    pub value: Value,
    pub kind: SymKind,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Scope {
    pub parent: Option<usize>,
    pub syms: HashMap<String, Sym>,
    pub named: HashMap<String, usize>,
}

pub(crate) struct Loaded {
    pub path: String,
    pub dialect: Dialect,
    pub parsed: rat_syntax::ParsedFile,
}

/// Output requested by the source (`.file`, `.disk`, `outPrg`, `!to`).
#[derive(Clone, Debug)]
pub(crate) enum OutReq {
    File { name: String, segments: Vec<String>, format: Option<String>, span: Span },
    Disk { name: String, disk_name: String, id: String, files: Vec<(String, String, Vec<String>)>, span: Span },
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PlatformState {
    pub xex_run: Option<u16>,
    pub xex_inits: Vec<(u32, u16)>,
    pub ines: Option<rat_output::InesHeader>,
    pub snes: Option<rat_output::SnesHeader>,
    pub snes_map: Option<rat_output::SnesMap>,
    pub amsdos: Option<(String, Option<u16>)>,
    pub crt: Option<rat_output::CrtInfo>,
    pub tap: Option<rat_output::spectrum::TapOptions>,
    pub sna: Option<rat_output::spectrum::SnaOptions>,
    pub dsk: Option<rat_output::dos33::DskOptions>,
}

pub(crate) struct ListLine {
    pub file: FileId,
    pub line: usize,
    pub pc: u32,
    pub bytes: Vec<u8>,
}

pub(crate) struct Engine<'o> {
    pub opts: &'o Options,
    pub vfs: &'o dyn Vfs,
    pub sources: SourceMap,
    pub files: HashMap<String, Rc<Loaded>>,
    pub load_errors: Vec<Diagnostic>,

    // ---- persistent across passes
    pub pass: u32,
    pub sizes: HashMap<(usize, u32), u8>,
    pub prev_scopes: Vec<Scope>,
    pub prev_anon: HashMap<String, Vec<i64>>,
    pub zp_names: HashSet<String>,

    // ---- per pass
    pub scopes: Vec<Scope>,
    pub scope_stack: Vec<usize>,
    pub cpu: &'static dyn Cpu,
    pub cpu_stack: Vec<&'static dyn Cpu>,
    pub cpu_state: CpuState,
    pub cpu_state_stack: Vec<CpuState>,
    pub platform: &'static Platform,
    pub platform_explicit: bool,
    pub prelude_scopes: HashMap<&'static str, usize>,
    pub endian: Option<Endian>,
    pub encoding: Option<String>,
    pub enc_stack: Vec<Option<String>>,
    pub convtab: Option<Box<[u8; 256]>>,
    pub segments: IndexMap<String, Segment>,
    pub cur_seg: String,
    pub pc_offsets: Vec<i64>,
    pub zone: u32,
    pub zone_counter: u32,
    pub cheap: u32,
    pub anon: HashMap<String, Vec<i64>>,
    pub site_counts: HashMap<usize, u32>,
    pub diags: Vec<Diagnostic>,
    pub changed: bool,
    pub unresolved: bool,
    pub forward_used: bool,
    pub undefined: Vec<(String, Span)>,
    pub prints: Vec<String>,
    pub pp_defines: HashSet<String>,
    pub once: HashSet<String>,
    pub file_stack: Vec<Rc<Loaded>>,
    pub dialect_stack: Vec<Dialect>,
    pub outputs: Vec<OutReq>,
    pub main_output: Option<(String, Option<String>)>,
    pub symbol_lists: Vec<String>,
    pub plat: PlatformState,
    pub listing: Vec<ListLine>,
    pub emit_log: Vec<u8>,
    pub breaks: Vec<u32>,
    pub watches: Vec<String>,
    pub initmem: u8,
    pub xor: u8,
    pub depth: u32,
    pub zp_hint: bool,
    pub in_function: u32,
    pub fatal: Option<Diagnostic>,
    pub hunk_bases: Vec<u32>,
    pub hunk_order: Vec<String>,
    pub rng: u64,
}

pub(crate) const ROOT: usize = 0;

impl<'o> Engine<'o> {
    pub fn new(opts: &'o Options, vfs: &'o dyn Vfs) -> Engine<'o> {
        Engine {
            opts,
            vfs,
            sources: SourceMap::new(),
            files: HashMap::new(),
            load_errors: Vec::new(),
            pass: 0,
            sizes: HashMap::new(),
            prev_scopes: Vec::new(),
            prev_anon: HashMap::new(),
            zp_names: HashSet::new(),
            scopes: Vec::new(),
            scope_stack: Vec::new(),
            cpu: rat_cpu::by_name("6502").unwrap(),
            cpu_stack: Vec::new(),
            cpu_state: CpuState::default(),
            cpu_state_stack: Vec::new(),
            platform: rat_platform::by_name("generic").unwrap(),
            platform_explicit: false,
            prelude_scopes: HashMap::new(),
            endian: None,
            encoding: None,
            enc_stack: Vec::new(),
            convtab: None,
            segments: IndexMap::new(),
            cur_seg: String::new(),
            pc_offsets: Vec::new(),
            zone: 0,
            zone_counter: 0,
            cheap: 0,
            anon: HashMap::new(),
            site_counts: HashMap::new(),
            diags: Vec::new(),
            changed: false,
            unresolved: false,
            forward_used: false,
            undefined: Vec::new(),
            prints: Vec::new(),
            pp_defines: HashSet::new(),
            once: HashSet::new(),
            file_stack: Vec::new(),
            dialect_stack: Vec::new(),
            outputs: Vec::new(),
            main_output: None,
            symbol_lists: Vec::new(),
            plat: PlatformState::default(),
            listing: Vec::new(),
            emit_log: Vec::new(),
            breaks: Vec::new(),
            watches: Vec::new(),
            initmem: 0,
            xor: 0,
            depth: 0,
            zp_hint: false,
            in_function: 0,
            fatal: None,
            hunk_bases: Vec::new(),
            hunk_order: Vec::new(),
            rng: 0x2545_f491_4f6c_dd1d,
        }
    }

    // ------------------------------------------------------------ files

    pub fn dialect(&self) -> Dialect {
        self.dialect_stack.last().copied().unwrap_or(Dialect::Rat)
    }

    fn dir_of(path: &str) -> String {
        match path.rfind(['/', '\\']) {
            Some(i) => path[..i].to_owned(),
            None => String::new(),
        }
    }

    /// Resolve an include path: relative to the including file, then the
    /// include directories.
    pub fn resolve(&self, name: &str) -> Option<String> {
        let mut cands = Vec::new();
        if let Some(f) = self.file_stack.last() {
            cands.push(crate::join(&Self::dir_of(&f.path), name));
        }
        cands.push(name.to_owned());
        for d in &self.opts.include_dirs {
            cands.push(crate::join(d, name));
        }
        cands.into_iter().find(|c| self.vfs.exists(c))
    }

    pub fn read_file(&self, name: &str, span: Span) -> Result<(String, Vec<u8>), Diagnostic> {
        let path = self.resolve(name).ok_or_else(|| Diagnostic::error(format!("file not found: {name}"), Some(span)))?;
        let data = self
            .vfs
            .read(&path)
            .ok_or_else(|| Diagnostic::error(format!("cannot read {path}"), Some(span)))?;
        Ok((path, data))
    }

    /// Load and parse a source file once.
    pub fn load(&mut self, path: &str, forced: Option<Dialect>, text: Option<String>) -> Result<Rc<Loaded>, Diagnostic> {
        if let Some(l) = self.files.get(path) {
            return Ok(l.clone());
        }
        let text = match text {
            Some(t) => t,
            None => {
                let bytes = self.vfs.read(path).ok_or_else(|| Diagnostic::error(format!("cannot read {path}"), None))?;
                String::from_utf8_lossy(&bytes).into_owned()
            }
        };
        let inherited = self.dialect_stack.last().copied().unwrap_or(Dialect::Rat);
        let dialect = rat_syntax::detect_dialect(&text, path, forced, inherited);
        let id = self.sources.add(path, text.clone());
        let opts = rat_syntax::Options {
            dialect,
            cpu: self.cpu,
            platform_cpu: rat_platform::platform_cpu,
        };
        let (parsed, errs) = rat_syntax::parse(id, &text, &opts);
        self.zp_names.extend(parsed.zp_names.iter().cloned());
        self.load_errors.extend(errs);
        let l = Rc::new(Loaded { path: path.to_owned(), dialect, parsed });
        self.files.insert(path.to_owned(), l.clone());
        Ok(l)
    }

    /// Execute a loaded file in the current scope.
    pub fn exec_file(&mut self, l: Rc<Loaded>) -> Result<(), Fatal> {
        if self.file_stack.len() > 64 {
            return Err(Fatal(Diagnostic::error(format!("include nesting too deep at {}", l.path), None)));
        }
        self.file_stack.push(l.clone());
        self.dialect_stack.push(l.dialect);
        let depth = self.scope_stack.len();
        self.hoist(&l.parsed.stmts);
        let r = self.exec_block(&l.parsed.stmts);
        // `.filenamespace` / unbraced `.namespace` end with the file
        self.scope_stack.truncate(depth);
        self.dialect_stack.pop();
        self.file_stack.pop();
        r.map(|_| ())
    }

    /// Define macros/functions/structs of a block up front so they can be
    /// used before their definition.
    pub fn hoist(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            if let StmtKind::MacroDef(m) = &s.kind {
                let (key, sid) = match m.name.strip_prefix('@') {
                    Some(r) => (r.to_owned(), ROOT),
                    None => (m.name.clone(), self.cur_scope()),
                };
                self.scopes[sid].syms.entry(key).or_insert_with(|| Sym { value: Value::Macro(m.clone()), kind: SymKind::Def, span: s.span });
            }
        }
    }

    // ----------------------------------------------------------- scopes

    pub fn cur_scope(&self) -> usize {
        *self.scope_stack.last().unwrap_or(&ROOT)
    }

    pub fn new_scope(&mut self, name: Option<&str>) -> usize {
        let parent = self.cur_scope();
        let id = self.scopes.len();
        self.scopes.push(Scope { parent: Some(parent), ..Default::default() });
        if let Some(n) = name {
            self.scopes[parent].named.insert(n.to_owned(), id);
        }
        id
    }

    /// Storage key of a name: ACME zone locals and cheap locals are mangled
    /// into the root scope.
    fn key(&self, n: &Name) -> (usize, String) {
        match n.kind {
            NameKind::Plain => (usize::MAX, n.text.clone()),
            NameKind::ZoneLocal => (ROOT, format!("\u{1}z{}.{}", self.zone, n.text)),
            NameKind::Cheap => (ROOT, format!("\u{1}c{}@{}", self.cheap, n.text)),
            NameKind::Root => (ROOT, n.text.clone()),
        }
    }

    pub fn define(&mut self, n: &Name, value: Value, kind: SymKind, span: Span) {
        let (fixed, key) = self.key(n);
        let sid = if fixed != usize::MAX {
            fixed
        } else if self.dialect() == Dialect::Acme && self.in_function == 0 && kind != SymKind::Var {
            ROOT
        } else {
            self.cur_scope()
        };
        self.define_in(sid, &key, value, kind, span);
        if n.kind == NameKind::Plain && kind == SymKind::Label && matches!(self.dialect(), Dialect::Acme | Dialect::Rat) {
            self.cheap += 1;
        }
    }

    pub fn define_in(&mut self, sid: usize, key: &str, value: Value, kind: SymKind, span: Span) {
        if let Some(old) = self.scopes[sid].syms.get(key) {
            let redefinable = matches!(old.kind, SymKind::Var | SymKind::Set) && matches!(kind, SymKind::Var | SymKind::Set);
            if !redefinable && !(old.kind == SymKind::Def && kind == SymKind::Def) {
                if old.value != value || old.kind != kind {
                    let mut d = Diagnostic::error(format!("symbol `{}` is already defined", display_key(key)), Some(span));
                    if let Some((f, l, c)) = self.sources.location(old.span) {
                        d = d.with_note(format!("first defined at {f}:{l}:{c}"));
                    }
                    self.diags.push(d);
                }
                return;
            }
        }
        if !matches!(kind, SymKind::Var | SymKind::Set) {
            let prev = self.prev_scopes.get(sid).and_then(|s| s.syms.get(key));
            match prev {
                Some(p) if p.value == value => {}
                _ => {
                    if std::env::var_os("RAW_DEBUG").is_some() {
                        eprintln!("pass {}: `{}` changed in scope {sid}: {:?} -> {}", self.pass, display_key(key), prev.map(|p| p.value.to_string()), value);
                    }
                    self.changed = true
                }
            }
        }
        self.scopes[sid].syms.insert(key.to_owned(), Sym { value, kind, span });
    }

    /// Look up a name through the scope chain, then the previous pass, then
    /// the active platform prelude.
    pub fn lookup(&mut self, n: &Name) -> Option<Value> {
        let (fixed, key) = self.key(n);
        if fixed != usize::MAX {
            return self.lookup_in(fixed, &key);
        }
        let mut sid = Some(self.cur_scope());
        while let Some(s) = sid {
            if let Some(v) = self.lookup_in(s, &key) {
                return Some(v);
            }
            sid = self.scopes[s].parent;
        }
        let pid = self.platform.id;
        if let Some(&ps) = self.prelude_scopes.get(pid) {
            if let Some(sym) = self.scopes[ps].syms.get(&key) {
                return Some(sym.value.clone());
            }
        }
        None
    }

    fn lookup_in(&mut self, s: usize, key: &str) -> Option<Value> {
        if let Some(sym) = self.scopes[s].syms.get(key) {
            return Some(sym.value.clone());
        }
        if let Some(sym) = self.prev_scopes.get(s).and_then(|p| p.syms.get(key)) {
            if sym.kind != SymKind::Var {
                self.forward_used = true;
                return Some(sym.value.clone());
            }
        }
        None
    }

    /// Named child scope reachable from the current scope chain.
    pub fn find_named_scope(&self, name: &str) -> Option<usize> {
        let mut sid = Some(self.cur_scope());
        while let Some(s) = sid {
            if let Some(&c) = self.scopes[s].named.get(name) {
                return Some(c);
            }
            sid = self.scopes[s].parent;
        }
        // previous pass (forward reference to a later scope)
        let mut sid = Some(self.cur_scope());
        while let Some(s) = sid {
            if let Some(&c) = self.prev_scopes.get(s).and_then(|p| p.named.get(name)) {
                if c < self.prev_scopes.len() {
                    return Some(c);
                }
            }
            sid = self.scopes.get(s).and_then(|x| x.parent);
        }
        let pid = self.platform.id;
        if name.eq_ignore_ascii_case(pid) {
            return self.prelude_scopes.get(pid).copied();
        }
        None
    }

    /// Symbol inside a specific scope (current or previous pass).
    pub fn scope_member(&mut self, sid: usize, key: &str) -> Option<Value> {
        if let Some(sym) = self.scopes.get(sid).and_then(|s| s.syms.get(key)) {
            return Some(sym.value.clone());
        }
        if let Some(sym) = self.prev_scopes.get(sid).and_then(|s| s.syms.get(key)) {
            self.forward_used = true;
            return Some(sym.value.clone());
        }
        if let Some(&c) = self.scopes.get(sid).and_then(|s| s.named.get(key)) {
            return Some(Value::Scope(c));
        }
        None
    }

    /// Assign to an existing variable (Kick `.eval x = ...`).
    pub fn assign_var(&mut self, n: &Name, value: Value, span: Span) -> Result<(), Diagnostic> {
        let (fixed, key) = self.key(n);
        let mut sid = Some(if fixed != usize::MAX { fixed } else { self.cur_scope() });
        while let Some(s) = sid {
            if let Some(sym) = self.scopes[s].syms.get_mut(&key) {
                if !matches!(sym.kind, SymKind::Var | SymKind::Set) {
                    return Err(Diagnostic::error(format!("`{}` is not a variable", n.text), Some(span)));
                }
                sym.value = value;
                return Ok(());
            }
            sid = self.scopes[s].parent;
        }
        // rat/ACME: create on first assignment
        if self.dialect() != Dialect::Kick {
            self.define(n, value, SymKind::Var, span);
            return Ok(());
        }
        Err(Diagnostic::error(format!("unknown variable `{}` (declare it with .var)", n.text), Some(span)))
    }

    // -------------------------------------------------------- diagnostics

    pub fn err(&mut self, msg: impl Into<String>, span: Span) {
        self.diags.push(Diagnostic::error(msg, Some(span)));
    }

    pub fn warn(&mut self, msg: impl Into<String>, span: Span) {
        self.diags.push(Diagnostic::warning(msg, Some(span)));
    }

    // ------------------------------------------------------------ memory

    pub fn seg(&mut self) -> &mut Segment {
        let k = self.cur_seg.clone();
        self.segments.get_mut(&k).expect("current segment exists")
    }

    pub fn real_pc(&self) -> u32 {
        self.segments.get(&self.cur_seg).map_or(0, |s| s.pc)
    }

    pub fn pc(&self) -> u32 {
        let off: i64 = self.pc_offsets.last().copied().unwrap_or(0);
        (i64::from(self.real_pc()) + off) as u32
    }

    pub fn emit(&mut self, bytes: &[u8]) {
        if self.in_function > 0 {
            return;
        }
        let x = self.xor;
        if x != 0 {
            let v: Vec<u8> = bytes.iter().map(|b| b ^ x).collect();
            self.seg().emit(&v);
            self.emit_log.extend(v);
        } else {
            self.seg().emit(bytes);
            self.emit_log.extend_from_slice(bytes);
        }
    }

    pub fn ensure_segment(&mut self, name: &str, params: SegParams) {
        if !self.segments.contains_key(name) {
            let start = params.start.unwrap_or(0);
            let mut s = Segment::new(name, start);
            s.params = params;
            self.segments.insert(name.to_owned(), s);
        }
    }

    // --------------------------------------------------------- platform

    pub fn set_platform(&mut self, p: &'static Platform, span: Span) -> Result<(), Fatal> {
        self.platform = p;
        if let Some(c) = rat_cpu::by_name(p.cpu) {
            self.cpu = c;
        }
        // Amiga code lives in hunks from the start
        if p.default_format() == rat_output::Format::Hunk && self.cur_seg == "Default" && self.hunk_order.is_empty() {
            self.hunk_section("code", rat_output::HunkKind::Code, span);
        }
        self.load_prelude(span)
    }

    /// Run the platform prelude (macros) once per pass in its own scope.
    pub fn load_prelude(&mut self, _span: Span) -> Result<(), Fatal> {
        let p = self.platform;
        let Some(src) = p.prelude else { return Ok(()) };
        if self.prelude_scopes.contains_key(p.id) {
            return Ok(());
        }
        let path = format!("<prelude:{}>", p.id);
        let l = match self.load(&path, Some(Dialect::Rat), Some(src.to_owned())) {
            Ok(l) => l,
            Err(d) => return Err(Fatal(d)),
        };
        // the prelude scope hangs off the root
        let saved = std::mem::take(&mut self.scope_stack);
        self.scope_stack.push(ROOT);
        let sid = self.new_scope(Some(p.id));
        self.scope_stack.push(sid);
        self.prelude_scopes.insert(p.id, sid);
        let r = self.exec_file(l);
        self.scope_stack = saved;
        r
    }

    // ------------------------------------------------------------ passes

    pub fn reset_pass(&mut self) {
        self.prev_scopes = std::mem::take(&mut self.scopes);
        self.prev_anon = std::mem::take(&mut self.anon);
        self.scopes = vec![Scope::default()];
        self.scope_stack = vec![ROOT];
        self.cpu_stack.clear();
        self.cpu_state = CpuState::default();
        self.cpu_state_stack.clear();
        self.prelude_scopes.clear();
        self.endian = None;
        self.encoding = None;
        self.enc_stack.clear();
        self.convtab = None;
        self.segments.clear();
        self.pc_offsets.clear();
        self.zone = 0;
        self.zone_counter = 0;
        self.cheap = 0;
        self.site_counts.clear();
        self.diags.clear();
        self.changed = false;
        self.unresolved = false;
        self.forward_used = false;
        self.undefined.clear();
        self.prints.clear();
        self.pp_defines.clear();
        self.once.clear();
        self.file_stack.clear();
        self.dialect_stack.clear();
        self.outputs.clear();
        self.main_output = None;
        self.symbol_lists.clear();
        self.plat = PlatformState::default();
        self.listing.clear();
        self.emit_log.clear();
        self.breaks.clear();
        self.watches.clear();
        self.initmem = 0;
        self.xor = 0;
        self.depth = 0;
        self.fatal = None;
        self.hunk_order.clear();
        self.rng = 0x2545_f491_4f6c_dd1d;
    }

    /// Site key for sticky operand sizes.
    pub fn site(&mut self, s: &Stmt) -> (usize, u32) {
        let p = s as *const Stmt as usize;
        let c = self.site_counts.entry(p).or_insert(0);
        *c += 1;
        (p, *c)
    }

    // -------------------------------------------------------- execution

    pub fn exec_block(&mut self, stmts: &[Stmt]) -> R {
        for s in stmts {
            match self.exec_stmt(s)? {
                Flow::Normal => {}
                f => return Ok(f),
            }
        }
        Ok(Flow::Normal)
    }

    /// Execute a block in a fresh child scope.
    pub fn exec_scoped(&mut self, stmts: &[Stmt], name: Option<&str>) -> R {
        let sid = self.new_scope(name);
        self.scope_stack.push(sid);
        self.hoist(stmts);
        let r = self.exec_block(stmts);
        self.scope_stack.pop();
        r
    }

    pub fn eval_or_err(&mut self, e: &Expr) -> Option<Value> {
        match self.eval(e) {
            Ok(v) => Some(v),
            Err(d) => {
                self.diags.push(d);
                None
            }
        }
    }

    /// Evaluate to an integer; `None` (with `unresolved` set) when not yet
    /// known, error reported when not a number.
    pub fn eval_int(&mut self, e: &Expr) -> Option<i64> {
        let v = self.eval_or_err(e)?;
        match v {
            Value::Undef => {
                self.unresolved = true;
                None
            }
            v => match v.as_i64() {
                Some(n) => Some(n),
                None => {
                    self.err(format!("expected a number, found {}", v.type_name()), e.span);
                    None
                }
            },
        }
    }

    pub fn eval_str(&mut self, e: &Expr) -> Option<String> {
        match self.eval_or_err(e)? {
            Value::Str(s) => Some(s.to_string()),
            Value::Undef => {
                self.unresolved = true;
                None
            }
            v => Some(v.to_string()),
        }
    }

    pub fn eval_cond(&mut self, e: &Expr) -> Option<bool> {
        let v = self.eval_or_err(e)?;
        match v.truthy() {
            Some(b) => Some(b),
            None => {
                self.unresolved = true;
                None
            }
        }
    }

    pub fn encoding_name(&self) -> String {
        if let Some(e) = &self.encoding {
            return e.clone();
        }
        match self.dialect() {
            Dialect::Kick => "screencode_mixed".into(),
            Dialect::Acme => "ascii".into(),
            Dialect::Rat => self.platform.encoding.into(),
        }
    }

    pub fn encode_char(&self, ch: char) -> i64 {
        if let Some(t) = &self.convtab {
            if (ch as u32) < 256 {
                return i64::from(t[ch as usize]);
            }
        }
        i64::from(crate::encoding::encode_char(&self.encoding_name(), ch))
    }

    /// Emit a value as `size`-byte units.
    pub fn emit_value(&mut self, v: &Value, size: u8, endian: Endian, span: Span) {
        match v {
            Value::Undef => {
                self.unresolved = true;
                self.emit(&vec![0; size as usize]);
            }
            Value::Str(s) => {
                let enc = self.encoding_name();
                let bytes = crate::encoding::encode_str(&enc, s, self.convtab.as_deref());
                for b in bytes {
                    self.emit_value(&Value::Int(i64::from(b)), size, endian, span);
                }
            }
            Value::List(l) => {
                let items = l.borrow().clone();
                for x in &items {
                    self.emit_value(x, size, endian, span);
                }
            }
            v => {
                let Some(n) = v.as_i64() else {
                    self.err(format!("cannot emit a {}", v.type_name()), span);
                    return;
                };
                let bits = u32::from(size) * 8;
                if bits < 64 {
                    let lo = -(1i64 << (bits - 1));
                    let hi = (1i64 << bits) - 1;
                    if n < lo || n > hi {
                        self.err(format!("value {n} does not fit in {size} byte(s)"), span);
                    }
                }
                let bytes = n.to_le_bytes();
                let mut out = bytes[..size as usize].to_vec();
                if endian == Endian::Big {
                    out.reverse();
                }
                self.emit(&out);
            }
        }
    }

    pub fn data_endian(&self, explicit: Option<Endian>) -> Endian {
        explicit.or(self.endian).unwrap_or_else(|| self.cpu.endian())
    }

    fn record_listing(&mut self, s: &Stmt, pc: u32, start: usize) {
        if !self.opts.listing || self.in_function > 0 {
            return;
        }
        let bytes = self.emit_log[start..].to_vec();
        if bytes.is_empty() && !matches!(s.kind, StmtKind::Label(_)) {
            return;
        }
        let line = self.sources.get(s.span.file).map_or(0, |f| f.line_col(s.span.start).0);
        self.listing.push(ListLine { file: s.span.file, line, pc, bytes });
    }

    pub fn exec_stmt(&mut self, s: &Stmt) -> R {
        if let Some(d) = self.fatal.take() {
            return Err(Fatal(d));
        }
        let pc0 = self.pc();
        let log0 = self.emit_log.len();
        let r = crate::stmt::exec(self, s);
        if matches!(
            s.kind,
            StmtKind::Instr(_) | StmtKind::Data { .. } | StmtKind::Text { .. } | StmtKind::Hex(_) | StmtKind::Fill { .. } | StmtKind::Binary { .. }
        ) {
            self.record_listing(s, pc0, log0);
        }
        r
    }

    // ------------------------------------------------------ instructions

    pub fn assemble_instr(&mut self, s: &Stmt, ins: &Instr) -> R {
        let cpu = self.cpu;
        let mut operand: &[Token] = &ins.operand;
        let is65 = matches!(cpu.info().id, "6502" | "6502ni" | "6510" | "2a03" | "6507" | "65816");
        if is65 && operand.len() > 2 && operand[1].is_punct(":") {
            if let Tok::Ident(n) = &operand[0].tok {
                let at = i64::from(self.pc()) + 1;
                self.define(&Name::plain(n.clone()), Value::Int(at), SymKind::Label, s.span);
                operand = &operand[2..];
            }
        }
        let substituted = self.substitute_cmdargs(operand, ins.dialect);
        let lenient = ins.dialect == Dialect::Kick || substituted.as_slice() != operand;
        let toks = substituted;
        let site = self.site(s);
        let pc = self.pc();
        let st = self.cpu_state.clone();
        let mut ctx = InstrCtx {
            eng: self,
            toks,
            dialect: ins.dialect,
            pc,
            site,
            errors: Vec::new(),
            warns: Vec::new(),
            lenient,
        };
        let res = cpu.assemble(&ins.mnemonic, ins.suffix.as_deref(), &mut ctx, &st);
        let InstrCtx { errors, warns, .. } = ctx;
        for w in warns {
            self.warn(w, s.span);
        }
        match res {
            Ok(bytes) => {
                for e in errors {
                    self.err(e, s.span);
                }
                self.emit(&bytes);
            }
            Err(e) => self.err(e, s.span),
        }
        Ok(Flow::Normal)
    }

    /// An operand that is just a pseudocommand argument becomes that
    /// argument's tokens (Kick `lda arg` inside a pseudocommand).
    fn substitute_cmdargs(&mut self, toks: &[Token], dialect: Dialect) -> Vec<Token> {
        if toks.len() == 1 {
            if let Tok::Ident(n) = &toks[0].tok {
                if let Some(Value::CmdArg(c)) = self.lookup(&Name::plain(n.clone())) {
                    return crate::stmt::cmdarg_tokens(&c, toks[0].span);
                }
            }
        }
        // a function returning a CmdArgument: `lda _16bitnextArgument(arg)`
        let calls = toks.windows(2).any(|w| matches!(w[0].tok, Tok::Ident(_)) && w[1].is_punct("(") && !w[1].space_before);
        if calls && !toks.first().is_some_and(|t| t.is_punct("#") || t.is_punct("(")) {
            if let Ok(e) = rat_syntax::parse_expr_tokens(dialect, toks) {
                let before = self.diags.len();
                if let Ok(Value::CmdArg(c)) = self.eval(&e) {
                    return crate::stmt::cmdarg_tokens(&c, toks[0].span);
                }
                self.diags.truncate(before);
            }
        }
        toks.to_vec()
    }
}

fn display_key(k: &str) -> String {
    match k.find(['.', '@']) {
        Some(i) if k.starts_with('\u{1}') => k[i..].to_owned(),
        _ => k.to_owned(),
    }
}

pub(crate) struct InstrCtx<'a, 'o> {
    pub eng: &'a mut Engine<'o>,
    pub toks: Vec<Token>,
    pub dialect: Dialect,
    pub pc: u32,
    pub site: (usize, u32),
    pub errors: Vec<String>,
    pub warns: Vec<String>,
    pub lenient: bool,
}

impl OperandCtx for InstrCtx<'_, '_> {
    fn toks(&self) -> &[Token] {
        &self.toks
    }

    fn eval(&mut self, start: usize, end: usize) -> Result<Num, String> {
        let e = rat_syntax::parse_expr_tokens(self.dialect, &self.toks[start..end]).map_err(|d| d.message)?;
        self.eng.zp_hint = false;
        let saved = self.eng.dialect_stack.len();
        self.eng.dialect_stack.push(self.dialect);
        let v = self.eng.eval(&e);
        self.eng.dialect_stack.truncate(saved);
        let v = v.map_err(|d| d.message)?;
        Ok(match v {
            Value::Undef => {
                self.eng.unresolved = true;
                Num { value: 0, known: false, zp_hint: self.eng.zp_hint }
            }
            // a one-character string is that character (`#"?!#".charAt(1)`)
            Value::Str(s) if s.chars().count() == 1 => Num::known(self.eng.encode_char(s.chars().next().unwrap())),
            v => match v.as_i64() {
                Some(n) => Num::known(n),
                None => return Err(format!("expected a number, found {}", v.type_name())),
            },
        })
    }

    fn pc(&self) -> u32 {
        self.pc
    }

    fn min_size(&self) -> u8 {
        self.eng.sizes.get(&self.site).copied().unwrap_or(0)
    }

    fn grow(&mut self, size: u8) {
        let e = self.eng.sizes.entry(self.site).or_insert(0);
        if size > *e {
            *e = size;
        }
    }

    fn error(&mut self, msg: String) {
        self.errors.push(msg);
    }

    fn warn(&mut self, msg: String) {
        self.warns.push(msg);
    }

    fn lenient_immediates(&self) -> bool {
        self.lenient
    }
}
