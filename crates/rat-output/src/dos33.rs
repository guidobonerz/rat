//! Apple II DOS 3.3 disk images (DSK, DOS order): a 140 KB image with the
//! program as one binary (`B`) file.
//!
//! Layout (*Beneath Apple DOS*, chapter 4): 35 tracks of 16 sectors of
//! 256 bytes. The VTOC sits at track 17 sector 0 (catalog pointer, DOS
//! release 3, volume number, 122 track/sector pairs per list, 35 tracks,
//! 16 sectors, 256 bytes, the free-sector bitmap: four bytes a track, bit
//! 7 of the first byte = sector 15 … bit 0 of the second = sector 0). The
//! catalog runs from sector 15 of track 17 down to sector 1, each sector
//! linked to the next and holding seven 35-byte entries (track/sector
//! list, file type, 30-character name in high-bit ASCII padded with
//! `$A0`, sector count). A track/sector list names the data sectors in
//! order from offset `$0C`. A binary file begins with its load address
//! and length (little endian). Tracks 0–2 are reserved for DOS (the image
//! holds none: boot a DOS and `BRUN` the file); mimic reads the image in
//! DOS 3.3 order (`media::a2disk`).

use crate::{extent, flatten, Chunk};

pub const TRACKS: usize = 35;
pub const SECTORS: usize = 16;
pub const SECTOR: usize = 256;
pub const VTOC_TRACK: usize = 17;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DskOptions {
    /// Catalog name (up to 30 characters).
    pub name: String,
    pub volume: u8,
    /// File type byte (`$04` binary).
    pub file_type: u8,
    pub locked: bool,
}

impl Default for DskOptions {
    fn default() -> Self {
        DskOptions { name: "PROGRAM".into(), volume: 254, file_type: 0x04, locked: false }
    }
}

struct Image {
    data: Vec<u8>,
    free: [[bool; SECTORS]; TRACKS],
}

impl Image {
    fn sector(&mut self, t: usize, s: usize) -> &mut [u8] {
        let o = (t * SECTORS + s) * SECTOR;
        &mut self.data[o..o + SECTOR]
    }

    /// Next free sector: outward from track 18, then inward from 16.
    fn alloc(&mut self) -> Option<(usize, usize)> {
        let order = (VTOC_TRACK + 1..TRACKS).chain((3..VTOC_TRACK).rev());
        for t in order {
            for s in (0..SECTORS).rev() {
                if self.free[t][s] {
                    self.free[t][s] = false;
                    return Some((t, s));
                }
            }
        }
        None
    }
}

/// A DOS 3.3 image holding `data` as a binary file loading at `load`.
pub fn dsk_binary(load: u16, data: &[u8], o: &DskOptions) -> Result<Vec<u8>, String> {
    let mut img = Image { data: vec![0; TRACKS * SECTORS * SECTOR], free: [[true; SECTORS]; TRACKS] };
    for t in 0..3 {
        img.free[t] = [false; SECTORS];
    }
    img.free[VTOC_TRACK] = [false; SECTORS];
    let mut file = load.to_le_bytes().to_vec();
    file.extend((data.len() as u16).to_le_bytes());
    file.extend(data);
    if data.len() > 0xffff {
        return Err("a DOS 3.3 binary file holds at most 64K".into());
    }
    let n = file.len().div_ceil(SECTOR);
    let lists = n.div_ceil(122).max(1);
    let mut list_secs = Vec::new();
    for _ in 0..lists {
        list_secs.push(img.alloc().ok_or("disk full")?);
    }
    let mut data_secs = Vec::new();
    for _ in 0..n {
        data_secs.push(img.alloc().ok_or("disk full")?);
    }
    for (i, &(t, s)) in list_secs.iter().enumerate() {
        let next = list_secs.get(i + 1).copied().unwrap_or((0, 0));
        let sec = img.sector(t, s);
        sec[1] = next.0 as u8;
        sec[2] = next.1 as u8;
        sec[5..7].copy_from_slice(&((i * 122) as u16).to_le_bytes());
        for (j, &(dt, ds)) in data_secs.iter().skip(i * 122).take(122).enumerate() {
            sec[0x0c + j * 2] = dt as u8;
            sec[0x0d + j * 2] = ds as u8;
        }
    }
    for (i, &(t, s)) in data_secs.iter().enumerate() {
        let part = &file[i * SECTOR..((i + 1) * SECTOR).min(file.len())];
        img.sector(t, s)[..part.len()].copy_from_slice(part);
    }
    // catalog: sectors 15..1 of track 17, linked
    for s in (1..SECTORS).rev() {
        let sec = img.sector(VTOC_TRACK, s);
        if s > 1 {
            sec[1] = VTOC_TRACK as u8;
            sec[2] = (s - 1) as u8;
        }
    }
    {
        let (lt, ls) = list_secs[0];
        let entry = &mut img.sector(VTOC_TRACK, 15)[0x0b..0x0b + 35];
        entry[0] = lt as u8;
        entry[1] = ls as u8;
        entry[2] = o.file_type | if o.locked { 0x80 } else { 0 };
        let mut name: Vec<u8> = o.name.to_ascii_uppercase().bytes().take(30).map(|b| b | 0x80).collect();
        name.resize(30, 0xa0);
        entry[3..33].copy_from_slice(&name);
        entry[33..35].copy_from_slice(&((n + lists) as u16).to_le_bytes());
    }
    // VTOC
    let free = img.free;
    let vtoc = img.sector(VTOC_TRACK, 0);
    vtoc[1] = VTOC_TRACK as u8;
    vtoc[2] = 15;
    vtoc[3] = 3;
    vtoc[6] = o.volume;
    vtoc[0x27] = 122;
    vtoc[0x30] = (VTOC_TRACK + 1) as u8;
    vtoc[0x31] = 1;
    vtoc[0x34] = TRACKS as u8;
    vtoc[0x35] = SECTORS as u8;
    vtoc[0x36..0x38].copy_from_slice(&(SECTOR as u16).to_le_bytes());
    for (t, secs) in free.iter().enumerate() {
        let mut bits = 0u16;
        for (s, &f) in secs.iter().enumerate() {
            if f {
                bits |= 1 << s;
            }
        }
        vtoc[0x38 + t * 4..0x3a + t * 4].copy_from_slice(&bits.to_be_bytes());
    }
    Ok(img.data)
}

/// The program (lowest to highest address) as a binary file on a DOS 3.3
/// disk.
pub fn dsk(chunks: &[Chunk], fill: u8, o: &DskOptions) -> Result<Vec<u8>, String> {
    let Some((lo, hi)) = extent(chunks) else { return Ok(Vec::new()) };
    if hi > 0x1_0000 {
        return Err(format!("code ends at ${hi:x}, beyond the 64K address space"));
    }
    dsk_binary(lo as u16, &flatten(chunks, lo, hi, fill), o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_and_vtoc() {
        let img = dsk_binary(0x2000, &[0x60; 300], &DskOptions { name: "Hello".into(), ..Default::default() }).unwrap();
        assert_eq!(img.len(), 143_360);
        let sec = |t: usize, s: usize| &img[(t * 16 + s) * 256..(t * 16 + s + 1) * 256];
        let vtoc = sec(17, 0);
        assert_eq!((vtoc[1], vtoc[2], vtoc[3], vtoc[6], vtoc[0x34], vtoc[0x35]), (17, 15, 3, 254, 35, 16));
        assert_eq!(&vtoc[0x38..0x3c], &[0, 0, 0, 0], "track 0 reserved");
        assert_eq!(&vtoc[0x38 + 20 * 4..0x3a + 20 * 4], &[0xff, 0xff], "track 20 free");
        let cat = sec(17, 15);
        assert_eq!((cat[1], cat[2]), (17, 14));
        let e = &cat[0x0b..0x0b + 35];
        assert_eq!(e[2], 0x04);
        assert_eq!(&e[3..8], b"\xc8\xc5\xcc\xcc\xcf");
        assert_eq!(e[8], 0xa0);
        assert_eq!(u16::from_le_bytes([e[33], e[34]]), 3, "list + 2 data sectors");
        let list = sec(e[0] as usize, e[1] as usize);
        let first = sec(list[0x0c] as usize, list[0x0d] as usize);
        assert_eq!(&first[..4], &[0x00, 0x20, 0x2c, 0x01]);
        assert_eq!(first[4], 0x60);
    }
}
