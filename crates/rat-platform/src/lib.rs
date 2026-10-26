//! Target platforms: default CPU, the output formats a platform allows,
//! its text encoding, platform directives and platform functions, and a
//! prelude of platform macros (rat dialect).
//!
//! Platform-specific features stay with their platform: `BasicUpstart`,
//! `LoadSid`, `LoadPicture` and `.disk` exist only on the Commodore
//! machines; `bin`, `hex` and `srec` output work everywhere, every other
//! format only on its platform.

use rat_output::Format;

pub mod cbm;

/// A platform directive, for documentation and `GET /platforms`.
#[derive(Clone, Debug, serde::Serialize)]
pub struct DirSpec {
    pub name: &'static str,
    pub args: &'static str,
    pub description: &'static str,
}

#[derive(Debug, serde::Serialize)]
pub struct Platform {
    pub id: &'static str,
    pub aliases: &'static [&'static str],
    pub name: &'static str,
    pub cpu: &'static str,
    /// Allowed output formats; the first is the default. `bin`, `hex`,
    /// `srec` are always allowed in addition.
    pub formats: &'static [Format],
    /// Origin used when the source sets none.
    pub default_org: Option<u32>,
    /// Start of BASIC memory (Commodore).
    pub basic_start: Option<u16>,
    /// Default `.text` encoding in the rat dialect.
    pub encoding: &'static str,
    pub directives: &'static [DirSpec],
    /// Platform functions callable from expressions.
    pub functions: &'static [&'static str],
    #[serde(skip)]
    pub prelude: Option<&'static str>,
}

/// Formats every platform can write.
pub const GENERIC_FORMATS: [Format; 3] = [Format::Bin, Format::Hex, Format::Srec];

impl Platform {
    pub fn default_format(&self) -> Format {
        self.formats.first().copied().unwrap_or(Format::Bin)
    }

    pub fn allows(&self, f: Format) -> bool {
        GENERIC_FORMATS.contains(&f) || self.formats.contains(&f)
    }

    /// All allowed formats (platform ones first).
    pub fn all_formats(&self) -> Vec<Format> {
        let mut v = self.formats.to_vec();
        for f in GENERIC_FORMATS {
            if !v.contains(&f) {
                v.push(f);
            }
        }
        v
    }

    pub fn has_directive(&self, name: &str) -> bool {
        self.directives.iter().any(|d| d.name.eq_ignore_ascii_case(name))
    }

    pub fn has_function(&self, name: &str) -> bool {
        self.functions.iter().any(|f| f.eq_ignore_ascii_case(name))
    }
}

const CBM_DIRS: &[DirSpec] = &[
    DirSpec { name: "basicupstart", args: "address", description: "BASIC line `10 SYS address` at the start of BASIC, code continues after it (KickAssembler BasicUpstart2)" },
    DirSpec { name: "disk", args: "[filename=, name=, id=] { [name=, type=, segments=] ... }", description: "build a D64 disk image (KickAssembler .disk)" },
];
const C64_DIRS: &[DirSpec] = &[
    DirSpec { name: "basicupstart", args: "address", description: "BASIC line `10 SYS address` at $0801 (KickAssembler BasicUpstart2)" },
    DirSpec { name: "crt", args: "type=, name=, exrom=, game=", description: "CRT cartridge header for the crt output format" },
    DirSpec { name: "disk", args: "[filename=, name=, id=] { [name=, type=, segments=] ... }", description: "build a D64 disk image (KickAssembler .disk)" },
];
const CBM_FUNCS: &[&str] = &["LoadSid", "LoadPicture", "BasicUpstart", "BasicUpstart2"];
const ATARI8_DIRS: &[DirSpec] = &[
    DirSpec { name: "run", args: "address", description: "RUNAD segment: start address after loading" },
    DirSpec { name: "init", args: "address", description: "INITAD segment after the current segment" },
];
const A26_DIRS: &[DirSpec] = &[DirSpec { name: "bank", args: "n [, address]", description: "start 4K bank n (F8/F6/F4 bank switching), default address $F000" }];
const NES_DIRS: &[DirSpec] = &[
    DirSpec { name: "ines", args: "mapper=, prg=, chr=, mirroring=h|v, battery=", description: "iNES header; prg in 16K units, chr in 8K units" },
    DirSpec { name: "chr", args: "{ ... }", description: "assemble the block into CHR ROM (address 0..)" },
    DirSpec { name: "bank", args: "n [, address]", description: "start 16K PRG bank n" },
];
const SNES_DIRS: &[DirSpec] = &[
    DirSpec { name: "header", args: "title=, map=lorom|hirom|exhirom, fast=, type=, sram=, region=, version=", description: "internal ROM header; checksum is computed" },
    DirSpec { name: "spc", args: "[address] { ... }", description: "SPC700 code assembled for `address` (default $0200), stored at the current ROM position" },
];
const CPC_DIRS: &[DirSpec] = &[DirSpec { name: "amsdos", args: "name=, exec=", description: "AMSDOS header file name and entry point" }];
const AMIGA_DIRS: &[DirSpec] = &[DirSpec { name: "section", args: "name, code|data|bss", description: "start a hunk; absolute longwords are relocated (RELOC32)" }];
const VECTREX_DIRS: &[DirSpec] = &[DirSpec { name: "header", args: "title [, music]", description: "cartridge header (`g GCE` copyright, music, title)" }];
const COLECO_DIRS: &[DirSpec] = &[DirSpec { name: "header", args: "start [, title]", description: "cartridge header at $8000 ($AA55, pointers, start address)" }];
const SPECTRUM_DIRS: &[DirSpec] = &[
    DirSpec { name: "tap", args: "name=, exec=, loader=", description: "TAP output: name of the CODE block; with exec= (or loader=true) a BASIC loader `CLEAR/LOAD \"\" CODE/RANDOMIZE USR exec` goes first" },
    DirSpec { name: "sna", args: "exec=, sp=, border=, im=, ei=", description: "SNA output: entry point (default: lowest address), stack, border colour, interrupt mode" },
];
const APPLE2_DIRS: &[DirSpec] = &[DirSpec { name: "dsk", args: "name=, volume=, type=, locked=", description: "DSK output: catalog name, volume number (254), DOS file type byte ($04 binary)" }];

const NONE: &[DirSpec] = &[];

macro_rules! platforms {
    ($($p:expr),* $(,)?) => {
        pub static PLATFORMS: &[Platform] = &[$($p),*];
    };
}

platforms![
    Platform {
        id: "generic",
        aliases: &["none", "raw"],
        name: "Generic (no platform)",
        cpu: "6502",
        formats: &[Format::Bin],
        default_org: None,
        basic_start: None,
        encoding: "ascii",
        directives: NONE,
        functions: &[],
        prelude: None,
    },
    Platform {
        id: "c64",
        aliases: &["commodore64", "c-64"],
        name: "Commodore 64",
        cpu: "6510",
        formats: &[Format::Prg, Format::D64, Format::Crt],
        default_org: Some(0x0801),
        basic_start: Some(0x0801),
        encoding: "petscii_mixed",
        directives: C64_DIRS,
        functions: CBM_FUNCS,
        prelude: Some(cbm::PRELUDE_C64),
    },
    Platform {
        id: "c128",
        aliases: &["commodore128"],
        name: "Commodore 128 (8502 side)",
        cpu: "6502",
        formats: &[Format::Prg, Format::D64],
        default_org: Some(0x1c01),
        basic_start: Some(0x1c01),
        encoding: "petscii_mixed",
        directives: CBM_DIRS,
        functions: CBM_FUNCS,
        prelude: Some(cbm::PRELUDE_C128),
    },
    Platform {
        id: "vic20",
        aliases: &["vic-20", "vc20"],
        name: "Commodore VIC-20 (unexpanded)",
        cpu: "6502",
        formats: &[Format::Prg, Format::D64],
        default_org: Some(0x1001),
        basic_start: Some(0x1001),
        encoding: "petscii_mixed",
        directives: CBM_DIRS,
        functions: CBM_FUNCS,
        prelude: Some(cbm::PRELUDE_VIC20),
    },
    Platform {
        id: "pet",
        aliases: &["cbm", "pet2001"],
        name: "Commodore PET",
        cpu: "6502",
        formats: &[Format::Prg, Format::D64],
        default_org: Some(0x0401),
        basic_start: Some(0x0401),
        encoding: "petscii_mixed",
        directives: CBM_DIRS,
        functions: CBM_FUNCS,
        prelude: Some(cbm::PRELUDE_PET),
    },
    Platform {
        id: "plus4",
        aliases: &["c16", "ted", "264", "c116"],
        name: "Commodore 16 / Plus/4",
        cpu: "6502",
        formats: &[Format::Prg, Format::D64],
        default_org: Some(0x1001),
        basic_start: Some(0x1001),
        encoding: "petscii_mixed",
        directives: CBM_DIRS,
        functions: CBM_FUNCS,
        prelude: Some(cbm::PRELUDE_PLUS4),
    },
    Platform {
        id: "atari8",
        aliases: &["atari800", "atarixl", "atarixe", "a8", "atari"],
        name: "Atari 400/800/XL/XE",
        cpu: "6502",
        formats: &[Format::Xex],
        default_org: Some(0x2000),
        basic_start: None,
        encoding: "ascii",
        directives: ATARI8_DIRS,
        functions: &[],
        prelude: None,
    },
    Platform {
        id: "atari2600",
        aliases: &["vcs", "a2600", "2600"],
        name: "Atari 2600 (VCS)",
        cpu: "6507",
        formats: &[Format::A26],
        default_org: Some(0xf000),
        basic_start: None,
        encoding: "ascii",
        directives: A26_DIRS,
        functions: &[],
        prelude: None,
    },
    Platform {
        id: "nes",
        aliases: &["famicom"],
        name: "Nintendo Entertainment System",
        cpu: "2a03",
        formats: &[Format::Nes],
        default_org: Some(0x8000),
        basic_start: None,
        encoding: "ascii",
        directives: NES_DIRS,
        functions: &[],
        prelude: None,
    },
    Platform {
        id: "snes",
        aliases: &["superfamicom", "sfc"],
        name: "Super Nintendo",
        cpu: "65816",
        formats: &[Format::Sfc],
        default_org: Some(0x008000),
        basic_start: None,
        encoding: "ascii",
        directives: SNES_DIRS,
        functions: &[],
        prelude: None,
    },
    Platform {
        id: "cpc",
        aliases: &["amstrad", "cpc464", "cpc664", "cpc6128"],
        name: "Amstrad CPC",
        cpu: "z80",
        formats: &[Format::Amsdos],
        default_org: Some(0x4000),
        basic_start: None,
        encoding: "ascii",
        directives: CPC_DIRS,
        functions: &[],
        prelude: None,
    },
    Platform {
        id: "zx81",
        aliases: &["ts1000"],
        name: "Sinclair ZX81",
        cpu: "z80",
        formats: &[Format::P],
        default_org: Some(0x4009),
        basic_start: None,
        encoding: "zx81",
        directives: NONE,
        functions: &[],
        prelude: None,
    },
    Platform {
        id: "spectrum",
        aliases: &["zxspectrum", "zx", "spectrum48", "spectrum128", "zx128", "plus2", "+2", "speccy"],
        name: "Sinclair ZX Spectrum 48K/128K/+2",
        cpu: "z80",
        formats: &[Format::Tap, Format::Sna],
        default_org: Some(0x8000),
        basic_start: None,
        encoding: "ascii",
        directives: SPECTRUM_DIRS,
        functions: &[],
        prelude: None,
    },
    Platform {
        id: "coleco",
        aliases: &["colecovision"],
        name: "ColecoVision",
        cpu: "z80",
        formats: &[Format::Bin],
        default_org: Some(0x8000),
        basic_start: None,
        encoding: "ascii",
        directives: COLECO_DIRS,
        functions: &[],
        prelude: Some(PRELUDE_COLECO),
    },
    Platform {
        id: "vectrex",
        aliases: &["gce"],
        name: "Vectrex",
        cpu: "6809",
        formats: &[Format::Bin],
        default_org: Some(0x0000),
        basic_start: None,
        encoding: "ascii",
        directives: VECTREX_DIRS,
        functions: &[],
        prelude: Some(PRELUDE_VECTREX),
    },
    Platform {
        id: "apple2",
        aliases: &["appleii", "apple", "apple2plus", "iiplus", "a2"],
        name: "Apple II / II+ / IIe",
        cpu: "6502",
        formats: &[Format::Dsk],
        default_org: Some(0x2000),
        basic_start: None,
        encoding: "ascii",
        directives: APPLE2_DIRS,
        functions: &[],
        prelude: None,
    },
    Platform {
        id: "apple2e",
        aliases: &["apple2e_enhanced", "iie", "apple2c", "iic"],
        name: "Apple IIe enhanced (65C02)",
        cpu: "65c02",
        formats: &[Format::Dsk],
        default_org: Some(0x2000),
        basic_start: None,
        encoding: "ascii",
        directives: APPLE2_DIRS,
        functions: &[],
        prelude: None,
    },
    Platform {
        id: "mac",
        aliases: &["macintosh", "mac128", "mac512", "macplus", "macse", "mac128k", "mac512k"],
        name: "Apple Macintosh 128K/512K/Plus/SE",
        cpu: "68000",
        formats: &[Format::Bin],
        default_org: Some(0),
        basic_start: None,
        encoding: "ascii",
        directives: NONE,
        functions: &[],
        prelude: None,
    },
    Platform {
        id: "amiga",
        aliases: &["a500", "a1000", "a2000", "a500plus", "ocs", "ecs"],
        name: "Amiga 500/1000/2000",
        cpu: "68000",
        formats: &[Format::Hunk],
        default_org: Some(0),
        basic_start: None,
        encoding: "ascii",
        directives: AMIGA_DIRS,
        functions: &[],
        prelude: None,
    },
    Platform {
        id: "amiga1200",
        aliases: &["a1200", "aga"],
        name: "Amiga 1200",
        cpu: "68020",
        formats: &[Format::Hunk],
        default_org: Some(0),
        basic_start: None,
        encoding: "ascii",
        directives: AMIGA_DIRS,
        functions: &[],
        prelude: None,
    },
    Platform {
        id: "sim6502",
        aliases: &["sim"],
        name: "mimic sim6502 test machine",
        cpu: "6502",
        formats: &[Format::Bin],
        default_org: Some(0x0400),
        basic_start: None,
        encoding: "ascii",
        directives: NONE,
        functions: &[],
        prelude: None,
    },
];

pub fn by_name(name: &str) -> Option<&'static Platform> {
    let n = name.trim().to_ascii_lowercase();
    PLATFORMS.iter().find(|p| p.id == n || p.aliases.contains(&n.as_str()))
}

/// Default CPU id of a platform (used by the parsers to follow switches).
pub fn platform_cpu(name: &str) -> Option<&'static str> {
    by_name(name).map(|p| p.cpu)
}

/// Platform owning a namespaced directive or function prefix such as
/// `xex`, `ines`, `a26`, `c64`.
pub fn by_namespace(ns: &str) -> Option<&'static Platform> {
    let n = ns.to_ascii_lowercase();
    let id = match n.as_str() {
        "xex" => "atari8",
        "a26" | "vcs" => "atari2600",
        "ines" => "nes",
        "spc" => "snes",
        "amsdos" => "cpc",
        "hunk" => "amiga",
        "tap" | "sna" => "spectrum",
        "dsk" | "dos33" => "apple2",
        _ => return by_name(&n),
    };
    by_name(id)
}

const PRELUDE_VECTREX: &str = r#"
.macro VectrexHeader(title, music = $fd0d) {
    .text "g GCE 2025"
    .byte $80
    .word music
    .byte $f8, $50, $20, -$56
    .text title
    .byte $80, 0
}
"#;

const PRELUDE_COLECO: &str = r#"
.macro ColecoHeader(start) {
    .byte $aa, $55
    .word 0, 0, 0, 0
    .word start
    .fill 21, 0
}
"#;
