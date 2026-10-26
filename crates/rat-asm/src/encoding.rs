//! Text encodings: ASCII, PETSCII (mixed/upper), C64 screen codes
//! (mixed/upper), ATASCII screen codes, ZX81, Apple II high-bit ASCII.

/// Canonical encoding name for the spellings of ACME, Kick and rat.
pub fn canonical(name: &str) -> Option<&'static str> {
    Some(match name.to_ascii_lowercase().as_str() {
        "ascii" | "raw" | "none" | "plain" => "ascii",
        "petscii_mixed" | "pet" | "petscii" | "cbm" => "petscii_mixed",
        "petscii_upper" => "petscii_upper",
        "screencode_mixed" | "scr" | "screen" | "screencode" => "screencode_mixed",
        "screencode_upper" => "screencode_upper",
        "atascii" => "ascii",
        "atari_screen" | "antic" => "atari_screen",
        "zx81" => "zx81",
        "apple" | "apple_high" | "hiascii" | "ascii_high" => "apple",
        _ => return None,
    })
}

fn screen_mixed(c: u32) -> u32 {
    match c {
        0x40 => 0,
        0x61..=0x7a => c - 0x60,
        0x5b..=0x5f => c - 0x40,
        0x41..=0x5a => c,
        0x20..=0x3f => c,
        0xa3 => 0x1c, // £
        _ => c & 0xff,
    }
}

fn zx81(c: char) -> u8 {
    let c = c.to_ascii_uppercase();
    match c {
        ' ' => 0,
        '"' => 11,
        '£' => 12,
        '$' => 13,
        ':' => 14,
        '?' => 15,
        '(' => 16,
        ')' => 17,
        '>' => 18,
        '<' => 19,
        '=' => 20,
        '+' => 21,
        '-' => 22,
        '*' => 23,
        '/' => 24,
        ';' => 25,
        ',' => 26,
        '.' => 27,
        '0'..='9' => 28 + (c as u8 - b'0'),
        'A'..='Z' => 38 + (c as u8 - b'A'),
        _ => 15,
    }
}

/// Encode one character.
pub fn encode_char(enc: &str, ch: char) -> u8 {
    let c = ch as u32;
    match enc {
        "petscii_mixed" => match c {
            0x61..=0x7a => (c - 0x20) as u8,
            0x41..=0x5a => (c + 0x80) as u8,
            0xa3 => 0x5c,
            _ => c as u8,
        },
        "petscii_upper" => match c {
            0x61..=0x7a => (c - 0x20) as u8,
            _ => c as u8,
        },
        "screencode_mixed" => screen_mixed(c) as u8,
        "screencode_upper" => match c {
            0x61..=0x7a => (c - 0x60) as u8,
            0x41..=0x5a => (c - 0x40) as u8,
            _ => screen_mixed(c) as u8,
        },
        "atari_screen" => match c {
            0x20..=0x5f => (c - 0x20) as u8,
            0x00..=0x1f => (c + 0x40) as u8,
            _ => c as u8,
        },
        "zx81" => zx81(ch),
        "apple" => if c < 0x80 { c as u8 | 0x80 } else { b'?' | 0x80 },
        _ => {
            if c < 0x100 {
                c as u8
            } else {
                b'?'
            }
        }
    }
}

/// ACME `!pet`: lower case to upper, upper case to shifted.
pub fn encode_str(enc: &str, s: &str, table: Option<&[u8; 256]>) -> Vec<u8> {
    s.chars()
        .map(|ch| match table {
            Some(t) if (ch as u32) < 256 => t[ch as usize],
            _ => encode_char(enc, ch),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert_eq!(encode_str("screencode_mixed", "@aA1", None), vec![0, 1, 0x41, 0x31]);
        assert_eq!(encode_str("petscii_mixed", "aA", None), vec![0x41, 0xc1]);
        assert_eq!(encode_str("zx81", "HI", None), vec![45, 46]);
    }
}
