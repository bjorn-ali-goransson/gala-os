//! The Win32 functions Chromium's start-up calls that gala lacked (ROADMAP
//! BR stage 1), with the results Microsoft documents for them:
//!
//! 1-3. mitigation policies: DEP is on and permanent in a 64-bit process;
//!    a policy set with SetProcessMitigationPolicy (extension points off,
//!    as Chromium's browser process does) reads back; a wrong size is
//!    ERROR_INVALID_PARAMETER;
//! 4-6. SetProcessInformation (power throttling, memory priority) and
//!    GetProcessInformation reading the memory priority back;
//!    SetThreadInformation (memory priority);
//! 7. GetProcessHandleCount grows by one with a new handle;
//! 8. ProcessIdToSessionId of this process is the PEB's SessionId;
//! 9. GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) is SYSTEM_INFO's count,
//!    GetMaximumProcessorCount at least that;
//! 10. HeapSetInformation(HeapEnableTerminationOnCorruption); GetProcessHeaps
//!    lists the process heap;
//! 11. K32QueryWorkingSetEx: a written page is valid with PAGE_READWRITE,
//!    a reserved page is not;
//! 12. power requests: create, set, clear, close;
//! 13. GetTempFileNameW: with a number, the name `<dir>\gal1234.tmp` and no
//!    file; with 0, a new file; a file opened with FILE_FLAG_DELETE_ON_CLOSE
//!    (write access only) is gone once closed;
//! 14-16. WaitOnAddress: returns at once when the value differs, times out
//!    (ERROR_TIMEOUT) when it does not change, and wakes on
//!    WakeByAddressSingle and (two threads) WakeByAddressAll;
//! 17. CompareObjectHandles: a duplicate is the same object, another handle
//!    is not (ERROR_NOT_SAME_OBJECT);
//! 18. FlsGetValue2/TlsGetValue2 read the slot without touching the last
//!    error;
//! 19. RtlFormatCurrentUserKeyPath is `\REGISTRY\USER\S-1-5-...`;
//! 20. GetAddrInfoExW (synchronous, NS_DNS) resolves "localhost" to
//!    127.0.0.1 with the service's port;
//! 21. CheckRemoteDebuggerPresent says no debugger, QueryProcessCycleTime
//!    succeeds;
//! 22. PEB.LoaderLock points at an RTL_CRITICAL_SECTION not owned by
//!    this thread outside the loader;
//! 23. BuildExplicitAccessWithNameW and BuildSecurityDescriptorW make a
//!    self-relative descriptor with a one-ACE DACL and no SACL;
//! 24. AddMandatoryAce adds a SYSTEM_MANDATORY_LABEL_ACE (type 0x11) with
//!    the policy as its mask; BuildTrusteeWithSidW makes a SID trustee;
//! 25. a mutex named `Local\x` is the one named `x` (ERROR_ALREADY_EXISTS);
//! 26. GetFileInformationByHandleEx(FileNameInfo) of a named pipe is
//!    `\<name>`;
//! 27. RegisterGPNotification/UnregisterGPNotification succeed;
//! 28. a workgroup NT workstation: IsOS(OS_NT) but not OS_DOMAINMEMBER,
//!    NetGetJoinInformation NetSetupWorkgroupName, DsRole standalone;
//! 29. SetProcessDpiAwarenessContext(per monitor v2) once, then
//!    ERROR_ACCESS_DENIED; shcore's GetProcessDpiAwareness says per monitor;
//! 30. the clipboard: CF_UNICODETEXT set while open is available and read
//!    back after closing; the sequence number changes; a caret blink time;
//! 31. WTSQuerySessionInformationW: this session's id, active; the DWM
//!    composes; the US layout is no IME.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn Sleep(ms: u32);
    fn GetLastError() -> u32;
    fn SetLastError(e: u32);
    fn CloseHandle(h: Handle) -> i32;
    fn GetCurrentProcess() -> Handle;
    fn GetCurrentThread() -> Handle;
    fn GetCurrentProcessId() -> u32;
    fn CreateEventW(sa: *const c_void, manual: i32, initial: i32, name: *const u16) -> Handle;
    fn DuplicateHandle(
        sp: Handle,
        s: Handle,
        tp: Handle,
        t: *mut Handle,
        access: u32,
        inherit: i32,
        options: u32,
    ) -> i32;
    fn GetProcessMitigationPolicy(p: Handle, policy: u32, buf: *mut c_void, len: usize) -> i32;
    fn SetProcessMitigationPolicy(policy: u32, buf: *const c_void, len: usize) -> i32;
    fn SetProcessInformation(p: Handle, class: u32, info: *const c_void, size: u32) -> i32;
    fn GetProcessInformation(p: Handle, class: u32, info: *mut c_void, size: u32) -> i32;
    fn SetThreadInformation(t: Handle, class: u32, info: *const c_void, size: u32) -> i32;
    fn GetProcessHandleCount(p: Handle, count: *mut u32) -> i32;
    fn ProcessIdToSessionId(id: u32, session: *mut u32) -> i32;
    fn GetActiveProcessorCount(group: u16) -> u32;
    fn GetMaximumProcessorCount(group: u16) -> u32;
    fn GetSystemInfo(info: *mut [u32; 12]);
    fn HeapSetInformation(h: Handle, class: u32, info: *const c_void, len: usize) -> i32;
    fn GetProcessHeaps(n: u32, heaps: *mut Handle) -> u32;
    fn GetProcessHeap() -> Handle;
    fn VirtualAlloc(a: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut u8;
    fn VirtualFree(a: *mut u8, size: usize, kind: u32) -> i32;
    fn K32QueryWorkingSetEx(p: Handle, pv: *mut [u64; 2], cb: u32) -> i32;
    fn PowerCreateRequest(context: *const ReasonContext) -> Handle;
    fn PowerSetRequest(h: Handle, kind: u32) -> i32;
    fn PowerClearRequest(h: Handle, kind: u32) -> i32;
    fn GetTempPathW(n: u32, buf: *mut u16) -> u32;
    fn GetTempFileNameW(path: *const u16, prefix: *const u16, unique: u32, out: *mut u16) -> u32;
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn DeleteFileW(path: *const u16) -> i32;
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        sa: *const c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CreateThread(
        sa: *const c_void,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        param: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn CompareObjectHandles(a: Handle, b: Handle) -> i32;
    fn FlsAlloc(callback: usize) -> u32;
    fn FlsSetValue(i: u32, v: *mut c_void) -> i32;
    fn FlsGetValue2(i: u32) -> *mut c_void;
    fn TlsAlloc() -> u32;
    fn TlsSetValue(i: u32, v: *mut c_void) -> i32;
    fn TlsGetValue2(i: u32) -> *mut c_void;
    fn CheckRemoteDebuggerPresent(p: Handle, present: *mut i32) -> i32;
    fn LocalFree(p: *mut c_void) -> *mut c_void;
    fn GlobalAlloc(flags: u32, bytes: usize) -> Handle;
    fn GlobalLock(h: Handle) -> *mut c_void;
    fn GlobalUnlock(h: Handle) -> i32;
    fn CreateMutexW(sa: *const c_void, owner: i32, name: *const u16) -> Handle;
    #[allow(clippy::too_many_arguments)]
    fn CreateNamedPipeW(
        name: *const u16,
        open: u32,
        mode: u32,
        max: u32,
        out: u32,
        inb: u32,
        timeout: u32,
        sa: *const c_void,
    ) -> Handle;
    fn GetFileInformationByHandleEx(h: Handle, class: u32, buf: *mut c_void, size: u32) -> i32;
    fn QueryProcessCycleTime(p: Handle, cycles: *mut u64) -> i32;
}

#[link(name = "api-ms-win-core-synch-l1-2-0", kind = "raw-dylib")]
unsafe extern "system" {
    fn WaitOnAddress(a: *const c_void, c: *const c_void, size: usize, ms: u32) -> i32;
    fn WakeByAddressSingle(a: *const c_void);
    fn WakeByAddressAll(a: *const c_void);
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn RtlFormatCurrentUserKeyPath(out: *mut UnicodeString) -> u32;
    fn RtlFreeUnicodeString(s: *mut UnicodeString);
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn BuildExplicitAccessWithNameW(ea: *mut u8, name: *const u16, perm: u32, mode: u32, inh: u32);
    fn BuildTrusteeWithSidW(t: *mut u8, sid: *const u8);
    #[allow(clippy::too_many_arguments)]
    fn BuildSecurityDescriptorW(
        owner: *const u8,
        group: *const u8,
        n: u32,
        entries: *const u8,
        audits: u32,
        audit: *const u8,
        old: *const u8,
        size: *mut u32,
        new: *mut *mut u8,
    ) -> u32;
    fn IsValidSecurityDescriptor(sd: *const u8) -> i32;
    fn GetSecurityDescriptorDacl(sd: *const u8, p: *mut i32, acl: *mut *mut u8, d: *mut i32)
    -> i32;
    fn GetSecurityDescriptorSacl(sd: *const u8, p: *mut i32, acl: *mut *mut u8, d: *mut i32)
    -> i32;
    fn InitializeAcl(acl: *mut u8, size: u32, revision: u32) -> i32;
    fn AddMandatoryAce(acl: *mut u8, rev: u32, flags: u32, policy: u32, sid: *const u8) -> i32;
    fn GetAce(acl: *const u8, index: u32, ace: *mut *mut u8) -> i32;
}

#[link(name = "ws2_32", kind = "raw-dylib")]
unsafe extern "system" {
    fn WSAStartup(version: u16, data: *mut [u8; 408]) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn GetAddrInfoExW(
        name: *const u16,
        service: *const u16,
        namespace: u32,
        nsp: *const c_void,
        hints: *const AddrInfoExW,
        result: *mut *mut AddrInfoExW,
        timeout: *const c_void,
        overlapped: *mut c_void,
        completion: usize,
        handle: *mut Handle,
    ) -> i32;
    fn FreeAddrInfoExW(ai: *mut AddrInfoExW);
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum: u16,
    buffer: *mut u16,
}

#[repr(C)]
struct ReasonContext {
    version: u32,
    flags: u32,
    text: *const u16,
}

#[repr(C)]
struct AddrInfoExW {
    flags: i32,
    family: i32,
    socktype: i32,
    protocol: i32,
    addrlen: usize,
    canonname: *mut u16,
    addr: *const u8,
    blob: *mut c_void,
    bloblen: usize,
    provider: *mut c_void,
    next: *mut AddrInfoExW,
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn __chkstk();
}

const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_TIMEOUT: u32 = 1460;
const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_NOT_SAME_OBJECT: u32 = 1656;
const INFINITE: u32 = 0xffff_ffff;

fn out(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid buffer and output pointer.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        )
    };
}

/// Ends the program with check `n` failed.
fn fail(n: u32) -> ! {
    let mut line = *b"browser_api_test: check 00 failed\r\n";
    line[24] = b'0' + (n / 10) as u8;
    line[25] = b'0' + (n % 10) as u8;
    out(&line);
    // SAFETY: ends the process.
    unsafe { ExitProcess(n) }
}

fn check(ok: bool, n: u32) {
    if !ok {
        fail(n);
    }
}

fn w(s: &str, buf: &mut [u16; 64]) -> *const u16 {
    for (i, c) in s.bytes().enumerate() {
        buf[i] = u16::from(c);
    }
    buf[s.len()] = 0;
    buf.as_ptr()
}

static VALUE: AtomicU32 = AtomicU32::new(0);
static WOKEN: AtomicU32 = AtomicU32::new(0);

/// Waits on VALUE until it is no longer 0, then counts itself woken.
extern "system" fn waiter(_: *mut c_void) -> u32 {
    let zero = 0u32;
    while VALUE.load(Ordering::Acquire) == 0 {
        // SAFETY: VALUE and a 4-byte compare value.
        unsafe { WaitOnAddress(VALUE.as_ptr().cast(), (&raw const zero).cast(), 4, INFINITE) };
    }
    WOKEN.fetch_add(1, Ordering::AcqRel);
    0
}

fn tests() {
    // SAFETY (whole function): documented calls with valid arguments.
    unsafe {
        let me = GetCurrentProcess();
        // 1-3. Mitigation policies.
        let mut dep = [0u32; 2];
        check(GetProcessMitigationPolicy(me, 0, dep.as_mut_ptr().cast(), 8) != 0, 1);
        check(dep[0] & 1 == 1 && dep[1] & 0xff == 1, 1);
        let on = 1u32; // DisableExtensionPoints
        check(SetProcessMitigationPolicy(6, (&raw const on).cast(), 4) != 0, 2);
        let mut got = 0u32;
        check(GetProcessMitigationPolicy(me, 6, (&raw mut got).cast(), 4) != 0, 2);
        check(got & 1 == 1, 2);
        check(GetProcessMitigationPolicy(me, 6, (&raw mut got).cast(), 3) == 0, 3);
        check(GetLastError() == ERROR_INVALID_PARAMETER, 3);

        // 4-6. Process and thread information.
        let throttle = [1u32, 1, 1]; // version 1, EXECUTION_SPEED on
        check(SetProcessInformation(me, 4, throttle.as_ptr().cast(), 12) != 0, 4);
        let normal = 5u32;
        check(SetProcessInformation(me, 0, (&raw const normal).cast(), 4) != 0, 5);
        let mut priority = 0u32;
        check(GetProcessInformation(me, 0, (&raw mut priority).cast(), 4) != 0, 5);
        check(priority == 5, 5);
        check(SetThreadInformation(GetCurrentThread(), 0, (&raw const normal).cast(), 4) != 0, 6);

        // 7. Handle count.
        let (mut before, mut after) = (0u32, 0u32);
        check(GetProcessHandleCount(me, &mut before) != 0 && before > 0, 7);
        let e = CreateEventW(core::ptr::null(), 1, 0, core::ptr::null());
        check(!e.is_null(), 7);
        check(GetProcessHandleCount(me, &mut after) != 0 && after == before + 1, 7);

        // 8. Session.
        let mut session = 0xffff_ffffu32;
        check(ProcessIdToSessionId(GetCurrentProcessId(), &mut session) != 0, 8);
        let peb: usize;
        core::arch::asm!("mov {}, gs:[0x60]", out(reg) peb);
        check(session == *((peb + 0x2c0) as *const u32), 8);

        // 9. Processors.
        let mut si = [0u32; 12];
        GetSystemInfo(&mut si);
        let n = si[8]; // dwNumberOfProcessors (offset 32)
        check(GetActiveProcessorCount(0xffff) == n, 9);
        check(GetMaximumProcessorCount(0xffff) >= n, 9);

        // 10. Heaps.
        check(HeapSetInformation(core::ptr::null_mut(), 1, core::ptr::null(), 0) != 0, 10);
        let mut heaps = [core::ptr::null_mut(); 64];
        let count = GetProcessHeaps(64, heaps.as_mut_ptr());
        check(count >= 1, 10);
        check(heaps[..count.min(64) as usize].contains(&GetProcessHeap()), 10);

        // 11. Working set.
        let page = VirtualAlloc(core::ptr::null_mut(), 0x2000, 0x1000 | 0x2000, 4);
        check(!page.is_null(), 11);
        *page = 1;
        let reserved = VirtualAlloc(core::ptr::null_mut(), 0x1000, 0x2000, 1);
        check(!reserved.is_null(), 11);
        let mut ws = [[page as u64, 0], [reserved as u64, 0]];
        check(K32QueryWorkingSetEx(me, ws.as_mut_ptr(), 32) != 0, 11);
        check(ws[0][1] & 1 == 1 && (ws[0][1] >> 4) & 0x7ff == 4, 11);
        check(ws[1][1] & 1 == 0, 11);
        VirtualFree(page, 0, 0x8000);
        VirtualFree(reserved, 0, 0x8000);

        // 12. Power requests.
        let mut why = [0u16; 64];
        let context = ReasonContext { version: 0, flags: 1, text: w("browser_api_test", &mut why) };
        let request = PowerCreateRequest(&context);
        check(!request.is_null() && request as isize != -1, 12);
        check(PowerSetRequest(request, 1) != 0, 12); // PowerRequestSystemRequired
        check(PowerClearRequest(request, 1) != 0, 12);
        check(CloseHandle(request) != 0, 12);

        // 13. Temporary file names.
        let mut dir = [0u16; 261];
        check(GetTempPathW(261, dir.as_mut_ptr()) != 0, 13);
        let mut prefix = [0u16; 64];
        let mut name = [0u16; 261];
        check(
            GetTempFileNameW(dir.as_ptr(), w("gala", &mut prefix), 0x1234, name.as_mut_ptr())
                == 0x1234,
            13,
        );
        let len = name.iter().position(|&c| c == 0).unwrap_or(0);
        let tail: [u16; 12] = core::array::from_fn(|i| u16::from(b"\\gal1234.tmp"[i]));
        check(len > 12 && name[len - 12..len] == tail, 13);
        check(GetFileAttributesW(name.as_ptr()) == 0xffff_ffff, 13);
        let made = GetTempFileNameW(dir.as_ptr(), w("gal", &mut prefix), 0, name.as_mut_ptr());
        check(made != 0, 13);
        check(GetFileAttributesW(name.as_ptr()) != 0xffff_ffff, 13);
        check(DeleteFileW(name.as_ptr()) != 0, 13);
        // A lock file as Chromium's process singleton makes it: write
        // access, FILE_FLAG_DELETE_ON_CLOSE; gone once closed.
        let lock = CreateFileW(
            name.as_ptr(),
            0x4000_0000, // GENERIC_WRITE
            1,           // FILE_SHARE_READ
            core::ptr::null(),
            2, // CREATE_ALWAYS
            0x80 | 0x0400_0000,
            core::ptr::null_mut(),
        );
        check(lock as isize != -1, 13);
        check(CloseHandle(lock) != 0, 13);
        check(GetFileAttributesW(name.as_ptr()) == 0xffff_ffff, 13);

        // 14-16. WaitOnAddress.
        let one = 1u32;
        let zero = 0u32;
        check(WaitOnAddress(VALUE.as_ptr().cast(), (&raw const one).cast(), 4, INFINITE) != 0, 14);
        SetLastError(0);
        check(WaitOnAddress(VALUE.as_ptr().cast(), (&raw const zero).cast(), 4, 50) == 0, 15);
        check(GetLastError() == ERROR_TIMEOUT, 15);
        let threads = [
            CreateThread(
                core::ptr::null(),
                0,
                waiter,
                core::ptr::null_mut(),
                0,
                core::ptr::null_mut(),
            ),
            CreateThread(
                core::ptr::null(),
                0,
                waiter,
                core::ptr::null_mut(),
                0,
                core::ptr::null_mut(),
            ),
        ];
        check(!threads[0].is_null() && !threads[1].is_null(), 16);
        Sleep(100);
        check(WOKEN.load(Ordering::Acquire) == 0, 16);
        // A wake with the value unchanged: the waiters wait again.
        WakeByAddressSingle(VALUE.as_ptr().cast());
        Sleep(50);
        check(WOKEN.load(Ordering::Acquire) == 0, 16);
        VALUE.store(1, Ordering::Release);
        WakeByAddressAll(VALUE.as_ptr().cast());
        for t in threads {
            check(WaitForSingleObject(t, 10_000) == 0, 16);
            CloseHandle(t);
        }
        check(WOKEN.load(Ordering::Acquire) == 2, 16);

        // 17. CompareObjectHandles.
        let mut copy = core::ptr::null_mut();
        check(DuplicateHandle(me, e, me, &mut copy, 0, 0, 2) != 0, 17); // SAME_ACCESS
        check(CompareObjectHandles(e, copy) != 0, 17);
        let other = CreateEventW(core::ptr::null(), 1, 0, core::ptr::null());
        check(CompareObjectHandles(e, other) == 0, 17);
        check(GetLastError() == ERROR_NOT_SAME_OBJECT, 17);
        CloseHandle(copy);
        CloseHandle(other);
        CloseHandle(e);

        // 18. FlsGetValue2/TlsGetValue2.
        let fls = FlsAlloc(0);
        check(fls != 0xffff_ffff && FlsSetValue(fls, 0x1234 as *mut c_void) != 0, 18);
        let tls = TlsAlloc();
        check(tls != 0xffff_ffff && TlsSetValue(tls, 0x5678 as *mut c_void) != 0, 18);
        SetLastError(77);
        check(FlsGetValue2(fls) as usize == 0x1234 && TlsGetValue2(tls) as usize == 0x5678, 18);
        check(GetLastError() == 77, 18);

        // 19. The current user's key.
        let mut path = UnicodeString { length: 0, maximum: 0, buffer: core::ptr::null_mut() };
        check(RtlFormatCurrentUserKeyPath(&mut path) == 0, 19);
        let prefix = b"\\REGISTRY\\USER\\S-1-5-";
        check(usize::from(path.length) / 2 > prefix.len(), 19);
        for (i, &c) in prefix.iter().enumerate() {
            check(*path.buffer.add(i) == u16::from(c), 19);
        }
        RtlFreeUnicodeString(&mut path);

        // 20. GetAddrInfoExW.
        let mut wsa = [0u8; 408];
        check(WSAStartup(0x202, &mut wsa) == 0, 20);
        let hints = AddrInfoExW {
            flags: 0,
            family: 2, // AF_INET
            socktype: 1,
            protocol: 6,
            addrlen: 0,
            canonname: core::ptr::null_mut(),
            addr: core::ptr::null(),
            blob: core::ptr::null_mut(),
            bloblen: 0,
            provider: core::ptr::null_mut(),
            next: core::ptr::null_mut(),
        };
        let (mut host, mut port) = ([0u16; 64], [0u16; 64]);
        let mut result: *mut AddrInfoExW = core::ptr::null_mut();
        let r = GetAddrInfoExW(
            w("localhost", &mut host),
            w("8080", &mut port),
            12, // NS_DNS
            core::ptr::null(),
            &hints,
            &mut result,
            core::ptr::null(),
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
        );
        check(r == 0 && !result.is_null(), 20);
        let sa = (*result).addr;
        check((*result).family == 2 && !sa.is_null(), 20);
        check(*sa.add(2) == 0x1f && *sa.add(3) == 0x90, 20); // port 8080
        check(*sa.add(4) == 127 && *sa.add(7) == 1, 20);
        FreeAddrInfoExW(result);

        // 21. Debugger and cycle time.
        let mut present = 1i32;
        check(CheckRemoteDebuggerPresent(me, &mut present) != 0 && present == 0, 21);
        let mut cycles = 0u64;
        check(QueryProcessCycleTime(me, &mut cycles) != 0, 21);

        // 22. PEB.LoaderLock is an RTL_CRITICAL_SECTION, not owned by this
        // thread outside the loader (Crashpad's IsThreadInLoaderLock reads
        // its OwningThread).
        let lock = *((peb + 0x110) as *const usize);
        check(lock != 0, 22);
        let owner = *((lock + 0x10) as *const usize);
        let me_id: usize;
        core::arch::asm!("mov {}, gs:[0x48]", out(reg) me_id);
        check(owner != me_id, 22);

        // 23. Trustees and descriptors built from explicit access entries.
        let mut name = [0u16; 64];
        let mut ea = [0u8; 48];
        // CURRENT_USER, GENERIC_ALL, SET_ACCESS, NO_INHERITANCE.
        BuildExplicitAccessWithNameW(
            ea.as_mut_ptr(),
            w("CURRENT_USER", &mut name),
            0x1000_0000,
            2,
            0,
        );
        check(*ea.as_ptr().cast::<u32>() == 0x1000_0000 && ea[16 + 12] == 1, 23); // TRUSTEE_IS_NAME
        let mut size = 0u32;
        let mut sd: *mut u8 = core::ptr::null_mut();
        let r = BuildSecurityDescriptorW(
            core::ptr::null(),
            core::ptr::null(),
            1,
            ea.as_ptr(),
            0,
            core::ptr::null(),
            core::ptr::null(),
            &mut size,
            &mut sd,
        );
        check(r == 0 && !sd.is_null() && size >= 20, 23);
        check(IsValidSecurityDescriptor(sd) != 0, 23);
        let (mut present, mut defaulted) = (0i32, 0i32);
        let mut acl: *mut u8 = core::ptr::null_mut();
        check(GetSecurityDescriptorDacl(sd, &mut present, &mut acl, &mut defaulted) != 0, 23);
        check(present != 0 && !acl.is_null() && *acl.add(4) == 1, 23); // one ACE
        check(GetSecurityDescriptorSacl(sd, &mut present, &mut acl, &mut defaulted) != 0, 23);
        check(present == 0, 23);
        LocalFree(sd.cast());

        // 24. A mandatory label ACE (low integrity, no write up).
        let mut sacl = [0u8; 64];
        check(InitializeAcl(sacl.as_mut_ptr(), 64, 2) != 0, 24);
        let low = [1u8, 1, 0, 0, 0, 0, 0, 16, 0x00, 0x10, 0, 0]; // S-1-16-4096
        check(AddMandatoryAce(sacl.as_mut_ptr(), 2, 0, 1, low.as_ptr()) != 0, 24);
        let mut ace: *mut u8 = core::ptr::null_mut();
        check(GetAce(sacl.as_ptr(), 0, &mut ace) != 0, 24);
        check(*ace == 0x11 && *ace.add(4) == 1, 24);
        let mut trustee = [0u8; 32];
        BuildTrusteeWithSidW(trustee.as_mut_ptr(), low.as_ptr());
        check(
            trustee[12] == 0 && *trustee.as_ptr().add(24).cast::<*const u8>() == low.as_ptr(),
            24,
        );

        // 25. `Local\` names the session's namespace: the same mutex as
        // the plain name (Chromium's process singleton).
        let mut plain = [0u16; 64];
        let mut local = [0u16; 64];
        let m = CreateMutexW(core::ptr::null(), 0, w("browser_api_test_mutex", &mut plain));
        check(!m.is_null(), 25);
        let again =
            CreateMutexW(core::ptr::null(), 0, w("Local\\browser_api_test_mutex", &mut local));
        check(!again.is_null() && GetLastError() == ERROR_ALREADY_EXISTS, 25);
        check(CompareObjectHandles(m, again) != 0, 25);
        CloseHandle(again);
        CloseHandle(m);

        // 26. A named pipe's FileNameInfo is `\<name>` (Crashpad's handler
        // finds its pipe's name so).
        let mut pipe_name = [0u16; 64];
        let pipe = CreateNamedPipeW(
            w("\\\\.\\pipe\\browser_api_test", &mut pipe_name),
            3,     // PIPE_ACCESS_DUPLEX
            4 | 2, // PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE
            1,
            512,
            512,
            0,
            core::ptr::null(),
        );
        check(pipe as isize != -1, 26);
        let mut info = [0u32; 64];
        check(GetFileInformationByHandleEx(pipe, 2, info.as_mut_ptr().cast(), 256) != 0, 26);
        let expect = "\\browser_api_test";
        check(info[0] as usize == 2 * expect.len(), 26);
        let got = info.as_ptr().cast::<u16>().add(2);
        for (i, c) in expect.bytes().enumerate() {
            check(*got.add(i) == u16::from(c), 26);
        }
        CloseHandle(pipe);

        // 27. Group policy notifications (no policy is applied on a machine
        // without a domain, but registering works).
        let gp = CreateEventW(core::ptr::null(), 0, 0, core::ptr::null());
        check(RegisterGPNotification(gp, 0) != 0, 27);
        check(UnregisterGPNotification(gp) != 0, 27);
        CloseHandle(gp);

        // 28. A standalone (workgroup) NT workstation: IsOS, the join
        // information and the primary domain role (assumes the Windows
        // test machine is in no domain).
        check(IsOS(1) != 0 && IsOS(28) == 0, 28); // OS_NT, OS_DOMAINMEMBER
        let mut join_name: *mut u16 = core::ptr::null_mut();
        let mut join_kind = 0u32;
        check(NetGetJoinInformation(core::ptr::null(), &mut join_name, &mut join_kind) == 0, 28);
        check(join_kind == 2 && !join_name.is_null(), 28); // NetSetupWorkgroupName
        NetApiBufferFree(join_name.cast());
        let mut role: *mut u32 = core::ptr::null_mut();
        check(DsRoleGetPrimaryDomainInformation(core::ptr::null(), 1, &mut role) == 0, 28);
        check(!role.is_null() && *role == 0, 28); // DsRole_RoleStandaloneWorkstation
        DsRoleFreeMemory(role.cast());

        // 29. DPI awareness: per-monitor v2 set once (Chromium's), read
        // back as per monitor; a second setting is refused.
        check(SetProcessDpiAwarenessContext(-4) != 0, 29);
        check(GetAwarenessFromDpiAwarenessContext(GetThreadDpiAwarenessContext()) == 2, 29);
        check(SetProcessDpiAwarenessContext(-2) == 0 && GetLastError() == 5, 29);
        let mut awareness = 9u32;
        check(GetProcessDpiAwareness(core::ptr::null_mut(), &mut awareness) == 0, 29);
        check(awareness == 2, 29); // PROCESS_PER_MONITOR_DPI_AWARE

        // 30. The clipboard: text set while open is there after closing,
        // and the sequence number moved.
        let before = GetClipboardSequenceNumber();
        check(OpenClipboard(0) != 0, 30);
        check(EmptyClipboard() != 0, 30);
        let text = GlobalAlloc(2, 16); // GMEM_MOVEABLE
        check(!text.is_null(), 30);
        let p = GlobalLock(text).cast::<u16>();
        for (i, &c) in b"gala\0".iter().enumerate() {
            *p.add(i) = u16::from(c);
        }
        GlobalUnlock(text);
        check(SetClipboardData(13, text) == text, 30); // CF_UNICODETEXT
        check(CloseClipboard() != 0, 30);
        check(GetClipboardSequenceNumber() != before, 30);
        check(IsClipboardFormatAvailable(13) != 0, 30);
        check(OpenClipboard(0) != 0, 30);
        let got = GetClipboardData(13);
        check(!got.is_null() && *GlobalLock(got).cast::<u16>() == u16::from(b'g'), 30);
        GlobalUnlock(got);
        check(CloseClipboard() != 0, 30);
        check(GetCaretBlinkTime() != 0, 30);

        // 31. The session (wtsapi32), the DWM, the input method manager.
        let mut info: *mut u8 = core::ptr::null_mut();
        let mut bytes = 0u32;
        check(WTSQuerySessionInformationW(0, u32::MAX, 4, &mut info, &mut bytes) != 0, 31);
        check(bytes == 4 && *info.cast::<u32>() == *((peb + 0x2c0) as *const u32), 31);
        WTSFreeMemory(info);
        check(WTSQuerySessionInformationW(0, u32::MAX, 8, &mut info, &mut bytes) != 0, 31);
        check(*info.cast::<u32>() == 0, 31); // WTSActive
        WTSFreeMemory(info);
        let mut composed = 0i32;
        check(DwmIsCompositionEnabled(&mut composed) == 0 && composed != 0, 31);
        let mut ime = [0u16; 8];
        check(ImmGetIMEFileNameW(0x0409_0409, ime.as_mut_ptr(), 8) == 0, 31); // US: no IME
    }
}

#[link(name = "wtsapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn WTSQuerySessionInformationW(
        server: usize,
        session: u32,
        class: u32,
        out: *mut *mut u8,
        bytes: *mut u32,
    ) -> i32;
    fn WTSFreeMemory(p: *mut u8);
}

#[link(name = "dwmapi", kind = "raw-dylib")]
unsafe extern "system" {
    fn DwmIsCompositionEnabled(enabled: *mut i32) -> i32;
}

#[link(name = "imm32", kind = "raw-dylib")]
unsafe extern "system" {
    fn ImmGetIMEFileNameW(layout: usize, name: *mut u16, length: u32) -> u32;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn OpenClipboard(owner: usize) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn SetClipboardData(format: u32, memory: Handle) -> Handle;
    fn GetClipboardData(format: u32) -> Handle;
    fn IsClipboardFormatAvailable(format: u32) -> i32;
    fn GetClipboardSequenceNumber() -> u32;
    fn GetCaretBlinkTime() -> u32;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetProcessDpiAwarenessContext(context: isize) -> i32;
    fn GetThreadDpiAwarenessContext() -> isize;
    fn GetAwarenessFromDpiAwarenessContext(context: isize) -> i32;
}

#[link(name = "api-ms-win-shcore-scaling-l1-1-1", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessDpiAwareness(process: Handle, value: *mut u32) -> i32;
}

#[link(name = "shlwapi", kind = "raw-dylib")]
unsafe extern "system" {
    fn IsOS(os: u32) -> i32;
}

#[link(name = "netapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn NetGetJoinInformation(server: *const u16, name: *mut *mut u16, kind: *mut u32) -> u32;
    fn NetApiBufferFree(p: *mut c_void) -> u32;
    fn DsRoleGetPrimaryDomainInformation(server: *const u16, level: u32, out: *mut *mut u32)
    -> u32;
    fn DsRoleFreeMemory(p: *mut c_void);
}

#[link(name = "userenv", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegisterGPNotification(event: Handle, machine: i32) -> i32;
    fn UnregisterGPNotification(event: Handle) -> i32;
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    tests();
    out(b"browser_api_test: ok\r\n");
    // SAFETY: ends the process.
    unsafe { ExitProcess(0) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
