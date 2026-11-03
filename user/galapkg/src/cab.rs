//! Reading cabinet files (`.cab`), written from Microsoft's published
//! specifications [MS-CAB] (the header, folders, files, data blocks and
//! their checksum) and [MS-MCI] (MSZIP: each data block is `CK` and a
//! deflate stream (RFC 1951) whose window carries over from the folder's
//! previous blocks; inflated by `miniz_oxide`, which is given the
//! folder's output so far as the history).
//!
//! Supported: compression types none and MSZIP, reserved areas, data
//! checksums (checked when not 0). Not supported (an error says which):
//! LZX and Quantum (TODO: LZX, which large MSIs use), cabinets spanning
//! several files (a folder continued from or into another cabinet).
//!
//! Untrusted input: sizes are checked against the file and against each
//! other before anything is allocated (a folder's output is the sum of
//! its blocks' sizes, at most 32 KiB each); errors give the offset, the
//! folder, the block or the file's name.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::{DecompressorOxide, decompress, inflate_flags};

/// Why a cabinet could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Not a cabinet, or its structures are cut short or inconsistent.
    Corrupt(String),
    /// A valid feature this reader does not have.
    Unsupported(String),
    /// A data block's checksum or a file's bytes do not match.
    Damaged(String),
    /// A buffer of this many bytes could not be allocated.
    OutOfMemory(u64),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Corrupt(what) => write!(f, "not a valid cabinet: {what}"),
            Error::Unsupported(what) => write!(f, "cabinet feature not supported: {what}"),
            Error::Damaged(what) => write!(f, "cabinet data is damaged: {what}"),
            Error::OutOfMemory(n) => write!(f, "out of memory reading a cabinet ({n} bytes)"),
        }
    }
}

/// [`Cabinet::extract_all`]'s error: the cabinet's, or the callback's.
#[derive(Debug)]
pub enum ExtractError<E> {
    Cab(Error),
    Caller(E),
}

macro_rules! corrupt {
    ($($t:tt)*) => { Error::Corrupt(alloc::format!($($t)*)) };
}

/// The compression of a folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    None,
    MsZip,
    Quantum,
    /// LZX with a window of 2^n bytes.
    Lzx(u8),
    Other(u16),
}

/// A folder: a run of data blocks compressed as one stream.
#[derive(Clone, Debug)]
pub struct Folder {
    /// The offset of its first data block.
    pub data_offset: u32,
    pub blocks: u16,
    pub compression: Compression,
}

/// A file in the cabinet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CabFile {
    /// The name as stored (`\` separated; UTF-8 when the attribute says
    /// so, else taken as Latin-1).
    pub name: String,
    pub size: u32,
    /// The offset in its folder's uncompressed data.
    pub offset: u32,
    pub folder: u16,
    /// MS-DOS date and time.
    pub date: u16,
    pub time: u16,
    pub attributes: u16,
}

impl CabFile {
    /// `_A_NAME_IS_UTF`.
    pub const NAME_IS_UTF: u16 = 0x80;
}

/// A cabinet held in memory.
pub struct Cabinet<'a> {
    data: &'a [u8],
    /// Bytes of per-block reserved data.
    data_reserve: usize,
    pub folders: Vec<Folder>,
    pub files: Vec<CabFile>,
    /// The cabinet's set id and index (for spanning sets).
    pub set_id: u16,
    pub index: u16,
}

fn u16_at(b: &[u8], at: usize) -> Result<u16, Error> {
    b.get(at..at.wrapping_add(2))
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| corrupt!("cut short at offset {at} ({} bytes)", b.len()))
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, Error> {
    b.get(at..at.wrapping_add(4))
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| corrupt!("cut short at offset {at} ({} bytes)", b.len()))
}

/// A NUL-terminated string at `at` (at most 256 bytes, as the
/// specification limits them); returns it and the offset after the NUL.
fn cstr<'b>(b: &'b [u8], at: usize, what: &str) -> Result<(&'b [u8], usize), Error> {
    let rest = b.get(at..).unwrap_or_default();
    match rest.iter().take(257).position(|&c| c == 0) {
        Some(n) => Ok((&rest[..n], at + n + 1)),
        None => Err(corrupt!("{what} at offset {at} has no terminating NUL within 256 bytes")),
    }
}

/// The data block checksum of [MS-CAB] ("Checksum Method"): the 32-bit
/// little-endian words XORed together, the 1-3 bytes left over taken as
/// one more word (the first of them in the highest position), seeded.
pub fn checksum(data: &[u8], seed: u32) -> u32 {
    let mut csum = seed;
    let mut words = data.chunks_exact(4);
    for w in &mut words {
        csum ^= u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
    }
    let mut ul = 0u32;
    for &b in words.remainder() {
        ul = (ul << 8) | u32::from(b);
    }
    csum ^ ul
}

const FLAG_PREV_CABINET: u16 = 1;
const FLAG_NEXT_CABINET: u16 = 2;
const FLAG_RESERVE_PRESENT: u16 = 4;
/// Largest uncompressed data block.
const MAX_BLOCK: usize = 32768;

impl<'a> Cabinet<'a> {
    /// Reads the header, the folders and the file list of `data`.
    pub fn open(data: &'a [u8]) -> Result<Cabinet<'a>, Error> {
        if data.get(..4) != Some(b"MSCF") {
            return Err(corrupt!("no MSCF signature at offset 0"));
        }
        let cb_cabinet = u32_at(data, 8)? as usize;
        if cb_cabinet > data.len() {
            return Err(corrupt!(
                "the header says {cb_cabinet} bytes (offset 8), the file has {}",
                data.len()
            ));
        }
        let data = &data[..cb_cabinet];
        let coff_files = u32_at(data, 16)? as usize;
        let (minor, major) = (data.get(24).copied(), data.get(25).copied());
        if major != Some(1) {
            return Err(Error::Unsupported(alloc::format!(
                "format version {}.{} (want 1.3)",
                major.unwrap_or(0),
                minor.unwrap_or(0)
            )));
        }
        let folders = u16_at(data, 26)?;
        let files = u16_at(data, 28)?;
        let flags = u16_at(data, 30)?;
        let set_id = u16_at(data, 32)?;
        let index = u16_at(data, 34)?;
        let mut at = 36;
        let (mut folder_reserve, mut data_reserve) = (0usize, 0usize);
        if flags & FLAG_RESERVE_PRESENT != 0 {
            let header_reserve = usize::from(u16_at(data, 36)?);
            folder_reserve = usize::from(*data.get(38).ok_or_else(|| corrupt!("cut short at 38"))?);
            data_reserve = usize::from(*data.get(39).ok_or_else(|| corrupt!("cut short at 39"))?);
            at = 40 + header_reserve;
        }
        if flags & FLAG_PREV_CABINET != 0 {
            at = cstr(data, at, "the previous cabinet's name")?.1;
            at = cstr(data, at, "the previous disk's name")?.1;
        }
        if flags & FLAG_NEXT_CABINET != 0 {
            at = cstr(data, at, "the next cabinet's name")?.1;
            at = cstr(data, at, "the next disk's name")?.1;
        }
        let mut fv = Vec::new();
        fv.try_reserve_exact(usize::from(folders))
            .map_err(|_| Error::OutOfMemory(folders.into()))?;
        for i in 0..folders {
            let data_offset = u32_at(data, at)?;
            let blocks = u16_at(data, at + 4)?;
            let ty = u16_at(data, at + 6)?;
            let compression = match ty & 0x000f {
                0 => Compression::None,
                1 => Compression::MsZip,
                2 => Compression::Quantum,
                3 => Compression::Lzx(((ty >> 8) & 0x1f) as u8),
                _ => Compression::Other(ty),
            };
            if data_offset as usize > data.len() {
                return Err(corrupt!(
                    "folder {i} (offset {at}): data at {data_offset}, past the end ({} bytes)",
                    data.len()
                ));
            }
            fv.push(Folder { data_offset, blocks, compression });
            at += 8 + folder_reserve;
        }
        let mut list = Vec::new();
        list.try_reserve_exact(usize::from(files)).map_err(|_| Error::OutOfMemory(files.into()))?;
        let mut at = coff_files;
        for i in 0..files {
            let size = u32_at(data, at)?;
            let offset = u32_at(data, at + 4)?;
            let folder = u16_at(data, at + 8)?;
            let date = u16_at(data, at + 10)?;
            let time = u16_at(data, at + 12)?;
            let attributes = u16_at(data, at + 14)?;
            let (raw, next) = cstr(data, at + 16, "a file name")?;
            let name = if attributes & CabFile::NAME_IS_UTF != 0 || raw.is_ascii() {
                String::from_utf8_lossy(raw).into_owned()
            } else {
                raw.iter().map(|&c| char::from(c)).collect()
            };
            if folder >= 0xfffd {
                return Err(Error::Unsupported(alloc::format!(
                    "file {i} ({name}) continues from or into another cabinet (folder {folder:#x})"
                )));
            }
            if folder >= folders {
                return Err(corrupt!("file {i} ({name}): folder {folder} of {folders}"));
            }
            list.push(CabFile { name, size, offset, folder, date, time, attributes });
            at = next;
        }
        Ok(Cabinet { data, data_reserve, folders: fv, files: list, set_id, index })
    }

    /// The uncompressed data of folder `index`, all its blocks.
    pub fn folder_data(&self, index: usize) -> Result<Vec<u8>, Error> {
        let Some(f) = self.folders.get(index) else {
            return Err(corrupt!("folder {index} of {}", self.folders.len()));
        };
        match f.compression {
            Compression::None | Compression::MsZip => {}
            Compression::Lzx(w) => {
                return Err(Error::Unsupported(alloc::format!(
                    "folder {index}: LZX compression (window 2^{w}) is not implemented yet"
                )));
            }
            Compression::Quantum => {
                return Err(Error::Unsupported(alloc::format!(
                    "folder {index}: Quantum compression"
                )));
            }
            Compression::Other(t) => {
                return Err(Error::Unsupported(alloc::format!(
                    "folder {index}: compression type {t:#x}"
                )));
            }
        }
        // First pass: the blocks' places and sizes, all checked.
        let mut blocks = Vec::new();
        blocks
            .try_reserve_exact(usize::from(f.blocks))
            .map_err(|_| Error::OutOfMemory(f.blocks.into()))?;
        let mut at = f.data_offset as usize;
        let mut total = 0usize;
        for b in 0..f.blocks {
            let csum = u32_at(self.data, at)?;
            let cb_data = usize::from(u16_at(self.data, at + 4)?);
            let cb_uncomp = usize::from(u16_at(self.data, at + 6)?);
            let start = at + 8 + self.data_reserve;
            let end = start + cb_data;
            if end > self.data.len() {
                return Err(corrupt!(
                    "folder {index} block {b} (offset {at}): {cb_data} bytes past the end ({} bytes)",
                    self.data.len()
                ));
            }
            if cb_uncomp > MAX_BLOCK {
                return Err(corrupt!(
                    "folder {index} block {b} (offset {at}): {cb_uncomp} uncompressed bytes (most 32768)"
                ));
            }
            if f.compression == Compression::MsZip && cb_data < 2 && cb_uncomp > 0 {
                return Err(corrupt!(
                    "folder {index} block {b} (offset {at}): {cb_data} bytes cannot hold an MSZIP block"
                ));
            }
            if f.compression == Compression::None && cb_uncomp != cb_data {
                return Err(corrupt!(
                    "folder {index} block {b} (offset {at}): stored, but {cb_data} bytes become {cb_uncomp}"
                ));
            }
            if csum != 0 {
                let c = checksum(&self.data[start..end], 0);
                let c = checksum(&self.data[at + 4..at + 8 + self.data_reserve], c);
                if c != csum {
                    return Err(Error::Damaged(alloc::format!(
                        "folder {index} block {b} (offset {at}): checksum {c:#010x}, stored {csum:#010x}"
                    )));
                }
            }
            blocks.push((at, start, end, cb_uncomp));
            total += cb_uncomp;
            at = end;
        }
        let mut out = Vec::new();
        out.try_reserve_exact(total).map_err(|_| Error::OutOfMemory(total as u64))?;
        for (b, &(at, start, end, cb_uncomp)) in blocks.iter().enumerate() {
            let input = &self.data[start..end];
            if f.compression == Compression::None {
                out.extend_from_slice(input);
                continue;
            }
            if input.get(..2) != Some(b"CK") {
                return Err(corrupt!(
                    "folder {index} block {b} (offset {at}): no MSZIP signature CK"
                ));
            }
            let pos = out.len();
            out.resize(pos + cb_uncomp, 0);
            let mut state = DecompressorOxide::new();
            let (status, _, written) = decompress(
                &mut state,
                &input[2..],
                &mut out[..pos + cb_uncomp],
                pos,
                inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF,
            );
            if status != TINFLStatus::Done || written != cb_uncomp {
                return Err(Error::Damaged(alloc::format!(
                    "folder {index} block {b} (offset {at}): MSZIP inflate {status:?} after {written} of {cb_uncomp} bytes"
                )));
            }
        }
        Ok(out)
    }

    /// File `i`'s range in its folder's data, checked.
    fn range(&self, i: usize, folder_len: usize) -> Result<core::ops::Range<usize>, Error> {
        let f = &self.files[i];
        let start = f.offset as usize;
        let end = start.saturating_add(f.size as usize);
        if end > folder_len {
            return Err(corrupt!(
                "file {} at {start}..{end} of folder {}, which has {folder_len} bytes",
                f.name,
                f.folder
            ));
        }
        Ok(start..end)
    }

    /// The bytes of file `i` (decompresses its whole folder: use
    /// [`Self::extract_all`] for many files).
    pub fn extract(&self, i: usize) -> Result<Vec<u8>, Error> {
        let Some(f) = self.files.get(i) else {
            return Err(corrupt!("file {i} of {}", self.files.len()));
        };
        let data = self.folder_data(usize::from(f.folder))?;
        let r = self.range(i, data.len())?;
        Ok(data[r].to_vec())
    }

    /// Calls `each(file, bytes)` for every file, decompressing each folder
    /// once; stops at the first error.
    pub fn extract_all<E>(
        &self,
        mut each: impl FnMut(&CabFile, &[u8]) -> Result<(), E>,
    ) -> Result<(), ExtractError<E>> {
        for folder in 0..self.folders.len() {
            if !self.files.iter().any(|f| usize::from(f.folder) == folder) {
                continue;
            }
            let data = self.folder_data(folder).map_err(ExtractError::Cab)?;
            for (i, f) in self.files.iter().enumerate() {
                if usize::from(f.folder) == folder {
                    let r = self.range(i, data.len()).map_err(ExtractError::Cab)?;
                    each(f, &data[r]).map_err(ExtractError::Caller)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests;
