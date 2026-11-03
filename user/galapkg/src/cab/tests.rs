//! Cabinet tests against gcab's cabinets, with the expected names, sizes,
//! times and contents recorded from `gcab -l`, `7z l` and `gcab -x`
//! (testdata/msi/expected, made by make_fixtures.sh), and damaged ones.

use super::*;
use alloc::vec;
use std::string::ToString;
use std::vec::Vec as StdVec;

fn fixture(name: &str) -> Vec<u8> {
    crate::cfb::tests::fixture(name)
}

fn expected(name: &str) -> String {
    let path = std::format!("{}/testdata/msi/expected/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// `sha256  ./path` lines as (path with `\`, lowercase hex).
pub(crate) fn sums(name: &str) -> StdVec<(String, String)> {
    expected(name)
        .lines()
        .filter_map(|l| l.split_once("  "))
        .map(|(h, p)| (p.trim_start_matches("./").replace('/', "\\"), h.to_string()))
        .collect()
}

fn dos_time(date: u16, time: u16) -> String {
    std::format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        1980 + (date >> 9),
        (date >> 5) & 15,
        date & 31,
        time >> 11,
        (time >> 5) & 63,
        (time & 31) * 2
    )
}

#[test]
fn gcab_cabinets_match_gcab_and_7z() {
    for (cab, compression) in [("mszip", Compression::MsZip), ("stored", Compression::None)] {
        let data = fixture(&std::format!("{cab}.cab"));
        let c = Cabinet::open(&data).unwrap();
        assert!(c.folders.iter().all(|f| f.compression == compression));
        // gcab -l: name, size, time, attributes.
        let listed: StdVec<String> = c
            .files
            .iter()
            .map(|f| {
                std::format!(
                    "{} {} {} {:#x}",
                    f.name,
                    f.size,
                    dos_time(f.date, f.time),
                    f.attributes
                )
            })
            .collect();
        let want: StdVec<String> =
            expected(&std::format!("{cab}.gcab-t")).lines().map(String::from).collect();
        assert_eq!(listed, want);
        // 7z l: the total of the sizes and the cabinet's size.
        let summary = expected(&std::format!("{cab}.7z-l"));
        let last = summary.lines().last().unwrap();
        let total: u32 = c.files.iter().map(|f| f.size).sum();
        assert!(last.contains(&std::format!(" {total} ")), "{last}");
        assert!(last.contains(&std::format!(" {} ", data.len())), "{last}");
        // gcab -x: the contents.
        let want = sums(&std::format!("{cab}.sha256"));
        let mut seen = 0;
        c.extract_all(|f, bytes| {
            let (_, hash) = want.iter().find(|(p, _)| *p == f.name).unwrap();
            assert_eq!(&crate::sha256_hex(bytes).to_lowercase(), hash, "{}", f.name);
            seen += 1;
            Ok::<(), ()>(())
        })
        .unwrap();
        assert_eq!(seen, want.len());
        for (i, f) in c.files.iter().enumerate() {
            assert_eq!(c.extract(i).unwrap().len() as u32, f.size);
        }
    }
}

#[test]
fn mszip_spans_blocks_with_shared_history() {
    // big.txt (112000 bytes) needs four 32 KiB blocks; its later blocks
    // refer back into earlier ones.
    let data = fixture("mszip.cab");
    let c = Cabinet::open(&data).unwrap();
    assert!(c.folders[0].blocks >= 4, "{:?}", c.folders);
    let i = c.files.iter().position(|f| f.name == "dir\\big.txt").unwrap();
    let big = c.extract(i).unwrap();
    assert!(big.starts_with(b"line 00000 of the big file\r\n"));
    assert!(big.ends_with(b"line 03999 of the big file\r\n"));
}

#[test]
fn checksum_known_value() {
    // [MS-CAB]: the leftover bytes are combined first-highest.
    assert_eq!(checksum(b"", 0), 0);
    assert_eq!(checksum(&[1, 2, 3, 4], 0), 0x0403_0201);
    assert_eq!(checksum(&[1, 2, 3, 4, 5, 6, 7], 0), 0x0403_0201 ^ 0x0005_0607);
    assert_eq!(checksum(&[9], 0x1000), 0x1009);
}

/// A one-folder cabinet of `files` (stored, or MSZIP), made from
/// [MS-CAB] like the reader (7-Zip and gcab read what it makes).
pub(crate) fn build(files: &[(&str, &[u8])], mszip: bool) -> Vec<u8> {
    build_with(files, mszip, None)
}

/// As [`build`], with reserved areas: `reserve` is (header bytes,
/// per-folder bytes, per-block bytes), filled with 0xAA.
pub(crate) fn build_with(
    files: &[(&str, &[u8])],
    mszip: bool,
    reserve: Option<(u16, u8, u8)>,
) -> Vec<u8> {
    let mut folder = Vec::new();
    for (_, d) in files {
        folder.extend_from_slice(d);
    }
    let mut blocks: StdVec<(Vec<u8>, usize)> = StdVec::new();
    for chunk in folder.chunks(MAX_BLOCK) {
        let payload = if mszip {
            // Each block alone (no shared history): valid MSZIP too.
            let mut p = b"CK".to_vec();
            p.extend_from_slice(&miniz_oxide::deflate::compress_to_vec(chunk, 6));
            p
        } else {
            chunk.to_vec()
        };
        blocks.push((payload, chunk.len()));
    }
    let (rh, rf, rd) =
        reserve.map_or((0, 0, 0), |(h, f, d)| (usize::from(h), usize::from(f), usize::from(d)));
    let fixed = if reserve.is_some() { 40 + rh } else { 36 };
    let names: usize = files.iter().map(|(n, _)| 16 + n.len() + 1).sum();
    let coff_files = fixed + 8 + rf;
    let data_start = coff_files + names;
    let mut out = vec![0u8; data_start];
    out[..4].copy_from_slice(b"MSCF");
    out[16..20].copy_from_slice(&(coff_files as u32).to_le_bytes());
    out[24] = 3;
    out[25] = 1;
    out[26..28].copy_from_slice(&1u16.to_le_bytes());
    out[28..30].copy_from_slice(&(files.len() as u16).to_le_bytes());
    if let Some((h, f, d)) = reserve {
        out[30..32].copy_from_slice(&FLAG_RESERVE_PRESENT.to_le_bytes());
        out[36..38].copy_from_slice(&h.to_le_bytes());
        out[38] = f;
        out[39] = d;
        out[40..40 + rh].fill(0xaa);
        out[fixed + 8..fixed + 8 + rf].fill(0xaa);
    }
    out[fixed..fixed + 4].copy_from_slice(&(data_start as u32).to_le_bytes());
    out[fixed + 4..fixed + 6].copy_from_slice(&(blocks.len() as u16).to_le_bytes());
    out[fixed + 6..fixed + 8].copy_from_slice(&(u16::from(mszip)).to_le_bytes());
    let mut at = coff_files;
    let mut off = 0u32;
    for (n, d) in files {
        out[at..at + 4].copy_from_slice(&(d.len() as u32).to_le_bytes());
        out[at + 4..at + 8].copy_from_slice(&off.to_le_bytes());
        out[at + 10..at + 12].copy_from_slice(&0x5c22u16.to_le_bytes());
        out[at + 14..at + 16].copy_from_slice(&0x20u16.to_le_bytes());
        out[at + 16..at + 16 + n.len()].copy_from_slice(n.as_bytes());
        at += 16 + n.len() + 1;
        off += d.len() as u32;
    }
    for (payload, uncomp) in blocks {
        let mut head = vec![0u8; 8 + rd];
        head[4..6].copy_from_slice(&(payload.len() as u16).to_le_bytes());
        head[6..8].copy_from_slice(&(uncomp as u16).to_le_bytes());
        head[8..].fill(0xaa);
        let c = checksum(&payload, 0);
        let c = checksum(&head[4..], c);
        head[..4].copy_from_slice(&c.to_le_bytes());
        out.extend_from_slice(&head);
        out.extend_from_slice(&payload);
    }
    let len = out.len() as u32;
    out[8..12].copy_from_slice(&len.to_le_bytes());
    out
}

fn err(data: &[u8]) -> String {
    match Cabinet::open(data) {
        Err(e) => e.to_string(),
        Ok(c) => match c.folder_data(0) {
            Err(e) => e.to_string(),
            Ok(_) => match c.extract(0) {
                Err(e) => e.to_string(),
                Ok(_) => "ok".into(),
            },
        },
    }
}

#[test]
fn built_cabinets_round_trip() {
    let a = vec![7u8; 70_000];
    let files: [(&str, &[u8]); 2] = [("a.bin", &a), ("b.txt", b"hello")];
    for mszip in [false, true] {
        let data = build(&files, mszip);
        let c = Cabinet::open(&data).unwrap();
        assert_eq!(c.extract(0).unwrap(), a);
        assert_eq!(c.extract(1).unwrap(), b"hello");
        // Reserved areas in the header, the folder and each block (the
        // block's reserve is inside its checksum).
        for reserve in [(0, 0, 0), (6, 3, 5), (100, 0, 1)] {
            let data = build_with(&files, mszip, Some(reserve));
            let c = Cabinet::open(&data).unwrap();
            assert_eq!(c.extract(0).unwrap(), a, "{reserve:?}");
            assert_eq!(c.extract(1).unwrap(), b"hello", "{reserve:?}");
        }
    }
}

#[test]
fn damage_is_reported_with_its_place() {
    let a = vec![7u8; 40_000];
    let good = build(&[("a.bin", &a)], true);
    assert_eq!(err(&good), "ok");
    assert!(err(&good[..3]).contains("no MSCF"));
    assert!(
        err(&good[..good.len() - 10]).contains("the header says"),
        "{}",
        err(&good[..good.len() - 10])
    );
    let data_start = u32::from_le_bytes([good[36], good[37], good[38], good[39]]) as usize;
    // A flipped byte in the compressed data: the checksum says so.
    let mut d = good.clone();
    d[data_start + 20] ^= 0x55;
    assert!(err(&d).contains("block 0") && err(&d).contains("checksum"), "{}", err(&d));
    // The same with the checksum zeroed: the inflater notices, or the
    // file comes out different (never a panic).
    d[data_start..data_start + 4].fill(0);
    let e = err(&d);
    assert!(e.contains("MSZIP") || e == "ok", "{e}");
    // No CK.
    let mut d = good.clone();
    d[data_start + 8] = b'X';
    d[data_start..data_start + 4].fill(0);
    assert!(err(&d).contains("no MSZIP signature"), "{}", err(&d));
    // A block bigger than 32 KiB.
    let mut d = good.clone();
    d[data_start + 6..data_start + 8].copy_from_slice(&0xffffu16.to_le_bytes());
    assert!(err(&d).contains("most 32768"), "{}", err(&d));
    // A file outside its folder.
    let mut d = good.clone();
    d[44..48].copy_from_slice(&50_000u32.to_le_bytes());
    assert!(err(&d).contains("file a.bin at 0..50000 of folder 0"), "{}", err(&d));
    // MSZIP data taken as LZX: an LZX error naming the block; a window
    // LZX does not have; Quantum and spanning: refused, saying what.
    let mut d = good.clone();
    d[42..44].copy_from_slice(&0x1503u16.to_le_bytes());
    let e = err(&d);
    assert!(e.contains("folder 0 block 0") && e.contains("LZX"), "{e}");
    d[42..44].copy_from_slice(&0x1603u16.to_le_bytes());
    assert!(err(&d).contains("LZX window 2^22"), "{}", err(&d));
    d[42..44].copy_from_slice(&0x0002u16.to_le_bytes());
    assert!(err(&d).contains("Quantum"), "{}", err(&d));
    let mut d = good.clone();
    d[44 + 8..44 + 10].copy_from_slice(&0xfffeu16.to_le_bytes());
    assert!(err(&d).contains("another cabinet"), "{}", err(&d));
    // A file in a folder that does not exist.
    let mut d = good.clone();
    d[44 + 8..44 + 10].copy_from_slice(&3u16.to_le_bytes());
    assert!(err(&d).contains("folder 3 of 1"), "{}", err(&d));
}

#[test]
fn truncated_and_corrupted_cabinets_never_panic() {
    for data in [fixture("mszip.cab"), fixture("stored.cab")] {
        for len in (0..data.len()).step_by(97) {
            let _ = err(&data[..len]);
        }
        let mut x = 0x9e37_79b9u32;
        for i in 0..1500 {
            let mut d = data.clone();
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let at = if i % 2 == 0 { x as usize % 200.min(d.len()) } else { x as usize % d.len() };
            d[at] ^= (x >> 24) as u8 | 1;
            if let Ok(c) = Cabinet::open(&d) {
                let _ = c.extract_all(|_, _| Ok::<(), ()>(()));
            }
        }
    }
}
