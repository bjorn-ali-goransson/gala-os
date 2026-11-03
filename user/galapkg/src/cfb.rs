//! Reading Compound File Binary files (OLE structured storage), written
//! from Microsoft's published specification [MS-CFB]: the header, the
//! DIFAT, FAT and mini FAT sector chains, the directory (a tree of
//! storages and streams, each storage's children a red-black tree of
//! siblings), regular streams and the mini stream for small ones.
//! Versions 3 (512-byte sectors) and 4 (4096-byte sectors).
//!
//! MSI packages are compound files (see [`crate::msi`]).
//!
//! Untrusted input: every offset, sector number, chain and tree link is
//! checked; a malformed file is an [`Error`] saying what was wrong and
//! where (the byte offset, the sector, the directory entry or stream),
//! never a panic, an endless loop or an allocation larger than the file
//! justifies. Chains are bounded by the number of sectors the file has,
//! the directory tree by its entry count (an entry reached twice is an
//! error). Buffers are reserved with `try_reserve`, so a huge stream in
//! a short-of-memory process is an error too.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// Why a compound file could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Not a compound file, or its structures are cut short or
    /// inconsistent: what and where.
    Corrupt(String),
    /// A valid feature this reader does not have.
    Unsupported(String),
    /// A buffer of this many bytes could not be allocated.
    OutOfMemory(u64),
    /// No stream or storage of this name.
    NotFound(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Corrupt(what) => write!(f, "not a valid compound file: {what}"),
            Error::Unsupported(what) => write!(f, "compound file feature not supported: {what}"),
            Error::OutOfMemory(n) => write!(f, "out of memory reading a compound file ({n} bytes)"),
            Error::NotFound(name) => write!(f, "compound file has no entry {name}"),
        }
    }
}

macro_rules! corrupt {
    ($($t:tt)*) => { Error::Corrupt(alloc::format!($($t)*)) };
}

/// The signature at offset 0.
pub const SIGNATURE: [u8; 8] = [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1];
/// Largest regular sector number.
const MAXREGSECT: u32 = 0xffff_fffa;
/// A FAT entry marking a DIFAT sector.
const DIFSECT: u32 = 0xffff_fffc;
/// A FAT entry marking a FAT sector.
const FATSECT: u32 = 0xffff_fffd;
/// The end of a chain.
const ENDOFCHAIN: u32 = 0xffff_fffe;
/// An unused sector (and the "no entry" value of directory links).
const FREESECT: u32 = 0xffff_ffff;
/// "No sibling/child" in a directory entry.
const NOSTREAM: u32 = 0xffff_ffff;
/// The first 109 DIFAT entries live in the header.
const HEADER_DIFAT: usize = 109;

/// What a directory entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Storage,
    Stream,
    Root,
}

/// A directory entry reachable from the root.
#[derive(Clone, Debug)]
pub struct Entry {
    /// The name as stored (UTF-16 code units, without the terminator).
    pub name: Vec<u16>,
    pub kind: Kind,
    /// The containing storage's entry index (None for the root).
    pub parent: Option<usize>,
    /// The stream's size in bytes (for the root: the mini stream's).
    pub size: u64,
    /// The first sector (regular or mini, by size) of the stream.
    pub start: u32,
    /// The children of a storage, in tree (sorted) order.
    pub children: Vec<usize>,
    /// The entry's index in the directory (its stream ID).
    pub id: u32,
}

impl Entry {
    /// The name as text (unpaired surrogates become U+FFFD).
    pub fn name_lossy(&self) -> String {
        char::decode_utf16(self.name.iter().copied())
            .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect()
    }
}

/// A compound file held in memory.
pub struct File<'a> {
    data: &'a [u8],
    /// 3 or 4.
    pub version: u16,
    sector_shift: u32,
    mini_cutoff: u32,
    fat: Vec<u32>,
    minifat: Vec<u32>,
    /// The mini stream (the root entry's stream), read whole.
    mini_stream: Vec<u8>,
    /// Entries reachable from the root; index 0 is the root.
    entries: Vec<Entry>,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    let s = b.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from(u32_at(b, at)?) | (u64::from(u32_at(b, at + 4)?) << 32))
}

/// A vector with room for `n` elements, or [`Error::OutOfMemory`].
pub(crate) fn try_vec<T>(n: usize) -> Result<Vec<T>, Error> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| Error::OutOfMemory(n as u64))?;
    Ok(v)
}

/// A set of sector numbers, to find chains that loop.
struct Bits(Vec<u64>);

impl Bits {
    fn new(n: usize) -> Result<Bits, Error> {
        let mut v = try_vec::<u64>(n.div_ceil(64))?;
        v.resize(n.div_ceil(64), 0);
        Ok(Bits(v))
    }

    /// Adds `i` (below the size given to `new`); false if it was there.
    fn insert(&mut self, i: usize) -> bool {
        let (w, b) = (i / 64, 1u64 << (i % 64));
        let was = self.0[w] & b != 0;
        self.0[w] |= b;
        !was
    }
}

fn sector_name(s: u32) -> String {
    match s {
        DIFSECT => "DIFSECT".into(),
        FATSECT => "FATSECT".into(),
        ENDOFCHAIN => "ENDOFCHAIN".into(),
        FREESECT => "FREESECT".into(),
        n => alloc::format!("{n:#x}"),
    }
}

impl<'a> File<'a> {
    /// Reads the header, the FAT, the mini FAT, the directory and the
    /// mini stream of `data`.
    pub fn open(data: &'a [u8]) -> Result<File<'a>, Error> {
        if data.len() < 512 {
            return Err(corrupt!("{} bytes, shorter than the 512-byte header", data.len()));
        }
        if data[..8] != SIGNATURE {
            return Err(corrupt!("no signature at offset 0"));
        }
        let h = |at| u16_at(data, at).unwrap_or(0);
        let w = |at| u32_at(data, at).unwrap_or(0);
        let version = h(26);
        if h(28) != 0xfffe {
            return Err(corrupt!("byte order mark {:#x} at offset 28 (want 0xfffe)", h(28)));
        }
        let sector_shift = u32::from(h(30));
        match (version, sector_shift) {
            (3, 9) | (4, 12) => {}
            (3 | 4, s) => {
                return Err(corrupt!("version {version} with sector shift {s} (offset 30)"));
            }
            (v, _) => return Err(Error::Unsupported(alloc::format!("major version {v}"))),
        }
        if h(32) != 6 {
            return Err(corrupt!("mini sector shift {} at offset 32 (want 6)", h(32)));
        }
        let mini_cutoff = w(56);
        if mini_cutoff != 4096 {
            return Err(corrupt!("mini stream cutoff {mini_cutoff} at offset 56 (want 4096)"));
        }
        let sector_size = 1usize << sector_shift;
        // Sectors the file has room for (the header takes the first
        // sector's place): every chain is bounded by this.
        let sector_count = (data.len() - 512).div_ceil(sector_size).max(1);
        let mut file = File {
            data,
            version,
            sector_shift,
            mini_cutoff,
            fat: Vec::new(),
            minifat: Vec::new(),
            mini_stream: Vec::new(),
            entries: Vec::new(),
        };

        // The DIFAT: 109 entries in the header, then a chain of DIFAT
        // sectors, each ending with the next one's number.
        let fat_sectors = w(44) as usize;
        if fat_sectors > sector_count {
            return Err(corrupt!(
                "{fat_sectors} FAT sectors (offset 44) in a file of {sector_count} sectors"
            ));
        }
        let mut difat = try_vec::<u32>(fat_sectors)?;
        for i in 0..HEADER_DIFAT.min(fat_sectors) {
            difat.push(w(76 + 4 * i));
        }
        let per_sector = sector_size / 4;
        let mut next = w(68);
        let mut seen = 0usize;
        while difat.len() < fat_sectors {
            if next == ENDOFCHAIN || next == FREESECT {
                return Err(corrupt!(
                    "the DIFAT chain ends after {} of {fat_sectors} FAT sector numbers",
                    difat.len()
                ));
            }
            seen += 1;
            if seen > sector_count {
                return Err(corrupt!("the DIFAT chain loops (at sector {next:#x})"));
            }
            let sector = file.sector(next).map_err(|e| corrupt!("DIFAT sector: {e}"))?;
            for i in 0..per_sector - 1 {
                if difat.len() == fat_sectors {
                    break;
                }
                difat.push(u32_at(sector, 4 * i).unwrap_or(FREESECT));
            }
            next = u32_at(sector, sector_size - 4).unwrap_or(ENDOFCHAIN);
        }

        // The FAT.
        let mut fat = try_vec::<u32>(fat_sectors * per_sector)?;
        for (i, &s) in difat.iter().enumerate() {
            let sector =
                file.sector(s).map_err(|e| corrupt!("FAT sector {i} (DIFAT entry {i}): {e}"))?;
            fat.extend(
                sector.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])),
            );
        }
        file.fat = fat;

        // The directory.
        let dir =
            file.read_chain(w(48), None, "the directory").map_err(|e| prefix(e, "directory: "))?;
        file.entries = read_directory(&dir, version)?;

        // The mini FAT and the mini stream.
        let minifat_start = w(60);
        let minifat_sectors = w(64) as usize;
        if minifat_sectors > 0 && minifat_start != ENDOFCHAIN {
            let bytes = file.read_chain(minifat_start, None, "the mini FAT")?;
            let mut minifat = try_vec::<u32>(bytes.len() / 4)?;
            minifat.extend(
                bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])),
            );
            file.minifat = minifat;
        }
        let root = &file.entries[0];
        if root.size > 0 {
            let (start, size) = (root.start, root.size);
            file.mini_stream = file.read_chain(start, Some(size), "the mini stream")?;
        }
        Ok(file)
    }

    /// The bytes of sector `n`; the last sector of a file may be short.
    fn sector(&self, n: u32) -> Result<&'a [u8], Error> {
        if n > MAXREGSECT {
            return Err(corrupt!("sector number {}", sector_name(n)));
        }
        let size = 1usize << self.sector_shift;
        let at = (n as usize + 1).checked_mul(size).filter(|&at| at < self.data.len());
        let Some(at) = at else {
            return Err(corrupt!(
                "sector {n:#x} lies past the end of the file ({} bytes)",
                self.data.len()
            ));
        };
        Ok(&self.data[at..(at + size).min(self.data.len())])
    }

    /// Follows a FAT chain from `start`, reading `size` bytes (or the whole
    /// chain). `what` names the stream in errors.
    fn read_chain(&self, start: u32, size: Option<u64>, what: &str) -> Result<Vec<u8>, Error> {
        let sector_size = 1usize << self.sector_shift;
        let limit = self.fat.len().min(self.data.len() / sector_size + 1);
        let want = match size {
            Some(s) => {
                if s > self.data.len() as u64 {
                    return Err(corrupt!(
                        "{what}: size {s} is larger than the file ({} bytes)",
                        self.data.len()
                    ));
                }
                s as usize
            }
            None => limit * sector_size,
        };
        let mut out = try_vec::<u8>(want)?;
        let mut seen = Bits::new(self.fat.len())?;
        let mut s = start;
        while out.len() < want {
            if s == ENDOFCHAIN && size.is_none() {
                break;
            }
            if s > MAXREGSECT || s as usize >= self.fat.len() {
                return Err(corrupt!(
                    "{what}: chain from sector {start:#x} reaches {} after {} of {want} bytes",
                    sector_name(s),
                    out.len()
                ));
            }
            if !seen.insert(s as usize) {
                return Err(corrupt!("{what}: chain from sector {start:#x} loops (at {s:#x})"));
            }
            let bytes = self.sector(s).map_err(|e| prefix(e, &alloc::format!("{what}: ")))?;
            let take = (want - out.len()).min(bytes.len());
            out.extend_from_slice(&bytes[..take]);
            if bytes.len() < sector_size && out.len() < want {
                return Err(corrupt!("{what}: the file ends inside sector {s:#x}"));
            }
            s = self.fat[s as usize];
        }
        Ok(out)
    }

    /// Follows a mini FAT chain from `start` for `size` bytes.
    fn read_mini(&self, start: u32, size: u64, what: &str) -> Result<Vec<u8>, Error> {
        if size > self.mini_stream.len() as u64 {
            return Err(corrupt!(
                "{what}: size {size} is larger than the mini stream ({} bytes)",
                self.mini_stream.len()
            ));
        }
        let want = size as usize;
        let mut out = try_vec::<u8>(want)?;
        let mut seen = Bits::new(self.minifat.len())?;
        let mut s = start;
        while out.len() < want {
            if s as usize >= self.minifat.len() {
                return Err(corrupt!(
                    "{what}: mini chain from {start:#x} reaches {} after {} of {want} bytes",
                    sector_name(s),
                    out.len()
                ));
            }
            if !seen.insert(s as usize) {
                return Err(corrupt!("{what}: mini chain from {start:#x} loops (at {s:#x})"));
            }
            let at = s as usize * 64;
            let Some(bytes) = self.mini_stream.get(at..at + 64) else {
                return Err(corrupt!(
                    "{what}: mini sector {s:#x} lies past the end of the mini stream ({} bytes)",
                    self.mini_stream.len()
                ));
            };
            let take = (want - out.len()).min(64);
            out.extend_from_slice(&bytes[..take]);
            s = self.minifat[s as usize];
        }
        Ok(out)
    }

    /// The entries reachable from the root; index 0 is the root.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The child of storage `parent` called `name` (compared as the
    /// specification does: case-insensitively for simple case folding;
    /// here ASCII and the code units as stored).
    pub fn child(&self, parent: usize, name: &[u16]) -> Option<usize> {
        let p = self.entries.get(parent)?;
        p.children.iter().copied().find(|&c| {
            let n = &self.entries[c].name;
            n.len() == name.len() && n.iter().zip(name).all(|(&a, &b)| fold(a) == fold(b))
        })
    }

    /// The child of the root called `name` (UTF-16 of the text).
    pub fn root_child(&self, name: &str) -> Option<usize> {
        let units: Vec<u16> = name.encode_utf16().collect();
        self.child(0, &units)
    }

    /// The bytes of stream entry `index`.
    pub fn stream(&self, index: usize) -> Result<Vec<u8>, Error> {
        let Some(e) = self.entries.get(index) else {
            return Err(Error::NotFound(alloc::format!("#{index}")));
        };
        let what = alloc::format!("stream {:?} (entry {})", e.name_lossy(), e.id);
        if e.kind != Kind::Stream {
            return Err(corrupt!("{what} is a storage, not a stream"));
        }
        if e.size < u64::from(self.mini_cutoff) {
            self.read_mini(e.start, e.size, &what)
        } else {
            self.read_chain(e.start, Some(e.size), &what)
        }
    }
}

/// Upper-cases ASCII letters for name comparison.
fn fold(u: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&u) { u - 32 } else { u }
}

fn prefix(e: Error, p: &str) -> Error {
    match e {
        Error::Corrupt(s) if !s.starts_with(p) => Error::Corrupt(alloc::format!("{p}{s}")),
        e => e,
    }
}

/// Parses the 128-byte entries of the directory stream and walks the
/// tree from the root, keeping only reachable entries.
fn read_directory(dir: &[u8], version: u16) -> Result<Vec<Entry>, Error> {
    struct Raw {
        name: Vec<u16>,
        kind: u8,
        left: u32,
        right: u32,
        child: u32,
        start: u32,
        size: u64,
    }
    let count = dir.len() / 128;
    if count == 0 {
        return Err(corrupt!("the directory is empty (no root entry)"));
    }
    let mut raw = try_vec::<Raw>(count)?;
    for i in 0..count {
        let e = &dir[i * 128..(i + 1) * 128];
        let name_len = usize::from(u16_at(e, 64).unwrap_or(0));
        let kind = e[66];
        let mut name = Vec::new();
        if kind != 0 {
            if name_len > 64 || name_len % 2 != 0 || name_len == 0 {
                return Err(corrupt!("directory entry {i}: name length {name_len}"));
            }
            for j in 0..name_len / 2 - 1 {
                name.push(u16_at(e, 2 * j).unwrap_or(0));
            }
        }
        let mut size = u64_at(e, 120).unwrap_or(0);
        if version == 3 {
            // [MS-CFB]: version 3 readers ignore the high 32 bits.
            size &= 0xffff_ffff;
        }
        raw.push(Raw {
            name,
            kind,
            left: u32_at(e, 68).unwrap_or(NOSTREAM),
            right: u32_at(e, 72).unwrap_or(NOSTREAM),
            child: u32_at(e, 76).unwrap_or(NOSTREAM),
            start: u32_at(e, 116).unwrap_or(ENDOFCHAIN),
            size,
        });
    }
    if raw[0].kind != 5 {
        return Err(corrupt!("directory entry 0 has type {} (want 5, the root)", raw[0].kind));
    }
    // Walk: each storage's child is the root of a tree of siblings.
    let mut index_of = try_vec::<Option<usize>>(count)?;
    index_of.resize(count, None);
    let mut out = try_vec::<Entry>(count)?;
    index_of[0] = Some(0);
    out.push(Entry {
        name: core::mem::take(&mut raw[0].name),
        kind: Kind::Root,
        parent: None,
        size: raw[0].size,
        start: raw[0].start,
        children: Vec::new(),
        id: 0,
    });
    let mut storages = Vec::new();
    storages.push((0usize, raw[0].child));
    while let Some((parent, child)) = storages.pop() {
        let mut stack = Vec::new();
        let mut children = Vec::new();
        // In-order traversal, iterative.
        let mut cur = child;
        loop {
            while cur != NOSTREAM {
                let i = cur as usize;
                if i >= count {
                    return Err(corrupt!(
                        "directory entry {}: link to entry {cur} of {count}",
                        out[parent].id
                    ));
                }
                if index_of[i].is_some() {
                    return Err(corrupt!("directory entry {cur} is reached twice (a loop)"));
                }
                let kind = match raw[i].kind {
                    1 => Kind::Storage,
                    2 => Kind::Stream,
                    k => {
                        return Err(corrupt!("directory entry {cur} (in the tree) has type {k}"));
                    }
                };
                index_of[i] = Some(out.len());
                out.push(Entry {
                    name: core::mem::take(&mut raw[i].name),
                    kind,
                    parent: Some(parent),
                    size: raw[i].size,
                    start: raw[i].start,
                    children: Vec::new(),
                    id: cur,
                });
                stack.push(i);
                cur = raw[i].left;
            }
            let Some(i) = stack.pop() else { break };
            let at = index_of[i].unwrap_or(0);
            children.push(at);
            if out[at].kind == Kind::Storage {
                storages.push((at, raw[i].child));
            }
            cur = raw[i].right;
        }
        out[parent].children = children;
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests;
