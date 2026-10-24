//! Platform directives (`.ines`, `.xex.run`, `.snes.header`, ...).

use raw_core::*;
use raw_platform::Platform;

use crate::engine::{Engine, Fatal, Flow, R};
use crate::memory::SegParams;
use crate::value::Value;

fn get<'a>(args: &'a [Arg], key: &str, pos: usize) -> Option<&'a Expr> {
    args.iter()
        .find(|a| a.key.as_deref().is_some_and(|k| k.eq_ignore_ascii_case(key)))
        .or_else(|| args.iter().filter(|a| a.key.is_none()).nth(pos))
        .map(|a| &a.value)
}

impl Engine<'_> {
    fn arg_int(&mut self, args: &[Arg], key: &str, pos: usize) -> Option<i64> {
        let e = get(args, key, pos)?.clone();
        self.eval_int(&e)
    }

    fn arg_str(&mut self, args: &[Arg], key: &str, pos: usize) -> Option<String> {
        let e = get(args, key, pos)?.clone();
        // bare words (`map=lorom`, `section data, bss`) are names
        if let Some(w) = e.as_ident() {
            if self.lookup(&Name::plain(w)).is_none() {
                return Some(w.to_owned());
            }
        }
        self.eval_str(&e)
    }

    fn arg_bool(&mut self, args: &[Arg], key: &str, pos: usize) -> bool {
        let Some(e) = get(args, key, pos).cloned() else { return false };
        self.eval_cond(&e).unwrap_or(false)
    }

    /// Call a macro of a platform prelude (`BasicUpstart2`).
    fn prelude_macro(&mut self, plat: &'static Platform, name: &str, args: Vec<Value>, span: Span) -> R {
        let saved = self.platform;
        self.platform = plat;
        let r = self.load_prelude(span);
        self.platform = saved;
        r?;
        let sid = self.prelude_scopes[plat.id];
        match self.scope_member(sid, name) {
            Some(Value::Macro(m)) => self.call_macro(&m, args, Vec::new(), None, span),
            _ => Err(Fatal(Diagnostic::error(format!("platform {} has no macro `{name}`", plat.id), Some(span)))),
        }
    }

    fn bank_segment(&mut self, prefix: &str, n: i64, addr: u32, span: Span) {
        let name = format!("\u{1}{prefix}{n}");
        if !self.segments.contains_key(&name) {
            self.ensure_segment(&name, SegParams { start: Some(addr), ..Default::default() });
            self.segments[&name].bank = Some(n as u32);
        }
        self.pc_offsets.clear();
        self.switch_segment(&name, None, span);
    }
}

pub(crate) fn directive(e: &mut Engine<'_>, plat: &'static Platform, name: &str, args: &[Arg], body: Option<&Block>, span: Span) -> R {
    match (plat.id, name.to_ascii_lowercase().as_str()) {
        (_, "basicupstart") => {
            let a = e.arg_int(args, "address", 0).map_or(Value::Undef, Value::Int);
            return e.prelude_macro(plat, "BasicUpstart2", vec![a], span);
        }
        ("c64", "crt") => {
            let hw = e.arg_int(args, "type", 0).unwrap_or(0) as u16;
            let name = e.arg_str(args, "name", 1).unwrap_or_else(|| "RAW".into());
            let exrom = e.arg_int(args, "exrom", 2).unwrap_or(0) as u8;
            let game = e.arg_int(args, "game", 3).unwrap_or(1) as u8;
            e.plat.crt = Some(raw_output::CrtInfo { hw_type: hw, exrom, game, name });
        }
        ("atari8", "run") => e.plat.xex_run = e.arg_int(args, "address", 0).map(|v| v as u16),
        ("atari8", "init") => {
            if let Some(a) = e.arg_int(args, "address", 0) {
                let at = e.real_pc().saturating_sub(1);
                e.plat.xex_inits.push((at, a as u16));
            }
        }
        ("atari2600", "bank") | ("nes", "bank") => {
            let n = e.arg_int(args, "bank", 0).unwrap_or(0);
            let default = if plat.id == "nes" { 0x8000 } else { 0xf000 };
            let addr = e.arg_int(args, "address", 1).unwrap_or(default) as u32;
            let prefix = if plat.id == "nes" { "prg" } else { "bank" };
            e.bank_segment(prefix, n, addr, span);
        }
        ("nes", "ines") => {
            let mut h = raw_output::InesHeader::default();
            if let Some(m) = e.arg_int(args, "mapper", 0) {
                h.mapper = m as u16;
            }
            if let Some(p) = e.arg_int(args, "prg", 1) {
                h.prg_banks = p as u8;
            }
            if let Some(c) = e.arg_int(args, "chr", 2) {
                h.chr_banks = c as u8;
            }
            if let Some(m) = e.arg_str(args, "mirroring", 3) {
                h.vertical_mirroring = matches!(m.to_ascii_lowercase().as_str(), "v" | "vertical" | "1");
                h.four_screen = matches!(m.to_ascii_lowercase().as_str(), "4" | "four");
            }
            h.battery = e.arg_bool(args, "battery", 4);
            h.nes2 = e.arg_bool(args, "nes2", 5);
            e.plat.ines = Some(h);
        }
        ("nes", "chr") => {
            let seg = "\u{1}chr".to_string();
            e.ensure_segment(&seg, SegParams { start: Some(0), ..Default::default() });
            let saved = e.cur_seg.clone();
            let saved_off = std::mem::take(&mut e.pc_offsets);
            e.switch_segment(&seg, None, span);
            if let Some(b) = body {
                let r = e.exec_block(b);
                e.cur_seg = saved;
                e.pc_offsets = saved_off;
                return r;
            }
        }
        ("snes", "header") => {
            let map = match e.arg_str(args, "map", 1) {
                Some(m) => match raw_output::SnesMap::parse(&m) {
                    Some(m) => m,
                    None => {
                        e.err(format!("unknown SNES memory map `{m}` (lorom, hirom, exhirom)"), span);
                        raw_output::SnesMap::LoRom
                    }
                },
                None => raw_output::SnesMap::LoRom,
            };
            let h = raw_output::SnesHeader {
                title: e.arg_str(args, "title", 0).unwrap_or_default(),
                map,
                fast: e.arg_bool(args, "fast", 2),
                cart_type: e.arg_int(args, "type", 3).unwrap_or(0) as u8,
                sram_kb: e.arg_int(args, "sram", 4).unwrap_or(0) as u32,
                region: e.arg_int(args, "region", 5).unwrap_or(1) as u8,
                license: e.arg_int(args, "license", 6).unwrap_or(0) as u8,
                version: e.arg_int(args, "version", 7).unwrap_or(0) as u8,
            };
            e.plat.snes_map = Some(map);
            e.plat.snes = Some(h);
        }
        ("snes", "spc") => {
            let addr = e.arg_int(args, "address", 0).unwrap_or(0x200);
            let Some(b) = body else {
                e.err("`.snes.spc` needs a block", span);
                return Ok(Flow::Normal);
            };
            let saved = e.cpu;
            e.cpu = raw_cpu::by_name("spc700").unwrap();
            let off = addr - i64::from(e.real_pc());
            e.pc_offsets.push(off);
            let r = e.exec_block(b);
            e.pc_offsets.pop();
            e.cpu = saved;
            return r;
        }
        ("cpc", "amsdos") => {
            let name = e.arg_str(args, "name", 0).unwrap_or_default();
            let exec = e.arg_int(args, "exec", 1).map(|v| v as u16);
            e.plat.amsdos = Some((name, exec));
        }
        ("amiga" | "amiga1200", "section") => {
            let name = e.arg_str(args, "name", 0).unwrap_or_else(|| "code".into());
            let kind = match e.arg_str(args, "kind", 1).map(|k| k.to_ascii_lowercase()).as_deref() {
                Some("data") => raw_output::HunkKind::Data,
                Some("bss") => raw_output::HunkKind::Bss,
                _ => raw_output::HunkKind::Code,
            };
            e.hunk_section(&name, kind, span);
        }
        ("vectrex", "header") => {
            let mut a = vec![e.arg_str(args, "title", 0).map_or(Value::Undef, Value::str)];
            if let Some(m) = e.arg_int(args, "music", 1) {
                a.push(Value::Int(m));
            }
            return e.prelude_macro(plat, "VectrexHeader", a, span);
        }
        ("coleco", "header") => {
            let a = e.arg_int(args, "start", 0).map_or(Value::Undef, Value::Int);
            return e.prelude_macro(plat, "ColecoHeader", vec![a], span);
        }
        (p, d) => e.err(format!("directive `{d}` is not implemented for platform {p}"), span),
    }
    Ok(Flow::Normal)
}

impl Engine<'_> {
    /// Switch to (or start) an Amiga hunk section.
    pub fn hunk_section(&mut self, name: &str, kind: raw_output::HunkKind, span: Span) {
        let seg = format!("\u{1}hunk:{name}");
        if !self.segments.contains_key(&seg) {
            let idx = self.hunk_order.len();
            let base = self.hunk_base(idx);
            self.ensure_segment(&seg, SegParams { start: Some(base), ..Default::default() });
            self.segments[&seg].hunk = Some(kind);
            self.hunk_order.push(seg.clone());
        }
        self.pc_offsets.clear();
        self.switch_segment(&seg, None, span);
    }

    /// Load address of hunk `idx` in this assembly run.
    pub fn hunk_base(&self, idx: usize) -> u32 {
        self.hunk_bases.get(idx).copied().unwrap_or(crate::output::HUNK_BASE + idx as u32 * crate::output::HUNK_STRIDE)
    }
}
