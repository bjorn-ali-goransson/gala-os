//! Reading zip archives (PKWARE's APPNOTE.TXT, the public specification):
//! the end of central directory record, the central directory, local
//! headers; entries stored (method 0) or deflated (method 8, RFC 1951,
//! inflated by `miniz_oxide`), each checked against its CRC-32 and size.
//!
//! ZIP64 (APPNOTE 4.3.14-4.3.15, 4.5.3: the zip64 end of central
//! directory record and its locator, the zip64 extended information
//! extra field) is read, as MSIX packages use it; entries of 4 GiB or
//! more are refused (archives are held in memory). Data descriptors
//! (flag bit 3, APPNOTE 4.3.9) need nothing: sizes and CRC-32 come from
//! the central directory, which is authoritative.
//!
//! Not supported (an error says which): encryption, other compression
//! methods, archives split over several disks. Names are UTF-8
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
    local_offset: u64,
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
const ZIP64_EOCD: u32 = 0x0606_4b50;
const ZIP64_LOCATOR: u32 = 0x0706_4b50;

fn u64_at(b: &[u8], at: usize) -> Result<u64, Error> {
    Ok(u64::from(u32_at(b, at)?) | u64::from(u32_at(b, at + 4)?) << 32)
}

fn to_usize(v: u64) -> Result<usize, Error> {
    usize::try_from(v).map_err(|_| Error::Corrupt("an offset beyond the archive"))
}

/// The zip64 extended information extra field (id 1) of a central
/// directory entry: the 64-bit values of the fields saved as all ones,
/// in the order size, compressed size, local header offset.
fn zip64_extra(
    extra: &[u8],
    size: &mut u64,
    csize: &mut u64,
    offset: &mut u64,
) -> Result<(), Error> {
    let mut p = 0;
    while p + 4 <= extra.len() {
        let id = u16_at(extra, p)?;
        let len = usize::from(u16_at(extra, p + 2)?);
        let body =
            extra.get(p + 4..p + 4 + len).ok_or(Error::Corrupt("an extra field cut short"))?;
        if id == 1 {
            let mut q = 0;
            for field in [size, csize, offset] {
                if *field == u64::from(u32::MAX) {
                    *field = u64_at(body, q)
                        .map_err(|_| Error::Corrupt("a zip64 extra field cut short"))?;
                    q += 8;
                }
            }
            return Ok(());
        }
        p += 4 + len;
    }
    Ok(())
}

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
        let mut count = u64::from(u16_at(data, eocd + 10)?);
        let mut cd_offset = u64::from(u32_at(data, eocd + 16)?);
        let saturated = count == 0xffff
            || u32_at(data, eocd + 12)? == u32::MAX
            || cd_offset == u64::from(u32::MAX);
        // A zip64 locator just before the end record points at the zip64
        // end record, whose values replace the saturated ones.
        if eocd >= 20 && u32_at(data, eocd - 20)? == ZIP64_LOCATOR {
            if u32_at(data, eocd - 16)? != 0 || u32_at(data, eocd - 4)? > 1 {
                return Err(Error::Unsupported("an archive split over several disks"));
            }
            let at = to_usize(u64_at(data, eocd - 12)?)?;
            if u32_at(data, at)? != ZIP64_EOCD {
                return Err(Error::Corrupt("a bad zip64 end of central directory record"));
            }
            if u32_at(data, at + 16)? != 0
                || u32_at(data, at + 20)? != 0
                || u64_at(data, at + 24)? != u64_at(data, at + 32)?
            {
                return Err(Error::Unsupported("an archive split over several disks"));
            }
            count = u64_at(data, at + 32)?;
            cd_offset = u64_at(data, at + 48)?;
        } else if saturated {
            return Err(Error::Corrupt(
                "ZIP64 values without a zip64 end of central directory locator",
            ));
        } else if disk != 0 || cd_disk != 0 || u16_at(data, eocd + 8)? != count as u16 {
            return Err(Error::Unsupported("an archive split over several disks"));
        }
        // Every central directory entry is at least 46 bytes: a count
        // beyond that is a lie, not a reason to reserve memory.
        if count > (data.len() / 46) as u64 {
            return Err(Error::Corrupt("more entries than the archive could hold"));
        }
        let mut entries = Vec::new();
        let mut p = to_usize(cd_offset)?;
        for _ in 0..count {
            if u32_at(data, p)? != CENTRAL {
                return Err(Error::Corrupt("a bad central directory entry"));
            }
            let flags = u16_at(data, p + 8)?;
            let method = u16_at(data, p + 10)?;
            let crc32 = u32_at(data, p + 16)?;
            let mut csize = u64::from(u32_at(data, p + 20)?);
            let mut usize_ = u64::from(u32_at(data, p + 24)?);
            let name_len = usize::from(u16_at(data, p + 28)?);
            let extra_len = usize::from(u16_at(data, p + 30)?);
            let comment_len = usize::from(u16_at(data, p + 32)?);
            let mut local_offset = u64::from(u32_at(data, p + 42)?);
            let raw = data.get(p + 46..p + 46 + name_len).ok_or(Error::Corrupt("cut short"))?;
            let name = name_text(raw, flags & 0x800 != 0)?;
            let extra = data
                .get(p + 46 + name_len..p + 46 + name_len + extra_len)
                .ok_or(Error::Corrupt("cut short"))?;
            zip64_extra(extra, &mut usize_, &mut csize, &mut local_offset)?;
            let (Ok(compressed_size), Ok(size)) = (u32::try_from(csize), u32::try_from(usize_))
            else {
                return Err(Error::Unsupported("entries of 4 GiB or more"));
            };
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

    /// The size of an entry's local file header (30 bytes, its name and
    /// its extra field), as MSIX block maps record it (LfhSize).
    pub fn local_header_size(&self, e: &Entry) -> Result<usize, Error> {
        let at = to_usize(e.local_offset)?;
        if u32_at(self.data, at)? != LOCAL {
            return Err(Error::Corrupt("a bad local header"));
        }
        let name_len = usize::from(u16_at(self.data, at + 26)?);
        let extra_len = usize::from(u16_at(self.data, at + 28)?);
        Ok(30 + name_len + extra_len)
    }

    /// Where an entry's (stored or compressed) data starts in the archive.
    pub fn data_offset(&self, e: &Entry) -> Result<usize, Error> {
        Ok(to_usize(e.local_offset)? + self.local_header_size(e)?)
    }

    /// An entry's data as stored in the archive (compressed for method 8),
    /// unchecked.
    pub fn raw(&self, e: &Entry) -> Result<&'a [u8], Error> {
        if e.flags & 1 != 0 {
            return Err(Error::Unsupported("encrypted entries"));
        }
        let start = self.data_offset(e)?;
        self.data
            .get(start..start + e.compressed_size as usize)
            .ok_or(Error::Corrupt("entry data cut short"))
    }

    /// An entry's contents, checked.
    pub fn read(&self, e: &Entry) -> Result<Vec<u8>, Error> {
        let raw = self.raw(e)?;
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

    /// A stored archive written the ZIP64 way, with data descriptors (the
    /// local headers carry no sizes), every central size and offset in
    /// the zip64 extra field and a zip64 end record.
    pub fn build_zip64(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in files {
            let offset = out.len() as u64;
            let crc = crc32(data);
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&45u16.to_le_bytes());
            out.extend_from_slice(&8u16.to_le_bytes()); // data descriptor
            out.extend_from_slice(&[0; 6]); // method 0, time, date
            out.extend_from_slice(&[0; 4]); // CRC-32 in the descriptor
            out.extend_from_slice(&u32::MAX.to_le_bytes());
            out.extend_from_slice(&u32::MAX.to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&1u16.to_le_bytes());
            out.extend_from_slice(&16u16.to_le_bytes());
            out.extend_from_slice(&[0; 16]);
            out.extend_from_slice(data);
            out.extend_from_slice(&0x0807_4b50u32.to_le_bytes());
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(data.len() as u64).to_le_bytes());
            out.extend_from_slice(&(data.len() as u64).to_le_bytes());
            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&45u16.to_le_bytes());
            central.extend_from_slice(&45u16.to_le_bytes());
            central.extend_from_slice(&8u16.to_le_bytes());
            central.extend_from_slice(&[0; 6]); // method 0, time, date
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&u32::MAX.to_le_bytes());
            central.extend_from_slice(&u32::MAX.to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&28u16.to_le_bytes());
            central.extend_from_slice(&[0; 2 + 2 + 2 + 4]);
            central.extend_from_slice(&u32::MAX.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
            central.extend_from_slice(&1u16.to_le_bytes());
            central.extend_from_slice(&24u16.to_le_bytes());
            central.extend_from_slice(&(data.len() as u64).to_le_bytes());
            central.extend_from_slice(&(data.len() as u64).to_le_bytes());
            central.extend_from_slice(&offset.to_le_bytes());
        }
        let cd_offset = out.len() as u64;
        out.extend_from_slice(&central);
        let z64 = out.len() as u64;
        out.extend_from_slice(&0x0606_4b50u32.to_le_bytes());
        out.extend_from_slice(&44u64.to_le_bytes());
        out.extend_from_slice(&45u16.to_le_bytes());
        out.extend_from_slice(&45u16.to_le_bytes());
        out.extend_from_slice(&[0; 8]);
        out.extend_from_slice(&(files.len() as u64).to_le_bytes());
        out.extend_from_slice(&(files.len() as u64).to_le_bytes());
        out.extend_from_slice(&(central.len() as u64).to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0x0706_4b50u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&z64.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&0xffffu16.to_le_bytes());
        out.extend_from_slice(&0xffffu16.to_le_bytes());
        out.extend_from_slice(&u32::MAX.to_le_bytes());
        out.extend_from_slice(&u32::MAX.to_le_bytes());
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
    fn zip64_with_data_descriptors() {
        let z = writer::build_zip64(&[("a.txt", b"hello"), ("dir/b.bin", &[7; 300])]);
        let a = Archive::open(&z).unwrap();
        assert_eq!(a.entries.len(), 2);
        let e = a.find("dir/b.bin").unwrap();
        assert_eq!((e.size, e.compressed_size), (300, 300));
        assert_eq!(a.read(e).unwrap(), [7; 300]);
        assert_eq!(a.read(a.find("a.txt").unwrap()).unwrap(), b"hello");
        // 30 + name + the 20-byte zip64 extra field.
        assert_eq!(a.local_header_size(&a.entries[0]).unwrap(), 30 + 5 + 20);
        assert_eq!(a.raw(&a.entries[0]).unwrap(), b"hello");
        assert_eq!(a.data_offset(&a.entries[0]).unwrap(), 55);
        // The locator pointing elsewhere is found out.
        let mut bad = z.clone();
        let loc = bad.len() - 22 - 20;
        bad[loc + 8] ^= 0x40;
        assert!(Archive::open(&bad).is_err());
        // Saturated values with no locator.
        let mut bad = z.clone();
        bad[loc] = 0;
        assert_eq!(
            Archive::open(&bad).err(),
            Some(Error::Corrupt("ZIP64 values without a zip64 end of central directory locator"))
        );
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
