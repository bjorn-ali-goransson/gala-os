//! MSI tests against packages built by wixl, with the expected values
//! recorded from msitools' `msiinfo tables/streams/export/suminfo` and
//! `msiextract` (testdata/msi/expected, made by make_fixtures.sh), plus
//! hand-made pools, tables and directory trees for the edge cases.

use super::*;
use crate::cab::tests::sums;
use crate::cfb::tests::{Node, build, fixture};
use alloc::vec;
use std::string::ToString;
use std::vec::Vec as StdVec;

fn expected(name: &str) -> String {
    let path = std::format!("{}/testdata/msi/expected/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn basic() -> StdVec<u8> {
    fixture("basic.msi")
}

#[test]
fn stream_names_encode_and_decode() {
    // Observed: "File" as a table stream is 4840 430f 422f.
    assert_eq!(encode_name("File", true), [0x4840, 0x430f, 0x422f]);
    assert_eq!(decode_name(&[0x4840, 0x430f, 0x422f]), ("File".into(), true));
    // A lone last letter, '.' and '_'.
    let units = encode_name("Binary.Script", false);
    assert_eq!(units.last(), Some(&(0x4800 + 55))); // 't'
    assert_eq!(decode_name(&units), ("Binary.Script".into(), false));
    for name in ["_StringPool", "a", "", "x.y_z09", "with space", "\u{5}SummaryInformation", "é!"]
    {
        for table in [false, true] {
            assert_eq!(decode_name(&encode_name(name, table)), (name.to_string(), table), "{name}");
        }
    }
}

#[test]
fn tables_and_streams_match_msiinfo() {
    let data = basic();
    let p = Package::open(&data).unwrap();
    // msiinfo tables also lists its two pseudo-tables.
    let mut mine: StdVec<String> = p.tables().to_vec();
    mine.extend(["_ForceCodepage".to_string(), "_SummaryInformation".to_string()]);
    mine.sort();
    let want: StdVec<String> = expected("basic.tables").lines().map(String::from).collect();
    assert_eq!(mine, want);
    let mut streams: StdVec<String> = p.streams().map(String::from).collect();
    streams.sort();
    let want: StdVec<String> = expected("basic.streams").lines().map(String::from).collect();
    assert_eq!(streams, want);
}

/// A table in msiinfo's export format (.idt).
fn idt(t: &Table) -> String {
    let mut s = String::new();
    let names: StdVec<&str> = t.columns.iter().map(|c| c.name.as_str()).collect();
    s += &names.join("\t");
    s += "\r\n";
    let types: StdVec<String> = t.columns.iter().map(|c| c.ty.to_string()).collect();
    s += &types.join("\t");
    s += "\r\n";
    let keys: StdVec<&str> =
        t.columns.iter().filter(|c| c.ty.key()).map(|c| c.name.as_str()).collect();
    s += &std::format!("{}\t{}\r\n", t.name, keys.join("\t"));
    for row in &t.rows {
        let v: StdVec<String> = row.iter().map(|v| v.to_string()).collect();
        s += &v.join("\t");
        s += "\r\n";
    }
    s
}

#[test]
fn tables_match_msiinfo_export() {
    let data = basic();
    let p = Package::open(&data).unwrap();
    for t in [
        "File",
        "Component",
        "Directory",
        "Feature",
        "Registry",
        "Shortcut",
        "Media",
        "CustomAction",
        "Binary",
    ] {
        let mine = idt(&p.table(t).unwrap());
        let want = expected(&std::format!("basic.{t}.idt"));
        // msiinfo may order rows differently: compare the header lines,
        // then the rows as a set.
        let (mh, mr) = mine.split_at(nth_line(&mine, 3));
        let (wh, wr) = want.split_at(nth_line(&want, 3));
        assert_eq!(mh, wh, "table {t}");
        let mut a: StdVec<&str> = mr.split("\r\n").collect();
        let mut b: StdVec<&str> = wr.split("\r\n").collect();
        a.sort();
        b.sort();
        assert_eq!(a, b, "table {t}");
    }
}

fn nth_line(s: &str, n: usize) -> usize {
    s.match_indices("\r\n").nth(n - 1).map_or(s.len(), |(i, _)| i + 2)
}

#[test]
fn typed_rows() {
    let data = basic();
    let p = Package::open(&data).unwrap();
    let files = p.files().unwrap();
    assert_eq!(files.len(), 3);
    let long = files.iter().find(|f| f.file == "long").unwrap();
    assert_eq!(
        (long.name(), long.size, long.sequence, long.component.as_str()),
        ("Long file name.txt", 37, 2, "Main")
    );
    let comps = p.components().unwrap();
    let sub = comps.iter().find(|c| c.component == "Sub").unwrap();
    assert_eq!(sub.component_id.as_deref(), Some("{11111111-2222-3333-4444-888888888888}"));
    assert_eq!((sub.directory.as_str(), sub.key_path.as_deref()), ("SUB", Some("big.txt")));
    let dirs = p.directories().unwrap();
    let target = dirs.iter().find(|d| d.directory == "TARGETDIR").unwrap();
    assert!(target.is_root());
    assert_eq!(target.default_dir, "SourceDir");
    let f = &p.features().unwrap()[0];
    assert_eq!(
        (f.feature.as_str(), f.title.as_deref(), f.level, f.display),
        ("Complete", Some("Everything"), 1, Some(2))
    );
    let reg = p.registry().unwrap();
    assert_eq!(reg.len(), 2);
    let v = reg.iter().find(|r| r.name.as_deref() == Some("Version")).unwrap();
    assert_eq!(
        (v.root, v.key.as_str(), v.value.as_deref()),
        (1, "Software\\Gala\\Test", Some("1.2.3"))
    );
    // Not ASCII: the strings decode (code page 0 holding UTF-8 here).
    let cafe = reg.iter().find(|r| r.name.as_deref() == Some("Café")).unwrap();
    assert_eq!(cafe.value.as_deref(), Some("crème brûlée"));
    let sc = &p.shortcuts().unwrap()[0];
    assert_eq!(
        (sc.name.as_str(), sc.target.as_str(), sc.directory.as_str()),
        ("Gala Test", "[INSTALLDIR]hello.txt", "INSTALLDIR")
    );
    let m = &p.media().unwrap()[0];
    assert_eq!((m.disk_id, m.last_sequence, m.cabinet.as_deref()), (1, 3, Some("#data.cab")));
    let ca = &p.custom_actions().unwrap()[0];
    assert_eq!(
        (ca.action.as_str(), ca.ty, ca.source.as_deref(), ca.target.as_deref(), ca.extended_type),
        ("RunIt", 65, Some("Script"), Some("Main"), None)
    );
    let b = &p.binaries().unwrap()[0];
    assert_eq!((b.name.as_str(), b.stream.as_deref()), ("Script", Some("Binary.Script")));
    // The Binary row's data: msiinfo export's Binary/Binary.Script.
    let (_, hash) = &sums("basic.binary.sha256")[0];
    assert_eq!(&crate::sha256_hex(&p.binary("Script").unwrap()).to_lowercase(), hash);
    assert!(matches!(p.binary("Nope"), Err(Error::NotFound(_))));
    assert!(matches!(p.table("Nope"), Err(Error::NotFound(_))));
}

#[test]
fn summary_matches_msiinfo_suminfo() {
    let data = basic();
    let p = Package::open(&data).unwrap();
    let s = p.summary().unwrap();
    let want = expected("basic.suminfo");
    let field = |k: &str| {
        want.lines().find_map(|l| l.strip_prefix(k).map(|v| v.trim().to_string())).unwrap()
    };
    assert_eq!(s.title.unwrap(), field("Title:"));
    assert_eq!(s.subject.unwrap(), field("Subject:"));
    assert_eq!(s.author.unwrap(), field("Author:"));
    assert_eq!(s.keywords.unwrap(), field("Keywords:"));
    assert_eq!(s.comments.unwrap(), field("Comments:"));
    assert_eq!(s.template.unwrap(), field("Template:"));
    assert_eq!(s.revision.unwrap(), field("Revision number (UUID):"));
    assert_eq!(s.app_name.unwrap(), field("Application:"));
    assert_eq!(
        std::format!("{} ({:x})", s.page_count.unwrap(), s.page_count.unwrap()),
        field("Version:")
    );
    assert_eq!(
        std::format!("{} ({:x})", s.word_count.unwrap(), s.word_count.unwrap()),
        field("Source:")
    );
    assert_eq!(
        std::format!("{} ({:x})", s.security.unwrap(), s.security.unwrap()),
        field("Security:")
    );
}

#[test]
fn install_files_match_msiextract() {
    let data = basic();
    let p = Package::open(&data).unwrap();
    // msiextract lays the files out from TARGETDIR, the standard folders
    // being "." (their DefaultDir).
    let files = p.install_files(&|d| (d == "TARGETDIR").then(|| "X:".to_string())).unwrap();
    let mut paths: StdVec<String> =
        files.iter().map(|f| f.target.trim_start_matches("X:\\").replace('\\', "/")).collect();
    paths.sort();
    let want: StdVec<String> = expected("basic.extract").lines().map(String::from).collect();
    assert_eq!(paths, want);
    assert!(
        files
            .iter()
            .all(|f| f.source == FileSource::Embedded { cabinet: "data.cab".into(), disk_id: 1 })
    );
    // gala's way: the standard folder from the caller.
    let files = p
        .install_files(&|d| match d {
            "LocalAppDataFolder" => Some("C:\\Users\\u\\AppData\\Local".into()),
            _ => None,
        })
        .unwrap();
    let big = files.iter().find(|f| f.file.file == "big.txt").unwrap();
    assert_eq!(big.target, "C:\\Users\\u\\AppData\\Local\\GalaTest\\sub dir\\big.txt");
    assert_eq!(big.directory, "SUB");
    // INSTALLDIR set from outside: TARGETDIR needs no value.
    let files = p.install_files(&|d| (d == "INSTALLDIR").then(|| "D:\\GT".to_string())).unwrap();
    assert_eq!(files[0].target, "D:\\GT\\hello.txt");
    // Nothing set: the directories cannot be placed.
    let e = p.install_files(&|_| None).unwrap_err().to_string();
    assert!(e.contains("target path for directory INSTALLDIR (component Main)"), "{e}");
}

#[test]
fn extraction_matches_msiextract() {
    let data = basic();
    let p = Package::open(&data).unwrap();
    let want = sums("basic.sha256");
    let files = p.install_files(&|d| (d == "TARGETDIR").then(|| "X:".to_string())).unwrap();
    let mut seen = 0;
    p.extract_all(|f, bytes| {
        let target = &files.iter().find(|i| i.file.file == f.file).unwrap().target;
        let rel = target.trim_start_matches("X:\\");
        let (_, hash) = want.iter().find(|(p, _)| p == rel).unwrap();
        assert_eq!(&crate::sha256_hex(bytes).to_lowercase(), hash, "{rel}");
        seen += 1;
    })
    .unwrap();
    assert_eq!(seen, 3);
    let hello = p.extract("hello.txt").unwrap();
    assert_eq!(hello, b"Hello from gala\r\n");
    assert!(matches!(p.extract("nope"), Err(Error::NotFound(_))));
}

#[test]
fn long_string_entry() {
    let z = fixture("long.msi.zlib");
    let data = miniz_oxide::inflate::decompress_to_vec_zlib(&z).unwrap();
    let p = Package::open(&data).unwrap();
    let t = p.table("Property").unwrap();
    let want = expected("long.Property");
    let mut n = 0;
    for line in want.lines() {
        let mut parts = line.split('\t');
        let (k, len, hash) = (parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap());
        let row = t.rows.iter().find(|r| r[0].as_str() == Some(k)).unwrap();
        let v = row[1].as_str().unwrap();
        assert_eq!(v.len().to_string(), len, "{k}");
        assert_eq!(crate::sha256_hex(v.as_bytes()).to_lowercase(), hash, "{k}");
        n += 1;
    }
    assert_eq!(n, t.rows.len());
}

fn pool_bytes(header: u32, strings: &[&[u8]]) -> (StdVec<u8>, StdVec<u8>) {
    let mut pool = header.to_le_bytes().to_vec();
    let mut data = StdVec::new();
    for s in strings {
        if s.len() >= 0x10000 {
            pool.extend_from_slice(&[0, 0, 1, 0]);
            pool.extend_from_slice(&(s.len() as u32).to_le_bytes());
        } else if s.is_empty() {
            pool.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            pool.extend_from_slice(&(s.len() as u16).to_le_bytes());
            pool.extend_from_slice(&1u16.to_le_bytes());
        }
        data.extend_from_slice(s);
    }
    (pool, data)
}

#[test]
fn string_pool_variants() {
    let big = vec![b'z'; 0x12345];
    let (pool, data) = pool_bytes(1252, &[b"one", b"", &big, b"caf\xe9 \x80"]);
    let sp = StringPool::parse(&pool, &data).unwrap();
    assert_eq!((sp.codepage, sp.long_ids, sp.len()), (1252, false, 5));
    assert_eq!(sp.get(1), Some("one"));
    assert_eq!(sp.get(2), Some(""), "an unused id");
    assert_eq!(sp.get(3).unwrap().len(), 0x12345);
    assert_eq!(sp.get(4), Some("café €"), "Windows-1252");
    assert_eq!(sp.get(5), None);
    // The long-id flag: string ids are 3 bytes in tables.
    let (pool, data) = pool_bytes(0x8000_0000 | 65001, &[b"a", "\u{e9}".as_bytes()]);
    let sp = StringPool::parse(&pool, &data).unwrap();
    assert_eq!((sp.codepage, sp.long_ids, sp.id_size()), (65001, true, 3));
    assert_eq!(sp.get(2), Some("é"));
    let cols = [
        Column { name: "K".into(), ty: ColumnType(0x2d48) },
        Column { name: "N".into(), ty: ColumnType(0x1502) },
        Column { name: "L".into(), ty: ColumnType(0x1104) },
    ];
    // Two rows, column-major: K (3 bytes each), N (2), L (4).
    let mut t = StdVec::new();
    t.extend_from_slice(&[1, 0, 0, 2, 0, 0]);
    t.extend_from_slice(&(0x8000u16 ^ 5).to_le_bytes());
    t.extend_from_slice(&[0, 0]);
    t.extend_from_slice(&(0x8000_0000u32 ^ (-7i32 as u32)).to_le_bytes());
    t.extend_from_slice(&0x8000_0000u32.to_le_bytes());
    let rows = read_columns("T", &cols, &t, &sp).unwrap();
    assert_eq!(rows[0], [Value::Str("a".into()), Value::Int(5), Value::Int(-7)]);
    assert_eq!(rows[1], [Value::Str("é".into()), Value::Null, Value::Int(0)]);
    // Errors say where.
    let e = read_columns("T", &cols, &t[..t.len() - 1], &sp).unwrap_err().to_string();
    assert!(e.contains("table T: 17 bytes is not a whole number of 9-byte rows"), "{e}");
    t[3] = 9;
    let e = read_columns("T", &cols, &t, &sp).unwrap_err().to_string();
    assert!(e.contains("table T row 2 column 1 (K): string id 9 of 3"), "{e}");
    let e = StringPool::parse(&pool, &data[..1]).unwrap_err().to_string();
    assert!(e.contains("string id 2: 2 bytes at _StringData offset 1"), "{e}");
    let e =
        StringPool::parse(&pool, &[data.as_slice(), b"extra"].concat()).unwrap_err().to_string();
    assert!(e.contains("add up to 3 bytes, _StringData has 8"), "{e}");
    let e = StringPool::parse(&[0, 0, 0, 0, 0, 0, 1, 0], b"").unwrap_err().to_string();
    assert!(e.contains("string id 1 (a long string) has no length"), "{e}");
    // An unknown code page: decoded with replacements, and flagged.
    let (pool, data) = pool_bytes(932, &[b"\x82\xa0"]);
    let sp = StringPool::parse(&pool, &data).unwrap();
    assert!(sp.lossy);
    assert_eq!(sp.get(1), Some("\u{fffd}\u{fffd}"));
}

fn dir(id: &str, parent: Option<&str>, default: &str) -> Directory {
    Directory { directory: id.into(), parent: parent.map(Into::into), default_dir: default.into() }
}

#[test]
fn directory_resolution() {
    let dirs = [
        dir("TARGETDIR", None, "SourceDir"),
        dir("ProgramFilesFolder", Some("TARGETDIR"), "PFiles"),
        dir("Vendor", Some("ProgramFilesFolder"), "VENDOR~1|Vendor Name:SrcVend"),
        dir("App", Some("Vendor"), "App"),
        dir("Same", Some("App"), "."),
        dir("Bin", Some("Same"), "bin|binaries"),
    ];
    let paths = resolve_directories(&dirs, &|d| match d {
        "TARGETDIR" => Some("C:\\".into()),
        "ProgramFilesFolder" => Some("C:\\Program Files".into()),
        _ => None,
    })
    .unwrap();
    assert_eq!(paths["TARGETDIR"], "C:\\");
    assert_eq!(paths["ProgramFilesFolder"], "C:\\Program Files\\");
    assert_eq!(paths["Vendor"], "C:\\Program Files\\Vendor Name\\");
    assert_eq!(paths["Same"], "C:\\Program Files\\Vendor Name\\App\\");
    assert_eq!(paths["Bin"], "C:\\Program Files\\Vendor Name\\App\\binaries\\");
    // A directory set from outside (INSTALLDIR=... on a command line).
    let paths = resolve_directories(&dirs, &|d| match d {
        "App" => Some("D:\\Apps\\X".into()),
        "ROOTDRIVE" => Some("C:\\".into()),
        _ => None,
    })
    .unwrap();
    assert_eq!(paths["Bin"], "D:\\Apps\\X\\binaries\\");
    assert_eq!(paths["ProgramFilesFolder"], "C:\\PFiles\\", "ROOTDRIVE for TARGETDIR");
    // A loop and a missing parent.
    let looped = [dir("A", Some("B"), "a"), dir("B", Some("A"), "b")];
    let e = resolve_directories(&looped, &|_| None).unwrap_err().to_string();
    assert!(e.contains("is its own ancestor"), "{e}");
    let orphan = [dir("A", Some("Nowhere"), "a")];
    let e = resolve_directories(&orphan, &|_| None).unwrap_err().to_string();
    assert!(e.contains("A's parent Nowhere is not in the table"), "{e}");
}

#[test]
fn not_an_msi_and_damaged_catalogs() {
    // A compound file without a string pool.
    let (plain, _) = build(3, &[crate::cfb::tests::stream("x", b"y")]);
    let e = Package::open(&plain).err().unwrap().to_string();
    assert!(e.contains("_StringPool stream (is this an MSI package?)"), "{e}");
    // A _Columns row numbering its columns wrongly.
    let (pool, data) = pool_bytes(0, &[b"T", b"C"]);
    let s = |n: &str| Node {
        name: encode_name(n, true),
        storage: false,
        parent: 0,
        data: StdVec::new(),
    };
    let mut cols = StdVec::new();
    cols.extend_from_slice(&1u16.to_le_bytes()); // Table: "T"
    cols.extend_from_slice(&(0x8000u16 ^ 2).to_le_bytes()); // Number 2
    cols.extend_from_slice(&2u16.to_le_bytes()); // Name: "C"
    cols.extend_from_slice(&(0x8000u16 ^ 0x1502).to_le_bytes());
    let nodes = [
        Node { data: pool, ..s("_StringPool") },
        Node { data, ..s("_StringData") },
        Node { data: 1u16.to_le_bytes().to_vec(), ..s("_Tables") },
        Node { data: cols, ..s("_Columns") },
    ];
    let (file, _) = build(3, &nodes);
    let e = Package::open(&file).err().unwrap().to_string();
    assert!(e.contains("_Columns: table T column C is numbered 2 (want 1)"), "{e}");
}

/// Opens, reads every table and extracts every file: must not panic.
fn exercise(data: &[u8]) {
    let Ok(p) = Package::open(data) else { return };
    for t in p.tables().to_vec() {
        let _ = p.table(&t);
    }
    let _ = p.summary();
    let _ = p.install_files(&|_| Some("C:".into()));
    let _ = p.extract_all(|_, _| {});
    let _ = p.binary("Script");
}

#[test]
fn truncated_and_corrupted_packages_never_panic() {
    let data = basic();
    exercise(&data);
    for len in (0..data.len()).step_by(211) {
        exercise(&data[..len]);
    }
    let mut x = 0x1234_5678u32;
    for _ in 0..1500 {
        let mut d = data.clone();
        for _ in 0..3 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let at = x as usize % d.len();
            d[at] ^= (x >> 24) as u8 | 1;
        }
        exercise(&d);
    }
}
