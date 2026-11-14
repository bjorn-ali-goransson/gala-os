//! Gives the test program (and its 32-bit twin, built from the same
//! source) an RT_HTML resource 1 holding an XSD schema in UTF-16 with a
//! BOM, as App-V's DLLs carry theirs: `schema.rs` adds it to a schema
//! cache through a `res:` URL. The compiled resource file is the .res
//! format documented in "Resource File Formats".

/// The schema (the same text as `schema.rs`'s EXT_XSD).
const EXT_XSD: &str = "<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema' \
targetNamespace='urn:gala:ext' elementFormDefault='qualified'>\
<xs:element name='Ext'><xs:complexType><xs:simpleContent>\
<xs:extension base='xs:string'><xs:attribute name='k' type='xs:hexBinary'/>\
</xs:extension></xs:simpleContent></xs:complexType></xs:element>\
</xs:schema>";

/// A .res entry of an ordinal type and name, US English.
fn entry(out: &mut Vec<u8>, kind: u16, name: u16, data: &[u8]) {
    let empty = kind == 0;
    out.extend_from_slice(&(data.len() as u32).to_le_bytes()); // DataSize
    out.extend_from_slice(&32u32.to_le_bytes()); // HeaderSize
    for id in [kind, name] {
        out.extend_from_slice(&0xffffu16.to_le_bytes());
        out.extend_from_slice(&id.to_le_bytes());
    }
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

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let mut html = vec![0xff, 0xfe];
    html.extend(EXT_XSD.encode_utf16().flat_map(|u| u.to_le_bytes()));
    let mut res = Vec::new();
    entry(&mut res, 0, 0, &[]);
    entry(&mut res, 23, 1, &html); // RT_HTML
    let path = format!("{out_dir}/resources.res");
    std::fs::write(&path, res).unwrap();
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-link-arg={path}");
}
