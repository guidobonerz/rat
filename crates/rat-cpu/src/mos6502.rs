//! NMOS 6502 family including the undocumented opcodes, and the CMOS
//! 65C02 (Apple IIe enhanced).
//!
//! Opcode matrices ported from `mimic/crates/cpu-6502/src/table.rs`
//! (undocumented names after Groepaz, "NMOS 6510 Unintended Opcodes"; the
//! 65C02 without the Rockwell bit instructions, like mimic's: every
//! undefined CMOS opcode is a NOP of one to three bytes).

use std::collections::HashMap;

use rat_core::{Endian, Token};

use crate::util::*;
use crate::{Cpu, CpuInfo, CpuState, Num, OperandCtx};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Imp,
    Imm,
    Zp,
    ZpX,
    ZpY,
    Abs,
    AbsX,
    AbsY,
    Ind,
    IndX,
    IndY,
    Rel,
    /// 65C02 `(zp)`.
    Zpi,
    /// 65C02 `jmp (abs,x)`.
    AbsXInd,
}

impl Mode {
    pub fn size(self) -> usize {
        match self {
            Mode::Imp => 1,
            Mode::Abs | Mode::AbsX | Mode::AbsY | Mode::Ind | Mode::AbsXInd => 3,
            _ => 2,
        }
    }
}

use Mode::*;

#[rustfmt::skip]
pub const TABLE: [(&str, Mode); 256] = [
    ("brk",Imp),("ora",IndX),("jam",Imp),("slo",IndX),("nop",Zp),("ora",Zp),("asl",Zp),("slo",Zp),
    ("php",Imp),("ora",Imm),("asl",Imp),("anc",Imm),("nop",Abs),("ora",Abs),("asl",Abs),("slo",Abs),
    ("bpl",Rel),("ora",IndY),("jam",Imp),("slo",IndY),("nop",ZpX),("ora",ZpX),("asl",ZpX),("slo",ZpX),
    ("clc",Imp),("ora",AbsY),("nop",Imp),("slo",AbsY),("nop",AbsX),("ora",AbsX),("asl",AbsX),("slo",AbsX),
    ("jsr",Abs),("and",IndX),("jam",Imp),("rla",IndX),("bit",Zp),("and",Zp),("rol",Zp),("rla",Zp),
    ("plp",Imp),("and",Imm),("rol",Imp),("anc",Imm),("bit",Abs),("and",Abs),("rol",Abs),("rla",Abs),
    ("bmi",Rel),("and",IndY),("jam",Imp),("rla",IndY),("nop",ZpX),("and",ZpX),("rol",ZpX),("rla",ZpX),
    ("sec",Imp),("and",AbsY),("nop",Imp),("rla",AbsY),("nop",AbsX),("and",AbsX),("rol",AbsX),("rla",AbsX),
    ("rti",Imp),("eor",IndX),("jam",Imp),("sre",IndX),("nop",Zp),("eor",Zp),("lsr",Zp),("sre",Zp),
    ("pha",Imp),("eor",Imm),("lsr",Imp),("alr",Imm),("jmp",Abs),("eor",Abs),("lsr",Abs),("sre",Abs),
    ("bvc",Rel),("eor",IndY),("jam",Imp),("sre",IndY),("nop",ZpX),("eor",ZpX),("lsr",ZpX),("sre",ZpX),
    ("cli",Imp),("eor",AbsY),("nop",Imp),("sre",AbsY),("nop",AbsX),("eor",AbsX),("lsr",AbsX),("sre",AbsX),
    ("rts",Imp),("adc",IndX),("jam",Imp),("rra",IndX),("nop",Zp),("adc",Zp),("ror",Zp),("rra",Zp),
    ("pla",Imp),("adc",Imm),("ror",Imp),("arr",Imm),("jmp",Ind),("adc",Abs),("ror",Abs),("rra",Abs),
    ("bvs",Rel),("adc",IndY),("jam",Imp),("rra",IndY),("nop",ZpX),("adc",ZpX),("ror",ZpX),("rra",ZpX),
    ("sei",Imp),("adc",AbsY),("nop",Imp),("rra",AbsY),("nop",AbsX),("adc",AbsX),("ror",AbsX),("rra",AbsX),
    ("nop",Imm),("sta",IndX),("nop",Imm),("sax",IndX),("sty",Zp),("sta",Zp),("stx",Zp),("sax",Zp),
    ("dey",Imp),("nop",Imm),("txa",Imp),("ane",Imm),("sty",Abs),("sta",Abs),("stx",Abs),("sax",Abs),
    ("bcc",Rel),("sta",IndY),("jam",Imp),("sha",IndY),("sty",ZpX),("sta",ZpX),("stx",ZpY),("sax",ZpY),
    ("tya",Imp),("sta",AbsY),("txs",Imp),("tas",AbsY),("shy",AbsX),("sta",AbsX),("shx",AbsY),("sha",AbsY),
    ("ldy",Imm),("lda",IndX),("ldx",Imm),("lax",IndX),("ldy",Zp),("lda",Zp),("ldx",Zp),("lax",Zp),
    ("tay",Imp),("lda",Imm),("tax",Imp),("lxa",Imm),("ldy",Abs),("lda",Abs),("ldx",Abs),("lax",Abs),
    ("bcs",Rel),("lda",IndY),("jam",Imp),("lax",IndY),("ldy",ZpX),("lda",ZpX),("ldx",ZpY),("lax",ZpY),
    ("clv",Imp),("lda",AbsY),("tsx",Imp),("las",AbsY),("ldy",AbsX),("lda",AbsX),("ldx",AbsY),("lax",AbsY),
    ("cpy",Imm),("cmp",IndX),("nop",Imm),("dcp",IndX),("cpy",Zp),("cmp",Zp),("dec",Zp),("dcp",Zp),
    ("iny",Imp),("cmp",Imm),("dex",Imp),("sbx",Imm),("cpy",Abs),("cmp",Abs),("dec",Abs),("dcp",Abs),
    ("bne",Rel),("cmp",IndY),("jam",Imp),("dcp",IndY),("nop",ZpX),("cmp",ZpX),("dec",ZpX),("dcp",ZpX),
    ("cld",Imp),("cmp",AbsY),("nop",Imp),("dcp",AbsY),("nop",AbsX),("cmp",AbsX),("dec",AbsX),("dcp",AbsX),
    ("cpx",Imm),("sbc",IndX),("nop",Imm),("isc",IndX),("cpx",Zp),("sbc",Zp),("inc",Zp),("isc",Zp),
    ("inx",Imp),("sbc",Imm),("nop",Imp),("sbc",Imm),("cpx",Abs),("sbc",Abs),("inc",Abs),("isc",Abs),
    ("beq",Rel),("sbc",IndY),("jam",Imp),("isc",IndY),("nop",ZpX),("sbc",ZpX),("inc",ZpX),("isc",ZpX),
    ("sed",Imp),("sbc",AbsY),("nop",Imp),("isc",AbsY),("nop",AbsX),("sbc",AbsX),("inc",AbsX),("isc",AbsX),
];

/// The 65C02 matrix: the NMOS one with the new instructions and modes,
/// every other undefined opcode a NOP (mimic's `cmos_table`).
pub fn cmos_table() -> [(&'static str, Mode); 256] {
    let mut t = TABLE;
    for i in 0..256 {
        let (m, mode) = TABLE[i];
        let lo = i & 0x0f;
        let odd_row = i & 0x10 != 0;
        t[i] = if lo == 0x03 || lo == 0x0b {
            ("nop", Imp)
        } else if lo == 0x07 {
            ("nop", if odd_row { ZpX } else { Zp })
        } else if lo == 0x0f {
            ("nop", Abs)
        } else if lo == 0x02 && matches!(m, "jam" | "nop") {
            if odd_row {
                // ORA AND EOR ADC STA LDA CMP SBC (zp): the (zp),Y column's ops
                (TABLE[i - 1].0, Zpi)
            } else {
                ("nop", Imm)
            }
        } else {
            match m {
                "slo" | "rla" | "sre" | "rra" | "sax" | "lax" | "dcp" | "isc" | "anc" | "alr" | "arr" | "ane" | "lxa" | "sbx" | "las" | "tas" | "sha" | "shx" | "shy" => ("nop", Imp),
                _ => (m, mode),
            }
        };
    }
    t[0x04] = ("tsb", Zp);
    t[0x0c] = ("tsb", Abs);
    t[0x14] = ("trb", Zp);
    t[0x1c] = ("trb", Abs);
    t[0x1a] = ("inc", Imp);
    t[0x3a] = ("dec", Imp);
    t[0x34] = ("bit", ZpX);
    t[0x3c] = ("bit", AbsX);
    t[0x89] = ("bit", Imm);
    t[0x5a] = ("phy", Imp);
    t[0x7a] = ("ply", Imp);
    t[0xda] = ("phx", Imp);
    t[0xfa] = ("plx", Imp);
    t[0x64] = ("stz", Zp);
    t[0x74] = ("stz", ZpX);
    t[0x9c] = ("stz", Abs);
    t[0x9e] = ("stz", AbsX);
    t[0x7c] = ("jmp", AbsXInd);
    t[0x80] = ("bra", Rel);
    t[0x5c] = ("nop", Abs);
    t[0xdc] = ("nop", Abs);
    t[0xfc] = ("nop", Abs);
    t[0xcb] = ("nop", Imp);
    t[0xdb] = ("nop", ZpX);
    t[0x44] = ("nop", Zp);
    t[0x54] = ("nop", ZpX);
    t[0xd4] = ("nop", ZpX);
    t[0xf4] = ("nop", ZpX);
    t
}

const DOCUMENTED: &[&str] = &[
    "adc", "and", "asl", "bcc", "bcs", "beq", "bit", "bmi", "bne", "bpl", "brk", "bvc", "bvs",
    "clc", "cld", "cli", "clv", "cmp", "cpx", "cpy", "dec", "dex", "dey", "eor", "inc", "inx",
    "iny", "jmp", "jsr", "lda", "ldx", "ldy", "lsr", "nop", "ora", "pha", "php", "pla", "plp",
    "rol", "ror", "rti", "rts", "sbc", "sec", "sed", "sei", "sta", "stx", "sty", "tax", "tay",
    "tsx", "txa", "txs", "tya",
];

/// Opcodes that are the documented encoding of their (mnemonic, mode).
fn is_documented_opcode(op: u8) -> bool {
    let (m, _) = TABLE[op as usize];
    if !DOCUMENTED.contains(&m) {
        return false;
    }
    !(m == "nop" && op != 0xea) && op != 0xeb
}

/// Documented 65C02 opcodes: everything but the NOP fillers.
fn is_documented_cmos(table: &[(&str, Mode); 256], op: u8) -> bool {
    table[op as usize].0 != "nop" || op == 0xea
}

/// Alternative names used by ACME, KickAssembler, ca65 and others.
const ALIASES: &[(&str, &str)] = &[
    ("asr", "alr"),
    ("axs", "sbx"),
    ("xaa", "ane"),
    ("ahx", "sha"),
    ("axa", "sha"),
    ("shs", "tas"),
    ("xas", "shx"),
    ("sxa", "shx"),
    ("say", "shy"),
    ("sya", "shy"),
    ("lar", "las"),
    ("lds", "las"),
    ("lae", "las"),
    ("isb", "isc"),
    ("ins", "isc"),
    ("dcm", "dcp"),
    ("aso", "slo"),
    ("lse", "sre"),
    ("aax", "sax"),
    ("atx", "lxa"),
    ("oal", "lxa"),
    ("dop", "nop"),
    ("skb", "nop"),
    ("top", "nop"),
    ("skw", "nop"),
    ("ign", "nop"),
    ("kil", "jam"),
    ("hlt", "jam"),
    ("stp", "jam"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// NMOS 6502/6510/8502/7501/SALLY, all opcodes.
    Nmos,
    /// Documented opcodes only.
    NoIllegals,
    M6510,
    /// NES: no decimal mode.
    Ricoh2a03,
    /// Atari 2600: 13-bit address bus.
    M6507,
    /// CMOS 65C02 (Apple IIe enhanced): BRA, PHX/PHY/PLX/PLY, STZ, TRB/TSB,
    /// `(zp)`, `jmp (abs,x)`, `bit #`, `inc a`/`dec a`; no undocumented
    /// opcodes and no Rockwell bit instructions.
    Cmos65c02,
}

pub struct Mos6502 {
    variant: Variant,
    table: [(&'static str, Mode); 256],
    ops: HashMap<&'static str, HashMap<Mode, u8>>,
}

impl Mos6502 {
    pub fn new(variant: Variant) -> Mos6502 {
        let mut ops: HashMap<&'static str, HashMap<Mode, u8>> = HashMap::new();
        let cmos = variant == Variant::Cmos65c02;
        let table = if cmos { cmos_table() } else { TABLE };
        let legal_only = variant == Variant::NoIllegals;
        // documented encodings first so they win over duplicates
        for pass in 0..2 {
            for (op, &(m, mode)) in table.iter().enumerate() {
                let doc = if cmos { is_documented_cmos(&table, op as u8) } else { is_documented_opcode(op as u8) };
                if (pass == 0) != doc || (legal_only && !doc) {
                    continue;
                }
                ops.entry(m).or_default().entry(mode).or_insert(op as u8);
            }
        }
        if !legal_only && !cmos {
            ops.entry("anc2").or_default().insert(Imm, 0x2b);
            ops.entry("sbc2").or_default().insert(Imm, 0xeb);
            // KickAssembler: `lax #imm` is LXA ($AB)
            ops.entry("lax").or_default().insert(Imm, 0xab);
        }
        Mos6502 { variant, table, ops }
    }

    fn lookup(&self, m: &str) -> Option<&HashMap<Mode, u8>> {
        let m = m.to_ascii_lowercase();
        if let Some(t) = self.ops.get(m.as_str()) {
            return Some(t);
        }
        if matches!(self.variant, Variant::NoIllegals | Variant::Cmos65c02) {
            return None;
        }
        let canon = ALIASES.iter().find(|(a, _)| *a == m)?.1;
        self.ops.get(canon)
    }

    fn documented(&self, op: u8) -> bool {
        match self.variant {
            Variant::Cmos65c02 => is_documented_cmos(&self.table, op),
            _ => is_documented_opcode(op),
        }
    }
}

/// Size override from a mnemonic suffix: 1 = zero page, 2 = absolute.
pub fn size_suffix(suffix: Option<&str>) -> Result<Option<u8>, String> {
    Ok(match suffix.map(|s| s.to_ascii_lowercase()) {
        None => None,
        Some(s) => match s.as_str() {
            "z" | "zp" | "b" | "1" | "zx" | "zpx" | "zy" | "zpy" | "izx" | "izpx" | "izy" | "izpy" => Some(1),
            "a" | "abs" | "w" | "2" | "ax" | "absx" | "ay" | "absy" => Some(2),
            // KickAssembler's deprecated mode extensions that pick no size
            "im" | "imm" | "i" | "ind" | "r" | "rel" => None,
            "l" | "3" => Some(3),
            _ => return Err(format!("unknown mnemonic suffix `.{s}`")),
        },
    })
}

/// Parsed 65xx-style operand, shared with the 65816 backend.
pub enum Operand {
    None,
    Acc,
    Imm(std::ops::Range<usize>),
    Direct(std::ops::Range<usize>),
    DirX(std::ops::Range<usize>),
    DirY(std::ops::Range<usize>),
    DirS(std::ops::Range<usize>),
    Ind(std::ops::Range<usize>),
    IndX(std::ops::Range<usize>),
    IndY(std::ops::Range<usize>),
    IndSY(std::ops::Range<usize>),
    IndLong(std::ops::Range<usize>),
    IndLongY(std::ops::Range<usize>),
    /// Two plain operands (65816 MVN/MVP).
    Pair(std::ops::Range<usize>, std::ops::Range<usize>),
}

/// Parse the classic 65xx addressing syntax. `(expr)` counts as indirect
/// only when `ind_ok`; otherwise it is a parenthesized expression.
pub fn parse_operand(toks: &[Token], ind_ok: bool) -> Result<Operand, String> {
    let n = toks.len();
    if n == 0 {
        return Ok(Operand::None);
    }
    if n == 1 && crate::is_reg(&toks[0], "a") {
        return Ok(Operand::Acc);
    }
    if toks[0].is_punct("#") {
        return Ok(Operand::Imm(1..n));
    }
    let parts = commas(toks);
    let reg = |r: &std::ops::Range<usize>| single_ident(toks, r).map(|s| s.to_ascii_lowercase());
    // [dp] / [dp],y
    if toks[0].is_punct("[") {
        if let Some(close) = matching(toks, 0) {
            if close == n - 1 {
                return Ok(Operand::IndLong(1..close));
            }
            if close == n - 3 && toks[n - 2].is_punct(",") && crate::is_reg(&toks[n - 1], "y") {
                return Ok(Operand::IndLongY(1..close));
            }
        }
    }
    if toks[0].is_punct("(") {
        if let Some(close) = matching(toks, 0) {
            let inner = split_top(toks, 1..close, ",");
            if close == n - 1 {
                if inner.len() == 2 {
                    match reg(&inner[1]).as_deref() {
                        Some("x") => return Ok(Operand::IndX(inner[0].clone())),
                        Some("s") => return Ok(Operand::DirS(inner[0].clone())),
                        _ => {}
                    }
                }
                if inner.len() == 1 && ind_ok {
                    return Ok(Operand::Ind(1..close));
                }
            } else if close == n - 3 && toks[n - 2].is_punct(",") && crate::is_reg(&toks[n - 1], "y") {
                if inner.len() == 2 && reg(&inner[1]).as_deref() == Some("s") {
                    return Ok(Operand::IndSY(inner[0].clone()));
                }
                if inner.len() == 1 {
                    return Ok(Operand::IndY(1..close));
                }
            }
        }
    }
    match parts.len() {
        1 => Ok(Operand::Direct(0..n)),
        2 => match reg(&parts[1]).as_deref() {
            Some("x") => Ok(Operand::DirX(parts[0].clone())),
            Some("y") => Ok(Operand::DirY(parts[0].clone())),
            Some("s") => Ok(Operand::DirS(parts[0].clone())),
            _ => Ok(Operand::Pair(parts[0].clone(), parts[1].clone())),
        },
        _ => Err("too many operands".into()),
    }
}

/// Choose zero page (1) or absolute (2) for a value; sticky per site.
pub fn pick_size(ctx: &mut dyn OperandCtx, n: Num, force: Option<u8>, has_zp: bool, has_abs: bool) -> u8 {
    let size = if let Some(f) = force.filter(|&f| if f == 1 { has_zp } else { has_abs }) {
        f
    } else if !has_abs {
        1
    } else if !has_zp || ctx.min_size() >= 2 {
        2
    } else if n.known {
        if (0..=0xff).contains(&n.value) {
            1
        } else {
            2
        }
    } else if n.zp_hint {
        1
    } else {
        2
    };
    ctx.grow(size);
    size
}

impl Cpu for Mos6502 {
    fn info(&self) -> CpuInfo {
        let (id, aliases, description): (&str, &[&str], &str) = match self.variant {
            Variant::Nmos => (
                "6502",
                &["nmos6502", "6502x", "8502", "7501", "8501", "sally"],
                "MOS 6502 family with undocumented opcodes (6502, 8502, 7501/8501, SALLY)",
            ),
            Variant::NoIllegals => ("6502ni", &["6502noillegals"], "MOS 6502, documented opcodes only"),
            Variant::M6510 => ("6510", &["nmos6510"], "MOS 6510 (C64) with undocumented opcodes"),
            Variant::Ricoh2a03 => ("2a03", &["2a07", "nes"], "Ricoh 2A03/2A07 (NES), no decimal mode"),
            Variant::M6507 => ("6507", &[], "MOS 6507 (Atari 2600), 13-bit address bus"),
            Variant::Cmos65c02 => ("65c02", &["65sc02", "w65c02", "g65sc02"], "CMOS 65C02 (Apple IIe enhanced), no Rockwell bit instructions"),
        };
        CpuInfo {
            id,
            aliases,
            description,
            endian: "little",
            addr_bits: if self.variant == Variant::M6507 { 13 } else { 16 },
        }
    }

    fn endian(&self) -> Endian {
        Endian::Little
    }

    fn has_mnemonic(&self, m: &str) -> bool {
        self.lookup(m).is_some()
    }

    fn mnemonics(&self) -> Vec<String> {
        let mut v: Vec<String> = self.ops.keys().map(|s| s.to_string()).collect();
        if !matches!(self.variant, Variant::NoIllegals | Variant::Cmos65c02) {
            v.extend(ALIASES.iter().map(|(a, _)| a.to_string()));
        }
        v.sort();
        v
    }

    fn assemble(
        &self,
        mnemonic: &str,
        suffix: Option<&str>,
        ctx: &mut dyn OperandCtx,
        _st: &CpuState,
    ) -> Result<Vec<u8>, String> {
        let modes = self
            .lookup(mnemonic)
            .ok_or_else(|| format!("unknown instruction `{mnemonic}`"))?;
        let force = size_suffix(suffix)?.map(|s| s.min(2));
        let toks = ctx.toks().to_vec();
        let operand = parse_operand(&toks, modes.contains_key(&Ind) || modes.contains_key(&Zpi))?;
        let m = mnemonic.to_ascii_lowercase();
        let one = |mode: Mode| modes.get(&mode).copied();
        let mut out = Vec::with_capacity(3);
        match &operand {
            Operand::None | Operand::Acc => {
                let op = one(Imp).ok_or_else(|| format!("`{m}` needs an operand"))?;
                out.push(op);
            }
            Operand::Imm(r) => {
                let op = one(Imm).ok_or_else(|| format!("`{m}` has no immediate mode"))?;
                let n = eval(ctx, r)?;
                out.push(op);
                let hi = if ctx.lenient_immediates() { 0xffff } else { 255 };
                out.push(fit(ctx, n, -128, hi, 8, "immediate value") as u8);
            }
            Operand::Direct(r) if modes.contains_key(&Rel) => {
                let n = eval(ctx, r)?;
                let pc = i64::from(ctx.pc());
                out.push(modes[&Rel]);
                out.push(rel(ctx, n, pc + 2, 8) as u8);
            }
            Operand::Direct(r) | Operand::DirX(r) | Operand::DirY(r)
                if !matches!(operand, Operand::Direct(_)) || modes.contains_key(&Zp) || modes.contains_key(&Abs) =>
            {
                let (zp, abs) = match operand {
                    Operand::DirX(_) => (ZpX, AbsX),
                    Operand::DirY(_) => (ZpY, AbsY),
                    _ => (Zp, Abs),
                };
                let (zop, aop) = (one(zp), one(abs));
                if zop.is_none() && aop.is_none() {
                    return Err(format!("`{m}` does not support this addressing mode"));
                }
                let n = eval(ctx, r)?;
                let size = pick_size(ctx, n, force, zop.is_some(), aop.is_some());
                if size == 1 {
                    let op = zop.ok_or_else(|| format!("`{m}` has no zero-page form"))?;
                    out.push(op);
                    out.push(fit(ctx, n, 0, 255, 8, "zero-page address") as u8);
                } else {
                    let op = aop.ok_or_else(|| format!("`{m}` has no absolute form"))?;
                    out.push(op);
                    out.extend(le16(fit(ctx, n, -32768, 65535, 16, "address")));
                }
            }
            Operand::Ind(r) => {
                let n = eval(ctx, r)?;
                // 65C02 `(zp)`; `jmp (abs)` stays 16 bit
                if let Some(op) = one(Zpi) {
                    out.push(op);
                    out.push(fit(ctx, n, 0, 255, 8, "zero-page address") as u8);
                } else {
                    let op = one(Ind).ok_or_else(|| format!("`{m}` has no indirect mode"))?;
                    out.push(op);
                    out.extend(le16(fit(ctx, n, -32768, 65535, 16, "address")));
                }
            }
            // 65C02 `jmp (abs,x)`
            Operand::IndX(r) if one(AbsXInd).is_some() && one(IndX).is_none() => {
                let n = eval(ctx, r)?;
                out.push(one(AbsXInd).unwrap());
                out.extend(le16(fit(ctx, n, -32768, 65535, 16, "address")));
            }
            Operand::IndX(r) | Operand::IndY(r) => {
                let mode = if matches!(operand, Operand::IndX(_)) { IndX } else { IndY };
                let op = one(mode).ok_or_else(|| format!("`{m}` does not support this addressing mode"))?;
                let n = eval(ctx, r)?;
                out.push(op);
                out.push(fit(ctx, n, 0, 255, 8, "zero-page address") as u8);
            }
            // KickAssembler `[ ]` are parentheses: `jmp [$1000]`, `lda [$10],y`
            Operand::IndLong(_) | Operand::IndLongY(_) => {
                let close = matching(&toks, 0).unwrap_or(0);
                let fixed: Vec<rat_core::Token> = toks.clone();
                let r = 0..close + 1;
                let mode_toks = if matches!(operand, Operand::IndLongY(_)) { (Some(AbsY), Some(ZpY)) } else { (Some(Abs), Some(Zp)) };
                let n = eval(ctx, &r)?;
                let _ = fixed;
                let (abs, zp) = (mode_toks.0.and_then(one), mode_toks.1.and_then(one));
                let size = pick_size(ctx, n, force, zp.is_some(), abs.is_some());
                if size == 1 {
                    out.push(zp.ok_or_else(|| format!("`{m}` has no zero-page form"))?);
                    out.push(fit(ctx, n, 0, 255, 8, "zero-page address") as u8);
                } else {
                    out.push(abs.ok_or_else(|| format!("`{m}` has no absolute form"))?);
                    out.extend(le16(fit(ctx, n, -32768, 65535, 16, "address")));
                }
            }
            _ => return Err(format!("`{m}`: addressing mode not available on the 6502")),
        }
        if self.variant == Variant::Ricoh2a03 && (m == "sed") {
            ctx.warn("the 2A03 has no decimal mode; SED only sets the flag".into());
        }
        Ok(out)
    }

    fn disassemble(&self, bytes: &[u8], addr: u32, _st: &CpuState) -> Option<(usize, String)> {
        let op = *bytes.first()?;
        let (m, mode) = self.table[op as usize];
        if self.variant == Variant::NoIllegals && !self.documented(op) {
            return None;
        }
        let len = mode.size();
        if bytes.len() < len {
            return None;
        }
        let b1 = u32::from(bytes.get(1).copied().unwrap_or(0));
        let w = b1 | u32::from(bytes.get(2).copied().unwrap_or(0)) << 8;
        let m = match op {
            0x2b if self.variant != Variant::Cmos65c02 => "anc2",
            0xeb if self.variant != Variant::Cmos65c02 => "sbc2",
            _ => m,
        };
        let rmw_acc = matches!(op, 0x0a | 0x2a | 0x4a | 0x6a) || (mode == Imp && matches!(m, "inc" | "dec"));
        let operand = match mode {
            Imp if rmw_acc => "a".to_owned(),
            Imp => String::new(),
            Imm => format!("#{}", hex(b1, 2)),
            Zp => hex(b1, 2),
            ZpX => format!("{},x", hex(b1, 2)),
            ZpY => format!("{},y", hex(b1, 2)),
            Abs => hex(w, 4),
            AbsX => format!("{},x", hex(w, 4)),
            AbsY => format!("{},y", hex(w, 4)),
            Ind => format!("({})", hex(w, 4)),
            IndX => format!("({},x)", hex(b1, 2)),
            IndY => format!("({}),y", hex(b1, 2)),
            Rel => hex((addr.wrapping_add(2).wrapping_add(b1 as u8 as i8 as u32)) & 0xffff, 4),
            Zpi => format!("({})", hex(b1, 2)),
            AbsXInd => format!("({},x)", hex(w, 4)),
        };
        let text = if operand.is_empty() {
            m.to_owned()
        } else {
            format!("{m} {operand}")
        };
        Some((len, text))
    }
}

/// KickAssembler opcode constant (`LDA_IMM`, `JMP_IND`, `RTS`): the
/// opcode byte for a mnemonic and addressing mode.
pub fn opcode_constant(name: &str, illegal: bool) -> Option<u8> {
    let (m, mode) = match name.split_once('_') {
        Some((m, suffix)) => (m, match suffix {
            "IMM" => Imm,
            "ZP" => Zp,
            "ZPX" => ZpX,
            "ZPY" => ZpY,
            "IZPX" => IndX,
            "IZPY" => IndY,
            "ABS" => Abs,
            "ABSX" => AbsX,
            "ABSY" => AbsY,
            "IND" => Ind,
            "REL" => Rel,
            _ => return None,
        }),
        None => (name, Imp),
    };
    if !m.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()) {
        return None;
    }
    let lm = m.to_ascii_lowercase();
    let canon = ALIASES.iter().find(|(a, _)| *a == lm).map_or(lm.as_str(), |a| a.1);
    (0..=255u8).find(|&op| {
        let (tm, tmode) = TABLE[op as usize];
        tm == canon && tmode == mode && (illegal || is_documented_opcode(op)) && (canon != "nop" || mode != Imp || op == 0xea)
    })
}

/// Instruction length of an opcode (`asmCommandSize`).
pub fn opcode_size(op: u8) -> usize {
    TABLE[op as usize].1.size()
}
