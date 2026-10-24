//! 1541 D64 disk image writer (35 tracks, 683 blocks), used by
//! KickAssembler's `.disk` directive on the Commodore platforms.

const TRACKS: u8 = 35;

fn sectors(track: u8) -> u8 {
    match track {
        1..=17 => 21,
        18..=24 => 19,
        25..=30 => 18,
        _ => 17,
    }
}

fn offset(track: u8, sector: u8) -> usize {
    let mut o = 0usize;
    for t in 1..track {
        o += sectors(t) as usize * 256;
    }
    o + sector as usize * 256
}

#[derive(Clone, Debug)]
pub struct DiskFile {
    pub name: String,
    /// 0x82 PRG, 0x81 SEQ, 0x83 USR, 0x84 REL, 0x80 DEL (bit 7 = closed).
    pub kind: u8,
    pub data: Vec<u8>,
}

fn petscii_name(s: &str) -> [u8; 16] {
    let mut out = [0xa0; 16];
    for (i, ch) in s.chars().take(16).enumerate() {
        let c = ch as u32;
        out[i] = match ch {
            'a'..='z' => (c - 0x20) as u8,
            _ if c < 0x80 => c as u8,
            _ => b'?',
        };
    }
    out
}

/// Build a D64 image with the given disk name, id and files.
pub fn d64(disk_name: &str, id: &str, files: &[DiskFile]) -> Result<Vec<u8>, String> {
    let total: usize = (1..=TRACKS).map(|t| sectors(t) as usize).sum();
    let mut img = vec![0u8; total * 256];
    let mut used = vec![vec![false; 21]; TRACKS as usize + 1];
    used[18][0] = true;
    used[18][1] = true;
    // allocation order: tracks nearest the directory first, interleave 10
    let order: Vec<u8> = (1..18u8).rev().zip(19..=TRACKS).flat_map(|(a, b)| [a, b]).chain(std::iter::once(18).filter(|_| false)).collect();
    let alloc = |used: &mut Vec<Vec<bool>>, last: Option<(u8, u8)>| -> Option<(u8, u8)> {
        let start_track = last.map(|l| l.0);
        let tracks: Vec<u8> = match start_track {
            Some(t) => std::iter::once(t).chain(order.iter().copied().filter(|&x| x != t)).collect(),
            None => order.clone(),
        };
        for t in tracks {
            let n = sectors(t);
            let first = last.filter(|l| l.0 == t).map_or(0, |l| (l.1 + 10) % n);
            for k in 0..n {
                let s = (first + k) % n;
                if !used[t as usize][s as usize] {
                    used[t as usize][s as usize] = true;
                    return Some((t, s));
                }
            }
        }
        None
    };
    let mut entries = Vec::new();
    for f in files {
        let blocks: Vec<&[u8]> = if f.data.is_empty() { vec![&[][..]] } else { f.data.chunks(254).collect() };
        let mut chain = Vec::new();
        let mut last = None;
        for _ in &blocks {
            let ts = alloc(&mut used, last).ok_or("disk full")?;
            chain.push(ts);
            last = Some(ts);
        }
        for (i, b) in blocks.iter().enumerate() {
            let (t, s) = chain[i];
            let o = offset(t, s);
            if let Some(&(nt, ns)) = chain.get(i + 1) {
                img[o] = nt;
                img[o + 1] = ns;
            } else {
                img[o] = 0;
                img[o + 1] = b.len() as u8 + 1;
            }
            img[o + 2..o + 2 + b.len()].copy_from_slice(b);
        }
        entries.push((f, chain[0], blocks.len() as u16));
    }
    // directory sectors on track 18 from sector 1, interleave 3
    let dir_sectors: Vec<u8> = {
        let mut v = vec![1u8];
        let mut s = 1u8;
        while v.len() < 18 {
            s = (s + 3) % 19;
            if s == 0 || v.contains(&s) {
                s = (1..19).find(|x| !v.contains(x)).unwrap();
            }
            v.push(s);
        }
        v
    };
    let need = entries.len().div_ceil(8).max(1);
    if need > dir_sectors.len() {
        return Err("too many files for the directory".into());
    }
    for (k, chunk) in entries.chunks(8).enumerate().chain(std::iter::once((0, &[][..])).filter(|_| entries.is_empty())) {
        let s = dir_sectors[k];
        used[18][s as usize] = true;
        let o = offset(18, s);
        if k + 1 < need {
            img[o] = 18;
            img[o + 1] = dir_sectors[k + 1];
        } else {
            img[o] = 0;
            img[o + 1] = 0xff;
        }
        for (i, (f, (t, sec), blocks)) in chunk.iter().enumerate() {
            let e = o + i * 32;
            img[e + 2] = f.kind;
            img[e + 3] = *t;
            img[e + 4] = *sec;
            img[e + 5..e + 21].copy_from_slice(&petscii_name(&f.name));
            img[e + 30..e + 32].copy_from_slice(&blocks.to_le_bytes());
        }
    }
    // BAM
    let b = offset(18, 0);
    img[b] = 18;
    img[b + 1] = 1;
    img[b + 2] = 0x41;
    for t in 1..=TRACKS {
        let n = sectors(t);
        let mut bits = 0u32;
        let mut free = 0;
        for s in 0..n {
            if !used[t as usize][s as usize] {
                bits |= 1 << s;
                free += 1;
            }
        }
        let e = b + 4 * t as usize;
        img[e] = free;
        img[e + 1..e + 4].copy_from_slice(&bits.to_le_bytes()[..3]);
    }
    img[b + 0x90..b + 0xa0].copy_from_slice(&petscii_name(disk_name));
    img[b + 0xa0] = 0xa0;
    img[b + 0xa1] = 0xa0;
    img[b + 0xa4] = 0xa0;
    img[b + 0xa5] = b'2';
    img[b + 0xa6] = b'A';
    // the id: two characters, or up to five (covering the DOS type)
    for (i, ch) in id.bytes().take(5).enumerate() {
        img[b + 0xa2 + i] = ch.to_ascii_uppercase();
    }
    if id.len() < 2 {
        for i in id.len()..2 {
            img[b + 0xa2 + i] = 0x20;
        }
    }
    for x in &mut img[b + 0xa7..b + 0xab] {
        *x = 0xa0;
    }
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_file() {
        let img = d64("TEST", "01", &[DiskFile { name: "HELLO".into(), kind: 0x82, data: vec![1, 8, 0xea] }]).unwrap();
        assert_eq!(img.len(), 174848);
        let dir = offset(18, 1);
        assert_eq!(img[dir + 2], 0x82);
        let (t, s) = (img[dir + 3], img[dir + 4]);
        let o = offset(t, s);
        assert_eq!(&img[o..o + 5], &[0, 4, 1, 8, 0xea]);
    }
}
