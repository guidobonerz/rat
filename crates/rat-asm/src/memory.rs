//! Segments and memory blocks.

use rat_output::Chunk;

#[derive(Clone, Debug, Default)]
pub struct Block {
    pub start: u32,
    pub data: Vec<u8>,
    pub name: Option<String>,
    /// Bank number for banked formats (a26, nes).
    pub bank: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct SegParams {
    pub start: Option<u32>,
    pub start_after: Option<String>,
    pub min: Option<u32>,
    pub max: Option<u32>,
    pub fill: Option<u8>,
    pub allow_overlap: bool,
    pub segments: Vec<String>,
    pub out_prg: Option<String>,
    pub out_bin: Option<String>,
    pub virtual_: bool,
    pub hide: bool,
    pub align: Option<u32>,
    /// Bytes of `prgFiles` / `sidFiles`, placed at their load addresses.
    pub extra: Vec<Chunk>,
}

#[derive(Clone, Debug, Default)]
pub struct Segment {
    pub name: String,
    pub blocks: Vec<Block>,
    pub params: SegParams,
    /// Index of the block receiving bytes.
    pub cur: Option<usize>,
    /// Next real address.
    pub pc: u32,
    /// Has emitted or set an origin.
    pub started: bool,
    pub bank: Option<u32>,
    /// Hunk kind for Amiga sections.
    pub hunk: Option<rat_output::HunkKind>,
}

impl Segment {
    pub fn new(name: &str, pc: u32) -> Segment {
        Segment {
            name: name.to_owned(),
            pc,
            ..Default::default()
        }
    }

    pub fn org(&mut self, addr: u32, name: Option<String>) {
        self.pc = addr;
        self.cur = None;
        self.started = true;
        if name.is_some() {
            self.blocks.push(Block { start: addr, data: Vec::new(), name, bank: self.bank });
            self.cur = Some(self.blocks.len() - 1);
        }
    }

    pub fn emit(&mut self, bytes: &[u8]) {
        self.started = true;
        let idx = match self.cur {
            Some(i) if self.blocks[i].start + self.blocks[i].data.len() as u32 == self.pc => i,
            _ => {
                self.blocks.push(Block { start: self.pc, data: Vec::new(), name: None, bank: self.bank });
                self.blocks.len() - 1
            }
        };
        self.cur = Some(idx);
        self.blocks[idx].data.extend_from_slice(bytes);
        self.pc = self.pc.wrapping_add(bytes.len() as u32);
    }

    /// Reserve without output.
    pub fn skip(&mut self, n: u32) {
        self.pc = self.pc.wrapping_add(n);
        self.cur = None;
        self.started = true;
    }

    pub fn chunks(&self) -> Vec<Chunk> {
        let mut v = Vec::new();
        // `fill` with min/max pads the whole range (data overwrites it)
        if let (Some(f), Some(lo), Some(hi)) = (self.params.fill, self.params.min, self.params.max) {
            if hi >= lo {
                v.push(Chunk { addr: lo, data: vec![f; (hi - lo + 1) as usize] });
            }
        }
        v.extend(self.params.extra.iter().cloned());
        v.extend(self.blocks.iter().filter(|b| !b.data.is_empty()).map(|b| Chunk { addr: b.start, data: b.data.clone() }));
        v
    }

    /// End address (one past the highest byte).
    pub fn end(&self) -> u32 {
        self.blocks
            .iter()
            .map(|b| b.start + b.data.len() as u32)
            .max()
            .unwrap_or(self.pc)
            .max(self.pc)
    }

    /// First overlapping address of two blocks, if any.
    pub fn overlap(&self) -> Option<(u32, Option<String>, Option<String>)> {
        let mut v: Vec<&Block> = self.blocks.iter().filter(|b| !b.data.is_empty()).collect();
        v.sort_by_key(|b| b.start);
        for w in v.windows(2) {
            let a_end = w[0].start + w[0].data.len() as u32;
            if w[1].start < a_end && w[0].bank == w[1].bank {
                return Some((w[1].start, w[0].name.clone(), w[1].name.clone()));
            }
        }
        None
    }
}
