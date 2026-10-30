//! Tiny runtime for freestanding Windows test programs: raw system call
//! stubs (shaped like ntdll's) and access to the PEB. No imports, no CRT.
//! Programs written against it run unchanged on real Windows of the pinned
//! build, which is what differential testing needs.

#![no_std]

use core::arch::{asm, global_asm};
use core::ffi::c_void;

// Service numbers of the pinned build are in the `stubs!` list below and
// must match `kernel/src/nt/syscalls.rs`.

/// A handle value. Opaque to user mode, so an integer rather than a pointer.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle(pub usize);
pub type NtStatus = u32;

pub const STATUS_SUCCESS: NtStatus = 0;
pub const STATUS_INVALID_HANDLE: NtStatus = 0xc000_0008;
pub const STATUS_INVALID_SYSTEM_SERVICE: NtStatus = 0xc000_001c;

/// `NtCurrentProcess()`.
pub const CURRENT_PROCESS: Handle = Handle(usize::MAX);

/// Defines a system call stub shaped exactly like ntdll's:
/// `mov r10, rcx; mov eax, N; syscall; ret`.
macro_rules! stubs {
    ($($name:ident = $nr:expr;)*) => {
        global_asm!(
            $(
                concat!(".globl ", stringify!($name)),
                concat!(stringify!($name), ":"),
                "mov r10, rcx",
                concat!("mov eax, ", stringify!($nr)),
                "syscall",
                "ret",
            )*
        );
    };
}

stubs! {
    ntrt_nt_write_file = 0x0008;
    ntrt_nt_close = 0x000f;
    ntrt_nt_allocate_virtual_memory = 0x0018;
    ntrt_nt_free_virtual_memory = 0x001e;
    ntrt_nt_query_virtual_memory = 0x0023;
    ntrt_nt_protect_virtual_memory = 0x0050;
    ntrt_nt_terminate_process = 0x002c;
    ntrt_invalid_service = 0x0fff;
}

unsafe extern "system" {
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
    fn ntrt_nt_terminate_process(process: Handle, status: NtStatus) -> NtStatus;
    fn ntrt_invalid_service() -> NtStatus;
    fn ntrt_nt_allocate_virtual_memory(
        process: Handle,
        base: *mut usize,
        zero_bits: usize,
        size: *mut usize,
        kind: u32,
        protect: u32,
    ) -> NtStatus;
    fn ntrt_nt_free_virtual_memory(
        process: Handle,
        base: *mut usize,
        size: *mut usize,
        kind: u32,
    ) -> NtStatus;
    fn ntrt_nt_query_virtual_memory(
        process: Handle,
        address: usize,
        class: u32,
        buffer: *mut MemoryBasicInformation,
        length: usize,
        returned: *mut usize,
    ) -> NtStatus;
    fn ntrt_nt_protect_virtual_memory(
        process: Handle,
        base: *mut usize,
        size: *mut usize,
        new: u32,
        old: *mut u32,
    ) -> NtStatus;
}

pub const MEM_COMMIT: u32 = 0x1000;
pub const MEM_RESERVE: u32 = 0x2000;
pub const MEM_DECOMMIT: u32 = 0x4000;
pub const MEM_RELEASE: u32 = 0x8000;
pub const MEM_FREE: u32 = 0x1_0000;
pub const MEM_PRIVATE: u32 = 0x2_0000;
pub const MEM_IMAGE: u32 = 0x100_0000;
pub const PAGE_NOACCESS: u32 = 0x01;
pub const PAGE_READONLY: u32 = 0x02;
pub const PAGE_READWRITE: u32 = 0x04;
pub const PAGE_EXECUTE_WRITECOPY: u32 = 0x80;
pub const STATUS_CONFLICTING_ADDRESSES: NtStatus = 0xc000_0018;
pub const STATUS_INVALID_PAGE_PROTECTION: NtStatus = 0xc000_0045;

#[repr(C)]
#[derive(Default, Debug)]
pub struct MemoryBasicInformation {
    pub base_address: usize,
    pub allocation_base: usize,
    pub allocation_protect: u32,
    pub partition_id: u16,
    pub region_size: usize,
    pub state: u32,
    pub protect: u32,
    pub kind: u32,
}

/// NtAllocateVirtualMemory on the current process; returns (status, base, size).
pub fn allocate(base: usize, size: usize, kind: u32, protect: u32) -> (NtStatus, usize, usize) {
    let (mut b, mut s) = (base, size);
    // SAFETY: valid in/out pointers.
    let st = unsafe {
        ntrt_nt_allocate_virtual_memory(CURRENT_PROCESS, &mut b, 0, &mut s, kind, protect)
    };
    (st, b, s)
}

pub fn free(base: usize, size: usize, kind: u32) -> (NtStatus, usize, usize) {
    let (mut b, mut s) = (base, size);
    // SAFETY: valid in/out pointers.
    let st = unsafe { ntrt_nt_free_virtual_memory(CURRENT_PROCESS, &mut b, &mut s, kind) };
    (st, b, s)
}

pub fn query(address: usize) -> (NtStatus, MemoryBasicInformation) {
    let mut mbi = MemoryBasicInformation::default();
    let mut returned = 0;
    // SAFETY: valid output buffer of the declared size.
    let st = unsafe {
        ntrt_nt_query_virtual_memory(
            CURRENT_PROCESS,
            address,
            0,
            &mut mbi,
            core::mem::size_of::<MemoryBasicInformation>(),
            &mut returned,
        )
    };
    (st, mbi)
}

/// Returns (status, old protection).
pub fn protect(base: usize, size: usize, new: u32) -> (NtStatus, u32) {
    let (mut b, mut s, mut old) = (base, size, 0);
    // SAFETY: valid in/out pointers.
    let st =
        unsafe { ntrt_nt_protect_virtual_memory(CURRENT_PROCESS, &mut b, &mut s, new, &mut old) };
    (st, old)
}

/// `PEB->ImageBaseAddress`.
pub fn image_base() -> usize {
    // SAFETY: PEB+0x10 is ImageBaseAddress on x64.
    unsafe { *(peb().add(0x10) as *const usize) }
}

/// Calls service number 0xfff, which does not exist.
pub fn invalid_service() -> NtStatus {
    // SAFETY: the kernel rejects the number without reading arguments.
    unsafe { ntrt_invalid_service() }
}

#[repr(C)]
pub struct IoStatusBlock {
    pub status: usize,
    pub information: usize,
}

/// PEB of the current process (`gs:[0x60]` on x64).
pub fn peb() -> *const u8 {
    let peb: *const u8;
    // SAFETY: on x64 Windows, gs:[0x60] holds the current PEB pointer.
    unsafe { asm!("mov {}, gs:[0x60]", out(reg) peb, options(nostack, readonly, preserves_flags)) };
    peb
}

/// `PEB->ProcessParameters->StandardOutput` (public x64 offsets).
pub fn stdout() -> Handle {
    // SAFETY: the loader always provides ProcessParameters.
    unsafe {
        let params = *(peb().add(0x20) as *const *const u8);
        *(params.add(0x28) as *const Handle)
    }
}

pub fn write_file(handle: Handle, data: &[u8]) -> NtStatus {
    let mut iosb = IoStatusBlock { status: 0, information: 0 };
    let null = core::ptr::null_mut::<c_void>();
    let no_event = Handle(0);
    // SAFETY: valid buffer and IO_STATUS_BLOCK, optional arguments null.
    unsafe {
        ntrt_nt_write_file(
            handle,
            no_event,
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
