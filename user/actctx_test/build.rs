//! Embeds manifest.xml as the executable's RT_MANIFEST resource 1 (its
//! process's default activation context), as user/manifest_test does: a
//! compiled resource file (.res, "Resource File Formats") with an empty
//! first entry, then the manifest. Shared by wow64_actctx_test.
fn entry(out: &mut Vec<u8>, kind: u16, id: u16, data: &[u8]) {
    let push32 = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
    let push16 = |out: &mut Vec<u8>, v: u16| out.extend_from_slice(&v.to_le_bytes());
    push32(out, data.len() as u32); // DataSize
    push32(out, 32); // HeaderSize
    push16(out, 0xffff); // TYPE: an ordinal
    push16(out, kind);
    push16(out, 0xffff); // NAME: an ordinal
    push16(out, id);
    push32(out, 0); // DataVersion
    push16(out, if kind == 0 { 0 } else { 0x1030 }); // MemoryFlags
    push16(out, if kind == 0 { 0 } else { 0x0409 }); // LanguageId
    push32(out, 0); // Version
    push32(out, 0); // Characteristics
    out.extend_from_slice(data);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let source = format!("{dir}/../actctx_test/manifest.xml");
    println!("cargo:rerun-if-changed={source}");
    let manifest = std::fs::read(&source).unwrap();
    let mut res = Vec::new();
    entry(&mut res, 0, 0, &[]);
    entry(&mut res, 24, 1, &manifest); // RT_MANIFEST, CREATEPROCESS_MANIFEST_RESOURCE_ID
    let path = format!("{out_dir}/manifest.res");
    std::fs::write(&path, res).unwrap();
    println!("cargo:rustc-link-arg-bins={path}");
}
