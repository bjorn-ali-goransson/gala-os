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
    /// Size of the x64 TEB, rounded up to pages.
    pub const PAGES: u64 = 2;
}

pub mod peb {
    pub const IMAGE_BASE_ADDRESS: u64 = 0x10;
    pub const PROCESS_PARAMETERS: u64 = 0x20;
    pub const PAGES: u64 = 1;
}

pub mod params {
    pub const MAXIMUM_LENGTH: u64 = 0x00;
    pub const LENGTH: u64 = 0x04;
    pub const FLAGS: u64 = 0x08;
    pub const STANDARD_INPUT: u64 = 0x20;
    pub const STANDARD_OUTPUT: u64 = 0x28;
    pub const STANDARD_ERROR: u64 = 0x30;
    /// RTL_USER_PROCESS_PARAMETERS_NORMALIZED: pointers are absolute.
    pub const FLAG_NORMALIZED: u32 = 1;
    pub const PAGES: u64 = 1;
}

// Our placement of per-process structures (Windows randomizes these; apps
// must find them through gs:[0x30]/gs:[0x60], so the exact values are ours).
pub const PEB_ADDRESS: u64 = 0x0000_7ff0_0000_0000;
pub const PARAMS_ADDRESS: u64 = 0x0000_7ff0_0001_0000;
pub const FIRST_TEB_ADDRESS: u64 = 0x0000_7ff0_0010_0000;
/// Main thread stack: `[STACK_TOP - size, STACK_TOP)`, with an unmapped
/// guard page below.
pub const STACK_TOP: u64 = 0x0000_0000_0020_0000;
/// Main-thread stack size (committed, backed on demand).
pub const STACK_SIZE: u64 = 1024 * 1024;
