//! Sony SPC700 (SNES sound CPU), Sony syntax.
//!
//! Templates ported from `mimic/crates/cpu-spc700/src/disasm.rs` (`OPS`).
//! `!` forces an absolute address; without it the assembler picks direct
//! page or absolute by value. Two-address forms (`OR dst,src`,
//! `MOV dst,#imm`) store the source byte first.
//! Note: mimic lists `$FD` as `MOV A,Y`; it is `MOV Y,A` here.

use std::collections::HashMap;
use std::ops::Range;

use rat_core::{Endian, Tok, Token};

use crate::util::*;
use crate::{is_reg, Cpu, CpuInfo, CpuState, OperandCtx};

#[rustfmt::skip]
const OPS: [&str; 256] = [
    "NOP","TCALL 0","SET1 d.0","BBS d.0,r","OR A,d","OR A,!a","OR A,(X)","OR A,[d+X]",
    "OR A,#i","OR t,d","OR1 C,m","ASL d","ASL !a","PUSH PSW","TSET1 !a","BRK",
    "BPL r","TCALL 1","CLR1 d.0","BBC d.0,r","OR A,d+X","OR A,!a+X","OR A,!a+Y","OR A,[d]+Y",
    "OR t,#i","OR (X),(Y)","DECW d","ASL d+X","ASL A","DEC X","CMP X,!a","JMP [!a+X]",
    "CLRP","TCALL 2","SET1 d.1","BBS d.1,r","AND A,d","AND A,!a","AND A,(X)","AND A,[d+X]",
    "AND A,#i","AND t,d","OR1 C,/m","ROL d","ROL !a","PUSH A","CBNE d,r","BRA r",
    "BMI r","TCALL 3","CLR1 d.1","BBC d.1,r","AND A,d+X","AND A,!a+X","AND A,!a+Y","AND A,[d]+Y",
    "AND t,#i","AND (X),(Y)","INCW d","ROL d+X","ROL A","INC X","CMP X,d","CALL !a",
    "SETP","TCALL 4","SET1 d.2","BBS d.2,r","EOR A,d","EOR A,!a","EOR A,(X)","EOR A,[d+X]",
    "EOR A,#i","EOR t,d","AND1 C,m","LSR d","LSR !a","PUSH X","TCLR1 !a","PCALL i",
    "BVC r","TCALL 5","CLR1 d.2","BBC d.2,r","EOR A,d+X","EOR A,!a+X","EOR A,!a+Y","EOR A,[d]+Y",
    "EOR t,#i","EOR (X),(Y)","CMPW YA,d","LSR d+X","LSR A","MOV X,A","CMP Y,!a","JMP !a",
    "CLRC","TCALL 6","SET1 d.3","BBS d.3,r","CMP A,d","CMP A,!a","CMP A,(X)","CMP A,[d+X]",
    "CMP A,#i","CMP t,d","AND1 C,/m","ROR d","ROR !a","PUSH Y","DBNZ d,r","RET",
    "BVS r","TCALL 7","CLR1 d.3","BBC d.3,r","CMP A,d+X","CMP A,!a+X","CMP A,!a+Y","CMP A,[d]+Y",
    "CMP t,#i","CMP (X),(Y)","ADDW YA,d","ROR d+X","ROR A","MOV A,X","CMP Y,d","RETI",
    "SETC","TCALL 8","SET1 d.4","BBS d.4,r","ADC A,d","ADC A,!a","ADC A,(X)","ADC A,[d+X]",
    "ADC A,#i","ADC t,d","EOR1 C,m","DEC d","DEC !a","MOV Y,#i","POP PSW","MOV t,#i",
    "BCC r","TCALL 9","CLR1 d.4","BBC d.4,r","ADC A,d+X","ADC A,!a+X","ADC A,!a+Y","ADC A,[d]+Y",
    "ADC t,#i","ADC (X),(Y)","SUBW YA,d","DEC d+X","DEC A","MOV X,SP","DIV YA,X","XCN A",
    "EI","TCALL 10","SET1 d.5","BBS d.5,r","SBC A,d","SBC A,!a","SBC A,(X)","SBC A,[d+X]",
    "SBC A,#i","SBC t,d","MOV1 C,m","INC d","INC !a","CMP Y,#i","POP A","MOV (X)+,A",
    "BCS r","TCALL 11","CLR1 d.5","BBC d.5,r","SBC A,d+X","SBC A,!a+X","SBC A,!a+Y","SBC A,[d]+Y",
    "SBC t,#i","SBC (X),(Y)","MOVW YA,d","INC d+X","INC A","MOV SP,X","DAS A","MOV A,(X)+",
    "DI","TCALL 12","SET1 d.6","BBS d.6,r","MOV d,A","MOV !a,A","MOV (X),A","MOV [d+X],A",
    "CMP X,#i","MOV !a,X","MOV1 m,C","MOV d,Y","MOV !a,Y","MOV X,#i","POP X","MUL YA",
    "BNE r","TCALL 13","CLR1 d.6","BBC d.6,r","MOV d+X,A","MOV !a+X,A","MOV !a+Y,A","MOV [d]+Y,A",
    "MOV d,X","MOV d+Y,X","MOVW d,YA","MOV d+X,Y","DEC Y","MOV A,Y","CBNE d+X,r","DAA A",
    "CLRV","TCALL 14","SET1 d.7","BBS d.7,r","MOV A,d","MOV A,!a","MOV A,(X)","MOV A,[d+X]",
    "MOV A,#i","MOV X,!a","NOT1 m","MOV Y,d","MOV Y,!a","NOTC","POP Y","SLEEP",
    "BEQ r","TCALL 15","CLR1 d.7","BBC d.7,r","MOV A,d+X","MOV A,!a+X","MOV A,!a+Y","MOV A,[d]+Y",
    "MOV X,d","MOV X,d+Y","MOV t,d","MOV Y,d+X","INC Y","MOV Y,A","DBNZ Y,r","STOP",
];

#[derive(Clone, Debug, PartialEq, Eq)]
enum K {
    Lit(String),
    D,
    A,
    I,
    R,
    Dx,
    Dy,
    Ax,
    Ay,
    IndDx,
    IndDy,
    IndAx,
    DBit(u8),
    M,
    NotM,
    PcallI,
    TcallN(u8),
}

fn template(op: u8) -> (String, Vec<K>) {
    let text = OPS[op as usize];
    let (m, rest) = text.split_once(' ').unwrap_or((text, ""));
    let m = m.to_ascii_lowercase();
    if rest.is_empty() {
        return (m, Vec::new());
    }
    let kinds = rest
        .split(',')
        .map(|p| match p {
            "d" | "t" => K::D,
            "!a" => K::A,
            "#i" => K::I,
            "i" => K::PcallI,
            "r" => K::R,
            "d+X" => K::Dx,
            "d+Y" => K::Dy,
            "!a+X" => K::Ax,
            "!a+Y" => K::Ay,
            "[d+X]" => K::IndDx,
            "[d]+Y" => K::IndDy,
            "[!a+X]" => K::IndAx,
            "m" => K::M,
            "/m" => K::NotM,
            _ if p.starts_with("d.") => K::DBit(p[2..].parse().unwrap()),
            _ if m == "tcall" => K::TcallN(p.parse().unwrap()),
            _ => K::Lit(p.to_ascii_lowercase()),
        })
        .collect();
    (m, kinds)
}

/// Two-address forms store the second operand first.
fn reversed(op: u8) -> bool {
    let t = OPS[op as usize];
    t.ends_with(" t,d") || t.ends_with(" t,#i")
}

enum P {
    Reg(String),
    Imm(Range<usize>),
    Plain(Range<usize>),
    Bang(Range<usize>),
    PlusX(Range<usize>, bool),
    PlusY(Range<usize>, bool),
    IndX(Range<usize>, bool),
    IndY(Range<usize>),
    Bit(Range<usize>, u8),
    NotBit(Range<usize>, u8),
}

fn classify(toks: &[Token], r: Range<usize>) -> Result<P, String> {
    let n = r.end - r.start;
    if n == 0 {
        return Err("missing operand".into());
    }
    let t = |i: usize| &toks[r.start + i];
    let last = |k: usize| &toks[r.end - k];
    // registers and (X) (Y) (X)+
    if n == 1 {
        if let Some(s) = t(0).ident() {
            let l = s.to_ascii_lowercase();
            if matches!(l.as_str(), "a" | "x" | "y" | "ya" | "sp" | "psw" | "c") {
                return Ok(P::Reg(l));
            }
        }
    }
    if n >= 3 && t(0).is_punct("(") && t(2).is_punct(")") && (is_reg(t(1), "x") || is_reg(t(1), "y")) {
        let reg = t(1).ident().unwrap().to_ascii_lowercase();
        if n == 3 {
            return Ok(P::Reg(format!("({reg})")));
        }
        if n == 4 && t(3).is_punct("+") {
            return Ok(P::Reg(format!("({reg})+")));
        }
    }
    if t(0).is_punct("#") {
        return Ok(P::Imm(r.start + 1..r.end));
    }
    if t(0).is_punct("/") {
        if let P::Bit(a, b) = classify(toks, r.start + 1..r.end)? {
            return Ok(P::NotBit(a, b));
        }
        return Err("`/` needs a memory bit (`addr.bit`)".into());
    }
    if t(0).is_punct("[") {
        let close = matching(toks, r.start).ok_or("unbalanced `[`")?;
        let inner = r.start + 1..close;
        let bang = toks[inner.start].is_punct("!");
        let inner_e = if bang { inner.start + 1..inner.end } else { inner.clone() };
        if close == r.end - 1 && inner.end - inner.start >= 3 && toks[inner.end - 2].is_punct("+") && is_reg(&toks[inner.end - 1], "x") {
            return Ok(P::IndX(inner_e.start..inner.end - 2, bang));
        }
        if close == r.end - 3 && last(2).is_punct("+") && is_reg(last(1), "y") {
            return Ok(P::IndY(inner_e));
        }
        return Err("bad indirect operand".into());
    }
    let bang = t(0).is_punct("!");
    let s = if bang { r.start + 1 } else { r.start };
    if r.end - s >= 3 && last(2).is_punct("+") && (is_reg(last(1), "x") || is_reg(last(1), "y")) {
        let e = s..r.end - 2;
        return Ok(if is_reg(last(1), "x") { P::PlusX(e, bang) } else { P::PlusY(e, bang) });
    }
    if r.end - s >= 3 && last(2).is_punct(".") {
        if let Tok::Int(b) = last(1).tok {
            if (0..8).contains(&b) {
                return Ok(P::Bit(s..r.end - 2, b as u8));
            }
        }
    }
    Ok(if bang { P::Bang(s..r.end) } else { P::Plain(s..r.end) })
}

/// How well user operand `p` fits template part `k`: None = no fit;
/// Some(true) = fits only as an absolute (wide) form.
fn fits(p: &P, k: &K) -> Option<bool> {
    match (p, k) {
        (P::Reg(s), K::Lit(l)) if s == l => Some(false),
        (P::Imm(_), K::I) => Some(false),
        (P::Plain(_), K::D | K::R | K::PcallI | K::TcallN(_)) => Some(false),
        (P::Plain(_) | P::Bang(_), K::A) => Some(true),
        (P::PlusX(_, false), K::Dx) | (P::PlusY(_, false), K::Dy) => Some(false),
        (P::PlusX(..), K::Ax) | (P::PlusY(..), K::Ay) => Some(true),
        (P::IndX(_, false), K::IndDx) => Some(false),
        (P::IndX(..), K::IndAx) => Some(true),
        (P::IndY(_), K::IndDy) => Some(false),
        (P::Bit(_, b), K::DBit(t)) if b == t => Some(false),
        (P::Bit(..), K::M) | (P::NotBit(..), K::NotM) => Some(true),
        _ => None,
    }
}

fn expr_range(p: &P) -> Option<&Range<usize>> {
    match p {
        P::Reg(_) => None,
        P::Imm(r) | P::Plain(r) | P::Bang(r) | P::PlusX(r, _) | P::PlusY(r, _) | P::IndX(r, _) | P::IndY(r) => Some(r),
        P::Bit(r, _) | P::NotBit(r, _) => Some(r),
    }
}

pub struct Spc700 {
    by_name: HashMap<String, Vec<(u8, Vec<K>)>>,
}

impl Spc700 {
    pub fn new() -> Spc700 {
        let mut by_name: HashMap<String, Vec<(u8, Vec<K>)>> = HashMap::new();
        for op in 0..=255u8 {
            let (m, k) = template(op);
            by_name.entry(m).or_default().push((op, k));
        }
        Spc700 { by_name }
    }
}

impl Default for Spc700 {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu for Spc700 {
    fn info(&self) -> CpuInfo {
        CpuInfo {
            id: "spc700",
            aliases: &["spc"],
            description: "Sony SPC700 (SNES audio), Sony syntax, `!` forces absolute",
            endian: "little",
            addr_bits: 16,
        }
    }

    fn endian(&self) -> Endian {
        Endian::Little
    }

    fn has_mnemonic(&self, m: &str) -> bool {
        self.by_name.contains_key(&m.to_ascii_lowercase())
    }

    fn mnemonics(&self) -> Vec<String> {
        let mut v: Vec<String> = self.by_name.keys().cloned().collect();
        v.sort();
        v
    }

    fn assemble(
        &self,
        mnemonic: &str,
        _suffix: Option<&str>,
        ctx: &mut dyn OperandCtx,
        _st: &CpuState,
    ) -> Result<Vec<u8>, String> {
        let m = mnemonic.to_ascii_lowercase();
        let cands = self
            .by_name
            .get(&m)
            .ok_or_else(|| format!("unknown instruction `{mnemonic}`"))?;
        let toks = ctx.toks().to_vec();
        let parts: Vec<P> = commas(&toks)
            .into_iter()
            .map(|r| classify(&toks, r))
            .collect::<Result<_, _>>()?;
        // candidates and whether they use an absolute form
        let mut ok: Vec<(u8, &Vec<K>, bool)> = Vec::new();
        for (op, kinds) in cands {
            if kinds.len() != parts.len() {
                continue;
            }
            let mut wide = false;
            let mut good = true;
            for (p, k) in parts.iter().zip(kinds) {
                match fits(p, k) {
                    Some(w) => wide |= w,
                    None => {
                        good = false;
                        break;
                    }
                }
            }
            if good {
                ok.push((*op, kinds, wide));
            }
        }
        if ok.is_empty() {
            return Err(format!("`{m}` does not support these operands"));
        }
        // evaluate every expression once
        let mut vals = Vec::new();
        for p in &parts {
            vals.push(match expr_range(p) {
                Some(r) => Some(eval(ctx, r)?),
                None => None,
            });
        }
        let has_short = ok.iter().any(|c| !c.2);
        let has_wide = ok.iter().any(|c| c.2);
        let explicit_bang = parts.iter().any(|p| matches!(p, P::Bang(_) | P::PlusX(_, true) | P::PlusY(_, true) | P::IndX(_, true)));
        let want_wide = if has_short && has_wide {
            explicit_bang
                || ctx.min_size() >= 2
                || vals.iter().flatten().any(|n| if n.known { !(0..=0xff).contains(&n.value) } else { !n.zp_hint })
        } else {
            has_wide
        };
        let &(op, kinds, wide) = ok.iter().find(|c| c.2 == want_wide).unwrap_or(&ok[0]);
        if has_short && has_wide {
            ctx.grow(if wide { 2 } else { 1 });
        }
        let mut fields: Vec<Vec<u8>> = Vec::new();
        let pc = i64::from(ctx.pc());
        let len: i64 = 1 + kinds
            .iter()
            .map(|k| match k {
                K::Lit(_) | K::TcallN(_) => 0,
                K::A | K::Ax | K::Ay | K::IndAx | K::M | K::NotM => 2,
                _ => 1,
            })
            .sum::<i64>();
        let mut opcode = op;
        for ((p, k), n) in parts.iter().zip(kinds).zip(&vals) {
            let n = n.unwrap_or_default();
            let f = match k {
                K::Lit(_) => Vec::new(),
                K::TcallN(_) => {
                    let v = fit(ctx, n, 0, 15, 4, "TCALL number");
                    opcode = (v as u8) << 4 | 0x01;
                    Vec::new()
                }
                K::I => vec![fit(ctx, n, -128, 255, 8, "immediate value") as u8],
                K::PcallI => {
                    let v = if n.value >= 0xff00 { n.value - 0xff00 } else { n.value };
                    vec![fit(ctx, crate::Num::with(n, v), 0, 255, 8, "PCALL address") as u8]
                }
                K::D | K::Dx | K::Dy | K::IndDx | K::IndDy | K::DBit(_) => {
                    vec![fit(ctx, n, 0, 255, 8, "direct-page address") as u8]
                }
                K::A | K::Ax | K::Ay | K::IndAx => le16(fit(ctx, n, -32768, 0xffff, 16, "address")).to_vec(),
                K::M | K::NotM => {
                    let bit = match p {
                        P::Bit(_, b) | P::NotBit(_, b) => i64::from(*b),
                        _ => 0,
                    };
                    let a = fit(ctx, n, 0, 0x1fff, 13, "memory-bit address");
                    le16(a | bit << 13).to_vec()
                }
                K::R => {
                    // displacement is relative to the end of the instruction
                    vec![rel(ctx, n, pc + len, 8) as u8]
                }
            };
            fields.push(f);
        }
        if reversed(op) {
            fields.reverse();
        }
        let mut out = vec![opcode];
        for f in fields {
            out.extend(f);
        }
        Ok(out)
    }

    fn disassemble(&self, bytes: &[u8], addr: u32, _st: &CpuState) -> Option<(usize, String)> {
        let op = *bytes.first()?;
        let text = OPS[op as usize];
        let (m, template) = text.split_once(' ').unwrap_or((text, ""));
        let at = |n: usize| bytes.get(n).copied();
        let mut n = 1usize;
        let operand = if reversed(op) {
            let (b1, b2) = (at(1)?, at(2)?);
            n = 3;
            if template.ends_with("#i") {
                format!("${b2:02x},#${b1:02x}")
            } else {
                format!("${b2:02x},${b1:02x}")
            }
        } else {
            let mut s = String::new();
            for c in template.chars() {
                match c {
                    'd' => {
                        s.push_str(&format!("${:02x}", at(n)?));
                        n += 1;
                    }
                    'a' => {
                        s.push_str(&format!("${:04x}", u16::from_le_bytes([at(n)?, at(n + 1)?])));
                        n += 2;
                    }
                    'i' => {
                        s.push_str(&format!("${:02x}", at(n)?));
                        n += 1;
                    }
                    'r' => {
                        let d = at(n)? as i8;
                        n += 1;
                        let t = (addr as u16).wrapping_add(n as u16).wrapping_add(d as u16);
                        s.push_str(&format!("${t:04x}"));
                    }
                    'm' => {
                        let w = u16::from_le_bytes([at(n)?, at(n + 1)?]);
                        n += 2;
                        s.push_str(&format!("${:04x}.{}", w & 0x1fff, w >> 13));
                    }
                    _ => s.push(c.to_ascii_lowercase()),
                }
            }
            s
        };
        let m = m.to_ascii_lowercase();
        Some((n, if operand.is_empty() { m } else { format!("{m} {operand}") }))
    }
}
