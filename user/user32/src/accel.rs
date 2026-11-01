//! Accelerator tables (keyboard shortcuts): HACCEL objects of ACCEL
//! entries (fVirt flags, key, command). TranslateAcceleratorW turns a
//! matching WM_KEYDOWN/WM_SYSKEYDOWN (FVIRTKEY entries: the virtual key
//! with Shift/Ctrl/Alt as held now) or WM_CHAR (character entries) into
//! WM_COMMAND (high word 1) to the window. LoadAcceleratorsW reads an
//! RT_ACCELERATOR resource.

use crate::window::{Hwnd, Msg, SendMessageW};
use core::sync::atomic::{AtomicBool, Ordering};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn FindResourceW(module: usize, name: usize, kind: usize) -> usize;
    fn LoadResource(module: usize, res: usize) -> *const u16;
    fn SizeofResource(module: usize, res: usize) -> u32;
}

const MAX_TABLES: usize = 32;
const MAX_ENTRIES: usize = 128;
const HACCEL_BASE: usize = 0x0005_0000;

const FVIRTKEY: u8 = 0x01;
const FSHIFT: u8 = 0x04;
const FCONTROL: u8 = 0x08;
const FALT: u8 = 0x10;

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Accel {
    flags: u8,
    key: u16,
    cmd: u16,
}

const NO_ACCEL: Accel = Accel { flags: 0, key: 0, cmd: 0 };

struct Table {
    lock: AtomicBool,
    tables: core::cell::UnsafeCell<[(bool, usize, [Accel; MAX_ENTRIES]); MAX_TABLES]>,
}

// SAFETY: every access is under `lock`.
unsafe impl Sync for Table {}

static TABLE: Table = Table {
    lock: AtomicBool::new(false),
    tables: core::cell::UnsafeCell::new([(false, 0, [NO_ACCEL; MAX_ENTRIES]); MAX_TABLES]),
};

fn with<R>(f: impl FnOnce(&mut [(bool, usize, [Accel; MAX_ENTRIES]); MAX_TABLES]) -> R) -> R {
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = f(unsafe { &mut *TABLE.tables.get() });
    TABLE.lock.store(false, Ordering::Release);
    r
}

fn slot(h: usize) -> Option<usize> {
    let d = h.checked_sub(HACCEL_BASE)?;
    (d % 0x10 == 0 && d / 0x10 < MAX_TABLES).then_some(d / 0x10)
}

fn create(entries: impl Iterator<Item = Accel>) -> usize {
    with(|t| {
        let Some(i) = t.iter().position(|e| !e.0) else { return 0 };
        let mut n = 0;
        let mut list = [NO_ACCEL; MAX_ENTRIES];
        for e in entries.take(MAX_ENTRIES) {
            list[n] = e;
            n += 1;
        }
        t[i] = (true, n, list);
        HACCEL_BASE + i * 0x10
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateAcceleratorTableW(entries: *const Accel, n: i32) -> usize {
    if entries.is_null() || n <= 0 {
        crate::window::set_last_error(87);
        return 0;
    }
    // SAFETY: the caller's `n` ACCELs.
    let list = unsafe { core::slice::from_raw_parts(entries, n as usize) };
    create(list.iter().copied())
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateAcceleratorTableA(entries: *const Accel, n: i32) -> usize {
    CreateAcceleratorTableW(entries, n)
}

#[unsafe(no_mangle)]
pub extern "system" fn DestroyAcceleratorTable(h: usize) -> i32 {
    let Some(i) = slot(h) else { return 0 };
    with(|t| i32::from(core::mem::replace(&mut t[i].0, false)))
}

/// CopyAcceleratorTableW(h, dst, n): with no buffer, the entry count; else
/// the entries copied.
#[unsafe(no_mangle)]
pub extern "system" fn CopyAcceleratorTableW(h: usize, dst: *mut Accel, n: i32) -> i32 {
    let Some(i) = slot(h) else { return 0 };
    with(|t| {
        if !t[i].0 {
            return 0;
        }
        let count = t[i].1;
        if dst.is_null() {
            return count as i32;
        }
        let k = count.min(n.max(0) as usize);
        for (j, e) in t[i].2.iter().take(k).enumerate() {
            // SAFETY: the caller's `n` entries.
            unsafe { *dst.add(j) = *e };
        }
        k as i32
    })
}

/// LoadAcceleratorsW: an RT_ACCELERATOR resource (8-byte entries, 0x80 in
/// the flags of the last).
#[unsafe(no_mangle)]
pub extern "system" fn LoadAcceleratorsW(module: usize, name: *const u16) -> usize {
    // SAFETY: kernel32's resource functions on the caller's module.
    let data = unsafe {
        let r = FindResourceW(module, name as usize, 9);
        if r == 0 {
            return 0;
        }
        let n = SizeofResource(module, r) as usize;
        let p = LoadResource(module, r).cast::<u8>();
        if p.is_null() {
            return 0;
        }
        core::slice::from_raw_parts(p, n)
    };
    let mut end = false;
    create(
        data.chunks_exact(8)
            .take_while(|e| {
                let more = !end;
                end = e[0] & 0x80 != 0;
                more
            })
            .map(|e| Accel {
                flags: e[0] & 0x7f,
                key: u16::from_le_bytes([e[2], e[3]]),
                cmd: u16::from_le_bytes([e[4], e[5]]),
            }),
    )
}

const WM_KEYDOWN: u32 = 0x0100;
const WM_CHAR: u32 = 0x0102;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_SYSCHAR: u32 = 0x0106;
const WM_COMMAND: u32 = 0x0111;

fn held(vk: i32) -> bool {
    crate::window::GetKeyState(vk) < 0
}

/// TranslateAcceleratorW: TRUE when the message was a shortcut (and
/// WM_COMMAND went to the window).
#[unsafe(no_mangle)]
pub extern "system" fn TranslateAcceleratorW(hwnd: Hwnd, h: usize, msg: *const Msg) -> i32 {
    let Some(i) = slot(h) else { return 0 };
    if msg.is_null() {
        return 0;
    }
    // SAFETY: the caller's MSG.
    let m = unsafe { *msg };
    let virt = matches!(m.message, WM_KEYDOWN | WM_SYSKEYDOWN);
    let char = matches!(m.message, WM_CHAR | WM_SYSCHAR);
    if !virt && !char {
        return 0;
    }
    let mods = (if held(0x10) { FSHIFT } else { 0 })
        | (if held(0x11) { FCONTROL } else { 0 })
        | (if held(0x12) { FALT } else { 0 });
    let cmd = with(|t| {
        if !t[i].0 {
            return None;
        }
        t[i].2
            .iter()
            .take(t[i].1)
            .find(|e| {
                if virt {
                    e.flags & FVIRTKEY != 0
                        && usize::from(e.key) == m.wparam & 0xffff
                        && e.flags & (FSHIFT | FCONTROL | FALT) == mods
                } else {
                    e.flags & FVIRTKEY == 0
                        && usize::from(e.key) == m.wparam & 0xffff
                        && (e.flags & FALT != 0) == (m.message == WM_SYSCHAR)
                }
            })
            .map(|e| e.cmd)
    });
    match cmd {
        Some(c) => {
            SendMessageW(hwnd, WM_COMMAND, 0x1_0000 | usize::from(c), 0);
            1
        }
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn TranslateAcceleratorA(hwnd: Hwnd, h: usize, msg: *const Msg) -> i32 {
    TranslateAcceleratorW(hwnd, h, msg)
}
