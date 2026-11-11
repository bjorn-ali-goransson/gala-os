//! tray_test's resources, written as a compiled resource file (.res, the
//! format documented in "Resource File Formats") for the linker: icon
//! group 1 (RT_GROUP_ICON) with one 32 x 32 PNG image (RT_ICON 1:
//! Windows Vista's compressed icon form), and a manifest (RT_MANIFEST 1)
//! that asks for comctl32 6, whose LoadIconMetric the test imports. The
//! PNG (also `$OUT_DIR/icon.png`, which the test reads too) is made here
//! from the PNG specification: IHDR, one IDAT of stored (uncompressed)
//! deflate blocks in a zlib stream, IEND; a blue square with a white
//! middle on a transparent ground.
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

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    b << 16 | a
}

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    png.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let mut typed = kind.to_vec();
    typed.extend_from_slice(body);
    png.extend_from_slice(&typed);
    png.extend_from_slice(&crc32(&typed).to_be_bytes());
}

/// The 32 x 32 RGBA icon: transparent, a blue (#2080F0) square from 4 to
/// 28, white from 12 to 20.
fn png() -> Vec<u8> {
    const SIDE: usize = 32;
    let mut raw = Vec::new();
    for y in 0..SIDE {
        raw.push(0); // filter: none
        for x in 0..SIDE {
            let inside = |a: usize, b: usize| (a..b).contains(&x) && (a..b).contains(&y);
            let px = if inside(12, 20) {
                [0xff, 0xff, 0xff, 0xff]
            } else if inside(4, 28) {
                [0x20, 0x80, 0xf0, 0xff]
            } else {
                [0, 0, 0, 0]
            };
            raw.extend_from_slice(&px);
        }
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, block) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        z.extend_from_slice(&(block.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        z.extend_from_slice(block);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(SIDE as u32).to_be_bytes());
    ihdr.extend_from_slice(&(SIDE as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA, no interlace
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &z);
    chunk(&mut png, b"IEND", &[]);
    png
}

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>
"#;

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let png = png();
    std::fs::write(format!("{out_dir}/icon.png"), &png).unwrap();
    // GRPICONDIR: reserved, type 1 (icons), one entry: 32 x 32, no
    // palette, 1 plane, 32 bits, the image's size, RT_ICON 1.
    let mut group = Vec::new();
    group.extend_from_slice(&[0, 0, 1, 0, 1, 0]);
    group.extend_from_slice(&[32, 32, 0, 0, 1, 0, 32, 0]);
    group.extend_from_slice(&(png.len() as u32).to_le_bytes());
    group.extend_from_slice(&1u16.to_le_bytes());
    let mut res = Vec::new();
    entry(&mut res, 0, 0, &[]);
    entry(&mut res, 3, 1, &png); // RT_ICON
    entry(&mut res, 14, 1, &group); // RT_GROUP_ICON
    entry(&mut res, 24, 1, MANIFEST.as_bytes()); // RT_MANIFEST
    let path = format!("{out_dir}/tray.res");
    std::fs::write(&path, res).unwrap();
    println!("cargo:rustc-link-arg={path}");
}
