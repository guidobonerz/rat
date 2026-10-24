//! Output file writers. Pure functions from memory images to bytes; the
//! assembler decides what goes where.

pub mod d64;
pub mod formats;
pub mod symbols;

pub use formats::*;

/// One contiguous run of bytes at an address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub addr: u32,
    pub data: Vec<u8>,
}

/// Lowest and one-past-highest address of the chunks.
pub fn extent(chunks: &[Chunk]) -> Option<(u32, u32)> {
    let lo = chunks.iter().filter(|c| !c.data.is_empty()).map(|c| c.addr).min()?;
    let hi = chunks
        .iter()
        .filter(|c| !c.data.is_empty())
        .map(|c| c.addr + c.data.len() as u32)
        .max()?;
    Some((lo, hi))
}

/// Flatten chunks into one image from `lo` to `hi`, gaps filled.
pub fn flatten(chunks: &[Chunk], lo: u32, hi: u32, fill: u8) -> Vec<u8> {
    let mut out = vec![fill; (hi.saturating_sub(lo)) as usize];
    for c in chunks {
        for (i, &b) in c.data.iter().enumerate() {
            let a = c.addr as u64 + i as u64;
            if a >= lo as u64 && a < hi as u64 {
                out[(a - lo as u64) as usize] = b;
            }
        }
    }
    out
}

/// Supported output formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// Raw bytes from the lowest to the highest address.
    Bin,
    /// Commodore program: 2-byte load address + data.
    Prg,
    /// Atari 8-bit executable.
    Xex,
    /// NES iNES ROM.
    Nes,
    /// SNES ROM image.
    Sfc,
    /// Atari 2600 cartridge.
    A26,
    /// ZX81 program file.
    P,
    /// Amstrad CPC binary with AMSDOS header.
    Amsdos,
    /// Amiga hunk executable.
    Hunk,
    /// C64 cartridge.
    Crt,
    /// Commodore 1541 disk image.
    D64,
    /// Intel HEX.
    Hex,
    /// Motorola S-records.
    Srec,
}

impl Format {
    pub const ALL: [Format; 13] = [
        Format::Bin,
        Format::Prg,
        Format::Xex,
        Format::Nes,
        Format::Sfc,
        Format::A26,
        Format::P,
        Format::Amsdos,
        Format::Hunk,
        Format::Crt,
        Format::D64,
        Format::Hex,
        Format::Srec,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Format::Bin => "bin",
            Format::Prg => "prg",
            Format::Xex => "xex",
            Format::Nes => "nes",
            Format::Sfc => "sfc",
            Format::A26 => "a26",
            Format::P => "p",
            Format::Amsdos => "amsdos",
            Format::Hunk => "hunk",
            Format::Crt => "crt",
            Format::D64 => "d64",
            Format::Hex => "hex",
            Format::Srec => "srec",
        }
    }

    /// File extension for this format.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Amsdos => "bin",
            Format::Hunk => "exe",
            Format::Srec => "s19",
            f => f.name(),
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Format::Bin => "raw binary, lowest to highest written address",
            Format::Prg => "Commodore PRG (load address + data)",
            Format::Xex => "Atari 8-bit XEX (segments, RUNAD/INITAD)",
            Format::Nes => "iNES ROM (header + PRG + CHR)",
            Format::Sfc => "SNES ROM (LoROM/HiROM/ExHiROM, checksum fixed)",
            Format::A26 => "Atari 2600 cartridge (2K/4K, F8/F6/F4 banks)",
            Format::P => "ZX81 .P file (memory from $4009)",
            Format::Amsdos => "Amstrad CPC binary with AMSDOS header",
            Format::Hunk => "Amiga hunk executable (with RELOC32)",
            Format::Crt => "C64 CRT cartridge",
            Format::D64 => "1541 disk image",
            Format::Hex => "Intel HEX",
            Format::Srec => "Motorola S-records",
        }
    }

    pub fn parse(s: &str) -> Option<Format> {
        Some(match s.to_ascii_lowercase().trim_start_matches('.') {
            "bin" | "raw" | "plain" | "rom" | "vec" | "col" | "sms" => Format::Bin,
            "prg" | "cbm" => Format::Prg,
            "xex" | "atari" | "com" => Format::Xex,
            "nes" | "ines" => Format::Nes,
            "sfc" | "smc" | "snes" => Format::Sfc,
            "a26" | "2600" => Format::A26,
            "p" | "zx81" => Format::P,
            "amsdos" | "cpc" => Format::Amsdos,
            "hunk" | "exe" | "amiga" => Format::Hunk,
            "crt" => Format::Crt,
            "d64" => Format::D64,
            "hex" | "ihex" | "intel" => Format::Hex,
            "srec" | "s19" | "s28" | "s37" | "mot" => Format::Srec,
            _ => return None,
        })
    }
}
