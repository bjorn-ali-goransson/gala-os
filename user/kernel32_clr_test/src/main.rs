//! kernel32 functions the .NET runtime (CoreCLR) imports, with the results
//! real Windows gives (documented contracts) on a machine with two
//! processors in one group:
//!
//! - Waits: WaitForMultipleObjectsEx (any, all), SignalObjectAndWait
//!   (signals, then waits; times out), QueueUserAPC run by an alertable
//!   SleepEx (WAIT_IO_COMPLETION).
//! - Processors: GetActiveProcessorGroupCount, GetCurrentProcessorNumberEx,
//!   GetProcessGroupAffinity, Get/SetThreadGroupAffinity,
//!   SetThreadAffinityMask (the previous mask), Get/SetThreadIdealProcessorEx,
//!   GetNumaProcessorNodeEx, GetLogicalProcessorInformation(Ex) (one core
//!   entry per processor here, sizes, ERROR_INSUFFICIENT_BUFFER first).
//! - Contexts: GetEnabledXStateFeatures (x87 and SSE at least),
//!   InitializeContext (size query, aligned, flags set), CopyContext,
//!   LocateXStateFeature and SetXStateFeaturesMask without CONTEXT_XSTATE.
//! - Code: FlushInstructionCache, RtlInstallFunctionTableCallback (ntdll's
//!   RtlLookupFunctionEntry asks the callback; RtlDeleteFunctionTable),
//!   WerRegisterRuntimeExceptionModule.
//! - Threads and memory: SetThreadStackGuarantee, SetThreadErrorMode,
//!   QueryThreadCycleTime, VirtualLock/VirtualUnlock (ERROR_NOT_LOCKED),
//!   CreateMemoryResourceNotification (memory is not low),
//!   IsWow64Process2 (native x64), IsProcessInJob, named pipes that are
//!   not pipes (ConnectNamedPipe/DisconnectNamedPipe of an event).
//! - API sets: LoadLibrary and GetModuleHandle of api-ms-win-* contract
//!   names (any case, without the extension) reach the implementing DLL;
//!   an unknown contract is ERROR_MOD_NOT_FOUND.
//! - LoadLibraryExW with LOAD_WITH_ALTERED_SEARCH_PATH, AS_DATAFILE and
//!   AS_IMAGE_RESOURCE; RtlDllShutdownInProgress; Set/GetThreadDescription;
//!   GetLocaleInfoW(LOCALE_SPARENT).
//!
//! Prints `kernel32_clr_test: ok` or exits with the number of the failing
//! check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct GroupAffinity {
    mask: usize,
    group: u16,
    reserved: [u16; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ProcessorNumber {
    group: u16,
    number: u8,
    reserved: u8,
}

#[repr(C)]
struct RuntimeFunction {
    begin: u32,
    end: u32,
    unwind: u32,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetLastError() -> u32;
    fn SetLastError(e: u32);
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetCurrentProcess() -> Handle;
    fn GetCurrentThread() -> Handle;
    fn CreateEventW(attrs: *mut c_void, manual: Bool, initial: Bool, name: *const u16) -> Handle;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn WaitForMultipleObjectsEx(
        count: u32,
        handles: *const Handle,
        all: Bool,
        ms: u32,
        alertable: Bool,
    ) -> u32;
    fn SignalObjectAndWait(signal: Handle, wait: Handle, ms: u32, alertable: Bool) -> u32;
    fn QueueUserAPC(f: extern "system" fn(usize), thread: Handle, data: usize) -> u32;
    fn SleepEx(ms: u32, alertable: Bool) -> u32;
    fn GetActiveProcessorGroupCount() -> u16;
    fn GetCurrentProcessorNumberEx(n: *mut ProcessorNumber);
    fn GetProcessGroupAffinity(process: Handle, count: *mut u16, groups: *mut u16) -> Bool;
    fn GetThreadGroupAffinity(thread: Handle, affinity: *mut GroupAffinity) -> Bool;
    fn SetThreadGroupAffinity(
        thread: Handle,
        affinity: *const GroupAffinity,
        previous: *mut GroupAffinity,
    ) -> Bool;
    fn SetThreadAffinityMask(thread: Handle, mask: usize) -> usize;
    fn GetThreadIdealProcessorEx(thread: Handle, n: *mut ProcessorNumber) -> Bool;
    fn SetThreadIdealProcessorEx(
        thread: Handle,
        n: *const ProcessorNumber,
        previous: *mut ProcessorNumber,
    ) -> Bool;
    fn GetNumaProcessorNodeEx(n: *const ProcessorNumber, node: *mut u16) -> Bool;
    fn GetLogicalProcessorInformation(buf: *mut u8, len: *mut u32) -> Bool;
    fn GetLogicalProcessorInformationEx(relation: u32, buf: *mut u8, len: *mut u32) -> Bool;
    fn GetEnabledXStateFeatures() -> u64;
    fn InitializeContext(buf: *mut u8, flags: u32, context: *mut *mut u8, len: *mut u32) -> Bool;
    fn CopyContext(dst: *mut u8, flags: u32, src: *const u8) -> Bool;
    fn LocateXStateFeature(context: *mut u8, feature: u32, len: *mut u32) -> *mut u8;
    fn SetXStateFeaturesMask(context: *mut u8, mask: u64) -> Bool;
    fn FlushInstructionCache(process: Handle, base: *const c_void, size: usize) -> Bool;
    fn RtlInstallFunctionTableCallback(
        id: u64,
        base: u64,
        length: u32,
        callback: extern "system" fn(u64, *mut c_void) -> *const RuntimeFunction,
        context: *mut c_void,
        dll: *const u16,
    ) -> u8;
    fn WerRegisterRuntimeExceptionModule(dll: *const u16, context: *mut c_void) -> i32;
    fn SetThreadStackGuarantee(size: *mut u32) -> Bool;
    fn SetThreadErrorMode(mode: u32, old: *mut u32) -> Bool;
    fn GetThreadErrorMode() -> u32;
    fn QueryThreadCycleTime(thread: Handle, cycles: *mut u64) -> Bool;
    fn VirtualAlloc(base: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut u8;
    fn VirtualLock(base: *mut u8, size: usize) -> Bool;
    fn VirtualUnlock(base: *mut u8, size: usize) -> Bool;
    fn CreateMemoryResourceNotification(kind: u32) -> Handle;
    fn QueryMemoryResourceNotification(h: Handle, state: *mut Bool) -> Bool;
    fn IsWow64Process2(process: Handle, machine: *mut u16, native: *mut u16) -> Bool;
    fn IsProcessInJob(process: Handle, job: Handle, result: *mut Bool) -> Bool;
    fn ConnectNamedPipe(pipe: Handle, overlapped: *mut c_void) -> Bool;
    fn DisconnectNamedPipe(pipe: Handle) -> Bool;
    fn LoadLibraryA(name: *const u8) -> Handle;
    fn GetModuleHandleA(name: *const u8) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
    fn LoadLibraryExW(name: *const u16, file: Handle, flags: u32) -> Handle;
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn FreeLibrary(module: Handle) -> Bool;
    fn FindResourceW(module: Handle, name: *const u16, kind: *const u16) -> Handle;
    fn SetThreadDescription(thread: Handle, text: *const u16) -> i32;
    fn GetThreadDescription(thread: Handle, text: *mut *mut u16) -> i32;
    fn LocalFree(p: *mut c_void) -> *mut c_void;
    fn GetLocaleInfoW(lcid: u32, kind: u32, buf: *mut u16, size: i32) -> i32;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn RtlLookupFunctionEntry(
        pc: u64,
        base: *mut u64,
        history: *mut c_void,
    ) -> *const RuntimeFunction;
    fn RtlDeleteFunctionTable(table: *const c_void) -> u8;
    fn RtlDllShutdownInProgress() -> u8;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const WAIT_TIMEOUT: u64 = 0x102;
const WAIT_IO_COMPLETION: u64 = 0xc0;
const ERROR_INSUFFICIENT_BUFFER: u64 = 122;
const ERROR_NOT_LOCKED: u64 = 158;
const CONTEXT_FULL: u32 = 0x10_000b;

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

fn hex(v: u32) -> [u8; 8] {
    let mut s = [0u8; 8];
    for (i, c) in s.iter_mut().enumerate() {
        let d = (v >> (28 - 4 * i)) & 0xf;
        *c = if d < 10 { b'0' + d as u8 } else { b'a' + d as u8 - 10 };
    }
    s
}

struct Checks(u32);

impl Checks {
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"kernel32_clr_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

static mut APC_DATA: usize = 0;

extern "system" fn apc(data: usize) {
    // SAFETY: only this thread touches it.
    unsafe { APC_DATA = data };
}

fn waits(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain calls with handles and buffers of ours.
    unsafe {
        let null = core::ptr::null_mut();
        let a = CreateEventW(null, 1, 0, core::ptr::null());
        let b = CreateEventW(null, 1, 1, core::ptr::null());
        let both = [a, b];
        c.eq(WaitForMultipleObjectsEx(2, both.as_ptr(), 0, 0, 0).into(), 1)?;
        c.eq(WaitForMultipleObjectsEx(2, both.as_ptr(), 1, 0, 0).into(), WAIT_TIMEOUT)?;
        // Signals a, then waits for b (signaled).
        c.eq(SignalObjectAndWait(a, b, 0, 0).into(), 0)?;
        c.eq(WaitForSingleObject(a, 0).into(), 0)?;
        let d = CreateEventW(null, 1, 0, core::ptr::null());
        c.eq(SignalObjectAndWait(b, d, 0, 0).into(), WAIT_TIMEOUT)?;
        // An APC to this thread runs in an alertable wait.
        c.ok(QueueUserAPC(apc, GetCurrentThread(), 7) != 0, GetLastError().into())?;
        c.eq(SleepEx(0, 1).into(), WAIT_IO_COMPLETION)?;
        c.eq(APC_DATA as u64, 7)?;
        CloseHandle(a);
        CloseHandle(b);
        CloseHandle(d);
    }
    Ok(())
}

fn processors(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain calls with buffers of ours.
    unsafe {
        let me = GetCurrentThread();
        c.eq(GetActiveProcessorGroupCount().into(), 1)?;
        let mut n = ProcessorNumber { group: 9, number: 9, reserved: 9 };
        GetCurrentProcessorNumberEx(&mut n);
        c.ok(n.group == 0 && n.number < 2, n.number.into())?;
        let mut groups = [7u16; 4];
        let mut count = 4u16;
        c.ok(
            GetProcessGroupAffinity(GetCurrentProcess(), &mut count, groups.as_mut_ptr()) != 0,
            0,
        )?;
        c.ok(count == 1 && groups[0] == 0, count.into())?;
        let mut ga = GroupAffinity::default();
        c.ok(GetThreadGroupAffinity(me, &mut ga) != 0, GetLastError().into())?;
        c.ok(ga.mask == 3 && ga.group == 0, ga.mask as u64)?;
        let one = GroupAffinity { mask: 1, ..GroupAffinity::default() };
        let mut previous = GroupAffinity::default();
        c.ok(SetThreadGroupAffinity(me, &one, &mut previous) != 0, GetLastError().into())?;
        c.eq(previous.mask as u64, 3)?;
        c.eq(SetThreadAffinityMask(me, 3) as u64, 1)?;
        // Not a subset of the process's processors.
        c.eq(SetThreadAffinityMask(me, 4) as u64, 0)?;
        c.eq(GetLastError().into(), 87)?; // ERROR_INVALID_PARAMETER
        let mut ideal = ProcessorNumber::default();
        c.ok(GetThreadIdealProcessorEx(me, &mut ideal) != 0, 0)?;
        c.ok(ideal.group == 0 && ideal.number < 2, ideal.number.into())?;
        let zero = ProcessorNumber::default();
        c.ok(SetThreadIdealProcessorEx(me, &zero, &mut ideal) != 0, GetLastError().into())?;
        let mut node = 9u16;
        c.ok(GetNumaProcessorNodeEx(&zero, &mut node) != 0 && node == 0, node.into())?;

        // SYSTEM_LOGICAL_PROCESSOR_INFORMATION: 32 bytes each.
        let mut buf = [0u64; 512];
        let mut len = 0u32;
        c.eq(GetLogicalProcessorInformation(core::ptr::null_mut(), &mut len) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_INSUFFICIENT_BUFFER)?;
        c.ok(len > 0 && len.is_multiple_of(32) && len <= 4096, len.into())?;
        c.ok(GetLogicalProcessorInformation(buf.as_mut_ptr().cast(), &mut len) != 0, 0)?;
        let (mut cores, mut mask) = (0, 0u64);
        for e in buf[..len as usize / 8].chunks(4) {
            if e[1] as u32 == 0 {
                cores += 1; // RelationProcessorCore
                mask |= e[0];
            }
        }
        c.ok(cores == 2 && mask == 3, cores)?;

        // The Ex form: variable-sized entries, RelationProcessorCore (0)
        // ones are 48 bytes with one GROUP_AFFINITY.
        let mut len = 0u32;
        c.eq(GetLogicalProcessorInformationEx(0, core::ptr::null_mut(), &mut len) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_INSUFFICIENT_BUFFER)?;
        c.eq(len.into(), 2 * 48)?;
        let p = buf.as_mut_ptr().cast::<u8>();
        c.ok(GetLogicalProcessorInformationEx(0, p, &mut len) != 0, 0)?;
        let mut mask = 0u64;
        let mut at = 0;
        while at < len as usize {
            let relation = *(p.add(at) as *const u32);
            let size = *(p.add(at + 4) as *const u32);
            c.ok(relation == 0 && size == 48, size.into())?;
            c.eq((*(p.add(at + 30) as *const u16)).into(), 1)?; // GroupCount
            mask |= *(p.add(at + 32) as *const u64);
            at += size as usize;
        }
        c.eq(mask, 3)?;
        // RelationAll (0xffff): the entries' sizes add up.
        let mut len = 4096u32;
        c.ok(GetLogicalProcessorInformationEx(0xffff, p, &mut len) != 0, 0)?;
        let (mut at, mut cores) = (0, 0);
        while at < len as usize {
            if *(p.add(at) as *const u32) == 0 {
                cores += 1;
            }
            let size = *(p.add(at + 4) as *const u32) as usize;
            c.ok(size >= 8, size as u64)?;
            at += size;
        }
        c.ok(at == len as usize && cores == 2, cores)?;
    }
    Ok(())
}

fn contexts(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain calls with buffers of ours.
    unsafe {
        c.eq(GetEnabledXStateFeatures() & 3, 3)?;
        let mut len = 0u32;
        let mut ctx = core::ptr::null_mut();
        c.eq(InitializeContext(core::ptr::null_mut(), CONTEXT_FULL, &mut ctx, &mut len) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_INSUFFICIENT_BUFFER)?;
        c.ok((1232..=4096).contains(&len), len.into())?;
        let mut a = [0u64; 512];
        let mut b = [0u64; 512];
        let mut len = 4096u32;
        c.ok(InitializeContext(a.as_mut_ptr().cast(), CONTEXT_FULL, &mut ctx, &mut len) != 0, 0)?;
        c.ok((ctx as usize).is_multiple_of(16) && ctx as usize >= a.as_ptr() as usize, ctx as u64)?;
        c.eq((*(ctx.add(0x30) as *const u32)).into(), CONTEXT_FULL.into())?; // ContextFlags
        *(ctx.add(0x78) as *mut u64) = 0x1234; // Rax
        let mut other = core::ptr::null_mut();
        let mut len = 4096u32;
        InitializeContext(b.as_mut_ptr().cast(), CONTEXT_FULL, &mut other, &mut len);
        c.ok(CopyContext(other, CONTEXT_FULL, ctx) != 0, GetLastError().into())?;
        c.eq(*(other.add(0x78) as *const u64), 0x1234)?;
        // No extended state in this context.
        let mut flen = 0u32;
        c.ok(LocateXStateFeature(ctx, 2, &mut flen).is_null(), 0)?; // XSTATE_AVX
        c.eq(SetXStateFeaturesMask(ctx, 4) as u64, 0)?;
    }
    Ok(())
}

static FUNCTION: RuntimeFunction = RuntimeFunction { begin: 0x100, end: 0x200, unwind: 0x300 };
static mut CALLBACK_PC: u64 = 0;

extern "system" fn table_callback(pc: u64, context: *mut c_void) -> *const RuntimeFunction {
    // SAFETY: only this thread touches it.
    unsafe { CALLBACK_PC = pc };
    if context as usize == 0x5a { &FUNCTION } else { core::ptr::null() }
}

fn code(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain calls; the range registered is never executed.
    unsafe {
        c.ok(FlushInstructionCache(GetCurrentProcess(), core::ptr::null(), 0) != 0, 0)?;
        let base = 0x0000_7a00_0000_0000u64;
        let id = base | 3; // the low two bits of a callback table's id are set
        c.eq(
            RtlInstallFunctionTableCallback(
                id,
                base,
                0x1000,
                table_callback,
                0x5a as _,
                core::ptr::null(),
            )
            .into(),
            1,
        )?;
        let mut image = 0u64;
        let f = RtlLookupFunctionEntry(base + 0x180, &mut image, core::ptr::null_mut());
        c.ok(core::ptr::eq(f, &FUNCTION), f as u64)?;
        c.eq(image, base)?;
        c.eq(CALLBACK_PC, base + 0x180)?;
        // Outside the range: not asked.
        let f = RtlLookupFunctionEntry(base + 0x1000, &mut image, core::ptr::null_mut());
        c.ok(f.is_null(), 0)?;
        c.eq(RtlDeleteFunctionTable(id as *const c_void).into(), 1)?;
        let f = RtlLookupFunctionEntry(base + 0x180, &mut image, core::ptr::null_mut());
        c.ok(f.is_null(), 0)?;
        // An id without the low bits set is refused.
        c.eq(
            RtlInstallFunctionTableCallback(
                base,
                base,
                0x1000,
                table_callback,
                0x5a as _,
                core::ptr::null(),
            )
            .into(),
            0,
        )?;

        let dll = [b'x' as u16, 0];
        c.eq(WerRegisterRuntimeExceptionModule(dll.as_ptr(), core::ptr::null_mut()) as u64, 0)?;
    }
    Ok(())
}

fn threads_memory(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain calls with buffers of ours.
    unsafe {
        let mut size = 0x10000u32;
        c.ok(SetThreadStackGuarantee(&mut size) != 0, GetLastError().into())?;
        let mut size = 0u32;
        c.ok(SetThreadStackGuarantee(&mut size) != 0, 0)?;
        c.eq(size.into(), 0x10000)?;

        let mut old = 7u32;
        c.ok(SetThreadErrorMode(1, &mut old) != 0, GetLastError().into())?; // SEM_FAILCRITICALERRORS
        c.eq(GetThreadErrorMode().into(), 1)?;
        SetThreadErrorMode(old, core::ptr::null_mut());
        // Not a thread error mode.
        c.eq(SetThreadErrorMode(4, core::ptr::null_mut()) as u64, 0)?; // SEM_NOALIGNMENTFAULTEXCEPT
        c.eq(GetLastError().into(), 87)?;

        let mut cycles = 0u64;
        c.ok(QueryThreadCycleTime(GetCurrentThread(), &mut cycles) != 0 && cycles > 0, 0)?;

        let page = VirtualAlloc(core::ptr::null_mut(), 4096, 0x3000, 4);
        c.ok(!page.is_null(), 0)?;
        *page = 1;
        SetLastError(0);
        c.eq(VirtualUnlock(page, 4096) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_NOT_LOCKED)?;
        c.ok(VirtualLock(page, 4096) != 0, GetLastError().into())?;
        c.ok(VirtualUnlock(page, 4096) != 0, GetLastError().into())?;

        let low = CreateMemoryResourceNotification(0); // LowMemoryResourceNotification
        c.ok(!low.is_null(), GetLastError().into())?;
        let mut state = 7;
        c.ok(QueryMemoryResourceNotification(low, &mut state) != 0 && state == 0, state as u64)?;
        c.eq(WaitForSingleObject(low, 0).into(), WAIT_TIMEOUT)?;
        CloseHandle(low);

        let (mut machine, mut native) = (7u16, 7u16);
        c.ok(IsWow64Process2(GetCurrentProcess(), &mut machine, &mut native) != 0, 0)?;
        c.ok(machine == 0 && native == 0x8664, native.into())?; // IMAGE_FILE_MACHINE_AMD64
        let mut in_job = 7;
        c.ok(IsProcessInJob(GetCurrentProcess(), core::ptr::null_mut(), &mut in_job) != 0, 0)?;
        c.ok(in_job == 0 || in_job == 1, in_job as u64)?;

        // An event is not a pipe.
        let e = CreateEventW(core::ptr::null_mut(), 1, 0, core::ptr::null());
        c.eq(ConnectNamedPipe(e, core::ptr::null_mut()) as u64, 0)?;
        c.eq(DisconnectNamedPipe(e) as u64, 0)?;
        CloseHandle(e);
    }
    Ok(())
}

/// API set names (what the UCRT and CoreCLR load and probe): resolved to
/// the DLLs implementing them, whatever files exist; an unknown contract
/// is not found (ERROR_MOD_NOT_FOUND).
fn api_sets(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain calls with NUL-terminated names.
    unsafe {
        let h = LoadLibraryA(c"api-ms-win-core-synch-l1-2-0.dll".as_ptr().cast());
        c.ok(!h.is_null(), GetLastError().into())?;
        c.ok(!GetProcAddress(h, c"InitializeCriticalSectionEx".as_ptr().cast()).is_null(), 0)?;
        c.ok(!GetModuleHandleA(c"api-ms-win-core-synch-l1-2-0.dll".as_ptr().cast()).is_null(), 0)?;
        let h = LoadLibraryA(c"API-MS-Win-Core-ProcessThreads-L1-1-0".as_ptr().cast());
        c.ok(
            !h.is_null() && !GetProcAddress(h, c"GetCurrentThreadId".as_ptr().cast()).is_null(),
            0,
        )?;
        let h = LoadLibraryA(c"api-ms-win-gala-no-such-contract-l1-1-0.dll".as_ptr().cast());
        c.ok(h.is_null(), 0)?;
        c.eq(GetLastError().into(), 126)?; // ERROR_MOD_NOT_FOUND
    }
    Ok(())
}

/// UTF-16 of an ASCII string, NUL-terminated.
fn w(s: &str) -> [u16; 64] {
    let mut b = [0u16; 64];
    for (d, ch) in b.iter_mut().zip(s.bytes()) {
        *d = ch.into();
    }
    b
}

/// The NUL-terminated UTF-16 string at `p` is `want`.
fn same_text(p: *const u16, want: &str) -> bool {
    // SAFETY: a NUL-terminated string the system wrote.
    unsafe {
        want.bytes().enumerate().all(|(i, b)| *p.add(i) == u16::from(b)) && *p.add(want.len()) == 0
    }
}

/// LoadLibraryExW's flags: LOAD_WITH_ALTERED_SEARCH_PATH loads a module;
/// LOAD_LIBRARY_AS_DATAFILE and AS_IMAGE_RESOURCE map the file without
/// loading it (handles tagged in their low bits, not module handles, no
/// DllMain); FreeLibrary unmaps them. Thread descriptions, the parent
/// locale and RtlDllShutdownInProgress.
fn libraries(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain calls with NUL-terminated names.
    unsafe {
        let path = w("C:\\gala\\testdll.dll");
        let name = w("testdll.dll");
        let data = LoadLibraryExW(path.as_ptr(), core::ptr::null_mut(), 0x2); // AS_DATAFILE
        c.ok(!data.is_null() && data as usize & 3 == 1, data as u64)?;
        c.ok(GetModuleHandleW(name.as_ptr()).is_null(), 0)?;
        // MAKEINTRESOURCE(1) of RT_VERSION (16).
        let (one, version) = (core::ptr::without_provenance(1), core::ptr::without_provenance(16));
        c.ok(FindResourceW(data, one, version).is_null(), 0)?;
        c.eq(GetLastError().into(), 1812)?; // ERROR_RESOURCE_DATA_NOT_FOUND
        c.ok(FreeLibrary(data) != 0, GetLastError().into())?;
        let image = LoadLibraryExW(path.as_ptr(), core::ptr::null_mut(), 0x20); // AS_IMAGE_RESOURCE
        c.ok(!image.is_null() && image as usize & 3 == 2, image as u64)?;
        c.ok(FreeLibrary(image) != 0, GetLastError().into())?;
        let module = LoadLibraryExW(path.as_ptr(), core::ptr::null_mut(), 0x8); // ALTERED_SEARCH_PATH
        c.ok(!module.is_null() && module == GetModuleHandleW(name.as_ptr()), module as u64)?;
        let attached = GetProcAddress(module, c"GALA_ATTACH_COUNT".as_ptr().cast()) as *const u64;
        c.ok(!attached.is_null() && *attached == 1, 0)?;
        c.ok(FreeLibrary(module) != 0, 0)?;

        c.eq(RtlDllShutdownInProgress().into(), 0)?;

        let gala = w("gala worker");
        c.eq(SetThreadDescription(GetCurrentThread(), gala.as_ptr()) as u64, 0)?;
        let mut text = core::ptr::null_mut();
        c.eq(GetThreadDescription(GetCurrentThread(), &mut text) as u64, 0)?;
        c.ok(!text.is_null() && same_text(text, "gala worker"), 0)?;
        LocalFree(text.cast());

        let mut buf = [0u16; 16];
        c.eq(GetLocaleInfoW(0x409, 0x6d, buf.as_mut_ptr(), 16) as u64, 3)?; // LOCALE_SPARENT
        c.ok(same_text(buf.as_ptr(), "en"), 0)?;
        // LOCALE_INEUTRAL as a number: en-US is a specific locale (0).
        let mut number = [7u16; 2];
        c.eq(GetLocaleInfoW(0x409, 0x71 | 0x2000_0000, number.as_mut_ptr(), 2) as u64, 2)?;
        c.ok(number == [0, 0], number[0].into())?;
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    waits(&mut c)?;
    processors(&mut c)?;
    contexts(&mut c)?;
    code(&mut c)?;
    threads_memory(&mut c)?;
    api_sets(&mut c)?;
    libraries(&mut c)
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"kernel32_clr_test: ok\r\n");
            0
        }
        Err(id) => id,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
