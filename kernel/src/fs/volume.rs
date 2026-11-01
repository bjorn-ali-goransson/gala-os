//! The change log on a disk (D2): what keeps C: across reboots. Sector 0
//! is the superblock; the rest is two areas of equal size, one of them
//! active. The active area holds the log (`fs::log` records of the
//! superblock's generation) from its start. Appending writes the new
//! records after the last one and flushes the device before the call
//! returns. When an area is full, a snapshot of the changes (`RamFs::
//! snapshot`) is written to the other area with the next generation, the
//! device flushed, and only then the superblock switched: a crash at any
//! point leaves one complete log. A record that does not check out ends
//! the log (a torn last write is lost, nothing before it).
//!
//! Superblock: `GALAFS\0\x01`, version (u32 1), active area (u32),
//! generation (u64), the disk's sectors (u64), CRC-32 of the above.

use super::log::{self, Decoded, NoMemory, Op};
use crate::mm::heap::TryVec;
use alloc::vec::Vec;

pub const SECTOR: usize = 512;
const MAGIC: &[u8; 8] = b"GALAFS\0\x01";
const VERSION: u32 = 1;
/// How much of an area mounting reads at a time.
const CHUNK: usize = 64 * 1024;

/// A transfer the disk failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoError;

/// A disk, in sectors.
pub trait Blocks {
    fn sectors(&self) -> u64;
    fn read(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), IoError>;
    fn write(&mut self, sector: u64, data: &[u8]) -> Result<(), IoError>;
    fn flush(&mut self) -> Result<(), IoError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeError {
    /// The disk failed a transfer.
    Io,
    NoMemory,
    /// The changes do not fit in an area even as a snapshot.
    Full,
    /// Too small to hold a volume.
    TooSmall,
}

impl From<NoMemory> for VolumeError {
    fn from(_: NoMemory) -> Self {
        VolumeError::NoMemory
    }
}

pub struct Volume<B: Blocks> {
    dev: B,
    area: u32,
    generation: u64,
    area_sectors: u64,
    /// Bytes of log in the active area.
    tail: u64,
    /// The log's last partial sector (tail % SECTOR bytes of it).
    last: [u8; SECTOR],
}

/// What mounting found.
#[derive(Debug, PartialEq, Eq)]
pub enum Mounted {
    /// No volume was there: an empty one was made.
    Formatted,
    /// The log's records, to replay in order.
    Log(Vec<Op>),
}

fn io<T>(r: Result<T, IoError>) -> Result<T, VolumeError> {
    r.map_err(|IoError| VolumeError::Io)
}

impl<B: Blocks> Volume<B> {
    fn area_start(&self, area: u32) -> u64 {
        1 + u64::from(area) * self.area_sectors
    }

    fn superblock(&self) -> [u8; SECTOR] {
        let mut s = [0u8; SECTOR];
        s[0..8].copy_from_slice(MAGIC);
        s[8..12].copy_from_slice(&VERSION.to_le_bytes());
        s[12..16].copy_from_slice(&self.area.to_le_bytes());
        s[16..24].copy_from_slice(&self.generation.to_le_bytes());
        s[24..32].copy_from_slice(&self.dev.sectors().to_le_bytes());
        let crc = log::crc32(0, &s[..32]);
        s[32..36].copy_from_slice(&crc.to_le_bytes());
        s
    }

    /// Reads the volume on `dev`, or makes an empty one if it holds none.
    pub fn mount(mut dev: B) -> Result<(Self, Mounted), VolumeError> {
        let sectors = dev.sectors();
        if sectors < 1 + 2 * 8 {
            return Err(VolumeError::TooSmall);
        }
        let area_sectors = (sectors - 1) / 2;
        let mut s = [0u8; SECTOR];
        io(dev.read(0, &mut s))?;
        let mut v =
            Volume { dev, area: 0, generation: 1, area_sectors, tail: 0, last: [0; SECTOR] };
        let crc = u32::from_le_bytes([s[32], s[33], s[34], s[35]]);
        let valid = &s[0..8] == MAGIC
            && u32::from_le_bytes([s[8], s[9], s[10], s[11]]) == VERSION
            && crc == log::crc32(0, &s[..32])
            && u64::from_le_bytes(s[24..32].try_into().unwrap_or([0; 8])) == sectors;
        if !valid {
            // A fresh volume: an empty log of generation 1 in area 0.
            let sb = v.superblock();
            io(v.dev.write(0, &sb))?;
            io(v.dev.flush())?;
            return Ok((v, Mounted::Formatted));
        }
        v.area = u32::from_le_bytes([s[12], s[13], s[14], s[15]]) & 1;
        v.generation = u64::from_le_bytes(s[16..24].try_into().unwrap_or([0; 8]));
        let ops = v.read_log()?;
        Ok((v, Mounted::Log(ops)))
    }

    /// Reads the active area's records, leaving `tail` after the last.
    fn read_log(&mut self) -> Result<Vec<Op>, VolumeError> {
        let area_bytes = self.area_sectors * SECTOR as u64;
        let start = self.area_start(self.area);
        let mut ops = Vec::new();
        // Bytes of the area from `base` on.
        let mut buf: Vec<u8> = Vec::new();
        let mut base = 0u64;
        let mut at = 0usize;
        loop {
            match log::decode(&buf[at..], self.generation) {
                Decoded::Record(op, n) => {
                    ops.try_push(op).map_err(|_| VolumeError::NoMemory)?;
                    at += n;
                }
                Decoded::Need(n) => {
                    let have = base + buf.len() as u64;
                    if have >= area_bytes {
                        break;
                    }
                    // Drop what was used, then read more whole sectors.
                    buf.drain(..at);
                    base += at as u64;
                    at = 0;
                    let want = (n.max(CHUNK) as u64).min(area_bytes - have) as usize;
                    let want = want.next_multiple_of(SECTOR);
                    let old = buf.len();
                    buf.try_reserve_exact(want).map_err(|_| VolumeError::NoMemory)?;
                    buf.resize(old + want, 0);
                    let sector = start + have / SECTOR as u64;
                    io(self.dev.read(sector, &mut buf[old..]))?;
                }
                Decoded::End => break,
                Decoded::NoMemory => return Err(VolumeError::NoMemory),
            }
        }
        self.tail = base + at as u64;
        let partial = (self.tail % SECTOR as u64) as usize;
        let sector_at = at - partial;
        self.last = [0; SECTOR];
        if partial > 0 {
            self.last[..partial].copy_from_slice(&buf[sector_at..at]);
        }
        Ok(ops)
    }

    /// Appends `ops` to the log and flushes; Full if the area cannot take
    /// them (then `compact`).
    pub fn append(&mut self, ops: &[Op]) -> Result<(), VolumeError> {
        if ops.is_empty() {
            return Ok(());
        }
        let mut bytes = Vec::new();
        for op in ops {
            op.encode(self.generation, &mut bytes)?;
        }
        let area_bytes = self.area_sectors * SECTOR as u64;
        if self.tail + bytes.len() as u64 > area_bytes {
            return Err(VolumeError::Full);
        }
        self.write_at_tail(self.area, &bytes)?;
        io(self.dev.flush())
    }

    /// Writes `bytes` at the tail of area `area` (whole sectors: the
    /// partial last one is rewritten with what it held).
    fn write_at_tail(&mut self, area: u32, bytes: &[u8]) -> Result<(), VolumeError> {
        let partial = (self.tail % SECTOR as u64) as usize;
        let mut out = Vec::new();
        out.try_reserve_exact((partial + bytes.len()).next_multiple_of(SECTOR))
            .map_err(|_| VolumeError::NoMemory)?;
        out.try_extend_from_slice(&self.last[..partial]).map_err(|_| VolumeError::NoMemory)?;
        out.try_extend_from_slice(bytes).map_err(|_| VolumeError::NoMemory)?;
        let end = out.len();
        out.resize(end.next_multiple_of(SECTOR), 0);
        let sector = self.area_start(area) + self.tail / SECTOR as u64;
        // Large writes go in pieces the device takes.
        for (i, piece) in out.chunks(CHUNK).enumerate() {
            io(self.dev.write(sector + (i * CHUNK / SECTOR) as u64, piece))?;
        }
        self.tail += bytes.len() as u64;
        let now_partial = (self.tail % SECTOR as u64) as usize;
        self.last = [0; SECTOR];
        self.last[..now_partial].copy_from_slice(&out[end - now_partial..end]);
        Ok(())
    }

    /// Replaces the log by `snapshot` in the other area (see the module
    /// comment).
    pub fn compact(&mut self, snapshot: &[Op]) -> Result<(), VolumeError> {
        let generation = self.generation + 1;
        let mut bytes = Vec::new();
        for op in snapshot {
            op.encode(generation, &mut bytes)?;
        }
        let area_bytes = self.area_sectors * SECTOR as u64;
        // Room for the end marker below.
        if bytes.len() as u64 + SECTOR as u64 > area_bytes {
            return Err(VolumeError::Full);
        }
        let (old_area, old_tail, old_last) = (self.area, self.tail, self.last);
        let new_area = self.area ^ 1;
        self.tail = 0;
        self.last = [0; SECTOR];
        let written = self.write_at_tail(new_area, &bytes).and_then(|()| {
            // The sector after the log must not look like a record of the
            // new generation; the zeros after the last record (in its
            // sector) end it, and so does a zeroed next sector.
            if self.tail.is_multiple_of(SECTOR as u64) && self.tail < area_bytes {
                let sector = self.area_start(new_area) + self.tail / SECTOR as u64;
                io(self.dev.write(sector, &[0; SECTOR]))?;
            }
            io(self.dev.flush())
        });
        if let Err(e) = written {
            (self.area, self.tail, self.last) = (old_area, old_tail, old_last);
            return Err(e);
        }
        self.area = new_area;
        self.generation = generation;
        let sb = self.superblock();
        io(self.dev.write(0, &sb))?;
        io(self.dev.flush())
    }

    pub fn area_bytes(&self) -> u64 {
        self.area_sectors * SECTOR as u64
    }

    /// Bytes of log in the active area.
    pub fn used(&self) -> u64 {
        self.tail
    }

    pub fn into_device(self) -> B {
        self.dev
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A disk in memory that can stop taking writes (a crash).
    struct MemDisk {
        data: Vec<u8>,
        writes_left: Option<usize>,
    }

    impl Blocks for MemDisk {
        fn sectors(&self) -> u64 {
            (self.data.len() / SECTOR) as u64
        }
        fn read(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), IoError> {
            let at = sector as usize * SECTOR;
            buf.copy_from_slice(self.data.get(at..at + buf.len()).ok_or(IoError)?);
            Ok(())
        }
        fn write(&mut self, sector: u64, data: &[u8]) -> Result<(), IoError> {
            if let Some(n) = &mut self.writes_left {
                if *n == 0 {
                    return Err(IoError);
                }
                *n -= 1;
            }
            let at = sector as usize * SECTOR;
            self.data.get_mut(at..at + data.len()).ok_or(IoError)?.copy_from_slice(data);
            Ok(())
        }
        fn flush(&mut self) -> Result<(), IoError> {
            Ok(())
        }
    }

    fn disk(sectors: usize) -> MemDisk {
        MemDisk { data: vec![0; sectors * SECTOR], writes_left: None }
    }

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn write_op(i: u8, len: usize) -> Op {
        Op::Write { path: w("\\f"), offset: u64::from(i) * 1000, data: vec![i; len], written: 1 }
    }

    fn remount(v: Volume<MemDisk>) -> (Volume<MemDisk>, Mounted) {
        let mut d = v.into_device();
        d.writes_left = None;
        Volume::mount(d).unwrap()
    }

    #[test]
    fn a_fresh_disk_is_formatted_and_records_survive_remounts() {
        let (mut v, m) = Volume::mount(disk(200)).unwrap();
        assert_eq!(m, Mounted::Formatted);
        v.append(&[write_op(1, 10)]).unwrap();
        v.append(&[write_op(2, 700), write_op(3, 5)]).unwrap();
        let (mut v, m) = remount(v);
        assert_eq!(m, Mounted::Log(vec![write_op(1, 10), write_op(2, 700), write_op(3, 5)]));
        // Appending after a remount continues the log.
        v.append(&[write_op(4, 3)]).unwrap();
        let (_, m) = remount(v);
        let Mounted::Log(ops) = m else { panic!() };
        assert_eq!(ops.len(), 4);
        assert_eq!(ops[3], write_op(4, 3));
    }

    #[test]
    fn a_full_area_is_compacted_into_the_other() {
        let (mut v, _) = Volume::mount(disk(41)).unwrap(); // areas of 20 sectors
        let mut appended = 0;
        let full = loop {
            match v.append(&[write_op(appended, 1000)]) {
                Ok(()) => appended += 1,
                Err(e) => break e,
            }
        };
        assert_eq!(full, VolumeError::Full);
        assert!(appended >= 5);
        v.compact(&[write_op(9, 100)]).unwrap();
        v.append(&[write_op(10, 100)]).unwrap();
        let (mut v, m) = remount(v);
        assert_eq!(m, Mounted::Log(vec![write_op(9, 100), write_op(10, 100)]));
        // And back to the first area, whose old records are another
        // generation's.
        v.compact(&[write_op(11, 10)]).unwrap();
        let (_, m) = remount(v);
        assert_eq!(m, Mounted::Log(vec![write_op(11, 10)]));
    }

    #[test]
    fn a_crash_loses_at_most_the_write_in_progress() {
        let (mut v, _) = Volume::mount(disk(200)).unwrap();
        v.append(&[write_op(1, 600)]).unwrap();
        // The next append's write never reaches the disk.
        let mut d = v.into_device();
        d.writes_left = Some(0);
        let (mut v, _) = Volume::mount(d).unwrap();
        assert!(v.append(&[write_op(2, 600)]).is_err());
        let (_, m) = remount(v);
        assert_eq!(m, Mounted::Log(vec![write_op(1, 600)]));

        // A compaction cut short leaves the old log in force.
        let (mut v, _) = Volume::mount(disk(200)).unwrap();
        v.append(&[write_op(1, 10)]).unwrap();
        let mut d = v.into_device();
        d.writes_left = Some(1); // the snapshot, not the superblock
        let (mut v, _) = Volume::mount(d).unwrap();
        assert!(v.compact(&[write_op(5, 10)]).is_err());
        let (_, m) = remount(v);
        assert_eq!(m, Mounted::Log(vec![write_op(1, 10)]));
    }

    #[test]
    fn a_too_large_snapshot_is_refused() {
        let (mut v, _) = Volume::mount(disk(41)).unwrap();
        assert_eq!(v.compact(&[write_op(1, 30_000)]), Err(VolumeError::Full));
        let (_, m) = remount(v);
        assert_eq!(m, Mounted::Log(vec![]));
    }
}
