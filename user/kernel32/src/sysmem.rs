//! Memory, NUMA, psapi, process image, file information and version
//! functions. gala has one NUMA node with every processor, no large or
//! physical page allocation (both need privileges programs normally do not
//! have), and reports the process's memory use from its address space.

use super::*;

const ERROR_PRIVILEGE_NOT_HELD: u32 = 1314;
const ERROR_OLD_WIN_VERSION: u32 = 1150;
const ERROR_NO_MORE_ITEMS: u32 = 259;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_ACCESS_DENIED: u32 = 5;

/// (major, minor, build) as reported to this program (its compatibility
/// manifest decides; see `reported_version`).
fn kuser_version() -> (u32, u32, u32) {
    super::mapping::reported_version()
}

/// The x64 user address space: 0x10000 up to 0x7fff_fffe_ffff.
#[cfg(target_arch = "x86_64")]
fn user_space() -> u64 {
    0x7fff_fffe_0000
}

/// A 32-bit process's: 0x10000 up to its highest address (below 2 GiB, or
/// 4 GiB for a large-address-aware executable, as WOW64 gives it).
#[cfg(target_arch = "x86")]
fn user_space() -> u64 {
    (basic_information().maximum_address as u64 + 1).saturating_sub(0x1_0000)
}

/// MEMORY_BASIC_INFORMATION's fields this file reads.
struct Region {
    base: usize,
    allocation_base: usize,
    size: usize,
    kind: u32,
}

const MEM_IMAGE: u32 = 0x100_0000;

/// The regions of this process's address space, in address order (a
/// system call per region: module enumeration only; sizes come from the
/// kernel's counters, `vm_counters`).
fn regions(mut f: impl FnMut(&Region) -> bool) {
    let mut address = 0x1_0000usize;
    let end = user_space();
    while (address as u64) < end {
        let mut mbi = MemoryBasic::default();
        if VirtualQuery(address as *const c_void, (&raw mut mbi).cast(), size_of::<MemoryBasic>())
            == 0
        {
            return;
        }
        let r = Region {
            base: mbi.base,
            allocation_base: mbi.allocation_base,
            size: mbi.region_size,
            kind: mbi.kind,
        };
        if r.size == 0 || !f(&r) {
            return;
        }
        address = r.base + r.size;
    }
}

/// VM_COUNTERS (ProcessVmCounters), as SIZE_Ts: PeakVirtualSize (0),
/// VirtualSize (1), PageFaultCount (2: a ULONG, padded on 64-bit),
/// PeakWorkingSetSize (3), WorkingSetSize (4), the four pool quota
/// figures (5-8), PagefileUsage (9), PeakPagefileUsage (10).
fn vm_counters(process: Handle) -> Result<[usize; 11], u32> {
    let mut vm = [0usize; 11];
    // SAFETY: a VM_COUNTERS_EX buffer.
    let status = unsafe {
        NtQueryInformationProcess(
            process as isize,
            3,
            vm.as_mut_ptr().cast(),
            size_of_val(&vm) as u32,
            core::ptr::null_mut(),
        )
    };
    if status != STATUS_SUCCESS { Err(status) } else { Ok(vm) }
}

/// MEMORYSTATUSEX (dwLength must be 64).
#[repr(C)]
pub struct MemoryStatusEx {
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

/// Physical pages: (total, available), from SystemBasicInformation and
/// SystemPerformanceInformation.
/// Physical pages (total, available) and the commit charge (committed,
/// limit), from SYSTEM_BASIC_INFORMATION and SYSTEM_PERFORMANCE_INFORMATION.
fn memory_pages() -> (u64, u64, u64, u64) {
    let physical = u64::from(basic_information().physical_pages);
    let mut perf = [0u8; 0x158];
    let mut ret = 0;
    // SAFETY: a buffer of the class's size.
    unsafe { NtQuerySystemInformation(2, perf.as_mut_ptr(), 0x158, &mut ret) };
    let at =
        |b: &[u8], o: usize| u64::from(u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]));
    (physical, at(&perf, 0x2c), at(&perf, 0x30), at(&perf, 0x34))
}

/// GlobalMemoryStatusEx: physical memory from the kernel; the "page file"
/// figures are the commit limit and what is left of it (there is no page
/// file: the limit is physical memory); the user address space.
#[unsafe(no_mangle)]
pub extern "system" fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> Bool {
    // SAFETY: the caller's MEMORYSTATUSEX.
    let s = unsafe { &mut *status };
    if s.length != 64 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let (total, avail, committed, limit) = memory_pages();
    let (total, avail) = (total * 4096, (avail * 4096).min(total * 4096));
    // VirtualSize: what the process has reserved (one query; walking the
    // address space cost a system call per region, and programs call this
    // often: Chromium's memory checks on every main thread).
    let reserved = vm_counters(GetCurrentProcess()).map_or(0, |vm| vm[1] as u64);
    s.load = (avail * 100).checked_div(total).map_or(0, |free| (100 - free) as u32);
    s.total_phys = total;
    s.avail_phys = avail;
    s.total_page_file = limit * 4096;
    s.avail_page_file = limit.saturating_sub(committed) * 4096;
    s.total_virtual = user_space();
    s.avail_virtual = s.total_virtual.saturating_sub(reserved);
    s.avail_extended_virtual = 0;
    TRUE
}

/// GetLargePageMinimum: 2 MiB (large pages themselves need
/// SeLockMemoryPrivilege, which processes do not hold here).
#[unsafe(no_mangle)]
pub extern "system" fn GetLargePageMinimum() -> usize {
    0x20_0000
}

#[unsafe(no_mangle)]
pub extern "system" fn GetNumaHighestNodeNumber(node: *mut u32) -> Bool {
    // SAFETY: the caller's output.
    unsafe { *node = 0 };
    TRUE
}

/// GetNumaNodeProcessorMask: node 0 holds every processor.
#[unsafe(no_mangle)]
pub extern "system" fn GetNumaNodeProcessorMask(node: u8, mask: *mut u64) -> Bool {
    if node != 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let (mut process, mut system) = (0usize, 0usize);
    if GetProcessAffinityMask(GetCurrentProcess(), &mut process, &mut system) == FALSE {
        return FALSE;
    }
    // SAFETY: the caller's output.
    unsafe { *mask = system as u64 };
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn GetNumaProcessorNode(_processor: u8, node: *mut u8) -> Bool {
    // SAFETY: the caller's output.
    unsafe { *node = 0 };
    TRUE
}

/// VirtualAllocExNuma: VirtualAllocEx (one node).
#[unsafe(no_mangle)]
pub extern "system" fn VirtualAllocExNuma(
    process: Handle,
    address: *mut c_void,
    size: usize,
    kind: u32,
    protect: u32,
    _node: u32,
) -> *mut c_void {
    if !is_current(process) {
        unsupported(b"kernel32.dll!VirtualAllocExNuma(another process)", 0);
        return fail_with(ERROR_ACCESS_DENIED, core::ptr::null_mut());
    }
    VirtualAlloc(address, size, kind, protect)
}

/// AllocateUserPhysicalPages, FreeUserPhysicalPages, MapUserPhysicalPages:
/// AWE needs SeLockMemoryPrivilege (ERROR_PRIVILEGE_NOT_HELD).
#[unsafe(no_mangle)]
pub extern "system" fn AllocateUserPhysicalPages(
    _p: Handle,
    _n: *mut usize,
    _a: *mut usize,
) -> Bool {
    fail_with(ERROR_PRIVILEGE_NOT_HELD, FALSE)
}

#[unsafe(no_mangle)]
pub extern "system" fn FreeUserPhysicalPages(_p: Handle, _n: *mut usize, _a: *mut usize) -> Bool {
    fail_with(ERROR_INVALID_PARAMETER, FALSE)
}

#[unsafe(no_mangle)]
pub extern "system" fn MapUserPhysicalPages(_a: *mut c_void, _n: usize, _p: *mut usize) -> Bool {
    fail_with(ERROR_INVALID_PARAMETER, FALSE)
}

/// HeapQueryInformation: HeapCompatibilityInformation (0) is 2, the
/// low-fragmentation heap, as for Windows' default heaps.
#[unsafe(no_mangle)]
pub extern "system" fn HeapQueryInformation(
    _heap: Handle,
    class: u32,
    info: *mut c_void,
    length: usize,
    ret: *mut usize,
) -> Bool {
    if class != 0 {
        unsupported(b"kernel32.dll!HeapQueryInformation(class)", class.into());
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    if !ret.is_null() {
        // SAFETY: the caller's output.
        unsafe { *ret = 4 };
    }
    if length < 4 || info.is_null() {
        return fail_with(ERROR_INSUFFICIENT_BUFFER, FALSE);
    }
    // SAFETY: the caller's buffer of at least 4 bytes.
    unsafe { info.cast::<u32>().write_unaligned(2) };
    TRUE
}

/// HeapWalk: not supported (reported); the walk ends at once.
#[unsafe(no_mangle)]
pub extern "system" fn HeapWalk(_heap: Handle, _entry: *mut c_void) -> Bool {
    unsupported(b"kernel32.dll!HeapWalk", 0);
    fail_with(ERROR_NO_MORE_ITEMS, FALSE)
}

// ---- psapi (K32*) ----

fn is_current(process: Handle) -> bool {
    process == GetCurrentProcess() || super::process::GetProcessId(process) == GetCurrentProcessId()
}

/// The loaded images, the executable first, then by address.
pub(crate) fn modules(mut f: impl FnMut(usize)) {
    let exe = super::mapping::GetModuleHandleW(core::ptr::null()) as usize;
    f(exe);
    regions(|r| {
        if r.kind == MEM_IMAGE && r.base == r.allocation_base && r.base != exe {
            f(r.base);
        }
        true
    });
}

/// K32EnumProcessModules / EnumProcessModules (this process only).
#[unsafe(no_mangle)]
pub extern "system" fn K32EnumProcessModules(
    process: Handle,
    out: *mut Handle,
    size: u32,
    needed: *mut u32,
) -> Bool {
    if !is_current(process) {
        unsupported(b"kernel32.dll!K32EnumProcessModules(another process)", 0);
        return fail_with(ERROR_ACCESS_DENIED, FALSE);
    }
    let mut n = 0usize;
    let room = size as usize / size_of::<Handle>();
    modules(|base| {
        if n < room {
            // SAFETY: inside the caller's array of `room` handles.
            unsafe { *out.add(n) = base as Handle };
        }
        n += 1;
    });
    // SAFETY: the caller's output.
    unsafe { *needed = (n * size_of::<Handle>()) as u32 };
    TRUE
}

/// K32EnumProcessModulesEx / EnumProcessModulesEx: a 64-bit process's
/// modules are all 64-bit, so LIST_MODULES_DEFAULT (0), _64BIT (2) and
/// _ALL (3) list what K32EnumProcessModules does; LIST_MODULES_32BIT (1)
/// lists none.
#[unsafe(no_mangle)]
pub extern "system" fn K32EnumProcessModulesEx(
    process: Handle,
    out: *mut Handle,
    size: u32,
    needed: *mut u32,
    filter: u32,
) -> Bool {
    match filter {
        0 | 2 | 3 => K32EnumProcessModules(process, out, size, needed),
        1 if is_current(process) => {
            // SAFETY: the caller's output.
            unsafe { *needed = 0 };
            TRUE
        }
        _ => {
            unsupported(b"kernel32.dll!K32EnumProcessModulesEx(filter)", filter.into());
            fail_with(ERROR_INVALID_PARAMETER, FALSE)
        }
    }
}

/// MODULEINFO from the image's headers.
#[unsafe(no_mangle)]
pub extern "system" fn K32GetModuleInformation(
    process: Handle,
    module: Handle,
    info: *mut usize,
    size: u32,
) -> Bool {
    if !is_current(process) {
        return fail_with(ERROR_ACCESS_DENIED, FALSE);
    }
    // MODULEINFO: lpBaseOfDll, SizeOfImage (a DWORD, pointer-aligned),
    // EntryPoint.
    if (size as usize) < 3 * size_of::<usize>() {
        return fail_with(ERROR_INSUFFICIENT_BUFFER, FALSE);
    }
    let base =
        if module.is_null() { super::mapping::GetModuleHandleW(core::ptr::null()) } else { module }
            as usize;
    // SAFETY: a loaded image: e_lfanew at +0x3c, then the optional header
    // (AddressOfEntryPoint at +16, SizeOfImage at +56 of it).
    unsafe {
        let nt = base + *((base + 0x3c) as *const u32) as usize;
        if *(base as *const u16) != 0x5a4d || *(nt as *const u32) != 0x4550 {
            return fail_with(ERROR_INVALID_HANDLE, FALSE);
        }
        let entry = *((nt + 24 + 16) as *const u32) as usize;
        let image_size = *((nt + 24 + 56) as *const u32) as usize;
        *info = base;
        *info.add(1) = image_size;
        *info.add(2) = if entry == 0 { 0 } else { base + entry };
    }
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn K32GetModuleFileNameExW(
    process: Handle,
    module: Handle,
    buf: *mut u16,
    size: u32,
) -> u32 {
    if !is_current(process) {
        return fail_with(ERROR_ACCESS_DENIED, 0);
    }
    super::process::GetModuleFileNameW(module, buf, size)
}

/// K32GetModuleBaseNameW: the last component of the module's file name
/// (truncated to the buffer, NUL-terminated); its length.
#[unsafe(no_mangle)]
pub extern "system" fn K32GetModuleBaseNameW(
    process: Handle,
    module: Handle,
    buf: *mut u16,
    size: u32,
) -> u32 {
    if !is_current(process) {
        return fail_with(ERROR_ACCESS_DENIED, 0);
    }
    if buf.is_null() || size == 0 {
        return fail_with(ERROR_INSUFFICIENT_BUFFER, 0);
    }
    let mut path = [0u16; 520];
    let n = super::process::GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32);
    if n == 0 {
        return 0;
    }
    let path = &path[..n as usize];
    let start = path.iter().rposition(|&c| c == u16::from(b'\\')).map_or(0, |i| i + 1);
    let name = &path[start..];
    let len = name.len().min(size as usize - 1);
    // SAFETY: the caller's buffer of `size` characters.
    unsafe {
        core::ptr::copy_nonoverlapping(name.as_ptr(), buf, len);
        *buf.add(len) = 0;
    }
    len as u32
}

/// K32GetModuleBaseNameA: as the W form, in the ANSI code page (ASCII).
#[unsafe(no_mangle)]
pub extern "system" fn K32GetModuleBaseNameA(
    process: Handle,
    module: Handle,
    buf: *mut u8,
    size: u32,
) -> u32 {
    let mut wide = [0u16; 260];
    let n = K32GetModuleBaseNameW(process, module, wide.as_mut_ptr(), 260) as usize;
    if n == 0 {
        return 0;
    }
    if buf.is_null() || size == 0 {
        return fail_with(ERROR_INSUFFICIENT_BUFFER, 0);
    }
    let len = n.min(size as usize - 1);
    // SAFETY: the caller's buffer of `size` bytes.
    unsafe {
        for (i, &c) in wide[..len].iter().enumerate() {
            *buf.add(i) = if c < 0x80 { c as u8 } else { b'?' };
        }
        *buf.add(len) = 0;
    }
    len as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn K32GetModuleFileNameExA(
    process: Handle,
    module: Handle,
    buf: *mut u8,
    size: u32,
) -> u32 {
    if !is_current(process) {
        return fail_with(ERROR_ACCESS_DENIED, 0);
    }
    super::mapping::GetModuleFileNameA(module, buf, size)
}

/// PROCESS_MEMORY_COUNTERS(_EX): the working set and page file use are
/// the committed memory (gala does not page out), private usage the
/// committed private memory, the pool quota figures the kernel's
/// (ProcessVmCounters).
#[unsafe(no_mangle)]
pub extern "system" fn K32GetProcessMemoryInfo(
    process: Handle,
    counters: *mut u8,
    size: u32,
) -> Bool {
    // PROCESS_MEMORY_COUNTERS(_EX): cb and PageFaultCount (DWORDs), then
    // eight SIZE_Ts (nine with PrivateUsage).
    const P: usize = size_of::<usize>();
    if !is_current(process) {
        return fail_with(ERROR_ACCESS_DENIED, FALSE);
    }
    if (size as usize) < 8 + 8 * P {
        return fail_with(ERROR_INSUFFICIENT_BUFFER, FALSE);
    }
    // The working set and the commit charge as the kernel keeps them (one
    // query, not a walk of the address space).
    let vm = match vm_counters(process) {
        Ok(vm) => vm,
        Err(status) => return fail(status, FALSE),
    };
    let n = if size as usize >= 8 + 9 * P { 9 } else { 8 };
    let values = [vm[3], vm[4], vm[5], vm[6], vm[7], vm[8], vm[9], vm[10], vm[9]];
    // SAFETY: the caller's structure of `size` >= 8 + n * P bytes.
    unsafe {
        counters.cast::<u32>().write_unaligned((8 + n * P) as u32);
        counters.add(4).cast::<u32>().write_unaligned(0);
        for (i, &v) in values.iter().enumerate().take(n) {
            counters.add(8 + i * P).cast::<usize>().write_unaligned(v);
        }
    }
    TRUE
}

/// QueryFullProcessImageNameW: this process's image path (Win32 form).
#[unsafe(no_mangle)]
pub extern "system" fn QueryFullProcessImageNameW(
    process: Handle,
    flags: u32,
    buf: *mut u16,
    size: *mut u32,
) -> Bool {
    if !is_current(process) {
        unsupported(b"kernel32.dll!QueryFullProcessImageNameW(another process)", 0);
        return fail_with(ERROR_ACCESS_DENIED, FALSE);
    }
    if flags != 0 {
        unsupported(b"kernel32.dll!QueryFullProcessImageNameW(native)", flags.into());
    }
    // SAFETY: the caller's in/out size.
    let room = unsafe { *size };
    let n = super::process::GetModuleFileNameW(core::ptr::null_mut(), buf, room);
    if n == 0 || n >= room {
        return fail_with(ERROR_INSUFFICIENT_BUFFER, FALSE);
    }
    // SAFETY: as above.
    unsafe { *size = n };
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn QueryFullProcessImageNameA(
    process: Handle,
    flags: u32,
    buf: *mut u8,
    size: *mut u32,
) -> Bool {
    if !is_current(process) || flags != 0 {
        return fail_with(ERROR_ACCESS_DENIED, FALSE);
    }
    // SAFETY: the caller's in/out size.
    let room = unsafe { *size };
    let n = super::mapping::GetModuleFileNameA(core::ptr::null_mut(), buf, room);
    if n == 0 || n >= room {
        return fail_with(ERROR_INSUFFICIENT_BUFFER, FALSE);
    }
    // SAFETY: as above.
    unsafe { *size = n };
    TRUE
}

// ---- Files ----

/// SetFileAttributesW: FileBasicInformation with only FileAttributes set
/// (times 0 = unchanged; FILE_ATTRIBUTE_NORMAL alone clears them).
#[unsafe(no_mangle)]
pub extern "system" fn SetFileAttributesW(path: *const u16, attributes: u32) -> Bool {
    let h = match open_path(
        path,
        0x100 | 0x10_0000, // FILE_WRITE_ATTRIBUTES | SYNCHRONIZE
        SHARE_ALL,
        1,
        FILE_OPEN_FOR_BACKUP_INTENT | FILE_SYNCHRONOUS_IO_NONALERT,
        0,
    ) {
        Ok((h, _)) => h,
        Err(e) => return fail_with(e, FALSE),
    };
    let ok = set_basic_attributes(h, attributes);
    CloseHandle(h);
    ok
}

fn set_basic_attributes(h: Handle, attributes: u32) -> Bool {
    let mut info = [0u8; 40];
    info[32..36].copy_from_slice(&attributes.to_le_bytes());
    let mut iosb = [0usize; 2];
    // SAFETY: a 40-byte FILE_BASIC_INFORMATION.
    let status = unsafe { NtSetInformationFile(h, &mut iosb, info.as_ptr(), 40, 4) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

/// SetFileInformationByHandle: the FILE_INFO_BY_HANDLE_CLASS values whose
/// structures are the NT ones (basic, rename, disposition, allocation,
/// end of file) go to NtSetInformationFile; others are reported.
#[unsafe(no_mangle)]
pub extern "system" fn SetFileInformationByHandle(
    h: Handle,
    class: u32,
    info: *const c_void,
    size: u32,
) -> Bool {
    // (Win32 class, NT class, minimum size)
    let (nt, min) = match class {
        0 => (4, 40), // FileBasicInfo -> FileBasicInformation
        3 => (10, super::mapping::RENAME_HEADER as u32), // FileRenameInfo -> FileRenameInformation
        4 => (13, 1), // FileDispositionInfo -> FileDispositionInformation
        5 => (19, 8), // FileAllocationInfo -> FileAllocationInformation
        6 => (20, 8), // FileEndOfFileInfo -> FileEndOfFileInformation
        _ => {
            unsupported(b"kernel32.dll!SetFileInformationByHandle(class)", class.into());
            return fail_with(ERROR_INVALID_PARAMETER, FALSE);
        }
    };
    if size < min || info.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let mut iosb = [0usize; 2];
    // SAFETY: the caller's structure of `size` bytes.
    let status = unsafe { NtSetInformationFile(h, &mut iosb, info.cast(), size, nt) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

/// FindFirstFileExW: FindExInfoStandard/Basic and FindExSearchNameMatch
/// (or the directories-only hint) are FindFirstFileW.
#[unsafe(no_mangle)]
pub extern "system" fn FindFirstFileExW(
    name: *const u16,
    level: u32,
    data: *mut u16,
    op: u32,
    filter: *mut c_void,
    _flags: u32,
) -> Handle {
    if level > 1 || op > 1 || !filter.is_null() {
        unsupported(
            b"kernel32.dll!FindFirstFileExW(level/op)",
            (u64::from(level) << 8) | u64::from(op),
        );
        return fail_with(ERROR_INVALID_PARAMETER, INVALID_HANDLE_VALUE);
    }
    FindFirstFileW(name, data)
}

/// GetVolumePathNameW: gala has one volume, C:\.
#[unsafe(no_mangle)]
pub extern "system" fn GetVolumePathNameW(path: *const u16, buf: *mut u16, size: u32) -> Bool {
    if path.is_null() || buf.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let root = [u16::from(b'C'), u16::from(b':'), u16::from(b'\\')];
    if size < 4 {
        return fail_with(206, FALSE); // ERROR_FILENAME_EXCED_RANGE
    }
    // SAFETY: the caller's buffer of `size` >= 4 units.
    unsafe {
        core::ptr::copy_nonoverlapping(root.as_ptr(), buf, 3);
        *buf.add(3) = 0;
    }
    TRUE
}

// ---- Sound, diagnostics, version ----

/// Beep: there is no speaker; the call takes its duration.
#[unsafe(no_mangle)]
pub extern "system" fn Beep(_frequency: u32, ms: u32) -> Bool {
    Sleep(ms);
    TRUE
}

/// The execution state SetThreadExecutionState keeps (process-wide: gala
/// has no power management, so it only remembers it).
static EXECUTION_STATE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0x8000_0000); // ES_CONTINUOUS

/// SetThreadExecutionState(flags): the previous state; with ES_CONTINUOUS
/// the new one stays (gala never sleeps or turns the display off).
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadExecutionState(flags: u32) -> u32 {
    const ES_CONTINUOUS: u32 = 0x8000_0000;
    let order = core::sync::atomic::Ordering::Relaxed;
    if flags & ES_CONTINUOUS != 0 {
        EXECUTION_STATE.swap(flags, order)
    } else {
        EXECUTION_STATE.load(order)
    }
}

/// UnhandledExceptionFilter: the filter SetUnhandledExceptionFilter set,
/// if any; otherwise EXCEPTION_EXECUTE_HANDLER (1), so the caller ends the
/// process (there is no error reporting to show).
#[unsafe(no_mangle)]
pub extern "system" fn UnhandledExceptionFilter(pointers: *mut c_void) -> i32 {
    let filter = super::mapping::unhandled_filter();
    if filter == 0 {
        return 1;
    }
    // SAFETY: a TOP_LEVEL_EXCEPTION_FILTER registered by the program.
    let f: extern "system" fn(*mut c_void) -> i32 = unsafe { core::mem::transmute(filter) };
    f(pointers)
}

/// PRODUCT_PROFESSIONAL: the edition gala reports (the registry's
/// EditionID is Professional, its product type a workstation).
const PRODUCT_PROFESSIONAL: u32 = 0x30;

/// GetProductInfo: the product type of gala's edition for any Windows
/// version from 6.0 on (the documented minimum); FALSE below that.
#[unsafe(no_mangle)]
pub extern "system" fn GetProductInfo(
    major: u32,
    _minor: u32,
    _sp_major: u32,
    _sp_minor: u32,
    kind: *mut u32,
) -> i32 {
    if major < 6 || kind.is_null() {
        return 0;
    }
    // SAFETY: the caller's output (checked non-null).
    unsafe { kind.write(PRODUCT_PROFESSIONAL) };
    1
}

/// OSVERSIONINFOEXW's fields VerifyVersionInfoW compares, with this
/// system's values.
fn version_fields(info: *const u32) -> [(u32, u64, u64); 6] {
    // SAFETY: the caller's OSVERSIONINFOEXW (284 bytes): sizes at +0,
    // major +4, minor +8, build +12, platform +16, service pack major and
    // minor at +276/+278 (WORDs).
    let (major, minor, build, platform, sp_major, sp_minor) = unsafe {
        let b = info.cast::<u8>();
        (
            *info.add(1),
            *info.add(2),
            *info.add(3),
            *info.add(4),
            u32::from(b.add(276).cast::<u16>().read_unaligned()),
            u32::from(b.add(278).cast::<u16>().read_unaligned()),
        )
    };
    [
        (0x1, minor.into(), 0),                        // VER_MINORVERSION
        (0x2, major.into(), kuser_version().0.into()), // VER_MAJORVERSION
        (0x4, build.into(), kuser_version().2.into()), // VER_BUILDNUMBER
        (0x8, platform.into(), 2),                     // VER_PLATFORMID (NT)
        (0x10, sp_minor.into(), 0),                    // VER_SERVICEPACKMINOR
        (0x20, sp_major.into(), 0),                    // VER_SERVICEPACKMAJOR
    ]
}

/// VerifyVersionInfoA: VerifyVersionInfoW on the same fields;
/// OSVERSIONINFOEXA (156 bytes) has an ANSI szCSDVersion[128], so its
/// service pack WORDs are at +148/+150 (the W form's at +276/+278).
#[unsafe(no_mangle)]
pub extern "system" fn VerifyVersionInfoA(info: *mut u8, kind: u32, mask: u64) -> Bool {
    if info.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let mut wide = [0u32; 71]; // OSVERSIONINFOEXW, 284 bytes
    // SAFETY: the caller's OSVERSIONINFOEXA: the five DWORDs, then the
    // service pack WORDs after szCSDVersion.
    unsafe {
        for (i, w) in wide.iter_mut().enumerate().take(5) {
            *w = info.cast::<u32>().add(i).read_unaligned();
        }
        let sp = info.add(148).cast::<u32>().read_unaligned();
        wide.as_mut_ptr().cast::<u8>().add(276).cast::<u32>().write_unaligned(sp);
    }
    wide[0] = 284;
    VerifyVersionInfoW(wide.as_mut_ptr(), kind, mask)
}

/// VerifyVersionInfoW: compares the requested fields with the version
/// reported to this program (its manifest decides, as for GetVersionEx;
/// no service pack); ERROR_OLD_WIN_VERSION when they
/// do not match. Major, minor and service pack are compared as one
/// version, as documented, when several are given.
#[unsafe(no_mangle)]
pub extern "system" fn VerifyVersionInfoW(info: *mut u32, kind: u32, mask: u64) -> Bool {
    let mut fields = version_fields(info);
    // The minor version's system value.
    fields[0].2 = kuser_version().1.into();
    let condition = |bit: u32| (mask >> (3 * bit.trailing_zeros())) & 7;
    let compare = |cond: u64, system: u64, wanted: u64| match cond {
        1 => system == wanted, // VER_EQUAL
        2 => system > wanted,  // VER_GREATER
        3 => system >= wanted, // VER_GREATER_EQUAL
        4 => system < wanted,  // VER_LESS
        5 => system <= wanted, // VER_LESS_EQUAL
        _ => false,
    };
    // Major/minor/service pack as one number when compared together.
    let version_bits = kind & (0x1 | 0x2 | 0x10 | 0x20);
    if version_bits != 0 {
        let cond = condition(if kind & 0x2 != 0 {
            0x2
        } else {
            version_bits & version_bits.wrapping_neg()
        });
        let pack = |f: &[(u32, u64, u64); 6], pick: fn(&(u32, u64, u64)) -> u64| {
            let get = |bit: u32| {
                if kind & bit != 0 { pick(&f[bit.trailing_zeros() as usize]) } else { 0 }
            };
            (get(0x2) << 48) | (get(0x1) << 32) | (get(0x20) << 16) | get(0x10)
        };
        let wanted = pack(&fields, |f| f.1);
        let system = pack(&fields, |f| f.2);
        if !compare(cond, system, wanted) {
            return fail_with(ERROR_OLD_WIN_VERSION, FALSE);
        }
    }
    for &(bit, wanted, system) in &fields {
        if kind & bit != 0 && version_bits & bit == 0 && !compare(condition(bit), system, wanted) {
            return fail_with(ERROR_OLD_WIN_VERSION, FALSE);
        }
    }
    TRUE
}

/// K32GetPerformanceInfo(pPerformanceInformation, cb): PERFORMANCE_INFORMATION
/// (cb, ten SIZE_Ts, three DWORDs: 104 bytes on x64, 56 on x86) in pages: the commit total, limit and peak and physical
/// memory from the kernel's counters; the system cache and kernel pools
/// are not counted (0); handle, process and thread counts from the
/// process list.
#[unsafe(no_mangle)]
pub extern "system" fn K32GetPerformanceInfo(info: *mut u8, size: u32) -> Bool {
    const P: usize = size_of::<usize>();
    // The SIZE_Ts start pointer-aligned after cb; the DWORDs follow them.
    const DWORDS: usize = P + 10 * P;
    const SIZE: usize = (DWORDS + 12).next_multiple_of(P);
    if info.is_null() || (size as usize) < SIZE {
        return fail_with(ERROR_BAD_LENGTH, FALSE);
    }
    let physical = basic_information().physical_pages as usize;
    let mut perf = [0u8; 0x158];
    let mut ret = 0;
    // SAFETY: a buffer of the class's size.
    unsafe { NtQuerySystemInformation(2, perf.as_mut_ptr(), 0x158, &mut ret) };
    let at = |b: &[u8], o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize;
    let (processes, threads, handles) = super::toolhelp::system_counts();
    let fields: [usize; 13] = [
        at(&perf, 0x30),  // CommitTotal
        at(&perf, 0x34),  // CommitLimit
        at(&perf, 0x38),  // CommitPeak
        physical,         // PhysicalTotal
        at(&perf, 0x2c),  // PhysicalAvailable
        0,                // SystemCache
        0,                // KernelTotal
        0,                // KernelPaged
        0,                // KernelNonpaged
        4096,             // PageSize
        handles as usize, // HandleCount (DWORDs from here)
        processes as usize,
        threads as usize,
    ];
    // SAFETY: the caller's buffer of at least SIZE bytes.
    unsafe {
        info.cast::<u32>().write_unaligned(SIZE as u32);
        for (i, v) in fields[..10].iter().enumerate() {
            info.add(P + P * i).cast::<usize>().write_unaligned(*v);
        }
        for (i, v) in fields[10..].iter().enumerate() {
            info.add(DWORDS + 4 * i).cast::<u32>().write_unaligned(*v as u32);
        }
    }
    TRUE
}

const ERROR_BAD_LENGTH: u32 = 24;

/// K32GetProcessImageFileNameW(process, buf, size): the process's image
/// in the NT namespace (`\Device\HarddiskVolume1\...`), from
/// ProcessImageFileName; its length, or 0 (ERROR_INSUFFICIENT_BUFFER)
/// when it does not fit with its NUL.
#[unsafe(no_mangle)]
pub extern "system" fn K32GetProcessImageFileNameW(
    process: Handle,
    buf: *mut u16,
    size: u32,
) -> u32 {
    // UNICODE_STRING, then its characters.
    let mut info = [0usize; 1 + (2 * 1100) / size_of::<usize>()];
    let mut ret = 0u32;
    // SAFETY: a buffer of the given size.
    let status = unsafe {
        NtQueryInformationProcess(
            process as isize,
            27, // ProcessImageFileName
            info.as_mut_ptr().cast(),
            size_of_val(&info) as u32,
            &mut ret,
        )
    };
    if status != STATUS_SUCCESS {
        return fail(status, 0);
    }
    // SAFETY: the UNICODE_STRING the kernel wrote; its buffer in `info`.
    let (len, chars) = unsafe {
        let us = info.as_ptr().cast::<u16>();
        let len = usize::from(*us) / 2;
        let p = *info.as_ptr().add(1) as *const u16;
        (len, core::slice::from_raw_parts(p, len))
    };
    if buf.is_null() || len + 1 > size as usize {
        return fail_with(ERROR_INSUFFICIENT_BUFFER, 0);
    }
    // SAFETY: the caller's buffer of `size` > len characters.
    unsafe {
        core::ptr::copy_nonoverlapping(chars.as_ptr(), buf, len);
        *buf.add(len) = 0;
    }
    len as u32
}

/// K32GetProcessImageFileNameA: the W function in UTF-8 (gala's ANSI
/// code page).
#[unsafe(no_mangle)]
pub extern "system" fn K32GetProcessImageFileNameA(
    process: Handle,
    buf: *mut u8,
    size: u32,
) -> u32 {
    let mut w = [0u16; 1100];
    let n = K32GetProcessImageFileNameW(process, w.as_mut_ptr(), w.len() as u32) as usize;
    if n == 0 {
        return 0;
    }
    let mut len = 0;
    let mut out = [0u8; 3 * 1100];
    for c in char::decode_utf16(w[..n].iter().copied()) {
        let mut b = [0u8; 4];
        for &x in c.unwrap_or(char::REPLACEMENT_CHARACTER).encode_utf8(&mut b).as_bytes() {
            out[len] = x;
            len += 1;
        }
    }
    if buf.is_null() || len + 1 > size as usize {
        return fail_with(ERROR_INSUFFICIENT_BUFFER, 0);
    }
    // SAFETY: the caller's buffer of `size` > len bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(out.as_ptr(), buf, len);
        *buf.add(len) = 0;
    }
    len as u32
}

/// GetPhysicallyInstalledSystemMemory(&kilobytes): the memory installed,
/// which Windows reads from the firmware's SMBIOS memory devices. gala
/// reports the physical memory it manages (SystemBasicInformation's
/// pages): never less than GlobalMemoryStatusEx's total, as on Windows.
#[unsafe(no_mangle)]
pub extern "system" fn GetPhysicallyInstalledSystemMemory(kilobytes: *mut u64) -> Bool {
    if kilobytes.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let b = super::basic_information();
    let bytes = u64::from(b.physical_pages) * u64::from(b.page_size);
    if bytes == 0 {
        return fail_with(13, FALSE); // ERROR_INVALID_DATA
    }
    // SAFETY: the caller's ULONGLONG.
    unsafe { kilobytes.write_unaligned(bytes.div_ceil(1024)) };
    TRUE
}

/// IsSystemResumeAutomatic(): whether the system woke without a user
/// (a wake timer); gala does not sleep, so FALSE.
#[unsafe(no_mangle)]
pub extern "system" fn IsSystemResumeAutomatic() -> Bool {
    FALSE
}
