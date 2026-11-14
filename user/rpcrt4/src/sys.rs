//! What the runtime takes from kernel32 and ntdll, and small helpers over
//! it: a lock, threads, the process heap, wide strings, the ledger.

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::ffi::c_void;

pub type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    pub fn GetProcessHeap() -> Handle;
    pub fn HeapAlloc(heap: Handle, flags: u32, size: usize) -> *mut c_void;
    pub fn HeapFree(heap: Handle, flags: u32, p: *mut c_void) -> i32;
    pub fn AcquireSRWLockExclusive(lock: *mut usize);
    pub fn ReleaseSRWLockExclusive(lock: *mut usize);
    pub fn CreateThread(
        attributes: usize,
        stack: usize,
        start: unsafe extern "system" fn(*mut c_void) -> u32,
        parameter: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    pub fn CreateEventW(attributes: usize, manual: i32, initial: i32, name: *const u16) -> Handle;
    pub fn SetEvent(h: Handle) -> i32;
    pub fn ResetEvent(h: Handle) -> i32;
    pub fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    pub fn CloseHandle(h: Handle) -> i32;
    pub fn CreateNamedPipeW(
        name: *const u16,
        open_mode: u32,
        pipe_mode: u32,
        max_instances: u32,
        out_size: u32,
        in_size: u32,
        timeout: u32,
        attributes: usize,
    ) -> Handle;
    pub fn ConnectNamedPipe(pipe: Handle, overlapped: usize) -> i32;
    pub fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        attributes: usize,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    pub fn SetNamedPipeHandleState(
        pipe: Handle,
        mode: *const u32,
        max_collection: *const u32,
        timeout: *const u32,
    ) -> i32;
    pub fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, overlapped: usize) -> i32;
    pub fn WriteFile(
        h: Handle,
        buf: *const u8,
        len: u32,
        written: *mut u32,
        overlapped: usize,
    ) -> i32;
    pub fn GetLastError() -> u32;
    pub fn TlsAlloc() -> u32;
    pub fn TlsGetValue(index: u32) -> *mut c_void;
    pub fn TlsSetValue(index: u32, value: *mut c_void) -> i32;
    pub fn RaiseException(code: u32, flags: u32, count: u32, arguments: *const usize);
    pub fn LoadLibraryW(name: *const u16) -> Handle;
    pub fn GetProcAddress(module: Handle, name: *const u8) -> *const c_void;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
    pub fn NtFsControlFile(
        h: Handle,
        event: Handle,
        apc: usize,
        context: usize,
        iosb: *mut [usize; 2],
        code: u32,
        input: *const u8,
        input_length: u32,
        output: *mut u8,
        output_length: u32,
    ) -> u32;
    pub fn NtSetInformationThread(
        thread: Handle,
        class: u32,
        info: *const c_void,
        length: u32,
    ) -> u32;
    pub fn NtOpenThreadToken(thread: Handle, access: u32, as_self: u8, token: *mut Handle) -> u32;
    pub fn NtQueryInformationToken(
        token: Handle,
        class: u32,
        info: *mut c_void,
        length: u32,
        returned: *mut u32,
    ) -> u32;
    pub fn NtClose(h: Handle) -> u32;
    pub fn RtlNtStatusToDosError(status: u32) -> u32;
}

pub const INVALID_HANDLE: Handle = usize::MAX;
pub const CURRENT_THREAD: Handle = usize::MAX - 1;
pub const INFINITE: u32 = u32::MAX;

/// Records an unsupported case in the ledger ("unsupported").
pub fn unsupported(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, detail) };
}

/// Records a notable answer in the ledger ("other").
pub fn note(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 5, detail) };
}

/// `prefix` then `text` (UTF-16, cut to fit) for a ledger line.
pub fn note_with(prefix: &[u8], text: &[u16], unsupported_case: bool, detail: u64) {
    let mut line = [0u8; 96];
    let mut n = 0;
    for &b in prefix {
        if n < line.len() {
            line[n] = b;
            n += 1;
        }
    }
    for &c in text {
        if n + 1 >= line.len() {
            break;
        }
        line[n] = if c < 0x80 { c as u8 } else { b'?' };
        n += 1;
    }
    if n < line.len() {
        line[n] = b')';
        n += 1;
    }
    if unsupported_case { unsupported(&line[..n], detail) } else { note(&line[..n], detail) }
}

/// A lock over `T` (an SRW lock, held briefly, never across a call out).
pub struct Lock<T> {
    srw: UnsafeCell<usize>,
    value: UnsafeCell<T>,
}

// SAFETY: the SRW lock serializes every access to `value`.
unsafe impl<T: Send> Sync for Lock<T> {}

impl<T> Lock<T> {
    pub const fn new(value: T) -> Self {
        Self { srw: UnsafeCell::new(0), value: UnsafeCell::new(value) }
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        // SAFETY: the SRW lock gives this closure the only access.
        unsafe {
            AcquireSRWLockExclusive(self.srw.get());
            let r = f(&mut *self.value.get());
            ReleaseSRWLockExclusive(self.srw.get());
            r
        }
    }
}

/// Starts a thread running `f(arg)`; false if it could not.
pub fn spawn(f: unsafe extern "system" fn(*mut c_void) -> u32, arg: *mut c_void) -> bool {
    // SAFETY: a thread with the default stack; its handle is not kept.
    let h = unsafe { CreateThread(0, 0, f, arg, 0, core::ptr::null_mut()) };
    if h == 0 {
        return false;
    }
    // SAFETY: our handle.
    unsafe { CloseHandle(h) };
    true
}

/// `n` bytes on the process heap (null on failure).
pub fn heap_alloc(n: usize) -> *mut u8 {
    // SAFETY: plain call.
    unsafe { HeapAlloc(GetProcessHeap(), 0, n.max(1)).cast() }
}

pub fn heap_free(p: *mut u8) {
    if !p.is_null() {
        // SAFETY: a block of heap_alloc.
        unsafe { HeapFree(GetProcessHeap(), 0, p.cast()) };
    }
}

/// The length of a NUL-terminated string (null: 0).
///
/// # Safety
/// `p` is null or NUL-terminated.
pub unsafe fn strlen<T: Copy + Default + PartialEq>(p: *const T) -> usize {
    if p.is_null() {
        return 0;
    }
    let mut n = 0;
    // SAFETY: up to the terminator, as the caller promises.
    while unsafe { *p.add(n) } != T::default() {
        n += 1;
    }
    n
}

/// A NUL-terminated string's characters (without the NUL), widened.
///
/// # Safety
/// `p` is null or NUL-terminated.
pub unsafe fn wide_from<T: Copy + Default + PartialEq + Into<u32>>(
    p: *const T,
) -> Option<Vec<u16>> {
    // SAFETY: as the caller promises.
    let n = unsafe { strlen(p) };
    let mut v = Vec::new();
    v.try_reserve_exact(n + 1).ok()?;
    for i in 0..n {
        // SAFETY: within the string.
        let c: u32 = unsafe { *p.add(i) }.into();
        v.push(c as u16);
    }
    Some(v)
}

/// `text` as a new NUL-terminated string of `T` on the process heap
/// (RpcStringFree frees it); null on failure.
pub fn heap_string<T: Copy + From<u8> + TryFrom<u16>>(text: &[u16]) -> *mut T {
    let p = heap_alloc((text.len() + 1) * size_of::<T>()).cast::<T>();
    if p.is_null() {
        return p;
    }
    for (i, &c) in text.iter().enumerate() {
        let v = T::try_from(c).unwrap_or(T::from(b'?'));
        // SAFETY: within the block.
        unsafe { p.add(i).write(v) };
    }
    // SAFETY: the terminator's slot.
    unsafe { p.add(text.len()).write(T::from(0)) };
    p
}

/// A vector holding `items`, or None if it cannot be allocated.
pub fn try_vec<T: Clone>(items: &[T]) -> Option<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(items.len()).ok()?;
    v.extend_from_slice(items);
    Some(v)
}

/// ASCII case-insensitive comparison of UTF-16 text with `want`.
pub fn eq_ignore_case(text: &[u16], want: &str) -> bool {
    text.len() == want.len()
        && text.iter().zip(want.bytes()).all(|(&a, b)| {
            let a = if (u16::from(b'A')..=u16::from(b'Z')).contains(&a) { a + 32 } else { a };
            a == u16::from(b.to_ascii_lowercase())
        })
}

/// ASCII case-insensitive comparison of two UTF-16 strings.
pub fn eq_ignore_case_w(a: &[u16], b: &[u16]) -> bool {
    let low = |c: u16| if (u16::from(b'A')..=u16::from(b'Z')).contains(&c) { c + 32 } else { c };
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| low(x) == low(y))
}
