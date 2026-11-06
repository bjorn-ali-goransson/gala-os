//! The x86 runtime: what 32-bit test programs use (ARCHITECTURE "Plan:
//! WOW64"). Smaller than the x64 one: the system call stubs a 32-bit
//! program needs so far, the 32-bit TEB and PEB, printing and exiting.
//!
//! A 32-bit ntdll's stub, as on 64-bit Windows: the service number in eax,
//! then a call through TEB32.WOW32Reserved (`fs:[0xc0]`), the 32-bit
//! address of the WOW64 layer's transition to 64-bit code; the arguments
//! stay on the 32-bit stack, above the two return addresses. The numbers
//! are the x64 ones (WOW64 converts the arguments and makes the 64-bit
//! call). The stubs are cdecl here (`ret`, the caller pops), which the
//! transition does not see.

use core::arch::{asm, global_asm};
use core::ffi::c_void;

/// A handle value. Opaque to user mode, so an integer rather than a pointer.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle(pub usize);
pub type NtStatus = u32;

pub const STATUS_SUCCESS: NtStatus = 0;
pub const STATUS_INVALID_HANDLE: NtStatus = 0xc000_0008;

/// `NtCurrentProcess()`.
pub const CURRENT_PROCESS: Handle = Handle(usize::MAX);

pub const MEM_COMMIT: u32 = 0x1000;
pub const MEM_RESERVE: u32 = 0x2000;
pub const MEM_RELEASE: u32 = 0x8000;
pub const MEM_IMAGE: u32 = 0x100_0000;
pub const PAGE_READONLY: u32 = 0x02;
pub const PAGE_READWRITE: u32 = 0x04;

/// TEB32 offsets (public x86 layout).
pub mod teb {
    pub const STACK_BASE: usize = 0x04;
    pub const STACK_LIMIT: usize = 0x08;
    pub const SELF: usize = 0x18;
    pub const CLIENT_ID_PROCESS: usize = 0x20;
    pub const CLIENT_ID_THREAD: usize = 0x24;
    pub const PEB: usize = 0x30;
    /// WOW32Reserved: where 32-bit system calls go.
    pub const WOW32_RESERVED: usize = 0xc0;
}

/// PEB32 offsets (public x86 layout).
pub mod peb {
    pub const IMAGE_BASE_ADDRESS: usize = 0x08;
    pub const PROCESS_PARAMETERS: usize = 0x10;
    pub const NUMBER_OF_PROCESSORS: usize = 0x64;
    pub const OS_MAJOR_VERSION: usize = 0xa4;
    pub const OS_MINOR_VERSION: usize = 0xa8;
    pub const OS_BUILD_NUMBER: usize = 0xac;
    pub const OS_PLATFORM_ID: usize = 0xb0;
    pub const IMAGE_SUBSYSTEM: usize = 0xb4;
}

/// RTL_USER_PROCESS_PARAMETERS (x86) offsets.
pub mod params {
    pub const FLAGS: usize = 0x08;
    pub const STANDARD_OUTPUT: usize = 0x1c;
    pub const CURRENT_DIRECTORY: usize = 0x24;
    pub const IMAGE_PATH_NAME: usize = 0x38;
    pub const COMMAND_LINE: usize = 0x40;
    pub const ENVIRONMENT: usize = 0x48;
}

macro_rules! stubs {
    ($($name:ident = $nr:expr;)*) => {
        global_asm!(
            $(
                concat!(".globl _", stringify!($name)),
                concat!("_", stringify!($name), ":"),
                concat!("mov eax, ", stringify!($nr)),
                "call dword ptr fs:[0xc0]",
                "ret",
            )*
        );
    };
}

stubs! {
    ntrt_nt_write_file = 0x0008;
    ntrt_nt_close = 0x000f;
    ntrt_nt_allocate_virtual_memory = 0x0018;
    ntrt_nt_query_information_process = 0x0019;
    ntrt_nt_free_virtual_memory = 0x001e;
    ntrt_nt_query_virtual_memory = 0x0023;
    ntrt_nt_terminate_process = 0x002c;
    ntrt_nt_query_performance_counter = 0x0031;
    ntrt_nt_protect_virtual_memory = 0x0050;
}

unsafe extern "C" {
    fn ntrt_nt_write_file(
        file: Handle,
        event: Handle,
        apc_routine: *mut c_void,
        apc_context: *mut c_void,
        io_status: *mut IoStatusBlock,
        buffer: *const u8,
        length: u32,
        byte_offset: *const i64,
        key: *const u32,
    ) -> NtStatus;
    fn ntrt_nt_close(handle: Handle) -> NtStatus;
    fn ntrt_nt_allocate_virtual_memory(
        process: Handle,
        base: *mut usize,
        zero_bits: usize,
        size: *mut usize,
        kind: u32,
        protect: u32,
    ) -> NtStatus;
    fn ntrt_nt_query_information_process(
        h: Handle,
        class: u32,
        buf: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> NtStatus;
    fn ntrt_nt_free_virtual_memory(
        process: Handle,
        base: *mut usize,
        size: *mut usize,
        kind: u32,
    ) -> NtStatus;
    fn ntrt_nt_query_virtual_memory(
        process: Handle,
        base: usize,
        class: u32,
        buf: *mut u8,
        len: usize,
        ret: *mut usize,
    ) -> NtStatus;
    fn ntrt_nt_terminate_process(process: Handle, status: NtStatus) -> NtStatus;
    fn ntrt_nt_query_performance_counter(counter: *mut i64, frequency: *mut i64) -> NtStatus;
    fn ntrt_nt_protect_virtual_memory(
        process: Handle,
        base: *mut usize,
        size: *mut usize,
        new: u32,
        old: *mut u32,
    ) -> NtStatus;
}

/// IO_STATUS_BLOCK (x86: 8 bytes).
#[repr(C)]
pub struct IoStatusBlock {
    pub status: usize,
    pub information: usize,
}

/// Reads a pointer-sized TEB32 field (`fs:[offset]`).
pub fn teb_field(offset: usize) -> usize {
    let v: usize;
    // SAFETY: FS is the 32-bit TEB in a 32-bit Windows program; reading
    // one of its fields has no side effects.
    unsafe {
        asm!("mov {}, fs:[{}]", out(reg) v, in(reg) offset, options(nostack, readonly, preserves_flags))
    };
    v
}

/// The 32-bit PEB (`fs:[0x30]`).
pub fn peb() -> *const u8 {
    teb_field(teb::PEB) as *const u8
}

/// The 32-bit process parameters (`PEB32->ProcessParameters`).
pub fn process_parameters() -> *const u8 {
    // SAFETY: the PEB is mapped for the life of the process.
    unsafe { *(peb().add(peb::PROCESS_PARAMETERS) as *const *const u8) }
}

/// `ProcessParameters->StandardOutput`.
pub fn stdout() -> Handle {
    // SAFETY: the parameters are mapped for the life of the process.
    unsafe { *(process_parameters().add(params::STANDARD_OUTPUT) as *const Handle) }
}

pub fn write_file(handle: Handle, data: &[u8]) -> NtStatus {
    let mut iosb = IoStatusBlock { status: 0, information: 0 };
    let null = core::ptr::null_mut::<c_void>();
    // SAFETY: valid buffer and IO_STATUS_BLOCK, optional arguments null.
    unsafe {
        ntrt_nt_write_file(
            handle,
            Handle(0),
            null,
            null,
            &mut iosb,
            data.as_ptr(),
            data.len() as u32,
            core::ptr::null(),
            core::ptr::null(),
        )
    }
}

pub fn print(text: &str) {
    write_file(stdout(), text.as_bytes());
}

pub fn close(handle: Handle) -> NtStatus {
    // SAFETY: NtClose takes any value and validates it.
    unsafe { ntrt_nt_close(handle) }
}

pub fn exit(status: NtStatus) -> ! {
    // SAFETY: terminating the current process does not return.
    unsafe { ntrt_nt_terminate_process(CURRENT_PROCESS, status) };
    loop {
        core::hint::spin_loop();
    }
}

/// NtAllocateVirtualMemory in the current process: (status, base, size).
pub fn allocate(base: usize, size: usize, kind: u32, protect: u32) -> (NtStatus, usize, usize) {
    let (mut b, mut s) = (base, size);
    // SAFETY: valid in/out pointers.
    let st = unsafe {
        ntrt_nt_allocate_virtual_memory(CURRENT_PROCESS, &mut b, 0, &mut s, kind, protect)
    };
    (st, b, s)
}

/// NtFreeVirtualMemory in the current process: (status, base, size).
pub fn free(base: usize, size: usize, kind: u32) -> (NtStatus, usize, usize) {
    let (mut b, mut s) = (base, size);
    // SAFETY: valid in/out pointers.
    let st = unsafe { ntrt_nt_free_virtual_memory(CURRENT_PROCESS, &mut b, &mut s, kind) };
    (st, b, s)
}

/// NtProtectVirtualMemory: (status, old protection).
pub fn protect(base: usize, size: usize, new: u32) -> (NtStatus, u32) {
    let (mut b, mut s, mut old) = (base, size, 0);
    // SAFETY: valid in/out pointers.
    let st =
        unsafe { ntrt_nt_protect_virtual_memory(CURRENT_PROCESS, &mut b, &mut s, new, &mut old) };
    (st, old)
}

/// MEMORY_BASIC_INFORMATION (x86: 28 bytes).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct MemoryBasicInformation {
    pub base_address: usize,
    pub allocation_base: usize,
    pub allocation_protect: u32,
    pub region_size: usize,
    pub state: u32,
    pub protect: u32,
    pub kind: u32,
}

/// NtQueryVirtualMemory(MemoryBasicInformation): (status, info, length
/// returned).
pub fn query(addr: usize) -> (NtStatus, MemoryBasicInformation, usize) {
    let mut mbi = MemoryBasicInformation::default();
    let mut returned = 0;
    // SAFETY: the buffer is a MemoryBasicInformation of the given size.
    let st = unsafe {
        ntrt_nt_query_virtual_memory(
            CURRENT_PROCESS,
            addr,
            0,
            (&raw mut mbi).cast(),
            size_of::<MemoryBasicInformation>(),
            &mut returned,
        )
    };
    (st, mbi, returned)
}

/// NtQueryInformationProcess into `buf`: (status, length returned).
pub fn query_process(h: Handle, class: u32, buf: &mut [u8]) -> (NtStatus, u32) {
    let mut returned = 0;
    // SAFETY: `buf` is writable for its length.
    let st = unsafe {
        ntrt_nt_query_information_process(
            h,
            class,
            buf.as_mut_ptr(),
            buf.len() as u32,
            &mut returned,
        )
    };
    (st, returned)
}

/// NtQueryPerformanceCounter: (status, counter, frequency).
pub fn performance_counter() -> (NtStatus, i64, i64) {
    let (mut c, mut f) = (0, 0);
    // SAFETY: valid output pointers.
    let st = unsafe { ntrt_nt_query_performance_counter(&mut c, &mut f) };
    (st, c, f)
}

/// The current stack pointer.
pub fn stack_pointer() -> usize {
    let sp: usize;
    // SAFETY: reads a register.
    unsafe { asm!("mov {}, esp", out(reg) sp, options(nomem, nostack, preserves_flags)) };
    sp
}

/// The code segment selector (0x23 for 32-bit code on 64-bit Windows).
pub fn code_segment() -> u16 {
    let cs: u16;
    // SAFETY: reads a segment register.
    unsafe { asm!("mov {:x}, cs", out(reg) cs, options(nomem, nostack, preserves_flags)) };
    cs
}

/// Formats `value` as 8 hex digits.
pub fn hex32(value: u32) -> [u8; 8] {
    let mut out = [0u8; 8];
    for (i, b) in out.iter_mut().enumerate() {
        let nibble = (value >> (28 - 4 * i)) & 0xf;
        *b = b"0123456789abcdef"[nibble as usize];
    }
    out
}

/// Defines the entry point (`mainCRTStartup`) and a panic handler that
/// exits with 0xC0000409.
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn mainCRTStartup() -> ! {
            $crate::exit($main())
        }

        #[panic_handler]
        fn panic(_: &core::panic::PanicInfo) -> ! {
            $crate::exit(0xc000_0409)
        }
    };
}

// Memory intrinsics (cdecl: arguments on the stack), with `rep` string
// instructions so they cannot be compiled back into calls to themselves.
global_asm!(
    ".globl _memcpy",
    "_memcpy:",
    "push esi",
    "push edi",
    "mov edi, [esp + 12]",
    "mov esi, [esp + 16]",
    "mov ecx, [esp + 20]",
    "mov eax, edi",
    "rep movsb",
    "pop edi",
    "pop esi",
    "ret",
    ".globl _memmove",
    "_memmove:",
    "push esi",
    "push edi",
    "mov edi, [esp + 12]",
    "mov esi, [esp + 16]",
    "mov ecx, [esp + 20]",
    "mov eax, edi",
    "cmp edi, esi",
    "jbe 2f",
    // Overlapping with dst above src: copy backwards.
    "lea esi, [esi + ecx - 1]",
    "lea edi, [edi + ecx - 1]",
    "std",
    "rep movsb",
    "cld",
    "jmp 3f",
    "2:",
    "rep movsb",
    "3:",
    "pop edi",
    "pop esi",
    "ret",
    ".globl _memset",
    "_memset:",
    "push edi",
    "mov edi, [esp + 8]",
    "mov eax, [esp + 12]",
    "mov ecx, [esp + 16]",
    "mov edx, edi",
    "rep stosb",
    "mov eax, edx",
    "pop edi",
    "ret",
);

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

/// Same as `memcmp`; LLVM may emit it for equality-only comparisons.
///
/// # Safety
/// Both ranges must be readable for `n` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    // SAFETY: forwarded.
    unsafe { memcmp(a, b, n) }
}

/// MSVC marker symbol that object files using floating point reference.
#[unsafe(no_mangle)]
#[used]
pub static _fltused: i32 = 0;

/// Unwind personality referenced by the prebuilt `core` library's unwind
/// tables. With `panic = abort` nothing unwinds, so it is never called.
#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    exit(0xc000_0409)
}

// The MSVC run-time library's 64-bit division helpers, which the compiler
// calls on x86 (their convention: both operands on the stack, low dword
// first, the callee pops them, the result in edx:eax), on top of
// compiler_builtins' __udivdi3/__umoddi3 (cdecl).
global_asm!(
    ".globl __aulldiv",
    "__aulldiv:",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "call ___udivdi3",
    "add esp, 16",
    "ret 16",
    ".globl __aullrem",
    "__aullrem:",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "call ___umoddi3",
    "add esp, 16",
    "ret 16",
);
