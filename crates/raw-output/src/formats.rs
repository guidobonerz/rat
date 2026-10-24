//! Container formats.

use crate::{extent, flatten, Chunk};

/// Commodore PRG: load address + image from the lowest address.
pub fn prg(chunks: &[Chunk], fill: u8) -> Vec<u8> {
    let Some((lo, hi)) = extent(chunks) else { return Vec::new() };
    let mut v = (lo as u16).to_le_bytes().to_vec();
    v.extend(flatten(chunks, lo, hi, fill));
    v
}

pub fn bin(chunks: &[Chunk], fill: u8) -> Vec<u8> {
    let Some((lo, hi)) = extent(chunks) else { return Vec::new() };
    flatten(chunks, lo, hi, fill)
}

/// Merge touching chunks so each becomes one segment.
pub fn coalesce(chunks: &[Chunk]) -> Vec<Chunk> {
    let mut v: Vec<Chunk> = chunks.iter().filter(|c| !c.data.is_empty()).cloned().collect();
    v.sort_by_key(|c| c.addr);
    let mut out: Vec<Chunk> = Vec::new();
    for c in v {
        if let Some(last) = out.last_mut() {
            if last.addr + last.data.len() as u32 == c.addr {
                last.data.extend(c.data);
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// Atari XEX: `$FFFF`, then per segment start/end (inclusive) and data;
/// INITAD segments after their segment, RUNAD at the end.
pub fn xex(chunks: &[Chunk], inits: &[(u32, u16)], run: Option<u16>) -> Vec<u8> {
    let mut v = vec![0xff, 0xff];
    let seg = |v: &mut Vec<u8>, addr: u32, data: &[u8]| {
        v.extend((addr as u16).to_le_bytes());
        v.extend(((addr + data.len() as u32 - 1) as u16).to_le_bytes());
        v.extend(data);
    };
    for c in coalesce(chunks) {
        seg(&mut v, c.addr, &c.data);
        let end = c.addr + c.data.len() as u32;
        for &(after, init) in inits {
            if after >= c.addr && after < end.max(c.addr + 1) {
                seg(&mut v, 0x2e2, &init.to_le_bytes());
            }
        }
    }
    if let Some(r) = run {
        seg(&mut v, 0x2e0, &r.to_le_bytes());
    }
    v
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InesHeader {
    pub mapper: u16,
    /// PRG size in 16K units.
    pub prg_banks: u8,
    /// CHR size in 8K units (0 = CHR RAM).
    pub chr_banks: u8,
    pub vertical_mirroring: bool,
    pub battery: bool,
    pub four_screen: bool,
    pub nes2: bool,
}

impl Default for InesHeader {
    fn default() -> Self {
        InesHeader {
            mapper: 0,
            prg_banks: 2,
            chr_banks: 1,
            vertical_mirroring: false,
            battery: false,
            four_screen: false,
            nes2: false,
        }
    }
}

pub fn ines(h: &InesHeader, prg: &[u8], chr: &[u8]) -> Vec<u8> {
    let mut v = b"NES\x1a".to_vec();
    v.push(h.prg_banks);
    v.push(h.chr_banks);
    let f6 = (h.mapper as u8 & 0x0f) << 4
        | u8::from(h.vertical_mirroring)
        | u8::from(h.battery) << 1
        | u8::from(h.four_screen) << 3;
    let f7 = (h.mapper as u8 & 0xf0) | if h.nes2 { 0x08 } else { 0 };
    v.extend([f6, f7]);
    if h.nes2 {
        v.push((h.mapper >> 8) as u8 & 0x0f);
        v.extend([0; 7]);
    } else {
        v.extend([0; 8]);
    }
    let mut p = prg.to_vec();
    p.resize(h.prg_banks as usize * 0x4000, 0xff);
    v.extend(p);
    let mut c = chr.to_vec();
    c.resize(h.chr_banks as usize * 0x2000, 0);
    v.extend(c);
    v
}

/// SNES memory map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnesMap {
    LoRom,
    HiRom,
    ExHiRom,
}

impl SnesMap {
    pub fn parse(s: &str) -> Option<SnesMap> {
        match s.to_ascii_lowercase().as_str() {
            "lorom" | "lo" => Some(SnesMap::LoRom),
            "hirom" | "hi" => Some(SnesMap::HiRom),
            "exhirom" | "exhi" => Some(SnesMap::ExHiRom),
            _ => None,
        }
    }

    /// ROM file offset of a CPU address, if it maps to ROM.
    pub fn offset(self, addr: u32) -> Option<u32> {
        let bank = addr >> 16;
        let a = addr & 0xffff;
        match self {
            SnesMap::LoRom => {
                if a < 0x8000 && !(0x40..0x70).contains(&(bank & 0x7f)) {
                    return None;
                }
                Some((bank & 0x7f) * 0x8000 + (a & 0x7fff))
            }
            SnesMap::HiRom => {
                if bank & 0x7f >= 0x40 || a >= 0x8000 {
                    Some((bank & 0x3f) * 0x10000 + a)
                } else {
                    None
                }
            }
            SnesMap::ExHiRom => {
                let hi = if bank >= 0x80 { 0 } else { 0x40_0000 };
                if bank & 0x7f >= 0x40 || a >= 0x8000 {
                    Some(hi + (bank & 0x3f) * 0x10000 + a)
                } else {
                    None
                }
            }
        }
    }

    /// File offset of the internal header.
    pub fn header_offset(self) -> u32 {
        match self {
            SnesMap::LoRom => 0x7fc0,
            SnesMap::HiRom => 0xffc0,
            SnesMap::ExHiRom => 0x40ffc0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnesHeader {
    pub title: String,
    pub map: SnesMap,
    pub fast: bool,
    pub cart_type: u8,
    pub sram_kb: u32,
    pub region: u8,
    pub license: u8,
    pub version: u8,
}

/// SNES ROM: chunks mapped to file offsets, padded to a power of two
/// (at least 32K), internal header written, checksum computed.
pub fn sfc(chunks: &[Chunk], header: Option<&SnesHeader>, map: SnesMap) -> Result<Vec<u8>, String> {
    let mut rom: Vec<u8> = Vec::new();
    for c in chunks {
        for (i, &b) in c.data.iter().enumerate() {
            let addr = c.addr + i as u32;
            let off = map
                .offset(addr)
                .ok_or_else(|| format!("address ${addr:06x} is not ROM in this memory map"))? as usize;
            if off >= rom.len() {
                rom.resize(off + 1, 0);
            }
            rom[off] = b;
        }
    }
    let min = (map.header_offset() as usize + 0x40).max(0x8000);
    let size = rom.len().max(min).next_power_of_two();
    rom.resize(size, 0);
    if let Some(h) = header {
        let o = map.header_offset() as usize;
        let mut title = h.title.as_bytes().to_vec();
        title.resize(21, b' ');
        rom[o..o + 21].copy_from_slice(&title);
        let mode = match map {
            SnesMap::LoRom => 0x20,
            SnesMap::HiRom => 0x21,
            SnesMap::ExHiRom => 0x25,
        } | if h.fast { 0x10 } else { 0 };
        rom[o + 0x15] = mode;
        rom[o + 0x16] = h.cart_type;
        rom[o + 0x17] = (size.trailing_zeros() as u8).saturating_sub(10);
        rom[o + 0x18] = if h.sram_kb == 0 { 0 } else { (h.sram_kb.next_power_of_two().trailing_zeros()) as u8 };
        rom[o + 0x19] = h.region;
        rom[o + 0x1a] = h.license;
        rom[o + 0x1b] = h.version;
        // checksum over the image with complement/checksum = $FFFF/$0000
        rom[o + 0x1c..o + 0x20].copy_from_slice(&[0xff, 0xff, 0, 0]);
        let sum = rom.iter().fold(0u16, |s, &b| s.wrapping_add(u16::from(b)));
        rom[o + 0x1c..o + 0x1e].copy_from_slice(&(!sum).to_le_bytes());
        rom[o + 0x1e..o + 0x20].copy_from_slice(&sum.to_le_bytes());
    }
    Ok(rom)
}

/// Atari 2600: plain image of `banks` (each 4K) or one 2K/4K image.
pub fn a26(banks: &[Vec<u8>]) -> Vec<u8> {
    banks.concat()
}

/// ZX81 `.P`: memory from `$4009` (system variables) to the end.
pub fn zx81_p(chunks: &[Chunk]) -> Result<Vec<u8>, String> {
    let Some((lo, hi)) = extent(chunks) else { return Ok(Vec::new()) };
    if lo > 0x4009 {
        return Err(format!("ZX81 .P files start at $4009 (system variables); first byte is at ${lo:04x}"));
    }
    Ok(flatten(chunks, 0x4009, hi, 0))
}

/// Amstrad CPC AMSDOS header (128 bytes) + data.
pub fn amsdos(name: &str, load: u16, exec: u16, data: &[u8]) -> Vec<u8> {
    let mut h = [0u8; 128];
    let (base, ext) = name.rsplit_once('.').unwrap_or((name, "BIN"));
    let mut fname = [b' '; 11];
    for (i, b) in base.to_ascii_uppercase().bytes().take(8).enumerate() {
        fname[i] = b;
    }
    for (i, b) in ext.to_ascii_uppercase().bytes().take(3).enumerate() {
        fname[8 + i] = b;
    }
    h[1..12].copy_from_slice(&fname);
    h[18] = 2; // binary
    h[21..23].copy_from_slice(&load.to_le_bytes());
    h[24..26].copy_from_slice(&(data.len() as u16).to_le_bytes());
    h[26..28].copy_from_slice(&exec.to_le_bytes());
    let len = data.len() as u32;
    h[64..67].copy_from_slice(&len.to_le_bytes()[..3]);
    let sum: u16 = h[..67].iter().map(|&b| u16::from(b)).sum();
    h[67..69].copy_from_slice(&sum.to_le_bytes());
    let mut v = h.to_vec();
    v.extend(data);
    v
}

/// Kind of an Amiga hunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HunkKind {
    Code,
    Data,
    Bss,
}

#[derive(Clone, Debug)]
pub struct Hunk {
    pub kind: HunkKind,
    pub data: Vec<u8>,
    /// BSS size in bytes (for `Bss`).
    pub bss_size: u32,
    /// (target hunk, offsets of longwords to relocate).
    pub relocs: Vec<(u32, Vec<u32>)>,
}

/// Amiga hunk executable (HUNK_HEADER, CODE/DATA/BSS, RELOC32, END).
pub fn hunk(hunks: &[Hunk]) -> Vec<u8> {
    let mut v: Vec<u8> = Vec::new();
    let l = |v: &mut Vec<u8>, x: u32| v.extend(x.to_be_bytes());
    l(&mut v, 0x3f3);
    l(&mut v, 0);
    l(&mut v, hunks.len() as u32);
    l(&mut v, 0);
    l(&mut v, hunks.len().saturating_sub(1) as u32);
    let longs = |n: usize| n.div_ceil(4) as u32;
    for h in hunks {
        let n = match h.kind {
            HunkKind::Bss => longs(h.bss_size as usize),
            _ => longs(h.data.len()),
        };
        l(&mut v, n);
    }
    for h in hunks {
        match h.kind {
            HunkKind::Bss => {
                l(&mut v, 0x3eb);
                l(&mut v, longs(h.bss_size as usize));
            }
            k => {
                l(&mut v, if k == HunkKind::Code { 0x3e9 } else { 0x3ea });
                l(&mut v, longs(h.data.len()));
                v.extend(&h.data);
                while !v.len().is_multiple_of(4) {
                    v.push(0);
                }
            }
        }
        let relocs: Vec<_> = h.relocs.iter().filter(|r| !r.1.is_empty()).collect();
        if !relocs.is_empty() {
            l(&mut v, 0x3ec);
            for (target, offs) in relocs {
                l(&mut v, offs.len() as u32);
                l(&mut v, *target);
                for &o in offs {
                    l(&mut v, o);
                }
            }
            l(&mut v, 0);
        }
        l(&mut v, 0x3f2);
    }
    v
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrtInfo {
    pub hw_type: u16,
    pub exrom: u8,
    pub game: u8,
    pub name: String,
}

/// C64 CRT file with one CHIP packet per (bank, address, data).
pub fn crt(info: &CrtInfo, chips: &[(u16, u16, Vec<u8>)]) -> Vec<u8> {
    let mut v = b"C64 CARTRIDGE   ".to_vec();
    v.extend(0x40u32.to_be_bytes());
    v.extend([1, 0]);
    v.extend(info.hw_type.to_be_bytes());
    v.push(info.exrom);
    v.push(info.game);
    v.extend([0; 6]);
    let mut name = info.name.as_bytes().to_vec();
    name.resize(32, 0);
    v.extend(name);
    for (bank, addr, data) in chips {
        v.extend(b"CHIP");
        v.extend((0x10 + data.len() as u32).to_be_bytes());
        v.extend(0u16.to_be_bytes());
        v.extend(bank.to_be_bytes());
        v.extend(addr.to_be_bytes());
        v.extend((data.len() as u16).to_be_bytes());
        v.extend(data);
    }
    v
}

/// Intel HEX (record types 00, 04, 01).
pub fn intel_hex(chunks: &[Chunk]) -> Vec<u8> {
    let mut s = String::new();
    let mut upper = u32::MAX;
    let rec = |s: &mut String, kind: u8, addr: u16, data: &[u8]| {
        let mut sum = data.len() as u8;
        sum = sum.wrapping_add((addr >> 8) as u8).wrapping_add(addr as u8).wrapping_add(kind);
        s.push_str(&format!(":{:02X}{addr:04X}{kind:02X}", data.len()));
        for &b in data {
            s.push_str(&format!("{b:02X}"));
            sum = sum.wrapping_add(b);
        }
        s.push_str(&format!("{:02X}\n", sum.wrapping_neg()));
    };
    for c in coalesce(chunks) {
        for (i, part) in c.data.chunks(16).enumerate() {
            let a = c.addr + i as u32 * 16;
            if a >> 16 != upper {
                upper = a >> 16;
                rec(&mut s, 4, 0, &(upper as u16).to_be_bytes());
            }
            rec(&mut s, 0, a as u16, part);
        }
    }
    rec(&mut s, 1, 0, &[]);
    s.into_bytes()
}

/// Motorola S-records (S0 header, S1/S2/S3 by address size, end record).
pub fn srec(chunks: &[Chunk], name: &str) -> Vec<u8> {
    let max = extent(chunks).map_or(0, |e| e.1);
    let (kind, alen) = if max <= 0x1_0000 {
        (1, 2)
    } else if max <= 0x100_0000 {
        (2, 3)
    } else {
        (3, 4)
    };
    let mut s = String::new();
    let rec = |s: &mut String, t: u8, addr: u32, alen: usize, data: &[u8]| {
        let count = (alen + data.len() + 1) as u8;
        let mut sum = count;
        s.push_str(&format!("S{t}{count:02X}"));
        for i in (0..alen).rev() {
            let b = (addr >> (8 * i)) as u8;
            sum = sum.wrapping_add(b);
            s.push_str(&format!("{b:02X}"));
        }
        for &b in data {
            sum = sum.wrapping_add(b);
            s.push_str(&format!("{b:02X}"));
        }
        s.push_str(&format!("{:02X}\n", !sum));
    };
    rec(&mut s, 0, 0, 2, name.as_bytes());
    for c in coalesce(chunks) {
        for (i, part) in c.data.chunks(32).enumerate() {
            rec(&mut s, kind, c.addr + i as u32 * 32, alen, part);
        }
    }
    rec(&mut s, 10 - kind, 0, alen, &[]);
    s.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prg_and_bin() {
        let c = vec![Chunk { addr: 0x0801, data: vec![1, 2] }, Chunk { addr: 0x0805, data: vec![3] }];
        assert_eq!(prg(&c, 0), vec![0x01, 0x08, 1, 2, 0, 0, 3]);
        assert_eq!(bin(&c, 0xff), vec![1, 2, 0xff, 0xff, 3]);
    }

    #[test]
    fn xex_segments() {
        let c = vec![Chunk { addr: 0x2000, data: vec![0xa9, 0] }];
        assert_eq!(xex(&c, &[], Some(0x2000)), vec![0xff, 0xff, 0, 0x20, 1, 0x20, 0xa9, 0, 0xe0, 2, 0xe1, 2, 0, 0x20]);
    }

    #[test]
    fn intel_hex_checksum() {
        let c = vec![Chunk { addr: 0, data: vec![0x01, 0x02] }];
        let s = String::from_utf8(intel_hex(&c)).unwrap();
        assert!(s.contains(":0200000001 02FB".replace(' ', "").as_str()), "{s}");
        assert!(s.ends_with(":00000001FF\n"));
    }

    #[test]
    fn lorom_mapping() {
        assert_eq!(SnesMap::LoRom.offset(0x008000), Some(0));
        assert_eq!(SnesMap::LoRom.offset(0x01ffff), Some(0xffff));
        assert_eq!(SnesMap::LoRom.offset(0x000000), None);
        assert_eq!(SnesMap::HiRom.offset(0xc00000), Some(0));
    }
}
