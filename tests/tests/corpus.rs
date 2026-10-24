//! Corpus: every source in `tests/corpus` assembles to its golden output
//! (`tests/corpus/expected`), and converting it to each dialect and
//! assembling again gives the same bytes.
//!
//! Run with `RAT_BLESS=1` to (re)write the golden files after checking
//! the output by hand.

use std::path::Path;

use rat_asm::{assemble, convert, FsVfs, MemVfs, Options, Vfs};
use rat_core::Dialect;

const DIR: &str = "corpus";

fn sources() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(DIR)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| Dialect::from_extension(n).is_some())
        .collect();
    v.sort();
    v
}

fn main_output(opts: &Options, vfs: &dyn Vfs) -> Result<Vec<u8>, String> {
    let r = assemble(opts, vfs);
    if !r.ok {
        return Err(r.render_diagnostics(false));
    }
    r.outputs.first().map(|o| o.data.clone()).ok_or_else(|| "no output".into())
}

#[test]
fn corpus_matches_golden_files() {
    let bless = std::env::var_os("RAT_BLESS").is_some();
    let mut failures = Vec::new();
    for name in sources() {
        let opts = Options { main: format!("{DIR}/{name}"), ..Default::default() };
        let data = match main_output(&opts, &FsVfs) {
            Ok(d) => d,
            Err(e) => {
                failures.push(format!("{name}: {e}"));
                continue;
            }
        };
        let golden = Path::new(DIR).join("expected").join(format!("{name}.out"));
        if bless {
            std::fs::write(&golden, &data).unwrap();
            continue;
        }
        match std::fs::read(&golden) {
            Ok(g) if g == data => {}
            Ok(g) => failures.push(format!("{name}: output differs from {} ({} vs {} bytes)", golden.display(), data.len(), g.len())),
            Err(_) => failures.push(format!("{name}: missing golden file {} (run with RAT_BLESS=1)", golden.display())),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Sources whose features only exist in some dialects.
fn convertible(name: &str, to: Dialect) -> bool {
    // KickAssembler script values (lists, structs, functions, segments)
    // have no ACME counterpart
    !(name.starts_with("kick_") && to == Dialect::Acme)
}

#[test]
fn corpus_converts_to_every_dialect() {
    let mut failures = Vec::new();
    for name in sources() {
        let path = format!("{DIR}/{name}");
        let text = std::fs::read_to_string(&path).unwrap();
        let bin = Options { main: path.clone(), format: Some("bin".into()), ..Default::default() };
        let want = match main_output(&bin, &FsVfs) {
            Ok(d) => d,
            Err(e) => {
                failures.push(format!("{name}: {e}"));
                continue;
            }
        };
        for to in Dialect::ALL {
            if !convertible(&name, to) {
                continue;
            }
            let conv = match convert(&path, &text, None, to) {
                Ok(c) => c,
                Err(e) => {
                    failures.push(format!("{name} -> {}: convert failed: {e}", to.name()));
                    continue;
                }
            };
            // the converted file sits next to the original (for includes)
            let ext = match to {
                Dialect::Acme => "a",
                Dialect::Kick => "asm",
                Dialect::Rat => "rat",
            };
            let mut vfs = MemVfs::new();
            for f in std::fs::read_dir(DIR).unwrap().filter_map(|e| e.ok()) {
                if f.path().is_file() {
                    vfs.add(&format!("{DIR}/{}", f.file_name().to_string_lossy()), std::fs::read(f.path()).unwrap());
                }
            }
            let conv_path = format!("{DIR}/converted.{ext}");
            vfs.add(&conv_path, conv.text.as_bytes());
            let opts = Options { main: conv_path, format: Some("bin".into()), ..Default::default() };
            match main_output(&opts, &vfs) {
                Ok(d) if d == want => {}
                Ok(_) => failures.push(format!("{name} -> {}: different bytes\n{}", to.name(), conv.text)),
                Err(e) => failures.push(format!("{name} -> {}: {e}\n{}", to.name(), conv.text)),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn platform_features_are_gated() {
    let mut vfs = MemVfs::new();
    vfs.add("a.rat", ".platform nes\n.var s = LoadSid(\"x.sid\")\n.basicupstart start\nstart: rts\n");
    let r = assemble(&Options { main: "a.rat".into(), ..Default::default() }, &vfs);
    let msgs: Vec<&str> = r.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert!(msgs.iter().any(|m| m.contains("LoadSid") && m.contains(".platform c64")), "{msgs:?}");
    assert!(msgs.iter().any(|m| m.contains("basicupstart") && m.contains("current platform: nes")), "{msgs:?}");

    let mut vfs = MemVfs::new();
    vfs.add("b.rat", ".platform nes\nrts\n");
    let r = assemble(&Options { main: "b.rat".into(), format: Some("prg".into()), ..Default::default() }, &vfs);
    assert!(r.diagnostics.iter().any(|d| d.message.contains("not available on platform nes")));
}

#[test]
fn dialect_is_detected_from_directive_flag_and_extension() {
    let mut vfs = MemVfs::new();
    // `.syntax` beats the extension
    vfs.add("x.a", ".syntax kick\nlabel: .byte 1\n");
    let r = assemble(&Options { main: "x.a".into(), format: Some("bin".into()), ..Default::default() }, &vfs);
    assert!(r.ok, "{}", r.render_diagnostics(false));
    // ACME by extension: labels without colon, `!byte`
    vfs.add("y.a", "* = $1000\nlabel !byte 2\n");
    let r = assemble(&Options { main: "y.a".into(), format: Some("bin".into()), ..Default::default() }, &vfs);
    assert_eq!(r.outputs[0].data, vec![2]);
}

#[test]
fn errors_point_at_the_source() {
    let mut vfs = MemVfs::new();
    vfs.add("e.rat", "    nop\n    lda #$1000\n    bne far\n.org $2000\nfar: rts\n");
    let r = assemble(&Options { main: "e.rat".into(), ..Default::default() }, &vfs);
    assert!(!r.ok);
    let text = r.render_diagnostics(false);
    assert!(text.contains("e.rat:2:"), "{text}");
    assert!(text.contains("immediate value out of range"), "{text}");
    assert!(text.contains("branch out of range"), "{text}");
}
