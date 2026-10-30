//! NTSTATUS values (from the public ntstatus.h definitions in mingw-w64).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NtStatus(pub u32);

impl NtStatus {
    pub const SUCCESS: NtStatus = NtStatus(0x0000_0000);
    pub const TIMEOUT: NtStatus = NtStatus(0x0000_0102);
    pub const ACCESS_DENIED: NtStatus = NtStatus(0xc000_0022);
    pub const INVALID_PARAMETER_1: NtStatus = NtStatus(0xc000_00ef);
    pub const NOT_IMPLEMENTED: NtStatus = NtStatus(0xc000_0002);
    pub const ACCESS_VIOLATION: NtStatus = NtStatus(0xc000_0005);
    pub const INVALID_HANDLE: NtStatus = NtStatus(0xc000_0008);
    pub const INVALID_PARAMETER: NtStatus = NtStatus(0xc000_000d);
    pub const NO_MEMORY: NtStatus = NtStatus(0xc000_0017);
    pub const ILLEGAL_INSTRUCTION: NtStatus = NtStatus(0xc000_001d);
    pub const INVALID_SYSTEM_SERVICE: NtStatus = NtStatus(0xc000_001c);
    pub const OBJECT_TYPE_MISMATCH: NtStatus = NtStatus(0xc000_0024);
    pub const INTEGER_DIVIDE_BY_ZERO: NtStatus = NtStatus(0xc000_0094);
    pub const PRIVILEGED_INSTRUCTION: NtStatus = NtStatus(0xc000_0096);
    pub const BREAKPOINT: NtStatus = NtStatus(0x8000_0003);
    pub const STACK_OVERFLOW: NtStatus = NtStatus(0xc000_00fd);
    pub const CONFLICTING_ADDRESSES: NtStatus = NtStatus(0xc000_0018);
    pub const UNABLE_TO_FREE_VM: NtStatus = NtStatus(0xc000_001a);
    pub const NOT_COMMITTED: NtStatus = NtStatus(0xc000_002d);
    pub const INVALID_PAGE_PROTECTION: NtStatus = NtStatus(0xc000_0045);
    pub const FREE_VM_NOT_AT_BASE: NtStatus = NtStatus(0xc000_009f);
    pub const MEMORY_NOT_ALLOCATED: NtStatus = NtStatus(0xc000_00a0);
    pub const INVALID_INFO_CLASS: NtStatus = NtStatus(0xc000_0003);
    pub const INFO_LENGTH_MISMATCH: NtStatus = NtStatus(0xc000_0004);
    pub const INVALID_PARAMETER_2: NtStatus = NtStatus(0xc000_00f0);
    pub const INVALID_PARAMETER_3: NtStatus = NtStatus(0xc000_00f1);
    pub const INVALID_PARAMETER_4: NtStatus = NtStatus(0xc000_00f2);
    pub const INVALID_PARAMETER_5: NtStatus = NtStatus(0xc000_00f3);
    pub const INVALID_IMAGE_FORMAT: NtStatus = NtStatus(0xc000_007b);
}
