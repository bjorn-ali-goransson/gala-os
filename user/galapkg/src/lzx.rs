//! LZX decompression for cabinets, written from Microsoft's published
//! descriptions of the format: the Cabinet SDK's "Microsoft LZX Data
//! Compression Format" (learn.microsoft.com, previous-versions
//! bb417343: window sizes 2^15 to 2^21, the E8 preprocessing header and
//! its per-frame rule, 32 KiB frames that each CFDATA block holds
//! exactly, MAX_MATCH 257) and [MS-PATCH] "LZX DELTA Compression and
//! Decompression" (the bitstream of little-endian 16-bit words, the
//! block header, the three block types, the pretree encoding of path
//! lengths as deltas mod 17 with runs 17/18/19, the position slots and
//! footer bits, the aligned offset tree, the repeated offsets R0..R2).
//! Cabinet LZX is the DELTA format without reference data, chunk-size
//! prefixes or the extra length field.
//!
//! Where the two documents disagree, a cabinet Microsoft made (the
//! DirectX web installer's, testdata/lzx/fetch_real.sh) and cabextract
//! decided: aligned offset bits are used from 3 footer bits on
//! ([MS-PATCH] 2.6.3's "formatted offset above 15", not its decoding
//! pseudocode's "more than 3"), and E8 bytes are translated in a frame's
//! first `len - 10` bytes ([MS-PATCH] 2.2.2, not the SDK's "last 6").
//!
//! A folder is decoded frame by frame ([`Decoder::frame`]): each CFDATA
//! block's bytes give exactly its uncompressed size; LZX blocks, the
//! trees and the repeated offsets carry over from frame to frame, the
//! bitstream restarts at each frame (the encoder pads to 16 bits there).
//!
//! Untrusted input: every code is checked (an over-subscribed tree, a
//! code no element has, a run past a tree's end, a match before the
//! start of the data or past the end of its frame or block, a block type
//! other than 1, 2, 3) and the error names the frame and the output
//! position. Reading past the end of a frame's bytes gives zero bits for
//! at most 32 bits (codes near the end), then is an error.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// Why LZX data could not be decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The data is not valid LZX: what and where.
    Corrupt(String),
    /// A buffer of this many bytes could not be allocated.
    OutOfMemory(u64),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Corrupt(what) => write!(f, "LZX data is damaged: {what}"),
            Error::OutOfMemory(n) => write!(f, "out of memory decoding LZX ({n} bytes)"),
        }
    }
}

macro_rules! corrupt {
    ($($t:tt)*) => { Error::Corrupt(alloc::format!($($t)*)) };
}

/// A frame: the uncompressed size of each CFDATA block but the last.
pub const FRAME: usize = 32768;
const NUM_CHARS: usize = 256;
const PRETREE: usize = 20;
const LENGTH_TREE: usize = 249;
const ALIGNED_TREE: usize = 8;
const MAX_CODE: usize = 16;
/// Zero bits that may be read past a frame's end (Huffman peeks).
const OVERREAD_BITS: u32 = 32;

/// The footer bits of position slot `s` ([MS-PATCH] 2.6.2).
pub fn footer_bits(s: usize) -> u32 {
    if s < 4 {
        0
    } else if s < 36 {
        (s as u32 - 2) / 2
    } else {
        17
    }
}

/// The base position of slot `s`.
pub fn base_position(s: usize) -> u32 {
    let mut base = 0u32;
    for i in 0..s {
        base += 1 << footer_bits(i);
    }
    base
}

/// The number of position slots for a window of 2^`bits` bytes: the
/// slots whose base positions fall inside the window (30 for 2^15 ...
/// 50 for 2^21, as the format's tables list).
pub fn position_slots(bits: u8) -> usize {
    let size = 1u32 << bits;
    (0..400).take_while(|&s| base_position(s) < size).count()
}

/// Reads the bitstream: 16-bit little-endian words, bits taken from the
/// most significant end of each.
struct Bits<'a> {
    data: &'a [u8],
    /// The next byte to load.
    at: usize,
    buf: u32,
    /// Valid bits in `buf` (the high ones).
    n: u32,
    /// Zero bits supplied past the end.
    over: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Bits<'a> {
        Bits { data, at: 0, buf: 0, n: 0, over: 0 }
    }

    fn fill(&mut self) {
        while self.n <= 16 {
            let w = match self.data.get(self.at..self.at + 2) {
                Some(b) => u32::from(u16::from_le_bytes([b[0], b[1]])),
                None => {
                    self.over += 16;
                    0
                }
            };
            self.at += 2;
            self.buf |= w << (16 - self.n);
            self.n += 16;
        }
    }

    fn peek(&mut self, k: u32) -> u32 {
        if self.n < k {
            self.fill();
        }
        if k == 0 { 0 } else { self.buf >> (32 - k) }
    }

    fn skip(&mut self, k: u32) {
        self.buf = if k >= 32 { 0 } else { self.buf << k };
        self.n -= k;
    }

    fn read(&mut self, k: u32) -> u32 {
        // Callers read at most 17 bits at once.
        let v = self.peek(k);
        self.skip(k);
        v
    }

    /// Whether more zero bits were supplied than a valid stream needs.
    fn overrun(&self) -> bool {
        self.over.saturating_sub(self.n) > OVERREAD_BITS
    }

    /// Aligns for an uncompressed block (1 to 16 bits of padding) and
    /// switches to bytes: returns the byte offset that follows.
    fn align_to_bytes(&mut self) -> usize {
        if self.n == 0 {
            self.fill();
        }
        let drop = if self.n.is_multiple_of(16) { 16 } else { self.n % 16 };
        self.skip(drop);
        // Give back the whole words still buffered.
        let back = (self.n / 16) as usize * 2;
        let pos = self.at - back;
        self.buf = 0;
        self.n = 0;
        pos
    }

    /// Resumes the bitstream at byte `at`.
    fn resume(&mut self, at: usize) {
        self.at = at;
        self.buf = 0;
        self.n = 0;
    }
}

/// A canonical Huffman code over path lengths ([MS-PATCH] 2.4).
#[derive(Clone, Default)]
struct Tree {
    /// Elements by code length, then element number.
    symbols: Vec<u16>,
    /// The number of codes of each length 0..=16.
    count: [u16; MAX_CODE + 1],
}

impl Tree {
    fn build(lens: &[u8], what: &str) -> Result<Tree, Error> {
        let mut count = [0u16; MAX_CODE + 1];
        for &l in lens {
            count[usize::from(l)] += 1;
        }
        count[0] = 0;
        // Over-subscribed (Kraft sum above 1) is invalid; incomplete is
        // allowed and a code no element has is an error when read.
        let mut left: i64 = 1;
        for &c in &count[1..] {
            left = left * 2 - i64::from(c);
            if left < 0 {
                return Err(corrupt!("the {what}'s path lengths are over-subscribed"));
            }
        }
        let mut symbols = Vec::new();
        symbols.try_reserve_exact(lens.len()).map_err(|_| Error::OutOfMemory(lens.len() as u64))?;
        for len in 1..=MAX_CODE as u8 {
            for (s, &l) in lens.iter().enumerate() {
                if l == len {
                    symbols.push(s as u16);
                }
            }
        }
        Ok(Tree { symbols, count })
    }

    fn decode(&self, bits: &mut Bits, what: &str) -> Result<usize, Error> {
        let window = bits.peek(MAX_CODE as u32);
        let (mut code, mut first, mut index) = (0u32, 0u32, 0usize);
        for len in 1..=MAX_CODE {
            code |= (window >> (MAX_CODE - len)) & 1;
            let c = u32::from(self.count[len]);
            if code < first + c {
                bits.skip(len as u32);
                return Ok(usize::from(self.symbols[index + (code - first) as usize]));
            }
            index += c as usize;
            first = (first + c) << 1;
            code <<= 1;
        }
        Err(corrupt!("a code no element of the {what} has"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Verbatim,
    Aligned,
    Uncompressed,
}

/// The state of one folder's decompression.
pub struct Decoder {
    window_bits: u8,
    /// The last 2^window_bits bytes decoded (before E8 reversal), as a
    /// ring.
    window: Vec<u8>,
    /// Bytes decoded so far.
    pos: u64,
    frames: u32,
    r: [u32; 3],
    /// None until the stream header is read; then the E8 file size
    /// (0: no translation).
    e8: Option<u32>,
    main_lens: Vec<u8>,
    length_lens: [u8; LENGTH_TREE],
    main: Tree,
    length: Tree,
    aligned: Tree,
    kind: Option<Kind>,
    /// The current LZX block's size and the bytes left in it.
    block_size: u32,
    remaining: u32,
}

impl Decoder {
    /// A decoder for a folder with a window of 2^`window_bits` bytes
    /// (the folder's typeCompress, bits 8-12: 15 to 21).
    pub fn new(window_bits: u8) -> Result<Decoder, Error> {
        if !(15..=21).contains(&window_bits) {
            return Err(corrupt!("window size 2^{window_bits} (cabinets allow 2^15 to 2^21)"));
        }
        let size = 1usize << window_bits;
        let mut window = Vec::new();
        window.try_reserve_exact(size).map_err(|_| Error::OutOfMemory(size as u64))?;
        window.resize(size, 0);
        let main = NUM_CHARS + 8 * position_slots(window_bits);
        let mut main_lens = Vec::new();
        main_lens.try_reserve_exact(main).map_err(|_| Error::OutOfMemory(main as u64))?;
        main_lens.resize(main, 0);
        Ok(Decoder {
            window_bits,
            window,
            pos: 0,
            frames: 0,
            r: [1, 1, 1],
            e8: None,
            main_lens,
            length_lens: [0; LENGTH_TREE],
            main: Tree::default(),
            length: Tree::default(),
            aligned: Tree::default(),
            kind: None,
            block_size: 0,
            remaining: 0,
        })
    }

    /// The window size in bytes.
    pub fn window_size(&self) -> usize {
        1 << self.window_bits
    }

    /// The E8 translation size the stream header gave (None before the
    /// first frame, 0 when translation is off).
    pub fn e8_size(&self) -> Option<u32> {
        self.e8
    }

    /// Decodes one frame (a CFDATA block's `input`, giving `size` bytes,
    /// at most 32768) and appends it to `out`.
    pub fn frame(&mut self, input: &[u8], size: usize, out: &mut Vec<u8>) -> Result<(), Error> {
        let (frame, pos) = (self.frames, self.pos);
        self.frame_inner(input, size, out).map_err(|e| match e {
            Error::Corrupt(what) if !what.starts_with("frame ") => {
                corrupt!("frame {frame} (output position {}, from {pos}): {what}", self.pos)
            }
            e => e,
        })
    }

    fn frame_inner(&mut self, input: &[u8], size: usize, out: &mut Vec<u8>) -> Result<(), Error> {
        let frame = self.frames;
        if size > FRAME {
            return Err(corrupt!("frame {frame}: {size} bytes (most {FRAME})"));
        }
        out.try_reserve(size).map_err(|_| Error::OutOfMemory(size as u64))?;
        let start = out.len();
        let frame_pos = self.pos;
        let mut bits = Bits::new(input);
        if self.e8.is_none() {
            self.e8 = Some(if bits.read(1) == 1 {
                let hi = bits.read(16);
                let lo = bits.read(16);
                (hi << 16) | lo
            } else {
                0
            });
        }
        // An uncompressed block continues at the frame's first byte.
        let mut raw: Option<usize> =
            (self.remaining > 0 && self.kind == Some(Kind::Uncompressed)).then_some(0);
        let mut left = size;
        while left > 0 {
            if bits.overrun() {
                return Err(corrupt!(
                    "frame {frame}: the bitstream runs past the frame's {} bytes (output \
                     position {})",
                    input.len(),
                    self.pos
                ));
            }
            if self.remaining == 0 {
                raw = self.block_header(&mut bits, input, frame)?;
                continue;
            }
            let n = (self.remaining as usize).min(left);
            match self.kind {
                Some(Kind::Uncompressed) => {
                    let at = raw.unwrap_or(0);
                    let Some(src) = input.get(at..at + n) else {
                        return Err(corrupt!(
                            "frame {frame}: an uncompressed block needs {n} bytes at offset \
                             {at}, the frame has {}",
                            input.len()
                        ));
                    };
                    for &b in src {
                        self.put(b);
                    }
                    out.extend_from_slice(src);
                    self.remaining -= n as u32;
                    left -= n;
                    let mut next = at + n;
                    if self.remaining == 0 {
                        // Odd-sized: one byte realigns the bitstream.
                        if !self.block_size.is_multiple_of(2) {
                            next += 1;
                        }
                        bits.resume(next);
                        raw = None;
                    } else {
                        raw = Some(next);
                    }
                }
                Some(kind) => {
                    self.tokens(&mut bits, kind, n, frame, out)?;
                    self.remaining -= n as u32;
                    left -= n;
                }
                None => return Err(corrupt!("frame {frame}: data before any block header")),
            }
        }
        self.frames += 1;
        let e8 = self.e8.unwrap_or(0);
        if e8 != 0 && frame < 32768 {
            translate_e8(&mut out[start..], frame_pos as u32, e8);
        }
        Ok(())
    }

    fn put(&mut self, b: u8) {
        let mask = self.window.len() - 1;
        self.window[self.pos as usize & mask] = b;
        self.pos += 1;
    }

    /// Reads a block header and its trees; for an uncompressed block,
    /// returns the offset of its data in `input`.
    fn block_header(
        &mut self,
        bits: &mut Bits,
        input: &[u8],
        frame: u32,
    ) -> Result<Option<usize>, Error> {
        let ty = bits.read(3);
        let size = (bits.read(8) << 16) | (bits.read(8) << 8) | bits.read(8);
        let at = self.pos;
        let kind = match ty {
            1 => Kind::Verbatim,
            2 => Kind::Aligned,
            3 => Kind::Uncompressed,
            t => {
                return Err(corrupt!(
                    "frame {frame} (output position {at}): block type {t} (valid: 1 verbatim, \
                     2 aligned offset, 3 uncompressed)"
                ));
            }
        };
        self.kind = Some(kind);
        self.block_size = size;
        self.remaining = size;
        match kind {
            Kind::Uncompressed => {
                let p = bits.align_to_bytes();
                let Some(r) = input.get(p..p + 12) else {
                    return Err(corrupt!(
                        "frame {frame} (output position {at}): an uncompressed block's R0-R2 \
                         at offset {p} are past the frame's {} bytes",
                        input.len()
                    ));
                };
                for (i, w) in r.chunks_exact(4).enumerate() {
                    self.r[i] = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
                }
                if size == 0 {
                    bits.resume(p + 12);
                    return Ok(None);
                }
                Ok(Some(p + 12))
            }
            Kind::Verbatim | Kind::Aligned => {
                if kind == Kind::Aligned {
                    let mut lens = [0u8; ALIGNED_TREE];
                    for l in &mut lens {
                        *l = bits.read(3) as u8;
                    }
                    self.aligned = Tree::build(&lens, "aligned offset tree")?;
                }
                let mut main = core::mem::take(&mut self.main_lens);
                let r = read_lengths(bits, &mut main[..NUM_CHARS], frame, "main (literals)")
                    .and_then(|()| {
                        read_lengths(bits, &mut main[NUM_CHARS..], frame, "main (matches)")
                    });
                self.main_lens = main;
                r?;
                read_lengths(bits, &mut self.length_lens, frame, "length")?;
                self.main = Tree::build(&self.main_lens, "main tree")?;
                self.length = Tree::build(&self.length_lens, "length tree")?;
                Ok(None)
            }
        }
    }

    /// Decodes literals and matches giving exactly `n` bytes.
    fn tokens(
        &mut self,
        bits: &mut Bits,
        kind: Kind,
        n: usize,
        frame: u32,
        out: &mut Vec<u8>,
    ) -> Result<(), Error> {
        let mut done = 0usize;
        let mask = self.window.len() - 1;
        while done < n {
            if bits.overrun() {
                return Err(corrupt!(
                    "frame {frame}: the bitstream runs past the frame's end (output position {})",
                    self.pos
                ));
            }
            let e = self.main.decode(bits, "main tree")?;
            if e < NUM_CHARS {
                self.put(e as u8);
                out.push(e as u8);
                done += 1;
                continue;
            }
            let header = (e - NUM_CHARS) & 7;
            let len = if header == 7 {
                self.length.decode(bits, "length tree")? + 7 + 2
            } else {
                header + 2
            };
            let slot = (e - NUM_CHARS) >> 3;
            let offset = match slot {
                0 => self.r[0],
                1 => {
                    self.r.swap(0, 1);
                    self.r[0]
                }
                2 => {
                    self.r.swap(0, 2);
                    self.r[0]
                }
                _ => {
                    let extra = footer_bits(slot);
                    // Aligned bits when the formatted offset is above 15
                    // ([MS-PATCH] 2.6.3), i.e. from slot 8 (3 footer bits)
                    // on; the decoding pseudocode's "extra > 3" disagrees
                    // with that section, and Microsoft's cabinets follow
                    // 2.6.3 (the fetched cabinet's first aligned block).
                    let footer = if kind == Kind::Aligned && extra >= 3 {
                        let verbatim = bits.read(extra - 3) << 3;
                        verbatim + self.aligned.decode(bits, "aligned offset tree")? as u32
                    } else {
                        bits.read(extra)
                    };
                    let o = base_position(slot) + footer - 2;
                    self.r = [o, self.r[0], self.r[1]];
                    o
                }
            };
            let at = self.pos;
            if offset == 0 || u64::from(offset) > at || offset as usize > self.window.len() - 3 {
                return Err(corrupt!(
                    "frame {frame} (output position {at}): a match at offset {offset} (slot \
                     {slot}), {at} bytes decoded so far, window {}",
                    self.window.len()
                ));
            }
            if done + len > n {
                return Err(corrupt!(
                    "frame {frame} (output position {at}): a match of {len} bytes runs past the \
                     end of its frame or block ({} bytes left)",
                    n - done
                ));
            }
            for _ in 0..len {
                let b = self.window[(self.pos - u64::from(offset)) as usize & mask];
                self.put(b);
                out.push(b);
            }
            done += len;
        }
        Ok(())
    }
}

/// Reads a tree's path lengths through a pretree ([MS-PATCH] 2.5): each
/// as a delta from the previous block's length mod 17, or a run.
fn read_lengths(bits: &mut Bits, lens: &mut [u8], frame: u32, what: &str) -> Result<(), Error> {
    let mut pre = [0u8; PRETREE];
    for l in &mut pre {
        *l = bits.read(4) as u8;
    }
    let pretree = Tree::build(&pre, "pretree")?;
    let mut i = 0;
    while i < lens.len() {
        let code = pretree.decode(bits, "pretree")?;
        let (count, value) = match code {
            0..=16 => (1, (17 + lens[i] - code as u8) % 17),
            17 => (4 + bits.read(4) as usize, 0),
            18 => (20 + bits.read(5) as usize, 0),
            _ => {
                let count = 4 + bits.read(1) as usize;
                let code = pretree.decode(bits, "pretree")?;
                if code > 16 {
                    return Err(corrupt!(
                        "frame {frame}: the {what} tree's run of equal lengths gives code {code}"
                    ));
                }
                (count, (17 + lens[i] - code as u8) % 17)
            }
        };
        if i + count > lens.len() {
            return Err(corrupt!(
                "frame {frame}: a run of {count} path lengths at element {i} passes the {what} \
                 tree's {} elements",
                lens.len()
            ));
        }
        lens[i..i + count].fill(value);
        i += count;
        if bits.overrun() {
            return Err(corrupt!("frame {frame}: the {what} tree runs past the frame's end"));
        }
    }
    Ok(())
}

/// Reverses the E8 preprocessing on one frame decoded at folder offset
/// `frame_pos` (the 32-bit value after each E8 byte was made absolute).
/// E8 bytes are looked for in the frame's first `len - 10` bytes, as
/// [MS-PATCH] 2.2.2 writes it (nothing in a frame of 10 bytes or
/// fewer); the Cabinet SDK's text says the last 6 bytes, but
/// Microsoft's cabinets leave an E8 at `len - 10 .. len - 6` alone (the
/// fetched cabinet's frame 0 has one at 32759).
pub fn translate_e8(data: &mut [u8], frame_pos: u32, file_size: u32) {
    if data.len() <= 10 {
        return;
    }
    let mut i = 0;
    let end = data.len() - 10;
    while i < end {
        if data[i] != 0xe8 {
            i += 1;
            continue;
        }
        let cur = frame_pos.wrapping_add(i as u32) as i32;
        let v = i32::from_le_bytes([data[i + 1], data[i + 2], data[i + 3], data[i + 4]]);
        if v >= cur.wrapping_neg() && v < file_size as i32 {
            let rel = if v >= 0 { v.wrapping_sub(cur) } else { v.wrapping_add(file_size as i32) };
            data[i + 1..i + 5].copy_from_slice(&rel.to_le_bytes());
        }
        i += 5;
    }
}

#[cfg(test)]
pub(crate) mod encode;
#[cfg(test)]
pub(crate) mod tests;
