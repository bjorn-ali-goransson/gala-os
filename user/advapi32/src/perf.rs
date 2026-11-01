//! PerfLib V2 counter providers (perflib.h): what a provider such as SQL
//! Server registers and updates. gala has no counter consumers
//! (Performance Monitor, PDH), so the counters are kept in the provider's
//! memory and nobody reads them: providers, their counter sets (copies of
//! the templates: PERF_COUNTERSET_INFO, then PERF_COUNTER_INFO entries) and
//! instances (PERF_COUNTERSET_INSTANCE, the counter data block laid out by
//! the counters' offsets, then the instance name).

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, block: *mut u8) -> i32;
    fn SetLastError(code: u32);
}

const ERROR_SUCCESS: u32 = 0;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_NOT_FOUND: u32 = 1168;

const MAGIC: u32 = u32::from_le_bytes(*b"GPRF");
const MAX_SETS: usize = 64;
/// PERF_COUNTERSET_INSTANCE's size.
const HEADER: usize = 32;
const COUNTERSET_INFO: usize = 40;
const COUNTER_INFO: usize = 32;

/// A counter set: its template (a heap copy).
#[derive(Clone, Copy)]
struct Set {
    template: *mut u8,
}

struct Provider {
    magic: u32,
    lock: AtomicBool,
    sets: [Set; MAX_SETS],
    count: usize,
}

fn alloc(size: usize) -> *mut u8 {
    // SAFETY: plain allocation, zeroed.
    unsafe { HeapAlloc(GetProcessHeap(), 8, size) }
}

fn free(p: *mut u8) {
    if !p.is_null() {
        // SAFETY: a block of the process heap.
        unsafe { HeapFree(GetProcessHeap(), 0, p) };
    }
}

fn provider<'a>(handle: usize) -> Option<&'a mut Provider> {
    if handle == 0 {
        return None;
    }
    // SAFETY: a handle PerfStartProviderEx returned points at a Provider.
    let p = unsafe { &mut *(handle as *mut Provider) };
    (p.magic == MAGIC).then_some(p)
}

fn read_u32(p: *const u8, at: usize) -> u32 {
    // SAFETY: inside a template or instance of ours.
    unsafe { p.add(at).cast::<u32>().read_unaligned() }
}

fn guid(p: *const u8) -> [u8; 16] {
    // SAFETY: a GUID (16 bytes).
    unsafe { p.cast::<[u8; 16]>().read_unaligned() }
}

impl Provider {
    fn with<R>(&mut self, f: impl FnOnce(&mut Provider) -> R) -> R {
        while self.lock.swap(true, Ordering::Acquire) {
            core::hint::spin_loop();
        }
        let r = f(self);
        self.lock.store(false, Ordering::Release);
        r
    }

    fn set(&self, guid_of: &[u8; 16]) -> Option<Set> {
        self.sets[..self.count].iter().copied().find(|s| guid(s.template) == *guid_of)
    }
}

/// PerfStartProviderEx: registers a provider (its GUID; the context's
/// callbacks are not called: no consumer asks).
#[unsafe(no_mangle)]
pub extern "system" fn PerfStartProviderEx(
    _provider: *const u8,
    _context: *const u8,
    handle: *mut usize,
) -> u32 {
    if handle.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let p = alloc(size_of::<Provider>()).cast::<Provider>();
    if p.is_null() {
        return ERROR_NOT_ENOUGH_MEMORY;
    }
    // SAFETY: a fresh block of the right size; the caller's output.
    unsafe {
        p.write(Provider {
            magic: MAGIC,
            lock: AtomicBool::new(false),
            sets: [Set { template: core::ptr::null_mut() }; MAX_SETS],
            count: 0,
        });
        handle.write(p as usize);
    }
    ERROR_SUCCESS
}

/// PerfStopProvider: frees the provider and its templates.
#[unsafe(no_mangle)]
pub extern "system" fn PerfStopProvider(handle: usize) -> u32 {
    let Some(p) = provider(handle) else { return ERROR_INVALID_HANDLE };
    for s in &p.sets[..p.count] {
        free(s.template);
    }
    p.magic = 0;
    free((p as *mut Provider).cast());
    ERROR_SUCCESS
}

/// PerfSetCounterSetInfo: a counter set's template (copied).
#[unsafe(no_mangle)]
pub extern "system" fn PerfSetCounterSetInfo(handle: usize, template: *const u8, size: u32) -> u32 {
    let Some(p) = provider(handle) else { return ERROR_INVALID_HANDLE };
    let size = size as usize;
    if template.is_null() || size < COUNTERSET_INFO {
        return ERROR_INVALID_PARAMETER;
    }
    let counters = read_u32(template, 32) as usize;
    if size < COUNTERSET_INFO + COUNTER_INFO * counters {
        return ERROR_INVALID_PARAMETER;
    }
    let copy = alloc(size);
    if copy.is_null() {
        return ERROR_NOT_ENOUGH_MEMORY;
    }
    // SAFETY: the caller's template of `size` bytes, into our copy.
    unsafe { core::ptr::copy_nonoverlapping(template, copy, size) };
    p.with(|p| {
        if p.set(&guid(copy)).is_some() {
            free(copy);
            return ERROR_ALREADY_EXISTS;
        }
        if p.count == MAX_SETS {
            free(copy);
            return ERROR_NOT_ENOUGH_MEMORY;
        }
        p.sets[p.count] = Set { template: copy };
        p.count += 1;
        ERROR_SUCCESS
    })
}

/// (offset, size) of counter `id` in the set's data block.
fn counter(set: Set, id: u32) -> Option<(usize, usize)> {
    let counters = read_u32(set.template, 32) as usize;
    (0..counters).map(|i| COUNTERSET_INFO + COUNTER_INFO * i).find_map(|at| {
        (read_u32(set.template, at) == id).then(|| {
            (read_u32(set.template, at + 28) as usize, read_u32(set.template, at + 16) as usize)
        })
    })
}

/// The data block's size: the end of the furthest counter.
fn data_size(set: Set) -> usize {
    let counters = read_u32(set.template, 32) as usize;
    (0..counters)
        .map(|i| COUNTERSET_INFO + COUNTER_INFO * i)
        .map(|at| {
            read_u32(set.template, at + 28) as usize + read_u32(set.template, at + 16) as usize
        })
        .max()
        .unwrap_or(0)
        .next_multiple_of(8)
}

/// PerfCreateInstance: an instance of a counter set (zeroed counters).
#[unsafe(no_mangle)]
pub extern "system" fn PerfCreateInstance(
    handle: usize,
    set_guid: *const u8,
    name: *const u16,
    id: u32,
) -> *mut u8 {
    let fail = |code: u32| {
        // SAFETY: plain call.
        unsafe { SetLastError(code) };
        core::ptr::null_mut()
    };
    let Some(p) = provider(handle) else { return fail(ERROR_INVALID_HANDLE) };
    if set_guid.is_null() {
        return fail(ERROR_INVALID_PARAMETER);
    }
    let Some(set) = p.with(|p| p.set(&guid(set_guid))) else { return fail(ERROR_NOT_FOUND) };
    let mut name_len = 0;
    if !name.is_null() {
        // SAFETY: the caller's NUL-terminated name.
        while unsafe { *name.add(name_len) } != 0 {
            name_len += 1;
        }
    }
    let data = data_size(set);
    let name_bytes = 2 * (name_len + 1);
    let size = HEADER + data + name_bytes;
    let block = alloc(size);
    if block.is_null() {
        return fail(ERROR_NOT_ENOUGH_MEMORY);
    }
    // SAFETY: the block just allocated (zeroed).
    unsafe {
        core::ptr::copy_nonoverlapping(set_guid, block, 16);
        block.add(16).cast::<u32>().write(size as u32);
        block.add(20).cast::<u32>().write(id);
        block.add(24).cast::<u32>().write((HEADER + data) as u32);
        block.add(28).cast::<u32>().write(name_bytes as u32);
        if name_len != 0 {
            core::ptr::copy_nonoverlapping(name, block.add(HEADER + data).cast::<u16>(), name_len);
        }
    }
    block
}

/// PerfDeleteInstance.
#[unsafe(no_mangle)]
pub extern "system" fn PerfDeleteInstance(handle: usize, instance: *mut u8) -> u32 {
    if provider(handle).is_none() {
        return ERROR_INVALID_HANDLE;
    }
    if instance.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    free(instance);
    ERROR_SUCCESS
}

/// Applies `f` to counter `id`'s value in `instance` (a 4- or 8-byte
/// counter as the caller says).
fn update(handle: usize, instance: *mut u8, id: u32, wide: bool, f: impl FnOnce(&Value)) -> u32 {
    let Some(p) = provider(handle) else { return ERROR_INVALID_HANDLE };
    if instance.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let Some(set) = p.with(|p| p.set(&guid(instance))) else { return ERROR_NOT_FOUND };
    let Some((offset, size)) = counter(set, id) else { return ERROR_NOT_FOUND };
    if size < if wide { 8 } else { 4 } {
        return ERROR_INVALID_PARAMETER;
    }
    let at = instance.wrapping_add(HEADER + offset);
    f(&Value { at, wide });
    ERROR_SUCCESS
}

/// A counter's place in an instance's data block.
struct Value {
    at: *mut u8,
    wide: bool,
}

impl Value {
    /// Adds `delta` (wrapping; a negative delta subtracts), or stores it.
    fn apply(&self, delta: u64, store: bool) {
        // SAFETY: inside the instance's data block (the counter's offset
        // and size); aligned counters are updated atomically.
        unsafe {
            if self.wide && (self.at as usize).is_multiple_of(8) {
                let a = &*self.at.cast::<AtomicU64>();
                if store {
                    a.store(delta, Ordering::Relaxed)
                } else {
                    a.fetch_add(delta, Ordering::Relaxed);
                }
            } else if !self.wide && (self.at as usize).is_multiple_of(4) {
                let a = &*self.at.cast::<AtomicU32>();
                if store {
                    a.store(delta as u32, Ordering::Relaxed)
                } else {
                    a.fetch_add(delta as u32, Ordering::Relaxed);
                }
            } else if self.wide {
                let p = self.at.cast::<u64>();
                p.write_unaligned(if store {
                    delta
                } else {
                    p.read_unaligned().wrapping_add(delta)
                });
            } else {
                let p = self.at.cast::<u32>();
                p.write_unaligned(if store {
                    delta as u32
                } else {
                    p.read_unaligned().wrapping_add(delta as u32)
                });
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn PerfSetULongCounterValue(
    handle: usize,
    instance: *mut u8,
    id: u32,
    value: u32,
) -> u32 {
    update(handle, instance, id, false, |v| v.apply(value.into(), true))
}

#[unsafe(no_mangle)]
pub extern "system" fn PerfIncrementULongCounterValue(
    handle: usize,
    instance: *mut u8,
    id: u32,
    value: u32,
) -> u32 {
    update(handle, instance, id, false, |v| v.apply(value.into(), false))
}

#[unsafe(no_mangle)]
pub extern "system" fn PerfDecrementULongCounterValue(
    handle: usize,
    instance: *mut u8,
    id: u32,
    value: u32,
) -> u32 {
    update(handle, instance, id, false, |v| v.apply(u64::from(value.wrapping_neg()), false))
}

#[unsafe(no_mangle)]
pub extern "system" fn PerfSetULongLongCounterValue(
    handle: usize,
    instance: *mut u8,
    id: u32,
    value: u64,
) -> u32 {
    update(handle, instance, id, true, |v| v.apply(value, true))
}

#[unsafe(no_mangle)]
pub extern "system" fn PerfIncrementULongLongCounterValue(
    handle: usize,
    instance: *mut u8,
    id: u32,
    value: u64,
) -> u32 {
    update(handle, instance, id, true, |v| v.apply(value, false))
}

#[unsafe(no_mangle)]
pub extern "system" fn PerfDecrementULongLongCounterValue(
    handle: usize,
    instance: *mut u8,
    id: u32,
    value: u64,
) -> u32 {
    update(handle, instance, id, true, |v| v.apply(value.wrapping_neg(), false))
}
