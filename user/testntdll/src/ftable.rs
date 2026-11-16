//! Function tables: the RUNTIME_FUNCTION entries that find a pc's unwind
//! data, from the image containing it (its exception directory, sorted by
//! address) or registered at run time by code outside images (JIT
//! compilers: RtlAddFunctionTable, RtlAddGrowableFunctionTable,
//! RtlInstallFunctionTableCallback). Shared by x64 and ARM64; the entry's
//! format is the architecture's `seh` module's (`RuntimeFunction`, its
//! size, the search and how an entry that names another is followed).

use crate::image_of;
use crate::seh::{ENTRY_SIZE, RuntimeFunction, resolve, search};

/// Function tables registered with RtlAddFunctionTable: (first entry,
/// entry count, base address).
const MAX_TABLES: usize = 32;

/// A table registered with RtlInstallFunctionTableCallback: its id (the
/// low two bits set; 0: free slot), base, length, callback and context.
#[derive(Clone, Copy)]
struct Callback {
    id: u64,
    base: u64,
    length: u32,
    callback: usize,
    context: usize,
}

const NO_CALLBACK: Callback = Callback { id: 0, base: 0, length: 0, callback: 0, context: 0 };

/// A table registered with RtlAddGrowableFunctionTable: its entries
/// (0: free slot), how many are in use and may be, and the range
/// [base, end) they describe (relative to base). Its handle is
/// `growable_handle(slot)`.
#[derive(Clone, Copy)]
struct Growable {
    table: usize,
    count: u32,
    max: u32,
    base: u64,
    end: u64,
}

const NO_GROWABLE: Growable = Growable { table: 0, count: 0, max: 0, base: 0, end: 0 };

/// The opaque handle of growable slot `i` (never null).
fn growable_handle(i: usize) -> usize {
    0x6700_0000 + i * 16
}

fn growable_slot(handle: usize) -> Option<usize> {
    let i = handle.checked_sub(0x6700_0000)?;
    (i % 16 == 0 && i / 16 < MAX_TABLES).then_some(i / 16)
}

struct State {
    tables: [(usize, u32, u64); MAX_TABLES],
    callbacks: [Callback; MAX_TABLES],
    growable: [Growable; MAX_TABLES],
}

struct Tables {
    lock: core::sync::atomic::AtomicBool,
    state: core::cell::UnsafeCell<State>,
}

// SAFETY: the state is only touched with `lock` held (a spin lock: the
// critical sections are a few loads and stores, and callbacks run outside).
unsafe impl Sync for Tables {}

static TABLES: Tables = Tables {
    lock: core::sync::atomic::AtomicBool::new(false),
    state: core::cell::UnsafeCell::new(State {
        tables: [(0, 0, 0); MAX_TABLES],
        callbacks: [NO_CALLBACK; MAX_TABLES],
        growable: [NO_GROWABLE; MAX_TABLES],
    }),
};

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    use core::sync::atomic::Ordering;
    while TABLES
        .lock
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    // SAFETY: the lock is held.
    let r = f(unsafe { &mut *TABLES.state.get() });
    TABLES.lock.store(false, Ordering::Release);
    r
}

fn with_tables<R>(f: impl FnOnce(&mut [(usize, u32, u64); MAX_TABLES]) -> R) -> R {
    with_state(|s| f(&mut s.tables))
}

/// RtlLookupFunctionEntry(ControlPc, *ImageBase, HistoryTable): the
/// RUNTIME_FUNCTION covering `pc` in the loaded module containing it (from
/// its exception directory, sorted by address) or in a registered
/// function table; null for leaf code and addresses outside both.
///
/// # Safety
/// `image_base` must be null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn RtlLookupFunctionEntry(
    pc: u64,
    image_base: *mut u64,
    _history: *mut core::ffi::c_void,
) -> *const RuntimeFunction {
    let (base, entry) = lookup(pc);
    if !entry.is_null() && !image_base.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { *image_base = base };
    }
    entry
}

/// The image or table base and the entry covering `pc` (null: none, a
/// leaf function or code nothing describes).
pub(crate) fn lookup(pc: u64) -> (u64, *const RuntimeFunction) {
    if let Some(base) = image_of(pc as usize) {
        let (rva, size) = crate::directory(base, 3); // IMAGE_DIRECTORY_ENTRY_EXCEPTION
        if rva != 0 {
            let table = (base + rva) as *const RuntimeFunction;
            let f = search(base as u64, table, size / ENTRY_SIZE, (pc as usize - base) as u32);
            if !f.is_null() {
                return (base as u64, resolve(base as u64, f));
            }
        }
    }
    let found = with_state(|s| {
        let fixed = s.tables.iter().find_map(|&(table, count, base)| {
            if table == 0 || pc < base || pc - base > u64::from(u32::MAX) {
                return None;
            }
            let f =
                search(base, table as *const RuntimeFunction, count as usize, (pc - base) as u32);
            (!f.is_null()).then(|| (base, resolve(base, f)))
        });
        fixed.or_else(|| {
            s.growable.iter().find_map(|g| {
                if g.table == 0 || pc < g.base || pc >= g.end || pc - g.base > u64::from(u32::MAX) {
                    return None;
                }
                let f = search(
                    g.base,
                    g.table as *const RuntimeFunction,
                    g.count as usize,
                    (pc - g.base) as u32,
                );
                (!f.is_null()).then(|| (g.base, resolve(g.base, f)))
            })
        })
    });
    if let Some(found) = found {
        return found;
    }
    // A callback covering `pc`, called without the lock held.
    let cb = with_state(|s| {
        s.callbacks
            .iter()
            .copied()
            .find(|c| c.id != 0 && pc >= c.base && pc - c.base < u64::from(c.length))
    });
    if let Some(cb) = cb {
        // SAFETY: a PGET_RUNTIME_FUNCTION_CALLBACK the program registered.
        let f: extern "system" fn(u64, usize) -> *const RuntimeFunction =
            unsafe { core::mem::transmute(cb.callback) };
        let entry = f(pc, cb.context);
        if !entry.is_null() {
            return (cb.base, entry);
        }
    }
    (0, core::ptr::null())
}

/// RtlInstallFunctionTableCallback(TableIdentifier, BaseAddress, Length,
/// Callback, Context, OutOfProcessCallbackDll): unwind data for
/// [BaseAddress, +Length) comes from Callback(pc, Context). The id must
/// have its low two bits set (it is what RtlDeleteFunctionTable takes).
#[unsafe(no_mangle)]
pub extern "system" fn RtlInstallFunctionTableCallback(
    id: u64,
    base: u64,
    length: u32,
    callback: usize,
    context: usize,
    _dll: *const u16,
) -> u8 {
    if id & 3 != 3 || callback == 0 {
        return 0;
    }
    with_state(|s| match s.callbacks.iter_mut().find(|c| c.id == 0) {
        Some(slot) => {
            *slot = Callback { id, base, length, callback, context };
            1
        }
        None => 0,
    })
}

/// RtlAddFunctionTable(FunctionTable, EntryCount, BaseAddress): registers
/// unwind data for code outside images (JIT compilers). The entries must
/// be sorted, as for an image.
#[unsafe(no_mangle)]
pub extern "system" fn RtlAddFunctionTable(
    table: *const RuntimeFunction,
    count: u32,
    base: u64,
) -> u8 {
    with_tables(|t| match t.iter_mut().find(|e| e.0 == 0) {
        Some(slot) => {
            *slot = (table as usize, count, base);
            1
        }
        None => 0,
    })
}

/// RtlAddGrowableFunctionTable(*DynamicTable, FunctionTable, EntryCount,
/// MaximumEntryCount, RangeBase, RangeEnd): registers a table for code in
/// [RangeBase, RangeEnd) whose first EntryCount entries (sorted, relative
/// to RangeBase) are in use; RtlGrowFunctionTable counts more. V8
/// registers its code space with it. STATUS_SUCCESS and the table's
/// handle; STATUS_INVALID_PARAMETER for a count over the maximum or an
/// empty range; STATUS_NO_MEMORY when all slots are taken.
///
/// # Safety
/// `handle` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn RtlAddGrowableFunctionTable(
    handle: *mut usize,
    table: *const RuntimeFunction,
    count: u32,
    max: u32,
    base: usize,
    end: usize,
) -> u32 {
    if handle.is_null() || table.is_null() || count > max || end <= base {
        return 0xc000_000d; // STATUS_INVALID_PARAMETER
    }
    let slot = with_state(|s| {
        let i = s.growable.iter().position(|g| g.table == 0)?;
        s.growable[i] =
            Growable { table: table as usize, count, max, base: base as u64, end: end as u64 };
        Some(i)
    });
    match slot {
        Some(i) => {
            // SAFETY: the caller's output pointer.
            unsafe { *handle = growable_handle(i) };
            0
        }
        None => {
            let what = b"ntdll.dll!RtlAddGrowableFunctionTable(table slots full)";
            crate::GalaRecordMissing(what.as_ptr(), what.len(), 2, MAX_TABLES as u64);
            0xc000_0017 // STATUS_NO_MEMORY
        }
    }
}

/// RtlGrowFunctionTable(DynamicTable, NewEntryCount): the first
/// NewEntryCount entries are in use now (the caller has written them; at
/// most the maximum, and never fewer than before).
#[unsafe(no_mangle)]
pub extern "system" fn RtlGrowFunctionTable(handle: usize, count: u32) {
    let Some(i) = growable_slot(handle) else { return };
    with_state(|s| {
        let g = &mut s.growable[i];
        if g.table != 0 && count <= g.max && count >= g.count {
            g.count = count;
        }
    });
}

/// RtlDeleteGrowableFunctionTable(DynamicTable).
#[unsafe(no_mangle)]
pub extern "system" fn RtlDeleteGrowableFunctionTable(handle: usize) {
    if let Some(i) = growable_slot(handle) {
        with_state(|s| s.growable[i] = NO_GROWABLE);
    }
}

/// RtlDeleteFunctionTable(FunctionTable).
#[unsafe(no_mangle)]
pub extern "system" fn RtlDeleteFunctionTable(table: *const RuntimeFunction) -> u8 {
    let id = table as usize;
    with_state(|s| {
        if let Some(slot) = s.tables.iter_mut().find(|e| e.0 == id && id != 0) {
            *slot = (0, 0, 0);
            return 1;
        }
        match s.callbacks.iter_mut().find(|c| c.id == id as u64 && id != 0) {
            Some(slot) => {
                *slot = NO_CALLBACK;
                1
            }
            None => 0,
        }
    })
}
