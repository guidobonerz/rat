//! Texas Instruments TMS9900 (TI-99/4A).
//!
//! Instruction formats as in `mimic/crates/cpu-tms9900/src/disasm.rs`.
//! Operands use TI's syntax: `R1`, `*R1`, `*R1+`, `@addr`, `@addr(R2)`;
//! numbers are written as everywhere in rat (`$8C00`, not `>8C00`). A
//! workspace register may also be a plain number 0..15. Jump targets
//! are addresses. `NOP` (`JMP $+2`) and `RT` (`B *R11`) are the usual
//! pseudo-instructions.

use std::ops::Range;

use rat_core::{Endian, Token};

use crate::util::*;
use crate::{Cpu, CpuInfo, CpuState, Num, OperandCtx};

const FORMAT1: [&str; 12] = ["szc", "szcb", "s", "sb", "c", "cb", "a", "ab", "mov", "movb", "soc", "socb"];
const JUMPS: [&str; 13] = ["jmp", "jlt", "jle", "jeq", "jhe", "jgt", "jne", "jnc", "joc", "jno", "jl", "jh", "jop"];
const CRU: [&str; 3] = ["sbo", "sbz", "tb"];
const FORMAT3: [&str; 8] = ["coc", "czc", "xor", "xop", "ldcr", "stcr", "mpy", "div"];
const SHIFTS: [&str; 4] = ["sra", "srl", "sla", "src"];
const FORMAT6: [&str; 14] = ["blwp", "b", "x", "clr", "neg", "inv", "inc", "inct", "dec", "dect", "bl", "swpb", "seto", "abs"];
/// `Rw, immediate`.
const IMM: [(&str, u16); 5] = [("li", 0x0200), ("ai", 0x0220), ("andi", 0x0240), ("ori", 0x0260), ("ci", 0x0280)];
/// `Rw`.
const REG_ONLY: [(&str, u16); 2] = [("stwp", 0x02a0), ("stst", 0x02c0)];
/// `immediate`.
const IMM_ONLY: [(&str, u16); 2] = [("lwpi", 0x02e0), ("limi", 0x0300)];
const NO_ARG: [(&str, u16); 8] = [
    ("idle", 0x0340),
    ("rset", 0x0360),
    ("rtwp", 0x0380),
    ("ckon", 0x03a0),
    ("ckof", 0x03c0),
    ("lrex", 0x03e0),
    ("nop", 0x1000),
    ("rt", 0x045b),
];

pub struct Tms9900 {
    mnemonics: Vec<String>,
}

impl Tms9900 {
    pub fn new() -> Tms9900 {
        let mut m: Vec<String> = Vec::new();
        for t in [&FORMAT1[..], &JUMPS, &CRU, &FORMAT3, &SHIFTS, &FORMAT6] {
            m.extend(t.iter().map(|x| (*x).to_owned()));
        }
        for t in [&IMM[..], &REG_ONLY, &IMM_ONLY, &NO_ARG] {
            m.extend(t.iter().map(|x| x.0.to_owned()));
        }
        m.sort();
        m.dedup();
        Tms9900 { mnemonics: m }
    }
}

impl Default for Tms9900 {
    fn default() -> Self {
        Self::new()
    }
}

fn index(table: &[&str], name: &str) -> Option<u16> {
    table.iter().position(|t| *t == name).map(|i| i as u16)
}

fn reg_name(s: &str) -> Option<u16> {
    let s = s.to_ascii_lowercase();
    let n: u16 = s.strip_prefix('r')?.parse().ok()?;
    (n < 16 && !(s.len() > 2 && s.starts_with("r0"))).then_some(n)
}

/// A workspace register: `R0`..`R15` or an expression 0..15.
fn register(ctx: &mut dyn OperandCtx, toks: &[Token], r: &Range<usize>) -> Result<u16, String> {
    if let Some(n) = single_ident(toks, r).and_then(reg_name) {
        return Ok(n);
    }
    let n = eval(ctx, r)?;
    if n.known && !(0..=15).contains(&n.value) {
        return Err(format!("register number {} out of range (0..15); a memory address is written `@addr`", n.value));
    }
    Ok(n.value as u16 & 15)
}

/// A general operand: addressing mode, register and the address word of
/// the symbolic and indexed modes.
fn general(ctx: &mut dyn OperandCtx, toks: &[Token], r: &Range<usize>) -> Result<(u16, u16, Option<Num>), String> {
    if r.is_empty() {
        return Err("missing operand".into());
    }
    if toks[r.start].is_punct("*") {
        let inc = toks[r.end - 1].is_punct("+");
        let end = if inc { r.end - 1 } else { r.end };
        let reg = register(ctx, toks, &(r.start + 1..end))?;
        return Ok((if inc { 3 } else { 1 }, reg, None));
    }
    if toks[r.start].is_punct("@") {
        let mut end = r.end;
        let mut reg = 0;
        // `@addr(Rn)`
        if end >= r.start + 5 && toks[end - 1].is_punct(")") && toks[end - 3].is_punct("(") {
            if let Some(n) = toks[end - 2].ident().and_then(reg_name) {
                if n == 0 {
                    return Err("R0 cannot be an index register".into());
                }
                reg = n;
                end -= 3;
            }
        }
        let n = eval(ctx, &(r.start + 1..end))?;
        return Ok((2, reg, Some(n)));
    }
    Ok((0, register(ctx, toks, r)?, None))
}

fn words(op: u16, ext: &[Option<Num>], ctx: &mut dyn OperandCtx) -> Vec<u8> {
    let mut out = op.to_be_bytes().to_vec();
    for n in ext.iter().flatten() {
        out.extend(be16(fit(ctx, *n, -32768, 65535, 16, "address")));
    }
    out
}

impl Cpu for Tms9900 {
    fn info(&self) -> CpuInfo {
        CpuInfo {
            id: "tms9900",
            aliases: &["9900", "tms9900a"],
            description: "Texas Instruments TMS9900 (TI-99/4A)",
            endian: "big",
            addr_bits: 16,
        }
    }

    fn endian(&self) -> Endian {
        Endian::Big
    }

    fn unit(&self) -> usize {
        2
    }

    fn has_mnemonic(&self, m: &str) -> bool {
        self.mnemonics.binary_search(&m.to_ascii_lowercase()).is_ok()
    }

    fn mnemonics(&self) -> Vec<String> {
        self.mnemonics.clone()
    }

    fn assemble(&self, mnemonic: &str, _suffix: Option<&str>, ctx: &mut dyn OperandCtx, _st: &CpuState) -> Result<Vec<u8>, String> {
        let m = mnemonic.to_ascii_lowercase();
        let m = m.as_str();
        let toks = ctx.toks().to_vec();
        let parts = commas(&toks);
        let need = |n: usize| {
            if parts.len() == n {
                Ok(())
            } else {
                Err(format!("`{m}` takes {n} operand{}", if n == 1 { "" } else { "s" }))
            }
        };
        if let Some(&(_, op)) = NO_ARG.iter().find(|x| x.0 == m) {
            need(0)?;
            return Ok(op.to_be_bytes().to_vec());
        }
        if let Some(i) = index(&FORMAT1, m) {
            need(2)?;
            let (ts, s, es) = general(ctx, &toks, &parts[0])?;
            let (td, d, ed) = general(ctx, &toks, &parts[1])?;
            return Ok(words((i + 4) << 12 | td << 10 | d << 6 | ts << 4 | s, &[es, ed], ctx));
        }
        if let Some(i) = index(&JUMPS, m) {
            need(1)?;
            let t = eval(ctx, &parts[0])?;
            let mut disp = 0;
            if t.known {
                let d = t.value - (i64::from(ctx.pc()) + 2);
                if d & 1 != 0 {
                    ctx.error("jump target is at an odd address".into());
                } else if !(-256..=254).contains(&d) {
                    ctx.error(format!("jump out of range ({d} bytes, allowed -256..254)"));
                }
                disp = (d >> 1) as u16 & 0xff;
            }
            return Ok((0x1000 | i << 8 | disp).to_be_bytes().to_vec());
        }
        if let Some(i) = index(&CRU, m) {
            need(1)?;
            let n = eval(ctx, &parts[0])?;
            let d = fit(ctx, n, -128, 255, 8, "CRU bit displacement") as u16;
            return Ok(((0x1d00 + (i << 8)) | d).to_be_bytes().to_vec());
        }
        if let Some(i) = index(&FORMAT3, m) {
            need(2)?;
            let (ts, s, es) = general(ctx, &toks, &parts[0])?;
            let d = match i {
                // XOP number
                3 => {
                    let n = eval(ctx, &parts[1])?;
                    fit(ctx, n, 0, 15, 4, "XOP number") as u16
                }
                // LDCR/STCR bit count: 16 is written as 0
                4 | 5 => {
                    let n = eval(ctx, &parts[1])?;
                    fit(ctx, n, 0, 16, 4, "bit count") as u16
                }
                _ => register(ctx, &toks, &parts[1])?,
            };
            return Ok(words(0x2000 | i << 10 | d << 6 | ts << 4 | s, &[es], ctx));
        }
        if let Some(i) = index(&SHIFTS, m) {
            need(2)?;
            let w = register(ctx, &toks, &parts[0])?;
            let n = eval(ctx, &parts[1])?;
            let c = fit(ctx, n, 0, 15, 4, "shift count") as u16;
            return Ok((0x0800 | i << 8 | c << 4 | w).to_be_bytes().to_vec());
        }
        if let Some(i) = index(&FORMAT6, m) {
            need(1)?;
            let (ts, s, es) = general(ctx, &toks, &parts[0])?;
            return Ok(words((0x0400 + (i << 6)) | ts << 4 | s, &[es], ctx));
        }
        if let Some(&(_, op)) = IMM.iter().find(|x| x.0 == m) {
            need(2)?;
            let w = register(ctx, &toks, &parts[0])?;
            let n = eval(ctx, &parts[1])?;
            return Ok(words(op | w, &[Some(n)], ctx));
        }
        if let Some(&(_, op)) = REG_ONLY.iter().find(|x| x.0 == m) {
            need(1)?;
            return Ok((op | register(ctx, &toks, &parts[0])?).to_be_bytes().to_vec());
        }
        if let Some(&(_, op)) = IMM_ONLY.iter().find(|x| x.0 == m) {
            need(1)?;
            let n = eval(ctx, &parts[0])?;
            return Ok(words(op, &[Some(n)], ctx));
        }
        Err(format!("unknown instruction `{m}`"))
    }

    fn disassemble(&self, bytes: &[u8], addr: u32, _st: &CpuState) -> Option<(usize, String)> {
        let word = |i: usize| Some(u16::from_be_bytes([*bytes.get(i * 2)?, *bytes.get(i * 2 + 1)?]));
        let op = word(0)?;
        let mut len = 1usize;
        let mut gen = |mode: u16, reg: u16| -> Option<String> {
            Some(match mode {
                0 => format!("r{reg}"),
                1 => format!("*r{reg}"),
                2 => {
                    let v = u32::from(word(len)?);
                    len += 1;
                    if reg == 0 {
                        format!("@{}", hex(v, 4))
                    } else {
                        format!("@{}(r{reg})", hex(v, 4))
                    }
                }
                _ => format!("*r{reg}+"),
            })
        };
        let text = match op >> 12 {
            0x4..=0xf => {
                let s = gen(op >> 4 & 3, op & 15)?;
                let d = gen(op >> 10 & 3, op >> 6 & 15)?;
                format!("{} {s},{d}", FORMAT1[usize::from((op >> 12) - 4)])
            }
            0x1 => {
                let disp = (op & 0xff) as i8;
                let code = usize::from(op >> 8 & 0x0f);
                if op == 0x1000 {
                    "nop".to_owned()
                } else if code < JUMPS.len() {
                    let t = addr.wrapping_add(2).wrapping_add((i32::from(disp) * 2) as u32) & 0xffff;
                    format!("{} {}", JUMPS[code], hex(t, 4))
                } else {
                    format!("{} {disp}", CRU[code - 13])
                }
            }
            0x2 | 0x3 => {
                let k = usize::from(op >> 10 & 7);
                let d = op >> 6 & 15;
                let s = gen(op >> 4 & 3, op & 15)?;
                match k {
                    3..=5 => format!("{} {s},{d}", FORMAT3[k]),
                    _ => format!("{} {s},r{d}", FORMAT3[k]),
                }
            }
            _ => match op >> 8 {
                0x08..=0x0b => format!("{} r{},{}", SHIFTS[usize::from((op >> 8) - 8)], op & 15, op >> 4 & 15),
                0x04..=0x07 if op < 0x0780 => {
                    let k = usize::from((op - 0x0400) >> 6);
                    format!("{} {}", FORMAT6[k], gen(op >> 4 & 3, op & 15)?)
                }
                0x02 | 0x03 => {
                    let w = op & 15;
                    let base = op & 0xffe0;
                    if let Some(&(m, _)) = IMM.iter().find(|x| x.1 == base) {
                        len = 2;
                        format!("{m} r{w},{}", hex(u32::from(word(1)?), 4))
                    } else if let Some(&(m, _)) = REG_ONLY.iter().find(|x| x.1 == base) {
                        format!("{m} r{w}")
                    } else if let Some(&(m, _)) = IMM_ONLY.iter().find(|x| x.1 == base) {
                        len = 2;
                        format!("{m} {}", hex(u32::from(word(1)?), 4))
                    } else if let Some(&(m, _)) = NO_ARG.iter().find(|x| x.1 == op) {
                        m.to_owned()
                    } else {
                        return None;
                    }
                }
                _ => return None,
            },
        };
        Some((len * 2, text))
    }
}
