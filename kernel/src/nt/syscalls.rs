//! System service dispatch with the Windows x64 calling convention.
//!
//! Service numbers follow the pinned Windows build (see
//! `docs/ARCHITECTURE.md`, "NT personality"). They are black-box observable
//! values and must be verified against syscall traces from that build.

use super::platform;
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};

use super::file;
use super::namespace;
use super::object::{
    DIRECTORY_CREATE_OBJECT, DIRECTORY_TRAVERSE, EVENT_MODIFY_STATE, EVENT_QUERY_STATE, Event,
    EventType, Handle, HandleEntry, KObject, ObjectBody, ObjectRef, PROCESS_ALL_ACCESS,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION, PROCESS_TERMINATE, SYNCHRONIZE,
    THREAD_ALL_ACCESS, THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME, THREAD_TERMINATE,
};
use super::object::{SECTION_MAP_EXECUTE, SECTION_MAP_READ, SECTION_MAP_WRITE, SECTION_QUERY};
use super::platform::{SyscallFrame, UserRegs};
use super::process::{self, Process};
use super::section;
use super::status::NtStatus;
use super::thread;
use super::vad;
use super::vm;
use super::wait::{self, Dispatcher, Timeout};
use crate::console;
use crate::mm::heap::KArc;
use crate::mm::heap::TryVec;
use crate::sync::IrqMutex;

pub const NT_WAIT_FOR_SINGLE_OBJECT: u32 = 0x0004;
pub const NT_READ_FILE: u32 = 0x0006;
pub const NT_DEVICE_IO_CONTROL_FILE: u32 = 0x0007;
/// gala's number for NtCancelIoFileEx until verified (not Windows').
pub const NT_CANCEL_IO_FILE_EX: u32 = 0x7f01;
/// gala's process creation service (docs/ARCHITECTURE.md, "Processes
/// started by programs").
pub const GALA_CREATE_PROCESS: u32 = 0x7f02;
/// gala's numbers for these until verified on the pinned build (they are
/// reached through ntdll's exports, see docs/ARCHITECTURE.md).
pub const NT_CREATE_NAMED_PIPE_FILE: u32 = 0x7f03;
pub const NT_FS_CONTROL_FILE: u32 = 0x7f04;
pub const NT_SET_INFORMATION_OBJECT: u32 = 0x7f05;
// The registry. 0x12, 0x13, 0x16, 0x17, 0x1d and 0x32 are long-stable on
// x64; NtSetValueKey's 0x60 is to be verified on the pinned build; the
// others are gala's numbers until verified (reached through ntdll).
pub const NT_OPEN_KEY: u32 = 0x0012;
pub const NT_ENUMERATE_VALUE_KEY: u32 = 0x0013;
pub const NT_QUERY_KEY: u32 = 0x0016;
pub const NT_QUERY_VALUE_KEY: u32 = 0x0017;
pub const NT_CREATE_KEY: u32 = 0x001d;
pub const NT_ENUMERATE_KEY: u32 = 0x0032;
pub const NT_SET_VALUE_KEY: u32 = 0x0060;
pub const NT_OPEN_KEY_EX: u32 = 0x7f06;
pub const NT_DELETE_KEY: u32 = 0x7f07;
pub const NT_DELETE_VALUE_KEY: u32 = 0x7f08;
pub const NT_FLUSH_KEY: u32 = 0x7f09;
// Job objects: gala's numbers until verified (reached through ntdll).
pub const NT_CREATE_JOB_OBJECT: u32 = 0x7f0a;
pub const NT_ASSIGN_PROCESS_TO_JOB_OBJECT: u32 = 0x7f0b;
pub const NT_SET_INFORMATION_JOB_OBJECT: u32 = 0x7f0c;
pub const NT_QUERY_INFORMATION_JOB_OBJECT: u32 = 0x7f0d;
pub const NT_IS_PROCESS_IN_JOB: u32 = 0x7f0e;
// Keyed events: gala's numbers until verified (reached through ntdll).
pub const NT_CREATE_KEYED_EVENT: u32 = 0x7f0f;
pub const NT_OPEN_KEYED_EVENT: u32 = 0x7f10;
pub const NT_WAIT_FOR_KEYED_EVENT: u32 = 0x7f11;
pub const NT_RELEASE_KEYED_EVENT: u32 = 0x7f12;
/// gala's number until verified (reached through ntdll).
pub const NT_NOTIFY_CHANGE_DIRECTORY_FILE: u32 = 0x7f13;
pub const NT_READ_FILE_SCATTER: u32 = 0x7f14;
pub const NT_WRITE_FILE_GATHER: u32 = 0x7f15;
// Tokens (docs/SECURITY.md): gala's numbers until verified (reached
// through ntdll).
pub const NT_OPEN_PROCESS_TOKEN: u32 = 0x7f16;
pub const NT_OPEN_PROCESS_TOKEN_EX: u32 = 0x7f17;
pub const NT_OPEN_THREAD_TOKEN: u32 = 0x7f18;
pub const NT_OPEN_THREAD_TOKEN_EX: u32 = 0x7f19;
pub const NT_QUERY_INFORMATION_TOKEN: u32 = 0x7f1a;
pub const NT_ADJUST_PRIVILEGES_TOKEN: u32 = 0x7f1b;
pub const NT_DUPLICATE_TOKEN: u32 = 0x7f1c;
pub const NT_FILTER_TOKEN: u32 = 0x7f1d;
pub const NT_PRIVILEGE_CHECK: u32 = 0x7f1e;
// Security descriptors (docs/SECURITY.md S2): gala's numbers until verified.
pub const NT_QUERY_SECURITY_OBJECT: u32 = 0x7f1f;
pub const NT_SET_SECURITY_OBJECT: u32 = 0x7f20;
/// gala's logon service (LogonUserW; docs/SECURITY.md S3).
pub const GALA_LOGON_USER: u32 = 0x7f21;
pub const NT_WRITE_FILE: u32 = 0x0008;
pub const NT_QUERY_INFORMATION_FILE: u32 = 0x0011;
pub const NT_SET_INFORMATION_FILE: u32 = 0x0027;
pub const NT_OPEN_FILE: u32 = 0x0033;
pub const NT_QUERY_ATTRIBUTES_FILE: u32 = 0x003d;
pub const NT_CREATE_FILE: u32 = 0x0055;
pub const NT_QUERY_DIRECTORY_FILE: u32 = 0x0035;
pub const NT_QUERY_VOLUME_INFORMATION_FILE: u32 = 0x0049;
pub const NT_FLUSH_BUFFERS_FILE: u32 = 0x004b;
// Byte-range locks: gala's own numbers, UNVERIFIED for 26100 (kernel32
// reaches them through ntdll's exports).
pub const NT_LOCK_FILE: u32 = 0x0123;
pub const NT_UNLOCK_FILE: u32 = 0x01c5;
pub const NT_CONTINUE: u32 = 0x0043;
pub const NT_QUERY_SYSTEM_INFORMATION: u32 = 0x0036;
pub const NT_SET_INFORMATION_THREAD: u32 = 0x000d;
pub const NT_YIELD_EXECUTION: u32 = 0x0046;
pub const NT_QUEUE_APC_THREAD: u32 = 0x0045;
// Mutants and semaphores. NtReleaseSemaphore (0x0a) and NtReleaseMutant
// (0x20) are long-stable on x64; the create/open/query numbers vary by build
// and these are gala's own, UNVERIFIED for 26100 (tests reach them through
// ntdll's exports, see docs/ARCHITECTURE.md).
pub const NT_RELEASE_SEMAPHORE: u32 = 0x000a;
pub const NT_RELEASE_MUTANT: u32 = 0x0020;
pub const NT_CREATE_MUTANT: u32 = 0x00c3;
pub const NT_CREATE_SEMAPHORE: u32 = 0x00cf;
pub const NT_OPEN_MUTANT: u32 = 0x012c;
pub const NT_OPEN_SEMAPHORE: u32 = 0x012f;
pub const NT_QUERY_MUTANT: u32 = 0x0146;
pub const NT_QUERY_SEMAPHORE: u32 = 0x015a;
pub const NT_CREATE_SYMBOLIC_LINK_OBJECT: u32 = 0x00d1;
pub const NT_OPEN_SYMBOLIC_LINK_OBJECT: u32 = 0x0137;
pub const NT_QUERY_SYMBOLIC_LINK_OBJECT: u32 = 0x016a;
pub const NT_QUERY_DIRECTORY_OBJECT: u32 = 0x014a;
// I/O completion ports: NtRemoveIoCompletion (0x09) is long-stable on x64;
// the others are gala's own, UNVERIFIED.
pub const NT_REMOVE_IO_COMPLETION: u32 = 0x0009;
pub const NT_CREATE_IO_COMPLETION: u32 = 0x00c9;
pub const NT_QUERY_IO_COMPLETION: u32 = 0x014f;
pub const NT_SET_IO_COMPLETION: u32 = 0x01b0;
// Thread suspension and context: gala's own numbers, UNVERIFIED.
pub const NT_GET_CONTEXT_THREAD: u32 = 0x00f2;
pub const NT_GET_CURRENT_PROCESSOR_NUMBER: u32 = 0x00f8;
/// gala's own service (not Windows'): gala's user-mode DLLs report
/// unsupported functions and missing exports to the ledger.
pub const GALA_RECORD_MISSING: u32 = 0x7f00;
pub const NT_SET_CONTEXT_THREAD: u32 = 0x018d;
pub const NT_SUSPEND_THREAD: u32 = 0x01bc;
// Waitable timers: gala's own numbers, UNVERIFIED.
pub const NT_CREATE_TIMER: u32 = 0x00d9;
pub const NT_OPEN_TIMER: u32 = 0x013d;
pub const NT_QUERY_TIMER: u32 = 0x015f;
pub const NT_SET_TIMER: u32 = 0x019e;
pub const NT_CANCEL_TIMER: u32 = 0x00bd;
pub const NT_MAP_VIEW_OF_SECTION: u32 = 0x0028;
pub const NT_UNMAP_VIEW_OF_SECTION: u32 = 0x002a;
pub const NT_OPEN_SECTION: u32 = 0x0037;
pub const NT_CREATE_SECTION: u32 = 0x004a;
pub const NT_QUERY_SECTION: u32 = 0x0051;
pub const NT_QUERY_INFORMATION_PROCESS: u32 = 0x0019;
pub const NT_SET_INFORMATION_PROCESS: u32 = 0x001c;
pub const NT_QUERY_INFORMATION_THREAD: u32 = 0x0025;
pub const NT_CREATE_THREAD: u32 = 0x004e;
pub const NT_RESUME_THREAD: u32 = 0x0052;
pub const NT_TERMINATE_THREAD: u32 = 0x0053;
pub const NT_QUERY_OBJECT: u32 = 0x0010;
pub const NT_OPEN_EVENT: u32 = 0x0040;
pub const NT_OPEN_DIRECTORY_OBJECT: u32 = 0x0058;
pub const NT_SET_EVENT: u32 = 0x000e;
pub const NT_QUERY_PERFORMANCE_COUNTER: u32 = 0x0031;
pub const NT_DELAY_EXECUTION: u32 = 0x0034;
pub const NT_DUPLICATE_OBJECT: u32 = 0x003c;
pub const NT_CLEAR_EVENT: u32 = 0x003e;
pub const NT_CREATE_EVENT: u32 = 0x0048;
pub const NT_QUERY_EVENT: u32 = 0x0056;
pub const NT_QUERY_SYSTEM_TIME: u32 = 0x005a;
pub const NT_WAIT_FOR_MULTIPLE_OBJECTS: u32 = 0x005b;
pub const NT_CLOSE: u32 = 0x000f;
pub const NT_ALLOCATE_VIRTUAL_MEMORY: u32 = 0x0018;
pub const NT_FREE_VIRTUAL_MEMORY: u32 = 0x001e;
pub const NT_QUERY_VIRTUAL_MEMORY: u32 = 0x0023;
pub const NT_TERMINATE_PROCESS: u32 = 0x002c;
pub const NT_OPEN_PROCESS: u32 = 0x0026;
pub const NT_PROTECT_VIRTUAL_MEMORY: u32 = 0x0050;

type Handler = fn(&mut Call<'_>) -> NtStatus;

/// (number, name, argument count, handler) of every system service.
const TABLE: &[(u32, &str, usize, Handler)] = &[
    (NT_WAIT_FOR_SINGLE_OBJECT, "NtWaitForSingleObject", 3, nt_wait_for_single_object),
    (NT_READ_FILE, "NtReadFile", 9, nt_read_file),
    (NT_WRITE_FILE, "NtWriteFile", 9, nt_write_file),
    (NT_QUERY_INFORMATION_FILE, "NtQueryInformationFile", 5, nt_query_information_file),
    (NT_SET_INFORMATION_FILE, "NtSetInformationFile", 5, nt_set_information_file),
    (NT_OPEN_FILE, "NtOpenFile", 6, nt_open_file),
    (NT_QUERY_ATTRIBUTES_FILE, "NtQueryAttributesFile", 2, nt_query_attributes_file),
    (NT_CREATE_FILE, "NtCreateFile", 11, nt_create_file),
    (NT_QUERY_DIRECTORY_FILE, "NtQueryDirectoryFile", 11, nt_query_directory_file),
    (
        NT_QUERY_VOLUME_INFORMATION_FILE,
        "NtQueryVolumeInformationFile",
        5,
        nt_query_volume_information_file,
    ),
    (NT_FLUSH_BUFFERS_FILE, "NtFlushBuffersFile", 2, nt_flush_buffers_file),
    (NT_LOCK_FILE, "NtLockFile", 10, nt_lock_file),
    (NT_UNLOCK_FILE, "NtUnlockFile", 5, nt_unlock_file),
    (NT_CONTINUE, "NtContinue", 2, nt_continue),
    (NT_QUERY_SYSTEM_INFORMATION, "NtQuerySystemInformation", 4, nt_query_system_information),
    (NT_SET_INFORMATION_THREAD, "NtSetInformationThread", 4, nt_set_information_thread),
    (NT_YIELD_EXECUTION, "NtYieldExecution", 0, nt_yield_execution),
    (NT_QUEUE_APC_THREAD, "NtQueueApcThread", 5, nt_queue_apc_thread),
    (NT_RELEASE_SEMAPHORE, "NtReleaseSemaphore", 3, nt_release_semaphore),
    (NT_RELEASE_MUTANT, "NtReleaseMutant", 2, nt_release_mutant),
    (NT_CREATE_MUTANT, "NtCreateMutant", 4, nt_create_mutant),
    (NT_CREATE_SEMAPHORE, "NtCreateSemaphore", 5, nt_create_semaphore),
    (NT_OPEN_MUTANT, "NtOpenMutant", 3, nt_open_mutant),
    (NT_OPEN_SEMAPHORE, "NtOpenSemaphore", 3, nt_open_semaphore),
    (NT_QUERY_MUTANT, "NtQueryMutant", 5, nt_query_mutant),
    (NT_QUERY_SEMAPHORE, "NtQuerySemaphore", 5, nt_query_semaphore),
    (NT_CREATE_SYMBOLIC_LINK_OBJECT, "NtCreateSymbolicLinkObject", 4, nt_create_symbolic_link),
    (NT_OPEN_SYMBOLIC_LINK_OBJECT, "NtOpenSymbolicLinkObject", 3, nt_open_symbolic_link),
    (NT_QUERY_SYMBOLIC_LINK_OBJECT, "NtQuerySymbolicLinkObject", 3, nt_query_symbolic_link),
    (NT_QUERY_DIRECTORY_OBJECT, "NtQueryDirectoryObject", 7, nt_query_directory_object),
    (NT_REMOVE_IO_COMPLETION, "NtRemoveIoCompletion", 5, nt_remove_io_completion),
    (NT_CREATE_IO_COMPLETION, "NtCreateIoCompletion", 4, nt_create_io_completion),
    (NT_QUERY_IO_COMPLETION, "NtQueryIoCompletion", 5, nt_query_io_completion),
    (NT_SET_IO_COMPLETION, "NtSetIoCompletion", 5, nt_set_io_completion),
    (NT_GET_CONTEXT_THREAD, "NtGetContextThread", 2, nt_get_context_thread),
    (GALA_RECORD_MISSING, "GalaRecordMissing", 4, gala_record_missing),
    (
        NT_GET_CURRENT_PROCESSOR_NUMBER,
        "NtGetCurrentProcessorNumber",
        0,
        nt_get_current_processor_number,
    ),
    (NT_SET_CONTEXT_THREAD, "NtSetContextThread", 2, nt_set_context_thread),
    (NT_SUSPEND_THREAD, "NtSuspendThread", 2, nt_suspend_thread),
    (NT_CREATE_TIMER, "NtCreateTimer", 4, nt_create_timer),
    (NT_OPEN_TIMER, "NtOpenTimer", 3, nt_open_timer),
    (NT_QUERY_TIMER, "NtQueryTimer", 5, nt_query_timer),
    (NT_SET_TIMER, "NtSetTimer", 7, nt_set_timer),
    (NT_CANCEL_TIMER, "NtCancelTimer", 2, nt_cancel_timer),
    (NT_MAP_VIEW_OF_SECTION, "NtMapViewOfSection", 10, nt_map_view_of_section),
    (NT_UNMAP_VIEW_OF_SECTION, "NtUnmapViewOfSection", 2, nt_unmap_view_of_section),
    (NT_OPEN_SECTION, "NtOpenSection", 3, nt_open_section),
    (NT_CREATE_SECTION, "NtCreateSection", 7, nt_create_section),
    (NT_QUERY_SECTION, "NtQuerySection", 5, nt_query_section),
    (NT_QUERY_INFORMATION_PROCESS, "NtQueryInformationProcess", 5, nt_query_information_process),
    (NT_SET_INFORMATION_PROCESS, "NtSetInformationProcess", 4, nt_set_information_process),
    (NT_QUERY_INFORMATION_THREAD, "NtQueryInformationThread", 5, nt_query_information_thread),
    (NT_CREATE_THREAD, "NtCreateThread", 8, nt_create_thread),
    (NT_RESUME_THREAD, "NtResumeThread", 2, nt_resume_thread),
    (NT_TERMINATE_THREAD, "NtTerminateThread", 2, nt_terminate_thread),
    (NT_QUERY_OBJECT, "NtQueryObject", 5, nt_query_object),
    (NT_SET_INFORMATION_OBJECT, "NtSetInformationObject", 4, nt_set_information_object),
    (NT_CREATE_NAMED_PIPE_FILE, "NtCreateNamedPipeFile", 14, nt_create_named_pipe_file),
    (NT_FS_CONTROL_FILE, "NtFsControlFile", 10, nt_fs_control_file),
    (NT_OPEN_EVENT, "NtOpenEvent", 3, nt_open_event),
    (NT_OPEN_DIRECTORY_OBJECT, "NtOpenDirectoryObject", 3, nt_open_directory_object),
    (NT_SET_EVENT, "NtSetEvent", 2, nt_set_event),
    (NT_QUERY_PERFORMANCE_COUNTER, "NtQueryPerformanceCounter", 2, nt_query_performance_counter),
    (NT_DELAY_EXECUTION, "NtDelayExecution", 2, nt_delay_execution),
    (NT_DUPLICATE_OBJECT, "NtDuplicateObject", 7, nt_duplicate_object),
    (NT_CLEAR_EVENT, "NtClearEvent", 1, nt_clear_event),
    (NT_CREATE_EVENT, "NtCreateEvent", 5, nt_create_event),
    (NT_QUERY_EVENT, "NtQueryEvent", 5, nt_query_event),
    (NT_QUERY_SYSTEM_TIME, "NtQuerySystemTime", 1, nt_query_system_time),
    (NT_WAIT_FOR_MULTIPLE_OBJECTS, "NtWaitForMultipleObjects", 5, nt_wait_for_multiple_objects),
    (NT_CLOSE, "NtClose", 1, nt_close),
    (NT_DEVICE_IO_CONTROL_FILE, "NtDeviceIoControlFile", 10, nt_device_io_control_file),
    (NT_CANCEL_IO_FILE_EX, "NtCancelIoFileEx", 3, nt_cancel_io_file_ex),
    (GALA_CREATE_PROCESS, "GalaCreateProcess", 1, gala_create_process),
    (NT_ALLOCATE_VIRTUAL_MEMORY, "NtAllocateVirtualMemory", 6, nt_allocate_virtual_memory),
    (NT_FREE_VIRTUAL_MEMORY, "NtFreeVirtualMemory", 4, nt_free_virtual_memory),
    (NT_QUERY_VIRTUAL_MEMORY, "NtQueryVirtualMemory", 6, nt_query_virtual_memory),
    (NT_TERMINATE_PROCESS, "NtTerminateProcess", 2, nt_terminate_process),
    (NT_OPEN_PROCESS, "NtOpenProcess", 4, nt_open_process),
    (NT_PROTECT_VIRTUAL_MEMORY, "NtProtectVirtualMemory", 5, nt_protect_virtual_memory),
    (NT_OPEN_KEY, "NtOpenKey", 3, registry_calls::nt_open_key),
    (NT_OPEN_KEY_EX, "NtOpenKeyEx", 4, registry_calls::nt_open_key_ex),
    (NT_CREATE_KEY, "NtCreateKey", 7, registry_calls::nt_create_key),
    (NT_QUERY_KEY, "NtQueryKey", 5, registry_calls::nt_query_key),
    (NT_ENUMERATE_KEY, "NtEnumerateKey", 6, registry_calls::nt_enumerate_key),
    (NT_QUERY_VALUE_KEY, "NtQueryValueKey", 6, registry_calls::nt_query_value_key),
    (NT_ENUMERATE_VALUE_KEY, "NtEnumerateValueKey", 6, registry_calls::nt_enumerate_value_key),
    (NT_SET_VALUE_KEY, "NtSetValueKey", 6, registry_calls::nt_set_value_key),
    (NT_DELETE_KEY, "NtDeleteKey", 1, registry_calls::nt_delete_key),
    (NT_DELETE_VALUE_KEY, "NtDeleteValueKey", 2, registry_calls::nt_delete_value_key),
    (NT_FLUSH_KEY, "NtFlushKey", 1, registry_calls::nt_flush_key),
    (NT_CREATE_JOB_OBJECT, "NtCreateJobObject", 3, job_calls::nt_create_job_object),
    (NT_OPEN_PROCESS_TOKEN, "NtOpenProcessToken", 3, token_calls::nt_open_process_token),
    (NT_OPEN_PROCESS_TOKEN_EX, "NtOpenProcessTokenEx", 4, token_calls::nt_open_process_token_ex),
    (NT_OPEN_THREAD_TOKEN, "NtOpenThreadToken", 4, token_calls::nt_open_thread_token),
    (NT_OPEN_THREAD_TOKEN_EX, "NtOpenThreadTokenEx", 5, token_calls::nt_open_thread_token_ex),
    (
        NT_QUERY_INFORMATION_TOKEN,
        "NtQueryInformationToken",
        5,
        token_calls::nt_query_information_token,
    ),
    (
        NT_ADJUST_PRIVILEGES_TOKEN,
        "NtAdjustPrivilegesToken",
        6,
        token_calls::nt_adjust_privileges_token,
    ),
    (NT_DUPLICATE_TOKEN, "NtDuplicateToken", 6, token_calls::nt_duplicate_token),
    (NT_FILTER_TOKEN, "NtFilterToken", 6, token_calls::nt_filter_token),
    (NT_PRIVILEGE_CHECK, "NtPrivilegeCheck", 3, token_calls::nt_privilege_check),
    (
        NT_QUERY_SECURITY_OBJECT,
        "NtQuerySecurityObject",
        5,
        security_calls::nt_query_security_object,
    ),
    (NT_SET_SECURITY_OBJECT, "NtSetSecurityObject", 3, security_calls::nt_set_security_object),
    (GALA_LOGON_USER, "GalaLogonUser", 5, token_calls::gala_logon_user),
    (
        NT_ASSIGN_PROCESS_TO_JOB_OBJECT,
        "NtAssignProcessToJobObject",
        2,
        job_calls::nt_assign_process_to_job_object,
    ),
    (
        NT_SET_INFORMATION_JOB_OBJECT,
        "NtSetInformationJobObject",
        4,
        job_calls::nt_set_information_job_object,
    ),
    (
        NT_QUERY_INFORMATION_JOB_OBJECT,
        "NtQueryInformationJobObject",
        5,
        job_calls::nt_query_information_job_object,
    ),
    (NT_IS_PROCESS_IN_JOB, "NtIsProcessInJob", 2, job_calls::nt_is_process_in_job),
    (NT_CREATE_KEYED_EVENT, "NtCreateKeyedEvent", 4, nt_create_keyed_event),
    (NT_OPEN_KEYED_EVENT, "NtOpenKeyedEvent", 3, nt_open_keyed_event),
    (NT_WAIT_FOR_KEYED_EVENT, "NtWaitForKeyedEvent", 4, nt_wait_for_keyed_event),
    (NT_RELEASE_KEYED_EVENT, "NtReleaseKeyedEvent", 4, nt_release_keyed_event),
    (
        NT_NOTIFY_CHANGE_DIRECTORY_FILE,
        "NtNotifyChangeDirectoryFile",
        9,
        nt_notify_change_directory_file,
    ),
    (NT_READ_FILE_SCATTER, "NtReadFileScatter", 9, nt_read_file_scatter),
    (NT_WRITE_FILE_GATHER, "NtWriteFileGather", 9, nt_write_file_gather),
];

/// The name of service `number` (for diagnostics).
pub fn service_name(number: u32) -> &'static str {
    TABLE.iter().find(|(n, ..)| *n == number).map_or("an unknown service", |&(_, name, ..)| name)
}

/// One system call in progress.
pub struct Call<'a> {
    frame: &'a SyscallFrame,
    process: KArc<Process>,
}

impl Call<'_> {
    /// Argument `i` (0-based): r10, rdx, r8, r9, then the user stack.
    fn arg(&self, i: usize) -> Result<u64, NtStatus> {
        Ok(match i {
            0 => self.frame.r10,
            1 => self.frame.rdx,
            2 => self.frame.r8,
            3 => self.frame.r9,
            _ => {
                let addr = self.frame.rsp + 0x28 + 8 * (i as u64 - 4);
                self.process.read_u64(addr).map_err(|_| NtStatus::ACCESS_VIOLATION)?
            }
        })
    }
}

impl Call<'_> {
    fn read_u64(&self, addr: u64) -> Result<u64, NtStatus> {
        self.process.read_u64(addr).map_err(|_| NtStatus::ACCESS_VIOLATION)
    }

    fn write_u64(&self, addr: u64, value: u64) -> Result<(), NtStatus> {
        self.process.write_u64(addr, value).map_err(|_| NtStatus::ACCESS_VIOLATION)
    }

    fn write_u32(&self, addr: u64, value: u32) -> Result<(), NtStatus> {
        self.process.write(addr, &value.to_le_bytes()).map_err(|_| NtStatus::ACCESS_VIOLATION)
    }

    /// Looks up a handle in the current process.
    fn handle(&self, h: Handle) -> Result<HandleEntry, NtStatus> {
        match h {
            Handle::CURRENT_PROCESS => {
                let object = KObject::new(ObjectBody::Process(self.process.clone()))
                    .map_err(|_| NtStatus::NO_MEMORY)?;
                Ok(HandleEntry { object, access: PROCESS_ALL_ACCESS, attributes: 0 })
            }
            Handle::CURRENT_THREAD => {
                let object = thread::current().ok_or(NtStatus::INVALID_HANDLE)?;
                Ok(HandleEntry { object, access: THREAD_ALL_ACCESS, attributes: 0 })
            }
            _ => self.process.handles.lock().get(h).ok_or(NtStatus::INVALID_HANDLE),
        }
    }

    /// Reads an optional `PLARGE_INTEGER` timeout.
    fn timeout(&self, ptr: u64) -> Result<Timeout, NtStatus> {
        let value = if ptr == 0 { None } else { Some(self.read_u64(ptr)? as i64) };
        Ok(Timeout::from_nt(value))
    }

    /// Only the current process can be targeted so far.
    fn check_current_process(&self, handle: Handle) -> Result<(), NtStatus> {
        if handle == Handle::CURRENT_PROCESS { Ok(()) } else { Err(NtStatus::INVALID_HANDLE) }
    }
}

/// Evaluates a `Result<_, NtStatus>`, returning the error status.
macro_rules! tri {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(status) => return status,
        }
    };
}

macro_rules! arg {
    ($call:expr, $i:expr) => {
        match $call.arg($i) {
            Ok(v) => v,
            Err(status) => return status,
        }
    };
}

fn dispatch(frame: &mut SyscallFrame) {
    let number = frame.rax as u32;
    let Some(me) = thread::current() else {
        panic!("syscall {number:#x} from a thread without a process");
    };
    let ObjectBody::Thread(t) = &me.body else { unreachable!("thread objects are threads") };
    let process = t.process.clone();
    let service = TABLE.iter().find(|(n, ..)| *n == number);
    let started = platform::monotonic_ns();
    let mut call = Call { frame, process };
    thread::set_in_syscall(&me, Some((number, caller_regs(&call))));
    // A wait may never return: its event is written when it starts too
    // ("wait", status 0), so a thread blocked for good shows in the log.
    if let Some(&(_, name, argc, _)) = service
        && super::stats::is_wait(name)
        && crate::events::enabled(crate::events::Category::Syscall)
    {
        syscall_event(&call, number, Some((name, argc)), NtStatus::SUCCESS, started, "wait");
    }
    let status = match service {
        Some((.., handler)) => handler(&mut call),
        None => {
            warn!("{}: unimplemented system service {number:#x}", call.process.name);
            super::ledger::record("?", "unknown service", u64::from(number));
            NtStatus::INVALID_SYSTEM_SERVICE
        }
    };
    thread::set_in_syscall(&me, None);
    let name = service.map_or("?", |&(_, name, ..)| name);
    let counter =
        if super::stats::is_wait(name) { &super::stats::WAITS } else { &super::stats::SYSCALLS };
    counter.add_since(started);
    // Unsupported requests go to the missing-features ledger.
    if let Some(&(_, name, ..)) = service {
        match status {
            // (NtQueryInformationToken records its own: a primary token's
            // TokenImpersonationLevel is an answer, not a gap.)
            NtStatus::INVALID_INFO_CLASS if name != "NtQueryInformationToken" => {
                let class = super::ledger::class_argument(name)
                    .and_then(|i| call.arg(i).ok())
                    .map_or(u64::MAX, |c| c & 0xffff_ffff);
                super::ledger::record(name, "info class", class);
            }
            NtStatus::NOT_IMPLEMENTED => super::ledger::record(name, "not implemented", 0),
            _ => {}
        }
    }
    if crate::events::enabled(crate::events::Category::Syscall) {
        syscall_event(
            &call,
            number,
            service.map(|&(_, name, argc, _)| (name, argc)),
            status,
            started,
            "syscall",
        );
    }
    if TRACE.load(Ordering::Relaxed) {
        trace(&call, number, service.map(|&(_, name, argc, _)| (name, argc)), status);
    }
    frame.rax = u64::from(status.0);
}

/// Set by the `trace` kernel command-line flag: log every system call.
static TRACE: AtomicBool = AtomicBool::new(false);

/// Whether the `trace` flag is on (system calls and user faults logged).
pub fn tracing() -> bool {
    TRACE.load(Ordering::Relaxed)
}

pub fn set_trace(on: bool) {
    TRACE.store(on, Ordering::Relaxed);
}

/// The `syscall` event for a finished call (unreadable arguments: null).
fn syscall_event(
    call: &Call<'_>,
    number: u32,
    service: Option<(&str, usize)>,
    status: NtStatus,
    started: u64,
    event: &str,
) {
    struct Args<'a>(&'a Call<'a>, usize);
    impl fmt::Display for Args<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            for i in 0..self.1 {
                if i > 0 {
                    f.write_str(",")?;
                }
                match self.0.arg(i) {
                    Ok(v) => write!(f, "{v}")?,
                    Err(_) => f.write_str("null")?,
                }
            }
            Ok(())
        }
    }
    let (name, argc) = service.unwrap_or(("?", 0));
    let tid = thread::current().and_then(|t| match &t.body {
        ObjectBody::Thread(t) => Some(t.id),
        _ => None,
    });
    crate::events::emit(
        crate::events::Category::Syscall,
        event,
        format_args!(
            r#""pid":{},"tid":{},"nr":{number},"name":{},"args":[{}],"status":{},"ns":{}"#,
            call.process.id,
            tid.unwrap_or(0),
            crate::events::Str(name),
            Args(call, argc),
            status.0,
            platform::monotonic_ns().saturating_sub(started)
        ),
    );
}

/// Logs one finished system call (arguments as passed; unreadable stack
/// arguments show as `?`).
fn trace(call: &Call<'_>, number: u32, service: Option<(&str, usize)>, status: NtStatus) {
    let mut args = [None; 12];
    let (name, argc) = service.unwrap_or(("", 0));
    for (i, slot) in args.iter_mut().enumerate().take(argc) {
        *slot = Some(call.arg(i).ok());
    }
    let tid = thread::current().and_then(|t| match &t.body {
        ObjectBody::Thread(t) => Some(t.id),
        _ => None,
    });
    let line = TraceLine {
        pid: call.process.id,
        tid: tid.unwrap_or(0),
        number,
        name,
        args: &args,
        status,
    };
    kprintln!("{line}");
}

/// One trace line: `trace: pid P tid T NtName(arg, ...) -> 0xSTATUS`.
pub struct TraceLine<'a> {
    pub pid: u64,
    pub tid: u64,
    pub number: u32,
    /// Empty for an unknown service.
    pub name: &'a str,
    /// `Some(None)` is an argument that could not be read.
    pub args: &'a [Option<Option<u64>>],
    pub status: NtStatus,
}

impl fmt::Display for TraceLine<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "trace: pid {} tid {} ", self.pid, self.tid)?;
        if self.name.is_empty() {
            write!(f, "service {:#x}(", self.number)?;
        } else {
            write!(f, "{}(", self.name)?;
        }
        for (i, arg) in self.args.iter().flatten().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            match arg {
                Some(v) => write!(f, "{v:#x}")?,
                None => f.write_str("?")?,
            }
        }
        write!(f, ") -> {:#010x}", self.status.0)
    }
}

pub fn init() {
    platform::set_syscall_handler(dispatch);
}

/// NtWriteFile(FileHandle, Event, ApcRoutine, ApcContext, IoStatusBlock,
///             Buffer, Length, ByteOffset, Key)
fn nt_write_file(call: &mut Call<'_>) -> NtStatus {
    let handle = Handle(arg!(call, 0));
    let io_status = arg!(call, 4);
    let buffer = arg!(call, 5);
    let length = arg!(call, 6) as u32;
    let entry = tri!(call.handle(handle));
    if let ObjectBody::File(file) = &entry.object.body {
        let offset_ptr = arg!(call, 7);
        let apc = tri!(io_apc(call, file, io_status));
        let event = tri!(start_io(call, &entry.object, arg!(call, 1)));
        let status = file_write(call, &entry, file, io_status, buffer, length, offset_ptr);
        finish_io(&entry.object, event.as_ref(), apc, status);
        post_completion(call, file, io_status, status);
        return status;
    }
    if let ObjectBody::Pipe(end) = &entry.object.body {
        return pipe_io(call, &entry, end, io_status, buffer, length, true);
    }
    if let ObjectBody::Null(_) = &entry.object.body {
        tri!(call.write_iosb(io_status, NtStatus::SUCCESS, u64::from(length)));
        return NtStatus::SUCCESS;
    }
    if !matches!(entry.object.body, ObjectBody::ConsoleOutput) {
        return NtStatus::OBJECT_TYPE_MISMATCH;
    }
    let mut chunk = [0u8; 256];
    let mut done = 0u64;
    while done < u64::from(length) {
        let n = (u64::from(length) - done).min(chunk.len() as u64) as usize;
        if call.process.read(buffer + done, &mut chunk[..n]).is_err() {
            return NtStatus::ACCESS_VIOLATION;
        }
        console::write_bytes(&chunk[..n]);
        done += n as u64;
    }
    // IO_STATUS_BLOCK { union { NTSTATUS Status; PVOID Pointer; }; ULONG_PTR Information; }
    let mut iosb = [0u8; 16];
    iosb[8..].copy_from_slice(&done.to_le_bytes());
    if call.process.write(io_status, &iosb).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    NtStatus::SUCCESS
}

/// NtReadFile/NtWriteFile on a pipe end. A write completes once all of
/// it is in the pipe; a read on a synchronous end waits for data, on an
/// asynchronous one it may pend (see nt/pipe.rs). The event, the end
/// itself and (asynchronous ends) the completion port and APC report it.
fn pipe_io(
    call: &Call<'_>,
    entry: &HandleEntry,
    end: &super::pipe::PipeEnd,
    iosb: u64,
    buffer: u64,
    length: u32,
    write: bool,
) -> NtStatus {
    let needed = if write { file::FILE_WRITE_DATA } else { file::FILE_READ_DATA };
    if entry.access & needed == 0 {
        return NtStatus::ACCESS_DENIED;
    }
    let event = tri!(start_io(call, &entry.object, arg!(call, 1)));
    let request = tri!(pipe_request(call, end, event.clone(), iosb, buffer, length));
    let outcome = if write {
        let mut data = Vec::new();
        if data.try_reserve_exact(length as usize).is_err() {
            return NtStatus::NO_MEMORY;
        }
        data.resize(length as usize, 0);
        if call.process.read(buffer, &mut data).is_err() {
            return NtStatus::ACCESS_VIOLATION;
        }
        match end.write(&data) {
            Ok(()) => (NtStatus::SUCCESS, data.len() as u64, Vec::new()),
            Err(e) => (e, 0, Vec::new()),
        }
    } else {
        match end.read(length as usize, Some(request.clone())) {
            Ok(super::pipe::Outcome::Pending) => return NtStatus::PENDING,
            Ok(super::pipe::Outcome::Done(status, data)) => (status, data.len() as u64, data),
            Err(e) => (e, 0, Vec::new()),
        }
    };
    pipe_finish(call, entry, end, request, event.as_ref(), outcome)
}

/// Reports a pipe request that completed in its call: through the
/// completion port and APC too on an asynchronous end (not for errors).
fn pipe_finish(
    call: &Call<'_>,
    entry: &HandleEntry,
    end: &super::pipe::PipeEnd,
    request: super::notify::Request,
    event: Option<&ObjectRef>,
    (status, done, data): (NtStatus, u64, Vec<u8>),
) -> NtStatus {
    if !end.synchronous && status.0 < 0xc000_0000 {
        let mut request = request;
        // FILE_SKIP_COMPLETION_PORT_ON_SUCCESS: no packet for an I/O that
        // completed in its call.
        if end.modes.load(core::sync::atomic::Ordering::Acquire) & 1 != 0 {
            request.port = None;
        }
        // Writes copied nothing out; reads hand their bytes over here.
        if data.is_empty() && done > 0 {
            super::notify::complete_count(request, status, done);
        } else {
            super::notify::complete(request, status, &data);
        }
        return status;
    }
    if !data.is_empty() && call.process.write(request.buffer, &data).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    finish_io(&entry.object, event, None, status);
    tri!(call.write_iosb(request.iosb, status, done));
    status
}

/// The request of an I/O on pipe end `end` (arguments as NtReadFile's:
/// ApcRoutine 2, ApcContext 3).
fn pipe_request(
    call: &Call<'_>,
    end: &super::pipe::PipeEnd,
    event: Option<ObjectRef>,
    iosb: u64,
    buffer: u64,
    length: u32,
) -> Result<super::notify::Request, NtStatus> {
    let routine = call.arg(2)?;
    let apc = if routine == 0 {
        None
    } else if end.synchronous {
        return Err(NtStatus::INVALID_PARAMETER);
    } else {
        let apc = thread::UserApc { routine, args: [call.arg(3)?, iosb, 0] };
        thread::current().map(|t| (t, apc))
    };
    Ok(super::notify::Request {
        process: call.process.clone(),
        iosb,
        buffer,
        length,
        event,
        port: end.completion.lock().clone(),
        apc_context: call.arg(3)?,
        apc,
        // FILE_SKIP_SET_EVENT_ON_HANDLE: the end's own signal is left alone.
        file_signal: if end.modes.load(core::sync::atomic::Ordering::Acquire) & 2 != 0 {
            KArc::try_new(Dispatcher::new(0)).map_err(|_| NtStatus::NO_MEMORY)?
        } else {
            end.dispatcher.clone()
        },
        done: None,
    })
}

/// NtClose(Handle)
fn nt_close(call: &mut Call<'_>) -> NtStatus {
    let handle = Handle(arg!(call, 0));
    let closed = call.process.handles.lock().close(handle);
    let Some(entry) = closed else { return NtStatus::INVALID_HANDLE };
    let was_process = matches!(entry.object.body, ObjectBody::Process(_) | ObjectBody::Thread(_));
    drop(entry);
    if was_process {
        // Perhaps the last reference to an exited process (a thread holds
        // its process): free it now.
        process::prune();
    }
    NtStatus::SUCCESS
}

/// NtTerminateProcess(ProcessHandle, ExitStatus)
fn nt_terminate_process(call: &mut Call<'_>) -> NtStatus {
    let handle = Handle(arg!(call, 0));
    let status = NtStatus(arg!(call, 1) as u32);
    if handle == Handle(0) {
        // NULL: terminate every other thread of the current process.
        process::terminate(&call.process, status, false);
        return NtStatus::SUCCESS;
    }
    let entry = tri!(call.handle(handle));
    let object = tri!(entry.require(PROCESS_TERMINATE));
    let ObjectBody::Process(p) = &object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    // Includes the caller if it belongs to `p`; it exits on its way out.
    process::terminate(p, status, true);
    NtStatus::SUCCESS
}

/// NtOpenProcess(*ProcessHandle, DesiredAccess, ObjectAttributes, ClientId):
/// the process with CLIENT_ID.UniqueProcess (or, if that is 0, the
/// process of UniqueThread); STATUS_INVALID_CID if there is none.
/// Access is granted as asked (no security descriptors yet).
fn nt_open_process(call: &mut Call<'_>) -> NtStatus {
    let (out, access, attrs, cid) =
        (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2), arg!(call, 3));
    if cid == 0 {
        return NtStatus::INVALID_PARAMETER_MIX;
    }
    let attributes = tri!(call.handle_attributes(attrs));
    let pid = tri!(call.read_u64(cid));
    let tid = tri!(call.read_u64(cid + 8));
    let found = if pid != 0 {
        process::by_id(pid)
    } else if tid != 0 {
        thread::process_of(tid)
    } else {
        None
    };
    let Some(p) = found else { return NtStatus::INVALID_CID };
    let Ok(object) = KObject::new(ObjectBody::Process(p)) else { return NtStatus::NO_MEMORY };
    call.return_handle(out, object, access, attributes)
}

/// NtAllocateVirtualMemory(ProcessHandle, *BaseAddress, ZeroBits,
///                         *RegionSize, AllocationType, Protect)
fn nt_allocate_virtual_memory(call: &mut Call<'_>) -> NtStatus {
    tri!(call.check_current_process(Handle(arg!(call, 0))));
    let (base_ptr, zero_bits, size_ptr) = (arg!(call, 1), arg!(call, 2), arg!(call, 3));
    let (kind, protect) = (arg!(call, 4) as u32, arg!(call, 5) as u32);
    let base = tri!(call.read_u64(base_ptr));
    let size = tri!(call.read_u64(size_ptr));
    if zero_bits > 53 {
        return NtStatus::INVALID_PARAMETER_3;
    }
    // TODO: honour ZeroBits (address limit) when non-zero.
    let (base, size) = tri!(vm::allocate(&call.process, base, size, kind, protect));
    tri!(call.write_u64(base_ptr, base));
    tri!(call.write_u64(size_ptr, size));
    NtStatus::SUCCESS
}

/// NtFreeVirtualMemory(ProcessHandle, *BaseAddress, *RegionSize, FreeType)
fn nt_free_virtual_memory(call: &mut Call<'_>) -> NtStatus {
    tri!(call.check_current_process(Handle(arg!(call, 0))));
    let (base_ptr, size_ptr, free_type) = (arg!(call, 1), arg!(call, 2), arg!(call, 3) as u32);
    let base = tri!(call.read_u64(base_ptr));
    let size = tri!(call.read_u64(size_ptr));
    let (base, size) = tri!(vm::free(&call.process, base, size, free_type));
    tri!(call.write_u64(base_ptr, base));
    tri!(call.write_u64(size_ptr, size));
    NtStatus::SUCCESS
}

/// NtProtectVirtualMemory(ProcessHandle, *BaseAddress, *NumberOfBytes,
///                        NewProtect, *OldProtect)
fn nt_protect_virtual_memory(call: &mut Call<'_>) -> NtStatus {
    tri!(call.check_current_process(Handle(arg!(call, 0))));
    let (base_ptr, size_ptr) = (arg!(call, 1), arg!(call, 2));
    let (new, old_ptr) = (arg!(call, 3) as u32, arg!(call, 4));
    let base = tri!(call.read_u64(base_ptr));
    let size = tri!(call.read_u64(size_ptr));
    let (old, base, size) = tri!(vm::protect(&call.process, base, size, new));
    tri!(call.write_u64(base_ptr, base));
    tri!(call.write_u64(size_ptr, size));
    tri!(call.write_u32(old_ptr, old));
    NtStatus::SUCCESS
}

const MEMORY_BASIC_INFORMATION: u64 = 0;
const MEMORY_MAPPED_FILENAME_INFORMATION: u64 = 2;

/// MemoryMappedFilenameInformation: a UNICODE_STRING followed by the NT
/// path of the file mapped at `addr` (a section view or the executable).
fn query_mapped_file_name(
    call: &Call<'_>,
    addr: u64,
    buffer: u64,
    length: u64,
    ret_ptr: u64,
) -> NtStatus {
    let name = match section::mapped_file_name(&call.process, addr) {
        Some(name) => tri!(name),
        None => {
            let (base, size) = call.process.image;
            if !(base..base + size).contains(&addr) {
                return NtStatus::INVALID_ADDRESS;
            }
            tri!(call.process.image_nt_path())
        }
    };
    let bytes = name.len() * 2;
    let needed = 16 + bytes + 2;
    if ret_ptr != 0 {
        tri!(call.write_u64(ret_ptr, needed as u64));
    }
    if (length as usize) < needed {
        return NtStatus::BUFFER_OVERFLOW;
    }
    let mut out = Vec::new();
    if out.try_reserve_exact(needed).is_err() {
        return NtStatus::NO_MEMORY;
    }
    out.resize(needed, 0u8);
    out[0..2].copy_from_slice(&(bytes as u16).to_le_bytes());
    out[2..4].copy_from_slice(&((bytes + 2) as u16).to_le_bytes());
    out[8..16].copy_from_slice(&(buffer + 16).to_le_bytes());
    for (i, c) in name.iter().enumerate() {
        out[16 + 2 * i..18 + 2 * i].copy_from_slice(&c.to_le_bytes());
    }
    match call.process.write(buffer, &out) {
        Ok(()) => NtStatus::SUCCESS,
        Err(_) => NtStatus::ACCESS_VIOLATION,
    }
}
const MBI_SIZE: usize = 48;

/// NtQueryVirtualMemory(ProcessHandle, BaseAddress, MemoryInformationClass,
///                      Buffer, Length, *ReturnLength)
fn nt_query_virtual_memory(call: &mut Call<'_>) -> NtStatus {
    tri!(call.check_current_process(Handle(arg!(call, 0))));
    let (addr, class, buffer, length, ret_ptr) =
        (arg!(call, 1), arg!(call, 2), arg!(call, 3), arg!(call, 4), arg!(call, 5));
    // The class is a ULONG: only the low 32 bits are defined.
    let class = class as u32 as u64;
    if class == MEMORY_MAPPED_FILENAME_INFORMATION {
        return query_mapped_file_name(call, addr, buffer, length, ret_ptr);
    }
    if class != MEMORY_BASIC_INFORMATION {
        return NtStatus::INVALID_INFO_CLASS;
    }
    if (length as usize) < MBI_SIZE {
        return NtStatus::INFO_LENGTH_MISMATCH;
    }
    let info = tri!(call.process.vad.lock().query(addr));
    // MEMORY_BASIC_INFORMATION (x64): BaseAddress, AllocationBase,
    // AllocationProtect (u32), PartitionId (u16), pad, RegionSize, State,
    // Protect, Type (u32 each), pad.
    let mut mbi = [0u8; MBI_SIZE];
    mbi[0..8].copy_from_slice(&info.base.to_le_bytes());
    mbi[8..16].copy_from_slice(&info.allocation_base.to_le_bytes());
    mbi[16..20].copy_from_slice(&info.allocation_protect.to_le_bytes());
    mbi[24..32].copy_from_slice(&info.size.to_le_bytes());
    mbi[32..36].copy_from_slice(&info.state.to_le_bytes());
    mbi[36..40].copy_from_slice(&info.protect.to_le_bytes());
    mbi[40..44].copy_from_slice(&info.kind.to_le_bytes());
    if call.process.write(buffer, &mbi).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    if ret_ptr != 0 {
        tri!(call.write_u64(ret_ptr, MBI_SIZE as u64));
    }
    NtStatus::SUCCESS
}

const OBJ_CASE_INSENSITIVE: u32 = 0x40;
const OBJ_OPENIF: u32 = 0x80;
/// sizeof(OBJECT_ATTRIBUTES) on x64.
const OBJECT_ATTRIBUTES_SIZE: u32 = 48;
/// Longest name accepted (UNICODE_STRING lengths are u16 byte counts).
const MAX_NAME_CHARS: usize = 0x7fff;

/// Captured OBJECT_ATTRIBUTES.
struct Attributes {
    root: Option<ObjectRef>,
    /// Empty if there is no name.
    name: Vec<u16>,
    attributes: u32,
}

impl Attributes {
    fn case_insensitive(&self) -> bool {
        self.attributes & OBJ_CASE_INSENSITIVE != 0
    }
}

impl Call<'_> {
    /// Copies a UNICODE_STRING from user memory.
    fn unicode_string(&self, ptr: u64) -> Result<Vec<u16>, NtStatus> {
        let mut header = [0u8; 16];
        self.process.read(ptr, &mut header).map_err(|_| NtStatus::ACCESS_VIOLATION)?;
        let length = usize::from(u16::from_le_bytes([header[0], header[1]]));
        let maximum = usize::from(u16::from_le_bytes([header[2], header[3]]));
        let buffer = u64::from_le_bytes(header[8..16].try_into().unwrap_or([0; 8]));
        if length % 2 != 0 || length > maximum || length / 2 > MAX_NAME_CHARS {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(length).map_err(|_| NtStatus::NO_MEMORY)?;
        bytes.resize(length, 0u8);
        self.process.read(buffer, &mut bytes).map_err(|_| NtStatus::ACCESS_VIOLATION)?;
        let mut name = Vec::new();
        name.try_reserve_exact(length / 2).map_err(|_| NtStatus::NO_MEMORY)?;
        for pair in bytes.chunks_exact(2) {
            let _ = name.try_push(u16::from_le_bytes([pair[0], pair[1]]));
        }
        Ok(name)
    }

    /// Captures OBJECT_ATTRIBUTES; a RootDirectory handle must grant
    /// `root_access` on a directory.
    fn object_attributes(
        &self,
        ptr: u64,
        root_access: u32,
    ) -> Result<Option<Attributes>, NtStatus> {
        let Some((root, name, attributes)) = self.raw_object_attributes(ptr)? else {
            return Ok(None);
        };
        let root = match root {
            0 => None,
            h => {
                let entry = self.handle(Handle(h))?;
                let object = entry.require(root_access)?.clone();
                if object.as_directory().is_none() {
                    return Err(NtStatus::OBJECT_TYPE_MISMATCH);
                }
                Some(object)
            }
        };
        Ok(Some(Attributes { root, name, attributes }))
    }

    /// Captures OBJECT_ATTRIBUTES as (RootDirectory handle, name, Attributes).
    fn raw_object_attributes(&self, ptr: u64) -> Result<Option<(u64, Vec<u16>, u32)>, NtStatus> {
        if ptr == 0 {
            return Ok(None);
        }
        let mut raw = [0u8; OBJECT_ATTRIBUTES_SIZE as usize];
        self.process.read(ptr, &mut raw).map_err(|_| NtStatus::ACCESS_VIOLATION)?;
        let field = |off: usize| u64::from_le_bytes(raw[off..off + 8].try_into().unwrap_or([0; 8]));
        if u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) != OBJECT_ATTRIBUTES_SIZE {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let name = match field(16) {
            0 => Vec::new(),
            p => self.unicode_string(p)?,
        };
        Ok(Some((field(8), name, field(24) as u32)))
    }

    /// return_handle for an open of an existing object, after the access
    /// check (docs/SECURITY.md S3).
    fn return_opened(&self, out: u64, object: ObjectRef, access: u32, attributes: u32) -> NtStatus {
        match security_calls::check_object(self, &object, access) {
            Ok(granted) => self.return_handle(out, object, granted, attributes),
            Err(e) => e,
        }
    }

    /// Inserts a handle for `object` and stores its value at `out`.
    fn return_handle(&self, out: u64, object: ObjectRef, access: u32, attributes: u32) -> NtStatus {
        let entry = HandleEntry { access: object.mapping().map(access), object, attributes };
        let Ok(handle) = self.process.handles.lock().insert(entry) else {
            return NtStatus::NO_MEMORY;
        };
        if self.write_u64(out, handle.0).is_err() {
            self.process.handles.lock().close(handle);
            return NtStatus::ACCESS_VIOLATION;
        }
        NtStatus::SUCCESS
    }

    /// Opens the named object described by `attrs_ptr`, checking its type.
    fn open_named(
        &self,
        attrs_ptr: u64,
        is_type: fn(&KObject) -> bool,
    ) -> Result<(ObjectRef, u32), NtStatus> {
        self.open_named_with(attrs_ptr, is_type, true)
    }

    /// `open_named`; with `follow_final` false a symbolic link named by
    /// the full path is opened itself.
    fn open_named_with(
        &self,
        attrs_ptr: u64,
        is_type: fn(&KObject) -> bool,
        follow_final: bool,
    ) -> Result<(ObjectRef, u32), NtStatus> {
        let attrs = self
            .object_attributes(attrs_ptr, DIRECTORY_TRAVERSE)?
            .ok_or(NtStatus::INVALID_PARAMETER)?;
        if attrs.name.is_empty() && attrs.root.is_none() {
            return Err(NtStatus::OBJECT_PATH_SYNTAX_BAD);
        }
        let ci = attrs.case_insensitive();
        let object = if !follow_final {
            namespace::lookup_link(attrs.root.as_ref(), &attrs.name, ci)?
        } else {
            namespace::lookup(attrs.root.as_ref(), &attrs.name, ci)?
        };
        if !is_type(&object) {
            return Err(NtStatus::OBJECT_TYPE_MISMATCH);
        }
        Ok((object, attrs.attributes))
    }
}

/// Handle attribute bits that are kept on the handle.
const HANDLE_ATTRIBUTES: u32 = 0x02; // OBJ_INHERIT

/// NtCreateEvent(*EventHandle, DesiredAccess, ObjectAttributes, EventType,
///               InitialState)
fn nt_create_event(call: &mut Call<'_>) -> NtStatus {
    let (out, access, attrs_ptr) = (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2));
    let kind = match arg!(call, 3) as u32 {
        0 => EventType::Notification,
        1 => EventType::Synchronization,
        _ => return NtStatus::INVALID_PARAMETER_4,
    };
    let initial = arg!(call, 4) as u8 != 0;
    let attrs = tri!(call.object_attributes(attrs_ptr, DIRECTORY_CREATE_OBJECT));
    let event = Event { kind, dispatcher: Dispatcher::new(i32::from(initial)) };
    let Ok(object) = KObject::new(ObjectBody::Event(event)) else { return NtStatus::NO_MEMORY };
    tri!(security_calls::assign_object(call, &object, attrs_ptr));
    let (object, existed, handle_attributes) = match attrs {
        Some(a) if !a.name.is_empty() || a.root.is_some() => {
            let open_if = a.attributes & OBJ_OPENIF != 0;
            let (object, existed) = tri!(namespace::create(
                a.root.as_ref(),
                &a.name,
                object,
                a.case_insensitive(),
                open_if
            ));
            if existed && !matches!(object.body, ObjectBody::Event(_)) {
                return NtStatus::OBJECT_TYPE_MISMATCH;
            }
            (object, existed, a.attributes & HANDLE_ATTRIBUTES)
        }
        Some(a) => (object, false, a.attributes & HANDLE_ATTRIBUTES),
        None => (object, false, 0),
    };
    let status = if existed {
        call.return_opened(out, object, access, handle_attributes)
    } else {
        call.return_handle(out, object, access, handle_attributes)
    };
    if status == NtStatus::SUCCESS && existed { NtStatus::OBJECT_NAME_EXISTS } else { status }
}

/// NtOpenEvent(*EventHandle, DesiredAccess, ObjectAttributes)
fn nt_open_event(call: &mut Call<'_>) -> NtStatus {
    let (out, access) = (arg!(call, 0), arg!(call, 1) as u32);
    let (object, attributes) =
        tri!(call.open_named(arg!(call, 2), |o| matches!(o.body, ObjectBody::Event(_))));
    call.return_opened(out, object, access, attributes & HANDLE_ATTRIBUTES)
}

/// NtOpenDirectoryObject(*DirectoryHandle, DesiredAccess, ObjectAttributes)
fn nt_open_directory_object(call: &mut Call<'_>) -> NtStatus {
    let (out, access) = (arg!(call, 0), arg!(call, 1) as u32);
    let (object, attributes) = tri!(call.open_named(arg!(call, 2), |o| o.as_directory().is_some()));
    call.return_opened(out, object, access, attributes & HANDLE_ATTRIBUTES)
}

const OBJECT_BASIC_INFORMATION: u64 = 0;
const OBJECT_NAME_INFORMATION: u64 = 1;
const OBJECT_TYPE_INFORMATION: u64 = 2;
const OBJECT_TYPE_INFORMATION_SIZE: usize = 0x68;
/// OBJECT_HANDLE_FLAG_INFORMATION { BOOLEAN Inherit; BOOLEAN ProtectFromClose; }
const OBJECT_HANDLE_FLAG_INFORMATION: u64 = 4;
const OBJ_PROTECT_CLOSE: u32 = 0x01;
const OBJECT_BASIC_INFORMATION_SIZE: usize = 56;

/// NtQueryObject(Handle, ObjectInformationClass, Buffer, Length, *ReturnLength)
fn nt_query_object(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let (class, buffer, length, ret_ptr) =
        (arg!(call, 1), arg!(call, 2), arg!(call, 3) as u32 as usize, arg!(call, 4));
    // The class is a ULONG: only the low 32 bits are defined.
    let class = class as u32 as u64;
    let mut out = Vec::new();
    match class {
        OBJECT_BASIC_INFORMATION => {
            // Attributes, GrantedAccess, HandleCount, PointerCount, pool
            // charges, reserved, info sizes, CreationTime.
            if out.try_reserve(OBJECT_BASIC_INFORMATION_SIZE).is_err() {
                return NtStatus::NO_MEMORY;
            }
            out.resize(OBJECT_BASIC_INFORMATION_SIZE, 0u8);
            let put = |out: &mut Vec<u8>, off: usize, v: u32| {
                out[off..off + 4].copy_from_slice(&v.to_le_bytes())
            };
            put(&mut out, 0, entry.attributes);
            put(&mut out, 4, entry.access);
            put(&mut out, 8, entry.object.handle_count() as u32);
            put(&mut out, 12, KArc::strong_count(&entry.object) as u32);
        }
        OBJECT_HANDLE_FLAG_INFORMATION => {
            let flags = [
                u8::from(entry.attributes & HANDLE_ATTRIBUTES != 0),
                u8::from(entry.attributes & OBJ_PROTECT_CLOSE != 0),
            ];
            if out.try_extend_from_slice(&flags).is_err() {
                return NtStatus::NO_MEMORY;
            }
        }
        OBJECT_NAME_INFORMATION => {
            let Ok(name) = namespace::full_name(&entry.object) else { return NtStatus::NO_MEMORY };
            // OBJECT_NAME_INFORMATION { UNICODE_STRING Name; } followed by
            // the NUL-terminated string the Buffer field points to.
            let bytes = name.len() * 2;
            let total = 16 + if name.is_empty() { 0 } else { bytes + 2 };
            if out.try_reserve(total).is_err() {
                return NtStatus::NO_MEMORY;
            }
            out.resize(total, 0u8);
            if !name.is_empty() {
                out[0..2].copy_from_slice(&(bytes as u16).to_le_bytes());
                out[2..4].copy_from_slice(&(bytes as u16 + 2).to_le_bytes());
                out[8..16].copy_from_slice(&(buffer + 16).to_le_bytes());
                for (i, c) in name.iter().enumerate() {
                    out[16 + 2 * i..18 + 2 * i].copy_from_slice(&c.to_le_bytes());
                }
            }
        }
        OBJECT_TYPE_INFORMATION => {
            // OBJECT_TYPE_INFORMATION (0x68 bytes: TypeName, counts and
            // pool usage, InvalidAttributes, GenericMapping at 0x44,
            // ValidAccessMask at 0x54, ...) followed by the NUL-terminated
            // type name TypeName.Buffer points to.
            let name = entry.object.type_name();
            let total = OBJECT_TYPE_INFORMATION_SIZE + 2 * name.len() + 2;
            if out.try_reserve(total).is_err() {
                return NtStatus::NO_MEMORY;
            }
            out.resize(total, 0u8);
            let bytes = 2 * name.len() as u16;
            out[0..2].copy_from_slice(&bytes.to_le_bytes());
            out[2..4].copy_from_slice(&(bytes + 2).to_le_bytes());
            let at = buffer + OBJECT_TYPE_INFORMATION_SIZE as u64;
            out[8..16].copy_from_slice(&at.to_le_bytes());
            let m = entry.object.mapping();
            for (i, v) in [m.read, m.write, m.execute, m.all, m.all].iter().enumerate() {
                out[0x44 + 4 * i..0x48 + 4 * i].copy_from_slice(&v.to_le_bytes());
            }
            for (i, c) in name.bytes().enumerate() {
                let at = OBJECT_TYPE_INFORMATION_SIZE + 2 * i;
                out[at..at + 2].copy_from_slice(&u16::from(c).to_le_bytes());
            }
        }
        _ => return NtStatus::INVALID_INFO_CLASS,
    }
    if ret_ptr != 0 {
        tri!(call.write_u32(ret_ptr, out.len() as u32));
    }
    if length < out.len() {
        return NtStatus::INFO_LENGTH_MISMATCH;
    }
    if call.process.write(buffer, &out).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    NtStatus::SUCCESS
}

/// NtSetInformationObject(Handle, ObjectInformationClass, Buffer, Length):
/// ObjectHandleFlagInformation (SetHandleInformation) only.
fn nt_set_information_object(call: &mut Call<'_>) -> NtStatus {
    let handle = Handle(arg!(call, 0));
    let (class, buffer, length) =
        (arg!(call, 1) as u32 as u64, arg!(call, 2), arg!(call, 3) as u32);
    if class != OBJECT_HANDLE_FLAG_INFORMATION {
        return NtStatus::INVALID_INFO_CLASS;
    }
    if length < 2 {
        return NtStatus::INFO_LENGTH_MISMATCH;
    }
    let mut flags = [0u8; 2];
    if call.process.read(buffer, &mut flags).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let attributes = if flags[0] != 0 { HANDLE_ATTRIBUTES } else { 0 }
        | if flags[1] != 0 { OBJ_PROTECT_CLOSE } else { 0 };
    if call.process.handles.lock().set_attributes(handle, attributes) {
        NtStatus::SUCCESS
    } else {
        NtStatus::INVALID_HANDLE
    }
}

fn event_object(call: &Call<'_>, h: Handle, access: u32) -> Result<ObjectRef, NtStatus> {
    let entry = call.handle(h)?;
    let object = entry.require(access)?.clone();
    match object.body {
        ObjectBody::Event(_) => Ok(object),
        _ => Err(NtStatus::OBJECT_TYPE_MISMATCH),
    }
}

/// NtSetEvent(EventHandle, *PreviousState)
fn nt_set_event(call: &mut Call<'_>) -> NtStatus {
    let event = tri!(event_object(call, Handle(arg!(call, 0)), EVENT_MODIFY_STATE));
    let previous = wait::set_signal(&event, 1);
    let prev_ptr = arg!(call, 1);
    if prev_ptr != 0 {
        tri!(call.write_u32(prev_ptr, previous as u32));
    }
    NtStatus::SUCCESS
}

/// NtClearEvent(EventHandle)
fn nt_clear_event(call: &mut Call<'_>) -> NtStatus {
    let event = tri!(event_object(call, Handle(arg!(call, 0)), EVENT_MODIFY_STATE));
    wait::set_signal(&event, 0);
    NtStatus::SUCCESS
}

/// NtQueryEvent(EventHandle, EventInformationClass, Buffer, Length, *ReturnLength)
fn nt_query_event(call: &mut Call<'_>) -> NtStatus {
    let event = tri!(event_object(call, Handle(arg!(call, 0)), EVENT_QUERY_STATE));
    // Length is a ULONG: only the low 32 bits are defined.
    let (class, buffer, length, ret_ptr) =
        (arg!(call, 1), arg!(call, 2), arg!(call, 3) as u32 as u64, arg!(call, 4));
    // The class is a ULONG: only the low 32 bits are defined.
    let class = class as u32 as u64;
    if class != 0 {
        return NtStatus::INVALID_INFO_CLASS;
    }
    if length < 8 {
        return NtStatus::INFO_LENGTH_MISMATCH;
    }
    let ObjectBody::Event(e) = &event.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    // EVENT_BASIC_INFORMATION { EVENT_TYPE EventType; LONG EventState; }
    let kind = match e.kind {
        EventType::Notification => 0u32,
        EventType::Synchronization => 1,
    };
    tri!(call.write_u32(buffer, kind));
    tri!(call.write_u32(buffer + 4, e.dispatcher.signal_state() as u32));
    if ret_ptr != 0 {
        tri!(call.write_u32(ret_ptr, 8));
    }
    NtStatus::SUCCESS
}

/// Resolves a handle to a waitable object the handle may synchronize on.
fn waitable(call: &Call<'_>, h: Handle) -> Result<ObjectRef, NtStatus> {
    let entry = call.handle(h)?;
    let object = entry.require(SYNCHRONIZE)?.clone();
    if object.dispatcher().is_none() {
        return Err(NtStatus::OBJECT_TYPE_MISMATCH);
    }
    Ok(object)
}

/// NtWaitForSingleObject(Handle, Alertable, *Timeout)
fn nt_wait_for_single_object(call: &mut Call<'_>) -> NtStatus {
    let object = tri!(waitable(call, Handle(arg!(call, 0))));
    let timeout = tri!(call.timeout(arg!(call, 2)));
    let alertable = arg!(call, 1) as u8 != 0;
    let objects = [object];
    let status = wait::wait_for_alertable(&objects, false, timeout, alertable);
    drop(objects);
    deliver_apc_after(call, status)
}

const MAXIMUM_WAIT_OBJECTS: u64 = 64;

/// NtWaitForMultipleObjects(Count, Handles[], WaitType, Alertable, *Timeout)
fn nt_wait_for_multiple_objects(call: &mut Call<'_>) -> NtStatus {
    let (count, array, wait_type) =
        (arg!(call, 0) as u32 as u64, arg!(call, 1), arg!(call, 2) as u32);
    if count == 0 || count > MAXIMUM_WAIT_OBJECTS {
        return NtStatus::INVALID_PARAMETER_1;
    }
    let wait_all = match wait_type {
        0 => true,
        1 => false,
        _ => return NtStatus::INVALID_PARAMETER_3,
    };
    let timeout = tri!(call.timeout(arg!(call, 4)));
    let mut objects = Vec::new();
    if objects.try_reserve(count as usize).is_err() {
        return NtStatus::NO_MEMORY;
    }
    for i in 0..count {
        let h = Handle(tri!(call.read_u64(array + 8 * i)));
        let _ = objects.try_push(tri!(waitable(call, h)));
    }
    let alertable = arg!(call, 3) as u8 != 0;
    let status = wait::wait_for_alertable(&objects, wait_all, timeout, alertable);
    drop(objects);
    deliver_apc_after(call, status)
}

/// NtDelayExecution(Alertable, *DelayInterval)
fn nt_delay_execution(call: &mut Call<'_>) -> NtStatus {
    let ptr = arg!(call, 1);
    if ptr == 0 {
        return NtStatus::ACCESS_VIOLATION;
    }
    let status = wait::delay(tri!(call.timeout(ptr)), arg!(call, 0) as u8 != 0);
    deliver_apc_after(call, status)
}

fn is_timer(o: &KObject) -> bool {
    matches!(o.body, ObjectBody::Timer(_))
}

fn timer_object(call: &Call<'_>, h: u64, access: u32) -> Result<ObjectRef, NtStatus> {
    let entry = call.handle(Handle(h))?;
    let object = entry.require(access)?.clone();
    if is_timer(&object) { Ok(object) } else { Err(NtStatus::OBJECT_TYPE_MISMATCH) }
}

/// NtCreateTimer(*TimerHandle, DesiredAccess, ObjectAttributes, TimerType)
fn nt_create_timer(call: &mut Call<'_>) -> NtStatus {
    use super::object::{Timer, TimerState};
    let (out, access, attrs_ptr) = (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2));
    let synchronization = match arg!(call, 3) as u32 {
        0 => false, // NotificationTimer
        1 => true,  // SynchronizationTimer
        _ => return NtStatus::INVALID_PARAMETER_4,
    };
    let timer = Timer {
        dispatcher: Dispatcher::new(0),
        synchronization,
        state: IrqMutex::new(TimerState::default()),
    };
    let Ok(object) = KObject::new(ObjectBody::Timer(timer)) else { return NtStatus::NO_MEMORY };
    create_object(call, out, access, attrs_ptr, object, is_timer)
}

/// NtOpenTimer(*TimerHandle, DesiredAccess, ObjectAttributes)
fn nt_open_timer(call: &mut Call<'_>) -> NtStatus {
    let (out, access) = (arg!(call, 0), arg!(call, 1) as u32);
    let (object, attributes) = tri!(call.open_named(arg!(call, 2), is_timer));
    call.return_opened(out, object, access, attributes & HANDLE_ATTRIBUTES)
}

/// NtSetTimer(TimerHandle, *DueTime, TimerApcRoutine, TimerContext,
///            ResumeTimer, Period (ms), *PreviousState)
fn nt_set_timer(call: &mut Call<'_>) -> NtStatus {
    use super::timer;
    let object = tri!(timer_object(call, arg!(call, 0), super::object::TIMER_MODIFY_STATE));
    let due_ptr = arg!(call, 1);
    if due_ptr == 0 {
        return NtStatus::INVALID_PARAMETER_2;
    }
    let due = tri!(call.read_u64(due_ptr)) as i64;
    let period = arg!(call, 5) as u32 as i32;
    if period < 0 {
        return NtStatus::INVALID_PARAMETER_6;
    }
    let (routine, context) = (arg!(call, 2), arg!(call, 3));
    let apc = (routine != 0).then_some((routine, context));
    let now = platform::system_time();
    let due_time = timer::due_system_time(due, now);
    let due = timer::due_tick(due, platform::ticks(), now);
    let previous =
        tri!(timer::arm(&object, due, due_time, timer::period_ticks(period as u32), apc));
    let ptr = arg!(call, 6);
    if ptr != 0 && call.process.write(ptr, &[u8::from(previous)]).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    NtStatus::SUCCESS
}

/// NtCancelTimer(TimerHandle, *CurrentState)
fn nt_cancel_timer(call: &mut Call<'_>) -> NtStatus {
    let object = tri!(timer_object(call, arg!(call, 0), super::object::TIMER_MODIFY_STATE));
    let current = tri!(super::timer::cancel(&object));
    let ptr = arg!(call, 1);
    if ptr != 0 && call.process.write(ptr, &[u8::from(current)]).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    NtStatus::SUCCESS
}

/// NtQueryTimer(TimerHandle, Class, Buffer, Length, *ReturnLength):
/// TIMER_BASIC_INFORMATION { RemainingTime, TimerState }.
fn nt_query_timer(call: &mut Call<'_>) -> NtStatus {
    let object = tri!(timer_object(call, arg!(call, 0), super::object::TIMER_QUERY_STATE));
    if arg!(call, 1) as u32 != 0 {
        return NtStatus::INVALID_INFO_CLASS;
    }
    let signaled = object.dispatcher().is_some_and(|d| d.signal_state() > 0);
    let mut b = [0u8; 16];
    b[0..8].copy_from_slice(&super::timer::remaining(&object).to_le_bytes());
    b[8] = u8::from(signaled);
    call.return_info(&b, arg!(call, 2), arg!(call, 3) as u32 as u64, arg!(call, 4))
}

const THREAD_GET_CONTEXT: u32 = 0x0008;
const THREAD_SET_CONTEXT: u32 = 0x0010;

/// A thread handle with `access`, for the thread services.
fn thread_object(call: &Call<'_>, h: u64, access: u32) -> Result<ObjectRef, NtStatus> {
    let entry = call.handle(Handle(h))?;
    let object = entry.require(access)?.clone();
    match &object.body {
        ObjectBody::Thread(t) if KArc::ptr_eq(&t.process, &call.process) => Ok(object),
        // TODO: threads of other processes.
        ObjectBody::Thread(_) => Err(NtStatus::ACCESS_DENIED),
        _ => Err(NtStatus::OBJECT_TYPE_MISMATCH),
    }
}

fn is_current_thread(object: &ObjectRef) -> bool {
    thread::current().is_some_and(|me| KArc::ptr_eq(&me, object))
}

/// The calling thread's user registers as they will be when this system
/// call returns (rax = STATUS_SUCCESS).
fn caller_regs(call: &Call<'_>) -> UserRegs {
    let f = call.frame;
    UserRegs {
        gpr: [
            0, f.rip, f.rdx, f.rbx, f.rsp, f.rbp, f.rsi, f.rdi, f.r8, f.r9, f.r10, f.rflags, f.r12,
            f.r13, f.r14, f.r15,
        ],
        rip: f.rip,
        rflags: f.rflags,
    }
}

/// NtSuspendThread(ThreadHandle, *PreviousSuspendCount)
fn nt_suspend_thread(call: &mut Call<'_>) -> NtStatus {
    let object = tri!(thread_object(call, arg!(call, 0), THREAD_SUSPEND_RESUME));
    let previous = tri!(thread::suspend(&object));
    let ptr = arg!(call, 1);
    if ptr != 0 {
        tri!(call.write_u32(ptr, previous));
    }
    NtStatus::SUCCESS
}

/// NtGetContextThread(ThreadHandle, *Context): the user registers of the
/// caller, or of a suspended thread once it has stopped.
fn nt_get_context_thread(call: &mut Call<'_>) -> NtStatus {
    let object = tri!(thread_object(call, arg!(call, 0), THREAD_GET_CONTEXT));
    let (regs, fx) = if is_current_thread(&object) {
        (caller_regs(call), platform::fxsave())
    } else {
        let p = tri!(thread::parked_state(&object));
        (p.regs, p.fx)
    };
    // TODO: honour ContextFlags (only the requested parts).
    let context = super::context::to_bytes(&regs, Some(&fx));
    match call.process.write(arg!(call, 1), &context) {
        Ok(()) => NtStatus::SUCCESS,
        Err(_) => NtStatus::ACCESS_VIOLATION,
    }
}

/// NtSetContextThread(ThreadHandle, *Context): for the caller it takes
/// effect on return (as NtContinue); for a suspended thread, on resume.
fn nt_set_context_thread(call: &mut Call<'_>) -> NtStatus {
    let object = tri!(thread_object(call, arg!(call, 0), THREAD_SET_CONTEXT));
    let mut ctx = [0u8; super::context::SIZE];
    if call.process.read(arg!(call, 1), &mut ctx).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let regs = super::context::from_bytes(&ctx);
    if !regs.is_valid() {
        return NtStatus::INVALID_PARAMETER;
    }
    let fx = super::context::float_state(&ctx);
    if is_current_thread(&object) {
        if let Some(fx) = &fx {
            platform::fxrstor(fx);
        }
        platform::continue_with(regs);
        return NtStatus::SUCCESS;
    }
    match thread::set_parked_state(&object, regs, fx) {
        Ok(()) => NtStatus::SUCCESS,
        Err(e) => e,
    }
}

/// NtQueueApcThread(ThreadHandle, ApcRoutine, ApcArgument1, ApcArgument2,
///                  ApcArgument3)
fn nt_queue_apc_thread(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let object = tri!(entry.require(THREAD_SET_CONTEXT));
    let ObjectBody::Thread(t) = &object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    if !KArc::ptr_eq(&t.process, &call.process) {
        // TODO: APCs to other processes.
        return NtStatus::ACCESS_DENIED;
    }
    let apc = thread::UserApc {
        routine: arg!(call, 1),
        args: [arg!(call, 2), arg!(call, 3), arg!(call, 4)],
    };
    match thread::queue_user_apc(object, apc) {
        Ok(()) => NtStatus::SUCCESS,
        Err(e) => e,
    }
}

/// After an alertable wait ended with STATUS_USER_APC, makes the system
/// call return through ntdll!KiUserApcDispatcher to run the first queued
/// APC; the CONTEXT it gets resumes the caller with rax = STATUS_USER_APC.
fn deliver_apc_after(call: &Call<'_>, status: NtStatus) -> NtStatus {
    if status != NtStatus::USER_APC {
        return status;
    }
    let f = call.frame;
    let regs = UserRegs {
        // rax, rcx, rdx, rbx, rsp, rbp, rsi, rdi, r8..r15; SYSCALL left the
        // return address in rcx and the flags in r11.
        gpr: [
            u64::from(status.0),
            f.rip,
            f.rdx,
            f.rbx,
            f.rsp,
            f.rbp,
            f.rsi,
            f.rdi,
            f.r8,
            f.r9,
            f.r10,
            f.rflags,
            f.r12,
            f.r13,
            f.r14,
            f.r15,
        ],
        rip: f.rip,
        rflags: f.rflags,
    };
    let context = super::context::to_bytes(&regs, Some(&platform::fxsave()));
    deliver_apc(call, context, f.rsp);
    status
}

/// Runs the next user APC, if any, by resuming the thread in
/// ntdll!KiUserApcDispatcher with `context` (the state to return to, with
/// stack pointer `rsp`) at the top of a new frame below `rsp`. The x64
/// contract (Windows Internals): rsp points to the CONTEXT, whose P1Home..
/// P4Home hold NormalContext, SystemArgument1, SystemArgument2 and
/// NormalRoutine; the dispatcher calls the routine and NtContinues the
/// CONTEXT with TestAlert set, which delivers the next APC.
// TODO: verify the frame against the pinned build's ntdll.
fn deliver_apc(call: &Call<'_>, mut context: [u8; super::context::SIZE], rsp: u64) -> bool {
    let dispatcher = call.process.apc_dispatcher.load(Ordering::Relaxed);
    if dispatcher == 0 {
        return false;
    }
    let Some(apc) = thread::take_user_apc() else { return false };
    for (i, v) in [apc.args[0], apc.args[1], apc.args[2], apc.routine].iter().enumerate() {
        context[8 * i..8 * i + 8].copy_from_slice(&v.to_le_bytes());
    }
    let frame = (rsp.wrapping_sub(super::context::SIZE as u64)) & !0xf;
    if frame >= rsp || call.process.write(frame, &context).is_err() {
        return false;
    }
    let mut gpr = [0u64; 16];
    gpr[4] = frame;
    platform::continue_with(UserRegs { gpr, rip: dispatcher, rflags: 0 });
    true
}

const DUPLICATE_CLOSE_SOURCE: u32 = 1;
const DUPLICATE_SAME_ACCESS: u32 = 2;
const DUPLICATE_SAME_ATTRIBUTES: u32 = 4;

/// NtDuplicateObject(SourceProcess, SourceHandle, TargetProcess,
///                   *TargetHandle, DesiredAccess, HandleAttributes, Options)
fn nt_duplicate_object(call: &mut Call<'_>) -> NtStatus {
    tri!(call.check_current_process(Handle(arg!(call, 0))));
    let source = Handle(arg!(call, 1));
    let (target_process, out, access, attributes, options) = (
        arg!(call, 2),
        arg!(call, 3),
        arg!(call, 4) as u32,
        arg!(call, 5) as u32,
        arg!(call, 6) as u32,
    );
    // DUPLICATE_CLOSE_SOURCE closes the source whatever else happens (the
    // DuplicateHandle docs); with no target process or no target handle it
    // only closes (Go's os/exec closes handles this way).
    let close_only = target_process == 0 || out == 0;
    if options & DUPLICATE_CLOSE_SOURCE != 0 && close_only {
        return match call.process.handles.lock().close(source) {
            Some(_) => NtStatus::SUCCESS,
            None => NtStatus::INVALID_HANDLE,
        };
    }
    if let Err(status) = call.check_current_process(Handle(target_process)) {
        if options & DUPLICATE_CLOSE_SOURCE != 0 {
            call.process.handles.lock().close(source);
        }
        return status;
    }
    if out == 0 {
        // Nowhere to return a duplicate: nothing to do.
        return NtStatus::SUCCESS;
    }
    let entry = tri!(call.handle(source));
    let new = HandleEntry {
        access: if options & DUPLICATE_SAME_ACCESS != 0 {
            entry.access
        } else {
            entry.object.mapping().map(access)
        },
        attributes: if options & DUPLICATE_SAME_ATTRIBUTES != 0 {
            entry.attributes
        } else {
            attributes
        },
        object: entry.object.clone(),
    };
    let mut handles = call.process.handles.lock();
    let Ok(handle) = handles.insert(new) else { return NtStatus::NO_MEMORY };
    if options & DUPLICATE_CLOSE_SOURCE != 0 {
        handles.close(source);
    }
    drop(handles);
    if call.write_u64(out, handle.0).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    NtStatus::SUCCESS
}

use super::platform::PERFORMANCE_FREQUENCY;

/// NtQueryPerformanceCounter(*Counter, *Frequency)
fn nt_query_performance_counter(call: &mut Call<'_>) -> NtStatus {
    let (counter, frequency) = (arg!(call, 0), arg!(call, 1));
    tri!(call.write_u64(counter, platform::monotonic_ns() / 100));
    if frequency != 0 {
        tri!(call.write_u64(frequency, PERFORMANCE_FREQUENCY));
    }
    NtStatus::SUCCESS
}

/// NtQuerySystemTime(*SystemTime)
fn nt_query_system_time(call: &mut Call<'_>) -> NtStatus {
    tri!(call.write_u64(arg!(call, 0), platform::system_time()));
    NtStatus::SUCCESS
}

/// NtContinue(*Context, TestAlert): resumes the calling thread with the
/// registers in `Context` (returned to user mode through IRETQ, since
/// SYSRET cannot restore rcx and r11).
fn nt_continue(call: &mut Call<'_>) -> NtStatus {
    let ctx_ptr = arg!(call, 0);
    let mut ctx = [0u8; super::context::SIZE];
    if call.process.read(ctx_ptr, &mut ctx).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let regs = super::context::from_bytes(&ctx);
    if !regs.is_valid() {
        return NtStatus::INVALID_PARAMETER;
    }
    // TestAlert: run the next queued user APC before resuming the context.
    if arg!(call, 1) as u8 != 0 && thread::has_user_apc() && deliver_apc(call, ctx, regs.gpr[4]) {
        return NtStatus::SUCCESS;
    }
    // The kernel does not use x87/SSE, so the state loaded now is what the
    // thread resumes with (context switches save and restore it).
    if let Some(fx) = super::context::float_state(&ctx) {
        platform::fxrstor(&fx);
    }
    platform::continue_with(regs);
    NtStatus::SUCCESS
}

/// NtCreateThread(*ThreadHandle, DesiredAccess, ObjectAttributes,
///                ProcessHandle, *ClientId, *ThreadContext, *InitialTeb,
///                CreateSuspended)
fn nt_create_thread(call: &mut Call<'_>) -> NtStatus {
    let (out, access, _attrs) = (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2));
    tri!(call.check_current_process(Handle(arg!(call, 3))));
    let (cid_ptr, ctx_ptr, teb_ptr) = (arg!(call, 4), arg!(call, 5), arg!(call, 6));
    let suspended = arg!(call, 7) as u8 != 0;
    if ctx_ptr == 0 || teb_ptr == 0 {
        return NtStatus::INVALID_PARAMETER;
    }
    let mut ctx = [0u8; super::context::SIZE];
    if call.process.read(ctx_ptr, &mut ctx).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let regs = super::context::from_bytes(&ctx);
    // TODO: the rest of FltSave (x87 state, XMM registers).
    let mxcsr = super::context::mxcsr(&ctx);
    // INITIAL_TEB { OldStackBase, OldStackLimit, StackBase, StackLimit, StackAllocationBase }
    let stack_base = tri!(call.read_u64(teb_ptr + 16));
    let stack_limit = tri!(call.read_u64(teb_ptr + 24));
    let stack_bottom = tri!(call.read_u64(teb_ptr + 32));
    let object = tri!(thread::create(
        &call.process,
        regs,
        // As ReactOS does: the "return register" of the start context
        // (kernel32's thunks pass the Win32 start routine there).
        regs.gpr[0],
        mxcsr,
        (stack_base, stack_limit, stack_bottom),
        suspended
    ));
    let ObjectBody::Thread(t) = &object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let tid = t.id;
    let status = call.return_handle(out, object.clone(), access, 0);
    if status != NtStatus::SUCCESS {
        thread::request_termination(&object, status);
        thread::resume(&object);
        return status;
    }
    if cid_ptr != 0 {
        tri!(call.write_u64(cid_ptr, call.process.id));
        tri!(call.write_u64(cid_ptr + 8, tid));
    }
    NtStatus::SUCCESS
}

/// NtResumeThread(ThreadHandle, *PreviousSuspendCount)
fn nt_resume_thread(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let object = tri!(entry.require(THREAD_SUSPEND_RESUME));
    if !matches!(object.body, ObjectBody::Thread(_)) {
        return NtStatus::OBJECT_TYPE_MISMATCH;
    }
    let previous = thread::resume(object);
    let prev_ptr = arg!(call, 1);
    if prev_ptr != 0 {
        tri!(call.write_u32(prev_ptr, previous));
    }
    NtStatus::SUCCESS
}

/// NtTerminateThread(ThreadHandle, ExitStatus)
fn nt_terminate_thread(call: &mut Call<'_>) -> NtStatus {
    let handle = match Handle(arg!(call, 0)) {
        Handle(0) => Handle::CURRENT_THREAD,
        h => h,
    };
    let status = NtStatus(arg!(call, 1) as u32);
    let entry = tri!(call.handle(handle));
    let object = tri!(entry.require(THREAD_TERMINATE));
    if !matches!(object.body, ObjectBody::Thread(_)) {
        return NtStatus::OBJECT_TYPE_MISMATCH;
    }
    if thread::current().is_some_and(|me| KArc::ptr_eq(&me, object)) {
        thread::exit_current_on_return(status);
    } else {
        thread::request_termination(object, status);
    }
    NtStatus::SUCCESS
}

const BASIC_INFORMATION_SIZE: usize = 48;

impl Call<'_> {
    /// Writes an information-class result, honouring Length/ReturnLength.
    fn return_info(&self, data: &[u8], buffer: u64, length: u64, ret_ptr: u64) -> NtStatus {
        if ret_ptr != 0 {
            tri!(self.write_u32(ret_ptr, data.len() as u32));
        }
        if (length as usize) != data.len() {
            return NtStatus::INFO_LENGTH_MISMATCH;
        }
        match self.process.write(buffer, data) {
            Ok(()) => NtStatus::SUCCESS,
            Err(_) => NtStatus::ACCESS_VIOLATION,
        }
    }
}

/// NtQueryInformationThread(ThreadHandle, Class, Buffer, Length, *ReturnLength)
fn nt_query_information_thread(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let (class, buffer, length, ret_ptr) =
        (arg!(call, 1), arg!(call, 2), arg!(call, 3) as u32 as u64, arg!(call, 4));
    // The class is a ULONG: only the low 32 bits are defined.
    let class = class as u32 as u64;
    let object = tri!(entry.require(THREAD_QUERY_LIMITED_INFORMATION));
    let ObjectBody::Thread(t) = &object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    match class {
        0 => {}
        THREAD_TIMES => {
            // KERNEL_USER_TIMES { CreateTime, ExitTime, KernelTime, UserTime }
            // TODO: account kernel and user time.
            let mut b = [0u8; 32];
            b[0..8].copy_from_slice(&t.create_time.to_le_bytes());
            b[8..16].copy_from_slice(&t.exit_time.lock().to_le_bytes());
            return call.return_info(&b, buffer, length, ret_ptr);
        }
        THREAD_WIN32_START_ADDRESS => {
            return call.return_info(&t.win32_start.to_le_bytes(), buffer, length, ret_ptr);
        }
        THREAD_AM_I_LAST_THREAD => {
            let last = u32::from(t.process.thread_count() == 1);
            return call.return_info(&last.to_le_bytes(), buffer, length, ret_ptr);
        }
        THREAD_IS_TERMINATED => {
            let done = u32::from(t.has_exited());
            return call.return_info(&done.to_le_bytes(), buffer, length, ret_ptr);
        }
        _ => return NtStatus::INVALID_INFO_CLASS,
    }
    // THREAD_BASIC_INFORMATION { ExitStatus, TebBaseAddress, ClientId,
    // AffinityMask, Priority, BasePriority }
    let (priority, base) = *t.priority.lock();
    let mut b = [0u8; BASIC_INFORMATION_SIZE];
    b[0..4].copy_from_slice(&t.exit_status().0.to_le_bytes());
    b[8..16].copy_from_slice(&t.teb.to_le_bytes());
    b[16..24].copy_from_slice(&t.process.id.to_le_bytes());
    b[24..32].copy_from_slice(&t.id.to_le_bytes());
    b[32..40].copy_from_slice(&t.affinity().to_le_bytes());
    b[40..44].copy_from_slice(&priority.to_le_bytes());
    b[44..48].copy_from_slice(&base.to_le_bytes());
    call.return_info(&b, buffer, length, ret_ptr)
}

const THREAD_TIMES: u64 = 1;
const THREAD_PRIORITY: u64 = 2;
const THREAD_BASE_PRIORITY: u64 = 3;
const THREAD_AFFINITY_MASK: u64 = 4;
const THREAD_IMPERSONATION_TOKEN: u64 = 5;
const THREAD_WIN32_START_ADDRESS: u64 = 9;
const THREAD_AM_I_LAST_THREAD: u64 = 12;
const THREAD_HIDE_FROM_DEBUGGER: u64 = 17;
const THREAD_IS_TERMINATED: u64 = 20;
const THREAD_SET_INFORMATION: u32 = 0x0020;
/// Highest priority (real-time range ends at 31).
const MAXIMUM_PRIORITY: i32 = 31;

/// NtSetInformationThread(ThreadHandle, Class, Buffer, Length)
fn nt_set_information_thread(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let (class, buffer, length) =
        (arg!(call, 1) as u32 as u64, arg!(call, 2), arg!(call, 3) as u32);
    if class == THREAD_IMPERSONATION_TOKEN {
        return token_calls::set_thread_token(call, &entry, buffer, length);
    }
    let object = tri!(entry.require(THREAD_SET_INFORMATION));
    let ObjectBody::Thread(t) = &object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let need = match class {
        THREAD_HIDE_FROM_DEBUGGER => 0,
        THREAD_PRIORITY | THREAD_BASE_PRIORITY => 4,
        THREAD_AFFINITY_MASK => 8,
        _ => return NtStatus::INVALID_INFO_CLASS,
    };
    if length != need {
        return NtStatus::INFO_LENGTH_MISMATCH;
    }
    if class == THREAD_HIDE_FROM_DEBUGGER {
        return NtStatus::SUCCESS; // there is no debugger to hide from
    }
    if class == THREAD_AFFINITY_MASK {
        let mut v = [0u8; 8];
        if call.process.read(buffer, &mut v).is_err() {
            return NtStatus::ACCESS_VIOLATION;
        }
        // Non-empty and within the process's affinity (all online CPUs).
        let mask = u64::from_le_bytes(v);
        if mask == 0 || mask & !platform::online_mask() != 0 {
            return NtStatus::INVALID_PARAMETER;
        }
        t.set_affinity(mask);
        return NtStatus::SUCCESS;
    }
    let mut v = [0u8; 4];
    if call.process.read(buffer, &mut v).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let value = i32::from_le_bytes(v);
    let mut p = t.priority.lock();
    match class {
        THREAD_PRIORITY => {
            // KPRIORITY: 1..31 (raising into real time would need a privilege).
            if !(1..=MAXIMUM_PRIORITY).contains(&value) {
                return NtStatus::INVALID_PARAMETER;
            }
            p.0 = value;
        }
        _ => {
            let Some(base) = thread::base_priority(value) else {
                return NtStatus::INVALID_PARAMETER;
            };
            p.1 = base;
            p.0 = p.0.max(base);
        }
    }
    NtStatus::SUCCESS
}

/// GalaRecordMissing(Name, NameLength, What, Detail): counts a gap that
/// gala's user-mode DLLs ran into (ASCII name such as
/// `kernel32.dll!CreateFileW`; What: 1 missing export, 2 unsupported, 3
/// not implemented) in the missing-features ledger.
fn gala_record_missing(call: &mut Call<'_>) -> NtStatus {
    let (ptr, len) = (arg!(call, 0), arg!(call, 1) as usize);
    let mut name = [0u8; 64];
    let len = len.min(name.len());
    if call.process.read(ptr, &mut name[..len]).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let what = super::ledger::user_what(arg!(call, 2) as u32);
    super::ledger::record_user(&name[..len], what, arg!(call, 3));
    NtStatus::SUCCESS
}

/// NtGetCurrentProcessorNumber(): the caller's processor (it may move
/// right after); the return value is the number, not a status.
fn nt_get_current_processor_number(_call: &mut Call<'_>) -> NtStatus {
    NtStatus(platform::current_cpu())
}

/// NtYieldExecution(): lets another ready thread run.
fn nt_yield_execution(_call: &mut Call<'_>) -> NtStatus {
    platform::yield_now();
    NtStatus::SUCCESS
}

/// NtQueryInformationProcess(ProcessHandle, Class, Buffer, Length, *ReturnLength)
fn nt_query_information_process(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let (class, buffer, length, ret_ptr) =
        (arg!(call, 1), arg!(call, 2), arg!(call, 3) as u32 as u64, arg!(call, 4));
    // The class is a ULONG: only the low 32 bits are defined.
    let class = class as u32 as u64;
    let object = tri!(entry.require(PROCESS_QUERY_LIMITED_INFORMATION));
    let ObjectBody::Process(p) = &object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    match class {
        0 => {}
        PROCESS_DEBUG_PORT | PROCESS_WOW64_INFORMATION => {
            // No debugger; not a WOW64 process.
            return call.return_info(&0u64.to_le_bytes(), buffer, length, ret_ptr);
        }
        PROCESS_TIMES => {
            // KERNEL_USER_TIMES { CreateTime, ExitTime, KernelTime,
            // UserTime }; CPU times are not accounted yet (0).
            let mut t = [0u8; 32];
            t[0..8].copy_from_slice(&p.create_time.to_le_bytes());
            t[8..16].copy_from_slice(&p.exit_time.lock().to_le_bytes());
            return call.return_info(&t, buffer, length, ret_ptr);
        }
        PROCESS_PRIORITY_CLASS => {
            // PROCESS_PRIORITY_CLASS { BOOLEAN Foreground; UCHAR PriorityClass }.
            let class = p.priority_class.load(core::sync::atomic::Ordering::Relaxed);
            return call.return_info(&[0, class], buffer, length, ret_ptr);
        }
        PROCESS_SESSION_INFORMATION => {
            let session = process::SESSION_ID.to_le_bytes();
            return call.return_info(&session, buffer, length, ret_ptr);
        }
        PROCESS_IMAGE_FILE_NAME => {
            let name = tri!(p.image_nt_path());
            let Some(info) = super::sysinfo::unicode_string_in_place(&name, buffer) else {
                return NtStatus::NO_MEMORY;
            };
            if ret_ptr != 0 {
                tri!(call.write_u32(ret_ptr, info.len() as u32));
            }
            if (length as usize) < info.len() {
                return NtStatus::INFO_LENGTH_MISMATCH;
            }
            return match call.process.write(buffer, &info) {
                Ok(()) => NtStatus::SUCCESS,
                Err(_) => NtStatus::ACCESS_VIOLATION,
            };
        }
        _ => return NtStatus::INVALID_INFO_CLASS,
    }
    // PROCESS_BASIC_INFORMATION { ExitStatus, PebBaseAddress, AffinityMask,
    // BasePriority, UniqueProcessId, InheritedFromUniqueProcessId }
    let mut b = [0u8; BASIC_INFORMATION_SIZE];
    b[0..4].copy_from_slice(&p.exit_status().unwrap_or(NtStatus::PENDING).0.to_le_bytes());
    b[8..16].copy_from_slice(&super::layout::PEB_ADDRESS.to_le_bytes());
    b[16..24].copy_from_slice(&platform::online_mask().to_le_bytes());
    b[24..28].copy_from_slice(&8u32.to_le_bytes());
    b[32..40].copy_from_slice(&p.id.to_le_bytes());
    call.return_info(&b, buffer, length, ret_ptr)
}

const PROCESS_TIMES: u64 = 4;
const PROCESS_DEBUG_PORT: u64 = 7;
const PROCESS_PRIORITY_CLASS: u64 = 18;
const PROCESS_SESSION_INFORMATION: u64 = 24;
const PROCESS_WOW64_INFORMATION: u64 = 26;
const PROCESS_IMAGE_FILE_NAME: u64 = 27;

/// SystemProcessInformation for a buffer at user address `buffer`: the
/// idle process (id 0), then each running process with its threads.
fn process_information(buffer: u64) -> Result<Vec<u8>, NtStatus> {
    use super::sysinfo::{ProcessRecord, ThreadRecord, push_process};
    let no_memory = |_| NtStatus::NO_MEMORY;
    let mut out = Vec::new();
    let idle = ProcessRecord { id: 0, parent_id: 0, create_time: 0, handle_count: 0, name: "" };
    push_process(&mut out, &idle, &[], buffer).map_err(no_memory)?;
    for p in process::running().map_err(no_memory)? {
        let mut threads = Vec::new();
        for t in p.threads() {
            if let ObjectBody::Thread(t) = &t.body {
                let record =
                    ThreadRecord { id: t.id, start: t.win32_start, create_time: t.create_time };
                threads.try_push(record).map_err(no_memory)?;
            }
        }
        let record = ProcessRecord {
            id: p.id,
            parent_id: p.parent_id,
            create_time: p.create_time,
            handle_count: p.handles.lock().iter().count() as u32,
            name: p.name,
        };
        push_process(&mut out, &record, &threads, buffer).map_err(no_memory)?;
    }
    Ok(out)
}

/// NtQuerySystemInformation(Class, Buffer, Length, *ReturnLength)
fn nt_query_system_information(call: &mut Call<'_>) -> NtStatus {
    use super::sysinfo;
    let (class, buffer, length, ret_ptr) =
        (arg!(call, 0) as u32, arg!(call, 1), arg!(call, 2) as u32 as u64, arg!(call, 3));
    match class {
        sysinfo::SYSTEM_BASIC_INFORMATION => {
            let (_, total) = platform::frame_stats();
            // The timer ticks at TICK_HZ (100 ns units).
            let resolution = (10_000_000 / platform::TICK_HZ) as u32;
            let cpus = platform::online_count().min(64) as u8;
            let info = sysinfo::basic_information(total, cpus, resolution);
            call.return_info(&info, buffer, length, ret_ptr)
        }
        sysinfo::SYSTEM_PERFORMANCE_INFORMATION => {
            let (free, total) = platform::frame_stats();
            let info = sysinfo::performance_information(free, total);
            call.return_info(&info, buffer, length, ret_ptr)
        }
        sysinfo::SYSTEM_TIME_OF_DAY_INFORMATION => {
            let info = sysinfo::time_of_day(platform::boot_system_time(), platform::system_time());
            call.return_info(&info, buffer, length, ret_ptr)
        }
        sysinfo::SYSTEM_QUERY_PERFORMANCE_COUNTER_INFORMATION => {
            let info = sysinfo::query_performance_counter_information();
            call.return_info(&info, buffer, length, ret_ptr)
        }
        sysinfo::SYSTEM_PROCESS_INFORMATION => {
            let info = tri!(process_information(buffer));
            if ret_ptr != 0 {
                tri!(call.write_u32(ret_ptr, info.len() as u32));
            }
            // Any buffer large enough (the list changes between calls).
            if (length as usize) < info.len() {
                return NtStatus::INFO_LENGTH_MISMATCH;
            }
            match call.process.write(buffer, &info) {
                Ok(()) => NtStatus::SUCCESS,
                Err(_) => NtStatus::ACCESS_VIOLATION,
            }
        }
        sysinfo::GALA_NETWORK_INTERFACES => {
            let (records, n) = sysinfo::interfaces(crate::net::tcp::ethernet());
            if ret_ptr != 0 {
                tri!(call.write_u32(ret_ptr, n as u32));
            }
            if (length as usize) < n {
                return NtStatus::INFO_LENGTH_MISMATCH;
            }
            match call.process.write(buffer, &records[..n]) {
                Ok(()) => NtStatus::SUCCESS,
                Err(_) => NtStatus::ACCESS_VIOLATION,
            }
        }
        _ => NtStatus::INVALID_INFO_CLASS,
    }
}

/// NtSetInformationProcess(ProcessHandle, Class, Buffer, Length): only
/// ProcessPriorityClass (PROCESS_PRIORITY_CLASS, classes 1-6, recorded:
/// gala's scheduler gives every process the normal base priority); other
/// classes are STATUS_INVALID_INFO_CLASS (and go to the ledger).
fn nt_set_information_process(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let (class, buffer, length) = (arg!(call, 1) as u32 as u64, arg!(call, 2), arg!(call, 3));
    if class != PROCESS_PRIORITY_CLASS {
        return NtStatus::INVALID_INFO_CLASS;
    }
    let object = tri!(entry.require(PROCESS_SET_INFORMATION));
    let ObjectBody::Process(p) = &object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    if length as u32 != 2 {
        return NtStatus::INFO_LENGTH_MISMATCH;
    }
    let mut info = [0u8; 2];
    if call.process.read(buffer, &mut info).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    if !(1..=6).contains(&info[1]) {
        return NtStatus::INVALID_PARAMETER;
    }
    p.priority_class.store(info[1], core::sync::atomic::Ordering::Relaxed);
    NtStatus::SUCCESS
}

/// Creates or opens-if `object` under the name in `attrs` (if named), for
/// the create services. Returns the object to hand out and whether it
/// already existed.
fn link_new_object(
    attrs: Option<Attributes>,
    object: ObjectRef,
    same_type: fn(&KObject) -> bool,
) -> Result<(ObjectRef, bool, u32), NtStatus> {
    match attrs {
        Some(a) if !a.name.is_empty() || a.root.is_some() => {
            let open_if = a.attributes & OBJ_OPENIF != 0;
            let (object, existed) =
                namespace::create(a.root.as_ref(), &a.name, object, a.case_insensitive(), open_if)?;
            if existed && !same_type(&object) {
                return Err(NtStatus::OBJECT_TYPE_MISMATCH);
            }
            Ok((object, existed, a.attributes & HANDLE_ATTRIBUTES))
        }
        Some(a) => Ok((object, false, a.attributes & HANDLE_ATTRIBUTES)),
        None => Ok((object, false, 0)),
    }
}

fn is_mutant(o: &KObject) -> bool {
    matches!(o.body, ObjectBody::Mutant(_))
}

fn is_semaphore(o: &KObject) -> bool {
    matches!(o.body, ObjectBody::Semaphore(_))
}

/// Links a new object under its name (if any) and returns a handle.
fn create_object(
    call: &mut Call<'_>,
    out: u64,
    access: u32,
    attrs_ptr: u64,
    object: ObjectRef,
    same: fn(&KObject) -> bool,
) -> NtStatus {
    let attrs = tri!(call.object_attributes(attrs_ptr, DIRECTORY_CREATE_OBJECT));
    tri!(security_calls::assign_object(call, &object, attrs_ptr));
    let (object, existed, handle_attributes) = tri!(link_new_object(attrs, object, same));
    let status = if existed {
        call.return_opened(out, object, access, handle_attributes)
    } else {
        call.return_handle(out, object, access, handle_attributes)
    };
    if status == NtStatus::SUCCESS && existed { NtStatus::OBJECT_NAME_EXISTS } else { status }
}

/// NtCreateMutant(*MutantHandle, DesiredAccess, ObjectAttributes, InitialOwner)
fn nt_create_mutant(call: &mut Call<'_>) -> NtStatus {
    use super::object::{Mutant, MutantState};
    let (out, access, attrs_ptr) = (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2));
    let owned = arg!(call, 3) as u8 != 0;
    let state = match owned {
        true => MutantState { owner: thread::current_id(), count: 1, abandoned: false },
        false => MutantState::default(),
    };
    let mutant =
        Mutant { dispatcher: Dispatcher::new(i32::from(!owned)), state: IrqMutex::new(state) };
    let Ok(object) = KObject::new(ObjectBody::Mutant(mutant)) else { return NtStatus::NO_MEMORY };
    if owned {
        thread::note_mutant_owned(&object);
    }
    create_object(call, out, access, attrs_ptr, object, is_mutant)
}

/// NtOpenMutant(*MutantHandle, DesiredAccess, ObjectAttributes)
fn nt_open_mutant(call: &mut Call<'_>) -> NtStatus {
    let (out, access) = (arg!(call, 0), arg!(call, 1) as u32);
    let (object, attributes) = tri!(call.open_named(arg!(call, 2), is_mutant));
    call.return_opened(out, object, access, attributes & HANDLE_ATTRIBUTES)
}

/// NtReleaseMutant(MutantHandle, *PreviousCount)
fn nt_release_mutant(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let object = entry.object.clone();
    let ObjectBody::Mutant(m) = &object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let (previous, free) = {
        let mut state = m.state.lock();
        if state.owner.is_none() || state.owner != thread::current_id() {
            return NtStatus::MUTANT_NOT_OWNED;
        }
        let previous = state.current_count();
        state.count -= 1;
        if state.count == 0 {
            state.owner = None;
        }
        (previous, state.count == 0)
    };
    if free {
        thread::note_mutant_released(&object);
        wait::set_signal(&object, 1);
    }
    let ptr = arg!(call, 1);
    if ptr != 0 {
        tri!(call.write_u32(ptr, previous as u32));
    }
    NtStatus::SUCCESS
}

/// NtQueryMutant(MutantHandle, Class, Buffer, Length, *ReturnLength):
/// MUTANT_BASIC_INFORMATION { CurrentCount, OwnedByCaller, AbandonedState }.
fn nt_query_mutant(call: &mut Call<'_>) -> NtStatus {
    use super::object::MUTANT_QUERY_STATE;
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let object = tri!(entry.require(MUTANT_QUERY_STATE));
    let ObjectBody::Mutant(m) = &object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    if arg!(call, 1) as u32 != 0 {
        return NtStatus::INVALID_INFO_CLASS;
    }
    let state = *m.state.lock();
    let mut b = [0u8; 8];
    b[0..4].copy_from_slice(&state.current_count().to_le_bytes());
    b[4] = u8::from(state.owner.is_some() && state.owner == thread::current_id());
    b[5] = u8::from(state.abandoned);
    call.return_info(&b, arg!(call, 2), arg!(call, 3) as u32 as u64, arg!(call, 4))
}

/// NtCreateSemaphore(*SemaphoreHandle, DesiredAccess, ObjectAttributes,
///                   InitialCount, MaximumCount)
fn nt_create_semaphore(call: &mut Call<'_>) -> NtStatus {
    use super::object::{Semaphore, semaphore_counts_valid};
    let (out, access, attrs_ptr) = (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2));
    let (initial, maximum) = (arg!(call, 3) as u32 as i32, arg!(call, 4) as u32 as i32);
    if !semaphore_counts_valid(initial, maximum) {
        return NtStatus::INVALID_PARAMETER;
    }
    let semaphore = Semaphore { dispatcher: Dispatcher::new(initial), maximum };
    let Ok(object) = KObject::new(ObjectBody::Semaphore(semaphore)) else {
        return NtStatus::NO_MEMORY;
    };
    create_object(call, out, access, attrs_ptr, object, is_semaphore)
}

/// NtOpenSemaphore(*SemaphoreHandle, DesiredAccess, ObjectAttributes)
fn nt_open_semaphore(call: &mut Call<'_>) -> NtStatus {
    let (out, access) = (arg!(call, 0), arg!(call, 1) as u32);
    let (object, attributes) = tri!(call.open_named(arg!(call, 2), is_semaphore));
    call.return_opened(out, object, access, attributes & HANDLE_ATTRIBUTES)
}

/// NtReleaseSemaphore(SemaphoreHandle, ReleaseCount, *PreviousCount)
fn nt_release_semaphore(call: &mut Call<'_>) -> NtStatus {
    use super::object::{SEMAPHORE_MODIFY_STATE, semaphore_release};
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let object = tri!(entry.require(SEMAPHORE_MODIFY_STATE)).clone();
    let ObjectBody::Semaphore(s) = &object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let release = arg!(call, 1) as u32 as i32;
    let previous = s.dispatcher.signal_state();
    let next = tri!(semaphore_release(previous, s.maximum, release));
    wait::set_signal(&object, next);
    let ptr = arg!(call, 2);
    if ptr != 0 {
        tri!(call.write_u32(ptr, previous as u32));
    }
    NtStatus::SUCCESS
}

/// NtQuerySemaphore(SemaphoreHandle, Class, Buffer, Length, *ReturnLength):
/// SEMAPHORE_BASIC_INFORMATION { CurrentCount, MaximumCount }.
fn nt_query_semaphore(call: &mut Call<'_>) -> NtStatus {
    use super::object::SEMAPHORE_QUERY_STATE;
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let object = tri!(entry.require(SEMAPHORE_QUERY_STATE));
    let ObjectBody::Semaphore(s) = &object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    if arg!(call, 1) as u32 != 0 {
        return NtStatus::INVALID_INFO_CLASS;
    }
    let mut b = [0u8; 8];
    b[0..4].copy_from_slice(&s.dispatcher.signal_state().to_le_bytes());
    b[4..8].copy_from_slice(&s.maximum.to_le_bytes());
    call.return_info(&b, arg!(call, 2), arg!(call, 3) as u32 as u64, arg!(call, 4))
}

fn is_symbolic_link(o: &KObject) -> bool {
    matches!(o.body, ObjectBody::SymbolicLink(_))
}

/// NtCreateSymbolicLinkObject(*LinkHandle, DesiredAccess, ObjectAttributes,
///                            *LinkTarget)
fn nt_create_symbolic_link(call: &mut Call<'_>) -> NtStatus {
    let (out, access, attrs_ptr, target_ptr) =
        (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2), arg!(call, 3));
    if target_ptr == 0 {
        return NtStatus::INVALID_PARAMETER;
    }
    let target = tri!(call.unicode_string(target_ptr));
    let link = super::object::SymbolicLink { target };
    let Ok(object) = KObject::new(ObjectBody::SymbolicLink(link)) else {
        return NtStatus::NO_MEMORY;
    };
    create_object(call, out, access, attrs_ptr, object, is_symbolic_link)
}

/// NtOpenSymbolicLinkObject(*LinkHandle, DesiredAccess, ObjectAttributes)
fn nt_open_symbolic_link(call: &mut Call<'_>) -> NtStatus {
    let (out, access) = (arg!(call, 0), arg!(call, 1) as u32);
    // Opening a symbolic link opens the link, not its target.
    let (object, attributes) = tri!(call.open_named_with(arg!(call, 2), is_symbolic_link, false));
    call.return_handle(out, object, access, attributes & HANDLE_ATTRIBUTES)
}

/// NtQuerySymbolicLinkObject(LinkHandle, *LinkTarget, *ReturnedLength): the
/// target goes into the caller's UNICODE_STRING buffer (NUL-terminated if
/// there is room); ReturnedLength is the size needed with the terminator.
fn nt_query_symbolic_link(call: &mut Call<'_>) -> NtStatus {
    use super::object::SYMBOLIC_LINK_QUERY;
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let object = tri!(entry.require(SYMBOLIC_LINK_QUERY));
    let Some(target) = object.as_symbolic_link() else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let (string, ret_ptr) = (arg!(call, 1), arg!(call, 2));
    let mut header = [0u8; 16];
    if call.process.read(string, &mut header).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let maximum = usize::from(u16::from_le_bytes([header[2], header[3]]));
    let buffer = u64::from_le_bytes(header[8..16].try_into().unwrap_or([0; 8]));
    let bytes = target.len() * 2;
    let needed = bytes + 2;
    if ret_ptr != 0 {
        tri!(call.write_u32(ret_ptr, needed as u32));
    }
    if maximum < bytes {
        return NtStatus::BUFFER_TOO_SMALL;
    }
    let mut data = Vec::new();
    if data.try_reserve_exact(needed).is_err() {
        return NtStatus::NO_MEMORY;
    }
    for c in target {
        let _ = data.try_extend_from_slice(&c.to_le_bytes());
    }
    if maximum >= needed {
        let _ = data.try_extend_from_slice(&[0, 0]);
    }
    if call.process.write(buffer, &data).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    if call.process.write(string, &(bytes as u16).to_le_bytes()).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    NtStatus::SUCCESS
}

/// NtQueryDirectoryObject(DirectoryHandle, Buffer, Length, ReturnSingleEntry,
///                        RestartScan, *Context, *ReturnLength): entries
/// from index *Context on (0 with RestartScan); *Context moves past those
/// returned.
fn nt_query_directory_object(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let object = tri!(entry.require(super::object::DIRECTORY_QUERY)).clone();
    let (buffer, length) = (arg!(call, 1), arg!(call, 2) as u32 as usize);
    let (single, restart) = (arg!(call, 3) as u8 != 0, arg!(call, 4) as u8 != 0);
    let (context_ptr, ret_ptr) = (arg!(call, 5), arg!(call, 6));
    if context_ptr == 0 {
        return NtStatus::INVALID_PARAMETER;
    }
    let mut ctx = [0u8; 4];
    if call.process.read(context_ptr, &mut ctx).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let from = if restart { 0 } else { u32::from_le_bytes(ctx) as usize };
    let entries = tri!(namespace::directory_entries(&object, from));
    if entries.is_empty() {
        return NtStatus::NO_MORE_ENTRIES;
    }
    let Some((data, count)) = tri!(namespace::encode_directory(&entries, single, length, buffer))
    else {
        return NtStatus::BUFFER_TOO_SMALL;
    };
    if call.process.write(buffer, &data).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    tri!(call.write_u32(context_ptr, (from + count) as u32));
    if ret_ptr != 0 {
        tri!(call.write_u32(ret_ptr, data.len() as u32));
    }
    if count < entries.len() && !single { NtStatus::MORE_ENTRIES } else { NtStatus::SUCCESS }
}

fn is_section(o: &KObject) -> bool {
    matches!(o.body, ObjectBody::Section(_))
}

/// NtCreateSection(*SectionHandle, DesiredAccess, ObjectAttributes,
///                 *MaximumSize, SectionPageProtection, AllocationAttributes,
///                 FileHandle)
fn nt_create_section(call: &mut Call<'_>) -> NtStatus {
    let (out, access, attrs_ptr, size_ptr) =
        (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2), arg!(call, 3));
    let (protect, attributes, file) = (arg!(call, 4) as u32, arg!(call, 5) as u32, arg!(call, 6));
    if attributes & section::SEC_IMAGE != 0 {
        let body = tri!(image_section(call, file));
        return finish_create_section(call, out, access, attrs_ptr, body);
    }
    if file != 0 {
        let size = if size_ptr == 0 { 0 } else { tri!(call.read_u64(size_ptr)) };
        let body = tri!(file_section(call, file, size, protect));
        return finish_create_section(call, out, access, attrs_ptr, body);
    }
    if size_ptr == 0 {
        return NtStatus::INVALID_PARAMETER_4;
    }
    let size = tri!(call.read_u64(size_ptr));
    if size == 0 || size as i64 <= 0 {
        return NtStatus::INVALID_PARAMETER_4;
    }
    let body = tri!(section::Section::new(size, protect, attributes));
    finish_create_section(call, out, access, attrs_ptr, body)
}

/// A data section on an open file (a memory-mapped file): `size` 0 means
/// the file's size; a larger size extends the file (writable handles
/// only). The handle must allow what the protection allows.
fn file_section(
    call: &Call<'_>,
    file: u64,
    size: u64,
    protect: u32,
) -> Result<section::Section, NtStatus> {
    let entry = call.handle(Handle(file))?;
    let ObjectBody::File(f) = &entry.object.body else {
        return Err(NtStatus::OBJECT_TYPE_MISMATCH);
    };
    if f.directory {
        return Err(NtStatus::INVALID_FILE_FOR_SECTION);
    }
    let a = vad::access_of(protect)?;
    let writable = protect & (vad::PAGE_READWRITE | vad::PAGE_EXECUTE_READWRITE) != 0;
    let mut need = file::FILE_READ_DATA;
    if writable {
        need |= file::FILE_WRITE_DATA;
    }
    if a.execute {
        need |= FILE_EXECUTE;
    }
    if entry.access & need != need {
        return Err(NtStatus::ACCESS_DENIED);
    }
    let file_size = file::with_fs(|fs| fs.node(f.node).map_or(0, |n| n.size()));
    let size = if size == 0 { file_size } else { size };
    if size == 0 {
        return Err(NtStatus::MAPPED_FILE_SIZE_ZERO);
    }
    if size > file_size {
        if entry.access & file::FILE_WRITE_DATA == 0 {
            return Err(NtStatus::SECTION_TOO_BIG);
        }
        let now = platform::system_time();
        file::with_fs(|fs| fs.set_size(f.node, size, now)).map_err(file::fs_status)?;
    }
    section::Section::new_file(entry.object.clone(), f.node, size, protect)
}

const FILE_EXECUTE: u32 = 0x0020;

/// Builds a SEC_IMAGE section from the whole contents of an open file.
fn image_section(call: &Call<'_>, file: u64) -> Result<section::Section, NtStatus> {
    if file == 0 {
        return Err(NtStatus::INVALID_FILE_FOR_SECTION);
    }
    let entry = call.handle(Handle(file))?;
    let ObjectBody::File(f) = &entry.object.body else {
        return Err(NtStatus::OBJECT_TYPE_MISMATCH);
    };
    if f.directory {
        return Err(NtStatus::INVALID_FILE_FOR_SECTION);
    }
    if entry.access & file::FILE_READ_DATA == 0 {
        return Err(NtStatus::ACCESS_DENIED);
    }
    let mut section = section::Section::with_layout(section::image_layout(f.node)?);
    section.file_name = Some(file::nt_path(f.node)?);
    Ok(section)
}

fn finish_create_section(
    call: &mut Call<'_>,
    out: u64,
    access: u32,
    attrs_ptr: u64,
    body: section::Section,
) -> NtStatus {
    let attrs = tri!(call.object_attributes(attrs_ptr, DIRECTORY_CREATE_OBJECT));
    let Ok(object) = KObject::new(ObjectBody::Section(body)) else { return NtStatus::NO_MEMORY };
    tri!(security_calls::assign_object(call, &object, attrs_ptr));
    let (object, existed, handle_attributes) = tri!(link_new_object(attrs, object, is_section));
    let status = if existed {
        call.return_opened(out, object, access, handle_attributes)
    } else {
        call.return_handle(out, object, access, handle_attributes)
    };
    if status == NtStatus::SUCCESS && existed { NtStatus::OBJECT_NAME_EXISTS } else { status }
}

/// NtOpenSection(*SectionHandle, DesiredAccess, ObjectAttributes)
fn nt_open_section(call: &mut Call<'_>) -> NtStatus {
    let (out, access) = (arg!(call, 0), arg!(call, 1) as u32);
    let (object, attributes) = tri!(call.open_named(arg!(call, 2), is_section));
    call.return_opened(out, object, access, attributes & HANDLE_ATTRIBUTES)
}

const MEM_TOP_DOWN: u32 = 0x10_0000;

/// Section rights a view with `protect` needs.
fn view_access(protect: u32) -> Result<u32, NtStatus> {
    let a = vad::access_of(protect)?;
    let mut need = 0;
    if a.read {
        need |= SECTION_MAP_READ;
    }
    if a.write {
        need |= SECTION_MAP_WRITE;
    }
    if a.execute {
        need |= SECTION_MAP_EXECUTE;
    }
    Ok(need)
}

/// NtMapViewOfSection(SectionHandle, ProcessHandle, *BaseAddress, ZeroBits,
///                    CommitSize, *SectionOffset, *ViewSize,
///                    InheritDisposition, AllocationType, Win32Protect)
fn nt_map_view_of_section(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    tri!(call.check_current_process(Handle(arg!(call, 1))));
    let (base_ptr, offset_ptr, size_ptr) = (arg!(call, 2), arg!(call, 5), arg!(call, 6));
    let (allocation, protect) = (arg!(call, 8) as u32, arg!(call, 9) as u32);
    if base_ptr == 0 || size_ptr == 0 {
        return NtStatus::INVALID_PARAMETER;
    }
    let object = tri!(entry.require(tri!(view_access(protect))));
    if !is_section(object) {
        return NtStatus::OBJECT_TYPE_MISMATCH;
    }
    let base = tri!(call.read_u64(base_ptr));
    let offset = if offset_ptr == 0 { 0 } else { tri!(call.read_u64(offset_ptr)) };
    let size = tri!(call.read_u64(size_ptr));
    let top_down = allocation & MEM_TOP_DOWN != 0;
    let view =
        tri!(section::map_view(&call.process, object, base, offset, size, protect, top_down));
    tri!(call.write_u64(base_ptr, view.base));
    tri!(call.write_u64(size_ptr, view.size));
    if view.relocated { NtStatus::IMAGE_NOT_AT_BASE } else { NtStatus::SUCCESS }
}

/// NtUnmapViewOfSection(ProcessHandle, BaseAddress)
fn nt_unmap_view_of_section(call: &mut Call<'_>) -> NtStatus {
    tri!(call.check_current_process(Handle(arg!(call, 0))));
    match section::unmap_view(&call.process, arg!(call, 1)) {
        Ok(()) => NtStatus::SUCCESS,
        Err(e) => e,
    }
}

/// NtQuerySection(SectionHandle, SectionInformationClass, Buffer, Length,
///                *ReturnLength)
fn nt_query_section(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let (class, buffer, length, ret_ptr) =
        (arg!(call, 1), arg!(call, 2), arg!(call, 3), arg!(call, 4));
    // The class is a ULONG: only the low 32 bits are defined.
    let class = class as u32 as u64;
    let object = tri!(entry.require(SECTION_QUERY));
    let ObjectBody::Section(s) = &object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    if class != 0 {
        return NtStatus::INVALID_INFO_CLASS;
    }
    // SECTION_BASIC_INFORMATION { BaseAddress, AllocationAttributes, MaximumSize }
    let mut b = [0u8; 24];
    b[8..12].copy_from_slice(&s.attributes.to_le_bytes());
    b[16..24].copy_from_slice(&s.size.to_le_bytes());
    if ret_ptr != 0 {
        tri!(call.write_u64(ret_ptr, b.len() as u64));
    }
    if (length as usize) < b.len() {
        return NtStatus::INFO_LENGTH_MISMATCH;
    }
    match call.process.write(buffer, &b) {
        Ok(()) => NtStatus::SUCCESS,
        Err(_) => NtStatus::ACCESS_VIOLATION,
    }
}

impl Call<'_> {
    /// Writes an IO_STATUS_BLOCK { Status (pointer-sized), Information }.
    fn write_iosb(&self, ptr: u64, status: NtStatus, information: u64) -> Result<(), NtStatus> {
        let mut b = [0u8; 16];
        b[0..4].copy_from_slice(&status.0.to_le_bytes());
        b[8..16].copy_from_slice(&information.to_le_bytes());
        self.process.write(ptr, &b).map_err(|_| NtStatus::ACCESS_VIOLATION)
    }

    /// Resolves OBJECT_ATTRIBUTES naming a file to a path on the volume.
    fn file_path(&self, attrs_ptr: u64) -> Result<file::FilePath, NtStatus> {
        let (root, name, attributes) =
            self.raw_object_attributes(attrs_ptr)?.ok_or(NtStatus::INVALID_PARAMETER)?;
        self.resolve_file_name(root, name, attributes)
    }

    /// Resolves `name` relative to `root` (0, an object directory or an
    /// open directory file) to a path on the volume.
    fn resolve_file_name(
        &self,
        root: u64,
        name: Vec<u16>,
        attributes: u32,
    ) -> Result<file::FilePath, NtStatus> {
        let root = match root {
            0 => None,
            h => {
                let entry = self.handle(Handle(h))?;
                if let ObjectBody::File(f) = &entry.object.body {
                    // Relative to an open directory: no access needed on it.
                    if !f.directory {
                        return Err(NtStatus::INVALID_PARAMETER);
                    }
                    return Ok(file::FilePath { start: f.node, name });
                }
                let object = entry.require(DIRECTORY_TRAVERSE)?.clone();
                if object.as_directory().is_none() {
                    return Err(NtStatus::OBJECT_TYPE_MISMATCH);
                }
                Some(object)
            }
        };
        if name.is_empty() && root.is_none() {
            return Err(NtStatus::OBJECT_NAME_INVALID);
        }
        let attrs = Attributes { root, name, attributes };
        // File names are case-insensitive on our volume regardless of
        // OBJ_CASE_INSENSITIVE; the object directory part follows the flag.
        let (device, rest) =
            namespace::lookup_parse(attrs.root.as_ref(), &attrs.name, attrs.case_insensitive())?;
        if !matches!(device.body, ObjectBody::Volume) {
            return Err(NtStatus::OBJECT_TYPE_MISMATCH);
        }
        Ok(file::FilePath { start: crate::fs::ramfs::ROOT, name: rest })
    }

    /// Whether OBJECT_ATTRIBUTES name something below `\Device\Afd`.
    fn names_afd(&self, attrs_ptr: u64) -> Result<bool, NtStatus> {
        let Some((0, name, attributes)) = self.raw_object_attributes(attrs_ptr)? else {
            return Ok(false);
        };
        let ci = attributes & OBJ_CASE_INSENSITIVE != 0;
        Ok(namespace::lookup_parse(None, &name, ci)
            .is_ok_and(|(device, _)| matches!(device.body, ObjectBody::AfdDevice)))
    }

    /// The pipe name if OBJECT_ATTRIBUTES name something below
    /// `\Device\NamedPipe`.
    fn pipe_name(&self, attrs_ptr: u64) -> Result<Option<Vec<u16>>, NtStatus> {
        let Some((0, name, attributes)) = self.raw_object_attributes(attrs_ptr)? else {
            return Ok(None);
        };
        let ci = attributes & OBJ_CASE_INSENSITIVE != 0;
        Ok(match namespace::lookup_parse(None, &name, ci) {
            Ok((device, rest)) if matches!(device.body, ObjectBody::NamedPipeDevice) => Some(rest),
            _ => None,
        })
    }

    /// Whether OBJECT_ATTRIBUTES name `\Device\Null` (`\??\NUL`).
    /// The console object `\??\CONOUT$` or `\??\CONIN$` names (what
    /// Win32 opens for those names), if the attributes name one.
    fn names_console(&self, attrs_ptr: u64) -> Result<Option<ObjectBody>, NtStatus> {
        let Some((0, name, _)) = self.raw_object_attributes(attrs_ptr)? else { return Ok(None) };
        let is = |text: &str| {
            name.len() == text.len()
                && name
                    .iter()
                    .zip(text.bytes())
                    .all(|(&c, b)| u8::try_from(c).is_ok_and(|c| c.eq_ignore_ascii_case(&b)))
        };
        Ok(if is("\\??\\CONOUT$") {
            Some(ObjectBody::ConsoleOutput)
        } else if is("\\??\\CONIN$") {
            Some(ObjectBody::ConsoleInput)
        } else {
            None
        })
    }

    fn names_null(&self, attrs_ptr: u64) -> Result<bool, NtStatus> {
        let Some((0, name, attributes)) = self.raw_object_attributes(attrs_ptr)? else {
            return Ok(false);
        };
        let ci = attributes & OBJ_CASE_INSENSITIVE != 0;
        Ok(namespace::lookup_parse(None, &name, ci).is_ok_and(|(device, rest)| {
            matches!(device.body, ObjectBody::NullDevice) && rest.is_empty()
        }))
    }

    /// The handle attributes (OBJ_INHERIT) asked for in OBJECT_ATTRIBUTES.
    fn handle_attributes(&self, attrs_ptr: u64) -> Result<u32, NtStatus> {
        Ok(self.raw_object_attributes(attrs_ptr)?.map_or(0, |(_, _, a)| a & HANDLE_ATTRIBUTES))
    }

    /// Reads NtCreateFile's EA buffer (at most 4 KiB).
    fn ea_buffer(&self, ptr: u64, len: u64) -> Result<Vec<u8>, NtStatus> {
        if ptr == 0 || len == 0 {
            return Ok(Vec::new());
        }
        if len > 4096 {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let mut ea = Vec::new();
        ea.try_reserve_exact(len as usize).map_err(|_| NtStatus::NO_MEMORY)?;
        ea.resize(len as usize, 0);
        self.process.read(ptr, &mut ea).map_err(|_| NtStatus::ACCESS_VIOLATION)?;
        Ok(ea)
    }

    fn open_or_create(&self, r: OpenRequest) -> NtStatus {
        let OpenRequest { out, access, attrs_ptr, iosb, share, disposition, options, ea } = r;
        let mut granted = super::object::FILE_MAPPING.map(access);
        let (object, information) = if tri!(self.names_afd(attrs_ptr)) {
            let ea = tri!(self.ea_buffer(ea.0, ea.1));
            (tri!(super::afd::open(&ea)), file::FILE_OPENED)
        } else if let Some(body) = tri!(self.names_console(attrs_ptr)) {
            let Ok(object) = KObject::new(body) else { return NtStatus::NO_MEMORY };
            (object, file::FILE_OPENED)
        } else if tri!(self.names_null(attrs_ptr)) {
            let Ok(object) = KObject::new(ObjectBody::Null(Dispatcher::new(1))) else {
                return NtStatus::NO_MEMORY;
            };
            (object, file::FILE_OPENED)
        } else if let Some(name) = tri!(self.pipe_name(attrs_ptr)) {
            let synchronous = options
                & (file::FILE_SYNCHRONOUS_IO_ALERT | file::FILE_SYNCHRONOUS_IO_NONALERT)
                != 0;
            let end = tri!(super::pipe::open_client(&name, synchronous));
            let Ok(object) = KObject::new(ObjectBody::Pipe(end)) else {
                return NtStatus::NO_MEMORY;
            };
            (object, file::FILE_OPENED)
        } else {
            let path = tri!(self.file_path(attrs_ptr));
            // The access check (docs/SECURITY.md S3).
            granted = tri!(security_calls::file_access(self, &path, access, disposition, options));
            let created = tri!(file::create(&path, granted, share, disposition, options));
            if created.information == file::FILE_CREATED {
                tri!(security_calls::assign_file(self, &created.object, attrs_ptr));
            }
            (created.object, created.information)
        };
        let attributes = tri!(self.handle_attributes(attrs_ptr));
        let entry = HandleEntry { object, access: granted, attributes };
        let Ok(handle) = self.process.handles.lock().insert(entry) else {
            return NtStatus::NO_MEMORY;
        };
        if self.write_u64(out, handle.0).is_err() {
            self.process.handles.lock().close(handle);
            return NtStatus::ACCESS_VIOLATION;
        }
        tri!(self.write_iosb(iosb, NtStatus::SUCCESS, information));
        NtStatus::SUCCESS
    }
}

/// Arguments shared by NtCreateFile and NtOpenFile.
struct OpenRequest {
    out: u64,
    access: u32,
    attrs_ptr: u64,
    iosb: u64,
    share: u32,
    disposition: u32,
    options: u32,
    /// EaBuffer and EaLength (NtCreateFile only; the AFD open packet).
    ea: (u64, u64),
}

/// NtCreateNamedPipeFile(*FileHandle, DesiredAccess, ObjectAttributes,
///   *IoStatusBlock, ShareAccess, CreateDisposition, CreateOptions,
///   NamedPipeType, ReadMode, CompletionMode, MaximumInstances,
///   InboundQuota, OutboundQuota, *DefaultTimeout): an instance below
///   `\Device\NamedPipe` (see nt/pipe.rs). CompletionMode
///   (FILE_PIPE_COMPLETE_OPERATION) and the timeout are accepted and not
///   used.
fn nt_create_named_pipe_file(call: &mut Call<'_>) -> NtStatus {
    let (out, access, attrs_ptr, iosb) =
        (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2), arg!(call, 3));
    let (disposition, create_options) = (arg!(call, 5) as u32, arg!(call, 6) as u32);
    let (kind, read_mode) = (arg!(call, 7) as u32, arg!(call, 8) as u32);
    let max_instances = arg!(call, 10) as u32;
    let (inbound, outbound) = (arg!(call, 11) as u32, arg!(call, 12) as u32);
    if kind > 1 || read_mode > 1 || !(file::FILE_CREATE..=file::FILE_OPEN_IF).contains(&disposition)
    {
        return NtStatus::INVALID_PARAMETER;
    }
    let options = super::pipe::ServerOptions {
        inbound,
        outbound,
        max_instances,
        message_type: kind == 1,
        message_reads: read_mode == 1,
        first: disposition == file::FILE_CREATE,
        synchronous: create_options
            & (file::FILE_SYNCHRONOUS_IO_ALERT | file::FILE_SYNCHRONOUS_IO_NONALERT)
            != 0,
    };
    let Some(name) = tri!(call.pipe_name(attrs_ptr)) else {
        return NtStatus::OBJECT_NAME_INVALID;
    };
    if name.is_empty() {
        return NtStatus::OBJECT_NAME_INVALID;
    }
    let end = tri!(super::pipe::create_server(&name, &options));
    let Ok(object) = KObject::new(ObjectBody::Pipe(end)) else { return NtStatus::NO_MEMORY };
    let attributes = tri!(call.handle_attributes(attrs_ptr));
    let granted = super::object::FILE_MAPPING.map(access);
    let entry = HandleEntry { object, access: granted, attributes };
    let Ok(handle) = call.process.handles.lock().insert(entry) else {
        return NtStatus::NO_MEMORY;
    };
    if call.write_u64(out, handle.0).is_err() {
        call.process.handles.lock().close(handle);
        return NtStatus::ACCESS_VIOLATION;
    }
    tri!(call.write_iosb(iosb, NtStatus::SUCCESS, file::FILE_CREATED));
    NtStatus::SUCCESS
}

/// FSCTL_PIPE_PEEK: FILE_PIPE_PEEK_BUFFER { NamedPipeState,
/// ReadDataAvailable, NumberOfMessages, MessageLength, Data[] }.
const FSCTL_PIPE_PEEK: u32 = 0x0011_400c;

/// FSCTL_PIPE_DISCONNECT (DisconnectNamedPipe), FSCTL_PIPE_LISTEN
/// (ConnectNamedPipe).
const FSCTL_PIPE_DISCONNECT: u32 = 0x0011_0004;
const FSCTL_PIPE_LISTEN: u32 = 0x0011_0008;
/// FSCTL_PIPE_IMPERSONATE (ImpersonateNamedPipeClient).
const FSCTL_PIPE_IMPERSONATE: u32 = 0x0011_001c;

/// NtFsControlFile(FileHandle, Event, ApcRoutine, ApcContext,
///   *IoStatusBlock, FsControlCode, InputBuffer, InputBufferLength,
///   OutputBuffer, OutputBufferLength): FSCTL_PIPE_PEEK, _LISTEN,
///   _DISCONNECT and _IMPERSONATE on pipes.
fn nt_fs_control_file(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let (iosb, code, out, out_len) =
        (arg!(call, 4), arg!(call, 5) as u32, arg!(call, 8), arg!(call, 9) as u32 as usize);
    let ObjectBody::Pipe(end) = &entry.object.body else {
        return NtStatus::INVALID_DEVICE_REQUEST;
    };
    match code {
        FSCTL_PIPE_LISTEN => {
            let event = tri!(start_io(call, &entry.object, arg!(call, 1)));
            let request = tri!(pipe_request(call, end, event.clone(), iosb, 0, 0));
            let outcome = match end.listen(Some(request.clone())) {
                Ok(super::pipe::Outcome::Pending) => return NtStatus::PENDING,
                Ok(super::pipe::Outcome::Done(status, _)) => (status, 0, Vec::new()),
                Err(e) => (e, 0, Vec::new()),
            };
            return pipe_finish(call, &entry, end, request, event.as_ref(), outcome);
        }
        FSCTL_PIPE_DISCONNECT => {
            tri!(end.disconnect());
            tri!(call.write_iosb(iosb, NtStatus::SUCCESS, 0));
            return NtStatus::SUCCESS;
        }
        FSCTL_PIPE_IMPERSONATE => {
            // A server end with a client (gala has one user, so there is
            // nothing more to hand over).
            let state = end.state();
            let status = if !end.is_server() {
                NtStatus::ILLEGAL_FUNCTION
            } else if state == super::pipe::FILE_PIPE_CONNECTED_STATE
                || state == super::pipe::FILE_PIPE_CLOSING_STATE
            {
                NtStatus::SUCCESS
            } else {
                NtStatus::CANNOT_IMPERSONATE
            };
            if status == NtStatus::SUCCESS {
                let impersonated = token_calls::impersonate_pipe_client(call);
                if impersonated != NtStatus::SUCCESS {
                    return impersonated;
                }
                tri!(call.write_iosb(iosb, status, 0));
            }
            return status;
        }
        FSCTL_PIPE_PEEK => {}
        _ => {
            super::ledger::record("NtFsControlFile", "pipe control", u64::from(code));
            return NtStatus::INVALID_DEVICE_REQUEST;
        }
    }
    if out_len < 16 {
        return NtStatus::BUFFER_TOO_SMALL;
    }
    let (state, available, messages, message, data) = tri!(end.peek(out_len - 16));
    let mut header = [0u8; 16];
    header[0..4].copy_from_slice(&state.to_le_bytes());
    header[4..8].copy_from_slice(&(available as u32).to_le_bytes());
    header[8..12].copy_from_slice(&(messages as u32).to_le_bytes());
    header[12..16].copy_from_slice(&(message as u32).to_le_bytes());
    if call.process.write(out, &header).is_err() || call.process.write(out + 16, &data).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let limit = if message > 0 { message } else { available };
    let status = if data.len() < limit { NtStatus::BUFFER_OVERFLOW } else { NtStatus::SUCCESS };
    tri!(call.write_iosb(iosb, status, 16 + data.len() as u64));
    status
}

/// NtCreateFile(*FileHandle, DesiredAccess, ObjectAttributes, *IoStatusBlock,
///              *AllocationSize, FileAttributes, ShareAccess,
///              CreateDisposition, CreateOptions, EaBuffer, EaLength)
fn nt_create_file(call: &mut Call<'_>) -> NtStatus {
    let (out, access, attrs, iosb) =
        (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2), arg!(call, 3));
    let (share, disposition, options) =
        (arg!(call, 6) as u32, arg!(call, 7) as u32, arg!(call, 8) as u32);
    // EaLength is a ULONG: on the stack its upper half is garbage.
    let ea = (arg!(call, 9), arg!(call, 10) as u32 as u64);
    call.open_or_create(OpenRequest {
        out,
        access,
        attrs_ptr: attrs,
        iosb,
        share,
        disposition,
        options,
        ea,
    })
}

/// NtOpenFile(*FileHandle, DesiredAccess, ObjectAttributes, *IoStatusBlock,
///            ShareAccess, OpenOptions)
fn nt_open_file(call: &mut Call<'_>) -> NtStatus {
    let (out, access, attrs, iosb) =
        (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2), arg!(call, 3));
    let (share, options) = (arg!(call, 4) as u32, arg!(call, 5) as u32);
    call.open_or_create(OpenRequest {
        out,
        access,
        attrs_ptr: attrs,
        iosb,
        share,
        disposition: file::FILE_OPEN,
        options,
        ea: (0, 0),
    })
}

/// NtDeviceIoControlFile(FileHandle, Event, ApcRoutine, ApcContext,
///   *IoStatusBlock, IoControlCode, InputBuffer, InputBufferLength,
///   OutputBuffer, OutputBufferLength). Only AFD endpoints have device
/// controls so far. Requests complete before returning (see
/// docs/ARCHITECTURE.md, "AFD"): the IO_STATUS_BLOCK is written and the
/// event set on success.
fn nt_device_io_control_file(call: &mut Call<'_>) -> NtStatus {
    let (handle, event, apc, iosb, code) =
        (arg!(call, 0), arg!(call, 1), arg!(call, 2), arg!(call, 4), arg!(call, 5) as u32);
    let (input, input_len, output, output_len) =
        (arg!(call, 6), arg!(call, 7) as u32, arg!(call, 8), arg!(call, 9) as u32);
    let entry = tri!(call.handle(Handle(handle)));
    if !matches!(entry.object.body, ObjectBody::Socket(_)) {
        super::ledger::record("NtDeviceIoControlFile", "device", u64::from(code));
        return NtStatus::INVALID_DEVICE_REQUEST;
    }
    if apc != 0 {
        // Completion routines (WSARecv's lpCompletionRoutine): not yet.
        super::ledger::record("NtDeviceIoControlFile", "apc", u64::from(code));
        return NtStatus::NOT_SUPPORTED;
    }
    let event = match event {
        0 => None,
        h => Some(tri!(event_object(call, Handle(h), EVENT_MODIFY_STATE))),
    };
    if let Some(e) = &event {
        super::wait::set_signal(e, 0);
    }
    let request = super::afd::Request {
        process: &call.process,
        object: &entry.object,
        code,
        input,
        input_len,
        output,
        output_len,
    };
    let io = super::afd::Io { process: call.process.clone(), iosb, event, context: arg!(call, 3) };
    super::afd::ioctl(&request, io)
}

/// NtCancelIoFileEx(FileHandle, *IoRequestToCancel, *IoStatusBlock): cancels
/// the caller's pending I/O on a socket (the one whose IO_STATUS_BLOCK is
/// given, or all). gala's service number until verified on the pinned
/// build (docs/ARCHITECTURE.md, "AFD").
fn nt_cancel_io_file_ex(call: &mut Call<'_>) -> NtStatus {
    let (handle, request, iosb) = (arg!(call, 0), arg!(call, 1), arg!(call, 2));
    let entry = tri!(call.handle(Handle(handle)));
    let target = (request != 0).then_some(request);
    let cancelled = match &entry.object.body {
        ObjectBody::Socket(endpoint) => {
            super::afd::cancel(&endpoint.state, Some(&call.process), target)
        }
        // A directory's waiting change notifications; other file I/O
        // completes inside its call.
        ObjectBody::File(f) if f.directory => {
            super::notify::cancel(f.lock_owner(), &call.process, target)
        }
        ObjectBody::Pipe(end) => end.cancel(&call.process, target),
        _ => 0,
    };
    if cancelled == 0 {
        return NtStatus::NOT_FOUND;
    }
    if iosb != 0 {
        tri!(call.write_iosb(iosb, NtStatus::SUCCESS, 0));
    }
    NtStatus::SUCCESS
}

impl Call<'_> {
    /// `len` UTF-16 characters at `ptr` (at most 32 Ki).
    fn read_wide(&self, ptr: u64, len: u64) -> Result<Vec<u16>, NtStatus> {
        if len > 32 * 1024 {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(len as usize * 2).map_err(|_| NtStatus::NO_MEMORY)?;
        bytes.resize(len as usize * 2, 0u8);
        self.process.read(ptr, &mut bytes).map_err(|_| NtStatus::ACCESS_VIOLATION)?;
        let mut out = Vec::new();
        out.try_reserve_exact(len as usize).map_err(|_| NtStatus::NO_MEMORY)?;
        for pair in bytes.chunks_exact(2) {
            let _ = out.try_push(u16::from_le_bytes([pair[0], pair[1]]));
        }
        Ok(out)
    }
}

/// UTF-16 to UTF-8 (unpaired surrogates become U+FFFD).
fn utf8_of(wide: &[u16]) -> Result<Vec<u8>, NtStatus> {
    let mut out = Vec::new();
    for c in char::decode_utf16(wide.iter().copied()) {
        let mut b = [0u8; 4];
        let s = c.unwrap_or(char::REPLACEMENT_CHARACTER).encode_utf8(&mut b);
        out.try_extend_from_slice(s.as_bytes()).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    Ok(out)
}

/// GalaCreateProcess(*Parameters): gala's process creation (layout in
/// docs/ARCHITECTURE.md). Reads the image from the volume, builds the
/// process with the parent's inheritable and standard handles, and returns
/// handles to the process and its main thread.
fn gala_create_process(call: &mut Call<'_>) -> NtStatus {
    let ptr = arg!(call, 0);
    let mut b = [0u8; 120];
    if call.process.read(ptr, &mut b).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let u64_at = |at: usize| u64::from_le_bytes(b[at..at + 8].try_into().unwrap_or([0; 8]));
    let u32_at =
        |at: usize| u64::from(u32::from_le_bytes(b[at..at + 4].try_into().unwrap_or([0; 4])));
    let flags = u32_at(12);
    let (inherit, suspended) = (flags & 1 != 0, flags & 2 != 0);
    let image = tri!(call.read_wide(u64_at(0), u32_at(8)));
    let command_line = tri!(utf8_of(&tri!(call.read_wide(u64_at(16), u32_at(24)))));
    let directory = tri!(utf8_of(&tri!(call.read_wide(u64_at(32), u32_at(40)))));
    let environment = tri!(utf8_of(&tri!(call.read_wide(u64_at(48), u32_at(56)))));
    let std = [u64_at(64), u64_at(72), u64_at(80)];
    // Flag 4: only the handles listed at 120 (count at 128) are inherited
    // (PROC_THREAD_ATTRIBUTE_HANDLE_LIST).
    let mut list: Vec<u64> = Vec::new();
    if flags & 4 != 0 {
        let mut ext = [0u8; 16];
        if call.process.read(ptr + 120, &mut ext).is_err() {
            return NtStatus::ACCESS_VIOLATION;
        }
        let at = u64::from_le_bytes(ext[0..8].try_into().unwrap_or([0; 8]));
        let count = u32::from_le_bytes(ext[8..12].try_into().unwrap_or([0; 4]));
        if count > 256 {
            return NtStatus::INVALID_PARAMETER;
        }
        for i in 0..u64::from(count) {
            let h = tri!(call.read_u64(at + 8 * i));
            if list.try_push(h).is_err() {
                return NtStatus::NO_MEMORY;
            }
        }
    }

    // Flag 8: the C runtime's block (lpReserved2) at 136, length at 144,
    // for the child's RuntimeData.
    let mut runtime_data: Vec<u8> = Vec::new();
    if flags & 8 != 0 {
        let mut ext = [0u8; 16];
        if call.process.read(ptr + 136, &mut ext).is_err() {
            return NtStatus::ACCESS_VIOLATION;
        }
        let at = u64::from_le_bytes(ext[0..8].try_into().unwrap_or([0; 8]));
        let len = u32::from_le_bytes(ext[8..12].try_into().unwrap_or([0; 4])) as usize;
        if len > 8192 {
            return NtStatus::INVALID_PARAMETER;
        }
        if runtime_data.try_reserve_exact(len).is_err() {
            return NtStatus::NO_MEMORY;
        }
        runtime_data.resize(len, 0);
        if call.process.read(at, &mut runtime_data).is_err() {
            return NtStatus::ACCESS_VIOLATION;
        }
    }

    // Open and read the image (missing: STATUS_OBJECT_NAME_NOT_FOUND).
    let path = tri!(call.resolve_file_name(0, image.clone(), OBJ_CASE_INSENSITIVE));
    let opened = tri!(file::create(
        &path,
        file::FILE_READ_DATA,
        file::FILE_SHARE_READ,
        file::FILE_OPEN,
        file::FILE_NON_DIRECTORY_FILE
    ));
    let ObjectBody::File(f) = &opened.object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let layout = match section::image_layout(f.node) {
        Ok(layout) => layout,
        Err(NtStatus::NO_MEMORY) => return NtStatus::NO_MEMORY,
        Err(_) => return NtStatus::INVALID_IMAGE_FORMAT,
    };

    // Names: the Win32 path (without `\??\`) and the file name.
    let win32 = tri!(utf8_of(image.strip_prefix(&[0x5c, 0x3f, 0x3f, 0x5c][..]).unwrap_or(&image)));
    let win32 = core::str::from_utf8(&win32).unwrap_or("?");
    let file_name = win32.rsplit('\\').next().unwrap_or(win32);
    let (Ok(path_static), Ok(name)) = (process::intern(win32), process::intern(file_name)) else {
        return NtStatus::NO_MEMORY;
    };

    // The child's handles: inheritable ones and the standard handles, at
    // the parent's values.
    let mut table = super::object::HandleTable::default();
    {
        let parent = call.process.handles.lock();
        for (h, entry) in parent.iter() {
            let listed = flags & 4 == 0 || list.contains(&h.0);
            let wanted = (inherit && listed && entry.attributes & HANDLE_ATTRIBUTES != 0)
                || std.contains(&h.0);
            if wanted && table.insert_at(h, entry.clone()).is_err() {
                return NtStatus::NO_MEMORY;
            }
        }
    }

    // `NAME=value\0...\0\0` to a list of strings.
    let env_text = core::str::from_utf8(&environment).unwrap_or("");
    let mut vars: Vec<&str> = Vec::new();
    for var in env_text.split('\0').filter(|v| !v.is_empty()) {
        if vars.try_push(var).is_err() {
            return NtStatus::NO_MEMORY;
        }
    }
    let command_line = core::str::from_utf8(&command_line).unwrap_or("");
    let directory = core::str::from_utf8(&directory).unwrap_or("C:\\");
    // The caller's job: its limit, and the job the child joins.
    let job = tri!(job_calls::job_for_child(&call.process));
    let parent_token = call.process.token.lock().clone();
    let token = tri!(super::token::for_new_process(Some(&parent_token)));
    let spawned = process::spawn(process::Spawn {
        name,
        path: path_static,
        image: layout,
        command_line,
        current_directory: directory,
        environment: (!vars.is_empty()).then_some(&vars[..]),
        handles: Some((table, std)),
        suspended,
        runtime_data: &runtime_data,
        parent_id: call.process.id,
        token: Some(token),
    });
    let (child, thread_object) = match spawned {
        Ok(p) => p,
        Err(process::LoadError::Pe(_)) => return NtStatus::INVALID_IMAGE_FORMAT,
        Err(_) => return NtStatus::NO_MEMORY,
    };
    job_calls::join(&child, job);
    let thread_id = match &thread_object.body {
        ObjectBody::Thread(t) => t.id,
        _ => 0,
    };
    let child_id = child.id;
    let Ok(process_object) = KObject::new(ObjectBody::Process(child)) else {
        return NtStatus::NO_MEMORY;
    };
    let mut handles = call.process.handles.lock();
    let Ok(ph) = handles.insert(HandleEntry {
        object: process_object,
        access: PROCESS_ALL_ACCESS,
        attributes: 0,
    }) else {
        return NtStatus::NO_MEMORY;
    };
    let Ok(th) = handles.insert(HandleEntry {
        object: thread_object,
        access: THREAD_ALL_ACCESS,
        attributes: 0,
    }) else {
        return NtStatus::NO_MEMORY;
    };
    drop(handles);
    let mut out = [0u8; 32];
    out[0..8].copy_from_slice(&ph.0.to_le_bytes());
    out[8..16].copy_from_slice(&th.0.to_le_bytes());
    out[16..24].copy_from_slice(&child_id.to_le_bytes());
    out[24..32].copy_from_slice(&thread_id.to_le_bytes());
    if call.process.write(ptr + 88, &out).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    NtStatus::SUCCESS
}

/// ByteOffset values meaning "end of file" and "current position".
const FILE_WRITE_TO_END_OF_FILE: i64 = -1;
const FILE_USE_FILE_POINTER_POSITION: i64 = -2;

/// Chooses the offset of a read/write: explicit, or the file position for
/// synchronous handles (asynchronous handles must pass one).
fn io_offset(
    call: &Call<'_>,
    file: &file::FileObject,
    offset_ptr: u64,
    end: u64,
) -> Result<u64, NtStatus> {
    let explicit = if offset_ptr == 0 { None } else { Some(call.read_u64(offset_ptr)? as i64) };
    match explicit {
        Some(FILE_WRITE_TO_END_OF_FILE) => Ok(end),
        Some(FILE_USE_FILE_POINTER_POSITION) | None if file.synchronous => {
            Ok(*file.position.lock())
        }
        Some(o) if o >= 0 => Ok(o as u64),
        _ => Err(NtStatus::INVALID_PARAMETER),
    }
}

fn file_write(
    call: &Call<'_>,
    entry: &HandleEntry,
    file: &file::FileObject,
    iosb: u64,
    buffer: u64,
    length: u32,
    offset_ptr: u64,
) -> NtStatus {
    if file.directory {
        return NtStatus::INVALID_DEVICE_REQUEST;
    }
    if entry.access & (file::FILE_WRITE_DATA | file::FILE_APPEND_DATA) == 0 {
        return NtStatus::ACCESS_DENIED;
    }
    let size = file::with_fs(|fs| fs.node(file.node).map_or(0, |n| n.size()));
    let mut offset = tri!(io_offset(call, file, offset_ptr, size));
    if entry.access & file::FILE_WRITE_DATA == 0 {
        offset = size; // append-only handles always write at the end
    }
    tri!(super::locks::check_io(file.node, file.lock_owner(), offset, u64::from(length), true));
    let done = tri!(write_range(call, file, buffer, length, offset));
    if file.synchronous {
        *file.position.lock() = offset + done;
    }
    tri!(call.write_iosb(iosb, NtStatus::SUCCESS, done));
    NtStatus::SUCCESS
}

/// Writes `length` bytes from the caller's `buffer` at `offset`.
fn write_range(
    call: &Call<'_>,
    file: &file::FileObject,
    buffer: u64,
    length: u32,
    offset: u64,
) -> Result<u64, NtStatus> {
    let mut chunk = [0u8; 512];
    let mut done = 0u64;
    while done < u64::from(length) {
        let n = (u64::from(length) - done).min(chunk.len() as u64) as usize;
        if call.process.read(buffer + done, &mut chunk[..n]).is_err() {
            return Err(NtStatus::ACCESS_VIOLATION);
        }
        let now = platform::system_time();
        file::with_fs(|fs| fs.write(file.node, offset + done, &chunk[..n], now))
            .map_err(file::fs_status)?;
        done += n as u64;
    }
    Ok(done)
}

/// Reads up to `length` bytes at `offset` into the caller's `buffer`
/// (fewer at the end of the file).
fn read_range(
    call: &Call<'_>,
    file: &file::FileObject,
    buffer: u64,
    length: u32,
    offset: u64,
) -> Result<u64, NtStatus> {
    let mut chunk = [0u8; 512];
    let mut done = 0u64;
    while done < u64::from(length) {
        let want = (u64::from(length) - done).min(chunk.len() as u64) as usize;
        let n = file::with_fs(|fs| fs.read(file.node, offset + done, &mut chunk[..want]))
            .map_err(file::fs_status)?;
        if n == 0 {
            break;
        }
        if call.process.write(buffer + done, &chunk[..n]).is_err() {
            return Err(NtStatus::ACCESS_VIOLATION);
        }
        done += n as u64;
    }
    Ok(done)
}

/// NtWriteFileGather / NtReadFileScatter(FileHandle, Event, ApcRoutine,
/// ApcContext, *IoStatusBlock, SegmentArray, Length, *ByteOffset, *Key):
/// NtWriteFile/NtReadFile with the data in page-sized buffers (the
/// FILE_SEGMENT_ELEMENT array: one 64-bit pointer per page), completed the
/// same way (IO_STATUS_BLOCK, event, APC, completion port). gala's
/// numbers (unverified).
fn nt_file_segments(call: &mut Call<'_>, write: bool) -> NtStatus {
    const PAGE: u32 = 4096;
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let ObjectBody::File(file) = &entry.object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let (iosb, segments, length, offset_ptr) =
        (arg!(call, 4), arg!(call, 5), arg!(call, 6) as u32, arg!(call, 7));
    if file.directory {
        return NtStatus::INVALID_DEVICE_REQUEST;
    }
    let needed = if write { file::FILE_WRITE_DATA } else { file::FILE_READ_DATA };
    if entry.access & needed == 0 {
        return NtStatus::ACCESS_DENIED;
    }
    if offset_ptr == 0 || segments == 0 {
        return NtStatus::INVALID_PARAMETER;
    }
    let offset = tri!(call.read_u64(offset_ptr));
    let size = file::with_fs(|fs| fs.node(file.node).map_or(0, |n| n.size()));
    tri!(super::locks::check_io(file.node, file.lock_owner(), offset, u64::from(length), write));
    let apc = tri!(io_apc(call, file, iosb));
    let event = tri!(start_io(call, &entry.object, arg!(call, 1)));
    let transfer = || -> Result<u64, NtStatus> {
        if !write && offset >= size && length > 0 {
            return Err(NtStatus::END_OF_FILE);
        }
        let mut done = 0u64;
        let mut page = 0u64;
        while done < u64::from(length) {
            let buffer = call.read_u64(segments + 8 * page)?;
            let n = (u64::from(length) - done).min(u64::from(PAGE)) as u32;
            let moved = if write {
                write_range(call, file, buffer, n, offset + done)?
            } else {
                read_range(call, file, buffer, n, offset + done)?
            };
            done += moved;
            page += 1;
            if moved < u64::from(n) {
                break; // the end of the file
            }
        }
        Ok(done)
    };
    let status = match transfer() {
        Ok(done) => {
            tri!(call.write_iosb(iosb, NtStatus::SUCCESS, done));
            NtStatus::SUCCESS
        }
        Err(e) => {
            if e == NtStatus::END_OF_FILE {
                tri!(call.write_iosb(iosb, e, 0));
            }
            e
        }
    };
    finish_io(&entry.object, event.as_ref(), apc, status);
    post_completion(call, file, iosb, status);
    status
}

fn nt_write_file_gather(call: &mut Call<'_>) -> NtStatus {
    nt_file_segments(call, true)
}

fn nt_read_file_scatter(call: &mut Call<'_>) -> NtStatus {
    nt_file_segments(call, false)
}

/// NtLockFile(FileHandle, Event, ApcRoutine, ApcContext, *IoStatusBlock,
///            *ByteOffset, *Length, Key, FailImmediately, ExclusiveLock):
/// a byte-range lock (`locks`). A request that has to wait sleeps in the
/// call, whatever the handle's mode.
fn nt_lock_file(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let ObjectBody::File(file) = &entry.object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    if file.directory {
        return NtStatus::INVALID_PARAMETER;
    }
    if entry.access & (file::FILE_READ_DATA | file::FILE_WRITE_DATA) == 0 {
        return NtStatus::ACCESS_DENIED;
    }
    let iosb = arg!(call, 4);
    let (start, len) = (tri!(call.read_u64(arg!(call, 5))), tri!(call.read_u64(arg!(call, 6))));
    let key = arg!(call, 7) as u32;
    let (fail_now, exclusive) = (arg!(call, 8) as u8 != 0, arg!(call, 9) as u8 != 0);
    let apc = tri!(io_apc(call, file, iosb));
    let event = tri!(start_io(call, &entry.object, arg!(call, 1)));
    let owner = file.lock_owner();
    let status = match super::locks::lock(file.node, owner, key, start, len, exclusive, !fail_now) {
        Ok(()) => {
            tri!(call.write_iosb(iosb, NtStatus::SUCCESS, 0));
            NtStatus::SUCCESS
        }
        Err(e) => e,
    };
    finish_io(&entry.object, event.as_ref(), apc, status);
    post_completion(call, file, iosb, status);
    status
}

/// NtUnlockFile(FileHandle, *IoStatusBlock, *ByteOffset, *Length, Key):
/// releases exactly a range this file object locked with that key
/// (STATUS_RANGE_NOT_LOCKED otherwise).
fn nt_unlock_file(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let ObjectBody::File(file) = &entry.object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let (start, len) = (tri!(call.read_u64(arg!(call, 2))), tri!(call.read_u64(arg!(call, 3))));
    let key = arg!(call, 4) as u32;
    tri!(super::locks::unlock(file.node, file.lock_owner(), key, start, len));
    tri!(call.write_iosb(arg!(call, 1), NtStatus::SUCCESS, 0));
    NtStatus::SUCCESS
}

/// NtReadFile(FileHandle, Event, ApcRoutine, ApcContext, *IoStatusBlock,
///            Buffer, Length, *ByteOffset, *Key)
fn nt_read_file(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let (iosb, buffer, length, offset_ptr) =
        (arg!(call, 4), arg!(call, 5), arg!(call, 6) as u32, arg!(call, 7));
    if let ObjectBody::Pipe(end) = &entry.object.body {
        return pipe_io(call, &entry, end, iosb, buffer, length, false);
    }
    if let ObjectBody::Null(_) = &entry.object.body {
        let status = if length == 0 { NtStatus::SUCCESS } else { NtStatus::END_OF_FILE };
        tri!(call.write_iosb(iosb, status, 0));
        return status;
    }
    let ObjectBody::File(file) = &entry.object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let apc = tri!(io_apc(call, file, iosb));
    let event = tri!(start_io(call, &entry.object, arg!(call, 1)));
    let status = file_read(call, &entry, file, iosb, buffer, length, offset_ptr);
    finish_io(&entry.object, event.as_ref(), apc, status);
    post_completion(call, file, iosb, status);
    status
}

/// Posts a completed I/O to the file's completion port, if it has one:
/// (key, ApcContext, status, IoStatus.Information). An I/O that failed at
/// once posts nothing.
fn post_completion(call: &Call<'_>, file: &file::FileObject, iosb: u64, status: NtStatus) {
    if status.0 >= 0xc000_0000 {
        return;
    }
    let Some((port, key)) = file.completion.lock().clone() else { return };
    let information = call.read_u64(iosb + 8).unwrap_or(0);
    let packet = super::object::CompletionPacket {
        key,
        context: call.arg(3).unwrap_or(0),
        status: status.0,
        information,
    };
    post_packet(&port, packet);
}

fn post_packet(port: &ObjectRef, packet: super::object::CompletionPacket) -> NtStatus {
    let ObjectBody::IoCompletion(c) = &port.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    if c.queue.lock().push(packet).is_err() {
        return NtStatus::NO_MEMORY;
    }
    wait::set_signal(port, 1);
    NtStatus::SUCCESS
}

fn is_io_completion(o: &KObject) -> bool {
    matches!(o.body, ObjectBody::IoCompletion(_))
}

fn io_completion_object(call: &Call<'_>, h: u64, access: u32) -> Result<ObjectRef, NtStatus> {
    let entry = call.handle(Handle(h))?;
    let object = entry.require(access)?.clone();
    if is_io_completion(&object) { Ok(object) } else { Err(NtStatus::OBJECT_TYPE_MISMATCH) }
}

/// NtCreateIoCompletion(*Handle, DesiredAccess, ObjectAttributes, Count)
fn nt_create_io_completion(call: &mut Call<'_>) -> NtStatus {
    use super::object::{CompletionQueue, IoCompletion};
    let (out, access, attrs_ptr) = (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2));
    // TODO: the concurrency limit (Count) once there is more than one CPU.
    let port = IoCompletion {
        dispatcher: Dispatcher::new(0),
        queue: IrqMutex::new(CompletionQueue::default()),
    };
    let Ok(object) = KObject::new(ObjectBody::IoCompletion(port)) else {
        return NtStatus::NO_MEMORY;
    };
    create_object(call, out, access, attrs_ptr, object, is_io_completion)
}

/// NtSetIoCompletion(Handle, KeyContext, ApcContext, IoStatus, Information)
fn nt_set_io_completion(call: &mut Call<'_>) -> NtStatus {
    let port =
        tri!(io_completion_object(call, arg!(call, 0), super::object::IO_COMPLETION_MODIFY_STATE));
    let packet = super::object::CompletionPacket {
        key: arg!(call, 1),
        context: arg!(call, 2),
        status: arg!(call, 3) as u32,
        information: arg!(call, 4),
    };
    post_packet(&port, packet)
}

/// NtRemoveIoCompletion(Handle, *KeyContext, *ApcContext, *IoStatusBlock,
///                      *Timeout): the oldest packet, waiting for one.
fn nt_remove_io_completion(call: &mut Call<'_>) -> NtStatus {
    let port =
        tri!(io_completion_object(call, arg!(call, 0), super::object::IO_COMPLETION_MODIFY_STATE));
    let ObjectBody::IoCompletion(c) = &port.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let (key_ptr, context_ptr, iosb) = (arg!(call, 1), arg!(call, 2), arg!(call, 3));
    let timeout = tri!(call.timeout(arg!(call, 4)));
    let objects = [port.clone()];
    loop {
        if let Some(p) = c.queue.lock().pop() {
            tri!(call.write_u64(key_ptr, p.key));
            tri!(call.write_u64(context_ptr, p.context));
            tri!(call.write_iosb(iosb, NtStatus(p.status), p.information));
            return NtStatus::SUCCESS;
        }
        match wait::wait_for(&objects, false, timeout) {
            NtStatus::TIMEOUT => return NtStatus::TIMEOUT,
            NtStatus::SUCCESS => {}
            other => return other,
        }
    }
}

/// NtQueryIoCompletion(Handle, Class, Buffer, Length, *ReturnLength):
/// IO_COMPLETION_BASIC_INFORMATION { Depth }.
fn nt_query_io_completion(call: &mut Call<'_>) -> NtStatus {
    let port =
        tri!(io_completion_object(call, arg!(call, 0), super::object::IO_COMPLETION_QUERY_STATE));
    let ObjectBody::IoCompletion(c) = &port.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    if arg!(call, 1) as u32 != 0 {
        return NtStatus::INVALID_INFO_CLASS;
    }
    let depth = c.queue.lock().len() as u32;
    call.return_info(
        &depth.to_le_bytes(),
        arg!(call, 2),
        arg!(call, 3) as u32 as u64,
        arg!(call, 4),
    )
}

/// The I/O completion APC of a read or write (ApcRoutine, ApcContext):
/// queued to the calling thread when the I/O completes successfully, as
/// ApcRoutine(ApcContext, IoStatusBlock, 0). Synchronous handles take none.
fn io_apc(
    call: &Call<'_>,
    file: &file::FileObject,
    iosb: u64,
) -> Result<Option<thread::UserApc>, NtStatus> {
    let routine = call.arg(2)?;
    if routine == 0 {
        return Ok(None);
    }
    if file.synchronous {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    Ok(Some(thread::UserApc { routine, args: [call.arg(3)?, iosb, 0] }))
}

/// The start of an I/O on `file`: the event (if any; it must be an event
/// the caller may modify) and the file object are reset; they are signaled
/// again by `finish_io` when the I/O completes. Every I/O completes before
/// its system call returns so far.
// TODO: APCs and I/O completion ports.
fn start_io(call: &Call<'_>, file: &ObjectRef, event: u64) -> Result<Option<ObjectRef>, NtStatus> {
    let event = match event {
        0 => None,
        h => Some(event_object(call, Handle(h), EVENT_MODIFY_STATE)?),
    };
    if let Some(e) = &event {
        wait::set_signal(e, 0);
    }
    wait::set_signal(file, 0);
    Ok(event)
}

fn finish_io(
    file: &ObjectRef,
    event: Option<&ObjectRef>,
    apc: Option<thread::UserApc>,
    status: NtStatus,
) {
    wait::set_signal(file, 1);
    if let Some(e) = event {
        wait::set_signal(e, 1);
    }
    // A failed I/O queues no completion routine (ReadFileEx documents it).
    if let (Some(apc), Some(me)) = (apc, thread::current())
        && status.0 < 0x8000_0000
    {
        let _ = thread::queue_user_apc(&me, apc);
    }
}

fn file_read(
    call: &Call<'_>,
    entry: &HandleEntry,
    file: &file::FileObject,
    iosb: u64,
    buffer: u64,
    length: u32,
    offset_ptr: u64,
) -> NtStatus {
    if file.directory {
        return NtStatus::INVALID_DEVICE_REQUEST;
    }
    if entry.access & file::FILE_READ_DATA == 0 {
        return NtStatus::ACCESS_DENIED;
    }
    let size = file::with_fs(|fs| fs.node(file.node).map_or(0, |n| n.size()));
    let offset = tri!(io_offset(call, file, offset_ptr, size));
    tri!(super::locks::check_io(file.node, file.lock_owner(), offset, u64::from(length), false));
    if offset >= size && length > 0 {
        tri!(call.write_iosb(iosb, NtStatus::END_OF_FILE, 0));
        return NtStatus::END_OF_FILE;
    }
    let done = tri!(read_range(call, file, buffer, length, offset));
    if file.synchronous {
        *file.position.lock() = offset + done;
    }
    tri!(call.write_iosb(iosb, NtStatus::SUCCESS, done));
    NtStatus::SUCCESS
}

const FILE_BASIC_INFORMATION: u64 = 4;
const FILE_STANDARD_INFORMATION: u64 = 5;
const FILE_INTERNAL_INFORMATION: u64 = 6;
const FILE_NAME_INFORMATION: u64 = 9;
const FILE_ATTRIBUTE_TAG_INFORMATION: u64 = 35;
const FILE_RENAME_INFORMATION: u64 = 10;
const FILE_DISPOSITION_INFORMATION: u64 = 13;
const FILE_POSITION_INFORMATION: u64 = 14;
const FILE_MODE_INFORMATION: u64 = 16;
const FILE_COMPLETION_INFORMATION: u64 = 30;
/// FileIoCompletionNotificationInformation (SetFileCompletionNotificationModes).
const FILE_IO_COMPLETION_NOTIFICATION_INFORMATION: u64 = 41;
const FILE_END_OF_FILE_INFORMATION: u64 = 20;

/// NtQueryInformationFile(FileHandle, *IoStatusBlock, Buffer, Length, Class)
fn nt_query_information_file(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let (iosb, buffer, length, class) =
        (arg!(call, 1), arg!(call, 2), arg!(call, 3) as u32 as usize, arg!(call, 4));
    // The class is a ULONG: only the low 32 bits are defined.
    let class = class as u32 as u64;
    let ObjectBody::File(file) = &entry.object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    if class == FILE_NAME_INFORMATION {
        // FILE_NAME_INFORMATION { FileNameLength, FileName[] }: the path
        // from the volume root; what fits, with STATUS_BUFFER_OVERFLOW.
        if length < 4 {
            return NtStatus::INFO_LENGTH_MISMATCH;
        }
        let name = tri!(file::volume_path(file.node));
        let bytes = name.len() * 2;
        tri!(call.write_u32(buffer, bytes as u32));
        let fit = bytes.min(length - 4) / 2;
        for (i, c) in name[..fit].iter().enumerate() {
            if call.process.write(buffer + 4 + 2 * i as u64, &c.to_le_bytes()).is_err() {
                return NtStatus::ACCESS_VIOLATION;
            }
        }
        let status = if fit * 2 < bytes { NtStatus::BUFFER_OVERFLOW } else { NtStatus::SUCCESS };
        tri!(call.write_iosb(iosb, status, 4 + 2 * fit as u64));
        return status;
    }
    let mut out = [0u8; 40];
    let size = match class {
        FILE_BASIC_INFORMATION => {
            if entry.access & file::FILE_READ_ATTRIBUTES == 0 {
                return NtStatus::ACCESS_DENIED;
            }
            out = tri!(file::basic_information(file.node));
            40
        }
        FILE_STANDARD_INFORMATION => {
            out[..24].copy_from_slice(&tri!(file::standard_information(file.node)));
            24
        }
        FILE_MODE_INFORMATION => {
            out[..4].copy_from_slice(&file.mode.to_le_bytes());
            4
        }
        FILE_INTERNAL_INFORMATION => {
            // IndexNumber: the file's node, unique on the volume.
            out[..8].copy_from_slice(&file::index_number(file.node).to_le_bytes());
            8
        }
        FILE_ATTRIBUTE_TAG_INFORMATION => {
            // FileAttributes, ReparseTag (no reparse points on this volume).
            let basic = tri!(file::basic_information(file.node));
            out[..4].copy_from_slice(&basic[32..36]);
            8
        }
        FILE_POSITION_INFORMATION => {
            out[..8].copy_from_slice(&file.position.lock().to_le_bytes());
            8
        }
        _ => return NtStatus::INVALID_INFO_CLASS,
    };
    if length < size {
        return NtStatus::INFO_LENGTH_MISMATCH;
    }
    if call.process.write(buffer, &out[..size]).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    tri!(call.write_iosb(iosb, NtStatus::SUCCESS, size as u64));
    NtStatus::SUCCESS
}

/// NtSetInformationFile(FileHandle, *IoStatusBlock, Buffer, Length, Class)
/// NtSetInformationFile on a socket: the completion port
/// (CreateIoCompletionPort) and the notification modes
/// (SetFileCompletionNotificationModes).
fn set_socket_information(
    call: &Call<'_>,
    state: &super::afd::EndpointState,
    iosb: u64,
    buffer: u64,
    length: usize,
    class: u64,
) -> NtStatus {
    match class {
        FILE_COMPLETION_INFORMATION => {
            // FILE_COMPLETION_INFORMATION { HANDLE Port; PVOID Key; }
            if length < 16 {
                return NtStatus::INFO_LENGTH_MISMATCH;
            }
            let (port, key) = (tri!(call.read_u64(buffer)), tri!(call.read_u64(buffer + 8)));
            let port =
                tri!(io_completion_object(call, port, super::object::IO_COMPLETION_MODIFY_STATE));
            *state.completion.lock() = Some((port, key));
        }
        FILE_IO_COMPLETION_NOTIFICATION_INFORMATION => {
            // FILE_IO_COMPLETION_NOTIFICATION_INFORMATION { ULONG Flags; }:
            // modes can be added, not removed.
            if length < 4 {
                return NtStatus::INFO_LENGTH_MISMATCH;
            }
            let mut b = [0u8; 4];
            if call.process.read(buffer, &mut b).is_err() {
                return NtStatus::ACCESS_VIOLATION;
            }
            let flags = u32::from_le_bytes(b);
            if flags & !3 != 0 {
                super::ledger::record("NtSetInformationFile", "completion mode", u64::from(flags));
                return NtStatus::INVALID_PARAMETER;
            }
            state.modes.fetch_or(flags, core::sync::atomic::Ordering::AcqRel);
        }
        _ => return NtStatus::INVALID_INFO_CLASS,
    }
    tri!(call.write_iosb(iosb, NtStatus::SUCCESS, 0));
    NtStatus::SUCCESS
}

/// FilePipeInformation (23): FILE_PIPE_INFORMATION { ReadMode,
/// CompletionMode } (SetNamedPipeHandleState).
const FILE_PIPE_INFORMATION: u64 = 23;

/// NtSetInformationFile on a pipe end: the completion port, the
/// notification modes and the read mode (the completion mode is accepted:
/// no non-blocking pipes yet).
fn set_pipe_information(
    call: &Call<'_>,
    end: &super::pipe::PipeEnd,
    iosb: u64,
    buffer: u64,
    length: usize,
    class: u64,
) -> NtStatus {
    match class {
        FILE_COMPLETION_INFORMATION => {
            if length < 16 {
                return NtStatus::INFO_LENGTH_MISMATCH;
            }
            let (port, key) = (tri!(call.read_u64(buffer)), tri!(call.read_u64(buffer + 8)));
            let port =
                tri!(io_completion_object(call, port, super::object::IO_COMPLETION_MODIFY_STATE));
            *end.completion.lock() = Some((port, key));
        }
        FILE_IO_COMPLETION_NOTIFICATION_INFORMATION => {
            // ULONG Flags: modes can be added, not removed.
            if length < 4 {
                return NtStatus::INFO_LENGTH_MISMATCH;
            }
            let mut b = [0u8; 4];
            if call.process.read(buffer, &mut b).is_err() {
                return NtStatus::ACCESS_VIOLATION;
            }
            let flags = u32::from_le_bytes(b);
            if flags & !3 != 0 {
                super::ledger::record("NtSetInformationFile", "completion mode", u64::from(flags));
                return NtStatus::INVALID_PARAMETER;
            }
            end.modes.fetch_or(flags, core::sync::atomic::Ordering::AcqRel);
        }
        FILE_PIPE_INFORMATION => {
            if length < 8 {
                return NtStatus::INFO_LENGTH_MISMATCH;
            }
            let mut b = [0u8; 8];
            if call.process.read(buffer, &mut b).is_err() {
                return NtStatus::ACCESS_VIOLATION;
            }
            let read_mode = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            let completion = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
            if read_mode > 1 || completion > 1 {
                return NtStatus::INVALID_PARAMETER;
            }
            if completion == 1 {
                super::ledger::record("NtSetInformationFile", "pipe nowait", 1);
            }
            tri!(end.set_read_mode(read_mode == 1));
        }
        _ => return NtStatus::INVALID_INFO_CLASS,
    }
    tri!(call.write_iosb(iosb, NtStatus::SUCCESS, 0));
    NtStatus::SUCCESS
}

fn nt_set_information_file(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let (iosb, buffer, length, class) =
        (arg!(call, 1), arg!(call, 2), arg!(call, 3) as u32 as usize, arg!(call, 4));
    // The class is a ULONG: only the low 32 bits are defined.
    let class = class as u32 as u64;
    if let ObjectBody::Socket(endpoint) = &entry.object.body {
        return set_socket_information(call, &endpoint.state, iosb, buffer, length, class);
    }
    if let ObjectBody::Pipe(end) = &entry.object.body {
        return set_pipe_information(call, end, iosb, buffer, length, class);
    }
    let ObjectBody::File(file) = &entry.object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    if class == FILE_COMPLETION_INFORMATION {
        // FILE_COMPLETION_INFORMATION { HANDLE Port; PVOID Key; }
        if length < 16 {
            return NtStatus::INFO_LENGTH_MISMATCH;
        }
        let (port, key) = (tri!(call.read_u64(buffer)), tri!(call.read_u64(buffer + 8)));
        let port =
            tri!(io_completion_object(call, port, super::object::IO_COMPLETION_MODIFY_STATE));
        *file.completion.lock() = Some((port, key));
        tri!(call.write_iosb(iosb, NtStatus::SUCCESS, 0));
        return NtStatus::SUCCESS;
    }
    if class == FILE_BASIC_INFORMATION {
        // FILE_BASIC_INFORMATION { CreationTime, LastAccessTime,
        // LastWriteTime, ChangeTime, FileAttributes }: 0 (and -1, "stop
        // updating") keep a value. Access and change times are the write
        // time here.
        if length < 40 {
            return NtStatus::INFO_LENGTH_MISMATCH;
        }
        if entry.access & file::FILE_WRITE_ATTRIBUTES == 0 {
            return NtStatus::ACCESS_DENIED;
        }
        let mut b = [0u8; 40];
        if call.process.read(buffer, &mut b).is_err() {
            return NtStatus::ACCESS_VIOLATION;
        }
        let time = |at: usize| {
            let t = i64::from_le_bytes(b[at..at + 8].try_into().unwrap_or([0; 8]));
            (t > 0).then_some(t as u64)
        };
        let attributes = u32::from_le_bytes([b[32], b[33], b[34], b[35]]);
        let attributes = (attributes != 0).then_some(attributes);
        let (created, written) = (time(0), time(16));
        return match file::with_fs(|fs| fs.set_basic(file.node, created, written, attributes)) {
            Ok(()) => {
                tri!(call.write_iosb(iosb, NtStatus::SUCCESS, 0));
                NtStatus::SUCCESS
            }
            Err(e) => file::fs_status(e),
        };
    }
    if class == FILE_RENAME_INFORMATION {
        let status = rename_file(call, &entry, file, buffer, length);
        if status == NtStatus::SUCCESS {
            tri!(call.write_iosb(iosb, NtStatus::SUCCESS, 0));
        }
        return status;
    }
    let need = match class {
        FILE_POSITION_INFORMATION | FILE_END_OF_FILE_INFORMATION => 8,
        FILE_DISPOSITION_INFORMATION => 1,
        _ => return NtStatus::INVALID_INFO_CLASS,
    };
    if length < need {
        return NtStatus::INFO_LENGTH_MISMATCH;
    }
    let mut data = [0u8; 8];
    if call.process.read(buffer, &mut data[..need]).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let value = u64::from_le_bytes(data);
    let status = match class {
        FILE_POSITION_INFORMATION => {
            *file.position.lock() = value;
            NtStatus::SUCCESS
        }
        FILE_END_OF_FILE_INFORMATION => {
            if entry.access & file::FILE_WRITE_DATA == 0 {
                return NtStatus::ACCESS_DENIED;
            }
            let now = platform::system_time();
            match file::with_fs(|fs| fs.set_size(file.node, value, now)) {
                Ok(()) => NtStatus::SUCCESS,
                Err(e) => file::fs_status(e),
            }
        }
        _ => {
            // FILE_DISPOSITION_INFORMATION { BOOLEAN DeleteFile; }
            if entry.access & file::DELETE == 0 {
                return NtStatus::ACCESS_DENIED;
            }
            match file::with_fs(|fs| fs.set_delete_pending(file.node, data[0] != 0)) {
                Ok(()) => NtStatus::SUCCESS,
                Err(e) => file::fs_status(e),
            }
        }
    };
    if status == NtStatus::SUCCESS {
        tri!(call.write_iosb(iosb, NtStatus::SUCCESS, 0));
    }
    status
}

/// FileRenameInformation: FILE_RENAME_INFORMATION { BOOLEAN ReplaceIfExists;
/// HANDLE RootDirectory; ULONG FileNameLength; WCHAR FileName[]; } (24 bytes
/// with padding). A bare name renames within the same directory.
fn rename_file(
    call: &Call<'_>,
    entry: &HandleEntry,
    file: &file::FileObject,
    buffer: u64,
    length: usize,
) -> NtStatus {
    const HEADER: usize = 20;
    if entry.access & file::DELETE == 0 {
        return NtStatus::ACCESS_DENIED;
    }
    if length < 24 {
        return NtStatus::INFO_LENGTH_MISMATCH;
    }
    let mut header = [0u8; HEADER];
    if call.process.read(buffer, &mut header).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let replace = header[0] != 0;
    let root = u64::from_le_bytes(header[8..16].try_into().unwrap_or([0; 8]));
    let name_len = u32::from_le_bytes(header[16..20].try_into().unwrap_or([0; 4])) as usize;
    if name_len == 0 || !name_len.is_multiple_of(2) || HEADER + name_len > length {
        return NtStatus::INVALID_PARAMETER;
    }
    let mut bytes = Vec::new();
    if bytes.try_reserve_exact(name_len).is_err() {
        return NtStatus::NO_MEMORY;
    }
    bytes.resize(name_len, 0u8);
    if call.process.read(buffer + HEADER as u64, &mut bytes).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let mut name = Vec::new();
    if name.try_reserve_exact(name_len / 2).is_err() {
        return NtStatus::NO_MEMORY;
    }
    for pair in bytes.chunks_exact(2) {
        let _ = name.try_push(u16::from_le_bytes([pair[0], pair[1]]));
    }
    let node = file.node;
    let result = if root == 0 && !name.contains(&u16::from(b'\\')) {
        file::with_fs(|fs| {
            let parent = fs.node(node).ok_or(crate::fs::ramfs::FsError::NotFound)?.parent();
            fs.rename(node, parent, &name, replace)
        })
    } else {
        let path = tri!(call.resolve_file_name(root, name, OBJ_CASE_INSENSITIVE));
        let relative = path.start != crate::fs::ramfs::ROOT;
        let parts = tri!(file::components(&path.name, relative));
        let Some(last) = parts.last() else { return NtStatus::INVALID_PARAMETER };
        file::with_fs(|fs| {
            let parent = fs.parent_of(path.start, &parts)?;
            fs.rename(node, parent, last, replace)
        })
    };
    match result {
        Ok(()) => NtStatus::SUCCESS,
        Err(e) => file::fs_status(e),
    }
}

/// NtNotifyChangeDirectoryFile(FileHandle, Event, ApcRoutine, ApcContext,
///     IoStatusBlock, Buffer, Length, CompletionFilter, WatchTree): see
/// nt::notify. A synchronous handle's call waits for the result.
fn nt_notify_change_directory_file(call: &mut Call<'_>) -> NtStatus {
    use super::notify;
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let ObjectBody::File(dir) = &entry.object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    if !dir.directory {
        return NtStatus::INVALID_PARAMETER;
    }
    if entry.access & file::FILE_READ_DATA == 0 {
        // FILE_LIST_DIRECTORY
        return NtStatus::ACCESS_DENIED;
    }
    let (event_handle, iosb, buffer) = (arg!(call, 1), arg!(call, 4), arg!(call, 5));
    let (length, filter, tree) =
        (arg!(call, 6) as u32, arg!(call, 7) as u32, arg!(call, 8) as u8 != 0);
    if filter == 0 || filter & !0x17f != 0 {
        return NtStatus::INVALID_PARAMETER;
    }
    let event = tri!(start_io(call, &entry.object, event_handle));
    let apc = tri!(io_apc(call, dir, iosb)).and_then(|apc| thread::current().map(|t| (t, apc)));
    let done = if dir.synchronous {
        let e = Event { kind: EventType::Notification, dispatcher: Dispatcher::new(0) };
        match KObject::new(ObjectBody::Event(e)) {
            Ok(o) => Some(o),
            Err(_) => return NtStatus::NO_MEMORY,
        }
    } else {
        None
    };
    let request = notify::Request {
        process: call.process.clone(),
        iosb,
        buffer,
        length,
        event,
        port: dir.completion.lock().clone(),
        apc_context: arg!(call, 3),
        apc,
        file_signal: dir.dispatcher.clone(),
        done: done.clone(),
    };
    let key = dir.lock_owner();
    let node = dir.node;
    drop(entry);
    let status = notify::request(key, node, filter, tree, request);
    match (status, done) {
        (NtStatus::PENDING, Some(done)) => {
            let objects = [done];
            wait::wait_for(&objects, false, Timeout::Infinite);
            // The result is in the caller's IO_STATUS_BLOCK.
            let mut b = [0u8; 4];
            match call.process.read(iosb, &mut b) {
                Ok(()) => NtStatus(u32::from_le_bytes(b)),
                Err(_) => NtStatus::ACCESS_VIOLATION,
            }
        }
        (status, _) => status,
    }
}

/// NtQueryDirectoryFile(FileHandle, Event, ApcRoutine, ApcContext,
///                      *IoStatusBlock, FileInformation, Length,
///                      FileInformationClass, ReturnSingleEntry, *FileName,
///                      RestartScan)
// TODO: asynchronous handles (Event/APC completion).
fn nt_query_directory_file(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let (iosb, buffer, length, class) =
        (arg!(call, 4), arg!(call, 5), arg!(call, 6) as u32 as usize, arg!(call, 7) as u32);
    let (single, mask_ptr, restart) =
        (arg!(call, 8) as u8 != 0, arg!(call, 9), arg!(call, 10) as u8 != 0);
    let Some((fixed, _)) = file::directory_class_layout(class) else {
        return NtStatus::INVALID_INFO_CLASS;
    };
    if length < fixed {
        return NtStatus::INFO_LENGTH_MISMATCH;
    }
    let ObjectBody::File(dir) = &entry.object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    if !dir.directory {
        return NtStatus::INVALID_PARAMETER;
    }
    if entry.access & file::FILE_READ_DATA == 0 {
        // FILE_LIST_DIRECTORY
        return NtStatus::ACCESS_DENIED;
    }
    let mask = if mask_ptr == 0 { None } else { Some(tri!(call.unicode_string(mask_ptr))) };
    let listing = {
        let mut scan = dir.scan.lock();
        match scan.as_mut() {
            None => {
                let mask = match mask {
                    Some(m) if !m.is_empty() => m,
                    _ => {
                        let mut star = Vec::new();
                        if star.try_push(u16::from(b'*')).is_err() {
                            return NtStatus::NO_MEMORY;
                        }
                        star
                    }
                };
                *scan = Some(file::DirScan::new(mask));
            }
            Some(state) if restart => {
                state.restart();
                if let Some(m) = mask.filter(|m| !m.is_empty()) {
                    state.mask = m;
                }
            }
            Some(_) => {}
        }
        let Some(state) = scan.as_mut() else { return NtStatus::NO_MEMORY };
        tri!(file::list_directory(dir.node, state, class, length, single))
    };
    if call.process.write(buffer, &listing.data).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    if listing.status.0 < 0xc000_0000 {
        tri!(call.write_iosb(iosb, listing.status, listing.data.len() as u64));
    }
    listing.status
}

/// NtQueryVolumeInformationFile(FileHandle, *IoStatusBlock, FsInformation,
///                              Length, FsInformationClass)
fn nt_query_volume_information_file(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let (iosb, buffer, length, class) =
        (arg!(call, 1), arg!(call, 2), arg!(call, 3) as u32 as usize, arg!(call, 4) as u32);
    // FILE_FS_DEVICE_INFORMATION { DeviceType, Characteristics: 0 } of
    // console handles (FILE_DEVICE_CONSOLE) and pipes (FILE_DEVICE_NAMED_PIPE).
    let device = match entry.object.body {
        ObjectBody::ConsoleInput | ObjectBody::ConsoleOutput => Some(0x50u32),
        ObjectBody::Pipe(_) => Some(0x11),
        ObjectBody::Null(_) => Some(0x15), // FILE_DEVICE_NULL
        _ => None,
    };
    if let Some(device) = device
        && class == file::FILE_FS_DEVICE_INFORMATION
    {
        let mut info = [0u8; 8];
        info[0..4].copy_from_slice(&device.to_le_bytes());
        if length < info.len() {
            return NtStatus::INFO_LENGTH_MISMATCH;
        }
        if call.process.write(buffer, &info).is_err() {
            return NtStatus::ACCESS_VIOLATION;
        }
        tri!(call.write_iosb(iosb, NtStatus::SUCCESS, 8));
        return NtStatus::SUCCESS;
    }
    if !matches!(entry.object.body, ObjectBody::File(_)) {
        return NtStatus::OBJECT_TYPE_MISMATCH;
    }
    let (info, min) = tri!(file::volume_information(class));
    if length < min {
        return NtStatus::INFO_LENGTH_MISMATCH;
    }
    let n = info.len().min(length);
    if call.process.write(buffer, &info[..n]).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let status = if n < info.len() { NtStatus::BUFFER_OVERFLOW } else { NtStatus::SUCCESS };
    tri!(call.write_iosb(iosb, status, n as u64));
    status
}

/// NtFlushBuffersFile(FileHandle, *IoStatusBlock): the volume is in memory,
/// so only the access check remains.
fn nt_flush_buffers_file(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    if !matches!(entry.object.body, ObjectBody::File(_)) {
        return NtStatus::OBJECT_TYPE_MISMATCH;
    }
    if entry.access & (file::FILE_WRITE_DATA | file::FILE_APPEND_DATA) == 0 {
        return NtStatus::ACCESS_DENIED;
    }
    tri!(call.write_iosb(arg!(call, 1), NtStatus::SUCCESS, 0));
    NtStatus::SUCCESS
}

/// NtQueryAttributesFile(ObjectAttributes, *FileInformation)
fn nt_query_attributes_file(call: &mut Call<'_>) -> NtStatus {
    let (attrs, out) = (arg!(call, 0), arg!(call, 1));
    let path = tri!(call.file_path(attrs));
    let node = {
        let relative = path.start != crate::fs::ramfs::ROOT;
        let parts = tri!(file::components(&path.name, relative));
        tri!(file::with_fs(|fs| fs.resolve(path.start, &parts)).map_err(file::fs_status))
    };
    let info = tri!(file::basic_information(node));
    match call.process.write(out, &info) {
        Ok(()) => NtStatus::SUCCESS,
        Err(_) => NtStatus::ACCESS_VIOLATION,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_line_format() {
        let args = [Some(Some(0x24)), Some(None), Some(Some(0)), None];
        let line = TraceLine {
            pid: 4,
            tid: 8,
            number: 8,
            name: "NtWriteFile",
            args: &args,
            status: NtStatus::SUCCESS,
        };
        assert_eq!(line.to_string(), "trace: pid 4 tid 8 NtWriteFile(0x24, ?, 0x0) -> 0x00000000");
        let unknown = TraceLine {
            pid: 4,
            tid: 8,
            number: 0xfff,
            name: "",
            args: &[],
            status: NtStatus(0xc000_001c),
        };
        assert_eq!(unknown.to_string(), "trace: pid 4 tid 8 service 0xfff() -> 0xc000001c");
    }
}

#[path = "registry_calls.rs"]
mod registry_calls;

fn is_keyed_event(o: &KObject) -> bool {
    matches!(o.body, ObjectBody::KeyedEvent(_))
}

/// NtCreateKeyedEvent(*KeyedEventHandle, DesiredAccess, ObjectAttributes,
///                    Flags)
fn nt_create_keyed_event(call: &mut Call<'_>) -> NtStatus {
    use super::keyed_event::KeyedEvent;
    let (out, access, attrs) = (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2));
    let Ok(object) = KObject::new(ObjectBody::KeyedEvent(KeyedEvent::default())) else {
        return NtStatus::NO_MEMORY;
    };
    create_object(call, out, access, attrs, object, is_keyed_event)
}

/// NtOpenKeyedEvent(*KeyedEventHandle, DesiredAccess, ObjectAttributes)
fn nt_open_keyed_event(call: &mut Call<'_>) -> NtStatus {
    let (out, access) = (arg!(call, 0), arg!(call, 1) as u32);
    let (object, attributes) = tri!(call.open_named(arg!(call, 2), is_keyed_event));
    call.return_opened(out, object, access, attributes & HANDLE_ATTRIBUTES)
}

/// NtWaitForKeyedEvent / NtReleaseKeyedEvent(KeyedEventHandle, Key,
/// Alertable, *Timeout): a null handle is the system's keyed event.
fn keyed_event_call(call: &mut Call<'_>, release: bool) -> NtStatus {
    use super::keyed_event::{self, KEYEDEVENT_WAIT, KEYEDEVENT_WAKE};
    let h = arg!(call, 0);
    let object = if h == 0 {
        tri!(keyed_event::system())
    } else {
        let entry = tri!(call.handle(Handle(h)));
        tri!(entry.require(if release { KEYEDEVENT_WAKE } else { KEYEDEVENT_WAIT })).clone()
    };
    let (key, alertable) = (arg!(call, 1), arg!(call, 2) as u8 != 0);
    let timeout = tri!(call.timeout(arg!(call, 3)));
    let status = keyed_event::meet(&object, key, release, call.process.id, timeout, alertable);
    drop(object);
    deliver_apc_after(call, status)
}

fn nt_wait_for_keyed_event(call: &mut Call<'_>) -> NtStatus {
    keyed_event_call(call, false)
}

fn nt_release_keyed_event(call: &mut Call<'_>) -> NtStatus {
    keyed_event_call(call, true)
}

#[path = "job_calls.rs"]
mod job_calls;

#[path = "token_calls.rs"]
mod token_calls;

#[path = "security_calls.rs"]
mod security_calls;
