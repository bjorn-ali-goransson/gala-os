//! GUID partition tables (ROADMAP IN step 1): the on-disk layout of the
//! UEFI specification ("GUID Partition Table (GPT) Disk Layout") and the
//! structures Windows' disk IOCTLs exchange (winioctl.h's CREATE_DISK,
//! DRIVE_LAYOUT_INFORMATION_EX, PARTITION_INFORMATION_EX). Pure: the disk
//! is read and written by the caller (`nt::disk`). Host-tested.
//!
//! On disk: the protective MBR (LBA 0: one partition of type 0xEE over
//! the disk), the primary header (LBA 1) and entry array (from LBA 2),
//! the backup entries (ending at the last LBA but one) and header (the
//! last LBA). Entries are 128 bytes; arrays hold at least 128 of them
//! (32 sectors), so the usable range is LBA 34 to the 34th from the end.

use crate::mm::heap::TryVec;
use alloc::vec::Vec;

use super::status::NtStatus;

pub const SECTOR: usize = 512;
const ENTRY: usize = 128;
/// Entries an array holds at least (what Windows and UEFI make).
pub const MIN_ENTRIES: u32 = 128;
/// winioctl.h's PARTITION_STYLE.
pub const STYLE_MBR: u32 = 0;
pub const STYLE_GPT: u32 = 1;
pub const STYLE_RAW: u32 = 2;
/// DRIVE_LAYOUT_INFORMATION_EX's header (PartitionStyle, PartitionCount,
/// the GPT part padded to 40 bytes) and one PARTITION_INFORMATION_EX.
pub const LAYOUT_HEADER: usize = 48;
pub const PARTITION_INFO: usize = 144;

/// One used entry of the table.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Partition {
    pub kind: [u8; 16],
    pub id: [u8; 16],
    pub first: u64,
    /// Inclusive, as the GPT stores it.
    pub last: u64,
    pub attributes: u64,
    /// UTF-16LE, 36 units.
    pub name: [u8; 72],
}

/// A disk's GPT: its GUID, how many entries its array holds, and the used
/// entries in array order (Windows numbers them from 1 in that order).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Gpt {
    pub disk_id: [u8; 16],
    pub entries: u32,
    pub partitions: Vec<Partition>,
}

/// CRC-32 as the GPT uses it (IEEE 802.3, reflected).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    let mut v = [0u8; 4];
    v.copy_from_slice(&b[at..at + 4]);
    u32::from_le_bytes(v)
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(v)
}

fn guid_at(b: &[u8], at: usize) -> [u8; 16] {
    let mut g = [0u8; 16];
    g.copy_from_slice(&b[at..at + 16]);
    g
}

fn no_memory<T>(_: T) -> NtStatus {
    NtStatus::NO_MEMORY
}

/// Sectors an array of `entries` entries takes.
fn array_sectors(entries: u32) -> u64 {
    (entries as u64 * ENTRY as u64).div_ceil(SECTOR as u64)
}

/// The first and last usable LBA of a disk of `sectors` sectors whose
/// arrays hold `entries` entries (None: too small for a GPT).
pub fn usable(sectors: u64, entries: u32) -> Option<(u64, u64)> {
    let array = array_sectors(entries);
    let first = 2 + array;
    let last = sectors.checked_sub(2 + array)?;
    (last >= first).then_some((first, last))
}

/// Reads the disk's GPT through `read` (an LBA and a buffer of whole
/// sectors): None when the disk has none Windows would take (no
/// protective MBR, no valid primary header, an entry array whose CRC32 is
/// wrong). The backup is not consulted (a limit: ARCHITECTURE "Disk").
pub fn read(
    sectors: u64,
    mut read: impl FnMut(u64, &mut [u8]) -> Result<(), NtStatus>,
) -> Result<Option<Gpt>, NtStatus> {
    let mut s = [0u8; SECTOR];
    read(0, &mut s)?;
    let protective = s[510] == 0x55 && s[511] == 0xaa && s[446 + 4] == 0xee;
    if !protective {
        return Ok(None);
    }
    read(1, &mut s)?;
    let size = u32_at(&s, 12) as usize;
    if &s[..8] != b"EFI PART" || !(92..=SECTOR).contains(&size) {
        return Ok(None);
    }
    let mut header = [0u8; SECTOR];
    header[..size].copy_from_slice(&s[..size]);
    header[16..20].fill(0);
    if crc32(&header[..size]) != u32_at(&s, 16) || u64_at(&s, 24) != 1 {
        return Ok(None);
    }
    let (entries, entry_size, at) = (u32_at(&s, 80), u32_at(&s, 84) as usize, u64_at(&s, 72));
    let (first, last) = (u64_at(&s, 40), u64_at(&s, 48));
    if entry_size != ENTRY || entries == 0 || entries > 4096 || at < 2 || last >= sectors {
        return Ok(None);
    }
    let mut array = Vec::new();
    array.try_reserve_exact(array_sectors(entries) as usize * SECTOR).map_err(no_memory)?;
    array.resize(array_sectors(entries) as usize * SECTOR, 0);
    read(at, &mut array)?;
    let used = &array[..entries as usize * ENTRY];
    if crc32(used) != u32_at(&s, 88) {
        return Ok(None);
    }
    let mut partitions = Vec::new();
    for e in used.chunks_exact(ENTRY) {
        if e[..16].iter().all(|&b| b == 0) {
            continue;
        }
        let (p_first, p_last) = (u64_at(e, 32), u64_at(e, 40));
        if p_first < first || p_last > last || p_last < p_first {
            return Ok(None);
        }
        let mut name = [0u8; 72];
        name.copy_from_slice(&e[56..128]);
        partitions
            .try_push(Partition {
                kind: guid_at(e, 0),
                id: guid_at(e, 16),
                first: p_first,
                last: p_last,
                attributes: u64_at(e, 48),
                name,
            })
            .map_err(no_memory)?;
    }
    Ok(Some(Gpt { disk_id: guid_at(&s, 56), entries, partitions }))
}

/// The sectors that hold `gpt` on a disk of `sectors` sectors, as (first
/// LBA, bytes): the protective MBR, the primary header and array, the
/// backup array and header.
pub fn write(gpt: &Gpt, sectors: u64) -> Result<Vec<(u64, Vec<u8>)>, NtStatus> {
    let (first, last) = usable(sectors, gpt.entries).ok_or(NtStatus::INVALID_PARAMETER)?;
    let array_len = array_sectors(gpt.entries) as usize * SECTOR;
    let mut array = Vec::new();
    array.try_reserve_exact(array_len).map_err(no_memory)?;
    array.resize(array_len, 0);
    for (i, p) in gpt.partitions.iter().enumerate() {
        let e = &mut array[i * ENTRY..(i + 1) * ENTRY];
        e[..16].copy_from_slice(&p.kind);
        e[16..32].copy_from_slice(&p.id);
        e[32..40].copy_from_slice(&p.first.to_le_bytes());
        e[40..48].copy_from_slice(&p.last.to_le_bytes());
        e[48..56].copy_from_slice(&p.attributes.to_le_bytes());
        e[56..128].copy_from_slice(&p.name);
    }
    let array_crc = crc32(&array[..gpt.entries as usize * ENTRY]);
    let header = |me: u64, other: u64, at: u64| -> Result<Vec<u8>, NtStatus> {
        let mut h = Vec::new();
        h.try_reserve_exact(SECTOR).map_err(no_memory)?;
        h.resize(SECTOR, 0);
        h[..8].copy_from_slice(b"EFI PART");
        h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        h[12..16].copy_from_slice(&92u32.to_le_bytes());
        h[24..32].copy_from_slice(&me.to_le_bytes());
        h[32..40].copy_from_slice(&other.to_le_bytes());
        h[40..48].copy_from_slice(&first.to_le_bytes());
        h[48..56].copy_from_slice(&last.to_le_bytes());
        h[56..72].copy_from_slice(&gpt.disk_id);
        h[72..80].copy_from_slice(&at.to_le_bytes());
        h[80..84].copy_from_slice(&gpt.entries.to_le_bytes());
        h[84..88].copy_from_slice(&(ENTRY as u32).to_le_bytes());
        h[88..92].copy_from_slice(&array_crc.to_le_bytes());
        let crc = crc32(&h[..92]);
        h[16..20].copy_from_slice(&crc.to_le_bytes());
        Ok(h)
    };
    let mut mbr = Vec::new();
    mbr.try_reserve_exact(SECTOR).map_err(no_memory)?;
    mbr.resize(SECTOR, 0);
    // One record: CHS start 0/0/2, type 0xEE, CHS end all ones, LBA 1,
    // the rest of the disk (at most what 32 bits hold).
    mbr[446..454].copy_from_slice(&[0, 0, 2, 0, 0xee, 0xff, 0xff, 0xff]);
    mbr[454..458].copy_from_slice(&1u32.to_le_bytes());
    let span = u32::try_from(sectors - 1).unwrap_or(u32::MAX);
    mbr[458..462].copy_from_slice(&span.to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xaa;
    let backup_array_at = sectors - 1 - array_sectors(gpt.entries);
    let mut backup_array = Vec::new();
    backup_array.try_reserve_exact(array_len).map_err(no_memory)?;
    backup_array.try_extend_from_slice(&array).map_err(no_memory)?;
    let mut out = Vec::new();
    out.try_reserve_exact(5).map_err(no_memory)?;
    out.try_push((0, mbr)).map_err(no_memory)?;
    out.try_push((1, header(1, sectors - 1, 2)?)).map_err(no_memory)?;
    out.try_push((2, array)).map_err(no_memory)?;
    out.try_push((backup_array_at, backup_array)).map_err(no_memory)?;
    out.try_push((sectors - 1, header(sectors - 1, 1, backup_array_at)?)).map_err(no_memory)?;
    Ok(out)
}

/// IOCTL_DISK_GET_DRIVE_LAYOUT_EX's answer: a GPT's layout, or
/// PARTITION_STYLE_RAW with no partitions for a disk without one (MBR
/// disks are not read yet).
pub fn layout(gpt: Option<&Gpt>, sectors: u64) -> Result<Vec<u8>, NtStatus> {
    let count = gpt.map_or(0, |g| g.partitions.len());
    let mut out = Vec::new();
    let len = LAYOUT_HEADER + count * PARTITION_INFO;
    out.try_reserve_exact(len).map_err(no_memory)?;
    out.resize(len, 0);
    let Some(gpt) = gpt else {
        out[0..4].copy_from_slice(&STYLE_RAW.to_le_bytes());
        return Ok(out);
    };
    let (first, last) = usable(sectors, gpt.entries).ok_or(NtStatus::DISK_CORRUPT_ERROR)?;
    let sector = SECTOR as u64;
    out[0..4].copy_from_slice(&STYLE_GPT.to_le_bytes());
    out[4..8].copy_from_slice(&(count as u32).to_le_bytes());
    out[8..24].copy_from_slice(&gpt.disk_id);
    out[24..32].copy_from_slice(&(first * sector).to_le_bytes());
    out[32..40].copy_from_slice(&((last - first + 1) * sector).to_le_bytes());
    out[40..44].copy_from_slice(&gpt.entries.to_le_bytes());
    for (i, p) in gpt.partitions.iter().enumerate() {
        let b =
            &mut out[LAYOUT_HEADER + i * PARTITION_INFO..LAYOUT_HEADER + (i + 1) * PARTITION_INFO];
        b[0..4].copy_from_slice(&STYLE_GPT.to_le_bytes());
        b[8..16].copy_from_slice(&(p.first * sector).to_le_bytes());
        b[16..24].copy_from_slice(&((p.last - p.first + 1) * sector).to_le_bytes());
        b[24..28].copy_from_slice(&(i as u32 + 1).to_le_bytes());
        b[32..48].copy_from_slice(&p.kind);
        b[48..64].copy_from_slice(&p.id);
        b[64..72].copy_from_slice(&p.attributes.to_le_bytes());
        b[72..144].copy_from_slice(&p.name);
    }
    Ok(out)
}

/// IOCTL_DISK_GET_PARTITION_INFO_EX's answer for partition `number` (its
/// layout entry); for the whole disk (0), the disk as one partition of
/// its style, number 0.
pub fn partition_info(gpt: Option<&Gpt>, sectors: u64, number: u32) -> Result<Vec<u8>, NtStatus> {
    if number == 0 {
        let mut out = Vec::new();
        out.try_reserve_exact(PARTITION_INFO).map_err(no_memory)?;
        out.resize(PARTITION_INFO, 0);
        let style = if gpt.is_some() { STYLE_GPT } else { STYLE_RAW };
        out[0..4].copy_from_slice(&style.to_le_bytes());
        out[16..24].copy_from_slice(&(sectors * SECTOR as u64).to_le_bytes());
        return Ok(out);
    }
    let all = layout(gpt, sectors)?;
    let at = LAYOUT_HEADER + (number as usize - 1) * PARTITION_INFO;
    let entry = all.get(at..at + PARTITION_INFO).ok_or(NtStatus::INVALID_DEVICE_REQUEST)?;
    let mut out = Vec::new();
    out.try_extend_from_slice(entry).map_err(no_memory)?;
    Ok(out)
}

/// IOCTL_DISK_CREATE_DISK's input for a GPT: an empty table with the
/// given disk GUID and entry count (at least MIN_ENTRIES). Other styles:
/// STATUS_NOT_SUPPORTED (MBR is not written yet).
pub fn create(input: &[u8], sectors: u64) -> Result<Gpt, NtStatus> {
    if input.len() < 24 {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    if u32_at(input, 0) != STYLE_GPT {
        return Err(NtStatus::NOT_SUPPORTED);
    }
    let entries = u32_at(input, 20).max(MIN_ENTRIES);
    if entries > 4096 || usable(sectors, entries).is_none() {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    Ok(Gpt { disk_id: guid_at(input, 4), entries, partitions: Vec::new() })
}

/// IOCTL_DISK_SET_DRIVE_LAYOUT_EX's input for a GPT disk: the table it
/// asks for (the disk GUID and entry count from its header; the entries
/// with a partition type, in order). Partitions must be whole sectors in
/// the usable range and must not overlap (STATUS_INVALID_PARAMETER).
pub fn set(input: &[u8], sectors: u64) -> Result<Gpt, NtStatus> {
    if input.len() < LAYOUT_HEADER {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    if u32_at(input, 0) != STYLE_GPT {
        return Err(NtStatus::NOT_SUPPORTED);
    }
    let count = u32_at(input, 4) as usize;
    if input.len() < LAYOUT_HEADER + count * PARTITION_INFO {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    let entries = u32_at(input, 40).max(MIN_ENTRIES);
    let (first, last) = usable(sectors, entries).ok_or(NtStatus::INVALID_PARAMETER)?;
    let sector = SECTOR as u64;
    let mut partitions: Vec<Partition> = Vec::new();
    for i in 0..count {
        let b =
            &input[LAYOUT_HEADER + i * PARTITION_INFO..LAYOUT_HEADER + (i + 1) * PARTITION_INFO];
        let kind = guid_at(b, 32);
        if kind.iter().all(|&x| x == 0) {
            continue;
        }
        let (start, length) = (u64_at(b, 8), u64_at(b, 16));
        if start % sector != 0 || length % sector != 0 || length == 0 {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let (p_first, p_last) = (start / sector, (start + length) / sector - 1);
        let overlaps = partitions.iter().any(|q| p_first <= q.last && q.first <= p_last);
        if p_first < first || p_last > last || overlaps || partitions.len() >= entries as usize {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let mut name = [0u8; 72];
        name.copy_from_slice(&b[72..144]);
        partitions
            .try_push(Partition {
                kind,
                id: guid_at(b, 48),
                first: p_first,
                last: p_last,
                attributes: u64_at(b, 64),
                name,
            })
            .map_err(no_memory)?;
    }
    Ok(Gpt { disk_id: guid_at(input, 8), entries, partitions })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECTORS: u64 = 131072;

    fn disk_of(writes: &[(u64, Vec<u8>)]) -> Vec<u8> {
        let mut d = vec![0u8; SECTORS as usize * SECTOR];
        for (lba, bytes) in writes {
            let at = *lba as usize * SECTOR;
            d[at..at + bytes.len()].copy_from_slice(bytes);
        }
        d
    }

    fn reader(d: &[u8]) -> impl FnMut(u64, &mut [u8]) -> Result<(), NtStatus> + '_ {
        move |lba, buf| {
            let at = lba as usize * SECTOR;
            buf.copy_from_slice(&d[at..at + buf.len()]);
            Ok(())
        }
    }

    #[test]
    fn crc32_is_ieee() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn an_empty_disk_is_raw() {
        let d = vec![0u8; SECTORS as usize * SECTOR];
        assert_eq!(read(SECTORS, reader(&d)).unwrap(), None);
        let l = layout(None, SECTORS).unwrap();
        assert_eq!((u32_at(&l, 0), u32_at(&l, 4), l.len()), (STYLE_RAW, 0, LAYOUT_HEADER));
    }

    fn two_partitions() -> Vec<u8> {
        let mut input = vec![0u8; LAYOUT_HEADER + 2 * PARTITION_INFO];
        input[0..4].copy_from_slice(&STYLE_GPT.to_le_bytes());
        input[4..8].copy_from_slice(&2u32.to_le_bytes());
        input[8..24].copy_from_slice(&[7; 16]);
        input[40..44].copy_from_slice(&128u32.to_le_bytes());
        for (i, (start, len)) in
            [(1u64 << 20, 16u64 << 20), (17 << 20, 32 << 20)].into_iter().enumerate()
        {
            let b = &mut input[LAYOUT_HEADER + i * PARTITION_INFO..];
            b[0..4].copy_from_slice(&STYLE_GPT.to_le_bytes());
            b[8..16].copy_from_slice(&start.to_le_bytes());
            b[16..24].copy_from_slice(&len.to_le_bytes());
            b[32..48].copy_from_slice(&[i as u8 + 1; 16]);
            b[48..64].copy_from_slice(&[i as u8 + 9; 16]);
            b[72] = b'p';
        }
        input
    }

    #[test]
    fn a_layout_set_is_written_and_read_back() {
        let gpt = set(&two_partitions(), SECTORS).unwrap();
        assert_eq!(gpt.partitions.len(), 2);
        assert_eq!((gpt.partitions[0].first, gpt.partitions[0].last), (2048, 2048 + 32768 - 1));
        let d = disk_of(&write(&gpt, SECTORS).unwrap());
        assert_eq!(read(SECTORS, reader(&d)).unwrap().as_ref(), Some(&gpt));
        // The backup header: the last LBA, pointing at the primary and
        // at its array just below it.
        let backup = &d[(SECTORS as usize - 1) * SECTOR..];
        assert_eq!(&backup[..8], b"EFI PART");
        assert_eq!(
            (u64_at(backup, 24), u64_at(backup, 32), u64_at(backup, 72)),
            (SECTORS - 1, 1, SECTORS - 33)
        );
        let l = layout(Some(&gpt), SECTORS).unwrap();
        assert_eq!(u32_at(&l, 4), 2);
        assert_eq!(u64_at(&l, 24), 34 * 512);
        assert_eq!(u64_at(&l, 32), (SECTORS - 67) * 512);
        let p2 = &l[LAYOUT_HEADER + PARTITION_INFO..];
        assert_eq!((u64_at(p2, 8), u64_at(p2, 16), u32_at(p2, 24)), (17 << 20, 32 << 20, 2));
    }

    #[test]
    fn a_damaged_table_is_none() {
        let gpt = set(&two_partitions(), SECTORS).unwrap();
        let mut d = disk_of(&write(&gpt, SECTORS).unwrap());
        d[2 * SECTOR + 40] ^= 1; // an entry: the array's CRC32 no longer holds
        assert_eq!(read(SECTORS, reader(&d)).unwrap(), None);
    }

    #[test]
    fn overlapping_or_outside_partitions_are_refused() {
        let mut input = two_partitions();
        // The second starts inside the first.
        input[LAYOUT_HEADER + PARTITION_INFO + 8..][..8]
            .copy_from_slice(&(2u64 << 20).to_le_bytes());
        assert_eq!(set(&input, SECTORS), Err(NtStatus::INVALID_PARAMETER));
        let mut input = two_partitions();
        // The first before the usable range.
        input[LAYOUT_HEADER + 8..][..8].copy_from_slice(&512u64.to_le_bytes());
        assert_eq!(set(&input, SECTORS), Err(NtStatus::INVALID_PARAMETER));
    }

    #[test]
    fn create_makes_an_empty_table() {
        let mut input = [0u8; 24];
        input[0..4].copy_from_slice(&STYLE_GPT.to_le_bytes());
        input[4..20].copy_from_slice(&[3; 16]);
        let gpt = create(&input, SECTORS).unwrap();
        assert_eq!((gpt.entries, gpt.partitions.len(), gpt.disk_id), (128, 0, [3; 16]));
        input[0..4].copy_from_slice(&STYLE_MBR.to_le_bytes());
        assert_eq!(create(&input, SECTORS).err(), Some(NtStatus::NOT_SUPPORTED));
    }
}
