//! Real-time performance queries (pdh.h), as documented: PdhOpenQueryW,
//! PdhAddEnglishCounterW / PdhAddCounterW, PdhCollectQueryData,
//! PdhGetFormattedCounterValue, PdhRemoveCounter and PdhCloseQuery.
//! Chromium's CPU probe reads `\Processor(_Total)\% Processor Time`.
//!
//! The counters gala has are the processors' together (instance `_Total`
//! of `Processor` and `Processor Information`): `% Processor Time` and
//! `% Idle Time`, rates over the processors' times between the last two
//! collections (kernel32's GetSystemTimes). Other objects and counters are
//! recorded and fail as Windows does for ones a machine lacks. Handles are
//! slot numbers with a generation, so a closed one is PDH_INVALID_HANDLE.

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::sync::atomic::{AtomicBool, Ordering};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetSystemTimes(idle: *mut u64, kernel: *mut u64, user: *mut u64) -> i32;
}

const ERROR_SUCCESS: u32 = 0;
const PDH_CSTATUS_VALID_DATA: u32 = 0;
const PDH_CSTATUS_NO_INSTANCE: u32 = 0x8000_07d1;
const PDH_NO_DATA: u32 = 0x8000_07d5;
const PDH_CSTATUS_NO_OBJECT: u32 = 0xc000_0bb8;
const PDH_CSTATUS_NO_COUNTER: u32 = 0xc000_0bb9;
const PDH_CSTATUS_INVALID_DATA: u32 = 0xc000_0bba;
const PDH_MEMORY_ALLOCATION_FAILURE: u32 = 0xc000_0bbb;
const PDH_INVALID_HANDLE: u32 = 0xc000_0bbc;
const PDH_INVALID_ARGUMENT: u32 = 0xc000_0bbd;
const PDH_CSTATUS_BAD_COUNTERNAME: u32 = 0xc000_0bc0;
const PDH_INVALID_DATA: u32 = 0xc000_0bc6;

const PDH_FMT_LONG: u32 = 0x100;
const PDH_FMT_DOUBLE: u32 = 0x200;
const PDH_FMT_LARGE: u32 = 0x400;
const PDH_FMT_NOCAP100: u32 = 0x8000;

/// The counter types (winperf.h) of the two counters.
const PERF_100NSEC_TIMER: u32 = 0x2051_0500;
const PERF_100NSEC_TIMER_INV: u32 = 0x2057_0500;

const QUERIES: usize = 32;
const COUNTERS: usize = 128;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// % Processor Time: the share of time not idle.
    Busy,
    /// % Idle Time.
    Idle,
}

/// A processor-times sample: (idle, kernel + user), in 100 ns.
type Sample = (u64, u64);

#[derive(Clone, Copy)]
struct Counter {
    /// Its query's slot + 1 (0: a free slot).
    query: usize,
    generation: u16,
    kind: Kind,
    /// The last two collections, the latest last.
    samples: [Option<Sample>; 2],
}

#[derive(Clone, Copy)]
struct Query {
    open: bool,
    generation: u16,
}

struct State {
    queries: [Query; QUERIES],
    counters: [Counter; COUNTERS],
}

static LOCK: AtomicBool = AtomicBool::new(false);
static mut STATE: State = State {
    queries: [Query { open: false, generation: 0 }; QUERIES],
    counters: [Counter { query: 0, generation: 0, kind: Kind::Busy, samples: [None; 2] }; COUNTERS],
};

fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
    while LOCK.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    let state = &raw mut STATE;
    // SAFETY: the state is only touched under LOCK.
    let r = f(unsafe { &mut *state });
    LOCK.store(false, Ordering::Release);
    r
}

const QUERY_TAG: usize = 0x1;
const COUNTER_TAG: usize = 0x2;

fn handle(slot: usize, generation: u16, tag: usize) -> usize {
    (usize::from(generation) << 20) | (slot << 4) | tag
}

/// The slot `h` names if it is a live handle of kind `tag`.
fn slot_of(state: &State, h: usize, tag: usize) -> Option<usize> {
    if h & 0xf != tag {
        return None;
    }
    let (slot, generation) = ((h >> 4) & 0xffff, (h >> 20) as u16);
    if tag == QUERY_TAG {
        let q = state.queries.get(slot)?;
        (q.open && q.generation == generation).then_some(slot)
    } else {
        let c = state.counters.get(slot)?;
        (c.query != 0 && c.generation == generation).then_some(slot)
    }
}

/// PdhOpenQueryW(szDataSource, dwUserData, phQuery): a real-time query
/// (a log file as the data source is recorded and refused).
#[unsafe(no_mangle)]
pub extern "system" fn PdhOpenQueryW(source: *const u16, _user: usize, out: *mut usize) -> u32 {
    if out.is_null() {
        return PDH_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's string, if any.
    if !source.is_null() && unsafe { *source } != 0 {
        crate::unsupported(b"pdh.dll!PdhOpenQueryW(log file)", 0);
        return PDH_INVALID_ARGUMENT;
    }
    let made = with(|s| {
        let slot = s.queries.iter().position(|q| !q.open)?;
        let q = &mut s.queries[slot];
        q.open = true;
        q.generation = q.generation.wrapping_add(1).max(1);
        Some(handle(slot, q.generation, QUERY_TAG))
    });
    let Some(h) = made else {
        crate::unsupported(b"pdh.dll!PdhOpenQueryW(too many queries)", QUERIES as u64);
        return PDH_MEMORY_ALLOCATION_FAILURE;
    };
    // SAFETY: the caller's output pointer.
    unsafe { *out = h };
    ERROR_SUCCESS
}

/// ASCII case-insensitive equality of UTF-16 text and `name`.
fn is(text: &[u16], name: &str) -> bool {
    let fold = |c: u16| if (65..=90).contains(&c) { c + 32 } else { c };
    text.len() == name.len()
        && text.iter().zip(name.bytes()).all(|(&a, b)| fold(a) == fold(b.into()))
}

/// The counter a path names: `[\\machine]\object[(instance)]\counter`.
fn parse(path: &[u16]) -> Result<Kind, u32> {
    let bs = u16::from(b'\\');
    let mut rest = path;
    // A machine name: only this one.
    if rest.starts_with(&[bs, bs]) {
        let end = rest[2..].iter().position(|&c| c == bs).ok_or(PDH_CSTATUS_BAD_COUNTERNAME)?;
        rest = &rest[2 + end..];
    }
    let rest = rest.strip_prefix(&[bs]).ok_or(PDH_CSTATUS_BAD_COUNTERNAME)?;
    let split = rest.iter().rposition(|&c| c == bs).ok_or(PDH_CSTATUS_BAD_COUNTERNAME)?;
    let (object, counter) = (&rest[..split], &rest[split + 1..]);
    let (object, instance) = match object.iter().position(|&c| c == u16::from(b'(')) {
        Some(open) if object.last() == Some(&u16::from(b')')) => {
            (&object[..open], Some(&object[open + 1..object.len() - 1]))
        }
        Some(_) => return Err(PDH_CSTATUS_BAD_COUNTERNAME),
        None => (object, None),
    };
    if !is(object, "Processor") && !is(object, "Processor Information") {
        return Err(PDH_CSTATUS_NO_OBJECT);
    }
    let kind = if is(counter, "% Processor Time") {
        Kind::Busy
    } else if is(counter, "% Idle Time") {
        Kind::Idle
    } else {
        return Err(PDH_CSTATUS_NO_COUNTER);
    };
    match instance {
        Some(i) if is(i, "_Total") => Ok(kind),
        _ => Err(PDH_CSTATUS_NO_INSTANCE),
    }
}

/// PdhAddEnglishCounterW(hQuery, szFullCounterPath, dwUserData,
/// phCounter): a counter by its English path.
#[unsafe(no_mangle)]
pub extern "system" fn PdhAddEnglishCounterW(
    query: usize,
    path: *const u16,
    _user: usize,
    out: *mut usize,
) -> u32 {
    if path.is_null() || out.is_null() {
        return PDH_INVALID_ARGUMENT;
    }
    let mut len = 0;
    // SAFETY: the caller's NUL-terminated path (bounded by PDH's limit).
    while len < 1024 && unsafe { *path.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` characters were just read.
    let text = unsafe { core::slice::from_raw_parts(path, len) };
    let kind = match parse(text) {
        Ok(kind) => kind,
        Err(e) => {
            if with(|s| slot_of(s, query, QUERY_TAG)).is_none() {
                return PDH_INVALID_HANDLE;
            }
            let mut name = [0u8; 64];
            let n = name.len().min(len);
            for (b, &c) in name.iter_mut().zip(text) {
                *b = u8::try_from(c).unwrap_or(b'?');
            }
            crate::unsupported_named(b"pdh.dll!PdhAddEnglishCounterW(", &name[..n], e.into());
            return e;
        }
    };
    let made = with(|s| {
        let q = slot_of(s, query, QUERY_TAG).ok_or(PDH_INVALID_HANDLE)?;
        let slot =
            s.counters.iter().position(|c| c.query == 0).ok_or(PDH_MEMORY_ALLOCATION_FAILURE)?;
        let c = &mut s.counters[slot];
        *c = Counter {
            query: q + 1,
            generation: c.generation.wrapping_add(1).max(1),
            kind,
            samples: [None; 2],
        };
        Ok(handle(slot, c.generation, COUNTER_TAG))
    });
    match made {
        Ok(h) => {
            // SAFETY: the caller's output pointer.
            unsafe { *out = h };
            ERROR_SUCCESS
        }
        Err(e) => e,
    }
}

/// PdhAddCounterW: the same, with the system's (English) counter names.
#[unsafe(no_mangle)]
pub extern "system" fn PdhAddCounterW(
    query: usize,
    path: *const u16,
    user: usize,
    out: *mut usize,
) -> u32 {
    PdhAddEnglishCounterW(query, path, user, out)
}

/// PdhCollectQueryData(hQuery): a sample for each of the query's
/// counters; PDH_NO_DATA when it has none.
#[unsafe(no_mangle)]
pub extern "system" fn PdhCollectQueryData(query: usize) -> u32 {
    let (mut idle, mut kernel, mut user) = (0u64, 0u64, 0u64);
    // SAFETY: our outputs.
    let sample = (unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } != 0)
        .then_some((idle, kernel.saturating_add(user)));
    with(|s| {
        let Some(q) = slot_of(s, query, QUERY_TAG) else { return PDH_INVALID_HANDLE };
        let mut any = false;
        for c in s.counters.iter_mut().filter(|c| c.query == q + 1) {
            c.samples = [c.samples[1], sample];
            any = true;
        }
        if any { ERROR_SUCCESS } else { PDH_NO_DATA }
    })
}

/// PDH_FMT_COUNTERVALUE.
#[repr(C)]
pub struct CounterValue {
    status: u32,
    value: u64,
}

/// PdhGetFormattedCounterValue(hCounter, dwFormat, lpdwType, pValue): the
/// counter's rate between its last two samples as a long, double or
/// large (capped at 100 unless PDH_FMT_NOCAP100); PDH_INVALID_DATA
/// (CStatus PDH_CSTATUS_INVALID_DATA) before two samples.
#[unsafe(no_mangle)]
pub extern "system" fn PdhGetFormattedCounterValue(
    counter: usize,
    format: u32,
    kind_out: *mut u32,
    value: *mut CounterValue,
) -> u32 {
    let found = with(|s| slot_of(s, counter, COUNTER_TAG).map(|i| s.counters[i]));
    let Some(c) = found else { return PDH_INVALID_HANDLE };
    if value.is_null() || format & (PDH_FMT_LONG | PDH_FMT_DOUBLE | PDH_FMT_LARGE) == 0 {
        return PDH_INVALID_ARGUMENT;
    }
    if !kind_out.is_null() {
        let kind = if c.kind == Kind::Busy { PERF_100NSEC_TIMER_INV } else { PERF_100NSEC_TIMER };
        // SAFETY: the caller's output pointer.
        unsafe { *kind_out = kind };
    }
    let (Some(old), Some(new)) = (c.samples[0], c.samples[1]) else {
        // SAFETY: the caller's output.
        unsafe { *value = CounterValue { status: PDH_CSTATUS_INVALID_DATA, value: 0 } };
        return PDH_INVALID_DATA;
    };
    let total = new.1.saturating_sub(old.1);
    let idle = new.0.saturating_sub(old.0).min(total);
    let share = match (total, c.kind) {
        (0, _) => 0.0,
        (t, Kind::Busy) => (t - idle) as f64 * 100.0 / t as f64,
        (t, Kind::Idle) => idle as f64 * 100.0 / t as f64,
    };
    let share = if format & PDH_FMT_NOCAP100 == 0 { share.min(100.0) } else { share };
    let bits = if format & PDH_FMT_DOUBLE != 0 {
        share.to_bits()
    } else if format & PDH_FMT_LARGE != 0 {
        share as i64 as u64
    } else {
        u64::from(share as i32 as u32)
    };
    // SAFETY: the caller's output.
    unsafe { *value = CounterValue { status: PDH_CSTATUS_VALID_DATA, value: bits } };
    ERROR_SUCCESS
}

/// PdhRemoveCounter(hCounter).
#[unsafe(no_mangle)]
pub extern "system" fn PdhRemoveCounter(counter: usize) -> u32 {
    with(|s| match slot_of(s, counter, COUNTER_TAG) {
        Some(i) => {
            s.counters[i].query = 0;
            ERROR_SUCCESS
        }
        None => PDH_INVALID_HANDLE,
    })
}

/// PdhCloseQuery(hQuery): the query and its counters.
#[unsafe(no_mangle)]
pub extern "system" fn PdhCloseQuery(query: usize) -> u32 {
    with(|s| {
        let Some(q) = slot_of(s, query, QUERY_TAG) else { return PDH_INVALID_HANDLE };
        s.queries[q].open = false;
        for c in s.counters.iter_mut().filter(|c| c.query == q + 1) {
            c.query = 0;
        }
        ERROR_SUCCESS
    })
}
