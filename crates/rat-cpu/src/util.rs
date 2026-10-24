//! Operand token helpers shared by the backends.

use std::ops::Range;

use rat_core::{Tok, Token};

use crate::{Num, OperandCtx};

fn opens(t: &Token) -> bool {
    t.is_punct("(") || t.is_punct("[") || t.is_punct("{")
}

fn closes(t: &Token) -> bool {
    t.is_punct(")") || t.is_punct("]") || t.is_punct("}")
}

/// Index of the bracket closing the one at `open`.
pub fn matching(toks: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (i, t) in toks.iter().enumerate().skip(open) {
        if opens(t) {
            depth += 1;
        } else if closes(t) {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Split `toks[range]` at top-level occurrences of `sep`.
pub fn split_top(toks: &[Token], range: Range<usize>, sep: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = range.start;
    for i in range.clone() {
        let t = &toks[i];
        if opens(t) {
            depth += 1;
        } else if closes(t) {
            depth -= 1;
        } else if depth == 0 && t.is_punct(sep) {
            out.push(start..i);
            start = i + 1;
        }
    }
    out.push(start..range.end);
    out
}

/// Split the whole operand at top-level commas. An empty operand gives no
/// parts.
pub fn commas(toks: &[Token]) -> Vec<Range<usize>> {
    if toks.is_empty() {
        return Vec::new();
    }
    split_top(toks, 0..toks.len(), ",")
}

/// The single identifier in a range, if that is all it holds.
pub fn single_ident<'a>(toks: &'a [Token], r: &Range<usize>) -> Option<&'a str> {
    if r.end == r.start + 1 {
        if let Tok::Ident(s) = &toks[r.start].tok {
            return Some(s);
        }
    }
    None
}

/// Does `r` start with `(` and end with the matching `)`?
pub fn wrapped(toks: &[Token], r: &Range<usize>, open: &str, close: &str) -> bool {
    r.end > r.start + 1
        && toks[r.start].is_punct(open)
        && toks[r.end - 1].is_punct(close)
        && matching(toks, r.start) == Some(r.end - 1)
}

/// Evaluate a range with a nicer error for empty input.
pub fn eval(ctx: &mut dyn OperandCtx, r: &Range<usize>) -> Result<Num, String> {
    if r.start >= r.end {
        return Err("missing expression".into());
    }
    ctx.eval(r.start, r.end)
}

/// Check that a known value fits `[lo, hi]`; reports a soft error and
/// returns the value masked to `bits`.
pub fn fit(ctx: &mut dyn OperandCtx, n: Num, lo: i64, hi: i64, bits: u32, what: &str) -> i64 {
    if n.known && (n.value < lo || n.value > hi) {
        ctx.error(format!("{what} out of range: {} (allowed {lo}..{hi})", n.value));
    }
    let mask = if bits >= 64 { -1 } else { (1i64 << bits) - 1 };
    n.value & mask
}

/// Relative branch displacement from `next` (address after the
/// instruction) to target, checked against `bits` signed bits.
pub fn rel(ctx: &mut dyn OperandCtx, target: Num, next: i64, bits: u32) -> i64 {
    if !target.known {
        return 0;
    }
    let d = target.value - next;
    let lim = 1i64 << (bits - 1);
    if d < -lim || d >= lim {
        ctx.error(format!("branch out of range ({d} bytes, allowed {}..{})", -lim, lim - 1));
    }
    d & ((1i64 << bits) - 1)
}

pub fn le16(v: i64) -> [u8; 2] {
    (v as u16).to_le_bytes()
}

pub fn be16(v: i64) -> [u8; 2] {
    (v as u16).to_be_bytes()
}

/// Hex formatting for disassembly: `$xx` with at least `digits` digits.
pub fn hex(v: u32, digits: usize) -> String {
    format!("${v:0digits$x}")
}
