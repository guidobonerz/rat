//! ZX Spectrum files: TAP tapes and 48K SNA snapshots.
//!
//! **TAP**: a row of blocks, each a 16-bit little-endian length followed
//! by the bytes as the ROM saves them: a flag byte (`$00` header, `$FF`
//! data), the data and a checksum (XOR of flag and data). A header is 17
//! bytes: type (0 Program, 3 Code), 10-character name padded with spaces,
//! data length, and two parameters (Program: autostart line or 32768 and
//! the length of the program without variables; Code: the start address
//! and 32768). mimic's `media::cdt::Tape::from_tap` reads exactly this.
//!
//! **SNA (48K)**: the 27-byte header I, HL', DE', BC', AF', HL, DE, BC, IY,
//! IX, IFF2 (bit 2), R, AF, SP, IM, border, then the 48 KB from `$4000`;
//! PC lies on the stack and is popped on load (mimic's
//! `machine-spectrum::snapshot`).

use crate::{flatten, Chunk};

/// A standard-speed block: length, flag, data, checksum.
fn block(flag: u8, data: &[u8]) -> Vec<u8> {
    let mut v = ((data.len() + 2) as u16).to_le_bytes().to_vec();
    v.push(flag);
    v.extend(data);
    v.push(data.iter().fold(flag, |x, &b| x ^ b));
    v
}

fn header(kind: u8, name: &str, len: u16, p1: u16, p2: u16) -> Vec<u8> {
    let mut h = vec![kind];
    let mut n: Vec<u8> = name.bytes().take(10).collect();
    n.resize(10, b' ');
    h.extend(n);
    h.extend(len.to_le_bytes());
    h.extend(p1.to_le_bytes());
    h.extend(p2.to_le_bytes());
    block(0, &h)
}

/// A number inside a BASIC line: its digits and the 5-byte binary form.
fn basic_num(n: u16) -> Vec<u8> {
    let mut v = n.to_string().into_bytes();
    v.extend([0x0e, 0, 0]);
    v.extend(n.to_le_bytes());
    v.push(0);
    v
}

/// `10 CLEAR start-1: LOAD "" CODE: RANDOMIZE USR exec`.
pub fn basic_loader(start: u16, exec: u16) -> Vec<u8> {
    let mut body = vec![0xfd]; // CLEAR
    body.extend(basic_num(start.wrapping_sub(1)));
    body.extend([b':', 0xef, b'"', b'"', 0xaf]); // LOAD "" CODE
    body.extend([b':', 0xf9, 0xc0]); // RANDOMIZE USR
    body.extend(basic_num(exec));
    body.push(0x0d);
    let mut v = 10u16.to_be_bytes().to_vec();
    v.extend((body.len() as u16).to_le_bytes());
    v.extend(body);
    v
}

/// Options of a TAP file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TapOptions {
    /// Name of the CODE block (10 characters).
    pub name: String,
    /// Entry point; with `loader` the BASIC program jumps there.
    pub exec: Option<u16>,
    /// Put a BASIC loader before the code block.
    pub loader: bool,
}

/// TAP file: an optional BASIC loader, then the code as one CODE block
/// from the lowest to the highest address.
pub fn tap(chunks: &[Chunk], fill: u8, o: &TapOptions) -> Result<Vec<u8>, String> {
    let Some((lo, hi)) = crate::extent(chunks) else { return Ok(Vec::new()) };
    if hi > 0x1_0000 {
        return Err(format!("code ends at ${hi:x}, beyond the 64K address space"));
    }
    let data = flatten(chunks, lo, hi, fill);
    let mut v = Vec::new();
    if o.loader {
        let prog = basic_loader(lo as u16, o.exec.unwrap_or(lo as u16));
        v.extend(header(0, &o.name, prog.len() as u16, 10, prog.len() as u16));
        v.extend(block(0xff, &prog));
    }
    v.extend(header(3, &o.name, data.len() as u16, lo as u16, 32768));
    v.extend(block(0xff, &data));
    Ok(v)
}

/// Register values of a 48K SNA snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnaOptions {
    /// Entry point (default: the lowest address written).
    pub exec: Option<u16>,
    /// Stack pointer before PC is pushed (default `$FFFE`).
    pub sp: Option<u16>,
    pub border: u8,
    /// Interrupt mode (default 1).
    pub im: u8,
    /// Interrupts enabled (default true).
    pub ei: bool,
}

impl Default for SnaOptions {
    fn default() -> Self {
        SnaOptions { exec: None, sp: None, border: 7, im: 1, ei: true }
    }
}

/// 48K SNA snapshot of the program; the machine starts at `exec`.
pub fn sna(chunks: &[Chunk], fill: u8, o: &SnaOptions) -> Result<Vec<u8>, String> {
    let Some((lo, hi)) = crate::extent(chunks) else { return Ok(Vec::new()) };
    if lo < 0x4000 {
        return Err(format!("SNA snapshots hold RAM from $4000; the program starts at ${lo:04x}"));
    }
    if hi > 0x1_0000 {
        return Err(format!("code ends at ${hi:x}, beyond the 64K address space"));
    }
    let exec = o.exec.unwrap_or(lo as u16);
    let sp = o.sp.unwrap_or(0xfffe).wrapping_sub(2);
    if sp < 0x4000 {
        return Err("the stack pointer must lie in RAM ($4000 and above)".into());
    }
    let mut ram = flatten(chunks, 0x4000, 0x1_0000, fill);
    let at = usize::from(sp) - 0x4000;
    if (at..at + 2).any(|i| chunks.iter().any(|c| (c.addr..c.addr + c.data.len() as u32).contains(&(i as u32 + 0x4000)))) {
        return Err(format!("the stack at ${sp:04x} overlaps the program; set `sp=`"));
    }
    ram[at..at + 2].copy_from_slice(&exec.to_le_bytes());
    let mut v = vec![0x3f]; // I
    v.extend([0; 18]); // HL' DE' BC' AF' HL DE BC IY IX
    v.push(if o.ei { 4 } else { 0 });
    v.push(0); // R
    v.extend([0, 0]); // AF
    v.extend(sp.to_le_bytes());
    v.push(o.im.min(2));
    v.push(o.border & 7);
    v.extend(ram);
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_blocks_have_lengths_and_checksums() {
        let c = vec![Chunk { addr: 0x8000, data: vec![0xc9] }];
        let t = tap(&c, 0, &TapOptions { name: "T".into(), exec: None, loader: false }).unwrap();
        // header: length 19, flag 0, 17 bytes, checksum
        assert_eq!(&t[..3], &[19, 0, 0]);
        assert_eq!(t[3], 3);
        assert_eq!(&t[4..14], b"T         ");
        assert_eq!(&t[14..20], &[1, 0, 0x00, 0x80, 0, 0x80]);
        let sum = t[3..20].iter().fold(0, |x, &b| x ^ b);
        assert_eq!(t[20], sum);
        assert_eq!(&t[21..], &[3, 0, 0xff, 0xc9, 0xff ^ 0xc9]);
    }

    #[test]
    fn loader_line() {
        let l = basic_loader(0x8000, 0x8000);
        assert_eq!(&l[..4], &[0, 10, (l.len() - 4) as u8, 0]);
        assert_eq!(l[4], 0xfd);
        assert_eq!(&l[5..10], b"32767");
        assert_eq!(*l.last().unwrap(), 0x0d);
    }

    #[test]
    fn sna_has_pc_on_the_stack() {
        let c = vec![Chunk { addr: 0x8000, data: vec![0x18, 0xfe] }];
        let s = sna(&c, 0, &SnaOptions::default()).unwrap();
        assert_eq!(s.len(), 49179);
        assert_eq!(&s[23..25], &[0xfc, 0xff]);
        assert_eq!(&s[27 + 0xbffc..27 + 0xbffe], &[0x00, 0x80]);
        assert_eq!(&s[27 + 0x4000..27 + 0x4002], &[0x18, 0xfe]);
    }
}
