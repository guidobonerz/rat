//! rat: the assembler. Runs the shared statement IR of all dialects over
//! as many passes as needed, then writes the platform's output formats.

use std::collections::{HashMap, HashSet};

use rat_core::{Diagnostic, Dialect, SourceMap};
use rat_output::Format;

mod encoding;
mod engine;
mod eval;
mod memory;
mod output;
mod platform;
mod stmt;
mod value;

pub use rat_output::symbols::{acme_labels, kick_sym, vice_labels, Symbol};

/// Where source and binary files come from.
pub trait Vfs {
    fn read(&self, path: &str) -> Option<Vec<u8>>;
    fn exists(&self, path: &str) -> bool {
        self.read(path).is_some()
    }
}

/// The host file system.
pub struct FsVfs;

impl Vfs for FsVfs {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(path).ok()
    }

    fn exists(&self, path: &str) -> bool {
        std::path::Path::new(path).is_file()
    }
}

/// In-memory files (REST API, tests). Paths use `/`.
#[derive(Default, Clone)]
pub struct MemVfs {
    pub files: HashMap<String, Vec<u8>>,
}

impl MemVfs {
    pub fn new() -> MemVfs {
        MemVfs::default()
    }

    pub fn add(&mut self, path: &str, data: impl Into<Vec<u8>>) {
        self.files.insert(normalize(path), data.into());
    }
}

impl Vfs for MemVfs {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.files.get(&normalize(path)).cloned()
    }
}

/// Collapse `./`, `../` and backslashes.
pub fn normalize(path: &str) -> String {
    let p = path.replace('\\', "/");
    let abs = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !abs {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    let j = parts.join("/");
    if abs {
        format!("/{j}")
    } else {
        j
    }
}

/// Join a directory and a relative path (absolute paths win).
pub fn join(dir: &str, name: &str) -> String {
    let n = name.replace('\\', "/");
    let is_abs = n.starts_with('/') || n.as_bytes().get(1) == Some(&b':');
    if dir.is_empty() || is_abs {
        return normalize(&n);
    }
    normalize(&format!("{dir}/{n}"))
}

#[derive(Clone, Debug)]
pub struct Options {
    /// Main source path (as known to the VFS).
    pub main: String,
    pub syntax: Option<Dialect>,
    pub cpu: Option<String>,
    pub platform: Option<String>,
    pub format: Option<String>,
    /// Main output file name.
    pub output: Option<String>,
    /// `-D name=value`.
    pub defines: Vec<(String, String)>,
    pub include_dirs: Vec<String>,
    pub max_passes: u32,
    pub listing: bool,
    /// Internal: load addresses of Amiga hunks.
    pub hunk_bases: Vec<u32>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            main: String::new(),
            syntax: None,
            cpu: None,
            platform: None,
            format: None,
            output: None,
            defines: Vec::new(),
            include_dirs: Vec::new(),
            max_passes: 20,
            listing: false,
            hunk_bases: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Output {
    pub name: String,
    pub format: String,
    #[serde(skip)]
    pub data: Vec<u8>,
}

/// The addresses of a source line: what a statement of `file`, line `line` (1-based) emitted at `addr`.
#[derive(Clone, Debug, serde::Serialize)]
pub struct LineInfo {
    pub file: String,
    pub line: usize,
    pub addr: u32,
    pub size: usize,
}

#[derive(Debug)]
pub struct AsmResult {
    pub ok: bool,
    pub diagnostics: Vec<Diagnostic>,
    pub sources: SourceMap,
    pub outputs: Vec<Output>,
    pub symbols: Vec<Symbol>,
    pub listing: String,
    /// The source line of every address (recorded with `Options::listing`).
    pub lines: Vec<LineInfo>,
    pub prints: Vec<String>,
    pub breakpoints: Vec<u32>,
    pub passes: u32,
    pub cpu: String,
    pub platform: String,
}

impl AsmResult {
    pub fn errors(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.is_error()).count()
    }

    /// All diagnostics rendered rustc-style.
    pub fn render_diagnostics(&self, color: bool) -> String {
        self.diagnostics.iter().map(|d| d.render(&self.sources, color)).collect()
    }
}

/// Default platform of a dialect when none is given: KickAssembler and
/// ACME sources are C64 sources; rat starts generic.
fn default_platform(d: Dialect) -> &'static str {
    match d {
        Dialect::Kick | Dialect::Acme => "c64",
        Dialect::Rat => "generic",
    }
}

/// Assemble `opts.main`.
pub fn assemble(opts: &Options, vfs: &dyn Vfs) -> AsmResult {
    let mut r = assemble_once(opts, vfs);
    // Amiga: find relocations by moving one hunk at a time
    if r.ok && r.outputs.is_empty() && r.platform.starts_with("amiga") && main_is_hunk(opts, &r) {
        r = assemble_hunks(opts, vfs, r);
    }
    r
}

fn main_is_hunk(opts: &Options, r: &AsmResult) -> bool {
    match &opts.format {
        Some(f) => Format::parse(f) == Some(Format::Hunk),
        None => rat_platform::by_name(&r.platform).is_some_and(|p| p.default_format() == Format::Hunk),
    }
}

struct HunkRun {
    hunks: Vec<(rat_output::HunkKind, Vec<u8>, u32, u32)>,
}

fn hunk_run(opts: &Options, vfs: &dyn Vfs, bases: Vec<u32>) -> Result<(HunkRun, AsmResult), Box<AsmResult>> {
    let mut o = opts.clone();
    o.hunk_bases = bases;
    let mut e = engine::Engine::new(&o, vfs);
    e.hunk_bases = o.hunk_bases.clone();
    let res = drive(&mut e);
    if !res.ok {
        return Err(Box::new(res));
    }
    let mut hunks = Vec::new();
    for (i, name) in e.hunk_order.iter().enumerate() {
        let s = &e.segments[name];
        let base = e.hunk_base(i);
        let chunks = s.chunks();
        let end = s.end().max(base);
        let data = rat_output::flatten(&chunks, base, rat_output::extent(&chunks).map_or(base, |x| x.1), 0);
        hunks.push((s.hunk.unwrap_or(rat_output::HunkKind::Code), data, base, end - base));
    }
    Ok((HunkRun { hunks }, res))
}

fn assemble_hunks(opts: &Options, vfs: &dyn Vfs, first: AsmResult) -> AsmResult {
    let n = {
        let (run0, _) = match hunk_run(opts, vfs, Vec::new()) {
            Ok(x) => x,
            Err(r) => return *r,
        };
        run0.hunks.len()
    };
    let bases: Vec<u32> = (0..n).map(|i| output::HUNK_BASE + i as u32 * output::HUNK_STRIDE).collect();
    let (base_run, mut res) = match hunk_run(opts, vfs, bases.clone()) {
        Ok(x) => x,
        Err(r) => return *r,
    };
    let mut relocs: Vec<Vec<(u32, Vec<u32>)>> = vec![Vec::new(); n];
    let mut fixed: Vec<Vec<u8>> = base_run.hunks.iter().map(|h| h.1.clone()).collect();
    for t in 0..n {
        let mut moved = bases.clone();
        moved[t] += output::HUNK_SHIFT;
        let (run, _) = match hunk_run(opts, vfs, moved) {
            Ok(x) => x,
            Err(r) => return *r,
        };
        for h in 0..n {
            let a = &base_run.hunks[h].1;
            let b = &run.hunks[h].1;
            if a.len() != b.len() {
                res.diagnostics.push(Diagnostic::error("code size depends on the load address; cannot relocate", None));
                res.ok = false;
                return res;
            }
            let mut offs = Vec::new();
            let mut i = 0;
            while i + 4 <= a.len() {
                let va = u32::from_be_bytes([a[i], a[i + 1], a[i + 2], a[i + 3]]);
                let vb = u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
                if vb.wrapping_sub(va) == output::HUNK_SHIFT && va.wrapping_sub(bases[t]) < output::HUNK_STRIDE {
                    offs.push(i as u32);
                    let rel = va.wrapping_sub(bases[t]);
                    fixed[h][i..i + 4].copy_from_slice(&rel.to_be_bytes());
                    i += 4;
                } else {
                    i += 1;
                }
            }
            if !offs.is_empty() {
                relocs[h].push((t as u32, offs));
            }
        }
    }
    let hunks: Vec<rat_output::Hunk> = base_run
        .hunks
        .iter()
        .enumerate()
        .map(|(i, h)| rat_output::Hunk { kind: h.0, data: if h.0 == rat_output::HunkKind::Bss { Vec::new() } else { fixed[i].clone() }, bss_size: h.3, relocs: relocs[i].clone() })
        .collect();
    let name = opts.output.clone().unwrap_or_else(|| {
        let file = opts.main.rsplit(['/', '\\']).next().unwrap_or(&opts.main);
        format!("{}.exe", file.rsplit_once('.').map_or(file, |x| x.0))
    });
    res.outputs.insert(0, Output { name, format: "hunk".into(), data: rat_output::hunk(&hunks) });
    let _ = first;
    res
}

fn assemble_once(opts: &Options, vfs: &dyn Vfs) -> AsmResult {
    let mut e = engine::Engine::new(opts, vfs);
    e.hunk_bases = opts.hunk_bases.clone();
    drive(&mut e)
}

fn drive(e: &mut engine::Engine<'_>) -> AsmResult {
    let opts = e.opts;
    let main = normalize(&opts.main);
    let main_loaded = match e.load(&main, opts.syntax, None) {
        Ok(l) => Some(l),
        Err(d) => {
            e.load_errors.push(d);
            None
        }
    };
    let mut passes = 0;
    let mut converged = false;
    let mut last_undef: HashSet<String> = HashSet::new();
    if let Some(main_l) = &main_loaded {
        for pass in 1..=opts.max_passes.max(1) {
            passes = pass;
            e.pass = pass;
            e.reset_pass();
            e.dialect_stack.push(main_l.dialect);
            let pname = opts.platform.clone().unwrap_or_else(|| default_platform(main_l.dialect).into());
            let plat = match rat_platform::by_name(&pname) {
                Some(p) => p,
                None => {
                    e.load_errors.push(Diagnostic::error(format!("unknown platform `{pname}`"), None));
                    break;
                }
            };
            e.platform = plat;
            e.cpu = rat_cpu::by_name(plat.cpu).unwrap();
            if let Some(c) = &opts.cpu {
                match rat_cpu::by_name(c) {
                    Some(c) => e.cpu = c,
                    None => {
                        e.load_errors.push(Diagnostic::error(format!("unknown CPU `{c}` (supported: {})", rat_cpu::CPU_IDS.join(", ")), None));
                        break;
                    }
                }
            }
            let org = plat.default_org.unwrap_or(0);
            e.ensure_segment("Default", memory::SegParams { start: Some(org), ..Default::default() });
            e.cur_seg = "Default".into();
            if plat.default_format() == Format::Hunk {
                e.hunk_section("code", rat_output::HunkKind::Code, rat_core::Span::default());
            }
            let cpu_override = opts.cpu.is_some();
            let saved_cpu = e.cpu;
            if let Err(f) = e.load_prelude(rat_core::Span::default()) {
                e.diags.push(f.0);
            }
            if cpu_override {
                e.cpu = saved_cpu;
            }
            e.define_globals();
            if let Err(f) = e.exec_file(main_l.clone()) {
                e.diags.push(f.0);
                break;
            }
            let undef: HashSet<String> = e.undefined.iter().map(|u| u.0.clone()).collect();
            if !e.unresolved && (pass > 1 && !e.changed || pass == 1 && !e.forward_used) {
                converged = true;
                break;
            }
            if pass > 2 && !e.changed && undef == last_undef {
                break;
            }
            last_undef = undef;
        }
    }
    if main_loaded.is_some() && !converged && e.diags.iter().all(|d| !d.is_error()) {
        let mut seen = HashSet::new();
        let und: Vec<_> = e.undefined.clone();
        for (n, span) in und {
            if seen.insert((n.clone(), span)) {
                e.diags.push(Diagnostic::error(format!("undefined symbol `{n}`"), Some(span)));
            }
        }
        if seen.is_empty() {
            e.diags.push(Diagnostic::error(format!("values did not settle after {passes} passes"), None));
        }
    }
    if main_loaded.is_some() {
        e.check_segments();
    }
    let mut outputs = Vec::new();
    let mut diags: Vec<Diagnostic> = e.load_errors.clone();
    let pre_errors = diags.iter().chain(e.diags.iter()).any(Diagnostic::is_error);
    if main_loaded.is_some() && !pre_errors {
        outputs = e.build_outputs();
    }
    diags.extend(e.diags.iter().cloned());
    // dedupe identical diagnostics (loops report once)
    let mut seen = HashSet::new();
    diags.retain(|d| seen.insert((d.message.clone(), d.span)));
    let ok = !diags.iter().any(Diagnostic::is_error);
    if !ok {
        outputs.clear();
    }
    let symbols = if main_loaded.is_some() { e.symbols() } else { Vec::new() };
    // symbol list files requested by the source (ACME !sl)
    for f in e.symbol_lists.clone() {
        outputs.push(Output { name: f, format: "labels".into(), data: acme_labels(&symbols).into_bytes() });
    }
    let listing = listing(e);
    let lines = e
        .listing
        .iter()
        .filter(|l| !l.bytes.is_empty())
        .map(|l| LineInfo { file: e.sources.get(l.file).map_or(String::new(), |f| f.name.clone()), line: l.line, addr: l.pc, size: l.bytes.len() })
        .collect();
    AsmResult {
        ok,
        diagnostics: diags,
        sources: e.sources.clone(),
        outputs,
        symbols,
        listing,
        lines,
        prints: e.prints.clone(),
        breakpoints: e.breaks.clone(),
        passes,
        cpu: e.cpu.info().id.into(),
        platform: e.platform.id.into(),
    }
}

fn listing(e: &engine::Engine<'_>) -> String {
    let mut s = String::new();
    for l in &e.listing {
        let src = e.sources.get(l.file).map_or("", |f| f.line_text(l.line));
        let mut first = true;
        let chunks: Vec<&[u8]> = if l.bytes.is_empty() { vec![&[][..]] } else { l.bytes.chunks(8).collect() };
        for (i, c) in chunks.iter().enumerate() {
            let hex: Vec<String> = c.iter().map(|b| format!("{b:02x}")).collect();
            let addr = l.pc as usize + i * 8;
            if first {
                s.push_str(&format!("{addr:06x}  {:<24} {:5}  {src}\n", hex.join(" "), l.line));
                first = false;
            } else {
                s.push_str(&format!("{addr:06x}  {}\n", hex.join(" ")));
            }
        }
    }
    s
}

/// Disassemble `bytes` loaded at `org` with a CPU, in rat syntax.
pub fn disassemble(cpu: &str, bytes: &[u8], org: u32, m16: bool, x16: bool) -> Result<Vec<(u32, Vec<u8>, String)>, String> {
    let c = rat_cpu::by_name(cpu).ok_or_else(|| format!("unknown CPU `{cpu}`"))?;
    let st = rat_cpu::CpuState { m16, x16, dp: None };
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let addr = org.wrapping_add(i as u32);
        match c.disassemble(&bytes[i..], addr, &st) {
            Some((n, text)) if n > 0 => {
                out.push((addr, bytes[i..i + n].to_vec(), text));
                i += n;
            }
            _ => {
                let step = if c.endian() == rat_core::Endian::Big && c.info().addr_bits == 24 { 2.min(bytes.len() - i) } else { 1 };
                let v: Vec<u8> = bytes[i..i + step].to_vec();
                let text = if step == 2 { format!("dc.w ${:02x}{:02x}", v[0], v[1]) } else { format!(".byte ${:02x}", v[0]) };
                out.push((addr, v, text));
                i += step;
            }
        }
    }
    Ok(out)
}

mod convert;
pub use convert::{convert, Converted};
