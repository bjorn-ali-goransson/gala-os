//! The TraceLogging metadata an event carries, read back into the fields
//! the program named them with (ROADMAP OT4 step 2, OBSERVABILITY
//! "A session gala starts").
//!
//! An event a program writes through TraceLogging is three blobs, not
//! one: the provider's metadata (its name), the event's metadata (its
//! name and a description of every field), and the values packed end to
//! end with nothing in them to say what they are. The values are
//! unreadable without the metadata, and the metadata is written by the
//! same `TraceLoggingWrite` that wrote them -- which is what makes this
//! decoder possible at all: nothing outside the program is needed, no
//! manifest and no symbols.
//!
//! The layout is the one `TraceLoggingProvider.h` documents (the pinned
//! Windows SDK's public header; CLEAN_ROOM: a header, not disassembly):
//!
//! - **Provider metadata**: a `u16` size counting itself, the provider's
//!   name in UTF-8 with a NUL, then chunks this decoder does not need.
//! - **Event metadata**: a `u16` size counting itself, the event's tag
//!   (bytes carrying seven bits each, ending at the first byte with the
//!   high bit unset), the event's name in UTF-8 with a NUL, then one
//!   description per field.
//! - **A field**: its name in UTF-8 with a NUL, an in-type byte, then an
//!   out-type byte only if the in-type's high bit is set, then a tag only
//!   if the out-type's high bit is set, then a `u16` count only if the
//!   in-type asks for a fixed array.
//! - **The values**: in the fields' order, each as long as its type says.
//!   A fixed array is its values; a variable array is a `u16` count and
//!   then its values; a string, a blob or a SID carries its own length.
//!
//! A field's *tag* is read and not printed: a tag is the program's note
//! to its own consumers (Microsoft's mark what may leave the machine),
//! it names nothing a reader of a serial log needs, and misreading its
//! length would throw every field after it out of step -- which is the
//! thing the decoder has to get right.
//!
//! What cannot be read is said in the line rather than left out: a field
//! whose type this decoder does not know, a value the record was cut
//! short of, bytes no field claimed. A line stops where it stopped and
//! says why, because the values have no lengths of their own -- one
//! field misread leaves every one after it meaningless, and a decoder
//! that quietly drops the rest of an event cannot be told from a program
//! that never wrote it.

use core::fmt::{self, Write};

use crate::time::Civil;

/// The in-type's own bits (`_TlgInTypeMask`).
const TYPE: u8 = 31;
/// The two bits that say a field is an array (`_TlgInCcount`,
/// `TlgInVcount`): neither for one value, the first for a count in the
/// metadata, the second for a count in the data, both for a custom type
/// whose own description says how long it is.
const COUNT: u8 = 0x60;
const CCOUNT: u8 = 0x20;
const VCOUNT: u8 = 0x40;
const CUSTOM: u8 = 0x60;
/// The high bit of an in-type or an out-type: another byte follows
/// (`_TlgInChain`, `_TlgOutChain`).
const CHAIN: u8 = 0x80;

// In-types (`TlgIn_t`). Type 16 is a pointer, which TraceLogging itself
// calls unsupported and its macros never write: its width is the
// program's and nothing in the event says which, so it is left out
// rather than guessed -- a wrong width would put every field after it
// out of step and print values that look like the program's.
const IN_NULL: u8 = 0;
const IN_UNICODESTRING: u8 = 1;
const IN_ANSISTRING: u8 = 2;
const IN_INT8: u8 = 3;
const IN_UINT8: u8 = 4;
const IN_INT16: u8 = 5;
const IN_UINT16: u8 = 6;
const IN_INT32: u8 = 7;
const IN_UINT32: u8 = 8;
const IN_INT64: u8 = 9;
const IN_UINT64: u8 = 10;
const IN_FLOAT: u8 = 11;
const IN_DOUBLE: u8 = 12;
const IN_BOOL32: u8 = 13;
const IN_BINARY: u8 = 14;
const IN_GUID: u8 = 15;
const IN_FILETIME: u8 = 17;
const IN_SYSTEMTIME: u8 = 18;
const IN_SID: u8 = 19;
const IN_HEXINT32: u8 = 20;
const IN_HEXINT64: u8 = 21;
const IN_COUNTEDSTRING: u8 = 22;
const IN_COUNTEDANSISTRING: u8 = 23;
const IN_STRUCT: u8 = 24;
const IN_COUNTEDBINARY: u8 = 25;

// Out-types (`TlgOut_t`), those that change what a value means.
const OUT_BOOLEAN: u8 = 3;
const OUT_HEX: u8 = 4;
const OUT_PORT: u8 = 7;
const OUT_IPV4: u8 = 8;
const OUT_NTSTATUS: u8 = 14;
const OUT_HRESULT: u8 = 15;
const OUT_SIGNED: u8 = 17;
const OUT_UNSIGNED: u8 = 18;

/// The deepest nesting of structs a line follows. TraceLogging's own
/// macros nest two or three; past this a line is unreadable anyway, and
/// metadata that goes deeper is more likely malformed than deep.
const MAX_DEPTH: u8 = 8;

/// The most values of one array a line prints. The rest are counted and
/// their bytes still taken, so the fields after the array stay in step:
/// a line that runs to a thousand numbers is no longer a line anyone
/// reads, and dropping the fields behind it would lose more.
const MAX_VALUES: u16 = 32;

/// The most bytes of one blob a line prints, for the same reason.
const MAX_BLOB: usize = 32;

/// Bytes read in order, which is what both halves of an event are.
struct Bytes<'a> {
    all: &'a [u8],
    at: usize,
}

impl<'a> Bytes<'a> {
    fn new(all: &'a [u8]) -> Self {
        Bytes { all, at: 0 }
    }

    fn left(&self) -> usize {
        self.all.len().saturating_sub(self.at)
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let piece = self.all.get(self.at..end)?;
        self.at = end;
        Some(piece)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    /// A UTF-8 string up to its NUL, without it.
    fn cstr(&mut self) -> Option<&'a [u8]> {
        let rest = self.all.get(self.at..)?;
        let end = rest.iter().position(|&b| b == 0)?;
        self.at += end + 1;
        Some(&rest[..end])
    }

    /// A UTF-16 string up to its NUL unit, as the bytes before it.
    fn wstr(&mut self) -> Option<&'a [u8]> {
        let rest = self.all.get(self.at..)?;
        let mut end = 0;
        while end + 1 < rest.len() {
            if rest[end] == 0 && rest[end + 1] == 0 {
                self.at += end + 2;
                return Some(&rest[..end]);
            }
            end += 2;
        }
        None
    }

    /// A tag: seven bits a byte, most significant first, ending at the
    /// first byte with the high bit unset. Its value is not used (the
    /// module's note says why); reading its length right is.
    fn tag(&mut self) -> Option<u32> {
        let mut tag = 0u32;
        for shift in [21u32, 14, 7, 0] {
            let byte = self.u8()?;
            tag |= u32::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(tag);
            }
        }
        // Four bytes hold the whole of a 28-bit tag; a fifth means these
        // bytes are not a tag at all.
        None
    }
}

/// One event's metadata, with the two names read out of it.
pub struct Event<'a> {
    /// The provider's name, as its own metadata gives it.
    pub provider: &'a [u8],
    /// The event's name.
    pub name: &'a [u8],
    /// The description of the fields, after the event's name.
    fields: &'a [u8],
    /// The values.
    data: &'a [u8],
    /// Whether the record was cut to fit, which is why the values may
    /// end before the fields do.
    cut: bool,
}

impl<'a> Event<'a> {
    /// Reads the two names, leaving the fields for the line to walk.
    /// `None` when the blobs are not TraceLogging's, which is what a
    /// manifest provider's event looks like.
    pub fn parse(
        provider_meta: &'a [u8],
        event_meta: &'a [u8],
        data: &'a [u8],
        cut: bool,
    ) -> Option<Self> {
        let mut provider = Bytes::new(provider_meta);
        let size = usize::from(provider.u16()?);
        if size > provider_meta.len() {
            return None;
        }
        let provider_name = provider.cstr()?;

        let mut event = Bytes::new(event_meta);
        let size = usize::from(event.u16()?);
        if size > event_meta.len() || size < 3 {
            return None;
        }
        event.tag()?;
        let name = event.cstr()?;
        let fields = event_meta.get(event.at..size)?;
        Some(Event { provider: provider_name, name, fields, data, cut })
    }

    /// The fields as a line says them: `, name=value` for each, and
    /// nothing at all for an event that has none.
    pub fn fields(&self) -> Fields<'_> {
        Fields(self)
    }
}

/// The fields of one event, written as a line's tail.
pub struct Fields<'a>(&'a Event<'a>);

impl fmt::Display for Fields<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut meta = Bytes::new(self.0.fields);
        let mut data = Bytes::new(self.0.data);
        let mut walk = Walk { stopped: false, cut: self.0.cut };
        group(f, &mut meta, &mut data, None, 0, &mut walk)?;
        if !walk.stopped && data.left() != 0 {
            // Said and not swallowed: bytes no field claimed mean some
            // field's length was read wrongly, and a line that hides
            // that is a line that cannot be trusted.
            write!(f, ", (+{} byte(s) no field claims)", data.left())?;
        }
        Ok(())
    }
}

/// How far a line got.
struct Walk {
    stopped: bool,
    cut: bool,
}

impl Walk {
    /// Stops the line, saying why.
    fn stop<W: Write>(&mut self, w: &mut W, why: &str) -> fmt::Result {
        self.stopped = true;
        if self.cut {
            // The record gala kept was cut to fit, so the values ended
            // before the fields did and nothing is wrong with the event.
            w.write_str(" (the rest was cut to fit)")
        } else {
            write!(w, " ({why})")
        }
    }
}

/// Writes a group of fields: every one the metadata holds (`count` of
/// `None`, the event itself), or exactly `count` of them (a struct's).
fn group<W: Write>(
    w: &mut W,
    meta: &mut Bytes<'_>,
    data: &mut Bytes<'_>,
    count: Option<u8>,
    depth: u8,
    walk: &mut Walk,
) -> fmt::Result {
    let mut n = 0u8;
    loop {
        match count {
            Some(c) if n >= c => return Ok(()),
            None if meta.left() == 0 => return Ok(()),
            _ => {}
        }
        if walk.stopped {
            return Ok(());
        }
        // Every field of the event is a line's `, name=value`; inside a
        // struct the first follows the brace.
        if count.is_none() || n > 0 {
            w.write_str(", ")?;
        }
        n = n.saturating_add(1);

        let Some(name) = meta.cstr() else {
            w.write_str("(the field metadata ends mid-name)")?;
            walk.stopped = true;
            return Ok(());
        };
        let Some(in_type) = meta.u8() else {
            write!(w, "{}=?", Text(name))?;
            return walk.stop(w, "the field metadata ends before its type");
        };
        let mut out_type = 0u8;
        if in_type & CHAIN != 0 {
            let Some(out) = meta.u8() else {
                write!(w, "{}=?", Text(name))?;
                return walk.stop(w, "the field metadata ends before its out-type");
            };
            out_type = out & !CHAIN;
            if out & CHAIN != 0 && meta.tag().is_none() {
                write!(w, "{}=?", Text(name))?;
                return walk.stop(w, "the field's tag does not end");
            }
        }
        let kind = in_type & TYPE;
        // The count the metadata gives, the count the data gives, or
        // neither. A custom type's own description says what its bytes
        // mean, and this decoder cannot read one.
        let values = match in_type & COUNT {
            CCOUNT => match meta.u16() {
                Some(c) => Some(c),
                None => {
                    write!(w, "{}=?", Text(name))?;
                    return walk.stop(w, "the field metadata ends before its array count");
                }
            },
            VCOUNT => match data.u16() {
                Some(c) => Some(c),
                None => {
                    write!(w, "{}=?", Text(name))?;
                    return walk.stop(w, "the values end before this array's count");
                }
            },
            CUSTOM => {
                write!(w, "{}=?", Text(name))?;
                return walk.stop(w, "a custom type, whose length only its own description gives");
            }
            _ => None,
        };

        write!(w, "{}=", Text(name))?;
        if kind == IN_STRUCT {
            // A struct carries no value of its own: its out-type is how
            // many of the fields after it are its own.
            structure(w, meta, data, out_type, values, depth, walk)?;
            continue;
        }

        match values {
            None => {
                if !value(w, kind, out_type, data, walk)? {
                    return Ok(());
                }
            }
            Some(elements) => {
                w.write_str("[")?;
                let mut printed = 0u16;
                while printed < elements.min(MAX_VALUES) {
                    if printed > 0 {
                        w.write_str(", ")?;
                    }
                    printed += 1;
                    if !value(w, kind, out_type, data, walk)? {
                        break;
                    }
                }
                if !walk.stopped && elements > printed {
                    write!(w, ", ...({} more)", elements - printed)?;
                    let was = walk.stopped;
                    for _ in printed..elements {
                        if !value(&mut Nowhere, kind, out_type, data, walk)? {
                            break;
                        }
                    }
                    if !was && walk.stopped {
                        w.write_str("]")?;
                        return walk.stop(w, "the values end inside this array");
                    }
                }
                w.write_str("]")?;
                if walk.stopped {
                    return Ok(());
                }
            }
        }
    }
}

/// Writes a struct's fields, or an array of them: one description, read
/// again from its start for every element.
fn structure<W: Write>(
    w: &mut W,
    meta: &mut Bytes<'_>,
    data: &mut Bytes<'_>,
    fields: u8,
    values: Option<u16>,
    depth: u8,
    walk: &mut Walk,
) -> fmt::Result {
    if depth >= MAX_DEPTH {
        return walk.stop(w, "structs nested deeper than a line follows");
    }
    let Some(elements) = values else {
        w.write_str("{")?;
        group(w, meta, data, Some(fields), depth + 1, walk)?;
        return w.write_str("}");
    };
    let start = meta.at;
    w.write_str("[")?;
    let mut printed = 0u16;
    while printed < elements.min(MAX_VALUES) {
        if printed > 0 {
            w.write_str(", ")?;
        }
        printed += 1;
        meta.at = start;
        w.write_str("{")?;
        group(w, meta, data, Some(fields), depth + 1, walk)?;
        w.write_str("}")?;
        if walk.stopped {
            break;
        }
    }
    if !walk.stopped && elements > printed {
        write!(w, ", ...({} more)", elements - printed)?;
    }
    // The elements no line printed must still take their values: the
    // fields behind them are read from where these end.
    let was = walk.stopped;
    for _ in printed..elements {
        meta.at = start;
        group(&mut Nowhere, meta, data, Some(fields), depth + 1, walk)?;
    }
    w.write_str("]")?;
    if !was && walk.stopped {
        return walk.stop(w, "the values end inside this array");
    }
    if elements == 0 {
        // An array with no elements at all has no values in the event,
        // but its fields are described there all the same, and the
        // fields behind it are read from where that description ends.
        // So it is stepped over in the metadata alone: a walk that read
        // values here would find none and stop a line over an event
        // that is perfectly well formed.
        meta.at = start;
        if !step_over(meta, fields, depth + 1) {
            return walk.stop(w, "the field metadata ends inside this array's description");
        }
    }
    Ok(())
}

/// Steps over `fields` field descriptions in the metadata, touching no
/// values. False if the metadata ends inside one, or if the structs nest
/// deeper than this follows.
fn step_over(meta: &mut Bytes<'_>, fields: u8, depth: u8) -> bool {
    if depth >= MAX_DEPTH {
        return false;
    }
    for _ in 0..fields {
        if meta.cstr().is_none() {
            return false;
        }
        let Some(in_type) = meta.u8() else { return false };
        let mut out_type = 0u8;
        if in_type & CHAIN != 0 {
            let Some(out) = meta.u8() else { return false };
            out_type = out & !CHAIN;
            if out & CHAIN != 0 && meta.tag().is_none() {
                return false;
            }
        }
        match in_type & COUNT {
            // A count the metadata gives is part of the description. A
            // count the data gives is not read: no element of the array
            // being stepped over has one, because it has no elements.
            CCOUNT => {
                if meta.u16().is_none() {
                    return false;
                }
            }
            CUSTOM => {
                let Some(length) = meta.u16() else { return false };
                if meta.take(usize::from(length)).is_none() {
                    return false;
                }
            }
            _ => {}
        }
        // A struct's out-type is how many of the fields after it are its
        // own, and they are described once, here.
        if in_type & TYPE == IN_STRUCT && !step_over(meta, out_type, depth + 1) {
            return false;
        }
    }
    true
}

/// A sink for the values a line does not print but must still step over.
struct Nowhere;

impl Write for Nowhere {
    fn write_str(&mut self, _: &str) -> fmt::Result {
        Ok(())
    }
}

/// Writes one value and takes its bytes. False once the line has
/// stopped, with the reason already written.
fn value<W: Write>(
    w: &mut W,
    kind: u8,
    out: u8,
    data: &mut Bytes<'_>,
    walk: &mut Walk,
) -> Result<bool, fmt::Error> {
    /// The bytes this field needed and the record did not have.
    macro_rules! need {
        ($read:expr) => {
            match $read {
                Some(v) => v,
                None => {
                    w.write_str("?")?;
                    walk.stop(w, "the values end here")?;
                    return Ok(false);
                }
            }
        };
    }

    match kind {
        IN_NULL => w.write_str("null")?,
        IN_INT8 => {
            let v = need!(data.u8());
            integer(w, u64::from(v), out, 8, true)?;
        }
        IN_UINT8 => {
            let v = need!(data.u8());
            integer(w, u64::from(v), out, 8, false)?;
        }
        IN_INT16 => {
            let v = need!(data.u16());
            integer(w, u64::from(v), out, 16, true)?;
        }
        IN_UINT16 => {
            let v = need!(data.u16());
            if out == OUT_PORT {
                // A port is the one value TraceLogging keeps the other
                // way round, as it travels (`TlgOutPORT`).
                write!(w, "{}", v.swap_bytes())?;
            } else {
                integer(w, u64::from(v), out, 16, false)?;
            }
        }
        IN_INT32 => {
            let v = need!(data.u32());
            integer(w, u64::from(v), out, 32, true)?;
        }
        IN_UINT32 | IN_HEXINT32 => {
            let v = need!(data.u32());
            if out == OUT_IPV4 {
                let b = v.to_le_bytes();
                write!(w, "{}.{}.{}.{}", b[0], b[1], b[2], b[3])?;
            } else {
                // A hex in-type is hex unless the field says otherwise.
                let out = if kind == IN_HEXINT32 && out == 0 { OUT_HEX } else { out };
                integer(w, u64::from(v), out, 32, false)?;
            }
        }
        IN_INT64 => {
            let v = need!(data.u64());
            integer(w, v, out, 64, true)?;
        }
        IN_UINT64 | IN_HEXINT64 => {
            let v = need!(data.u64());
            let out = if kind == IN_HEXINT64 && out == 0 { OUT_HEX } else { out };
            integer(w, v, out, 64, false)?;
        }
        IN_FLOAT => {
            let v = f32::from_bits(need!(data.u32()));
            write!(w, "{v}")?;
        }
        IN_DOUBLE => {
            let v = f64::from_bits(need!(data.u64()));
            write!(w, "{v}")?;
        }
        IN_BOOL32 => {
            let v = need!(data.u32());
            write!(w, "{}", v != 0)?;
        }
        IN_GUID => {
            let bytes: [u8; 16] = need!(data.take(16).and_then(|b| b.try_into().ok()));
            write!(w, "{}", super::Guid(&bytes))?;
        }
        IN_FILETIME => {
            let v = need!(data.u64());
            write!(w, "{}", Civil::from_filetime(v))?;
        }
        IN_SYSTEMTIME => {
            let b = need!(data.take(16));
            let at = |i: usize| u16::from_le_bytes([b[i * 2], b[i * 2 + 1]]);
            // SYSTEMTIME: year, month, day of the week, day, hour,
            // minute, second, milliseconds. The day of the week says
            // nothing the date does not.
            write!(
                w,
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                at(0),
                at(1),
                at(3),
                at(4),
                at(5),
                at(6)
            )?;
        }
        IN_UNICODESTRING => {
            let bytes = need!(data.wstr());
            wide(w, bytes)?;
        }
        IN_COUNTEDSTRING => {
            let n = usize::from(need!(data.u16()));
            let bytes = need!(data.take(n));
            wide(w, bytes)?;
        }
        IN_ANSISTRING => {
            let bytes = need!(data.cstr());
            write!(w, "\"{}\"", Text(bytes))?;
        }
        IN_COUNTEDANSISTRING => {
            let n = usize::from(need!(data.u16()));
            let bytes = need!(data.take(n));
            write!(w, "\"{}\"", Text(bytes))?;
        }
        IN_BINARY | IN_COUNTEDBINARY => {
            let n = usize::from(need!(data.u16()));
            let bytes = need!(data.take(n));
            for byte in bytes.iter().take(MAX_BLOB) {
                write!(w, "{byte:02x}")?;
            }
            if bytes.len() > MAX_BLOB {
                write!(w, "...({} more byte(s))", bytes.len() - MAX_BLOB)?;
            }
        }
        IN_SID => {
            // A SID is as long as it says: a revision, a count of
            // sub-authorities, a six-byte authority, then that many
            // 32-bit sub-authorities.
            let head = need!(data.take(8));
            let subs = usize::from(head[1]);
            let mut authority = 0u64;
            for &byte in &head[2..8] {
                authority = (authority << 8) | u64::from(byte);
            }
            write!(w, "S-{}-{authority}", head[0])?;
            for _ in 0..subs {
                let sub = need!(data.u32());
                write!(w, "-{sub}")?;
            }
        }
        _ => {
            w.write_str("?")?;
            walk.stop(w, "a type this decoder does not know")?;
            return Ok(false);
        }
    }
    Ok(true)
}

/// An integer as its out-type says it is read: `bits` is its width, so a
/// status reads as wide as it is and a sign extends from where it ends.
fn integer<W: Write>(w: &mut W, v: u64, out: u8, bits: u32, signed: bool) -> fmt::Result {
    let mask = if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 };
    let bare = v & mask;
    // The same bits read as a sign, for a field that has one and for an
    // out-type that asks for one.
    let as_signed = ((bare << (64 - bits)) as i64) >> (64 - bits);
    match out {
        OUT_HEX => write!(w, "{bare:#x}"),
        OUT_BOOLEAN => write!(w, "{}", bare != 0),
        OUT_NTSTATUS | OUT_HRESULT => {
            write!(w, "{:#0width$x}", bare, width = (bits as usize / 4) + 2)
        }
        OUT_SIGNED => write!(w, "{as_signed}"),
        OUT_UNSIGNED => write!(w, "{bare}"),
        // Whatever the field says it is: an out-type this decoder does
        // not know must not turn a count into a negative number, which
        // is what reading every value as an i64 did.
        _ if signed => write!(w, "{as_signed}"),
        _ => write!(w, "{bare}"),
    }
}

/// A UTF-16 string as a line says it, in quotes.
fn wide<W: Write>(w: &mut W, bytes: &[u8]) -> fmt::Result {
    w.write_str("\"")?;
    let units = bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    for c in char::decode_utf16(units) {
        let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
        w.write_char(if c.is_control() { '.' } else { c })?;
    }
    if !bytes.len().is_multiple_of(2) {
        w.write_str("...(an odd byte)")?;
    }
    w.write_str("\"")
}

/// Bytes a program wrote as text, as a line can say them: a name or a
/// string is the program's, so an invalid byte is shown and not refused,
/// and a control character would end the line it is printed in.
pub struct Text<'a>(pub &'a [u8]);

impl fmt::Display for Text<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut rest = self.0;
        while !rest.is_empty() {
            match core::str::from_utf8(rest) {
                Ok(text) => return say(f, text),
                Err(e) => {
                    let good = e.valid_up_to();
                    if let Ok(text) = core::str::from_utf8(&rest[..good]) {
                        say(f, text)?;
                    }
                    f.write_char('?')?;
                    let skip = e.error_len().unwrap_or(rest.len() - good);
                    rest = &rest[(good + skip).min(rest.len())..];
                }
            }
        }
        Ok(())
    }
}

/// Text with its control characters shown as dots.
fn say(f: &mut fmt::Formatter<'_>, text: &str) -> fmt::Result {
    for c in text.chars() {
        f.write_char(if c.is_control() { '.' } else { c })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        CCOUNT, CHAIN, CUSTOM, Event, IN_ANSISTRING, IN_BINARY, IN_COUNTEDANSISTRING,
        IN_COUNTEDSTRING, IN_FILETIME, IN_GUID, IN_HEXINT64, IN_INT8, IN_INT16, IN_INT32, IN_INT64,
        IN_SID, IN_STRUCT, IN_UINT8, IN_UINT16, IN_UINT32, IN_UINT64, IN_UNICODESTRING,
        OUT_BOOLEAN, OUT_HEX, OUT_HRESULT, OUT_IPV4, OUT_PORT, OUT_UNSIGNED, VCOUNT,
    };
    use std::string::String;
    use std::vec::Vec;

    /// The blobs a `TraceLoggingWrite` builds, as a test builds them:
    /// the same layout `user/etwfields_test` writes from a program, so a
    /// test here and a scenario there check one decoder.
    struct Build {
        bytes: Vec<u8>,
    }

    impl Build {
        fn provider(name: &str) -> Vec<u8> {
            let mut b = Build { bytes: Vec::new() };
            b.bytes.extend_from_slice(&0u16.to_le_bytes());
            b.bytes.extend_from_slice(name.as_bytes());
            b.bytes.push(0);
            b.done()
        }

        fn event(name: &str) -> Self {
            let mut b = Build { bytes: Vec::new() };
            b.bytes.extend_from_slice(&0u16.to_le_bytes());
            b.bytes.push(0); // the event's tag
            b.bytes.extend_from_slice(name.as_bytes());
            b.bytes.push(0);
            b
        }

        fn field(&mut self, name: &str, in_type: u8) -> &mut Self {
            self.bytes.extend_from_slice(name.as_bytes());
            self.bytes.push(0);
            self.bytes.push(in_type);
            self
        }

        fn out(&mut self, name: &str, in_type: u8, out_type: u8) -> &mut Self {
            self.field(name, in_type | CHAIN);
            self.bytes.push(out_type);
            self
        }

        fn array(&mut self, name: &str, in_type: u8, count: u16) -> &mut Self {
            self.field(name, in_type | CCOUNT);
            self.bytes.extend_from_slice(&count.to_le_bytes());
            self
        }

        fn list(&mut self, name: &str, in_type: u8) -> &mut Self {
            self.field(name, in_type | VCOUNT)
        }

        fn structure(&mut self, name: &str, fields: u8) -> &mut Self {
            self.out(name, IN_STRUCT, fields)
        }

        /// A fixed array of structs: the out-type is the group's size and
        /// comes before the count, which is the order that decides
        /// whether every field after it reads right.
        fn structures(&mut self, name: &str, fields: u8, count: u16) -> &mut Self {
            self.field(name, IN_STRUCT | CHAIN | CCOUNT);
            self.bytes.push(fields);
            self.bytes.extend_from_slice(&count.to_le_bytes());
            self
        }

        fn done(&mut self) -> Vec<u8> {
            let size = (self.bytes.len() as u16).to_le_bytes();
            self.bytes[..2].copy_from_slice(&size);
            core::mem::take(&mut self.bytes)
        }
    }

    fn line(event: &mut Build, data: &[u8]) -> String {
        decode(event, data, false)
    }

    fn decode(event: &mut Build, data: &[u8], cut: bool) -> String {
        let provider = Build::provider("Gala.Test.Fields");
        let meta = event.done();
        let event =
            Event::parse(&provider, &meta, data, cut).expect("the blobs are TraceLogging's");
        assert_eq!(event.provider, b"Gala.Test.Fields");
        std::format!("{}", event.fields())
    }

    #[test]
    fn the_names_come_out_of_the_metadata() {
        let provider = Build::provider("Gala.Test.Fields");
        let meta = Build::event("Nothing").done();
        let event = Event::parse(&provider, &meta, &[], false).unwrap();
        assert_eq!(event.provider, b"Gala.Test.Fields");
        assert_eq!(event.name, b"Nothing");
        // An event with no fields says nothing, rather than reading as
        // one whose fields were lost.
        assert_eq!(std::format!("{}", event.fields()), "");
    }

    #[test]
    fn every_integer_width_reads_back() {
        let mut event = Build::event("Scalars");
        event
            .field("i8", IN_INT8)
            .field("u8", IN_UINT8)
            .field("i16", IN_INT16)
            .field("u16", IN_UINT16)
            .field("i32", IN_INT32)
            .field("u32", IN_UINT32)
            .field("i64", IN_INT64)
            .field("u64", IN_UINT64);
        let mut data = Vec::new();
        data.extend_from_slice(&(-8i8).to_le_bytes());
        data.extend_from_slice(&200u8.to_le_bytes());
        data.extend_from_slice(&(-1600i16).to_le_bytes());
        data.extend_from_slice(&60000u16.to_le_bytes());
        data.extend_from_slice(&(-32000i32).to_le_bytes());
        data.extend_from_slice(&4_000_000_000u32.to_le_bytes());
        data.extend_from_slice(&(-6_400_000_000i64).to_le_bytes());
        data.extend_from_slice(&18_000_000_000_000_000_000u64.to_le_bytes());
        assert_eq!(
            line(&mut event, &data),
            ", i8=-8, u8=200, i16=-1600, u16=60000, i32=-32000, u32=4000000000, \
             i64=-6400000000, u64=18000000000000000000"
        );
    }

    #[test]
    fn the_four_string_types_are_each_found_by_their_own_length() {
        let mut event = Build::event("Texts");
        event
            .field("wide", IN_UNICODESTRING)
            .field("ansi", IN_ANSISTRING)
            .field("cwide", IN_COUNTEDSTRING)
            .field("cansi", IN_COUNTEDANSISTRING)
            .field("after", IN_UINT8);
        let mut data = Vec::new();
        for unit in "wide".encode_utf16().chain(core::iter::once(0)) {
            data.extend_from_slice(&unit.to_le_bytes());
        }
        data.extend_from_slice(b"ansi\0");
        let counted: Vec<u8> = "cwide".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        data.extend_from_slice(&(counted.len() as u16).to_le_bytes());
        data.extend_from_slice(&counted);
        data.extend_from_slice(&5u16.to_le_bytes());
        data.extend_from_slice(b"cansi");
        data.push(7);
        // The field after them is the real check: a string whose length
        // was read wrongly puts every value behind it out of step.
        assert_eq!(
            line(&mut event, &data),
            ", wide=\"wide\", ansi=\"ansi\", cwide=\"cwide\", cansi=\"cansi\", after=7"
        );
    }

    #[test]
    fn a_struct_holds_the_fields_after_it() {
        let mut event = Build::event("Shapes");
        event
            .structure("rect", 2)
            .field("left", IN_INT32)
            .field("top", IN_INT32)
            .field("after", IN_UINT8);
        let mut data = Vec::new();
        data.extend_from_slice(&(-1i32).to_le_bytes());
        data.extend_from_slice(&2i32.to_le_bytes());
        data.push(9);
        assert_eq!(line(&mut event, &data), ", rect={left=-1, top=2}, after=9");
    }

    #[test]
    fn a_count_too_large_for_a_sign_keeps_its_size_whatever_its_out_type() {
        // The width Word's own counters use, with and without an
        // out-type: read through a sign either one would be negative.
        let mut event = Build::event("Big");
        event.field("plain", IN_UINT64).out("counted", IN_UINT64, OUT_UNSIGNED);
        let data = [0xffu8; 16];
        assert_eq!(
            line(&mut event, &data),
            ", plain=18446744073709551615, counted=18446744073709551615"
        );
    }

    #[test]
    fn an_array_of_structs_reads_one_description_many_times() {
        let mut event = Build::event("Rects");
        event.field("count", IN_UINT8).structures("rects", 2, 2);
        event.field("x", IN_UINT8).field("y", IN_UINT8);
        event.field("after", IN_UINT8);
        let data = [3u8, 1, 2, 3, 4, 9];
        assert_eq!(line(&mut event, &data), ", count=3, rects=[{x=1, y=2}, {x=3, y=4}], after=9");
    }

    #[test]
    fn an_empty_array_of_structs_still_steps_over_its_description() {
        let mut event = Build::event("Rects");
        event.structures("rects", 2, 0);
        event.field("x", IN_UINT8).field("y", IN_UINT8);
        event.field("after", IN_UINT8);
        let data = [9u8];
        assert_eq!(line(&mut event, &data), ", rects=[], after=9");
    }

    #[test]
    fn the_two_array_kinds_find_their_counts_in_their_own_places() {
        let mut event = Build::event("Shapes");
        event.array("three", IN_UINT32, 3).list("list", IN_UINT16).field("after", IN_UINT8);
        let mut data = Vec::new();
        for v in [10u32, 20, 30] {
            data.extend_from_slice(&v.to_le_bytes());
        }
        data.extend_from_slice(&4u16.to_le_bytes());
        for v in [1u16, 2, 3, 4] {
            data.extend_from_slice(&v.to_le_bytes());
        }
        data.push(9);
        assert_eq!(line(&mut event, &data), ", three=[10, 20, 30], list=[1, 2, 3, 4], after=9");
    }

    #[test]
    fn an_array_longer_than_a_line_prints_counts_the_rest_and_stays_in_step() {
        let mut event = Build::event("Many");
        event.list("many", IN_UINT8).field("after", IN_UINT8);
        let mut data = Vec::new();
        data.extend_from_slice(&40u16.to_le_bytes());
        for i in 0..40u8 {
            data.push(i);
        }
        data.push(9);
        let text = line(&mut event, &data);
        assert!(text.contains("31, ...(8 more)]"), "{text}");
        assert!(text.ends_with(", after=9"), "{text}");
    }

    #[test]
    fn a_tag_is_stepped_over_and_not_printed() {
        let mut event = Build::event("Tagged");
        event.out("tagged", IN_UINT32, CHAIN);
        event.bytes.push(0x12); // a one-byte tag
        event.field("after", IN_UINT8);
        let mut data = Vec::new();
        data.extend_from_slice(&99u32.to_le_bytes());
        data.push(9);
        assert_eq!(line(&mut event, &data), ", tagged=99, after=9");
    }

    #[test]
    fn the_out_types_that_change_a_value_do() {
        let mut event = Build::event("Shown");
        event
            .out("ashex", IN_UINT32, OUT_HEX)
            .out("aserror", IN_UINT32, 13)
            .out("ashresult", IN_UINT32, OUT_HRESULT)
            .out("asbool", IN_UINT8, OUT_BOOLEAN)
            .out("asport", IN_UINT16, OUT_PORT)
            .out("asip", IN_UINT32, OUT_IPV4);
        let mut data = Vec::new();
        data.extend_from_slice(&0x00c0_ffeeu32.to_le_bytes());
        data.extend_from_slice(&5u32.to_le_bytes());
        data.extend_from_slice(&0x8000_4005u32.to_le_bytes());
        data.push(1);
        data.extend_from_slice(&443u16.swap_bytes().to_le_bytes());
        data.extend_from_slice(&[10, 0, 0, 7]);
        assert_eq!(
            line(&mut event, &data),
            ", ashex=0xc0ffee, aserror=5, ashresult=0x80004005, asbool=true, \
             asport=443, asip=10.0.0.7"
        );
    }

    #[test]
    fn a_guid_a_filetime_and_a_blob_read_as_themselves() {
        let mut event = Build::event("Bytes");
        event
            .field("blob", IN_BINARY)
            .field("id", IN_GUID)
            .field("when", IN_FILETIME)
            .field("hex64", IN_HEXINT64);
        let mut data = Vec::new();
        data.extend_from_slice(&4u16.to_le_bytes());
        data.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        data.extend_from_slice(&[
            0x22, 0x22, 0x11, 0x11, 0x33, 0x33, 0x44, 0x44, 0x55, 0x55, 0x66, 0x66, 0x77, 0x77,
            0x88, 0x88,
        ]);
        data.extend_from_slice(&134_398_485_030_000_000u64.to_le_bytes());
        data.extend_from_slice(&0x0123_4567_89ab_cdefu64.to_le_bytes());
        assert_eq!(
            line(&mut event, &data),
            ", blob=deadbeef, id=11112222-3333-4444-5555-666677778888, \
             when=2026-11-22 19:15:03, hex64=0x123456789abcdef"
        );
    }

    #[test]
    fn a_sid_is_as_long_as_it_says() {
        let mut event = Build::event("Who");
        event.field("user", IN_SID).field("after", IN_UINT8);
        let mut data = Vec::new();
        data.extend_from_slice(&[1, 2, 0, 0, 0, 0, 0, 5]);
        data.extend_from_slice(&32u32.to_le_bytes());
        data.extend_from_slice(&544u32.to_le_bytes());
        data.push(9);
        assert_eq!(line(&mut event, &data), ", user=S-1-5-32-544, after=9");
    }

    #[test]
    fn values_that_end_early_say_so_and_the_line_stops() {
        let mut event = Build::event("Short");
        event.field("a", IN_UINT32).field("b", IN_UINT32);
        let data = 1u32.to_le_bytes();
        assert_eq!(line(&mut event, &data), ", a=1, b=? (the values end here)");
    }

    #[test]
    fn a_record_cut_to_fit_says_that_and_not_that_the_event_is_wrong() {
        let mut event = Build::event("Short");
        event.field("a", IN_UINT32).field("b", IN_UINT32);
        let data = 1u32.to_le_bytes();
        assert_eq!(decode(&mut event, &data, true), ", a=1, b=? (the rest was cut to fit)");
    }

    #[test]
    fn a_type_the_decoder_does_not_know_stops_the_line_where_it_is() {
        let mut event = Build::event("Odd");
        event.field("known", IN_UINT8).field("odd", 30).field("after", IN_UINT8);
        let data = [1u8, 2, 3];
        assert_eq!(line(&mut event, &data), ", known=1, odd=? (a type this decoder does not know)");
    }

    #[test]
    fn a_custom_type_stops_the_line_because_only_its_own_description_has_its_length() {
        let mut event = Build::event("Custom");
        event.field("known", IN_UINT8);
        event.field("custom", IN_UINT8 | CUSTOM);
        event.bytes.extend_from_slice(&2u16.to_le_bytes());
        event.bytes.extend_from_slice(&[0xaa, 0xbb]);
        let data = [1u8, 2, 3];
        assert_eq!(
            line(&mut event, &data),
            ", known=1, custom=? (a custom type, whose length only its own description gives)"
        );
    }

    #[test]
    fn bytes_no_field_claims_are_counted_in_the_line() {
        let mut event = Build::event("Extra");
        event.field("a", IN_UINT8);
        let data = [1u8, 2, 3];
        assert_eq!(line(&mut event, &data), ", a=1, (+2 byte(s) no field claims)");
    }

    #[test]
    fn blobs_that_are_not_tracelogging_are_refused_rather_than_read() {
        // A manifest provider's event carries no metadata at all, and a
        // blob too short to hold a size is nobody's.
        assert!(Event::parse(&[], &[], &[], false).is_none());
        assert!(Event::parse(&[2, 0], &[0, 0], &[], false).is_none());
        let provider = Build::provider("P");
        // A size past the blob: the record was cut, or it is not this.
        assert!(Event::parse(&provider, &[99, 0, 0, b'x', 0], &[], false).is_none());
    }

    #[test]
    fn a_name_that_is_not_utf8_is_shown_and_not_refused() {
        let mut event = Build::event("Odd");
        event.bytes.extend_from_slice(&[b'a', 0xff, b'b', 0]);
        event.bytes.push(IN_UINT8);
        let data = [7u8];
        assert_eq!(line(&mut event, &data), ", a?b=7");
    }
}
