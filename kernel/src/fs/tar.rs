//! Reading USTAR archives (POSIX.1-1988 `ustar` interchange format, as
//! IEEE Std 1003.1 "pax" documents it).
//!
//! xtask packs directory trees (a JDK, the .NET runtime) into one archive
//! per tree: Limine cannot load hundreds of separate boot modules. The
//! kernel adds the archive's regular files to the ramfs in place (their
//! contents stay in the module's memory).

/// A regular file in an archive: its path (`/`-separated) and contents.
#[derive(Debug, PartialEq, Eq)]
pub struct Entry<'p, 'a> {
    pub path: &'p str,
    /// The file name (the path's last component), inside the archive.
    pub name: &'a str,
    pub data: &'a [u8],
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// A header or a file's contents run past the end of the archive.
    Truncated,
    /// A header field is not what the format allows.
    BadHeader,
}

/// The NUL-terminated text at the start of `field`.
fn text(field: &[u8]) -> Result<&str, Error> {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    core::str::from_utf8(&field[..end]).map_err(|_| Error::BadHeader)
}

/// An octal number field (digits, then NUL or space padding).
fn octal(field: &[u8]) -> Result<usize, Error> {
    let mut n: usize = 0;
    for &b in field.iter().skip_while(|&&b| b == b' ') {
        match b {
            b'0'..=b'7' => {
                n = n
                    .checked_mul(8)
                    .and_then(|n| n.checked_add((b - b'0') as usize))
                    .ok_or(Error::BadHeader)?
            }
            0 | b' ' => break,
            _ => return Err(Error::BadHeader),
        }
    }
    Ok(n)
}

/// The archive's regular files, in order. Other entries (directories,
/// links) are skipped; a header that does not parse ends the walk with an
/// error.
pub struct Entries<'a> {
    archive: &'a [u8],
    at: usize,
    /// The path of the current entry: `prefix/name` needs a buffer.
    path: [u8; 256],
}

pub fn entries(archive: &[u8]) -> Entries<'_> {
    Entries { archive, at: 0, path: [0; 256] }
}

impl<'a> Entries<'a> {
    /// The next regular file, `Ok(None)` at the end of the archive.
    #[allow(clippy::should_implement_trait)] // the paths borrow `self`
    pub fn next(&mut self) -> Result<Option<Entry<'_, 'a>>, Error> {
        loop {
            let Some(header) = self.archive.get(self.at..self.at + 512) else {
                // No end-of-archive blocks: the end of the data ends it too.
                return if self.at >= self.archive.len() {
                    Ok(None)
                } else {
                    Err(Error::Truncated)
                };
            };
            if header.iter().all(|&b| b == 0) {
                return Ok(None);
            }
            // The checksum: the header's bytes summed, its own field as spaces.
            let sum: usize = header
                .iter()
                .enumerate()
                .map(|(i, &b)| if (148..156).contains(&i) { b' ' as usize } else { b as usize })
                .sum();
            if octal(&header[148..156])? != sum || &header[257..262] != b"ustar" {
                return Err(Error::BadHeader);
            }
            let size = octal(&header[124..136])?;
            let start = self.at + 512;
            let end = start.checked_add(size).ok_or(Error::Truncated)?;
            if end > self.archive.len() {
                return Err(Error::Truncated);
            }
            self.at = start + size.div_ceil(512) * 512;
            if !matches!(header[156], b'0' | 0) {
                continue;
            }
            let archive: &'a [u8] = self.archive;
            let header = &archive[start - 512..start];
            let prefix = text(&header[345..500])?;
            let name = text(&header[0..100])?;
            let file_name = name.rsplit('/').next().unwrap_or(name);
            let mut len = 0;
            for part in [prefix, if prefix.is_empty() { "" } else { "/" }, name] {
                self.path[len..len + part.len()].copy_from_slice(part.as_bytes());
                len += part.len();
            }
            let path = core::str::from_utf8(&self.path[..len]).map_err(|_| Error::BadHeader)?;
            return Ok(Some(Entry { path, name: file_name, data: &archive[start..end] }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    /// A header as xtask writes it (and GNU tar's ustar format).
    fn header(prefix: &str, name: &str, size: usize, kind: u8) -> [u8; 512] {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..107].copy_from_slice(b"0000644");
        h[124..135].copy_from_slice(alloc::format!("{size:011o}").as_bytes());
        h[156] = kind;
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        h[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
        h[148..156].copy_from_slice(b"        ");
        let sum: usize = h.iter().map(|&b| b as usize).sum();
        h[148..155].copy_from_slice(alloc::format!("{sum:06o}\0").as_bytes());
        h
    }

    fn archive(files: &[(&str, &str, &[u8], u8)]) -> Vec<u8> {
        let mut out = Vec::new();
        for &(prefix, name, data, kind) in files {
            out.extend_from_slice(&header(prefix, name, data.len(), kind));
            out.extend_from_slice(data);
            out.resize(out.len().div_ceil(512) * 512, 0);
        }
        out.extend_from_slice(&[0; 1024]);
        out
    }

    fn collect(a: &[u8]) -> Result<Vec<(alloc::string::String, Vec<u8>)>, Error> {
        let mut e = entries(a);
        let mut out = Vec::new();
        while let Some(f) = e.next()? {
            out.push((f.path.into(), f.data.to_vec()));
        }
        Ok(out)
    }

    #[test]
    fn files_and_prefixes() {
        let big = vec![7u8; 1000];
        let a = archive(&[
            ("", "a.txt", b"hello", b'0'),
            ("", "dir/", b"", b'5'),
            ("shared/Microsoft.NETCore.App", "8.0.33/coreclr.dll", &big, b'0'),
            ("", "empty", b"", 0),
        ]);
        let files = collect(&a).unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0], ("a.txt".into(), b"hello".to_vec()));
        assert_eq!(files[1].0, "shared/Microsoft.NETCore.App/8.0.33/coreclr.dll");
        let mut e = entries(&a);
        e.next().unwrap();
        assert_eq!(e.next().unwrap().unwrap().name, "coreclr.dll");
        assert_eq!(files[1].1, big);
        assert_eq!(files[2], ("empty".into(), Vec::new()));
    }

    #[test]
    fn errors() {
        let mut a = archive(&[("", "a.txt", b"hello", b'0')]);
        a[0] = b'b'; // the checksum no longer matches
        assert_eq!(collect(&a), Err(Error::BadHeader));
        let a = archive(&[("", "a.txt", &[1; 600], b'0')]);
        assert_eq!(collect(&a[..700]), Err(Error::Truncated));
        assert_eq!(collect(&a[..100]), Err(Error::Truncated));
        assert_eq!(collect(&[]), Ok(Vec::new()));
    }
}
