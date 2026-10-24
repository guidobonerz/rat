//! Commodore platform support: BASIC upstart macros, SID files
//! (`LoadSid`) and pictures (`LoadPicture`).

macro_rules! basic_prelude {
    ($start:literal, $end:literal) => {
        concat!(
            r#"
.macro BasicUpstart(address) {
    .word upstartEnd
    .word 10
    .byte $9e
    .text toIntString(address)
    .byte 0
upstartEnd:
    .word 0
}
.macro BasicUpstart2(address) {
    * = "#,
            $start,
            r#" "Basic"
    BasicUpstart(address)
    * = "#,
            $end,
            r#" "Basic End"
}
"#
        )
    };
}

pub const PRELUDE_C64: &str = basic_prelude!("$0801", "$080d");
pub const PRELUDE_C128: &str = basic_prelude!("$1c01", "$1c0d");
pub const PRELUDE_VIC20: &str = basic_prelude!("$1001", "$100d");
pub const PRELUDE_PET: &str = basic_prelude!("$0401", "$040d");
pub const PRELUDE_PLUS4: &str = basic_prelude!("$1001", "$100d");

/// A PSID/RSID file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sid {
    pub header: String,
    pub version: u16,
    pub location: u16,
    pub init: u16,
    pub play: u16,
    pub songs: u16,
    pub start_song: u16,
    pub speed: u32,
    pub name: String,
    pub author: String,
    pub copyright: String,
    pub flags: u16,
    pub start_page: u8,
    pub page_length: u8,
    /// Player data (without load address).
    pub data: Vec<u8>,
}

fn text(b: &[u8]) -> String {
    b.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect()
}

pub fn parse_sid(b: &[u8]) -> Result<Sid, String> {
    if b.len() < 0x76 || !(b.starts_with(b"PSID") || b.starts_with(b"RSID")) {
        return Err("not a PSID/RSID file".into());
    }
    let w = |o: usize| u16::from_be_bytes([b[o], b[o + 1]]);
    let version = w(4);
    let data_off = w(6) as usize;
    let mut location = w(8);
    if data_off > b.len() {
        return Err("SID data offset beyond the file".into());
    }
    let mut data = &b[data_off..];
    if location == 0 {
        if data.len() < 2 {
            return Err("SID data too short".into());
        }
        location = u16::from_le_bytes([data[0], data[1]]);
        data = &data[2..];
    }
    let (flags, start_page, page_length) = if version >= 2 && b.len() >= 0x7c {
        (w(0x76), b[0x78], b[0x79])
    } else {
        (0, 0, 0)
    };
    Ok(Sid {
        header: text(&b[0..4]),
        version,
        location,
        init: w(0x0a),
        play: w(0x0c),
        songs: w(0x0e),
        start_song: w(0x10),
        speed: u32::from_be_bytes([b[0x12], b[0x13], b[0x14], b[0x15]]),
        name: text(&b[0x16..0x36]),
        author: text(&b[0x36..0x56]),
        copyright: text(&b[0x56..0x76]),
        flags,
        start_page,
        page_length,
        data: data.to_vec(),
    })
}

/// A decoded picture (RGB pixels).
#[derive(Clone, Debug)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

impl Picture {
    pub fn decode(bytes: &[u8]) -> Result<Picture, String> {
        let img = image::load_from_memory(bytes).map_err(|e| format!("cannot decode picture: {e}"))?;
        let rgb = img.to_rgb8();
        let (width, height) = rgb.dimensions();
        let pixels = rgb
            .pixels()
            .map(|p| u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2]))
            .collect();
        Ok(Picture { width, height, pixels })
    }

    pub fn pixel(&self, x: i64, y: i64) -> Result<u32, String> {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return Err(format!("pixel ({x},{y}) outside the {}x{} picture", self.width, self.height));
        }
        Ok(self.pixels[(y as u32 * self.width + x as u32) as usize])
    }

    /// Index of `rgb` in the color table, adding it when there is room.
    fn color_index(table: &mut Vec<u32>, rgb: u32, max: usize) -> Result<u8, String> {
        if let Some(i) = table.iter().position(|&c| c == rgb) {
            return Ok(i as u8);
        }
        if table.len() >= max {
            return Err(format!("too many colors (#{rgb:06x} not in the color table)"));
        }
        table.push(rgb);
        Ok((table.len() - 1) as u8)
    }

    /// 8 pixels from column `x` (in bytes) on row `y`; bit set for colors
    /// other than table[0].
    pub fn singlecolor_byte(&self, x: i64, y: i64, table: &mut Vec<u32>) -> Result<u8, String> {
        let mut v = 0u8;
        for i in 0..8 {
            let c = Self::color_index(table, self.pixel(x * 8 + i, y)?, 2)?;
            v = v << 1 | c;
        }
        Ok(v)
    }

    /// 4 double-width pixels from column `x` (in bytes) on row `y`.
    pub fn multicolor_byte(&self, x: i64, y: i64, table: &mut Vec<u32>) -> Result<u8, String> {
        let mut v = 0u8;
        for i in 0..4 {
            let c = Self::color_index(table, self.pixel(x * 8 + i * 2, y)?, 4)?;
            v = v << 2 | c;
        }
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn psid_header() {
        let mut b = vec![0u8; 0x7c];
        b[..4].copy_from_slice(b"PSID");
        b[5] = 2;
        b[7] = 0x7c;
        b[0x0a..0x0c].copy_from_slice(&[0x10, 0x00]);
        b[0x0c..0x0e].copy_from_slice(&[0x10, 0x03]);
        b[0x0f] = 1;
        b[0x16..0x19].copy_from_slice(b"Abc");
        b.extend([0x00, 0x10, 0xea]);
        let s = parse_sid(&b).unwrap();
        assert_eq!((s.location, s.init, s.play, s.songs), (0x1000, 0x1000, 0x1003, 1));
        assert_eq!(s.name, "Abc");
        assert_eq!(s.data, vec![0xea]);
        assert!(PRELUDE_C64.contains("* = $0801"));
    }
}
