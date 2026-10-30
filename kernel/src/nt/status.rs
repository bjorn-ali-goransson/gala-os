//! NTSTATUS values (from the public ntstatus.h definitions in mingw-w64).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NtStatus(pub u32);

impl NtStatus {
    pub const SUCCESS: NtStatus = NtStatus(0x0000_0000);
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
    pub const INVALID_IMAGE_FORMAT: NtStatus = NtStatus(0xc000_007b);
}
