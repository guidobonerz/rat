//! Zilog Z80 with undocumented instructions (IXH/IXL/IYH/IYL, SLL,
//! DD CB register copies, `IN (C)`, `OUT (C),0`).
//!
//! The decoder follows the x/y/z/p/q opcode fields of
//! `mimic/crates/cpu-z80/src/disasm.rs`. The encoder is generated from it:
//! every opcode is decoded once into an operand pattern such as
//! `ld (ix+d),n`, which maps back to its byte layout.

use std::collections::HashMap;

use raw_core::{Endian, Tok, Token};

use crate::util::*;
use crate::{Cpu, CpuInfo, CpuState, Num, OperandCtx};

const R: [&str; 8] = ["b", "c", "d", "e", "h", "l", "(hl)", "a"];
const RP: [&str; 4] = ["bc", "de", "hl", "sp"];
const RP2: [&str; 4] = ["bc", "de", "hl", "af"];
const CC: [&str; 8] = ["nz", "z", "nc", "c", "po", "pe", "p", "m"];
const ALU: [(&str, &str); 8] = [
    ("add", "a,"),
    ("adc", "a,"),
    ("sub", ""),
    ("sbc", "a,"),
    ("and", ""),
    ("xor", ""),
    ("or", ""),
    ("cp", ""),
];
const ROT: [&str; 8] = ["rlc", "rrc", "rl", "rr", "sla", "sra", "sll", "srl"];
const ACC: [&str; 8] = ["rlca", "rrca", "rla", "rra", "daa", "cpl", "scf", "ccf"];
const BLOCK: [[&str; 4]; 4] = [
    ["ldi", "cpi", "ini", "outi"],
    ["ldd", "cpd", "ind", "outd"],
    ["ldir", "cpir", "inir", "otir"],
    ["lddr", "cpdr", "indr", "otdr"],
];

/// One byte of an instruction layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Piece {
    Fixed(u8),
    /// Immediate byte `n`.
    N,
    /// Low/high byte of a word `nn` (the low piece stands for both).
    NnLo,
    NnHi,
    /// Relative branch displacement.
    E,
    /// Index displacement `d`.
    D,
}

struct Reader<'a> {
    b: &'a [u8],
    pieces: Vec<Piece>,
    vals: Vec<u8>,
}

impl Reader<'_> {
    fn take(&mut self, p: Piece) -> Option<u8> {
        let v = *self.b.get(self.pieces.len())?;
        self.pieces.push(match p {
            Piece::Fixed(_) => Piece::Fixed(v),
            p => p,
        });
        self.vals.push(v);
        Some(v)
    }
    fn op(&mut self) -> Option<u8> {
        self.take(Piece::Fixed(0))
    }
    fn n(&mut self) -> Option<&'static str> {
        self.take(Piece::N)?;
        Some("{n}")
    }
    fn nn(&mut self) -> Option<&'static str> {
        self.take(Piece::NnLo)?;
        self.take(Piece::NnHi)?;
        Some("{nn}")
    }
    fn e(&mut self) -> Option<&'static str> {
        self.take(Piece::E)?;
        Some("{e}")
    }
}

fn idx_r(i: u8, ix: &str, mem: &str) -> String {
    match (i, ix) {
        (6, _) => mem.to_owned(),
        (4, "ix") => "ixh".into(),
        (5, "ix") => "ixl".into(),
        (4, "iy") => "iyh".into(),
        (5, "iy") => "iyl".into(),
        _ => R[i as usize].into(),
    }
}

fn rp(p: u8, ix: &str) -> String {
    if p == 2 { ix.into() } else { RP[p as usize].into() }
}

fn rp2(p: u8, ix: &str) -> String {
    if p == 2 { ix.into() } else { RP2[p as usize].into() }
}

fn s(m: &str, o: impl Into<String>) -> Option<String> {
    let o = o.into();
    Some(if o.is_empty() { m.to_owned() } else { format!("{m} {o}") })
}

/// Decode to a text template with `{n}` `{nn}` `{e}` `{d}` slots. `None`
/// for prefix-only bytes (no instruction).
fn decode_main(rd: &mut Reader<'_>, opc: u8, ix: &'static str) -> Option<String> {
    let (x, y, z) = (opc >> 6, (opc >> 3) & 7, opc & 7);
    let (p, q) = (y >> 1, y & 1);
    let uses_mem = match x {
        0 => (z == 4 || z == 5 || z == 6) && y == 6,
        1 => (y == 6) != (z == 6),
        2 => z == 6,
        _ => false,
    };
    let mut mem = "(hl)".to_string();
    if ix != "hl" && uses_mem {
        rd.take(Piece::D)?;
        mem = format!("({ix}+{{d}})");
    }
    let r = |i: u8| idx_r(i, ix, &mem);
    let plain = |i: u8| idx_r(i, "hl", &mem);
    match x {
        0 => match z {
            0 => match y {
                0 => s("nop", ""),
                1 => s("ex", "af,af'"),
                2 => s("djnz", rd.e()?),
                3 => s("jr", rd.e()?),
                _ => s("jr", format!("{},{}", CC[(y - 4) as usize], rd.e()?)),
            },
            1 if q == 0 => s("ld", format!("{},{}", rp(p, ix), rd.nn()?)),
            1 => s("add", format!("{},{}", rp(2, ix), rp(p, ix))),
            2 => match (q, p) {
                (0, 0) => s("ld", "(bc),a"),
                (0, 1) => s("ld", "(de),a"),
                (0, 2) => s("ld", format!("({}),{}", rd.nn()?, rp(2, ix))),
                (0, _) => s("ld", format!("({}),a", rd.nn()?)),
                (_, 0) => s("ld", "a,(bc)"),
                (_, 1) => s("ld", "a,(de)"),
                (_, 2) => s("ld", format!("{},({})", rp(2, ix), rd.nn()?)),
                (_, _) => s("ld", format!("a,({})", rd.nn()?)),
            },
            3 => s(if q == 0 { "inc" } else { "dec" }, rp(p, ix)),
            4 => s("inc", r(y)),
            5 => s("dec", r(y)),
            6 => s("ld", format!("{},{}", r(y), rd.n()?)),
            _ => s(ACC[y as usize], ""),
        },
        1 if y == 6 && z == 6 => s("halt", ""),
        1 if y == 6 => s("ld", format!("{},{}", mem, plain(z))),
        1 if z == 6 => s("ld", format!("{},{}", plain(y), mem)),
        1 => s("ld", format!("{},{}", r(y), r(z))),
        2 => {
            let (m, pre) = ALU[y as usize];
            s(m, format!("{pre}{}", r(z)))
        }
        _ => match z {
            0 => s("ret", CC[y as usize]),
            1 if q == 0 => s("pop", rp2(p, ix)),
            1 => match p {
                0 => s("ret", ""),
                1 => s("exx", ""),
                2 => s("jp", format!("({})", rp(2, ix))),
                _ => s("ld", format!("sp,{}", rp(2, ix))),
            },
            2 => s("jp", format!("{},{}", CC[y as usize], rd.nn()?)),
            3 => match y {
                0 => s("jp", rd.nn()?),
                1 => decode_cb(rd, ix),
                2 => s("out", format!("({}),a", rd.n()?)),
                3 => s("in", format!("a,({})", rd.n()?)),
                4 => s("ex", format!("(sp),{}", rp(2, ix))),
                5 => s("ex", "de,hl"),
                6 => s("di", ""),
                _ => s("ei", ""),
            },
            4 => s("call", format!("{},{}", CC[y as usize], rd.nn()?)),
            5 if q == 0 => s("push", rp2(p, ix)),
            5 => match p {
                0 => s("call", rd.nn()?),
                2 => {
                    let o = rd.op()?;
                    decode_ed(rd, o)
                }
                _ if ix != "hl" => None,
                _ => {
                    let o = rd.op()?;
                    if matches!(o, 0xdd | 0xfd | 0xed) {
                        return None;
                    }
                    decode_main(rd, o, if p == 1 { "ix" } else { "iy" })
                }
            },
            6 => {
                let (m, pre) = ALU[y as usize];
                s(m, format!("{pre}{}", rd.n()?))
            }
            _ => s("rst", format!("{}", u16::from(y) * 8)),
        },
    }
}

fn decode_cb(rd: &mut Reader<'_>, ix: &'static str) -> Option<String> {
    let mem = if ix == "hl" {
        "(hl)".to_string()
    } else {
        rd.take(Piece::D)?;
        format!("({ix}+{{d}})")
    };
    let o = rd.op()?;
    let (x, y, z) = (o >> 6, (o >> 3) & 7, o & 7);
    let target = if z == 6 || ix != "hl" { mem.clone() } else { R[z as usize].to_string() };
    let copy = if ix != "hl" && z != 6 && x != 1 { format!(",{}", R[z as usize]) } else { String::new() };
    match x {
        0 => s(ROT[y as usize], format!("{target}{copy}")),
        1 => s("bit", format!("{y},{target}")),
        2 => s("res", format!("{y},{target}{copy}")),
        _ => s("set", format!("{y},{target}{copy}")),
    }
}

fn decode_ed(rd: &mut Reader<'_>, o: u8) -> Option<String> {
    let (x, y, z) = (o >> 6, (o >> 3) & 7, o & 7);
    let (p, q) = (y >> 1, y & 1);
    if x == 2 && z <= 3 && y >= 4 {
        return s(BLOCK[(y - 4) as usize][z as usize], "");
    }
    if x != 1 {
        return None;
    }
    match z {
        0 if y == 6 => s("in", "(c)"),
        0 => s("in", format!("{},(c)", R[y as usize])),
        1 if y == 6 => s("out", "(c),0"),
        1 => s("out", format!("(c),{}", R[y as usize])),
        2 => s(if q == 0 { "sbc" } else { "adc" }, format!("hl,{}", RP[p as usize])),
        3 if q == 0 => s("ld", format!("({}),{}", rd.nn()?, RP[p as usize])),
        3 => s("ld", format!("{},({})", RP[p as usize], rd.nn()?)),
        4 => s("neg", ""),
        5 => s(if y == 1 { "reti" } else { "retn" }, ""),
        6 => s("im", ["0", "0", "1", "2"][(y & 3) as usize]),
        _ => match y {
            0 => s("ld", "i,a"),
            1 => s("ld", "r,a"),
            2 => s("ld", "a,i"),
            3 => s("ld", "a,r"),
            4 => s("rrd", ""),
            5 => s("rld", ""),
            _ => None,
        },
    }
}

/// Decode one instruction: template text, layout and raw byte values.
fn decode(b: &[u8]) -> Option<(String, Vec<Piece>, Vec<u8>)> {
    let mut rd = Reader { b, pieces: Vec::new(), vals: Vec::new() };
    let o = rd.op()?;
    let t = decode_main(&mut rd, o, "hl")?;
    Some((t, rd.pieces, rd.vals))
}

/// Pattern key of a template: slots become `n`, `(ix+{d})` becomes `(ix+d)`.
fn key_of(t: &str) -> String {
    t.replace("{nn}", "n").replace("{n}", "n").replace("{e}", "n").replace("{d}", "d")
}

pub struct Z80 {
    table: HashMap<String, Vec<Piece>>,
    mnemonics: Vec<String>,
}

impl Z80 {
    pub fn new() -> Z80 {
        let mut table: HashMap<String, Vec<Piece>> = HashMap::new();
        let mut add = |bytes: &[u8]| {
            if let Some((t, pieces, _)) = decode(bytes) {
                table.entry(key_of(&t)).or_insert(pieces);
            }
        };
        for o in 0..=255u8 {
            if !matches!(o, 0xcb | 0xdd | 0xed | 0xfd) {
                add(&[o, 0, 0, 0]);
            }
        }
        for o in 0..=255u8 {
            add(&[0xcb, o]);
        }
        for o in 0..=255u8 {
            add(&[0xed, o, 0, 0]);
        }
        for pre in [0xdd, 0xfd] {
            for o in 0..=255u8 {
                if o != 0xcb {
                    add(&[pre, o, 0, 0]);
                }
            }
            // documented (z = 6) forms first
            let order = (0..=255u8).filter(|o| o & 7 == 6).chain((0..=255u8).filter(|o| o & 7 != 6));
            for o in order {
                add(&[pre, 0xcb, 0, o]);
            }
        }
        // aliases
        for (alias, real) in [("in f,(c)", "in (c)"), ("ex af,af", "ex af,af'"), ("jp hl", "jp (hl)"), ("jp ix", "jp (ix)"), ("jp iy", "jp (iy)")] {
            if let Some(p) = table.get(real).cloned() {
                table.insert(alias.into(), p);
            }
        }
        let mut mnemonics: Vec<String> = table
            .keys()
            .map(|k| k.split(' ').next().unwrap().to_owned())
            .collect();
        mnemonics.extend(["sl1", "sli", "sls"].map(String::from));
        mnemonics.sort();
        mnemonics.dedup();
        Z80 { table, mnemonics }
    }
}

impl Default for Z80 {
    fn default() -> Self {
        Self::new()
    }
}

/// Normalized register spelling, if `s` names a register or condition.
fn reg_name(s: &str) -> Option<&'static str> {
    Some(match s.to_ascii_lowercase().as_str() {
        "a" => "a",
        "b" => "b",
        "c" => "c",
        "d" => "d",
        "e" => "e",
        "h" => "h",
        "l" => "l",
        "f" => "f",
        "i" => "i",
        "r" => "r",
        "af" => "af",
        "af'" => "af'",
        "bc" => "bc",
        "de" => "de",
        "hl" => "hl",
        "sp" => "sp",
        "ix" => "ix",
        "iy" => "iy",
        "ixh" | "hx" | "xh" | "ixu" => "ixh",
        "ixl" | "lx" | "xl" => "ixl",
        "iyh" | "hy" | "yh" | "iyu" => "iyh",
        "iyl" | "ly" | "yl" => "iyl",
        "nz" => "nz",
        "z" => "z",
        "nc" => "nc",
        "po" => "po",
        "pe" => "pe",
        "p" => "p",
        "m" => "m",
        _ => return None,
    })
}

enum Part {
    Lit(String),
    N(std::ops::Range<usize>),
    /// `(expr)`
    Mem(std::ops::Range<usize>),
    /// `(ix+expr)`; `neg` for `(ix-expr)`; empty range for `(ix)`.
    Idx(&'static str, std::ops::Range<usize>, bool),
}

fn classify(toks: &[Token], r: std::ops::Range<usize>) -> Result<Part, String> {
    if r.is_empty() {
        return Err("missing operand".into());
    }
    if let Some(name) = single_ident(toks, &r).and_then(reg_name) {
        return Ok(Part::Lit(name.into()));
    }
    if wrapped(toks, &r, "(", ")") {
        let inner = r.start + 1..r.end - 1;
        if let Some(name) = single_ident(toks, &inner).and_then(reg_name) {
            return Ok(match name {
                "ix" | "iy" => Part::Idx(name, inner.end..inner.end, false),
                _ => Part::Lit(format!("({name})")),
            });
        }
        if let Some(ix) = toks[inner.start].ident().and_then(reg_name).filter(|n| *n == "ix" || *n == "iy") {
            let op = &toks[inner.start + 1];
            if op.is_punct("+") || op.is_punct("-") {
                return Ok(Part::Idx(ix, inner.start + 2..inner.end, op.is_punct("-")));
            }
        }
        return Ok(Part::Mem(inner));
    }
    Ok(Part::N(r))
}

impl Cpu for Z80 {
    fn info(&self) -> CpuInfo {
        CpuInfo {
            id: "z80",
            aliases: &["z80undoc"],
            description: "Zilog Z80 with undocumented instructions (IXH/IXL, SLL, DD CB copies)",
            endian: "little",
            addr_bits: 16,
        }
    }

    fn endian(&self) -> Endian {
        Endian::Little
    }

    fn has_mnemonic(&self, m: &str) -> bool {
        let m = m.to_ascii_lowercase();
        self.mnemonics.binary_search(&m).is_ok()
    }

    fn mnemonics(&self) -> Vec<String> {
        self.mnemonics.clone()
    }

    fn assemble(
        &self,
        mnemonic: &str,
        _suffix: Option<&str>,
        ctx: &mut dyn OperandCtx,
        _st: &CpuState,
    ) -> Result<Vec<u8>, String> {
        let mut m = mnemonic.to_ascii_lowercase();
        if matches!(m.as_str(), "sl1" | "sli" | "sls") {
            m = "sll".into();
        }
        let toks = ctx.toks().to_vec();
        let mut parts: Vec<Part> = commas(&toks)
            .into_iter()
            .map(|r| classify(&toks, r))
            .collect::<Result<_, _>>()?;
        // `sub a,b` == `sub b`; `add b` == `add a,b`
        if matches!(m.as_str(), "sub" | "and" | "xor" | "or" | "cp") && parts.len() == 2 {
            if let Part::Lit(l) = &parts[0] {
                if l == "a" {
                    parts.remove(0);
                }
            }
        }
        // evaluate expressions
        let mut vals: Vec<Option<Num>> = Vec::new();
        for p in &parts {
            vals.push(match p {
                Part::N(r) | Part::Mem(r) => Some(eval(ctx, r)?),
                Part::Idx(_, r, neg) => {
                    if r.is_empty() {
                        Some(Num::known(0))
                    } else {
                        let n = eval(ctx, r)?;
                        Some(if *neg { Num::with(n, -n.value) } else { n })
                    }
                }
                Part::Lit(_) => None,
            });
        }
        let render = |literal: bool, idx_lit: bool| -> Option<String> {
            let mut out = Vec::new();
            for (p, v) in parts.iter().zip(&vals) {
                out.push(match p {
                    Part::Lit(l) => l.clone(),
                    Part::N(_) if literal => {
                        let v = v.unwrap();
                        if !v.known {
                            return None;
                        }
                        v.value.to_string()
                    }
                    Part::N(_) => "n".into(),
                    Part::Mem(_) => "(n)".into(),
                    Part::Idx(ix, r, _) if idx_lit && r.is_empty() => format!("({ix})"),
                    Part::Idx(ix, ..) => format!("({ix}+d)"),
                });
            }
            Some(if out.is_empty() { m.clone() } else { format!("{m} {}", out.join(",")) })
        };
        let mut found = None;
        for (lit, alt_a, idx_lit) in [(false, false, false), (true, false, false), (false, true, false), (true, true, false), (false, false, true)] {
            let Some(mut key) = render(lit, idx_lit) else { continue };
            if alt_a {
                // `add b` -> `add a,b`
                let Some((mn, rest)) = key.split_once(' ') else { continue };
                key = format!("{mn} a,{rest}");
            }
            if let Some(p) = self.table.get(&key) {
                found = Some((p.clone(), alt_a));
                break;
            }
        }
        let (pieces, alt_a) = found.ok_or_else(|| {
            format!("`{}` is not a Z80 instruction", render(false, false).unwrap_or_default())
        })?;
        let _ = alt_a;
        // values for slots, in operand order
        let mut imms: Vec<Num> = Vec::new();
        let mut disp: Option<Num> = None;
        for (p, v) in parts.iter().zip(&vals) {
            match p {
                Part::Idx(..) => disp = *v,
                Part::N(_) | Part::Mem(_) => imms.push(v.unwrap()),
                Part::Lit(_) => {}
            }
        }
        let has_slot = pieces.iter().any(|p| matches!(p, Piece::N | Piece::NnLo | Piece::E));
        let mut imm_iter = imms.into_iter();
        let imm = if has_slot { imm_iter.next() } else { None };
        let len = pieces.len() as i64;
        let pc = i64::from(ctx.pc());
        let mut out = Vec::with_capacity(pieces.len());
        for p in &pieces {
            match *p {
                Piece::Fixed(b) => out.push(b),
                Piece::N => out.push(fit(ctx, imm.unwrap_or_default(), -128, 255, 8, "byte value") as u8),
                Piece::NnLo => out.extend(le16(fit(ctx, imm.unwrap_or_default(), -32768, 65535, 16, "word value"))),
                Piece::NnHi => {}
                Piece::E => out.push(rel(ctx, imm.unwrap_or_default(), pc + len, 8) as u8),
                Piece::D => out.push(fit(ctx, disp.unwrap_or_default(), -128, 127, 8, "index displacement") as u8),
            }
        }
        Ok(out)
    }

    fn disassemble(&self, bytes: &[u8], addr: u32, _st: &CpuState) -> Option<(usize, String)> {
        let (t, pieces, vals) = decode(bytes)?;
        let len = pieces.len();
        let mut n = None;
        let mut nn = None;
        let mut e = None;
        let mut d = None;
        for (i, p) in pieces.iter().enumerate() {
            match p {
                Piece::N => n = Some(vals[i]),
                Piece::NnLo => nn = Some(u16::from_le_bytes([vals[i], vals[i + 1]])),
                Piece::E => e = Some((addr as u16).wrapping_add(len as u16).wrapping_add(vals[i] as i8 as u16)),
                Piece::D => d = Some(vals[i] as i8),
                _ => {}
            }
        }
        let mut text = t;
        if let Some(v) = n {
            text = text.replace("{n}", &format!("${v:02x}"));
        }
        if let Some(v) = nn {
            text = text.replace("{nn}", &format!("${v:04x}"));
        }
        if let Some(v) = e {
            text = text.replace("{e}", &format!("${v:04x}"));
        }
        if let Some(v) = d {
            let s = if v < 0 { format!("-${:02x}", -i16::from(v)) } else { format!("+${v:02x}") };
            text = text.replace("+{d}", &s);
        }
        Some((len, text))
    }
}

#[allow(dead_code)]
fn _unused(_: &Tok) {}
