//! A minimal stand-in for ntdll.dll, for testing the kernel's side of
//! process startup (written from the documented contract, Windows
//! Internals): the initial thread of a process starts at
//! `LdrInitializeThunk(Context, NtdllBase)`, where `Context` holds the
//! thread's real start; once user-mode initialization is done, the thunk
//! resumes that context with `NtContinue`.
//!
//! This one only records that it ran (`GalaTestNtdllState`, which
//! `startup_test` reads) and continues.

#![no_std]
#![allow(non_snake_case, non_upper_case_globals)]

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

/// Export `name` (or `ordinal` if `name` is empty) of the image at `base`.
fn export(base: usize, name: &[u8], ordinal: u32) -> Option<usize> {
    let (rva, _) = directory(base, 0);
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
    // TODO: forwarders (an RVA inside the export directory).
    Some(base + rd32(functions + 4 * index) as usize)
}

/// Maps `dll` (an ASCII file name) from the directory of the executable
/// (`ImagePathName`) and returns its base.
fn map_dll(dll: &[u8]) -> Result<usize, u32> {
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
    let mut path = [0u16; 300];
    let prefix = *b"\\??\\";
    let total = prefix.len() + dir_len + dll.len();
    if total > path.len() {
        return Err(STATUS_DLL_NOT_FOUND);
    }
    for (i, &b) in prefix.iter().enumerate() {
        path[i] = u16::from(b);
    }
    path[prefix.len()..prefix.len() + dir_len].copy_from_slice(&image_path[..dir_len]);
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
        STATUS_SUCCESS | STATUS_IMAGE_NOT_AT_BASE => Ok(base),
        _ => Err(STATUS_INVALID_IMAGE_FORMAT),
    }
}

/// Binds the imports of the image at `base`: maps each DLL, fills the IAT
/// and runs the DLL's entry point (DLL_PROCESS_ATTACH).
// TODO: the DLLs' own imports, a module list (PEB->Ldr), TLS, ordering.
fn bind_imports(base: usize) -> Result<(), u32> {
    let (rva, _) = directory(base, 1);
    if rva == 0 {
        return Ok(());
    }
    let mut desc = base + rva;
    while rd32(desc + 12) != 0 {
        let dll = map_dll(c_str(base + rd32(desc + 12) as usize))?;
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
            let target = if entry >> 63 != 0 {
                export(dll, &[], (entry & 0xffff) as u32)
            } else {
                export(dll, c_str(base + (entry as usize & 0x7fff_ffff) + 2), 0)
            };
            let target = target.ok_or(STATUS_ENTRYPOINT_NOT_FOUND)?;
            // SAFETY: the IAT slot, made writable above.
            unsafe { ((iat + 8 * i) as *mut u64).write_unaligned(target as u64) };
        }
        let (mut at, mut size, mut ignored) = (iat, count * 8, 0u32);
        NtProtectVirtualMemory(-1, &mut at, &mut size, old, &mut ignored);
        let entry_rva = rd32(dll + rd32(dll + 0x3c) as usize + 24 + 16) as usize;
        if entry_rva != 0 {
            // SAFETY: the DLL's entry point (DllMain signature).
            let entry: extern "system" fn(usize, u32, usize) -> i32 =
                unsafe { core::mem::transmute(dll + entry_rva) };
            entry(dll, 1, 0); // DLL_PROCESS_ATTACH
        }
        desc += 20;
    }
    Ok(())
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
    if let Err(status) = bind_imports(image) {
        NtTerminateProcess(-1, status);
    }
    NtContinue(context, 0);
    // NtContinue only returns on failure.
    loop {
        core::hint::spin_loop();
    }
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

/// Registered vectored handlers in call order, as (cookie, handler), and
/// the next cookie.
// TODO: a lock once processes have more than one thread using this.
struct Handlers(core::cell::UnsafeCell<([(usize, usize); MAX_HANDLERS], usize)>);

// SAFETY: only used single-threaded so far (see the TODO above).
unsafe impl Sync for Handlers {}

static HANDLERS: Handlers = Handlers(core::cell::UnsafeCell::new(([(0, 0); MAX_HANDLERS], 1)));

/// RtlAddVectoredExceptionHandler(First, Handler): the cookie, or null if
/// the table is full.
#[unsafe(no_mangle)]
pub extern "system" fn RtlAddVectoredExceptionHandler(
    first: u32,
    handler: VectoredHandler,
) -> *mut core::ffi::c_void {
    // SAFETY: single-threaded use (see HANDLERS).
    unsafe {
        let (table, next) = &mut *HANDLERS.0.get();
        let used = table.iter().take_while(|e| e.0 != 0).count();
        if used == MAX_HANDLERS {
            return core::ptr::null_mut();
        }
        let cookie = *next;
        *next += 1;
        let entry = (cookie, handler as usize);
        if first != 0 {
            table.copy_within(0..used, 1);
            table[0] = entry;
        } else {
            table[used] = entry;
        }
        cookie as *mut core::ffi::c_void
    }
}

/// RtlRemoveVectoredExceptionHandler(Handle): nonzero if it was registered.
#[unsafe(no_mangle)]
pub extern "system" fn RtlRemoveVectoredExceptionHandler(cookie: *mut core::ffi::c_void) -> u32 {
    // SAFETY: single-threaded use (see HANDLERS).
    unsafe {
        let (table, _) = &mut *HANDLERS.0.get();
        let Some(i) = table.iter().position(|e| e.0 != 0 && e.0 == cookie as usize) else {
            return 0;
        };
        table.copy_within(i + 1.., i);
        table[MAX_HANDLERS - 1] = (0, 0);
        1
    }
}

/// Where the kernel puts the EXCEPTION_RECORD relative to the CONTEXT at
/// rsp (gala's dispatcher frame; to be checked against the real layout).
const RECORD_OFFSET: usize = 0x4f0;

/// The kernel resumes a thread here after a user-mode exception, with rsp
/// pointing at the CONTEXT of the fault and the EXCEPTION_RECORD above it.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "system" fn KiUserExceptionDispatcher() -> ! {
    core::arch::naked_asm!(
        "cld",
        "mov rdx, rsp",
        "lea rcx, [rsp + {record}]",
        "sub rsp, 0x20",
        "call {dispatch}",
        "int3",
        record = const RECORD_OFFSET,
        dispatch = sym dispatch_exception,
    )
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

/// Calls the vectored handlers; resumes the context if one continues
/// execution, else ends the process with the exception code.
extern "system" fn dispatch_exception(record: *mut u8, context: *mut u8) -> ! {
    let mut pointers = ExceptionPointers { record, context };
    // SAFETY: single-threaded use (see HANDLERS); handlers were registered
    // as `VectoredHandler`s.
    let table = unsafe { (*HANDLERS.0.get()).0 };
    for &(cookie, raw) in &table {
        if cookie == 0 {
            break;
        }
        let handler: VectoredHandler = unsafe { core::mem::transmute(raw) };
        if handler(&mut pointers) == EXCEPTION_CONTINUE_EXECUTION {
            NtContinue(context, 0);
        }
    }
    // SAFETY: the record starts with its ExceptionCode.
    let code = unsafe { (record as *const u32).read_unaligned() };
    NtTerminateProcess(-1, code);
    loop {
        core::hint::spin_loop();
    }
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
