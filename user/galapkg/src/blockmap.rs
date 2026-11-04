//! AppxBlockMap.xml, the block map of an MSIX/AppX package or bundle,
//! and its checks against the package's zip archive (Microsoft's
//! "Package block map schema reference": BlockMap, File, Block).
//!
//! The rules, as documented: every file of the package except the
//! footprint files [`UNLISTED`] has a `File` element (`Name` with `\`
//! separators, `Size` the uncompressed size, `LfhSize` the size of its
//! zip local file header) holding one `Block` per 64 KiB of the
//! uncompressed data (the last one shorter), in order. A block's `Hash`
//! is the base64 hash of its uncompressed bytes with the BlockMap's
//! `HashMethod` (SHA-256, SHA-384 or SHA-512 by their W3C URIs). A
//! deflated file's blocks each have a `Size`: the number of compressed
//! bytes holding that block, so each block is its own byte-aligned run
//! of deflate data; a stored file's blocks have none (one given anyway
//! must be the block's length).
//!
//! [`check_file`] inflates a deflated file block by block (each block's
//! compressed run must give exactly that block), so damage is reported
//! with the block it is in; the zip CRC-32 is checked last.

use crate::msix::Error;
use crate::xml::{self, Element};
use crate::zip::{self, Archive};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The block map's namespace.
pub const NS: &str = "http://schemas.microsoft.com/appx/2010/blockmap";

/// The size of a block of uncompressed data.
pub const BLOCK: usize = 65536;

/// Footprint files a block map does not list.
pub const UNLISTED: [&str; 3] = ["AppxBlockMap.xml", "AppxSignature.p7x", "[Content_Types].xml"];

/// The block map's file name in a package.
pub const FILE: &str = "AppxBlockMap.xml";

/// A HashMethod.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashMethod {
    Sha256,
    Sha384,
    Sha512,
}

impl HashMethod {
    pub fn from_uri(uri: &str) -> Option<HashMethod> {
        match uri {
            "http://www.w3.org/2001/04/xmlenc#sha256" => Some(HashMethod::Sha256),
            "http://www.w3.org/2001/04/xmldsig-more#sha384" => Some(HashMethod::Sha384),
            "http://www.w3.org/2001/04/xmlenc#sha512" => Some(HashMethod::Sha512),
            _ => None,
        }
    }

    pub fn digest_len(self) -> usize {
        match self {
            HashMethod::Sha256 => 32,
            HashMethod::Sha384 => 48,
            HashMethod::Sha512 => 64,
        }
    }

    pub fn hash(self, data: &[u8]) -> Vec<u8> {
        use sha2::Digest;
        match self {
            HashMethod::Sha256 => sha2::Sha256::digest(data).to_vec(),
            HashMethod::Sha384 => sha2::Sha384::digest(data).to_vec(),
            HashMethod::Sha512 => sha2::Sha512::digest(data).to_vec(),
        }
    }
}

/// A Block element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub hash: Vec<u8>,
    /// The compressed size (deflated files only).
    pub size: Option<u64>,
}

/// A File element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    /// As written (`\` separators).
    pub name: String,
    pub size: u64,
    pub lfh_size: u64,
    pub blocks: Vec<Block>,
    /// The line of the File element (for messages).
    pub line: u32,
}

/// A parsed block map.
#[derive(Clone, Debug)]
pub struct BlockMap {
    pub hash_method: HashMethod,
    pub files: Vec<File>,
}

/// Base64 (RFC 4648, with padding), the form block hashes are written in.
pub fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in data.chunks(3) {
        let n = c.iter().enumerate().fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            s.push(if i <= c.len() {
                char::from(ABC[(n >> (18 - 6 * i) & 63) as usize])
            } else {
                '='
            });
        }
    }
    s
}

/// Decodes base64 (padding required to complete the last group); None
/// when it is not base64.
pub fn unbase64(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(4) {
        return None;
    }
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let mut out = Vec::new();
    let groups = s.len() / 4;
    for (g, c) in s.as_bytes().chunks(4).enumerate() {
        let pad = c.iter().rev().take_while(|&&b| b == b'=').count();
        if pad > 2 || (pad > 0 && g != groups - 1) {
            return None;
        }
        let mut n = 0u32;
        for &b in &c[..4 - pad] {
            n = n << 6 | u32::from(val(b)?);
        }
        n <<= 6 * pad as u32;
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..3 - pad]);
    }
    Some(out)
}

fn bad(file: &str, el: &Element, msg: String) -> Error {
    Error::Manifest { file: file.into(), element: el.qname.clone(), line: el.line, msg }
}

fn number(file: &str, el: &Element, attr: &str) -> Result<Option<u64>, Error> {
    match el.attr(attr) {
        None => Ok(None),
        Some(v) => v
            .trim()
            .parse::<u64>()
            .map(Some)
            .map_err(|_| bad(file, el, format!("{attr}=\"{v}\" is not a number"))),
    }
}

impl BlockMap {
    /// Parses AppxBlockMap.xml (`file` names it in messages: a bundle's
    /// inner package's block map is "<package>/AppxBlockMap.xml").
    pub fn parse(file: &str, bytes: &[u8]) -> Result<BlockMap, Error> {
        let root = xml::parse(bytes).map_err(|e| Error::Xml { file: file.into(), err: e })?;
        if !root.is(NS, "BlockMap") {
            return Err(bad(file, &root, format!("the root is not {{{NS}}}BlockMap")));
        }
        let uri =
            root.attr("HashMethod").ok_or_else(|| bad(file, &root, "no HashMethod".into()))?;
        let hash_method = HashMethod::from_uri(uri)
            .ok_or_else(|| bad(file, &root, format!("unsupported HashMethod {uri}")))?;
        let mut files = Vec::new();
        for f in root.children_named(NS, "File") {
            let name = f.attr("Name").ok_or_else(|| bad(file, f, "no Name".into()))?;
            if name.is_empty() {
                return Err(bad(file, f, "an empty Name".into()));
            }
            let size = number(file, f, "Size")?.ok_or_else(|| bad(file, f, "no Size".into()))?;
            let lfh_size =
                number(file, f, "LfhSize")?.ok_or_else(|| bad(file, f, "no LfhSize".into()))?;
            let mut blocks = Vec::new();
            for b in f.children_named(NS, "Block") {
                let text = b.attr("Hash").ok_or_else(|| bad(file, b, "no Hash".into()))?;
                let hash = unbase64(text)
                    .filter(|h| h.len() == hash_method.digest_len())
                    .ok_or_else(|| {
                        bad(
                            file,
                            b,
                            format!("Hash=\"{text}\" is not a base64 {hash_method:?} hash"),
                        )
                    })?;
                blocks.push(Block { hash, size: number(file, b, "Size")? });
            }
            files.push(File { name: name.into(), size, lfh_size, blocks, line: f.line });
        }
        if files.is_empty() {
            return Err(bad(file, &root, "no File elements".into()));
        }
        Ok(BlockMap { hash_method, files })
    }
}

/// A zip item name as a package file name: percent-escapes decoded
/// (OPC part names, ECMA-376 Part 2), `/` as `\`, for comparing with the
/// block map's names (ASCII case-insensitively, as Windows does).
pub fn file_key(name: &str) -> String {
    let b = name.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(if b[i] == b'/' { b'\\' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).to_ascii_lowercase()
}

/// What a successful check covered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Checked {
    pub files: usize,
    pub blocks: usize,
    /// Uncompressed bytes hashed.
    pub bytes: u64,
}

/// The zip entries by [`file_key`].
pub fn index(archive: &Archive<'_>) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for (i, e) in archive.entries.iter().enumerate() {
        if !e.is_dir() {
            m.entry(file_key(&e.name)).or_insert(i);
        }
    }
    m
}

/// Checks one block map file against its zip entry and returns its
/// contents. `ctx` prefixes file names in errors (a bundle's inner
/// package). Every failure names the file and, for data, the block.
pub fn check_file(
    ctx: &str,
    archive: &Archive<'_>,
    entry: &zip::Entry,
    method: HashMethod,
    f: &File,
) -> Result<Vec<u8>, Error> {
    let file = format!("{ctx}{}", f.name);
    let mismatch = |what: &str, expected: u64, actual: u64| Error::Mismatch {
        file: file.clone(),
        what: what.into(),
        expected: expected.to_string(),
        actual: actual.to_string(),
    };
    let zerr = |e: zip::Error| Error::Zip { file: file.clone(), err: e };
    if u64::from(entry.size) != f.size {
        return Err(mismatch("Size (uncompressed bytes)", f.size, entry.size.into()));
    }
    let lfh = archive.local_header_size(entry).map_err(zerr)? as u64;
    if lfh != f.lfh_size {
        return Err(mismatch("LfhSize (local file header bytes)", f.lfh_size, lfh));
    }
    let size = entry.size as usize;
    let want_blocks = size.div_ceil(BLOCK);
    if f.blocks.len() != want_blocks {
        return Err(mismatch("number of blocks", want_blocks as u64, f.blocks.len() as u64));
    }
    let raw = archive.raw(entry).map_err(zerr)?;
    let block_len = |i: usize| (size - i * BLOCK).min(BLOCK);
    let data: Vec<u8> = match entry.method {
        0 => {
            if raw.len() != size {
                return Err(mismatch("stored size", size as u64, raw.len() as u64));
            }
            for (i, b) in f.blocks.iter().enumerate() {
                if let Some(s) = b.size.filter(|&s| s != block_len(i) as u64) {
                    return Err(Error::Block {
                        file: file.clone(),
                        block: i,
                        msg: format!("Size {s} for a stored block of {} bytes", block_len(i)),
                    });
                }
            }
            let mut v = Vec::new();
            v.try_reserve_exact(size)
                .map_err(|_| Error::OutOfMemory { file: file.clone(), bytes: size })?;
            v.extend_from_slice(raw);
            v
        }
        8 => inflate_blocks(&file, raw, size, f)?,
        m => {
            return Err(Error::Block {
                file,
                block: 0,
                msg: format!("compression method {m} (only stored and deflate)"),
            });
        }
    };
    for (i, b) in f.blocks.iter().enumerate() {
        let chunk = &data[i * BLOCK..i * BLOCK + block_len(i)];
        let actual = method.hash(chunk);
        if actual != b.hash {
            return Err(Error::Hash {
                file,
                block: i,
                expected: base64(&b.hash),
                actual: base64(&actual),
            });
        }
    }
    if zip::crc32(&data) != entry.crc32 {
        return Err(Error::Zip { file, err: zip::Error::Damaged(entry.name.clone()) });
    }
    Ok(data)
}

/// Inflates a deflated file one block map block at a time: block i's
/// `Size` compressed bytes must give exactly block i.
fn inflate_blocks(file: &str, raw: &[u8], size: usize, f: &File) -> Result<Vec<u8>, Error> {
    use miniz_oxide::inflate::TINFLStatus;
    use miniz_oxide::inflate::core::{DecompressorOxide, decompress, inflate_flags};
    let block_err = |i: usize, msg: String| Error::Block { file: file.into(), block: i, msg };
    if size == 0 {
        // No blocks: whatever deflate data ends an empty file.
        return Ok(Vec::new());
    }
    let mut sizes = Vec::new();
    for (i, b) in f.blocks.iter().enumerate() {
        sizes.push(
            b.size.ok_or_else(|| block_err(i, "no Size for a block of a deflated file".into()))?,
        );
    }
    let total = sizes.iter().try_fold(0u64, |t, &s| t.checked_add(s)).unwrap_or(u64::MAX);
    if total != raw.len() as u64 {
        return Err(Error::Mismatch {
            file: file.into(),
            what: "compressed size (the sum of the blocks' Size)".into(),
            expected: total.to_string(),
            actual: raw.len().to_string(),
        });
    }
    let mut out = Vec::new();
    out.try_reserve_exact(size)
        .map_err(|_| Error::OutOfMemory { file: file.into(), bytes: size })?;
    out.resize(size, 0);
    let mut state = alloc::boxed::Box::<DecompressorOxide>::default();
    let (mut inp, mut outp) = (0usize, 0usize);
    for (i, &s) in sizes.iter().enumerate() {
        let last = i == sizes.len() - 1;
        let run = &raw[inp..inp + s as usize];
        let end = outp + (size - outp).min(BLOCK);
        let mut flags = inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
        if !last {
            flags |= inflate_flags::TINFL_FLAG_HAS_MORE_INPUT;
        }
        // With the output exactly a block long, the decompressor may stop
        // at the block's end (HasMoreOutput) before it has seen the rest
        // of the run (an empty stored block after a flush, the final
        // block's end): it is called again until the run is used up.
        let (mut used, mut written) = (0, 0);
        let mut status;
        loop {
            let (st, u, w) =
                decompress(&mut state, &run[used..], &mut out[..end], outp + written, flags);
            status = st;
            used += u;
            written += w;
            if st != TINFLStatus::HasMoreOutput || used == run.len() || (u == 0 && w == 0) {
                break;
            }
        }
        let ok = used == run.len()
            && outp + written == end
            && match status {
                TINFLStatus::Done => last,
                TINFLStatus::NeedsMoreInput | TINFLStatus::HasMoreOutput => !last,
                _ => false,
            };
        if !ok {
            return Err(block_err(
                i,
                format!(
                    "its {} compressed bytes inflate to {} of its {} bytes ({status:?}, {used} bytes used)",
                    run.len(),
                    written,
                    end - outp
                ),
            ));
        }
        inp += s as usize;
        outp = end;
    }
    Ok(out)
}

/// Checks a whole archive against its block map: every listed file
/// (present, sizes, blocks, CRC-32), no unlisted file but the footprint
/// files [`UNLISTED`], no file listed twice. Every problem is returned,
/// not only the first.
pub fn check_archive(
    ctx: &str,
    archive: &Archive<'_>,
    map: &BlockMap,
) -> Result<Checked, Vec<Error>> {
    let idx = index(archive);
    let mut problems = Vec::new();
    let mut done = Checked::default();
    let mut seen = BTreeMap::new();
    for f in &map.files {
        let key = file_key(&f.name.replace('\\', "/"));
        if let Some(line) = seen.insert(key.clone(), f.line) {
            problems.push(Error::Manifest {
                file: format!("{ctx}{FILE}"),
                element: "File".into(),
                line: f.line,
                msg: format!("{} listed twice (first on line {line})", f.name),
            });
            continue;
        }
        let Some(&i) = idx.get(&key) else {
            problems.push(Error::Missing { file: format!("{ctx}{}", f.name), listed_line: f.line });
            continue;
        };
        match check_file(ctx, archive, &archive.entries[i], map.hash_method, f) {
            Ok(data) => {
                done.files += 1;
                done.blocks += f.blocks.len();
                done.bytes += data.len() as u64;
            }
            Err(e) => problems.push(e),
        }
    }
    for e in archive.entries.iter().filter(|e| !e.is_dir()) {
        let key = file_key(&e.name);
        let footprint = UNLISTED.iter().any(|u| file_key(u) == key);
        if !footprint && !seen.contains_key(&key) {
            problems.push(Error::Unlisted { file: format!("{ctx}{}", e.name) });
        }
    }
    if problems.is_empty() { Ok(done) } else { Err(problems) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trip() {
        // RFC 4648 section 10.
        for (raw, enc) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(raw.as_bytes()), enc);
            assert_eq!(unbase64(enc).unwrap(), raw.as_bytes());
        }
        assert_eq!(unbase64("Zg="), None);
        assert_eq!(unbase64("Z==="), None);
        assert_eq!(unbase64("Zg==Zg=="), None);
        assert_eq!(unbase64("Zm9*"), None);
    }

    #[test]
    fn names_match_zip_items() {
        assert_eq!(file_key("Assets/My%20Logo.png"), file_key("Assets\\my logo.PNG"));
        assert_eq!(file_key("a%2"), "a%2");
        assert_eq!(file_key("%C3%A9"), "é");
    }

    #[test]
    fn parse_errors_name_element_and_line() {
        let doc = format!(
            "<BlockMap xmlns=\"{NS}\" HashMethod=\"http://www.w3.org/2001/04/xmlenc#sha256\">\n\
             <File Name=\"a\" Size=\"x\" LfhSize=\"31\"/>\n</BlockMap>"
        );
        let e = BlockMap::parse("AppxBlockMap.xml", doc.as_bytes()).unwrap_err();
        assert_eq!(
            alloc::format!("{e}"),
            "AppxBlockMap.xml line 2: <File>: Size=\"x\" is not a number"
        );
        let doc = format!("<BlockMap xmlns=\"{NS}\" HashMethod=\"urn:md5\"><File/></BlockMap>");
        let e = BlockMap::parse("AppxBlockMap.xml", doc.as_bytes()).unwrap_err();
        assert!(alloc::format!("{e}").contains("unsupported HashMethod urn:md5"));
        let e = BlockMap::parse("AppxBlockMap.xml", b"<BlockMap HashMethod='x'/>").unwrap_err();
        assert!(alloc::format!("{e}").contains("the root is not"));
    }
}
