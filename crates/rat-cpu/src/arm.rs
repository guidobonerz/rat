//! ARM2 (Acorn Archimedes), 26-bit address space, Acorn mnemonics.
//!
//! Encodings as in `mimic/crates/cpu-arm/src/disasm.rs`: data
//! processing, MUL/MLA, LDR/STR, LDM/STM, B/BL and SWI. A mnemonic is
//! the operation, an optional condition and optional flags (`ADDEQS`,
//! `LDRNEB`, `TEQP`, `STMFD`, `BLLT`); the flags may also stand before
//! the condition.
//!
//! Conveniences: `ADR Rd,label` (ADD/SUB from PC), `LDR Rd,label`
//! (PC-relative), `NOP` (`MOV R0,R0`), and an immediate that cannot be
//! encoded is tried with the opposite instruction (MOV/MVN, AND/BIC,
//! ADD/SUB, CMP/CMN).

use std::ops::Range;

use rat_core::{Endian, Token};

use crate::util::*;
use crate::{Cpu, CpuInfo, CpuState, Num, OperandCtx};

const COND: [&str; 16] = ["eq", "ne", "cs", "cc", "mi", "pl", "vs", "vc", "hi", "ls", "ge", "lt", "gt", "le", "al", "nv"];
const DP: [&str; 16] = ["and", "eor", "sub", "rsb", "add", "adc", "sbc", "rsc", "tst", "teq", "cmp", "cmn", "orr", "mov", "bic", "mvn"];
const SHIFTS: [&str; 4] = ["lsl", "lsr", "asr", "ror"];
const AL: u32 = 14;

fn cond(s: &str) -> Option<u32> {
    match s {
        "hs" => Some(2),
        "lo" => Some(3),
        _ => COND.iter().position(|c| *c == s).map(|i| i as u32),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Opcode, S flag, P flag.
    Dp(u32, bool, bool),
    Mul { acc: bool, s: bool },
    Mem { load: bool, byte: bool, t: bool },
    /// P and U bits.
    Block { load: bool, p: bool, u: bool },
    Branch { link: bool },
    Swi,
    Adr,
    Nop,
}

/// Condition and flag letters after the operation name, in either order.
fn tail(rest: &str) -> (u32, &str) {
    let n = rest.len();
    if n >= 2 {
        if let Some(c) = rest.get(..2).and_then(cond) {
            return (c, &rest[2..]);
        }
        if let Some(c) = rest.get(n - 2..).and_then(cond) {
            return (c, &rest[..n - 2]);
        }
    }
    (AL, rest)
}

fn parse_mnemonic(m: &str) -> Option<(Kind, u32)> {
    let m = m.to_ascii_lowercase();
    let m = m.as_str();
    if m == "nop" {
        return Some((Kind::Nop, AL));
    }
    // B<cond> before BL<cond>: BLT, BLS, BLE and BLO are branches
    for (prefix, link) in [("b", false), ("bl", true)] {
        if let Some(rest) = m.strip_prefix(prefix) {
            if rest.is_empty() {
                return Some((Kind::Branch { link }, AL));
            }
            if let Some(c) = cond(rest) {
                return Some((Kind::Branch { link }, c));
            }
        }
    }
    let (base, rest) = (m.get(..3)?, m.get(3..)?);
    let (c, flags) = tail(rest);
    let kind = match base {
        "mul" | "mla" if matches!(flags, "" | "s") => Kind::Mul { acc: base == "mla", s: flags == "s" },
        "ldr" | "str" if matches!(flags, "" | "b" | "t" | "bt") => Kind::Mem { load: base == "ldr", byte: flags.starts_with('b'), t: flags.ends_with('t') },
        "ldm" | "stm" => {
            let load = base == "ldm";
            let (p, u) = match (flags, load) {
                ("ia", _) | ("fd", true) | ("ea", false) => (false, true),
                ("ib", _) | ("ed", true) | ("fa", false) => (true, true),
                ("da", _) | ("fa", true) | ("ed", false) => (false, false),
                ("db", _) | ("ea", true) | ("fd", false) => (true, false),
                _ => return None,
            };
            Kind::Block { load, p, u }
        }
        "swi" if flags.is_empty() => Kind::Swi,
        "adr" if flags.is_empty() => Kind::Adr,
        _ => {
            let op = DP.iter().position(|d| *d == base)? as u32;
            let test = (8..=11).contains(&op);
            match flags {
                "" => Kind::Dp(op, test, false),
                "s" if !test => Kind::Dp(op, true, false),
                "p" if test => Kind::Dp(op, true, true),
                _ => return None,
            }
        }
    };
    Some((kind, c))
}

fn reg_name(s: &str) -> Option<u32> {
    let s = s.to_ascii_lowercase();
    match s.as_str() {
        "sp" => return Some(13),
        "lr" => return Some(14),
        "pc" => return Some(15),
        _ => {}
    }
    let n: u32 = s.strip_prefix('r')?.parse().ok()?;
    (n < 16 && !(s.len() > 2 && s.starts_with("r0"))).then_some(n)
}

fn reg(toks: &[Token], r: &Range<usize>) -> Result<u32, String> {
    single_ident(toks, r).and_then(reg_name).ok_or_else(|| "expected a register (R0..R15, SP, LR, PC)".to_string())
}

/// An 8-bit value rotated right by an even amount, as the 12-bit field.
fn encode_imm(v: u32) -> Option<u32> {
    (0..16).find_map(|rot| {
        let x = v.rotate_left(rot * 2);
        (x < 256).then_some(rot << 8 | x)
    })
}

/// `LSL #n`, `LSR Rs`, `RRX`: bits 11..4 of the shifted register operand.
fn shift(ctx: &mut dyn OperandCtx, toks: &[Token], r: &Range<usize>, reg_ok: bool) -> Result<u32, String> {
    let name = toks.get(r.start).and_then(|t| t.ident()).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    let rest = r.start + 1..r.end;
    if name == "rrx" && rest.is_empty() {
        return Ok(3 << 5);
    }
    let kind = match name.as_str() {
        "asl" => 0,
        n => SHIFTS.iter().position(|s| *s == n).ok_or("expected a shift (LSL, LSR, ASR, ROR, RRX)")? as u32,
    };
    if rest.is_empty() {
        return Err("missing shift amount".into());
    }
    if toks[rest.start].is_punct("#") {
        let n = eval(ctx, &(rest.start + 1..rest.end))?;
        let (lo, hi) = match kind {
            0 => (0, 31),
            3 => (1, 31),
            _ => (1, 32),
        };
        if n.known && !(lo..=hi).contains(&n.value) {
            ctx.error(format!("shift amount out of range: {} (allowed {lo}..{hi})", n.value));
        }
        return Ok((n.value as u32 & 31) << 7 | kind << 5);
    }
    if !reg_ok {
        return Err("the shift amount must be a constant here".into());
    }
    Ok(reg(toks, &rest)? << 8 | kind << 5 | 0x10)
}

/// `Rm` with an optional shift in the following part.
fn shifted(ctx: &mut dyn OperandCtx, toks: &[Token], parts: &[Range<usize>], reg_ok: bool) -> Result<u32, String> {
    let rm = reg(toks, &parts[0])?;
    match parts.len() {
        1 => Ok(rm),
        2 => Ok(shift(ctx, toks, &parts[1], reg_ok)? | rm),
        _ => Err("too many operands".into()),
    }
}

/// A value as an unsigned 32-bit number.
fn value32(ctx: &mut dyn OperandCtx, n: Num) -> u32 {
    if n.known && !(-(1i64 << 31)..1i64 << 32).contains(&n.value) {
        ctx.error(format!("value out of range: {}", n.value));
    }
    n.value as u32
}

/// Offset of LDR/STR: I bit, U bit and the 12-bit field.
fn offset(ctx: &mut dyn OperandCtx, toks: &[Token], parts: &[Range<usize>]) -> Result<(bool, bool, u32), String> {
    let Some(first) = parts.first() else { return Ok((false, true, 0)) };
    if first.is_empty() {
        return Err("missing offset".into());
    }
    if toks[first.start].is_punct("#") {
        if parts.len() > 1 {
            return Err("too many operands".into());
        }
        let n = eval(ctx, &(first.start + 1..first.end))?;
        if n.known && n.value.abs() > 4095 {
            ctx.error(format!("offset out of range: {} (allowed -4095..4095)", n.value));
        }
        // `#-0` keeps its sign
        let minus_zero = n.value == 0 && toks[first.start + 1].is_punct("-");
        return Ok((false, n.value >= 0 && !minus_zero, n.value.unsigned_abs() as u32 & 0xfff));
    }
    let (up, start) = match &toks[first.start] {
        t if t.is_punct("-") => (false, first.start + 1),
        t if t.is_punct("+") => (true, first.start + 1),
        _ => (true, first.start),
    };
    let mut p = parts.to_vec();
    p[0] = start..first.end;
    Ok((true, up, shifted(ctx, toks, &p, false)?))
}

pub struct Arm2;

impl Arm2 {
    pub fn new() -> Arm2 {
        Arm2
    }
}

impl Default for Arm2 {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu for Arm2 {
    fn info(&self) -> CpuInfo {
        CpuInfo {
            id: "arm2",
            aliases: &["arm", "arm26", "arm3"],
            description: "ARM2 (Acorn Archimedes), 26-bit, Acorn mnemonics",
            endian: "little",
            addr_bits: 26,
        }
    }

    fn endian(&self) -> Endian {
        Endian::Little
    }

    fn unit(&self) -> usize {
        4
    }

    fn has_mnemonic(&self, m: &str) -> bool {
        parse_mnemonic(m).is_some()
    }

    fn mnemonics(&self) -> Vec<String> {
        let mut v: Vec<String> = DP.iter().map(|s| (*s).to_owned()).collect();
        v.extend(["adr", "b", "bl", "ldm", "ldr", "mla", "mul", "nop", "stm", "str", "swi"].map(String::from));
        v.sort();
        v
    }

    fn assemble(&self, mnemonic: &str, _suffix: Option<&str>, ctx: &mut dyn OperandCtx, _st: &CpuState) -> Result<Vec<u8>, String> {
        let (kind, c) = parse_mnemonic(mnemonic).ok_or_else(|| format!("unknown instruction `{mnemonic}`"))?;
        let m = mnemonic.to_ascii_lowercase();
        let toks = ctx.toks().to_vec();
        let parts = commas(&toks);
        let need = |lo: usize, hi: usize| {
            if (lo..=hi).contains(&parts.len()) {
                Ok(())
            } else {
                Err(format!("`{m}`: wrong number of operands"))
            }
        };
        let pc = i64::from(ctx.pc());
        let word: u32 = match kind {
            Kind::Nop => {
                need(0, 0)?;
                0x01a0_0000
            }
            Kind::Dp(op, s, p) => {
                let test = (8..=11).contains(&op);
                let (rd, rn, at) = if test {
                    need(2, 3)?;
                    (if p { 15 } else { 0 }, reg(&toks, &parts[0])?, 1)
                } else if op == 13 || op == 15 {
                    need(2, 3)?;
                    (reg(&toks, &parts[0])?, 0, 1)
                } else {
                    need(3, 4)?;
                    (reg(&toks, &parts[0])?, reg(&toks, &parts[1])?, 2)
                };
                let op2 = &parts[at..];
                let mut op = op;
                let field = if !op2[0].is_empty() && toks[op2[0].start].is_punct("#") {
                    if op2.len() > 1 {
                        return Err("too many operands".into());
                    }
                    let n = eval(ctx, &(op2[0].start + 1..op2[0].end))?;
                    let v = value32(ctx, n);
                    // the opposite instruction with the complement or negative
                    let other = match op {
                        13 | 15 => Some((28 - op, !v)),
                        0 | 14 => Some((14 - op, !v)),
                        4 | 2 => Some((6 - op, v.wrapping_neg())),
                        10 | 11 => Some((21 - op, v.wrapping_neg())),
                        _ => None,
                    };
                    let enc = encode_imm(v).or_else(|| {
                        let (o, w) = other?;
                        let e = encode_imm(w)?;
                        op = o;
                        Some(e)
                    });
                    match enc {
                        Some(e) => 1 << 25 | e,
                        None => {
                            if n.known {
                                ctx.error(format!("immediate value ${v:x} cannot be encoded (8 bits rotated by an even amount)"));
                            }
                            1 << 25
                        }
                    }
                } else {
                    shifted(ctx, &toks, op2, true)?
                };
                op << 21 | u32::from(s) << 20 | rn << 16 | rd << 12 | field
            }
            Kind::Mul { acc, s } => {
                let n = if acc { 4 } else { 3 };
                need(n, n)?;
                let (rd, rm, rs) = (reg(&toks, &parts[0])?, reg(&toks, &parts[1])?, reg(&toks, &parts[2])?);
                let rn = if acc { reg(&toks, &parts[3])? } else { 0 };
                u32::from(acc) << 21 | u32::from(s) << 20 | rd << 16 | rn << 12 | rs << 8 | 0x90 | rm
            }
            Kind::Mem { load, byte, t } => {
                need(2, 4)?;
                let rd = reg(&toks, &parts[0])?;
                let a = &parts[1];
                let (rn, pre, wb, (imm_reg, up, field));
                if !a.is_empty() && toks[a.start].is_punct("[") {
                    let close = matching(&toks, a.start).ok_or("missing `]`")?;
                    let inner = split_top(&toks, a.start + 1..close, ",");
                    rn = reg(&toks, &inner[0])?;
                    let bang = close + 2 == a.end && toks[close + 1].is_punct("!");
                    if close + 1 != a.end && !bang {
                        return Err("unexpected text after `]`".into());
                    }
                    if parts.len() == 2 {
                        if t {
                            return Err(format!("`{m}` needs a post-indexed address (`[Rn],offset`)"));
                        }
                        (pre, wb) = (true, bang);
                        (imm_reg, up, field) = offset(ctx, &toks, &inner[1..])?;
                    } else {
                        if inner.len() != 1 || bang {
                            return Err("a post-indexed address is written `[Rn],offset`".into());
                        }
                        (pre, wb) = (false, t);
                        (imm_reg, up, field) = offset(ctx, &toks, &parts[2..])?;
                    }
                } else {
                    // a label: PC-relative
                    need(2, 2)?;
                    if t {
                        return Err(format!("`{m}` needs a post-indexed address (`[Rn],offset`)"));
                    }
                    let n = eval(ctx, a)?;
                    let d = if n.known { n.value - (pc + 8) } else { 0 };
                    if d.abs() > 4095 {
                        ctx.error(format!("address out of reach of a PC-relative load ({d} bytes, allowed -4095..4095)"));
                    }
                    (rn, pre, wb) = (15, true, false);
                    (imm_reg, up, field) = (false, d >= 0, d.unsigned_abs() as u32 & 0xfff);
                }
                1 << 26 | u32::from(imm_reg) << 25 | u32::from(pre) << 24 | u32::from(up) << 23 | u32::from(byte) << 22 | u32::from(wb) << 21 | u32::from(load) << 20 | rn << 16 | rd << 12 | field
            }
            Kind::Block { load, p, u } => {
                need(2, 2)?;
                let a = &parts[0];
                let wb = a.len() == 2 && toks[a.end - 1].is_punct("!");
                let rn = reg(&toks, &(a.start..if wb { a.end - 1 } else { a.end }))?;
                let l = &parts[1];
                let user = l.len() > 2 && toks[l.end - 1].is_punct("^");
                let l = l.start..if user { l.end - 1 } else { l.end };
                if !wrapped(&toks, &l, "{", "}") {
                    return Err("expected a register list `{R0-R3,R14}`".into());
                }
                let mut list = 0u32;
                for item in split_top(&toks, l.start + 1..l.end - 1, ",") {
                    let range = split_top(&toks, item.clone(), "-");
                    let (lo, hi) = match range.len() {
                        1 => (reg(&toks, &range[0])?, reg(&toks, &range[0])?),
                        2 => (reg(&toks, &range[0])?, reg(&toks, &range[1])?),
                        _ => return Err("bad register range".into()),
                    };
                    for r in lo.min(hi)..=lo.max(hi) {
                        list |= 1 << r;
                    }
                }
                4 << 25 | u32::from(p) << 24 | u32::from(u) << 23 | u32::from(user) << 22 | u32::from(wb) << 21 | u32::from(load) << 20 | rn << 16 | list
            }
            Kind::Branch { link } => {
                need(1, 1)?;
                let n = eval(ctx, &parts[0])?;
                let d = if n.known { n.value - (pc + 8) } else { 0 };
                // the 26-bit program counter wraps around
                let d = (d + (1 << 25)).rem_euclid(1 << 26) - (1 << 25);
                if d & 3 != 0 {
                    ctx.error("branch target is not word aligned".into());
                } else if n.known && !(0..1 << 26).contains(&n.value) {
                    ctx.error(format!("branch target ${:x} outside the 26-bit address space", n.value));
                }
                5 << 25 | u32::from(link) << 24 | (d >> 2) as u32 & 0x00ff_ffff
            }
            Kind::Swi => {
                need(1, 1)?;
                let n = eval(ctx, &parts[0])?;
                0x0f00_0000 | fit(ctx, n, 0, 0x00ff_ffff, 24, "SWI number") as u32
            }
            Kind::Adr => {
                need(2, 2)?;
                let rd = reg(&toks, &parts[0])?;
                let n = eval(ctx, &parts[1])?;
                let d = (if n.known { n.value - (pc + 8) } else { 0 }) as u32;
                let (op, e) = match (encode_imm(d), encode_imm(d.wrapping_neg())) {
                    (Some(e), _) => (4, e),
                    (None, Some(e)) => (2, e),
                    _ => {
                        ctx.error("address out of reach of ADR (the distance must fit 8 bits rotated by an even amount)".into());
                        (4, 0)
                    }
                };
                1 << 25 | op << 21 | 15 << 16 | rd << 12 | e
            }
        };
        Ok((c << 28 | word).to_le_bytes().to_vec())
    }

    fn disassemble(&self, bytes: &[u8], addr: u32, _st: &CpuState) -> Option<(usize, String)> {
        let i = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?);
        let c = COND[(i >> 28) as usize];
        let c = if c == "al" { "" } else { c };
        let r = |n: u32| match n & 15 {
            15 => "pc".to_owned(),
            n => format!("r{n}"),
        };
        let bit = |n: u32| i >> n & 1 != 0;
        // Rm with its shift; `None` for the multiply pattern
        let shifted = |with_reg: bool| -> Option<String> {
            let rm = r(i);
            let kind = (i >> 5 & 3) as usize;
            if bit(4) {
                return (with_reg && !bit(7)).then(|| format!("{rm},{} {}", SHIFTS[kind], r(i >> 8)));
            }
            Some(match (kind, i >> 7 & 31) {
                (0, 0) => rm,
                (3, 0) => format!("{rm},rrx"),
                (1 | 2, 0) => format!("{rm},{} #32", SHIFTS[kind]),
                (_, n) => format!("{rm},{} #{n}", SHIFTS[kind]),
            })
        };
        let text = match i >> 25 & 7 {
            0 if i & 0x0fc0_00f0 == 0x90 => {
                let s = if bit(20) { "s" } else { "" };
                if bit(21) {
                    format!("mla{c}{s} {},{},{},{}", r(i >> 16), r(i), r(i >> 8), r(i >> 12))
                } else {
                    format!("mul{c}{s} {},{},{}", r(i >> 16), r(i), r(i >> 8))
                }
            }
            0 | 1 => {
                let op = (i >> 21 & 15) as usize;
                let (rd, rn) = (i >> 12 & 15, i >> 16);
                let op2 = if bit(25) { format!("#{}", hex((i & 0xff).rotate_right((i >> 8 & 15) * 2), 1)) } else { shifted(true)? };
                let s = if bit(20) { "s" } else { "" };
                match op {
                    8..=11 => {
                        if !bit(20) {
                            return None;
                        }
                        format!("{}{c}{} {},{op2}", DP[op], if rd == 15 { "p" } else { "" }, r(rn))
                    }
                    13 | 15 => format!("{}{c}{s} {},{op2}", DP[op], r(rd)),
                    _ => format!("{}{c}{s} {},{},{op2}", DP[op], r(rd), r(rn)),
                }
            }
            2 | 3 => {
                let (pre, wb) = (bit(24), bit(21));
                let sign = if bit(23) { "" } else { "-" };
                let off = if bit(25) { format!("{sign}{}", shifted(false)?) } else { format!("#{sign}{}", hex(i & 0xfff, 1)) };
                let m = format!("{}{c}{}{}", if bit(20) { "ldr" } else { "str" }, if bit(22) { "b" } else { "" }, if !pre && wb { "t" } else { "" });
                let (rd, rn) = (r(i >> 12), r(i >> 16));
                if pre {
                    format!("{m} {rd},[{rn},{off}]{}", if wb { "!" } else { "" })
                } else {
                    format!("{m} {rd},[{rn}],{off}")
                }
            }
            4 => {
                let dir = match (bit(23), bit(24)) {
                    (true, false) => "ia",
                    (true, true) => "ib",
                    (false, false) => "da",
                    (false, true) => "db",
                };
                let regs: Vec<String> = (0..16).filter(|n| i >> n & 1 != 0).map(r).collect();
                if regs.is_empty() {
                    return None;
                }
                format!("{}{c}{dir} {}{},{{{}}}{}", if bit(20) { "ldm" } else { "stm" }, r(i >> 16), if bit(21) { "!" } else { "" }, regs.join(","), if bit(22) { "^" } else { "" })
            }
            5 => {
                let dest = addr.wrapping_add(8).wrapping_add(((i << 8) as i32 >> 6) as u32) & 0x03ff_ffff;
                format!("b{}{c} {}", if bit(24) { "l" } else { "" }, hex(dest, 1))
            }
            7 if bit(24) => format!("swi{c} {}", hex(i & 0x00ff_ffff, 1)),
            _ => return None,
        };
        Some((4, text))
    }
}
