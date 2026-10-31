//! Toolhelp snapshots (CreateToolhelp32Snapshot, Process32*, Thread32*),
//! as on Windows on top of NtQuerySystemInformation(SystemProcessInformation).
//! The snapshot is a section holding a header and that list (copied from
//! the query buffer); the First/Next functions keep their place in the
//! header, so the handle is all a caller needs.

use super::mapping::{CreateFileMappingW, MapViewOfFile, UnmapViewOfFile};
use super::*;

const TH32CS_SNAPPROCESS: u32 = 2;
const TH32CS_SNAPTHREAD: u32 = 4;
const SYSTEM_PROCESS_INFORMATION: u32 = 5;
const STATUS_INFO_LENGTH_MISMATCH: u32 = 0xc000_0004;
const ERROR_NO_MORE_FILES: u32 = 18;
const ERROR_BAD_LENGTH: u32 = 24;
const MAGIC: u32 = u32::from_le_bytes(*b"GTH1");

/// The snapshot's header; the list follows it.
#[repr(C)]
struct Header {
    magic: u32,
    flags: u32,
    /// Where the list was queried: ImageName pointers are relative to it.
    query_base: u64,
    /// The list's size in bytes.
    len: u32,
    /// Offset of the next process entry for Process32Next (u32::MAX: done).
    next_process: u32,
    /// The process entry and thread index for Thread32Next.
    thread_process: u32,
    next_thread: u32,
}

const HEADER: usize = size_of::<Header>();
/// SYSTEM_PROCESS_INFORMATION's size; SYSTEM_THREAD_INFORMATION's.
const PROCESS_ENTRY: usize = 0x100;
const THREAD_ENTRY: usize = 0x50;

/// CreateToolhelp32Snapshot(flags, process): the processes and threads
/// (TH32CS_SNAPPROCESS, TH32CS_SNAPTHREAD) as they are now. Modules and
/// heaps are not listed (their First functions find nothing).
#[unsafe(no_mangle)]
pub extern "system" fn CreateToolhelp32Snapshot(flags: u32, _process: u32) -> Handle {
    // Query with a growing buffer (processes may start in between).
    let mut size = 64 * 1024usize;
    let (list, len) = loop {
        let buf = VirtualAlloc(core::ptr::null_mut(), size, 0x3000, 4).cast::<u8>();
        if buf.is_null() {
            return fail_with(ERROR_NOT_ENOUGH_MEMORY, INVALID_HANDLE_VALUE);
        }
        let mut needed = 0u32;
        // SAFETY: our buffer of `size` bytes.
        let status = unsafe {
            NtQuerySystemInformation(SYSTEM_PROCESS_INFORMATION, buf, size as u32, &mut needed)
        };
        if status == STATUS_SUCCESS {
            break (buf, needed as usize);
        }
        VirtualFree(buf.cast(), 0, 0x8000);
        if status != STATUS_INFO_LENGTH_MISMATCH || size >= 16 << 20 {
            return fail(status, INVALID_HANDLE_VALUE);
        }
        size = (needed as usize + 4096).max(size * 2);
    };
    let total = HEADER + len;
    let section = CreateFileMappingW(
        INVALID_HANDLE_VALUE,
        core::ptr::null_mut(),
        4, // PAGE_READWRITE
        0,
        total as u32,
        core::ptr::null(),
    );
    if section.is_null() {
        VirtualFree(list.cast(), 0, 0x8000);
        return INVALID_HANDLE_VALUE;
    }
    let view = MapViewOfFile(section, 6, 0, 0, total).cast::<u8>(); // FILE_MAP_READ | WRITE
    if view.is_null() {
        VirtualFree(list.cast(), 0, 0x8000);
        CloseHandle(section);
        return INVALID_HANDLE_VALUE;
    }
    let header = Header {
        magic: MAGIC,
        flags,
        query_base: list as u64,
        len: len as u32,
        next_process: 0,
        thread_process: 0,
        next_thread: 0,
    };
    // SAFETY: the view holds `total` bytes; the query wrote `len`.
    unsafe {
        view.cast::<Header>().write(header);
        core::ptr::copy_nonoverlapping(list, view.add(HEADER), len);
    }
    UnmapViewOfFile(view.cast());
    VirtualFree(list.cast(), 0, 0x8000);
    section
}

/// Runs `f` on a snapshot's header and list (mapped for the call).
fn with_snapshot<R>(snapshot: Handle, f: impl FnOnce(&mut Header, &[u8]) -> R) -> Option<R> {
    let view = MapViewOfFile(snapshot, 6, 0, 0, 0).cast::<u8>();
    if view.is_null() {
        return None;
    }
    // SAFETY: a snapshot section starts with its header, then `len` bytes.
    let r = unsafe {
        let header = &mut *view.cast::<Header>();
        if header.magic != MAGIC {
            None
        } else {
            let list = core::slice::from_raw_parts(view.add(HEADER), header.len as usize);
            Some(f(header, list))
        }
    };
    UnmapViewOfFile(view.cast());
    r
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |v| u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    b.get(at..at + 8).map_or(0, |v| u64::from_le_bytes(v.try_into().unwrap_or([0; 8])))
}

/// What Process32 reports of the entry at `at`: (id, threads, parent,
/// base priority) and the image name (UTF-16).
struct ProcessInfo<'a> {
    id: u32,
    threads: u32,
    parent: u32,
    priority: u32,
    name: &'a [u8],
}

fn process_at<'a>(header: &Header, list: &'a [u8], at: usize) -> ProcessInfo<'a> {
    let name_len = usize::from(u32_at(list, at + 0x38) as u16);
    let name_ptr = u64_at(list, at + 0x40);
    let name = name_ptr
        .checked_sub(header.query_base)
        .and_then(|off| list.get(off as usize..off as usize + name_len))
        .unwrap_or(&[]);
    ProcessInfo {
        id: u64_at(list, at + 0x50) as u32,
        threads: u32_at(list, at + 4),
        parent: u64_at(list, at + 0x58) as u32,
        priority: u32_at(list, at + 0x48),
        name,
    }
}

/// The next process entry of the snapshot (from the start if `first`).
fn next_process<R>(
    snapshot: Handle,
    first: bool,
    f: impl FnOnce(&ProcessInfo<'_>) -> R,
) -> Option<R> {
    with_snapshot(snapshot, |header, list| {
        if header.flags & TH32CS_SNAPPROCESS == 0 {
            return None;
        }
        if first {
            header.next_process = 0;
        }
        let at = header.next_process as usize;
        if at == u32::MAX as usize || at + PROCESS_ENTRY > list.len() {
            return None;
        }
        let next = u32_at(list, at) as usize;
        header.next_process = if next == 0 { u32::MAX } else { (at + next) as u32 };
        Some(f(&process_at(header, list, at)))
    })
    .flatten()
}

/// Fills PROCESSENTRY32W/A (`wide`): dwSize, cntUsage, th32ProcessID (8),
/// th32DefaultHeapID (16), th32ModuleID (24), cntThreads (28),
/// th32ParentProcessID (32), pcPriClassBase (36), dwFlags (40),
/// szExeFile (44, 260 characters).
fn fill_process(entry: *mut u8, wide: bool, p: &ProcessInfo<'_>) {
    // SAFETY: the caller's entry, at least as large as its dwSize says
    // (checked to cover szExeFile).
    unsafe {
        let size = *entry.cast::<u32>() as usize;
        core::ptr::write_bytes(entry.add(4), 0, size - 4);
        let put = |at: usize, v: u32| entry.add(at).cast::<u32>().write_unaligned(v);
        put(8, p.id);
        put(28, p.threads);
        put(32, p.parent);
        put(36, p.priority);
        // The idle process has no image: Windows calls it this.
        let idle: &[u8] = b"[System Process]";
        let units = p.name.len() / 2;
        let name = entry.add(44);
        for i in 0..259 {
            let c = if p.name.is_empty() {
                match idle.get(i) {
                    Some(&b) => u16::from(b),
                    None => break,
                }
            } else if i < units {
                u16::from_le_bytes([p.name[2 * i], p.name[2 * i + 1]])
            } else {
                break;
            };
            if wide {
                name.cast::<u16>().add(i).write_unaligned(c);
            } else {
                // ASCII file names (gala's images); others become '?'.
                *name.add(i) = if c < 0x80 { c as u8 } else { b'?' };
            }
        }
    }
}

fn process32(snapshot: Handle, entry: *mut u8, first: bool, wide: bool) -> Bool {
    // SAFETY: the caller's entry starts with dwSize.
    let size = if entry.is_null() { 0 } else { unsafe { *entry.cast::<u32>() } };
    let needed = if wide { 44 + 2 * 260 } else { 44 + 260 };
    if (size as usize) < needed {
        return fail_with(ERROR_BAD_LENGTH, FALSE);
    }
    match next_process(snapshot, first, |p| fill_process(entry, wide, p)) {
        Some(()) => TRUE,
        None => fail_with(ERROR_NO_MORE_FILES, FALSE),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Process32FirstW(snapshot: Handle, entry: *mut u8) -> Bool {
    process32(snapshot, entry, true, true)
}

#[unsafe(no_mangle)]
pub extern "system" fn Process32NextW(snapshot: Handle, entry: *mut u8) -> Bool {
    process32(snapshot, entry, false, true)
}

#[unsafe(no_mangle)]
pub extern "system" fn Process32First(snapshot: Handle, entry: *mut u8) -> Bool {
    process32(snapshot, entry, true, false)
}

#[unsafe(no_mangle)]
pub extern "system" fn Process32Next(snapshot: Handle, entry: *mut u8) -> Bool {
    process32(snapshot, entry, false, false)
}

/// The next thread of the snapshot: (thread id, owner, base priority).
fn next_thread(snapshot: Handle, first: bool) -> Option<(u32, u32, u32)> {
    with_snapshot(snapshot, |header, list| {
        if header.flags & TH32CS_SNAPTHREAD == 0 {
            return None;
        }
        if first {
            header.thread_process = 0;
            header.next_thread = 0;
        }
        loop {
            let at = header.thread_process as usize;
            if at == u32::MAX as usize || at + PROCESS_ENTRY > list.len() {
                return None;
            }
            let i = header.next_thread as usize;
            if i < u32_at(list, at + 4) as usize {
                header.next_thread += 1;
                let t = at + PROCESS_ENTRY + THREAD_ENTRY * i;
                return Some((
                    u64_at(list, t + 0x30) as u32,
                    u64_at(list, t + 0x28) as u32,
                    u32_at(list, t + 0x3c),
                ));
            }
            let next = u32_at(list, at) as usize;
            header.thread_process = if next == 0 { u32::MAX } else { (at + next) as u32 };
            header.next_thread = 0;
        }
    })
    .flatten()
}

/// Fills THREADENTRY32: dwSize, cntUsage, th32ThreadID, th32OwnerProcessID,
/// tpBasePri, tpDeltaPri, dwFlags.
fn thread32(snapshot: Handle, entry: *mut u32, first: bool) -> Bool {
    // SAFETY: the caller's entry starts with dwSize.
    if entry.is_null() || unsafe { *entry } < 28 {
        return fail_with(ERROR_BAD_LENGTH, FALSE);
    }
    let Some((id, owner, priority)) = next_thread(snapshot, first) else {
        return fail_with(ERROR_NO_MORE_FILES, FALSE);
    };
    // SAFETY: the caller's THREADENTRY32 (28 bytes, checked).
    unsafe {
        let fields = [0, id, owner, priority, 0, 0];
        for (i, v) in fields.into_iter().enumerate() {
            *entry.add(1 + i) = v;
        }
    }
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn Thread32First(snapshot: Handle, entry: *mut u32) -> Bool {
    thread32(snapshot, entry, true)
}

#[unsafe(no_mangle)]
pub extern "system" fn Thread32Next(snapshot: Handle, entry: *mut u32) -> Bool {
    thread32(snapshot, entry, false)
}
