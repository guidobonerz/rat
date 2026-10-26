//! The 65C02 under every dialect: ACME `!cpu 65c02`, KickAssembler
//! `.cpu _65c02`, the rat `.platform apple2e`.

use rat_asm::{assemble, MemVfs, Options};

fn bytes(name: &str, src: &str) -> Vec<u8> {
    let mut vfs = MemVfs::new();
    vfs.add(name, src.as_bytes());
    let r = assemble(&Options { main: name.into(), format: Some("bin".into()), ..Default::default() }, &vfs);
    assert!(r.ok, "{}", r.render_diagnostics(false));
    r.outputs[0].data.clone()
}

const WANT: &[u8] = &[0x80, 0x00, 0x64, 0x10, 0xb2, 0x10, 0x1a, 0xda, 0xfa, 0x89, 0x80, 0x7c, 0x00, 0x20, 0x9c, 0x00, 0x03];

#[test]
fn acme_cpu_65c02() {
    let b = bytes("t.a", "!cpu 65c02\n* = $1000\n bra +\n+ stz $10\n lda ($10)\n inc\n phx\n plx\n bit #$80\n jmp ($2000,x)\n stz $300\n");
    assert_eq!(b, WANT);
}

#[test]
fn kick_cpu_65c02() {
    let b = bytes("t.asm", ".cpu _65c02\n* = $1000\n bra next\nnext: stz $10\n lda ($10)\n inc\n phx\n plx\n bit #$80\n jmp ($2000,x)\n stz $300\n");
    assert_eq!(b, WANT);
}

#[test]
fn nmos_rejects_cmos_opcodes() {
    let mut vfs = MemVfs::new();
    vfs.add("t.rat", ".cpu 6502\n stz $10\n lda ($10)\n");
    let r = assemble(&Options { main: "t.rat".into(), format: Some("bin".into()), ..Default::default() }, &vfs);
    assert!(!r.ok);
    let text = r.render_diagnostics(false);
    assert!(text.contains("65c02"), "{text}");
}

#[test]
fn cmos_has_no_undocumented_opcodes() {
    let mut vfs = MemVfs::new();
    vfs.add("t.rat", ".cpu 65c02\n lax $10\n");
    let r = assemble(&Options { main: "t.rat".into(), format: Some("bin".into()), ..Default::default() }, &vfs);
    assert!(!r.ok);
}
