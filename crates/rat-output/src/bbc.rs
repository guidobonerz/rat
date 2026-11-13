//! BBC Micro DFS disk images (SSD): a single-sided 40-track image with
//! the program as one file.
//!
//! Layout (Acorn *Disc Filing System User Guide*): tracks of ten
//! 256-byte sectors. The catalogue is sectors 0 and 1. Sector 0 holds the
//! first eight characters of the title and from offset 8 an eight-byte
//! entry per file (seven-character name, directory). Sector 1 holds the
//! last four title characters, the write count, the number of entries
//! times eight, the boot option (bits 4–5) with the high bits of the
//! sector count, the sector count, and from offset 8 per file: load
//! address, execution address, length (16 bits each), a byte with their
//! bits 16–17 (execution, length, load, start sector, two bits each) and
//! the start sector. Addresses in the I/O processor have bits 16–17 set.
//!
//! With `boot` the file is named `!BOOT` and the boot option is 2
//! (`*RUN !BOOT`), so Shift+BREAK starts it; otherwise `*RUN name`.
//! mimic reads the image with its DFS ROM (`machine-bbc::disc`).

use crate::{extent, flatten, Chunk};

pub const SECTOR: usize = 256;
pub const SECTORS: usize = 400;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SsdOptions {
    /// File name (up to 7 characters).
    pub name: String,
    /// Disc title (up to 12 characters).
    pub title: String,
    /// Execution address (default: the load address).
    pub exec: Option<u32>,
    pub boot: bool,
}

pub fn ssd(chunks: &[Chunk], fill: u8, o: &SsdOptions) -> Result<Vec<u8>, String> {
    let (lo, hi) = extent(chunks).ok_or("no code for the SSD image")?;
    let data = flatten(chunks, lo, hi, fill);
    if hi > 0x1_0000 {
        return Err(format!("program ends at ${hi:x}, above the BBC's 64 KB"));
    }
    let name = if o.boot { "!BOOT" } else { o.name.as_str() };
    if name.is_empty() || name.len() > 7 || !name.bytes().all(|b| b.is_ascii_graphic() && !b".:\"#*".contains(&b)) {
        return Err(format!("`{name}` is not a DFS file name (1 to 7 characters, none of . : \" # *)"));
    }
    if o.title.len() > 12 || !o.title.is_ascii() {
        return Err("the disc title has at most 12 ASCII characters".into());
    }
    let mut img = vec![0u8; SECTORS * SECTOR];
    let mut title = [b' '; 12];
    title[..o.title.len()].copy_from_slice(o.title.as_bytes());
    img[..8].copy_from_slice(&title[..8]);
    img[SECTOR..SECTOR + 4].copy_from_slice(&title[8..]);
    let mut entry = [b' '; 8];
    entry[..name.len()].copy_from_slice(name.as_bytes());
    entry[7] = b'$';
    img[8..16].copy_from_slice(&entry);
    img[SECTOR + 5] = 8;
    img[SECTOR + 6] = if o.boot { 2 << 4 } else { 0 } | (SECTORS >> 8) as u8;
    img[SECTOR + 7] = SECTORS as u8;
    let exec = o.exec.unwrap_or(lo);
    let len = data.len() as u32;
    let start = 2u32;
    let e = &mut img[SECTOR + 8..SECTOR + 16];
    e[..2].copy_from_slice(&(lo as u16).to_le_bytes());
    e[2..4].copy_from_slice(&(exec as u16).to_le_bytes());
    e[4..6].copy_from_slice(&(len as u16).to_le_bytes());
    // load and execution address in the I/O processor
    e[6] = 3 << 6 | ((len >> 16) as u8 & 3) << 4 | 3 << 2 | (start >> 8) as u8 & 3;
    e[7] = start as u8;
    let at = start as usize * SECTOR;
    img[at..at + data.len()].copy_from_slice(&data);
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue() {
        let c = [Chunk { addr: 0x1900, data: vec![0xa9, 0x41, 0x60] }];
        let img = ssd(&c, 0, &SsdOptions { name: "HELLO".into(), title: "RAT".into(), exec: Some(0x1901), boot: false }).unwrap();
        assert_eq!(img.len(), 102_400);
        assert_eq!(&img[..16], b"RAT     HELLO  $");
        assert_eq!(&img[0x105..0x110], &[8, 1, 0x90, 0x00, 0x19, 0x01, 0x19, 3, 0, 0xcc, 2]);
        assert_eq!(&img[0x200..0x203], &[0xa9, 0x41, 0x60]);
        let boot = ssd(&c, 0, &SsdOptions { boot: true, ..Default::default() }).unwrap();
        assert_eq!(&boot[8..16], b"!BOOT  $");
        assert_eq!(boot[0x106], 0x21);
    }
}
