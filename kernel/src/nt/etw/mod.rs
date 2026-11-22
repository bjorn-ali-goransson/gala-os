//! `etw=<provider or name>[:<level>[:<keyword>[:<n>]]]`: the trace
//! session gala starts on a program's behalf, so that a run can read a
//! program's own telemetry without the program asking for it
//! (docs/OBSERVABILITY.md, "Program telemetry").
//!
//! TraceLogging builds no payload for a provider no session enabled, so
//! a Word startup registers twenty providers and writes not one event.
//! advapi32 gives a program the private session it can start for itself
//! (`user/advapi32/src/trace.rs`), but nothing starts one for a program
//! that does not ask -- and a program being investigated is exactly the
//! one that does not. This is that session: the run names a provider,
//! the kernel hands its demands to advapi32 at the first registration,
//! and the events that come back are lines of the serial log.
//!
//! A provider is named by its GUID or, more usefully, by its
//! TraceLogging *name*: the GUID is a hash of the name ([`name_guid`]),
//! so a run can ask for `Microsoft.Windows.Something` without a
//! manifest of any kind, and `*` asks for every provider the program
//! registers.
//!
//! A line says what the program said: the provider's name, the event's,
//! and every field with the name the program gave it, read out of the
//! metadata TraceLogging ships beside each event (`tracelogging`). An
//! event whose metadata is not TraceLogging's -- a manifest provider's
//! -- has no field names anywhere in it, and its line is the account
//! that the program wrote at all, with the payload's size.

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::sync::IrqMutex;

pub mod sha1;
pub mod tracelogging;

/// The longest provider name an `etw=` value may name. TraceLogging's
/// own names run to half of this; a longer one is refused outright
/// rather than cut, because a cut name hashes to a GUID no provider
/// has.
pub const NAME_LEN: usize = 128;

/// How many events one run reports unless `:<n>` asks for another
/// number. Each is a line of the serial log, so this is the same shape
/// of budget `trace_from=` has: enough to cover a startup, far short of
/// what a program that runs on writes.
const BUDGET: u32 = 256;

/// The most events `:<n>` may ask for. Past this a run is reading
/// noise, and a snapshot is the better tool.
const MAX_BUDGET: u32 = 1 << 20;

/// The most providers one run's summary counts apart. A Word startup
/// registers twenty.
const SEEN_MAX: usize = 32;

/// The bytes op 0 of `GalaEtw` answers with: the provider id (all zeros
/// when every provider is asked for), a flags byte, the level, six
/// bytes of nothing, and the keyword as a little-endian `u64`. A fixed
/// layout and not a shared type: the DLL that reads it is another crate
/// (`user/advapi32/src/trace.rs`, `arm`), and the offsets are the
/// contract.
pub const ASK_LEN: usize = 32;

/// Bit 0 of the flags byte in an ask: every provider the program
/// registers, whatever its id.
pub const ASK_EVERY: u8 = 1;

/// The header op 1 of `GalaEtw` hands over, before the three blobs
/// whose lengths it gives: the provider id (16 bytes), the event
/// descriptor as the program built it (16), the whole payload's length
/// as a little-endian `u32`, then the provider's metadata, the event's
/// metadata and the values as `u16` lengths, and a `u16` of flags. A
/// fixed layout and not a shared type, for the reason `ASK_LEN` gives.
pub const RECORD_HEAD: usize = 44;

/// The most one record holds, header and blobs. An event past this is
/// cut to fit, and its line says so rather than reading as an event that
/// ended: the whole of it is a stack frame's in both halves, so no
/// allocation and no lock stands between a program's write and its line.
/// A Word event runs to a few hundred bytes.
pub const RECORD_MAX: usize = 2048;

/// Bit 0 of the record's flags: the values did not fit and were cut.
pub const RECORD_CUT: u16 = 1;

/// Bit 1: the event carried no TraceLogging metadata at all, which is
/// what a manifest provider's event looks like.
pub const RECORD_NO_META: u16 = 2;

/// What one `etw=` asks for.
struct Ask {
    /// `*`: every provider the program registers.
    every: bool,
    guid: [u8; 16],
    level: u8,
    /// The keywords asked for; 0 means every keyword, which is what the
    /// provider is then told (`EnableTraceEx2`, measured).
    keyword: u64,
    /// The name the value gave, when it gave one rather than a GUID.
    /// Kept for the line that says what the session is for: a GUID
    /// alone tells a reader nothing.
    name: [u8; NAME_LEN],
    name_len: usize,
}

impl Ask {
    const fn new() -> Self {
        Self { every: false, guid: [0; 16], level: 0, keyword: 0, name: [0; NAME_LEN], name_len: 0 }
    }
}

/// What the run asked for.
static ASK: IrqMutex<Ask> = IrqMutex::new(Ask::new());

/// Whether `etw=` was given at all, so a registration costs one relaxed
/// load where it was not.
static WANTED: AtomicBool = AtomicBool::new(false);

/// Events left in the budget.
static LEFT: AtomicU32 = AtomicU32::new(0);

/// Events written to the session.
static WRITTEN: AtomicU64 = AtomicU64::new(0);

/// The providers those events came from.
static SEEN: IrqMutex<Seen> = IrqMutex::new(Seen::new());

/// The distinct providers that wrote. Which providers a run reached is
/// something the summary has to say: `etw=*` arms every one a program
/// registers, and a reader cannot otherwise tell a program with one
/// provider from a run that reached one of twenty.
struct Seen {
    guids: [[u8; 16]; SEEN_MAX],
    count: usize,
    /// Whether a provider was left out for want of room.
    more: bool,
}

impl Seen {
    const fn new() -> Self {
        Seen { guids: [[0; 16]; SEEN_MAX], count: 0, more: false }
    }

    /// Notes a provider, and says whether this is the first event it
    /// wrote: the name it gives itself is said once, where a reader of
    /// the lines after it needs it.
    fn note(&mut self, guid: &[u8; 16]) -> bool {
        for i in 0..self.count {
            if self.guids[i] == *guid {
                return false;
            }
        }
        if self.count == SEEN_MAX {
            self.more = true;
            return false;
        }
        self.guids[self.count] = *guid;
        self.count += 1;
        true
    }
}

/// A provider id as Windows spells it: the first three fields are
/// little-endian in memory, the last two are bytes in order.
pub struct Guid<'a>(pub &'a [u8; 16]);

impl fmt::Display for Guid<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = self.0;
        let d1 = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let d2 = u16::from_le_bytes([b[4], b[5]]);
        let d3 = u16::from_le_bytes([b[6], b[7]]);
        write!(f, "{d1:08x}-{d2:04x}-{d3:04x}-{:02x}{:02x}-", b[8], b[9])?;
        for byte in &b[10..] {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// The keywords a session asks for, as a sentence names them: 0 is
/// every keyword and not "keyword 0x0", which would read as a session
/// that asks for nothing.
struct Keyword(u64);

impl fmt::Display for Keyword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 == 0 { f.write_str("every keyword") } else { write!(f, "keyword {:#x}", self.0) }
    }
}

/// The GUID a TraceLogging provider name hashes into, as
/// `TraceLoggingProvider.h` computes it: SHA-1 over a fixed sixteen-byte
/// signature followed by the name uppercased in big-endian UTF-16, cut
/// to sixteen bytes, with the version nibble set to 5, read as a
/// little-endian GUID. The name is the only handle a run has on a
/// provider whose id is in nobody's manifest, which is why `etw=` takes
/// one.
///
/// A name past ASCII is hashed byte for byte as it stands: every
/// provider name Windows ships is ASCII, and a name that is not would
/// need the whole of Unicode's uppercasing to hash the way Windows
/// hashes it.
pub fn name_guid(name: &[u8]) -> [u8; 16] {
    /// The signature the hash starts with (`TraceLoggingProvider.h`).
    const SIGNATURE: [u8; 16] = [
        0x48, 0x2c, 0x2d, 0xb2, 0xc3, 0x90, 0x47, 0xc8, 0x87, 0xf8, 0x1a, 0x15, 0xbf, 0xc1, 0x30,
        0xfb,
    ];
    let mut data = [0u8; SIGNATURE.len() + NAME_LEN * 2];
    data[..SIGNATURE.len()].copy_from_slice(&SIGNATURE);
    let mut n = SIGNATURE.len();
    for &byte in name.iter().take(NAME_LEN) {
        data[n] = 0;
        data[n + 1] = byte.to_ascii_uppercase();
        n += 2;
    }
    let hash = sha1::digest(&data[..n]);
    let mut guid = [0u8; 16];
    guid.copy_from_slice(&hash[..16]);
    guid[7] = (guid[7] & 0x0f) | 0x50;
    guid
}

/// A GUID as a value may spell it: 8-4-4-4-12 hex digits, in braces or
/// not. `None` when the text is not one, which is how a value naming a
/// provider is told from a value naming a GUID.
fn parse_guid(text: &str) -> Option<[u8; 16]> {
    let text = match text.strip_prefix('{') {
        Some(inner) => inner.strip_suffix('}')?,
        None => text,
    };
    let b = text.as_bytes();
    if b.len() != 36 || b[8] != b'-' || b[13] != b'-' || b[18] != b'-' || b[23] != b'-' {
        return None;
    }
    let mut nibbles = [0u8; 32];
    let mut n = 0;
    for &c in b {
        if c == b'-' {
            continue;
        }
        let digit = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => return None,
        };
        if n == nibbles.len() {
            return None;
        }
        nibbles[n] = digit;
        n += 1;
    }
    if n != nibbles.len() {
        return None;
    }
    let mut raw = [0u8; 16];
    for (byte, pair) in raw.iter_mut().zip(nibbles.chunks_exact(2)) {
        *byte = (pair[0] << 4) | pair[1];
    }
    // The first three fields are little-endian in memory; the rest are
    // bytes in the order they are written.
    let mut guid = raw;
    guid[0..4].copy_from_slice(&[raw[3], raw[2], raw[1], raw[0]]);
    guid[4..6].copy_from_slice(&[raw[5], raw[4]]);
    guid[6..8].copy_from_slice(&[raw[7], raw[6]]);
    Some(guid)
}

/// A keyword as a value spells it: hex with `0x`, else decimal.
fn parse_keyword(text: &str) -> Option<u64> {
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => text.parse::<u64>().ok(),
    }
}

/// `etw=<provider or name>[:<level>[:<keyword>[:<n>]]]`. A value that is
/// not one is refused outright rather than cut to the defaults: a
/// session that quietly asked for less than the run wanted reads
/// afterwards as a program that wrote nothing, which is the wrong
/// conclusion.
pub fn set(value: &str) {
    let mut parts = value.split(':');
    let who = parts.next().unwrap_or("");
    let level_text = parts.next();
    let keyword_text = parts.next();
    let budget_text = parts.next();
    if parts.next().is_some() {
        warn!(
            "etw: etw={value} has more fields than \
             <provider or name>[:<level>[:<keyword>[:<n>]]]: no session is started"
        );
        return;
    }
    if who.is_empty() {
        warn!(
            "etw: etw={value} names no provider: it reads \
             <provider or name>[:<level>[:<keyword>[:<n>]]], such as \
             Microsoft.Office.Word or *; no session is started"
        );
        return;
    }
    if who.len() > NAME_LEN {
        warn!(
            "etw: etw={value} names a provider longer than {NAME_LEN} bytes: \
             no session is started"
        );
        return;
    }
    // Level 1 is critical and 5 verbose; 255 is every level there is. A
    // session asking for 0 would admit only the events whose own level
    // is 0, which is no use to anyone and is not what a run means by it.
    let level = match level_text {
        None => 5,
        Some(text) => match text.parse::<u8>() {
            Ok(level) if level > 0 => level,
            _ => {
                warn!(
                    "etw: etw={value} asks for a level that is not a number from 1 (critical) \
                     to 5 (verbose), or 255 for every level: no session is started"
                );
                return;
            }
        },
    };
    let keyword = match keyword_text {
        None => 0,
        Some(text) => match parse_keyword(text) {
            Some(keyword) => keyword,
            None => {
                warn!(
                    "etw: etw={value} asks for a keyword that is neither 0x<hex> nor a \
                     decimal number: no session is started"
                );
                return;
            }
        },
    };
    let budget = match budget_text {
        None => BUDGET,
        Some(text) => match text.parse::<u32>() {
            Ok(n) if (1..=MAX_BUDGET).contains(&n) => n,
            _ => {
                warn!(
                    "etw: etw={value} ends in an event count that is not a number from 1 \
                     to {MAX_BUDGET}: no session is started"
                );
                return;
            }
        },
    };

    let every = who == "*";
    let by_guid = parse_guid(who);
    let guid = match (every, by_guid) {
        (true, _) => [0; 16],
        (false, Some(guid)) => guid,
        (false, None) => name_guid(who.as_bytes()),
    };
    {
        let mut ask = ASK.lock();
        ask.every = every;
        ask.guid = guid;
        ask.level = level;
        ask.keyword = keyword;
        ask.name_len = if every || by_guid.is_some() { 0 } else { who.len() };
        let len = ask.name_len;
        ask.name[..len].copy_from_slice(&who.as_bytes()[..len]);
    }
    LEFT.store(budget, Ordering::Relaxed);
    WANTED.store(true, Ordering::Release);
    let keyword = Keyword(keyword);
    if every {
        info!(
            "etw: a session for every provider, level {level} and {keyword}, \
             {budget} events at most (etw=)"
        );
    } else if by_guid.is_some() {
        info!(
            "etw: a session for {}, level {level} and {keyword}, {budget} events at most (etw=)",
            Guid(&guid)
        );
    } else {
        info!(
            "etw: a session for {who} ({}), level {level} and {keyword}, \
             {budget} events at most (etw=)",
            Guid(&guid)
        );
    }
}

/// Whether `etw=` was given (one load).
pub fn wanted() -> bool {
    WANTED.load(Ordering::Acquire)
}

/// What the run asks for, in the bytes op 0 answers with ([`ASK_LEN`]).
/// False when no session was asked for, which is what a DLL pays one
/// system call in its life to learn.
pub fn ask(out: &mut [u8; ASK_LEN]) -> bool {
    if !wanted() {
        return false;
    }
    let ask = ASK.lock();
    out[..16].copy_from_slice(&ask.guid);
    out[16] = if ask.every { ASK_EVERY } else { 0 };
    out[17] = ask.level;
    out[18..24].fill(0);
    out[24..32].copy_from_slice(&ask.keyword.to_le_bytes());
    true
}

/// Takes one event from the budget: what is left after it, or `None`
/// once the budget is spent. Taken before the line is written, so two
/// CPUs writing at once cannot both spend the last one.
fn take_line() -> Option<u32> {
    let mut left = LEFT.load(Ordering::Relaxed);
    loop {
        if left == 0 {
            return None;
        }
        match LEFT.compare_exchange_weak(left, left - 1, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Some(left - 1),
            Err(now) => left = now,
        }
    }
}

/// One event a session was given, as op 1's record carries it.
pub struct Written<'a> {
    /// The program that wrote it.
    pub process: &'a str,
    pub guid: &'a [u8; 16],
    /// The level and keyword of the event's own descriptor, which are
    /// the event's and not the session's.
    pub level: u8,
    pub keyword: u64,
    /// Every byte the program built, before anything was cut: what a
    /// line can say about an event it cannot read.
    pub bytes: u32,
    pub provider_meta: &'a [u8],
    pub event_meta: &'a [u8],
    pub data: &'a [u8],
    /// `RECORD_CUT`, `RECORD_NO_META`.
    pub flags: u16,
}

/// Records one event the session was given, and says whether the DLL
/// should keep handing them over: `false` once the budget is spent, so
/// a program whose session is over stops paying a system call an event.
pub fn record(event: &Written<'_>) -> bool {
    if !wanted() {
        return false;
    }
    let Some(left) = take_line() else { return false };
    let first = SEEN.lock().note(event.guid);
    WRITTEN.fetch_add(1, Ordering::Relaxed);
    let cut = event.flags & RECORD_CUT != 0;
    match tracelogging::Event::parse(event.provider_meta, event.event_meta, event.data, cut) {
        Some(decoded) => {
            if first {
                // The name a provider gives itself, said at its first
                // event: a run that asked for `*` has nothing else to
                // read the ids in the lines after this one by.
                info!(
                    "etw: provider {} is {}",
                    tracelogging::Text(decoded.provider),
                    Guid(event.guid)
                );
            }
            info!(
                "etw: {} {}/{} level {} keyword {:#x}{}",
                event.process,
                tracelogging::Text(decoded.provider),
                tracelogging::Text(decoded.name),
                event.level,
                event.keyword,
                decoded.fields()
            );
        }
        None => {
            // Nothing says what the values are: the event is a manifest
            // provider's, whose names live in a manifest outside the
            // program, or its own description does not read. Either way
            // the size is what a line can honestly say, and which of
            // the two it is decides whether anyone should look.
            let why = if event.flags & RECORD_NO_META != 0 {
                "no field metadata: a manifest provider's event"
            } else {
                "its field metadata does not read"
            };
            info!(
                "etw: {} {} level {} keyword {:#x}, {} bytes ({why})",
                event.process,
                Guid(event.guid),
                event.level,
                event.keyword,
                event.bytes
            );
        }
    }
    left > 0
}

/// What the session came to, at the end of the run.
pub fn log_stats() {
    if !wanted() {
        return;
    }
    let written = WRITTEN.load(Ordering::Relaxed);
    if written == 0 {
        // Said rather than left out: a run that armed a session and saw
        // nothing has learnt something, and the two reasons it can have
        // are what a reader looks for next.
        info!(
            "etw: not one event was written to the session (etw=): either the program \
             registered no provider the session asked for, or it wrote none in reach of \
             the level and keyword asked for"
        );
        return;
    }
    let (providers, more) = {
        let seen = SEEN.lock();
        (seen.count, seen.more)
    };
    let events = if written == 1 { "event" } else { "events" };
    let names = if providers == 1 { "provider" } else { "providers" };
    let or_more = if more { " or more" } else { "" };
    if LEFT.load(Ordering::Relaxed) == 0 {
        // The DLL is told to stop as the budget runs out, so what
        // follows is counted nowhere: say that the session ends before
        // the program did, rather than let a run read as a program that
        // stopped writing.
        info!(
            "etw: {written} {events} written to the session, from {providers} {names}{or_more} \
             (etw=); the budget is spent, so the session ends here, not the program"
        );
    } else {
        info!(
            "etw: {written} {events} written to the session, \
             from {providers} {names}{or_more} (etw=)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{Guid, Keyword, name_guid, parse_guid, parse_keyword};

    fn text(guid: [u8; 16]) -> std::string::String {
        std::format!("{}", Guid(&guid))
    }

    #[test]
    fn a_name_hashes_to_the_guid_windows_hashes_it_to() {
        // The example `TraceLoggingProvider.h` documents for its own
        // hash: "MyProvider" is this GUID, so the signature, the
        // uppercasing, the UTF-16 and the version nibble are all as
        // Windows has them.
        assert_eq!(text(name_guid(b"MyProvider")), "b3864c38-4273-58c5-545b-8b3608343471");
        // The name is uppercased before it is hashed, so a provider is
        // the same provider however a run spells it.
        assert_eq!(name_guid(b"myprovider"), name_guid(b"MyProvider"));
        assert_eq!(name_guid(b"MYPROVIDER"), name_guid(b"MyProvider"));
        // The version nibble is 5 whatever the name.
        for name in [&b"a"[..], b"Microsoft.Windows.Kernel.Process", b""] {
            assert_eq!(name_guid(name)[7] & 0xf0, 0x50, "{name:?}");
        }
    }

    #[test]
    fn a_guid_reads_back_as_it_was_written() {
        let spelling = "2542af41-3d78-596e-ff31-017dbbeacc32";
        let guid = parse_guid(spelling).expect("a GUID");
        assert_eq!(text(guid), spelling);
        // The first three fields are little-endian in memory, so the
        // bytes are not the digits in the order they are written.
        assert_eq!(guid[0], 0x41);
        assert_eq!(guid[15], 0x32);
        // Braces and either case, as a value may spell it.
        assert_eq!(parse_guid("{2542AF41-3D78-596E-FF31-017DBBEACC32}"), Some(guid));
        // A name is not a GUID, which is how the two are told apart.
        assert_eq!(parse_guid("Gala.Test.Arm"), None);
        assert_eq!(parse_guid("2542af41-3d78-596e-ff31-017dbbeacc3"), None);
        assert_eq!(parse_guid("2542af41-3d78-596e-ff31-017dbbeacc3z"), None);
        assert_eq!(parse_guid("{2542af41-3d78-596e-ff31-017dbbeacc32"), None);
        // The name `etw=` is given is the one it hashes, and this is
        // the provider the test program registers.
        assert_eq!(name_guid(b"Gala.Test.Arm"), guid);
    }

    #[test]
    fn a_keyword_reads_as_hex_or_decimal() {
        assert_eq!(parse_keyword("0x10"), Some(0x10));
        assert_eq!(parse_keyword("0X4000000000000000"), Some(0x4000_0000_0000_0000));
        assert_eq!(parse_keyword("16"), Some(16));
        assert_eq!(parse_keyword("0xffffffffffffffff"), Some(u64::MAX));
        assert_eq!(parse_keyword("0x"), None);
        assert_eq!(parse_keyword("ten"), None);
        // 0 is every keyword, and a sentence says so rather than
        // printing a mask nobody asked for.
        assert_eq!(std::format!("{}", Keyword(0)), "every keyword");
        assert_eq!(std::format!("{}", Keyword(0x10)), "keyword 0x10");
    }
}
