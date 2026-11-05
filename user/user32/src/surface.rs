//! Window surfaces: a top-level window's pixels (its menu bar's rows,
//! then the client area, 0x00RRGGBB) live in a pagefile-backed section
//! mapped into its process. The section is registered with the kernel's
//! window registry (GalaWindowSurface) so that another process of the
//! desktop can map it: GetDC of another process's window draws into the
//! same pixels the owner draws into, and ReleaseDC shows them
//! (GalaWindowPresent), as one surface per window on Windows. Chromium's
//! GPU process paints its browser's window this way.
//!
//! A foreign DC accumulates what it draws (gdi32's bounds, SetBoundsRect)
//! and ReleaseDC presents that rectangle only; the mapping is kept
//! between GetDC calls while the surface's generation stays (a resize
//! gives the window a new section). Limits: another process's child
//! windows have no DC (ledger); at most 16 foreign DCs at once and 8
//! mapped surfaces.

use crate::window::{Handle, Hwnd};
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, Ordering};

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaWindowSurface(hwnd: u64, section: Handle, offset: u64, width: u32, height: u32) -> u32;
    fn GalaWindowSurfaceOpen(hwnd: u64, section: *mut Handle, info: *mut [u8; 24]) -> u32;
    fn GalaWindowPresent(
        id: u32,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        pixels: *const u32,
        stride: usize,
    ) -> u32;
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateFileMappingW(
        file: Handle,
        sa: *const c_void,
        protect: u32,
        size_high: u32,
        size_low: u32,
        name: *const u16,
    ) -> Handle;
    fn MapViewOfFile(
        mapping: Handle,
        access: u32,
        offset_high: u32,
        offset_low: u32,
        size: usize,
    ) -> *mut c_void;
    fn UnmapViewOfFile(at: *const c_void) -> i32;
    fn CloseHandle(h: Handle) -> i32;
}

const PAGE_READWRITE: u32 = 4;
/// FILE_MAP_READ | FILE_MAP_WRITE.
const FILE_MAP_READ_WRITE: u32 = 6;

/// A buffer of `cw` x (`ch` + `bar`) pixels in a new section: its first
/// pixel and the section's handle.
pub(crate) fn alloc(cw: i32, ch: i32, bar: i32) -> Option<(*mut u32, Handle)> {
    let bytes = u64::from((cw * (ch + bar)).max(1) as u32) * 4;
    // SAFETY: plain calls; a pagefile-backed section (zero-filled).
    unsafe {
        let section = CreateFileMappingW(
            -1isize as Handle,
            core::ptr::null(),
            PAGE_READWRITE,
            (bytes >> 32) as u32,
            bytes as u32,
            core::ptr::null(),
        );
        if section.is_null() {
            return None;
        }
        let base = MapViewOfFile(section, FILE_MAP_READ_WRITE, 0, 0, 0).cast::<u32>();
        if base.is_null() {
            CloseHandle(section);
            return None;
        }
        Some((base, section))
    }
}

/// Frees a buffer `alloc` gave.
pub(crate) fn free(base: *mut u32, section: Handle) {
    // SAFETY: our view and handle, no longer used.
    unsafe {
        if !base.is_null() {
            UnmapViewOfFile(base.cast());
        }
        if !section.is_null() {
            CloseHandle(section);
        }
    }
}

/// Registers top-level window `hwnd`'s surface (after it is registered,
/// and after each new buffer): the client area starts `bar` rows in.
pub(crate) fn register(hwnd: Hwnd, section: Handle, cw: i32, ch: i32, bar: i32) {
    if section.is_null() || cw <= 0 || ch <= 0 {
        return;
    }
    let offset = u64::from((bar * cw) as u32) * 4;
    // SAFETY: a plain call on our section handle.
    let status = unsafe { GalaWindowSurface(hwnd as u64, section, offset, cw as u32, ch as u32) };
    if status != 0 {
        missing(b"user32.dll!window surface (GalaWindowSurface failed)", u64::from(status));
    }
}

fn missing(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer; the service only reads it.
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, detail) };
}

/// Another process's window's surface, mapped here: kept between GetDC
/// calls while the surface's generation stays (a resize gives the window
/// a new section and generation).
#[derive(Clone, Copy)]
struct Mapped {
    hwnd: Hwnd,
    generation: u32,
    view: *mut c_void,
    section: Handle,
    pixels: *mut u32,
    desktop: u32,
    width: u32,
    height: u32,
    /// The client area's first row on the desktop's window (a menu bar's
    /// rows come first).
    bar: u32,
}

/// A DC on another process's window.
#[derive(Clone, Copy)]
struct Foreign {
    hdc: Handle,
    hwnd: Hwnd,
}

const NO_MAPPING: Mapped = Mapped {
    hwnd: 0,
    generation: 0,
    view: core::ptr::null_mut(),
    section: core::ptr::null_mut(),
    pixels: core::ptr::null_mut(),
    desktop: 0,
    width: 0,
    height: 0,
    bar: 0,
};
const NO_DC: Foreign = Foreign { hdc: core::ptr::null_mut(), hwnd: 0 };

const MAX_FOREIGN: usize = 16;
const MAX_MAPPED: usize = 8;

struct Tables {
    dcs: [Foreign; MAX_FOREIGN],
    mapped: [Mapped; MAX_MAPPED],
    /// The next mapping slot to reuse when all are taken.
    next: usize,
}

struct Table {
    lock: AtomicBool,
    tables: core::cell::UnsafeCell<Tables>,
}

// SAFETY: every access is under `lock`.
unsafe impl Sync for Table {}

static FOREIGN: Table = Table {
    lock: AtomicBool::new(false),
    tables: core::cell::UnsafeCell::new(Tables {
        dcs: [NO_DC; MAX_FOREIGN],
        mapped: [NO_MAPPING; MAX_MAPPED],
        next: 0,
    }),
};

fn with<R>(f: impl FnOnce(&mut Tables) -> R) -> R {
    while FOREIGN.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err()
    {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = f(unsafe { &mut *FOREIGN.tables.get() });
    FOREIGN.lock.store(false, Ordering::Release);
    r
}

fn unmap(m: &Mapped) {
    if m.view.is_null() {
        return;
    }
    // SAFETY: our view and handle, no longer used.
    unsafe {
        UnmapViewOfFile(m.view);
        CloseHandle(m.section);
    }
}

/// Window `hwnd`'s surface mapped here (the kept mapping while its
/// generation stays), or None.
fn mapping(hwnd: Hwnd) -> Option<Mapped> {
    let (mut section, mut raw) = (core::ptr::null_mut(), [0u8; 24]);
    // SAFETY: our outputs.
    if unsafe { GalaWindowSurfaceOpen(hwnd as u64, &mut section, &mut raw) } != 0 {
        // Gone, or never had one: forget a kept mapping.
        let gone = with(|t| {
            t.mapped.iter_mut().find(|m| m.hwnd == hwnd).map(|m| core::mem::replace(m, NO_MAPPING))
        });
        if let Some(m) = gone {
            unmap(&m);
        }
        missing(b"user32.dll!GetDC(another process's window without a surface)", 0);
        return None;
    }
    let offset = u64::from_le_bytes(raw[..8].try_into().unwrap_or([0; 8]));
    let word = |at: usize| u32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]]);
    let (width, height, desktop, generation) = (word(8), word(12), word(16), word(20));
    let kept =
        with(|t| t.mapped.iter().find(|m| m.hwnd == hwnd && m.generation == generation).copied());
    if let Some(m) = kept {
        // SAFETY: the handle just opened; the kept one maps the section.
        unsafe { CloseHandle(section) };
        return Some(m);
    }
    // SAFETY: a plain call on the handle just opened.
    let view = unsafe { MapViewOfFile(section, FILE_MAP_READ_WRITE, 0, 0, 0) };
    if view.is_null() || width == 0 {
        // SAFETY: our view and handle.
        unsafe {
            if !view.is_null() {
                UnmapViewOfFile(view);
            }
            CloseHandle(section);
        }
        return None;
    }
    let m = Mapped {
        hwnd,
        generation,
        view,
        section,
        // SAFETY: the client area starts `offset` bytes into the view.
        pixels: unsafe { view.cast::<u8>().add(offset as usize).cast::<u32>() },
        desktop,
        width,
        height,
        bar: (offset / 4 / u64::from(width)) as u32,
    };
    let old = with(|t| {
        let i = match t.mapped.iter().position(|x| x.hwnd == hwnd || x.view.is_null()) {
            Some(i) => i,
            None => {
                t.next = (t.next + 1) % MAX_MAPPED;
                t.next
            }
        };
        core::mem::replace(&mut t.mapped[i], m)
    });
    // A DC still drawing into a replaced mapping is not expected (one
    // thread paints a window); the old view goes.
    unmap(&old);
    Some(m)
}

/// GetDC of another process's top-level window: a DC over its surface
/// (mapped here) that accumulates what it draws (SetBoundsRect), or null.
pub(crate) fn foreign_dc(hwnd: Hwnd) -> Handle {
    let Some(info) = crate::foreign::query(hwnd) else { return core::ptr::null_mut() };
    if info.parent != 0 {
        missing(b"user32.dll!GetDC(another process's child window)", 0);
        return core::ptr::null_mut();
    }
    let Some(m) = mapping(hwnd) else { return core::ptr::null_mut() };
    let hdc = crate::window::surface_dc(m.pixels, m.width as i32, m.height as i32, m.width as i32);
    if hdc.is_null() {
        return hdc;
    }
    if let Some(g) = crate::window::gdi() {
        // DCB_ENABLE | DCB_RESET: ReleaseDC shows what was drawn.
        (g.set_bounds)(hdc, core::ptr::null(), 4 | 1);
    }
    let kept = with(|t| match t.dcs.iter_mut().find(|d| d.hdc.is_null()) {
        Some(slot) => {
            *slot = Foreign { hdc, hwnd };
            true
        }
        None => false,
    });
    if !kept {
        missing(b"user32.dll!GetDC(too many DCs on other processes' windows)", 0);
        crate::window::free_surface_dc(hdc);
        return core::ptr::null_mut();
    }
    hdc
}

/// ReleaseDC of a DC `foreign_dc` gave: what it drew shows (its
/// accumulated bounds); None if `hdc` is not one.
pub(crate) fn release_foreign(hwnd: Hwnd, hdc: Handle) -> Option<i32> {
    with(|t| {
        let slot = t.dcs.iter_mut().find(|d| d.hdc == hdc && d.hwnd == hwnd)?;
        *slot = NO_DC;
        Some(())
    })?;
    let m = with(|t| t.mapped.iter().find(|m| m.hwnd == hwnd).copied());
    let mut drawn = [0i32; 4];
    // DCB_SET (3): something was drawn.
    let some = crate::window::gdi().is_some_and(|g| (g.get_bounds)(hdc, &mut drawn, 0) & 3 == 3);
    if let Some(m) = m
        && some
        && m.desktop != 0
    {
        let [l, t, r, b] = [
            drawn[0].max(0),
            drawn[1].max(0),
            drawn[2].min(m.width as i32),
            drawn[3].min(m.height as i32),
        ];
        if l < r && t < b {
            // SAFETY: inside the surface's client area, `width` pixels per
            // row.
            unsafe {
                GalaWindowPresent(
                    m.desktop,
                    l,
                    t + m.bar as i32,
                    (r - l) as u32,
                    (b - t) as u32,
                    m.pixels.add(t as usize * m.width as usize + l as usize),
                    m.width as usize * 4,
                );
            }
        }
    }
    crate::window::free_surface_dc(hdc);
    Some(1)
}
