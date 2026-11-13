//! Intel 8080 (IMSAI 8080) with the Intel mnemonics.
//!
//! Opcode fields as in `mimic/crates/cpu-8080/src/disasm.rs`. The
//! undocumented duplicates (`$08`..`$38` NOP, `$CB` JMP, `$D9` RET,
//! `$DD`/`$ED`/`$FD` CALL) are never produced and disassemble as data.

use std::ops::Range;

use rat_core::{Endian, Token};

use crate::util::*;
use crate::{Cpu, CpuInfo, CpuState, OperandCtx};

const REG: [&str; 8] = ["b", "c", "d", "e", "h", "l", "m", "a"];
const PAIR: [&str; 4] = ["b", "d", "h", "sp"];
const COND: [&str; 8] = ["nz", "z", "nc", "c", "po", "pe", "p", "m"];
const ALU: [&str; 8] = ["add", "adc", "sub", "sbb", "ana", "xra", "ora", "cmp"];
const ALU_IMM: [&str; 8] = ["adi", "aci", "sui", "sbi", "ani", "xri", "ori", "cpi"];
const IMPLIED: [(&str, u8); 17] = [
    ("nop", 0x00),
    ("rlc", 0x07),
    ("rrc", 0x0f),
    ("ral", 0x17),
    ("rar", 0x1f),
    ("daa", 0x27),
    ("cma", 0x2f),
    ("stc", 0x37),
    ("cmc", 0x3f),
    ("hlt", 0x76),
    ("ret", 0xc9),
    ("xthl", 0xe3),
    ("pchl", 0xe9),
    ("xchg", 0xeb),
    ("di", 0xf3),
    ("sphl", 0xf9),
    ("ei", 0xfb),
];
/// Instructions with a 16-bit address.
const ADDR: [(&str, u8); 6] = [("shld", 0x22), ("lhld", 0x2a), ("sta", 0x32), ("lda", 0x3a), ("jmp", 0xc3), ("call", 0xcd)];
const OTHER: [&str; 13] = ["mov", "mvi", "lxi", "stax", "ldax", "inx", "dcx", "dad", "inr", "dcr", "push", "pop", "rst"];

pub struct I8080 {
    mnemonics: Vec<String>,
}

impl I8080 {
    pub fn new() -> I8080 {
        let mut m: Vec<String> = Vec::new();
        m.extend(IMPLIED.iter().map(|x| x.0.to_owned()));
        m.extend(ADDR.iter().map(|x| x.0.to_owned()));
        m.extend(OTHER.iter().map(|x| (*x).to_owned()));
        m.extend(ALU.iter().chain(&ALU_IMM).map(|x| (*x).to_owned()));
        m.extend(["in", "out"].map(String::from));
        for c in COND {
            for p in ["j", "c", "r"] {
                m.push(format!("{p}{c}"));
            }
        }
        m.sort();
        m.dedup();
        I8080 { mnemonics: m }
    }
}

impl Default for I8080 {
    fn default() -> Self {
        Self::new()
    }
}

fn index(table: &[&str], name: &str) -> Option<u8> {
    table.iter().position(|t| *t == name).map(|i| i as u8)
}

fn reg8(toks: &[Token], r: &Range<usize>) -> Result<u8, String> {
    single_ident(toks, r)
        .and_then(|s| index(&REG, &s.to_ascii_lowercase()))
        .ok_or_else(|| "expected a register (B, C, D, E, H, L, M, A)".to_string())
}

/// Register pair; `psw` selects the PUSH/POP set (PSW instead of SP).
fn pair(toks: &[Token], r: &Range<usize>, psw: bool) -> Result<u8, String> {
    let name = single_ident(toks, r).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    Ok(match name.as_str() {
        "b" | "bc" => 0,
        "d" | "de" => 1,
        "h" | "hl" => 2,
        "sp" if !psw => 3,
        "psw" | "af" if psw => 3,
        _ => return Err(format!("expected a register pair (B, D, H, {})", if psw { "PSW" } else { "SP" })),
    })
}

impl Cpu for I8080 {
    fn info(&self) -> CpuInfo {
        CpuInfo {
            id: "8080",
            aliases: &["i8080", "8080a"],
            description: "Intel 8080 (IMSAI 8080), Intel mnemonics",
            endian: "little",
            addr_bits: 16,
        }
    }

    fn endian(&self) -> Endian {
        Endian::Little
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
        let byte = |ctx: &mut dyn OperandCtx, r: &Range<usize>| -> Result<u8, String> {
            let n = eval(ctx, r)?;
            Ok(fit(ctx, n, -128, 255, 8, "byte value") as u8)
        };
        let word = |ctx: &mut dyn OperandCtx, r: &Range<usize>| -> Result<[u8; 2], String> {
            let n = eval(ctx, r)?;
            Ok(le16(fit(ctx, n, -32768, 65535, 16, "word value")))
        };
        if let Some(&(_, op)) = IMPLIED.iter().find(|x| x.0 == m) {
            need(0)?;
            return Ok(vec![op]);
        }
        if let Some(&(_, op)) = ADDR.iter().find(|x| x.0 == m) {
            need(1)?;
            let w = word(ctx, &parts[0])?;
            return Ok(vec![op, w[0], w[1]]);
        }
        if let Some(i) = index(&ALU, m) {
            need(1)?;
            return Ok(vec![0x80 | i << 3 | reg8(&toks, &parts[0])?]);
        }
        if let Some(i) = index(&ALU_IMM, m) {
            need(1)?;
            return Ok(vec![0xc6 | i << 3, byte(ctx, &parts[0])?]);
        }
        Ok(match m {
            "mov" => {
                need(2)?;
                let (d, s) = (reg8(&toks, &parts[0])?, reg8(&toks, &parts[1])?);
                if d == 6 && s == 6 {
                    return Err("`mov m,m` does not exist (it is HLT)".into());
                }
                vec![0x40 | d << 3 | s]
            }
            "mvi" => {
                need(2)?;
                vec![0x06 | reg8(&toks, &parts[0])? << 3, byte(ctx, &parts[1])?]
            }
            "lxi" => {
                need(2)?;
                let w = word(ctx, &parts[1])?;
                vec![0x01 | pair(&toks, &parts[0], false)? << 4, w[0], w[1]]
            }
            "stax" | "ldax" => {
                need(1)?;
                let p = pair(&toks, &parts[0], false)?;
                if p > 1 {
                    return Err(format!("`{m}` takes B or D"));
                }
                vec![if m == "stax" { 0x02 } else { 0x0a } | p << 4]
            }
            "inx" | "dcx" | "dad" => {
                need(1)?;
                let base = match m {
                    "inx" => 0x03,
                    "dcx" => 0x0b,
                    _ => 0x09,
                };
                vec![base | pair(&toks, &parts[0], false)? << 4]
            }
            "inr" | "dcr" => {
                need(1)?;
                vec![if m == "inr" { 0x04 } else { 0x05 } | reg8(&toks, &parts[0])? << 3]
            }
            "push" | "pop" => {
                need(1)?;
                vec![if m == "push" { 0xc5 } else { 0xc1 } | pair(&toks, &parts[0], true)? << 4]
            }
            "in" | "out" => {
                need(1)?;
                vec![if m == "in" { 0xdb } else { 0xd3 }, byte(ctx, &parts[0])?]
            }
            "rst" => {
                need(1)?;
                let n = eval(ctx, &parts[0])?;
                vec![0xc7 | (fit(ctx, n, 0, 7, 3, "restart number") as u8) << 3]
            }
            _ => {
                // Jcc / Ccc / Rcc
                let (kind, cc) = m.split_at(1);
                let c = index(&COND, cc).ok_or_else(|| format!("unknown instruction `{m}`"))?;
                match kind {
                    "r" => {
                        need(0)?;
                        vec![0xc0 | c << 3]
                    }
                    "j" | "c" => {
                        need(1)?;
                        let w = word(ctx, &parts[0])?;
                        vec![if kind == "j" { 0xc2 } else { 0xc4 } | c << 3, w[0], w[1]]
                    }
                    _ => return Err(format!("unknown instruction `{m}`")),
                }
            }
        })
    }

    fn disassemble(&self, bytes: &[u8], _addr: u32, _st: &CpuState) -> Option<(usize, String)> {
        let op = *bytes.first()?;
        let dst = usize::from(op >> 3 & 7);
        let src = usize::from(op & 7);
        let rp = usize::from(op >> 4 & 3);
        let d8 = || bytes.get(1).map(|&b| hex(u32::from(b), 2));
        let a16 = || Some(hex(u32::from(*bytes.get(1)?) | u32::from(*bytes.get(2)?) << 8, 4));
        if let Some(&(m, _)) = IMPLIED.iter().find(|x| x.1 == op) {
            return Some((1, m.to_owned()));
        }
        if let Some(&(m, _)) = ADDR.iter().find(|x| x.1 == op) {
            return Some((3, format!("{m} {}", a16()?)));
        }
        Some(match op {
            0x08 | 0x10 | 0x18 | 0x20 | 0x28 | 0x30 | 0x38 | 0xcb | 0xd9 | 0xdd | 0xed | 0xfd => return None,
            0x01 | 0x11 | 0x21 | 0x31 => (3, format!("lxi {},{}", PAIR[rp], a16()?)),
            0x02 | 0x12 => (1, format!("stax {}", PAIR[rp])),
            0x0a | 0x1a => (1, format!("ldax {}", PAIR[rp])),
            0x03 | 0x13 | 0x23 | 0x33 => (1, format!("inx {}", PAIR[rp])),
            0x0b | 0x1b | 0x2b | 0x3b => (1, format!("dcx {}", PAIR[rp])),
            0x09 | 0x19 | 0x29 | 0x39 => (1, format!("dad {}", PAIR[rp])),
            _ if op < 0x40 => match op & 7 {
                4 => (1, format!("inr {}", REG[dst])),
                5 => (1, format!("dcr {}", REG[dst])),
                _ => (2, format!("mvi {},{}", REG[dst], d8()?)),
            },
            0x40..=0x7f => (1, format!("mov {},{}", REG[dst], REG[src])),
            0x80..=0xbf => (1, format!("{} {}", ALU[dst], REG[src])),
            0xc1 | 0xd1 | 0xe1 => (1, format!("pop {}", PAIR[rp])),
            0xc5 | 0xd5 | 0xe5 => (1, format!("push {}", PAIR[rp])),
            0xf1 => (1, "pop psw".into()),
            0xf5 => (1, "push psw".into()),
            0xd3 => (2, format!("out {}", d8()?)),
            0xdb => (2, format!("in {}", d8()?)),
            _ => match op & 7 {
                0 => (1, format!("r{}", COND[dst])),
                2 => (3, format!("j{} {}", COND[dst], a16()?)),
                4 => (3, format!("c{} {}", COND[dst], a16()?)),
                6 => (2, format!("{} {}", ALU_IMM[dst], d8()?)),
                _ => (1, format!("rst {dst}")),
            },
        })
    }
}
