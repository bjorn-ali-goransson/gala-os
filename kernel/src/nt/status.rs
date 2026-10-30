//! NTSTATUS values (from the public ntstatus.h definitions in mingw-w64).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NtStatus(pub u32);

impl NtStatus {
    pub const SUCCESS: NtStatus = NtStatus(0x0000_0000);
    pub const TIMEOUT: NtStatus = NtStatus(0x0000_0102);
    pub const ACCESS_DENIED: NtStatus = NtStatus(0xc000_0022);
    pub const INVALID_PARAMETER_1: NtStatus = NtStatus(0xc000_00ef);
    pub const OBJECT_NAME_EXISTS: NtStatus = NtStatus(0x4000_0035);
    pub const OBJECT_NAME_INVALID: NtStatus = NtStatus(0xc000_0033);
    pub const OBJECT_NAME_NOT_FOUND: NtStatus = NtStatus(0xc000_0034);
    pub const OBJECT_NAME_COLLISION: NtStatus = NtStatus(0xc000_0035);
    pub const OBJECT_PATH_NOT_FOUND: NtStatus = NtStatus(0xc000_003a);
    pub const OBJECT_PATH_SYNTAX_BAD: NtStatus = NtStatus(0xc000_003b);
    pub const PENDING: NtStatus = NtStatus(0x0000_0103);
    pub const THREAD_IS_TERMINATING: NtStatus = NtStatus(0xc000_004b);
    pub const NOT_MAPPED_VIEW: NtStatus = NtStatus(0xc000_0019);
    pub const UNABLE_TO_DELETE_SECTION: NtStatus = NtStatus(0xc000_001b);
    pub const INVALID_VIEW_SIZE: NtStatus = NtStatus(0xc000_001f);
    pub const SECTION_PROTECTION: NtStatus = NtStatus(0xc000_004e);
    pub const SECTION_TOO_BIG: NtStatus = NtStatus(0xc000_0040);
    pub const MAPPED_ALIGNMENT: NtStatus = NtStatus(0xc000_0220);
    pub const END_OF_FILE: NtStatus = NtStatus(0xc000_0011);
    pub const INVALID_DEVICE_REQUEST: NtStatus = NtStatus(0xc000_0010);
    pub const SHARING_VIOLATION: NtStatus = NtStatus(0xc000_0043);
    pub const DELETE_PENDING: NtStatus = NtStatus(0xc000_0056);
    pub const FILE_IS_A_DIRECTORY: NtStatus = NtStatus(0xc000_00ba);
    pub const DIRECTORY_NOT_EMPTY: NtStatus = NtStatus(0xc000_0101);
    pub const NOT_A_DIRECTORY: NtStatus = NtStatus(0xc000_0103);
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
    pub const GUARD_PAGE_VIOLATION: NtStatus = NtStatus(0x8000_0001);
    pub const BUFFER_OVERFLOW: NtStatus = NtStatus(0x8000_0005);
    pub const NO_MORE_FILES: NtStatus = NtStatus(0x8000_0006);
    pub const NO_SUCH_FILE: NtStatus = NtStatus(0xc000_000f);
    pub const FILE_DELETED: NtStatus = NtStatus(0xc000_0123);
    pub const INVALID_ADDRESS: NtStatus = NtStatus(0xc000_0141);
    pub const USER_MAPPED_FILE: NtStatus = NtStatus(0xc000_0243);
    pub const MAPPED_FILE_SIZE_ZERO: NtStatus = NtStatus(0xc000_011e);
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
    pub const INVALID_IMAGE_NOT_MZ: NtStatus = NtStatus(0xc000_012f);
    pub const INVALID_FILE_FOR_SECTION: NtStatus = NtStatus(0xc000_0020);
    pub const IMAGE_NOT_AT_BASE: NtStatus = NtStatus(0x4000_0003);
}
