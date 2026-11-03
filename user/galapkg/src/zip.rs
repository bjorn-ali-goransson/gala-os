//! Reading zip archives (PKWARE's APPNOTE.TXT, the public specification):
//! the end of central directory record, the central directory, local
//! headers; entries stored (method 0) or deflated (method 8, RFC 1951,
//! inflated by `miniz_oxide`), each checked against its CRC-32 and size.
//!
//! Not supported (an error says which): encryption, other compression
//! methods, ZIP64, archives split over several disks. Names are UTF-8
//! when the archive says so (flag bit 11), else code page 437, as the
//! specification has it. [`safe_path`] turns a name into a relative
//! Windows path that cannot leave the directory it is unpacked into
//! (no `..`, no drive or root, no `:` alternate streams).

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// Why an archive could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Not a zip archive, or its structures are cut short or inconsistent.
    Corrupt(&'static str),
    /// A feature this reader does not have.
    Unsupported(&'static str),
    /// An entry's data does not match its CRC-32 or size.
    Damaged(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Corrupt(what) => write!(f, "not a valid zip archive ({what})"),
            Error::Unsupported(what) => write!(f, "zip feature not supported: {what}"),
            Error::Damaged(name) => write!(f, "zip entry {name} is damaged (CRC-32 or size)"),
        }
    }
}

/// An entry of the central directory.
#[derive(Clone, Debug)]
pub struct Entry {
    /// The name as stored (`/` separated; directories end with `/`).
    pub name: String,
    pub method: u16,
    pub flags: u16,
    pub crc32: u32,
    pub compressed_size: u32,
    pub size: u32,
    local_offset: u32,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.name.ends_with('/') || self.name.ends_with('\\')
    }
}

/// An archive held in memory.
pub struct Archive<'a> {
    data: &'a [u8],
    pub entries: Vec<Entry>,
}

fn u16_at(b: &[u8], at: usize) -> Result<u16, Error> {
    b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or(Error::Corrupt("cut short"))
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, Error> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or(Error::Corrupt("cut short"))
}

const EOCD: u32 = 0x0605_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const LOCAL: u32 = 0x0403_4b50;

/// Code page 437's characters 0x80-0xff.
const CP437: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{a0}";

fn name_text(raw: &[u8], utf8: bool) -> Result<String, Error> {
    if utf8 || raw.is_ascii() {
        return core::str::from_utf8(raw)
            .map(ToString::to_string)
            .map_err(|_| Error::Corrupt("a name that is not UTF-8"));
    }
    let high: Vec<char> = CP437.chars().collect();
    Ok(raw
        .iter()
        .map(|&b| if b < 0x80 { char::from(b) } else { high[usize::from(b - 0x80)] })
        .collect())
}

impl<'a> Archive<'a> {
    /// Reads the central directory of `data`.
    pub fn open(data: &'a [u8]) -> Result<Archive<'a>, Error> {
        if data.len() < 22 {
            return Err(Error::Corrupt("too short"));
        }
        // The end record is the last 22 bytes plus a comment of up to
        // 64 KiB: searched backwards.
        let lowest = data.len().saturating_sub(22 + 0xffff);
        let mut at = data.len() - 22;
        let eocd = loop {
            if u32_at(data, at)? == EOCD
                && at + 22 + usize::from(u16_at(data, at + 20)?) == data.len()
            {
                break at;
            }
            if at == lowest {
                return Err(Error::Corrupt("no end of central directory record"));
            }
            at -= 1;
        };
        let disk = u16_at(data, eocd + 4)?;
        let cd_disk = u16_at(data, eocd + 6)?;
        let count = u16_at(data, eocd + 10)?;
        let cd_size = u32_at(data, eocd + 12)?;
        let cd_offset = u32_at(data, eocd + 16)?;
        if disk != 0 || cd_disk != 0 || u16_at(data, eocd + 8)? != count {
            return Err(Error::Unsupported("an archive split over several disks"));
        }
        if count == 0xffff || cd_size == u32::MAX || cd_offset == u32::MAX {
            return Err(Error::Unsupported("ZIP64"));
        }
        let mut entries = Vec::new();
        let mut p = cd_offset as usize;
        for _ in 0..count {
            if u32_at(data, p)? != CENTRAL {
                return Err(Error::Corrupt("a bad central directory entry"));
            }
            let flags = u16_at(data, p + 8)?;
            let method = u16_at(data, p + 10)?;
            let crc32 = u32_at(data, p + 16)?;
            let compressed_size = u32_at(data, p + 20)?;
            let size = u32_at(data, p + 24)?;
            let name_len = usize::from(u16_at(data, p + 28)?);
            let extra_len = usize::from(u16_at(data, p + 30)?);
            let comment_len = usize::from(u16_at(data, p + 32)?);
            let local_offset = u32_at(data, p + 42)?;
            if compressed_size == u32::MAX || size == u32::MAX || local_offset == u32::MAX {
                return Err(Error::Unsupported("ZIP64"));
            }
            let raw = data.get(p + 46..p + 46 + name_len).ok_or(Error::Corrupt("cut short"))?;
            let name = name_text(raw, flags & 0x800 != 0)?;
            entries.push(Entry { name, method, flags, crc32, compressed_size, size, local_offset });
            p += 46 + name_len + extra_len + comment_len;
        }
        Ok(Archive { data, entries })
    }

    /// The entry named `name` (separators `/` or `\`, any case: names
    /// on Windows are case-insensitive).
    pub fn find(&self, name: &str) -> Option<&Entry> {
        let norm = |s: &str| s.replace('\\', "/").trim_start_matches("./").to_ascii_lowercase();
        let want = norm(name);
        self.entries.iter().find(|e| norm(&e.name) == want)
    }

    /// An entry's contents, checked.
    pub fn read(&self, e: &Entry) -> Result<Vec<u8>, Error> {
        if e.flags & 1 != 0 {
            return Err(Error::Unsupported("encrypted entries"));
        }
        let at = e.local_offset as usize;
        if u32_at(self.data, at)? != LOCAL {
            return Err(Error::Corrupt("a bad local header"));
        }
        let name_len = usize::from(u16_at(self.data, at + 26)?);
        let extra_len = usize::from(u16_at(self.data, at + 28)?);
        let start = at + 30 + name_len + extra_len;
        let raw = self
            .data
            .get(start..start + e.compressed_size as usize)
            .ok_or(Error::Corrupt("entry data cut short"))?;
        let out = match e.method {
            0 => raw.to_vec(),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(raw, e.size as usize)
                .map_err(|_| Error::Damaged(e.name.clone()))?,
            _ => {
                return Err(Error::Unsupported(
                    "a compression method other than stored and deflate",
                ));
            }
        };
        if out.len() != e.size as usize || crc32(&out) != e.crc32 {
            return Err(Error::Damaged(e.name.clone()));
        }
        Ok(out)
    }
}

/// CRC-32 (ISO-HDLC, the zip and PNG one: reflected, 0xEDB88320).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// A zip name as a relative Windows path (`\` separated), or None when
/// it would leave the directory: an absolute name, a drive, `..`, `:`
/// (alternate data streams), or a name that is only separators.
pub fn safe_path(name: &str) -> Option<String> {
    if name.starts_with(['/', '\\']) {
        return None;
    }
    let mut out = String::new();
    for part in name.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.contains(':') || part.chars().any(|c| (c as u32) < 32) {
            return None;
        }
        if !out.is_empty() {
            out.push('\\');
        }
        out.push_str(part);
    }
    (!out.is_empty()).then_some(out)
}

/// Writes zip archives (tests only: the install tests' packages).
#[cfg(test)]
pub mod writer {
    use super::crc32;
    use alloc::vec::Vec;

    /// An archive of (name, data) entries, deflated when `deflate`.
    pub fn build(files: &[(&str, &[u8])], deflate: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in files {
            let packed = if deflate {
                miniz_oxide::deflate::compress_to_vec(data, 6)
            } else {
                data.to_vec()
            };
            let method: u16 = if deflate { 8 } else { 0 };
            let offset = out.len() as u32;
            let crc = crc32(data);
            let mut header = Vec::new();
            header.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            header.extend_from_slice(&20u16.to_le_bytes());
            header.extend_from_slice(&0x800u16.to_le_bytes());
            header.extend_from_slice(&method.to_le_bytes());
            header.extend_from_slice(&[0; 4]);
            header.extend_from_slice(&crc.to_le_bytes());
            header.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            header.extend_from_slice(&(data.len() as u32).to_le_bytes());
            header.extend_from_slice(&(name.len() as u16).to_le_bytes());
            header.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&header);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&packed);
            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&header[4..30]);
            central.extend_from_slice(&[0; 10]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd_offset = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn crc32_known_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn round_trip_stored_and_deflated() {
        let big: Vec<u8> = (0..20000u32).map(|i| (i % 251) as u8).collect();
        for deflate in [false, true] {
            let z =
                writer::build(&[("d/", b""), ("d/a.txt", b"hello"), ("d/big.bin", &big)], deflate);
            let a = Archive::open(&z).unwrap();
            assert_eq!(a.entries.len(), 3);
            assert!(a.entries[0].is_dir());
            assert_eq!(a.read(a.find("D\\A.TXT").unwrap()).unwrap(), b"hello");
            assert_eq!(a.read(a.find("d/big.bin").unwrap()).unwrap(), big);
        }
    }

    #[test]
    fn damage_is_found() {
        let mut z = writer::build(&[("a.txt", b"hello hello hello")], false);
        z[30 + 5] ^= 1; // a byte of the data
        let a = Archive::open(&z).unwrap();
        assert_eq!(a.read(&a.entries[0]), Err(Error::Damaged("a.txt".into())));
        assert!(Archive::open(b"PK not a zip at all, really not").is_err());
        assert!(Archive::open(&z[..z.len() - 3]).is_err());
    }

    #[test]
    fn python_zipfile_archive() {
        // Made by Python's zipfile (deflate, a directory entry).
        let z = include_bytes!("../testdata/python.zip");
        let a = Archive::open(z).unwrap();
        let names: Vec<&str> = a.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["tool-1.0/", "tool-1.0/README.txt", "tool-1.0/bin/tool.txt"]);
        let readme = a.read(a.find("tool-1.0/README.txt").unwrap()).unwrap();
        assert!(readme.starts_with(b"tool 1.0"));
    }

    #[test]
    fn paths_stay_inside() {
        assert_eq!(safe_path("a/b/c.exe").as_deref(), Some("a\\b\\c.exe"));
        assert_eq!(safe_path("./a//b/").as_deref(), Some("a\\b"));
        assert_eq!(safe_path("../evil.exe"), None);
        assert_eq!(safe_path("a/../../evil.exe"), None);
        assert_eq!(safe_path("/abs.exe"), None);
        assert_eq!(safe_path("\\abs.exe"), None);
        assert_eq!(safe_path("C:/x.exe"), None);
        assert_eq!(safe_path("a.txt:stream"), None);
        assert_eq!(safe_path("/"), None);
    }

    #[test]
    fn cp437_names() {
        assert_eq!(name_text(&[b'a', 0x81, 0xe1], false).unwrap(), "aüß");
        assert_eq!(name_text("é".as_bytes(), true).unwrap(), "é");
    }
}
