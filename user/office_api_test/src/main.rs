//! The kernel32 and ntdll functions Microsoft Office's programs import
//! (ROADMAP OF: ODT's setup.exe and the Word, Excel and PowerPoint load
//! list), with the results of Windows. One source for two programs:
//! `office_api_test` (x64) and `wow64_office_api_test` (x86, built from
//! this file by user/wow64_office_api_test).
//!
//! Expected values come from Microsoft's documentation of each function
//! and from Wine's conformance tests run on Windows (dlls/kernel32/tests
//! file.c test_CreateFile2, volume.c test_GetVolumePathNamesForVolumeNameW,
//! actctx.c test_basic_info and test_find_string_fail, dlls/ntdll/tests
//! rtlstr.c's RtlIntegerToUnicodeString table).
//!
//! - CreateFile2: CREATE_NEW makes the file, again ERROR_FILE_EXISTS;
//!   CREATE_ALWAYS and OPEN_ALWAYS on it ERROR_ALREADY_EXISTS with a handle;
//!   OPEN_EXISTING with CREATEFILE2_EXTENDED_PARAMETERS reads it; a missing
//!   file ERROR_FILE_NOT_FOUND, "" ERROR_PATH_NOT_FOUND.
//! - FindFirstFileNameW: the file's one name, the path without its drive;
//!   FindNextFileNameW then ERROR_HANDLE_EOF.
//! - CreateMutexExW (CREATE_MUTEX_INITIAL_OWNER: owned; the same name again:
//!   ERROR_ALREADY_EXISTS; unowned: ReleaseMutex ERROR_NOT_OWNER);
//!   OpenSemaphoreA/W (a named semaphore through a second handle; a missing
//!   name ERROR_FILE_NOT_FOUND, NULL ERROR_INVALID_PARAMETER).
//! - K32EnumProcesses lists this process; K32GetProcessImageFileNameW
//!   names it in the NT namespace (`\Device\...\<name>.exe`);
//!   QueueUserWorkItem runs its function; GetThreadIOPendingFlag FALSE;
//!   IsSystemResumeAutomatic FALSE; GetPhysicallyInstalledSystemMemory at
//!   least the memory Windows reports (or ERROR_INVALID_DATA where the
//!   firmware has no memory table: some virtual machines);
//!   WerRegisterMemoryBlock/WerUnregisterMemoryBlock S_OK;
//!   WerRegisterFile (WerRegFileTypeOther) and WerUnregisterFile of it
//!   S_OK (their documentation: the Click-to-Run client imports them);
//!   RtlCaptureStackBackTrace (kernel32's export) at most what was asked.
//! - GetThreadIOPendingFlag (Microsoft's documentation; ThreadIsIoPending:
//!   the thread's I/O requests not yet complete): TRUE while an overlapped
//!   ConnectNamedPipe waits for a client, for this thread and as another
//!   thread sees it, FALSE for that other thread; FALSE once the client
//!   connected; TRUE for an overlapped read waiting for data, FALSE once
//!   CancelIo ended it (ERROR_OPERATION_ABORTED); TRUE for an overlapped
//!   ReadDirectoryChangesW, FALSE once CancelIoEx ended it. An `info`
//!   line: a read on a pipe with a completion port (thread-agnostic I/O,
//!   which Windows Internals describes: not counted on gala).
//! - imm32's ImmDisableIME for the calling thread (0) and the process (-1):
//!   TRUE.
//! - normaliz's IdnToAscii and IdnToUnicode (setup.exe's next delay-loaded
//!   DLL): `bücher` is `xn--bcher-kva` and back, each the length of its
//!   result; powrprof's CallNtPowerInformation(ProcessorInformation):
//!   STATUS_INVALID_PARAMETER with no room (as Windows answers),
//!   STATUS_BUFFER_TOO_SMALL with room for one of several processors,
//!   success with room for every processor, the first entry processor 0.
//! - GetShortPathNameA of the Windows directory is itself; a missing path
//!   ERROR_FILE_NOT_FOUND; too small a buffer the size needed.
//! - GetVolumePathNamesForVolumeNameW: the system drive's volume lists the
//!   drive (`X:\`, double NUL-terminated); a buffer of 0: ERROR_MORE_DATA;
//!   "" ERROR_INVALID_NAME; an unknown volume GUID ERROR_FILE_NOT_FOUND.
//! - Activation contexts: none is active (GetCurrentActCtx NULL);
//!   QueryActCtxW's ActivationContextBasicInformation of it (NULL, flags
//!   0); FindActCtxSectionStringW's argument checks (ERROR_INVALID_PARAMETER).
//! - ntdll: RtlIntegerToUnicodeString (bases 0, 2, 10, 16; a buffer exactly
//!   the digits' size: no NUL; too small STATUS_BUFFER_OVERFLOW, the
//!   buffer untouched; base 20 STATUS_INVALID_PARAMETER);
//!   RtlIsNameInExpression (`*`, `?`, case); the AVL generic tables
//!   (insert, duplicates, lookup, enumerate in order, get by index,
//!   delete, count, empty; the callbacks get the table).
//! - GetUserDefaultGeoName (setup.exe asks kernel32 for it after reading
//!   its configuration): with no buffer, the size of the name with its
//!   NUL; the name, the user's region as a two-letter ISO 3166 code that
//!   is GetUserGeoID's GEO_ISO2 (whatever the machine's region, the two
//!   agree) or a three-digit UN M.49 code, that size; one character less
//!   ERROR_INSUFFICIENT_BUFFER.
//! - wldp (Windows Lockdown Policy, which setup.exe loads to check for S
//!   mode; expectations from Wine's dlls/wldp/tests run on Windows):
//!   WldpQueryWindowsLockdownMode S_OK and UNLOCKED; WldpGetLockdownPolicy
//!   for the global host S_OK with WLDP_LOCKDOWN_DEFINED_FLAG alone (no
//!   code integrity policy, as on a machine without App Control).
//! - urlmon's security manager (CoInternetCreateSecurityManager, which
//!   setup.exe creates on its download's error path; expectations from
//!   Wine's dlls/urlmon/tests sec_mgr.c test_SecurityManager run on
//!   Windows): MapUrlToZone and GetSecurityId of res:, file: (with a
//!   drive, and a host), a drive path, http:, ftp:, about: and zip: URLs
//!   (Local Machine 0 or Internet 3; the IDs `scheme:host` or "file:"
//!   then the zone's DWORD); a relative URL E_INVALIDARG; "http:" without
//!   "//" S_OK with URLZONE_INVALID and no ID; NULL arguments
//!   E_INVALIDARG, ProcessUrlAction's too.
//! - iphlpapi's GetAdaptersInfo (setup.exe delay-loads it; Wine's
//!   iphlpapi.c testGetAdaptersInfo run on Windows): no size pointer
//!   ERROR_INVALID_PARAMETER; no buffer ERROR_NO_DATA (no adapter) or
//!   ERROR_BUFFER_OVERFLOW with the bytes needed, then with them
//!   NO_ERROR; each adapter, linked by Next inside the buffer, is no
//!   loopback, is named by its GUID in braces, has its MAC's length (at
//!   most 8), an address and mask, and a gateway address and mask (as
//!   text, "0.0.0.0" when there is none). One byte less is
//!   ERROR_BUFFER_OVERFLOW again. The size needed is 704 bytes per
//!   adapter in a 64-bit process, 640 in a 32-bit one (adapters with one
//!   address each). The scenarios give gala a network card.
//! - AttachConsole and AllocConsole in a process that has a console
//!   (GetConsoleCP not 0; run-all.ps1's programs and gala's have one):
//!   ERROR_ACCESS_DENIED; GetDiskFreeSpaceA of the system drive.
//! - What the Click-to-Run client's UI (OfficeC2RClient.exe) needs to
//!   start (Wine's atom.c run on Windows): the local atom table
//!   (InitAtomTable TRUE; AddAtom W and A one string atom whatever the
//!   case, FindAtom finds it, the global table does not; GetAtomName the
//!   first spelling, cut to fit with its NUL and the characters copied
//!   returned; "#1234" and MAKEINTATOM their own values, "#1234" the
//!   name of 1234; counted deletes, then ERROR_FILE_NOT_FOUND); the
//!   app-model API set `api-ms-win-appmodel-runtime-l1-1-2.dll` loads,
//!   its AppPolicy queries answer a desktop program's policies,
//!   GetCurrentPackageFullName APPMODEL_ERROR_NO_PACKAGE.
//!   oleaut32 by ordinal, as the client imports it (Wine's vartest.c,
//!   safearray.c and olefont.c run on Windows): DECIMAL from an integer
//!   and a double (15 digits, no trailing zeros) and back; addition and
//!   subtraction at the larger scale; a product at the scales' sum
//!   (1.5 x 2 = 3.0); quotients exact where they can be (1/4 = 0.25),
//!   else 28 digits (1/3), by zero DISP_E_DIVBYZERO; Int towards
//!   negative infinity, Fix towards zero; Round; Cmp across scales and
//!   signs. SafeArrayRedim grows the last dimension (its lower bound
//!   too; old elements kept, new ones zero), shrinks it, and refuses a
//!   locked array (DISP_E_ARRAYISLOCKED). OleCreateFontIndirect gives
//!   an IFont with the description's name, size, bold (weight above
//!   550), italic and weight; put_Bold(FALSE) is weight 400; its HFONT
//!   has that face, weight and italic at the size in points at the
//!   screen's pixels per inch.
//!
//! Prints `<name>: ok` and exits with 0, or prints `<name>: check N got X
//! (line L)` and exits with N.

#![no_std]
#![no_main]
#![allow(clippy::manual_c_str_literals)]

use core::ffi::c_void;
use core::ptr::{null, null_mut};
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "office_api_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_office_api_test";

type Handle = usize;
const INVALID: Handle = usize::MAX;

/// CREATEFILE2_EXTENDED_PARAMETERS (fileapi.h).
#[repr(C)]
struct CreateFile2Params {
    size: u32,
    attributes: u32,
    flags: u32,
    qos: u32,
    security: usize,
    template: Handle,
}

/// ACTCTX_SECTION_KEYED_DATA (winbase.h), up to what the checks use.
#[repr(C)]
struct KeyedData {
    size: u32,
    version: u32,
    data: usize,
    length: u32,
    section_global: usize,
    section_global_length: u32,
    section_base: usize,
    section_total_length: u32,
    actctx: Handle,
    assembly_roster_index: u32,
    flags: u32,
    // ACTCTX_SECTION_KEYED_DATA_ASSEMBLY_METADATA: five pointer-sized
    // and ULONG fields.
    metadata: [usize; 6],
}

/// ACTIVATION_CONTEXT_BASIC_INFORMATION.
#[repr(C)]
struct BasicInfo {
    actctx: Handle,
    flags: u32,
}

/// UNICODE_STRING.
#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum: u16,
    buffer: *mut u16,
}

/// RTL_AVL_TABLE (ntddk.h): the balanced links of the root, the ordered
/// element cache, the counts, the restart key and the four callbacks and
/// context.
#[repr(C)]
struct AvlTable {
    root: [usize; 4],
    ordered_pointer: usize,
    which_ordered_element: u32,
    number_elements: u32,
    depth: u32,
    restart_key: usize,
    delete_count: u32,
    compare: usize,
    allocate: usize,
    free: usize,
    context: usize,
}

const _: () = assert!(size_of::<AvlTable>() == if cfg!(target_arch = "x86") { 56 } else { 104 });

type Compare = extern "system" fn(*mut AvlTable, *const c_void, *const c_void) -> u32;
type Allocate = extern "system" fn(*mut AvlTable, u32) -> *mut c_void;
type Free = extern "system" fn(*mut AvlTable, *mut c_void);

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetSystemInfo(info: *mut u8);
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> i32;
    fn GetLastError() -> u32;
    fn SetLastError(e: u32);
    fn GetTempPathW(len: u32, buf: *mut u16) -> u32;
    fn GetLongPathNameW(short: *const u16, long: *mut u16, len: u32) -> u32;
    fn DeleteFileW(name: *const u16) -> i32;
    fn GetCurrentProcess() -> Handle;
    fn GetCurrentProcessId() -> u32;
    fn GetCurrentThread() -> Handle;
    fn GetModuleFileNameW(module: usize, buf: *mut u16, len: u32) -> u32;
    fn GetWindowsDirectoryA(buf: *mut u8, len: u32) -> u32;
    fn GetWindowsDirectoryW(buf: *mut u16, len: u32) -> u32;
    fn ReleaseMutex(h: Handle) -> i32;
    fn CreateSemaphoreW(attributes: usize, initial: i32, maximum: i32, name: *const u16) -> Handle;
    fn ReleaseSemaphore(h: Handle, count: i32, previous: *mut i32) -> i32;
    fn CreateEventW(attributes: usize, manual: i32, initial: i32, name: *const u16) -> Handle;
    fn SetEvent(h: Handle) -> i32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn FindClose(h: Handle) -> i32;
    fn GetVolumeNameForVolumeMountPointW(mount: *const u16, name: *mut u16, len: u32) -> i32;
    fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> i32;
    fn GetCurrentActCtx(h: *mut Handle) -> i32;

    // The functions under test.
    fn CreateFile2(
        name: *const u16,
        access: u32,
        share: u32,
        disposition: u32,
        params: *const CreateFile2Params,
    ) -> Handle;
    fn FindFirstFileNameW(name: *const u16, flags: u32, len: *mut u32, link: *mut u16) -> Handle;
    fn FindNextFileNameW(h: Handle, len: *mut u32, link: *mut u16) -> i32;
    fn CreateMutexExW(attributes: usize, name: *const u16, flags: u32, access: u32) -> Handle;
    fn OpenSemaphoreA(access: u32, inherit: i32, name: *const u8) -> Handle;
    fn OpenSemaphoreW(access: u32, inherit: i32, name: *const u16) -> Handle;
    fn K32EnumProcesses(ids: *mut u32, size: u32, needed: *mut u32) -> i32;
    fn K32GetProcessImageFileNameW(process: Handle, buf: *mut u16, len: u32) -> u32;
    fn QueueUserWorkItem(
        f: extern "system" fn(*mut c_void) -> u32,
        ctx: *mut c_void,
        flags: u32,
    ) -> i32;
    fn GetThreadIOPendingFlag(thread: Handle, pending: *mut i32) -> i32;
    fn CreateNamedPipeW(
        name: *const u16,
        open: u32,
        mode: u32,
        instances: u32,
        out_size: u32,
        in_size: u32,
        timeout: u32,
        attributes: usize,
    ) -> Handle;
    fn ConnectNamedPipe(pipe: Handle, overlapped: *mut Overlapped) -> i32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        attributes: usize,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> Handle;
    fn CancelIo(h: Handle) -> i32;
    fn CancelIoEx(h: Handle, ov: *mut Overlapped) -> i32;
    fn GetOverlappedResult(h: Handle, ov: *mut Overlapped, n: *mut u32, wait: i32) -> i32;
    fn ReadDirectoryChangesW(
        dir: Handle,
        buf: *mut u32,
        len: u32,
        subtree: i32,
        filter: u32,
        returned: *mut u32,
        ov: *mut Overlapped,
        routine: usize,
    ) -> i32;
    fn CreateIoCompletionPort(file: Handle, port: Handle, key: usize, threads: u32) -> Handle;
    fn CreateDirectoryW(name: *const u16, attributes: usize) -> i32;
    fn RemoveDirectoryW(name: *const u16) -> i32;
    fn DuplicateHandle(
        from: Handle,
        h: Handle,
        to: Handle,
        out: *mut Handle,
        access: u32,
        inherit: i32,
        options: u32,
    ) -> i32;
    fn CreateThread(
        security: usize,
        stack: usize,
        start: extern "system" fn(usize) -> u32,
        param: usize,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn IsSystemResumeAutomatic() -> i32;
    fn GetPhysicallyInstalledSystemMemory(kb: *mut u64) -> i32;
    fn WerRegisterMemoryBlock(p: *mut c_void, size: u32) -> u32;
    fn WerRegisterFile(path: *const u16, kind: u32, flags: u32) -> u32;
    fn WerUnregisterFile(path: *const u16) -> u32;
    fn WerUnregisterMemoryBlock(p: *mut c_void) -> u32;
    fn RtlCaptureStackBackTrace(skip: u32, count: u32, frames: *mut usize, hash: *mut u32) -> u16;
    fn GetShortPathNameA(long: *const u8, short: *mut u8, len: u32) -> u32;
    fn GetVolumePathNamesForVolumeNameW(
        volume: *const u16,
        names: *mut u16,
        len: u32,
        returned: *mut u32,
    ) -> i32;
    fn QueryActCtxW(
        flags: u32,
        actctx: Handle,
        sub: *mut c_void,
        class: u32,
        buf: *mut c_void,
        len: usize,
        written: *mut usize,
    ) -> i32;
    fn FindActCtxSectionStringW(
        flags: u32,
        guid: *const c_void,
        section: u32,
        name: *const u16,
        data: *mut KeyedData,
    ) -> i32;
    fn AttachConsole(pid: u32) -> i32;
    fn GetConsoleCP() -> u32;
    fn AllocConsole() -> i32;
    fn GetDiskFreeSpaceA(
        root: *const u8,
        sectors: *mut u32,
        bytes: *mut u32,
        free: *mut u32,
        total: *mut u32,
    ) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "imm32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "imm32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn ImmDisableIME(thread: u32) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "normaliz", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "normaliz", kind = "raw-dylib"))]
unsafe extern "system" {
    fn IdnToAscii(flags: u32, unicode: *const u16, n: i32, ascii: *mut u16, size: i32) -> i32;
    fn IdnToUnicode(flags: u32, ascii: *const u16, n: i32, unicode: *mut u16, size: i32) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn InitAtomTable(size: u32) -> i32;
    fn AddAtomW(name: *const u16) -> u16;
    fn AddAtomA(name: *const u8) -> u16;
    fn FindAtomW(name: *const u16) -> u16;
    fn FindAtomA(name: *const u8) -> u16;
    fn DeleteAtom(atom: u16) -> u16;
    fn GetAtomNameW(atom: u16, buf: *mut u16, size: i32) -> u32;
    fn GetAtomNameA(atom: u16, buf: *mut u8, size: i32) -> u32;
    fn GlobalFindAtomW(name: *const u16) -> u16;
    fn LoadLibraryW(name: *const u16) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> usize;
    fn GetUserDefaultGeoName(name: *mut u16, size: i32) -> i32;
    fn GetUserGeoID(class: u32) -> i32;
    fn GetGeoInfoW(geo: i32, kind: u32, data: *mut u16, size: i32, lang: u16) -> i32;
}

/// WLDP_HOST_INFORMATION (wldp.h).
#[repr(C)]
struct WldpHostInformation {
    revision: u32,
    host_id: u32,
    source: *const u16,
    handle: Handle,
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "wldp", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "wldp", kind = "raw-dylib"))]
unsafe extern "system" {
    fn WldpQueryWindowsLockdownMode(mode: *mut u32) -> i32;
    fn WldpGetLockdownPolicy(host: *const WldpHostInformation, state: *mut u32, flags: u32) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "urlmon", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "urlmon", kind = "raw-dylib"))]
unsafe extern "system" {
    fn CoInternetCreateSecurityManager(provider: usize, manager: *mut usize, reserved: u32) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "iphlpapi", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "iphlpapi", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetAdaptersInfo(info: *mut IpAdapterInfo, size: *mut u32) -> u32;
}

/// IP_ADDR_STRING (iptypes.h).
#[repr(C)]
struct IpAddrString {
    next: usize,
    address: [u8; 16],
    mask: [u8; 16],
    context: u32,
}

/// IP_ADAPTER_INFO (iptypes.h), up to the fields the checks read.
#[repr(C)]
struct IpAdapterInfo {
    next: *mut IpAdapterInfo,
    combo_index: u32,
    adapter_name: [u8; 260],
    description: [u8; 132],
    address_length: u32,
    address: [u8; 8],
    index: u32,
    if_type: u32,
    dhcp_enabled: u32,
    current_ip_address: usize,
    ip_address_list: IpAddrString,
    gateway_list: IpAddrString,
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "powrprof", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "powrprof", kind = "raw-dylib"))]
unsafe extern "system" {
    fn CallNtPowerInformation(
        level: u32,
        input: *const u8,
        input_size: u32,
        output: *mut u8,
        output_size: u32,
    ) -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn RtlIntegerToUnicodeString(value: u32, base: u32, out: *mut UnicodeString) -> u32;
    fn RtlIsNameInExpression(
        expression: *const UnicodeString,
        name: *const UnicodeString,
        ignore_case: u8,
        upcase: *const u16,
    ) -> u8;
    fn RtlInitializeGenericTableAvl(
        table: *mut AvlTable,
        compare: Compare,
        allocate: Allocate,
        free: Free,
        context: usize,
    );
    fn RtlInsertElementGenericTableAvl(
        table: *mut AvlTable,
        buffer: *const c_void,
        size: u32,
        new: *mut u8,
    ) -> *mut c_void;
    fn RtlDeleteElementGenericTableAvl(table: *mut AvlTable, buffer: *const c_void) -> u8;
    fn RtlLookupElementGenericTableAvl(table: *mut AvlTable, buffer: *const c_void) -> *mut c_void;
    fn RtlEnumerateGenericTableAvl(table: *mut AvlTable, restart: u8) -> *mut c_void;
    fn RtlEnumerateGenericTableWithoutSplayingAvl(
        table: *mut AvlTable,
        restart: *mut usize,
    ) -> *mut c_void;
    fn RtlGetElementGenericTableAvl(table: *mut AvlTable, index: u32) -> *mut c_void;
    fn RtlNumberGenericTableElementsAvl(table: *mut AvlTable) -> u32;
    fn RtlIsGenericTableEmptyAvl(table: *mut AvlTable) -> u8;
}

/// MEMORYSTATUSEX.
#[repr(C)]
#[derive(Default)]
struct MemoryStatusEx {
    length: u32,
    load: u32,
    total_phys: u64,
    avail_phys: u64,
    total_page_file: u64,
    avail_page_file: u64,
    total_virtual: u64,
    avail_virtual: u64,
    avail_extended_virtual: u64,
}

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const CREATE_NEW: u32 = 1;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const OPEN_ALWAYS: u32 = 4;
const FILE_SHARE_READ: u32 = 1;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_PATH_NOT_FOUND: u32 = 3;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_INVALID_DATA: u32 = 13;
const ERROR_HANDLE_EOF: u32 = 38;
const ERROR_FILE_EXISTS: u32 = 80;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INVALID_NAME: u32 = 123;
const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_MORE_DATA: u32 = 234;
const ERROR_NOT_OWNER: u32 = 288;
const STATUS_BUFFER_OVERFLOW: u32 = 0x8000_0005;
const STATUS_INVALID_PARAMETER: u32 = 0xc000_000d;

// ---- Output and checks ---------------------------------------------------

fn print(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid arguments.
    unsafe {
        WriteFile(GetStdHandle(-11i32 as u32), text.as_ptr(), text.len() as u32, &mut written, 0)
    };
}

struct Line {
    buf: [u8; 160],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 160], n: 0 }
    }

    fn s(mut self, s: &[u8]) -> Self {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn dec(self, v: u64) -> Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        let mut v = v;
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&d[i..])
    }

    fn hex(self, v: u64) -> Self {
        let mut d = [0u8; 18];
        d[0] = b'0';
        d[1] = b'x';
        for i in 0..16 {
            d[2 + i] = b"0123456789abcdef"[((v >> (60 - 4 * i)) & 0xf) as usize];
        }
        self.s(&d)
    }

    fn print(self) {
        print(&self.buf[..self.n]);
    }
}

/// An `info` line: what Windows answers, for the record (not checked).
fn info(what: &str, v: u64) {
    Line::new()
        .s(NAME.as_bytes())
        .s(b": info ")
        .s(what.as_bytes())
        .s(b" ")
        .hex(v)
        .s(b"\r\n")
        .print();
}

/// OVERLAPPED.
#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    event: Handle,
}

impl Overlapped {
    fn with(event: Handle) -> Overlapped {
        Overlapped { internal: 0, internal_high: 0, offset: 0, offset_high: 0, event }
    }
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got)
            .s(b" (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(b")\r\n")
            .print();
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// A call that failed (`ok` false) with `error`.
    #[track_caller]
    fn fails(&mut self, ok: bool, error: u32) -> Result<(), u32> {
        // SAFETY: plain call.
        let e = unsafe { GetLastError() };
        self.ok(!ok && e == error, u64::from(e))
    }
}

/// A NUL-terminated UTF-16 string's units.
fn units(p: *const u16) -> &'static [u16] {
    let mut n = 0;
    // SAFETY: a NUL-terminated string.
    unsafe {
        while p.add(n).read_volatile() != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    }
}

fn eq_ci(a: &[u16], b: &[u16]) -> bool {
    let up = |c: u16| if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c };
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| up(x) == up(y))
}

fn ends_with_ci(a: &[u16], b: &[u16]) -> bool {
    a.len() >= b.len() && eq_ci(&a[a.len() - b.len()..], b)
}

/// An ASCII string as NUL-terminated UTF-16.
fn w<const N: usize>(s: &str) -> [u16; N] {
    let mut out = [0u16; N];
    for (o, b) in out.iter_mut().zip(s.bytes()) {
        *o = u16::from(b);
    }
    out
}

// ---- Files ---------------------------------------------------------------

fn files(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid arguments throughout; the file is ours.
    unsafe {
        let mut path = [0u16; 300];
        let n = GetTempPathW(260, path.as_mut_ptr()) as usize;
        c.ok(n > 0 && n < 240, n as u64)?;
        for (i, ch) in "gala_office_api.txt".bytes().enumerate() {
            path[n + i] = u16::from(ch);
        }
        DeleteFileW(path.as_ptr());
        let p = path.as_ptr();

        let h = CreateFile2(p, GENERIC_READ | GENERIC_WRITE, 0, CREATE_NEW, null());
        c.ok(h != INVALID, 0)?;
        let mut written = 0;
        c.ok(WriteFile(h, b"gala".as_ptr(), 4, &mut written, 0) != 0 && written == 4, 0)?;
        CloseHandle(h);
        c.fails(CreateFile2(p, GENERIC_READ, 0, CREATE_NEW, null()) != INVALID, ERROR_FILE_EXISTS)?;
        let params = CreateFile2Params {
            size: size_of::<CreateFile2Params>() as u32,
            attributes: 0x80, // FILE_ATTRIBUTE_NORMAL
            flags: 0,
            qos: 0,
            security: 0,
            template: 0,
        };
        SetLastError(0xdead);
        let h = CreateFile2(p, GENERIC_READ, FILE_SHARE_READ, OPEN_ALWAYS, &params);
        c.ok(h != INVALID, 0)?;
        c.eq(GetLastError().into(), ERROR_ALREADY_EXISTS.into())?;
        let mut data = [0u8; 8];
        let mut read = 0;
        c.ok(ReadFile(h, data.as_mut_ptr(), 8, &mut read, 0) != 0 && read == 4, read.into())?;
        c.ok(&data[..4] == b"gala", 0)?;
        CloseHandle(h);
        let h = CreateFile2(p, GENERIC_READ, FILE_SHARE_READ, OPEN_EXISTING, &params);
        c.ok(h != INVALID, 0)?;
        CloseHandle(h);

        // FindFirstFileNameW: its one name, without the drive.
        let mut long = [0u16; 300];
        let ln = GetLongPathNameW(p, long.as_mut_ptr(), 300) as usize;
        c.ok(ln > 3 && ln < 300, ln as u64)?;
        let mut name = [0u16; 300];
        let mut len = 300u32;
        let f = FindFirstFileNameW(p, 0, &mut len, name.as_mut_ptr());
        c.ok(f != INVALID, GetLastError().into())?;
        c.ok(eq_ci(units(name.as_ptr()), &long[2..ln]), len.into())?;
        let mut len = 300u32;
        c.fails(FindNextFileNameW(f, &mut len, name.as_mut_ptr()) != 0, ERROR_HANDLE_EOF)?;
        c.ok(FindClose(f) != 0, 0)?;

        SetLastError(0xdead);
        let h = CreateFile2(p, GENERIC_READ, FILE_SHARE_READ, CREATE_ALWAYS, &params);
        c.ok(h != INVALID, 0)?;
        c.eq(GetLastError().into(), ERROR_ALREADY_EXISTS.into())?;
        CloseHandle(h);
        c.ok(DeleteFileW(p) != 0, 0)?;
        c.fails(
            CreateFile2(p, GENERIC_READ, 0, OPEN_EXISTING, null()) != INVALID,
            ERROR_FILE_NOT_FOUND,
        )?;
        let empty = [0u16; 1];
        c.fails(
            CreateFile2(empty.as_ptr(), GENERIC_READ, 0, CREATE_NEW, &params) != INVALID,
            ERROR_PATH_NOT_FOUND,
        )?;

        // GetShortPathNameA: the Windows directory is its own short form.
        let mut windows = [0u8; 260];
        let wn = GetWindowsDirectoryA(windows.as_mut_ptr(), 260) as usize;
        c.ok(wn > 2 && wn < 200, wn as u64)?;
        let mut short = [0u8; 260];
        c.eq(GetShortPathNameA(windows.as_ptr(), short.as_mut_ptr(), 260).into(), wn as u64)?;
        c.ok(short[..wn].eq_ignore_ascii_case(&windows[..wn]), 0)?;
        c.eq(GetShortPathNameA(windows.as_ptr(), short.as_mut_ptr(), 2).into(), wn as u64 + 1)?;
        let missing = &mut short;
        missing[..wn].copy_from_slice(&windows[..wn]);
        missing[wn..wn + 15].copy_from_slice(b"\\gala_no_such\0\0");
        let mut out = [0u8; 260];
        c.fails(
            GetShortPathNameA(missing.as_ptr(), out.as_mut_ptr(), 260) != 0,
            ERROR_FILE_NOT_FOUND,
        )?;

        // GetVolumePathNamesForVolumeNameW: the system drive's volume.
        let mut win = [0u16; 260];
        GetWindowsDirectoryW(win.as_mut_ptr(), 260);
        let root = [win[0], u16::from(b':'), u16::from(b'\\'), 0];
        let mut volume = [0u16; 64];
        c.ok(GetVolumeNameForVolumeMountPointW(root.as_ptr(), volume.as_mut_ptr(), 64) != 0, 0)?;
        let mut names = [0xffffu16; 128];
        let mut returned = 0u32;
        c.ok(
            GetVolumePathNamesForVolumeNameW(
                volume.as_ptr(),
                names.as_mut_ptr(),
                128,
                &mut returned,
            ) != 0,
            GetLastError().into(),
        )?;
        c.ok(returned >= 5 && (returned as usize) <= 128, returned.into())?;
        let list = &names[..returned as usize];
        c.ok(list[list.len() - 1] == 0 && list[list.len() - 2] == 0, 0)?;
        let mut found = false;
        let mut at = 0;
        while at < list.len() && list[at] != 0 {
            let one = units(list[at..].as_ptr());
            found |= eq_ci(one, &root[..3]);
            at += one.len() + 1;
        }
        c.ok(found, 0)?;
        c.fails(
            GetVolumePathNamesForVolumeNameW(volume.as_ptr(), names.as_mut_ptr(), 0, &mut returned)
                != 0,
            ERROR_MORE_DATA,
        )?;
        c.fails(
            GetVolumePathNamesForVolumeNameW(empty.as_ptr(), null_mut(), 0, null_mut()) != 0,
            ERROR_INVALID_NAME,
        )?;
        let none = w::<64>("\\\\?\\Volume{00000000-0000-0000-0000-000000000000}\\");
        c.fails(
            GetVolumePathNamesForVolumeNameW(none.as_ptr(), names.as_mut_ptr(), 128, &mut returned)
                != 0,
            ERROR_FILE_NOT_FOUND,
        )?;

        let (mut a, mut b, mut f, mut t) = (0, 0, 0, 0);
        let drive = [win[0] as u8, b':', b'\\', 0];
        c.ok(GetDiskFreeSpaceA(drive.as_ptr(), &mut a, &mut b, &mut f, &mut t) != 0, 0)?;
        c.ok(a > 0 && b >= 512 && t > 0, b.into())?;
    }
    Ok(())
}

// ---- Synchronization and processes -------------------------------------------

static WORK: AtomicUsize = AtomicUsize::new(0);

extern "system" fn work(ctx: *mut c_void) -> u32 {
    WORK.store(ctx as usize, Ordering::Release);
    // SAFETY: the event this test made.
    unsafe { SetEvent(ctx as Handle) };
    0
}

fn sync_and_processes(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid arguments throughout.
    unsafe {
        let name = w::<40>("Local\\gala_office_api_mutex");
        let m = CreateMutexExW(0, name.as_ptr(), 1, 0x1f_0001); // INITIAL_OWNER, MUTEX_ALL_ACCESS
        c.ok(m != 0, GetLastError().into())?;
        c.ok(ReleaseMutex(m) != 0, GetLastError().into())?;
        let again = CreateMutexExW(0, name.as_ptr(), 0, 0x1f_0001);
        c.ok(again != 0, 0)?;
        c.eq(GetLastError().into(), ERROR_ALREADY_EXISTS.into())?;
        CloseHandle(again);
        CloseHandle(m);
        let unowned = CreateMutexExW(0, null(), 0, 0x1f_0001);
        c.ok(unowned != 0, 0)?;
        c.fails(ReleaseMutex(unowned) != 0, ERROR_NOT_OWNER)?;
        CloseHandle(unowned);

        let sem_name = w::<40>("Local\\gala_office_api_sem");
        let s = CreateSemaphoreW(0, 1, 5, sem_name.as_ptr());
        c.ok(s != 0, 0)?;
        let opened = OpenSemaphoreA(0x1f_0003, 0, b"Local\\gala_office_api_sem\0".as_ptr());
        c.ok(opened != 0, GetLastError().into())?;
        let mut previous = -1;
        c.ok(ReleaseSemaphore(opened, 2, &mut previous) != 0, 0)?;
        c.eq(previous as u64, 1)?;
        let opened_w = OpenSemaphoreW(0x1f_0003, 0, sem_name.as_ptr());
        c.ok(opened_w != 0, 0)?;
        c.ok(ReleaseSemaphore(opened_w, 1, &mut previous) != 0, 0)?;
        c.eq(previous as u64, 3)?;
        CloseHandle(opened_w);
        CloseHandle(opened);
        CloseHandle(s);
        c.fails(
            OpenSemaphoreA(0x1f_0003, 0, b"Local\\gala_office_none\0".as_ptr()) != 0,
            ERROR_FILE_NOT_FOUND,
        )?;
        c.fails(OpenSemaphoreW(0x1f_0003, 0, null()) != 0, ERROR_INVALID_PARAMETER)?;

        // A work item runs.
        let done = CreateEventW(0, 1, 0, null());
        c.ok(QueueUserWorkItem(work, done as *mut c_void, 0) != 0, 0)?;
        c.eq(WaitForSingleObject(done, 10_000).into(), 0)?;
        c.eq(WORK.load(Ordering::Acquire) as u64, done as u64)?;
        CloseHandle(done);

        // Processes.
        let mut ids = [0u32; 1024];
        let mut needed = 0;
        c.ok(K32EnumProcesses(ids.as_mut_ptr(), 4096, &mut needed) != 0, 0)?;
        c.ok(needed % 4 == 0 && needed > 0 && needed <= 4096, needed.into())?;
        let me = GetCurrentProcessId();
        c.ok(ids[..needed as usize / 4].contains(&me), me.into())?;
        let mut image = [0u16; 300];
        let n = K32GetProcessImageFileNameW(GetCurrentProcess(), image.as_mut_ptr(), 300);
        c.ok(n > 0 && n < 300, n.into())?;
        let image = &image[..n as usize];
        c.ok(image.starts_with(&w::<8>("\\Device\\")[..8]), u64::from(image[0]))?;
        let mut module = [0u16; 300];
        let mn = GetModuleFileNameW(0, module.as_mut_ptr(), 300) as usize;
        let base = module[..mn].iter().rposition(|&ch| ch == u16::from(b'\\')).map_or(0, |i| i);
        c.ok(ends_with_ci(image, &module[base..mn]), 0)?;
        c.eq(K32GetProcessImageFileNameW(GetCurrentProcess(), module.as_mut_ptr(), 2).into(), 0)?;

        let mut pending = 1;
        c.ok(GetThreadIOPendingFlag(GetCurrentThread(), &mut pending) != 0, 0)?;
        c.eq(pending as u64, 0)?;
        c.eq(IsSystemResumeAutomatic() as u64, 0)?;
        let mut kb = 0u64;
        let mut status = MemoryStatusEx { length: 64, ..Default::default() };
        GlobalMemoryStatusEx(&mut status);
        if GetPhysicallyInstalledSystemMemory(&mut kb) != 0 {
            c.ok(kb * 1024 >= status.total_phys, kb)?;
        } else {
            c.eq(GetLastError().into(), ERROR_INVALID_DATA.into())?;
        }
        c.fails(GetPhysicallyInstalledSystemMemory(null_mut()) != 0, ERROR_INVALID_PARAMETER)?;
        let mut block = [0u8; 64];
        c.eq(WerRegisterMemoryBlock(block.as_mut_ptr().cast(), 64).into(), 0)?;
        c.eq(WerUnregisterMemoryBlock(block.as_mut_ptr().cast()).into(), 0)?;
        // WerRegFileTypeOther (2), no flags.
        let mut report = [0u16; 32];
        for (slot, &b) in report.iter_mut().zip(b"C:\\gala_wer_file.log") {
            *slot = b.into();
        }
        c.eq(WerRegisterFile(report.as_ptr(), 2, 0).into(), 0)?;
        c.eq(WerUnregisterFile(report.as_ptr()).into(), 0)?;
        let mut frames = [0usize; 8];
        let mut hash = 0;
        let got = RtlCaptureStackBackTrace(0, 4, frames.as_mut_ptr(), &mut hash);
        c.ok(got <= 4, got.into())?;
        c.ok(frames[4..].iter().all(|&f| f == 0), 0)?;
        #[cfg(target_arch = "x86_64")]
        c.ok(got >= 1 && frames[0] != 0, got.into())?;

        // A process that has a console cannot attach to or make another.
        if GetConsoleCP() != 0 {
            c.fails(AttachConsole(u32::MAX) != 0, ERROR_ACCESS_DENIED)?;
            c.fails(AllocConsole() != 0, ERROR_ACCESS_DENIED)?;
        }
    }
    Ok(())
}

// ---- Activation contexts ---------------------------------------------------------

fn actctx(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid arguments throughout.
    unsafe {
        let mut current = 1usize;
        c.ok(GetCurrentActCtx(&mut current) != 0, 0)?;
        c.eq(current as u64, 0)?;
        let mut basic = BasicInfo { actctx: 1, flags: 9 };
        let mut size = 0usize;
        let no_addref = 0x8000_0000;
        c.ok(
            QueryActCtxW(
                no_addref,
                0,
                null_mut(),
                1,
                (&raw mut basic).cast(),
                size_of::<BasicInfo>(),
                &mut size,
            ) != 0,
            GetLastError().into(),
        )?;
        c.eq(size as u64, size_of::<BasicInfo>() as u64)?;
        c.ok(basic.actctx == 0 && basic.flags == 0, basic.flags.into())?;
        basic = BasicInfo { actctx: 1, flags: 9 };
        c.ok(
            QueryActCtxW(
                no_addref | 4,
                0,
                null_mut(),
                1,
                (&raw mut basic).cast(),
                size_of::<BasicInfo>(),
                &mut size,
            ) != 0,
            0,
        )?;
        c.ok(basic.actctx == 0 && basic.flags == 0, basic.flags.into())?;

        let dll = w::<16>("gala_office.dll");
        let mut data: KeyedData = core::mem::zeroed();
        data.size = size_of::<KeyedData>() as u32;
        c.fails(
            FindActCtxSectionStringW(0, null(), 2, dll.as_ptr(), null_mut()) != 0,
            ERROR_INVALID_PARAMETER,
        )?;
        c.fails(
            FindActCtxSectionStringW(0, null(), 2, null(), &mut data) != 0,
            ERROR_INVALID_PARAMETER,
        )?;
        data.size = 0;
        c.fails(
            FindActCtxSectionStringW(0, null(), 2, dll.as_ptr(), &mut data) != 0,
            ERROR_INVALID_PARAMETER,
        )?;
    }
    Ok(())
}

// ---- ntdll: strings ---------------------------------------------------------------

fn rtl_strings(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid arguments throughout.
    unsafe {
        let mut buf: [u16; 36];
        let cases: [(u32, u32, &str); 6] = [
            (10, 123, "123"),
            (0, 4_294_967_295, "4294967295"),
            (2, 10, "1010"),
            (8, 2_147_483_647, "17777777777"),
            (16, 0xffff_fffe, "FFFFFFFE"),
            (16, 0, "0"),
        ];
        for (base, value, text) in cases {
            buf = [u16::from(b'-'); 36];
            let mut us = UnicodeString { length: 0, maximum: 70, buffer: buf.as_mut_ptr() };
            c.eq(RtlIntegerToUnicodeString(value, base, &mut us).into(), 0)?;
            c.eq(us.length.into(), 2 * text.len() as u64)?;
            c.eq(us.maximum.into(), 70)?;
            let want: [u16; 36] = w(text);
            c.ok(
                buf[..text.len()] == want[..text.len()] && buf[text.len()] == 0,
                us.length.into(),
            )?;
        }
        // Room for the digits but not their NUL: too small (Windows,
        // measured by CI's windows job: STATUS_BUFFER_OVERFLOW), untouched.
        buf = [u16::from(b'-'); 36];
        let mut us = UnicodeString { length: 0, maximum: 2, buffer: buf.as_mut_ptr() };
        c.eq(RtlIntegerToUnicodeString(0xa, 16, &mut us).into(), STATUS_BUFFER_OVERFLOW.into())?;
        c.ok(buf[0] == u16::from(b'-'), buf[0].into())?;
        // Room for the NUL too: written.
        let mut us = UnicodeString { length: 0, maximum: 4, buffer: buf.as_mut_ptr() };
        c.eq(RtlIntegerToUnicodeString(0xa, 16, &mut us).into(), 0)?;
        c.ok(buf[0] == u16::from(b'A') && buf[1] == 0, buf[1].into())?;
        // Too small: the buffer is untouched.
        buf = [u16::from(b'-'); 36];
        let mut us = UnicodeString { length: 0, maximum: 14, buffer: buf.as_mut_ptr() };
        c.eq(
            RtlIntegerToUnicodeString(0xffff_ffff, 16, &mut us).into(),
            STATUS_BUFFER_OVERFLOW.into(),
        )?;
        c.ok(buf.iter().all(|&ch| ch == u16::from(b'-')), 0)?;
        let mut us = UnicodeString { length: 0, maximum: 18, buffer: buf.as_mut_ptr() };
        c.eq(
            RtlIntegerToUnicodeString(0xdead_beef, 20, &mut us).into(),
            STATUS_INVALID_PARAMETER.into(),
        )?;

        // RtlIsNameInExpression.
        let cases: [(&str, &str, u8, u8); 9] = [
            ("*.TXT", "FOO.TXT", 0, 1),
            ("*.TXT", "foo.txt", 1, 1),
            ("*.TXT", "foo.txt", 0, 0),
            ("F?O", "FOO", 0, 1),
            ("F?O", "FO", 0, 0),
            ("*", "ANYTHING", 0, 1),
            ("A*B*C", "AXBYC", 0, 1),
            ("A*B*C", "AXBY", 0, 0),
            ("FOO", "FOOBAR", 0, 0),
        ];
        for (expression, name, ignore_case, want) in cases {
            let mut e: [u16; 16] = w(expression);
            let mut n: [u16; 16] = w(name);
            let es = UnicodeString {
                length: 2 * expression.len() as u16,
                maximum: 32,
                buffer: e.as_mut_ptr(),
            };
            let ns = UnicodeString {
                length: 2 * name.len() as u16,
                maximum: 32,
                buffer: n.as_mut_ptr(),
            };
            c.eq(RtlIsNameInExpression(&es, &ns, ignore_case, null()).into(), want.into())?;
        }
    }
    Ok(())
}

// ---- ntdll: AVL tables --------------------------------------------------------------

static TABLE_SEEN: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED: AtomicU32 = AtomicU32::new(0);
static FREED: AtomicU32 = AtomicU32::new(0);
/// The heap the callbacks allocate from: a bump area (freed blocks are
/// counted, not reused).
static mut ARENA: [u8; 4096] = [0; 4096];
static ARENA_AT: AtomicUsize = AtomicUsize::new(0);

extern "system" fn compare(table: *mut AvlTable, a: *const c_void, b: *const c_void) -> u32 {
    TABLE_SEEN.store(table as usize, Ordering::Relaxed);
    // SAFETY: the elements are u32s.
    let (a, b) = unsafe { (*a.cast::<u32>(), *b.cast::<u32>()) };
    match a.cmp(&b) {
        core::cmp::Ordering::Less => 0,    // GenericLessThan
        core::cmp::Ordering::Greater => 1, // GenericGreaterThan
        core::cmp::Ordering::Equal => 2,   // GenericEqual
    }
}

extern "system" fn allocate(_table: *mut AvlTable, size: u32) -> *mut c_void {
    ALLOCATED.fetch_add(1, Ordering::Relaxed);
    let size = (size as usize).div_ceil(16) * 16;
    let at = ARENA_AT.fetch_add(size, Ordering::Relaxed);
    if at + size > 4096 {
        return null_mut();
    }
    // SAFETY: inside the arena; 16-byte steps from its start.
    unsafe { (&raw mut ARENA).cast::<u8>().add(at).cast() }
}

extern "system" fn free(_table: *mut AvlTable, _p: *mut c_void) {
    FREED.fetch_add(1, Ordering::Relaxed);
}

fn avl(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid arguments throughout; the table outlives its use.
    unsafe {
        let mut table: AvlTable = core::mem::zeroed();
        let t = &raw mut table;
        RtlInitializeGenericTableAvl(t, compare, allocate, free, 0x1234);
        c.eq(table.context as u64, 0x1234)?;
        c.eq(RtlIsGenericTableEmptyAvl(t).into(), 1)?;
        c.eq(RtlNumberGenericTableElementsAvl(t).into(), 0)?;
        c.ok(RtlEnumerateGenericTableAvl(t, 1).is_null(), 0)?;
        let mut first = [null_mut(); 6];
        for (i, v) in [50u32, 30, 80, 10, 40, 90].iter().enumerate() {
            let mut new = 0u8;
            let p = RtlInsertElementGenericTableAvl(t, (v as *const u32).cast(), 4, &mut new);
            c.ok(!p.is_null() && new == 1, i as u64)?;
            c.eq((*p.cast::<u32>()).into(), (*v).into())?;
            first[i] = p;
        }
        c.eq(ALLOCATED.load(Ordering::Relaxed).into(), 6)?;
        c.eq(TABLE_SEEN.load(Ordering::Relaxed) as u64, t as u64)?;
        let mut new = 1u8;
        let dup = RtlInsertElementGenericTableAvl(t, (&40u32 as *const u32).cast(), 4, &mut new);
        c.ok(dup == first[4] && new == 0, new.into())?;
        c.eq(ALLOCATED.load(Ordering::Relaxed).into(), 6)?;
        c.eq(RtlNumberGenericTableElementsAvl(t).into(), 6)?;
        c.eq(table.number_elements.into(), 6)?;
        c.eq(RtlIsGenericTableEmptyAvl(t).into(), 0)?;
        c.ok(RtlLookupElementGenericTableAvl(t, (&80u32 as *const u32).cast()) == first[2], 0)?;
        c.ok(RtlLookupElementGenericTableAvl(t, (&81u32 as *const u32).cast()).is_null(), 0)?;
        // In order.
        let mut seen = [0u32; 8];
        let mut n = 0;
        let mut p = RtlEnumerateGenericTableAvl(t, 1);
        while !p.is_null() && n < 8 {
            seen[n] = *p.cast::<u32>();
            n += 1;
            p = RtlEnumerateGenericTableAvl(t, 0);
        }
        c.ok(n == 6 && seen[..6] == [10, 30, 40, 50, 80, 90], n as u64)?;
        let mut key = 0usize;
        let mut n = 0;
        let mut p = RtlEnumerateGenericTableWithoutSplayingAvl(t, &mut key);
        while !p.is_null() && n < 8 {
            seen[n] = *p.cast::<u32>();
            n += 1;
            p = RtlEnumerateGenericTableWithoutSplayingAvl(t, &mut key);
        }
        c.ok(n == 6 && seen[..6] == [10, 30, 40, 50, 80, 90], n as u64)?;
        c.eq((*RtlGetElementGenericTableAvl(t, 0).cast::<u32>()).into(), 10)?;
        c.eq((*RtlGetElementGenericTableAvl(t, 4).cast::<u32>()).into(), 80)?;
        c.ok(RtlGetElementGenericTableAvl(t, 6).is_null(), 0)?;
        // Delete.
        c.eq(RtlDeleteElementGenericTableAvl(t, (&50u32 as *const u32).cast()).into(), 1)?;
        c.eq(FREED.load(Ordering::Relaxed).into(), 1)?;
        c.eq(RtlDeleteElementGenericTableAvl(t, (&50u32 as *const u32).cast()).into(), 0)?;
        c.eq(RtlNumberGenericTableElementsAvl(t).into(), 5)?;
        c.eq((*RtlGetElementGenericTableAvl(t, 3).cast::<u32>()).into(), 80)?;
        for v in [10u32, 30, 40, 80, 90] {
            c.eq(RtlDeleteElementGenericTableAvl(t, (&v as *const u32).cast()).into(), 1)?;
        }
        c.eq(RtlIsGenericTableEmptyAvl(t).into(), 1)?;
        c.eq(FREED.load(Ordering::Relaxed).into(), 6)?;
    }
    Ok(())
}

/// GetThreadIOPendingFlag of `thread` (-1 if the call failed).
fn io_flag(thread: Handle) -> i64 {
    let mut pending = 7;
    // SAFETY: plain call.
    if unsafe { GetThreadIOPendingFlag(thread, &mut pending) } == 0 {
        return -1;
    }
    pending.into()
}

/// What a second thread sees: the main thread's flag and its own.
static SEEN: [AtomicUsize; 2] = [AtomicUsize::new(9), AtomicUsize::new(9)];

extern "system" fn see_flags(main: usize) -> u32 {
    SEEN[0].store(io_flag(main) as usize, Ordering::SeqCst);
    // SAFETY: plain call.
    SEEN[1].store(io_flag(unsafe { GetCurrentThread() }) as usize, Ordering::SeqCst);
    0
}

/// GetThreadIOPendingFlag while a pipe connect, a pipe read and a
/// directory watch are outstanding, and after they end.
fn io_pending(c: &mut Checks) -> Result<(), u32> {
    const ERROR_IO_PENDING: u32 = 997;
    const ERROR_OPERATION_ABORTED: u32 = 995;
    // SAFETY: valid arguments throughout; the OVERLAPPEDs and buffers
    // outlive their requests (each waited for).
    unsafe {
        let me = GetCurrentThread();
        c.eq(io_flag(me) as u64, 0)?;
        let name = w::<48>("\\\\.\\pipe\\gala_office_api_iopending");
        // PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED; byte mode, waiting.
        let pipe = CreateNamedPipeW(name.as_ptr(), 3 | 0x4000_0000, 0, 1, 512, 512, 0, 0);
        c.ok(pipe != INVALID, GetLastError().into())?;
        let e = CreateEventW(0, 1, 0, null());
        let mut ov = Overlapped::with(e);
        c.fails(ConnectNamedPipe(pipe, &mut ov) != 0, ERROR_IO_PENDING)?;
        c.eq(io_flag(me) as u64, 1)?;
        // Another thread sees it on this one, not on itself.
        let mut real = 0;
        c.ok(
            DuplicateHandle(GetCurrentProcess(), me, GetCurrentProcess(), &mut real, 0, 0, 2) != 0,
            0,
        )?;
        let t = CreateThread(0, 0, see_flags, real, 0, null_mut());
        c.ok(t != 0, 0)?;
        c.eq(WaitForSingleObject(t, 10_000).into(), 0)?;
        CloseHandle(t);
        CloseHandle(real);
        c.eq(SEEN[0].load(Ordering::SeqCst) as u64, 1)?;
        c.eq(SEEN[1].load(Ordering::SeqCst) as u64, 0)?;
        // A client connects: the request is done.
        let client = CreateFileW(name.as_ptr(), 0xc000_0000, 0, 0, 3, 0, 0);
        c.ok(client != INVALID, GetLastError().into())?;
        c.eq(WaitForSingleObject(e, 10_000).into(), 0)?;
        c.eq(io_flag(me) as u64, 0)?;
        // A read waiting for data, ended by CancelIo.
        let e2 = CreateEventW(0, 1, 0, null());
        let mut ov2 = Overlapped::with(e2);
        let mut buf = [0u8; 16];
        c.fails(
            ReadFile(pipe, buf.as_mut_ptr(), 16, null_mut(), (&raw mut ov2) as usize) != 0,
            ERROR_IO_PENDING,
        )?;
        c.eq(io_flag(me) as u64, 1)?;
        c.ok(CancelIo(pipe) != 0, 0)?;
        c.eq(WaitForSingleObject(e2, 10_000).into(), 0)?;
        let mut n = 0;
        c.fails(GetOverlappedResult(pipe, &mut ov2, &mut n, 0) != 0, ERROR_OPERATION_ABORTED)?;
        c.eq(io_flag(me) as u64, 0)?;
        // With a completion port (thread-agnostic I/O).
        let port = CreateIoCompletionPort(pipe, 0, 1, 0);
        c.ok(port != 0, 0)?;
        let e3 = CreateEventW(0, 1, 0, null());
        let mut ov3 = Overlapped::with(e3);
        c.fails(
            ReadFile(pipe, buf.as_mut_ptr(), 16, null_mut(), (&raw mut ov3) as usize) != 0,
            ERROR_IO_PENDING,
        )?;
        info(
            "GetThreadIOPendingFlag with a read on a pipe with a completion port",
            io_flag(me) as u64,
        );
        c.ok(CancelIo(pipe) != 0, 0)?;
        c.eq(WaitForSingleObject(e3, 10_000).into(), 0)?;
        CloseHandle(client);
        CloseHandle(pipe);
        CloseHandle(port);
        for h in [e, e2, e3] {
            CloseHandle(h);
        }

        // A directory watch, ended by CancelIoEx.
        let mut temp = [0u16; 260];
        let n = GetTempPathW(260, temp.as_mut_ptr()) as usize;
        let tail = w::<32>("gala_office_api_watch");
        let mut dir = [0u16; 300];
        dir[..n].copy_from_slice(&temp[..n]);
        dir[n..n + 21].copy_from_slice(&tail[..21]);
        CreateDirectoryW(dir.as_ptr(), 0);
        // FILE_LIST_DIRECTORY; BACKUP_SEMANTICS | OVERLAPPED.
        let h = CreateFileW(dir.as_ptr(), 1, 7, 0, 3, 0x0200_0000 | 0x4000_0000, 0);
        c.ok(h != INVALID, GetLastError().into())?;
        let e4 = CreateEventW(0, 1, 0, null());
        let mut ov4 = Overlapped::with(e4);
        let mut changes = [0u32; 256];
        c.ok(
            ReadDirectoryChangesW(h, changes.as_mut_ptr(), 1024, 0, 1, null_mut(), &mut ov4, 0)
                != 0,
            GetLastError().into(),
        )?;
        c.eq(io_flag(me) as u64, 1)?;
        c.ok(CancelIoEx(h, &mut ov4) != 0, GetLastError().into())?;
        c.eq(WaitForSingleObject(e4, 10_000).into(), 0)?;
        c.eq(io_flag(me) as u64, 0)?;
        CloseHandle(h);
        CloseHandle(e4);
        RemoveDirectoryW(dir.as_ptr());
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    files(&mut c)?;
    sync_and_processes(&mut c)?;
    io_pending(&mut c)?;
    actctx(&mut c)?;
    rtl_strings(&mut c)?;
    avl(&mut c)?;
    imm(&mut c)?;
    idn_and_power(&mut c)?;
    geo_and_lockdown(&mut c)?;
    url_zones(&mut c)?;
    adapters(&mut c)?;
    c2r_client(&mut c)?;
    c2r_oleaut(&mut c)?;
    #[cfg(target_arch = "x86_64")]
    c2r_d3dcompiler(&mut c)?;
    Ok(())
}

/// d3dcompiler_47.dll, which OfficeC2RClient.exe (64-bit) imports
/// (D3DCompile): D3DCreateBlob gives a zeroed blob of the size asked
/// (ID3DBlob: QueryInterface, AddRef, Release, GetBufferPointer,
/// GetBufferSize). D3DCompile of a one-line pixel shader: on Windows
/// S_OK with code; on gala, which has no HLSL compiler yet (a recorded
/// stopgap, ROADMAP GPU G2), a failure with no code and an error-message
/// blob, as a compile that cannot be done answers.
#[cfg(target_arch = "x86_64")]
fn c2r_d3dcompiler(c: &mut Checks) -> Result<(), u32> {
    type CreateBlob = extern "system" fn(usize, *mut usize) -> i32;
    type Compile = extern "system" fn(
        *const u8,
        usize,
        *const u8,
        usize,
        usize,
        *const u8,
        *const u8,
        u32,
        u32,
        *mut usize,
        *mut usize,
    ) -> i32;
    // SAFETY: the DLL's documented functions; our blobs.
    unsafe {
        let dll: [u16; 24] = w("d3dcompiler_47.dll");
        let m = LoadLibraryW(dll.as_ptr());
        c.ok(m != 0, u64::from(GetLastError()))?;
        let create = GetProcAddress(m, b"D3DCreateBlob\0".as_ptr());
        let compile = GetProcAddress(m, b"D3DCompile\0".as_ptr());
        c.ok(create != 0 && compile != 0, 0)?;
        let create: CreateBlob = core::mem::transmute(create);
        let compile: Compile = core::mem::transmute(compile);
        let slot = |obj: usize, i: usize| -> usize { *(*(obj as *const *const usize)).add(i) };
        let mut blob = 0usize;
        c.eq(create(16, &mut blob).into_u64(), 0)?;
        let size: extern "system" fn(usize) -> usize = core::mem::transmute(slot(blob, 4));
        let ptr: extern "system" fn(usize) -> *const u8 = core::mem::transmute(slot(blob, 3));
        c.eq(size(blob) as u64, 16)?;
        let p = ptr(blob);
        c.ok(!p.is_null() && core::slice::from_raw_parts(p, 16).iter().all(|&b| b == 0), 0)?;
        let release: extern "system" fn(usize) -> u32 = core::mem::transmute(slot(blob, 2));
        c.eq(release(blob).into(), 0)?;
        let src = b"float4 main() : SV_Target { return float4(1, 0, 0, 1); }";
        let (mut code, mut errors) = (0usize, 0usize);
        let hr = compile(
            src.as_ptr(),
            src.len(),
            null(),
            0,
            0,
            b"main\0".as_ptr(),
            b"ps_4_0\0".as_ptr(),
            0,
            0,
            &mut code,
            &mut errors,
        );
        if ntrt::on_gala() {
            c.ok(hr < 0 && code == 0 && errors != 0, hr.into_u64())?;
            c.ok(size(errors) > 1, 0)?;
            release(errors);
        } else {
            c.eq(hr.into_u64(), 0)?;
            c.ok(code != 0 && size(code) > 0, 0)?;
            release(code);
            if errors != 0 {
                release(errors);
            }
        }
    }
    Ok(())
}

/// DECIMAL (wtypes.h).
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Debug)]
struct Decimal {
    reserved: u16,
    scale: u8,
    sign: u8,
    hi: u32,
    lo: u64,
}

fn dec(sign: u8, scale: u8, lo: u64) -> Decimal {
    Decimal { reserved: 0, scale, sign, hi: 0, lo }
}

/// FONTDESC (olectl.h).
#[repr(C)]
struct FontDesc {
    size: u32,
    name: *const u16,
    cy: i64,
    weight: i16,
    charset: i16,
    italic: i32,
    underline: i32,
    strikethrough: i32,
}

/// SAFEARRAYBOUND.
#[repr(C)]
#[derive(Clone, Copy)]
struct SafeArrayBound {
    count: u32,
    lower: i32,
}

// oleaut32 by ordinal, as OfficeC2RClient.exe imports it.
#[cfg_attr(
    target_arch = "x86",
    link(name = "oleaut32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "oleaut32", kind = "raw-dylib"))]
unsafe extern "system" {
    #[link_ordinal(40)]
    fn SafeArrayRedim(sa: usize, bound: *const SafeArrayBound) -> i32;
    #[link_ordinal(177)]
    fn VarDecAdd(a: *const Decimal, b: *const Decimal, out: *mut Decimal) -> i32;
    #[link_ordinal(178)]
    fn VarDecDiv(a: *const Decimal, b: *const Decimal, out: *mut Decimal) -> i32;
    #[link_ordinal(179)]
    fn VarDecMul(a: *const Decimal, b: *const Decimal, out: *mut Decimal) -> i32;
    #[link_ordinal(181)]
    fn VarDecSub(a: *const Decimal, b: *const Decimal, out: *mut Decimal) -> i32;
    #[link_ordinal(187)]
    fn VarDecFix(a: *const Decimal, out: *mut Decimal) -> i32;
    #[link_ordinal(188)]
    fn VarDecInt(a: *const Decimal, out: *mut Decimal) -> i32;
    #[link_ordinal(192)]
    fn VarDecFromI4(v: i32, out: *mut Decimal) -> i32;
    #[link_ordinal(194)]
    fn VarDecFromR8(v: f64, out: *mut Decimal) -> i32;
    #[link_ordinal(203)]
    fn VarDecRound(a: *const Decimal, places: i32, out: *mut Decimal) -> i32;
    #[link_ordinal(204)]
    fn VarDecCmp(a: *const Decimal, b: *const Decimal) -> i32;
    #[link_ordinal(220)]
    fn VarR8FromDec(a: *const Decimal, out: *mut f64) -> i32;
    #[link_ordinal(420)]
    fn OleCreateFontIndirect(desc: *const FontDesc, iid: *const [u8; 16], out: *mut usize) -> i32;
    #[link_ordinal(411)]
    fn SafeArrayCreateVector(vt: u16, lower: i32, count: u32) -> usize;
    #[link_ordinal(16)]
    fn SafeArrayDestroy(sa: usize) -> i32;
    #[link_ordinal(21)]
    fn SafeArrayLock(sa: usize) -> i32;
    #[link_ordinal(22)]
    fn SafeArrayUnlock(sa: usize) -> i32;
    #[link_ordinal(19)]
    fn SafeArrayGetUBound(sa: usize, dim: u32, out: *mut i32) -> i32;
    #[link_ordinal(20)]
    fn SafeArrayGetLBound(sa: usize, dim: u32, out: *mut i32) -> i32;
    #[link_ordinal(25)]
    fn SafeArrayGetElement(sa: usize, index: *const i32, out: *mut i32) -> i32;
    #[link_ordinal(26)]
    fn SafeArrayPutElement(sa: usize, index: *const i32, value: *const i32) -> i32;
    #[link_ordinal(6)]
    fn SysFreeString(s: *mut u16);
}

/// What OfficeC2RClient.exe imports from oleaut32 by ordinal: DECIMAL
/// arithmetic and conversions (Wine's vartest.c run on Windows),
/// SafeArrayRedim (safearray.c), OleCreateFontIndirect (olefont.c).
fn c2r_oleaut(c: &mut Checks) -> Result<(), u32> {
    const DISP_E_DIVBYZERO: i32 = 0x8002_0012u32 as i32;
    const DISP_E_ARRAYISLOCKED: i32 = 0x8002_000du32 as i32;
    const VT_I4: u16 = 3;
    let mut out = Decimal::default();
    let eq = |c: &mut Checks, hr: i32, got: Decimal, want: Decimal| -> Result<(), u32> {
        c.eq(hr.into_u64(), 0)?;
        c.ok(got == want, u64::from(got.scale) << 56 | u64::from(got.sign) << 48 | got.lo)
    };
    // SAFETY: our DECIMALs, arrays and objects throughout.
    unsafe {
        c.eq(VarDecFromI4(-5, &mut out).into_u64(), 0)?;
        eq(c, 0, out, dec(0x80, 0, 5))?;
        let r = VarDecAdd(&dec(0, 1, 15), &dec(0, 2, 225), &mut out);
        eq(c, r, out, dec(0, 2, 375))?; // 1.5 + 2.25 = 3.75
        let r = VarDecSub(&dec(0, 0, 1), &dec(0, 0, 3), &mut out);
        eq(c, r, out, dec(0x80, 0, 2))?; // 1 - 3 = -2
        let r = VarDecMul(&dec(0, 1, 15), &dec(0, 0, 2), &mut out);
        eq(c, r, out, dec(0, 1, 30))?; // 1.5 * 2 = 3.0
        let r = VarDecDiv(&dec(0, 0, 1), &dec(0, 0, 4), &mut out);
        eq(c, r, out, dec(0, 2, 25))?; // 1 / 4 = 0.25
        let r = VarDecDiv(&dec(0, 0, 1), &dec(0, 0, 3), &mut out);
        eq(c, r, out, Decimal { scale: 28, hi: 0x0ac5_44ca, lo: 0x14b7_00cb_0555_5555, ..out })?;
        c.eq(
            VarDecDiv(&dec(0, 0, 1), &dec(0, 0, 0), &mut out).into_u64(),
            DISP_E_DIVBYZERO.into_u64(),
        )?;
        let r = VarDecInt(&dec(0x80, 1, 15), &mut out);
        eq(c, r, out, dec(0x80, 0, 2))?; // Int(-1.5) = -2
        let r = VarDecFix(&dec(0x80, 1, 15), &mut out);
        eq(c, r, out, dec(0x80, 0, 1))?; // Fix(-1.5) = -1
        let r = VarDecRound(&dec(0, 3, 1236), 2, &mut out);
        eq(c, r, out, dec(0, 2, 124))?;
        let r = VarDecRound(&dec(0, 4, 12345), 2, &mut out);
        eq(c, r, out, dec(0, 2, 123))?;
        c.eq(VarDecCmp(&dec(0, 1, 15), &dec(0, 2, 150)).into_u64(), 1)?; // VARCMP_EQ
        c.eq(VarDecCmp(&dec(0x80, 0, 1), &dec(0, 0, 1)).into_u64(), 0)?; // VARCMP_LT
        c.eq(VarDecCmp(&dec(0, 0, 2), &dec(0, 0, 1)).into_u64(), 2)?; // VARCMP_GT
        let r = VarDecFromR8(0.1, &mut out);
        eq(c, r, out, dec(0, 1, 1))?;
        let r = VarDecFromR8(-1234.5678, &mut out);
        eq(c, r, out, dec(0x80, 4, 12_345_678))?;
        let mut d = 0.0f64;
        c.eq(VarR8FromDec(&dec(0, 4, 12_345_678), &mut d).into_u64(), 0)?;
        c.ok(d == 1234.5678, d.to_bits())?;

        // SafeArrayRedim: longer (the lower bound too), shorter, locked.
        let sa = SafeArrayCreateVector(VT_I4, 0, 3);
        c.ok(sa != 0, 0)?;
        for i in 0..3 {
            let v = 10 + i;
            c.eq(SafeArrayPutElement(sa, &i, &v).into_u64(), 0)?;
        }
        c.eq(SafeArrayRedim(sa, &SafeArrayBound { count: 5, lower: 1 }).into_u64(), 0)?;
        let (mut lo, mut hi) = (0, 0);
        SafeArrayGetLBound(sa, 1, &mut lo);
        SafeArrayGetUBound(sa, 1, &mut hi);
        c.eq((lo as u64) << 32 | hi as u64, 1 << 32 | 5)?;
        for (i, want) in [(1, 10), (3, 12), (5, 0)] {
            let mut v = -1;
            c.eq(SafeArrayGetElement(sa, &i, &mut v).into_u64(), 0)?;
            c.eq(v as u64, want)?;
        }
        c.eq(SafeArrayRedim(sa, &SafeArrayBound { count: 2, lower: 1 }).into_u64(), 0)?;
        SafeArrayGetUBound(sa, 1, &mut hi);
        c.eq(hi as u64, 2)?;
        SafeArrayLock(sa);
        let locked = SafeArrayRedim(sa, &SafeArrayBound { count: 4, lower: 0 });
        c.eq(locked.into_u64(), DISP_E_ARRAYISLOCKED.into_u64())?;
        SafeArrayUnlock(sa);
        c.eq(SafeArrayDestroy(sa).into_u64(), 0)?;

        // OleCreateFontIndirect: IFont's properties and its HFONT.
        const IID_IFONT: [u8; 16] = [
            0x02, 0xe0, 0xf6, 0xbe, 0x74, 0xa8, 0x1a, 0x10, 0x8b, 0xba, 0x00, 0xaa, 0x00, 0x30,
            0x0c, 0xab,
        ];
        let face: [u16; 8] = w("Arial");
        let desc = FontDesc {
            size: size_of::<FontDesc>() as u32,
            name: face.as_ptr(),
            cy: 180_000, // 18 points
            weight: 700,
            charset: 0,
            italic: 1,
            underline: 0,
            strikethrough: 0,
        };
        let mut font = 0usize;
        c.eq(OleCreateFontIndirect(&desc, &IID_IFONT, &mut font).into_u64(), 0)?;
        c.ok(font != 0, 0)?;
        let call = |slot: usize| -> usize { *(*(font as *const *const usize)).add(slot) };
        let mut name: *mut u16 = null_mut();
        let get_name: extern "system" fn(usize, *mut *mut u16) -> i32 =
            core::mem::transmute(call(3));
        c.eq(get_name(font, &mut name).into_u64(), 0)?;
        c.ok(!name.is_null() && units(name) == &face[..5], 0)?;
        SysFreeString(name);
        let mut cy = 0i64;
        let get_size: extern "system" fn(usize, *mut i64) -> i32 = core::mem::transmute(call(5));
        c.eq(get_size(font, &mut cy).into_u64(), 0)?;
        c.eq(cy as u64, 180_000)?;
        let get_flag = |slot: usize, out: &mut i32| -> i32 {
            let f: extern "system" fn(usize, *mut i32) -> i32 = core::mem::transmute(call(slot));
            f(font, out)
        };
        let mut v = 7;
        c.eq(get_flag(7, &mut v).into_u64(), 0)?; // get_Bold
        c.eq(v as u64, 1)?;
        c.eq(get_flag(9, &mut v).into_u64(), 0)?; // get_Italic
        c.eq(v as u64, 1)?;
        c.eq(get_flag(11, &mut v).into_u64(), 0)?; // get_Underline
        c.eq(v as u64, 0)?;
        let mut weight = 0i16;
        let get_weight: extern "system" fn(usize, *mut i16) -> i32 = core::mem::transmute(call(15));
        c.eq(get_weight(font, &mut weight).into_u64(), 0)?;
        c.eq(weight as u64, 700)?;
        let put_bold: extern "system" fn(usize, i32) -> i32 = core::mem::transmute(call(8));
        c.eq(put_bold(font, 0).into_u64(), 0)?;
        get_weight(font, &mut weight);
        c.eq(weight as u64, 400)?;
        let mut hfont = 0usize;
        let get_hfont: extern "system" fn(usize, *mut usize) -> i32 =
            core::mem::transmute(call(19));
        c.eq(get_hfont(font, &mut hfont).into_u64(), 0)?;
        c.ok(hfont != 0, 0)?;
        // The HFONT: Arial, 400, italic, 18 points at the screen's LOGPIXELSY.
        let gdi: [u16; 12] = w("gdi32.dll");
        let user: [u16; 12] = w("user32.dll");
        let (g, u) = (LoadLibraryW(gdi.as_ptr()), LoadLibraryW(user.as_ptr()));
        let get_object: extern "system" fn(usize, i32, *mut u8) -> i32 =
            core::mem::transmute(GetProcAddress(g, b"GetObjectW\0".as_ptr()));
        let caps: extern "system" fn(usize, i32) -> i32 =
            core::mem::transmute(GetProcAddress(g, b"GetDeviceCaps\0".as_ptr()));
        let get_dc: extern "system" fn(usize) -> usize =
            core::mem::transmute(GetProcAddress(u, b"GetDC\0".as_ptr()));
        let release_dc: extern "system" fn(usize, usize) -> i32 =
            core::mem::transmute(GetProcAddress(u, b"ReleaseDC\0".as_ptr()));
        let dc = get_dc(0);
        let dpi = caps(dc, 90);
        release_dc(0, dc);
        let mut lf = [0u8; 92];
        c.eq(get_object(hfont, 92, lf.as_mut_ptr()) as u64, 92)?;
        let height = i32::from_le_bytes([lf[0], lf[1], lf[2], lf[3]]);
        c.eq(height as u64, (-(18 * dpi + 36) / 72) as u64)?;
        c.eq(u64::from(u32::from_le_bytes([lf[16], lf[17], lf[18], lf[19]])), 400)?;
        c.eq(lf[20].into(), 1)?;
        c.ok(lf[28..38] == [65, 0, 114, 0, 105, 0, 97, 0, 108, 0], lf[28].into())?;
        let release: extern "system" fn(usize) -> u32 = core::mem::transmute(call(2));
        c.eq(release(font).into(), 0)?;
    }
    Ok(())
}

/// What OfficeC2RClient.exe (Click-to-Run's React Native UI) needs of
/// kernel32 to start: the local atom table (AddAtomW, FindAtomW,
/// GetAtomNameW/A, DeleteAtom; expectations from Wine's kernel32 atom.c
/// run on Windows), and the app-model API set its C++ runtime loads for
/// the app policies (`api-ms-win-appmodel-runtime-l1-1-2.dll`: a desktop
/// program ends with ExitProcess, initializes no Windows Runtime, shows
/// developer diagnostics, is a classic desktop window model, and has no
/// package: APPMODEL_ERROR_NO_PACKAGE).
fn c2r_client(c: &mut Checks) -> Result<(), u32> {
    const ERROR_FILE_NOT_FOUND: u32 = 2;
    const APPMODEL_ERROR_NO_PACKAGE: u32 = 15700;
    print(NAME.as_bytes());
    print(b": c2r client\r\n");
    // SAFETY: our strings and buffers with their sizes.
    unsafe {
        c.eq(InitAtomTable(37).into_u64(), 1)?;
        let name: [u16; 16] = w("gala_c2r_atom");
        let atom = AddAtomW(name.as_ptr());
        c.ok(atom >= 0xc000, atom.into())?;
        c.eq(AddAtomA(b"GALA_C2R_ATOM\0".as_ptr()).into(), atom.into())?;
        let other: [u16; 16] = w("Gala_C2R_Atom");
        c.eq(FindAtomW(other.as_ptr()).into(), atom.into())?;
        c.eq(FindAtomA(b"gala_c2r_ATOM\0".as_ptr()).into(), atom.into())?;
        // The local table is not the global one.
        SetLastError(0xdead_beef);
        let global = GlobalFindAtomW(name.as_ptr());
        c.fails(global != 0, ERROR_FILE_NOT_FOUND)?;
        // The name as first added, as much as fits with its NUL.
        let mut buf = [0x2e2eu16; 16];
        c.eq(GetAtomNameW(atom, buf.as_mut_ptr(), 16).into(), 13)?;
        c.ok(buf[..14] == name[..14], u64::from(buf[0]))?;
        let mut short = [0x2e2eu16; 8];
        c.eq(GetAtomNameW(atom, short.as_mut_ptr(), 6).into(), 5)?;
        c.ok(
            short[..6] == [0x67, 0x61, 0x6c, 0x61, 0x5f, 0] && short[6] == 0x2e2e,
            short[5].into(),
        )?;
        let mut a = [b'.'; 16];
        c.eq(GetAtomNameA(atom, a.as_mut_ptr(), 16).into(), 13)?;
        c.ok(&a[..14] == b"gala_c2r_atom\0", a[0].into())?;
        let mut a = [b'.'; 8];
        c.eq(GetAtomNameA(atom, a.as_mut_ptr(), 6).into(), 5)?;
        c.ok(&a[..7] == b"gala_\0.", a[5].into())?;
        // Integer atoms: "#1234" and MAKEINTATOM are their own values.
        let hash: [u16; 8] = w("#1234");
        c.eq(AddAtomW(hash.as_ptr()).into(), 1234)?;
        c.eq(AddAtomW(0x1234 as *const u16).into(), 0x1234)?;
        let mut n = [0u16; 8];
        c.eq(GetAtomNameW(1234, n.as_mut_ptr(), 8).into(), 5)?;
        c.ok(n[..6] == hash[..6], n[0].into())?;
        // Counted: two adds, two deletes, then gone.
        c.eq(DeleteAtom(atom).into(), 0)?;
        c.eq(FindAtomW(name.as_ptr()).into(), atom.into())?;
        c.eq(DeleteAtom(atom).into(), 0)?;
        SetLastError(0xdead_beef);
        let gone = FindAtomW(name.as_ptr());
        c.fails(gone != 0, ERROR_FILE_NOT_FOUND)?;

        // The app-model API set and its policies.
        let set: [u16; 48] = w("api-ms-win-appmodel-runtime-l1-1-2.dll");
        let m = LoadLibraryW(set.as_ptr());
        c.ok(m != 0, u64::from(GetLastError()))?;
        let token = -6isize as Handle; // GetCurrentThreadEffectiveToken
        for (name, want) in [
            (&b"AppPolicyGetProcessTerminationMethod\0"[..], 0u32), // ExitProcess
            (b"AppPolicyGetThreadInitializationType\0", 0),         // None
            (b"AppPolicyGetShowDeveloperDiagnostic\0", 1),          // ShowUI
            (b"AppPolicyGetWindowingModel\0", 2),                   // ClassicDesktop
        ] {
            let f = GetProcAddress(m, name.as_ptr());
            c.ok(f != 0, 0)?;
            let f: extern "system" fn(Handle, *mut u32) -> i32 = core::mem::transmute(f);
            let mut policy = 0xdead_beef;
            c.eq(f(token, &mut policy).into_u64(), 0)?;
            c.eq(policy.into(), want.into())?;
        }
        let f = GetProcAddress(m, b"GetCurrentPackageFullName\0".as_ptr());
        c.ok(f != 0, 0)?;
        let f: extern "system" fn(*mut u32, *mut u16) -> i32 = core::mem::transmute(f);
        let mut len = 0u32;
        c.eq(f(&mut len, null_mut()).into_u64(), APPMODEL_ERROR_NO_PACKAGE.into())?;
    }
    Ok(())
}

/// GetAdaptersInfo, as the module's comment says.
fn adapters(c: &mut Checks) -> Result<(), u32> {
    const ERROR_BUFFER_OVERFLOW: u32 = 111;
    const ERROR_NO_DATA: u32 = 232;
    const MIB_IF_TYPE_LOOPBACK: u32 = 24;
    static mut BUF: [u64; 1024] = [0; 1024];
    let buf = (&raw mut BUF).cast::<IpAdapterInfo>();
    let end = buf as usize + size_of::<[u64; 1024]>();
    // SAFETY: our buffer with its size; the adapters inside it.
    unsafe {
        c.eq(GetAdaptersInfo(null_mut(), null_mut()).into(), u64::from(ERROR_INVALID_PARAMETER))?;
        let mut len = 0;
        let r = GetAdaptersInfo(null_mut(), &mut len);
        if r == ERROR_NO_DATA {
            return Ok(());
        }
        c.eq(r.into(), u64::from(ERROR_BUFFER_OVERFLOW))?;
        c.ok(
            len as usize >= size_of::<IpAdapterInfo>() && len as usize <= end - buf as usize,
            len.into(),
        )?;
        let mut short = len - 1;
        c.eq(GetAdaptersInfo(buf, &mut short).into(), u64::from(ERROR_BUFFER_OVERFLOW))?;
        c.eq(short.into(), len.into())?;
        c.eq(GetAdaptersInfo(buf, &mut len).into(), 0)?;
        let text = |s: &[u8]| s[0] != 0 && s.contains(&0);
        let mut a = buf;
        let mut count = 0;
        while !a.is_null() {
            count += 1;
            c.ok(
                a as usize >= buf as usize && (a as usize) < buf as usize + len as usize,
                a as u64,
            )?;
            let i = &*a;
            c.ok(i.if_type != MIB_IF_TYPE_LOOPBACK, i.if_type.into())?;
            let name = &i.adapter_name;
            c.ok(name[0] == b'{' && name[37] == b'}' && name[38] == 0, u64::from(name[0]))?;
            c.ok(i.address_length <= 8, i.address_length.into())?;
            c.ok(text(&i.ip_address_list.address) && text(&i.ip_address_list.mask), 0)?;
            c.ok(text(&i.gateway_list.address) && text(&i.gateway_list.mask), 0)?;
            a = i.next;
        }
        // The size needed is the adapters' structures: 704 bytes each in
        // a 64-bit process, 640 in a 32-bit one (Windows 11 25H2, eight
        // adapters with one address each, issue #57).
        let unit = if cfg!(target_arch = "x86") { 640 } else { 704 };
        c.eq(len.into(), count * unit)?;
    }
    Ok(())
}

/// IInternetSecurityManager's methods (urlmon.idl's order after IUnknown):
/// SetSecuritySite, GetSecuritySite, MapUrlToZone, GetSecurityId,
/// ProcessUrlAction, ...
type MapUrlToZone = extern "system" fn(usize, *const u16, *mut u32, u32) -> i32;
type GetSecurityId = extern "system" fn(usize, *const u16, *mut u8, *mut u32, usize) -> i32;
type ProcessUrlAction =
    extern "system" fn(usize, *const u16, u32, *mut u8, u32, *mut u8, u32, u32, u32) -> i32;
type Release = extern "system" fn(usize) -> u32;

/// Method `slot` of COM object `object`.
fn method(object: usize, slot: usize) -> usize {
    // SAFETY: a COM object's vtable, which has the slot.
    unsafe { *(*(object as *const *const usize)).add(slot) }
}

/// urlmon's security manager, which setup.exe creates on its download's
/// error path: the zones and security IDs of the default settings (no
/// zone mappings), as Wine's urlmon tests (sec_mgr.c
/// test_SecurityManager's table) find them on Windows.
fn url_zones(c: &mut Checks) -> Result<(), u32> {
    const E_INVALIDARG: u32 = 0x8007_0057;
    const URLACTION_SCRIPT_RUN: u32 = 0x1400;
    // (URL, zone, security ID without its zone DWORD).
    // (res://mshtml.dll/blank.htm, Wine's first row, is not here: Windows
    // Server 2025, which has no MSHTML, answers it as the Internet zone.)
    let table: [(&str, u32, &[u8]); 12] = [
        ("file://c:\\Index.htm", 0, b"file:"),
        ("http://www.zone3.winetest", 3, b"http:www.zone3.winetest"),
        ("about:blank", 3, b"about:blank"),
        ("ftp://zone3.winetest/file.test", 3, b"ftp:zone3.winetest"),
        ("file://c:/Index.htm", 0, b"file:"),
        ("file:///c:/Index.htm", 0, b"file:"),
        ("file:///c:", 0, b"file:"),
        ("file:///c:c\\", 0, b"file:"),
        ("c:\\test.htm", 0, b"file:"),
        ("file://some%20file.jpg", 3, b"file:some%20file.jpg"),
        ("zip:testing.com/testing", 3, b"zip:testing.com/testing"),
        ("http://www.zone3.winetest/site/about", 3, b"http:www.zone3.winetest"),
    ];
    let mut manager = 0usize;
    // SAFETY: our output.
    c.eq(unsafe { CoInternetCreateSecurityManager(0, &mut manager, 0) }.into_u64(), 0)?;
    c.ok(manager != 0, 0)?;
    // SAFETY: the manager's methods, by IInternetSecurityManager's layout.
    let (map, id, action, release) = unsafe {
        (
            core::mem::transmute::<usize, MapUrlToZone>(method(manager, 5)),
            core::mem::transmute::<usize, GetSecurityId>(method(manager, 6)),
            core::mem::transmute::<usize, ProcessUrlAction>(method(manager, 7)),
            core::mem::transmute::<usize, Release>(method(manager, 2)),
        )
    };
    for (row, (url, zone, secid)) in table.into_iter().enumerate() {
        let wide: [u16; 64] = w(url);
        let mut got = 100u32;
        c.eq(map(manager, wide.as_ptr(), &mut got, 0).into_u64(), 0)?;
        // On failure: the row in the high byte, the zone got in the low.
        c.ok(got == zone, ((row as u64) << 8) | u64::from(got))?;
        let mut buf = [0xf0u8; 128];
        let mut size = buf.len() as u32;
        c.eq(id(manager, wide.as_ptr(), buf.as_mut_ptr(), &mut size, 0).into_u64(), 0)?;
        c.eq(size.into(), (secid.len() + 4) as u64)?;
        c.ok(buf[..secid.len()] == *secid, buf[0].into())?;
        c.ok(buf[secid.len()..secid.len() + 4] == zone.to_le_bytes(), buf[secid.len()].into())?;
    }
    // A relative URL: E_INVALIDARG (IE 7 and later), no zone.
    let relative: [u16; 16] = w("index.htm");
    let mut got = 100u32;
    let r = map(manager, relative.as_ptr(), &mut got, 0) as u32;
    c.eq(r.into(), E_INVALIDARG.into())?;
    c.ok(got == 100 || got == u32::MAX, got.into())?;
    let mut buf = [0u8; 128];
    let mut size = buf.len() as u32;
    let r = id(manager, relative.as_ptr(), buf.as_mut_ptr(), &mut size, 0) as u32;
    c.eq(r.into(), E_INVALIDARG.into())?;
    // http without "//": S_OK and URLZONE_INVALID; no security ID.
    let no_host: [u16; 16] = w("http:google.com");
    c.eq(map(manager, no_host.as_ptr(), &mut got, 0).into_u64(), 0)?;
    c.eq(got.into(), u32::MAX.into())?;
    let r = id(manager, no_host.as_ptr(), buf.as_mut_ptr(), &mut size, 0) as u32;
    c.eq(r.into(), E_INVALIDARG.into())?;
    // No URL, no buffer, no size: E_INVALIDARG.
    c.eq((map(manager, null(), &mut got, 0) as u32).into(), E_INVALIDARG.into())?;
    let r = id(manager, null(), buf.as_mut_ptr(), &mut size, 0) as u32;
    c.eq(r.into(), E_INVALIDARG.into())?;
    let r = id(manager, relative.as_ptr(), null_mut(), &mut size, 0) as u32;
    c.eq(r.into(), E_INVALIDARG.into())?;
    let r = id(manager, relative.as_ptr(), buf.as_mut_ptr(), null_mut(), 0) as u32;
    c.eq(r.into(), E_INVALIDARG.into())?;
    let mut policy = 0u32;
    let r = action(
        manager,
        null(),
        URLACTION_SCRIPT_RUN,
        (&raw mut policy).cast(),
        2,
        null_mut(),
        0,
        0,
        0,
    ) as u32;
    c.eq(r.into(), E_INVALIDARG.into())?;
    release(manager);
    Ok(())
}

/// GetUserDefaultGeoName and wldp's lockdown queries, which setup.exe
/// asks for after reading its configuration.
fn geo_and_lockdown(c: &mut Checks) -> Result<(), u32> {
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    const GEOCLASS_NATION: u32 = 16;
    const GEO_ISO2: u32 = 4;
    // SAFETY: our buffers with their sizes.
    unsafe {
        let need = GetUserDefaultGeoName(null_mut(), 0);
        // A two-letter ISO 3166 code, or a region without one as its
        // three-digit UN M.49 code ("001", the world), and the NUL.
        c.ok(need == 3 || need == 4, need.into_u64())?;
        let mut name = [0xffffu16; 8];
        c.eq(GetUserDefaultGeoName(name.as_mut_ptr(), need).into_u64(), need as u64)?;
        let len = need as usize - 1;
        c.eq(name[len].into(), 0)?;
        let first = u64::from(name[0]) << 16 | u64::from(name[1]);
        if len == 2 {
            let upper = |u: u16| (u16::from(b'A')..=u16::from(b'Z')).contains(&u);
            c.ok(upper(name[0]) && upper(name[1]), first)?;
            // The same region as GetUserGeoID's, named by GEO_ISO2.
            let mut iso2 = [0u16; 8];
            let geo = GetUserGeoID(GEOCLASS_NATION);
            c.eq(GetGeoInfoW(geo, GEO_ISO2, iso2.as_mut_ptr(), 8, 0).into_u64(), 3)?;
            c.ok(iso2[..3] == name[..3], u64::from(iso2[0]) << 16 | u64::from(iso2[1]))?;
        } else {
            let digit = |u: u16| (u16::from(b'0')..=u16::from(b'9')).contains(&u);
            c.ok(name[..3].iter().all(|&u| digit(u)), first)?;
        }
        let short = GetUserDefaultGeoName(name.as_mut_ptr(), need - 1);
        c.fails(short != 0, ERROR_INSUFFICIENT_BUFFER)?;

        let mut mode = 0xdead_beef;
        c.eq(WldpQueryWindowsLockdownMode(&mut mode).into_u64(), 0)?;
        c.eq(mode.into(), 0)?; // WLDP_WINDOWS_LOCKDOWN_MODE_UNLOCKED
        let host = WldpHostInformation {
            revision: 1, // WLDP_HOST_INFORMATION_REVISION
            host_id: 1,  // WLDP_HOST_ID_GLOBAL
            source: null(),
            handle: 0,
        };
        let mut state = 0xdead_beef;
        c.eq(WldpGetLockdownPolicy(&host, &mut state, 0).into_u64(), 0)?;
        c.eq(state.into(), 0x8000_0000)?; // WLDP_LOCKDOWN_DEFINED_FLAG
    }
    Ok(())
}

/// A signed result as the bits the failure line shows.
trait IntoU64 {
    fn into_u64(self) -> u64;
}

impl IntoU64 for i32 {
    fn into_u64(self) -> u64 {
        u64::from(self as u32)
    }
}

/// normaliz's IDN functions and powrprof's processor information, the
/// DLLs setup.exe reaches after imm32.
fn idn_and_power(c: &mut Checks) -> Result<(), u32> {
    let mut unicode: [u16; 6] = w("bucher");
    unicode[1] = 0xfc; // ü
    let ascii: [u16; 13] = w("xn--bcher-kva");
    let mut out = [0u16; 64];
    // SAFETY: our buffers with their sizes.
    unsafe {
        let n = IdnToAscii(0, unicode.as_ptr(), 6, out.as_mut_ptr(), 64);
        c.eq(n as u64, 13)?;
        c.ok(out[..13] == ascii, 0)?;
        let n = IdnToUnicode(0, ascii.as_ptr(), 13, out.as_mut_ptr(), 64);
        c.eq(n as u64, 6)?;
        c.ok(out[..6] == unicode, 0)?;

        // PROCESSOR_POWER_INFORMATION: six ULONGs per processor.
        const PROCESSOR_INFORMATION: u32 = 11;
        let mut info = [0u32; 6 * 256];
        c.eq(
            CallNtPowerInformation(PROCESSOR_INFORMATION, null(), 0, info.as_mut_ptr().cast(), 0)
                .into(),
            0xc000_000d,
        )?;
        // Room for one entry is too small where there are more processors.
        let mut system = [0u8; 64];
        GetSystemInfo(system.as_mut_ptr());
        let at = 8 + 3 * core::mem::size_of::<usize>();
        let cpus = u32::from_le_bytes([system[at], system[at + 1], system[at + 2], system[at + 3]]);
        if cpus > 1 {
            c.eq(
                CallNtPowerInformation(
                    PROCESSOR_INFORMATION,
                    null(),
                    0,
                    info.as_mut_ptr().cast(),
                    24,
                )
                .into(),
                0xc000_0023,
            )?;
        }
        let size = core::mem::size_of_val(&info) as u32;
        c.eq(
            CallNtPowerInformation(
                PROCESSOR_INFORMATION,
                null(),
                0,
                info.as_mut_ptr().cast(),
                size,
            )
            .into(),
            0,
        )?;
        c.eq(info[0].into(), 0)?;
    }
    Ok(())
}

/// imm32's ImmDisableIME, which ODT's setup.exe calls before its first
/// window (its documentation: 0 is the calling thread, -1 every thread of
/// the process; TRUE on success).
fn imm(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain calls.
    unsafe {
        c.ok(ImmDisableIME(0) != 0, 0)?;
        c.ok(ImmDisableIME(u32::MAX) != 0, 0)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            Line::new().s(NAME.as_bytes()).s(b": ok\r\n").print();
            0
        }
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
