//! Record/replay at device boundaries (ROADMAP R7, live mocking;
//! docs/ARCHITECTURE.md "Record/replay"): what a run reads from outside
//! (the clock, random numbers, the boot time, the settings it ran with)
//! is written to a recording; a later run reads it back from there
//! instead, so a failure that depends on those inputs repeats.
//!
//! **Format, version 1** (little-endian): the magic `GALAREC\0`, the
//! version (u16), a reserved u16 (0), then records: a kind (u8), a stream
//! (u32: the thread that read the input: for random numbers its number in
//! its process, the same on every run and platform; for gala-um's clock
//! its platform thread id; 0 for none), the payload's length (u32) and
//! the payload. Kinds:
//!
//! | Kind | Payload |
//! |---|---|
//! | 1 `info` | `key=value` (UTF-8): the run's settings (program, args, sim, cpus, clock, ...) |
//! | 2 `boot_time` | u64: the system time at the clock's start (100 ns since 1601) |
//! | 3 `clock` | u64: one reading of the raw monotonic clock (ns) |
//! | 4 `random` | the bytes one request for random numbers got |
//! | 0xff `end` | u64: the number of records before it |
//!
//! A reader accepts versions 1..=VERSION (invariant 3: a reader takes its
//! version and the previous one) and skips kinds it does not know.
//! Replay hands out the records of each (kind, stream) in order,
//! independently of the others: a thread gets the inputs it got when
//! recorded, however the threads interleave this time. When one runs out,
//! or a request differs from the recorded one (another length), the run
//! has diverged: that is logged once and the live source is used from
//! then on.
//!
//! Off (the default), each source costs one relaxed load of `MODE`.
//! Nothing here allocates: records are written straight to the
//! platform's sink (gala-um: a file; the kernel: hex lines on the debug
//! console, `"ev":"rec"`), and a replay reads a buffer in place.

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

pub const MAGIC: [u8; 8] = *b"GALAREC\0";
pub const VERSION: u16 = 1;
/// The file header: magic, version, reserved.
pub const HEADER: usize = 12;
/// A record's head: kind, stream, length.
pub const HEAD: usize = 9;
/// (kind, stream) pairs a replay follows at most.
const STREAMS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Info = 1,
    BootTime = 2,
    Clock = 3,
    Random = 4,
    End = 0xff,
}

const KINDS: usize = 5;
const ALL: [Kind; KINDS] = [Kind::Info, Kind::BootTime, Kind::Clock, Kind::Random, Kind::End];

impl Kind {
    pub const fn name(self) -> &'static str {
        match self {
            Kind::Info => "info",
            Kind::BootTime => "boot_time",
            Kind::Clock => "clock",
            Kind::Random => "random",
            Kind::End => "end",
        }
    }

    /// Index into per-kind tables.
    const fn slot(self) -> usize {
        match self {
            Kind::Info => 0,
            Kind::BootTime => 1,
            Kind::Clock => 2,
            Kind::Random => 3,
            Kind::End => 4,
        }
    }
}

/// The file header.
pub fn header() -> [u8; HEADER] {
    let mut h = [0u8; HEADER];
    h[..8].copy_from_slice(&MAGIC);
    h[8..10].copy_from_slice(&VERSION.to_le_bytes());
    h
}

/// A record's head: kind, stream and payload length.
pub fn record_head(kind: Kind, stream: u32, len: usize) -> [u8; HEAD] {
    let mut h = [0u8; HEAD];
    h[0] = kind as u8;
    h[1..5].copy_from_slice(&stream.to_le_bytes());
    h[5..9].copy_from_slice(&(len as u32).to_le_bytes());
    h
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatError {
    /// Not a recording (no magic).
    Magic,
    /// A version this build does not read.
    Version(u16),
    /// A record at this offset runs past the end.
    Truncated(usize),
}

/// A recording being read: the next record of each (kind, stream).
#[derive(Clone, Debug)]
pub struct Recording<'a> {
    data: &'a [u8],
    pub version: u16,
    /// (kind, stream, where to look for its next record), in first-use
    /// order; `used` of them.
    cursors: [(u8, u32, usize); STREAMS],
    used: usize,
    /// Per kind: records handed out.
    pub taken: [u64; KINDS],
    /// Per kind: records in the recording.
    present: [u64; KINDS],
}

/// One record: kind byte, stream, payload, and where the next starts.
struct Record<'a> {
    kind: u8,
    stream: u32,
    payload: &'a [u8],
    next: usize,
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    let b = data.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

impl<'a> Recording<'a> {
    /// Checks the header and that every record is complete.
    pub fn parse(data: &'a [u8]) -> Result<Self, FormatError> {
        if data.len() < HEADER || data[..8] != MAGIC {
            return Err(FormatError::Magic);
        }
        let version = u16::from_le_bytes([data[8], data[9]]);
        if !(1..=VERSION).contains(&version) {
            return Err(FormatError::Version(version));
        }
        let mut at = HEADER;
        let mut present = [0; KINDS];
        while at < data.len() {
            let len = u32_at(data, at + 5).ok_or(FormatError::Truncated(at))? as usize;
            if let Some(kind) = ALL.iter().find(|k| **k as u8 == data[at]) {
                present[kind.slot()] += 1;
            }
            at = at
                .checked_add(HEAD + len)
                .filter(|&e| e <= data.len())
                .ok_or(FormatError::Truncated(at))?;
        }
        Ok(Recording {
            data,
            version,
            cursors: [(0, 0, 0); STREAMS],
            used: 0,
            taken: [0; KINDS],
            present,
        })
    }

    fn record_at(&self, at: usize) -> Option<Record<'a>> {
        let kind = *self.data.get(at)?;
        let stream = u32_at(self.data, at + 1)?;
        let len = u32_at(self.data, at + 5)? as usize;
        let payload = self.data.get(at + HEAD..at + HEAD + len)?;
        Some(Record { kind, stream, payload, next: at + HEAD + len })
    }

    /// The next record of `kind` read by `stream`, if any is left (None
    /// too when more streams are in use than a replay follows).
    pub fn next(&mut self, kind: Kind, stream: u32) -> Option<&'a [u8]> {
        let k = kind as u8;
        let slot = match self.cursors[..self.used].iter().position(|c| c.0 == k && c.1 == stream) {
            Some(i) => i,
            None if self.used < STREAMS => {
                self.cursors[self.used] = (k, stream, HEADER);
                self.used += 1;
                self.used - 1
            }
            None => return None,
        };
        let mut at = self.cursors[slot].2;
        while let Some(r) = self.record_at(at) {
            at = r.next;
            if r.kind == k && r.stream == stream {
                self.cursors[slot].2 = at;
                self.taken[kind.slot()] += 1;
                return Some(r.payload);
            }
        }
        self.cursors[slot].2 = at;
        None
    }

    /// The value of the `info` record `key=...`.
    pub fn info(&self, key: &str) -> Option<&'a str> {
        let mut at = HEADER;
        while let Some(r) = self.record_at(at) {
            at = r.next;
            if r.kind != Kind::Info as u8 {
                continue;
            }
            let Ok(text) = core::str::from_utf8(r.payload) else { continue };
            if let Some((k, v)) = text.split_once('=')
                && k == key
            {
                return Some(v);
            }
        }
        None
    }

    /// Records of each kind in the whole recording.
    pub fn counts(&self) -> [u64; KINDS] {
        self.present
    }
}

// The run's mode and its sink or recording.

const OFF: u8 = 0;
const RECORD: u8 = 1;
const REPLAY: u8 = 2;

static MODE: AtomicU8 = AtomicU8::new(OFF);
/// Where a recording goes (the platform's writer).
type Sink = fn(&[u8]);
/// Recording: writes bytes to the recording (records are written whole
/// under the lock, so threads' records do not mix).
static SINK: crate::sync::IrqMutex<Option<Sink>> = crate::sync::IrqMutex::new(None);
static RECORDED: [AtomicU64; KINDS] = [const { AtomicU64::new(0) }; KINDS];
static REPLAYING: crate::sync::IrqMutex<Option<Recording<'static>>> =
    crate::sync::IrqMutex::new(None);
static DIVERGED: AtomicBool = AtomicBool::new(false);
/// Inputs that found no record (or a record of another size).
static MISSES: AtomicU64 = AtomicU64::new(0);

/// Whether a recording or a replay is on (one relaxed load).
#[inline]
pub fn on() -> bool {
    MODE.load(Ordering::Relaxed) != OFF
}

pub fn recording() -> bool {
    MODE.load(Ordering::Relaxed) == RECORD
}

pub fn replaying() -> bool {
    MODE.load(Ordering::Relaxed) == REPLAY
}

/// Starts recording into `sink` (the header first).
pub fn record_to(sink: fn(&[u8])) {
    sink(&header());
    *SINK.lock() = Some(sink);
    MODE.store(RECORD, Ordering::Release);
}

/// Starts replaying `data` (checked first).
pub fn replay_from(data: &'static [u8]) -> Result<(), FormatError> {
    let recording = Recording::parse(data)?;
    *REPLAYING.lock() = Some(recording);
    MODE.store(REPLAY, Ordering::Release);
    Ok(())
}

/// The replayed recording's `info` value for `key`.
pub fn replay_info(key: &str) -> Option<&'static str> {
    REPLAYING.lock().as_ref().and_then(|r| r.info(key))
}

/// Appends a record read by `stream` (when recording).
pub fn write(kind: Kind, stream: u32, payload: &[u8]) {
    if !recording() {
        return;
    }
    let sink = SINK.lock();
    if let Some(sink) = *sink {
        sink(&record_head(kind, stream, payload.len()));
        sink(payload);
        RECORDED[kind.slot()].fetch_add(1, Ordering::Relaxed);
    }
}

/// Records a setting (`key=value`; at most 256 bytes).
pub fn info(key: &str, value: &str) {
    if !recording() {
        return;
    }
    let mut buf = [0u8; 256];
    let n = (key.len() + 1 + value.len()).min(buf.len());
    let text = key.bytes().chain(core::iter::once(b'=')).chain(value.bytes());
    for (b, c) in buf.iter_mut().zip(text) {
        *b = c;
    }
    write(Kind::Info, 0, &buf[..n]);
}

/// The replay has left the recording at a `kind` input of `stream`: that
/// input is live (the first such one is logged).
fn diverged(kind: Kind, stream: u32, why: &str) {
    MISSES.fetch_add(1, Ordering::Relaxed);
    if !DIVERGED.swap(true, Ordering::Relaxed) {
        crate::warn!(
            "replay: diverged at a {} input of thread {stream}: {why}; it is live",
            kind.name()
        );
    }
}

/// The next replayed payload of (kind, stream), None (after reporting
/// the divergence) if there is none.
/// A kind the recording has none of (a kernel recording has no clock
/// readings) is live without counting as a divergence.
fn replayed(kind: Kind, stream: u32) -> Option<&'static [u8]> {
    let (next, recorded) = REPLAYING
        .lock()
        .as_mut()
        .map_or((None, false), |r| (r.next(kind, stream), r.present[kind.slot()] > 0));
    if next.is_none() && recorded {
        diverged(kind, stream, "the recording has no more");
    }
    next
}

/// A 64-bit input of `kind` read by `stream`: replayed, or `live()`
/// (recorded when recording).
pub fn u64_input(kind: Kind, stream: u32, live: impl FnOnce() -> u64) -> u64 {
    match MODE.load(Ordering::Relaxed) {
        RECORD => {
            let value = live();
            write(kind, stream, &value.to_le_bytes());
            value
        }
        REPLAY => match replayed(kind, stream) {
            Some(p) if p.len() == 8 => {
                u64::from_le_bytes([p[0], p[1], p[2], p[3], p[4], p[5], p[6], p[7]])
            }
            Some(_) => {
                diverged(kind, stream, "a record of another size");
                live()
            }
            None => live(),
        },
        _ => live(),
    }
}

/// Bytes of `kind` read by `stream` into `buf`: replayed, or filled by
/// `live` (recorded when recording).
pub fn bytes_input(kind: Kind, stream: u32, buf: &mut [u8], live: impl FnOnce(&mut [u8])) {
    match MODE.load(Ordering::Relaxed) {
        RECORD => {
            live(buf);
            write(kind, stream, buf);
        }
        REPLAY => match replayed(kind, stream) {
            Some(p) if p.len() == buf.len() => buf.copy_from_slice(p),
            Some(_) => {
                diverged(kind, stream, "a request of another length");
                live(buf)
            }
            None => live(buf),
        },
        _ => live(buf),
    }
}

/// Ends a recording (the `end` record) or a replay; logs what was
/// recorded or replayed.
pub fn finish() {
    match MODE.swap(OFF, Ordering::AcqRel) {
        RECORD => {
            let total: u64 = RECORDED.iter().map(|c| c.load(Ordering::Relaxed)).sum();
            {
                let sink = SINK.lock();
                if let Some(sink) = *sink {
                    sink(&record_head(Kind::End, 0, 8));
                    sink(&total.to_le_bytes());
                }
            }
            let n = |k: Kind| RECORDED[k.slot()].load(Ordering::Relaxed);
            crate::info!(
                "replay: recorded {total} records ({} clock, {} random, {} boot time, {} info)",
                n(Kind::Clock),
                n(Kind::Random),
                n(Kind::BootTime),
                n(Kind::Info)
            );
        }
        REPLAY => {
            let r = REPLAYING.lock();
            if let Some(r) = r.as_ref() {
                let all = r.counts();
                let s = |k: Kind| (r.taken[k.slot()], all[k.slot()]);
                let ((c, ct), (rn, rt), (b, bt)) =
                    (s(Kind::Clock), s(Kind::Random), s(Kind::BootTime));
                crate::info!(
                    "replay: replayed {c} of {ct} clock, {rn} of {rt} random, {b} of {bt} boot \
                     time records; {} inputs live",
                    MISSES.load(Ordering::Relaxed)
                );
            }
        }
        _ => {}
    }
}

/// Whether the replay left the recording.
pub fn has_diverged() -> bool {
    DIVERGED.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn recording(records: &[(Kind, u32, &[u8])]) -> Vec<u8> {
        let mut v = header().to_vec();
        for (k, stream, p) in records {
            v.extend_from_slice(&record_head(*k, *stream, p.len()));
            v.extend_from_slice(p);
        }
        v
    }

    #[test]
    fn each_thread_gets_its_own_inputs_in_order() {
        let data = recording(&[
            (Kind::Info, 0, b"sim=7"),
            (Kind::Clock, 1, &1u64.to_le_bytes()),
            (Kind::Random, 1, &[1, 2, 3]),
            (Kind::Clock, 2, &5u64.to_le_bytes()),
            (Kind::Clock, 1, &2u64.to_le_bytes()),
            (Kind::Random, 1, &[4]),
        ]);
        let mut r = Recording::parse(&data).unwrap();
        assert_eq!(r.info("sim"), Some("7"));
        assert_eq!(r.info("cpus"), None);
        assert_eq!(r.next(Kind::Random, 1), Some(&[1u8, 2, 3][..]));
        // Thread 2 reads first this time: it still gets its own reading.
        assert_eq!(r.next(Kind::Clock, 2), Some(&5u64.to_le_bytes()[..]));
        assert_eq!(r.next(Kind::Clock, 1), Some(&1u64.to_le_bytes()[..]));
        assert_eq!(r.next(Kind::Clock, 1), Some(&2u64.to_le_bytes()[..]));
        assert_eq!(r.next(Kind::Clock, 1), None);
        assert_eq!(r.next(Kind::Clock, 2), None);
        assert_eq!(r.next(Kind::Random, 1), Some(&[4u8][..]));
        assert_eq!(r.next(Kind::Random, 1), None);
        assert_eq!(r.next(Kind::Random, 3), None);
        assert_eq!(r.taken[Kind::Clock.slot()], 3);
        assert_eq!(r.counts()[Kind::Random.slot()], 2);
    }

    #[test]
    fn bad_recordings_are_refused_and_unknown_kinds_skipped() {
        assert_eq!(Recording::parse(b"GALAREC").unwrap_err(), FormatError::Magic);
        let mut data = recording(&[]);
        data[8] = 9;
        assert_eq!(Recording::parse(&data).unwrap_err(), FormatError::Version(9));
        let mut data = recording(&[(Kind::Random, 0, &[1, 2, 3])]);
        data.pop();
        assert_eq!(Recording::parse(&data).unwrap_err(), FormatError::Truncated(HEADER));
        let mut data = recording(&[(Kind::Random, 0, &[9])]);
        data.extend_from_slice(&[0x42, 0, 0, 0, 0, 1, 0, 0, 0, 7]);
        let mut r = Recording::parse(&data).unwrap();
        assert_eq!(r.next(Kind::Random, 0), Some(&[9u8][..]));
        assert_eq!(r.next(Kind::Clock, 0), None);
    }
}
