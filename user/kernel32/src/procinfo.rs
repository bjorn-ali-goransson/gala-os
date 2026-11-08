//! Process and thread information Chromium's start-up reads and sets
//! (ROADMAP BR): mitigation policies, Get/SetProcessInformation,
//! SetThreadInformation, heap information, power requests, the working-set
//! query, session ids, processor counts and a few small queries.
//!
//! Mitigation policies are kept per process (this DLL's statics) as the
//! flags programs set; gala does not enforce them yet (BR stage 3, the
//! sandbox). Settings that only steer Windows' memory manager or scheduler
//! (memory and power priorities, shutdown order) are kept and read back
//! but change nothing in gala.

use super::*;
use core::sync::atomic::{AtomicU32, Ordering};

const ERROR_NOT_FOUND: u32 = 1168;
const ERROR_BUFFER_OVERFLOW: u32 = 111;

// ---- Mitigation policies ----------------------------------------------

/// PROCESS_MITIGATION_POLICY values up to ProcessActivationContextTrustPolicy.
const POLICIES: usize = 20;
const PROCESS_DEP_POLICY: u32 = 0;
const PROCESS_MITIGATION_OPTIONS_MASK: u32 = 5;

/// The flags this process set, per policy.
static POLICY_FLAGS: [AtomicU32; POLICIES] = [const { AtomicU32::new(0) }; POLICIES];

/// The size of policy `policy`'s structure.
fn policy_size(policy: u32) -> usize {
    match policy {
        // PROCESS_MITIGATION_DEP_POLICY { DWORD Flags; BOOLEAN Permanent }.
        PROCESS_DEP_POLICY => 8,
        // ULONG64[2] (the second is optional: 8 is accepted too).
        PROCESS_MITIGATION_OPTIONS_MASK => 16,
        _ => 4,
    }
}

fn is_current_process(process: Handle) -> bool {
    process == GetCurrentProcess() || super::process::GetProcessId(process) == GetCurrentProcessId()
}

/// GetProcessMitigationPolicy(hProcess, MitigationPolicy, lpBuffer,
/// dwLength): this process's flags as set; DEP is always on and permanent
/// (x64). Another process's policies are not tracked (reported; zeros).
#[unsafe(no_mangle)]
pub extern "system" fn GetProcessMitigationPolicy(
    process: Handle,
    policy: u32,
    buffer: *mut c_void,
    length: usize,
) -> Bool {
    if policy as usize >= POLICIES || buffer.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let size = policy_size(policy);
    let ok_size = length == size || (policy == PROCESS_MITIGATION_OPTIONS_MASK && length == 8);
    if !ok_size {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let flags = if is_current_process(process) {
        POLICY_FLAGS[policy as usize].load(Ordering::Relaxed)
    } else {
        unsupported(b"kernel32.dll!GetProcessMitigationPolicy(other process)", policy.into());
        0
    };
    // SAFETY: the caller's buffer of `length` bytes.
    unsafe {
        core::ptr::write_bytes(buffer.cast::<u8>(), 0, length);
        if policy == PROCESS_DEP_POLICY {
            // Enable, and Permanent: 64-bit processes always run with DEP.
            buffer.cast::<u32>().write_unaligned(1);
            buffer.cast::<u8>().add(4).write(1);
        } else if policy != PROCESS_MITIGATION_OPTIONS_MASK {
            buffer.cast::<u32>().write_unaligned(flags);
        }
    }
    TRUE
}

/// SetProcessMitigationPolicy(MitigationPolicy, lpBuffer, dwLength): turns
/// policies on for this process (flags accumulate: a policy once on stays
/// on, as on Windows). DEP cannot be changed in a 64-bit process
/// (ERROR_NOT_SUPPORTED); the options mask is read-only.
#[unsafe(no_mangle)]
pub extern "system" fn SetProcessMitigationPolicy(
    policy: u32,
    buffer: *const c_void,
    length: usize,
) -> Bool {
    if policy as usize >= POLICIES
        || policy == PROCESS_MITIGATION_OPTIONS_MASK
        || buffer.is_null()
        || length != policy_size(policy)
    {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    if policy == PROCESS_DEP_POLICY {
        return fail_with(ERROR_NOT_SUPPORTED, FALSE);
    }
    // SAFETY: the caller's 4-byte structure.
    let flags = unsafe { buffer.cast::<u32>().read_unaligned() };
    POLICY_FLAGS[policy as usize].fetch_or(flags, Ordering::Relaxed);
    TRUE
}

// ---- Process and thread information -----------------------------------

const PROCESS_MEMORY_PRIORITY: u32 = 0;
const PROCESS_POWER_THROTTLING: u32 = 4;
const PROCESS_PROTECTION_LEVEL_INFO: u32 = 7;
const PROCESS_MACHINE_TYPE_INFO: u32 = 9;
const MEMORY_PRIORITY_NORMAL: u32 = 5;
const POWER_THROTTLING_CURRENT_VERSION: u32 = 1;

static MEMORY_PRIORITY: AtomicU32 = AtomicU32::new(MEMORY_PRIORITY_NORMAL);
/// PROCESS_POWER_THROTTLING_STATE's ControlMask and StateMask.
static POWER_CONTROL: AtomicU32 = AtomicU32::new(0);
static POWER_STATE: AtomicU32 = AtomicU32::new(0);

/// SetProcessInformation(hProcess, ProcessInformationClass,
/// ProcessInformation, ProcessInformationSize): the memory priority (1-5)
/// and the power throttling state of this process are kept; other classes
/// and other processes are reported and fail.
#[unsafe(no_mangle)]
pub extern "system" fn SetProcessInformation(
    process: Handle,
    class: u32,
    info: *const c_void,
    size: u32,
) -> Bool {
    if info.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    if !is_current_process(process) {
        unsupported(b"kernel32.dll!SetProcessInformation(other process)", class.into());
        return fail_with(ERROR_NOT_SUPPORTED, FALSE);
    }
    let p = info.cast::<u32>();
    match class {
        PROCESS_MEMORY_PRIORITY if size == 4 => {
            // SAFETY: the caller's 4-byte MEMORY_PRIORITY_INFORMATION.
            let priority = unsafe { p.read_unaligned() };
            if !(1..=5).contains(&priority) {
                return fail_with(ERROR_INVALID_PARAMETER, FALSE);
            }
            MEMORY_PRIORITY.store(priority, Ordering::Relaxed);
            TRUE
        }
        PROCESS_POWER_THROTTLING if size == 12 => {
            // SAFETY: the caller's 12-byte PROCESS_POWER_THROTTLING_STATE.
            let (version, control, state) = unsafe {
                (p.read_unaligned(), p.add(1).read_unaligned(), p.add(2).read_unaligned())
            };
            // Known bits: EXECUTION_SPEED (1), IGNORE_TIMER_RESOLUTION (4).
            if version != POWER_THROTTLING_CURRENT_VERSION
                || control & !5 != 0
                || state & !control != 0
            {
                return fail_with(ERROR_INVALID_PARAMETER, FALSE);
            }
            POWER_CONTROL.store(control, Ordering::Relaxed);
            POWER_STATE.store(state, Ordering::Relaxed);
            TRUE
        }
        PROCESS_MEMORY_PRIORITY | PROCESS_POWER_THROTTLING => {
            fail_with(ERROR_INVALID_PARAMETER, FALSE)
        }
        _ => {
            unsupported(b"kernel32.dll!SetProcessInformation(class)", class.into());
            fail_with(ERROR_INVALID_PARAMETER, FALSE)
        }
    }
}

/// GetProcessInformation: the memory priority and power throttling state
/// (as set), the protection level (none: gala has no protected processes)
/// and the machine type (x64, user and kernel mode).
#[unsafe(no_mangle)]
pub extern "system" fn GetProcessInformation(
    process: Handle,
    class: u32,
    info: *mut c_void,
    size: u32,
) -> Bool {
    if info.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let p = info.cast::<u32>();
    let current = is_current_process(process);
    // SAFETY (each write): the caller's buffer of `size` bytes, checked.
    match class {
        PROCESS_MEMORY_PRIORITY if size == 4 => unsafe {
            let v = if current {
                MEMORY_PRIORITY.load(Ordering::Relaxed)
            } else {
                MEMORY_PRIORITY_NORMAL
            };
            p.write_unaligned(v);
        },
        PROCESS_POWER_THROTTLING if size == 12 => unsafe {
            p.write_unaligned(POWER_THROTTLING_CURRENT_VERSION);
            let (c, s) = if current {
                (POWER_CONTROL.load(Ordering::Relaxed), POWER_STATE.load(Ordering::Relaxed))
            } else {
                (0, 0)
            };
            p.add(1).write_unaligned(c);
            p.add(2).write_unaligned(s);
        },
        PROCESS_PROTECTION_LEVEL_INFO if size == 4 => unsafe {
            p.write_unaligned(0xffff_fffe); // PROTECTION_LEVEL_NONE
        },
        PROCESS_MACHINE_TYPE_INFO if size == 8 => unsafe {
            // PROCESS_MACHINE_INFORMATION { USHORT ProcessMachine; USHORT
            // Res0; MACHINE_ATTRIBUTES (UserEnabled | KernelEnabled) }.
            info.cast::<u16>().write_unaligned(0x8664);
            info.cast::<u16>().add(1).write_unaligned(0);
            p.add(1).write_unaligned(3);
        },
        PROCESS_MEMORY_PRIORITY
        | PROCESS_POWER_THROTTLING
        | PROCESS_PROTECTION_LEVEL_INFO
        | PROCESS_MACHINE_TYPE_INFO => return fail_with(ERROR_INVALID_PARAMETER, FALSE),
        _ => {
            unsupported(b"kernel32.dll!GetProcessInformation(class)", class.into());
            return fail_with(ERROR_INVALID_PARAMETER, FALSE);
        }
    }
    TRUE
}

/// SetThreadInformation(hThread, ThreadInformationClass, ...): the memory
/// priority (1-5), absolute CPU priority, dynamic code policy and power
/// throttling state are accepted (they only steer Windows' memory manager
/// and scheduler); other classes are reported.
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadInformation(
    _thread: Handle,
    class: u32,
    info: *const c_void,
    size: u32,
) -> Bool {
    if info.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let p = info.cast::<u32>();
    match (class, size) {
        // ThreadMemoryPriority: MEMORY_PRIORITY_INFORMATION.
        (0, 4) => {
            // SAFETY: the caller's 4 bytes.
            let v = unsafe { p.read_unaligned() };
            if !(1..=5).contains(&v) {
                return fail_with(ERROR_INVALID_PARAMETER, FALSE);
            }
            TRUE
        }
        // ThreadAbsoluteCpuPriority, ThreadDynamicCodePolicy (a DWORD).
        (1 | 2, 4) => TRUE,
        // ThreadPowerThrottling: THREAD_POWER_THROTTLING_STATE.
        (3, 12) => {
            // SAFETY: the caller's 12 bytes.
            let (version, control, state) = unsafe {
                (p.read_unaligned(), p.add(1).read_unaligned(), p.add(2).read_unaligned())
            };
            if version != POWER_THROTTLING_CURRENT_VERSION
                || control & !1 != 0
                || state & !control != 0
            {
                return fail_with(ERROR_INVALID_PARAMETER, FALSE);
            }
            TRUE
        }
        (0..=3, _) => fail_with(ERROR_INVALID_PARAMETER, FALSE),
        _ => {
            unsupported(b"kernel32.dll!SetThreadInformation(class)", class.into());
            fail_with(ERROR_INVALID_PARAMETER, FALSE)
        }
    }
}

static SHUTDOWN_LEVEL: AtomicU32 = AtomicU32::new(0x280);
static SHUTDOWN_FLAGS: AtomicU32 = AtomicU32::new(0);

/// SetProcessShutdownParameters(dwLevel, dwFlags): kept (gala shuts down
/// without asking processes, so the order changes nothing). Levels 0-0x4ff;
/// 0-0xff are reserved for the system (ERROR_INVALID_PARAMETER).
#[unsafe(no_mangle)]
pub extern "system" fn SetProcessShutdownParameters(level: u32, flags: u32) -> Bool {
    const SHUTDOWN_NORETRY: u32 = 1;
    if !(0x100..=0x4ff).contains(&level) || flags & !SHUTDOWN_NORETRY != 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    SHUTDOWN_LEVEL.store(level, Ordering::Relaxed);
    SHUTDOWN_FLAGS.store(flags, Ordering::Relaxed);
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn GetProcessShutdownParameters(level: *mut u32, flags: *mut u32) -> Bool {
    if level.is_null() || flags.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller's outputs.
    unsafe {
        *level = SHUTDOWN_LEVEL.load(Ordering::Relaxed);
        *flags = SHUTDOWN_FLAGS.load(Ordering::Relaxed);
    }
    TRUE
}

/// GetProcessHandleCount(hProcess, *pdwHandleCount): ProcessHandleCount.
#[unsafe(no_mangle)]
pub extern "system" fn GetProcessHandleCount(process: Handle, count: *mut u32) -> Bool {
    let mut n = 0u32;
    // SAFETY: a 4-byte buffer.
    let status = unsafe {
        NtQueryInformationProcess(
            process as isize,
            20,
            (&raw mut n).cast(),
            4,
            core::ptr::null_mut(),
        )
    };
    if status != STATUS_SUCCESS {
        return fail(status, FALSE);
    }
    if count.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller's output.
    unsafe { *count = n };
    TRUE
}

// ProcessIdToSessionId and WTSGetActiveConsoleSessionId are in process.rs
// (RS B2: the caller's session).

/// QueryProcessCycleTime: the process's processor time scaled to cycles,
/// as QueryThreadCycleTime does.
#[unsafe(no_mangle)]
pub extern "system" fn QueryProcessCycleTime(process: Handle, cycles: *mut u64) -> Bool {
    let (mut created, mut exited, mut kernel, mut user) = (0u64, 0u64, 0u64, 0u64);
    if super::process::GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user)
        == FALSE
    {
        return FALSE;
    }
    if cycles.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller's output.
    unsafe { *cycles = ((kernel + user) * 300).max(1) };
    TRUE
}

/// CheckRemoteDebuggerPresent: gala has no debuggers.
#[unsafe(no_mangle)]
pub extern "system" fn CheckRemoteDebuggerPresent(process: Handle, present: *mut Bool) -> Bool {
    if process.is_null() || present.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller's output.
    unsafe { *present = FALSE };
    TRUE
}

/// GetFirmwareType(*FirmwareType): FirmwareTypeUnknown (0); gala boots
/// from BIOS and UEFI alike and does not tell programs which.
#[unsafe(no_mangle)]
pub extern "system" fn GetFirmwareType(kind: *mut u32) -> Bool {
    if kind.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller's output.
    unsafe { *kind = 0 };
    TRUE
}

/// GetConsoleDisplayMode(*lpModeFlags): 0, a windowed console.
#[unsafe(no_mangle)]
pub extern "system" fn GetConsoleDisplayMode(flags: *mut u32) -> Bool {
    if flags.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller's output.
    unsafe { *flags = 0 };
    TRUE
}

/// The processors of group `group` (0, or ALL_PROCESSOR_GROUPS): gala has
/// one group; 0 with ERROR_INVALID_PARAMETER for another group.
fn processors_in(group: u16) -> u32 {
    if group != 0 && group != 0xffff {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let mut info = core::mem::MaybeUninit::<SystemInfo>::zeroed();
    GetSystemInfo(info.as_mut_ptr());
    // SAFETY: GetSystemInfo filled it.
    unsafe { info.assume_init() }.number_of_processors
}

#[unsafe(no_mangle)]
pub extern "system" fn GetActiveProcessorCount(group: u16) -> u32 {
    processors_in(group)
}

/// GetMaximumProcessorCount: gala does not add processors while running,
/// so the active ones.
#[unsafe(no_mangle)]
pub extern "system" fn GetMaximumProcessorCount(group: u16) -> u32 {
    processors_in(group)
}

// ---- Heaps -------------------------------------------------------------

/// GetProcessHeaps(NumberOfHeaps, *ProcessHeaps): gala reports the process
/// heap (heaps made with HeapCreate are not listed; TODO).
#[unsafe(no_mangle)]
pub extern "system" fn GetProcessHeaps(count: u32, heaps: *mut Handle) -> u32 {
    if count >= 1 && !heaps.is_null() {
        // SAFETY: the caller's array of `count` handles.
        unsafe { *heaps = super::heap::GetProcessHeap() };
    }
    1
}

/// HeapSetInformation: HeapCompatibilityInformation 2 (the
/// low-fragmentation heap, which gala's heaps already behave as),
/// HeapEnableTerminationOnCorruption (gala's heap ends the process on
/// corruption anyway) and HeapOptimizeResources are accepted.
#[unsafe(no_mangle)]
pub extern "system" fn HeapSetInformation(
    _heap: Handle,
    class: u32,
    info: *const c_void,
    length: usize,
) -> Bool {
    match class {
        0 => {
            if info.is_null() || length < 4 {
                return fail_with(ERROR_INVALID_PARAMETER, FALSE);
            }
            // SAFETY: the caller's 4 bytes.
            let v = unsafe { info.cast::<u32>().read_unaligned() };
            if v != 2 && v != 0 {
                return fail_with(ERROR_INVALID_PARAMETER, FALSE);
            }
            TRUE
        }
        1 => TRUE,
        3 => {
            // HEAP_OPTIMIZE_RESOURCES_INFORMATION { Version (1), Flags }.
            if info.is_null() || length < 8 {
                return fail_with(ERROR_INVALID_PARAMETER, FALSE);
            }
            TRUE
        }
        _ => {
            unsupported(b"kernel32.dll!HeapSetInformation(class)", class.into());
            fail_with(ERROR_INVALID_PARAMETER, FALSE)
        }
    }
}

/// HeapLock/HeapUnlock: gala's heaps take their own lock for each call;
/// a program that holds a heap "locked" only needs the calls to succeed
/// (it walks or frees from one thread).
#[unsafe(no_mangle)]
pub extern "system" fn HeapLock(heap: Handle) -> Bool {
    if heap.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn HeapUnlock(heap: Handle) -> Bool {
    if heap.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    TRUE
}

// ---- Memory -------------------------------------------------------------

/// K32QueryWorkingSetEx(hProcess, pv, cb): for each
/// PSAPI_WORKING_SET_EX_INFORMATION { VirtualAddress, VirtualAttributes },
/// whether the page is resident. gala never pages out, so a committed page
/// is valid (Valid, ShareCount 1, Win32Protection, Shared for image and
/// mapped pages); other pages get 0.
#[unsafe(no_mangle)]
pub extern "system" fn K32QueryWorkingSetEx(process: Handle, pv: *mut usize, cb: u32) -> Bool {
    const MEM_COMMIT: u32 = 0x1000;
    const MEM_PRIVATE: u32 = 0x2_0000;
    // An entry: VirtualAddress, VirtualAttributes (a ULONG_PTR).
    const ENTRY: usize = 2 * size_of::<usize>();
    if pv.is_null() || (cb as usize) < ENTRY {
        return fail_with(ERROR_BAD_LENGTH, FALSE);
    }
    for i in 0..cb as usize / ENTRY {
        // SAFETY: the caller's array of cb / ENTRY entries.
        let address = unsafe { pv.add(2 * i).read_unaligned() };
        let mut mbi = MemoryBasic::default();
        let n = super::runtime::VirtualQueryEx(
            process,
            address as *const c_void,
            (&raw mut mbi).cast(),
            size_of::<MemoryBasic>(),
        );
        if n == 0 && i == 0 && GetLastError() == ERROR_INVALID_HANDLE {
            return FALSE;
        }
        let (state, protect, kind) = (mbi.state, mbi.protect, mbi.kind);
        let mut attributes = 0usize;
        if n != 0 && state == MEM_COMMIT && protect & 0x01 == 0 {
            attributes = 1 | (1 << 1) | ((protect & 0x7ff) as usize) << 4;
            if kind != MEM_PRIVATE {
                attributes |= 1 << 15;
            }
        }
        // SAFETY: as above.
        unsafe { pv.add(2 * i + 1).write_unaligned(attributes) };
    }
    TRUE
}

const ERROR_BAD_LENGTH: u32 = 24;
const ERROR_INVALID_HANDLE: u32 = 6;

/// DiscardVirtualMemory(VirtualAddress, Size): the contents may be thrown
/// away; gala keeps them (allowed: the pages stay committed and their
/// contents are then undefined to the program). ERROR_SUCCESS.
#[unsafe(no_mangle)]
pub extern "system" fn DiscardVirtualMemory(address: *mut c_void, size: usize) -> u32 {
    if address.is_null() || size == 0 {
        return ERROR_INVALID_PARAMETER;
    }
    0
}

// ---- Power requests ----------------------------------------------------

/// PowerCreateRequest(REASON_CONTEXT*): a handle to close later; gala does
/// not sleep or turn the display off, so requests are kept by nobody (an
/// unnamed event stands for the request object).
#[unsafe(no_mangle)]
pub extern "system" fn PowerCreateRequest(context: *const c_void) -> Handle {
    if context.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, INVALID_HANDLE_VALUE);
    }
    let h = super::sync::CreateEventW(core::ptr::null_mut(), TRUE, FALSE, core::ptr::null());
    if h.is_null() { INVALID_HANDLE_VALUE } else { h }
}

/// PowerSetRequest(PowerRequest, RequestType): types 0-3.
#[unsafe(no_mangle)]
pub extern "system" fn PowerSetRequest(request: Handle, kind: u32) -> Bool {
    if request.is_null() || request == INVALID_HANDLE_VALUE || kind > 3 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn PowerClearRequest(request: Handle, kind: u32) -> Bool {
    PowerSetRequest(request, kind)
}

// ---- Packages, pipes, threads ------------------------------------------

/// GetPackagesByPackageFamily: no packaged applications are installed on
/// gala; ERROR_SUCCESS with count 0.
#[unsafe(no_mangle)]
pub extern "system" fn GetPackagesByPackageFamily(
    _family: *const u16,
    count: *mut u32,
    _names: *mut *mut u16,
    buffer_length: *mut u32,
    _buffer: *mut u16,
) -> u32 {
    if count.is_null() || buffer_length.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    // SAFETY: the caller's outputs.
    unsafe {
        *count = 0;
        *buffer_length = 0;
    }
    0
}

/// GetPackagePathByFullName: no package of that name (ERROR_NOT_FOUND).
#[unsafe(no_mangle)]
pub extern "system" fn GetPackagePathByFullName(
    name: *const u16,
    length: *mut u32,
    _path: *mut u16,
) -> u32 {
    if name.is_null() || length.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    ERROR_NOT_FOUND
}

/// GetNamedPipeServerProcessId: gala's pipes do not record their server
/// process yet (reported; ERROR_NOT_SUPPORTED).
#[unsafe(no_mangle)]
pub extern "system" fn GetNamedPipeServerProcessId(_pipe: Handle, _id: *mut u32) -> Bool {
    unsupported(b"kernel32.dll!GetNamedPipeServerProcessId", 0);
    fail_with(ERROR_NOT_SUPPORTED, FALSE)
}

/// Wow64GetThreadContext: not yet (a 64-bit debugger's view of a 32-bit
/// thread: ThreadWow64Context, ROADMAP WOW); fails as for a native thread.
#[unsafe(no_mangle)]
pub extern "system" fn Wow64GetThreadContext(_thread: Handle, _context: *mut c_void) -> Bool {
    fail_with(ERROR_INVALID_PARAMETER, FALSE)
}

// ---- Temporary file names -----------------------------------------------

// GetTempFileNameW is in lib.rs (`<pre><HEX>.tmp`, as Windows names them).

/// GetTempFileNameA: GetTempFileNameW with UTF-8 strings (gala's ANSI code
/// page).
#[unsafe(no_mangle)]
pub extern "system" fn GetTempFileNameA(
    path: *const u8,
    prefix: *const u8,
    unique: u32,
    out: *mut u8,
) -> u32 {
    const MAX: usize = 520;
    let (mut p, mut x, mut o) = ([0u16; MAX], [0u16; 8], [0u16; 261]);
    if path.is_null() || out.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let Some(n) = super::sync::ansi_to_wide(path, &mut p[..MAX - 1]) else {
        return fail_with(ERROR_FILENAME_EXCED_RANGE, 0);
    };
    p[n] = 0;
    let prefix_w = if prefix.is_null() {
        core::ptr::null()
    } else {
        // Only the first three characters count.
        let mut short = [0u8; 4];
        let mut i = 0;
        // SAFETY: the caller's NUL-terminated string.
        unsafe {
            while i < 3 && *prefix.add(i) != 0 {
                short[i] = *prefix.add(i);
                i += 1;
            }
        }
        let m = super::sync::ansi_to_wide(short.as_ptr(), &mut x[..7]).unwrap_or(0);
        x[m] = 0;
        x.as_ptr()
    };
    let r = GetTempFileNameW(p.as_ptr(), prefix_w, unique, o.as_mut_ptr());
    if r == 0 {
        return 0;
    }
    let len = o.iter().position(|&c| c == 0).unwrap_or(260);
    // Path characters are ASCII or converted back to UTF-8.
    let mut written = 0;
    for &c in &o[..len] {
        let mut b = [0u8; 4];
        let s = char::from_u32(u32::from(c)).unwrap_or('?').encode_utf8(&mut b);
        if written + s.len() >= 260 {
            return fail_with(ERROR_BUFFER_OVERFLOW, 0);
        }
        // SAFETY: the caller's MAX_PATH buffer.
        unsafe { core::ptr::copy_nonoverlapping(s.as_ptr(), out.add(written), s.len()) };
        written += s.len();
    }
    // SAFETY: as above.
    unsafe { *out.add(written) = 0 };
    r
}

/// FindFirstFileExA: FindFirstFileExW with a UTF-8 name, the data
/// converted back.
#[unsafe(no_mangle)]
pub extern "system" fn FindFirstFileExA(
    name: *const u8,
    level: u32,
    data: *mut u8,
    op: u32,
    filter: *mut c_void,
    flags: u32,
) -> Handle {
    const MAX: usize = 1024;
    let mut buf = [0u16; MAX];
    if name.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, INVALID_HANDLE_VALUE);
    }
    let Some(n) = super::sync::ansi_to_wide(name, &mut buf[..MAX - 1]) else {
        return fail_with(ERROR_FILENAME_EXCED_RANGE, INVALID_HANDLE_VALUE);
    };
    buf[n] = 0;
    let mut w = [0u16; 296];
    let find =
        super::sysmem::FindFirstFileExW(buf.as_ptr(), level, w.as_mut_ptr(), op, filter, flags);
    if find != INVALID_HANDLE_VALUE {
        super::ansi::find_data_to_ansi(&w, data);
    }
    find
}

/// QueryInterruptTime(Precise) and QueryUnbiasedInterruptTimePrecise
/// (API set api-ms-win-core-realtime, whose host kernelbase has its own):
/// KUSER_SHARED_DATA's InterruptTime, 100 ns since boot.
#[unsafe(no_mangle)]
pub extern "system" fn QueryInterruptTimePrecise(out: *mut u64) {
    let _ = super::mapping::QueryUnbiasedInterruptTime(out);
}

#[unsafe(no_mangle)]
pub extern "system" fn QueryUnbiasedInterruptTimePrecise(out: *mut u64) {
    let _ = super::mapping::QueryUnbiasedInterruptTime(out);
}

#[unsafe(no_mangle)]
pub extern "system" fn QueryInterruptTime(out: *mut u64) {
    let _ = super::mapping::QueryUnbiasedInterruptTime(out);
}

// ---- Power status, prefetch, mapped file names ---------------------------

/// GetSystemPowerStatus(lpSystemPowerStatus): on AC power with no system
/// battery (BatteryFlag 128, life unknown: 255 and -1), battery saver off.
#[unsafe(no_mangle)]
pub extern "system" fn GetSystemPowerStatus(status: *mut u8) -> Bool {
    if status.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SYSTEM_POWER_STATUS: ACLineStatus, BatteryFlag, BatteryLifePercent,
    // SystemStatusFlag, BatteryLifeTime, BatteryFullLifeTime.
    // SAFETY: the caller's 12-byte structure.
    unsafe {
        *status = 1;
        *status.add(1) = 128;
        *status.add(2) = 255;
        *status.add(3) = 0;
        status.add(4).cast::<u32>().write_unaligned(u32::MAX);
        status.add(8).cast::<u32>().write_unaligned(u32::MAX);
    }
    TRUE
}

/// PrefetchVirtualMemory(hProcess, NumberOfEntries, VirtualAddresses,
/// Flags): a hint; gala's memory is resident already.
#[unsafe(no_mangle)]
pub extern "system" fn PrefetchVirtualMemory(
    _process: Handle,
    entries: usize,
    addresses: *const c_void,
    flags: u32,
) -> Bool {
    if flags != 0 || (entries != 0 && addresses.is_null()) {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    TRUE
}

/// K32GetMappedFileNameW(hProcess, lpv, lpFilename, nSize): the NT path of
/// the file mapped at `lpv` (MemoryMappedFilenameInformation), truncated
/// to nSize characters; the length copied, 0 for memory that maps no file.
#[unsafe(no_mangle)]
pub extern "system" fn K32GetMappedFileNameW(
    process: Handle,
    address: *const c_void,
    name: *mut u16,
    size: u32,
) -> u32 {
    if name.is_null() || size == 0 {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    // UNICODE_STRING then the characters.
    let mut buf = [0u64; 2 + 132];
    let mut ret = 0usize;
    // SAFETY: our buffer of the size given.
    let status = unsafe {
        NtQueryVirtualMemory(
            process as isize,
            address as usize,
            2,
            buf.as_mut_ptr().cast(),
            core::mem::size_of_val(&buf),
            &mut ret,
        )
    };
    if status != STATUS_SUCCESS {
        return fail(status, 0);
    }
    // SAFETY: the UNICODE_STRING the kernel wrote at the buffer's start.
    let us = unsafe { &*buf.as_ptr().cast::<UnicodeString>() };
    let length = usize::from(us.length) / 2;
    let text = us.buffer;
    let n = length.min(size as usize - 1);
    // SAFETY: the string the kernel wrote into `buf`; the caller's buffer.
    unsafe {
        core::ptr::copy_nonoverlapping(text, name, n);
        *name.add(n) = 0;
    }
    n as u32
}

// ---- Application restart (Windows Error Reporting's restart manager) ----

/// The registered restart command line and flags (one per process).
struct Restart {
    lock: core::sync::atomic::AtomicBool,
    state: core::cell::UnsafeCell<(usize, u32, [u16; RESTART_MAX_CMD_LINE])>,
}

// SAFETY: every access is under `lock`.
unsafe impl Sync for Restart {}

const RESTART_MAX_CMD_LINE: usize = 1024;

static RESTART: Restart = Restart {
    lock: core::sync::atomic::AtomicBool::new(false),
    state: core::cell::UnsafeCell::new((0, 0, [0; RESTART_MAX_CMD_LINE])),
};

fn with_restart<R>(f: impl FnOnce(&mut (usize, u32, [u16; RESTART_MAX_CMD_LINE])) -> R) -> R {
    use core::sync::atomic::Ordering;
    while RESTART.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err()
    {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = f(unsafe { &mut *RESTART.state.get() });
    RESTART.lock.store(false, Ordering::Release);
    r
}

/// RegisterApplicationRestart(command line, flags): the arguments to
/// restart the program with after a crash, hang, patch or reboot (the
/// RESTART_NO_* flags exclude cases). E_INVALIDARG for a command line of
/// RESTART_MAX_CMD_LINE characters or more, or unknown flags. gala keeps
/// the registration (GetApplicationRestartSettings reads it back) but
/// has no restart manager: nothing restarts the program yet.
#[unsafe(no_mangle)]
pub extern "system" fn RegisterApplicationRestart(command: *const u16, flags: u32) -> i32 {
    const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;
    if flags & !0xf != 0 {
        return E_INVALIDARG;
    }
    let mut n = 0;
    if !command.is_null() {
        // SAFETY: the caller's NUL-terminated command line.
        while unsafe { *command.add(n) } != 0 {
            n += 1;
            if n >= RESTART_MAX_CMD_LINE {
                return E_INVALIDARG;
            }
        }
    }
    with_restart(|state| {
        if n > 0 {
            // SAFETY: the caller's `n` characters.
            unsafe { core::ptr::copy_nonoverlapping(command, state.2.as_mut_ptr(), n) };
        }
        state.0 = n + 1; // registered (the stored length with the NUL)
        state.1 = flags;
        state.2[n] = 0;
    });
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn UnregisterApplicationRestart() -> i32 {
    with_restart(|state| state.0 = 0);
    0
}

/// GetApplicationRestartSettings(process, buffer, *size, *flags): the
/// current process's registration; HRESULT_FROM_WIN32(ERROR_NOT_FOUND)
/// without one, HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER) (and the
/// size needed) for a short buffer. Other processes: E_NOTIMPL, reported.
#[unsafe(no_mangle)]
pub extern "system" fn GetApplicationRestartSettings(
    process: Handle,
    buffer: *mut u16,
    size: *mut u32,
    flags: *mut u32,
) -> i32 {
    const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;
    const NOT_FOUND: i32 = 0x8007_0490_u32 as i32;
    const INSUFFICIENT_BUFFER: i32 = 0x8007_007a_u32 as i32;
    if size.is_null() {
        return E_INVALIDARG;
    }
    if process as isize != -1 && crate::process::GetProcessId(process) != GetCurrentProcessId() {
        unsupported(b"kernel32.dll!GetApplicationRestartSettings(another process)", 0);
        return 0x8000_4001_u32 as i32;
    }
    with_restart(|state| {
        if state.0 == 0 {
            return NOT_FOUND;
        }
        // SAFETY: the caller's outputs.
        unsafe {
            let room = *size as usize;
            *size = state.0 as u32;
            if !flags.is_null() {
                *flags = state.1;
            }
            if buffer.is_null() || room < state.0 {
                return INSUFFICIENT_BUFFER;
            }
            core::ptr::copy_nonoverlapping(state.2.as_ptr(), buffer, state.0);
        }
        0
    })
}

// ---- Process snapshots (processsnapshot.h: PssCaptureSnapshot and friends) ----
//
// A snapshot here holds the process's PSS_PROCESS_INFORMATION, captured
// from the kernel's process queries: the one class CPython reads (its
// os.getppid). Clones of the address space, handle and thread lists
// (the other PSS_CAPTURE flags) are not taken: those flags are recorded
// in the ledger and their query classes answer ERROR_NOT_FOUND.

/// sizeof(PSS_PROCESS_INFORMATION) on x64: ExitStatus, PebBaseAddress,
/// AffinityMask, BasePriority, ProcessId (28), ParentProcessId (32),
/// Flags, the four times (40..72), PriorityClass (72), the VM counters
/// (80..176), ExecuteFlags (176), ImageFileName[MAX_PATH] (180).
#[cfg(target_arch = "x86_64")]
const PSS_PROCESS_INFORMATION_SIZE: usize = 704;
const PSS_QUERY_PROCESS_INFORMATION: u32 = 0;

/// PssCaptureSnapshot(ProcessHandle, CaptureFlags, ThreadContextFlags,
/// *SnapshotHandle): the process's information, now.
#[unsafe(no_mangle)]
pub extern "system" fn PssCaptureSnapshot(
    process: Handle,
    flags: u32,
    _context_flags: u32,
    snapshot: *mut Handle,
) -> u32 {
    #[cfg(target_arch = "x86_64")]
    {
        if snapshot.is_null() {
            return ERROR_INVALID_PARAMETER;
        }
        // PSS_CAPTURE_VA_CLONE and the other capture flags (handles,
        // threads, the address space) are not taken.
        if flags != 0 {
            unsupported(b"kernel32.dll!PssCaptureSnapshot(capture flags)", flags.into());
        }
        let mut info = [0u8; PSS_PROCESS_INFORMATION_SIZE];
        if let Err(e) = capture(process, &mut info) {
            return e;
        }
        let block =
            super::heap::HeapAlloc(super::heap::GetProcessHeap(), 0, info.len()).cast::<u8>();
        if block.is_null() {
            return ERROR_NOT_ENOUGH_MEMORY;
        }
        // SAFETY: a fresh block of the size; the caller's output.
        unsafe {
            core::ptr::copy_nonoverlapping(info.as_ptr(), block, info.len());
            *snapshot = block.cast();
        }
        0
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = (process, flags, snapshot);
        unsupported(b"kernel32.dll!PssCaptureSnapshot(32-bit)", 0);
        ERROR_NOT_SUPPORTED
    }
}

/// PSS_PROCESS_INFORMATION of `process` from ProcessBasicInformation,
/// ProcessTimes, ProcessVmCounters, its priority class and (for this
/// process) its image's path.
#[cfg(target_arch = "x86_64")]
fn capture(process: Handle, info: &mut [u8; PSS_PROCESS_INFORMATION_SIZE]) -> Result<(), u32> {
    let query = |class: u32, buf: &mut [u8]| -> Result<(), u32> {
        // SAFETY: our buffer of its size.
        let status = unsafe {
            NtQueryInformationProcess(
                process as isize,
                class,
                buf.as_mut_ptr(),
                buf.len() as u32,
                core::ptr::null_mut(),
            )
        };
        if status != STATUS_SUCCESS {
            // SAFETY: plain call.
            return Err(unsafe { RtlNtStatusToDosError(status) });
        }
        Ok(())
    };
    // PROCESS_BASIC_INFORMATION: ExitStatus, PebBaseAddress, AffinityMask,
    // BasePriority, UniqueProcessId, InheritedFromUniqueProcessId.
    let mut basic = [0u8; 48];
    query(0, &mut basic)?;
    let field =
        |b: &[u8], at: usize| u64::from_le_bytes(b[at..at + 8].try_into().unwrap_or([0; 8]));
    info[0..4].copy_from_slice(&basic[0..4]);
    info[8..16].copy_from_slice(&basic[8..16]);
    info[16..24].copy_from_slice(&basic[16..24]);
    info[24..28].copy_from_slice(&basic[24..28]);
    info[28..32].copy_from_slice(&(field(&basic, 32) as u32).to_le_bytes());
    info[32..36].copy_from_slice(&(field(&basic, 40) as u32).to_le_bytes());
    // KERNEL_USER_TIMES: CreateTime, ExitTime, KernelTime, UserTime.
    let mut times = [0u8; 32];
    if query(4, &mut times).is_ok() {
        info[40..72].copy_from_slice(&times);
    }
    info[72..76].copy_from_slice(&super::runtime::GetPriorityClass(process).to_le_bytes());
    // VM_COUNTERS_EX (96 bytes): PeakVirtualSize .. PrivateUsage, laid out
    // as PSS_PROCESS_INFORMATION has them from PeakVirtualSize on.
    let mut vm = [0u8; 96];
    if query(3, &mut vm).is_ok() {
        info[80..176].copy_from_slice(&vm);
    }
    if process == GetCurrentProcess()
        || super::process::GetProcessId(process) == GetCurrentProcessId()
    {
        let name = &mut info[180..180 + 520];
        let mut path = [0u16; 260];
        let n = super::process::GetModuleFileNameW(core::ptr::null_mut(), path.as_mut_ptr(), 260)
            as usize;
        for (i, c) in path[..n.min(259)].iter().enumerate() {
            name[2 * i..2 * i + 2].copy_from_slice(&c.to_le_bytes());
        }
    }
    Ok(())
}

/// PssQuerySnapshot(SnapshotHandle, InformationClass, Buffer,
/// BufferLength): PSS_QUERY_PROCESS_INFORMATION (the length must be the
/// structure's); other classes were not captured (ERROR_NOT_FOUND).
#[unsafe(no_mangle)]
pub extern "system" fn PssQuerySnapshot(
    snapshot: Handle,
    class: u32,
    buf: *mut u8,
    len: u32,
) -> u32 {
    #[cfg(target_arch = "x86_64")]
    {
        if snapshot.is_null() || buf.is_null() {
            return ERROR_INVALID_PARAMETER;
        }
        if class != PSS_QUERY_PROCESS_INFORMATION {
            unsupported(b"kernel32.dll!PssQuerySnapshot(class)", class.into());
            return ERROR_NOT_FOUND;
        }
        if len as usize != PSS_PROCESS_INFORMATION_SIZE {
            return ERROR_BAD_LENGTH;
        }
        // SAFETY: our snapshot block; the caller's buffer of `len` bytes.
        unsafe {
            core::ptr::copy_nonoverlapping(snapshot.cast::<u8>(), buf, PSS_PROCESS_INFORMATION_SIZE)
        };
        0
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = (
            snapshot,
            class,
            buf,
            len,
            ERROR_BAD_LENGTH,
            ERROR_NOT_FOUND,
            PSS_QUERY_PROCESS_INFORMATION,
        );
        ERROR_NOT_SUPPORTED
    }
}

/// PssFreeSnapshot(ProcessHandle, SnapshotHandle).
#[unsafe(no_mangle)]
pub extern "system" fn PssFreeSnapshot(_process: Handle, snapshot: Handle) -> u32 {
    if snapshot.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    super::heap::HeapFree(super::heap::GetProcessHeap(), 0, snapshot.cast());
    0
}
