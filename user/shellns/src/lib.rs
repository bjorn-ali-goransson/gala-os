//! gala's shell namespace, the parts without system calls (ROADMAP EX
//! step 1; ARCHITECTURE "The file explorer"): what shell32's
//! IShellFolder and IShellItem (`user/shell32/src/namespace.rs`) and
//! gala's `explorer.exe` agree on, tested on the host with `cargo test
//! -p shellns`.
//!
//! The tree, as Windows 11's File Explorer shows it, with the Linux
//! distributions beside the drives:
//!
//! ```text
//! Desktop (the empty list)
//! ├── This PC        ::{20D04FE0-3AEA-1069-A2D8-08002B30309D}
//! │   └── C:\ ...     drives, then their folders and files
//! └── Linux          ::{B2B4A4D1-2754-4140-A2EB-9A76D9D7CDC6}
//!     └── <distro>   \\wsl.localhost\<distro>, then its tree
//! ```
//!
//! - **Item ID lists** are gala's own (Windows' are not documented): one
//!   SHITEMID per level, `cb` (u16), `b"gala"`, a kind (u16), then the
//!   kind's data; a zero `cb` ends the list. Kinds: a root folder (its
//!   CLSID), a drive (its letter), a file-system item (attributes, then
//!   its name in UTF-16), a distribution (its name). The lists of
//!   earlier gala builds (one item holding a whole path, and "This PC")
//!   are still read.
//! - **Parsing names** (IShellFolder::ParseDisplayName, SIGDN_DESKTOP-
//!   ABSOLUTEPARSING): `::{CLSID}` for a root (and below it `\`-separated
//!   children), `X:\a\b` below This PC, `\\wsl.localhost\<distro>\a` and
//!   `\\wsl$\<distro>\a` below Linux (WSL's names: gala's own UI says
//!   "Linux").
//! - **Order** (IShellFolder::CompareIDs, Explorer's name sort): folders
//!   first, then names compared as StrCmpLogicalW does (digit runs by
//!   value, letters ignoring case).

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

use alloc::vec::Vec;
use core::cmp::Ordering;

/// A GUID as it lies in memory (Data1-3 little endian).
pub type Guid = [u8; 16];

pub const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

/// CLSID_MyComputer, "This PC".
pub const CLSID_MY_COMPUTER: Guid =
    guid(0x20d0_4fe0, 0x3aea, 0x1069, [0xa2, 0xd8, 0x08, 0x00, 0x2b, 0x30, 0x30, 0x9d]);
/// The CLSID of WSL's "Linux" folder in Explorer's navigation pane.
pub const CLSID_LINUX: Guid =
    guid(0xb2b4_a4d1, 0x2754, 0x4140, [0xa2, 0xeb, 0x9a, 0x76, 0xd9, 0xd7, 0xcd, 0xc6]);

/// The root folders below the desktop, in Explorer's order.
pub const ROOTS: [Guid; 2] = [CLSID_MY_COMPUTER, CLSID_LINUX];

/// WSL's server name that parsing names and paths use.
pub const WSL_SERVER: &str = "wsl.localhost";

const MAGIC: &[u8; 4] = b"gala";
/// Earlier builds' lists: one item with a whole path; "This PC".
const KIND_PATH: u16 = 1;
const KIND_DRIVES: u16 = 2;
const KIND_ROOT: u16 = 3;
const KIND_DRIVE: u16 = 4;
const KIND_FS: u16 = 5;
const KIND_DISTRO: u16 = 6;

/// FILE_ATTRIBUTE_DIRECTORY.
pub const DIRECTORY: u32 = 0x10;

/// One level of the namespace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// A root folder below the desktop (This PC, Linux).
    Root(Guid),
    /// A drive below This PC (its letter, upper case).
    Drive(u8),
    /// A file or folder: its attributes (FILE_ATTRIBUTE_*) and name.
    Fs { attributes: u32, name: Vec<u16> },
    /// A WSL distribution below Linux (its name).
    Distro(Vec<u16>),
}

impl Item {
    /// A file-system folder (a drive and a distribution are folders).
    pub fn is_folder(&self) -> bool {
        match self {
            Item::Fs { attributes, .. } => attributes & DIRECTORY != 0,
            _ => true,
        }
    }

    /// A folder named `name`.
    pub fn folder(name: &[u16]) -> Item {
        Item::Fs { attributes: DIRECTORY, name: name.to_vec() }
    }
}

fn w(s: &str) -> impl Iterator<Item = u16> + '_ {
    s.encode_utf16()
}

fn push_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Appends `item`'s SHITEMID to `out`.
pub fn encode_item(item: &Item, out: &mut Vec<u8>) {
    let start = out.len();
    push_u16(out, 0);
    out.extend_from_slice(MAGIC);
    match item {
        Item::Root(clsid) => {
            push_u16(out, KIND_ROOT);
            out.extend_from_slice(clsid);
        }
        Item::Drive(letter) => {
            push_u16(out, KIND_DRIVE);
            push_u16(out, u16::from(*letter));
        }
        Item::Fs { attributes, name } => {
            push_u16(out, KIND_FS);
            out.extend_from_slice(&attributes.to_le_bytes());
            name.iter().for_each(|&c| push_u16(out, c));
        }
        Item::Distro(name) => {
            push_u16(out, KIND_DISTRO);
            name.iter().for_each(|&c| push_u16(out, c));
        }
    }
    let cb = (out.len() - start) as u16;
    out[start..start + 2].copy_from_slice(&cb.to_le_bytes());
}

/// `items` as a whole list (terminator included).
pub fn encode_list(items: &[Item]) -> Vec<u8> {
    let mut out = Vec::new();
    for item in items {
        encode_item(item, &mut out);
    }
    push_u16(&mut out, 0);
    out
}

fn units(b: &[u8]) -> Vec<u16> {
    b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
}

/// One SHITEMID's bytes (its `cb` included) as items: one, or an earlier
/// build's whole path as the items it names. None: not gala's.
fn decode_item(b: &[u8], out: &mut Vec<Item>) -> Option<()> {
    if b.len() < 8 || &b[2..6] != MAGIC {
        return None;
    }
    let data = &b[8..];
    match u16::from_le_bytes([b[6], b[7]]) {
        KIND_ROOT => out.push(Item::Root(data.get(..16)?.try_into().ok()?)),
        KIND_DRIVE => {
            let letter = u8::try_from(u16::from_le_bytes([*data.first()?, *data.get(1)?])).ok()?;
            out.push(Item::Drive(letter.to_ascii_uppercase()));
        }
        KIND_FS => {
            let attributes = u32::from_le_bytes(data.get(..4)?.try_into().ok()?);
            out.push(Item::Fs { attributes, name: units(&data[4..]) });
        }
        KIND_DISTRO => out.push(Item::Distro(units(data))),
        KIND_DRIVES => out.push(Item::Root(CLSID_MY_COMPUTER)),
        KIND_PATH => out.extend(parse(&units(data))?),
        _ => return None,
    }
    Some(())
}

/// The items of the list at the start of `b` (up to its zero `cb`, or
/// the end of `b`). None when an item is not gala's or is cut off.
pub fn decode_list(b: &[u8]) -> Option<Vec<Item>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 2 <= b.len() {
        let cb = usize::from(u16::from_le_bytes([b[at], b[at + 1]]));
        if cb == 0 {
            return Some(out);
        }
        decode_item(b.get(at..at + cb)?, &mut out)?;
        at += cb;
    }
    Some(out)
}

/// The size of the list at the start of `b` (ILGetSize: terminator
/// included), reading up to the end of `b`.
pub fn list_size(b: &[u8]) -> usize {
    let mut at = 0;
    while at + 2 <= b.len() {
        let cb = usize::from(u16::from_le_bytes([b[at], b[at + 1]]));
        if cb == 0 {
            return at + 2;
        }
        at += cb;
    }
    at
}

// ---- GUIDs ------------------------------------------------------------------

/// `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}` (upper case, as Windows
/// writes a CLSID in a parsing name).
pub fn guid_string(g: &Guid) -> Vec<u16> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut s = Vec::with_capacity(38);
    let order = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    s.push(u16::from(b'{'));
    for (i, &at) in order.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            s.push(u16::from(b'-'));
        }
        s.push(u16::from(HEX[usize::from(g[at] >> 4)]));
        s.push(u16::from(HEX[usize::from(g[at] & 15)]));
    }
    s.push(u16::from(b'}'));
    s
}

/// A `{...}` GUID string (either case).
pub fn parse_guid(s: &[u16]) -> Option<Guid> {
    if s.len() != 38 || s[0] != u16::from(b'{') || s[37] != u16::from(b'}') {
        return None;
    }
    let order = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    let mut g = [0u8; 16];
    let mut digits = s[1..37].iter().filter(|&&c| c != u16::from(b'-'));
    if s[1..37].iter().filter(|&&c| c == u16::from(b'-')).count() != 4 {
        return None;
    }
    for &at in &order {
        let mut v = 0u8;
        for _ in 0..2 {
            let c = char::from_u32(u32::from(*digits.next()?))?.to_digit(16)?;
            v = v << 4 | c as u8;
        }
        g[at] = v;
    }
    Some(g)
}

// ---- Parsing names -----------------------------------------------------------

fn is_sep(c: u16) -> bool {
    c == u16::from(b'\\') || c == u16::from(b'/')
}

fn eq_ascii(a: &[u16], b: &str) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.bytes())
            .all(|(&c, d)| u8::try_from(c).is_ok_and(|c| c.eq_ignore_ascii_case(&d)))
}

/// File-system items for the `\`-separated names in `rest` (folders; the
/// caller sets the last one's attributes). None for `.` or `..` (not
/// parsing names).
fn fs_items(rest: &[u16], out: &mut Vec<Item>) -> Option<()> {
    for part in rest.split(|&c| is_sep(c)).filter(|p| !p.is_empty()) {
        if part == [u16::from(b'.')] || part == [u16::from(b'.'), u16::from(b'.')] {
            return None;
        }
        out.push(Item::folder(part));
    }
    Some(())
}

/// The items a desktop-absolute parsing name names (every file-system
/// item a folder: the caller sets the last one's attributes from the
/// file). None for a name outside the namespace.
pub fn parse(name: &[u16]) -> Option<Vec<Item>> {
    let mut out = Vec::new();
    if name.is_empty() {
        return Some(out);
    }
    // `::{CLSID}`, then what is below it.
    if name.len() >= 40 && name[0] == u16::from(b':') && name[1] == u16::from(b':') {
        let clsid = parse_guid(&name[2..40])?;
        if !ROOTS.contains(&clsid) {
            return None;
        }
        out.push(Item::Root(clsid));
        let rest = &name[40..];
        if rest.is_empty() {
            return Some(out);
        }
        if !is_sep(rest[0]) {
            return None;
        }
        let rest = &rest[1..];
        if clsid == CLSID_MY_COMPUTER {
            let drive = parse(rest)?;
            return (drive.first() == Some(&Item::Root(CLSID_MY_COMPUTER))).then_some(drive);
        }
        let mut parts = rest.splitn(2, |&c| is_sep(c));
        if let Some(distro) = parts.next().filter(|d| !d.is_empty()) {
            out.push(Item::Distro(distro.to_vec()));
            fs_items(parts.next().unwrap_or(&[]), &mut out)?;
        }
        return Some(out);
    }
    // `X:` or `X:\...`.
    if name.len() >= 2
        && name[1] == u16::from(b':')
        && let Ok(letter) = u8::try_from(name[0])
        && letter.is_ascii_alphabetic()
    {
        if name.len() > 2 && !is_sep(name[2]) {
            return None;
        }
        out.push(Item::Root(CLSID_MY_COMPUTER));
        out.push(Item::Drive(letter.to_ascii_uppercase()));
        fs_items(&name[2..], &mut out)?;
        return Some(out);
    }
    // `\\wsl.localhost\<distro>\...` and `\\wsl$\<distro>\...`.
    if name.len() > 2 && is_sep(name[0]) && is_sep(name[1]) {
        let body = &name[2..];
        let mut parts = body.splitn(3, |&c| is_sep(c));
        let server = parts.next()?;
        if !eq_ascii(server, WSL_SERVER) && !eq_ascii(server, "wsl$") {
            return None;
        }
        out.push(Item::Root(CLSID_LINUX));
        if let Some(distro) = parts.next().filter(|d| !d.is_empty()) {
            out.push(Item::Distro(distro.to_vec()));
            fs_items(parts.next().unwrap_or(&[]), &mut out)?;
        }
        return Some(out);
    }
    None
}

/// The file-system path of `items` (`X:\a\b`, `\\wsl.localhost\d\a`);
/// None for a virtual folder (the desktop's is its own directory, which
/// the caller knows).
pub fn fs_path(items: &[Item]) -> Option<Vec<u16>> {
    let mut out: Vec<u16> = Vec::new();
    match items {
        [Item::Root(CLSID_MY_COMPUTER), Item::Drive(letter), ..] => {
            out.extend([u16::from(*letter), u16::from(b':'), u16::from(b'\\')]);
        }
        [Item::Root(CLSID_LINUX), Item::Distro(name), ..] => {
            out.extend(w("\\\\"));
            out.extend(w(WSL_SERVER));
            out.push(u16::from(b'\\'));
            out.extend_from_slice(name);
        }
        _ => return None,
    }
    let rest = &items[2..];
    for item in rest {
        let Item::Fs { name, .. } = item else { return None };
        if out.last() != Some(&u16::from(b'\\')) {
            out.push(u16::from(b'\\'));
        }
        out.extend_from_slice(name);
    }
    Some(out)
}

/// The desktop-absolute parsing name of `items` (SIGDN_DESKTOPABSOLUTE-
/// PARSING): the file-system path where there is one, else `::{CLSID}`
/// (the desktop's own, the empty list, is its directory: the caller's).
pub fn parsing_name(items: &[Item]) -> Vec<u16> {
    if let Some(path) = fs_path(items) {
        return path;
    }
    let mut out = Vec::new();
    for item in items {
        match item {
            Item::Root(g) => {
                out.extend(w("::"));
                out.extend(guid_string(g));
            }
            Item::Drive(_) | Item::Fs { .. } | Item::Distro(_) => {}
        }
    }
    out
}

/// The name of `item` within its parent (SIGDN_PARENTRELATIVEPARSING):
/// `::{CLSID}`, `C:\`, the file's or distribution's name.
pub fn relative_parsing_name(item: &Item) -> Vec<u16> {
    match item {
        Item::Root(g) => {
            let mut s: Vec<u16> = w("::").collect();
            s.extend(guid_string(g));
            s
        }
        Item::Drive(letter) => [u16::from(*letter), u16::from(b':'), u16::from(b'\\')].to_vec(),
        Item::Fs { name, .. } | Item::Distro(name) => name.clone(),
    }
}

/// What Explorer shows for `item` (SIGDN_NORMALDISPLAY): "This PC",
/// "Linux", a drive as "<label> (C:)" ("Local Disk (C:)" without a
/// label), a name as it is (file name extensions shown).
pub fn display_name(item: &Item, label: &[u16]) -> Vec<u16> {
    match item {
        Item::Root(CLSID_MY_COMPUTER) => w("This PC").collect(),
        Item::Root(CLSID_LINUX) => w("Linux").collect(),
        Item::Root(g) => guid_string(g),
        Item::Drive(letter) => {
            let mut s: Vec<u16> =
                if label.is_empty() { w("Local Disk").collect() } else { label.to_vec() };
            s.extend(w(" ("));
            s.push(u16::from(*letter));
            s.extend(w(":)"));
            s
        }
        Item::Fs { name, .. } | Item::Distro(name) => name.clone(),
    }
}

// ---- Order ------------------------------------------------------------------

fn fold(c: u16) -> u16 {
    if (u16::from(b'A')..=u16::from(b'Z')).contains(&c) { c + 32 } else { c }
}

fn is_digit(c: u16) -> bool {
    (u16::from(b'0')..=u16::from(b'9')).contains(&c)
}

/// Names compared as StrCmpLogicalW does: runs of digits by their value
/// (fewer leading zeros first when equal), the rest one character at a
/// time ignoring ASCII case.
pub fn compare_names(a: &[u16], b: &[u16]) -> Ordering {
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if is_digit(a[i]) && is_digit(b[j]) {
            let (si, sj) = (i, j);
            while i < a.len() && is_digit(a[i]) {
                i += 1;
            }
            while j < b.len() && is_digit(b[j]) {
                j += 1;
            }
            let (x, y) = (trim_zeros(&a[si..i]), trim_zeros(&b[sj..j]));
            let by_value = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
            if by_value != Ordering::Equal {
                return by_value;
            }
            let by_zeros = (i - si).cmp(&(j - sj)).reverse();
            if by_zeros != Ordering::Equal {
                return by_zeros;
            }
            continue;
        }
        let by_char = fold(a[i]).cmp(&fold(b[j]));
        if by_char != Ordering::Equal {
            return by_char;
        }
        i += 1;
        j += 1;
    }
    (a.len() - i).cmp(&(b.len() - j))
}

fn trim_zeros(d: &[u16]) -> &[u16] {
    let n = d.iter().take_while(|&&c| c == u16::from(b'0')).count();
    &d[n.min(d.len().saturating_sub(1))..]
}

/// Explorer's order of two items of one folder: roots as ROOTS lists
/// them, drives by letter, folders before files, then names.
pub fn compare_items(a: &Item, b: &Item) -> Ordering {
    let rank = |i: &Item| match i {
        Item::Root(g) => ROOTS.iter().position(|r| r == g).unwrap_or(ROOTS.len()),
        _ => 100,
    };
    match (a, b) {
        (Item::Drive(x), Item::Drive(y)) => x.cmp(y),
        (Item::Root(_), _) | (_, Item::Root(_)) => rank(a).cmp(&rank(b)),
        _ => b.is_folder().cmp(&a.is_folder()).then_with(|| {
            let name = |i: &Item| match i {
                Item::Fs { name, .. } | Item::Distro(name) => name.clone(),
                _ => Vec::new(),
            };
            compare_names(&name(a), &name(b))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    fn s(t: &str) -> Vec<u16> {
        t.encode_utf16().collect()
    }

    #[test]
    fn lists_round_trip() {
        let items = vec![
            Item::Root(CLSID_LINUX),
            Item::Distro(s("bb")),
            Item::folder(&s("etc")),
            Item::Fs { attributes: 0x20, name: s("passwd") },
        ];
        let b = encode_list(&items);
        assert_eq!(list_size(&b), b.len());
        assert_eq!(decode_list(&b), Some(items));
        assert_eq!(decode_list(&[0, 0]), Some(vec![]));
        // Not gala's.
        assert_eq!(decode_list(&[6, 0, 1, 2, 3, 4, 0, 0]), None);
    }

    #[test]
    fn earlier_lists_are_read() {
        let mut b = vec![0u8, 0];
        b.extend_from_slice(b"gala");
        b.extend_from_slice(&KIND_PATH.to_le_bytes());
        for c in s("C:\\Windows") {
            b.extend_from_slice(&c.to_le_bytes());
        }
        let cb = b.len() as u16;
        b[..2].copy_from_slice(&cb.to_le_bytes());
        b.extend_from_slice(&[0, 0]);
        let items = decode_list(&b).unwrap();
        assert_eq!(fs_path(&items), Some(s("C:\\Windows")));
    }

    #[test]
    fn guids_as_text() {
        let text = s("{20D04FE0-3AEA-1069-A2D8-08002B30309D}");
        assert_eq!(guid_string(&CLSID_MY_COMPUTER), text);
        assert_eq!(parse_guid(&text), Some(CLSID_MY_COMPUTER));
        assert_eq!(parse_guid(&s("{b2b4a4d1-2754-4140-a2eb-9a76d9d7cdc6}")), Some(CLSID_LINUX));
        assert_eq!(parse_guid(&s("{b2b4a4d1-2754-4140-a2eb-9a76d9d7cdc}")), None);
    }

    #[test]
    fn parses_drives_and_distributions() {
        let p = parse(&s("c:\\Windows\\System32")).unwrap();
        assert_eq!(p[..2], [Item::Root(CLSID_MY_COMPUTER), Item::Drive(b'C')]);
        assert_eq!(fs_path(&p), Some(s("C:\\Windows\\System32")));
        assert_eq!(fs_path(&parse(&s("C:")).unwrap()), Some(s("C:\\")));
        assert_eq!(fs_path(&parse(&s("C:\\")).unwrap()), Some(s("C:\\")));
        let p = parse(&s("\\\\wsl$\\bb\\etc")).unwrap();
        assert_eq!(p[..2], [Item::Root(CLSID_LINUX), Item::Distro(s("bb"))]);
        assert_eq!(fs_path(&p), Some(s("\\\\wsl.localhost\\bb\\etc")));
        assert_eq!(parse(&s("\\\\WSL.LOCALHOST")).unwrap(), vec![Item::Root(CLSID_LINUX)]);
        assert_eq!(parse(&s("\\\\fileserver\\share")), None);
        assert_eq!(parse(&s("C:\\a\\..\\b")), None);
        assert_eq!(parse(&s("relative")), None);
    }

    #[test]
    fn parses_roots_and_below() {
        let pc = s("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}");
        assert_eq!(parse(&pc), Some(vec![Item::Root(CLSID_MY_COMPUTER)]));
        assert_eq!(parsing_name(&parse(&pc).unwrap()), pc);
        let mut below = pc.clone();
        below.extend(s("\\C:\\Windows"));
        assert_eq!(fs_path(&parse(&below).unwrap()), Some(s("C:\\Windows")));
        let mut linux = s("::{B2B4A4D1-2754-4140-A2EB-9A76D9D7CDC6}");
        assert_eq!(parse(&linux), Some(vec![Item::Root(CLSID_LINUX)]));
        linux.extend(s("\\bb\\etc"));
        assert_eq!(fs_path(&parse(&linux).unwrap()), Some(s("\\\\wsl.localhost\\bb\\etc")));
        assert_eq!(parse(&s("::{00000000-0000-0000-0000-000000000000}")), None);
    }

    #[test]
    fn names_for_display() {
        assert_eq!(display_name(&Item::Root(CLSID_MY_COMPUTER), &[]), s("This PC"));
        assert_eq!(display_name(&Item::Root(CLSID_LINUX), &[]), s("Linux"));
        assert_eq!(display_name(&Item::Drive(b'C'), &[]), s("Local Disk (C:)"));
        assert_eq!(display_name(&Item::Drive(b'C'), &s("Windows")), s("Windows (C:)"));
        assert_eq!(relative_parsing_name(&Item::Drive(b'C')), s("C:\\"));
        assert_eq!(display_name(&Item::Distro(s("bb")), &[]), s("bb"));
    }

    #[test]
    fn logical_name_order() {
        use Ordering::*;
        assert_eq!(compare_names(&s("file2"), &s("file10")), Less);
        assert_eq!(compare_names(&s("File"), &s("file")), Equal);
        assert_eq!(compare_names(&s("a"), &s("ab")), Less);
        assert_eq!(compare_names(&s("x01"), &s("x1")), Less);
        assert_eq!(compare_names(&s("b"), &s("A")), Greater);
    }

    #[test]
    fn folders_first() {
        let dir = Item::folder(&s("zeta"));
        let file = Item::Fs { attributes: 0x20, name: s("alpha") };
        assert_eq!(compare_items(&dir, &file), Ordering::Less);
        assert_eq!(
            compare_items(&Item::Root(CLSID_MY_COMPUTER), &Item::Root(CLSID_LINUX)),
            Ordering::Less
        );
        assert_eq!(compare_items(&Item::Drive(b'D'), &Item::Drive(b'C')), Ordering::Greater);
    }
}
