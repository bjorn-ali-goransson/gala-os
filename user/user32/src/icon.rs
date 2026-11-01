//! Icons and cursors: HICON objects holding a colour bitmap and a mask
//! (gdi32 bitmaps), a hotspot and whether it is an icon. CreateIconIndirect
//! keeps copies of the bitmaps, GetIconInfo hands out copies (the caller
//! deletes them), DrawIconEx draws through gdi32 (`GalaDrawIcon`: the
//! colour's alpha if it has any, else AND mask then XOR colour). System
//! icons and cursors (LoadIcon/LoadCursor with no module) are made once
//! and shared; programs' own come from their resources (RT_GROUP_ICON or
//! RT_GROUP_CURSOR, then the entry nearest the size asked) through
//! CreateIconFromResourceEx, which reads the BMP form (1-32 bits, the AND
//! mask after the colours); PNG-compressed entries are not supported yet.

use crate::window::Handle;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *const c_void;
    fn FindResourceW(module: usize, name: usize, kind: usize) -> usize;
    fn LoadResource(module: usize, res: usize) -> *const u16;
    fn SizeofResource(module: usize, res: usize) -> u32;
    fn VirtualAlloc(at: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    fn VirtualFree(at: *mut c_void, size: usize, kind: u32) -> i32;
}

/// A zeroed scratch buffer of `n` items of T (freed with `free`).
fn scratch<T>(n: usize) -> Option<&'static mut [T]> {
    // SAFETY: a plain call; zeroed memory is a valid [T] for the integer
    // and bool types used here.
    unsafe {
        let p =
            VirtualAlloc(core::ptr::null_mut(), n.max(1) * size_of::<T>(), 0x3000, 4).cast::<T>();
        (!p.is_null()).then(|| core::slice::from_raw_parts_mut(p, n))
    }
}

fn free<T>(s: &mut [T]) {
    // SAFETY: memory from `scratch`.
    unsafe { VirtualFree(s.as_mut_ptr().cast(), 0, 0x8000) };
}

const HICON_BASE: usize = 0x0004_0000;
const MAX_ICONS: usize = 256;

#[derive(Clone, Copy)]
struct Icon {
    used: bool,
    icon: bool,
    shared: bool,
    hotspot: (u32, u32),
    mask: usize,
    color: usize,
}

const NO_ICON: Icon =
    Icon { used: false, icon: false, shared: false, hotspot: (0, 0), mask: 0, color: 0 };

struct Table {
    lock: AtomicBool,
    icons: core::cell::UnsafeCell<[Icon; MAX_ICONS]>,
}

// SAFETY: every access is under `lock`.
unsafe impl Sync for Table {}

static TABLE: Table = Table {
    lock: AtomicBool::new(false),
    icons: core::cell::UnsafeCell::new([NO_ICON; MAX_ICONS]),
};

fn with<R>(f: impl FnOnce(&mut [Icon; MAX_ICONS]) -> R) -> R {
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = f(unsafe { &mut *TABLE.icons.get() });
    TABLE.lock.store(false, Ordering::Release);
    r
}

fn slot(h: usize) -> Option<usize> {
    let d = h.checked_sub(HICON_BASE)?;
    (d % 0x10 == 0 && d / 0x10 < MAX_ICONS).then_some(d / 0x10)
}

fn get(h: usize) -> Option<Icon> {
    let i = slot(h)?;
    with(|t| t[i].used.then_some(t[i]))
}

/// gdi32's function `name` (NUL-terminated), resolved once per slot.
fn gdi(cache: &AtomicUsize, name: &[u8]) -> usize {
    let f = cache.load(Ordering::Acquire);
    if f != 0 {
        return f;
    }
    let dll: [u16; 10] = core::array::from_fn(|i| u16::from(*b"gdi32.dll\0".get(i).unwrap_or(&0)));
    // SAFETY: a NUL-terminated name; GetProcAddress on a loaded module.
    let f = unsafe {
        let m = LoadLibraryW(dll.as_ptr());
        if m.is_null() { 0 } else { GetProcAddress(m, name.as_ptr()) as usize }
    };
    cache.store(f, Ordering::Release);
    f
}

macro_rules! gdi_fn {
    ($name:literal, $ty:ty) => {{
        static CACHE: AtomicUsize = AtomicUsize::new(0);
        let f = gdi(&CACHE, concat!($name, "\0").as_bytes());
        // SAFETY: gdi32's function of that name has this signature.
        (f != 0).then(|| unsafe { core::mem::transmute::<usize, $ty>(f) })
    }};
}

fn copy_bitmap(h: usize) -> usize {
    match gdi_fn!("GalaCopyBitmap", extern "system" fn(usize) -> usize) {
        Some(f) if h != 0 => f(h),
        _ => 0,
    }
}

fn delete_object(h: usize) {
    if let Some(f) = gdi_fn!("DeleteObject", extern "system" fn(usize) -> i32)
        && h != 0
    {
        f(h);
    }
}

/// GetDC(NULL): a DC on the screen (gdi32's display DC).
pub fn screen_dc() -> Handle {
    let display: [u16; 8] = core::array::from_fn(|i| u16::from(*b"DISPLAY\0".get(i).unwrap_or(&0)));
    type CreateDc = extern "system" fn(*const u16, *const u16, *const u16, *const c_void) -> Handle;
    match gdi_fn!("CreateDCW", CreateDc) {
        Some(f) => f(display.as_ptr(), core::ptr::null(), core::ptr::null(), core::ptr::null()),
        None => core::ptr::null_mut(),
    }
}

/// ICONINFO.
#[repr(C)]
pub struct IconInfo {
    icon: i32,
    x_hotspot: u32,
    y_hotspot: u32,
    mask: usize,
    color: usize,
}

fn new_icon(icon: Icon) -> usize {
    with(|t| match t.iter().position(|x| !x.used) {
        Some(i) => {
            t[i] = Icon { used: true, ..icon };
            HICON_BASE + i * 0x10
        }
        None => 0,
    })
}

/// CreateIconIndirect: an icon or cursor with copies of the bitmaps (a
/// mask is needed; without a colour bitmap it is monochrome, the mask
/// twice as tall).
#[unsafe(no_mangle)]
pub extern "system" fn CreateIconIndirect(info: *const IconInfo) -> usize {
    if info.is_null() {
        return 0;
    }
    // SAFETY: the caller's ICONINFO.
    let ii = unsafe { &*info };
    if ii.mask == 0 {
        crate::window::set_last_error(87);
        return 0;
    }
    let mask = copy_bitmap(ii.mask);
    let color = copy_bitmap(ii.color);
    if mask == 0 {
        return 0;
    }
    let h = new_icon(Icon {
        used: true,
        icon: ii.icon != 0,
        shared: false,
        hotspot: (ii.x_hotspot, ii.y_hotspot),
        mask,
        color,
    });
    if h == 0 {
        delete_object(mask);
        delete_object(color);
    }
    h
}

/// GetIconInfo: copies of the bitmaps, which the caller deletes.
#[unsafe(no_mangle)]
pub extern "system" fn GetIconInfo(h: usize, info: *mut IconInfo) -> i32 {
    let Some(icon) = get(h) else {
        crate::window::set_last_error(1402); // ERROR_INVALID_CURSOR_HANDLE
        return 0;
    };
    if info.is_null() {
        return 0;
    }
    // SAFETY: the caller's ICONINFO.
    unsafe {
        *info = IconInfo {
            icon: i32::from(icon.icon),
            x_hotspot: icon.hotspot.0,
            y_hotspot: icon.hotspot.1,
            mask: copy_bitmap(icon.mask),
            color: copy_bitmap(icon.color),
        };
    }
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn CopyIcon(h: usize) -> usize {
    let Some(icon) = get(h) else { return 0 };
    let ii = IconInfo {
        icon: i32::from(icon.icon),
        x_hotspot: icon.hotspot.0,
        y_hotspot: icon.hotspot.1,
        mask: icon.mask,
        color: icon.color,
    };
    CreateIconIndirect(&ii)
}

/// DestroyIcon: TRUE once (a shared icon stays).
#[unsafe(no_mangle)]
pub extern "system" fn DestroyIcon(h: usize) -> i32 {
    let Some(i) = slot(h) else { return 0 };
    let gone = with(|t| {
        if !t[i].used {
            return None;
        }
        if t[i].shared {
            return Some(None);
        }
        Some(Some(core::mem::replace(&mut t[i], NO_ICON)))
    });
    match gone {
        None => 0,
        Some(None) => 1,
        Some(Some(icon)) => {
            delete_object(icon.mask);
            delete_object(icon.color);
            1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn DestroyCursor(h: usize) -> i32 {
    DestroyIcon(h)
}

/// DrawIconEx(hdc, x, y, icon, cx, cy, step, brush, flags).
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn DrawIconEx(
    hdc: Handle,
    x: i32,
    y: i32,
    h: usize,
    cx: i32,
    cy: i32,
    _step: u32,
    _brush: Handle,
    flags: u32,
) -> i32 {
    let Some(icon) = get(h) else { return 0 };
    match gdi_fn!(
        "GalaDrawIcon",
        extern "system" fn(Handle, i32, i32, i32, i32, usize, usize, u32) -> i32
    ) {
        Some(f) => f(hdc, x, y, cx, cy, icon.color, icon.mask, flags),
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn DrawIcon(hdc: Handle, x: i32, y: i32, h: usize) -> i32 {
    DrawIconEx(hdc, x, y, h, 0, 0, 0, core::ptr::null_mut(), 3)
}

// ---- Making icons from pixels ----

/// An icon from top-down 0xAARRGGBB pixels and a mask (true: transparent).
fn from_pixels(
    w: i32,
    h: i32,
    pixels: &[u32],
    mask: &[bool],
    icon: bool,
    hotspot: (u32, u32),
    shared: bool,
) -> usize {
    type CreateDib =
        extern "system" fn(Handle, *const u8, u32, *mut *mut u32, Handle, u32) -> usize;
    type CreateBitmap = extern "system" fn(i32, i32, u32, u32, *const u8) -> usize;
    let (Some(dib), Some(bitmap)) =
        (gdi_fn!("CreateDIBSection", CreateDib), gdi_fn!("CreateBitmap", CreateBitmap))
    else {
        return 0;
    };
    // A top-down 32-bit BITMAPINFOHEADER.
    let mut bih = [0u8; 40];
    bih[0..4].copy_from_slice(&40u32.to_le_bytes());
    bih[4..8].copy_from_slice(&w.to_le_bytes());
    bih[8..12].copy_from_slice(&(-h).to_le_bytes());
    bih[12..14].copy_from_slice(&1u16.to_le_bytes());
    bih[14..16].copy_from_slice(&32u16.to_le_bytes());
    let mut bits: *mut u32 = core::ptr::null_mut();
    let color = dib(core::ptr::null_mut(), bih.as_ptr(), 0, &mut bits, core::ptr::null_mut(), 0);
    if color == 0 || bits.is_null() {
        return 0;
    }
    for (k, &p) in pixels.iter().enumerate().take((w * h) as usize) {
        // SAFETY: the DIB's w x h pixels.
        unsafe { *bits.add(k) = p };
    }
    // The mask: 1 bit per pixel, rows padded to 16 bits, set = transparent.
    let row = (((w + 15) / 16) * 2) as usize;
    let Some(m) = scratch::<u8>(row * h as usize) else {
        delete_object(color);
        return 0;
    };
    for y in 0..h as usize {
        for x in 0..w as usize {
            if mask.get(y * w as usize + x).copied().unwrap_or(true) {
                m[y * row + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    let mask_bmp = bitmap(w, h, 1, 1, m.as_ptr());
    free(m);
    let h_icon = new_icon(Icon { used: true, icon, shared, hotspot, mask: mask_bmp, color });
    if h_icon == 0 {
        delete_object(mask_bmp);
        delete_object(color);
    }
    h_icon
}

/// from_pixels with a byte mask (nonzero: transparent).
fn from_pixels_u8(
    w: i32,
    h: i32,
    pixels: &[u32],
    mask: &[u8],
    icon: bool,
    hotspot: (u32, u32),
) -> usize {
    let Some(m) = scratch::<bool>(mask.len()) else { return 0 };
    for (d, &b) in m.iter_mut().zip(mask) {
        *d = b != 0;
    }
    let h_icon = from_pixels(w, h, pixels, m, icon, hotspot, false);
    free(m);
    h_icon
}

/// The standard icons and cursors (shared): a soft blue square for the
/// icons, the arrow for every cursor (gala draws its own pointer).
fn system(icon: bool) -> usize {
    static ICON: AtomicUsize = AtomicUsize::new(0);
    static CURSOR: AtomicUsize = AtomicUsize::new(0);
    let cache = if icon { &ICON } else { &CURSOR };
    let h = cache.load(Ordering::Acquire);
    if h != 0 {
        return h;
    }
    let (Some(pixels), Some(mask)) = (scratch::<u32>(32 * 32), scratch::<bool>(32 * 32)) else {
        return 0;
    };
    mask.fill(true);
    if icon {
        for y in 4..28 {
            for x in 4..28 {
                pixels[y * 32 + x] = 0xff4d_7fe0;
                mask[y * 32 + x] = false;
            }
        }
    } else {
        const ARROW: [&[u8; 12]; 19] = [
            b"#           ",
            b"##          ",
            b"#.#         ",
            b"#..#        ",
            b"#...#       ",
            b"#....#      ",
            b"#.....#     ",
            b"#......#    ",
            b"#.......#   ",
            b"#........#  ",
            b"#.........# ",
            b"#..........#",
            b"#......#####",
            b"#...#..#    ",
            b"#..# #..#   ",
            b"#.#  #..#   ",
            b"##    #..#  ",
            b"      #..#  ",
            b"       ##   ",
        ];
        for (y, row) in ARROW.iter().enumerate() {
            for (x, &c) in row.iter().enumerate() {
                if c != b' ' {
                    pixels[y * 32 + x] = if c == b'#' { 0xff00_0000 } else { 0xffff_ffff };
                    mask[y * 32 + x] = false;
                }
            }
        }
    }
    let new = from_pixels(32, 32, pixels, mask, icon, (0, 0), true);
    free(pixels);
    free(mask);
    match cache.compare_exchange(0, new, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => new,
        Err(other) => {
            DestroyIcon(new);
            other
        }
    }
}

/// CreateIconFromResourceEx(bits, size, icon, version, cx, cy, flags): an
/// icon or cursor from an RT_ICON/RT_CURSOR resource's data (a cursor's
/// starts with its hotspot): a BITMAPINFOHEADER of twice the height, the
/// palette, the colours bottom-up, then the AND mask.
#[unsafe(no_mangle)]
pub extern "system" fn CreateIconFromResourceEx(
    bits: *const u8,
    size: u32,
    icon: i32,
    _version: u32,
    _cx: i32,
    _cy: i32,
    _flags: u32,
) -> usize {
    if bits.is_null() || size < 40 {
        return 0;
    }
    // SAFETY: the caller's resource data of `size` bytes.
    let data = unsafe { core::slice::from_raw_parts(bits, size as usize) };
    let (hotspot, data) = if icon == 0 {
        let hx = u16::from_le_bytes([data[0], data[1]]);
        let hy = u16::from_le_bytes([data[2], data[3]]);
        ((u32::from(hx), u32::from(hy)), &data[4..])
    } else {
        ((0, 0), data)
    };
    if data.starts_with(b"\x89PNG") {
        crate::window::unsupported(b"user32.dll!CreateIconFromResourceEx(PNG)", 0);
        return 0;
    }
    let u32_at = |at: usize| {
        data.get(at..at + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let header = u32_at(0) as usize;
    let w = u32_at(4) as i32;
    let h = (u32_at(8) as i32) / 2;
    let bpp = data.get(14..16).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
    let used = u32_at(32);
    if !(1..=256).contains(&w) || !(1..=256).contains(&h) || ![1, 4, 8, 24, 32].contains(&bpp) {
        return 0;
    }
    let colors = if bpp <= 8 { if used != 0 { used as usize } else { 1 << bpp } } else { 0 };
    let palette = header;
    let xor = palette + 4 * colors;
    let xor_row = (((w as usize) * bpp as usize).div_ceil(32)) * 4;
    let and = xor + xor_row * h as usize;
    let and_row = (w as usize).div_ceil(32) * 4;
    if data.len() < and + and_row * h as usize {
        return 0;
    }
    let wu = w as usize;
    let (Some(pixels), Some(mask)) =
        (scratch::<u32>(wu * h as usize), scratch::<u8>(wu * h as usize))
    else {
        return 0;
    };
    for y in 0..h as usize {
        // Bottom-up rows.
        let src = (h as usize - 1 - y) * xor_row + xor;
        let msrc = (h as usize - 1 - y) * and_row + and;
        for x in 0..wu {
            let p = match bpp {
                32 => u32_at(src + 4 * x),
                24 => {
                    u32::from(data[src + 3 * x])
                        | u32::from(data[src + 3 * x + 1]) << 8
                        | u32::from(data[src + 3 * x + 2]) << 16
                }
                _ => {
                    let bit = x * bpp as usize;
                    let byte = data[src + bit / 8];
                    let index = (byte >> (8 - bpp as usize - bit % 8)) & ((1u16 << bpp) - 1) as u8;
                    u32_at(palette + 4 * index as usize) & 0x00ff_ffff
                }
            };
            pixels[y * wu + x] = p;
            mask[y * wu + x] = u8::from(data[msrc + x / 8] & (0x80 >> (x % 8)) != 0);
        }
    }
    let h_icon = from_pixels_u8(w, h, pixels, mask, icon != 0, hotspot);
    free(pixels);
    free(mask);
    h_icon
}

/// The resource entry of a group (RT_GROUP_ICON or RT_GROUP_CURSOR)
/// nearest `want` pixels: its RT_ICON/RT_CURSOR id.
fn pick(module: Handle, name: *const u16, group: usize, want: i32) -> Option<u16> {
    // SAFETY: kernel32's resource functions on the caller's module.
    unsafe {
        let r = FindResourceW(module as usize, name as usize, group);
        if r == 0 {
            return None;
        }
        let n = SizeofResource(module as usize, r) as usize;
        let p = LoadResource(module as usize, r).cast::<u8>();
        if p.is_null() || n < 6 {
            return None;
        }
        let d = core::slice::from_raw_parts(p, n);
        let count = u16::from_le_bytes([d[4], d[5]]) as usize;
        let mut best: Option<(i32, u16)> = None;
        for k in 0..count {
            let e = 6 + 14 * k;
            let Some(entry) = d.get(e..e + 14) else { break };
            let size = if entry[0] == 0 { 256 } else { i32::from(entry[0]) };
            let id = u16::from_le_bytes([entry[12], entry[13]]);
            let score = (size - want).abs();
            if best.is_none_or(|(b, _)| score < b) {
                best = Some((score, id));
            }
        }
        best.map(|(_, id)| id)
    }
}

/// LoadImageW(module, name, type, cx, cy, flags): IMAGE_ICON (1) and
/// IMAGE_CURSOR (2) from a module's resources, or the standard ones
/// (no module, an OIC_/OCR_ id); bitmaps and files are not supported yet.
#[unsafe(no_mangle)]
pub extern "system" fn LoadImageW(
    module: Handle,
    name: *const u16,
    kind: u32,
    cx: i32,
    cy: i32,
    flags: u32,
) -> usize {
    let _ = cy;
    if kind > 2 || kind == 0 || flags & 0x10 != 0 {
        crate::window::unsupported(
            b"user32.dll!LoadImageW(type)",
            u64::from(kind) | u64::from(flags) << 8,
        );
        return 0;
    }
    let icon = kind == 1;
    if module.is_null() {
        return system(icon);
    }
    let want = if cx > 0 { cx } else { 32 };
    let (group, single) = if icon { (14usize, 3usize) } else { (12, 1) };
    let Some(id) = pick(module, name, group, want) else {
        crate::window::set_last_error(1813); // ERROR_RESOURCE_TYPE_NOT_FOUND
        return 0;
    };
    // SAFETY: kernel32's resource functions on the caller's module.
    unsafe {
        let r = FindResourceW(module as usize, usize::from(id), single);
        if r == 0 {
            return 0;
        }
        let n = SizeofResource(module as usize, r);
        let p = LoadResource(module as usize, r).cast::<u8>();
        CreateIconFromResourceEx(p, n, i32::from(icon), 0x30000, cx, cy, 0)
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn LoadIconW(module: Handle, name: *const u16) -> usize {
    if module.is_null() {
        return system(true);
    }
    match LoadImageW(module, name, 1, 32, 32, 0) {
        0 => {
            // An unknown name in the program's own module: as Windows, NULL.
            0
        }
        h => h,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn LoadCursorW(module: Handle, name: *const u16) -> usize {
    if module.is_null() {
        return system(false);
    }
    LoadImageW(module, name, 2, 32, 32, 0)
}

#[unsafe(no_mangle)]
pub extern "system" fn LoadIconA(module: Handle, name: *const u8) -> usize {
    let mut w = [0u16; 64];
    LoadIconW(module, crate::window::ansi_into_ptr(name, &mut w))
}

#[unsafe(no_mangle)]
pub extern "system" fn LoadCursorA(module: Handle, name: *const u8) -> usize {
    let mut w = [0u16; 64];
    LoadCursorW(module, crate::window::ansi_into_ptr(name, &mut w))
}
