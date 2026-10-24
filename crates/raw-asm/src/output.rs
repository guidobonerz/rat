//! Turning the final pass's memory into output files.

use raw_core::{Diagnostic, Dialect, Span};
use raw_output::{Chunk, Format};

use crate::engine::{Engine, OutReq, SymKind};
use crate::{Output, Symbol};

/// Amiga hunks are assembled at high, distinct bases so absolute
/// references never shrink to `.w`; relocations are found by assembling
/// again with one hunk moved.
pub const HUNK_BASE: u32 = 0x1000_0000;
pub const HUNK_STRIDE: u32 = 0x0100_0000;
pub const HUNK_SHIFT: u32 = 0x0080_0000;

/// Main output format: CLI flag, then `!to`/`.output`, then the file
/// extension, then the platform default.
pub(crate) fn main_format(e: &Engine<'_>) -> Result<Format, String> {
    let req = e
        .opts
        .format
        .clone()
        .or_else(|| e.main_output.as_ref().and_then(|m| m.1.clone()))
        .or_else(|| {
            let name = e.opts.output.clone().or_else(|| e.main_output.as_ref().map(|m| m.0.clone()))?;
            let ext = name.rsplit_once('.')?.1.to_owned();
            Format::parse(&ext).map(|f| f.name().to_owned())
        });
    let f = match req {
        Some(r) => match r.as_str() {
            "plain" => Format::Bin,
            "cbm" => Format::Prg,
            "apple" => Format::Bin,
            r => Format::parse(r).ok_or_else(|| format!("unknown output format `{r}`"))?,
        },
        None => e.platform.default_format(),
    };
    if !e.platform.allows(f) {
        let owners: Vec<&str> = raw_platform::PLATFORMS.iter().filter(|p| p.formats.contains(&f)).map(|p| p.id).collect();
        return Err(format!(
            "output format `{}` is not available on platform {} (allowed: {}); it belongs to {}",
            f.name(),
            e.platform.id,
            e.platform.all_formats().iter().map(|f| f.name()).collect::<Vec<_>>().join(", "),
            if owners.is_empty() { "no platform".into() } else { owners.join(", ") }
        ));
    }
    Ok(f)
}

fn base_name(path: &str) -> String {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match file.rsplit_once('.') {
        Some((b, _)) => b.to_owned(),
        None => file.to_owned(),
    }
}

/// Main output file name.
pub(crate) fn main_name(e: &Engine<'_>, f: Format) -> String {
    if let Some(o) = &e.opts.output {
        return o.clone();
    }
    if let Some((n, _)) = &e.main_output {
        return n.clone();
    }
    format!("{}.{}", base_name(&e.opts.main), f.extension())
}

impl Engine<'_> {
    /// Segments that go into the main file.
    fn main_chunks(&self) -> Vec<Chunk> {
        let referenced: Vec<&String> = self
            .outputs
            .iter()
            .flat_map(|o| match o {
                OutReq::File { segments, .. } => segments.iter().collect::<Vec<_>>(),
                OutReq::Disk { files, .. } => files.iter().flat_map(|f| f.2.iter()).collect(),
            })
            .collect();
        let kick = self.files.get(&self.opts.main).is_some_and(|l| l.dialect == Dialect::Kick);
        let mut out = Vec::new();
        for (name, s) in &self.segments {
            let user = !name.starts_with('\u{1}');
            let include = if name == "Default" {
                true
            } else if !user {
                false
            } else {
                !kick
                    && !s.params.virtual_
                    && s.params.out_prg.is_none()
                    && s.params.out_bin.is_none()
                    && !referenced.contains(&name)
                    && !self.segments.values().any(|o| o.params.segments.contains(name))
            };
            if include {
                out.extend(s.chunks());
                for inc in &s.params.segments {
                    if let Some(o) = self.segments.get(inc) {
                        out.extend(o.chunks());
                    }
                }
            }
        }
        out
    }

    fn segs_chunks(&self, names: &[String], span: Span) -> Result<Vec<Chunk>, Diagnostic> {
        let mut v = Vec::new();
        for n in names {
            let s = self.segments.get(n).ok_or_else(|| Diagnostic::error(format!("unknown segment `{n}`"), Some(span)))?;
            v.extend(s.chunks());
            for inc in &s.params.segments {
                if let Some(o) = self.segments.get(inc) {
                    v.extend(o.chunks());
                }
            }
        }
        Ok(v)
    }

    fn fill_of(&self) -> u8 {
        self.segments.get("Default").and_then(|s| s.params.fill).unwrap_or(self.initmem)
    }

    /// Write `chunks` in format `f`.
    pub(crate) fn write(&self, f: Format, chunks: &[Chunk], name: &str) -> Result<Vec<u8>, String> {
        let fill = self.fill_of();
        Ok(match f {
            Format::Bin => raw_output::bin(chunks, fill),
            Format::Prg => raw_output::prg(chunks, fill),
            Format::Hex => raw_output::intel_hex(chunks),
            Format::Srec => raw_output::srec(chunks, &base_name(name)),
            Format::Xex => raw_output::xex(chunks, &self.plat.xex_inits, self.plat.xex_run),
            Format::P => raw_output::zx81_p(chunks)?,
            Format::Amsdos => {
                let Some((lo, hi)) = raw_output::extent(chunks) else { return Ok(Vec::new()) };
                let data = raw_output::flatten(chunks, lo, hi, fill);
                let (n, exec) = self.plat.amsdos.clone().unwrap_or_default();
                let n = if n.is_empty() { base_name(name) } else { n };
                raw_output::amsdos(&n, lo as u16, exec.unwrap_or(lo as u16), &data)
            }
            Format::Sfc => {
                let map = self.plat.snes_map.unwrap_or(raw_output::SnesMap::LoRom);
                raw_output::sfc(chunks, self.plat.snes.as_ref(), map)?
            }
            Format::Nes => self.nes()?,
            Format::A26 => self.a26()?,
            Format::Crt => {
                let Some((lo, hi)) = raw_output::extent(chunks) else { return Ok(Vec::new()) };
                let data = raw_output::flatten(chunks, lo, hi, 0xff);
                let mut info = self.plat.crt.clone().unwrap_or(raw_output::CrtInfo { hw_type: 0, exrom: 0, game: 1, name: base_name(name).to_uppercase() });
                if self.plat.crt.is_none() && data.len() > 0x2000 {
                    info.game = 0;
                }
                let chips: Vec<(u16, u16, Vec<u8>)> = data
                    .chunks(0x2000)
                    .enumerate()
                    .map(|(i, c)| {
                        let mut c = c.to_vec();
                        c.resize(0x2000, 0xff);
                        (0, (lo as usize + i * 0x2000) as u16, c)
                    })
                    .collect();
                raw_output::crt(&info, &chips)
            }
            Format::D64 => {
                let data = raw_output::prg(chunks, fill);
                let fname = base_name(name).to_uppercase();
                raw_output::d64::d64(&fname, "01", &[raw_output::d64::DiskFile { name: fname.clone(), kind: 0x82, data }])?
            }
            Format::Hunk => return Err("internal: hunk output is built by the driver".into()),
        })
    }

    fn banks(&self, prefix: &str, size: u32) -> Vec<(u32, Vec<u8>)> {
        let mut v: Vec<(u32, Vec<u8>)> = self
            .segments
            .iter()
            .filter(|(n, _)| n.starts_with(&format!("\u{1}{prefix}")))
            .map(|(_, s)| {
                let start = s.params.start.unwrap_or(0) & !(size - 1);
                (s.bank.unwrap_or(0), raw_output::flatten(&s.chunks(), start, start + size, 0xff))
            })
            .collect();
        v.sort_by_key(|b| b.0);
        v
    }

    fn nes(&self) -> Result<Vec<u8>, String> {
        let mut h = self.plat.ines.clone().unwrap_or_default();
        let banks = self.banks("prg", 0x4000);
        let prg = if !banks.is_empty() {
            if self.plat.ines.is_none() {
                h.prg_banks = banks.len() as u8;
            }
            banks.into_iter().flat_map(|b| b.1).collect::<Vec<u8>>()
        } else {
            let chunks = self.main_chunks();
            let lo = raw_output::extent(&chunks).map_or(0xc000, |e| e.0);
            if self.plat.ines.is_none() {
                h.prg_banks = if lo >= 0xc000 { 1 } else { 2 };
            }
            let start = 0x1_0000 - u32::from(h.prg_banks) * 0x4000;
            if lo < start {
                return Err(format!("PRG code starts at ${lo:04x}, below ${start:04x} for {} PRG bank(s)", h.prg_banks));
            }
            raw_output::flatten(&chunks, start, 0x1_0000, 0xff)
        };
        let chr = match self.segments.get("\u{1}chr") {
            Some(s) => {
                let c = s.chunks();
                let hi = raw_output::extent(&c).map_or(0, |e| e.1);
                if self.plat.ines.is_none() {
                    h.chr_banks = hi.div_ceil(0x2000) as u8;
                }
                raw_output::flatten(&c, 0, u32::from(h.chr_banks) * 0x2000, 0)
            }
            None => {
                if self.plat.ines.is_none() {
                    h.chr_banks = 0;
                }
                Vec::new()
            }
        };
        Ok(raw_output::ines(&h, &prg, &chr))
    }

    fn a26(&self) -> Result<Vec<u8>, String> {
        let banks = self.banks("bank", 0x1000);
        if !banks.is_empty() {
            return Ok(raw_output::a26(&banks.into_iter().map(|b| b.1).collect::<Vec<_>>()));
        }
        let chunks = self.main_chunks();
        let Some((lo, hi)) = raw_output::extent(&chunks) else { return Ok(Vec::new()) };
        let top = (hi - 1) | 0x0fff;
        let size = if top + 1 - lo <= 0x800 { 0x800 } else { 0x1000 };
        if top + 1 - lo > 0x1000 {
            return Err("2600 code is larger than 4K; use `.bank n` for bank switching".into());
        }
        Ok(raw_output::flatten(&chunks, top + 1 - size, top + 1, 0xff))
    }

    /// Outputs of this (final) pass, except Amiga hunks.
    pub(crate) fn build_outputs(&mut self) -> Vec<Output> {
        let mut outs = Vec::new();
        let span = Span::default();
        match main_format(self) {
            Ok(Format::Hunk) => {}
            Ok(f) => {
                let name = main_name(self, f);
                let chunks = self.main_chunks();
                let skip_main = chunks.is_empty() && !self.outputs.is_empty();
                if !skip_main {
                    match self.write(f, &chunks, &name) {
                        Ok(data) => outs.push(Output { name, format: f.name().into(), data }),
                        Err(m) => self.diags.push(Diagnostic::error(m, None)),
                    }
                }
            }
            Err(m) => self.diags.push(Diagnostic::error(m, None)),
        }
        // segments with outPrg / outBin
        let seg_files: Vec<(String, Format, String)> = self
            .segments
            .iter()
            .flat_map(|(n, s)| {
                let mut v = Vec::new();
                if let Some(p) = &s.params.out_prg {
                    v.push((n.clone(), Format::Prg, p.clone()));
                }
                if let Some(b) = &s.params.out_bin {
                    v.push((n.clone(), Format::Bin, b.clone()));
                }
                v
            })
            .collect();
        for (seg, f, file) in seg_files {
            if !self.platform.allows(f) {
                self.diags.push(Diagnostic::error(format!("output format `{}` is not available on platform {}", f.name(), self.platform.id), None));
                continue;
            }
            match self.segs_chunks(&[seg], span).map_err(|d| d.message).and_then(|c| self.write(f, &c, &file)) {
                Ok(data) => outs.push(Output { name: file, format: f.name().into(), data }),
                Err(m) => self.diags.push(Diagnostic::error(m, None)),
            }
        }
        let reqs = self.outputs.clone();
        for r in reqs {
            match r {
                OutReq::File { name, segments, format, span } => {
                    let f = match format.as_deref().and_then(Format::parse).or_else(|| name.rsplit_once('.').and_then(|x| Format::parse(x.1))) {
                        Some(f) => f,
                        None => Format::Prg.min_for(self.platform),
                    };
                    if !self.platform.allows(f) {
                        self.diags.push(Diagnostic::error(format!("output format `{}` is not available on platform {}", f.name(), self.platform.id), Some(span)));
                        continue;
                    }
                    match self.segs_chunks(&segments, span) {
                        Ok(c) => match self.write(f, &c, &name) {
                            Ok(data) => outs.push(Output { name, format: f.name().into(), data }),
                            Err(m) => self.diags.push(Diagnostic::error(m, Some(span))),
                        },
                        Err(d) => self.diags.push(d),
                    }
                }
                OutReq::Disk { name, disk_name, id, files, span } => {
                    let mut dfiles = Vec::new();
                    let mut ok = true;
                    for (fname, kind, segs) in files {
                        let segs = if segs.is_empty() { vec!["Default".to_owned()] } else { segs };
                        let chunks = match self.segs_chunks(&segs, span) {
                            Ok(c) => c,
                            Err(d) => {
                                self.diags.push(d);
                                ok = false;
                                continue;
                            }
                        };
                        let kl = kind.to_ascii_lowercase();
                        let locked = kl.ends_with('<');
                        let base = match kl.trim_end_matches('<') {
                            "seq" => 0x81,
                            "usr" => 0x83,
                            "rel" => 0x84,
                            "del" => 0x80,
                            _ => 0x82,
                        };
                        let data = if base == 0x82 { raw_output::prg(&chunks, 0) } else { raw_output::bin(&chunks, 0) };
                        let k = if locked { base | 0x40 } else { base };
                        dfiles.push(raw_output::d64::DiskFile { name: fname, kind: k, data });
                    }
                    if ok {
                        match raw_output::d64::d64(&disk_name, &id, &dfiles) {
                            Ok(data) => outs.push(Output { name, format: "d64".into(), data }),
                            Err(m) => self.diags.push(Diagnostic::error(m, Some(span))),
                        }
                    }
                }
            }
        }
        outs
    }

    /// Exported symbols of the final pass (labels and constants).
    pub(crate) fn symbols(&self) -> Vec<Symbol> {
        let mut out = Vec::new();
        let mut stack = vec![(0usize, String::new())];
        let mut seen = std::collections::HashSet::new();
        while let Some((sid, prefix)) = stack.pop() {
            if !seen.insert(sid) {
                continue;
            }
            let s = &self.scopes[sid];
            let mut names: Vec<&String> = s.syms.keys().collect();
            names.sort();
            for n in names {
                let sym = &s.syms[n];
                if n.starts_with('\u{1}') || n == "cmdLineVars" {
                    continue;
                }
                if !matches!(sym.kind, SymKind::Label | SymKind::Const | SymKind::Set) {
                    continue;
                }
                if let crate::value::Value::Int(v) = sym.value {
                    out.push(Symbol { name: format!("{prefix}{n}"), value: v, is_label: sym.kind == SymKind::Label });
                }
            }
            for (n, &c) in &s.named {
                if self.prelude_scopes.values().any(|&p| p == c) {
                    continue;
                }
                stack.push((c, format!("{prefix}{n}.")));
            }
        }
        out.sort_by(|a, b| a.value.cmp(&b.value).then(a.name.cmp(&b.name)));
        out
    }

    /// Check overlaps and segment limits.
    pub(crate) fn check_segments(&mut self) {
        let mut errs = Vec::new();
        for s in self.segments.values() {
            let n = &s.name;
            if !s.params.allow_overlap {
                if let Some((a, x, y)) = s.overlap() {
                    let what = match (x, y) {
                        (Some(x), Some(y)) => format!(" (memory blocks `{x}` and `{y}`)"),
                        _ => String::new(),
                    };
                    let seg = if n.starts_with('\u{1}') { String::new() } else { format!(" in segment {n}") };
                    errs.push(format!("memory overlap at ${a:04x}{seg}{what}"));
                }
            }
            for b in &s.blocks {
                if b.data.is_empty() {
                    continue;
                }
                let end = b.start + b.data.len() as u32 - 1;
                if s.params.min.is_some_and(|m| b.start < m) || s.params.max.is_some_and(|m| end > m) {
                    errs.push(format!("segment {n}: ${:04x}-${end:04x} outside its min/max range", b.start));
                }
            }
        }
        for m in errs {
            self.diags.push(Diagnostic::error(m, None));
        }
    }
}

trait FormatExt {
    fn min_for(self, p: &raw_platform::Platform) -> Format;
}

impl FormatExt for Format {
    /// `self` if the platform allows it, else the platform default.
    fn min_for(self, p: &raw_platform::Platform) -> Format {
        if p.allows(self) {
            self
        } else {
            p.default_format()
        }
    }
}
