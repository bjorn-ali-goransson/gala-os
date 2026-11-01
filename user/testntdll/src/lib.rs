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

mod heap;
mod seh;
mod tls;
mod unimplemented;

/// "galantdl": marks the state as written by this DLL.
const MAGIC: u64 = 0x6761_6c61_6e74_646c;
/// CONTEXT.Rip (public x64 layout).
const CONTEXT_RIP: usize = 0xf8;
/// NtContinue on the pinned build (to be verified, like all numbers).
const NT_CONTINUE: u32 = 0x43;

/// [magic, NtdllBase argument, Context->Rip, number of thunk calls]
#[unsafe(no_mangle)]
pub static mut GalaTestNtdllState: [u64; 4] = [0; 4];

/// NtContinue(Context, TestAlert): resumes `Context`; does not return on
/// success.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "system" fn NtContinue(_context: *const u8, _test_alert: u8) -> i32 {
    core::arch::naked_asm!("mov r10, rcx", "mov eax, {n}", "syscall", "ret", n = const NT_CONTINUE)
}

/// System call stubs shaped like ntdll's: `mov r10, rcx; mov eax, n;
/// syscall; ret` (stack arguments stay where the kernel expects them).
macro_rules! syscall_stubs {
    ($($name:ident = $n:expr, fn($($arg:ident: $ty:ty),*);)*) => {$(
        #[unsafe(naked)]
        #[unsafe(no_mangle)]
        pub extern "system" fn $name($(_: $ty),*) -> u32 {
            core::arch::naked_asm!("mov r10, rcx", "mov eax, {n}", "syscall", "ret", n = const $n)
        }
    )*};
}

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
    let opt = base + rd32(base + 0x3c) as usize + 24;
    (rd32(opt + 112 + 8 * index) as usize, rd32(opt + 116 + 8 * index) as usize)
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
    /// Next to the executable, then System32.
    Default,
    /// System32 only (LOAD_LIBRARY_SEARCH_SYSTEM32).
    System32,
    /// This directory only (a path with a directory was given; ends with
    /// a backslash).
    Dir(&'a [u16]),
}

fn map_dll(dll: &[u8], search: Search) -> Result<usize, u32> {
    if let Search::Dir(dir) = search {
        return open_dll(dir, dll);
    }
    let system32_only = matches!(search, Search::System32);
    // SAFETY: gs:[0x60] is the PEB; ProcessParameters and ImagePathName are
    // filled in by the kernel.
    let image_path = unsafe {
        let peb: usize;
        core::arch::asm!("mov {}, gs:[0x60]", out(reg) peb, options(nostack, readonly));
        let params = rd64(peb + 0x20) as usize;
        let len = usize::from(*((params + 0x60) as *const u16)) / 2;
        core::slice::from_raw_parts(rd64(params + 0x68) as *const u16, len)
    };
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
    for &b in b"\\System32\\" {
        system32[n] = u16::from(b);
        n += 1;
    }
    if system32_only {
        return open_dll(&system32[..n], dll);
    }
    // The executable's directory, then SetDllDirectory's (as documented
    // for the standard search order), then System32.
    match open_dll(&image_path[..dir_len], dll) {
        Err(STATUS_DLL_NOT_FOUND) => {}
        other => return other,
    }
    // SAFETY: under the loader lock (as `map_dll` is).
    let (dir, len) = unsafe { *DLL_DIRECTORY.0.get() };
    if len != 0 {
        match open_dll(&dir[..len], dll) {
            Err(STATUS_DLL_NOT_FOUND) => {}
            other => return other,
        }
    }
    open_dll(&system32[..n], dll)
}

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
}

fn open_dll(dir: &[u16], dll: &[u8]) -> Result<usize, u32> {
    let mut path = [0u16; 300];
    let prefix = *b"\\??\\";
    let total = prefix.len() + dir.len() + dll.len();
    if total > path.len() {
        return Err(STATUS_DLL_NOT_FOUND);
    }
    for (i, &b) in prefix.iter().enumerate() {
        path[i] = u16::from(b);
    }
    path[prefix.len()..prefix.len() + dir.len()].copy_from_slice(dir);
    for (i, &b) in dll.iter().enumerate() {
        path[prefix.len() + dir.len() + i] = u16::from(b);
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
    if st != STATUS_SUCCESS {
        return Err(STATUS_INVALID_IMAGE_FORMAT);
    }
    let (mut base, mut size) = (0usize, 0usize);
    // ViewShare, PAGE_READWRITE (image views take their own protections).
    let st =
        NtMapViewOfSection(section, -1, &mut base, 0, 0, core::ptr::null_mut(), &mut size, 1, 0, 4);
    NtClose(section);
    match st {
        STATUS_SUCCESS | STATUS_IMAGE_NOT_AT_BASE => {}
        _ => return Err(STATUS_INVALID_IMAGE_FORMAT),
    }
    // Image sections take 32-bit images too; a 64-bit process loads one
    // as a module only if it is an IL-only .NET assembly (no code runs).
    if is_pe32(base) && !is_il_only(base) {
        NtUnmapViewOfSection(-1, base);
        return Err(STATUS_INVALID_IMAGE_FORMAT);
    }
    Ok(base)
}

/// Whether the image at `base` is 32-bit (PE32).
fn is_pe32(base: usize) -> bool {
    rd32(base + rd32(base + 0x3c) as usize + 24) & 0xffff == 0x10b
}

/// Whether the PE32 image at `base` is an IL-only .NET assembly: its CLI
/// header (data directory 14, at +96 + 14 * 8 in a PE32 optional header)
/// has COMIMAGE_FLAGS_ILONLY (bit 0 of Flags, at +16). Windows loads
/// these into 64-bit processes; nothing in them runs (no imports are
/// bound, no entry point is called).
fn is_il_only(base: usize) -> bool {
    let optional = base + rd32(base + 0x3c) as usize + 24;
    let cli = rd32(optional + 96 + 14 * 8) as usize;
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
/// The owner's thread id, and its depth (touched only by the owner).
static LOADER_OWNER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static LOADER_DEPTH: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Set while the executable's imports are bound at startup.
static STARTUP: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Runs `f` holding the loader lock.
pub(crate) fn with_loader_lock<R>(f: impl FnOnce() -> R) -> R {
    use core::sync::atomic::Ordering;
    let me: u64;
    // SAFETY: gs:[0x48] is TEB.ClientId.UniqueThread.
    unsafe { core::arch::asm!("mov {}, gs:[0x48]", out(reg) me, options(nostack, readonly)) };
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

// TODO: PEB->Ldr lists.
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

/// The slot of the module named `name` (already normalized).
fn find_module(name: &[u8]) -> Option<usize> {
    with_modules(|m| m.iter().position(|m| m.base != 0 && &m.name[..m.name_len] == name))
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

/// The DLL implementing API set `name` (`api-ms-win-...`, normalized), as
/// Windows' API set schema maps these contract names to the DLLs that
/// implement them (whatever files of those names exist). gala keeps the
/// mapping coarse: the C runtime sets are ucrtbase, COM and WinRT are
/// ole32 (gala has no combase), security, eventing, registry and services
/// are advapi32, the other core sets are kernel32 (gala has no
/// kernelbase); other names are looked up as files.
fn api_set_host(name: &[u8]) -> Option<&'static [u8]> {
    let set = name.strip_prefix(b"api-ms-win-")?;
    let host: &[u8] = if set.starts_with(b"crt-") {
        b"ucrtbase.dll"
    } else if set.starts_with(b"core-com-") || set.starts_with(b"core-winrt-") {
        b"ole32.dll"
    } else if ["security-", "eventing-", "core-registry-", "service-"]
        .iter()
        .any(|p| set.starts_with(p.as_bytes()))
    {
        b"advapi32.dll"
    } else if set.starts_with(b"core-") {
        b"kernel32.dll"
    } else {
        return None; // not a set gala hosts: looked up as a file
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
    if let Some(i) = find_module(name) {
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
        if is_pe32(base) {
            return Ok(()); // an IL-only assembly: mapped, nothing to run
        }
        bind_imports(base, Some(slot))?;
        // Its TLS, then its TLS callbacks and DllMain (DLL_PROCESS_ATTACH).
        if tls::register(base) {
            tls::callbacks(base, tls::DLL_PROCESS_ATTACH);
        }
        with_modules(|m| m[slot].attached = true);
        match entry_point(base) {
            Some(entry) if entry(base, 1, 0) == 0 => Err(STATUS_DLL_INIT_FAILED),
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
        (m[slot].refs == 0).then(|| core::mem::replace(&mut m[slot], FREE))
    });
    let Some(module) = module else { return };
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
        while rd64(lookup + 8 * count) != 0 {
            count += 1;
        }
        let (mut at, mut size, mut old) = (iat, count * 8, 0u32);
        if NtProtectVirtualMemory(-1, &mut at, &mut size, 0x04, &mut old) != STATUS_SUCCESS {
            return Err(STATUS_ENTRYPOINT_NOT_FOUND);
        }
        for i in 0..count {
            let entry = rd64(lookup + 8 * i);
            let (name, ordinal) = if entry >> 63 != 0 {
                (&[][..], (entry & 0xffff) as u32)
            } else {
                (c_str(base + (entry as usize & 0x7fff_ffff) + 2), 0)
            };
            match export(dll, name, ordinal) {
                // SAFETY: the IAT slot, made writable above.
                Some(target) => unsafe {
                    ((iat + 8 * i) as *mut u64).write_unaligned(target as u64)
                },
                None => {
                    // As on Windows the image will not load, but every
                    // missing import is reported first (not just the first).
                    report_missing_export(c_str(base + rd32(desc + 12) as usize), name, ordinal);
                    missing = true;
                }
            }
        }
        let (mut at, mut size, mut ignored) = (iat, count * 8, 0u32);
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
/// from the executable's directory (the search path is ignored).
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
    const LOAD_LIBRARY_SEARCH_SYSTEM32: usize = 0x800;
    // A name with an absolute directory loads from that directory only.
    let dir = explicit_dir(name);
    let search = match dir {
        Some(d) => Search::Dir(d),
        None if flags & LOAD_LIBRARY_SEARCH_SYSTEM32 != 0 => Search::System32,
        None => Search::Default,
    };
    let mut buf = [0u8; MAX_NAME];
    let loaded = unicode_name(name, &mut buf).and_then(|n| load_module(&buf[..n], search));
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
    let module = if module == 0 {
        // SAFETY: gs:[0x60] is the PEB; ImageBaseAddress is at +0x10.
        unsafe {
            let peb: usize;
            core::arch::asm!("mov {}, gs:[0x60]", out(reg) peb, options(nostack, readonly));
            rd64(peb + 0x10) as usize
        }
    } else {
        module
    };
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
            _ => export(module, name, 0).ok_or(STATUS_PROCEDURE_NOT_FOUND),
        }
    };
    let found = found.and_then(|target| {
        let missing =
            if name.is_null() { STATUS_ORDINAL_NOT_FOUND } else { STATUS_PROCEDURE_NOT_FOUND };
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

/// Records ntdll and the executable as pinned modules.
fn register_startup_modules(ntdll: usize, image: usize) -> Result<(), u32> {
    let ntdll_slot = add_module(b"ntdll.dll", ntdll, PINNED)?;
    // ntdll's file: <NtSystemRoot>\System32\ntdll.dll.
    let mut path = [0u16; MAX_PATH];
    let mut n = 0;
    // SAFETY: KUSER_SHARED_DATA.NtSystemRoot (+0x30), NUL-terminated.
    unsafe {
        while n < 200 && *((0x7ffe_0030 + 2 * n) as *const u16) != 0 {
            path[n] = *((0x7ffe_0030 + 2 * n) as *const u16);
            n += 1;
        }
    }
    for &b in b"\\System32\\ntdll.dll" {
        path[n] = u16::from(b);
        n += 1;
    }
    set_module_path(ntdll_slot, &path[..n]);
    // SAFETY: gs:[0x60] is the PEB; ProcessParameters->ImagePathName is
    // filled in by the kernel.
    let image_path = unsafe {
        let peb: usize;
        core::arch::asm!("mov {}, gs:[0x60]", out(reg) peb, options(nostack, readonly));
        let params = rd64(peb + 0x20) as usize;
        let len = usize::from(*((params + 0x60) as *const u16)) / 2;
        core::slice::from_raw_parts(rd64(params + 0x68) as *const u16, len)
    };
    let mut name = [0u8; MAX_NAME];
    // An executable whose name is not a plain ASCII one is just not listed.
    if let Ok(n) = normalize(image_path.iter().copied(), &mut name) {
        let slot = add_module(&name[..n], image, PINNED)?;
        set_module_path(slot, image_path);
    }
    Ok(())
}

/// NTSTATUS -> Win32 error code, for the statuses gala returns (written from
/// the documented correspondences: Microsoft's NTSTATUS/Win32 error
/// reference, Wine's mapping table as a cross-check).
const STATUS_TO_ERROR: &[(u32, u32)] = &[
    (0x0000_0000, 0),    // SUCCESS
    (0x0000_0102, 1460), // TIMEOUT -> ERROR_TIMEOUT
    (0x0000_0103, 997),  // PENDING -> ERROR_IO_PENDING
    (0x4000_0000, 183),  // OBJECT_NAME_EXISTS -> ERROR_ALREADY_EXISTS
    (0x8000_0005, 234),  // BUFFER_OVERFLOW -> ERROR_MORE_DATA
    (0x8000_0006, 18),   // NO_MORE_FILES
    (0xc000_0001, 31),   // UNSUCCESSFUL -> ERROR_GEN_FAILURE
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
    (0xc000_0030, 87),   // INVALID_PARAMETER_MIX -> ERROR_INVALID_PARAMETER
    (0xc000_000f, 2),    // NO_SUCH_FILE -> ERROR_FILE_NOT_FOUND
    (0xc000_0011, 38),   // END_OF_FILE -> ERROR_HANDLE_EOF
    (0xc000_0017, 8),    // NO_MEMORY -> ERROR_NOT_ENOUGH_MEMORY
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
    (0xc000_0035, 183),  // OBJECT_NAME_COLLISION -> ERROR_ALREADY_EXISTS
    (0xc000_0039, 161),  // OBJECT_PATH_INVALID -> ERROR_BAD_PATHNAME
    (0xc000_003a, 3),    // OBJECT_PATH_NOT_FOUND -> ERROR_PATH_NOT_FOUND
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
    (0xc000_0181, 1021), // CHILD_MUST_BE_VOLATILE -> ERROR_CHILD_MUST_BE_VOLATILE
    (0xc000_0120, 995),  // CANCELLED -> ERROR_OPERATION_ABORTED
    (0xc000_0135, 126),  // DLL_NOT_FOUND -> ERROR_MOD_NOT_FOUND
    (0xc000_0138, 182),  // ORDINAL_NOT_FOUND -> ERROR_INVALID_ORDINAL
    (0xc000_0139, 127),  // ENTRYPOINT_NOT_FOUND -> ERROR_PROC_NOT_FOUND
    (0xc000_0225, 1168), // NOT_FOUND -> ERROR_NOT_FOUND
];

/// RtlNtStatusToDosError(Status): the Win32 error for an NTSTATUS
/// (ERROR_MR_MID_NOT_FOUND, 317, for statuses without one).
#[unsafe(no_mangle)]
pub extern "system" fn RtlNtStatusToDosError(status: u32) -> u32 {
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
    let peb: usize;
    // SAFETY: gs:[0x60] is TEB.ProcessEnvironmentBlock.
    unsafe { core::arch::asm!("mov {}, gs:[0x60]", out(reg) peb, options(nostack, readonly)) };
    peb
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

/// The initial thread's first user-mode instruction.
///
/// # Safety
/// Called by the kernel with a valid CONTEXT.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn LdrInitializeThunk(context: *const u8, ntdll_base: u64) -> ! {
    // SAFETY: the kernel passes a readable CONTEXT; the state is only
    // written here, before any other thread exists.
    unsafe {
        let rip = (context.add(CONTEXT_RIP) as *const u64).read_unaligned();
        let state = &raw mut GalaTestNtdllState;
        let calls = (*state)[3];
        state.write_volatile([MAGIC, ntdll_base, rip, calls + 1]);
    }
    // SAFETY: gs:[0x60] is the PEB; ImageBaseAddress is at +0x10.
    let image = unsafe {
        let peb: usize;
        core::arch::asm!("mov {}, gs:[0x60]", out(reg) peb, options(nostack, readonly));
        rd64(peb + 0x10) as usize
    };
    // The process heap, before any DLL's code runs.
    heap::create_process_heap();
    let started = with_loader_lock(|| {
        register_startup_modules(ntdll_base as usize, image)?;
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
    NtContinue(context, 0);
    // NtContinue only returns on failure.
    loop {
        core::hint::spin_loop();
    }
}

/// VerSetConditionMask: three condition bits for each VER_* type bit,
/// at three times the bit's index (kernel32 forwards here, as on Windows).
#[unsafe(no_mangle)]
pub extern "system" fn VerSetConditionMask(mask: u64, kind: u32, condition: u8) -> u64 {
    let mut mask = mask;
    for bit in 0..8 {
        if kind & (1 << bit) != 0 {
            let shift = 3 * bit;
            mask = (mask & !(7 << shift)) | (u64::from(condition & 7) << shift);
        }
    }
    mask
}

/// NtTerminateProcess on the pinned build (to be verified).
const NT_TERMINATE_PROCESS: u32 = 0x2c;

/// NtTerminateProcess(ProcessHandle, ExitStatus).
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "system" fn NtTerminateProcess(_process: isize, _status: u32) -> i32 {
    core::arch::naked_asm!(
        "mov r10, rcx",
        "mov eax, {n}",
        "syscall",
        "ret",
        n = const NT_TERMINATE_PROCESS
    )
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

/// The kernel resumes a thread here to run a user APC: rsp points at the
/// CONTEXT to return to, whose P1Home..P4Home hold NormalContext,
/// SystemArgument1, SystemArgument2 and NormalRoutine. The routine runs
/// with the CONTEXT's home area as its shadow space; NtContinue with
/// TestAlert then delivers the next APC or resumes the CONTEXT.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "system" fn KiUserApcDispatcher() -> ! {
    core::arch::naked_asm!(
        "mov rcx, [rsp]",
        "mov rdx, [rsp + 8]",
        "mov r8, [rsp + 0x10]",
        "mov rax, [rsp + 0x18]",
        "call rax",
        "mov rcx, rsp",
        "mov edx, 1",
        "call {continue}",
        "int3",
        continue = sym NtContinue,
    )
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

// Memory routines (ntdll exports them; the compiler also calls them).
// `rep` string instructions, so they cannot compile back into calls.

/// memcpy(dst, src, n) -> dst
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn memcpy(_dst: *mut u8, _src: *const u8, _n: usize) -> *mut u8 {
    core::arch::naked_asm!(
        "mov rax, rcx",
        "mov r9, rdi",
        "mov r10, rsi",
        "mov rdi, rcx",
        "mov rsi, rdx",
        "mov rcx, r8",
        "rep movsb",
        "mov rdi, r9",
        "mov rsi, r10",
        "ret",
    )
}

/// memmove(dst, src, n) -> dst (overlap-safe)
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn memmove(_dst: *mut u8, _src: *const u8, _n: usize) -> *mut u8 {
    core::arch::naked_asm!(
        "mov rax, rcx",
        "mov r9, rdi",
        "mov r10, rsi",
        "mov rdi, rcx",
        "mov rsi, rdx",
        "mov rcx, r8",
        "cmp rdi, rsi",
        "jbe 2f",
        // Destination above source: copy backwards.
        "lea rsi, [rsi + rcx - 1]",
        "lea rdi, [rdi + rcx - 1]",
        "std",
        "rep movsb",
        "cld",
        "jmp 3f",
        "2:",
        "rep movsb",
        "3:",
        "mov rdi, r9",
        "mov rsi, r10",
        "ret",
    )
}

/// memset(dst, value, n) -> dst
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn memset(_dst: *mut u8, _value: i32, _n: usize) -> *mut u8 {
    core::arch::naked_asm!(
        "mov r9, rdi",
        "mov r10, rcx",
        "mov rdi, rcx",
        "mov eax, edx",
        "mov rcx, r8",
        "rep stosb",
        "mov rdi, r9",
        "mov rax, r10",
        "ret",
    )
}

/// __chkstk: the stack probe compilers call before allocating more than a
/// page of stack (rax = bytes). Touches each page from the caller's stack
/// pointer down, so guard pages grow the stack in order; changes no
/// register (the caller then subtracts rax from rsp). Exported by ntdll,
/// as on Windows.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn __chkstk() {
    core::arch::naked_asm!(
        "push rcx",
        "push rax",
        "lea rcx, [rsp + 24]", // the caller's rsp (after our return)
        "cmp rax, 0x1000",
        "jb 2f",
        "1:",
        "sub rcx, 0x1000",
        "test byte ptr [rcx], 0",
        "sub rax, 0x1000",
        "cmp rax, 0x1000",
        "ja 1b",
        "2:",
        "sub rcx, rax",
        "test byte ptr [rcx], 0",
        "pop rax",
        "pop rcx",
        "ret",
    )
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
    let base = if module == 0 {
        // SAFETY: gs:[0x60] is the PEB; ImageBaseAddress is at +0x10.
        unsafe {
            let peb: usize;
            core::arch::asm!("mov {}, gs:[0x60]", out(reg) peb, options(nostack, readonly));
            rd64(peb + 0x10) as usize
        }
    } else {
        module
    };
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
