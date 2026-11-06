//! User-visible structure offsets (x64) and our user address-space layout.
//!
//! Offsets come from the public TEB/PEB/RTL_USER_PROCESS_PARAMETERS
//! definitions (mingw-w64 winternl.h, Microsoft documentation).
//! TODO: replace with bindgen-generated `#[repr(C)]` types from mingw-w64
//! headers, as the project brief prescribes.

pub mod teb {
    pub const STACK_BASE: u64 = 0x08;
    pub const STACK_LIMIT: u64 = 0x10;
    pub const SELF: u64 = 0x30;
    pub const CLIENT_ID_PROCESS: u64 = 0x40;
    pub const CLIENT_ID_THREAD: u64 = 0x48;
    pub const PEB: u64 = 0x60;
    /// The bottom of the stack reservation.
    pub const DEALLOCATION_STACK: u64 = 0x1478;
    /// Size of the x64 TEB, rounded up to pages.
    pub const PAGES: u64 = 2;
}

pub mod peb {
    pub const BEING_DEBUGGED: u64 = 0x02;
    pub const IMAGE_BASE_ADDRESS: u64 = 0x10;
    pub const PROCESS_PARAMETERS: u64 = 0x20;
    pub const NUMBER_OF_PROCESSORS: u64 = 0xb8;
    pub const OS_MAJOR_VERSION: u64 = 0x118;
    pub const OS_MINOR_VERSION: u64 = 0x11c;
    pub const OS_BUILD_NUMBER: u64 = 0x120;
    pub const OS_PLATFORM_ID: u64 = 0x124;
    pub const IMAGE_SUBSYSTEM: u64 = 0x128;
    pub const IMAGE_SUBSYSTEM_MAJOR_VERSION: u64 = 0x12c;
    pub const IMAGE_SUBSYSTEM_MINOR_VERSION: u64 = 0x130;
    pub const SESSION_ID: u64 = 0x2c0;
    pub const PAGES: u64 = 1;
}

pub mod params {
    pub const MAXIMUM_LENGTH: u64 = 0x00;
    pub const LENGTH: u64 = 0x04;
    pub const FLAGS: u64 = 0x08;
    pub const STANDARD_INPUT: u64 = 0x20;
    pub const STANDARD_OUTPUT: u64 = 0x28;
    pub const STANDARD_ERROR: u64 = 0x30;
    /// CURDIR { UNICODE_STRING DosPath; HANDLE Handle; }
    pub const CURRENT_DIRECTORY: u64 = 0x38;
    pub const CURRENT_DIRECTORY_HANDLE: u64 = 0x48;
    pub const DLL_PATH: u64 = 0x50;
    pub const IMAGE_PATH_NAME: u64 = 0x60;
    pub const COMMAND_LINE: u64 = 0x70;
    pub const ENVIRONMENT: u64 = 0x80;
    pub const WINDOW_TITLE: u64 = 0xb0;
    /// STARTUPINFO's cbReserved2/lpReserved2 (the C runtime's inherited
    /// file descriptors), as a UNICODE_STRING-shaped byte block.
    pub const RUNTIME_DATA: u64 = 0xe0;
    pub const ENVIRONMENT_SIZE: u64 = 0x3f0;
    /// Size of the structure itself (strings follow it).
    pub const STRUCT_SIZE: u64 = 0x440;
    /// RTL_USER_PROCESS_PARAMETERS_NORMALIZED: pointers are absolute.
    pub const FLAG_NORMALIZED: u32 = 1;
}

// Our placement of per-process structures (Windows randomizes these; apps
// must find them through gs:[0x30]/gs:[0x60], so the exact values are ours).
pub const PEB_ADDRESS: u64 = 0x0000_7ff0_0000_0000;
pub const PARAMS_ADDRESS: u64 = 0x0000_7ff0_0001_0000;
pub const FIRST_TEB_ADDRESS: u64 = 0x0000_7ff0_0010_0000;
/// Where the main thread's stack ends when its reservation (the image's
/// SizeOfStackReserve) fits below: `[STACK_TOP - reserve, STACK_TOP)`,
/// the bottom page never committed.
pub const STACK_TOP: u64 = 0x0000_0000_0020_0000;
/// The largest main-thread stack reservation taken from a header.
pub const MAX_STACK: u64 = 256 * 1024 * 1024;
/// Initially committed top of the main-thread stack.
pub const STACK_COMMIT: u64 = 0x4000;

/// The 32-bit structures of a WOW64 process (public x86 layouts) and
/// gala's placement of them (ARCHITECTURE "Plan: WOW64").
pub mod wow64 {
    /// TEB32 (x86 TEB).
    pub mod teb32 {
        pub const EXCEPTION_LIST: u64 = 0x00;
        pub const STACK_BASE: u64 = 0x04;
        pub const STACK_LIMIT: u64 = 0x08;
        pub const SELF: u64 = 0x18;
        pub const CLIENT_ID_PROCESS: u64 = 0x20;
        pub const CLIENT_ID_THREAD: u64 = 0x24;
        pub const PEB: u64 = 0x30;
        /// WOW32Reserved: the 32-bit address 32-bit system calls go
        /// through (`call fs:[0xc0]`): gala's wow64.dll's transition.
        pub const WOW32_RESERVED: u64 = 0xc0;
        pub const DEALLOCATION_STACK: u64 = 0xe0c;
        /// One page.
        pub const SIZE: u64 = 0x1000;
    }

    /// PEB32 (x86 PEB).
    pub mod peb32 {
        pub const BEING_DEBUGGED: u64 = 0x02;
        pub const IMAGE_BASE_ADDRESS: u64 = 0x08;
        pub const PROCESS_PARAMETERS: u64 = 0x10;
        pub const NUMBER_OF_PROCESSORS: u64 = 0x64;
        pub const OS_MAJOR_VERSION: u64 = 0xa4;
        pub const OS_MINOR_VERSION: u64 = 0xa8;
        pub const OS_BUILD_NUMBER: u64 = 0xac;
        pub const OS_PLATFORM_ID: u64 = 0xb0;
        pub const IMAGE_SUBSYSTEM: u64 = 0xb4;
        pub const IMAGE_SUBSYSTEM_MAJOR_VERSION: u64 = 0xb8;
        pub const IMAGE_SUBSYSTEM_MINOR_VERSION: u64 = 0xbc;
        pub const SESSION_ID: u64 = 0x1d4;
        pub const SIZE: u64 = 0x1000;
    }

    /// In the 64-bit TEB of a WOW64 thread: TlsSlots[1]
    /// (WOW64_TLS_CPURESERVED) points at the thread's CPU area, and
    /// NtTib.ExceptionList at its TEB32 (as on Windows).
    pub const TEB64_CPU_SLOT: u64 = 0x1480 + 8;
    pub const TEB64_EXCEPTION_LIST: u64 = 0x00;
    /// Where gala keeps the CPU area: in the 64-bit TEB's second page,
    /// past the structure (which ends before 0x1900).
    pub const TEB64_CPU_AREA: u64 = 0x1a00;

    /// The CPU area (gala's layout but for the WOW64_CPURESERVED header):
    /// the thread's 32-bit registers while it is not running 32-bit code
    /// (its start; a system call's caller), shared with wow64.dll.
    pub mod cpu {
        /// WOW64_CPURESERVED.Flags (u16) and .Machine (u16, 0x14c).
        pub const FLAGS: u64 = 0x00;
        pub const MACHINE: u64 = 0x02;
        /// A WOW64_CONTEXT (the x86 CONTEXT, 0x2cc bytes).
        pub const CONTEXT: u64 = 0x04;
        /// The 64-bit stack pointer wow64.dll's transition from 32-bit
        /// code switches to (u64).
        pub const STACK64: u64 = 0x2d0;
        /// The 32-bit ntdll's KiUserExceptionDispatcher and
        /// KiUserApcDispatcher (u32 each; 0 without a 32-bit ntdll), where
        /// wow64.dll hands 32-bit code's exceptions and APCs.
        pub const DISPATCHER32: u64 = 0x2d8;
        pub const APC_DISPATCHER32: u64 = 0x2dc;
        pub const SIZE: u64 = 0x2e0;
    }

    /// WOW64_CONTEXT offsets (winnt.h's x86 CONTEXT).
    pub mod context32 {
        pub const CONTEXT_FLAGS: u64 = 0x00;
        pub const SEG_GS: u64 = 0x8c;
        pub const SEG_FS: u64 = 0x90;
        pub const SEG_ES: u64 = 0x94;
        pub const SEG_DS: u64 = 0x98;
        pub const EDI: u64 = 0x9c;
        pub const ESI: u64 = 0xa0;
        pub const EBX: u64 = 0xa4;
        pub const EDX: u64 = 0xa8;
        pub const ECX: u64 = 0xac;
        pub const EAX: u64 = 0xb0;
        pub const EBP: u64 = 0xb4;
        pub const EIP: u64 = 0xb8;
        pub const SEG_CS: u64 = 0xbc;
        pub const EFLAGS: u64 = 0xc0;
        pub const ESP: u64 = 0xc4;
        pub const SEG_SS: u64 = 0xc8;
        pub const SIZE: u64 = 0x2cc;
        /// CONTEXT_i386 | CONTEXT_CONTROL | CONTEXT_INTEGER | CONTEXT_SEGMENTS.
        pub const FLAGS_FULL: u32 = 0x0001_0007;
    }

    /// The selectors 32-bit code runs with (as on 64-bit Windows).
    pub const CS32: u32 = 0x23;
    pub const DS32: u32 = 0x2b;
    pub const FS32: u32 = 0x53;

    /// The PEB32's fixed place; the 32-bit process parameters and TEB32s
    /// go wherever there is room below the process's 32-bit limit.
    pub const PEB32_ADDRESS: u64 = 0x7ffd_0000;
    /// Highest address + 1 a 32-bit process's memory is given below
    /// without IMAGE_FILE_LARGE_ADDRESS_AWARE (2 GiB) and with it (4 GiB
    /// less 64 KiB), as on 64-bit Windows.
    pub const LIMIT: u64 = 0x8000_0000;
    pub const LIMIT_LARGE: u64 = 0xffff_0000;
    /// The 64-bit stack each WOW64 thread's wow64.dll code runs on.
    pub const STACK64_RESERVE: u64 = 0x4_0000;
    pub const STACK64_COMMIT: u64 = 0x4000;
}
