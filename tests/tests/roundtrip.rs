//! Round trips against mimic: every opcode is disassembled by mimic, the
//! text is assembled by rat, and rat's bytes must disassemble (by mimic)
//! to the same text. Comparing text accepts equivalent encodings (the
//! documented NOP for an undocumented one, a 5-bit 6809 offset for an
//! 8-bit one, ...).

use emu_core::{Bus, Cpu, Size};
use rat_asm::{assemble, MemVfs, Options};

const ORG: u32 = 0x1000;

/// Assemble one instruction with rat (rat dialect).
fn rat(cpu: &str, text: &str, prelude: &str) -> Result<Vec<u8>, String> {
    let src = format!(".cpu {cpu}\n.org ${ORG:x}\n{prelude}\n    {text}\n");
    let mut vfs = MemVfs::new();
    vfs.add("t.rat", src.as_bytes());
    let opts = Options { main: "t.rat".into(), format: Some("bin".into()), ..Default::default() };
    let r = assemble(&opts, &vfs);
    if !r.ok {
        return Err(r.diagnostics.iter().map(|d| d.message.clone()).collect::<Vec<_>>().join("; "));
    }
    Ok(r.outputs.first().map(|o| o.data.clone()).unwrap_or_default())
}

fn norm(s: &str) -> String {
    s.to_ascii_uppercase().replace(' ', "")
}

struct Report {
    tested: usize,
    failures: Vec<String>,
}

impl Report {
    fn new() -> Report {
        Report { tested: 0, failures: Vec::new() }
    }

    fn check(&mut self, what: &str, text: &str, prelude: &str, cpu: &str, redis: impl Fn(&[u8]) -> String) {
        self.tested += 1;
        match rat(cpu, text, prelude) {
            Ok(bytes) => {
                let back = redis(&bytes);
                if norm(&back) != norm(text) {
                    self.failures.push(format!("{what}: `{text}` -> {bytes:02x?} -> `{back}`"));
                }
            }
            Err(e) => self.failures.push(format!("{what}: `{text}`: {e}")),
        }
    }

    fn finish(self, name: &str) {
        if !self.failures.is_empty() {
            let n = self.failures.len();
            let shown: Vec<_> = self.failures.into_iter().take(60).collect();
            panic!("{name}: {n} of {} round trips failed:\n{}", self.tested, shown.join("\n"));
        }
        eprintln!("{name}: {} round trips ok", self.tested);
    }
}

// ------------------------------------------------------------- 6502

#[test]
fn mos6502_all_opcodes() {
    let cpu = cpu_6502::Cpu6502::new(cpu_6502::Config::default());
    let dis = |b: &[u8]| {
        let mut bus = emu_core::testing::FlatBus::new(16);
        bus.load(ORG, b);
        cpu.disassemble(&bus, ORG).to_string()
    };
    let mut rep = Report::new();
    for op in 0..=255u8 {
        let text = dis(&[op, 0x34, 0x12]);
        rep.check(&format!("${op:02x}"), &text, "", "6502", dis);
    }
    rep.finish("6502");
}

#[test]
fn w65c02_all_opcodes() {
    let cpu = cpu_6502::Cpu6502::new(cpu_6502::Config { variant: cpu_6502::Variant::Cmos65C02, ..cpu_6502::Config::default() });
    let dis = |b: &[u8]| {
        let mut bus = emu_core::testing::FlatBus::new(16);
        bus.load(ORG, b);
        cpu.disassemble(&bus, ORG).to_string()
    };
    let mut rep = Report::new();
    for op in 0..=255u8 {
        let text = dis(&[op, 0x34, 0x12]);
        rep.check(&format!("${op:02x}"), &text, "", "65c02", dis);
    }
    rep.finish("65c02");
}

// ------------------------------------------------------------ 65816

struct Bus816(Vec<u8>);

impl cpu_65816::Bus816 for Bus816 {
    fn read(&mut self, a: u32) -> u8 {
        self.peek(a)
    }
    fn write(&mut self, _: u32, _: u8) {}
    fn idle(&mut self, _: u32) {}
    fn peek(&self, a: u32) -> u8 {
        self.0.get(a.wrapping_sub(ORG) as usize).copied().unwrap_or(0)
    }
}

#[test]
fn w65816_all_opcodes_all_widths() {
    let mut rep = Report::new();
    for (m8, x8) in [(true, true), (false, true), (true, false), (false, false)] {
        let prelude = format!("{}\n{}", if m8 { ".a8" } else { ".a16" }, if x8 { ".i8" } else { ".i16" });
        let dis = |b: &[u8]| cpu_65816::disassemble(&Bus816(b.to_vec()), ORG, m8, x8).to_string();
        for op in 0..=255u8 {
            let text = dis(&[op, 0x34, 0x12, 0x7e]);
            rep.check(&format!("${op:02x} m8={m8} x8={x8}"), &text, &prelude, "65816", dis);
        }
    }
    rep.finish("65816");
}

// ----------------------------------------------------------- SPC700

struct SpcBus(Vec<u8>);

impl cpu_spc700::SpcBus for SpcBus {
    fn read(&mut self, a: u16) -> u8 {
        self.peek(a)
    }
    fn write(&mut self, _: u16, _: u8) {}
    fn idle(&mut self) {}
    fn peek(&self, a: u16) -> u8 {
        self.0.get(usize::from(a.wrapping_sub(ORG as u16))).copied().unwrap_or(0)
    }
}

#[test]
fn spc700_all_opcodes() {
    let dis = |b: &[u8]| cpu_spc700::disassemble(&SpcBus(b.to_vec()), ORG as u16).to_string();
    let mut rep = Report::new();
    for op in 0..=255u8 {
        // mimic shows $FD as `MOV A,Y` (it is MOV Y,A; noted in mimic)
        if op == 0xfd {
            continue;
        }
        let text = dis(&[op, 0x34, 0x12]);
        rep.check(&format!("${op:02x}"), &text, "", "spc700", dis);
    }
    rep.finish("spc700");
}

// -------------------------------------------------------------- Z80

#[test]
fn z80_all_prefix_pages() {
    let cpu = cpu_z80::Z80::new();
    let dis = |b: &[u8]| {
        let mut bus = emu_core::testing::FlatBus::new(16);
        bus.load(ORG, b);
        cpu.disassemble(&bus, ORG).to_string()
    };
    let mut rep = Report::new();
    let mut cases: Vec<Vec<u8>> = Vec::new();
    for op in 0..=255u8 {
        cases.push(vec![op, 0x34, 0x12, 0x00]);
        cases.push(vec![0xcb, op]);
        cases.push(vec![0xed, op, 0x34, 0x12]);
        for pre in [0xdd, 0xfd] {
            cases.push(vec![pre, op, 0x05, 0x12]);
            cases.push(vec![pre, 0xcb, 0xfb, op]);
        }
    }
    for b in cases {
        let text = dis(&b);
        if text.contains(';') {
            continue; // prefix-only / undefined ED (mimic shows them as NOP comments)
        }
        rep.check(&format!("{b:02x?}"), &text, "", "z80", dis);
    }
    rep.finish("z80");
}

// ------------------------------------------------------------- 6809

struct Bus6809(Vec<u8>);

impl cpu_6809::M6809Bus for Bus6809 {
    fn read(&mut self, a: u16) -> u8 {
        self.peek(a)
    }
    fn write(&mut self, _: u16, _: u8) {}
    fn idle(&mut self) {}
    fn peek(&self, a: u16) -> u8 {
        self.0.get(usize::from(a.wrapping_sub(ORG as u16))).copied().unwrap_or(0)
    }
}

#[test]
fn m6809_all_pages_and_postbytes() {
    // `0,X` is assembled as the shorter `,X`
    let dis = |b: &[u8]| cpu_6809::disassemble(&Bus6809(b.to_vec()), ORG as u16).to_string().replace(" $0,", " ,").replace("[$0,", "[,");
    let mut rep = Report::new();
    let mut cases: Vec<Vec<u8>> = Vec::new();
    for op in 0..=255u8 {
        if op != 0x10 && op != 0x11 {
            cases.push(vec![op, 0x12, 0x34, 0x56]);
        }
        cases.push(vec![0x10, op, 0x12, 0x34, 0x56]);
        cases.push(vec![0x11, op, 0x12, 0x34, 0x56]);
        // every indexed postbyte with LDA
        cases.push(vec![0xa6, op, 0x12, 0x34]);
    }
    for b in cases {
        let text = dis(&b);
        if text.contains('?') || text.starts_with("HCF") {
            continue; // undefined opcodes / postbytes, halt-and-catch-fire
        }
        // store-immediate opcodes do not exist; single +/- with [] is illegal
        if ["STA", "STB", "STD", "STX", "STU", "STY", "STS"].contains(&text.as_str()) || text.contains("+]") || (text.contains("[,-") && !text.contains("[,--")) {
            continue;
        }
        rep.check(&format!("{b:02x?}"), &text, "", "6809", dis);
    }
    rep.finish("6809");
}

// ------------------------------------------------------------- 68k

struct Mem68(Vec<u8>);

impl Bus for Mem68 {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }
    fn peek(&self, addr: u32, size: Size) -> u32 {
        let b = |a: u32| u32::from(self.0.get(a.wrapping_sub(ORG) as usize).copied().unwrap_or(0));
        match size {
            Size::Byte => b(addr),
            _ => b(addr) << 8 | b(addr + 1),
        }
    }
    fn write(&mut self, _: u32, _: Size, _: u32) {}
    fn addr_bits(&self) -> u8 {
        24
    }
}

/// mimic writes unsized Bcc for the word form; rat would pick `.s`.
fn m68k_text(t: &str) -> String {
    let (m, rest) = t.split_once(' ').unwrap_or((t, ""));
    let is_branch = m.starts_with('B') && !m.contains('.') && !matches!(m, "BTST" | "BCHG" | "BCLR" | "BSET" | "BKPT") && !m.starts_with("BF");
    if is_branch {
        format!("{m}.W {rest}")
    } else {
        t.to_owned()
    }
}

/// Encodings mimic decodes but no assembler should produce: byte
/// operations on address registers, immediate forms that rat writes as
/// ORI/ANDI/... (equivalent, different opcode).
fn invalid_68k(t: &str) -> bool {
    let (m, rest) = t.split_once(' ').unwrap_or((t, ""));
    let byte_an = m.ends_with(".B") && (rest.starts_with('A') || rest.contains(",A")) && rest.as_bytes().get(1).is_some_and(u8::is_ascii_digit);
    let imm_arith = ["OR.", "AND.", "SUB.", "ADD.", "CMP.", "EOR."].iter().any(|p| m.starts_with(p)) && rest.starts_with('#');
    // memory shifts with a data register (rat reads `asr.w d1` as `#1,d1`)
    let mem_shift_dn = ["AS", "LS", "ROX", "RO"].iter().any(|p| m.starts_with(p)) && !rest.contains(',') && rest.starts_with('D');
    byte_an || imm_arith || mem_shift_dn
}

fn m68k(model020: bool) {
    let cpu = if model020 { cpu_m68k::M68000::with_model(cpu_m68k::Model::Mc68020) } else { cpu_m68k::M68000::new() };
    let dis = |b: &[u8]| cpu.disassemble(&Mem68(b.to_vec()), ORG).to_string();
    let dis_cmp = |b: &[u8]| m68k_text(&dis(b));
    let mut rep = Report::new();
    let ext: [u8; 10] = [0x10, 0x04, 0x00, 0x20, 0x00, 0x30, 0x00, 0x40, 0x00, 0x50];
    for op in (0..=0xffffu32).step_by(if model020 { 3 } else { 1 }) {
        let mut b = vec![(op >> 8) as u8, op as u8];
        b.extend(ext);
        let text = dis(&b);
        if text.starts_with("DC.W") || invalid_68k(&text) {
            continue;
        }
        let cpu_name = if model020 { "68020" } else { "68000" };
        rep.check(&format!("${op:04x}"), &m68k_text(&text), "", cpu_name, dis_cmp);
    }
    rep.finish(if model020 { "68020" } else { "68000" });
}

#[test]
fn m68000_all_opcode_words() {
    m68k(false);
}

#[test]
fn m68020_opcode_words() {
    m68k(true);
}
