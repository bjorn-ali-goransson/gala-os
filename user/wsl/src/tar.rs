//! Tar archives as distributions' root file systems come (`wsl --import`
//! takes `docker export`'s output, distributions' rootfs tarballs and
//! `wsl --export`'s, plain or gzip'd): POSIX ustar headers, GNU's long
//! names and links (`L`, `K`) and base-256 numbers, and pax extended
//! headers (`x`: path, linkpath, size, mtime; `g` read past). Written from
//! POSIX.1-2008's pax description of the ustar and pax formats and GNU
//! tar's manual (its "Standard" section); nothing is borrowed.
//!
//! Every header's checksum is checked; an archive ends at a zero block or
//! at the end of the data.

use alloc::vec::Vec;

/// What an entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Directory,
    File,
    Symlink,
    HardLink,
    /// A character or block device or a FIFO.
    Special,
}

/// One member of the archive.
#[derive(Debug)]
pub struct Entry<'a> {
    /// As the archive names it (see `clean`).
    pub path: Vec<u8>,
    pub kind: Kind,
    /// The permission bits (07777).
    pub mode: u32,
    /// Seconds since 1970.
    pub mtime: u64,
    /// A file's bytes.
    pub data: &'a [u8],
    /// A link's target.
    pub link: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// The header at this offset, or its data, runs past the end.
    Truncated(usize),
    /// The header at this offset has a wrong checksum.
    Checksum(usize),
    /// A number field of the header at this offset cannot be read.
    Number(usize),
    OutOfMemory,
}

const BLOCK: usize = 512;

/// Reads the archive's entries one after another.
pub struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
    done: bool,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Reader<'a> {
        Reader { buf, at: 0, done: false }
    }
}

/// The text of a NUL-padded field.
fn field(b: &[u8]) -> &[u8] {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    &b[..end]
}

/// An octal number field (spaces and NULs around it), or GNU's base-256
/// (the first byte's top bit set).
fn number(b: &[u8]) -> Option<u64> {
    if b.first().is_some_and(|&c| c & 0x80 != 0) {
        let mut v: u64 = u64::from(b[0] & 0x7f);
        for &c in &b[1..] {
            v = v.checked_mul(256)?.checked_add(u64::from(c))?;
        }
        return Some(v);
    }
    let mut v: u64 = 0;
    let mut seen = false;
    for &c in b {
        match c {
            b'0'..=b'7' => {
                v = v.checked_mul(8)?.checked_add(u64::from(c - b'0'))?;
                seen = true;
            }
            b' ' | 0 if !seen => {}
            b' ' | 0 => break,
            _ => return None,
        }
    }
    Some(v)
}

fn copy(b: &[u8]) -> Result<Vec<u8>, Error> {
    let mut v = Vec::new();
    v.try_reserve_exact(b.len()).map_err(|_| Error::OutOfMemory)?;
    v.extend_from_slice(b);
    Ok(v)
}

/// What pax extended headers (and GNU long names) say about the next
/// entry.
#[derive(Default)]
struct Overrides {
    path: Option<Vec<u8>>,
    link: Option<Vec<u8>>,
    size: Option<u64>,
    mtime: Option<u64>,
}

/// Reads a pax extended header's records (`<len> <key>=<value>\n`).
fn pax(data: &[u8], o: &mut Overrides, at: usize) -> Result<(), Error> {
    let mut rest = data;
    while !rest.is_empty() {
        let space = rest.iter().position(|&c| c == b' ').ok_or(Error::Number(at))?;
        let len = core::str::from_utf8(&rest[..space])
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|&l| l > space + 1 && l <= rest.len())
            .ok_or(Error::Number(at))?;
        let record = &rest[space + 1..len];
        let record = record.strip_suffix(b"\n").unwrap_or(record);
        if let Some(eq) = record.iter().position(|&c| c == b'=') {
            let (key, value) = (&record[..eq], &record[eq + 1..]);
            let decimal = || {
                let whole = value.split(|&c| c == b'.').next().unwrap_or(b"");
                core::str::from_utf8(whole).ok().and_then(|s| s.parse::<u64>().ok())
            };
            match key {
                b"path" => o.path = Some(copy(value)?),
                b"linkpath" => o.link = Some(copy(value)?),
                b"size" => o.size = Some(decimal().ok_or(Error::Number(at))?),
                b"mtime" => o.mtime = decimal(),
                _ => {}
            }
        }
        rest = &rest[len..];
    }
    Ok(())
}

impl<'a> Iterator for Reader<'a> {
    type Item = Result<Entry<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let r = self.read();
        if !matches!(r, Some(Ok(_))) {
            self.done = true;
        }
        r
    }
}

impl<'a> Reader<'a> {
    fn read(&mut self) -> Option<Result<Entry<'a>, Error>> {
        let mut o = Overrides::default();
        loop {
            let at = self.at;
            if at >= self.buf.len() {
                return None;
            }
            let Some(h) = self.buf.get(at..at + BLOCK) else {
                return Some(Err(Error::Truncated(at)));
            };
            if h.iter().all(|&c| c == 0) {
                return None;
            }
            let Some(sum) = number(&h[148..156]) else { return Some(Err(Error::Number(at))) };
            let actual: u64 = h
                .iter()
                .enumerate()
                .map(|(i, &c)| if (148..156).contains(&i) { 32 } else { u64::from(c) })
                .sum();
            if actual != sum {
                return Some(Err(Error::Checksum(at)));
            }
            let Some(mut size) = number(&h[124..136]) else {
                return Some(Err(Error::Number(at)));
            };
            let kind = h[156];
            if let Some(s) = o.size
                && kind != b'x'
                && kind != b'g'
            {
                size = s;
            }
            let start = at + BLOCK;
            let Some(end) = usize::try_from(size).ok().and_then(|s| start.checked_add(s)) else {
                return Some(Err(Error::Truncated(at)));
            };
            let Some(data) = self.buf.get(start..end) else {
                return Some(Err(Error::Truncated(at)));
            };
            self.at = start + (end - start).next_multiple_of(BLOCK);
            let r = match kind {
                b'L' => copy(field(data)).map(|v| o.path = Some(v)),
                b'K' => copy(field(data)).map(|v| o.link = Some(v)),
                b'x' => pax(data, &mut o, at),
                b'g' => Ok(()),
                _ => {
                    let posix = &h[257..263] == b"ustar\0";
                    let path = match o.path.take() {
                        Some(p) => p,
                        None => {
                            let name = field(&h[0..100]);
                            let prefix = if posix { field(&h[345..500]) } else { b"" };
                            let mut p = Vec::new();
                            if p.try_reserve_exact(prefix.len() + 1 + name.len()).is_err() {
                                return Some(Err(Error::OutOfMemory));
                            }
                            if !prefix.is_empty() {
                                p.extend_from_slice(prefix);
                                p.push(b'/');
                            }
                            p.extend_from_slice(name);
                            p
                        }
                    };
                    let link = match o.link.take() {
                        Some(l) => l,
                        None => match copy(field(&h[157..257])) {
                            Ok(l) => l,
                            Err(e) => return Some(Err(e)),
                        },
                    };
                    let mode = number(&h[100..108]).unwrap_or(0o644) as u32 & 0o7777;
                    let mtime = o.mtime.or_else(|| number(&h[136..148])).unwrap_or(0);
                    let kind = match kind {
                        b'0' | 0 | b'7' => {
                            if path.last() == Some(&b'/') {
                                Kind::Directory
                            } else {
                                Kind::File
                            }
                        }
                        b'1' => Kind::HardLink,
                        b'2' => Kind::Symlink,
                        b'5' => Kind::Directory,
                        _ => Kind::Special,
                    };
                    let data = if kind == Kind::File { data } else { &data[..0] };
                    return Some(Ok(Entry { path, kind, mode, mtime, data, link }));
                }
            };
            if let Err(e) = r {
                return Some(Err(e));
            }
        }
    }
}

/// An entry's path as a path from the root: leading `/` and `./` and a
/// trailing `/` taken off; None for the root itself (`.`, `./`, `/`).
pub fn clean(path: &[u8]) -> Option<&[u8]> {
    let mut p = path;
    loop {
        if let Some(r) = p.strip_prefix(b"./") {
            p = r;
        } else if let Some(r) = p.strip_prefix(b"/") {
            p = r;
        } else {
            break;
        }
    }
    while let Some(r) = p.strip_suffix(b"/") {
        p = r;
    }
    if p.is_empty() || p == b"." { None } else { Some(p) }
}

/// Seconds since 1970 as NT time (100 ns since 1601).
pub fn nt_time(unix: u64) -> u64 {
    unix.saturating_mul(10_000_000).saturating_add(116_444_736_000_000_000)
}

impl PartialEq for Entry<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path && self.kind == other.kind && self.data == other.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    /// A ustar header for `name` (`kind`, `size`, `link`), checksum set.
    fn header(name: &[u8], kind: u8, size: usize, link: &[u8]) -> Vec<u8> {
        let mut h = vec![0u8; BLOCK];
        h[..name.len()].copy_from_slice(name);
        h[100..107].copy_from_slice(b"0000755");
        h[124..135].copy_from_slice(std::format!("{size:011o}").as_bytes());
        h[136..147].copy_from_slice(b"14000000000");
        h[156] = kind;
        h[157..157 + link.len()].copy_from_slice(link);
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        h[148..156].copy_from_slice(b"        ");
        let sum: u32 = h.iter().map(|&c| u32::from(c)).sum();
        h[148..155].copy_from_slice(std::format!("{sum:06o}\0").as_bytes());
        h
    }

    fn member(out: &mut Vec<u8>, name: &[u8], kind: u8, data: &[u8], link: &[u8]) {
        out.extend(header(name, kind, data.len(), link));
        out.extend_from_slice(data);
        out.resize(out.len().next_multiple_of(BLOCK), 0);
    }

    #[test]
    fn reads_ustar_members_of_every_kind() {
        let mut a = Vec::new();
        member(&mut a, b"./", b'5', b"", b"");
        member(&mut a, b"./bin/", b'5', b"", b"");
        member(&mut a, b"./bin/busybox", b'0', b"ELF...", b"");
        member(&mut a, b"./bin/sh", b'2', b"", b"busybox");
        member(&mut a, b"./bin/ash", b'1', b"", b"./bin/busybox");
        member(&mut a, b"./dev/null", b'3', b"", b"");
        a.extend([0u8; 1024]);
        let e: Vec<Entry> = Reader::new(&a).map(Result::unwrap).collect();
        assert_eq!(e.len(), 6);
        assert_eq!(clean(&e[0].path), None);
        assert_eq!((clean(&e[1].path), e[1].kind), (Some(&b"bin"[..]), Kind::Directory));
        assert_eq!((e[2].kind, e[2].data, e[2].mode), (Kind::File, &b"ELF..."[..], 0o755));
        assert_eq!(e[2].mtime, 0o14000000000);
        assert_eq!((e[3].kind, &e[3].link[..]), (Kind::Symlink, &b"busybox"[..]));
        assert_eq!((e[4].kind, &e[4].link[..]), (Kind::HardLink, &b"./bin/busybox"[..]));
        assert_eq!(e[5].kind, Kind::Special);
    }

    #[test]
    fn long_names_pax_and_base256() {
        let long = [b'a'; 150];
        let mut a = Vec::new();
        member(&mut a, b"././@LongLink", b'L', &[&long[..], b"\0"].concat(), b"");
        member(&mut a, b"short", b'0', b"x", b"");
        let rec = b"29 path=pax/name/longer/than\n";
        member(&mut a, b"PaxHeader", b'x', rec, b"");
        member(&mut a, b"ignored", b'0', b"", b"");
        // A size in base-256: 3 bytes.
        let mut h = header(b"big", b'0', 0, b"");
        h[124..136].copy_from_slice(&[0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3]);
        h[148..156].copy_from_slice(b"        ");
        let sum: u32 = h.iter().map(|&c| u32::from(c)).sum();
        h[148..155].copy_from_slice(std::format!("{sum:06o}\0").as_bytes());
        a.extend(h);
        a.extend(b"abc");
        a.resize(a.len().next_multiple_of(BLOCK), 0);
        let e: Vec<Entry> = Reader::new(&a).map(Result::unwrap).collect();
        assert_eq!(e[0].path, long);
        assert_eq!(e[1].path, b"pax/name/longer/than");
        assert_eq!(e[2].data, b"abc");
    }

    #[test]
    fn a_bad_checksum_or_a_cut_archive_is_an_error() {
        let mut a = Vec::new();
        member(&mut a, b"f", b'0', b"data", b"");
        let mut bad = a.clone();
        bad[0] = b'g';
        assert_eq!(Reader::new(&bad).next(), Some(Err(Error::Checksum(0))));
        assert_eq!(Reader::new(&a[..514]).next(), Some(Err(Error::Truncated(0))));
        assert_eq!(Reader::new(&a[..100]).next(), Some(Err(Error::Truncated(0))));
        assert!(Reader::new(&[]).next().is_none());
    }

    #[test]
    fn paths_are_cleaned_and_times_converted() {
        assert_eq!(clean(b"/etc/passwd"), Some(&b"etc/passwd"[..]));
        assert_eq!(clean(b"./././a/"), Some(&b"a"[..]));
        assert_eq!(clean(b"/"), None);
        assert_eq!(nt_time(0), 116_444_736_000_000_000);
    }
}
