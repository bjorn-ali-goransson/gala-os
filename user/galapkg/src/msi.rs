//! Reading Windows Installer packages (`.msi`): the database inside a
//! compound file ([`crate::cfb`]), written from the Windows Installer
//! SDK documentation (the database tables and their columns, the
//! Directory table's DefaultDir syntax, the File and Media tables'
//! sequence numbers, the summary information stream's properties) and
//! the published property set format [MS-OLEPS]; where the documentation
//! stops (the stream-name encoding, the string pool, the table storage),
//! the format was observed black-box on packages made by wixl and read
//! by msitools' msiinfo (THIRD_PARTY.md: tools, nothing linked).
//!
//! The pieces:
//! - stream names: [`decode_name`]/[`encode_name`]: names of characters
//!   from a 64-letter alphabet (`0-9A-Za-z._`) are packed two per UTF-16
//!   code unit (0x3800 + a + b·64), a lone one as 0x4800 + a, other
//!   characters as themselves; a table's stream starts with 0x4840;
//! - the string pool ([`StringPool`]): `_StringPool` (a header with the
//!   code page and the long-string-id flag, then a length and reference
//!   count per string id; a string of 64 KiB or more is a (0, refs)
//!   marker followed by its 32-bit length) over `_StringData` (the bytes,
//!   in the code page);
//! - the catalog: `_Tables` and `_Columns`, then any table as [`Table`]
//!   (rows of [`Value`]); a table's stream is column-major: each
//!   column's values for every row, then the next column's; integers are
//!   stored with the sign bit flipped (0 is NULL), strings as string ids
//!   (2 bytes, 3 with long ids), streams as a 2-byte "present" mark, the
//!   data being the stream `<Table>.<key>[.<key>...]`;
//! - typed rows of the tables an installer reads first ([`File`],
//!   [`Component`], [`Directory`], [`Feature`], [`Registry`],
//!   [`Shortcut`], [`Media`], [`CustomAction`], [`Binary`]), read by
//!   column name so schema variants work;
//! - [`Package`]: what `gala install` calls: the files with their target
//!   paths ([`Package::install_files`]), a file's bytes from its embedded
//!   cabinet ([`Package::extract`], [`Package::extract_all`]).
//!
//! Errors name the stream, table, row and column, or the string id.

use crate::cab;
use crate::cfb;
use alloc::borrow::ToOwned;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

pub mod cond;
pub mod format;
pub mod plan;

/// Why a package could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The compound file underneath.
    Cfb(cfb::Error),
    /// An embedded cabinet (its stream name first).
    Cab(String, cab::Error),
    /// The database is inconsistent: what and where.
    Corrupt(String),
    /// Something the caller asked for is not in the package.
    NotFound(String),
    /// A valid feature this reader does not have.
    Unsupported(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Cfb(e) => write!(f, "MSI package: {e}"),
            Error::Cab(name, e) => write!(f, "MSI package: cabinet {name}: {e}"),
            Error::Corrupt(what) => write!(f, "MSI package is damaged: {what}"),
            Error::NotFound(what) => write!(f, "MSI package has no {what}"),
            Error::Unsupported(what) => write!(f, "MSI feature not supported: {what}"),
        }
    }
}

impl From<cfb::Error> for Error {
    fn from(e: cfb::Error) -> Error {
        Error::Cfb(e)
    }
}

macro_rules! corrupt {
    ($($t:tt)*) => { Error::Corrupt(format!($($t)*)) };
}

// ---------------------------------------------------------------------
// Stream names.

const ALPHABET: &[u8; 64] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz._";
/// The first code unit of a table's stream name.
const TABLE_PREFIX: u16 = 0x4840;

fn letter(c: char) -> Option<u16> {
    ALPHABET.iter().position(|&a| char::from(a) == c).map(|p| p as u16)
}

/// Encodes a stream name as the package stores it; `table` adds the
/// table-stream prefix.
pub fn encode_name(name: &str, table: bool) -> Vec<u16> {
    let mut out = Vec::new();
    if table {
        out.push(TABLE_PREFIX);
    }
    let chars: Vec<char> = name.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match (letter(chars[i]), chars.get(i + 1).and_then(|&c| letter(c))) {
            (Some(a), Some(b)) => {
                out.push(0x3800 + a + (b << 6));
                i += 2;
            }
            (Some(a), None) => {
                out.push(0x4800 + a);
                i += 1;
            }
            (None, _) => {
                let mut buf = [0u16; 2];
                out.extend_from_slice(chars[i].encode_utf16(&mut buf));
                i += 1;
            }
        }
    }
    out
}

/// Decodes a stored stream name: the name, and whether it is a table's.
pub fn decode_name(units: &[u16]) -> (String, bool) {
    let (table, rest) = match units.split_first() {
        Some((&TABLE_PREFIX, rest)) => (true, rest),
        _ => (false, units),
    };
    let mut out = String::new();
    for c in char::decode_utf16(rest.iter().copied()) {
        let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
        let u = u32::from(c);
        if (0x3800..0x4800).contains(&u) {
            let v = (u - 0x3800) as usize;
            out.push(char::from(ALPHABET[v & 63]));
            out.push(char::from(ALPHABET[(v >> 6) & 63]));
        } else if (0x4800..0x4840).contains(&u) {
            out.push(char::from(ALPHABET[(u - 0x4800) as usize]));
        } else {
            out.push(c);
        }
    }
    (out, table)
}

// ---------------------------------------------------------------------
// Code pages.

/// Windows-1252's characters 0x80-0x9f (U+FFFD where undefined).
const CP1252_HIGH: [u16; 32] = [
    0x20ac, 0xfffd, 0x201a, 0x0192, 0x201e, 0x2026, 0x2020, 0x2021, 0x02c6, 0x2030, 0x0160, 0x2039,
    0x0152, 0xfffd, 0x017d, 0xfffd, 0xfffd, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2013, 0x2014,
    0x02dc, 0x2122, 0x0161, 0x203a, 0x0153, 0xfffd, 0x017e, 0x0178,
];

/// Decodes `bytes` in `codepage`; the flag is false when a character had
/// to be replaced because the code page is not one this reader knows.
pub fn decode_codepage(bytes: &[u8], codepage: u32) -> (String, bool) {
    if bytes.is_ascii() {
        return (String::from_utf8_lossy(bytes).into_owned(), true);
    }
    let cp1252 = |b: &[u8]| -> String {
        b.iter()
            .map(|&c| match c {
                0x80..=0x9f => char::from_u32(u32::from(CP1252_HIGH[usize::from(c - 0x80)]))
                    .unwrap_or(char::REPLACEMENT_CHARACTER),
                c => char::from(c),
            })
            .collect()
    };
    match codepage {
        65001 => (String::from_utf8_lossy(bytes).into_owned(), core::str::from_utf8(bytes).is_ok()),
        // 0 is "neutral": tools that write UTF-8 there exist (wixl).
        0 => match core::str::from_utf8(bytes) {
            Ok(s) => (s.to_owned(), true),
            Err(_) => (cp1252(bytes), true),
        },
        1252 => (cp1252(bytes), true),
        28591 => (bytes.iter().map(|&c| char::from(c)).collect(), true),
        _ => (
            bytes.iter().map(|&c| if c < 0x80 { char::from(c) } else { '\u{fffd}' }).collect(),
            false,
        ),
    }
}

// ---------------------------------------------------------------------
// The string pool.

/// The package's strings, by string id (0 is the null string).
#[derive(Debug, Default)]
pub struct StringPool {
    /// The code page of the strings (from `_StringPool`'s header).
    pub codepage: u32,
    /// String ids take 3 bytes in tables instead of 2.
    pub long_ids: bool,
    strings: Vec<String>,
    refs: Vec<u32>,
    /// Some string had characters this reader could not decode.
    pub lossy: bool,
}

impl StringPool {
    /// Parses `_StringPool` (`pool`) over `_StringData` (`data`).
    pub fn parse(pool: &[u8], data: &[u8]) -> Result<StringPool, Error> {
        let Some(header) = pool.get(..4) else {
            return Err(corrupt!("_StringPool is {} bytes, shorter than its header", pool.len()));
        };
        let header = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let mut sp = StringPool {
            codepage: header & 0x7fff_ffff,
            long_ids: header & 0x8000_0000 != 0,
            ..StringPool::default()
        };
        let count = (pool.len() - 4) / 4;
        sp.strings.try_reserve_exact(count + 1).map_err(|_| oom("the string pool"))?;
        sp.refs.try_reserve_exact(count + 1).map_err(|_| oom("the string pool"))?;
        sp.strings.push(String::new());
        sp.refs.push(0);
        let mut at = 0usize; // in _StringData
        let mut i = 4;
        while i + 4 <= pool.len() {
            let id = sp.strings.len();
            let mut len = u32::from(u16::from_le_bytes([pool[i], pool[i + 1]])) as usize;
            let refs = u32::from(u16::from_le_bytes([pool[i + 2], pool[i + 3]]));
            i += 4;
            if len == 0 && refs != 0 {
                // A long string: its length is the next 32-bit word.
                let Some(l) = pool.get(i..i + 4) else {
                    return Err(corrupt!(
                        "_StringPool: string id {id} (a long string) has no length (offset {i})"
                    ));
                };
                len = u32::from_le_bytes([l[0], l[1], l[2], l[3]]) as usize;
                i += 4;
            }
            let Some(bytes) = data.get(at..at.saturating_add(len)) else {
                return Err(corrupt!(
                    "string id {id}: {len} bytes at _StringData offset {at}, past its end ({} bytes)",
                    data.len()
                ));
            };
            at += len;
            let (s, exact) = decode_codepage(bytes, sp.codepage);
            sp.lossy |= !exact;
            sp.strings.push(s);
            sp.refs.push(refs);
        }
        if at != data.len() {
            return Err(corrupt!(
                "_StringPool's lengths add up to {at} bytes, _StringData has {}",
                data.len()
            ));
        }
        Ok(sp)
    }

    /// The string of id `id` (0 is "", the null string).
    pub fn get(&self, id: u32) -> Option<&str> {
        self.strings.get(id as usize).map(String::as_str)
    }

    /// The number of ids (including the null string's).
    pub fn len(&self) -> usize {
        self.strings.len()
    }

    /// Whether the pool holds no strings but the null string.
    pub fn is_empty(&self) -> bool {
        self.strings.len() <= 1
    }

    /// The bytes a string id takes in a table.
    pub fn id_size(&self) -> usize {
        if self.long_ids { 3 } else { 2 }
    }
}

fn oom(what: &str) -> Error {
    corrupt!("out of memory reading {what}")
}

// ---------------------------------------------------------------------
// Tables.

/// A column's type, as `_Columns` stores it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColumnType(pub u16);

impl ColumnType {
    const STRING: u16 = 0x0800;
    const NOT_STREAM: u16 = 0x0400;
    const LOCALIZABLE: u16 = 0x0200;
    const NULLABLE: u16 = 0x1000;
    const KEY: u16 = 0x2000;

    pub fn is_string(self) -> bool {
        self.0 & Self::STRING != 0 && self.0 & Self::NOT_STREAM != 0
    }
    pub fn is_stream(self) -> bool {
        self.0 & Self::STRING != 0 && self.0 & Self::NOT_STREAM == 0
    }
    pub fn is_int(self) -> bool {
        self.0 & Self::STRING == 0
    }
    pub fn nullable(self) -> bool {
        self.0 & Self::NULLABLE != 0
    }
    pub fn key(self) -> bool {
        self.0 & Self::KEY != 0
    }
    pub fn localizable(self) -> bool {
        self.0 & Self::LOCALIZABLE != 0
    }
    /// The size field: a string's maximum length (0: unlimited), an
    /// integer's bytes.
    pub fn size(self) -> u16 {
        self.0 & 0xff
    }
    /// The bytes one value takes in the table's stream.
    fn width(self, id_size: usize) -> usize {
        if self.is_string() {
            id_size
        } else if self.is_stream() {
            2
        } else if self.size() == 4 {
            4
        } else {
            2
        }
    }
}

impl fmt::Display for ColumnType {
    /// The type as `.idt` files write it (`s72`, `I2`, `L255`, `v0`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let c = if self.is_stream() {
            'v'
        } else if self.is_int() {
            'i'
        } else if self.localizable() {
            'l'
        } else {
            's'
        };
        let c = if self.nullable() { c.to_ascii_uppercase() } else { c };
        write!(f, "{c}{}", self.size())
    }
}

/// A column of a table.
#[derive(Clone, Debug)]
pub struct Column {
    pub name: String,
    pub ty: ColumnType,
}

/// A value in a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Null,
    Int(i32),
    Str(String),
    /// A stream column's data, by the stream's name.
    Stream(String),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i32> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }
}

impl fmt::Display for Value {
    /// As `.idt` files write a value (NULL is empty).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => Ok(()),
            Value::Int(i) => write!(f, "{i}"),
            Value::Str(s) => f.write_str(s),
            Value::Stream(s) => f.write_str(s),
        }
    }
}

/// A table read whole.
#[derive(Clone, Debug)]
pub struct Table {
    pub name: String,
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Value>>,
}

impl Table {
    /// The index of column `name`.
    pub fn column(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }
}

/// Reads `n` columns of `rows` rows from a column-major table stream.
fn read_columns(
    table: &str,
    columns: &[Column],
    bytes: &[u8],
    pool: &StringPool,
) -> Result<Vec<Vec<Value>>, Error> {
    let id = pool.id_size();
    let row_size: usize = columns.iter().map(|c| c.ty.width(id)).sum();
    if row_size == 0 {
        return Err(corrupt!("table {table} has no columns"));
    }
    if !bytes.len().is_multiple_of(row_size) {
        return Err(corrupt!(
            "table {table}: {} bytes is not a whole number of {row_size}-byte rows",
            bytes.len()
        ));
    }
    let count = bytes.len() / row_size;
    let mut rows: Vec<Vec<Value>> = Vec::new();
    rows.try_reserve_exact(count).map_err(|_| oom(table))?;
    for _ in 0..count {
        let mut r = Vec::new();
        r.try_reserve_exact(columns.len()).map_err(|_| oom(table))?;
        rows.push(r);
    }
    let mut at = 0;
    for (ci, c) in columns.iter().enumerate() {
        let w = c.ty.width(id);
        for (ri, row) in rows.iter_mut().enumerate() {
            let b = &bytes[at + ri * w..at + (ri + 1) * w];
            let raw = match w {
                2 => u32::from(u16::from_le_bytes([b[0], b[1]])),
                3 => u32::from(b[0]) | (u32::from(b[1]) << 8) | (u32::from(b[2]) << 16),
                _ => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            };
            let v = if raw == 0 {
                Value::Null
            } else if c.ty.is_string() {
                match pool.get(raw) {
                    Some(s) => Value::Str(s.to_owned()),
                    None => {
                        return Err(corrupt!(
                            "table {table} row {} column {} ({}): string id {raw} of {}",
                            ri + 1,
                            ci + 1,
                            c.name,
                            pool.len()
                        ));
                    }
                }
            } else if c.ty.is_stream() {
                // Named later, from the row's keys.
                Value::Stream(String::new())
            } else if w == 4 {
                Value::Int((raw ^ 0x8000_0000) as i32)
            } else {
                Value::Int(i32::from((raw as u16 ^ 0x8000) as i16))
            };
            row.push(v);
        }
        at += w * count;
    }
    // Stream columns: `<table>.<key>[.<key>...]`.
    if columns.iter().any(|c| c.ty.is_stream()) {
        for row in &mut rows {
            let mut name = table.to_string();
            for (c, v) in columns.iter().zip(row.iter()) {
                if c.ty.key() {
                    name.push('.');
                    name.push_str(&v.to_string());
                }
            }
            for (c, v) in columns.iter().zip(row.iter_mut()) {
                if c.ty.is_stream() && *v != Value::Null {
                    *v = Value::Stream(name.clone());
                }
            }
        }
    }
    Ok(rows)
}

// ---------------------------------------------------------------------
// Summary information.

/// The summary information stream (`\x05SummaryInformation`): the
/// properties an installer looks at ([MS-OLEPS], the Windows Installer
/// SDK's "Summary Information Stream Property Set").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub codepage: Option<i32>,
    pub title: Option<String>,
    pub subject: Option<String>,
    pub author: Option<String>,
    pub keywords: Option<String>,
    pub comments: Option<String>,
    /// "Platform;Languages", e.g. `Intel;1033` or `x64;1033`.
    pub template: Option<String>,
    /// The package code (a GUID).
    pub revision: Option<String>,
    /// The minimum installer version (200 is 2.0).
    pub page_count: Option<i32>,
    /// Source flags: 1 short names, 2 compressed, 4 administrative
    /// image, 8 no elevation needed.
    pub word_count: Option<i32>,
    pub app_name: Option<String>,
    pub security: Option<i32>,
}

impl Summary {
    /// Parses a property set stream; unknown properties are skipped.
    pub fn parse(b: &[u8]) -> Result<Summary, Error> {
        let u16_at = |at: usize| b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]]));
        let u32_at = |at: usize| {
            b.get(at..at.checked_add(4)?).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        };
        let bad = |what: &str| corrupt!("summary information: {what}");
        if u16_at(0) != Some(0xfffe) {
            return Err(bad("no byte order mark at offset 0"));
        }
        if u32_at(24).unwrap_or(0) == 0 {
            return Err(bad("no property section"));
        }
        let section = u32_at(44).ok_or_else(|| bad("cut short in the header"))? as usize;
        let count = u32_at(section + 4).ok_or_else(|| bad("section header past the end"))?;
        let mut s = Summary::default();
        let mut strings: Vec<(u32, Vec<u8>)> = Vec::new();
        for k in 0..count.min(256) as usize {
            let at = section + 8 + 8 * k;
            let (Some(id), Some(off)) = (u32_at(at), u32_at(at + 4)) else {
                return Err(bad(&format!("property {k} of {count} past the end")));
            };
            let p = section.checked_add(off as usize).ok_or_else(|| bad("an offset overflows"))?;
            let ty = u32_at(p).ok_or_else(|| bad(&format!("property {id} at {p} past the end")))?;
            match ty {
                2 => {
                    let v = u16_at(p + 4).ok_or_else(|| bad("VT_I2 cut short"))? as i16 as i32;
                    match id {
                        1 => s.codepage = Some(v),
                        19 => s.security = Some(v),
                        _ => {}
                    }
                }
                3 => {
                    let v = u32_at(p + 4).ok_or_else(|| bad("VT_I4 cut short"))? as i32;
                    match id {
                        14 => s.page_count = Some(v),
                        15 => s.word_count = Some(v),
                        19 => s.security = Some(v),
                        _ => {}
                    }
                }
                30 => {
                    let len = u32_at(p + 4).ok_or_else(|| bad("VT_LPSTR cut short"))? as usize;
                    let Some(bytes) = b.get(p + 8..(p + 8).saturating_add(len)) else {
                        return Err(bad(&format!("property {id}: {len} bytes past the end")));
                    };
                    let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
                    strings.push((id, bytes[..end].to_vec()));
                }
                _ => {}
            }
        }
        let cp = s.codepage.map(|c| c as u16 as u32).unwrap_or(0);
        for (id, bytes) in strings {
            let v = Some(decode_codepage(&bytes, cp).0);
            match id {
                2 => s.title = v,
                3 => s.subject = v,
                4 => s.author = v,
                5 => s.keywords = v,
                6 => s.comments = v,
                7 => s.template = v,
                9 => s.revision = v,
                18 => s.app_name = v,
                _ => {}
            }
        }
        Ok(s)
    }
}

// ---------------------------------------------------------------------
// Typed rows.

/// Reads typed rows: `get` takes column names to values.
struct Rows<'t> {
    table: &'t Table,
    row: usize,
}

impl Rows<'_> {
    fn value(&self, col: &str) -> Result<&Value, Error> {
        let Some(c) = self.table.column(col) else {
            return Err(corrupt!("table {} has no column {col}", self.table.name));
        };
        Ok(&self.table.rows[self.row][c])
    }
    /// A string column that must not be NULL.
    fn s(&self, col: &str) -> Result<String, Error> {
        match self.value(col)? {
            Value::Str(s) => Ok(s.clone()),
            v => Err(corrupt!(
                "table {} row {} column {col}: {v:?} where a string is required",
                self.table.name,
                self.row + 1
            )),
        }
    }
    /// A nullable string column (missing columns read as NULL).
    fn os(&self, col: &str) -> Option<String> {
        self.table
            .column(col)
            .and_then(|c| self.table.rows[self.row][c].as_str().map(str::to_owned))
    }
    fn i(&self, col: &str) -> Result<i32, Error> {
        match self.value(col)? {
            Value::Int(i) => Ok(*i),
            v => Err(corrupt!(
                "table {} row {} column {col}: {v:?} where an integer is required",
                self.table.name,
                self.row + 1
            )),
        }
    }
    fn oi(&self, col: &str) -> Option<i32> {
        self.table.column(col).and_then(|c| self.table.rows[self.row][c].as_int())
    }
}

/// Splits `short|long` and picks the long name (the short one when only
/// one is given).
pub fn long_name(s: &str) -> &str {
    s.split_once('|').map_or(s, |(_, l)| l)
}

/// The File table: one row per file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    /// The key (also the file's name in its cabinet).
    pub file: String,
    pub component: String,
    /// `short|long` or one name.
    pub file_name: String,
    pub size: i32,
    pub version: Option<String>,
    pub language: Option<String>,
    pub attributes: i32,
    pub sequence: i32,
}

impl File {
    /// msidbFileAttributesNoncompressed.
    pub const NONCOMPRESSED: i32 = 0x2000;
    /// msidbFileAttributesCompressed.
    pub const COMPRESSED: i32 = 0x4000;

    /// The long name.
    pub fn name(&self) -> &str {
        long_name(&self.file_name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Component {
    pub component: String,
    pub component_id: Option<String>,
    pub directory: String,
    pub attributes: i32,
    pub condition: Option<String>,
    pub key_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Directory {
    pub directory: String,
    pub parent: Option<String>,
    /// `[target][:source]`, each `[short|]long`; `.` is the parent.
    pub default_dir: String,
}

impl Directory {
    /// The target directory's long name (`.` for "the parent itself").
    pub fn target_name(&self) -> &str {
        long_name(self.default_dir.split_once(':').map_or(&self.default_dir, |(t, _)| t))
    }
    /// A root: no parent, or itself as parent.
    pub fn is_root(&self) -> bool {
        self.parent.as_deref().is_none_or(|p| p == self.directory)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Feature {
    pub feature: String,
    pub parent: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub display: Option<i32>,
    pub level: i32,
    pub directory: Option<String>,
    pub attributes: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registry {
    pub registry: String,
    /// -1 per-user/per-machine, 0 HKCR, 1 HKCU, 2 HKLM, 3 HKU.
    pub root: i32,
    pub key: String,
    pub name: Option<String>,
    pub value: Option<String>,
    pub component: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub shortcut: String,
    pub directory: String,
    pub name: String,
    pub component: String,
    /// A feature (advertised) or a formatted path.
    pub target: String,
    pub arguments: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub icon_index: Option<i32>,
    pub show_cmd: Option<i32>,
    pub working_dir: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Media {
    pub disk_id: i32,
    pub last_sequence: i32,
    pub disk_prompt: Option<String>,
    /// `#name`: a stream of the package; else a file next to it.
    pub cabinet: Option<String>,
    pub volume_label: Option<String>,
    pub source: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomAction {
    pub action: String,
    /// The type bits (the low 6: source and kind, e.g. 1 a DLL in the
    /// Binary table, 2 an EXE in it, 34 an EXE in a directory, 51 a
    /// property set, 65 = 1 | continue on error).
    pub ty: i32,
    pub source: Option<String>,
    pub target: Option<String>,
    pub extended_type: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binary {
    pub name: String,
    /// The stream holding the data (`Binary.<name>`), if any.
    pub stream: Option<String>,
}

/// Where a file's bytes are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileSource {
    /// In a cabinet stored in the package (the stream's name, no `#`).
    Embedded { cabinet: String, disk_id: i32 },
    /// In a cabinet file next to the package.
    External { cabinet: String, disk_id: i32 },
    /// An uncompressed file in the source tree next to the package.
    Loose { disk_id: i32 },
}

/// A file as `gala install` sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallFile {
    pub file: File,
    pub directory: String,
    /// The full target path (`<directory path><long name>`).
    pub target: String,
    pub source: FileSource,
}

// ---------------------------------------------------------------------
// The package.

/// An MSI package held in memory.
pub struct Package<'a> {
    cfb: cfb::File<'a>,
    /// Root streams by decoded name: (name, is a table, entry index).
    streams: Vec<(String, bool, usize)>,
    pub strings: StringPool,
    /// Table names from `_Tables`, in its order.
    tables: Vec<String>,
    /// Each table's columns, from `_Columns`.
    columns: BTreeMap<String, Vec<Column>>,
}

impl<'a> Package<'a> {
    /// Opens the compound file, the string pool and the catalog.
    pub fn open(data: &'a [u8]) -> Result<Package<'a>, Error> {
        let cfb = cfb::File::open(data)?;
        let mut streams = Vec::new();
        for &c in &cfb.entries()[0].children {
            let e = &cfb.entries()[c];
            if e.kind == cfb::Kind::Stream {
                let (name, table) = decode_name(&e.name);
                streams.push((name, table, c));
            }
        }
        let mut p = Package {
            cfb,
            streams,
            strings: StringPool::default(),
            tables: Vec::new(),
            columns: BTreeMap::new(),
        };
        let pool = p.table_stream("_StringPool")?.ok_or_else(|| {
            Error::NotFound("_StringPool stream (is this an MSI package?)".into())
        })?;
        let data = p.table_stream("_StringData")?.unwrap_or_default();
        p.strings = StringPool::parse(&pool, &data)?;

        let string = |n: &str| Column { name: n.into(), ty: ColumnType(0x0d40) };
        let int = |n: &str| Column { name: n.into(), ty: ColumnType(0x0502) };
        let tables_cols = [string("Name")];
        let raw = p.table_stream("_Tables")?.unwrap_or_default();
        for row in read_columns("_Tables", &tables_cols, &raw, &p.strings)? {
            if let Value::Str(s) = &row[0] {
                p.tables.push(s.clone());
            }
        }
        let columns_cols = [string("Table"), int("Number"), string("Name"), int("Type")];
        let raw = p.table_stream("_Columns")?.unwrap_or_default();
        let mut cols: BTreeMap<String, Vec<(i32, Column)>> = BTreeMap::new();
        for (ri, row) in
            read_columns("_Columns", &columns_cols, &raw, &p.strings)?.into_iter().enumerate()
        {
            let (Value::Str(t), Value::Int(n), Value::Str(name), Value::Int(ty)) =
                (&row[0], &row[1], &row[2], &row[3])
            else {
                return Err(corrupt!("_Columns row {}: a NULL value ({row:?})", ri + 1));
            };
            cols.entry(t.clone())
                .or_default()
                .push((*n, Column { name: name.clone(), ty: ColumnType(*ty as u16) }));
        }
        for (t, mut list) in cols {
            list.sort_by_key(|(n, _)| *n);
            for (k, (n, c)) in list.iter().enumerate() {
                if *n != k as i32 + 1 {
                    return Err(corrupt!(
                        "_Columns: table {t} column {} is numbered {n} (want {})",
                        c.name,
                        k + 1
                    ));
                }
            }
            p.columns.insert(t, list.into_iter().map(|(_, c)| c).collect());
        }
        Ok(p)
    }

    /// The bytes of the table stream `name` (None: no such stream, an
    /// empty table).
    fn table_stream(&self, name: &str) -> Result<Option<Vec<u8>>, Error> {
        match self.streams.iter().find(|(n, t, _)| *t && n == name) {
            Some(&(_, _, i)) => Ok(Some(self.cfb.stream(i)?)),
            None => Ok(None),
        }
    }

    /// The tables `_Tables` lists.
    pub fn tables(&self) -> &[String] {
        &self.tables
    }

    /// The names of the streams that are not tables (e.g. `Binary.X`,
    /// cabinets, `\u{5}SummaryInformation`).
    pub fn streams(&self) -> impl Iterator<Item = &str> {
        self.streams.iter().filter(|(_, t, _)| !t).map(|(n, _, _)| n.as_str())
    }

    /// The columns of table `name`.
    pub fn columns(&self, name: &str) -> Option<&[Column]> {
        self.columns.get(name).map(Vec::as_slice)
    }

    /// Reads table `name` whole.
    pub fn table(&self, name: &str) -> Result<Table, Error> {
        let Some(columns) = self.columns.get(name) else {
            return Err(Error::NotFound(format!("table {name}")));
        };
        let raw = self.table_stream(name)?.unwrap_or_default();
        let rows = read_columns(name, columns, &raw, &self.strings)?;
        Ok(Table { name: name.into(), columns: columns.clone(), rows })
    }

    /// A table that may be absent (absent reads as empty).
    fn table_or_empty(&self, name: &str) -> Result<Option<Table>, Error> {
        if self.columns.contains_key(name) { self.table(name).map(Some) } else { Ok(None) }
    }

    fn typed<T>(&self, name: &str, f: impl Fn(&Rows) -> Result<T, Error>) -> Result<Vec<T>, Error> {
        let Some(t) = self.table_or_empty(name)? else { return Ok(Vec::new()) };
        let mut out = Vec::new();
        out.try_reserve_exact(t.rows.len()).map_err(|_| oom(name))?;
        for row in 0..t.rows.len() {
            out.push(f(&Rows { table: &t, row })?);
        }
        Ok(out)
    }

    /// The bytes of a stream that is not a table (`Binary.X`, a cabinet).
    pub fn stream(&self, name: &str) -> Result<Vec<u8>, Error> {
        match self.streams.iter().find(|(n, t, _)| !*t && n == name) {
            Some(&(_, _, i)) => Ok(self.cfb.stream(i)?),
            None => Err(Error::NotFound(format!("stream {name}"))),
        }
    }

    /// The summary information.
    pub fn summary(&self) -> Result<Summary, Error> {
        Summary::parse(&self.stream("\u{5}SummaryInformation")?)
    }

    /// Whether files are compressed unless their attributes say
    /// otherwise (the summary's word count, bit 2).
    pub fn compressed_by_default(&self) -> Result<bool, Error> {
        Ok(self.summary()?.word_count.unwrap_or(0) & 2 != 0)
    }

    pub fn files(&self) -> Result<Vec<File>, Error> {
        self.typed("File", |r| {
            Ok(File {
                file: r.s("File")?,
                component: r.s("Component_")?,
                file_name: r.s("FileName")?,
                size: r.i("FileSize")?,
                version: r.os("Version"),
                language: r.os("Language"),
                attributes: r.oi("Attributes").unwrap_or(0),
                sequence: r.i("Sequence")?,
            })
        })
    }

    pub fn components(&self) -> Result<Vec<Component>, Error> {
        self.typed("Component", |r| {
            Ok(Component {
                component: r.s("Component")?,
                component_id: r.os("ComponentId"),
                directory: r.s("Directory_")?,
                attributes: r.i("Attributes")?,
                condition: r.os("Condition"),
                key_path: r.os("KeyPath"),
            })
        })
    }

    pub fn directories(&self) -> Result<Vec<Directory>, Error> {
        self.typed("Directory", |r| {
            Ok(Directory {
                directory: r.s("Directory")?,
                parent: r.os("Directory_Parent"),
                default_dir: r.s("DefaultDir")?,
            })
        })
    }

    pub fn features(&self) -> Result<Vec<Feature>, Error> {
        self.typed("Feature", |r| {
            Ok(Feature {
                feature: r.s("Feature")?,
                parent: r.os("Feature_Parent"),
                title: r.os("Title"),
                description: r.os("Description"),
                display: r.oi("Display"),
                level: r.i("Level")?,
                directory: r.os("Directory_"),
                attributes: r.i("Attributes")?,
            })
        })
    }

    pub fn registry(&self) -> Result<Vec<Registry>, Error> {
        self.typed("Registry", |r| {
            Ok(Registry {
                registry: r.s("Registry")?,
                root: r.i("Root")?,
                key: r.s("Key")?,
                name: r.os("Name"),
                value: r.os("Value"),
                component: r.s("Component_")?,
            })
        })
    }

    pub fn shortcuts(&self) -> Result<Vec<Shortcut>, Error> {
        self.typed("Shortcut", |r| {
            Ok(Shortcut {
                shortcut: r.s("Shortcut")?,
                directory: r.s("Directory_")?,
                name: r.s("Name")?,
                component: r.s("Component_")?,
                target: r.s("Target")?,
                arguments: r.os("Arguments"),
                description: r.os("Description"),
                icon: r.os("Icon_"),
                icon_index: r.oi("IconIndex"),
                show_cmd: r.oi("ShowCmd"),
                working_dir: r.os("WkDir"),
            })
        })
    }

    pub fn media(&self) -> Result<Vec<Media>, Error> {
        self.typed("Media", |r| {
            Ok(Media {
                disk_id: r.i("DiskId")?,
                last_sequence: r.i("LastSequence")?,
                disk_prompt: r.os("DiskPrompt"),
                cabinet: r.os("Cabinet"),
                volume_label: r.os("VolumeLabel"),
                source: r.os("Source"),
            })
        })
    }

    pub fn custom_actions(&self) -> Result<Vec<CustomAction>, Error> {
        self.typed("CustomAction", |r| {
            Ok(CustomAction {
                action: r.s("Action")?,
                ty: r.i("Type")?,
                source: r.os("Source"),
                target: r.os("Target"),
                extended_type: r.oi("ExtendedType"),
            })
        })
    }

    pub fn binaries(&self) -> Result<Vec<Binary>, Error> {
        self.typed("Binary", |r| {
            Ok(Binary {
                name: r.s("Name")?,
                stream: match r.value("Data")? {
                    Value::Stream(s) => Some(s.clone()),
                    _ => None,
                },
            })
        })
    }

    /// The data of Binary table row `name` (a custom action's DLL, an
    /// icon, a script).
    pub fn binary(&self, name: &str) -> Result<Vec<u8>, Error> {
        let Some(b) = self.binaries()?.into_iter().find(|b| b.name == name) else {
            return Err(Error::NotFound(format!("Binary table row {name}")));
        };
        let Some(stream) = b.stream else {
            return Err(Error::NotFound(format!("data for Binary table row {name} (NULL)")));
        };
        self.stream(&stream)
    }

    /// The target path of every directory, ending with `\`, as the
    /// installer's directory resolution would make it: `root(id)` gives a
    /// directory's value when it is set from outside (a standard folder
    /// property such as `LocalAppDataFolder` or `ProgramFilesFolder`,
    /// TARGETDIR, a public property set on the command line); otherwise
    /// a directory is its parent's path plus its DefaultDir's long
    /// target name (`.`: the parent's path itself). A root without a
    /// value takes `root("ROOTDRIVE")`; without that it and the
    /// directories below it are left out of the map. Errors (loops,
    /// missing parents) name the directory.
    pub fn resolve_directories(
        &self,
        root: &dyn Fn(&str) -> Option<String>,
    ) -> Result<BTreeMap<String, String>, Error> {
        resolve_directories(&self.directories()?, root)
    }

    /// Where file `f`'s bytes are: the Media row whose LastSequence is
    /// the first at or above the file's Sequence; compressed per the
    /// file's attributes, else the summary's word count.
    pub fn file_source(
        &self,
        f: &File,
        media: &[Media],
        compressed: bool,
    ) -> Result<FileSource, Error> {
        let Some(m) = media
            .iter()
            .filter(|m| m.last_sequence >= f.sequence)
            .min_by_key(|m| (m.last_sequence, m.disk_id))
        else {
            return Err(corrupt!(
                "file {}: sequence {} is past every Media row's LastSequence",
                f.file,
                f.sequence
            ));
        };
        let compressed = if f.attributes & File::COMPRESSED != 0 {
            true
        } else if f.attributes & File::NONCOMPRESSED != 0 {
            false
        } else {
            compressed
        };
        if !compressed {
            return Ok(FileSource::Loose { disk_id: m.disk_id });
        }
        match m.cabinet.as_deref() {
            Some(c) if c.starts_with('#') => {
                Ok(FileSource::Embedded { cabinet: c[1..].into(), disk_id: m.disk_id })
            }
            Some(c) => Ok(FileSource::External { cabinet: c.into(), disk_id: m.disk_id }),
            None => Err(corrupt!(
                "file {} is compressed but Media row {} names no cabinet",
                f.file,
                m.disk_id
            )),
        }
    }

    /// Every file with its target path (see [`Self::resolve_directories`]
    /// for `root`) and where its bytes are, in File table order.
    pub fn install_files(
        &self,
        root: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Vec<InstallFile>, Error> {
        let dirs = self.directories()?;
        let paths = resolve_directories(&dirs, root)?;
        let comps = self.components()?;
        let media = self.media()?;
        let compressed = self.compressed_by_default()?;
        let mut out = Vec::new();
        for f in self.files()? {
            let Some(c) = comps.iter().find(|c| c.component == f.component) else {
                return Err(corrupt!(
                    "file {}: component {} is not in the Component table",
                    f.file,
                    f.component
                ));
            };
            let Some(dir) = paths.get(&c.directory) else {
                if dirs.iter().any(|d| d.directory == c.directory) {
                    return Err(Error::NotFound(format!(
                        "target path for directory {} (component {}): its root directory has \
                         no value and there is no ROOTDRIVE",
                        c.directory, c.component
                    )));
                }
                return Err(corrupt!(
                    "component {}: directory {} is not in the Directory table",
                    c.component,
                    c.directory
                ));
            };
            let source = self.file_source(&f, &media, compressed)?;
            let target = format!("{dir}{}", f.name());
            out.push(InstallFile { directory: c.directory.clone(), target, source, file: f });
        }
        Ok(out)
    }

    /// The bytes of the embedded cabinet stream `name`.
    pub fn cabinet(&self, name: &str) -> Result<Vec<u8>, Error> {
        self.stream(name)
    }

    /// The bytes of file `key` (the File table's key), from its embedded
    /// cabinet, checked against the File table's size.
    pub fn extract(&self, key: &str) -> Result<Vec<u8>, Error> {
        let files = self.files()?;
        let Some(f) = files.iter().find(|f| f.file == key) else {
            return Err(Error::NotFound(format!("File table row {key}")));
        };
        let media = self.media()?;
        let compressed = self.compressed_by_default()?;
        let name = match self.file_source(f, &media, compressed)? {
            FileSource::Embedded { cabinet, .. } => cabinet,
            other => {
                return Err(Error::Unsupported(format!(
                    "file {key} is not in an embedded cabinet ({other:?})"
                )));
            }
        };
        let bytes = self.cabinet(&name)?;
        let cab = cab::Cabinet::open(&bytes).map_err(|e| Error::Cab(name.clone(), e))?;
        let Some(i) = cab.files.iter().position(|c| c.name == key) else {
            return Err(Error::NotFound(format!("file {key} in cabinet {name}")));
        };
        let data = cab.extract(i).map_err(|e| Error::Cab(name.clone(), e))?;
        check_size(f, &data, &name)?;
        Ok(data)
    }

    /// Calls `each(file, bytes)` for every file in an embedded cabinet,
    /// decompressing each cabinet folder once.
    pub fn extract_all(&self, mut each: impl FnMut(&File, &[u8])) -> Result<(), Error> {
        let files = self.files()?;
        let media = self.media()?;
        let compressed = self.compressed_by_default()?;
        let mut cabinets: Vec<String> = Vec::new();
        for f in &files {
            if let FileSource::Embedded { cabinet, .. } = self.file_source(f, &media, compressed)?
                && !cabinets.contains(&cabinet)
            {
                cabinets.push(cabinet);
            }
        }
        for name in cabinets {
            let bytes = self.cabinet(&name)?;
            let cab = cab::Cabinet::open(&bytes).map_err(|e| Error::Cab(name.clone(), e))?;
            cab.extract_all(|cf, data| {
                match files.iter().find(|f| f.file == cf.name) {
                    Some(f) => match check_size(f, data, &name) {
                        Ok(()) => {
                            each(f, data);
                            Ok(())
                        }
                        Err(e) => Err(e),
                    },
                    // A cabinet file no File row names: not installed.
                    None => Ok(()),
                }
            })
            .map_err(|e| match e {
                cab::ExtractError::Cab(e) => Error::Cab(name.clone(), e),
                cab::ExtractError::Caller(e) => e,
            })?;
        }
        Ok(())
    }
}

/// [`Package::resolve_directories`] over Directory rows.
pub fn resolve_directories(
    dirs: &[Directory],
    root: &dyn Fn(&str) -> Option<String>,
) -> Result<BTreeMap<String, String>, Error> {
    let by_id: BTreeMap<&str, &Directory> =
        dirs.iter().map(|d| (d.directory.as_str(), d)).collect();
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    'dirs: for d in dirs {
        // Walk up to a resolved or rooted ancestor, then back down.
        let mut chain: Vec<&Directory> = Vec::new();
        let mut cur = d;
        let base = loop {
            if let Some(p) = out.get(&cur.directory) {
                break p.clone();
            }
            if let Some(v) = root(&cur.directory) {
                break with_slash(v);
            }
            if cur.is_root() {
                // No value: this directory and those below it stay
                // unresolved (an error only if a file needs them).
                let Some(v) = root("ROOTDRIVE") else { continue 'dirs };
                break with_slash(v);
            }
            chain.push(cur);
            if chain.len() > dirs.len() {
                return Err(corrupt!("Directory table: {} is its own ancestor", d.directory));
            }
            let parent = cur.parent.as_deref().unwrap_or_default();
            let Some(&p) = by_id.get(parent) else {
                return Err(corrupt!(
                    "Directory table: {}'s parent {parent} is not in the table",
                    cur.directory
                ));
            };
            cur = p;
        };
        out.insert(cur.directory.clone(), base.clone());
        let mut path = base;
        for c in chain.iter().rev() {
            let name = c.target_name();
            if name != "." && !name.is_empty() {
                path.push_str(name);
                path.push('\\');
            }
            out.insert(c.directory.clone(), path.clone());
        }
    }
    Ok(out)
}

fn with_slash(mut s: String) -> String {
    if !s.ends_with('\\') {
        s.push('\\');
    }
    s
}

fn check_size(f: &File, data: &[u8], cabinet: &str) -> Result<(), Error> {
    if data.len() as u64 != f.size as u64 {
        return Err(corrupt!(
            "file {}: {} bytes in cabinet {cabinet}, the File table says {}",
            f.file,
            data.len(),
            f.size
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
