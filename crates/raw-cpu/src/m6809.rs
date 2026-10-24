//! Motorola 6809 (Vectrex), Motorola syntax, big-endian.
//!
//! Opcode pages and modes ported from `mimic/crates/cpu-6809/src/table.rs`,
//! indexed postbytes from `disasm.rs`. Direct page is assumed 0 until
//! `setdp`; `<expr` forces direct (or an 8-bit offset), `>expr` extended
//! (or a 16-bit offset).

use std::collections::HashMap;
use std::ops::Range;

use raw_core::{Endian, Tok, Token};

use crate::util::*;
use crate::{is_reg, Cpu, CpuInfo, CpuState, Num, OperandCtx};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Inh,
    Imm8,
    Imm16,
    Dir,
    Ext,
    Idx,
    Rel8,
    Rel16,
    StackS,
    StackU,
    Pair,
}

const BRANCH: [&str; 16] = [
    "bra", "brn", "bhi", "bls", "bcc", "bcs", "bne", "beq", "bvc", "bvs", "bpl", "bmi", "bge", "blt", "bgt", "ble",
];
const RMW: [&str; 16] = [
    "neg", "neg", "ngc", "com", "lsr", "lsr", "ror", "asr", "asl", "rol", "dec", "dec", "inc", "tst", "jmp", "clr",
];
const ALU_A: [&str; 16] = [
    "suba", "cmpa", "sbca", "subd", "anda", "bita", "lda", "sta", "eora", "adca", "ora", "adda", "cmpx", "jsr", "ldx", "stx",
];
const ALU_B: [&str; 16] = [
    "subb", "cmpb", "sbcb", "addd", "andb", "bitb", "ldb", "stb", "eorb", "adcb", "orb", "addb", "ldd", "std", "ldu", "stu",
];

fn page0(op: u8) -> Option<Mode> {
    Some(match op {
        0x00..=0x0f => Mode::Dir,
        0x12 | 0x13 | 0x14 | 0x15 | 0x19 | 0x1b | 0x1d => Mode::Inh,
        0x16 | 0x17 => Mode::Rel16,
        0x1a | 0x1c => Mode::Imm8,
        0x1e | 0x1f => Mode::Pair,
        0x20..=0x2f => Mode::Rel8,
        0x30..=0x33 => Mode::Idx,
        0x34 | 0x35 => Mode::StackS,
        0x36 | 0x37 => Mode::StackU,
        0x39 | 0x3a | 0x3b | 0x3d | 0x3f => Mode::Inh,
        0x3c => Mode::Imm8,
        0x40..=0x5f => Mode::Inh,
        0x60..=0x6f => Mode::Idx,
        0x70..=0x7f => Mode::Ext,
        0x80..=0xff => return alu(op),
        _ => return None,
    })
}

fn alu(op: u8) -> Option<Mode> {
    let col = (op >> 4) & 3;
    let k = op & 15;
    let b = op >= 0xc0;
    if col == 0 && (k == 7 || k == 15 || (b && k == 13)) {
        return None;
    }
    if !b && k == 13 && col == 0 {
        return Some(Mode::Rel8);
    }
    let wide = matches!(k, 3 | 12 | 14 | 15) || (b && k == 13);
    Some([if wide { Mode::Imm16 } else { Mode::Imm8 }, Mode::Dir, Mode::Idx, Mode::Ext][col as usize])
}

fn page2(op: u8) -> Option<Mode> {
    match op {
        0x21..=0x2f => Some(Mode::Rel16),
        0x3f => Some(Mode::Inh),
        0x83 | 0x93 | 0xa3 | 0xb3 | 0x8c | 0x9c | 0xac | 0xbc => alu(op),
        0x8e | 0x9e | 0xae | 0xbe | 0x9f | 0xaf | 0xbf => alu(op),
        0xce | 0xde | 0xee | 0xfe | 0xdf | 0xef | 0xff => alu(op),
        _ => None,
    }
}

fn page3(op: u8) -> Option<Mode> {
    match op {
        0x3f => Some(Mode::Inh),
        0x83 | 0x93 | 0xa3 | 0xb3 | 0x8c | 0x9c | 0xac | 0xbc => alu(op),
        _ => None,
    }
}

fn name(page: u8, op: u8) -> String {
    let k = usize::from(op & 15);
    match (page, op) {
        (2, 0x21..=0x2f) => format!("l{}", BRANCH[k]),
        (2, 0x3f) => "swi2".into(),
        (3, 0x3f) => "swi3".into(),
        (2, _) => match k {
            3 => "cmpd",
            12 => "cmpy",
            _ if op >= 0xc0 => ["lds", "sts"][k & 1],
            _ => ["ldy", "sty"][k & 1],
        }
        .into(),
        (3, _) => if k == 3 { "cmpu" } else { "cmps" }.into(),
        (_, 0x00..=0x0f) | (_, 0x60..=0x7f) => RMW[k].into(),
        (_, 0x40..=0x5f) => {
            let n = if k == 14 { "clr" } else { RMW[k] };
            format!("{n}{}", if op < 0x50 { 'a' } else { 'b' })
        }
        (_, 0x20..=0x2f) => BRANCH[k].into(),
        (_, 0x8d) => "bsr".into(),
        (_, 0x80..=0xbf) => ALU_A[k].into(),
        (_, 0xc0..=0xff) => ALU_B[k].into(),
        _ => match op {
            0x12 | 0x1b => "nop",
            0x13 => "sync",
            0x14 | 0x15 => "hcf",
            0x16 => "lbra",
            0x17 => "lbsr",
            0x19 => "daa",
            0x1a => "orcc",
            0x1c => "andcc",
            0x1d => "sex",
            0x1e => "exg",
            0x1f => "tfr",
            0x30 => "leax",
            0x31 => "leay",
            0x32 => "leas",
            0x33 => "leau",
            0x34 => "pshs",
            0x35 => "puls",
            0x36 => "pshu",
            0x37 => "pulu",
            0x39 => "rts",
            0x3a => "abx",
            0x3b => "rti",
            0x3c => "cwai",
            0x3d => "mul",
            0x3f => "swi",
            _ => "???",
        }
        .into(),
    }
}

const ALIASES: &[(&str, &str)] = &[
    ("lsl", "asl"),
    ("lsla", "asla"),
    ("lslb", "aslb"),
    ("bhs", "bcc"),
    ("blo", "bcs"),
    ("lbhs", "lbcc"),
    ("lblo", "lbcs"),
    ("swi1", "swi"),
];

const PAIR: [(&str, u8); 10] = [
    ("d", 0), ("x", 1), ("y", 2), ("u", 3), ("s", 4), ("pc", 5), ("a", 8), ("b", 9), ("cc", 10), ("dp", 11),
];

/// Encoding: prefix (0, $10, $11) and opcode.
type Enc = (u8, u8);

pub struct M6809 {
    ops: HashMap<String, HashMap<Mode, Enc>>,
}

impl M6809 {
    pub fn new() -> M6809 {
        let mut ops: HashMap<String, HashMap<Mode, Enc>> = HashMap::new();
        for (page, prefix) in [(0u8, 0u8), (2, 0x10), (3, 0x11)] {
            for op in 0..=255u8 {
                let mode = match page {
                    0 if op == 0x10 || op == 0x11 => None,
                    0 => page0(op),
                    2 => page2(op),
                    _ => page3(op),
                };
                if let Some(mode) = mode {
                    ops.entry(name(page, op)).or_default().entry(mode).or_insert((prefix, op));
                }
            }
        }
        ops.remove("???");
        // the undocumented page-0 aliases ($01 NEG...) lose to the documented ones
        ops.get_mut("neg").unwrap().insert(Mode::Dir, (0, 0x00));
        ops.get_mut("lsr").unwrap().insert(Mode::Dir, (0, 0x04));
        ops.get_mut("dec").unwrap().insert(Mode::Dir, (0, 0x0a));
        ops.get_mut("nop").unwrap().insert(Mode::Inh, (0, 0x12));
        M6809 { ops }
    }

    fn lookup(&self, m: &str) -> Option<&HashMap<Mode, Enc>> {
        let m = m.to_ascii_lowercase();
        self.ops.get(&m).or_else(|| self.ops.get(ALIASES.iter().find(|(a, _)| *a == m)?.1))
    }
}

impl Default for M6809 {
    fn default() -> Self {
        Self::new()
    }
}

fn index_reg(t: &Token) -> Option<u8> {
    let s = t.ident()?.to_ascii_lowercase();
    Some(match s.as_str() {
        "x" => 0,
        "y" => 1,
        "u" => 2,
        "s" => 3,
        _ => return None,
    })
}

/// Strip a `<` / `>` size prefix: 1 = force short, 2 = force long.
fn force_prefix(toks: &[Token], r: Range<usize>) -> (Range<usize>, Option<u8>) {
    if r.end > r.start + 1 {
        if toks[r.start].is_punct("<") {
            return (r.start + 1..r.end, Some(1));
        }
        if toks[r.start].is_punct(">") {
            return (r.start + 1..r.end, Some(2));
        }
    }
    (r, None)
}

impl M6809 {
    /// Postbyte and extra bytes of an indexed operand.
    fn indexed(
        &self,
        ctx: &mut dyn OperandCtx,
        toks: &[Token],
        r: Range<usize>,
        head_len: i64,
    ) -> Result<Vec<u8>, String> {
        let mut ind = 0u8;
        let mut r = r;
        if wrapped(toks, &r, "[", "]") {
            ind = 0x10;
            r = r.start + 1..r.end - 1;
            let parts = split_top(toks, r.clone(), ",");
            if parts.len() == 1 {
                // extended indirect [nnnn]
                let n = eval(ctx, &parts[0])?;
                let mut v = vec![0x9f];
                v.extend(be16(fit(ctx, n, -32768, 0xffff, 16, "address")));
                return Ok(v);
            }
        }
        let parts = split_top(toks, r.clone(), ",");
        if parts.len() != 2 {
            return Err("bad indexed operand".into());
        }
        let (left, right) = (parts[0].clone(), parts[1].clone());
        let rt = &toks[right.clone()];
        // auto increment / decrement
        let (reg, auto) = match rt {
            [t] => (index_reg(t), None),
            [t, p] if p.is_punct("+") => (index_reg(t), Some(0x80u8)),
            [t, p] if p.is_punct("++") => (index_reg(t), Some(0x81)),
            [p, t] if p.is_punct("-") => (index_reg(t), Some(0x82)),
            [p, t] if p.is_punct("--") => (index_reg(t), Some(0x83)),
            _ => (None, None),
        };
        let pcr = rt.len() == 1 && (is_reg(&rt[0], "pcr") || is_reg(&rt[0], "pc"));
        if pcr {
            let relative = is_reg(&rt[0], "pcr");
            let (lr, force) = force_prefix(toks, left);
            let n = eval(ctx, &lr)?;
            let pc = i64::from(ctx.pc());
            let short = match force {
                Some(f) => f == 1,
                None => {
                    if ctx.min_size() >= 3 || !n.known {
                        false
                    } else {
                        let off = if relative { n.value - (pc + head_len + 2) } else { n.value };
                        (-128..=127).contains(&off)
                    }
                }
            };
            ctx.grow(if short { 2 } else { 3 });
            return Ok(if short {
                let off = if relative { n.value - (pc + head_len + 2) } else { n.value };
                vec![0x8c | ind, fit(ctx, Num::with(n, off), -128, 127, 8, "offset") as u8]
            } else {
                let off = if relative { n.value - (pc + head_len + 3) } else { n.value };
                let mut v = vec![0x8d | ind];
                v.extend(be16(off));
                v
            });
        }
        let reg = reg.ok_or("expected index register x, y, u, s or pc(r)")?;
        let rr = reg << 5;
        if let Some(a) = auto {
            if !left.is_empty() {
                return Err("auto increment/decrement takes no offset".into());
            }
            if ind != 0 && (a == 0x80 || a == 0x82) {
                return Err("indirect needs double increment/decrement".into());
            }
            return Ok(vec![0x80 | rr | a | ind]);
        }
        if left.is_empty() {
            return Ok(vec![0x84 | rr | ind]);
        }
        if let Some(acc) = single_ident(toks, &left).map(|s| s.to_ascii_lowercase()) {
            match acc.as_str() {
                "a" => return Ok(vec![0x86 | rr | ind]),
                "b" => return Ok(vec![0x85 | rr | ind]),
                "d" => return Ok(vec![0x8b | rr | ind]),
                _ => {}
            }
        }
        let (lr, force) = force_prefix(toks, left);
        let n = eval(ctx, &lr)?;
        // size classes: 0 = none, 1 = 5 bit, 2 = 8 bit, 3 = 16 bit
        let want = match force {
            Some(1) => 2,
            Some(_) => 3,
            None if !n.known => 3,
            None if n.value == 0 && ind == 0 => 0,
            None if ind == 0 && (-16..=15).contains(&n.value) => 1,
            None if (-128..=127).contains(&n.value) => 2,
            None => 3,
        };
        let size = if force.is_some() { want } else { want.max(ctx.min_size()) };
        ctx.grow(size);
        Ok(match size {
            0 => vec![0x84 | rr | ind],
            1 => vec![rr | (n.value as u8 & 0x1f)],
            2 => vec![0x88 | rr | ind, fit(ctx, n, -128, 127, 8, "offset") as u8],
            _ => {
                let mut v = vec![0x89 | rr | ind];
                v.extend(be16(fit(ctx, n, -32768, 0xffff, 16, "offset")));
                v
            }
        })
    }
}

impl Cpu for M6809 {
    fn info(&self) -> CpuInfo {
        CpuInfo {
            id: "6809",
            aliases: &["68a09", "mc6809"],
            description: "Motorola 6809 / 68A09 (Vectrex), big-endian, `setdp` for direct page",
            endian: "big",
            addr_bits: 16,
        }
    }

    fn endian(&self) -> Endian {
        Endian::Big
    }

    fn has_mnemonic(&self, m: &str) -> bool {
        self.lookup(m).is_some()
    }

    fn mnemonics(&self) -> Vec<String> {
        let mut v: Vec<String> = self.ops.keys().cloned().collect();
        v.extend(ALIASES.iter().map(|(a, _)| a.to_string()));
        v.sort();
        v
    }

    fn assemble(
        &self,
        mnemonic: &str,
        _suffix: Option<&str>,
        ctx: &mut dyn OperandCtx,
        st: &CpuState,
    ) -> Result<Vec<u8>, String> {
        let m = mnemonic.to_ascii_lowercase();
        let modes = self.lookup(&m).ok_or_else(|| format!("unknown instruction `{mnemonic}`"))?;
        let toks = ctx.toks().to_vec();
        let n = toks.len();
        let head = |e: Enc| -> Vec<u8> {
            if e.0 == 0 { vec![e.1] } else { vec![e.0, e.1] }
        };
        let bad = || format!("`{m}` does not support this addressing mode");
        let pc = i64::from(ctx.pc());
        if n == 0 {
            let e = *modes.get(&Mode::Inh).ok_or_else(|| format!("`{m}` needs an operand"))?;
            return Ok(head(e));
        }
        if let Some(&e) = modes.get(&Mode::Rel8) {
            let t = eval(ctx, &(0..n))?;
            let mut v = head(e);
            let next = pc + v.len() as i64 + 1;
            v.push(rel(ctx, t, next, 8) as u8);
            return Ok(v);
        }
        if let Some(&e) = modes.get(&Mode::Rel16) {
            let t = eval(ctx, &(0..n))?;
            let mut v = head(e);
            let next = pc + v.len() as i64 + 2;
            v.extend(be16(rel(ctx, t, next, 16)));
            return Ok(v);
        }
        if let Some(&e) = modes.get(&Mode::StackS).or(modes.get(&Mode::StackU)) {
            let user = modes.contains_key(&Mode::StackU);
            let mut mask = 0u8;
            if toks[0].is_punct("#") {
                let v = eval(ctx, &(1..n))?;
                mask = fit(ctx, v, 0, 255, 8, "register mask") as u8;
            } else {
                for r in commas(&toks) {
                    let s = single_ident(&toks, &r).ok_or("expected a register list")?.to_ascii_lowercase();
                    let bit = match s.as_str() {
                        "cc" | "ccr" => 0,
                        "a" => 1,
                        "b" => 2,
                        "d" => {
                            mask |= 6;
                            continue;
                        }
                        "dp" => 3,
                        "x" => 4,
                        "y" => 5,
                        "u" if !user => 6,
                        "s" if user => 6,
                        "pc" => 7,
                        _ => return Err(format!("register `{s}` cannot be pushed/pulled here")),
                    };
                    mask |= 1 << bit;
                }
            }
            let mut v = head(e);
            v.push(mask);
            return Ok(v);
        }
        if let Some(&e) = modes.get(&Mode::Pair) {
            let parts = commas(&toks);
            if parts.len() != 2 {
                return Err(format!("`{m}` needs two registers"));
            }
            let code = |r: &Range<usize>| -> Result<u8, String> {
                let s = single_ident(&toks, r).ok_or("expected a register")?.to_ascii_lowercase();
                PAIR.iter().find(|(n, _)| *n == s).map(|p| p.1).ok_or(format!("unknown register `{s}`"))
            };
            let mut v = head(e);
            v.push(code(&parts[0])? << 4 | code(&parts[1])?);
            return Ok(v);
        }
        if toks[0].is_punct("#") {
            let r = 1..n;
            let nv = eval(ctx, &r)?;
            if let Some(&e) = modes.get(&Mode::Imm8) {
                let mut v = head(e);
                v.push(fit(ctx, nv, -128, 255, 8, "immediate value") as u8);
                return Ok(v);
            }
            if let Some(&e) = modes.get(&Mode::Imm16) {
                let mut v = head(e);
                v.extend(be16(fit(ctx, nv, -32768, 0xffff, 16, "immediate value")));
                return Ok(v);
            }
            return Err(format!("`{m}` has no immediate mode"));
        }
        let indexed = commas(&toks).len() > 1 || wrapped(&toks, &(0..n), "[", "]");
        if indexed {
            let e = *modes.get(&Mode::Idx).ok_or_else(bad)?;
            let mut v = head(e);
            let hl = v.len() as i64;
            v.extend(self.indexed(ctx, &toks, 0..n, hl)?);
            return Ok(v);
        }
        // direct or extended
        let (r, force) = force_prefix(&toks, 0..n);
        let nv = eval(ctx, &r)?;
        let dp = i64::from(st.dp.unwrap_or(0));
        let dir_ok = modes.contains_key(&Mode::Dir);
        let ext_ok = modes.contains_key(&Mode::Ext);
        let short = match force {
            Some(f) => f == 1,
            None => dir_ok && ctx.min_size() < 2 && nv.known && (nv.value >> 8) == dp,
        };
        if dir_ok && ext_ok && force.is_none() {
            ctx.grow(if short { 1 } else { 2 });
        }
        if short || !ext_ok {
            let e = *modes.get(&Mode::Dir).ok_or_else(bad)?;
            let mut v = head(e);
            v.push(nv.value as u8);
            return Ok(v);
        }
        let e = *modes.get(&Mode::Ext).ok_or_else(bad)?;
        let mut v = head(e);
        v.extend(be16(fit(ctx, nv, -32768, 0xffff, 16, "address")));
        Ok(v)
    }

    fn disassemble(&self, bytes: &[u8], addr: u32, _st: &CpuState) -> Option<(usize, String)> {
        let mut i = 0usize;
        let mut next = || {
            let v = bytes.get(i).copied();
            i += 1;
            v
        };
        let mut page = 0;
        let mut op = next()?;
        if op == 0x10 || op == 0x11 {
            page = if op == 0x10 { 2 } else { 3 };
            op = next()?;
        }
        let mode = match page {
            2 => page2(op),
            3 => page3(op),
            _ => page0(op),
        }?;
        let m = name(page, op);
        let hx = |v: i32| if v < 0 { format!("-${:x}", -v) } else { format!("${v:x}") };
        let operand = match mode {
            Mode::Inh => String::new(),
            Mode::Imm8 => format!("#${:02x}", next()?),
            Mode::Imm16 => format!("#${:04x}", u16::from_be_bytes([next()?, next()?])),
            Mode::Dir => format!("<${:02x}", next()?),
            Mode::Ext => format!("${:04x}", u16::from_be_bytes([next()?, next()?])),
            Mode::Rel8 => {
                let o = next()? as i8;
                format!("${:04x}", (addr as u16).wrapping_add(i as u16).wrapping_add(o as u16))
            }
            Mode::Rel16 => {
                let o = u16::from_be_bytes([next()?, next()?]);
                format!("${:04x}", (addr as u16).wrapping_add(i as u16).wrapping_add(o))
            }
            Mode::StackS | Mode::StackU => {
                let mask = next()?;
                let other = if mode == Mode::StackS { "u" } else { "s" };
                let names = ["cc", "a", "b", "dp", "x", "y", other, "pc"];
                (0..8).filter(|b| mask & (1 << b) != 0).map(|b| names[b]).collect::<Vec<_>>().join(",")
            }
            Mode::Pair => {
                let p = next()?;
                let nm = |c: u8| PAIR.iter().find(|x| x.1 == c).map_or("?", |x| x.0);
                format!("{},{}", nm(p >> 4), nm(p & 15))
            }
            Mode::Idx => {
                let post = next()?;
                let r = ["x", "y", "u", "s"][((post >> 5) & 3) as usize];
                if post & 0x80 == 0 {
                    let off = i32::from(((post & 0x1f) << 3) as i8 >> 3);
                    format!("{},{r}", hx(off))
                } else {
                    let inner = match post & 15 {
                        0 => format!(",{r}+"),
                        1 => format!(",{r}++"),
                        2 => format!(",-{r}"),
                        3 => format!(",--{r}"),
                        4 => format!(",{r}"),
                        5 => format!("b,{r}"),
                        6 => format!("a,{r}"),
                        8 => format!("<{},{r}", hx(i32::from(next()? as i8))),
                        9 => format!(">{},{r}", hx(i32::from(i16::from_be_bytes([next()?, next()?])))),
                        11 => format!("d,{r}"),
                        12 => {
                            let o = next()? as i8;
                            format!("<${:04x},pcr", (addr as u16).wrapping_add(i as u16).wrapping_add(o as u16))
                        }
                        13 => {
                            let o = u16::from_be_bytes([next()?, next()?]);
                            format!(">${:04x},pcr", (addr as u16).wrapping_add(i as u16).wrapping_add(o))
                        }
                        15 => format!("${:04x}", u16::from_be_bytes([next()?, next()?])),
                        _ => return None,
                    };
                    if post & 0x10 != 0 && matches!(post & 15, 0 | 2) {
                        return None; // [,R+] and [,-R] are illegal
                    }
                    if post & 0x10 != 0 {
                        format!("[{inner}]")
                    } else {
                        inner
                    }
                }
            }
        };
        Some((i, if operand.is_empty() { m } else { format!("{m} {operand}") }))
    }
}

#[allow(dead_code)]
fn _unused(_: &Tok) {}
