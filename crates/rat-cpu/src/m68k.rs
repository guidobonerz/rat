//! Motorola 68000 and 68020 (Amiga), Motorola syntax, big-endian.
//!
//! Encodings follow the M68000 Family Programmer's Reference Manual, the
//! same source as `mimic/crates/cpu-m68k` (whose disassembler is ported in
//! `m68k_dis.rs`). Both `d16(An)` and `(d16,An)` syntaxes are accepted;
//! `label(pc)` names the target address. Unsized absolute addresses pick
//! `.w` or `.l` by value; unsized branches pick `.s`/`.w` by distance.

use std::ops::Range;

use rat_core::{Endian, Tok, Token};

use crate::m68k_dis;
use crate::util::*;
use crate::{Cpu, CpuInfo, CpuState, Num, OperandCtx};

const CC: [&str; 16] = [
    "t", "f", "hi", "ls", "cc", "cs", "ne", "eq", "vc", "vs", "pl", "mi", "ge", "lt", "gt", "le",
];

fn cond(s: &str) -> Option<u16> {
    match s {
        "hs" => Some(4),
        "lo" => Some(5),
        _ => CC.iter().position(|c| *c == s).map(|i| i as u16),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sz {
    B,
    W,
    L,
}

impl Sz {
    fn bits(self) -> u16 {
        match self {
            Sz::B => 0,
            Sz::W => 1,
            Sz::L => 2,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Idx {
    addr: bool,
    reg: u16,
    long: bool,
    scale: u16,
}

impl Idx {
    fn bits(self) -> u16 {
        (u16::from(self.addr) << 15) | self.reg << 12 | (u16::from(self.long) << 11) | (self.scale << 9)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Base {
    An(u16),
    Pc,
    /// Suppressed base (`ZA0`, `ZPC` or omitted).
    None,
    ZPc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Indirect {
    No,
    Pre,
    Post,
}

#[derive(Clone, Debug)]
enum Ea {
    Dn(u16),
    An(u16),
    Ind(u16),
    PostInc(u16),
    PreDec(u16),
    /// Displacement/index modes with an optional base register.
    Ext { base: Base, bd: Option<Num>, idx: Option<Idx>, ind: Indirect, od: Option<Num>, bd_force: Option<Sz> },
    Abs(Num, Option<Sz>),
    Imm(Num),
    Special(&'static str),
}

fn reg_of(s: &str) -> Option<Ea> {
    let l = s.to_ascii_lowercase();
    if l == "sp" {
        return Some(Ea::An(7));
    }
    if l.len() == 2 {
        let (k, n) = l.split_at(1);
        if let Ok(n) = n.parse::<u16>() {
            if n < 8 {
                match k {
                    "d" => return Some(Ea::Dn(n)),
                    "a" => return Some(Ea::An(n)),
                    _ => {}
                }
            }
        }
    }
    Some(Ea::Special(match l.as_str() {
        "sr" => "sr",
        "ccr" => "ccr",
        "usp" => "usp",
        "sfc" => "sfc",
        "dfc" => "dfc",
        "cacr" => "cacr",
        "vbr" => "vbr",
        "caar" => "caar",
        "msp" => "msp",
        "isp" => "isp",
        _ => return None,
    }))
}

fn an_of(t: &Token) -> Option<u16> {
    match t.ident().and_then(reg_of) {
        Some(Ea::An(n)) => Some(n),
        _ => None,
    }
}

fn dn_of(t: &Token) -> Option<u16> {
    match t.ident().and_then(reg_of) {
        Some(Ea::Dn(n)) => Some(n),
        _ => None,
    }
}

/// `.w` / `.l` suffix at the end of a range (`$1234.w`, `d1.l`).
fn size_tail(toks: &[Token], r: &Range<usize>) -> Option<Sz> {
    if r.end >= r.start + 3 && toks[r.end - 2].is_punct(".") && !toks[r.end - 2].space_before {
        match toks[r.end - 1].ident()?.to_ascii_lowercase().as_str() {
            "w" => return Some(Sz::W),
            "l" => return Some(Sz::L),
            "b" => return Some(Sz::B),
            _ => {}
        }
    }
    None
}

/// Index register spec: `d1`, `a2.l`, `d3.w*4`.
fn parse_idx(toks: &[Token], r: &Range<usize>, ctx: &mut dyn OperandCtx) -> Result<Option<Idx>, String> {
    let t = &toks[r.clone()];
    let Some(first) = t.first() else { return Ok(None) };
    let (addr, reg) = match first.ident().and_then(reg_of) {
        Some(Ea::Dn(n)) => (false, n),
        Some(Ea::An(n)) => (true, n),
        _ => return Ok(None),
    };
    let mut i = 1;
    let mut long = false;
    if t.len() > i + 1 && t[i].is_punct(".") {
        match t[i + 1].ident().map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("w") => {}
            Some("l") => long = true,
            _ => return Ok(None),
        }
        i += 2;
    }
    let mut scale = 0;
    if t.len() > i && t[i].is_punct("*") {
        let n = ctx.eval(r.start + i + 1, r.end)?;
        scale = match n.value {
            1 => 0,
            2 => 1,
            4 => 2,
            8 => 3,
            _ => return Err("index scale must be 1, 2, 4 or 8".into()),
        };
        i = t.len();
    }
    if i != t.len() {
        return Ok(None);
    }
    Ok(Some(Idx { addr, reg, long, scale }))
}

/// Inside of `( ... )`: registers, index and displacement elements.
fn parse_inner(
    toks: &[Token],
    r: Range<usize>,
    ctx: &mut dyn OperandCtx,
    base: &mut Base,
    bd: &mut Option<Num>,
    idx: &mut Option<Idx>,
    bd_force: &mut Option<Sz>,
) -> Result<bool, String> {
    let mut any_reg = false;
    for part in split_top(toks, r, ",") {
        if part.is_empty() {
            continue;
        }
        if let Some(s) = single_ident(toks, &part) {
            let l = s.to_ascii_lowercase();
            match l.as_str() {
                "pc" => {
                    *base = Base::Pc;
                    any_reg = true;
                    continue;
                }
                "zpc" => {
                    *base = Base::ZPc;
                    any_reg = true;
                    continue;
                }
                _ if l.starts_with('z') && matches!(reg_of(&l[1..]), Some(Ea::An(_))) => {
                    *base = Base::None;
                    any_reg = true;
                    continue;
                }
                _ => {}
            }
            if let Some(Ea::An(n)) = reg_of(&l) {
                if *base == Base::None && idx.is_none() && !any_reg {
                    *base = Base::An(n);
                    any_reg = true;
                    continue;
                }
            }
        }
        if let Some(i) = parse_idx(toks, &part, ctx)? {
            if idx.is_some() {
                return Err("two index registers".into());
            }
            *idx = Some(i);
            any_reg = true;
            continue;
        }
        if bd.is_some() {
            return Err("two displacements".into());
        }
        let mut p = part.clone();
        if let Some(sz) = size_tail(toks, &p) {
            *bd_force = Some(sz);
            p = p.start..p.end - 2;
        }
        *bd = Some(eval(ctx, &p)?);
    }
    Ok(any_reg)
}

fn parse_ea(toks: &[Token], r: Range<usize>, ctx: &mut dyn OperandCtx) -> Result<Ea, String> {
    if r.is_empty() {
        return Err("missing operand".into());
    }
    let t = &toks[r.clone()];
    if let Some(s) = single_ident(toks, &r) {
        if let Some(e) = reg_of(s) {
            return Ok(e);
        }
    }
    if t[0].is_punct("#") {
        return Ok(Ea::Imm(eval(ctx, &(r.start + 1..r.end))?));
    }
    if t.len() == 4 && t[0].is_punct("-") && t[1].is_punct("(") && t[3].is_punct(")") {
        if let Some(n) = an_of(&t[2]) {
            return Ok(Ea::PreDec(n));
        }
    }
    if t.len() == 4 && t[0].is_punct("(") && t[2].is_punct(")") && t[3].is_punct("+") {
        if let Some(n) = an_of(&t[1]) {
            return Ok(Ea::PostInc(n));
        }
    }
    if t.len() == 3 && t[0].is_punct("(") && t[2].is_punct(")") {
        if let Some(n) = an_of(&t[1]) {
            return Ok(Ea::Ind(n));
        }
    }
    // trailing paren group: prefix(…) or (…)
    let mut end = r.end;
    let mut abs_force = None;
    if let Some(sz) = size_tail(toks, &r) {
        abs_force = Some(sz);
        end -= 2;
    }
    if toks[end - 1].is_punct(")") {
        // find the opening paren of the last group
        let mut open = None;
        for i in (r.start..end).rev() {
            if toks[i].is_punct("(") && matching(toks, i) == Some(end - 1) {
                open = Some(i);
                break;
            }
        }
        if let Some(open) = open {
            let inner = open + 1..end - 1;
            let mut base = Base::None;
            let mut bd = None;
            let mut idx = None;
            let mut bd_force = None;
            let mut ind = Indirect::No;
            let mut od = None;
            // memory indirect: ([...], ...)
            let parts = split_top(toks, inner.clone(), ",");
            let first_bracket = parts.first().is_some_and(|p| wrapped(toks, p, "[", "]"));
            let has_reg = if first_bracket {
                let br = parts[0].clone();
                let any = parse_inner(toks, br.start + 1..br.end - 1, ctx, &mut base, &mut bd, &mut idx, &mut bd_force)?;
                ind = if idx.is_some() { Indirect::Pre } else { Indirect::Post };
                for p in &parts[1..] {
                    if let Some(i) = parse_idx(toks, p, ctx)? {
                        if idx.is_some() {
                            return Err("two index registers".into());
                        }
                        idx = Some(i);
                        ind = Indirect::Post;
                    } else {
                        od = Some(eval(ctx, p)?);
                    }
                }
                any || true
            } else {
                parse_inner(toks, inner, ctx, &mut base, &mut bd, &mut idx, &mut bd_force)?
            };
            if has_reg {
                if open > r.start {
                    if bd.is_some() {
                        return Err("two displacements".into());
                    }
                    let mut p = r.start..open;
                    if let Some(sz) = size_tail(toks, &p) {
                        bd_force = Some(sz);
                        p = p.start..p.end - 2;
                    }
                    bd = Some(eval(ctx, &p)?);
                }
                if base == Base::None && idx.is_none() && bd.is_none() && ind == Indirect::No {
                    return Err("empty address".into());
                }
                return Ok(Ea::Ext { base, bd, idx, ind, od, bd_force });
            }
        }
    }
    let n = eval(ctx, &(r.start..end))?;
    Ok(Ea::Abs(n, abs_force))
}

/// Encoded effective address: mode/reg field and extension words.
struct Enc {
    field: u16,
    ext: Vec<u16>,
}

fn fits8(v: i64) -> bool {
    (-128..=127).contains(&v)
}

fn fits16(v: i64) -> bool {
    (-32768..=32767).contains(&v)
}

fn push32(v: &mut Vec<u16>, x: i64) {
    v.push((x >> 16) as u16);
    v.push(x as u16);
}

struct Asm<'a> {
    ctx: &'a mut dyn OperandCtx,
    m020: bool,
    /// Size class growth for this instruction (one sticky value per site).
    size_hint: u8,
}

impl Asm<'_> {
    /// Encode `ea`. `ext_pc` is the address of its first extension word
    /// (for PC-relative displacements).
    fn ea(&mut self, ea: &Ea, sz: Sz, ext_pc: i64) -> Result<Enc, String> {
        Ok(match ea {
            Ea::Dn(n) => Enc { field: *n, ext: vec![] },
            Ea::An(n) => Enc { field: 8 | n, ext: vec![] },
            Ea::Ind(n) => Enc { field: 0x10 | n, ext: vec![] },
            Ea::PostInc(n) => Enc { field: 0x18 | n, ext: vec![] },
            Ea::PreDec(n) => Enc { field: 0x20 | n, ext: vec![] },
            Ea::Imm(n) => {
                let mut ext = Vec::new();
                match sz {
                    Sz::B => ext.push(fit(self.ctx, *n, -128, 255, 8, "byte immediate") as u16),
                    Sz::W => ext.push(fit(self.ctx, *n, -32768, 0xffff, 16, "word immediate") as u16),
                    Sz::L => push32(&mut ext, n.value),
                }
                Enc { field: 0x3c, ext }
            }
            Ea::Abs(n, force) => {
                let short = match force {
                    Some(Sz::W) => true,
                    Some(_) => false,
                    None => {
                        let fitw = n.known && (fits16(n.value) || (0xffff_8000..=0xffff_ffff).contains(&n.value));
                        let s = fitw && self.ctx.min_size() < 3;
                        self.size_hint = self.size_hint.max(if s { 1 } else { 3 });
                        s
                    }
                };
                if short {
                    Enc { field: 0x38, ext: vec![n.value as u16] }
                } else {
                    let mut ext = Vec::new();
                    push32(&mut ext, n.value);
                    Enc { field: 0x39, ext }
                }
            }
            Ea::Special(s) => return Err(format!("`{s}` is not allowed here")),
            Ea::Ext { base, bd, idx, ind, od, bd_force } => {
                let pcrel = matches!(base, Base::Pc);
                let breg = match base {
                    Base::An(n) => *n,
                    _ => 0,
                };
                let mut bdv = bd.unwrap_or(Num::known(0));
                if pcrel && bd.is_some() && bdv.known {
                    bdv.value -= ext_pc;
                }
                let simple = *ind == Indirect::No && od.is_none() && !matches!(base, Base::None | Base::ZPc);
                // (d16,An) / (d16,PC)
                if simple && idx.is_none() && bd_force != &Some(Sz::L) && (fits16(bdv.value) || !bdv.known || !self.m020) {
                    if bdv.known && !fits16(bdv.value) {
                        self.ctx.error(format!("displacement out of range: {}", bdv.value));
                    }
                    if !pcrel && bd.is_none() {
                        return Ok(Enc { field: 0x10 | breg, ext: vec![] });
                    }
                    return Ok(Enc { field: if pcrel { 0x3a } else { 0x28 | breg }, ext: vec![bdv.value as u16] });
                }
                let field = if pcrel || *base == Base::ZPc { 0x3b } else { 0x30 | breg };
                // brief format (d8,An,Xn)
                let i = idx.unwrap_or(Idx { addr: false, reg: 0, long: false, scale: 0 });
                if simple && idx.is_some() && bd_force.is_none() && (fits8(bdv.value) || !self.m020) {
                    if i.scale != 0 && !self.m020 {
                        return Err("index scale needs the 68020".into());
                    }
                    if bdv.known && !fits8(bdv.value) {
                        self.ctx.error(format!("displacement out of range: {}", bdv.value));
                    }
                    return Ok(Enc { field, ext: vec![i.bits() | (bdv.value as u8 as u16)] });
                }
                if !self.m020 {
                    return Err("this addressing mode needs the 68020".into());
                }
                // full format
                let mut w = i.bits() | 0x100;
                if idx.is_none() {
                    w |= 0x40;
                }
                if matches!(base, Base::None | Base::ZPc) {
                    w |= 0x80;
                }
                let mut ext = vec![0];
                let bd_size = match (bd, bd_force) {
                    (None, _) => 1,
                    (Some(_), Some(Sz::W)) => 2,
                    (Some(_), Some(Sz::L)) => 3,
                    (Some(_), _) if bdv.known && fits16(bdv.value) => 2,
                    _ => 3,
                };
                // PC-relative bd is relative to the first extension word
                w |= bd_size << 4;
                match bd_size {
                    2 => ext.push(bdv.value as u16),
                    3 => push32(&mut ext, bdv.value),
                    _ => {}
                }
                let iis = match ind {
                    Indirect::No => 0,
                    _ => {
                        let ods = match od {
                            None => 1,
                            Some(o) if o.known && fits16(o.value) => 2,
                            Some(_) => 3,
                        };
                        if let Some(o) = od {
                            if ods == 2 {
                                ext.push(o.value as u16);
                            } else {
                                push32(&mut ext, o.value);
                            }
                        }
                        let post = *ind == Indirect::Post && idx.is_some();
                        ods | if post { 4 } else { 0 }
                    }
                };
                w |= iis;
                ext[0] = w;
                Enc { field, ext }
            }
        })
    }
}

/// Register list `d0-d7/a0-a6` as a MOVEM mask (bit 0 = D0).
fn reglist(toks: &[Token], r: &Range<usize>) -> Option<u16> {
    let mut mask = 0u16;
    for part in split_top(toks, r.clone(), "/") {
        let t = &toks[part];
        let num = |t: &Token| -> Option<u16> {
            match t.ident().and_then(reg_of)? {
                Ea::Dn(n) => Some(n),
                Ea::An(n) => Some(n + 8),
                _ => None,
            }
        };
        match t {
            [a] => mask |= 1 << num(a)?,
            [a, m, b] if m.is_punct("-") => {
                let (x, y) = (num(a)?, num(b)?);
                for i in x.min(y)..=x.max(y) {
                    mask |= 1 << i;
                }
            }
            _ => return None,
        }
    }
    Some(mask)
}

pub struct M68k {
    m020: bool,
}

impl M68k {
    pub fn new(m020: bool) -> M68k {
        M68k { m020 }
    }
}

const BASE_MNEMONICS: &[&str] = &[
    "abcd", "add", "adda", "addi", "addq", "addx", "and", "andi", "asl", "asr", "bchg", "bclr", "bset", "btst", "bkpt",
    "chk", "clr", "cmp", "cmpa", "cmpi", "cmpm", "divs", "divu", "eor", "eori", "exg", "ext", "illegal", "jmp", "jsr",
    "lea", "link", "lsl", "lsr", "move", "movea", "movem", "movep", "moveq", "muls", "mulu", "nbcd", "neg", "negx",
    "nop", "not", "or", "ori", "pea", "reset", "rol", "ror", "roxl", "roxr", "rte", "rtr", "rts", "sbcd", "stop",
    "sub", "suba", "subi", "subq", "subx", "swap", "tas", "trap", "trapv", "tst", "unlk", "bra", "bsr", "dbra",
];
const M020_MNEMONICS: &[&str] = &[
    "extb", "divsl", "divul", "movec", "moves", "rtd", "cas", "cas2", "chk2", "cmp2", "pack", "unpk", "bftst",
    "bfextu", "bfchg", "bfexts", "bfclr", "bfffo", "bfset", "bfins",
];

fn split_cc(m: &str, prefix: &str) -> Option<u16> {
    m.strip_prefix(prefix).and_then(cond)
}

impl Cpu for M68k {
    fn info(&self) -> CpuInfo {
        if self.m020 {
            CpuInfo {
                id: "68020",
                aliases: &["68ec020", "mc68020"],
                description: "Motorola 68020/68EC020 (Amiga 1200), big-endian",
                endian: "big",
                addr_bits: 24,
            }
        } else {
            CpuInfo {
                id: "68000",
                aliases: &["68k", "mc68000"],
                description: "Motorola 68000 (Amiga 500/1000/2000), big-endian",
                endian: "big",
                addr_bits: 24,
            }
        }
    }

    fn endian(&self) -> Endian {
        Endian::Big
    }

    fn has_mnemonic(&self, m: &str) -> bool {
        let m = m.to_ascii_lowercase();
        if BASE_MNEMONICS.contains(&m.as_str()) || (self.m020 && M020_MNEMONICS.contains(&m.as_str())) {
            return true;
        }
        if m == "dbf" || m == "dbt" {
            return true;
        }
        split_cc(&m, "b").is_some_and(|c| c >= 2)
            || split_cc(&m, "db").is_some()
            || split_cc(&m, "s").is_some()
            || (self.m020 && split_cc(&m, "trap").is_some())
    }

    fn mnemonics(&self) -> Vec<String> {
        let mut v: Vec<String> = BASE_MNEMONICS.iter().map(|s| s.to_string()).collect();
        if self.m020 {
            v.extend(M020_MNEMONICS.iter().map(|s| s.to_string()));
        }
        for c in CC {
            if c != "t" && c != "f" {
                v.push(format!("b{c}"));
            }
            v.push(format!("db{c}"));
            v.push(format!("s{c}"));
            if self.m020 {
                v.push(format!("trap{c}"));
            }
        }
        v.sort();
        v.dedup();
        v
    }

    fn assemble(
        &self,
        mnemonic: &str,
        suffix: Option<&str>,
        ctx: &mut dyn OperandCtx,
        _st: &CpuState,
    ) -> Result<Vec<u8>, String> {
        let m = mnemonic.to_ascii_lowercase();
        let sfx = suffix.map(|s| s.to_ascii_lowercase());
        let size: Option<Sz> = match sfx.as_deref() {
            None => None,
            Some("b") => Some(Sz::B),
            Some("w") => Some(Sz::W),
            Some("l") => Some(Sz::L),
            Some("s") => Some(Sz::B), // branch short
            Some(s) => return Err(format!("unknown size `.{s}`")),
        };
        let toks = ctx.toks().to_vec();
        let parts = commas(&toks);
        let pc = i64::from(ctx.pc());
        let m020 = self.m020;
        let mut a = Asm { ctx, m020, size_hint: 0 };
        let words = encode(&mut a, &m, size, sfx.as_deref() == Some("s"), &toks, &parts, pc)?;
        if a.size_hint > 0 {
            a.ctx.grow(a.size_hint);
        }
        Ok(words.iter().flat_map(|w| w.to_be_bytes()).collect())
    }

    fn disassemble(&self, bytes: &[u8], addr: u32, _st: &CpuState) -> Option<(usize, String)> {
        m68k_dis::disassemble(bytes, addr, self.m020)
    }
}

fn need(parts: &[Range<usize>], n: usize, m: &str) -> Result<(), String> {
    if parts.len() != n {
        return Err(format!("`{m}` takes {n} operand(s)"));
    }
    Ok(())
}

/// Instruction words with EA extension words appended in order.
fn with(op: u16, exts: &[&Enc]) -> Vec<u16> {
    let mut v = vec![op];
    for e in exts {
        v.extend(&e.ext);
    }
    v
}

#[allow(clippy::too_many_arguments)]
fn encode(
    a: &mut Asm<'_>,
    m: &str,
    size: Option<Sz>,
    short: bool,
    toks: &[Token],
    parts: &[Range<usize>],
    pc: i64,
) -> Result<Vec<u16>, String> {
    let sz = size.unwrap_or(Sz::W);
    let ea = |a: &mut Asm<'_>, i: usize| parse_ea(toks, parts[i].clone(), a.ctx);
    let m020 = a.m020;
    let req020 = |what: &str| -> Result<(), String> {
        if m020 { Ok(()) } else { Err(format!("`{what}` needs the 68020")) }
    };
    // fixed opcodes
    let fixed = match m {
        "illegal" => Some(0x4afc),
        "reset" => Some(0x4e70),
        "nop" => Some(0x4e71),
        "rte" => Some(0x4e73),
        "rts" => Some(0x4e75),
        "trapv" => Some(0x4e76),
        "rtr" => Some(0x4e77),
        _ => None,
    };
    if let Some(op) = fixed {
        need(parts, 0, m)?;
        return Ok(vec![op]);
    }
    // branches
    let bcc = match m {
        "bra" => Some(0u16),
        "bsr" => Some(1),
        _ if m.len() >= 3 => split_cc(m, "b").filter(|&c| c >= 2),
        _ => None,
    };
    if let Some(c) = bcc {
        need(parts, 1, m)?;
        let t = eval(a.ctx, &parts[0])?;
        let disp = t.value - (pc + 2);
        let class = if short {
            1
        } else if size == Some(Sz::W) {
            2
        } else if size == Some(Sz::L) {
            3
        } else {
            let want = if t.known && fits8(disp) && disp != 0 && !(m020 && disp == -1) { 1 } else { 2 };
            let c = want.max(a.ctx.min_size());
            a.size_hint = c;
            c
        };
        let op = 0x6000 | c << 8;
        return Ok(match class {
            1 => {
                if t.known && (!fits8(disp) || disp == 0) {
                    a.ctx.error(format!("short branch out of range ({disp})"));
                }
                vec![op | (disp as u8 as u16)]
            }
            2 => {
                if t.known && !fits16(disp) {
                    a.ctx.error(format!("branch out of range ({disp})"));
                }
                vec![op, if t.known { disp as u16 } else { 0 }]
            }
            _ => {
                req020("bcc.l")?;
                let mut v = vec![op | 0xff];
                push32(&mut v, if t.known { disp } else { 0 });
                v
            }
        });
    }
    let dbcc = match m {
        "dbra" | "dbf" => Some(1),
        _ => split_cc(m, "db"),
    };
    if let Some(c) = dbcc {
        need(parts, 2, m)?;
        let dn = dn_of(&toks[parts[0].start]).filter(|_| parts[0].len() == 1).ok_or("expected a data register")?;
        let t = eval(a.ctx, &parts[1])?;
        let d = rel(a.ctx, t, pc + 2, 16);
        return Ok(vec![0x50c8 | c << 8 | dn, d as u16]);
    }
    match m {
        "move" | "movea" => {
            need(parts, 2, m)?;
            let src = ea(a, 0)?;
            let dst = ea(a, 1)?;
            match (&src, &dst) {
                (Ea::Special("sr"), _) => {
                    let d = a.ea(&dst, Sz::W, pc + 2)?;
                    return Ok(with(0x40c0 | d.field, &[&d]));
                }
                (Ea::Special("ccr"), _) => {
                    req020("move ccr,ea")?;
                    let d = a.ea(&dst, Sz::W, pc + 2)?;
                    return Ok(with(0x42c0 | d.field, &[&d]));
                }
                (_, Ea::Special("ccr")) => {
                    let s = a.ea(&src, Sz::W, pc + 2)?;
                    return Ok(with(0x44c0 | s.field, &[&s]));
                }
                (_, Ea::Special("sr")) => {
                    let s = a.ea(&src, Sz::W, pc + 2)?;
                    return Ok(with(0x46c0 | s.field, &[&s]));
                }
                (Ea::An(n), Ea::Special("usp")) => return Ok(vec![0x4e60 | n]),
                (Ea::Special("usp"), Ea::An(n)) => return Ok(vec![0x4e68 | n]),
                _ => {}
            }
            let s = a.ea(&src, sz, pc + 2)?;
            let s_len = s.ext.len() as i64 * 2;
            let d = a.ea(&dst, sz, pc + 2 + s_len)?;
            if sz == Sz::B && matches!(dst, Ea::An(_)) {
                return Err("byte move to an address register".into());
            }
            if matches!(dst, Ea::Imm(_)) || d.field == 0x3a || d.field == 0x3b {
                return Err("invalid destination".into());
            }
            let szf = match sz {
                Sz::B => 1,
                Sz::W => 3,
                Sz::L => 2,
            };
            let dfield = (d.field & 7) << 9 | (d.field >> 3) << 6;
            Ok(with(szf << 12 | dfield | s.field, &[&s, &d]))
        }
        "moveq" => {
            need(parts, 2, m)?;
            let src = ea(a, 0)?;
            let Ea::Imm(n) = src else { return Err("moveq needs #imm".into()) };
            let dn = match ea(a, 1)? {
                Ea::Dn(d) => d,
                _ => return Err("moveq needs a data register".into()),
            };
            let v = fit(a.ctx, n, -128, 255, 8, "moveq value");
            Ok(vec![0x7000 | dn << 9 | (v as u8 as u16)])
        }
        "movem" => {
            need(parts, 2, m)?;
            let szb = match sz {
                Sz::L => 0x40,
                Sz::W => 0,
                Sz::B => return Err("movem is .w or .l".into()),
            };
            let list1 = toks[parts[1].clone()].iter().any(|t| t.is_punct("/") || t.is_punct("-")) && reglist(toks, &parts[1]).is_some();
            let regs0 = reglist(toks, &parts[0]).filter(|_| !toks[parts[0].start].is_punct("#") && parse_ea_is_reglist(toks, &parts[0]) && !list1);
            if let Some(mask) = regs0 {
                // registers to memory
                let dst = ea(a, 1)?;
                let rev = matches!(dst, Ea::PreDec(_));
                let mask = if rev { mask.reverse_bits() } else { mask };
                let d = a.ea(&dst, Sz::W, pc + 4)?;
                return Ok(with(0x4880 | szb | d.field, &[&Enc { field: 0, ext: vec![mask] }, &d]));
            }
            let mask = if let Some(mk) = reglist(toks, &parts[1]) {
                mk
            } else if toks[parts[1].start].is_punct("#") {
                eval(a.ctx, &(parts[1].start + 1..parts[1].end))?.value as u16
            } else {
                return Err("movem needs a register list".into());
            };
            let src = ea(a, 0)?;
            let s = a.ea(&src, Sz::W, pc + 4)?;
            Ok(with(0x4c80 | szb | s.field, &[&Enc { field: 0, ext: vec![mask] }, &s]))
        }
        "movep" => {
            need(parts, 2, m)?;
            let long = if sz == Sz::L { 0x40 } else { 0 };
            let (x, y) = (ea(a, 0)?, ea(a, 1)?);
            let disp = |e: &Ea| -> Option<(u16, Num)> {
                if let Ea::Ext { base: Base::An(n), bd, idx: None, ind: Indirect::No, od: None, .. } = e {
                    return Some((*n, bd.unwrap_or(Num::known(0))));
                }
                if let Ea::Ind(n) = e {
                    return Some((*n, Num::known(0)));
                }
                None
            };
            match (&x, &y) {
                (Ea::Dn(d), e) => {
                    let (an, dv) = disp(e).ok_or("movep needs d16(An)")?;
                    Ok(vec![0x0188 | long | d << 9 | an, dv.value as u16])
                }
                (e, Ea::Dn(d)) => {
                    let (an, dv) = disp(e).ok_or("movep needs d16(An)")?;
                    Ok(vec![0x0108 | long | d << 9 | an, dv.value as u16])
                }
                _ => Err("movep needs a data register".into()),
            }
        }
        "movec" => {
            req020(m)?;
            need(parts, 2, m)?;
            let (x, y) = (ea(a, 0)?, ea(a, 1)?);
            let ctrl = |s: &str| -> Option<u16> {
                Some(match s {
                    "sfc" => 0x000,
                    "dfc" => 0x001,
                    "cacr" => 0x002,
                    "usp" => 0x800,
                    "vbr" => 0x801,
                    "caar" => 0x802,
                    "msp" => 0x803,
                    "isp" => 0x804,
                    _ => return None,
                })
            };
            let gen = |e: &Ea| match e {
                Ea::Dn(n) => Some(n << 12),
                Ea::An(n) => Some(0x8000 | n << 12),
                _ => None,
            };
            match (&x, &y) {
                (Ea::Special(c), g) => Ok(vec![0x4e7a, gen(g).ok_or("bad register")? | ctrl(c).ok_or("bad control register")?]),
                (g, Ea::Special(c)) => Ok(vec![0x4e7b, gen(g).ok_or("bad register")? | ctrl(c).ok_or("bad control register")?]),
                _ => Err("movec needs a control register".into()),
            }
        }
        "moves" => {
            req020(m)?;
            need(parts, 2, m)?;
            let (x, y) = (ea(a, 0)?, ea(a, 1)?);
            let gen = |e: &Ea| match e {
                Ea::Dn(n) => Some(n << 12),
                Ea::An(n) => Some(0x8000 | n << 12),
                _ => None,
            };
            if let Some(g) = gen(&x).filter(|_| !matches!(y, Ea::Dn(_) | Ea::An(_))) {
                let d = a.ea(&y, sz, pc + 4)?;
                Ok(with(0x0e00 | sz.bits() << 6 | d.field, &[&Enc { field: 0, ext: vec![g | 0x0800] }, &d]))
            } else {
                let g = gen(&y).ok_or("moves needs a register")?;
                let s = a.ea(&x, sz, pc + 4)?;
                Ok(with(0x0e00 | sz.bits() << 6 | s.field, &[&Enc { field: 0, ext: vec![g] }, &s]))
            }
        }
        "add" | "sub" | "and" | "or" | "cmp" | "eor" => {
            need(parts, 2, m)?;
            let (src, dst) = (ea(a, 0)?, ea(a, 1)?);
            // immediate and address register forms
            if let Ea::Imm(_) = src {
                if !matches!(dst, Ea::An(_)) {
                    let alt = format!("{m}i");
                    return encode(a, &alt, size, short, toks, parts, pc);
                }
            }
            if let Ea::An(_) = dst {
                if matches!(m, "add" | "sub" | "cmp") {
                    let alt = format!("{m}a");
                    return encode(a, &alt, size, short, toks, parts, pc);
                }
            }
            if let (Ea::Special(r), "and" | "or" | "eor") = (&dst, m) {
                let _ = r;
                let alt = format!("{m}i");
                return encode(a, &alt, size, short, toks, parts, pc);
            }
            let base: u16 = match m {
                "or" => 0x8000,
                "sub" => 0x9000,
                "cmp" => 0xb000,
                "eor" => 0xb100,
                "and" => 0xc000,
                _ => 0xd000,
            };
            if m == "eor" {
                let Ea::Dn(d) = src else { return Err("eor needs Dn as source".into()) };
                let e = a.ea(&dst, sz, pc + 2)?;
                return Ok(with(base | d << 9 | sz.bits() << 6 | e.field, &[&e]));
            }
            if let Ea::Dn(d) = dst {
                if sz == Sz::B && matches!(src, Ea::An(_)) {
                    return Err("byte operation on an address register".into());
                }
                let e = a.ea(&src, sz, pc + 2)?;
                return Ok(with(base | d << 9 | sz.bits() << 6 | e.field, &[&e]));
            }
            if m == "cmp" {
                return Err("cmp needs a data register destination".into());
            }
            let Ea::Dn(d) = src else { return Err(format!("`{m}` needs a data register operand")) };
            let e = a.ea(&dst, sz, pc + 2)?;
            Ok(with(base | d << 9 | (4 + sz.bits()) << 6 | e.field, &[&e]))
        }
        "adda" | "suba" | "cmpa" => {
            need(parts, 2, m)?;
            let (src, dst) = (ea(a, 0)?, ea(a, 1)?);
            let Ea::An(an) = dst else { return Err(format!("`{m}` needs an address register")) };
            let base: u16 = match m {
                "adda" => 0xd000,
                "suba" => 0x9000,
                _ => 0xb000,
            };
            let mode = match sz {
                Sz::W => 3,
                Sz::L => 7,
                Sz::B => return Err(format!("`{m}` is .w or .l")),
            };
            let e = a.ea(&src, sz, pc + 2)?;
            Ok(with(base | an << 9 | mode << 6 | e.field, &[&e]))
        }
        "ori" | "andi" | "subi" | "addi" | "eori" | "cmpi" => {
            need(parts, 2, m)?;
            let base: u16 = match m {
                "ori" => 0x0000,
                "andi" => 0x0200,
                "subi" => 0x0400,
                "addi" => 0x0600,
                "eori" => 0x0a00,
                _ => 0x0c00,
            };
            let (src, dst) = (ea(a, 0)?, ea(a, 1)?);
            let Ea::Imm(n) = src else { return Err(format!("`{m}` needs #imm")) };
            match dst {
                Ea::Special("ccr") => return Ok(vec![base | 0x3c, fit(a.ctx, n, -128, 255, 8, "value") as u16 & 0xff]),
                Ea::Special("sr") => return Ok(vec![base | 0x7c, n.value as u16]),
                _ => {}
            }
            let i = a.ea(&src, sz, pc + 2)?;
            let e = a.ea(&dst, sz, pc + 2 + i.ext.len() as i64 * 2)?;
            Ok(with(base | sz.bits() << 6 | e.field, &[&i, &e]))
        }
        "addq" | "subq" => {
            need(parts, 2, m)?;
            let (src, dst) = (ea(a, 0)?, ea(a, 1)?);
            let Ea::Imm(n) = src else { return Err(format!("`{m}` needs #imm")) };
            let q = fit(a.ctx, n, 1, 8, 4, "quick value") as u16 & 7;
            let e = a.ea(&dst, sz, pc + 2)?;
            let sub = if m == "subq" { 0x100 } else { 0 };
            Ok(with(0x5000 | q << 9 | sub | sz.bits() << 6 | e.field, &[&e]))
        }
        "addx" | "subx" | "abcd" | "sbcd" => {
            need(parts, 2, m)?;
            let (src, dst) = (ea(a, 0)?, ea(a, 1)?);
            let base: u16 = match m {
                "addx" => 0xd100 | sz.bits() << 6,
                "subx" => 0x9100 | sz.bits() << 6,
                "abcd" => 0xc100,
                _ => 0x8100,
            };
            match (src, dst) {
                (Ea::Dn(y), Ea::Dn(x)) => Ok(vec![base | x << 9 | y]),
                (Ea::PreDec(y), Ea::PreDec(x)) => Ok(vec![base | x << 9 | 8 | y]),
                _ => Err(format!("`{m}` needs Dy,Dx or -(Ay),-(Ax)")),
            }
        }
        "cmpm" => {
            need(parts, 2, m)?;
            match (ea(a, 0)?, ea(a, 1)?) {
                (Ea::PostInc(y), Ea::PostInc(x)) => Ok(vec![0xb108 | x << 9 | sz.bits() << 6 | y]),
                _ => Err("cmpm needs (Ay)+,(Ax)+".into()),
            }
        }
        "negx" | "clr" | "neg" | "not" | "tst" => {
            need(parts, 1, m)?;
            let base: u16 = match m {
                "negx" => 0x4000,
                "clr" => 0x4200,
                "neg" => 0x4400,
                "not" => 0x4600,
                _ => 0x4a00,
            };
            let d = ea(a, 0)?;
            let e = a.ea(&d, sz, pc + 2)?;
            Ok(with(base | sz.bits() << 6 | e.field, &[&e]))
        }
        "nbcd" | "tas" | "pea" | "jmp" | "jsr" => {
            need(parts, 1, m)?;
            let base: u16 = match m {
                "nbcd" => 0x4800,
                "tas" => 0x4ac0,
                "pea" => 0x4840,
                "jmp" => 0x4ec0,
                _ => 0x4e80,
            };
            let d = ea(a, 0)?;
            let esz = if matches!(m, "nbcd" | "tas") { Sz::B } else { Sz::L };
            let e = a.ea(&d, esz, pc + 2)?;
            Ok(with(base | e.field, &[&e]))
        }
        "lea" => {
            need(parts, 2, m)?;
            let (src, dst) = (ea(a, 0)?, ea(a, 1)?);
            let Ea::An(an) = dst else { return Err("lea needs an address register".into()) };
            let e = a.ea(&src, Sz::L, pc + 2)?;
            Ok(with(0x41c0 | an << 9 | e.field, &[&e]))
        }
        "chk" => {
            need(parts, 2, m)?;
            let (src, dst) = (ea(a, 0)?, ea(a, 1)?);
            let Ea::Dn(d) = dst else { return Err("chk needs a data register".into()) };
            let e = a.ea(&src, sz, pc + 2)?;
            let op = match sz {
                Sz::L => {
                    req020("chk.l")?;
                    0x4100
                }
                _ => 0x4180,
            };
            Ok(with(op | d << 9 | e.field, &[&e]))
        }
        "ext" | "extb" | "swap" => {
            need(parts, 1, m)?;
            let Ea::Dn(d) = ea(a, 0)? else { return Err(format!("`{m}` needs a data register")) };
            Ok(vec![match (m, sz) {
                ("swap", _) => 0x4840 | d,
                ("extb", _) => {
                    req020(m)?;
                    0x49c0 | d
                }
                (_, Sz::L) => 0x48c0 | d,
                _ => 0x4880 | d,
            }])
        }
        "link" => {
            need(parts, 2, m)?;
            let Ea::An(an) = ea(a, 0)? else { return Err("link needs an address register".into()) };
            let Ea::Imm(n) = ea(a, 1)? else { return Err("link needs #displacement".into()) };
            if sz == Sz::L {
                req020("link.l")?;
                let mut v = vec![0x4808 | an];
                push32(&mut v, n.value);
                Ok(v)
            } else {
                Ok(vec![0x4e50 | an, fit(a.ctx, n, -32768, 32767, 16, "displacement") as u16])
            }
        }
        "unlk" => {
            need(parts, 1, m)?;
            let Ea::An(an) = ea(a, 0)? else { return Err("unlk needs an address register".into()) };
            Ok(vec![0x4e58 | an])
        }
        "trap" | "stop" | "rtd" | "bkpt" => {
            need(parts, 1, m)?;
            let Ea::Imm(n) = ea(a, 0)? else { return Err(format!("`{m}` needs #imm")) };
            Ok(match m {
                "trap" => vec![0x4e40 | fit(a.ctx, n, 0, 15, 4, "trap vector") as u16],
                "stop" => vec![0x4e72, n.value as u16],
                "rtd" => {
                    req020(m)?;
                    vec![0x4e74, n.value as u16]
                }
                _ => {
                    req020(m)?;
                    vec![0x4848 | fit(a.ctx, n, 0, 7, 3, "breakpoint") as u16]
                }
            })
        }
        "exg" => {
            need(parts, 2, m)?;
            match (ea(a, 0)?, ea(a, 1)?) {
                (Ea::Dn(x), Ea::Dn(y)) => Ok(vec![0xc140 | x << 9 | y]),
                (Ea::An(x), Ea::An(y)) => Ok(vec![0xc148 | x << 9 | y]),
                (Ea::Dn(x), Ea::An(y)) | (Ea::An(y), Ea::Dn(x)) => Ok(vec![0xc188 | x << 9 | y]),
                _ => Err("exg needs two registers".into()),
            }
        }
        "mulu" | "muls" | "divu" | "divs" | "divul" | "divsl" => {
            need(parts, 2, m)?;
            let src = ea(a, 0)?;
            let signed = m.contains('s');
            let div = m.starts_with("div");
            let pair = split_top(toks, parts[1].clone(), ":");
            if sz == Sz::L || m.ends_with('l') || pair.len() == 2 {
                req020(&format!("{m}.l"))?;
                let regs: Vec<u16> = pair
                    .iter()
                    .map(|r| dn_of(&toks[r.start]).filter(|_| r.len() == 1).ok_or("expected a data register"))
                    .collect::<Result<_, _>>()?;
                let (dh, dl, wide) = match (regs.as_slice(), m.ends_with('l')) {
                    ([dl], _) => (*dl, *dl, false),
                    ([dh, dl], true) => (*dh, *dl, false),
                    ([dh, dl], false) => (*dh, *dl, true),
                    _ => return Err("bad register pair".into()),
                };
                let e = a.ea(&src, Sz::L, pc + 4)?;
                let ext = dl << 12 | if signed { 0x800 } else { 0 } | if wide { 0x400 } else { 0 } | dh;
                let op = if div { 0x4c40 } else { 0x4c00 };
                return Ok(with(op | e.field, &[&Enc { field: 0, ext: vec![ext] }, &e]));
            }
            let Ea::Dn(d) = ea(a, 1)? else { return Err(format!("`{m}` needs a data register")) };
            let e = a.ea(&src, Sz::W, pc + 2)?;
            let op: u16 = match (div, signed) {
                (false, false) => 0xc0c0,
                (false, true) => 0xc1c0,
                (true, false) => 0x80c0,
                (true, true) => 0x81c0,
            };
            Ok(with(op | d << 9 | e.field, &[&e]))
        }
        "asl" | "asr" | "lsl" | "lsr" | "rol" | "ror" | "roxl" | "roxr" => {
            let kind: u16 = match &m[..m.len() - 1] {
                "as" => 0,
                "ls" => 1,
                "rox" => 2,
                _ => 3,
            };
            let left = if m.ends_with('l') { 0x100 } else { 0 };
            if parts.len() == 1 {
                let d = ea(a, 0)?;
                if let Ea::Dn(dy) = d {
                    return Ok(vec![0xe000 | 1 << 9 | left | sz.bits() << 6 | kind << 3 | dy]);
                }
                let e = a.ea(&d, Sz::W, pc + 2)?;
                return Ok(with(0xe0c0 | kind << 9 | left | e.field, &[&e]));
            }
            need(parts, 2, m)?;
            let (c, d) = (ea(a, 0)?, ea(a, 1)?);
            let Ea::Dn(dy) = d else { return Err("shift destination must be a data register".into()) };
            match c {
                Ea::Imm(n) => {
                    let q = fit(a.ctx, n, 1, 8, 4, "shift count") as u16 & 7;
                    Ok(vec![0xe000 | q << 9 | left | sz.bits() << 6 | kind << 3 | dy])
                }
                Ea::Dn(dx) => Ok(vec![0xe000 | dx << 9 | left | sz.bits() << 6 | 0x20 | kind << 3 | dy]),
                _ => Err("shift count must be #n or Dn".into()),
            }
        }
        "btst" | "bchg" | "bclr" | "bset" => {
            need(parts, 2, m)?;
            let kind: u16 = match m {
                "btst" => 0,
                "bchg" => 1,
                "bclr" => 2,
                _ => 3,
            };
            let (b, d) = (ea(a, 0)?, ea(a, 1)?);
            match b {
                Ea::Dn(dn) => {
                    let e = a.ea(&d, Sz::B, pc + 2)?;
                    Ok(with(0x0100 | dn << 9 | kind << 6 | e.field, &[&e]))
                }
                Ea::Imm(n) => {
                    let e = a.ea(&d, Sz::B, pc + 4)?;
                    Ok(with(0x0800 | kind << 6 | e.field, &[&Enc { field: 0, ext: vec![n.value as u16 & 0xff] }, &e]))
                }
                _ => Err("bit number must be #n or Dn".into()),
            }
        }
        "pack" | "unpk" => {
            req020(m)?;
            need(parts, 3, m)?;
            let (x, y, adj) = (ea(a, 0)?, ea(a, 1)?, ea(a, 2)?);
            let Ea::Imm(n) = adj else { return Err("missing #adjustment".into()) };
            let base: u16 = if m == "pack" { 0x8140 } else { 0x8180 };
            match (x, y) {
                (Ea::Dn(s), Ea::Dn(d)) => Ok(vec![base | d << 9 | s, n.value as u16]),
                (Ea::PreDec(s), Ea::PreDec(d)) => Ok(vec![base | d << 9 | 8 | s, n.value as u16]),
                _ => Err(format!("`{m}` needs Dy,Dx or -(Ay),-(Ax)")),
            }
        }
        "cas" => {
            req020(m)?;
            need(parts, 3, m)?;
            let (dc, du, e) = (ea(a, 0)?, ea(a, 1)?, ea(a, 2)?);
            let (Ea::Dn(dc), Ea::Dn(du)) = (dc, du) else { return Err("cas needs Dc,Du,<ea>".into()) };
            let szf: u16 = match sz {
                Sz::B => 0x0ac0,
                Sz::W => 0x0cc0,
                Sz::L => 0x0ec0,
            };
            let en = a.ea(&e, sz, pc + 4)?;
            Ok(with(szf | en.field, &[&Enc { field: 0, ext: vec![du << 6 | dc] }, &en]))
        }
        "cas2" => {
            req020(m)?;
            need(parts, 3, m)?;
            let pairs: Vec<Vec<Range<usize>>> = parts.iter().map(|p| split_top(toks, p.clone(), ":")).collect();
            let dreg = |r: &Range<usize>| dn_of(&toks[r.start]).filter(|_| r.len() == 1).ok_or("expected a data register".to_string());
            let mreg = |r: &Range<usize>| -> Result<u16, String> {
                if r.len() == 3 && toks[r.start].is_punct("(") {
                    let t = &toks[r.start + 1];
                    if let Some(n) = dn_of(t) {
                        return Ok(n << 12);
                    }
                    if let Some(n) = an_of(t) {
                        return Ok(0x8000 | n << 12);
                    }
                }
                Err("expected (Rn)".into())
            };
            if pairs.iter().any(|p| p.len() != 2) {
                return Err("cas2 needs Dc1:Dc2,Du1:Du2,(Rn1):(Rn2)".into());
            }
            let e1 = mreg(&pairs[2][0])? | dreg(&pairs[1][0])? << 6 | dreg(&pairs[0][0])?;
            let e2 = mreg(&pairs[2][1])? | dreg(&pairs[1][1])? << 6 | dreg(&pairs[0][1])?;
            let op = if sz == Sz::L { 0x0efc } else { 0x0cfc };
            Ok(vec![op, e1, e2])
        }
        "chk2" | "cmp2" => {
            req020(m)?;
            need(parts, 2, m)?;
            let (e, r) = (ea(a, 0)?, ea(a, 1)?);
            let g = match r {
                Ea::Dn(n) => n << 12,
                Ea::An(n) => 0x8000 | n << 12,
                _ => return Err(format!("`{m}` needs a register")),
            };
            let en = a.ea(&e, sz, pc + 4)?;
            let ext = g | if m == "chk2" { 0x0800 } else { 0 };
            Ok(with(sz.bits() << 9 | 0x00c0 | en.field, &[&Enc { field: 0, ext: vec![ext] }, &en]))
        }
        _ if m.starts_with("bf") => {
            req020(m)?;
            let kind: u16 = match m {
                "bftst" => 0,
                "bfextu" => 1,
                "bfchg" => 2,
                "bfexts" => 3,
                "bfclr" => 4,
                "bfffo" => 5,
                "bfset" => 6,
                "bfins" => 7,
                _ => return Err(format!("unknown instruction `{m}`")),
            };
            // locate the field operand `<ea>{off:width}` and the data register
            let (fi, di) = match kind {
                7 => (1, Some(0)),
                1 | 3 | 5 => (0, Some(1)),
                _ => (0, None),
            };
            need(parts, if di.is_some() { 2 } else { 1 }, m)?;
            let fr = parts[fi].clone();
            let open = (fr.start..fr.end).find(|&i| toks[i].is_punct("{")).ok_or("missing {offset:width}")?;
            if !toks[fr.end - 1].is_punct("}") {
                return Err("missing }".into());
            }
            let spec = split_top(toks, open + 1..fr.end - 1, ":");
            if spec.len() != 2 {
                return Err("expected {offset:width}".into());
            }
            let mut ext = 0u16;
            if let Some(d) = dn_of(&toks[spec[0].start]).filter(|_| spec[0].len() == 1) {
                ext |= 0x0800 | d << 6;
            } else {
                ext |= (eval(a.ctx, &spec[0])?.value as u16 & 31) << 6;
            }
            if let Some(d) = dn_of(&toks[spec[1].start]).filter(|_| spec[1].len() == 1) {
                ext |= 0x0020 | d;
            } else {
                ext |= eval(a.ctx, &spec[1])?.value as u16 & 31;
            }
            if let Some(di) = di {
                let d = dn_of(&toks[parts[di].start]).filter(|_| parts[di].len() == 1).ok_or("expected a data register")?;
                ext |= d << 12;
            }
            let e = parse_ea(toks, fr.start..open, a.ctx)?;
            let en = a.ea(&e, Sz::L, pc + 4)?;
            Ok(with(0xe8c0 | kind << 8 | en.field, &[&Enc { field: 0, ext: vec![ext] }, &en]))
        }
        _ => {
            if let Some(c) = split_cc(m, "trap") {
                req020(m)?;
                return Ok(match size {
                    None => {
                        need(parts, 0, m)?;
                        vec![0x50fc | c << 8]
                    }
                    Some(Sz::W) => {
                        let Ea::Imm(n) = ea(a, 0)? else { return Err("needs #imm".into()) };
                        vec![0x50fa | c << 8, n.value as u16]
                    }
                    Some(_) => {
                        let Ea::Imm(n) = ea(a, 0)? else { return Err("needs #imm".into()) };
                        let mut v = vec![0x50fb | c << 8];
                        push32(&mut v, n.value);
                        v
                    }
                });
            }
            if let Some(c) = split_cc(m, "s") {
                need(parts, 1, m)?;
                let d = ea(a, 0)?;
                let e = a.ea(&d, Sz::B, pc + 2)?;
                return Ok(with(0x50c0 | c << 8 | e.field, &[&e]));
            }
            Err(format!("unknown instruction `{m}`"))
        }
    }
}

/// A register list (`d0-d3/a0`) or a single register (not an EA).
fn parse_ea_is_reglist(toks: &[Token], r: &Range<usize>) -> bool {
    toks[r.clone()].iter().all(|t| matches!(t.tok, Tok::Ident(_)) || t.is_punct("-") || t.is_punct("/"))
}
