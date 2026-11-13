//! The new output formats read back with mimic's media code: Spectrum
//! TAP and SNA, Apple II DSK, MSX CAS; the new platforms.

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

#[test]
fn msx_cas_is_a_bload_file() {
    let cas = build(".platform msx
.cas name=\"HELLO\", exec=main
.org $c000
 nop
main: jr main
", "cas");
    let blocks = media::msx_cas::blocks(&cas).unwrap();
    assert_eq!(blocks.len(), 2, "file header, data");
    assert_eq!(&blocks[0][..10], &[0xd0; 10]);
    assert_eq!(&blocks[0][10..16], b"HELLO ");
    assert_eq!(blocks[1], [0x00, 0xc0, 0x02, 0xc0, 0x01, 0xc0, 0x00, 0x18, 0xfe]);
}

#[test]
fn new_platforms_pick_their_cpu_and_format() {
    for (src, format, want) in [
        (".platform imsai
 mvi a,$2a
 hlt
", "bin", &[0x3e, 0x2a, 0x76][..]),
        (".platform ti99
 li r1,$1234
 rt
", "bin", &[0x02, 0x01, 0x12, 0x34, 0x04, 0x5b]),
        (".platform archimedes
 mov r0,#1
", "bin", &[0x01, 0x00, 0xa0, 0xe3]),
        (".platform bbcmaster
 stz $70
", "bin", &[0x64, 0x70]),
        (".platform mega65
.org $2100
 ldq $10
", "prg", &[0x00, 0x21, 0x42, 0x42, 0xa5, 0x10]),
        (".platform trs80
.cmd start
.org $5200
start: ret
", "cmd", &[0x01, 0x03, 0x00, 0x52, 0xc9, 0x02, 0x02, 0x00, 0x52]),
    ] {
        assert_eq!(build(src, format), want, "{src}");
    }
}

#[test]
fn acme_and_kick_name_the_new_cpus() {
    let mut vfs = MemVfs::new();
    vfs.add("a.a", "!cpu 65ce02
* = $1000
 ldz #1
 lbne $1000
");
    vfs.add("k.asm", ".cpu _45gs02
* = $1000
 ldz #1
 lbne $1000
");
    for main in ["a.a", "k.asm"] {
        let r = assemble(&Options { main: main.into(), format: Some("bin".into()), ..Default::default() }, &vfs);
        assert!(r.ok, "{}", r.render_diagnostics(false));
        assert_eq!(r.outputs[0].data, [0xa3, 0x01, 0xd3, 0xfc, 0xff], "{main}");
    }
}
