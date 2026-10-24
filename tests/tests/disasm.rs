//! raw's own disassemblers: disassemble, assemble the text again,
//! disassemble the result; both texts must agree.

use raw_asm::{assemble, disassemble, MemVfs, Options};

const ORG: u32 = 0x1000;

fn asm(cpu: &str, text: &str, prelude: &str) -> Result<Vec<u8>, String> {
    let src = format!(".cpu {cpu}\n.org ${ORG:x}\n{prelude}\n    {text}\n");
    let mut vfs = MemVfs::new();
    vfs.add("t.rsm", src.as_bytes());
    let r = assemble(&Options { main: "t.rsm".into(), format: Some("bin".into()), ..Default::default() }, &vfs);
    if !r.ok {
        return Err(r.diagnostics.iter().map(|d| d.message.clone()).collect::<Vec<_>>().join("; "));
    }
    Ok(r.outputs.first().map(|o| o.data.clone()).unwrap_or_default())
}

fn first(cpu: &str, bytes: &[u8], wide: bool) -> Option<String> {
    let lines = disassemble(cpu, bytes, ORG, wide, wide).ok()?;
    let (_, _, t) = lines.into_iter().next()?;
    (!t.starts_with(".byte") && !t.starts_with("dc.w")).then_some(t)
}

fn check(cpu: &str, cases: impl Iterator<Item = Vec<u8>>, prelude: &str, wide: bool) {
    let mut fails = Vec::new();
    let mut n = 0;
    for b in cases {
        let Some(text) = first(cpu, &b, wide) else { continue };
        // equivalent forms raw writes differently: `0,x` -> `,x`,
        // `or.w #n,dn` -> `ori.w #n,dn`
        let (m, rest) = text.split_once(' ').unwrap_or((&text, ""));
        if text.contains(" $0,") || ["or.", "and.", "sub.", "add.", "cmp.", "eor."].iter().any(|p| m.starts_with(p)) && rest.starts_with('#') {
            continue;
        }
        n += 1;
        match asm(cpu, &text, prelude) {
            Ok(out) => {
                let back = first(cpu, &out, wide).unwrap_or_default();
                if back != text {
                    fails.push(format!("{b:02x?}: `{text}` -> {out:02x?} -> `{back}`"));
                }
            }
            Err(e) => fails.push(format!("{b:02x?}: `{text}`: {e}")),
        }
    }
    assert!(fails.is_empty(), "{cpu}: {} of {n} failed:\n{}", fails.len(), fails.into_iter().take(40).collect::<Vec<_>>().join("\n"));
}

#[test]
fn mos6502() {
    check("6502", (0..=255u8).map(|o| vec![o, 0x34, 0x12]), "", false);
}

#[test]
fn w65816() {
    check("65816", (0..=255u8).map(|o| vec![o, 0x34, 0x12, 0x7e]), ".a8\n.i8", false);
    check("65816", (0..=255u8).map(|o| vec![o, 0x34, 0x12, 0x7e]), ".a16\n.i16", true);
}

#[test]
fn spc700() {
    check("spc700", (0..=255u8).map(|o| vec![o, 0x34, 0x12]), "", false);
}

#[test]
fn z80() {
    let cases = (0..=255u8).flat_map(|o| {
        vec![vec![o, 0x34, 0x12, 0], vec![0xcb, o], vec![0xed, o, 0x34, 0x12], vec![0xdd, o, 0x05, 0x12], vec![0xfd, 0xcb, 0xfb, o]]
    });
    check("z80", cases, "", false);
}

#[test]
fn m6809() {
    let cases = (0..=255u8).flat_map(|o| vec![vec![o, 0x12, 0x34, 0x56], vec![0x10, o, 0x12, 0x34], vec![0x11, o, 0x12, 0x34], vec![0xa6, o, 0x12, 0x34]]);
    check("6809", cases, "", false);
}

#[test]
fn m68k() {
    let cases = (0..=0xffffu32).step_by(7).map(|op| vec![(op >> 8) as u8, op as u8, 0x10, 0x04, 0x00, 0x20, 0x00, 0x30, 0x00, 0x40]);
    check("68000", cases, "", false);
    let cases = (0..=0xffffu32).step_by(11).map(|op| vec![(op >> 8) as u8, op as u8, 0x10, 0x04, 0x00, 0x20, 0x00, 0x30, 0x00, 0x40]);
    check("68020", cases, "", false);
}
