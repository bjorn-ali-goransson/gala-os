//! A fresh FAT32 volume holding a fixed set of files, as Microsoft's FAT
//! specification ("FAT: General Overview of On-Disk Format", 1.03) lays
//! it out: the boot sector and FSInfo (sectors 0 and 1, their backups at
//! 6 and 7), two FATs, then the clusters. Every file and directory gets
//! contiguous clusters, the root directory first (cluster 2). Names that
//! are not already an upper-case 8.3 name get long-name entries before a
//! generated short name (`G0000001`...). Times are a fixed date
//! (2026-01-01), so a release's files make the same volume each time.
//! The geometry is the specification's for FAT32 (32 reserved sectors,
//! the sectors per cluster from its table, the FAT size by its formula);
//! a volume with fewer than 65525 clusters is refused (firmware would
//! read it as FAT16).

use alloc::string::String;
use alloc::vec::Vec;

pub const SECTOR: usize = 512;
const RESERVED: u32 = 32;
const FATS: u32 = 2;
/// 2026-01-01 as a FAT date.
const DATE: u16 = ((2026 - 1980) << 9) | (1 << 5) | 1;

#[derive(Debug, PartialEq, Eq)]
pub enum Error<E> {
    /// Too few sectors for FAT32 (fewer than 65525 clusters).
    TooSmall,
    /// The files do not fit.
    Full,
    /// A path with an empty part, or a file where a directory is.
    BadPath,
    NoMemory,
    /// The writer's own error.
    Write(E),
}

/// The volume: its size in sectors, its first sector on the disk (the
/// BPB's hidden sectors), its serial number and label.
pub struct Volume {
    pub sectors: u64,
    pub hidden: u32,
    pub id: u32,
    pub label: [u8; 11],
}

/// What was made: sectors per cluster, the FAT's size in sectors, the
/// clusters there are and those used.
#[derive(Debug, PartialEq, Eq)]
pub struct Made {
    pub per_cluster: u32,
    pub fat_sectors: u32,
    pub clusters: u32,
    pub used: u32,
}

struct Node {
    name: String,
    /// Index into `files` (a file), or None (a directory).
    file: Option<usize>,
    children: Vec<usize>,
    parent: usize,
    first: u32,
    count: u32,
}

fn per_cluster(sectors: u64) -> u32 {
    match sectors {
        0..=532_480 => 1,
        532_481..=16_777_216 => 8,
        16_777_217..=33_554_432 => 16,
        33_554_433..=67_108_864 => 32,
        _ => 64,
    }
}

/// Whether `name` is its own upper-case 8.3 short name (no long name
/// needed): the 11-byte form if so.
fn short_of(name: &str) -> Option<[u8; 11]> {
    let ok =
        |c: u8| c.is_ascii_uppercase() || c.is_ascii_digit() || b"!#$%&'()-@^_`{}~".contains(&c);
    let (base, ext) = match name.rsplit_once('.') {
        Some((b, e)) => (b, e),
        None => (name, ""),
    };
    if base.is_empty()
        || base.len() > 8
        || ext.len() > 3
        || !base.bytes().chain(ext.bytes()).all(ok)
    {
        return None;
    }
    let mut s = [b' '; 11];
    s[..base.len()].copy_from_slice(base.as_bytes());
    s[8..8 + ext.len()].copy_from_slice(ext.as_bytes());
    Some(s)
}

fn long_entries(name: &str) -> usize {
    if short_of(name).is_some() { 0 } else { name.encode_utf16().count().div_ceil(13) }
}

fn checksum(short: &[u8; 11]) -> u8 {
    short.iter().fold(0u8, |s, &b| ((s & 1) << 7).wrapping_add(s >> 1).wrapping_add(b))
}

fn put16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn short_entry(e: &mut [u8], name: &[u8; 11], dir: bool, cluster: u32, size: u32) {
    e[..11].copy_from_slice(name);
    e[11] = if dir { 0x10 } else { 0x20 };
    put16(e, 16, DATE);
    put16(e, 18, DATE);
    put16(e, 20, (cluster >> 16) as u16);
    put16(e, 24, DATE);
    put16(e, 26, cluster as u16);
    put32(e, 28, size);
}

/// Writes a FAT32 volume holding `files` (`/`-separated paths, their
/// bytes) through `write` (a sector of the volume, whole sectors).
pub fn write<E>(
    v: &Volume,
    files: &[(&str, &[u8])],
    mut write: impl FnMut(u64, &[u8]) -> Result<(), E>,
) -> Result<Made, Error<E>> {
    let spc = per_cluster(v.sectors);
    let total = u32::try_from(v.sectors).map_err(|_| Error::TooSmall)?;
    // The specification's FAT size: enough entries for the clusters left.
    let tmp1 = total.checked_sub(RESERVED).ok_or(Error::TooSmall)?;
    let tmp2 = (256 * spc + FATS) / 2;
    let fat_sectors = tmp1.div_ceil(tmp2);
    let clusters = (total - RESERVED - FATS * fat_sectors) / spc;
    if clusters < 65_525 {
        return Err(Error::TooSmall);
    }
    let cluster_bytes = spc as usize * SECTOR;

    // The tree of names.
    let mut nodes: Vec<Node> = Vec::new();
    nodes.try_reserve(files.len() + 1).map_err(|_| Error::NoMemory)?;
    nodes.push(Node {
        name: String::new(),
        file: None,
        children: Vec::new(),
        parent: 0,
        first: 0,
        count: 0,
    });
    for (i, (path, _)) in files.iter().enumerate() {
        let mut at = 0;
        let parts: Vec<&str> = path.split('/').collect();
        for (k, part) in parts.iter().enumerate() {
            if part.is_empty() {
                return Err(Error::BadPath);
            }
            let last = k + 1 == parts.len();
            let found = nodes[at]
                .children
                .iter()
                .copied()
                .find(|&c| nodes[c].name.eq_ignore_ascii_case(part));
            at = match found {
                Some(c) if last || nodes[c].file.is_some() => return Err(Error::BadPath),
                Some(c) => c,
                None => {
                    nodes.try_reserve(1).map_err(|_| Error::NoMemory)?;
                    let n = nodes.len();
                    nodes.push(Node {
                        name: String::from(*part),
                        file: last.then_some(i),
                        children: Vec::new(),
                        parent: at,
                        first: 0,
                        count: 0,
                    });
                    nodes[at].children.try_reserve(1).map_err(|_| Error::NoMemory)?;
                    nodes[at].children.push(n);
                    n
                }
            };
        }
    }

    // Clusters, in order: each directory (the root first), each file.
    let dir_bytes = |nodes: &[Node], d: usize| -> usize {
        let dots = if d == 0 { 0 } else { 2 };
        32 * (dots
            + nodes[d].children.iter().map(|&c| long_entries(&nodes[c].name) + 1).sum::<usize>())
    };
    let mut next = 2u32;
    let mut order: Vec<usize> = Vec::new();
    order.try_reserve(nodes.len()).map_err(|_| Error::NoMemory)?;
    order.push(0);
    let mut k = 0;
    while k < order.len() {
        let d = order[k];
        k += 1;
        let bytes = match nodes[d].file {
            None => dir_bytes(&nodes, d).max(1),
            Some(f) => files[f].1.len(),
        };
        let count = u32::try_from(bytes.div_ceil(cluster_bytes)).map_err(|_| Error::Full)?;
        if count > 0 {
            if next - 2 + count > clusters {
                return Err(Error::Full);
            }
            nodes[d].first = next;
            nodes[d].count = count;
            next += count;
        }
        if nodes[d].file.is_none() {
            for &c in &nodes[d].children {
                order.push(c);
            }
        }
    }
    let used = next - 2;

    // The boot sector, FSInfo and their backups.
    let mut bs = [0u8; SECTOR];
    bs[..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
    bs[3..11].copy_from_slice(b"GALA    ");
    put16(&mut bs, 11, SECTOR as u16);
    bs[13] = spc as u8;
    put16(&mut bs, 14, RESERVED as u16);
    bs[16] = FATS as u8;
    bs[21] = 0xf8;
    put16(&mut bs, 24, 63);
    put16(&mut bs, 26, 255);
    put32(&mut bs, 28, v.hidden);
    put32(&mut bs, 32, total);
    put32(&mut bs, 36, fat_sectors);
    put32(&mut bs, 44, 2);
    put16(&mut bs, 48, 1);
    put16(&mut bs, 50, 6);
    bs[64] = 0x80;
    bs[66] = 0x29;
    put32(&mut bs, 67, v.id);
    bs[71..82].copy_from_slice(&v.label);
    bs[82..90].copy_from_slice(b"FAT32   ");
    bs[510] = 0x55;
    bs[511] = 0xaa;
    let mut fsinfo = [0u8; SECTOR];
    put32(&mut fsinfo, 0, 0x4161_5252);
    put32(&mut fsinfo, 484, 0x6141_7272);
    put32(&mut fsinfo, 488, clusters - used);
    put32(&mut fsinfo, 492, next);
    put32(&mut fsinfo, 508, 0xaa55_0000);
    let zero = [0u8; SECTOR];
    for s in 0..RESERVED {
        let sector: &[u8] = match s {
            0 | 6 => &bs,
            1 | 7 => &fsinfo,
            _ => &zero,
        };
        write(u64::from(s), sector).map_err(Error::Write)?;
    }

    // Both FATs: the chains (each node's clusters in a row, the last one
    // ending it), then zeros to the FAT's end.
    let mut fat: Vec<u32> = Vec::new();
    fat.try_reserve(next as usize).map_err(|_| Error::NoMemory)?;
    fat.resize(next as usize, 0);
    fat[0] = 0x0fff_fff8;
    fat[1] = 0x0fff_ffff;
    for n in nodes.iter().filter(|n| n.count > 0) {
        for c in n.first..n.first + n.count {
            fat[c as usize] = if c + 1 == n.first + n.count { 0x0fff_ffff } else { c + 1 };
        }
    }
    let mut chunk = [0u8; 64 * SECTOR];
    for copy in 0..FATS {
        let base = RESERVED + copy * fat_sectors;
        let mut s = 0;
        while s < fat_sectors {
            let n = (fat_sectors - s).min(64);
            chunk.fill(0);
            for i in 0..(n as usize * SECTOR / 4) {
                let cluster = s * (SECTOR as u32 / 4) + i as u32;
                let value = fat.get(cluster as usize).copied().unwrap_or(0);
                put32(&mut chunk, i * 4, value);
            }
            write(u64::from(base + s), &chunk[..n as usize * SECTOR]).map_err(Error::Write)?;
            s += n;
        }
    }

    // The clusters: directories' entries and files' bytes.
    let data = u64::from(RESERVED + FATS * fat_sectors);
    let at = |cluster: u32| data + u64::from(cluster - 2) * u64::from(spc);
    let mut buf: Vec<u8> = Vec::new();
    for &d in &order {
        let node = &nodes[d];
        if node.count == 0 {
            continue;
        }
        let len = node.count as usize * cluster_bytes;
        buf.clear();
        buf.try_reserve(len).map_err(|_| Error::NoMemory)?;
        buf.resize(len, 0);
        match node.file {
            Some(f) => buf[..files[f].1.len()].copy_from_slice(files[f].1),
            None => {
                let mut e = 0;
                if d != 0 {
                    let parent = if node.parent == 0 { 0 } else { nodes[node.parent].first };
                    short_entry(&mut buf[0..32], b".          ", true, node.first, 0);
                    short_entry(&mut buf[32..64], b"..         ", true, parent, 0);
                    e = 64;
                }
                let mut serial = 0u32;
                for &c in &node.children {
                    let child = &nodes[c];
                    let short = match short_of(&child.name) {
                        Some(s) => s,
                        None => {
                            serial += 1;
                            let mut s = *b"G0000000   ";
                            let mut v = serial;
                            for i in (1..8).rev() {
                                s[i] = b"0123456789ABCDEF"[(v & 15) as usize];
                                v >>= 4;
                            }
                            s
                        }
                    };
                    let units: Vec<u16> = child.name.encode_utf16().collect();
                    let n = long_entries(&child.name);
                    let sum = checksum(&short);
                    for k in (0..n).rev() {
                        let entry = &mut buf[e..e + 32];
                        entry[0] = (k as u8 + 1) | if k + 1 == n { 0x40 } else { 0 };
                        entry[11] = 0x0f;
                        entry[13] = sum;
                        for j in 0..13 {
                            let i = k * 13 + j;
                            let unit = match i.cmp(&units.len()) {
                                core::cmp::Ordering::Less => units[i],
                                core::cmp::Ordering::Equal => 0,
                                core::cmp::Ordering::Greater => 0xffff,
                            };
                            let off = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30][j];
                            put16(entry, off, unit);
                        }
                        e += 32;
                    }
                    let size = child.file.map_or(0, |f| files[f].1.len() as u32);
                    short_entry(
                        &mut buf[e..e + 32],
                        &short,
                        child.file.is_none(),
                        child.first,
                        size,
                    );
                    e += 32;
                }
            }
        }
        write(at(node.first), &buf).map_err(Error::Write)?;
    }
    Ok(Made { per_cluster: spc, fat_sectors, clusters, used })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::string::ToString;
    use std::vec;

    /// A reader of what `write` makes, from the specification (apart
    /// from the writer): files by their long (or short) names.
    fn read(image: &[u8]) -> BTreeMap<std::string::String, Vec<u8>> {
        let u16_at = |at: usize| u16::from_le_bytes([image[at], image[at + 1]]);
        let u32_at = |at: usize| u32::from_le_bytes(image[at..at + 4].try_into().unwrap());
        let spc = image[13] as usize;
        let reserved = u16_at(14) as usize;
        let fat = u32_at(36) as usize;
        let data = (reserved + 2 * fat) * SECTOR;
        let next = |c: u32| u32_at(reserved * SECTOR + c as usize * 4) & 0x0fff_ffff;
        let chain = |first: u32| -> Vec<u8> {
            let mut out = vec![];
            let mut c = first;
            while (2..0x0fff_fff8).contains(&c) {
                let at = data + (c as usize - 2) * spc * SECTOR;
                out.extend_from_slice(&image[at..at + spc * SECTOR]);
                c = next(c);
            }
            out
        };
        let mut found = BTreeMap::new();
        let mut dirs = vec![(std::string::String::new(), u32_at(44))];
        while let Some((prefix, cluster)) = dirs.pop() {
            let bytes = chain(cluster);
            let mut long: Vec<u16> = vec![];
            for e in bytes.chunks(32) {
                if e[0] == 0 {
                    break;
                }
                if e[11] == 0x0f {
                    let mut part: Vec<u16> = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30]
                        .iter()
                        .map(|&o| u16::from_le_bytes([e[o], e[o + 1]]))
                        .take_while(|&u| u != 0 && u != 0xffff)
                        .collect();
                    part.extend_from_slice(&long);
                    long = part;
                    continue;
                }
                if e[0] == b'.' {
                    continue;
                }
                let name = if long.is_empty() {
                    let base = std::str::from_utf8(&e[..8]).unwrap().trim_end();
                    let ext = std::str::from_utf8(&e[8..11]).unwrap().trim_end();
                    if ext.is_empty() { base.to_string() } else { std::format!("{base}.{ext}") }
                } else {
                    std::string::String::from_utf16(&long).unwrap()
                };
                long.clear();
                let first = u32::from(u16::from_le_bytes([e[20], e[21]])) << 16
                    | u32::from(u16::from_le_bytes([e[26], e[27]]));
                let path = if prefix.is_empty() { name } else { std::format!("{prefix}/{name}") };
                if e[11] & 0x10 != 0 {
                    dirs.push((path, first));
                } else {
                    let size = u32::from_le_bytes(e[28..32].try_into().unwrap()) as usize;
                    let mut content = if first == 0 { vec![] } else { chain(first) };
                    content.truncate(size);
                    found.insert(path, content);
                }
            }
        }
        found
    }

    fn make(sectors: u64, files: &[(&str, &[u8])]) -> Result<(Vec<u8>, Made), Error<()>> {
        let mut image = vec![0u8; sectors as usize * SECTOR];
        let v = Volume { sectors, hidden: 2048, id: 0x6a1a_0001, label: *b"GALA ESP   " };
        let made = write(&v, files, |s, b| {
            image[s as usize * SECTOR..s as usize * SECTOR + b.len()].copy_from_slice(b);
            Ok(())
        })?;
        Ok((image, made))
    }

    #[test]
    fn files_are_read_back_by_their_names() {
        let big: Vec<u8> = (0..70_000u32).map(|i| i as u8).collect();
        let files: [(&str, &[u8]); 5] = [
            ("EFI/BOOT/BOOTX64.EFI", b"limine"),
            ("boot/kernel", &big),
            ("boot/limine/limine.conf", b"timeout: 0\n"),
            ("boot/Windows/System32/kernel32.dll", b"MZ"),
            ("boot/empty", b""),
        ];
        let (image, made) = make(262_144, &files).unwrap();
        assert_eq!(made.per_cluster, 1);
        assert!(made.clusters >= 65_525);
        let back = read(&image);
        assert_eq!(back.len(), files.len());
        for (path, data) in files {
            assert_eq!(back.get(path).map(Vec::as_slice), Some(data), "{path}");
        }
        // The boot sector's signature and FSInfo's.
        assert_eq!((image[510], image[511]), (0x55, 0xaa));
        assert_eq!(&image[SECTOR..SECTOR + 4], &0x4161_5252u32.to_le_bytes());
        // The same files make the same volume.
        assert_eq!(make(262_144, &files).unwrap().0, image);
    }

    #[test]
    fn a_volume_too_small_for_fat32_is_refused() {
        assert_eq!(make(32_768, &[]).err(), Some(Error::TooSmall));
    }

    #[test]
    fn short_names_and_long_names() {
        assert!(short_of("BOOTX64.EFI").is_some());
        assert!(short_of("EFI").is_some());
        assert!(short_of("kernel").is_none());
        assert!(short_of("limine.conf").is_none());
        assert_eq!(long_entries("limine-bios.sys"), 2);
    }

    /// mtools reads what `write` makes the same (where the host has it):
    /// an implementation apart from both.
    #[test]
    fn mtools_reads_it_too() {
        let Ok(out) = std::process::Command::new("mtype").arg("--version").output() else { return };
        if !out.status.success() {
            return;
        }
        let files: [(&str, &[u8]); 2] =
            [("EFI/BOOT/BOOTX64.EFI", b"limine"), ("boot/limine/limine.conf", b"timeout: 0\n")];
        let (image, _) = make(262_144, &files).unwrap();
        let dir = std::env::temp_dir().join(std::format!("setup-fat32-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("esp.img");
        std::fs::write(&path, &image).unwrap();
        for (name, data) in files {
            let out = std::process::Command::new("mtype")
                .arg("-i")
                .arg(&path)
                .arg(std::format!("::/{name}"))
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{name}: {}",
                std::string::String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(out.stdout, data, "{name}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
