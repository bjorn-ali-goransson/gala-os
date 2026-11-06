//! Export forwarders: these kernel32 exports name ntdll's functions, as on
//! Windows (the loader follows them; defining them here would shadow the
//! ntdll imports of the same name). Those of the architecture built.
//!
//! The version resource: programs read the system's version from
//! kernel32.dll's VS_VERSION_INFO (Chromium's base::win::OSInfo CHECKs it
//! is there), so the DLL carries one with the pinned build (10.0.26100,
//! the update revision of the registry seed's UBR), written here as a
//! compiled resource file (.res, the format documented in "Resource File
//! Formats"; the VS_VERSIONINFO layout as documented for it).

/// The pinned version: major, minor, build, revision (UBR).
const VERSION: [u16; 4] = [10, 0, 26100, 1];

fn words(out: &mut Vec<u8>, text: &str) {
    for c in text.encode_utf16().chain([0]) {
        out.extend_from_slice(&c.to_le_bytes());
    }
}

fn align4(out: &mut Vec<u8>) {
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

/// A version block: wLength, wValueLength, wType, the key, the value
/// (aligned), the children (each aligned). `text`: wType 1, the value's
/// length in characters.
fn block(key: &str, value: &[u8], text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0u8; 6];
    words(&mut b, key);
    align4(&mut b);
    b.extend_from_slice(value);
    for child in children {
        align4(&mut b);
        b.extend_from_slice(child);
    }
    let value_length = if text { value.len() / 2 } else { value.len() };
    let length = b.len() as u16;
    b[0..2].copy_from_slice(&length.to_le_bytes());
    b[2..4].copy_from_slice(&(value_length as u16).to_le_bytes());
    b[4..6].copy_from_slice(&u16::from(text).to_le_bytes());
    b
}

fn string(key: &str, value: &str) -> Vec<u8> {
    let mut v = Vec::new();
    words(&mut v, value);
    block(key, &v, true, &[])
}

fn version_info() -> Vec<u8> {
    let [major, minor, build, revision] = VERSION.map(u32::from);
    let ms = major << 16 | minor;
    let ls = build << 16 | revision;
    // VS_FIXEDFILEINFO: signature, structure version, file and product
    // versions, flags mask and flags, VOS_NT_WINDOWS32, VFT_DLL, no
    // subtype, no date.
    let fixed: [u32; 13] =
        [0xfeef_04bd, 0x0001_0000, ms, ls, ms, ls, 0x3f, 0, 0x0004_0004, 2, 0, 0, 0];
    let fixed: Vec<u8> = fixed.iter().flat_map(|v| v.to_le_bytes()).collect();
    let text = format!("{major}.{minor}.{build}.{revision}");
    let strings = block(
        "040904B0", // US English, Unicode
        &[],
        true,
        &[
            string("CompanyName", "gala"),
            string("FileDescription", "gala's Windows NT base API client DLL"),
            string("FileVersion", &text),
            string("InternalName", "kernel32"),
            string("OriginalFilename", "kernel32.dll"),
            string("ProductName", "gala"),
            string("ProductVersion", &text),
        ],
    );
    let string_info = block("StringFileInfo", &[], true, &[strings]);
    let translation = block("Translation", &[0x09, 0x04, 0xb0, 0x04], false, &[]);
    let var_info = block("VarFileInfo", &[], true, &[translation]);
    block("VS_VERSION_INFO", &fixed, false, &[string_info, var_info])
}

/// One resource of a .res file: its header (sizes, type and name as
/// ordinals, flags, language) and its data.
fn entry(out: &mut Vec<u8>, kind: u16, name: u16, language: u16, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes()); // DataSize
    out.extend_from_slice(&32u32.to_le_bytes()); // HeaderSize
    for id in [kind, name] {
        out.extend_from_slice(&0xffffu16.to_le_bytes());
        out.extend_from_slice(&id.to_le_bytes());
    }
    out.extend_from_slice(&0u32.to_le_bytes()); // DataVersion
    let flags: u16 = if kind == 0 { 0 } else { 0x1030 }; // MOVEABLE | PURE | DISCARDABLE
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&language.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // Version
    out.extend_from_slice(&0u32.to_le_bytes()); // Characteristics
    out.extend_from_slice(data);
    align4(out);
}

fn main() {
    // The x86 build (SysWOW64's kernel32) forwards what the 32-bit ntdll
    // has: x86 has no function tables or virtual unwinding.
    let forwarders: &[&str] = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86") => {
            &["RtlPcToFileHeader", "RtlCaptureContext", "RtlUnwind", "VerSetConditionMask"]
        }
        _ => &[
            "RtlLookupFunctionEntry",
            "RtlVirtualUnwind",
            "RtlPcToFileHeader",
            "RtlCaptureContext",
            "RtlUnwind",
            "RtlUnwindEx",
            "RtlRestoreContext",
            "RtlAddFunctionTable",
            "RtlDeleteFunctionTable",
            "RtlCaptureStackBackTrace",
            "RtlInstallFunctionTableCallback",
            "VerSetConditionMask",
        ],
    };
    for name in forwarders {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name}=ntdll.{name}");
    }

    let mut res = Vec::new();
    entry(&mut res, 0, 0, 0, &[]); // the empty first entry
    entry(&mut res, 16, 1, 0x0409, &version_info()); // RT_VERSION 1
    let path = format!("{}/version.res", std::env::var("OUT_DIR").unwrap());
    std::fs::write(&path, res).unwrap();
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-cdylib-link-arg={path}");
}
