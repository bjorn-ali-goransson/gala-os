//! Global and local memory as Windows hands it out (Microsoft's
//! GlobalAlloc, GlobalLock, GlobalUnlock, GlobalReAlloc, GlobalFlags,
//! GlobalHandle and the Local* pages; the details as Wine's
//! `dlls/kernel32/tests/heap.c` checks them on Windows):
//!
//! - GMEM_FIXED memory is a pointer: GlobalLock returns it, GlobalHandle
//!   of it is itself, GlobalFlags 0, GlobalUnlock TRUE (LocalUnlock FALSE
//!   with ERROR_NOT_LOCKED); GlobalAlloc(GMEM_FIXED, 0) is one byte.
//! - GMEM_MOVEABLE memory is a handle to an entry {flags, pointer}: the
//!   handle is the address of the pointer, so `handle % 16 == 8` (heap
//!   blocks are 16-aligned); entry flags 3 (|4 discardable, |8
//!   discarded, |0x8000 GMEM_DDESHARE, the lock count in bits 16-23,
//!   saturating at 255). GlobalLock counts, GlobalUnlock returns TRUE
//!   while still locked, FALSE with NO_ERROR at zero and FALSE with
//!   ERROR_NOT_LOCKED when not locked; GlobalHandle of the data pointer
//!   is the handle.
//! - Size 0 makes a discarded block: GlobalLock fails with
//!   ERROR_DISCARDED, GlobalSize 0, GlobalFlags GMEM_DISCARDED.
//! - GlobalReAlloc: GMEM_MODIFY changes flags only (GMEM_MOVEABLE turns
//!   fixed memory moveable, contents and size kept); a locked block grows
//!   in place unless GMEM_MOVEABLE; size 0 discards an unlocked block
//!   (GlobalDiscard) and fails on a locked one; GMEM_ZEROINIT zeroes the
//!   growth; GMEM_MOVEABLE lets fixed memory move.
//! - Freed and made-up handles: ERROR_INVALID_HANDLE, GlobalFlags
//!   GMEM_INVALID_HANDLE, GlobalFree returning the handle.

use core::ffi::c_void;

use super::{Checks, GetLastError, SetLastError};

type Bool = i32;
type HGlobal = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GlobalAlloc(flags: u32, bytes: usize) -> HGlobal;
    fn GlobalReAlloc(mem: HGlobal, bytes: usize, flags: u32) -> HGlobal;
    fn GlobalFree(mem: HGlobal) -> HGlobal;
    fn GlobalLock(mem: HGlobal) -> *mut u8;
    fn GlobalUnlock(mem: HGlobal) -> Bool;
    fn GlobalSize(mem: HGlobal) -> usize;
    fn GlobalFlags(mem: HGlobal) -> u32;
    fn GlobalHandle(p: *const c_void) -> HGlobal;
    fn LocalAlloc(flags: u32, bytes: usize) -> HGlobal;
    fn LocalReAlloc(mem: HGlobal, bytes: usize, flags: u32) -> HGlobal;
    fn LocalFree(mem: HGlobal) -> HGlobal;
    fn LocalLock(mem: HGlobal) -> *mut u8;
    fn LocalUnlock(mem: HGlobal) -> Bool;
    fn LocalSize(mem: HGlobal) -> usize;
    fn LocalFlags(mem: HGlobal) -> u32;
    fn LocalHandle(p: *const c_void) -> HGlobal;
}

const GMEM_FIXED: u32 = 0;
const GMEM_MOVEABLE: u32 = 0x2;
const GMEM_ZEROINIT: u32 = 0x40;
const GMEM_MODIFY: u32 = 0x80;
const GMEM_DISCARDABLE: u32 = 0x100;
const GMEM_DDESHARE: u32 = 0x2000;
const GMEM_DISCARDED: u32 = 0x4000;
const GMEM_INVALID_HANDLE: u32 = 0x8000;
const LMEM_MOVEABLE: u32 = 0x2;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_DISCARDED: u32 = 157;
const ERROR_NOT_LOCKED: u32 = 158;

/// The entry behind a moveable handle: (flags, data pointer).
///
/// # Safety
/// `h` is a live moveable handle.
unsafe fn entry(h: HGlobal) -> (usize, usize) {
    // SAFETY: the handle is the address of the entry's pointer, the flags
    // word just before it.
    unsafe { (*(h as *const usize).sub(1), *(h as *const usize)) }
}

fn is_entry(h: HGlobal) -> bool {
    h as usize & 15 == 8
}

pub fn global_memory(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: documented kernel32 calls; memory used within its sizes;
    // entries read only behind live handles.
    unsafe {
        c.ok(GlobalFree(core::ptr::null_mut()).is_null(), 0)?;
        c.ok(GlobalReAlloc(core::ptr::null_mut(), 10, GMEM_MOVEABLE).is_null(), 0)?;

        // Fixed memory.
        let fixed = GlobalAlloc(GMEM_FIXED, 100);
        c.ok(!fixed.is_null() && fixed as usize & 15 == 0, fixed as u64)?;
        c.eq(GlobalLock(fixed) as u64, fixed as u64)?;
        SetLastError(0xdead_beef);
        c.ok(GlobalUnlock(fixed) != 0, 0)?;
        c.eq(GetLastError().into(), 0xdead_beef)?;
        c.eq(GlobalHandle(fixed) as u64, fixed as u64)?;
        c.eq(GlobalFlags(fixed).into(), 0)?;
        c.eq(GlobalSize(fixed) as u64, 100)?;
        // GMEM_MODIFY without GMEM_MOVEABLE: the same block, size kept.
        c.eq(GlobalReAlloc(fixed, 1 << 20, GMEM_MODIFY) as u64, fixed as u64)?;
        c.eq(GlobalSize(fixed) as u64, 100)?;
        c.ok(GlobalFree(fixed).is_null(), 0)?;
        let one = GlobalAlloc(GMEM_FIXED, 0);
        c.ok(!one.is_null(), 0)?;
        c.eq(GlobalSize(one) as u64, 1)?;
        c.ok(GlobalFree(one).is_null(), 0)?;
        // GMEM_MOVEABLE lets fixed memory move; the result is still fixed.
        let small = GlobalAlloc(GMEM_FIXED, 12);
        *small.cast::<u8>() = 0x5a;
        let moved = GlobalReAlloc(small, 4096, GMEM_MOVEABLE);
        c.ok(!moved.is_null() && !is_entry(moved), moved as u64)?;
        c.ok(*moved.cast::<u8>() == 0x5a && GlobalLock(moved) == moved.cast(), 0)?;
        c.eq(GlobalSize(moved) as u64, 4096)?;
        c.ok(GlobalFree(moved).is_null(), 0)?;

        // Moveable memory: a handle to an entry.
        let mem = GlobalAlloc(GMEM_MOVEABLE, 256);
        c.ok(is_entry(mem), mem as u64)?;
        c.eq(entry(mem).0 as u64, 3)?;
        c.eq(GlobalSize(mem) as u64, 256)?;
        c.eq(GlobalFlags(mem).into(), 0)?;
        let p = GlobalLock(mem);
        c.ok(!p.is_null() && !core::ptr::eq(p.cast(), mem) && p as usize & 15 == 0, p as u64)?;
        c.eq(entry(mem).1 as u64, p as u64)?;
        c.eq(entry(mem).0 as u64, 0x1_0003)?;
        c.eq(GlobalHandle(p.cast()) as u64, mem as u64)?;
        c.eq(GlobalFlags(mem).into(), 1)?;
        c.eq(GlobalLock(mem) as u64, p as u64)?;
        c.eq(GlobalFlags(mem).into(), 2)?;
        c.ok(GlobalUnlock(mem) != 0, 0)?;
        c.eq(GlobalFlags(mem).into(), 1)?;
        SetLastError(0xdead_beef);
        c.eq(GlobalUnlock(mem) as u64, 0)?;
        c.eq(GetLastError().into(), 0)?;
        c.eq(GlobalFlags(mem).into(), 0)?;
        SetLastError(0xdead_beef);
        c.eq(GlobalUnlock(mem) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_NOT_LOCKED.into())?;
        // The lock count saturates at 255.
        for _ in 0..300 {
            GlobalLock(mem);
        }
        c.eq(entry(mem).0 as u64, 0xff_0003)?;
        for _ in 0..254 {
            c.ok(GlobalUnlock(mem) != 0, 0)?;
        }
        c.eq(GlobalUnlock(mem) as u64, 0)?;
        c.eq(entry(mem).0 as u64, 3)?;
        c.ok(GlobalFree(mem).is_null(), 0)?;
        // Freed handles are caught.
        SetLastError(0xdead_beef);
        c.eq(GlobalFree(mem) as u64, mem as u64)?;
        c.eq(GetLastError().into(), ERROR_INVALID_HANDLE.into())?;
        SetLastError(0xdead_beef);
        c.eq(GlobalFlags(mem).into(), GMEM_INVALID_HANDLE.into())?;
        c.eq(GetLastError().into(), ERROR_INVALID_HANDLE.into())?;
        SetLastError(0xdead_beef);
        c.eq(GlobalSize(mem) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_INVALID_HANDLE.into())?;
        SetLastError(0xdead_beef);
        c.ok(GlobalLock(mem).is_null(), 0)?;
        c.eq(GetLastError().into(), ERROR_INVALID_HANDLE.into())?;
        SetLastError(0xdead_beef);
        c.ok(GlobalReAlloc(mem, 0, GMEM_MOVEABLE).is_null(), 0)?;
        c.eq(GetLastError().into(), ERROR_INVALID_HANDLE.into())?;
        // So are made-up ones.
        let invalid = (0xdead_bee0usize + 8) as HGlobal;
        SetLastError(0xdead_beef);
        c.eq(GlobalFree(invalid) as u64, invalid as u64)?;
        c.eq(GetLastError().into(), ERROR_INVALID_HANDLE.into())?;
        c.eq(GlobalFlags(invalid).into(), GMEM_INVALID_HANDLE.into())?;
        SetLastError(0xdead_beef);
        c.ok(GlobalLock(invalid).is_null(), 0)?;
        c.eq(GetLastError().into(), ERROR_INVALID_HANDLE.into())?;

        // Size 0: a discarded block.
        let empty = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, 0);
        c.ok(is_entry(empty), empty as u64)?;
        c.ok(entry(empty) == (0xb, 0), entry(empty).0 as u64)?;
        c.eq(GlobalSize(empty) as u64, 0)?;
        c.eq(GlobalFlags(empty).into(), GMEM_DISCARDED.into())?;
        SetLastError(0xdead_beef);
        c.ok(GlobalLock(empty).is_null(), 0)?;
        c.eq(GetLastError().into(), ERROR_DISCARDED.into())?;
        // Reallocating gives it memory again; size 0 discards it again.
        c.eq(GlobalReAlloc(empty, 10, GMEM_MOVEABLE) as u64, empty as u64)?;
        c.eq(GlobalSize(empty) as u64, 10)?;
        c.eq(GlobalFlags(empty).into(), 0)?;
        c.eq(GlobalReAlloc(empty, 0, GMEM_MOVEABLE) as u64, empty as u64)?;
        c.ok(entry(empty) == (0xb, 0), entry(empty).0 as u64)?;
        c.ok(GlobalFree(empty).is_null(), 0)?;
        let discardable = GlobalAlloc(GMEM_MOVEABLE | GMEM_DISCARDABLE | GMEM_DDESHARE, 1);
        c.eq(entry(discardable).0 as u64, 0x8007)?;
        c.eq(GlobalFlags(discardable).into(), (GMEM_DISCARDABLE | GMEM_DDESHARE).into())?;
        c.ok(GlobalFree(discardable).is_null(), 0)?;
        let discarded = GlobalAlloc(GMEM_MOVEABLE | GMEM_DISCARDABLE, 0);
        c.eq(entry(discarded).0 as u64, 0xf)?;
        c.eq(GlobalFlags(discarded).into(), (GMEM_DISCARDED | GMEM_DISCARDABLE).into())?;
        c.ok(GlobalFree(discarded).is_null(), 0)?;

        // Fixed memory made moveable (GMEM_MODIFY | GMEM_MOVEABLE): a new
        // handle, the contents and size kept.
        let fixed = GlobalAlloc(GMEM_ZEROINIT, 1000);
        *fixed.cast::<u8>().add(999) = 0x77;
        let mem = GlobalReAlloc(fixed, 0, GMEM_MODIFY | GMEM_MOVEABLE);
        c.ok(is_entry(mem), mem as u64)?;
        c.eq(GlobalSize(mem) as u64, 1000)?;
        let p = GlobalLock(mem);
        c.ok(!p.is_null() && *p.add(999) == 0x77 && *p == 0, 0)?;
        c.eq(GlobalUnlock(mem) as u64, 0)?;
        // Growing with GMEM_ZEROINIT: the same handle, the growth zeroed.
        c.eq(GlobalReAlloc(mem, 5000, GMEM_MOVEABLE | GMEM_ZEROINIT) as u64, mem as u64)?;
        c.eq(GlobalSize(mem) as u64, 5000)?;
        let p = GlobalLock(mem);
        c.ok(*p.add(999) == 0x77 && (1000..5000).all(|i| *p.add(i) == 0), 0)?;
        c.eq(GlobalHandle(p.cast()) as u64, mem as u64)?;
        // A locked block is not discarded, nor moved without GMEM_MOVEABLE
        // (growing in place fails), and shrinks in place.
        c.ok(GlobalReAlloc(mem, 0, GMEM_MOVEABLE).is_null(), 0)?;
        c.ok(GlobalReAlloc(mem, 1 << 20, 0).is_null(), 0)?;
        c.eq(GlobalReAlloc(mem, 10, 0) as u64, mem as u64)?;
        c.eq(entry(mem).1 as u64, p as u64)?;
        c.eq(GlobalSize(mem) as u64, 10)?;
        // With GMEM_MOVEABLE it moves even while locked.
        c.eq(GlobalReAlloc(mem, 1 << 20, GMEM_MOVEABLE) as u64, mem as u64)?;
        c.eq(GlobalSize(mem) as u64, 1 << 20)?;
        c.eq(GlobalFlags(mem).into(), 1)?;
        // GMEM_DISCARDABLE without GMEM_MODIFY fails; with it, it marks
        // the block discardable.
        c.ok(GlobalReAlloc(mem, 64, GMEM_MOVEABLE | GMEM_DISCARDABLE).is_null(), 0)?;
        c.eq(GlobalReAlloc(mem, 0, GMEM_MODIFY | GMEM_DISCARDABLE) as u64, mem as u64)?;
        c.eq(entry(mem).0 as u64, 0x1_0007)?;
        c.eq(GlobalUnlock(mem) as u64, 0)?;
        // GlobalDiscard (GlobalReAlloc(h, 0, GMEM_MOVEABLE)) once unlocked.
        c.eq(GlobalReAlloc(mem, 0, GMEM_MOVEABLE) as u64, mem as u64)?;
        c.eq(GlobalFlags(mem).into(), (GMEM_DISCARDED | GMEM_DISCARDABLE).into())?;
        c.ok(GlobalFree(mem).is_null(), 0)?;

        // Local memory: the same handles; LocalUnlock of fixed memory is
        // FALSE with ERROR_NOT_LOCKED.
        let fixed = LocalAlloc(0, 16);
        SetLastError(0xdead_beef);
        c.eq(LocalUnlock(fixed) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_NOT_LOCKED.into())?;
        c.eq(LocalHandle(fixed) as u64, fixed as u64)?;
        c.ok(LocalFree(fixed).is_null(), 0)?;
        let mem = LocalAlloc(LMEM_MOVEABLE, 256);
        c.ok(is_entry(mem), mem as u64)?;
        let p = LocalLock(mem);
        c.ok(!p.is_null() && LocalHandle(p.cast()) == mem, p as u64)?;
        c.eq(LocalFlags(mem).into(), 1)?;
        c.eq(LocalSize(mem) as u64, 256)?;
        SetLastError(0xdead_beef);
        c.eq(LocalUnlock(mem) as u64, 0)?;
        c.eq(GetLastError().into(), 0)?;
        c.eq(LocalReAlloc(mem, 512, LMEM_MOVEABLE) as u64, mem as u64)?;
        c.eq(LocalSize(mem) as u64, 512)?;
        // Global and local handles are one kind.
        c.eq(GlobalSize(mem) as u64, 512)?;
        c.ok(LocalFree(mem).is_null(), 0)?;
        SetLastError(0xdead_beef);
        c.eq(LocalUnlock(mem) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_INVALID_HANDLE.into())?;
    }
    Ok(())
}
