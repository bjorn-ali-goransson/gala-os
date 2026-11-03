//! Compound file tests: files made by a small writer here (written from
//! [MS-CFB] like the reader, for the edge cases wixl does not make:
//! 4096-byte sectors, more than 109 FAT sectors, nested storages) and
//! the wixl-built fixtures, plus damaged files of every kind.

use super::*;
use alloc::vec;
use std::string::ToString;

/// A stream or storage for [`build`]: `parent` is the index of a
/// previous storage in the list plus one (0: the root).
pub(crate) struct Node {
    pub name: Vec<u16>,
    pub storage: bool,
    pub parent: usize,
    pub data: Vec<u8>,
}

pub(crate) fn stream(name: &str, data: &[u8]) -> Node {
    Node { name: name.encode_utf16().collect(), storage: false, parent: 0, data: data.to_vec() }
}

/// What [`build`] put where, for tests that damage the result.
#[derive(Debug, Default)]
pub(crate) struct Layout {
    pub sector_size: usize,
    pub difat_sectors: usize,
    /// The first directory sector.
    pub dir_sector: u32,
    /// The first sector of each regular stream (by node index).
    pub starts: Vec<u32>,
}

impl Layout {
    pub fn offset(&self, sector: u32) -> usize {
        (sector as usize + 1) * self.sector_size
    }
}

/// Writes a compound file of `version` (3 or 4) holding `nodes`.
pub(crate) fn build(version: u16, nodes: &[Node]) -> (Vec<u8>, Layout) {
    let s = if version == 4 { 4096 } else { 512 };
    let per = s / 4;
    let sectors_of = |n: usize| n.div_ceil(s);

    // The mini stream and mini FAT.
    let mut mini = Vec::new();
    let mut minifat: Vec<u32> = Vec::new();
    let mut mini_start = vec![ENDOFCHAIN; nodes.len()];
    for (i, n) in nodes.iter().enumerate() {
        if n.storage || n.data.is_empty() || n.data.len() >= 4096 {
            continue;
        }
        let first = (mini.len() / 64) as u32;
        let count = n.data.len().div_ceil(64);
        mini_start[i] = first;
        for k in 0..count as u32 {
            minifat.push(if k + 1 == count as u32 { ENDOFCHAIN } else { first + k + 1 });
        }
        mini.extend_from_slice(&n.data);
        mini.resize(mini.len().div_ceil(64) * 64, 0);
    }
    let dir_bytes = (nodes.len() + 1) * 128;
    let mut parts: Vec<Vec<u8>> = Vec::new(); // in sector order after FAT/DIFAT
    let mut dir = vec![0u8; sectors_of(dir_bytes) * s];
    let mut mf = Vec::new();
    for v in &minifat {
        mf.extend_from_slice(&v.to_le_bytes());
    }
    // Data sectors: directory, mini FAT, mini stream, then big streams.
    let d = sectors_of(dir.len())
        + sectors_of(mf.len())
        + sectors_of(mini.len())
        + nodes
            .iter()
            .filter(|n| !n.storage && n.data.len() >= 4096)
            .map(|n| sectors_of(n.data.len()))
            .sum::<usize>();
    let mut nfat = 1usize;
    let mut ndifat;
    loop {
        ndifat = if nfat > 109 { (nfat - 109).div_ceil(per - 1) } else { 0 };
        if nfat * per >= d + nfat + ndifat {
            break;
        }
        nfat += 1;
    }
    let total = nfat + ndifat + d;
    let mut fat = vec![FREESECT; nfat * per];
    for f in fat.iter_mut().take(nfat) {
        *f = FATSECT;
    }
    for f in fat.iter_mut().skip(nfat).take(ndifat) {
        *f = DIFSECT;
    }
    let mut next = (nfat + ndifat) as u32;
    let mut chain = |fat: &mut Vec<u32>, len: usize| -> u32 {
        let n = sectors_of(len) as u32;
        if n == 0 {
            return ENDOFCHAIN;
        }
        let first = next;
        for k in 0..n {
            fat[(first + k) as usize] = if k + 1 == n { ENDOFCHAIN } else { first + k + 1 };
        }
        next += n;
        first
    };
    let dir_sector = chain(&mut fat, dir.len());
    let mf_sector = chain(&mut fat, mf.len());
    let mini_sector = chain(&mut fat, mini.len());
    let mut starts = vec![ENDOFCHAIN; nodes.len()];
    for (i, n) in nodes.iter().enumerate() {
        if !n.storage && n.data.len() >= 4096 {
            starts[i] = chain(&mut fat, n.data.len());
        }
    }
    assert_eq!(next as usize, total);

    // Directory entries: each storage's children chained by right links.
    let mut first_child = vec![NOSTREAM; nodes.len() + 1];
    let mut right = vec![NOSTREAM; nodes.len() + 1];
    for i in (0..nodes.len()).rev() {
        let p = nodes[i].parent;
        right[i + 1] = first_child[p];
        first_child[p] = (i + 1) as u32;
    }
    let mut put = |idx: usize, name: &[u16], kind: u8, start: u32, size: u64| {
        let e = &mut dir[idx * 128..(idx + 1) * 128];
        for (j, &u) in name.iter().enumerate() {
            e[2 * j..2 * j + 2].copy_from_slice(&u.to_le_bytes());
        }
        e[64..66].copy_from_slice(&(((name.len() + 1) * 2) as u16).to_le_bytes());
        e[66] = kind;
        e[67] = 1;
        e[68..72].copy_from_slice(&NOSTREAM.to_le_bytes());
        e[72..76].copy_from_slice(&right[idx].to_le_bytes());
        e[76..80].copy_from_slice(&first_child[idx].to_le_bytes());
        e[116..120].copy_from_slice(&start.to_le_bytes());
        e[120..128].copy_from_slice(&size.to_le_bytes());
    };
    let root_name: Vec<u16> = "Root Entry".encode_utf16().collect();
    put(
        0,
        &root_name,
        5,
        if mini.is_empty() { ENDOFCHAIN } else { mini_sector },
        mini.len() as u64,
    );
    for (i, n) in nodes.iter().enumerate() {
        let (kind, start) = if n.storage {
            (1, 0)
        } else if n.data.len() >= 4096 {
            (2, starts[i])
        } else {
            (2, if n.data.is_empty() { ENDOFCHAIN } else { mini_start[i] })
        };
        put(i + 1, &n.name, kind, start, n.data.len() as u64);
    }
    // Unused entries in the last directory sector stay zero (type 0) with
    // NOSTREAM links.
    for idx in nodes.len() + 1..dir.len() / 128 {
        let e = &mut dir[idx * 128..(idx + 1) * 128];
        e[68..80].fill(0xff);
    }
    parts.push(dir);
    parts.push(mf);
    parts.push(mini);
    for n in nodes {
        if !n.storage && n.data.len() >= 4096 {
            parts.push(n.data.clone());
        }
    }

    // The header.
    let mut out = vec![0u8; s];
    out[..8].copy_from_slice(&SIGNATURE);
    out[24..26].copy_from_slice(&0x3eu16.to_le_bytes());
    out[26..28].copy_from_slice(&version.to_le_bytes());
    out[28..30].copy_from_slice(&0xfffeu16.to_le_bytes());
    out[30..32].copy_from_slice(&(if version == 4 { 12u16 } else { 9 }).to_le_bytes());
    out[32..34].copy_from_slice(&6u16.to_le_bytes());
    if version == 4 {
        out[40..44].copy_from_slice(&(sectors_of(dir_bytes) as u32).to_le_bytes());
    }
    out[44..48].copy_from_slice(&(nfat as u32).to_le_bytes());
    out[48..52].copy_from_slice(&dir_sector.to_le_bytes());
    out[56..60].copy_from_slice(&4096u32.to_le_bytes());
    out[60..64].copy_from_slice(&mf_sector.to_le_bytes());
    out[64..68].copy_from_slice(&(sectors_of(minifat.len() * 4) as u32).to_le_bytes());
    out[68..72].copy_from_slice(&(if ndifat > 0 { nfat as u32 } else { ENDOFCHAIN }).to_le_bytes());
    out[72..76].copy_from_slice(&(ndifat as u32).to_le_bytes());
    for i in 0..109 {
        let v = if i < nfat { i as u32 } else { FREESECT };
        out[76 + 4 * i..80 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    // FAT sectors.
    for v in &fat {
        out.extend_from_slice(&v.to_le_bytes());
    }
    // DIFAT sectors: FAT sector numbers 109.., each ending with the next.
    for k in 0..ndifat {
        let mut sec = vec![0xffu8; s];
        for j in 0..per - 1 {
            let f = 109 + k * (per - 1) + j;
            if f < nfat {
                sec[4 * j..4 * j + 4].copy_from_slice(&(f as u32).to_le_bytes());
            }
        }
        let nxt = if k + 1 == ndifat { ENDOFCHAIN } else { (nfat + k + 1) as u32 };
        sec[s - 4..].copy_from_slice(&nxt.to_le_bytes());
        out.extend_from_slice(&sec);
    }
    for p in parts {
        let len = p.len();
        out.extend_from_slice(&p);
        out.resize(out.len() + (sectors_of(len) * s - len), 0);
    }
    assert_eq!(out.len(), (total + 1) * s);
    let layout = Layout { sector_size: s, difat_sectors: ndifat, dir_sector, starts };
    (out, layout)
}

fn bytes(n: usize, seed: u32) -> Vec<u8> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (x >> 16) as u8
        })
        .collect()
}

fn named(f: &File, parent: usize, name: &str) -> Vec<u8> {
    let units: Vec<u16> = name.encode_utf16().collect();
    let i = f.child(parent, &units).unwrap_or_else(|| panic!("no {name}"));
    f.stream(i).unwrap()
}

#[test]
fn both_versions_regular_and_mini_streams() {
    for version in [3u16, 4] {
        let small = bytes(100, 1);
        let edge_mini = bytes(4095, 2);
        let edge_big = bytes(4096, 3);
        let big = bytes(70_000, 4);
        let nodes = [
            stream("small", &small),
            stream("empty", b""),
            stream("edge mini", &edge_mini),
            stream("edge big", &edge_big),
            stream("big", &big),
        ];
        let (data, layout) = build(version, &nodes);
        assert_eq!(layout.sector_size, if version == 4 { 4096 } else { 512 });
        let f = File::open(&data).unwrap();
        assert_eq!(f.version, version);
        assert_eq!(f.entries()[0].kind, Kind::Root);
        assert_eq!(f.entries()[0].children.len(), 5);
        assert_eq!(named(&f, 0, "small"), small);
        assert_eq!(named(&f, 0, "SMALL"), small, "names compare case-insensitively");
        assert_eq!(named(&f, 0, "empty"), b"");
        assert_eq!(named(&f, 0, "edge mini"), edge_mini);
        assert_eq!(named(&f, 0, "edge big"), edge_big);
        assert_eq!(named(&f, 0, "big"), big);
        assert!(f.root_child("missing").is_none());
    }
}

#[test]
fn nested_storages() {
    let nodes = [
        Node { name: "dir".encode_utf16().collect(), storage: true, parent: 0, data: vec![] },
        Node {
            name: "inner".encode_utf16().collect(),
            storage: false,
            parent: 1,
            data: b"x".to_vec(),
        },
        Node { name: "sub".encode_utf16().collect(), storage: true, parent: 1, data: vec![] },
        Node {
            name: "deep".encode_utf16().collect(),
            storage: false,
            parent: 3,
            data: bytes(5000, 9),
        },
        stream("top", b"top"),
    ];
    let (data, _) = build(3, &nodes);
    let f = File::open(&data).unwrap();
    let dir = f.root_child("dir").unwrap();
    assert_eq!(f.entries()[dir].kind, Kind::Storage);
    assert_eq!(named(&f, dir, "inner"), b"x");
    let sub = f.child(dir, &"sub".encode_utf16().collect::<Vec<_>>()).unwrap();
    assert_eq!(f.entries()[sub].parent, Some(dir));
    assert_eq!(named(&f, sub, "deep"), bytes(5000, 9));
    assert_eq!(named(&f, 0, "top"), b"top");
    assert!(matches!(f.stream(dir), Err(Error::Corrupt(m)) if m.contains("is a storage")));
}

#[test]
fn many_entries_and_a_difat_chain() {
    // 300 entries: a directory of many sectors; one 7.5 MB stream needs
    // more than 109 FAT sectors with 512-byte sectors, so a DIFAT chain.
    let mut nodes: Vec<Node> =
        (0..300).map(|i| stream(&std::format!("s{i}"), &bytes(i * 7, i as u32))).collect();
    let huge = bytes(7_500_000, 77);
    nodes.push(stream("huge", &huge));
    let (data, layout) = build(3, &nodes);
    assert!(layout.difat_sectors >= 1, "{layout:?}");
    let f = File::open(&data).unwrap();
    assert_eq!(f.entries()[0].children.len(), 301);
    for i in [0usize, 1, 150, 299] {
        assert_eq!(named(&f, 0, &std::format!("s{i}")), bytes(i * 7, i as u32));
    }
    assert!(named(&f, 0, "huge") == huge);
}

fn err_of(data: &[u8]) -> String {
    match File::open(data) {
        Ok(_) => "opened".to_string(),
        Err(e) => e.to_string(),
    }
}

fn put32(data: &mut [u8], at: usize, v: u32) {
    data[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

#[test]
fn header_errors_say_what() {
    let (good, _) = build(3, &[stream("a", b"abc")]);
    assert!(err_of(&good[..100]).contains("shorter than the 512-byte header"));
    let mut d = good.clone();
    d[0] = 0;
    assert!(err_of(&d).contains("no signature"));
    let mut d = good.clone();
    d[30] = 12;
    assert!(err_of(&d).contains("version 3 with sector shift 12"), "{}", err_of(&d));
    let mut d = good.clone();
    d[26] = 5;
    assert!(err_of(&d).contains("major version 5"));
    let mut d = good.clone();
    d[28] = 0;
    assert!(err_of(&d).contains("byte order"));
    let mut d = good.clone();
    put32(&mut d, 56, 512);
    assert!(err_of(&d).contains("cutoff 512"));
    let mut d = good.clone();
    put32(&mut d, 44, 1000);
    assert!(err_of(&d).contains("1000 FAT sectors"), "{}", err_of(&d));
    let mut d = good.clone();
    put32(&mut d, 76, 500);
    assert!(err_of(&d).contains("FAT sector 0"), "{}", err_of(&d));
}

#[test]
fn chain_errors_say_where() {
    let big = bytes(10_000, 5);
    let (good, layout) = build(3, &[stream("big", &big), stream("small", b"tiny")]);
    let fat_at = 512; // FAT sector 0 is sector 0.
    let start = layout.starts[0];
    // A loop in the big stream's chain.
    let mut d = good.clone();
    put32(&mut d, fat_at + 4 * (start as usize + 2), start);
    let f = File::open(&d).unwrap();
    let i = f.root_child("big").unwrap();
    let e = f.stream(i).unwrap_err().to_string();
    assert!(e.contains("stream \"big\"") && e.contains("loops"), "{e}");
    // A chain that ends early.
    let mut d = good.clone();
    put32(&mut d, fat_at + 4 * (start as usize + 3), ENDOFCHAIN);
    let f = File::open(&d).unwrap();
    let e = f.stream(f.root_child("big").unwrap()).unwrap_err().to_string();
    assert!(e.contains("reaches ENDOFCHAIN after 2048 of 10000 bytes"), "{e}");
    // A chain pointing past the end of the file.
    let mut d = good.clone();
    put32(&mut d, fat_at + 4 * (start as usize), 0x0fff_0000);
    let f = File::open(&d).unwrap();
    let e = f.stream(f.root_child("big").unwrap()).unwrap_err().to_string();
    assert!(e.contains("0xfff0000"), "{e}");
    // A file cut inside the stream.
    let cut = &good[..good.len() - 700];
    let f = File::open(cut).unwrap();
    let e = f.stream(f.root_child("big").unwrap()).unwrap_err().to_string();
    assert!(e.contains("past the end") || e.contains("ends inside"), "{e}");
    // A stream bigger than the file.
    let mut d = good.clone();
    let dir_at = layout.offset(layout.dir_sector);
    d[dir_at + 128 + 120..dir_at + 128 + 128].copy_from_slice(&(1u64 << 30).to_le_bytes());
    let f = File::open(&d).unwrap();
    let e = f.stream(f.root_child("big").unwrap()).unwrap_err().to_string();
    assert!(e.contains("larger than the file"), "{e}");
    // A mini stream entry larger than the mini stream.
    let mut d = good.clone();
    put32(&mut d, dir_at + 2 * 128 + 120, 3000);
    let f = File::open(&d).unwrap();
    let e = f.stream(f.root_child("small").unwrap()).unwrap_err().to_string();
    assert!(e.contains("larger than the mini stream"), "{e}");
}

#[test]
fn directory_errors_say_which_entry() {
    let (good, layout) = build(3, &[stream("a", b"1"), stream("b", b"2")]);
    let dir_at = layout.offset(layout.dir_sector);
    // b's right sibling back to a: a loop.
    let mut d = good.clone();
    put32(&mut d, dir_at + 2 * 128 + 72, 1);
    assert!(err_of(&d).contains("reached twice"), "{}", err_of(&d));
    // A link past the directory.
    let mut d = good.clone();
    put32(&mut d, dir_at + 2 * 128 + 72, 9999);
    assert!(err_of(&d).contains("link to entry 9999"), "{}", err_of(&d));
    // The root's type.
    let mut d = good.clone();
    d[dir_at + 66] = 1;
    assert!(err_of(&d).contains("entry 0 has type 1"), "{}", err_of(&d));
    // A name length that is odd.
    let mut d = good.clone();
    d[dir_at + 128 + 64] = 3;
    assert!(err_of(&d).contains("entry 1: name length 3"), "{}", err_of(&d));
    // An unallocated entry linked into the tree.
    let mut d = good.clone();
    d[dir_at + 2 * 128 + 66] = 0;
    assert!(err_of(&d).contains("entry 2 (in the tree) has type 0"), "{}", err_of(&d));
}

/// Opens and reads every stream: must not panic, whatever `data` is.
pub(crate) fn exercise(data: &[u8]) -> usize {
    let Ok(f) = File::open(data) else { return 0 };
    let mut ok = 0;
    for i in 0..f.entries().len() {
        if f.stream(i).is_ok() {
            ok += 1;
        }
    }
    ok
}

pub(crate) fn fixture(name: &str) -> Vec<u8> {
    let path = std::format!("{}/testdata/msi/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn wixl_package_opens() {
    let data = fixture("basic.msi");
    let f = File::open(&data).unwrap();
    assert_eq!(f.version, 3);
    // Every root child is a stream (MSI keeps no storages here).
    let root = &f.entries()[0];
    assert!(root.children.len() > 20);
    for &c in &root.children {
        assert_eq!(f.entries()[c].kind, Kind::Stream);
        f.stream(c).unwrap();
    }
    assert!(f.root_child("\u{5}SummaryInformation").is_some());
}

#[test]
fn truncated_and_corrupted_files_never_panic() {
    let msi = fixture("basic.msi");
    let (v4, _) = build(4, &[stream("a", &bytes(9000, 1)), stream("b", b"bb")]);
    for data in [&msi, &v4] {
        let step = (data.len() / 300).max(1);
        for len in (0..data.len()).step_by(step) {
            exercise(&data[..len]);
        }
        // Single-byte damage across the headers, FAT and directory, and
        // random damage elsewhere.
        let mut x = 0x2545_f491u32;
        for i in 0..4000 {
            let mut d = data.clone();
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let at = if i < 2000 {
                (x as usize) % data.len().min(4096 * 3)
            } else {
                x as usize % data.len()
            };
            d[at] ^= (x >> 24) as u8 | 1;
            if i % 3 == 0 {
                // Also a whole 32-bit field set to a special value.
                let at = at & !3;
                if at + 4 <= d.len() {
                    let v = [0u32, 1, ENDOFCHAIN, FREESECT, 0x7fff_ffff, 0xffff_fffa][i % 6];
                    put32(&mut d, at, v);
                }
            }
            exercise(&d);
        }
    }
}
