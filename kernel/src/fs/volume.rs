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
//! Superblock: `GALAFS\0\x01`, version (u32), active area (u32),
//! generation (u64), the disk's sectors (u64), CRC-32 of the above.
//! Version 2 keeps the last CRASH_SECTORS sectors out of the areas for the
//! crash record (`crate::crash`). Version 3 (new volumes, D4: a
//! disk-backed C:) makes the two areas small (`area_sectors_of`) and gives
//! the rest of the disk, from the next 4 KiB boundary to the crash area,
//! to a data area of 4 KiB clusters that hold files' pages (`data_area`;
//! the log's Clusters records say which page is in which cluster). Version
//! 1 and 2 volumes (no data area: files' contents are in the log) are still
//! read as they are (ARCHITECTURE invariant 3); a kernel from before
//! version 3 formats a version 3 disk, as one from before version 2 did a
//! version 2 one.
//!
//! The rollback floor (R8, ARCHITECTURE "Chain of trust" step 6) is a block
//! of its own at byte 64 of the superblock's sector, with its own magic,
//! version and CRC (so the volume's layout version stays 2 and kernels
//! before it still mount the disk): `GALAFLOR`, version (u32, 1), length
//! (u32, 32), the floor (u64), the highest release seen (u64), CRC-32 of
//! those 32 bytes. A later version keeps these fields first and may be
//! longer (its CRC after its length). Missing or damaged reads as 0.
//! Every superblock this code writes carries it.

use super::log::{self, Decoded, NoMemory, Op};
use crate::mm::heap::TryVec;
use alloc::vec::Vec;

pub const SECTOR: usize = 512;
/// The superblock's magic; its last byte is the volume format's version
/// (1), so another version is not taken for this format (ARCHITECTURE
/// invariant 3).
const MAGIC: &[u8; 8] = b"GALAFS\0\x01";
/// The layout's version: 3 since the data area. Mounting reads 1 and 2
/// too.
const VERSION: u32 = 3;
/// Sectors per cluster of the data area (4 KiB clusters).
pub const CLUSTER_SECTORS: u64 = 8;
/// A version 3 log area: 1/32 of the disk, between 1 MiB and 256 MiB
/// (a quarter of the disk on disks too small for that).
const MIN_AREA: u64 = 2048;
const MAX_AREA: u64 = 512 * 1024;
/// Sectors kept at the end of a version 2 disk for the crash record.
pub const CRASH_SECTORS: u64 = 16;
/// How much of an area mounting reads at a time.
const CHUNK: usize = 64 * 1024;
/// The rollback floor's block in the superblock's sector (see above).
const FLOOR_AT: usize = 64;
const FLOOR_MAGIC: &[u8; 8] = b"GALAFLOR";
const FLOOR_VERSION: u32 = 1;
const FLOOR_LEN: usize = 32;

/// The rollback floor kept on the volume (R8 step 6).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Floor {
    /// The oldest release that may run on this disk.
    pub floor: u64,
    /// The highest release that has booted on it.
    pub highest: u64,
}

impl Floor {
    fn write(&self, s: &mut [u8; SECTOR]) {
        let b = &mut s[FLOOR_AT..FLOOR_AT + FLOOR_LEN + 4];
        b[0..8].copy_from_slice(FLOOR_MAGIC);
        b[8..12].copy_from_slice(&FLOOR_VERSION.to_le_bytes());
        b[12..16].copy_from_slice(&(FLOOR_LEN as u32).to_le_bytes());
        b[16..24].copy_from_slice(&self.floor.to_le_bytes());
        b[24..32].copy_from_slice(&self.highest.to_le_bytes());
        let crc = log::crc32(0, &b[..FLOOR_LEN]);
        b[32..36].copy_from_slice(&crc.to_le_bytes());
    }

    /// The block in sector `s`, or the default (0) when there is none or
    /// it does not check out. A later version keeps these fields first and
    /// may be longer (its CRC-32 follows its `length` bytes), so this
    /// version still reads its floor.
    fn read(s: &[u8; SECTOR]) -> Floor {
        let b = &s[FLOOR_AT..];
        let u32_at = |at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
        let u64_at = |at: usize| u64::from_le_bytes(b[at..at + 8].try_into().unwrap_or([0; 8]));
        let len = u32_at(12) as usize;
        if &b[0..8] != FLOOR_MAGIC
            || u32_at(8) < FLOOR_VERSION
            || len < FLOOR_LEN
            || len + 4 > b.len()
            || u32_at(len) != log::crc32(0, &b[..len])
        {
            return Floor::default();
        }
        Floor { floor: u64_at(16), highest: u64_at(24) }
    }
}

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
    /// The layout version found or made (1 or 2).
    version: u32,
    area: u32,
    generation: u64,
    area_sectors: u64,
    /// Bytes of log in the active area.
    tail: u64,
    /// The log's last partial sector (tail % SECTOR bytes of it).
    last: [u8; SECTOR],
    /// The rollback floor (R8).
    floor: Floor,
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
        s[8..12].copy_from_slice(&self.version.to_le_bytes());
        s[12..16].copy_from_slice(&self.area.to_le_bytes());
        s[16..24].copy_from_slice(&self.generation.to_le_bytes());
        s[24..32].copy_from_slice(&self.dev.sectors().to_le_bytes());
        let crc = log::crc32(0, &s[..32]);
        s[32..36].copy_from_slice(&crc.to_le_bytes());
        self.floor.write(&mut s);
        s
    }

    /// Reads the volume on `dev`, or makes an empty one if it holds none.
    pub fn mount(mut dev: B) -> Result<(Self, Mounted), VolumeError> {
        let sectors = dev.sectors();
        if sectors < 1 + 2 * 8 + CRASH_SECTORS {
            return Err(VolumeError::TooSmall);
        }
        let mut s = [0u8; SECTOR];
        io(dev.read(0, &mut s))?;
        let crc = u32::from_le_bytes([s[32], s[33], s[34], s[35]]);
        let found = u32::from_le_bytes([s[8], s[9], s[10], s[11]]);
        let valid = &s[0..8] == MAGIC
            && (1..=VERSION).contains(&found)
            && crc == log::crc32(0, &s[..32])
            && u64::from_le_bytes(s[24..32].try_into().unwrap_or([0; 8])) == sectors;
        let version = if valid { found } else { VERSION };
        let area_sectors = Self::area_sectors_of(version, sectors);
        let mut v = Volume {
            dev,
            version,
            area: 0,
            generation: 1,
            area_sectors,
            tail: 0,
            last: [0; SECTOR],
            floor: if valid { Floor::read(&s) } else { Floor::default() },
        };
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

    /// Each area's sectors in layout `version` on a disk of `sectors`.
    fn area_sectors_of(version: u32, sectors: u64) -> u64 {
        match version {
            1 => (sectors - 1) / 2,
            2 => (sectors - 1 - CRASH_SECTORS) / 2,
            _ => {
                let usable = sectors - 1 - CRASH_SECTORS;
                let area = (usable / 32).clamp(MIN_AREA, MAX_AREA);
                if 4 * area > usable { usable / 4 } else { area }
            }
        }
    }

    /// The data area (version 3): its first sector (4 KiB aligned) and its
    /// clusters.
    pub fn data_area(&self) -> Option<(u64, u32)> {
        if self.version < 3 {
            return None;
        }
        let first = (1 + 2 * self.area_sectors).next_multiple_of(CLUSTER_SECTORS);
        let end = self.dev.sectors() - CRASH_SECTORS;
        let clusters = end.saturating_sub(first) / CLUSTER_SECTORS;
        (clusters > 0).then(|| (first, u32::try_from(clusters).unwrap_or(u32::MAX)))
    }

    /// The crash record's sectors (first, count): version 2 volumes only.
    pub fn crash_area(&self) -> Option<(u64, u64)> {
        (self.version >= 2).then(|| (self.dev.sectors() - CRASH_SECTORS, CRASH_SECTORS))
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

    /// The rollback floor kept on the volume.
    pub fn floor(&self) -> Floor {
        self.floor
    }

    /// Records `floor` (only ever raised: each field takes the higher of
    /// the stored and the given value) in the superblock and flushes.
    pub fn raise_floor(&mut self, floor: Floor) -> Result<Floor, VolumeError> {
        let new = Floor {
            floor: self.floor.floor.max(floor.floor),
            highest: self.floor.highest.max(floor.highest),
        };
        if new == self.floor {
            return Ok(new);
        }
        let old = self.floor;
        self.floor = new;
        let sb = self.superblock();
        let written = io(self.dev.write(0, &sb)).and_then(|()| io(self.dev.flush()));
        if let Err(e) = written {
            self.floor = old;
            return Err(e);
        }
        Ok(new)
    }

    pub fn area_bytes(&self) -> u64 {
        self.area_sectors * SECTOR as u64
    }

    /// Bytes of log in the active area.
    pub fn used(&self) -> u64 {
        self.tail
    }

    /// The active log area (0 or 1) and the log's generation.
    pub fn position(&self) -> (u32, u64) {
        (self.area, self.generation)
    }

    pub fn sectors(&self) -> u64 {
        self.dev.sectors()
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
        let (mut v, _) = Volume::mount(disk(97)).unwrap(); // areas of 20 sectors
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
        let (mut v, _) = Volume::mount(disk(97)).unwrap();
        assert_eq!(v.compact(&[write_op(1, 30_000)]), Err(VolumeError::Full));
        let (_, m) = remount(v);
        assert_eq!(m, Mounted::Log(vec![]));
    }

    #[test]
    fn new_volumes_keep_a_crash_area_and_version_1_volumes_still_mount() {
        // A new volume: the last CRASH_SECTORS sectors kept.
        let (mut v, _) = Volume::mount(disk(200)).unwrap();
        assert_eq!(v.crash_area(), Some((200 - CRASH_SECTORS, CRASH_SECTORS)));
        v.append(&[write_op(1, 10)]).unwrap();
        let (v, m) = remount(v);
        assert_eq!(m, Mounted::Log(vec![write_op(1, 10)]));
        // A version 1 volume (from before): mounted as it is, its log read,
        // no crash area.
        let mut d = v.into_device();
        d.data.fill(0);
        let mut old = Volume {
            dev: d,
            version: 1,
            area: 0,
            generation: 1,
            area_sectors: (200 - 1) / 2,
            tail: 0,
            last: [0; SECTOR],
            floor: Floor::default(),
        };
        let sb = old.superblock();
        old.dev.write(0, &sb).unwrap();
        old.append(&[write_op(2, 20)]).unwrap();
        let (v, m) = remount(old);
        assert_eq!(m, Mounted::Log(vec![write_op(2, 20)]));
        assert_eq!(v.crash_area(), None);
        assert_eq!(v.data_area(), None);
    }

    #[test]
    fn new_volumes_have_a_data_area_after_small_log_areas() {
        // A small disk: areas of a quarter each, the rest is data.
        let (v, _) = Volume::mount(disk(97)).unwrap();
        assert_eq!(v.area_bytes(), 20 * SECTOR as u64);
        // Data from sector 48 (41 rounded up to 8) to the crash area.
        assert_eq!(v.data_area(), Some((48, (97 - 16 - 48) / 8)));
        // 16 GiB: two 256 MiB areas (1/32 capped), the rest in clusters.
        let sectors = 16u64 << 21;
        let usable = sectors - 1 - CRASH_SECTORS;
        assert_eq!(Volume::<MemDisk>::area_sectors_of(3, sectors), MAX_AREA.min(usable / 32));
        // Version 2 volumes have none.
        assert_eq!(Volume::<MemDisk>::area_sectors_of(2, 200), (200 - 1 - CRASH_SECTORS) / 2);
    }

    #[test]
    fn the_rollback_floor_is_kept_and_only_raised() {
        let (mut v, _) = Volume::mount(disk(97)).unwrap();
        assert_eq!(v.floor(), Floor::default());
        let two = Floor { floor: 2, highest: 2 };
        assert_eq!(v.raise_floor(two).unwrap(), two);
        v.append(&[write_op(1, 10)]).unwrap();
        let (mut v, m) = remount(v);
        assert_eq!(m, Mounted::Log(vec![write_op(1, 10)]));
        assert_eq!(v.floor(), two);
        // Never lowered (a release 1 with floor 1 changes nothing); the
        // highest release seen goes up on its own.
        assert_eq!(v.raise_floor(Floor { floor: 1, highest: 1 }).unwrap(), two);
        assert_eq!(
            v.raise_floor(Floor { floor: 2, highest: 3 }).unwrap(),
            Floor { floor: 2, highest: 3 }
        );
        // A compaction rewrites the superblock: the floor stays.
        while v.append(&[write_op(2, 1000)]).is_ok() {}
        v.compact(&[write_op(9, 100)]).unwrap();
        let (v, _) = remount(v);
        assert_eq!(v.floor(), Floor { floor: 2, highest: 3 });
        // A later, longer version of the block still gives its floor.
        let mut d = v.into_device();
        let b = &mut d.data[FLOOR_AT..FLOOR_AT + 48];
        b[8..12].copy_from_slice(&2u32.to_le_bytes());
        b[12..16].copy_from_slice(&44u32.to_le_bytes());
        b[32..44].fill(7);
        let crc = log::crc32(0, &b[..44]);
        b[44..48].copy_from_slice(&crc.to_le_bytes());
        let (v, _) = Volume::mount(d).unwrap();
        assert_eq!(v.floor(), Floor { floor: 2, highest: 3 });
        // A damaged block reads as no floor, the volume still mounts; a
        // superblock from before the floor (zeros there) too.
        let mut d = v.into_device();
        d.data[FLOOR_AT + 16] ^= 1;
        let (v, m) = Volume::mount(d).unwrap();
        assert_eq!(m, Mounted::Log(vec![write_op(9, 100)]));
        assert_eq!(v.floor(), Floor::default());
        let mut d = v.into_device();
        d.data[FLOOR_AT..FLOOR_AT + FLOOR_LEN + 4].fill(0);
        let (v, _) = Volume::mount(d).unwrap();
        assert_eq!(v.floor(), Floor::default());
    }
}
