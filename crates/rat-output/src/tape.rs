//! Cassette and loader formats of the Z80 home computers: MSX `.cas`
//! (a `BLOAD` file), TRS-80 `.cas` (a Level II `SYSTEM` tape) and TRS-80
//! `/CMD` load modules.
//!
//! **MSX CAS** (`mimic/crates/media/src/msx_cas.rs`): blocks, each
//! introduced by `1F A6 DE BA CC 13 7D 74` at a file offset that is a
//! multiple of eight. A binary file is a header block of ten `$D0` bytes
//! and a six-character name, then a block with start, end (the last
//! byte) and execution address followed by the data. `BLOAD"CAS:",R`
//! loads and starts it.
//!
//! **TRS-80 SYSTEM tape** (Level II ROM, as its `SYSTEM` command reads
//! it): a leader of 255 zero bytes and the sync byte `$A5`, then `$55`
//! and a six-character name, data blocks of `$3C`, a count (0 = 256),
//! the load address, the data and a checksum (the sum of the address
//! bytes and the data), and the end mark `$78` with the entry address.
//! `SYSTEM`, the name, then `/` starts it.
//!
//! **TRS-80 /CMD** (TRSDOS load module): records of a type, a length
//! and data; `01` loads a block (length counts the two address bytes;
//! 0, 1, 2 stand for 256, 257, 258), `02` ends with the entry address.

use crate::{extent, flatten, Chunk};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TapeOptions {
    /// File name on the tape (up to 6 characters).
    pub name: String,
    /// Entry point (default: the lowest address).
    pub exec: Option<u16>,
}

pub const MSX_MARK: [u8; 8] = [0x1f, 0xa6, 0xde, 0xba, 0xcc, 0x13, 0x7d, 0x74];

fn name6(name: &str) -> Result<[u8; 6], String> {
    if name.len() > 6 || !name.is_ascii() {
        return Err(format!("tape name `{name}` is longer than 6 characters"));
    }
    let mut n = [b' '; 6];
    n[..name.len()].copy_from_slice(name.as_bytes());
    Ok(n)
}

/// One block from the lowest to the highest address, within 64 KB.
fn image(chunks: &[Chunk], fill: u8) -> Result<(u16, Vec<u8>), String> {
    let (lo, hi) = extent(chunks).ok_or("no code for the tape")?;
    if hi > 0x1_0000 {
        return Err(format!("program ends at ${hi:x}, above 64 KB"));
    }
    Ok((lo as u16, flatten(chunks, lo, hi, fill)))
}

pub fn msx_cas(chunks: &[Chunk], fill: u8, o: &TapeOptions) -> Result<Vec<u8>, String> {
    let (lo, data) = image(chunks, fill)?;
    let end = lo.wrapping_add(data.len() as u16).wrapping_sub(1);
    let mut out = MSX_MARK.to_vec();
    out.extend([0xd0; 10]);
    out.extend(name6(&o.name)?);
    out.resize(out.len().next_multiple_of(8), 0);
    out.extend(MSX_MARK);
    for w in [lo, end, o.exec.unwrap_or(lo)] {
        out.extend(w.to_le_bytes());
    }
    out.extend(data);
    Ok(out)
}

pub fn trs80_system(chunks: &[Chunk], o: &TapeOptions) -> Result<Vec<u8>, String> {
    let (lo, _) = extent(chunks).ok_or("no code for the tape")?;
    let mut out = vec![0u8; 255];
    out.extend([0xa5, 0x55]);
    out.extend(name6(&o.name.to_ascii_uppercase())?);
    for c in chunks {
        for (i, block) in c.data.chunks(256).enumerate() {
            let addr = (c.addr as usize + i * 256) as u16;
            let [al, ah] = addr.to_le_bytes();
            out.extend([0x3c, block.len() as u8, al, ah]);
            out.extend(block);
            out.push(block.iter().fold(al.wrapping_add(ah), |s, &b| s.wrapping_add(b)));
        }
    }
    out.push(0x78);
    out.extend(o.exec.unwrap_or(lo as u16).to_le_bytes());
    Ok(out)
}

pub fn trs80_cmd(chunks: &[Chunk], exec: Option<u16>) -> Result<Vec<u8>, String> {
    let (lo, _) = extent(chunks).ok_or("no code for the load module")?;
    let mut out = Vec::new();
    for c in chunks {
        for (i, block) in c.data.chunks(256).enumerate() {
            let addr = (c.addr as usize + i * 256) as u16;
            out.extend([0x01, (block.len() + 2) as u8]);
            out.extend(addr.to_le_bytes());
            out.extend(block);
        }
    }
    out.extend([0x02, 0x02]);
    out.extend(exec.unwrap_or(lo as u16).to_le_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code() -> Vec<Chunk> {
        vec![Chunk { addr: 0x5200, data: vec![0x3e, 0x2a, 0xc9] }]
    }

    #[test]
    fn msx_bload_file() {
        let cas = msx_cas(&code(), 0, &TapeOptions { name: "HI".into(), exec: Some(0x5201) }).unwrap();
        assert_eq!(&cas[..8], &MSX_MARK);
        assert_eq!(&cas[8..24], b"\xd0\xd0\xd0\xd0\xd0\xd0\xd0\xd0\xd0\xd0HI    ");
        assert_eq!(&cas[24..32], &MSX_MARK);
        assert_eq!(&cas[32..], &[0x00, 0x52, 0x02, 0x52, 0x01, 0x52, 0x3e, 0x2a, 0xc9]);
    }

    #[test]
    fn trs80_formats() {
        let t = trs80_system(&code(), &TapeOptions { name: "hi".into(), exec: None }).unwrap();
        assert_eq!(&t[255..263], b"\xa5\x55HI    ");
        assert_eq!(&t[263..], &[0x3c, 3, 0x00, 0x52, 0x3e, 0x2a, 0xc9, 0x83, 0x78, 0x00, 0x52]);
        let c = trs80_cmd(&code(), Some(0x5202)).unwrap();
        assert_eq!(c, [0x01, 5, 0x00, 0x52, 0x3e, 0x2a, 0xc9, 0x02, 0x02, 0x02, 0x52]);
    }
}
