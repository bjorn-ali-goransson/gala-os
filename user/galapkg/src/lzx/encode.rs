//! A small LZX encoder for the tests, written from the same
//! specifications as the decoder ([MS-PATCH] sections 2.3-2.6, the
//! Cabinet SDK's frames and E8 preprocessing): greedy matching with the
//! repeated offsets, real Huffman codes (length-limited by halving
//! frequencies), path lengths as pretree deltas with the 17/18/19 runs,
//! all three block types. Its cabinets are checked against cabinet
//! readers that are not gala's (cabextract, 7-Zip), so a shared
//! misreading of the specification would show there.

use super::*;
use std::vec;
use std::vec::Vec as StdVec;

/// Which block type covers the next `n` bytes.
#[derive(Clone, Copy, Debug)]
pub enum Block {
    Verbatim(usize),
    Aligned(usize),
    Uncompressed(usize),
}

impl Block {
    fn len(self) -> usize {
        match self {
            Block::Verbatim(n) | Block::Aligned(n) | Block::Uncompressed(n) => n,
        }
    }
}

struct Writer {
    out: StdVec<u8>,
    acc: u32,
    n: u32,
    /// Byte offsets where frames end.
    cuts: StdVec<usize>,
}

impl Writer {
    fn bits(&mut self, v: u32, k: u32) {
        for i in (0..k).rev() {
            self.acc = (self.acc << 1) | ((v >> i) & 1);
            self.n += 1;
            if self.n == 16 {
                self.out.extend_from_slice(&(self.acc as u16).to_le_bytes());
                self.acc = 0;
                self.n = 0;
            }
        }
    }
    fn align(&mut self) {
        if self.n > 0 {
            self.bits(0, 16 - self.n);
        }
    }
    /// The total bits written.
    fn position(&self) -> usize {
        self.out.len() * 8 + self.n as usize
    }
}

/// Huffman path lengths for `freq`, none longer than `limit`; at least
/// two elements get a code (the specification's rule for a tree).
pub fn lengths(freq: &[u32], limit: u8) -> StdVec<u8> {
    let mut f: StdVec<u64> = freq.iter().map(|&x| u64::from(x)).collect();
    let mut used = f.iter().filter(|&&x| x > 0).count();
    let mut i = 0;
    while used < 2 {
        if f[i] == 0 {
            f[i] = 1;
            used += 1;
        }
        i += 1;
    }
    loop {
        // Nodes: (weight, leaves under it).
        let mut nodes: StdVec<(u64, StdVec<usize>)> =
            f.iter().enumerate().filter(|(_, w)| **w > 0).map(|(s, &w)| (w, vec![s])).collect();
        let mut lens = vec![0u8; f.len()];
        while nodes.len() > 1 {
            nodes.sort_by_key(|n| core::cmp::Reverse(n.0));
            let a = nodes.pop().unwrap();
            let b = nodes.pop().unwrap();
            let mut leaves = a.1;
            leaves.extend(b.1);
            for &s in &leaves {
                lens[s] += 1;
            }
            nodes.push((a.0 + b.0, leaves));
        }
        if lens.iter().all(|&l| l <= limit) {
            return lens;
        }
        for w in &mut f {
            if *w > 0 {
                *w = (*w).div_ceil(2);
            }
        }
    }
}

/// Canonical codes for path lengths ([MS-PATCH] 2.4).
fn codes(lens: &[u8]) -> StdVec<u32> {
    let mut codes = vec![0u32; lens.len()];
    let mut code = 0u32;
    for len in 1..=16u8 {
        for (s, &l) in lens.iter().enumerate() {
            if l == len {
                codes[s] = code;
                code += 1;
            }
        }
        code <<= 1;
    }
    codes
}

/// Writes a tree's path lengths through a pretree, as deltas from
/// `prev` with runs.
fn write_lengths(w: &mut Writer, lens: &[u8], prev: &[u8]) {
    // (pretree code, extra bits, extra value, then a second code for 19).
    let mut ops: StdVec<(usize, u32, u32, Option<usize>)> = StdVec::new();
    let delta = |i: usize, v: u8| ((17 + prev[i] - v) % 17) as usize;
    let mut i = 0;
    while i < lens.len() {
        let run = lens[i..].iter().take_while(|&&l| l == lens[i]).count();
        if lens[i] == 0 && run >= 20 {
            let n = run.min(51);
            ops.push((18, 5, (n - 20) as u32, None));
            i += n;
        } else if lens[i] == 0 && run >= 4 {
            let n = run.min(19);
            ops.push((17, 4, (n - 4) as u32, None));
            i += n;
        } else if run >= 4 {
            let n = run.min(5);
            ops.push((19, 1, (n - 4) as u32, Some(delta(i, lens[i]))));
            i += n;
        } else {
            ops.push((delta(i, lens[i]), 0, 0, None));
            i += 1;
        }
    }
    let mut freq = [0u32; PRETREE];
    for &(c, _, _, second) in &ops {
        freq[c] += 1;
        if let Some(s) = second {
            freq[s] += 1;
        }
    }
    let pl = lengths(&freq, 15);
    let pc = codes(&pl);
    for &l in &pl {
        w.bits(u32::from(l), 4);
    }
    for (c, k, v, second) in ops {
        w.bits(pc[c], u32::from(pl[c]));
        w.bits(v, k);
        if let Some(s) = second {
            w.bits(pc[s], u32::from(pl[s]));
        }
    }
}

#[derive(Clone, Copy)]
enum Token {
    Literal(u8),
    /// Main element, length footer, offset slot and footer.
    Match {
        main: usize,
        len_footer: Option<usize>,
        slot: usize,
        footer: u32,
    },
}

fn slot_of(formatted: u32) -> usize {
    let mut s = 0;
    while base_position(s + 1) <= formatted {
        s += 1;
    }
    s
}

/// The forward E8 translation of one frame ([MS-PATCH] 2.2.2).
pub fn e8_forward(data: &mut [u8], frame_pos: u32, file_size: u32) {
    if data.len() <= 10 {
        return;
    }
    let mut i = 0;
    while i < data.len() - 10 {
        if data[i] != 0xe8 {
            i += 1;
            continue;
        }
        let cur = (frame_pos + i as u32) as i64;
        let disp = i32::from_le_bytes([data[i + 1], data[i + 2], data[i + 3], data[i + 4]]) as i64;
        let mut target = cur + disp;
        if target >= 0 && target < i64::from(file_size) + cur {
            if target >= i64::from(file_size) {
                target = disp - i64::from(file_size);
            }
            data[i + 1..i + 5].copy_from_slice(&(target as i32).to_le_bytes());
        }
        i += 5;
    }
}

/// Records position `q` under its first three bytes (the 48 latest).
fn remember<'s>(
    chains: &mut std::collections::HashMap<&'s [u8], StdVec<usize>>,
    src: &'s [u8],
    q: usize,
) {
    if q + 3 <= src.len() {
        let list = chains.entry(&src[q..q + 3]).or_default();
        if list.len() == 48 {
            list.remove(0);
        }
        list.push(q);
    }
}

/// Encodes `data` in the given blocks (their sizes must add up to its
/// length); returns the frames (compressed bytes, uncompressed size),
/// one per CFDATA block.
pub fn encode(
    window: u8,
    e8: Option<u32>,
    blocks: &[Block],
    data: &[u8],
) -> StdVec<(StdVec<u8>, usize)> {
    encode_with_positions(window, e8, blocks, data).0
}

/// As [`encode`], with the bit position before each block's header (to
/// pick sizes that put an uncompressed block's header end on a word
/// boundary: its padding is then 16 bits).
pub fn encode_with_positions(
    window: u8,
    e8: Option<u32>,
    blocks: &[Block],
    data: &[u8],
) -> (StdVec<(StdVec<u8>, usize)>, StdVec<usize>) {
    let mut positions = StdVec::new();
    assert_eq!(blocks.iter().map(|b| b.len()).sum::<usize>(), data.len());
    let mut src = data.to_vec();
    if let Some(size) = e8 {
        for (k, chunk) in src.chunks_mut(FRAME).enumerate() {
            e8_forward(chunk, (k * FRAME) as u32, size);
        }
    }
    let wsize = 1usize << window;
    let slots = position_slots(window);
    let mut w = Writer { out: StdVec::new(), acc: 0, n: 0, cuts: StdVec::new() };
    match e8 {
        Some(size) => {
            w.bits(1, 1);
            w.bits(size >> 16, 16);
            w.bits(size & 0xffff, 16);
        }
        None => w.bits(0, 1),
    }
    let mut r = [1u32; 3];
    let mut chains: std::collections::HashMap<&[u8], StdVec<usize>> =
        std::collections::HashMap::new();
    let mut prev_main = vec![0u8; 256 + 8 * slots];
    let mut prev_len = vec![0u8; LENGTH_TREE];
    let mut pos = 0usize;
    let cut = |w: &mut Writer, pos: usize| {
        if pos.is_multiple_of(FRAME) {
            w.align();
            w.cuts.push(w.out.len());
        }
    };
    for &b in blocks {
        let n = b.len();
        let end = pos + n;
        positions.push(w.position());
        let ty = match b {
            Block::Verbatim(_) => 1,
            Block::Aligned(_) => 2,
            Block::Uncompressed(_) => 3,
        };
        w.bits(ty, 3);
        w.bits(n as u32, 24);
        if let Block::Uncompressed(_) = b {
            // 1 to 16 bits of padding, then bytes.
            let pad = if w.n == 0 { 16 } else { 16 - w.n };
            w.bits(0, pad);
            for v in r {
                w.out.extend_from_slice(&v.to_le_bytes());
            }
            for (i, &byte) in src.iter().enumerate().take(end).skip(pos) {
                w.out.push(byte);
                if i + 1 == end && !n.is_multiple_of(2) {
                    w.out.push(0);
                }
                cut(&mut w, i + 1);
            }
            pos = end;
            continue;
        }
        let aligned = matches!(b, Block::Aligned(_));
        // Tokens, greedy, never across a frame or the block's end.
        let mut tokens = StdVec::new();
        let mut p = pos;
        while p < end {
            let limit = (end - p).min(FRAME - p % FRAME).min(257);
            let mut best = (0usize, 0u32);
            if limit >= 2 {
                let mut cands: StdVec<u32> = r.to_vec();
                if p + 3 <= src.len()
                    && let Some(list) = chains.get(&src[p..p + 3])
                {
                    cands.extend(list.iter().rev().map(|&q| (p - q) as u32));
                }
                for &o in &cands {
                    let o = o as usize;
                    if o == 0 || o > p || o > wsize - 3 {
                        continue;
                    }
                    let l = (0..limit).take_while(|&j| src[p + j] == src[p + j - o]).count();
                    // The first longest: a repeated offset at equal
                    // length (they come first).
                    if l > best.0 {
                        best = (l, o as u32);
                    }
                }
            }
            if best.0 >= 2 {
                let (len, off) = best;
                let formatted = if off == r[0] {
                    0
                } else if off == r[1] {
                    r.swap(0, 1);
                    1
                } else if off == r[2] {
                    r.swap(0, 2);
                    2
                } else {
                    r = [off, r[0], r[1]];
                    off + 2
                };
                let slot = slot_of(formatted);
                let footer = formatted - base_position(slot);
                let (header, len_footer) =
                    if len <= 8 { (len - 2, None) } else { (7, Some(len - 9)) };
                tokens.push(Token::Match {
                    main: 256 + (slot << 3) + header,
                    len_footer,
                    slot,
                    footer,
                });
                for q in p..p + len {
                    remember(&mut chains, &src, q);
                }
                p += len;
            } else {
                tokens.push(Token::Literal(src[p]));
                remember(&mut chains, &src, p);
                p += 1;
            }
        }
        // Trees.
        let mut fm = vec![0u32; 256 + 8 * slots];
        let mut fl = vec![0u32; LENGTH_TREE];
        let mut fa = [0u32; 8];
        for t in &tokens {
            match *t {
                Token::Literal(c) => fm[usize::from(c)] += 1,
                Token::Match { main, len_footer, slot, footer } => {
                    fm[main] += 1;
                    if let Some(l) = len_footer {
                        fl[l] += 1;
                    }
                    if aligned && slot >= 4 && footer_bits(slot) >= 3 {
                        fa[(footer & 7) as usize] += 1;
                    }
                }
            }
        }
        let lm = lengths(&fm, 16);
        let ll = lengths(&fl, 16);
        let la = lengths(&fa, 7);
        if aligned {
            for &l in &la {
                w.bits(u32::from(l), 3);
            }
        }
        write_lengths(&mut w, &lm[..256], &prev_main[..256]);
        write_lengths(&mut w, &lm[256..], &prev_main[256..]);
        write_lengths(&mut w, &ll, &prev_len);
        prev_main = lm.clone();
        prev_len = ll.clone();
        let (cm, cl, ca) = (codes(&lm), codes(&ll), codes(&la));
        for t in tokens {
            match t {
                Token::Literal(c) => {
                    let c = usize::from(c);
                    w.bits(cm[c], u32::from(lm[c]));
                    pos += 1;
                }
                Token::Match { main, len_footer, slot, footer } => {
                    w.bits(cm[main], u32::from(lm[main]));
                    let mut len = (main - 256) % 8 + 2;
                    if let Some(l) = len_footer {
                        w.bits(cl[l], u32::from(ll[l]));
                        len = l + 9;
                    }
                    let extra = footer_bits(slot);
                    if aligned && extra >= 3 {
                        w.bits(footer >> 3, extra - 3);
                        let a = (footer & 7) as usize;
                        w.bits(ca[a], u32::from(la[a]));
                    } else {
                        w.bits(footer, extra);
                    }
                    pos += len;
                }
            }
            cut(&mut w, pos);
        }
    }
    if !pos.is_multiple_of(FRAME) {
        w.align();
        w.cuts.push(w.out.len());
    }
    let mut frames = StdVec::new();
    let mut start = 0;
    for (k, &c) in w.cuts.iter().enumerate() {
        let size = (data.len() - k * FRAME).min(FRAME);
        frames.push((w.out[start..c].to_vec(), size));
        start = c;
    }
    (frames, positions)
}
