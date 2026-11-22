//! The display driver's adapter enumeration (D3DKMT): the two calls the
//! Vulkan loader looks up by name before it looks for a driver, and
//! which DXVK's dxgi asks for an adapter's registry key.
//!
//! gala has no display driver (GPU G2), and what Windows answers on a
//! machine with no adapter is an answer gala can give truthfully:
//! `D3DKMTEnumAdapters2` succeeds and writes none, and
//! `D3DKMTQueryAdapterInfo` refuses every handle offered it with
//! STATUS_INVALID_PARAMETER -- which is what Windows answers for a
//! handle it did not issue, and no handle here was ever issued. The
//! other nineteen `D3DKMT*` thunks still refuse outright (`absent`):
//! each of them takes an adapter or a device, and this one hands out
//! neither.
//!
//! The sizes and the refusals are measured (`user/d3dkmt_test`, which
//! passes on a machine with adapters and on one without).
//!
//! Observability: the first call that asks for adapters leaves a
//! "reached" row with the room the caller made, because a program told
//! there is no GPU leaves nothing else behind; a query leaves an
//! "unsupported" row naming its type, once per type. The ledger has a
//! fixed number of slots, so a row's name carries nothing of the
//! caller's -- with one exception, the first registry value a process
//! asks an adapter for, which is the one row that says what to answer
//! next.

use super::{reached, unsupported};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const STATUS_SUCCESS: i32 = 0;
const STATUS_INVALID_PARAMETER: i32 = 0xc000_000d_u32 as i32;

/// The query types (KMTQAITYPE) this file knows by name.
const UMDRIVERNAME: u32 = 1;
const ADAPTERREGISTRYINFO: u32 = 8;
const ADAPTERTYPE: u32 = 15;
const QUERYREGISTRY: u32 = 48;

/// D3DDDI_QUERYREGISTRY_INFO: its size, which is the smallest a
/// QUERYREGISTRY query can be, and where its value name sits in it (a
/// WCHAR[MAX_PATH]).
const QRI_SIZE: usize = 552;
const QRI_VALUE_NAME: usize = 8;
const QRI_VALUE_NAME_BYTES: usize = 520;

/// D3DKMT_ENUMADAPTERS2: how many adapters there is room for (and, on
/// the way back, how many there are), and where to write them.
#[repr(C)]
pub struct EnumAdapters2 {
    num_adapters: u32,
    adapters: *mut u8,
}

/// D3DKMT_QUERYADAPTERINFO: which adapter, which question, and the
/// buffer the answer goes in.
#[repr(C)]
pub struct QueryAdapterInfo {
    adapter: u32,
    kind: u32,
    data: *mut u8,
    size: u32,
}

/// The adapters there are, of which gala has none. With no array this
/// is a question about room, and the answer is still none; with one,
/// nothing is written. `NumAdapters` comes back 0 either way, as a
/// Windows with no adapter answers, and no caller can be short of room
/// for none -- so the STATUS_BUFFER_TOO_SMALL a short array gets there
/// cannot arise here.
#[unsafe(no_mangle)]
pub extern "system" fn D3DKMTEnumAdapters2(info: *mut EnumAdapters2) -> i32 {
    // SAFETY: the caller's D3DKMT_ENUMADAPTERS2, or nothing (which is
    // what the one argument being required means).
    let Some(info) = (unsafe { info.as_mut() }) else {
        return STATUS_INVALID_PARAMETER;
    };
    note_enum(u64::from(info.num_adapters));
    info.num_adapters = 0;
    STATUS_SUCCESS
}

/// What an adapter answers about itself -- of which there is none. Every
/// handle a caller offers is one this file never issued, which is the
/// STATUS_INVALID_PARAMETER Windows answers for one of those, whatever
/// the query type and whatever size it was given.
#[unsafe(no_mangle)]
pub extern "system" fn D3DKMTQueryAdapterInfo(info: *mut QueryAdapterInfo) -> i32 {
    // SAFETY: the caller's D3DKMT_QUERYADAPTERINFO, or nothing.
    let Some(info) = (unsafe { info.as_ref() }) else {
        return STATUS_INVALID_PARAMETER;
    };
    note_query(info);
    STATUS_INVALID_PARAMETER
}

/// A row for the first call that asks for adapters, the room the caller
/// made as its detail: a program that takes its "no GPU" path is not
/// refused anything and leaves nothing else behind, so this row is what
/// says it asked.
fn note_enum(room: u64) {
    static ASKED: AtomicBool = AtomicBool::new(false);
    if !ASKED.swap(true, Ordering::Relaxed) {
        reached(b"gdi32.dll!D3DKMTEnumAdapters2(no adapters)", room);
    }
}

/// A row naming the query type, once per type however often it is
/// asked, and for a registry query the value name as well (`note_value`).
/// A type of 64 or more leaves one row for the first of them: they are
/// not types any caller gala runs has asked for, and the slot is better
/// spent than lost.
fn note_query(info: &QueryAdapterInfo) {
    static LOW: AtomicU64 = AtomicU64::new(0);
    static HIGH: AtomicBool = AtomicBool::new(false);
    let first = match info.kind {
        kind @ 0..64 => {
            let mask = 1u64 << kind;
            LOW.fetch_or(mask, Ordering::Relaxed) & mask == 0
        }
        _ => !HIGH.swap(true, Ordering::Relaxed),
    };
    if first {
        let mut name = [0u8; 64];
        let n = query_name(info.kind, &mut name);
        unsupported(&name[..n], u64::from(info.kind));
    }
    if info.kind == QUERYREGISTRY {
        note_value(info);
    }
}

/// `gdi32.dll!D3DKMTQueryAdapterInfo(<type>)` in `name`, its length
/// back: the types this file knows by name, and the number for the
/// rest.
fn query_name(kind: u32, name: &mut [u8; 64]) -> usize {
    const PREFIX: &[u8] = b"gdi32.dll!D3DKMTQueryAdapterInfo(";
    name[..PREFIX.len()].copy_from_slice(PREFIX);
    let mut n = PREFIX.len();
    let known: &[u8] = match kind {
        UMDRIVERNAME => b"UMDRIVERNAME",
        ADAPTERREGISTRYINFO => b"ADAPTERREGISTRYINFO",
        ADAPTERTYPE => b"ADAPTERTYPE",
        QUERYREGISTRY => b"QUERYREGISTRY",
        _ => b"type ",
    };
    name[n..n + known.len()].copy_from_slice(known);
    n += known.len();
    if !matches!(kind, UMDRIVERNAME | ADAPTERREGISTRYINFO | ADAPTERTYPE | QUERYREGISTRY) {
        n += decimal(kind, &mut name[n..]);
    }
    name[n] = b')';
    n + 1
}

/// One row for the first registry value a process asks an adapter for
/// (the Vulkan loader asks for `VulkanDriverName`), its D3DDDI query
/// type -- the adapter's key, the service's, the driver store's -- as
/// the detail. The caller chooses the name, so this is the one row and
/// no more: the ledger's slots are fixed and a program that asked for a
/// new name each time would spend them all.
fn note_value(info: &QueryAdapterInfo) {
    static ASKED: AtomicBool = AtomicBool::new(false);
    if info.data.is_null() || (info.size as usize) < QRI_SIZE || ASKED.swap(true, Ordering::Relaxed)
    {
        return;
    }
    // SAFETY: the caller's D3DDDI_QUERYREGISTRY_INFO, which it says is
    // at least `size` bytes and which is at least the structure's own
    // size (both checked above).
    let query = unsafe { core::slice::from_raw_parts(info.data, QRI_SIZE) };
    const PREFIX: &[u8] = b"gdi32.dll!D3DKMTQueryAdapterInfo(QUERYREGISTRY ";
    let mut name = [0u8; 128];
    name[..PREFIX.len()].copy_from_slice(PREFIX);
    let mut n = PREFIX.len();
    let value = &query[QRI_VALUE_NAME..QRI_VALUE_NAME + QRI_VALUE_NAME_BYTES];
    let mut i = 0;
    while i + 1 < value.len() && n + 1 < name.len() {
        let c = u16::from_le_bytes([value[i], value[i + 1]]);
        if c == 0 {
            break;
        }
        name[n] = if (0x20..0x7f).contains(&c) { c as u8 } else { b'?' };
        n += 1;
        i += 2;
    }
    name[n] = b')';
    let key = u32::from_le_bytes([query[0], query[1], query[2], query[3]]);
    unsupported(&name[..n + 1], u64::from(key));
}

/// `v` in decimal at the start of `out`, how many bytes it took.
fn decimal(v: u32, out: &mut [u8]) -> usize {
    let mut digits = [0u8; 10];
    let mut at = digits.len();
    let mut left = v;
    loop {
        at -= 1;
        digits[at] = b'0' + (left % 10) as u8;
        left /= 10;
        if left == 0 {
            break;
        }
    }
    let n = digits.len() - at;
    out[..n].copy_from_slice(&digits[at..]);
    n
}
