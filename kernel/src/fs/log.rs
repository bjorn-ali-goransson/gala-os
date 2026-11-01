//! The change log that keeps C: across reboots (D2): what changed in the
//! ramfs after boot, as records keyed by path (node ids do not survive a
//! reboot). `fs::volume` stores the records on the disk; replaying them
//! over the boot's files (`RamFs::apply`) brings the changes back.
//!
//! A record: magic `GLOG`, payload length (u32), the log's generation
//! (u64), the kind (u8, 3 reserved bytes), a CRC-32 of the header (CRC
//! field zero) and payload, then the payload, padded to 8 bytes. A record
//! whose magic, generation or CRC does not match ends the log (a torn
//! write, or older contents of the disk). Paths are u32 length + UTF-16
//! units; descriptors u32 length + bytes (0: none). Little endian.

use crate::mm::heap::TryVec;
use alloc::vec::Vec;

/// One change, by path from the volume root (`\a\b`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// A directory or file was created (a file: emptied if it exists).
    Create {
        path: Vec<u16>,
        directory: bool,
        attributes: u32,
        created: u64,
        written: u64,
    },
    Write {
        path: Vec<u16>,
        offset: u64,
        data: Vec<u8>,
        written: u64,
    },
    SetSize {
        path: Vec<u16>,
        size: u64,
        written: u64,
    },
    SetBasic {
        path: Vec<u16>,
        created: u64,
        written: u64,
        attributes: u32,
    },
    SetSecurity {
        path: Vec<u16>,
        descriptor: Vec<u8>,
    },
    Rename {
        from: Vec<u16>,
        to: Vec<u16>,
    },
    /// Removed (a directory with everything below it).
    Delete {
        path: Vec<u16>,
    },
}

const MAGIC: u32 = u32::from_le_bytes(*b"GLOG");
pub const HEADER: usize = 24;
/// The largest payload a record may have (a Write's data is split).
pub const MAX_PAYLOAD: usize = 128 * 1024;
/// How much data one Write record carries at most.
pub const MAX_WRITE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoMemory;

/// CRC-32 (IEEE 802.3, reflected, as zlib computes it).
pub fn crc32(crc: u32, data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut t = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
                k += 1;
            }
            t[i] = c;
            i += 1;
        }
        t
    };
    let mut c = !crc;
    for &b in data {
        c = TABLE[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

fn put(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), NoMemory> {
    out.try_extend_from_slice(bytes).map_err(|_| NoMemory)
}

fn put_path(out: &mut Vec<u8>, path: &[u16]) -> Result<(), NoMemory> {
    put(out, &(path.len() as u32).to_le_bytes())?;
    for u in path {
        put(out, &u.to_le_bytes())?;
    }
    Ok(())
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), NoMemory> {
    put(out, &(bytes.len() as u32).to_le_bytes())?;
    put(out, bytes)
}

impl Op {
    fn kind(&self) -> u8 {
        match self {
            Op::Create { .. } => 1,
            Op::Write { .. } => 2,
            Op::SetSize { .. } => 3,
            Op::SetBasic { .. } => 4,
            Op::SetSecurity { .. } => 5,
            Op::Rename { .. } => 6,
            Op::Delete { .. } => 7,
        }
    }

    fn payload(&self, out: &mut Vec<u8>) -> Result<(), NoMemory> {
        match self {
            Op::Create { path, directory, attributes, created, written } => {
                put_path(out, path)?;
                put(out, &[u8::from(*directory)])?;
                put(out, &attributes.to_le_bytes())?;
                put(out, &created.to_le_bytes())?;
                put(out, &written.to_le_bytes())
            }
            Op::Write { path, offset, data, written } => {
                put_path(out, path)?;
                put(out, &offset.to_le_bytes())?;
                put(out, &written.to_le_bytes())?;
                put_bytes(out, data)
            }
            Op::SetSize { path, size, written } => {
                put_path(out, path)?;
                put(out, &size.to_le_bytes())?;
                put(out, &written.to_le_bytes())
            }
            Op::SetBasic { path, created, written, attributes } => {
                put_path(out, path)?;
                put(out, &created.to_le_bytes())?;
                put(out, &written.to_le_bytes())?;
                put(out, &attributes.to_le_bytes())
            }
            Op::SetSecurity { path, descriptor } => {
                put_path(out, path)?;
                put_bytes(out, descriptor)
            }
            Op::Rename { from, to } => {
                put_path(out, from)?;
                put_path(out, to)
            }
            Op::Delete { path } => put_path(out, path),
        }
    }

    /// Appends this change's record (of log generation `generation`) to
    /// `out`.
    pub fn encode(&self, generation: u64, out: &mut Vec<u8>) -> Result<(), NoMemory> {
        let start = out.len();
        put(out, &[0; HEADER])?;
        self.payload(out)?;
        let len = out.len() - start - HEADER;
        let header = &mut out[start..start + HEADER];
        header[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        header[4..8].copy_from_slice(&(len as u32).to_le_bytes());
        header[8..16].copy_from_slice(&generation.to_le_bytes());
        header[16] = self.kind();
        let crc = crc32(0, &out[start..]);
        out[start + 20..start + 24].copy_from_slice(&crc.to_le_bytes());
        while !(out.len() - start).is_multiple_of(8) {
            put(out, &[0])?;
        }
        Ok(())
    }
}

/// What the start of `bytes` holds.
#[derive(Debug, PartialEq, Eq)]
pub enum Decoded {
    /// A record and its length on disk (padded).
    Record(Op, usize),
    /// A record whose end is not in `bytes` yet: read at least this many.
    Need(usize),
    /// Not a record of this generation: the log ends here.
    End,
    NoMemory,
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.b.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(s)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn path(&mut self) -> Option<Result<Vec<u16>, NoMemory>> {
        let n = self.u32()? as usize;
        let raw = self.take(n.checked_mul(2)?)?;
        let mut v = Vec::new();
        if v.try_reserve_exact(n).is_err() {
            return Some(Err(NoMemory));
        }
        v.extend(raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])));
        Some(Ok(v))
    }

    fn bytes(&mut self) -> Option<Result<Vec<u8>, NoMemory>> {
        let n = self.u32()? as usize;
        let raw = self.take(n)?;
        let mut v = Vec::new();
        Some(v.try_extend_from_slice(raw).map(|()| v).map_err(|_| NoMemory))
    }
}

/// Decodes the record at the start of `bytes` if it belongs to log
/// generation `generation`.
pub fn decode(bytes: &[u8], generation: u64) -> Decoded {
    if bytes.len() < HEADER {
        return Decoded::Need(HEADER);
    }
    let field =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    let len = field(4) as usize;
    if field(0) != MAGIC
        || u64::from_le_bytes(bytes[8..16].try_into().unwrap_or([0; 8])) != generation
        || len > MAX_PAYLOAD
    {
        return Decoded::End;
    }
    let total = (HEADER + len).next_multiple_of(8);
    if bytes.len() < HEADER + len {
        return Decoded::Need(total);
    }
    let mut header = [0u8; HEADER];
    header.copy_from_slice(&bytes[..HEADER]);
    header[20..24].fill(0);
    let crc = crc32(crc32(0, &header), &bytes[HEADER..HEADER + len]);
    if crc != field(20) {
        return Decoded::End;
    }
    let mut r = Reader { b: &bytes[HEADER..HEADER + len], at: 0 };
    let op = match parse(bytes[16], &mut r) {
        Some(Ok(op)) => op,
        Some(Err(NoMemory)) => return Decoded::NoMemory,
        None => return Decoded::End,
    };
    Decoded::Record(op, total)
}

fn parse(kind: u8, r: &mut Reader<'_>) -> Option<Result<Op, NoMemory>> {
    macro_rules! path {
        () => {
            match r.path()? {
                Ok(p) => p,
                Err(e) => return Some(Err(e)),
            }
        };
    }
    Some(Ok(match kind {
        1 => {
            let path = path!();
            let directory = r.take(1)?[0] != 0;
            let (attributes, created, written) = (r.u32()?, r.u64()?, r.u64()?);
            Op::Create { path, directory, attributes, created, written }
        }
        2 => {
            let path = path!();
            let (offset, written) = (r.u64()?, r.u64()?);
            match r.bytes()? {
                Ok(data) => Op::Write { path, offset, data, written },
                Err(e) => return Some(Err(e)),
            }
        }
        3 => {
            let path = path!();
            Op::SetSize { path, size: r.u64()?, written: r.u64()? }
        }
        4 => {
            let path = path!();
            Op::SetBasic { path, created: r.u64()?, written: r.u64()?, attributes: r.u32()? }
        }
        5 => {
            let path = path!();
            match r.bytes()? {
                Ok(descriptor) => Op::SetSecurity { path, descriptor },
                Err(e) => return Some(Err(e)),
            }
        }
        6 => {
            let from = path!();
            let to = path!();
            Op::Rename { from, to }
        }
        7 => Op::Delete { path: path!() },
        _ => return None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn all_kinds() -> Vec<Op> {
        vec![
            Op::Create {
                path: w("\\d"),
                directory: true,
                attributes: 0x10,
                created: 5,
                written: 6,
            },
            Op::Write { path: w("\\d\\f"), offset: 7, data: vec![1, 2, 3], written: 9 },
            Op::SetSize { path: w("\\d\\f"), size: 2, written: 10 },
            Op::SetBasic { path: w("\\d\\f"), created: 1, written: 2, attributes: 0x22 },
            Op::SetSecurity { path: w("\\d"), descriptor: vec![1, 0, 4, 0x80] },
            Op::Rename { from: w("\\d\\f"), to: w("\\d\\g") },
            Op::Delete { path: w("\\d\\g") },
        ]
    }

    #[test]
    fn crc32_matches_zlib() {
        assert_eq!(crc32(0, b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(crc32(0, b"1234"), b"56789"), 0xcbf4_3926);
    }

    #[test]
    fn records_round_trip() {
        let mut bytes = Vec::new();
        for op in all_kinds() {
            op.encode(3, &mut bytes).unwrap();
        }
        assert!(bytes.len().is_multiple_of(8));
        let mut at = 0;
        let mut back = Vec::new();
        while let Decoded::Record(op, n) = decode(&bytes[at..], 3) {
            back.push(op);
            at += n;
        }
        assert_eq!(back, all_kinds());
        assert_eq!(at, bytes.len());
        assert_eq!(decode(&bytes[at..], 3), Decoded::Need(HEADER));
    }

    #[test]
    fn the_log_ends_at_a_bad_record() {
        let mut bytes = Vec::new();
        all_kinds()[1].encode(3, &mut bytes).unwrap();
        // Another generation: older contents of the disk.
        assert_eq!(decode(&bytes, 4), Decoded::End);
        // A torn write.
        let mut torn = bytes.clone();
        torn[HEADER + 2] ^= 1;
        assert_eq!(decode(&torn, 3), Decoded::End);
        // Zeros.
        assert_eq!(decode(&[0; 64], 3), Decoded::End);
        // Not all there yet.
        assert!(matches!(decode(&bytes[..HEADER + 1], 3), Decoded::Need(n) if n == bytes.len()));
    }
}
