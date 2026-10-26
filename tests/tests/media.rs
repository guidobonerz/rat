//! The new output formats read back with mimic's media code: Spectrum
//! TAP and SNA, Apple II DSK.

use rat_asm::{assemble, MemVfs, Options};

fn build(src: &str, format: &str) -> Vec<u8> {
    let mut vfs = MemVfs::new();
    vfs.add("t.rat", src.as_bytes());
    let r = assemble(&Options { main: "t.rat".into(), format: Some(format.into()), ..Default::default() }, &vfs);
    assert!(r.ok, "{}", r.render_diagnostics(false));
    r.outputs[0].data.clone()
}

const SPECTRUM: &str = ".platform spectrum\n.tap name=\"HELLO\", exec=start\n.org $8000\nstart: ld a, 2\n jr start\n";

#[test]
fn spectrum_tap_loads_as_standard_blocks() {
    let tap = build(SPECTRUM, "tap");
    let tape = media::cdt::Tape::from_tap(&tap).unwrap();
    assert_eq!(tape.blocks.len(), 4, "program header, program, code header, code");
    let code_header = &tape.blocks[2].data;
    assert_eq!(code_header[0], 0, "header flag");
    assert_eq!(code_header[1], 3, "CODE");
    assert_eq!(&code_header[2..12], b"HELLO     ");
    assert_eq!(&code_header[12..18], &[4, 0, 0x00, 0x80, 0x00, 0x80]);
    let code = &tape.blocks[3].data;
    assert_eq!(&code[1..5], &[0x3e, 0x02, 0x18, 0xfc]);
    assert_eq!(code[5], code[..5].iter().fold(0, |x, &b| x ^ b));
    let basic = &tape.blocks[1].data;
    assert!(basic[1..].starts_with(&[0, 10]), "line 10");
}

#[test]
fn spectrum_sna_starts_at_the_entry_point() {
    let sna = build(".platform spectrum\n.sna exec=main, border=2\n.org $8000\n nop\nmain: jr main\n", "sna");
    let snap = machine_spectrum::snapshot::parse(&sna).unwrap();
    assert_eq!(snap.cpu.pc, 0x8001);
    assert_eq!(snap.cpu.sp, 0xfffe);
    assert_eq!(snap.border, 2);
    assert!(snap.paging.is_none(), "48K");
    let bank2 = snap.banks.iter().find(|b| b.0 == 2).unwrap();
    assert_eq!(&bank2.1[..3], &[0x00, 0x18, 0xfe]);
}

#[test]
fn apple2_dsk_encodes_as_a_disk_ii_image() {
    let dsk = build(".platform apple2e\n.dsk name=\"T\"\n.org $300\n lda ($06)\n rts\n", "dsk");
    assert_eq!(dsk.len(), media::a2disk::DSK_BYTES);
    let nib = media::a2disk::encode_dsk(&dsk, media::a2disk::SectorOrder::Dos).unwrap();
    // the VTOC comes back from the nibble stream
    let mut back = vec![0u8; media::a2disk::DSK_BYTES];
    media::a2disk::decode_dsk(&nib, media::a2disk::SectorOrder::Dos, &mut back).unwrap();
    assert_eq!(back, dsk);
    let vtoc = &dsk[17 * 16 * 256..17 * 16 * 256 + 256];
    assert_eq!((vtoc[1], vtoc[2], vtoc[0x34]), (17, 15, 35));
}
