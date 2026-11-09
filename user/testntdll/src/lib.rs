//! A minimal stand-in for ntdll.dll, for testing the kernel's side of
//! process startup (written from the documented contract, Windows
//! Internals): the initial thread of a process starts at
//! `LdrInitializeThunk(Context, NtdllBase)`, where `Context` holds the
//! thread's real start; once user-mode initialization is done, the thunk
//! resumes that context with `NtContinue`.
//!
//! This one records that it ran (`GalaTestNtdllState`, which
//! `startup_test` reads), binds the executable's imports and continues. It
//! also has a small loader (LdrLoadDll and friends: DLLs from the
//! executable's directory, reference counted), vectored exception
//! handling, the APC dispatcher, the heap API (heap.rs) and the system
//! call stubs tests use.

#![no_std]
#![allow(non_snake_case, non_upper_case_globals)]

// The architecture's parts: x64 (System32's ntdll) or x86 (the 32-bit
// ntdll in SysWOW64, ARCHITECTURE "Plan: WOW64"). First: they define the
// system call stub macro.
#[cfg(target_arch = "x86_64")]
#[macro_use]
#[path = "x64.rs"]
mod arch;
#[cfg(target_arch = "x86")]
#[macro_use]
#[path = "x86.rs"]
mod arch;

mod avl;
mod heap;
mod rtl;
#[cfg(target_arch = "x86_64")]
mod seh;
#[cfg(target_arch = "x86")]
#[path = "seh_x86.rs"]
mod seh;
mod tls;
mod unimplemented;

pub use arch::*;

/// "galantdl": marks the state as written by this DLL.
const MAGIC: u64 = 0x6761_6c61_6e74_646c;

/// [magic, NtdllBase argument, Context->Rip, number of thunk calls]
#[unsafe(no_mangle)]
pub static mut GalaTestNtdllState: [u64; 4] = [0; 4];

// Numbers of the pinned build (to be verified), as in ntrt.
syscall_stubs! {
    NtClose = 0x0f, fn(h: usize);
    // gala's number until verified on the pinned build (see the kernel).
    NtCancelIoFileEx = 0x7f01, fn(h: usize, request: *mut [usize; 2], iosb: *mut [usize; 2]);
    // gala's process creation (kernel32's CreateProcess; see the kernel).
    GalaCreateProcess = 0x7f02, fn(parameters: *mut u8);
    // gala's numbers until verified on the pinned build (see the kernel).
    NtCreateNamedPipeFile = 0x7f03, fn(
        handle: *mut usize, access: u32, attributes: *const ObjectAttributes,
        iosb: *mut [usize; 2], share: u32, disposition: u32, options: u32, kind: u32,
        read_mode: u32, completion: u32, instances: u32, inbound: u32, outbound: u32,
        timeout: *const i64
    );
    NtFsControlFile = 0x7f04, fn(
        h: usize, event: usize, apc: usize, context: usize, iosb: *mut [usize; 2], code: u32,
        input: *const u8, input_length: u32, output: *mut u8, output_length: u32
    );
    NtSetInformationObject = 0x7f05, fn(h: usize, class: u32, buffer: *const u8, length: u32);
    // The registry: 0x12, 0x13, 0x16, 0x17, 0x1d and 0x32 are long-stable on
    // x64, 0x60 is to be verified; the others are gala's numbers (see the
    // kernel).
    NtOpenKey = 0x12, fn(handle: *mut usize, access: u32, attributes: *const ObjectAttributes);
    NtEnumerateValueKey = 0x13, fn(
        h: usize, index: u32, class: u32, buf: *mut u8, len: u32, ret: *mut u32
    );
    NtQueryKey = 0x16, fn(h: usize, class: u32, buf: *mut u8, len: u32, ret: *mut u32);
    NtQueryValueKey = 0x17, fn(
        h: usize, name: *const UnicodeString, class: u32, buf: *mut u8, len: u32, ret: *mut u32
    );
    NtCreateKey = 0x1d, fn(
        handle: *mut usize, access: u32, attributes: *const ObjectAttributes, title: u32,
        class: *const UnicodeString, options: u32, disposition: *mut u32
    );
    NtEnumerateKey = 0x32, fn(
        h: usize, index: u32, class: u32, buf: *mut u8, len: u32, ret: *mut u32
    );
    NtSetValueKey = 0x60, fn(
        h: usize, name: *const UnicodeString, title: u32, kind: u32, data: *const u8, size: u32
    );
    NtOpenKeyEx = 0x7f06, fn(
        handle: *mut usize, access: u32, attributes: *const ObjectAttributes, options: u32
    );
    NtDeleteKey = 0x7f07, fn(h: usize);
    NtDeleteValueKey = 0x7f08, fn(h: usize, name: *const UnicodeString);
    NtFlushKey = 0x7f09, fn(h: usize);
    NtSetInformationKey = 0x7fd6, fn(h: usize, class: u32, buf: *const u8, len: u32);
    // Job objects (gala's numbers, unverified).
    NtCreateJobObject = 0x7f0a, fn(
        handle: *mut usize, access: u32, attributes: *const ObjectAttributes
    );
    NtAssignProcessToJobObject = 0x7f0b, fn(job: usize, process: isize);
    NtSetInformationJobObject = 0x7f0c, fn(job: usize, class: u32, buf: *const u8, len: u32);
    NtQueryInformationJobObject = 0x7f0d, fn(
        job: usize, class: u32, buf: *mut u8, len: u32, ret: *mut u32
    );
    NtIsProcessInJob = 0x7f0e, fn(process: isize, job: usize);
    // Keyed events (gala's numbers, unverified).
    NtCreateKeyedEvent = 0x7f0f, fn(
        handle: *mut usize, access: u32, attributes: *const ObjectAttributes, flags: u32
    );
    NtOpenKeyedEvent = 0x7f10, fn(
        handle: *mut usize, access: u32, attributes: *const ObjectAttributes
    );
    NtWaitForKeyedEvent = 0x7f11, fn(h: usize, key: usize, alertable: u8, timeout: *const i64);
    NtReleaseKeyedEvent = 0x7f12, fn(h: usize, key: usize, alertable: u8, timeout: *const i64);
    // Directory change notification (gala's number, unverified).
    NtNotifyChangeDirectoryFile = 0x7f13, fn(
        h: usize, event: usize, apc: usize, apc_context: usize, iosb: *mut [usize; 2],
        buffer: *mut u8, length: u32, filter: u32, tree: u8
    );
    // Scatter/gather file I/O (gala's numbers, unverified).
    NtReadFileScatter = 0x7f14, fn(
        h: usize, event: usize, apc: usize, apc_context: usize, iosb: *mut [usize; 2],
        segments: *const u64, length: u32, offset: *const i64, key: *const u32
    );
    NtWriteFileGather = 0x7f15, fn(
        h: usize, event: usize, apc: usize, apc_context: usize, iosb: *mut [usize; 2],
        segments: *const u64, length: u32, offset: *const i64, key: *const u32
    );
    // Tokens (gala's numbers, unverified; docs/SECURITY.md).
    NtOpenProcessToken = 0x7f16, fn(process: isize, access: u32, token: *mut usize);
    NtOpenProcessTokenEx = 0x7f17, fn(
        process: isize, access: u32, attributes: u32, token: *mut usize
    );
    NtOpenThreadToken = 0x7f18, fn(thread: isize, access: u32, as_self: u8, token: *mut usize);
    NtOpenThreadTokenEx = 0x7f19, fn(
        thread: isize, access: u32, as_self: u8, attributes: u32, token: *mut usize
    );
    NtQueryInformationToken = 0x7f1a, fn(
        token: usize, class: u32, buf: *mut u8, len: u32, ret: *mut u32
    );
    NtAdjustPrivilegesToken = 0x7f1b, fn(
        token: usize, disable_all: u8, new: *const u8, length: u32, previous: *mut u8,
        ret: *mut u32
    );
    NtDuplicateToken = 0x7f1c, fn(
        token: usize, access: u32, attributes: *const ObjectAttributes, effective_only: u8,
        kind: u32, new: *mut usize
    );
    NtFilterToken = 0x7f1d, fn(
        token: usize, flags: u32, disable: *const u8, delete: *const u8, restrict: *const u8,
        new: *mut usize
    );
    NtPrivilegeCheck = 0x7f1e, fn(token: usize, set: *mut u8, result: *mut u8);
    // Security descriptors (gala's numbers, unverified).
    NtQuerySecurityObject = 0x7f1f, fn(
        h: usize, info: u32, sd: *mut u8, length: u32, needed: *mut u32
    );
    NtSetSecurityObject = 0x7f20, fn(h: usize, info: u32, sd: *const u8);
    // gala's logon service (LogonUserW).
    GalaLogonUser = 0x7f21, fn(
        user: *const UnicodeString, domain: *const UnicodeString,
        password: *const UnicodeString, kind: u32, token: *mut usize
    );
    NtAllocateLocallyUniqueId = 0x7f22, fn(luid: *mut u64);
    NtSetInformationToken = 0x7f23, fn(token: usize, class: u32, info: *const u8, length: u32);
    NtUnmapViewOfSection = 0x2a, fn(process: isize, base: usize);
    NtMapViewOfSection = 0x28, fn(
        section: usize, process: isize, base: *mut usize, zero_bits: usize, commit: usize,
        offset: *mut u64, size: *mut usize, inherit: u32, kind: u32, protect: u32
    );
    NtOpenFile = 0x33, fn(
        handle: *mut usize, access: u32, attributes: *const ObjectAttributes,
        iosb: *mut [usize; 2], share: u32, options: u32
    );
    NtCreateSection = 0x4a, fn(
        handle: *mut usize, access: u32, attributes: *const ObjectAttributes, size: *const i64,
        protect: u32, attributes: u32, file: usize
    );
    NtOpenSection = 0x37, fn(handle: *mut usize, access: u32, attributes: *const ObjectAttributes);
    NtQuerySection = 0x51, fn(h: usize, class: u32, buf: *mut u8, len: usize, ret: *mut usize);
    NtProtectVirtualMemory = 0x50, fn(
        process: isize, base: *mut usize, size: *mut usize, protect: u32, old: *mut u32
    );
    // Mutants and semaphores. 0x0a and 0x20 are long-stable on x64; the
    // others vary by build: gala's choices (kernel/src/nt/syscalls.rs).
    NtReleaseSemaphore = 0x0a, fn(h: usize, count: i32, previous: *mut i32);
    NtReleaseMutant = 0x20, fn(h: usize, previous: *mut i32);
    NtCreateMutant = 0xc3, fn(
        handle: *mut usize, access: u32, attributes: *const ObjectAttributes, owner: u8
    );
    NtCreateSemaphore = 0xcf, fn(
        handle: *mut usize, access: u32, attributes: *const ObjectAttributes, initial: i32,
        maximum: i32
    );
    NtOpenMutant = 0x12c, fn(handle: *mut usize, access: u32, attributes: *const ObjectAttributes);
    NtOpenSemaphore = 0x12f, fn(handle: *mut usize, access: u32, attributes: *const ObjectAttributes);
    NtQueryMutant = 0x146, fn(h: usize, class: u32, buf: *mut u8, len: u32, ret: *mut u32);
    NtQuerySemaphore = 0x15a, fn(h: usize, class: u32, buf: *mut u8, len: u32, ret: *mut u32);
    // Symbolic links and directory queries (gala's numbers, unverified).
    NtCreateSymbolicLinkObject = 0xd1, fn(
        handle: *mut usize, access: u32, attributes: *const ObjectAttributes,
        target: *const UnicodeString
    );
    NtOpenSymbolicLinkObject = 0x137, fn(
        handle: *mut usize, access: u32, attributes: *const ObjectAttributes
    );
    NtQuerySymbolicLinkObject = 0x16a, fn(h: usize, target: *mut UnicodeString, returned: *mut u32);
    // I/O completion ports: 0x09 is long-stable on x64; the others are
    // gala's numbers (unverified).
    NtRemoveIoCompletion = 0x09, fn(
        h: usize, key: *mut usize, apc_context: *mut usize, iosb: *mut [usize; 2],
        timeout: *const i64
    );
    NtCreateIoCompletion = 0xc9, fn(
        handle: *mut usize, access: u32, attributes: *const ObjectAttributes, count: u32
    );
    NtSetIoCompletion = 0x1b0, fn(
        h: usize, key: usize, apc_context: usize, status: u32, information: usize
    );
    NtQueryIoCompletion = 0x14f, fn(h: usize, class: u32, buf: *mut u8, len: u32, ret: *mut u32);
    // Thread suspension and register state (gala's numbers, unverified).
    NtGetContextThread = 0xf2, fn(h: usize, context: *mut u8);
    NtSetContextThread = 0x18d, fn(h: usize, context: *const u8);
    NtSuspendThread = 0x1bc, fn(h: usize, previous: *mut u32);
    // Processors (gala's number, unverified).
    NtGetCurrentProcessorNumber = 0xf8, fn();
    // For gala's kernel32 (long-stable x64 numbers).
    NtWaitForSingleObject = 0x04, fn(h: usize, alertable: u8, timeout: *const i64);
    NtWriteFile = 0x08, fn(
        h: usize, event: usize, apc: usize, apc_context: usize, iosb: *mut [usize; 2],
        buffer: *const u8, length: u32, offset: *const i64, key: *const u32
    );
    NtSetEvent = 0x0e, fn(h: usize, previous: *mut i32);
    NtAllocateVirtualMemory = 0x18, fn(
        process: isize, base: *mut usize, zero_bits: usize, size: *mut usize, kind: u32,
        protect: u32
    );
    NtFreeVirtualMemory = 0x1e, fn(process: isize, base: *mut usize, size: *mut usize, kind: u32);
    NtReadVirtualMemory = 0x3f, fn(process: isize, base: usize, buffer: *mut u8, size: usize, done: *mut usize);
    NtWriteVirtualMemory = 0x3a, fn(process: isize, base: usize, buffer: *const u8, size: usize, done: *mut usize);
    NtSetInformationThread = 0x0d, fn(h: usize, class: u32, buf: *const u8, len: u32);
    NtLockFile = 0x123, fn(
        h: usize, event: usize, apc: usize, apc_context: usize, iosb: *mut [usize; 2],
        offset: *const i64, length: *const i64, key: u32, fail_immediately: u8, exclusive: u8
    );
    NtUnlockFile = 0x1c5, fn(h: usize, iosb: *mut [usize; 2], offset: *const i64, length: *const i64, key: u32);
    NtOpenProcess = 0x26, fn(
        handle: *mut usize, access: u32, attributes: *const ObjectAttributes, client_id: *const [usize; 2]
    );
    NtQueryPerformanceCounter = 0x31, fn(counter: *mut i64, frequency: *mut i64);
    NtDelayExecution = 0x34, fn(alertable: u8, interval: *const i64);
    NtOpenEvent = 0x40, fn(handle: *mut usize, access: u32, attributes: *const ObjectAttributes);
    NtQueryEvent = 0x56, fn(h: usize, class: u32, buf: *mut u8, len: u32, ret: *mut u32);
    NtOpenDirectoryObject = 0x58, fn(handle: *mut usize, access: u32, attributes: *const ObjectAttributes);
    // gala's own services: the desktop's windows (nt::desktop).
    GalaWindowCreate = 0x7f30, fn(x: i32, y: i32, w: u32, h: u32, title: *const u16, len: usize, id: *mut u32, style: u32);
    GalaWindowShow = 0x7f31, fn(id: u32, show: u8);
    GalaWindowPresent = 0x7f32, fn(id: u32, x: i32, y: i32, w: u32, h: u32, pixels: *const u32, stride: usize);
    GalaWindowDestroy = 0x7f33, fn(id: u32);
    GalaWindowListen = 0x7f34, fn(event: *mut core::ffi::c_void);
    GalaWindowInput = 0x7f35, fn(records: *mut u8, max: u32, count: *mut u32);
    GalaScreenInfo = 0x7f36, fn(info: *mut [i32; 8]);
    GalaWindowPlace = 0x7f37, fn(id: u32, x: i32, y: i32, w: u32, h: u32);
    GalaWindowSetTitle = 0x7f38, fn(id: u32, title: *const u16, len: usize);
    GalaWindowCapture = 0x7f39, fn(id: u32);
    GalaConsoleHost = 0x7f3a, fn(id: u32, event: *mut core::ffi::c_void);
    GalaConsoleRead = 0x7f3b, fn(id: u32, buf: *mut u8, len: u32, count: *mut u32);
    GalaConsoleInput = 0x7f3c, fn(id: u32, records: *const u8, count: u32);
    GalaConsoleInputEvents = 0x7f3d, fn(records: *mut u8, max: u32, count: *mut u32, flags: u32);
    GalaWindowSetIcon = 0x7f3e, fn(id: u32, pixels: *const u32, size: usize);
    GalaConsoleMode = 0x7f3f, fn(mode: u32, set: u32);
    GalaConsoleCtrl = 0x7f40, fn(seen: *mut u32);
    GalaConsoleCreate = 0x7f80, fn(flags: u32, size: u32, info: *mut [u32; 2]);
    GalaConsoleSize = 0x7f81, fn(id: u32, size: *mut u32, set: u32);
    GalaConsoleClose = 0x7f82, fn(id: u32);
    GalaConsoleTrace = 0x7f83, fn(id: u32, direction: u32, bytes: *const u8, len: usize);
    // Object identity (gala's number, unverified): CompareObjectHandles.
    NtCompareObjects = 0x7f90, fn(first: usize, second: usize);
    // Process suspension (gala's numbers, unverified).
    NtSuspendProcess = 0x7f91, fn(process: usize);
    NtOpenThread = 0x7f93, fn(
        handle: *mut usize, access: u32, attributes: *const ObjectAttributes, client_id: *const [usize; 2]
    );
    NtResumeProcess = 0x7f92, fn(process: usize);
    // Window stations and desktops (gala's own; nt::winsta), for user32.
    GalaUserObjectOpen = 0x7fa0, fn(
        kind: u32, name: *const u16, length: u32, access: u32, inherit: u32, handle: *mut usize
    );
    GalaUserObjectConnection = 0x7fa1, fn(what: u32, tid: u32, handle: *mut usize);
    GalaUserObjectSet = 0x7fa2, fn(what: u32, handle: usize);
    GalaUserObjectClose = 0x7fa3, fn(kind: u32, handle: usize);
    GalaUserObjectInformation = 0x7fa4, fn(
        handle: usize, index: u32, buffer: *mut u8, length: u32, needed: *mut u32
    );
    GalaUserObjectEnum = 0x7fa5, fn(handle: usize, buffer: *mut u8, length: u32, needed: *mut u32);
    // Sessions' presenters (gala's own; nt::session_calls), for `gala attach`.
    GalaSession = 0x7fb0, fn(op: u64, session: u32, arg: u64, buffer: u64, length: u64);
    // The window station's clipboard (gala's own; nt::clipboard_calls), for user32.
    GalaClipboardOpen = 0x7fc0, fn(hwnd: u64);
    GalaClipboardClose = 0x7fc1, fn(locale: u32);
    GalaClipboardEmpty = 0x7fc2, fn();
    GalaClipboardSet = 0x7fc3, fn(format: u32, kind: u32, data: *const u8, length: u32, serial: *mut u32);
    GalaClipboardGet = 0x7fc4, fn(format: u32, buffer: *mut u8, length: u32, info: *mut u8);
    GalaClipboardInfo = 0x7fc5, fn(record: *mut u8);
    GalaClipboardFormats = 0x7fc6, fn(formats: *mut u32, max: u32, count: *mut u32);
    GalaClipboardListener = 0x7fc7, fn(hwnd: u64, add: u32);
    // The service control manager (gala's own; nt::scm_calls), for advapi32.
    GalaScmOpen = 0x7fd0, fn(access: u32, handle: *mut usize);
    GalaServiceOpen = 0x7fd1, fn(scm: usize, name: *const u16, length: u32, access: u32, handle: *mut usize);
    GalaServiceCreate = 0x7fd2, fn(scm: usize, record: *const u8, length: u32, access: u32, handle: *mut usize);
    GalaServiceQuery = 0x7fd3, fn(service: usize, class: u32, buffer: *mut u8, length: u32, needed: *mut u32);
    GalaServiceEnum = 0x7fd4, fn(
        scm: usize, kinds: u32, states: u32, buffer: *mut u8, length: u32, needed: *mut u32
    );
    GalaServiceControl = 0x7fd5, fn(handle: usize, op: u32, code: u32, status: *mut u32);
    GalaServiceConfigure = 0x7fd7, fn(service: usize, level: u32, record: *const u8, length: u32);
    GalaServiceDispatch = 0x7fd8, fn(op: u32, value: u32, buffer: *mut u8, length: u32, needed: *mut u32);
    GalaWindowRegister = 0x7f41, fn(info: *const u8, hwnd: *mut u64);
    GalaWindowUpdate = 0x7f42, fn(info: *const u8);
    GalaWindowUnregister = 0x7f43, fn(hwnd: u64);
    GalaWindowQuery = 0x7f44, fn(hwnd: u64, info: *mut u8);
    GalaWindowChildren = 0x7f45, fn(parent: u64, hwnds: *mut u64, max: u32, count: *mut u32);
    GalaWindowPost = 0x7f46, fn(hwnd: u64, message: u32, wparam: u64, lparam: u64);
    GalaWindowPosted = 0x7f47, fn(records: *mut u8, max: u32, count: *mut u32);
    GalaWindowSend = 0x7f48, fn(request: *const u8, id: *mut u64);
    GalaWindowSendWait = 0x7f49, fn(id: u64, ms: u32, result: *mut i64, out: *mut u8, out_len: *mut u32);
    GalaWindowReceive = 0x7f4a, fn(tid: u32, head: *mut u8, input: *mut u8, max: u32);
    GalaWindowReply = 0x7f4b, fn(id: u64, result: i64, out: *const u8, len: u32);
    GalaWindowPostThread = 0x7f50, fn(tid: u32, message: u32, wparam: u64, lparam: u64);
    GalaWindowSendAbandon = 0x7f51, fn(id: u64);
    GalaWindowSurface = 0x7f52, fn(hwnd: u64, section: usize, offset: u64, width: u32, height: u32);
    GalaWindowSurfaceOpen = 0x7f53, fn(hwnd: u64, section: *mut usize, info: *mut u8);
    GalaWindowDrag = 0x7f54, fn(id: u32);
    GalaWindowResize = 0x7f55, fn(id: u32, edges: u32);
    // UIPI's message filters (ChangeWindowMessageFilter(Ex)).
    GalaWindowFilter = 0x7f6a, fn(hwnd: u64, message: u32, action: u32, ext_status: *mut u32);
    // WinEvents across processes (SetWinEventHook, NotifyWinEvent).
    GalaWinEventHook = 0x7f70, fn(min: u32, max: u32, pid: u32, tid: u32, flags: u32, id: *mut u32);
    GalaWinEventUnhook = 0x7f71, fn(id: u32);
    GalaWinEventNotify = 0x7f72, fn(event: u32, hwnd: u64, object: i32, child: i32);
    GalaWinEventTake = 0x7f73, fn(records: *mut u8, max: u32, count: *mut u32);
    // gala's test-only clock control and its mocked random source (R7).
    GalaClockControl = 0x7f4c, fn(operation: u32, a: i64, b: i64, state: *mut u8);
    GalaRandom = 0x7f4d, fn(buffer: *mut u8, length: usize);
    // The shell's named actions (nt::action; 0x7f4c-0x7f57 are taken).
    GalaDesktopAction = 0x7f58, fn(request: *const u8, len: u32, out: *mut u8, out_len: u32, returned: *mut u32);
    // gala's number, UNVERIFIED.
    NtCreateDirectoryObject = 0x7f24, fn(handle: *mut usize, access: u32, attributes: *const ObjectAttributes);
    NtClearEvent = 0x3e, fn(h: usize);
    NtQuerySystemTime = 0x5a, fn(time: *mut i64);
    NtCreateEvent = 0x48, fn(
        handle: *mut usize, access: u32, attributes: *const ObjectAttributes, kind: u32,
        initial: u8
    );
    // More for gala's kernel32 (long-stable x64 numbers).
    NtQueryInformationProcess = 0x19, fn(
        process: isize, class: u32, buf: *mut u8, len: u32, ret: *mut u32
    );
    NtSetInformationProcess = 0x1c, fn(process: isize, class: u32, buf: *const u8, len: u32);
    NtQueryVirtualMemory = 0x23, fn(
        process: isize, address: usize, class: u32, buf: *mut u8, len: usize, ret: *mut usize
    );
    NtQuerySystemInformation = 0x36, fn(class: u32, buf: *mut u8, len: u32, ret: *mut u32);
    NtDuplicateObject = 0x3c, fn(
        source_process: isize, source: usize, target_process: isize, target: *mut usize,
        access: u32, attributes: u32, options: u32
    );
    NtYieldExecution = 0x46, fn();
    NtQueryInformationThread = 0x25, fn(
        h: usize, class: u32, buf: *mut u8, len: u32, ret: *mut u32
    );
    NtCreateThread = 0x4e, fn(
        handle: *mut usize, access: u32, attributes: *const ObjectAttributes, process: isize,
        client_id: *mut [usize; 2], context: *const u8, initial_teb: *const [usize; 5],
        suspended: u8
    );
    NtResumeThread = 0x52, fn(h: usize, previous: *mut u32);
    NtQueueApcThread = 0x45, fn(h: usize, routine: usize, a1: usize, a2: usize, a3: usize);
    // Files (long-stable x64 numbers).
    NtReadFile = 0x06, fn(
        h: usize, event: usize, apc: usize, apc_context: usize, iosb: *mut [usize; 2],
        buffer: *mut u8, length: u32, offset: *const i64, key: *const u32
    );
    // Device controls (AFD sockets; long-stable x64 number).
    NtDeviceIoControlFile = 0x07, fn(
        h: usize, event: usize, apc: usize, apc_context: usize, iosb: *mut [usize; 2],
        code: u32, input: *const u8, input_length: u32, output: *mut u8, output_length: u32
    );
    NtQueryInformationFile = 0x11, fn(
        h: usize, iosb: *mut [usize; 2], buf: *mut u8, len: u32, class: u32
    );
    NtQueryDirectoryFile = 0x35, fn(
        h: usize, event: usize, apc: usize, apc_context: usize, iosb: *mut [usize; 2],
        buf: *mut u8, len: u32, class: u32, single: u8, mask: *const UnicodeString,
        restart: u8
    );
    NtQueryAttributesFile = 0x3d, fn(attributes: *const ObjectAttributes, info: *mut u8);
    NtFlushBuffersFile = 0x4b, fn(h: usize, iosb: *mut [usize; 2]);
    NtCreateFile = 0x55, fn(
        handle: *mut usize, access: u32, attributes: *const ObjectAttributes,
        iosb: *mut [usize; 2], allocation: *const i64, file_attributes: u32, share: u32,
        disposition: u32, options: u32, ea: *const u8, ea_length: u32
    );
    NtQueryObject = 0x10, fn(h: usize, class: u32, buf: *mut u8, len: u32, ret: *mut u32);
    NtQueryVolumeInformationFile = 0x49, fn(
        h: usize, iosb: *mut [usize; 2], buf: *mut u8, len: u32, class: u32
    );
    NtSetInformationFile = 0x27, fn(
        h: usize, iosb: *mut [usize; 2], buf: *const u8, len: u32, class: u32
    );
    NtTerminateThread = 0x53, fn(h: isize, status: u32);
    NtWaitForMultipleObjects = 0x5b, fn(
        count: u32, handles: *const usize, wait_type: u32, alertable: u8, timeout: *const i64
    );
    // gala's own service: report a gap to the kernel's missing-features
    // ledger (not part of Windows; gala's DLLs only).
    GalaRecordMissing = 0x7f00, fn(name: *const u8, length: usize, what: u32, detail: u64);
    // gala's own: the exception no handler took, before the top-level
    // filter runs (null: the filter continued); the kernel's crash report
    // describes it whatever status the process then ends with.
    GalaUnhandledException = 0x7f5c, fn(record: *const u8, context: *const u8);
    // Waitable timers (gala's numbers, unverified).
    NtCreateTimer = 0xd9, fn(
        handle: *mut usize, access: u32, attributes: *const ObjectAttributes, kind: u32
    );
    NtOpenTimer = 0x13d, fn(handle: *mut usize, access: u32, attributes: *const ObjectAttributes);
    NtQueryTimer = 0x15f, fn(h: usize, class: u32, buf: *mut u8, len: u32, ret: *mut u32);
    NtSetTimer = 0x19e, fn(
        h: usize, due: *const i64, routine: usize, context: usize, resume: u8, period: i32,
        previous: *mut u8
    );
    NtCancelTimer = 0xbd, fn(h: usize, current: *mut u8);
    NtQueryDirectoryObject = 0x14a, fn(
        h: usize, buf: *mut u8, len: u32, single: u8, restart: u8, context: *mut u32,
        returned: *mut u32
    );
}

#[repr(C)]
pub struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *const u16,
}

#[repr(C)]
pub struct ObjectAttributes {
    length: u32,
    root_directory: usize,
    object_name: *const UnicodeString,
    attributes: u32,
    security_descriptor: usize,
    security_quality_of_service: usize,
}

const STATUS_SUCCESS: u32 = 0;
const STATUS_IMAGE_NOT_AT_BASE: u32 = 0x4000_0003;
/// A view of an image for another machine (informational: mapped).
const STATUS_IMAGE_MACHINE_TYPE_MISMATCH: u32 = 0x4000_000e;
const STATUS_DLL_NOT_FOUND: u32 = 0xc000_0135;
const STATUS_ENTRYPOINT_NOT_FOUND: u32 = 0xc000_0139;
const STATUS_INVALID_IMAGE_FORMAT: u32 = 0xc000_007b;

fn rd32(addr: usize) -> u32 {
    // SAFETY: callers read inside mapped images and system structures.
    unsafe { (addr as *const u32).read_unaligned() }
}

fn rd64(addr: usize) -> u64 {
    // SAFETY: as above.
    unsafe { (addr as *const u64).read_unaligned() }
}

/// The size of a pointer (and of an IAT slot, a TLS array slot).
const PTR: usize = core::mem::size_of::<usize>();

/// A pointer-sized value (an address in this build's images and system
/// structures).
fn rdp(addr: usize) -> usize {
    // SAFETY: as above.
    unsafe { (addr as *const usize).read_unaligned() }
}

/// The UNICODE_STRING at `addr` (Length, then the buffer a pointer
/// further).
fn unicode_at(addr: usize) -> &'static [u16] {
    let len = usize::from(rd32(addr) as u16) / 2;
    // SAFETY: a UNICODE_STRING the kernel or the loader filled in.
    unsafe { core::slice::from_raw_parts(rdp(addr + PTR) as *const u16, len) }
}

/// ProcessParameters->ImagePathName.
fn image_path() -> &'static [u16] {
    unicode_at(rdp(peb() + PEB_PROCESS_PARAMETERS) + PARAMS_IMAGE_PATH)
}

/// PEB.ImageBaseAddress: the executable.
fn image_base() -> usize {
    rdp(peb() + PEB_IMAGE_BASE)
}

/// A NUL-terminated string inside a mapped image.
fn c_str(addr: usize) -> &'static [u8] {
    let mut n = 0;
    // SAFETY: names in import/export tables are NUL-terminated.
    unsafe {
        while *((addr + n) as *const u8) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(addr as *const u8, n)
    }
}

/// The IMAGE_DATA_DIRECTORY `index` of the image at `base`: (rva, size).
fn directory(base: usize, index: usize) -> (usize, usize) {
    let opt = base + rd32(base + 0x3c) as usize + 24 + DATA_DIRECTORIES;
    (rd32(opt + 8 * index) as usize, rd32(opt + 4 + 8 * index) as usize)
}

/// Whether `address` is code in a loaded DLL's `.gstub` section: the
/// exports gala's DLLs define for functions not implemented yet
/// (`gala_stubs`). Imports bind to them so that programs load, but a
/// program probing with GetProcAddress gets "not found", as it did before
/// the stub existed, and takes its fallback.
fn is_unimplemented_stub(address: usize) -> bool {
    let base = with_modules(|m| {
        m.iter().map(|e| e.base).find(|&b| {
            b != 0 && address >= b && {
                let size = rd32(b + rd32(b + 0x3c) as usize + 24 + 56) as usize;
                address < b + size
            }
        })
    });
    let Some(base) = base else { return false };
    let nt = base + rd32(base + 0x3c) as usize;
    let count = (rd32(nt + 4) >> 16) as usize; // NumberOfSections
    let optional_size = (rd32(nt + 20) & 0xffff) as usize;
    let table = nt + 24 + optional_size;
    (0..count).any(|i| {
        let s = table + 40 * i;
        let start = base + rd32(s + 12) as usize;
        rd64(s) == u64::from_le_bytes(*b".gstub\0\0")
            && (start..start + rd32(s + 8) as usize).contains(&address)
    })
}

/// Export `name` (or `ordinal` if `name` is empty) of the image at `base`.
fn export(base: usize, name: &[u8], ordinal: u32) -> Option<usize> {
    export_at_depth(base, name, ordinal, 0)
}

/// `export`, following forwarders (`DLL.Function` or `DLL.#ordinal`
/// strings in place of an address) at most a few levels deep. A forwarder
/// loads its target DLL (a reference that stays, as on Windows).
fn export_at_depth(base: usize, name: &[u8], ordinal: u32, depth: u32) -> Option<usize> {
    let (rva, size) = directory(base, 0);
    if rva == 0 {
        return None;
    }
    let dir = base + rva;
    let functions = base + rd32(dir + 28) as usize;
    let index = if name.is_empty() {
        ordinal.checked_sub(rd32(dir + 16))? as usize // minus Base
    } else {
        let names = base + rd32(dir + 32) as usize;
        let ordinals = base + rd32(dir + 36) as usize;
        let i = (0..rd32(dir + 24) as usize)
            .find(|&i| c_str(base + rd32(names + 4 * i) as usize) == name)?;
        // SAFETY: inside the export directory.
        usize::from(unsafe { ((ordinals + 2 * i) as *const u16).read_unaligned() })
    };
    if index >= rd32(dir + 20) as usize {
        return None;
    }
    let target = rd32(functions + 4 * index) as usize;
    if !(rva..rva + size).contains(&target) {
        return Some(base + target);
    }
    // A forwarder string.
    if depth == 4 {
        return None;
    }
    let forward = c_str(base + target);
    let dot = forward.iter().rposition(|&b| b == b'.')?;
    let (dll, function) = (&forward[..dot], &forward[dot + 1..]);
    let mut module = [0u8; MAX_NAME];
    let len = normalize(dll.iter().map(|&b| u16::from(b)), &mut module).ok()?;
    let slot = load_module(&module[..len], Search::Default).ok()?;
    let target_base = with_modules(|m| m[slot].base);
    match function.strip_prefix(b"#") {
        Some(digits) => {
            let ordinal = digits
                .iter()
                .try_fold(0u32, |n, &d| d.is_ascii_digit().then(|| n * 10 + u32::from(d - b'0')))?;
            export_at_depth(target_base, &[], ordinal, depth + 1)
        }
        None => export_at_depth(target_base, function, 0, depth + 1),
    }
}

/// Opens `dll` (an ASCII file name) from the directory of the executable
/// (`ImagePathName`), else from `<SystemRoot>\System32` (KUSER_SHARED_DATA's
/// NtSystemRoot), and maps it; returns its base.
/// Where a DLL is looked for.
#[derive(Clone, Copy)]
enum Search<'a> {
    /// As the load's LOAD_LIBRARY_SEARCH_* flags say (`LOAD_FLAGS`; none:
    /// next to the executable, SetDllDirectory's directory, System32).
    Default,
    /// This directory only (a path with a directory was given; ends with
    /// a backslash).
    Dir(&'a [u16]),
}

fn map_dll(dll: &[u8], search: Search) -> Result<usize, u32> {
    if let Search::Dir(dir) = search {
        return open_dll(dir, dll);
    }
    // Tries `dir`; Ok(None) when the DLL is not there.
    let try_dir = |dir: &[u16]| match open_dll(dir, dll) {
        Err(STATUS_DLL_NOT_FOUND) => Ok(None),
        Err(e) => Err(e),
        Ok(base) => Ok(Some(base)),
    };
    // SAFETY: under the loader lock (as `map_dll` is).
    let (load_dir, load_len) = unsafe { *LOAD_DIR.0.get() };
    // SAFETY: as above.
    let flags = unsafe { *LOAD_FLAGS.0.get() };
    // LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR: the directory of the DLL being
    // loaded, first, for every dependency of this load.
    if load_len != 0
        && let Some(base) = try_dir(&load_dir[..load_len])?
    {
        return Ok(base);
    }
    let image_path = image_path();
    let dir_len = image_path.iter().rposition(|&c| c == u16::from(b'\\')).map_or(0, |i| i + 1);
    let mut system32 = [0u16; 280];
    let mut n = 0;
    // SAFETY: KUSER_SHARED_DATA is mapped read-only at 0x7ffe0000 in every
    // process; NtSystemRoot (at +0x30) is a NUL-terminated WCHAR[260].
    unsafe {
        while n < 260 && *((0x7ffe_0030 + 2 * n) as *const u16) != 0 {
            system32[n] = *((0x7ffe_0030 + 2 * n) as *const u16);
            n += 1;
        }
    }
    for &b in SYSTEM_DIR {
        system32[n] = u16::from(b);
        n += 1;
    }
    let where_flags =
        SEARCH_APPLICATION_DIR | SEARCH_USER_DIRS | SEARCH_SYSTEM32 | SEARCH_DEFAULT_DIRS;
    if flags & where_flags == 0 {
        // The standard order: the executable's directory, then
        // SetDllDirectory's (as documented), then System32.
        if let Some(base) = try_dir(&image_path[..dir_len])? {
            return Ok(base);
        }
        // SAFETY: under the loader lock (as `map_dll` is).
        let (dir, len) = unsafe { *DLL_DIRECTORY.0.get() };
        if len != 0
            && let Some(base) = try_dir(&dir[..len])?
        {
            return Ok(base);
        }
        return open_dll(&system32[..n], dll);
    }
    // LOAD_LIBRARY_SEARCH_*: the application's directory, the
    // AddDllDirectory directories, System32, each when asked for
    // (DEFAULT_DIRS: all three, in that order).
    let default = flags & SEARCH_DEFAULT_DIRS != 0;
    if (default || flags & SEARCH_APPLICATION_DIR != 0)
        && let Some(base) = try_dir(&image_path[..dir_len])?
    {
        return Ok(base);
    }
    if default || flags & SEARCH_USER_DIRS != 0 {
        for slot in 0..USER_DIRS {
            // SAFETY: under the loader lock.
            let (dir, len) = unsafe { (*USER_DIR_LIST.0.get())[slot] };
            if len != 0
                && let Some(base) = try_dir(&dir[..len])?
            {
                return Ok(base);
            }
        }
    }
    if default || flags & SEARCH_SYSTEM32 != 0 {
        return open_dll(&system32[..n], dll);
    }
    Err(STATUS_DLL_NOT_FOUND)
}

/// LoadLibraryExW's LOAD_LIBRARY_SEARCH_* flags (passed in LdrLoadDll's
/// DllPath with its low bit set).
const SEARCH_DLL_LOAD_DIR: usize = 0x100;
const SEARCH_APPLICATION_DIR: usize = 0x200;
const SEARCH_USER_DIRS: usize = 0x400;
const SEARCH_SYSTEM32: usize = 0x800;
const SEARCH_DEFAULT_DIRS: usize = 0x1000;

/// The search flags of the LdrLoadDll in progress (its dependencies are
/// searched the same way), under the loader lock.
struct LoadFlags(core::cell::UnsafeCell<usize>);

// SAFETY: used under the loader lock.
unsafe impl Sync for LoadFlags {}

static LOAD_FLAGS: LoadFlags = LoadFlags(core::cell::UnsafeCell::new(0));

/// LdrAddDllDirectory's directories (each ends with a backslash; length 0:
/// a free slot; a cookie is its slot + 1), under the loader lock.
const USER_DIRS: usize = 16;

struct UserDirs(core::cell::UnsafeCell<[([u16; MAX_PATH], usize); USER_DIRS]>);

// SAFETY: used under the loader lock.
unsafe impl Sync for UserDirs {}

static USER_DIR_LIST: UserDirs =
    UserDirs(core::cell::UnsafeCell::new([([0; MAX_PATH], 0); USER_DIRS]));

/// The directory LdrLoadDll's LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR adds in
/// front of the search for the dependencies of the DLL it loads (ends
/// with a backslash; empty: none), set for that call, under the loader
/// lock.
static LOAD_DIR: DllDirectory = DllDirectory(core::cell::UnsafeCell::new(([0; MAX_PATH], 0)));

/// SetDllDirectory's directory (ends with a backslash; empty: none), used
/// under the loader lock.
struct DllDirectory(core::cell::UnsafeCell<([u16; MAX_PATH], usize)>);

// SAFETY: used under the loader lock.
unsafe impl Sync for DllDirectory {}

static DLL_DIRECTORY: DllDirectory = DllDirectory(core::cell::UnsafeCell::new(([0; MAX_PATH], 0)));

/// Opens `dir` + `dll` (`dir` ends with a backslash) and maps it.
/// The Win32 path of the DLL `open_dll` opened last (used under the
/// loader lock, right after it returns).
struct LastPath(core::cell::UnsafeCell<([u16; MAX_PATH], usize)>);

// SAFETY: used under the loader lock.
unsafe impl Sync for LastPath {}

static LAST_PATH: LastPath = LastPath(core::cell::UnsafeCell::new(([0; MAX_PATH], 0)));

/// Records `path` as the module's file (truncated to MAX_PATH).
fn set_module_path(slot: usize, path: &[u16]) {
    let n = path.len().min(MAX_PATH);
    with_modules(|m| {
        m[slot].path[..n].copy_from_slice(&path[..n]);
        m[slot].path_len = n;
    });
    // The module is complete: into PEB.Ldr's lists.
    ldr_link(slot);
}

/// Resolves the `.` and `..` components of the absolute directory `dir`
/// (`C:\\a\\.\\b\\..\\` or `\\\\server\\share\\...`, backslashes, ending in
/// one) in place, as RtlGetFullPathName does: `.` goes, `..` takes the
/// component before it with it, never the drive or the share. Returns the
/// new length.
fn resolve_dots(dir: &mut [u16]) -> usize {
    let sep = u16::from(b'\\');
    let dot = u16::from(b'.');
    // The root that `..` never removes: `C:\` or `\\server\share\`.
    let root = if dir.len() >= 2 && dir[0] == sep && dir[1] == sep {
        let mut seps = 0;
        dir.iter()
            .position(|&c| {
                seps += usize::from(c == sep);
                seps == 4
            })
            .map_or(dir.len(), |i| i + 1)
    } else {
        dir.iter().position(|&c| c == sep).map_or(dir.len(), |i| i + 1)
    };
    let (mut read, mut write) = (root, root);
    while read < dir.len() {
        let end = dir[read..].iter().position(|&c| c == sep).map_or(dir.len(), |i| read + i);
        let part = read..end;
        let next = (end + 1).min(dir.len());
        if dir[part.clone()] == [dot] || part.is_empty() {
            // `.` (or an empty component): nothing.
        } else if dir[part.clone()] == [dot, dot] {
            // `..`: back to the separator before the last component.
            if write > root {
                write = dir[root..write - 1]
                    .iter()
                    .rposition(|&c| c == sep)
                    .map_or(root, |i| root + i + 1);
            }
        } else {
            dir.copy_within(read..next, write);
            write += next - read;
        }
        read = next;
    }
    write
}

fn open_dll(dir: &[u16], dll: &[u8]) -> Result<usize, u32> {
    let mut path = [0u16; 300];
    let prefix = *b"\\??\\";
    if prefix.len() + dir.len() + dll.len() > path.len() {
        return Err(STATUS_DLL_NOT_FOUND);
    }
    for (i, &b) in prefix.iter().enumerate() {
        path[i] = u16::from(b);
    }
    // A DOS path's forward slashes are backslashes in an NT path, and its
    // `.` and `..` components are resolved (as RtlDosPathNameToNtPathName
    // makes them).
    for (slot, &c) in path[prefix.len()..prefix.len() + dir.len()].iter_mut().zip(dir) {
        *slot = if c == u16::from(b'/') { u16::from(b'\\') } else { c };
    }
    let dir_len = resolve_dots(&mut path[prefix.len()..prefix.len() + dir.len()]);
    let total = prefix.len() + dir_len + dll.len();
    for (i, &b) in dll.iter().enumerate() {
        path[prefix.len() + dir_len + i] = u16::from(b);
    }
    let name = UnicodeString {
        length: (total * 2) as u16,
        maximum_length: (total * 2) as u16,
        buffer: path.as_ptr(),
    };
    let attrs = ObjectAttributes {
        length: core::mem::size_of::<ObjectAttributes>() as u32,
        root_directory: 0,
        object_name: &name,
        attributes: 0x40, // OBJ_CASE_INSENSITIVE
        security_descriptor: 0,
        security_quality_of_service: 0,
    };
    let (mut file, mut section, mut iosb) = (0usize, 0usize, [0usize; 2]);
    // GENERIC_READ | GENERIC_EXECUTE | SYNCHRONIZE; share read+delete;
    // FILE_SYNCHRONOUS_IO_NONALERT | FILE_NON_DIRECTORY_FILE.
    // SAFETY: under the loader lock (see `LastPath`).
    unsafe {
        let last = &mut *LAST_PATH.0.get();
        let n = (total - prefix.len()).min(MAX_PATH);
        last.0[..n].copy_from_slice(&path[prefix.len()..prefix.len() + n]);
        last.1 = n;
    }
    if NtOpenFile(&mut file, 0xa010_0000, &attrs, &mut iosb, 5, 0x60) != STATUS_SUCCESS {
        return Err(STATUS_DLL_NOT_FOUND);
    }
    // SECTION_ALL_ACCESS, PAGE_EXECUTE, SEC_IMAGE.
    let st = NtCreateSection(
        &mut section,
        0xf_001f,
        core::ptr::null(),
        core::ptr::null(),
        0x10,
        0x100_0000,
        file,
    );
    NtClose(file);
    // The section's own status (not a bad format: a kernel out of memory
    // says so, and LoadLibrary's error tells it apart).
    if st != STATUS_SUCCESS {
        return Err(st);
    }
    let (mut base, mut size) = (0usize, 0usize);
    // ViewShare, PAGE_READWRITE (image views take their own protections).
    let st =
        NtMapViewOfSection(section, -1, &mut base, 0, 0, core::ptr::null_mut(), &mut size, 1, 0, 4);
    NtClose(section);
    match st {
        STATUS_SUCCESS | STATUS_IMAGE_NOT_AT_BASE | STATUS_IMAGE_MACHINE_TYPE_MISMATCH => {}
        _ => return Err(st),
    }
    // Image sections take both kinds; an image of the other kind loads as
    // a module only if it is an IL-only .NET assembly (no code runs).
    if is_foreign(base) && !is_il_only(base) {
        NtUnmapViewOfSection(-1, base);
        return Err(STATUS_INVALID_IMAGE_FORMAT);
    }
    Ok(base)
}

/// Whether the image at `base` is not of this build's kind (PE32 for the
/// 64-bit ntdll, PE32+ for the 32-bit one).
fn is_foreign(base: usize) -> bool {
    rd32(base + rd32(base + 0x3c) as usize + 24) & 0xffff != NATIVE_MAGIC
}

/// Whether the image at `base` (of the other kind) is an IL-only .NET
/// assembly: its CLI header (data directory 14, at +96 + 14 * 8 in a PE32
/// optional header, +112 in a PE32+ one) has COMIMAGE_FLAGS_ILONLY (bit 0
/// of Flags, at +16). Windows loads these into processes of either kind;
/// nothing in them runs (no imports are bound, no entry point is called).
fn is_il_only(base: usize) -> bool {
    let optional = base + rd32(base + 0x3c) as usize + 24;
    let directories = if rd32(optional) & 0xffff == 0x10b { 96 } else { 112 };
    let cli = rd32(optional + directories + 14 * 8) as usize;
    cli != 0 && rd32(base + cli + 16) & 1 != 0
}

const STATUS_NO_MEMORY: u32 = 0xc000_0017;
const STATUS_PROCEDURE_NOT_FOUND: u32 = 0xc000_007a;
const STATUS_ORDINAL_NOT_FOUND: u32 = 0xc000_0138;
const STATUS_DLL_INIT_FAILED: u32 = 0xc000_0142;

/// Loaded modules (the JDK alone brings about 60 DLLs, most of them the
/// api-ms-win-* forwarders).
const MAX_MODULES: usize = 256;
pub(crate) const MODULE_COUNT: usize = MAX_MODULES;
/// DLLs one module imports from (ucrtbase alone imports from about 30
/// api-ms-win-core sets).
const MAX_DEPENDENCIES: usize = 64;
const MAX_NAME: usize = 64;
/// The longest module path kept.
const MAX_PATH: usize = 260;
/// Reference count of modules that are never unloaded (ntdll, the exe).
const PINNED: u32 = u32::MAX;

/// A loaded image: its base (0 for a free slot), reference count, base
/// name (lowercase ASCII), whether DllMain has seen DLL_PROCESS_ATTACH,
/// and the modules its imports hold references on.
#[derive(Clone, Copy)]
struct Module {
    base: usize,
    refs: u32,
    name: [u8; MAX_NAME],
    name_len: usize,
    attached: bool,
    /// DisableThreadLibraryCalls: no DLL_THREAD_* to its DllMain.
    no_thread_calls: bool,
    /// The file it was loaded from (a Win32 path, `C:\...`).
    path: [u16; MAX_PATH],
    path_len: usize,
    dependencies: [usize; MAX_DEPENDENCIES],
    dependency_count: usize,
}

const FREE: Module = Module {
    base: 0,
    refs: 0,
    name: [0; MAX_NAME],
    name_len: 0,
    attached: false,
    no_thread_calls: false,
    path: [0; MAX_PATH],
    path_len: 0,
    dependencies: [0; MAX_DEPENDENCIES],
    dependency_count: 0,
};

/// The loader lock: one thread at a time in the loader (loading,
/// unloading, thread notifications), recursively (DllMain may load DLLs).
/// Laid out as an RTL_CRITICAL_SECTION (DebugInfo, LockCount,
/// RecursionCount, OwningThread at +0x10, LockSemaphore, SpinCount), whose
/// address is PEB.LoaderLock (+0x110): programs read the owner from there
/// to tell whether they run under the loader lock (Crashpad's
/// IsThreadInLoaderLock, in Chromium's chrome_elf.dll). gala's loader only
/// uses the owner and the depth (touched only by the owner).
#[repr(C)]
struct LoaderLock {
    debug_info: usize,
    lock_count: i32,
    depth: core::sync::atomic::AtomicU32,
    owner: core::sync::atomic::AtomicUsize,
    semaphore: usize,
    spin_count: usize,
}

static LOADER_LOCK: LoaderLock = LoaderLock {
    debug_info: 0,
    lock_count: -1,
    depth: core::sync::atomic::AtomicU32::new(0),
    owner: core::sync::atomic::AtomicUsize::new(0),
    semaphore: 0,
    spin_count: 0,
};
static LOADER_OWNER: &core::sync::atomic::AtomicUsize = &LOADER_LOCK.owner;
static LOADER_DEPTH: &core::sync::atomic::AtomicU32 = &LOADER_LOCK.depth;

/// Set while the executable's imports are bound at startup.
static STARTUP: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Runs `f` holding the loader lock.
pub(crate) fn with_loader_lock<R>(f: impl FnOnce() -> R) -> R {
    use core::sync::atomic::Ordering;
    let me = thread_id();
    if LOADER_OWNER.load(Ordering::Acquire) == me {
        LOADER_DEPTH.fetch_add(1, Ordering::Relaxed);
        let r = f();
        LOADER_DEPTH.fetch_sub(1, Ordering::Relaxed);
        return r;
    }
    let mut spins = 0u32;
    while LOADER_OWNER.compare_exchange(0, me, Ordering::Acquire, Ordering::Relaxed).is_err() {
        spins += 1;
        if spins < 100 {
            core::hint::spin_loop();
        } else {
            NtYieldExecution();
        }
    }
    LOADER_DEPTH.store(1, Ordering::Relaxed);
    let r = f();
    LOADER_DEPTH.store(0, Ordering::Relaxed);
    LOADER_OWNER.store(0, Ordering::Release);
    r
}

/// LIST_ENTRY.
#[repr(C)]
#[derive(Clone, Copy)]
struct ListEntry {
    flink: usize,
    blink: usize,
}

/// Padding words of the x64 layouts (UNICODE_STRING's after the lengths,
/// LDR_DATA_TABLE_ENTRY's after SizeOfImage); none in the x86 ones.
const PAD: usize = if PTR == 8 { 1 } else { 0 };

/// UNICODE_STRING (lengths in bytes, padding on x64, the buffer).
#[repr(C)]
#[derive(Clone, Copy)]
struct UnicodeStr {
    length: u16,
    maximum: u16,
    pad: [u32; PAD],
    buffer: usize,
}

/// PEB_LDR_DATA (x64: 0x58 bytes; x86: 0x30), what PEB.Ldr points at.
#[repr(C)]
struct LdrData {
    length: u32,
    initialized: u32,
    ss_handle: usize,
    load_order: ListEntry,
    memory_order: ListEntry,
    init_order: ListEntry,
    entry_in_progress: usize,
    shutdown_in_progress: u32,
    shutdown_thread: usize,
}

/// LDR_DATA_TABLE_ENTRY, the fields up to TimeDateStamp (x64: +0x80;
/// x86: +0x44).
#[repr(C)]
#[derive(Clone, Copy)]
struct LdrEntry {
    load_order: ListEntry,
    memory_order: ListEntry,
    init_order: ListEntry,
    dll_base: usize,
    entry_point: usize,
    size_of_image: u32,
    pad: [u32; PAD],
    full_name: UnicodeStr,
    base_name: UnicodeStr,
    flags: u32,
    load_count: u16,
    tls_index: u16,
    hash_links: ListEntry,
    time_date_stamp: u32,
    pad2: u32,
}

#[cfg(target_arch = "x86_64")]
const _: () = {
    assert!(core::mem::size_of::<LdrData>() == 0x58);
    assert!(core::mem::offset_of!(LdrEntry, full_name) == 0x48);
    assert!(core::mem::offset_of!(LdrEntry, time_date_stamp) == 0x80);
};
#[cfg(target_arch = "x86")]
const _: () = {
    assert!(core::mem::size_of::<LdrData>() == 0x30);
    assert!(core::mem::offset_of!(LdrEntry, full_name) == 0x24);
    assert!(core::mem::offset_of!(LdrEntry, time_date_stamp) == 0x44);
};

const NO_LIST: ListEntry = ListEntry { flink: 0, blink: 0 };
const NO_STRING: UnicodeStr = UnicodeStr { length: 0, maximum: 0, pad: [0; PAD], buffer: 0 };
const NO_ENTRY: LdrEntry = LdrEntry {
    load_order: NO_LIST,
    memory_order: NO_LIST,
    init_order: NO_LIST,
    dll_base: 0,
    entry_point: 0,
    size_of_image: 0,
    pad: [0; PAD],
    full_name: NO_STRING,
    base_name: NO_STRING,
    flags: 0,
    load_count: 0,
    tls_index: 0,
    hash_links: NO_LIST,
    time_date_stamp: 0,
    pad2: 0,
};

/// The loader's lists as Windows keeps them (PEB.Ldr), for code that walks
/// them: debuggers, crash reporters (Crashpad's handler reads a crashed
/// process's InLoadOrderModuleList to list its modules), module
/// enumeration. One entry per module slot, in ntdll's own data (so in the
/// process's memory, where another process can read it): the executable
/// first in load order, then ntdll, then each DLL as it is mapped; in
/// initialization order each DLL as its DllMain is first called (ntdll
/// first, the executable never). Changed under the loader lock only.
struct Ldr(core::cell::UnsafeCell<(LdrData, [LdrEntry; MAX_MODULES])>);

// SAFETY: as `Modules`.
unsafe impl Sync for Ldr {}

static LDR: Ldr = Ldr(core::cell::UnsafeCell::new((
    LdrData {
        length: core::mem::size_of::<LdrData>() as u32,
        initialized: 0,
        ss_handle: 0,
        load_order: NO_LIST,
        memory_order: NO_LIST,
        init_order: NO_LIST,
        entry_in_progress: 0,
        shutdown_in_progress: 0,
        shutdown_thread: 0,
    },
    [NO_ENTRY; MAX_MODULES],
)));

/// The list heads and entries (under the loader lock).
fn ldr() -> &'static mut (LdrData, [LdrEntry; MAX_MODULES]) {
    // SAFETY: changed under the loader lock or before a second thread.
    unsafe { &mut *LDR.0.get() }
}

/// Makes the list heads empty (pointing at themselves) and PEB.Ldr point
/// at them.
fn ldr_init() {
    let (data, _) = ldr();
    for head in [&mut data.load_order, &mut data.memory_order, &mut data.init_order] {
        let me = head as *mut ListEntry as usize;
        *head = ListEntry { flink: me, blink: me };
    }
    // SAFETY: PEB.Ldr, written by this loader only.
    unsafe { ((peb() + PEB_LDR) as *mut usize).write(data as *mut LdrData as usize) };
}

/// Puts `entry` last in the list at `head`.
fn list_append(head: *mut ListEntry, entry: *mut ListEntry) {
    // SAFETY: a list head and an entry of `LDR`, under the loader lock.
    unsafe {
        let last = (*head).blink as *mut ListEntry;
        (*entry).flink = head as usize;
        (*entry).blink = last as usize;
        (*last).flink = entry as usize;
        (*head).blink = entry as usize;
    }
}

/// Takes `entry` out of its list (if it is in one).
fn list_remove(entry: *mut ListEntry) {
    // SAFETY: an entry of `LDR`, under the loader lock.
    unsafe {
        if (*entry).flink == 0 {
            return;
        }
        let (prev, next) = ((*entry).blink as *mut ListEntry, (*entry).flink as *mut ListEntry);
        (*prev).flink = next as usize;
        (*next).blink = prev as usize;
        *entry = NO_LIST;
    }
}

/// Fills slot `slot`'s entry from its module (base, image size, entry
/// point, time stamp, the file's full and base names) and appends it to
/// the load and memory order lists.
fn ldr_link(slot: usize) {
    let (base, refs) = with_modules(|m| (m[slot].base, m[slot].refs));
    let (path, len) = with_modules(|m| (m[slot].path.as_ptr() as usize, m[slot].path_len));
    let (data, entries) = ldr();
    let e = &mut entries[slot];
    list_remove(&mut e.load_order);
    list_remove(&mut e.memory_order);
    list_remove(&mut e.init_order);
    let pe = base + rd32(base + 0x3c) as usize;
    let entry = rd32(pe + 24 + 16) as usize;
    // SAFETY: the module's path, `len` units in its slot.
    let units = unsafe { core::slice::from_raw_parts(path as *const u16, len) };
    let file = units.iter().rposition(|&c| c == u16::from(b'\\')).map_or(0, |i| i + 1);
    *e = LdrEntry {
        dll_base: base,
        entry_point: if entry == 0 { 0 } else { base + entry },
        size_of_image: rd32(pe + 24 + 56),
        full_name: UnicodeStr {
            length: (2 * len) as u16,
            maximum: (2 * len) as u16,
            pad: [0; PAD],
            buffer: path,
        },
        base_name: UnicodeStr {
            length: (2 * (len - file)) as u16,
            maximum: (2 * (len - file)) as u16,
            pad: [0; PAD],
            buffer: path + 2 * file,
        },
        load_count: if refs == PINNED { u16::MAX } else { refs.min(0xfffe) as u16 },
        time_date_stamp: rd32(pe + 8),
        ..NO_ENTRY
    };
    list_append(&mut data.load_order, &mut e.load_order);
    list_append(&mut data.memory_order, &mut e.memory_order);
}

/// Appends slot `slot`'s entry to the initialization order list (its
/// DllMain is about to run for the first time).
fn ldr_initialized(slot: usize) {
    let (data, entries) = ldr();
    if entries[slot].init_order.flink == 0 && entries[slot].dll_base != 0 {
        list_append(&mut data.init_order, &mut entries[slot].init_order);
    }
}

/// Takes slot `slot`'s entry out of the lists (its module is unloaded).
fn ldr_unlink(slot: usize) {
    let (_, entries) = ldr();
    let e = &mut entries[slot];
    list_remove(&mut e.load_order);
    list_remove(&mut e.memory_order);
    list_remove(&mut e.init_order);
    *e = NO_ENTRY;
}

/// RTL_UNLOAD_EVENT_TRACE (x64: 104 bytes; x86: 92): one unloaded module.
#[repr(C)]
#[derive(Clone, Copy)]
struct UnloadEvent {
    base: usize,
    size_of_image: usize,
    sequence: u32,
    time_date_stamp: u32,
    checksum: u32,
    image_name: [u16; 32],
    version: [u32; 2],
}

const _: () = assert!(core::mem::size_of::<UnloadEvent>() == if PTR == 8 { 104 } else { 92 });

/// How many unloads the trace keeps (the newest replace the oldest).
const UNLOAD_EVENTS: usize = 64;

const NO_UNLOAD: UnloadEvent = UnloadEvent {
    base: 0,
    size_of_image: 0,
    sequence: 0,
    time_date_stamp: 0,
    checksum: 0,
    image_name: [0; 32],
    version: [0; 2],
};

/// The unload event trace (RtlGetUnloadEventTrace/Ex): the element size
/// and count, the pointer to the array (NULL until the first unload, as
/// on Windows), the array, the next sequence number. Changed under the
/// loader lock; another process reads it (Crashpad's reports list the
/// unloaded modules).
struct UnloadTrace(core::cell::UnsafeCell<(u32, u32, usize, [UnloadEvent; UNLOAD_EVENTS], u32)>);

// SAFETY: as `Modules`.
unsafe impl Sync for UnloadTrace {}

static UNLOADS: UnloadTrace = UnloadTrace(core::cell::UnsafeCell::new((
    core::mem::size_of::<UnloadEvent>() as u32,
    UNLOAD_EVENTS as u32,
    0,
    [NO_UNLOAD; UNLOAD_EVENTS],
    0,
)));

/// Records the unload of the module in `slot` (its image still mapped).
fn record_unload(slot: usize) {
    let (base, path, len) =
        with_modules(|m| (m[slot].base, m[slot].path.as_ptr() as usize, m[slot].path_len));
    // SAFETY: under the loader lock (see `UnloadTrace`).
    let trace = unsafe { &mut *UNLOADS.0.get() };
    let pe = base + rd32(base + 0x3c) as usize;
    // SAFETY: the module's path, `len` units in its slot.
    let units = unsafe { core::slice::from_raw_parts(path as *const u16, len) };
    let file = units.iter().rposition(|&c| c == u16::from(b'\\')).map_or(0, |i| i + 1);
    let mut event = UnloadEvent {
        base,
        size_of_image: rd32(pe + 24 + 56) as usize,
        sequence: trace.4,
        time_date_stamp: rd32(pe + 8),
        checksum: rd32(pe + 24 + 64),
        ..NO_UNLOAD
    };
    for (d, &c) in event.image_name[..31].iter_mut().zip(&units[file..]) {
        *d = c;
    }
    let i = trace.4 as usize % UNLOAD_EVENTS;
    trace.3[i] = event;
    trace.4 = trace.4.wrapping_add(1);
    trace.2 = trace.3.as_ptr() as usize;
}

/// RtlGetUnloadEventTraceEx(*ElementSize, *ElementCount, *EventTrace):
/// where the element size, the count and the pointer to the array of the
/// last unloaded modules are (as documented; the pointer is NULL until a
/// module is unloaded).
///
/// # Safety
/// The outputs must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn RtlGetUnloadEventTraceEx(
    size: *mut *mut u32,
    count: *mut *mut u32,
    trace: *mut *mut usize,
) {
    let t = UNLOADS.0.get();
    // SAFETY: the caller's outputs; addresses of the static's fields.
    unsafe {
        if !size.is_null() {
            *size = &raw mut (*t).0;
        }
        if !count.is_null() {
            *count = &raw mut (*t).1;
        }
        if !trace.is_null() {
            *trace = &raw mut (*t).2;
        }
    }
}

/// RtlGetUnloadEventTrace: the array of the last unloaded modules.
#[unsafe(no_mangle)]
pub extern "system" fn RtlGetUnloadEventTrace() -> *mut u8 {
    // SAFETY: the static's array.
    unsafe { (*UNLOADS.0.get()).3.as_mut_ptr().cast() }
}

struct Modules(core::cell::UnsafeCell<[Module; MAX_MODULES]>);

// SAFETY: used under the loader lock (`with_loader_lock`), or before a
// second thread exists.
unsafe impl Sync for Modules {}

static MODULES: Modules = Modules(core::cell::UnsafeCell::new([FREE; MAX_MODULES]));

/// Runs `f` on the module table. `f` must not call back into the loader
/// (no DllMain calls inside).
fn with_modules<R>(f: impl FnOnce(&mut [Module; MAX_MODULES]) -> R) -> R {
    // SAFETY: single-threaded (see `Modules`), and `f` does not re-enter.
    f(unsafe { &mut *MODULES.0.get() })
}

/// The base of the loaded image containing `pc`, if any.
pub(crate) fn image_of(pc: usize) -> Option<usize> {
    with_modules(|m| {
        m.iter().find_map(|module| {
            let base = module.base;
            if base == 0 || pc < base {
                return None;
            }
            let size = rd32(base + rd32(base + 0x3c) as usize + 24 + 56) as usize; // SizeOfImage (PE32 and PE32+)
            (pc < base + size).then_some(base)
        })
    })
}

/// RtlPcToFileHeader(PcValue, *BaseOfImage): the base of the image
/// containing `pc` (also stored), or null.
///
/// # Safety
/// `base` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn RtlPcToFileHeader(pc: usize, base: *mut usize) -> usize {
    let found = image_of(pc).unwrap_or(0);
    if !base.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { *base = found };
    }
    found
}

/// The slot of the module named `name` (already normalized).
fn find_module(name: &[u8]) -> Option<usize> {
    with_modules(|m| m.iter().position(|m| m.base != 0 && &m.name[..m.name_len] == name))
}

/// The loaded module mapped from `dir` (a path-qualified load's
/// directory, ending in a backslash) and `name` (normalized): the same
/// file, compared as Windows compares paths (case-insensitively, `/` as
/// `\`, `.` and `..` resolved). Another directory's DLL of that name is
/// another module.
fn find_module_at(dir: &[u16], name: &[u8]) -> Option<usize> {
    let mut want = [0u16; MAX_PATH];
    if dir.len() + name.len() > want.len() {
        return None;
    }
    for (slot, &c) in want.iter_mut().zip(dir) {
        *slot = if c == u16::from(b'/') { u16::from(b'\\') } else { c };
    }
    let n = resolve_dots(&mut want[..dir.len()]);
    for (i, &b) in name.iter().enumerate() {
        want[n + i] = u16::from(b);
    }
    let want = &want[..n + name.len()];
    let fold = |c: u16| if (u16::from(b'A')..=u16::from(b'Z')).contains(&c) { c + 32 } else { c };
    with_modules(|m| {
        m.iter().position(|m| {
            m.base != 0
                && &m.name[..m.name_len] == name
                && m.path_len == want.len()
                && m.path[..m.path_len].iter().zip(want).all(|(&a, &b)| fold(a) == fold(b))
        })
    })
}

/// Records a new module; returns its slot.
fn add_module(name: &[u8], base: usize, refs: u32) -> Result<usize, u32> {
    if name.len() > MAX_NAME {
        return Err(STATUS_DLL_NOT_FOUND);
    }
    with_modules(|m| {
        let Some(i) = m.iter().position(|m| m.base == 0) else {
            let what = b"ntdll.dll!LdrLoadDll(module table full)";
            GalaRecordMissing(what.as_ptr(), what.len(), 2, MAX_MODULES as u64);
            return Err(STATUS_NO_MEMORY);
        };
        m[i] = Module { base, refs, name_len: name.len(), ..FREE };
        m[i].name[..name.len()].copy_from_slice(name);
        Ok(i)
    })
}

/// A module's normalized name: the file part of `path`, lowercased, with
/// ".dll" appended if it has no extension.
fn normalize(path: impl Iterator<Item = u16>, out: &mut [u8; MAX_NAME]) -> Result<usize, u32> {
    let mut n = 0;
    let mut dot = false;
    for c in path {
        if c == u16::from(b'\\') || c == u16::from(b'/') {
            n = 0;
            dot = false;
            continue;
        }
        let c = u8::try_from(c).ok().filter(u8::is_ascii).ok_or(STATUS_DLL_NOT_FOUND)?;
        *out.get_mut(n).ok_or(STATUS_DLL_NOT_FOUND)? = c.to_ascii_lowercase();
        dot |= c == b'.';
        n += 1;
    }
    if !dot {
        out.get_mut(n..n + 4).ok_or(STATUS_DLL_NOT_FOUND)?.copy_from_slice(b".dll");
        n += 4;
    }
    if let Some(host) = api_set_host(&out[..n]) {
        out[..host.len()].copy_from_slice(host);
        n = host.len();
    }
    Ok(n)
}

/// RtlDllShutdownInProgress: whether the process is shutting down (DLLs
/// get DLL_PROCESS_DETACH with a non-null reserved argument then). gala's
/// ExitProcess does not call DLLs back yet, so never.
#[unsafe(no_mangle)]
pub extern "system" fn RtlDllShutdownInProgress() -> u8 {
    0
}

/// The process's placeholder compatibility mode (PHCM_*; 0 is the
/// application default). gala's files have no placeholders, so it only
/// matters as a value read back.
static PLACEHOLDER_MODE: core::sync::atomic::AtomicI8 = core::sync::atomic::AtomicI8::new(0);

#[unsafe(no_mangle)]
pub extern "system" fn RtlQueryProcessPlaceholderCompatibilityMode() -> i8 {
    PLACEHOLDER_MODE.load(core::sync::atomic::Ordering::Relaxed)
}

/// RtlSetProcessPlaceholderCompatibilityMode: the previous mode, or
/// PHCM_ERROR_INVALID_PARAMETER (-1) for a mode outside 0..=2.
#[unsafe(no_mangle)]
pub extern "system" fn RtlSetProcessPlaceholderCompatibilityMode(mode: i8) -> i8 {
    if !(0..=2).contains(&mode) {
        return -1;
    }
    PLACEHOLDER_MODE.swap(mode, core::sync::atomic::Ordering::Relaxed)
}

/// The core API sets Windows' schema does not give kernelbase (as Wine's
/// apisetschema.spec lists them), by contract name up to its level, and
/// their hosts; gala's ole32 stands in for combase, coml2 and wintypes.
const CORE_HOSTS: &[(&[u8], &[u8])] = &[
    (b"core-apiquery-l1-", b"ntdll.dll"),
    (b"core-rtlsupport-l1-", b"ntdll.dll"),
    (b"core-xstate-l1-", b"ntdll.dll"),
    (b"core-appinit-l1-", b"kernel32.dll"),
    (b"core-atoms-l1-", b"kernel32.dll"),
    (b"core-calendar-l1-", b"kernel32.dll"),
    (b"core-console-ansi-l2-", b"kernel32.dll"),
    (b"core-file-ansi-l1-", b"kernel32.dll"),
    (b"core-file-ansi-l2-", b"kernel32.dll"),
    (b"core-firmware-l1-", b"kernel32.dll"),
    (b"core-heap-obsolete-l1-", b"kernel32.dll"),
    (b"core-io-l1-", b"kernel32.dll"),
    (b"core-job-l2-", b"kernel32.dll"),
    (b"core-kernel32-legacy-ansi-l1-", b"kernel32.dll"),
    (b"core-kernel32-legacy-l1-", b"kernel32.dll"),
    (b"core-kernel32-private-l1-", b"kernel32.dll"),
    (b"core-localization-ansi-l1-", b"kernel32.dll"),
    (b"core-namedpipe-ansi-l1-", b"kernel32.dll"),
    (b"core-namespace-ansi-l1-", b"kernel32.dll"),
    (b"core-privateprofile-l1-", b"kernel32.dll"),
    (b"core-processenvironment-ansi-l1-", b"kernel32.dll"),
    (b"core-processsecurity-l1-", b"kernel32.dll"),
    (b"core-processthreads-l1-", b"kernel32.dll"),
    (b"core-processtopology-obsolete-l1-", b"kernel32.dll"),
    (b"core-sidebyside-ansi-l1-", b"kernel32.dll"),
    (b"core-string-obsolete-l1-", b"kernel32.dll"),
    (b"core-synch-ansi-l1-", b"kernel32.dll"),
    (b"core-toolhelp-l1-", b"kernel32.dll"),
    (b"core-ums-l1-", b"kernel32.dll"),
    (b"core-util-l1-", b"kernel32.dll"),
    (b"core-registry-l2-", b"advapi32.dll"),
    (b"core-registry-private-l1-", b"advapi32.dll"),
    (b"core-shutdown-ansi-l1-", b"advapi32.dll"),
    (b"core-shutdown-l1-", b"advapi32.dll"),
    (b"core-sysinfo-l2-", b"advapi32.dll"),
    (b"core-com-", b"ole32.dll"),
    (b"core-marshal-l1-", b"ole32.dll"),
    (b"core-winrt-", b"ole32.dll"),
    (b"core-crt-", b"msvcrt.dll"),
    (b"core-multipleproviderrouter-l1-", b"mpr.dll"),
    (b"core-featurestaging-l1-", b"shcore.dll"),
    (b"core-slapi-l1-", b"slc.dll"),
    (b"core-biplmapi-", b"twinapi.appcore.dll"),
    (b"core-biptcltapi-", b"twinapi.appcore.dll"),
    (b"core-psm-", b"twinapi.appcore.dll"),
];

/// The DLL implementing API set `name` (`api-ms-win-...`, normalized), as
/// Windows' API set schema maps these contract names to the DLLs that
/// implement them (whatever files of those names exist): the core sets to
/// kernelbase but those of CORE_HOSTS, the C runtime sets to ucrtbase;
/// gala keeps the rest coarse: security, eventing and services are
/// advapi32; other names are looked up as files.
fn api_set_host(name: &[u8]) -> Option<&'static [u8]> {
    let set = name.strip_prefix(b"api-ms-win-")?;
    let host: &[u8] = if set.starts_with(b"crt-") {
        b"ucrtbase.dll"
    } else if set.starts_with(b"core-") {
        CORE_HOSTS.iter().find(|(p, _)| set.starts_with(p)).map_or(b"kernelbase.dll", |&(_, h)| h)
    } else if ["security-", "eventing-", "service-"].iter().any(|p| set.starts_with(p.as_bytes())) {
        b"advapi32.dll"
    } else if set.starts_with(b"shcore-") {
        b"shcore.dll"
    } else if set.starts_with(b"power-") {
        b"powrprof.dll"
    } else {
        // Other sets are looked up as files, except the downlevel ones,
        // which name the DLL that implements them
        // (api-ms-win-downlevel-shell32-l1-1-0: CommandLineToArgvW).
        let rest = set.strip_prefix(b"downlevel-")?;
        const DOWNLEVEL: [&[u8]; 8] = [
            b"kernel32",
            b"advapi32",
            b"user32",
            b"shell32",
            b"shlwapi",
            b"ole32",
            b"oleaut32",
            b"version",
        ];
        const HOSTS: [&[u8]; 8] = [
            b"kernel32.dll",
            b"advapi32.dll",
            b"user32.dll",
            b"shell32.dll",
            b"shlwapi.dll",
            b"ole32.dll",
            b"oleaut32.dll",
            b"version.dll",
        ];
        let i = DOWNLEVEL
            .iter()
            .position(|d| rest.starts_with(d) && rest.get(d.len()) == Some(&b'-'))?;
        HOSTS[i]
    };
    Some(host)
}

/// The entry point of the image at `base`, if it has one.
fn entry_point(base: usize) -> Option<extern "system" fn(usize, u32, usize) -> i32> {
    let rva = rd32(base + rd32(base + 0x3c) as usize + 24 + 16) as usize;
    // SAFETY: the image's entry point (DllMain signature for a DLL).
    (rva != 0).then(|| unsafe {
        core::mem::transmute::<usize, extern "system" fn(usize, u32, usize) -> i32>(base + rva)
    })
}

/// Loads the DLL `name` (normalized) or takes another reference on it:
/// maps it from the executable's directory, binds its imports (loading
/// those DLLs first) and calls its DllMain with DLL_PROCESS_ATTACH once.
fn load_module(name: &[u8], search: Search) -> Result<usize, u32> {
    let loaded = match search {
        Search::Dir(dir) => find_module_at(dir, name),
        Search::Default => find_module(name),
    };
    if let Some(i) = loaded {
        return Ok(with_modules(|m| {
            if m[i].refs != PINNED {
                m[i].refs += 1;
            }
            i
        }));
    }
    let base = match map_dll(name, search) {
        Ok(base) => base,
        Err(status) => {
            if status == STATUS_DLL_NOT_FOUND {
                // Gaps are never silent: the ledger names the DLL.
                GalaRecordMissing(name.as_ptr(), name.len(), 4, 0);
            }
            return Err(status);
        }
    };
    // The executable's static imports (loaded at startup) stay for the
    // process's lifetime, as on Windows: FreeLibrary does not unload them.
    let refs = if STARTUP.load(core::sync::atomic::Ordering::Relaxed) { PINNED } else { 1 };
    let slot = match add_module(name, base, refs) {
        Ok(slot) => slot,
        Err(status) => {
            NtUnmapViewOfSection(-1, base);
            return Err(status);
        }
    };
    // SAFETY: under the loader lock; map_dll's last open_dll succeeded.
    let (path, n) = unsafe { *LAST_PATH.0.get() };
    set_module_path(slot, &path[..n]);
    let attach = || {
        if is_foreign(base) {
            return Ok(()); // an IL-only assembly: mapped, nothing to run
        }
        bind_imports(base, Some(slot))?;
        // Its TLS, then its TLS callbacks and DllMain (DLL_PROCESS_ATTACH).
        if tls::register(base) {
            tls::callbacks(base, tls::DLL_PROCESS_ATTACH);
        }
        with_modules(|m| m[slot].attached = true);
        ldr_initialized(slot);
        // lpvReserved: non-NULL for a DLL loaded with the program (the
        // initial context, as Windows passes), NULL for LoadLibrary.
        let reserved = if STARTUP.load(core::sync::atomic::Ordering::Relaxed) {
            INITIAL_CONTEXT.load(core::sync::atomic::Ordering::Relaxed)
        } else {
            0
        };
        match entry_point(base) {
            Some(entry) if entry(base, 1, reserved) == 0 => Err(STATUS_DLL_INIT_FAILED),
            _ => Ok(()),
        }
    };
    if let Err(status) = attach() {
        unload_module(slot);
        return Err(status);
    }
    Ok(slot)
}

/// Drops a reference on the module in `slot`; the last one detaches it
/// (DLL_PROCESS_DETACH), unmaps it and releases its dependencies.
fn unload_module(slot: usize) {
    let module = with_modules(|m| {
        if m[slot].refs == PINNED {
            return None;
        }
        m[slot].refs -= 1;
        if m[slot].refs != 0 {
            return None;
        }
        Some(m[slot])
    });
    let Some(module) = module else { return };
    // Recorded while its slot still has its path and its image is mapped.
    record_unload(slot);
    with_modules(|m| m[slot] = FREE);
    ldr_unlink(slot);
    if module.attached
        && let Some(entry) = entry_point(module.base)
    {
        entry(module.base, 0, 0); // DLL_PROCESS_DETACH
    }
    tls::unregister(module.base);
    NtUnmapViewOfSection(-1, module.base);
    for &dependency in module.dependencies[..module.dependency_count].iter().rev() {
        unload_module(dependency);
    }
}

/// Binds the imports of the image at `base` (the module in `slot`, or the
/// executable): loads each DLL and fills the IAT.
// TODO: TLS, delay-load imports, forwarders, bound imports.
fn bind_imports(base: usize, slot: Option<usize>) -> Result<(), u32> {
    let (rva, _) = directory(base, 1);
    if rva == 0 {
        return Ok(());
    }
    let mut desc = base + rva;
    let mut missing = false;
    while rd32(desc + 12) != 0 {
        let mut name = [0u8; MAX_NAME];
        let len = normalize(
            c_str(base + rd32(desc + 12) as usize).iter().map(|&b| u16::from(b)),
            &mut name,
        )?;
        let dependency = load_module(&name[..len], Search::Default)?;
        if let Some(slot) = slot {
            let recorded = with_modules(|m| {
                let n = m[slot].dependency_count;
                let free = m[slot].dependencies.get_mut(n)?;
                *free = dependency;
                m[slot].dependency_count += 1;
                Some(())
            });
            if recorded.is_none() {
                let what = b"ntdll.dll!LdrLoadDll(too many dependencies)";
                GalaRecordMissing(what.as_ptr(), what.len(), 2, MAX_DEPENDENCIES as u64);
                unload_module(dependency);
                return Err(STATUS_NO_MEMORY);
            }
        }
        let dll = with_modules(|m| m[dependency].base);
        let lookup = base + rd32(if rd32(desc) != 0 { desc } else { desc + 16 }) as usize;
        let iat = base + rd32(desc + 16) as usize;
        let mut count = 0;
        while rdp(lookup + PTR * count) != 0 {
            count += 1;
        }
        let (mut at, mut size, mut old) = (iat, count * PTR, 0u32);
        if NtProtectVirtualMemory(-1, &mut at, &mut size, 0x04, &mut old) != STATUS_SUCCESS {
            return Err(STATUS_ENTRYPOINT_NOT_FOUND);
        }
        for i in 0..count {
            let entry = rdp(lookup + PTR * i);
            // IMAGE_ORDINAL_FLAG: the top bit.
            let (name, ordinal) = if entry >> (8 * PTR - 1) != 0 {
                (&[][..], (entry & 0xffff) as u32)
            } else {
                (c_str(base + (entry & 0x7fff_ffff) + 2), 0)
            };
            match export(dll, name, ordinal) {
                // SAFETY: the IAT slot, made writable above.
                Some(target) => unsafe { ((iat + PTR * i) as *mut usize).write_unaligned(target) },
                None => {
                    // As on Windows the image will not load, but every
                    // missing import is reported first (not just the first).
                    report_missing_export(c_str(base + rd32(desc + 12) as usize), name, ordinal);
                    missing = true;
                }
            }
        }
        let (mut at, mut size, mut ignored) = (iat, count * PTR, 0u32);
        NtProtectVirtualMemory(-1, &mut at, &mut size, old, &mut ignored);
        desc += 20;
    }
    if missing { Err(STATUS_ENTRYPOINT_NOT_FOUND) } else { Ok(()) }
}

/// Reports `dll!name` (or `dll!#ordinal`) to the kernel's missing-features
/// ledger (gala's own service; see `GalaRecordMissing`).
fn report_missing_export(dll: &[u8], name: &[u8], ordinal: u32) {
    let mut text = [0u8; 64];
    let mut n = 0;
    let mut push = |bytes: &[u8]| {
        for &b in bytes {
            if n < text.len() {
                text[n] = b;
                n += 1;
            }
        }
    };
    push(dll);
    push(b"!");
    if name.is_empty() {
        push(b"#");
        let mut digits = [0u8; 10];
        let (mut v, mut d) = (ordinal, digits.len());
        loop {
            d -= 1;
            digits[d] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        push(&digits[d..]);
    } else {
        push(name);
    }
    GalaRecordMissing(text.as_ptr(), n, 1, u64::from(ordinal));
}

/// The normalized name in a UNICODE_STRING.
fn unicode_name(name: *const UnicodeString, out: &mut [u8; MAX_NAME]) -> Result<usize, u32> {
    if name.is_null() {
        return Err(STATUS_DLL_NOT_FOUND);
    }
    // SAFETY: the caller passes a valid UNICODE_STRING.
    let chars = unsafe {
        let name = &*name;
        core::slice::from_raw_parts(name.buffer, usize::from(name.length) / 2)
    };
    normalize(chars.iter().copied(), out)
}

/// LdrLoadDll(DllPath, DllCharacteristics, DllName, DllHandle): loads a DLL
/// (a path's from its directory; a name from the executable's, then
/// System32), its dependencies first from the DLL's own directory with
/// LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR.
///
/// # Safety
/// `name` must be a valid UNICODE_STRING and `handle` writable.
unsafe extern "system" fn ldr_load_dll(
    path: *const u16,
    _characteristics: *const u32,
    name: *const UnicodeString,
    handle: *mut usize,
) -> u32 {
    // As on newer Windows, LoadLibraryExW's LOAD_LIBRARY_SEARCH_* flags come
    // in `path` with its low bit set (a real path is 2-byte aligned).
    let flags = if path as usize & 1 != 0 { path as usize & !1 } else { 0 };
    // A name with an absolute directory loads from that directory only.
    let dir = explicit_dir(name);
    let search = match dir {
        Some(d) => Search::Dir(d),
        None => Search::Default,
    };
    // SAFETY: under the loader lock (LdrLoadDll holds it); the outer
    // load's directory and flags are kept for a DllMain that loads
    // another DLL.
    let (saved, saved_flags) = unsafe { (*LOAD_DIR.0.get(), *LOAD_FLAGS.0.get()) };
    // SAFETY: as above.
    unsafe { *LOAD_FLAGS.0.get() = flags };
    if let Some(d) = dir.filter(|d| flags & SEARCH_DLL_LOAD_DIR != 0 && d.len() <= MAX_PATH) {
        // SAFETY: as above.
        unsafe {
            let slot = &mut *LOAD_DIR.0.get();
            slot.0[..d.len()].copy_from_slice(d);
            slot.1 = d.len();
        }
    }
    let mut buf = [0u8; MAX_NAME];
    let loaded = unicode_name(name, &mut buf).and_then(|n| load_module(&buf[..n], search));
    // SAFETY: as above.
    unsafe {
        *LOAD_DIR.0.get() = saved;
        *LOAD_FLAGS.0.get() = saved_flags;
    }
    match loaded {
        Ok(slot) => {
            // SAFETY: the caller passes a writable handle.
            unsafe { handle.write(with_modules(|m| m[slot].base)) };
            STATUS_SUCCESS
        }
        Err(status) => status,
    }
}

/// The directory of an absolute DLL path (`C:\dir\x.dll` or
/// `\\server\...`), up to and including its last backslash; None for a
/// bare or relative name.
fn explicit_dir(name: *const UnicodeString) -> Option<&'static [u16]> {
    if name.is_null() {
        return None;
    }
    // SAFETY: the caller's UNICODE_STRING.
    let units = unsafe {
        let n = &*name;
        if n.buffer.is_null() {
            return None;
        }
        core::slice::from_raw_parts(n.buffer, usize::from(n.length) / 2)
    };
    let slash = |c: u16| c == u16::from(b'\\') || c == u16::from(b'/');
    let absolute = (units.len() > 2 && units[1] == u16::from(b':') && slash(units[2]))
        || (units.len() > 1 && slash(units[0]) && slash(units[1]));
    let end = units.iter().rposition(|&c| slash(c))?;
    absolute.then(|| &units[..=end])
}

/// LdrGetDllHandle(DllPath, DllCharacteristics, DllName, DllHandle): the
/// base of a loaded module.
///
/// # Safety
/// As for `LdrLoadDll`.
unsafe extern "system" fn ldr_get_dll_handle(
    _path: *const u16,
    _characteristics: *const u32,
    name: *const UnicodeString,
    handle: *mut usize,
) -> u32 {
    let mut buf = [0u8; MAX_NAME];
    let found = unicode_name(name, &mut buf)
        .and_then(|n| find_module(&buf[..n]).ok_or(STATUS_DLL_NOT_FOUND));
    match found {
        Ok(slot) => {
            // SAFETY: the caller passes a writable handle.
            unsafe { handle.write(with_modules(|m| m[slot].base)) };
            STATUS_SUCCESS
        }
        Err(status) => status,
    }
}

#[repr(C)]
pub struct AnsiString {
    length: u16,
    maximum_length: u16,
    buffer: *const u8,
}

/// LdrGetProcedureAddress(DllHandle, ProcedureName, ProcedureNumber,
/// ProcedureAddress): an export by name, or by ordinal if the name is null.
///
/// # Safety
/// `module` must be a loaded image, `name` null or a valid ANSI_STRING,
/// `address` writable.
unsafe extern "system" fn ldr_get_procedure_address(
    module: usize,
    name: *const AnsiString,
    ordinal: u32,
    address: *mut usize,
) -> u32 {
    // A null module means the executable (as on Windows, where
    // GetProcAddress(NULL, ...) searches it).
    let module = if module == 0 { image_base() } else { module };
    let found = if name.is_null() {
        export(module, &[], ordinal).ok_or(STATUS_ORDINAL_NOT_FOUND)
    } else {
        // SAFETY: the caller passes a valid ANSI_STRING.
        let name = unsafe {
            let name = &*name;
            core::slice::from_raw_parts(name.buffer, usize::from(name.length))
        };
        match name {
            [] => Err(STATUS_PROCEDURE_NOT_FOUND),
            // A missing name is STATUS_ENTRYPOINT_NOT_FOUND (Windows 11
            // 25H2); both are ERROR_PROC_NOT_FOUND to GetProcAddress.
            _ => export(module, name, 0).ok_or(STATUS_ENTRYPOINT_NOT_FOUND),
        }
    };
    let found = found.and_then(|target| {
        let missing =
            if name.is_null() { STATUS_ORDINAL_NOT_FOUND } else { STATUS_ENTRYPOINT_NOT_FOUND };
        if is_unimplemented_stub(target) { Err(missing) } else { Ok(target) }
    });
    // Programs probe for optional functions this way: every miss goes to
    // the missing-features ledger (`dll!name`).
    if found.is_err() {
        let mut dll = [0u8; MAX_NAME];
        let len = with_modules(|m| {
            m.iter().find(|e| e.base != 0 && e.base == module).map_or(0, |e| {
                dll[..e.name_len].copy_from_slice(&e.name[..e.name_len]);
                e.name_len
            })
        });
        let wanted = if name.is_null() {
            &[][..]
        } else {
            // SAFETY: the caller passes a valid ANSI_STRING.
            unsafe { core::slice::from_raw_parts((*name).buffer, usize::from((*name).length)) }
        };
        report_missing_export(&dll[..len], wanted, ordinal);
    }
    match found {
        Ok(target) => {
            // SAFETY: the caller passes a writable address.
            unsafe { address.write(target) };
            STATUS_SUCCESS
        }
        Err(status) => status,
    }
}

/// LdrUnloadDll(DllHandle): drops a reference taken by LdrLoadDll.
extern "system" fn ldr_unload_dll(module: usize) -> u32 {
    match with_modules(|m| m.iter().position(|m| m.base != 0 && m.base == module)) {
        Some(slot) => {
            unload_module(slot);
            STATUS_SUCCESS
        }
        None => STATUS_DLL_NOT_FOUND,
    }
}

/// Records the executable and ntdll as pinned modules (in that order, the
/// order of PEB.Ldr's load order list on Windows).
fn register_startup_modules(ntdll: usize, image: usize) -> Result<(), u32> {
    ldr_init();
    let image_path = image_path();
    let mut name = [0u8; MAX_NAME];
    // An executable whose name is not a plain ASCII one is just not listed.
    if let Ok(n) = normalize(image_path.iter().copied(), &mut name) {
        let slot = add_module(&name[..n], image, PINNED)?;
        set_module_path(slot, image_path);
    }
    let ntdll_slot = add_module(b"ntdll.dll", ntdll, PINNED)?;
    // ntdll's file: <NtSystemRoot>\System32\ntdll.dll (SysWOW64 for the
    // 32-bit one).
    let mut path = [0u16; MAX_PATH];
    let mut n = 0;
    // SAFETY: KUSER_SHARED_DATA.NtSystemRoot (+0x30), NUL-terminated.
    unsafe {
        while n < 200 && *((0x7ffe_0030 + 2 * n) as *const u16) != 0 {
            path[n] = *((0x7ffe_0030 + 2 * n) as *const u16);
            n += 1;
        }
    }
    for &b in SYSTEM_DIR.iter().chain(b"ntdll.dll") {
        path[n] = u16::from(b);
        n += 1;
    }
    set_module_path(ntdll_slot, &path[..n]);
    // ntdll is initialized first; the executable is never in that list.
    ldr_initialized(ntdll_slot);
    ldr().0.initialized = 1;
    Ok(())
}

/// NTSTATUS -> Win32 error code, for the statuses gala returns (written from
/// the documented correspondences: Microsoft's NTSTATUS/Win32 error
/// reference, Wine's mapping table as a cross-check).
const STATUS_TO_ERROR: &[(u32, u32)] = &[
    (0x0000_0000, 0),    // SUCCESS
    (0x0000_0080, 735),  // ABANDONED_WAIT_0 -> ERROR_ABANDONED_WAIT_0
    (0x0000_0102, 1460), // TIMEOUT -> ERROR_TIMEOUT
    (0x0000_0103, 997),  // PENDING -> ERROR_IO_PENDING
    (0x4000_0000, 183),  // OBJECT_NAME_EXISTS -> ERROR_ALREADY_EXISTS
    (0x8000_0005, 234),  // BUFFER_OVERFLOW -> ERROR_MORE_DATA
    (0x8000_0006, 18),   // NO_MORE_FILES
    (0xc000_0001, 31),   // UNSUCCESSFUL -> ERROR_GEN_FAILURE
    (0xc000_01ac, 1656), // NOT_SAME_OBJECT -> ERROR_NOT_SAME_OBJECT
    (0xc000_0002, 1),    // NOT_IMPLEMENTED -> ERROR_INVALID_FUNCTION
    (0xc000_0003, 87),   // INVALID_INFO_CLASS -> ERROR_INVALID_PARAMETER
    (0xc000_0004, 24),   // INFO_LENGTH_MISMATCH -> ERROR_BAD_LENGTH
    (0xc000_0005, 998),  // ACCESS_VIOLATION -> ERROR_NOACCESS
    (0xc000_0008, 6),    // INVALID_HANDLE
    (0xc000_000b, 87),   // INVALID_CID -> ERROR_INVALID_PARAMETER
    (0xc000_000d, 87),   // INVALID_PARAMETER
    (0xc000_0054, 33),   // FILE_LOCK_CONFLICT -> ERROR_LOCK_VIOLATION
    (0xc000_0055, 33),   // LOCK_NOT_GRANTED -> ERROR_LOCK_VIOLATION
    (0xc000_007e, 158),  // RANGE_NOT_LOCKED -> ERROR_NOT_LOCKED
    (0xc000_007f, 112),  // DISK_FULL -> ERROR_DISK_FULL
    (0xc000_0185, 1117), // IO_DEVICE_ERROR -> ERROR_IO_DEVICE
    (0xc000_0030, 87),   // INVALID_PARAMETER_MIX -> ERROR_INVALID_PARAMETER
    (0xc000_000f, 2),    // NO_SUCH_FILE -> ERROR_FILE_NOT_FOUND
    (0xc000_0010, 1),    // INVALID_DEVICE_REQUEST -> ERROR_INVALID_FUNCTION
    (0xc000_0011, 38),   // END_OF_FILE -> ERROR_HANDLE_EOF
    (0xc000_0017, 8),    // NO_MEMORY -> ERROR_NOT_ENOUGH_MEMORY
    (0xc000_012d, 1455), // COMMITMENT_LIMIT -> ERROR_COMMITMENT_LIMIT
    (0xc000_0018, 487),  // CONFLICTING_ADDRESSES -> ERROR_INVALID_ADDRESS
    (0xc000_001c, 1),    // INVALID_SYSTEM_SERVICE -> ERROR_INVALID_FUNCTION
    (0xc000_0022, 5),    // ACCESS_DENIED
    (0xc000_0023, 122),  // BUFFER_TOO_SMALL -> ERROR_INSUFFICIENT_BUFFER
    (0xc000_0024, 6),    // OBJECT_TYPE_MISMATCH -> ERROR_INVALID_HANDLE
    (0xc000_0033, 123),  // OBJECT_NAME_INVALID -> ERROR_INVALID_NAME
    (0xc000_0034, 2),    // OBJECT_NAME_NOT_FOUND -> ERROR_FILE_NOT_FOUND
    (0xc000_014b, 109),  // PIPE_BROKEN -> ERROR_BROKEN_PIPE
    (0xc000_00b1, 232),  // PIPE_CLOSING -> ERROR_NO_DATA
    (0xc000_00ab, 231),  // INSTANCE_NOT_AVAILABLE -> ERROR_PIPE_BUSY
    (0xc000_00ac, 231),  // PIPE_NOT_AVAILABLE -> ERROR_PIPE_BUSY
    (0xc000_00ad, 230),  // INVALID_PIPE_STATE -> ERROR_BAD_PIPE
    (0xc000_00ae, 231),  // PIPE_BUSY -> ERROR_PIPE_BUSY
    (0xc000_00af, 1),    // ILLEGAL_FUNCTION -> ERROR_INVALID_FUNCTION
    (0xc000_00b0, 233),  // PIPE_DISCONNECTED -> ERROR_PIPE_NOT_CONNECTED
    (0xc000_00b2, 535),  // PIPE_CONNECTED -> ERROR_PIPE_CONNECTED
    (0xc000_00b3, 536),  // PIPE_LISTENING -> ERROR_PIPE_LISTENING
    (0xc000_010d, 1368), // CANNOT_IMPERSONATE -> ERROR_CANNOT_IMPERSONATE
    (0x0000_0106, 1300), // NOT_ALL_ASSIGNED -> ERROR_NOT_ALL_ASSIGNED
    (0xc000_005c, 1309), // NO_IMPERSONATION_TOKEN -> ERROR_NO_IMPERSONATION_TOKEN
    (0xc000_0060, 1313), // NO_SUCH_PRIVILEGE -> ERROR_NO_SUCH_PRIVILEGE
    (0xc000_0061, 1314), // PRIVILEGE_NOT_HELD -> ERROR_PRIVILEGE_NOT_HELD
    (0xc000_0078, 1337), // INVALID_SID -> ERROR_INVALID_SID
    (0xc000_007c, 1008), // NO_TOKEN -> ERROR_NO_TOKEN
    (0xc000_00a5, 1346), // BAD_IMPERSONATION_LEVEL -> ERROR_BAD_IMPERSONATION_LEVEL
    (0xc000_00a8, 1349), // BAD_TOKEN_TYPE -> ERROR_BAD_TOKEN_TYPE
    (0xc000_0058, 1305), // UNKNOWN_REVISION -> ERROR_UNKNOWN_REVISION
    (0xc000_005a, 1307), // INVALID_OWNER -> ERROR_INVALID_OWNER
    (0xc000_005b, 1308), // INVALID_PRIMARY_GROUP -> ERROR_INVALID_PRIMARY_GROUP
    (0xc000_0077, 1336), // INVALID_ACL -> ERROR_INVALID_ACL
    (0xc000_0079, 1338), // INVALID_SECURITY_DESCR -> ERROR_INVALID_SECURITY_DESCR
    (0xc000_006d, 1326), // LOGON_FAILURE -> ERROR_LOGON_FAILURE
    (0xc000_015b, 1385), // LOGON_TYPE_NOT_GRANTED -> ERROR_LOGON_TYPE_NOT_GRANTED
    (0xc000_00fe, 1364), // NO_SUCH_PACKAGE -> ERROR_NO_SUCH_PACKAGE
    (0xc000_0035, 183),  // OBJECT_NAME_COLLISION -> ERROR_ALREADY_EXISTS
    (0xc000_0039, 161),  // OBJECT_PATH_INVALID -> ERROR_BAD_PATHNAME
    (0xc000_003a, 3),    // OBJECT_PATH_NOT_FOUND -> ERROR_PATH_NOT_FOUND
    (0xc000_003b, 161),  // OBJECT_PATH_SYNTAX_BAD -> ERROR_BAD_PATHNAME
    (0x8000_0011, 170),  // DEVICE_BUSY -> ERROR_BUSY
    (0xc000_0235, 6),    // HANDLE_NOT_CLOSABLE -> ERROR_INVALID_HANDLE
    (0xc000_0041, 5),    // PORT_CONNECTION_REFUSED -> ERROR_ACCESS_DENIED
    (0xc000_0043, 32),   // SHARING_VIOLATION
    (0xc000_0045, 87),   // INVALID_PAGE_PROTECTION -> ERROR_INVALID_PARAMETER
    (0xc000_0046, 288),  // MUTANT_NOT_OWNED -> ERROR_NOT_OWNER
    (0xc000_0047, 298),  // SEMAPHORE_LIMIT_EXCEEDED -> ERROR_TOO_MANY_POSTS
    (0xc000_004b, 5),    // THREAD_IS_TERMINATING -> ERROR_ACCESS_DENIED
    (0xc000_007a, 127),  // PROCEDURE_NOT_FOUND -> ERROR_PROC_NOT_FOUND
    (0xc000_007b, 193),  // INVALID_IMAGE_FORMAT -> ERROR_BAD_EXE_FORMAT
    (0xc000_00a0, 487),  // MEMORY_NOT_ALLOCATED -> ERROR_INVALID_ADDRESS
    (0xc000_00ba, 5),    // FILE_IS_A_DIRECTORY -> ERROR_ACCESS_DENIED
    (0xc000_00bb, 50),   // NOT_SUPPORTED
    (0xc000_0056, 5),    // DELETE_PENDING -> ERROR_ACCESS_DENIED
    (0xc000_0101, 145),  // DIRECTORY_NOT_EMPTY -> ERROR_DIR_NOT_EMPTY
    (0xc000_0103, 267),  // NOT_A_DIRECTORY -> ERROR_DIRECTORY
    (0xc000_0121, 5),    // CANNOT_DELETE -> ERROR_ACCESS_DENIED
    (0x8000_001a, 259),  // NO_MORE_ENTRIES -> ERROR_NO_MORE_ITEMS
    (0xc000_017c, 1018), // KEY_DELETED -> ERROR_KEY_DELETED
    (0xc000_0044, 1816), // QUOTA_EXCEEDED -> ERROR_NOT_ENOUGH_QUOTA
    (0xc000_009a, 1450), // INSUFFICIENT_RESOURCES -> ERROR_NO_SYSTEM_RESOURCES
    (0xc000_0181, 1021), // CHILD_MUST_BE_VOLATILE -> ERROR_CHILD_MUST_BE_VOLATILE
    (0xc000_0120, 995),  // CANCELLED -> ERROR_OPERATION_ABORTED
    (0xc000_0135, 126),  // DLL_NOT_FOUND -> ERROR_MOD_NOT_FOUND
    (0xc000_012f, 193),  // INVALID_IMAGE_NOT_MZ -> ERROR_BAD_EXE_FORMAT
    (0xc000_0138, 182),  // ORDINAL_NOT_FOUND -> ERROR_INVALID_ORDINAL
    (0xc000_0139, 127),  // ENTRYPOINT_NOT_FOUND -> ERROR_PROC_NOT_FOUND
    (0xc000_0225, 1168), // NOT_FOUND -> ERROR_NOT_FOUND
];

/// NtFlushBuffersFileEx(FileHandle, Flags, Parameters, ParametersSize,
/// *IoStatusBlock): NtFlushBuffersFile (gala's volume is in memory, so the
/// FLUSH_FLAGS_FILE_DATA_ONLY / NO_SYNC / FILE_DATA_SYNC_ONLY variants are
/// all the same). Flags beyond those are not refused: Windows 11 25H2
/// flushed with 0x100 on NTFS (kernel32_file_test); which, if any, it
/// refuses is not measured, so they are recorded in the ledger and
/// ignored. Parameters are STATUS_INVALID_PARAMETER (none is documented).
#[unsafe(no_mangle)]
pub extern "system" fn NtFlushBuffersFileEx(
    h: usize,
    flags: u32,
    parameters: *const u8,
    size: u32,
    iosb: *mut [usize; 2],
) -> u32 {
    if !parameters.is_null() || size != 0 {
        return 0xc000_000d;
    }
    if flags & !7 != 0 {
        let what = b"ntdll.dll!NtFlushBuffersFileEx(unknown flags)";
        GalaRecordMissing(what.as_ptr(), what.len(), 2, flags.into());
    }
    NtFlushBuffersFile(h, iosb)
}

/// RtlGetLastNtStatus(): TEB.LastStatusValue.
#[unsafe(no_mangle)]
pub extern "system" fn RtlGetLastNtStatus() -> u32 {
    last_status()
}

/// RtlNtStatusToDosError(Status): the Win32 error for an NTSTATUS
/// (ERROR_MR_MID_NOT_FOUND, 317, for statuses without one). As Windows'
/// does, it keeps `status` in TEB.LastStatusValue (RtlGetLastNtStatus).
#[unsafe(no_mangle)]
pub extern "system" fn RtlNtStatusToDosError(status: u32) -> u32 {
    set_last_status(status);
    if (0xc000_00ef..=0xc000_00fa).contains(&status) {
        return 87; // STATUS_INVALID_PARAMETER_1..12
    }
    match STATUS_TO_ERROR.iter().find(|&&(s, _)| s == status) {
        Some(&(_, error)) => error,
        None => {
            GalaRecordMissing(
                b"ntdll.dll!RtlNtStatusToDosError".as_ptr(),
                31,
                2,
                u64::from(status),
            );
            317
        }
    }
}

/// RtlGetCurrentPeb(): this process's PEB.
#[unsafe(no_mangle)]
pub extern "system" fn RtlGetCurrentPeb() -> usize {
    peb()
}

/// RtlGetVersion(*OSVERSIONINFO[EX]W): the version KUSER_SHARED_DATA
/// reports (NtMajorVersion +0x26c, NtMinorVersion +0x270, NtBuildNumber
/// +0x260), platform VER_PLATFORM_WIN32_NT, no service pack; the EX fields
/// (when dwOSVersionInfoSize says so) as a workstation.
///
/// # Safety
/// `info` must point to an OSVERSIONINFOW (276 bytes) or OSVERSIONINFOEXW
/// (284 bytes) whose first field is its size.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn RtlGetVersion(info: *mut u32) -> u32 {
    // SAFETY: forwarded to the caller; KUSER_SHARED_DATA is always mapped.
    unsafe {
        let size = *info;
        if size != 276 && size != 284 {
            return 0xc000_000d; // STATUS_INVALID_PARAMETER
        }
        let kuser = |offset: usize| core::ptr::read_volatile((0x7ffe_0000 + offset) as *const u32);
        *info.add(1) = kuser(0x26c);
        *info.add(2) = kuser(0x270);
        *info.add(3) = kuser(0x260) & 0xffff;
        *info.add(4) = 2; // VER_PLATFORM_WIN32_NT
        core::ptr::write_bytes(info.add(5).cast::<u8>(), 0, 256); // szCSDVersion
        if size == 284 {
            // wServicePackMajor/Minor, wSuiteMask, wProductType
            // (VER_NT_WORKSTATION), wReserved.
            let ex = info.cast::<u8>().add(276);
            core::ptr::write_bytes(ex, 0, 8);
            *ex.add(6) = 1;
        }
    }
    STATUS_SUCCESS
}

/// The update build revision the registry seed gives
/// (HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\UBR).
const UBR: u64 = 1;

/// RtlGetDeviceFamilyInfoEnum(*uap_info, *family, *form): the version as
/// major.minor.build.revision in 16-bit fields (major highest), the
/// device family DEVICEFAMILYINFOENUM_DESKTOP (3) and the form
/// DEVICEFAMILYDEVICEFORM_UNKNOWN (0); each output optional.
///
/// # Safety
/// Each non-null output must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn RtlGetDeviceFamilyInfoEnum(
    uap_info: *mut u64,
    family: *mut u32,
    form: *mut u32,
) {
    // SAFETY: KUSER_SHARED_DATA (as RtlGetVersion reads it); the caller's
    // outputs, each written only when given.
    unsafe {
        let kuser = |offset: usize| {
            u64::from(core::ptr::read_volatile((0x7ffe_0000 + offset) as *const u32))
        };
        if !uap_info.is_null() {
            *uap_info =
                kuser(0x26c) << 48 | kuser(0x270) << 32 | (kuser(0x260) & 0xffff) << 16 | UBR;
        }
        if !family.is_null() {
            *family = 3;
        }
        if !form.is_null() {
            *form = 0;
        }
    }
}

/// RtlGetNtVersionNumbers(*Major, *Minor, *Build): the pinned build's
/// version (KUSER_SHARED_DATA has the same numbers), with 0xF0000000 in
/// the build number as a free (release) build reports it.
#[unsafe(no_mangle)]
pub extern "system" fn RtlGetNtVersionNumbers(major: *mut u32, minor: *mut u32, build: *mut u32) {
    // SAFETY: KUSER_SHARED_DATA (0x7ffe0000): NtMajorVersion at +0x26c,
    // NtMinorVersion at +0x270, NtBuildNumber at +0x260; the caller's
    // pointers (each may be null).
    unsafe {
        let kuser = 0x7ffe_0000usize;
        let values = [
            (major, rd32(kuser + 0x26c)),
            (minor, rd32(kuser + 0x270)),
            (build, rd32(kuser + 0x260) | 0xf000_0000),
        ];
        for (p, v) in values {
            if !p.is_null() {
                *p = v;
            }
        }
    }
}

/// The initial thread's CONTEXT while the process starts (DllMain's
/// lpvReserved for the DLLs loaded with the program).
static INITIAL_CONTEXT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// The initial thread's first user-mode instruction.
///
/// # Safety
/// Called by the kernel with a valid CONTEXT.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn LdrInitializeThunk(context: *const u8, ntdll_base: usize) -> ! {
    // SAFETY: the kernel passes a readable CONTEXT; the state is only
    // written here, before any other thread exists.
    unsafe {
        let pc = (context.add(CONTEXT_PC) as *const usize).read_unaligned();
        let state = &raw mut GalaTestNtdllState;
        let calls = (*state)[3];
        state.write_volatile([MAGIC, ntdll_base as u64, pc as u64, calls + 1]);
    }
    let image = image_base();
    // PEB.LoaderLock: where programs find the loader lock's owner.
    // SAFETY: the PEB, writable by the process.
    unsafe { ((peb() + PEB_LOADER_LOCK) as *mut usize).write(&raw const LOADER_LOCK as usize) };
    // The process heap, before any DLL's code runs.
    heap::create_process_heap();
    INITIAL_CONTEXT.store(context as usize, core::sync::atomic::Ordering::Relaxed);
    let started = with_loader_lock(|| {
        register_startup_modules(ntdll_base, image)?;
        // The executable's TLS (and this thread's blocks) before its
        // imports load; its TLS callbacks after their DllMains.
        tls::process_init(image);
        STARTUP.store(true, core::sync::atomic::Ordering::Relaxed);
        let bound = bind_imports(image, None);
        STARTUP.store(false, core::sync::atomic::Ordering::Relaxed);
        bound?;
        tls::callbacks(image, tls::DLL_PROCESS_ATTACH);
        Ok(())
    });
    if let Err(status) = started {
        NtTerminateProcess(-1, status);
    }
    // TestAlert, as Windows' loader: user APCs queued during
    // initialization (DllMain, TLS callbacks) run before the entry point.
    NtContinue(context, 1);
    // NtContinue only returns on failure.
    loop {
        core::hint::spin_loop();
    }
}

/// VerSetConditionMask: three condition bits for each VER_* type bit,
/// at three times the bit's index, and bit 63 set once a condition is
/// (Windows 11 25H2: (0, VER_MAJORVERSION, VER_GREATER_EQUAL) is
/// 0x8000000000000018; with no type bit the mask is returned as it is,
/// which is not measured). kernel32 forwards here, as on Windows.
#[unsafe(no_mangle)]
pub extern "system" fn VerSetConditionMask(mask: u64, kind: u32, condition: u8) -> u64 {
    let mut mask = mask;
    for bit in 0..8 {
        if kind & (1 << bit) != 0 {
            let shift = 3 * bit;
            mask = (mask & !(7 << shift)) | (u64::from(condition & 7) << shift) | 1 << 63;
        }
    }
    mask
}

/// RtlVerifyVersionInfo(*VersionInfo, TypeMask, ConditionMask): compares
/// the requested OSVERSIONINFOEXW fields with the version RtlGetVersion
/// gives, the real one whatever the program's manifest (kernel32's
/// VerifyVersionInfoW compares with the version its manifest gets
/// instead; curl asks this one). Major, minor and service pack are
/// compared as one version when several are given, as documented; build
/// and platform each on their own. STATUS_REVISION_MISMATCH when they do
/// not match; STATUS_INVALID_PARAMETER for an empty TypeMask. The suite
/// and product type are not compared (recorded).
///
/// # Safety
/// `info` must point to an OSVERSIONINFOEXW (284 bytes).
#[unsafe(no_mangle)]
pub unsafe extern "system" fn RtlVerifyVersionInfo(info: *const u32, kind: u32, mask: u64) -> u32 {
    const STATUS_REVISION_MISMATCH: u32 = 0xc000_0059;
    if info.is_null() || kind == 0 || mask == 0 {
        return 0xc000_000d; // STATUS_INVALID_PARAMETER
    }
    if kind & (0x40 | 0x80) != 0 {
        let what = b"ntdll.dll!RtlVerifyVersionInfo(suite or product type)";
        GalaRecordMissing(what.as_ptr(), what.len(), 2, kind.into());
    }
    let mut system = [0u32; 71];
    system[0] = 284;
    // SAFETY: an OSVERSIONINFOEXW of the size it says; the caller's
    // OSVERSIONINFOEXW: major +4, minor +8, build +12, platform +16, service
    // pack major and minor at +276/+278 (WORDs).
    let (wanted, have) = unsafe {
        RtlGetVersion(system.as_mut_ptr());
        let fields = |v: *const u32| {
            let b = v.cast::<u8>();
            [
                *v.add(2),                                            // VER_MINORVERSION 0x1
                *v.add(1),                                            // VER_MAJORVERSION 0x2
                *v.add(3),                                            // VER_BUILDNUMBER 0x4
                *v.add(4),                                            // VER_PLATFORMID 0x8
                u32::from(b.add(278).cast::<u16>().read_unaligned()), // VER_SERVICEPACKMINOR 0x10
                u32::from(b.add(276).cast::<u16>().read_unaligned()), // VER_SERVICEPACKMAJOR 0x20
            ]
        };
        (fields(info), fields(system.as_ptr()))
    };
    let condition = |bit: u32| (mask >> (3 * bit.trailing_zeros())) & 7;
    let compare = |cond: u64, system: u64, wanted: u64| match cond {
        1 => system == wanted, // VER_EQUAL
        2 => system > wanted,  // VER_GREATER
        3 => system >= wanted, // VER_GREATER_EQUAL
        4 => system < wanted,  // VER_LESS
        5 => system <= wanted, // VER_LESS_EQUAL
        _ => false,
    };
    // Major/minor/service pack as one number, with the condition of the
    // most significant one given.
    let version_bits = kind & (0x1 | 0x2 | 0x10 | 0x20);
    if version_bits != 0 {
        let lead =
            [0x2, 0x1, 0x20, 0x10].into_iter().find(|&b| version_bits & b != 0).unwrap_or(0x2);
        let pack = |f: &[u32; 6]| {
            let get = |bit: u32| {
                if kind & bit != 0 { u64::from(f[bit.trailing_zeros() as usize]) } else { 0 }
            };
            (get(0x2) << 48) | (get(0x1) << 32) | (get(0x20) << 16) | get(0x10)
        };
        if !compare(condition(lead), pack(&have), pack(&wanted)) {
            return STATUS_REVISION_MISMATCH;
        }
    }
    for bit in [0x4u32, 0x8] {
        let i = bit.trailing_zeros() as usize;
        if kind & bit != 0 && !compare(condition(bit), have[i].into(), wanted[i].into()) {
            return STATUS_REVISION_MISMATCH;
        }
    }
    STATUS_SUCCESS
}

/// PVECTORED_EXCEPTION_HANDLER: LONG (PEXCEPTION_POINTERS).
type VectoredHandler = extern "system" fn(*mut ExceptionPointers) -> i32;

#[repr(C)]
pub struct ExceptionPointers {
    record: *mut u8,
    context: *mut u8,
}

const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
const MAX_HANDLERS: usize = 16;

/// Registered vectored handlers in call order, as (cookie, handler): the
/// exception handlers and the continue handlers, and the next cookie.
struct HandlerTables {
    exception: [(usize, usize); MAX_HANDLERS],
    continue_: [(usize, usize); MAX_HANDLERS],
    next: usize,
}

/// The tables behind a spinlock (threads register and dispatch at the same
/// time; the critical sections are short and never block).
struct Handlers {
    lock: core::sync::atomic::AtomicBool,
    tables: core::cell::UnsafeCell<HandlerTables>,
}

// SAFETY: `tables` is only accessed with `lock` held (see `with_handlers`).
unsafe impl Sync for Handlers {}

static HANDLERS: Handlers = Handlers {
    lock: core::sync::atomic::AtomicBool::new(false),
    tables: core::cell::UnsafeCell::new(HandlerTables {
        exception: [(0, 0); MAX_HANDLERS],
        continue_: [(0, 0); MAX_HANDLERS],
        next: 1,
    }),
};

fn with_handlers<R>(f: impl FnOnce(&mut HandlerTables) -> R) -> R {
    use core::sync::atomic::Ordering;
    while HANDLERS
        .lock
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    // SAFETY: the lock is held, so this is the only reference.
    let result = f(unsafe { &mut *HANDLERS.tables.get() });
    HANDLERS.lock.store(false, Ordering::Release);
    result
}

fn add_handler(continue_: bool, first: u32, handler: VectoredHandler) -> *mut core::ffi::c_void {
    with_handlers(|t| {
        let cookie = t.next;
        let table = if continue_ { &mut t.continue_ } else { &mut t.exception };
        let used = table.iter().take_while(|e| e.0 != 0).count();
        if used == MAX_HANDLERS {
            return core::ptr::null_mut();
        }
        let entry = (cookie, handler as usize);
        if first != 0 {
            table.copy_within(0..used, 1);
            table[0] = entry;
        } else {
            table[used] = entry;
        }
        t.next += 1;
        cookie as *mut core::ffi::c_void
    })
}

fn remove_handler(continue_: bool, cookie: *mut core::ffi::c_void) -> u32 {
    with_handlers(|t| {
        let table = if continue_ { &mut t.continue_ } else { &mut t.exception };
        let Some(i) = table.iter().position(|e| e.0 != 0 && e.0 == cookie as usize) else {
            return 0;
        };
        table.copy_within(i + 1.., i);
        table[MAX_HANDLERS - 1] = (0, 0);
        1
    })
}

/// RtlAddVectoredExceptionHandler(First, Handler): the cookie, or null if
/// the table is full.
#[unsafe(no_mangle)]
pub extern "system" fn RtlAddVectoredExceptionHandler(
    first: u32,
    handler: VectoredHandler,
) -> *mut core::ffi::c_void {
    add_handler(false, first, handler)
}

/// RtlRemoveVectoredExceptionHandler(Handle): nonzero if it was registered.
#[unsafe(no_mangle)]
pub extern "system" fn RtlRemoveVectoredExceptionHandler(cookie: *mut core::ffi::c_void) -> u32 {
    remove_handler(false, cookie)
}

/// RtlAddVectoredContinueHandler(First, Handler).
#[unsafe(no_mangle)]
pub extern "system" fn RtlAddVectoredContinueHandler(
    first: u32,
    handler: VectoredHandler,
) -> *mut core::ffi::c_void {
    add_handler(true, first, handler)
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlRemoveVectoredContinueHandler(cookie: *mut core::ffi::c_void) -> u32 {
    remove_handler(true, cookie)
}

/// Calls the handlers of one table in order; whether one of them returned
/// EXCEPTION_CONTINUE_EXECUTION.
fn call_handlers(table: &[(usize, usize)], pointers: &mut ExceptionPointers) -> bool {
    for &(cookie, raw) in table {
        if cookie == 0 {
            break;
        }
        // SAFETY: handlers were registered as `VectoredHandler`s.
        let handler: VectoredHandler = unsafe { core::mem::transmute(raw) };
        if handler(pointers) == EXCEPTION_CONTINUE_EXECUTION {
            return true;
        }
    }
    false
}

/// strlen (exported by ntdll, as on Windows; gala's DLLs import it).
///
/// # Safety
/// `s` must be a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn strlen(s: *const u8) -> usize {
    let mut n = 0;
    // SAFETY: forwarded to the caller. Volatile so that this loop is not
    // itself turned into a call to strlen.
    unsafe {
        while core::ptr::read_volatile(s.add(n)) != 0 {
            n += 1;
        }
    }
    n
}

/// memcmp(a, b, n)
///
/// # Safety
/// Both ranges must be readable for `n` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    for i in 0..n {
        // SAFETY: within the caller-guaranteed ranges.
        let (x, y) = unsafe { (*a.add(i), *b.add(i)) };
        if x != y {
            return i32::from(x) - i32::from(y);
        }
    }
    0
}

/// MSVC marker symbol that object files using floating point reference.
#[unsafe(no_mangle)]
#[used]
pub static _fltused: i32 = 0;

/// Unwind personality referenced by the prebuilt `core` library's unwind
/// tables. With `panic = abort` nothing unwinds, so it is never called.
#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    NtTerminateProcess(-1, 0xc000_0409);
    loop {
        core::hint::spin_loop();
    }
}

/// Entry point required by our link flags; a DllMain that does nothing.
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

// The loader's exports, each under the loader lock.

/// LdrLoadDll(DllPath, DllCharacteristics, DllName, *DllHandle).
///
/// # Safety
/// As for the functions it wraps: valid UNICODE_STRING and output.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn LdrLoadDll(
    path: *const u16,
    characteristics: *const u32,
    name: *const UnicodeString,
    handle: *mut usize,
) -> u32 {
    // SAFETY: the caller's arguments, passed on.
    with_loader_lock(|| unsafe { ldr_load_dll(path, characteristics, name, handle) })
}

/// LdrGetDllHandle(DllPath, DllCharacteristics, DllName, *DllHandle).
///
/// # Safety
/// As for `LdrLoadDll`.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn LdrGetDllHandle(
    path: *const u16,
    characteristics: *const u32,
    name: *const UnicodeString,
    handle: *mut usize,
) -> u32 {
    // SAFETY: as above.
    with_loader_lock(|| unsafe { ldr_get_dll_handle(path, characteristics, name, handle) })
}

/// LdrGetProcedureAddress(DllHandle, ProcedureName, ProcedureNumber,
/// *ProcedureAddress).
///
/// # Safety
/// `module` must be a loaded image, `name` null or a valid ANSI_STRING,
/// `address` writable.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn LdrGetProcedureAddress(
    module: usize,
    name: *const AnsiString,
    ordinal: u32,
    address: *mut usize,
) -> u32 {
    // SAFETY: as above.
    with_loader_lock(|| unsafe { ldr_get_procedure_address(module, name, ordinal, address) })
}

/// LdrUnloadDll(DllHandle).
#[unsafe(no_mangle)]
pub extern "system" fn LdrUnloadDll(module: usize) -> u32 {
    with_loader_lock(|| ldr_unload_dll(module))
}

/// LdrAddDllDirectory(*Directory, *Cookie) (kernel32's AddDllDirectory):
/// a directory searched with LOAD_LIBRARY_SEARCH_USER_DIRS or
/// _DEFAULT_DIRS, for the DLL and its dependencies. A full path (a drive
/// and a separator) only; STATUS_INVALID_PARAMETER otherwise,
/// STATUS_NO_MEMORY when all slots are taken.
///
/// # Safety
/// `dir` must point at a valid UNICODE_STRING and `cookie` be writable.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn LdrAddDllDirectory(
    dir: *const UnicodeString,
    cookie: *mut usize,
) -> u32 {
    const STATUS_INVALID_PARAMETER: u32 = 0xc000_000d;
    if dir.is_null() || cookie.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: the caller's UNICODE_STRING.
    let text = unsafe {
        let us = &*dir;
        if us.buffer.is_null() {
            return STATUS_INVALID_PARAMETER;
        }
        core::slice::from_raw_parts(us.buffer, usize::from(us.length) / 2)
    };
    let sep = |c: u16| c == u16::from(b'\\') || c == u16::from(b'/');
    if text.len() < 3 || text[1] != u16::from(b':') || !sep(text[2]) || text.len() + 1 > MAX_PATH {
        return STATUS_INVALID_PARAMETER;
    }
    with_loader_lock(|| {
        // SAFETY: under the loader lock.
        let list = unsafe { &mut *USER_DIR_LIST.0.get() };
        let Some(slot) = list.iter().position(|(_, len)| *len == 0) else {
            return STATUS_NO_MEMORY;
        };
        let (buf, len) = &mut list[slot];
        buf[..text.len()].copy_from_slice(text);
        *len = text.len();
        if !sep(buf[*len - 1]) {
            buf[*len] = u16::from(b'\\');
            *len += 1;
        }
        // SAFETY: the caller's cookie.
        unsafe { cookie.write(slot + 1) };
        STATUS_SUCCESS
    })
}

/// LdrRemoveDllDirectory(Cookie) (RemoveDllDirectory): takes the directory
/// out of the search; STATUS_INVALID_PARAMETER for an unknown cookie.
#[unsafe(no_mangle)]
pub extern "system" fn LdrRemoveDllDirectory(cookie: usize) -> u32 {
    with_loader_lock(|| {
        // SAFETY: under the loader lock.
        let list = unsafe { &mut *USER_DIR_LIST.0.get() };
        match cookie.checked_sub(1).and_then(|i| list.get_mut(i)) {
            Some((_, len)) if *len != 0 => {
                *len = 0;
                STATUS_SUCCESS
            }
            _ => 0xc000_000d,
        }
    })
}

/// LdrSetDllDirectory(*Directory): the directory searched for DLLs after
/// the executable's (kernel32's SetDllDirectoryW); null or empty: none.
///
/// # Safety
/// `dir` must be null or point at a valid UNICODE_STRING.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn LdrSetDllDirectory(dir: *const UnicodeString) -> u32 {
    let text: &[u16] = if dir.is_null() {
        &[]
    } else {
        // SAFETY: the caller's UNICODE_STRING.
        unsafe {
            let us = &*dir;
            if us.buffer.is_null() {
                &[]
            } else {
                core::slice::from_raw_parts(us.buffer, usize::from(us.length) / 2)
            }
        }
    };
    // Room for a backslash after it.
    if text.len() + 1 > MAX_PATH {
        return 0xc000_0106; // STATUS_NAME_TOO_LONG
    }
    with_loader_lock(|| {
        // SAFETY: under the loader lock.
        let ((buf, len), given) =
            unsafe { (&mut *DLL_DIRECTORY.0.get(), &mut *GIVEN_LENGTH.0.get()) };
        buf[..text.len()].copy_from_slice(text);
        *given = text.len();
        *len = text.len();
        // The search joins it with names: end it with a backslash.
        if *len != 0 && buf[*len - 1] != u16::from(b'\\') {
            buf[*len] = u16::from(b'\\');
            *len += 1;
        }
    });
    STATUS_SUCCESS
}

/// The length of the directory as LdrSetDllDirectory was given it.
struct GivenLength(core::cell::UnsafeCell<usize>);

// SAFETY: used under the loader lock.
unsafe impl Sync for GivenLength {}

static GIVEN_LENGTH: GivenLength = GivenLength(core::cell::UnsafeCell::new(0));

/// LdrGetDllDirectory(*Directory): the directory LdrSetDllDirectory set
/// (empty if none) into the caller's buffer, NUL-terminated if room;
/// STATUS_BUFFER_TOO_SMALL (Length: what is needed) if it does not fit.
///
/// # Safety
/// `dir` must point at a UNICODE_STRING with a writable buffer of
/// MaximumLength bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn LdrGetDllDirectory(dir: *mut UnicodeString) -> u32 {
    with_loader_lock(|| {
        // SAFETY: under the loader lock.
        let (buf, n) = unsafe { (&(*DLL_DIRECTORY.0.get()).0, *GIVEN_LENGTH.0.get()) };
        // SAFETY: the caller's UNICODE_STRING and buffer.
        unsafe {
            let us = &mut *dir;
            us.length = (2 * n) as u16;
            if usize::from(us.maximum_length) < 2 * n + 2 {
                return 0xc000_0023; // STATUS_BUFFER_TOO_SMALL
            }
            core::ptr::copy_nonoverlapping(buf.as_ptr(), us.buffer.cast_mut(), n);
            *us.buffer.cast_mut().add(n) = 0;
        }
        STATUS_SUCCESS
    })
}

/// LdrGetDllFullName(DllHandle, *FullDllName): the file a module was
/// loaded from (the executable's for a null handle) into the caller's
/// UNICODE_STRING buffer; STATUS_BUFFER_TOO_SMALL (with the length
/// needed) if it does not fit, STATUS_DLL_NOT_FOUND for an unknown handle.
///
/// # Safety
/// `name` must point at a UNICODE_STRING with a writable buffer of
/// MaximumLength bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn LdrGetDllFullName(module: usize, name: *mut UnicodeString) -> u32 {
    let base = if module == 0 { image_base() } else { module };
    with_loader_lock(|| {
        let found = with_modules(|m| {
            m.iter().find(|m| m.base == base && base != 0).map(|m| (m.path, m.path_len))
        });
        let Some((path, n)) = found else { return STATUS_DLL_NOT_FOUND };
        // SAFETY: the caller's UNICODE_STRING and buffer.
        unsafe {
            let us = &mut *name;
            us.length = (2 * n) as u16;
            if usize::from(us.maximum_length) < 2 * n {
                return 0xc000_0023; // STATUS_BUFFER_TOO_SMALL
            }
            core::ptr::copy_nonoverlapping(path.as_ptr(), us.buffer.cast_mut(), n);
            if usize::from(us.maximum_length) >= 2 * n + 2 {
                *us.buffer.cast_mut().add(n) = 0;
            }
        }
        STATUS_SUCCESS
    })
}

/// LdrAddRefDll(Flags, DllHandle): another reference on a loaded module;
/// LDR_ADDREF_DLL_PIN (1) pins it for the process's lifetime.
#[unsafe(no_mangle)]
pub extern "system" fn LdrAddRefDll(flags: u32, module: usize) -> u32 {
    with_loader_lock(|| {
        with_modules(|m| match m.iter_mut().find(|m| m.base == module && module != 0) {
            Some(entry) => {
                if flags & 1 != 0 {
                    entry.refs = PINNED;
                } else if entry.refs != PINNED {
                    entry.refs += 1;
                }
                STATUS_SUCCESS
            }
            None => STATUS_DLL_NOT_FOUND,
        })
    })
}
