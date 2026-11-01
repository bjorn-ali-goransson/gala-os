//! Gives the test program resources (a compiled resource file, the .res
//! format documented in "Resource File Formats"): a string table block,
//! a VS_VERSION_INFO, a named RCDATA resource and a message table.

/// A resource name or type: an ordinal or a string.
enum Id {
    Num(u16),
    Text(&'static str),
}

fn push_id(out: &mut Vec<u8>, id: &Id) {
    match id {
        Id::Num(n) => {
            out.extend_from_slice(&0xffffu16.to_le_bytes());
            out.extend_from_slice(&n.to_le_bytes());
        }
        Id::Text(t) => {
            for c in t.encode_utf16().chain([0]) {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
    }
}

fn entry(out: &mut Vec<u8>, kind: Id, name: Id, data: &[u8]) {
    let mut ids = Vec::new();
    push_id(&mut ids, &kind);
    push_id(&mut ids, &name);
    while !ids.len().is_multiple_of(4) {
        ids.push(0);
    }
    let empty = matches!(kind, Id::Num(0));
    out.extend_from_slice(&(data.len() as u32).to_le_bytes()); // DataSize
    out.extend_from_slice(&(8 + ids.len() as u32 + 16).to_le_bytes()); // HeaderSize
    out.extend_from_slice(&ids);
    out.extend_from_slice(&0u32.to_le_bytes()); // DataVersion
    out.extend_from_slice(&(if empty { 0u16 } else { 0x1030 }).to_le_bytes()); // MemoryFlags
    out.extend_from_slice(&(if empty { 0u16 } else { 0x0409 }).to_le_bytes()); // LanguageId
    out.extend_from_slice(&0u32.to_le_bytes()); // Version
    out.extend_from_slice(&0u32.to_le_bytes()); // Characteristics
    out.extend_from_slice(data);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// String table block 7 (ids 96..=111): 100 is "", 101 "gala string",
/// 102 "second".
fn string_block() -> Vec<u8> {
    let mut words = Vec::new();
    for id in 96..112 {
        let text = match id {
            101 => utf16("gala string"),
            102 => utf16("second"),
            _ => Vec::new(),
        };
        words.push(text.len() as u16);
        words.extend(text);
    }
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// VS_VERSION_INFO with VS_FIXEDFILEINFO (file version 1.2.3.4).
fn version() -> Vec<u8> {
    let mut w: Vec<u16> = vec![0, 52, 0];
    w.extend(utf16("VS_VERSION_INFO"));
    w.push(0);
    w.push(0); // padding to 40 bytes
    let fixed: [u32; 13] = [
        0xfeef_04bd,
        0x0001_0000,
        0x0001_0002,
        0x0003_0004,
        0x0001_0002,
        0x0003_0004,
        0,
        0,
        4,
        1,
        0,
        0,
        0,
    ];
    for v in fixed {
        w.push(v as u16);
        w.push((v >> 16) as u16);
    }
    w[0] = (w.len() * 2) as u16;
    w.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// A message table (MESSAGE_RESOURCE_DATA, as the Message Compiler
/// writes it): ids 100 (Unicode, with an insert) and 101 (ANSI) in one
/// block, 0x4000_0005 in another.
fn message_table() -> Vec<u8> {
    let entry = |text: &str, unicode: bool| {
        let mut bytes: Vec<u8> = if unicode {
            text.encode_utf16().chain([0]).flat_map(|c| c.to_le_bytes()).collect()
        } else {
            text.bytes().chain([0]).collect()
        };
        while !(bytes.len() + 4).is_multiple_of(4) {
            bytes.push(0);
        }
        let mut e = Vec::new();
        e.extend_from_slice(&((bytes.len() + 4) as u16).to_le_bytes()); // Length
        e.extend_from_slice(&u16::from(unicode).to_le_bytes()); // Flags
        e.extend(bytes);
        e
    };
    let first = [entry("Hello %1 from table.\r\n", true), entry("Plain ansi\r\n", false)].concat();
    let second = entry("Warning %1!d!.\r\n", true);
    let entries_at = 4 + 2 * 12;
    let mut out = Vec::new();
    out.extend_from_slice(&2u32.to_le_bytes()); // NumberOfBlocks
    for (low, high, at) in
        [(100u32, 101u32, entries_at), (0x4000_0005, 0x4000_0005, entries_at + first.len())]
    {
        out.extend_from_slice(&low.to_le_bytes());
        out.extend_from_slice(&high.to_le_bytes());
        out.extend_from_slice(&(at as u32).to_le_bytes());
    }
    out.extend(first);
    out.extend(second);
    out
}

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let mut res = Vec::new();
    entry(&mut res, Id::Num(0), Id::Num(0), &[]);
    entry(&mut res, Id::Num(6), Id::Num(7), &string_block()); // RT_STRING
    entry(&mut res, Id::Num(16), Id::Num(1), &version()); // RT_VERSION
    entry(&mut res, Id::Num(10), Id::Text("GALADATA"), b"hello resource"); // RT_RCDATA
    entry(&mut res, Id::Num(11), Id::Num(1), &message_table()); // RT_MESSAGETABLE
    let path = format!("{out_dir}/resources.res");
    std::fs::write(&path, res).unwrap();
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-link-arg={path}");
}
