//! CPU backends: instruction encoders and disassemblers.
//!
//! Every backend parses its own native operand syntax from tokens (Zilog
//! for Z80, Motorola for 6809/68k, Sony for SPC700, MOS for the 65xx), so
//! the same CPU works under every source dialect. Expressions inside an
//! operand are parsed and evaluated by the assembler through
//! [`OperandCtx`], which knows the dialect and the symbol table.
//!
//! Encoding tables are ported from mimic's cores and disassemblers
//! (`mimic/crates/cpu-*`), which are the source of truth; mimic is used as
//! a dev-dependency to check that every opcode round-trips.

use raw_core::{Endian, Tok, Token};

pub mod m6809;
pub mod m68k;
mod m68k_dis;
pub mod mos6502;
pub mod spc700;
pub mod w65816;
pub mod z80;
mod util;

pub use util::*;

/// Value of an operand expression.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Num {
    pub value: i64,
    /// False while a forward reference is still unresolved; `value` is 0.
    pub known: bool,
    /// The (unresolved) symbol is declared zero page (Kick `.zp`).
    pub zp_hint: bool,
}

impl Num {
    pub fn known(v: i64) -> Num {
        Num {
            value: v,
            known: true,
            zp_hint: false,
        }
    }
}

/// Mutable per-pass CPU state set by directives.
#[derive(Clone, Debug, PartialEq, Eq)]
#[derive(Default)]
pub struct CpuState {
    /// 65816 accumulator is 16 bit (`.a16`).
    pub m16: bool,
    /// 65816 index registers are 16 bit (`.i16`).
    pub x16: bool,
    /// Assumed direct page: 6809 `setdp` (page number), 65816 D register.
    pub dp: Option<u32>,
}


/// What the assembler offers a backend while it encodes one instruction.
pub trait OperandCtx {
    /// Operand tokens (no trailing newline).
    fn toks(&self) -> &[Token];
    /// Parse and evaluate `toks()[start..end]` as one expression.
    fn eval(&mut self, start: usize, end: usize) -> Result<Num, String>;
    /// Address of the first byte of this instruction (pseudo PC aware).
    fn pc(&self) -> u32;
    /// Smallest operand size class this site used in an earlier pass. Sizes
    /// only grow, so passes converge.
    fn min_size(&self) -> u8;
    /// Record the size class chosen at this site.
    fn grow(&mut self, size: u8);
    /// Report an error but keep the encoded size (range errors).
    fn error(&mut self, msg: String);
    fn warn(&mut self, msg: String);
    /// KickAssembler keeps the low byte of wide immediates.
    fn lenient_immediates(&self) -> bool {
        false
    }
}

/// Metadata of one CPU (or CPU variant).
#[derive(Clone, Debug, serde::Serialize)]
pub struct CpuInfo {
    pub id: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub endian: &'static str,
    pub addr_bits: u8,
}

pub trait Cpu: Send + Sync {
    fn info(&self) -> CpuInfo;
    fn endian(&self) -> Endian;
    fn has_mnemonic(&self, m: &str) -> bool;
    fn mnemonics(&self) -> Vec<String>;
    /// Encode one instruction. `Err` is a hard error (bad syntax).
    fn assemble(
        &self,
        mnemonic: &str,
        suffix: Option<&str>,
        ctx: &mut dyn OperandCtx,
        st: &CpuState,
    ) -> Result<Vec<u8>, String>;
    /// Decode the instruction at the start of `bytes` (loaded at `addr`):
    /// length and text in raw syntax, or `None` for an invalid opcode.
    fn disassemble(&self, bytes: &[u8], addr: u32, st: &CpuState) -> Option<(usize, String)>;
}

macro_rules! cpus {
    ($($id:literal => $make:expr),* $(,)?) => {
        /// All CPU ids, in display order.
        pub const CPU_IDS: &[&str] = &[$($id),*];

        fn make(id: &str) -> Option<&'static dyn Cpu> {
            match id {
                $($id => {
                    static C: std::sync::OnceLock<Box<dyn Cpu>> = std::sync::OnceLock::new();
                    Some(C.get_or_init(|| Box::new($make)).as_ref())
                })*
                _ => None,
            }
        }
    };
}

cpus! {
    "6502" => mos6502::Mos6502::new(mos6502::Variant::Nmos),
    "6502ni" => mos6502::Mos6502::new(mos6502::Variant::NoIllegals),
    "6510" => mos6502::Mos6502::new(mos6502::Variant::M6510),
    "2a03" => mos6502::Mos6502::new(mos6502::Variant::Ricoh2a03),
    "6507" => mos6502::Mos6502::new(mos6502::Variant::M6507),
    "65816" => w65816::W65816::new(),
    "spc700" => spc700::Spc700::new(),
    "z80" => z80::Z80::new(),
    "6809" => m6809::M6809::new(),
    "68000" => m68k::M68k::new(false),
    "68020" => m68k::M68k::new(true),
}

/// Map alias spellings (ACME `!cpu`, Kick `.cpu`) to an id.
fn canonical(name: &str) -> Option<&'static str> {
    let n = name.trim().trim_start_matches('_').to_ascii_lowercase().replace(['-', ' '], "");
    Some(match n.as_str() {
        "6502" | "nmos6502" | "mos6502" | "6502illegal" | "6502illegals" | "6502x" | "8502"
        | "7501" | "8501" | "sally" | "6502c" => "6502",
        "6502ni" | "6502noillegals" | "6502nmos" | "6502legal" => "6502ni",
        "6510" | "nmos6510" => "6510",
        "2a03" | "2a07" | "rp2a03" | "nes" | "ricoh" => "2a03",
        "6507" => "6507",
        "65816" | "65c816" | "w65c816" | "5a22" | "wdc65816" => "65816",
        "spc700" | "spc" | "spc700sony" => "spc700",
        "z80" | "zilogz80" | "z80undoc" => "z80",
        "6809" | "68a09" | "mc6809" | "m6809" | "68b09" => "6809",
        "68000" | "68k" | "m68k" | "mc68000" | "68010" => "68000",
        "68020" | "68ec020" | "mc68020" | "020" => "68020",
        _ => return None,
    })
}

/// Look up a CPU by id or alias.
pub fn by_name(name: &str) -> Option<&'static dyn Cpu> {
    make(canonical(name)?)
}

/// Canonical id of a CPU name or alias.
pub fn canonical_id(name: &str) -> Option<&'static str> {
    canonical(name)
}

/// All CPUs.
pub fn all() -> Vec<&'static dyn Cpu> {
    CPU_IDS.iter().filter_map(|id| make(id)).collect()
}

/// Is `t` the register `name` (case-insensitive identifier)?
pub fn is_reg(t: &Token, name: &str) -> bool {
    matches!(&t.tok, Tok::Ident(s) if s.eq_ignore_ascii_case(name))
}
