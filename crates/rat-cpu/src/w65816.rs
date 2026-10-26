//! WDC 65C816 (SNES 5A22). Tables ported from
//! `mimic/crates/cpu-65816/src/disasm.rs` (`NAMES`, `mode`).
//!
//! Immediate widths follow the assumed M/X flags (`.a8/.a16/.i8/.i16`,
//! ACME `!as/!al/!rs/!rl`).

use std::collections::HashMap;

use rat_core::Endian;

use crate::mos6502::{parse_operand, size_suffix, Operand};
use crate::util::*;
use crate::{Cpu, CpuInfo, CpuState, Num, OperandCtx};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Am {
    Imp,
    Acc,
    ImmM,
    ImmX,
    Imm8,
    Dp,
    DpX,
    DpY,
    Abs,
    AbsX,
    AbsY,
    Long,
    LongX,
    DpInd,
    DpIndX,
    DpIndY,
    DpIndLong,
    DpIndLongY,
    Sr,
    SrIndY,
    Rel,
    RelLong,
    AbsInd,
    AbsIndX,
    AbsIndLong,
    Move,
}

#[rustfmt::skip]
const NAMES: [&str; 256] = [
    "brk","ora","cop","ora","tsb","ora","asl","ora","php","ora","asl","phd","tsb","ora","asl","ora",
    "bpl","ora","ora","ora","trb","ora","asl","ora","clc","ora","inc","tcs","trb","ora","asl","ora",
    "jsr","and","jsl","and","bit","and","rol","and","plp","and","rol","pld","bit","and","rol","and",
    "bmi","and","and","and","bit","and","rol","and","sec","and","dec","tsc","bit","and","rol","and",
    "rti","eor","wdm","eor","mvp","eor","lsr","eor","pha","eor","lsr","phk","jmp","eor","lsr","eor",
    "bvc","eor","eor","eor","mvn","eor","lsr","eor","cli","eor","phy","tcd","jml","eor","lsr","eor",
    "rts","adc","per","adc","stz","adc","ror","adc","pla","adc","ror","rtl","jmp","adc","ror","adc",
    "bvs","adc","adc","adc","stz","adc","ror","adc","sei","adc","ply","tdc","jmp","adc","ror","adc",
    "bra","sta","brl","sta","sty","sta","stx","sta","dey","bit","txa","phb","sty","sta","stx","sta",
    "bcc","sta","sta","sta","sty","sta","stx","sta","tya","sta","txs","txy","stz","sta","stz","sta",
    "ldy","lda","ldx","lda","ldy","lda","ldx","lda","tay","lda","tax","plb","ldy","lda","ldx","lda",
    "bcs","lda","lda","lda","ldy","lda","ldx","lda","clv","lda","tsx","tyx","ldy","lda","ldx","lda",
    "cpy","cmp","rep","cmp","cpy","cmp","dec","cmp","iny","cmp","dex","wai","cpy","cmp","dec","cmp",
    "bne","cmp","cmp","cmp","pei","cmp","dec","cmp","cld","cmp","phx","stp","jml","cmp","dec","cmp",
    "cpx","sbc","sep","sbc","cpx","sbc","inc","sbc","inx","sbc","nop","xba","cpx","sbc","inc","sbc",
    "beq","sbc","sbc","sbc","pea","sbc","inc","sbc","sed","sbc","plx","xce","jsr","sbc","inc","sbc",
];

pub fn mode(op: u8) -> Am {
    match op {
        0x00 | 0x02 | 0x42 | 0xc2 | 0xe2 => Am::Imm8,
        0x20 | 0x4c => Am::Abs,
        0x22 | 0x5c => Am::Long,
        0x6c => Am::AbsInd,
        0x7c | 0xfc => Am::AbsIndX,
        0xdc => Am::AbsIndLong,
        0x82 | 0x62 => Am::RelLong,
        0x44 | 0x54 => Am::Move,
        0xf4 => Am::Abs,
        0xd4 => Am::DpInd,
        // the exceptions of the low bits: trb dp, trb abs, stz abs
        0x14 => Am::Dp,
        0x1c | 0x9c => Am::Abs,
        0x0a | 0x1a | 0x2a | 0x3a | 0x4a | 0x6a => Am::Acc,
        0xa0 | 0xa2 | 0xc0 | 0xe0 => Am::ImmX,
        0x89 => Am::ImmM,
        0x96 | 0xb6 => Am::DpY,
        0xbe => Am::AbsY,
        _ if op & 0x1f == 0x10 || op == 0x80 => Am::Rel,
        _ => match op & 0x1f {
            0x01 => Am::DpIndX,
            0x03 => Am::Sr,
            0x04..=0x06 => Am::Dp,
            0x07 => Am::DpIndLong,
            0x09 => Am::ImmM,
            0x0c..=0x0e => Am::Abs,
            0x0f => Am::Long,
            0x11 => Am::DpIndY,
            0x12 => Am::DpInd,
            0x13 => Am::SrIndY,
            0x14..=0x16 => Am::DpX,
            0x17 => Am::DpIndLongY,
            0x19 => Am::AbsY,
            0x1c..=0x1e => Am::AbsX,
            0x1f => Am::LongX,
            _ => Am::Imp,
        },
    }
}

const ALIASES: &[(&str, &str)] = &[
    ("tad", "tcd"),
    ("tas", "tcs"),
    ("tda", "tdc"),
    ("tsa", "tsc"),
    ("swa", "xba"),
    ("cpa", "cmp"),
    ("bge", "bcs"),
    ("blt", "bcc"),
    ("jsl", "jsl"),
];

pub struct W65816 {
    ops: HashMap<String, HashMap<Am, u8>>,
}

impl W65816 {
    pub fn new() -> W65816 {
        let mut ops: HashMap<String, HashMap<Am, u8>> = HashMap::new();
        for op in 0..=255u8 {
            ops.entry(NAMES[op as usize].to_owned())
                .or_default()
                .entry(mode(op))
                .or_insert(op);
        }
        // long forms reachable through the short names
        ops.get_mut("jmp").unwrap().insert(Am::Long, 0x5c);
        ops.get_mut("jmp").unwrap().insert(Am::AbsIndLong, 0xdc);
        ops.get_mut("jsr").unwrap().insert(Am::Long, 0x22);
        ops.get_mut("jml").unwrap().insert(Am::AbsInd, 0xdc);
        ops.entry("dea".into()).or_default().insert(Am::Acc, 0x3a);
        ops.entry("ina".into()).or_default().insert(Am::Acc, 0x1a);
        W65816 { ops }
    }

    fn lookup(&self, m: &str) -> Option<&HashMap<Am, u8>> {
        let m = m.to_ascii_lowercase();
        self.ops.get(&m).or_else(|| {
            let c = ALIASES.iter().find(|(a, _)| *a == m)?.1;
            self.ops.get(c)
        })
    }
}

impl Default for W65816 {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu for W65816 {
    fn info(&self) -> CpuInfo {
        CpuInfo {
            id: "65816",
            aliases: &["65c816", "w65c816", "5a22"],
            description: "WDC 65C816 (SNES 5A22), 24-bit addressing, M/X widths via .a8/.a16/.i8/.i16",
            endian: "little",
            addr_bits: 24,
        }
    }

    fn endian(&self) -> Endian {
        Endian::Little
    }

    fn has_mnemonic(&self, m: &str) -> bool {
        self.lookup(m).is_some()
    }

    fn mnemonics(&self) -> Vec<String> {
        let mut v: Vec<String> = self.ops.keys().cloned().collect();
        v.extend(ALIASES.iter().map(|(a, _)| a.to_string()));
        v.sort();
        v.dedup();
        v
    }

    fn assemble(
        &self,
        mnemonic: &str,
        suffix: Option<&str>,
        ctx: &mut dyn OperandCtx,
        st: &CpuState,
    ) -> Result<Vec<u8>, String> {
        let m = mnemonic.to_ascii_lowercase();
        let modes = self
            .lookup(&m)
            .ok_or_else(|| format!("unknown instruction `{mnemonic}`"))?;
        let force = size_suffix(suffix)?;
        let has = |a: Am| modes.contains_key(&a);
        let toks = ctx.toks().to_vec();
        let ind_ok = has(Am::DpInd) || has(Am::AbsInd);
        let operand = parse_operand(&toks, ind_ok)?;
        let bad = || format!("`{m}` does not support this addressing mode");
        let pc = i64::from(ctx.pc());
        let mut out = Vec::new();
        // pick among dp(1)/abs(2)/long(3) forms
        let sized = |ctx: &mut dyn OperandCtx,
                     out: &mut Vec<u8>,
                     r: &std::ops::Range<usize>,
                     cands: [Option<Am>; 3]|
         -> Result<(), String> {
            let n = eval(ctx, r)?;
            let avail: Vec<u8> = (1..=3).filter(|&s| cands[s as usize - 1].is_some()).collect();
            if avail.is_empty() {
                return Err(bad());
            }
            let mut v = n.value;
            if let (Some(dp), true) = (st.dp, n.known) {
                let d = i64::from(dp);
                if cands[0].is_some() && (d..=d + 0xff).contains(&v) && force.is_none() && ctx.min_size() <= 1 {
                    v -= d;
                }
            }
            let need = if let Some(f) = force {
                f
            } else if !n.known {
                if n.zp_hint { 1 } else { 2 }
            } else if (0..=0xff).contains(&v) {
                1
            } else if (0..=0xffff).contains(&v) || v < 0 {
                2
            } else {
                3
            };
            let need = need.max(ctx.min_size());
            let size = avail.iter().copied().find(|&s| s >= need).unwrap_or(*avail.last().unwrap());
            ctx.grow(size);
            out.push(modes[&cands[size as usize - 1].unwrap()]);
            match size {
                1 => out.push(fit(ctx, Num::with(n, v), 0, 0xff, 8, "direct-page address") as u8),
                2 => out.extend(le16(n.value)),
                _ => out.extend(&(n.value as u32).to_le_bytes()[..3]),
            }
            Ok(())
        };
        match &operand {
            Operand::None => {
                if let Some(&op) = modes.get(&Am::Imp).or(modes.get(&Am::Acc)) {
                    out.push(op);
                } else if let Some(&op) = modes.get(&Am::Imm8) {
                    out.push(op); // BRK/COP/WDM without signature byte
                } else {
                    return Err(format!("`{m}` needs an operand"));
                }
            }
            Operand::Acc => out.push(*modes.get(&Am::Acc).ok_or_else(bad)?),
            Operand::Imm(r) => {
                let n = eval(ctx, r)?;
                if let Some(&op) = modes.get(&Am::Imm8) {
                    out.push(op);
                    out.push(fit(ctx, n, -128, 255, 8, "immediate value") as u8);
                } else if has(Am::Abs) && m == "pea" {
                    out.push(modes[&Am::Abs]);
                    out.extend(le16(fit(ctx, n, -32768, 0xffff, 16, "value")));
                } else {
                    let (op, wide) = if let Some(&op) = modes.get(&Am::ImmM) {
                        (op, st.m16)
                    } else if let Some(&op) = modes.get(&Am::ImmX) {
                        (op, st.x16)
                    } else {
                        return Err(format!("`{m}` has no immediate mode"));
                    };
                    let wide = match force {
                        Some(1) => false,
                        Some(_) => true,
                        None => wide,
                    };
                    out.push(op);
                    if wide {
                        out.extend(le16(fit(ctx, n, -32768, 0xffff, 16, "immediate value")));
                    } else {
                        out.push(fit(ctx, n, -128, 255, 8, "immediate value") as u8);
                    }
                }
            }
            Operand::Direct(r) if has(Am::Rel) => {
                let n = eval(ctx, r)?;
                out.push(modes[&Am::Rel]);
                out.push(rel(ctx, n, pc + 2, 8) as u8);
            }
            Operand::Direct(r) if has(Am::RelLong) => {
                let n = eval(ctx, r)?;
                out.push(modes[&Am::RelLong]);
                out.extend(le16(rel(ctx, n, pc + 3, 16)));
            }
            Operand::Direct(r) => {
                let cands = [modes.get(&Am::Dp).map(|_| Am::Dp), modes.get(&Am::Abs).map(|_| Am::Abs), modes.get(&Am::Long).map(|_| Am::Long)];
                sized(ctx, &mut out, r, cands)?;
            }
            Operand::DirX(r) => {
                let cands = [has(Am::DpX).then_some(Am::DpX), has(Am::AbsX).then_some(Am::AbsX), has(Am::LongX).then_some(Am::LongX)];
                sized(ctx, &mut out, r, cands)?;
            }
            Operand::DirY(r) => {
                let cands = [has(Am::DpY).then_some(Am::DpY), has(Am::AbsY).then_some(Am::AbsY), None];
                sized(ctx, &mut out, r, cands)?;
            }
            Operand::DirS(r) => {
                let n = eval(ctx, r)?;
                out.push(*modes.get(&Am::Sr).ok_or_else(bad)?);
                out.push(fit(ctx, n, 0, 255, 8, "stack offset") as u8);
            }
            Operand::Ind(r) => {
                let n = eval(ctx, r)?;
                if let Some(&op) = modes.get(&Am::AbsInd) {
                    out.push(op);
                    out.extend(le16(n.value));
                } else {
                    out.push(*modes.get(&Am::DpInd).ok_or_else(bad)?);
                    out.push(fit(ctx, n, 0, 255, 8, "direct-page address") as u8);
                }
            }
            Operand::IndX(r) => {
                let n = eval(ctx, r)?;
                if let Some(&op) = modes.get(&Am::AbsIndX) {
                    out.push(op);
                    out.extend(le16(n.value));
                } else {
                    out.push(*modes.get(&Am::DpIndX).ok_or_else(bad)?);
                    out.push(fit(ctx, n, 0, 255, 8, "direct-page address") as u8);
                }
            }
            Operand::IndY(r) => {
                let n = eval(ctx, r)?;
                out.push(*modes.get(&Am::DpIndY).ok_or_else(bad)?);
                out.push(fit(ctx, n, 0, 255, 8, "direct-page address") as u8);
            }
            Operand::IndSY(r) => {
                let n = eval(ctx, r)?;
                out.push(*modes.get(&Am::SrIndY).ok_or_else(bad)?);
                out.push(fit(ctx, n, 0, 255, 8, "stack offset") as u8);
            }
            Operand::IndLong(r) => {
                let n = eval(ctx, r)?;
                if let Some(&op) = modes.get(&Am::AbsIndLong) {
                    out.push(op);
                    out.extend(le16(n.value));
                } else {
                    out.push(*modes.get(&Am::DpIndLong).ok_or_else(bad)?);
                    out.push(fit(ctx, n, 0, 255, 8, "direct-page address") as u8);
                }
            }
            Operand::IndLongY(r) => {
                let n = eval(ctx, r)?;
                out.push(*modes.get(&Am::DpIndLongY).ok_or_else(bad)?);
                out.push(fit(ctx, n, 0, 255, 8, "direct-page address") as u8);
            }
            Operand::Pair(a, b) => {
                let op = *modes.get(&Am::Move).ok_or_else(bad)?;
                let strip = |r: &std::ops::Range<usize>| {
                    if toks[r.start].is_punct("#") {
                        r.start + 1..r.end
                    } else {
                        r.clone()
                    }
                };
                let src = eval(ctx, &strip(a))?;
                let dst = eval(ctx, &strip(b))?;
                let bank = |v: i64| if v > 0xff { v >> 16 } else { v } as u8;
                out.extend([op, bank(dst.value), bank(src.value)]);
            }
        }
        Ok(out)
    }

    fn disassemble(&self, bytes: &[u8], addr: u32, st: &CpuState) -> Option<(usize, String)> {
        let op = *bytes.first()?;
        let am = mode(op);
        let len = match am {
            Am::Imp | Am::Acc => 1,
            Am::ImmM => 3 - usize::from(!st.m16),
            Am::ImmX => 3 - usize::from(!st.x16),
            Am::Abs | Am::AbsX | Am::AbsY | Am::AbsInd | Am::AbsIndX | Am::AbsIndLong => 3,
            Am::RelLong | Am::Move => 3,
            Am::Long | Am::LongX => 4,
            _ => 2,
        };
        if bytes.len() < len {
            return None;
        }
        let b1 = u32::from(bytes.get(1).copied().unwrap_or(0));
        let w = b1 | u32::from(bytes.get(2).copied().unwrap_or(0)) << 8;
        let l = w | u32::from(bytes.get(3).copied().unwrap_or(0)) << 16;
        let bank = addr & 0xff_0000;
        let next = (addr & 0xffff) + len as u32;
        let tgt = |t: u32| if bank == 0 { hex(t, 4) } else { hex(bank | t, 6) };
        let operand = match am {
            Am::Imp => String::new(),
            Am::Acc => "a".into(),
            Am::Imm8 => format!("#{}", hex(b1, 2)),
            Am::ImmM | Am::ImmX => format!("#{}", if len == 2 { hex(b1, 2) } else { hex(w, 4) }),
            Am::Dp => hex(b1, 2),
            Am::DpX => format!("{},x", hex(b1, 2)),
            Am::DpY => format!("{},y", hex(b1, 2)),
            Am::Abs if op == 0xf4 => format!("#{}", hex(w, 4)),
            Am::Abs => hex(w, 4),
            Am::AbsX => format!("{},x", hex(w, 4)),
            Am::AbsY => format!("{},y", hex(w, 4)),
            Am::Long => hex(l, 6),
            Am::LongX => format!("{},x", hex(l, 6)),
            Am::DpInd => format!("({})", hex(b1, 2)),
            Am::DpIndX => format!("({},x)", hex(b1, 2)),
            Am::DpIndY => format!("({}),y", hex(b1, 2)),
            Am::DpIndLong => format!("[{}]", hex(b1, 2)),
            Am::DpIndLongY => format!("[{}],y", hex(b1, 2)),
            Am::Sr => format!("{},s", hex(b1, 2)),
            Am::SrIndY => format!("({},s),y", hex(b1, 2)),
            Am::Rel => tgt(next.wrapping_add(b1 as u8 as i8 as u32) & 0xffff),
            Am::RelLong => tgt(next.wrapping_add(w) & 0xffff),
            Am::AbsInd => format!("({})", hex(w, 4)),
            Am::AbsIndX => format!("({},x)", hex(w, 4)),
            Am::AbsIndLong => format!("[{}]", hex(w, 4)),
            Am::Move => format!("{},{}", hex(w >> 8, 2), hex(b1, 2)),
        };
        let m = NAMES[op as usize];
        // force the width when the plain form would pick a shorter one
        let m = match am {
            Am::Abs | Am::AbsX | Am::AbsY if w <= 0xff && op != 0xf4 && op != 0x20 && op != 0x4c => format!("{m}.w"),
            Am::Long | Am::LongX if l <= 0xffff && op != 0x22 && op != 0x5c => format!("{m}.l"),
            Am::Long if op == 0x5c || op == 0x22 => m.to_owned(),
            _ => m.to_owned(),
        };
        Some((len, if operand.is_empty() { m } else { format!("{m} {operand}") }))
    }
}

impl crate::Num {
    /// Same resolution state, different value.
    pub fn with(n: crate::Num, v: i64) -> crate::Num {
        crate::Num { value: v, ..n }
    }
}
