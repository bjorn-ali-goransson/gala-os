//! LZX tests: the specifications' own examples and tables, cabinets made
//! by a small encoder written here from the same specifications (checked
//! in under testdata/lzx and decoded by cabextract and 7-Zip into the
//! expected hashes, see make_fixtures.sh), and, when it has been
//! fetched (`sh user/galapkg/testdata/lzx/fetch_real.sh`), a cabinet
//! Microsoft made: the one inside the DirectX web installer.

use super::encode::{Block, encode, encode_with_positions};
use super::*;
use crate::cab::Cabinet;
use std::string::{String, ToString};
use std::vec::Vec as StdVec;
use std::{format, vec};

fn hex_sha(d: &[u8]) -> String {
    crate::sha256_hex(d).to_ascii_lowercase()
}

fn decode(frames: &[(&[u8], usize)], window: u8) -> Result<Vec<u8>, Error> {
    let mut d = Decoder::new(window)?;
    let mut out = Vec::new();
    for (input, size) in frames {
        d.frame(input, *size, &mut out)?;
    }
    Ok(out)
}

#[test]
fn spec_example_uncompressed_block() {
    // [MS-PATCH] section 3: "abc" as an uncompressed block (the 2-byte
    // chunk-size prefix of LZX DELTA is not part of cabinet LZX).
    let data = [0x00, 0x30, 0x30, 0x00, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0x61, 0x62, 0x63, 0x00];
    assert_eq!(decode(&[(&data, 3)], 15).unwrap(), b"abc");
}

#[test]
fn position_slots_and_footers_match_the_tables() {
    // Cabinet SDK: 32K 30 slots, 64K 32, 128K 34, 256K 36, 512K 38;
    // [MS-PATCH] 2.1.6: 1 MB 42, 2 MB 50 (the slots the base positions
    // of 2.6.2 need).
    let want = [(15, 30), (16, 32), (17, 34), (18, 36), (19, 38), (20, 42), (21, 50)];
    for (bits, slots) in want {
        assert_eq!(position_slots(bits), slots, "window 2^{bits}");
    }
    // [MS-PATCH] 2.6.2's table, sampled.
    let rows = [
        (0, 0, 0),
        (3, 3, 0),
        (4, 4, 1),
        (6, 8, 2),
        (10, 32, 4),
        (24, 4096, 11),
        (29, 24576, 13),
        (34, 131072, 16),
        (36, 262144, 17),
        (42, 1048576, 17),
        (49, 1966080, 17),
        (289, 33423360, 17),
    ];
    for (slot, base, bits) in rows {
        assert_eq!((base_position(slot), footer_bits(slot)), (base, bits), "slot {slot}");
    }
}

/// The cabinet the DirectX web installer carries (dsetup.dll,
/// dsetup32.dll, dxwsetup.exe, dxwsetup.cif, dxwsetup.inf; LZX:21 with
/// E8 translation), fetched by testdata/lzx/fetch_real.sh into
/// target/galapkg-fixtures (it is Microsoft's code, so it is not in the
/// repository); its files' SHA-256 are what cabextract and 7-Zip give.
#[test]
fn microsoft_cabinet_matches_cabextract() {
    let path = std::env::var("GALA_LZX_REAL_CAB").unwrap_or_else(|_| {
        format!("{}/../../target/galapkg-fixtures/dxwebsetup.cab", env!("CARGO_MANIFEST_DIR"))
    });
    let Ok(data) = std::fs::read(&path) else {
        std::eprintln!("skipped: {path} not fetched (testdata/lzx/fetch_real.sh)");
        return;
    };
    assert_eq!(
        hex_sha(&data),
        "82a3dba19cf203ccf41b637e81922d5400a9bf8b7629036d0d2cc11ed4271634",
        "{path} is not the cabinet fetch_real.sh extracts"
    );
    let want = std::fs::read_to_string(format!(
        "{}/testdata/lzx/expected/dxwebsetup.sha256",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let cab = Cabinet::open(&data).unwrap();
    assert!(matches!(cab.folders[0].compression, crate::cab::Compression::Lzx(21)));
    let mut got = String::new();
    cab.extract_all(|f, d| {
        got.push_str(&format!("{}  ./{}\n", hex_sha(d), f.name));
        if let Ok(dir) = std::env::var("GALA_LZX_DUMP") {
            std::fs::write(format!("{dir}/{}", f.name), d).unwrap();
        }
        Ok::<(), ()>(())
    })
    .unwrap();
    assert_eq!(sorted(&got), sorted(&want));
}

fn sorted(s: &str) -> StdVec<String> {
    let mut v: StdVec<String> = s.lines().map(String::from).collect();
    v.sort();
    v
}

/// A deterministic byte source (an LCG).
fn noise(n: usize, seed: u32) -> StdVec<u8> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(1103515245).wrapping_add(12345) & 0x7fff_ffff;
            (x >> 16) as u8
        })
        .collect()
}

/// Text with many repeats, near and far.
fn text(n: usize) -> StdVec<u8> {
    let words = ["gala ", "installs ", "packages ", "from ", "cabinets ", "with ", "LZX\r\n"];
    let mut out = StdVec::new();
    let mut x = 7u32;
    while out.len() < n {
        x = x.wrapping_mul(1103515245).wrapping_add(12345) & 0x7fff_ffff;
        let w = words[(x >> 16) as usize % words.len()];
        out.extend_from_slice(w.as_bytes());
        if (x >> 8).is_multiple_of(97) {
            out.extend_from_slice(format!("line {} ", out.len()).as_bytes());
        }
    }
    out.truncate(n);
    out
}

/// Random chunks repeated at long distances (aligned offset blocks).
fn far_repeats(n: usize) -> StdVec<u8> {
    let base = noise(n / 3, 99);
    let mut out = base.clone();
    let mut k = 0;
    while out.len() < n {
        let at = (k * 7919) % (base.len() - 600);
        out.extend_from_slice(&base[at..at + 300 + k % 250]);
        out.extend_from_slice(&noise(40, k as u32));
        k += 1;
    }
    out.truncate(n);
    out
}

/// x86-like bytes: E8 calls with small displacements, and E8 bytes at
/// every place in the last 12 bytes of the first frames.
fn calls(n: usize) -> StdVec<u8> {
    let mut out = noise(n, 5);
    let mut i = 0;
    while i + 5 < n {
        out[i] = 0xe8;
        let d = ((i as i32 * 37) % 20000) - 10000;
        out[i + 1..i + 5].copy_from_slice(&d.to_le_bytes());
        i += 9 + (i % 13);
    }
    for k in 1..4 {
        for j in 2..12 {
            let at = k * FRAME - j;
            if at < n {
                out[at] = 0xe8;
            }
        }
    }
    out
}

/// A one-folder LZX cabinet ([MS-CAB]) of `files` from encoded frames.
fn cabinet(files: &[(&str, &[u8])], window: u8, frames: &[(StdVec<u8>, usize)]) -> StdVec<u8> {
    let names: usize = files.iter().map(|(n, _)| 16 + n.len() + 1).sum();
    let coff_files = 36 + 8;
    let data_start = coff_files + names;
    let mut out = vec![0u8; data_start];
    out[..4].copy_from_slice(b"MSCF");
    out[16..20].copy_from_slice(&(coff_files as u32).to_le_bytes());
    out[24] = 3;
    out[25] = 1;
    out[26..28].copy_from_slice(&1u16.to_le_bytes());
    out[28..30].copy_from_slice(&(files.len() as u16).to_le_bytes());
    out[36..40].copy_from_slice(&(data_start as u32).to_le_bytes());
    out[40..42].copy_from_slice(&(frames.len() as u16).to_le_bytes());
    out[42..44].copy_from_slice(&(3u16 | (u16::from(window) << 8)).to_le_bytes());
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
    for (payload, size) in frames {
        let mut head = vec![0u8; 8];
        head[4..6].copy_from_slice(&(payload.len() as u16).to_le_bytes());
        head[6..8].copy_from_slice(&(*size as u16).to_le_bytes());
        let c = crate::cab::checksum(payload, 0);
        let c = crate::cab::checksum(&head[4..], c);
        head[..4].copy_from_slice(&c.to_le_bytes());
        out.extend_from_slice(&head);
        out.extend_from_slice(payload);
    }
    let len = out.len() as u32;
    out[8..12].copy_from_slice(&len.to_le_bytes());
    out
}

fn block_len(b: &Block) -> usize {
    match b {
        Block::Verbatim(n) | Block::Aligned(n) | Block::Uncompressed(n) => *n,
    }
}

type Fixture = (&'static str, u8, Option<u32>, StdVec<Block>, StdVec<(&'static str, StdVec<u8>)>);

/// The fixtures: (cabinet name, window, E8 size, blocks, files).
fn fixtures() -> StdVec<Fixture> {
    let mut v = StdVec::new();
    // Verbatim blocks (the trees as deltas from the previous block's),
    // spanning frames; two files.
    let a = text(70_000);
    let b = text(31_000);
    let n = a.len() + b.len();
    v.push((
        "verbatim.cab",
        16,
        None,
        vec![Block::Verbatim(40_000), Block::Verbatim(30_000), Block::Verbatim(n - 70_000)],
        vec![("a.txt", a), ("dir\\b.txt", b)],
    ));
    // Aligned offset blocks over long distances, window 2^21.
    let d = far_repeats(160_000);
    v.push((
        "aligned.cab",
        21,
        None,
        vec![Block::Aligned(80_000), Block::Verbatim(30_000), Block::Aligned(50_000)],
        vec![("far.bin", d)],
    ));
    // Uncompressed blocks: odd sizes, one spanning a frame, one ending
    // on a frame boundary, one whose header ends on a word (16 bits of
    // padding), mixed with verbatim blocks.
    let mut d = noise(8_000, 3);
    d.extend(text(70_000));
    d.extend(noise(8_000, 4));
    let mut blocks = vec![
        Block::Uncompressed(5),
        Block::Verbatim(1_000),
        Block::Uncompressed(40_001),
        Block::Verbatim(FRAME * 2 - 41_006 - 999),
        Block::Uncompressed(999),
    ];
    let used: usize = blocks.iter().map(block_len).sum();
    let mut chosen = None;
    for size in 200..400 {
        let mut t = blocks.clone();
        t.push(Block::Verbatim(size));
        t.push(Block::Uncompressed(d.len() - used - size));
        let (_, pos) = encode_with_positions(15, None, &t, &d);
        if (pos[t.len() - 1] + 27) % 16 == 0 {
            chosen = Some(size);
            break;
        }
    }
    let size = chosen.expect("a size that puts the header's end on a word");
    blocks.push(Block::Verbatim(size));
    blocks.push(Block::Uncompressed(d.len() - used - size));
    v.push(("uncompressed.cab", 15, None, blocks, vec![("mixed.bin", d)]));
    // E8 translation (x86-like calls), window 2^17.
    let d = calls(70_000);
    v.push(("e8.cab", 17, Some(12_000_000), vec![Block::Verbatim(70_000)], vec![("code.bin", d)]));
    v
}

fn fixture_cabinet(
    window: u8,
    e8: Option<u32>,
    blocks: &[Block],
    files: &[(&'static str, StdVec<u8>)],
) -> StdVec<u8> {
    let data: StdVec<u8> = files.iter().flat_map(|(_, d)| d.iter().copied()).collect();
    let frames = encode(window, e8, blocks, &data);
    let refs: StdVec<(&str, &[u8])> = files.iter().map(|(n, d)| (*n, d.as_slice())).collect();
    cabinet(&refs, window, &frames)
}

/// Writes the fixtures into $GALA_LZX_FIXTURES (make_fixtures.sh runs
/// this, then cabextract and 7-Zip on them).
#[test]
#[ignore]
fn write_fixtures() {
    let dir = std::env::var("GALA_LZX_FIXTURES").expect("GALA_LZX_FIXTURES");
    for (name, window, e8, blocks, files) in fixtures() {
        let cab = fixture_cabinet(window, e8, &blocks, &files);
        std::fs::write(format!("{dir}/{name}"), cab).unwrap();
    }
}

#[test]
fn encoder_round_trips() {
    for (name, window, e8, blocks, files) in fixtures() {
        let data: StdVec<u8> = files.iter().flat_map(|(_, d)| d.iter().copied()).collect();
        let frames = encode(window, e8, &blocks, &data);
        let refs: StdVec<(&[u8], usize)> = frames.iter().map(|(f, n)| (f.as_slice(), *n)).collect();
        let got = decode(&refs, window).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(got == data, "{name}: decoded data differs");
        for (f, _) in &frames {
            assert!(f.len() <= FRAME + 6144, "{name}: a frame of {} bytes", f.len());
        }
    }
}

/// The checked-in cabinets decode to what cabextract and 7-Zip gave.
#[test]
fn fixtures_match_cabextract_and_7z() {
    let dir = format!("{}/testdata/lzx", env!("CARGO_MANIFEST_DIR"));
    for (name, window, e8, blocks, files) in fixtures() {
        let data = std::fs::read(format!("{dir}/{name}")).unwrap();
        // The generator still makes the checked-in file.
        assert!(data == fixture_cabinet(window, e8, &blocks, &files), "{name}: regenerate");
        let stem = name.trim_end_matches(".cab");
        for tool in ["cabextract", "7z"] {
            let want =
                std::fs::read_to_string(format!("{dir}/expected/{stem}.{tool}.sha256")).unwrap();
            let cab = Cabinet::open(&data).unwrap();
            let mut got = String::new();
            cab.extract_all(|f, d| {
                got.push_str(&format!("{}  ./{}\n", hex_sha(d), f.name.replace('\\', "/")));
                Ok::<(), ()>(())
            })
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(sorted(&got), sorted(&want), "{name} against {tool}");
        }
    }
}

#[test]
fn e8_translation_round_trips() {
    let orig = calls(3 * FRAME + 100);
    for size in [1_000u32, 12_000_000, 0x7fff_ffff] {
        let mut d = orig.clone();
        for (k, c) in d.chunks_mut(FRAME).enumerate() {
            super::encode::e8_forward(c, (k * FRAME) as u32, size);
        }
        assert!(d != orig);
        for (k, c) in d.chunks_mut(FRAME).enumerate() {
            translate_e8(c, (k * FRAME) as u32, size);
        }
        assert!(d == orig, "E8 size {size}");
    }
    // A call at 100 to +20 becomes 120 absolute and back; one in the
    // last 10 bytes of a frame stays as it is.
    let mut d = vec![0u8; 200];
    d[100] = 0xe8;
    d[101..105].copy_from_slice(&20i32.to_le_bytes());
    d[191] = 0xe8;
    d[192..196].copy_from_slice(&5i32.to_le_bytes());
    let before = d.clone();
    super::encode::e8_forward(&mut d, 0, 10_000);
    assert_eq!(&d[101..105], &120i32.to_le_bytes());
    assert_eq!(&d[192..196], &5i32.to_le_bytes());
    translate_e8(&mut d, 0, 10_000);
    assert!(d == before);
}

#[test]
fn damage_is_reported_and_never_panics() {
    let d = text(50_000);
    let frames = encode(16, None, &[Block::Aligned(20_000), Block::Verbatim(30_000)], &d);
    // Block type 0.
    let e = decode(&[(&[0x00, 0x00, 0x00, 0x00], 10)], 16).unwrap_err().to_string();
    assert!(e.contains("frame 0") && e.contains("block type 0"), "{e}");
    // A window cabinets do not have.
    assert!(Decoder::new(22).is_err() && Decoder::new(14).is_err());
    // An uncompressed block whose R0-R2 are missing.
    let e = decode(&[(&[0x00, 0x30, 0x30, 0x00, 1, 0], 3)], 15).unwrap_err().to_string();
    assert!(e.contains("R0-R2"), "{e}");
    // Frame 0 cut short: the bitstream runs out.
    let e = decode(&[(&frames[0].0[..40], frames[0].1)], 16).unwrap_err().to_string();
    assert!(e.contains("frame 0"), "{e}");
    // Random damage and truncation.
    let mut x = 0x1234_5678u32;
    for i in 0..400 {
        let mut f: StdVec<(StdVec<u8>, usize)> = frames.clone();
        for _ in 0..1 + i % 4 {
            x = x.wrapping_mul(1103515245).wrapping_add(12345);
            let k = (x >> 8) as usize % f.len();
            let len = f[k].0.len().max(1);
            if f[k].0.is_empty() {
                continue;
            }
            let at = (x >> 4) as usize % len;
            f[k].0[at] ^= 1 << (x % 8);
            if i % 7 == 0 {
                f[k].0.truncate(at);
            }
        }
        let refs: StdVec<(&[u8], usize)> = f.iter().map(|(a, n)| (a.as_slice(), *n)).collect();
        let _ = decode(&refs, 16);
    }
}
