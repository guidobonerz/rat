//! MEGA65 45GS02: the CSG 4510 / 65CE02 instruction set with the
//! MEGA65's prefixes.
//!
//! Opcode matrix ported from `mimic/crates/cpu-45gs02/src/table.rs`
//! (generated there from the MEGA65 user guide). The prefix rules are
//! mimic's: `42 42` before an opcode with a Q form makes it a 32-bit Q
//! instruction (`ldq`, `adcq`, ...), `EA` before a `(zp),z` opcode makes
//! the pointer flat (`[zp],z`), `42 42 EA` both.
//!
//! Branches pick the 8-bit form when the target is in reach and the
//! 16-bit form otherwise; `lbne`, `lbra`, ... force the 16-bit form.

use std::collections::HashMap;
use std::ops::Range;

use rat_core::{Endian, Token};

use crate::mos6502::{pick_size, size_suffix};
use crate::util::*;
use crate::{Cpu, CpuInfo, CpuState, OperandCtx};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Imp,
    Acc,
    Imm,
    /// `phw #$nnnn`
    Imm16,
    Zp,
    Zpx,
    Zpy,
    Abs,
    Absx,
    Absy,
    /// `jmp/jsr ($nnnn)`
    Ind,
    /// `jmp/jsr ($nnnn,x)`
    Iax,
    Izx,
    Izy,
    /// `($nn),z`; with the `EA` prefix `[$nn],z`
    Izz,
    /// `($nn,sp),y`
    Isy,
    Rel,
    Rel16,
    /// `bbr0 $nn,target`
    Zrel,
}

use Mode::*;

#[rustfmt::skip]
pub const OPS: [(&str, Mode); 256] = [
    ("brk",Imp),("ora",Izx),("cle",Imp),("see",Imp),("tsb",Zp),("ora",Zp),("asl",Zp),("rmb0",Zp),
    ("php",Imp),("ora",Imm),("asl",Acc),("tsy",Imp),("tsb",Abs),("ora",Abs),("asl",Abs),("bbr0",Zrel),
    ("bpl",Rel),("ora",Izy),("ora",Izz),("bpl",Rel16),("trb",Zp),("ora",Zpx),("asl",Zpx),("rmb1",Zp),
    ("clc",Imp),("ora",Absy),("inc",Acc),("inz",Imp),("trb",Abs),("ora",Absx),("asl",Absx),("bbr1",Zrel),
    ("jsr",Abs),("and",Izx),("jsr",Ind),("jsr",Iax),("bit",Zp),("and",Zp),("rol",Zp),("rmb2",Zp),
    ("plp",Imp),("and",Imm),("rol",Acc),("tys",Imp),("bit",Abs),("and",Abs),("rol",Abs),("bbr2",Zrel),
    ("bmi",Rel),("and",Izy),("and",Izz),("bmi",Rel16),("bit",Zpx),("and",Zpx),("rol",Zpx),("rmb3",Zp),
    ("sec",Imp),("and",Absy),("dec",Acc),("dez",Imp),("bit",Absx),("and",Absx),("rol",Absx),("bbr3",Zrel),
    ("rti",Imp),("eor",Izx),("neg",Acc),("asr",Acc),("asr",Zp),("eor",Zp),("lsr",Zp),("rmb4",Zp),
    ("pha",Imp),("eor",Imm),("lsr",Acc),("taz",Imp),("jmp",Abs),("eor",Abs),("lsr",Abs),("bbr4",Zrel),
    ("bvc",Rel),("eor",Izy),("eor",Izz),("bvc",Rel16),("asr",Zpx),("eor",Zpx),("lsr",Zpx),("rmb5",Zp),
    ("cli",Imp),("eor",Absy),("phy",Imp),("tab",Imp),("map",Imp),("eor",Absx),("lsr",Absx),("bbr5",Zrel),
    ("rts",Imp),("adc",Izx),("rts",Imm),("bsr",Rel16),("stz",Zp),("adc",Zp),("ror",Zp),("rmb6",Zp),
    ("pla",Imp),("adc",Imm),("ror",Acc),("tza",Imp),("jmp",Ind),("adc",Abs),("ror",Abs),("bbr6",Zrel),
    ("bvs",Rel),("adc",Izy),("adc",Izz),("bvs",Rel16),("stz",Zpx),("adc",Zpx),("ror",Zpx),("rmb7",Zp),
    ("sei",Imp),("adc",Absy),("ply",Imp),("tba",Imp),("jmp",Iax),("adc",Absx),("ror",Absx),("bbr7",Zrel),
    ("bra",Rel),("sta",Izx),("sta",Isy),("bra",Rel16),("sty",Zp),("sta",Zp),("stx",Zp),("smb0",Zp),
    ("dey",Imp),("bit",Imm),("txa",Imp),("sty",Absx),("sty",Abs),("sta",Abs),("stx",Abs),("bbs0",Zrel),
    ("bcc",Rel),("sta",Izy),("sta",Izz),("bcc",Rel16),("sty",Zpx),("sta",Zpx),("stx",Zpy),("smb1",Zp),
    ("tya",Imp),("sta",Absy),("txs",Imp),("stx",Absy),("stz",Abs),("sta",Absx),("stz",Absx),("bbs1",Zrel),
    ("ldy",Imm),("lda",Izx),("ldx",Imm),("ldz",Imm),("ldy",Zp),("lda",Zp),("ldx",Zp),("smb2",Zp),
    ("tay",Imp),("lda",Imm),("tax",Imp),("ldz",Abs),("ldy",Abs),("lda",Abs),("ldx",Abs),("bbs2",Zrel),
    ("bcs",Rel),("lda",Izy),("lda",Izz),("bcs",Rel16),("ldy",Zpx),("lda",Zpx),("ldx",Zpy),("smb3",Zp),
    ("clv",Imp),("lda",Absy),("tsx",Imp),("ldz",Absx),("ldy",Absx),("lda",Absx),("ldx",Absy),("bbs3",Zrel),
    ("cpy",Imm),("cmp",Izx),("cpz",Imm),("dew",Zp),("cpy",Zp),("cmp",Zp),("dec",Zp),("smb4",Zp),
    ("iny",Imp),("cmp",Imm),("dex",Imp),("asw",Abs),("cpy",Abs),("cmp",Abs),("dec",Abs),("bbs4",Zrel),
    ("bne",Rel),("cmp",Izy),("cmp",Izz),("bne",Rel16),("cpz",Zp),("cmp",Zpx),("dec",Zpx),("smb5",Zp),
    ("cld",Imp),("cmp",Absy),("phx",Imp),("phz",Imp),("cpz",Abs),("cmp",Absx),("dec",Absx),("bbs5",Zrel),
    ("cpx",Imm),("sbc",Izx),("lda",Isy),("inw",Zp),("cpx",Zp),("sbc",Zp),("inc",Zp),("smb6",Zp),
    ("inx",Imp),("sbc",Imm),("eom",Imp),("row",Abs),("cpx",Abs),("sbc",Abs),("inc",Abs),("bbs6",Zrel),
    ("beq",Rel),("sbc",Izy),("sbc",Izz),("beq",Rel16),("phw",Imm16),("sbc",Zpx),("inc",Zpx),("smb7",Zp),
    ("sed",Imp),("sbc",Absy),("plx",Imp),("plz",Imp),("phw",Abs),("sbc",Absx),("inc",Absx),("bbs7",Zrel),
];

/// Opcodes with a Q form after `42 42` (mimic's `Q_OPS`).
const Q_OPCODES: [u8; 59] = [
    0x05, 0x06, 0x0a, 0x0d, 0x0e, 0x12, 0x16, 0x1a, 0x1e, 0x24, 0x25, 0x26, 0x2a, 0x2c, 0x2d, 0x2e, 0x32, 0x36, 0x3a, 0x3e, 0x43, 0x44, 0x45, 0x46,
    0x4a, 0x4d, 0x4e, 0x52, 0x54, 0x56, 0x5e, 0x65, 0x66, 0x6a, 0x6d, 0x6e, 0x72, 0x76, 0x7e, 0x85, 0x8d, 0x92, 0xa5, 0xad, 0xb2, 0xc5, 0xc6, 0xcd,
    0xce, 0xd2, 0xd6, 0xde, 0xe5, 0xe6, 0xed, 0xee, 0xf2, 0xf6, 0xfe,
];

fn is_q(op: u8) -> bool {
    Q_OPCODES.contains(&op)
}

fn is_izz(op: u8) -> bool {
    op & 0x1f == 0x12
}

/// Q mnemonic and the 8-bit one it is the prefixed form of.
const Q_NAMES: [(&str, &str); 16] = [
    ("ldq", "lda"),
    ("stq", "sta"),
    ("adcq", "adc"),
    ("sbcq", "sbc"),
    ("cmpq", "cmp"),
    ("eorq", "eor"),
    ("andq", "and"),
    ("orq", "ora"),
    ("bitq", "bit"),
    ("aslq", "asl"),
    ("asrq", "asr"),
    ("lsrq", "lsr"),
    ("rolq", "rol"),
    ("rorq", "ror"),
    ("inq", "inc"),
    ("deq", "dec"),
];

enum Opd {
    None,
    Acc,
    Imm(Range<usize>),
    Dir(Range<usize>),
    DirX(Range<usize>),
    DirY(Range<usize>),
    /// `(e)`
    Ind(Range<usize>),
    IndX(Range<usize>),
    IndY(Range<usize>),
    IndZ(Range<usize>),
    /// `(e,sp),y`
    IndSpY(Range<usize>),
    /// `[e]` and `[e],z`
    Flat(Range<usize>),
    Pair(Range<usize>, Range<usize>),
}

fn parse(toks: &[Token], ind_ok: bool) -> Result<Opd, String> {
    let n = toks.len();
    if n == 0 {
        return Ok(Opd::None);
    }
    if n == 1 && (crate::is_reg(&toks[0], "a") || crate::is_reg(&toks[0], "q")) {
        return Ok(Opd::Acc);
    }
    if toks[0].is_punct("#") {
        return Ok(Opd::Imm(1..n));
    }
    let reg = |r: &Range<usize>| single_ident(toks, r).map(|s| s.to_ascii_lowercase());
    let tail_reg = |close: usize| (close + 3 == n && toks[n - 2].is_punct(",")).then(|| toks[n - 1].ident().map(|s| s.to_ascii_lowercase())).flatten();
    if toks[0].is_punct("[") {
        if let Some(close) = matching(toks, 0) {
            if close == n - 1 || tail_reg(close).as_deref() == Some("z") {
                return Ok(Opd::Flat(1..close));
            }
        }
    }
    if toks[0].is_punct("(") {
        if let Some(close) = matching(toks, 0) {
            let inner = split_top(toks, 1..close, ",");
            if close == n - 1 {
                if inner.len() == 2 && reg(&inner[1]).as_deref() == Some("x") {
                    return Ok(Opd::IndX(inner[0].clone()));
                }
                if inner.len() == 1 && ind_ok {
                    return Ok(Opd::Ind(1..close));
                }
            } else if let Some(r) = tail_reg(close) {
                match (r.as_str(), inner.len()) {
                    ("y", 2) if matches!(reg(&inner[1]).as_deref(), Some("sp" | "s")) => return Ok(Opd::IndSpY(inner[0].clone())),
                    ("y", 1) => return Ok(Opd::IndY(1..close)),
                    ("z", 1) => return Ok(Opd::IndZ(1..close)),
                    _ => {}
                }
            }
        }
    }
    let parts = commas(toks);
    match parts.len() {
        1 => Ok(Opd::Dir(0..n)),
        2 => match reg(&parts[1]).as_deref() {
            Some("x") => Ok(Opd::DirX(parts[0].clone())),
            Some("y") => Ok(Opd::DirY(parts[0].clone())),
            _ => Ok(Opd::Pair(parts[0].clone(), parts[1].clone())),
        },
        _ => Err("too many operands".into()),
    }
}

pub struct M45gs02 {
    ops: HashMap<&'static str, HashMap<Mode, u8>>,
}

impl M45gs02 {
    pub fn new() -> M45gs02 {
        let mut ops: HashMap<&'static str, HashMap<Mode, u8>> = HashMap::new();
        for (op, &(m, mode)) in OPS.iter().enumerate() {
            ops.entry(m).or_default().entry(mode).or_insert(op as u8);
        }
        M45gs02 { ops }
    }

    /// Opcode modes of a mnemonic, whether it is a Q instruction, and
    /// whether it is a forced long branch.
    fn resolve(&self, m: &str) -> Option<(&HashMap<Mode, u8>, bool, bool)> {
        let m = m.to_ascii_lowercase();
        let m = match m.as_str() {
            "rtn" => "rts",
            "nop" => "eom",
            m => m,
        };
        if let Some(t) = self.ops.get(m) {
            return Some((t, false, false));
        }
        if let Some(&(_, base)) = Q_NAMES.iter().find(|x| x.0 == m) {
            return Some((&self.ops[base], true, false));
        }
        let t = self.ops.get(m.strip_prefix('l')?)?;
        t.contains_key(&Rel16).then_some((t, false, true))
    }
}

impl Default for M45gs02 {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu for M45gs02 {
    fn info(&self) -> CpuInfo {
        CpuInfo {
            id: "45gs02",
            aliases: &["4510", "65ce02", "4502", "m65"],
            description: "MEGA65 45GS02 (CSG 4510 / 65CE02 with Q instructions and flat pointers)",
            endian: "little",
            addr_bits: 28,
        }
    }

    fn endian(&self) -> Endian {
        Endian::Little
    }

    fn has_mnemonic(&self, m: &str) -> bool {
        self.resolve(m).is_some()
    }

    fn mnemonics(&self) -> Vec<String> {
        let mut v: Vec<String> = self.ops.keys().map(|s| s.to_string()).collect();
        v.extend(Q_NAMES.iter().map(|x| x.0.to_owned()));
        v.extend(self.ops.iter().filter(|(_, t)| t.contains_key(&Rel16)).map(|(m, _)| format!("l{m}")));
        v.extend(["rtn", "nop"].map(String::from));
        v.sort();
        v
    }

    fn assemble(&self, mnemonic: &str, suffix: Option<&str>, ctx: &mut dyn OperandCtx, _st: &CpuState) -> Result<Vec<u8>, String> {
        let m = mnemonic.to_ascii_lowercase();
        let (modes, q, long) = self.resolve(&m).ok_or_else(|| format!("unknown instruction `{m}`"))?;
        let force = size_suffix(suffix)?.map(|s| s.min(2));
        // an opcode of this mnemonic; a Q instruction needs its Q form
        let has = |mode: Mode| modes.get(&mode).copied().filter(|&op| !q || is_q(op));
        let no_mode = || format!("`{m}` does not support this addressing mode");
        let toks = ctx.toks().to_vec();
        let operand = parse(&toks, q || modes.contains_key(&Ind))?;
        let mut flat = false;
        let mut body: Vec<u8> = Vec::with_capacity(3);
        let zp8 = |ctx: &mut dyn OperandCtx, r: &Range<usize>| -> Result<u8, String> {
            let n = eval(ctx, r)?;
            Ok(fit(ctx, n, 0, 255, 8, "base-page address") as u8)
        };
        let abs16 = |ctx: &mut dyn OperandCtx, r: &Range<usize>| -> Result<[u8; 2], String> {
            let n = eval(ctx, r)?;
            Ok(le16(fit(ctx, n, -32768, 65535, 16, "address")))
        };
        match &operand {
            Opd::None | Opd::Acc => {
                let op = has(Acc).or_else(|| has(Imp)).ok_or_else(|| format!("`{m}` needs an operand"))?;
                body.push(op);
            }
            Opd::Imm(r) => {
                let n = eval(ctx, r)?;
                if let Some(op) = has(Imm16) {
                    body.push(op);
                    body.extend(le16(fit(ctx, n, -32768, 65535, 16, "immediate value")));
                } else {
                    body.push(has(Imm).ok_or_else(|| format!("`{m}` has no immediate mode"))?);
                    let hi = if ctx.lenient_immediates() { 0xffff } else { 255 };
                    body.push(fit(ctx, n, -128, hi, 8, "immediate value") as u8);
                }
            }
            Opd::Dir(r) if modes.contains_key(&Rel) || modes.contains_key(&Rel16) => {
                let n = eval(ctx, r)?;
                let next = i64::from(ctx.pc()) + 2;
                let (short, far) = (has(Rel), has(Rel16));
                let fits = !n.known || (-128..=127).contains(&(n.value - next));
                let use_far = match (short, far) {
                    (_, None) => false,
                    (None, _) => true,
                    _ => long || ctx.min_size() >= 2 || !fits,
                };
                ctx.grow(if use_far { 2 } else { 1 });
                if use_far {
                    // relative to the address of the last byte
                    body.push(far.unwrap());
                    body.extend(le16(rel(ctx, n, next, 16)));
                } else {
                    body.push(short.unwrap());
                    body.push(rel(ctx, n, next, 8) as u8);
                }
            }
            Opd::Dir(r) | Opd::DirX(r) | Opd::DirY(r) => {
                let (zp, abs) = match operand {
                    Opd::DirX(_) => (Zpx, Absx),
                    Opd::DirY(_) => (Zpy, Absy),
                    _ => (Zp, Abs),
                };
                let (zop, aop) = (has(zp), has(abs));
                if zop.is_none() && aop.is_none() {
                    return Err(no_mode());
                }
                let n = eval(ctx, r)?;
                if pick_size(ctx, n, force, zop.is_some(), aop.is_some()) == 1 {
                    body.push(zop.ok_or_else(|| format!("`{m}` has no base-page form"))?);
                    body.push(fit(ctx, n, 0, 255, 8, "base-page address") as u8);
                } else {
                    body.push(aop.ok_or_else(|| format!("`{m}` has no absolute form"))?);
                    body.extend(le16(fit(ctx, n, -32768, 65535, 16, "address")));
                }
            }
            // `orq ($nn)`; `jmp ($nnnn)`
            Opd::Ind(r) => {
                if q {
                    body.push(has(Izz).ok_or_else(no_mode)?);
                    body.push(zp8(ctx, r)?);
                } else {
                    body.push(has(Ind).ok_or_else(no_mode)?);
                    body.extend(abs16(ctx, r)?);
                }
            }
            Opd::IndX(r) => {
                if let Some(op) = has(Iax) {
                    body.push(op);
                    body.extend(abs16(ctx, r)?);
                } else {
                    body.push(has(Izx).ok_or_else(no_mode)?);
                    body.push(zp8(ctx, r)?);
                }
            }
            Opd::IndY(r) | Opd::IndZ(r) | Opd::IndSpY(r) | Opd::Flat(r) => {
                let mode = match operand {
                    Opd::IndY(_) => Izy,
                    Opd::IndSpY(_) => Isy,
                    _ => Izz,
                };
                flat = matches!(operand, Opd::Flat(_));
                body.push(has(mode).ok_or_else(no_mode)?);
                body.push(zp8(ctx, r)?);
            }
            Opd::Pair(a, b) => {
                let op = has(Zrel).ok_or_else(no_mode)?;
                let zp = zp8(ctx, a)?;
                let t = eval(ctx, b)?;
                let next = i64::from(ctx.pc()) + 3;
                body.extend([op, zp, rel(ctx, t, next, 8) as u8]);
            }
        }
        let mut out = Vec::with_capacity(body.len() + 3);
        if q {
            out.extend([0x42, 0x42]);
        }
        if flat {
            out.push(0xea);
        }
        out.extend(body);
        Ok(out)
    }

    fn disassemble(&self, bytes: &[u8], addr: u32, _st: &CpuState) -> Option<(usize, String)> {
        let at = |i: usize| bytes.get(i).copied();
        let mut pos = 0usize;
        let (mut q, mut flat) = (false, false);
        if at(0)? == 0x42 && at(1) == Some(0x42) {
            if at(2) == Some(0xea) && at(3).is_some_and(is_izz) {
                (q, flat, pos) = (true, true, 3);
            } else if at(2).is_some_and(is_q) {
                (q, pos) = (true, 2);
            }
        } else if at(0)? == 0xea && at(1).is_some_and(is_izz) {
            (flat, pos) = (true, 1);
        }
        let code = at(pos)?;
        let (name, mode) = OPS[usize::from(code)];
        let b1 = u32::from(at(pos + 1).unwrap_or(0));
        let w = b1 | u32::from(at(pos + 2).unwrap_or(0)) << 8;
        let size = match mode {
            Imp | Acc => 1,
            Imm16 | Abs | Absx | Absy | Ind | Iax | Rel16 | Zrel => 3,
            _ => 2,
        };
        let len = pos + size;
        if bytes.len() < len {
            return None;
        }
        let m = if q {
            Q_NAMES.iter().find(|x| x.1 == name).map_or(name.to_owned(), |x| x.0.to_owned())
        } else if mode == Rel16 && name != "bsr" {
            format!("l{name}")
        } else {
            name.to_owned()
        };
        let pc = addr.wrapping_add(len as u32);
        let operand = match mode {
            Imp => String::new(),
            Acc if q => String::new(),
            Acc => "a".into(),
            Imm => format!("#{}", hex(b1, 2)),
            Imm16 => format!("#{}", hex(w, 4)),
            Zp => hex(b1, 2),
            Zpx => format!("{},x", hex(b1, 2)),
            Zpy => format!("{},y", hex(b1, 2)),
            Abs => hex(w, 4),
            Absx => format!("{},x", hex(w, 4)),
            Absy => format!("{},y", hex(w, 4)),
            Ind => format!("({})", hex(w, 4)),
            Iax => format!("({},x)", hex(w, 4)),
            Izx => format!("({},x)", hex(b1, 2)),
            Izy => format!("({}),y", hex(b1, 2)),
            Isy => format!("({},sp),y", hex(b1, 2)),
            Izz => {
                let z = if !q || name == "lda" { ",z" } else { "" };
                if flat {
                    format!("[{}]{z}", hex(b1, 2))
                } else {
                    format!("({}){z}", hex(b1, 2))
                }
            }
            Rel => hex(pc.wrapping_add(b1 as u8 as i8 as u32) & 0xffff, 4),
            Rel16 => hex(pc.wrapping_sub(1).wrapping_add(w) & 0xffff, 4),
            Zrel => format!("{},{}", hex(b1, 2), hex(pc.wrapping_add((w >> 8) as u8 as i8 as u32) & 0xffff, 4)),
        };
        Some((len, if operand.is_empty() { m } else { format!("{m} {operand}") }))
    }
}
