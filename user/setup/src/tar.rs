//! The release's files in a ustar archive (POSIX.1-1988 `ustar`, as GNU
//! tar writes it): 512-byte headers (name, size in octal, type, and the
//! name's prefix), each file's bytes padded to 512, two zero blocks at the
//! end. Regular files only; directories are implied by the paths.

use alloc::string::String;
use alloc::vec::Vec;

/// A file of the archive: its path (`/`-separated, no leading `/`) and
/// its bytes within the archive.
pub struct Entry<'a> {
    pub path: String,
    pub data: &'a [u8],
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// A header or a file runs past the archive's end.
    Truncated,
    /// A header's checksum or size is not what ustar writes.
    BadHeader,
    NoMemory,
}

fn octal(field: &[u8]) -> Option<u64> {
    let mut v = 0u64;
    let mut any = false;
    for &b in field {
        match b {
            b'0'..=b'7' => {
                v = v.checked_mul(8)?.checked_add(u64::from(b - b'0'))?;
                any = true;
            }
            b' ' | 0 if any => break,
            b' ' | 0 => {}
            _ => return None,
        }
    }
    Some(v)
}

fn text(field: &[u8]) -> &[u8] {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    &field[..end]
}

/// The regular files of `archive`, in archive order.
pub fn entries(archive: &[u8]) -> Result<Vec<Entry<'_>>, Error> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 512 <= archive.len() {
        let h = &archive[at..at + 512];
        if h.iter().all(|&b| b == 0) {
            return Ok(out);
        }
        let want = octal(&h[148..156]).ok_or(Error::BadHeader)?;
        let sum: u64 = h
            .iter()
            .enumerate()
            .map(|(i, &b)| if (148..156).contains(&i) { 32 } else { u64::from(b) })
            .sum();
        if sum != want {
            return Err(Error::BadHeader);
        }
        let size = usize::try_from(octal(&h[124..136]).ok_or(Error::BadHeader)?)
            .map_err(|_| Error::BadHeader)?;
        let start = at + 512;
        let end = start.checked_add(size).ok_or(Error::Truncated)?;
        if end > archive.len() {
            return Err(Error::Truncated);
        }
        let kind = h[156];
        if kind == b'0' || kind == 0 {
            let (prefix, name) = (text(&h[345..500]), text(&h[0..100]));
            let mut path = String::new();
            path.try_reserve(prefix.len() + 1 + name.len()).map_err(|_| Error::NoMemory)?;
            if !prefix.is_empty() {
                path.push_str(core::str::from_utf8(prefix).map_err(|_| Error::BadHeader)?);
                path.push('/');
            }
            path.push_str(core::str::from_utf8(name).map_err(|_| Error::BadHeader)?);
            let path = String::from(path.trim_start_matches("./"));
            if !path.is_empty() && !path.ends_with('/') {
                out.try_reserve(1).map_err(|_| Error::NoMemory)?;
                out.push(Entry { path, data: &archive[start..end] });
            }
        }
        at = start + size.div_ceil(512) * 512;
    }
    Err(Error::Truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    fn header(name: &str, size: usize, kind: u8) -> [u8; 512] {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..107].copy_from_slice(b"0000644");
        let s = std::format!("{size:011o}");
        h[124..135].copy_from_slice(s.as_bytes());
        h[156] = kind;
        h[257..263].copy_from_slice(b"ustar\0");
        h[148..156].fill(b' ');
        let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
        h[148..155].copy_from_slice(std::format!("{sum:06o}\0").as_bytes());
        h
    }

    #[test]
    fn files_are_read_and_directories_skipped() {
        let mut a = vec![];
        a.extend_from_slice(&header("./EFI/", 0, b'5'));
        a.extend_from_slice(&header("./EFI/BOOT/BOOTX64.EFI", 3, b'0'));
        let mut data = [0u8; 512];
        data[..3].copy_from_slice(b"abc");
        a.extend_from_slice(&data);
        a.extend_from_slice(&[0u8; 1024]);
        let e = entries(&a).unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!((e[0].path.as_str(), e[0].data), ("EFI/BOOT/BOOTX64.EFI", &b"abc"[..]));
    }

    #[test]
    fn a_cut_or_damaged_archive_is_an_error() {
        let mut a = vec![];
        a.extend_from_slice(&header("x", 600, b'0'));
        a.extend_from_slice(&[0u8; 512]);
        assert_eq!(entries(&a).err(), Some(Error::Truncated));
        let mut h = header("x", 0, b'0');
        h[0] = b'y';
        assert_eq!(entries(&h).err(), Some(Error::BadHeader));
    }
}
