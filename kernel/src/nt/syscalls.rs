//! System service dispatch with the Windows x64 calling convention.
//!
//! Service numbers follow the pinned Windows build (see
//! `docs/ARCHITECTURE.md`, "NT personality"). They are black-box observable
//! values and must be verified against syscall traces from that build.

use alloc::vec::Vec;

use super::file;
use super::namespace;
use super::object::{
    DIRECTORY_CREATE_OBJECT, DIRECTORY_TRAVERSE, EVENT_MODIFY_STATE, EVENT_QUERY_STATE, Event,
    EventType, Handle, HandleEntry, KObject, ObjectBody, ObjectRef, PROCESS_ALL_ACCESS,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, SYNCHRONIZE, THREAD_ALL_ACCESS,
    THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME, THREAD_TERMINATE,
};
use super::object::{SECTION_MAP_EXECUTE, SECTION_MAP_READ, SECTION_MAP_WRITE, SECTION_QUERY};
use super::process::{self, Process};
use super::section;
use super::status::NtStatus;
use super::thread;
use super::vad;
use super::vm;
use super::wait::{self, Dispatcher, Timeout};
use crate::arch::syscall::{self, SyscallFrame, UserRegs};
use crate::console;
use crate::mm::heap::KArc;
use crate::mm::heap::TryVec;

pub const NT_WAIT_FOR_SINGLE_OBJECT: u32 = 0x0004;
pub const NT_READ_FILE: u32 = 0x0006;
pub const NT_WRITE_FILE: u32 = 0x0008;
pub const NT_QUERY_INFORMATION_FILE: u32 = 0x0011;
pub const NT_SET_INFORMATION_FILE: u32 = 0x0027;
pub const NT_OPEN_FILE: u32 = 0x0033;
pub const NT_QUERY_ATTRIBUTES_FILE: u32 = 0x003d;
pub const NT_CREATE_FILE: u32 = 0x0055;
pub const NT_MAP_VIEW_OF_SECTION: u32 = 0x0028;
pub const NT_UNMAP_VIEW_OF_SECTION: u32 = 0x002a;
pub const NT_OPEN_SECTION: u32 = 0x0037;
pub const NT_CREATE_SECTION: u32 = 0x004a;
pub const NT_QUERY_SECTION: u32 = 0x0051;
pub const NT_QUERY_INFORMATION_PROCESS: u32 = 0x0019;
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
pub const NT_PROTECT_VIRTUAL_MEMORY: u32 = 0x0050;

type Handler = fn(&mut Call<'_>) -> NtStatus;

const TABLE: &[(u32, &str, Handler)] = &[
    (NT_WAIT_FOR_SINGLE_OBJECT, "NtWaitForSingleObject", nt_wait_for_single_object),
    (NT_READ_FILE, "NtReadFile", nt_read_file),
    (NT_WRITE_FILE, "NtWriteFile", nt_write_file),
    (NT_QUERY_INFORMATION_FILE, "NtQueryInformationFile", nt_query_information_file),
    (NT_SET_INFORMATION_FILE, "NtSetInformationFile", nt_set_information_file),
    (NT_OPEN_FILE, "NtOpenFile", nt_open_file),
    (NT_QUERY_ATTRIBUTES_FILE, "NtQueryAttributesFile", nt_query_attributes_file),
    (NT_CREATE_FILE, "NtCreateFile", nt_create_file),
    (NT_MAP_VIEW_OF_SECTION, "NtMapViewOfSection", nt_map_view_of_section),
    (NT_UNMAP_VIEW_OF_SECTION, "NtUnmapViewOfSection", nt_unmap_view_of_section),
    (NT_OPEN_SECTION, "NtOpenSection", nt_open_section),
    (NT_CREATE_SECTION, "NtCreateSection", nt_create_section),
    (NT_QUERY_SECTION, "NtQuerySection", nt_query_section),
    (NT_QUERY_INFORMATION_PROCESS, "NtQueryInformationProcess", nt_query_information_process),
    (NT_QUERY_INFORMATION_THREAD, "NtQueryInformationThread", nt_query_information_thread),
    (NT_CREATE_THREAD, "NtCreateThread", nt_create_thread),
    (NT_RESUME_THREAD, "NtResumeThread", nt_resume_thread),
    (NT_TERMINATE_THREAD, "NtTerminateThread", nt_terminate_thread),
    (NT_QUERY_OBJECT, "NtQueryObject", nt_query_object),
    (NT_OPEN_EVENT, "NtOpenEvent", nt_open_event),
    (NT_OPEN_DIRECTORY_OBJECT, "NtOpenDirectoryObject", nt_open_directory_object),
    (NT_SET_EVENT, "NtSetEvent", nt_set_event),
    (NT_QUERY_PERFORMANCE_COUNTER, "NtQueryPerformanceCounter", nt_query_performance_counter),
    (NT_DELAY_EXECUTION, "NtDelayExecution", nt_delay_execution),
    (NT_DUPLICATE_OBJECT, "NtDuplicateObject", nt_duplicate_object),
    (NT_CLEAR_EVENT, "NtClearEvent", nt_clear_event),
    (NT_CREATE_EVENT, "NtCreateEvent", nt_create_event),
    (NT_QUERY_EVENT, "NtQueryEvent", nt_query_event),
    (NT_QUERY_SYSTEM_TIME, "NtQuerySystemTime", nt_query_system_time),
    (NT_WAIT_FOR_MULTIPLE_OBJECTS, "NtWaitForMultipleObjects", nt_wait_for_multiple_objects),
    (NT_CLOSE, "NtClose", nt_close),
    (NT_ALLOCATE_VIRTUAL_MEMORY, "NtAllocateVirtualMemory", nt_allocate_virtual_memory),
    (NT_FREE_VIRTUAL_MEMORY, "NtFreeVirtualMemory", nt_free_virtual_memory),
    (NT_QUERY_VIRTUAL_MEMORY, "NtQueryVirtualMemory", nt_query_virtual_memory),
    (NT_TERMINATE_PROCESS, "NtTerminateProcess", nt_terminate_process),
    (NT_PROTECT_VIRTUAL_MEMORY, "NtProtectVirtualMemory", nt_protect_virtual_memory),
];

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
    let Some(process) = process::current() else {
        panic!("syscall {number:#x} from a thread without a process");
    };
    let status = match TABLE.iter().find(|(n, _, _)| *n == number) {
        Some((_, _, handler)) => handler(&mut Call { frame, process }),
        None => {
            warn!("{}: unimplemented system service {number:#x}", process.name);
            NtStatus::INVALID_SYSTEM_SERVICE
        }
    };
    frame.rax = u64::from(status.0);
}

pub fn init() {
    syscall::init_bsp(dispatch);
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
        return file_write(call, &entry, file, io_status, buffer, length, offset_ptr);
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

/// NtClose(Handle)
fn nt_close(call: &mut Call<'_>) -> NtStatus {
    let handle = Handle(arg!(call, 0));
    match call.process.handles.lock().close(handle) {
        Some(_) => NtStatus::SUCCESS,
        None => NtStatus::INVALID_HANDLE,
    }
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
const MBI_SIZE: usize = 48;

/// NtQueryVirtualMemory(ProcessHandle, BaseAddress, MemoryInformationClass,
///                      Buffer, Length, *ReturnLength)
fn nt_query_virtual_memory(call: &mut Call<'_>) -> NtStatus {
    tri!(call.check_current_process(Handle(arg!(call, 0))));
    let (addr, class, buffer, length, ret_ptr) =
        (arg!(call, 1), arg!(call, 2), arg!(call, 3), arg!(call, 4), arg!(call, 5));
    // The class is a ULONG: only the low 32 bits are defined.
    let class = class as u32 as u64;
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
        if ptr == 0 {
            return Ok(None);
        }
        let mut raw = [0u8; OBJECT_ATTRIBUTES_SIZE as usize];
        self.process.read(ptr, &mut raw).map_err(|_| NtStatus::ACCESS_VIOLATION)?;
        let field = |off: usize| u64::from_le_bytes(raw[off..off + 8].try_into().unwrap_or([0; 8]));
        if u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) != OBJECT_ATTRIBUTES_SIZE {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let root = match field(8) {
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
        let name = match field(16) {
            0 => Vec::new(),
            p => self.unicode_string(p)?,
        };
        Ok(Some(Attributes { root, name, attributes: field(24) as u32 }))
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
        let attrs = self
            .object_attributes(attrs_ptr, DIRECTORY_TRAVERSE)?
            .ok_or(NtStatus::INVALID_PARAMETER)?;
        if attrs.name.is_empty() && attrs.root.is_none() {
            return Err(NtStatus::OBJECT_PATH_SYNTAX_BAD);
        }
        let object = namespace::lookup(attrs.root.as_ref(), &attrs.name, attrs.case_insensitive())?;
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
    let status = call.return_handle(out, object, access, handle_attributes);
    if status == NtStatus::SUCCESS && existed { NtStatus::OBJECT_NAME_EXISTS } else { status }
}

/// NtOpenEvent(*EventHandle, DesiredAccess, ObjectAttributes)
fn nt_open_event(call: &mut Call<'_>) -> NtStatus {
    let (out, access) = (arg!(call, 0), arg!(call, 1) as u32);
    let (object, attributes) =
        tri!(call.open_named(arg!(call, 2), |o| matches!(o.body, ObjectBody::Event(_))));
    call.return_handle(out, object, access, attributes & HANDLE_ATTRIBUTES)
}

/// NtOpenDirectoryObject(*DirectoryHandle, DesiredAccess, ObjectAttributes)
fn nt_open_directory_object(call: &mut Call<'_>) -> NtStatus {
    let (out, access) = (arg!(call, 0), arg!(call, 1) as u32);
    let (object, attributes) = tri!(call.open_named(arg!(call, 2), |o| o.as_directory().is_some()));
    call.return_handle(out, object, access, attributes & HANDLE_ATTRIBUTES)
}

const OBJECT_BASIC_INFORMATION: u64 = 0;
const OBJECT_NAME_INFORMATION: u64 = 1;
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
    let (class, buffer, length, ret_ptr) =
        (arg!(call, 1), arg!(call, 2), arg!(call, 3), arg!(call, 4));
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
    let objects = [object];
    wait::wait_for(&objects, false, timeout)
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
    wait::wait_for(&objects, wait_all, timeout)
}

/// NtDelayExecution(Alertable, *DelayInterval)
fn nt_delay_execution(call: &mut Call<'_>) -> NtStatus {
    let ptr = arg!(call, 1);
    if ptr == 0 {
        return NtStatus::ACCESS_VIOLATION;
    }
    wait::delay(tri!(call.timeout(ptr)));
    NtStatus::SUCCESS
}

const DUPLICATE_CLOSE_SOURCE: u32 = 1;
const DUPLICATE_SAME_ACCESS: u32 = 2;
const DUPLICATE_SAME_ATTRIBUTES: u32 = 4;

/// NtDuplicateObject(SourceProcess, SourceHandle, TargetProcess,
///                   *TargetHandle, DesiredAccess, HandleAttributes, Options)
fn nt_duplicate_object(call: &mut Call<'_>) -> NtStatus {
    tri!(call.check_current_process(Handle(arg!(call, 0))));
    let source = Handle(arg!(call, 1));
    tri!(call.check_current_process(Handle(arg!(call, 2))));
    let (out, access, attributes, options) =
        (arg!(call, 3), arg!(call, 4) as u32, arg!(call, 5) as u32, arg!(call, 6) as u32);
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
    if out != 0 && call.write_u64(out, handle.0).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    NtStatus::SUCCESS
}

use crate::time::PERFORMANCE_FREQUENCY;

/// NtQueryPerformanceCounter(*Counter, *Frequency)
fn nt_query_performance_counter(call: &mut Call<'_>) -> NtStatus {
    let (counter, frequency) = (arg!(call, 0), arg!(call, 1));
    tri!(call.write_u64(counter, crate::time::monotonic_ns() / 100));
    if frequency != 0 {
        tri!(call.write_u64(frequency, PERFORMANCE_FREQUENCY));
    }
    NtStatus::SUCCESS
}

/// NtQuerySystemTime(*SystemTime)
fn nt_query_system_time(call: &mut Call<'_>) -> NtStatus {
    tri!(call.write_u64(arg!(call, 0), crate::time::system_time()));
    NtStatus::SUCCESS
}

/// Offsets in the x64 CONTEXT: Rax..R15 at 0x78 (16 registers), Rip at 0xf8,
/// EFlags at 0x44.
const CONTEXT_EFLAGS: usize = 0x44;
const CONTEXT_GPR: usize = 0x78;
const CONTEXT_RIP: usize = 0xf8;

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
    let mut ctx = [0u8; CONTEXT_RIP + 8];
    if call.process.read(ctx_ptr, &mut ctx).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let word = |off: usize| u64::from_le_bytes(ctx[off..off + 8].try_into().unwrap_or([0; 8]));
    let mut regs = UserRegs::default();
    for (i, r) in regs.gpr.iter_mut().enumerate() {
        *r = word(CONTEXT_GPR + 8 * i);
    }
    regs.rip = word(CONTEXT_RIP);
    regs.rflags = u64::from(u32::from_le_bytes(
        ctx[CONTEXT_EFLAGS..CONTEXT_EFLAGS + 4].try_into().unwrap_or([0; 4]),
    ));
    // INITIAL_TEB { OldStackBase, OldStackLimit, StackBase, StackLimit, StackAllocationBase }
    let stack_base = tri!(call.read_u64(teb_ptr + 16));
    let stack_limit = tri!(call.read_u64(teb_ptr + 24));
    let object = tri!(thread::create(&call.process, regs, (stack_base, stack_limit), suspended));
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
    if class != 0 {
        return NtStatus::INVALID_INFO_CLASS;
    }
    // THREAD_BASIC_INFORMATION { ExitStatus, TebBaseAddress, ClientId,
    // AffinityMask, Priority, BasePriority }
    let mut b = [0u8; BASIC_INFORMATION_SIZE];
    b[0..4].copy_from_slice(&t.exit_status().0.to_le_bytes());
    b[8..16].copy_from_slice(&t.teb.to_le_bytes());
    b[16..24].copy_from_slice(&t.process.id.to_le_bytes());
    b[24..32].copy_from_slice(&t.id.to_le_bytes());
    b[32..40].copy_from_slice(&1u64.to_le_bytes());
    b[40..44].copy_from_slice(&8u32.to_le_bytes());
    call.return_info(&b, buffer, length, ret_ptr)
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
    if class != 0 {
        return NtStatus::INVALID_INFO_CLASS;
    }
    // PROCESS_BASIC_INFORMATION { ExitStatus, PebBaseAddress, AffinityMask,
    // BasePriority, UniqueProcessId, InheritedFromUniqueProcessId }
    let mut b = [0u8; BASIC_INFORMATION_SIZE];
    b[0..4].copy_from_slice(&p.exit_status().unwrap_or(NtStatus::PENDING).0.to_le_bytes());
    b[8..16].copy_from_slice(&super::layout::PEB_ADDRESS.to_le_bytes());
    b[16..24].copy_from_slice(&1u64.to_le_bytes());
    b[24..28].copy_from_slice(&8u32.to_le_bytes());
    b[32..40].copy_from_slice(&p.id.to_le_bytes());
    call.return_info(&b, buffer, length, ret_ptr)
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
        // TODO: file-backed data sections.
        return NtStatus::NOT_IMPLEMENTED;
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
    let size = file::with_fs(|fs| fs.node(f.node).map_or(0, |n| n.size()));
    let len = usize::try_from(size).map_err(|_| NtStatus::SECTION_TOO_BIG)?;
    let mut data = Vec::new();
    data.try_reserve_exact(len).map_err(|_| NtStatus::NO_MEMORY)?;
    data.resize(len, 0);
    let n = file::with_fs(|fs| fs.read(f.node, 0, &mut data)).map_err(file::fs_status)?;
    data.truncate(n);
    section::Section::new_image(&data)
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
    let (object, existed, handle_attributes) = tri!(link_new_object(attrs, object, is_section));
    let status = call.return_handle(out, object, access, handle_attributes);
    if status == NtStatus::SUCCESS && existed { NtStatus::OBJECT_NAME_EXISTS } else { status }
}

/// NtOpenSection(*SectionHandle, DesiredAccess, ObjectAttributes)
fn nt_open_section(call: &mut Call<'_>) -> NtStatus {
    let (out, access) = (arg!(call, 0), arg!(call, 1) as u32);
    let (object, attributes) = tri!(call.open_named(arg!(call, 2), is_section));
    call.return_handle(out, object, access, attributes & HANDLE_ATTRIBUTES)
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

    /// Resolves OBJECT_ATTRIBUTES naming a file to (volume, path on volume).
    fn file_path(&self, attrs_ptr: u64) -> Result<(ObjectRef, Vec<u16>), NtStatus> {
        let attrs = self
            .object_attributes(attrs_ptr, DIRECTORY_TRAVERSE)?
            .ok_or(NtStatus::INVALID_PARAMETER)?;
        if attrs.name.is_empty() && attrs.root.is_none() {
            return Err(NtStatus::OBJECT_NAME_INVALID);
        }
        // File names are case-insensitive on our volume regardless of
        // OBJ_CASE_INSENSITIVE; the object directory part follows the flag.
        let (device, rest) =
            namespace::lookup_parse(attrs.root.as_ref(), &attrs.name, attrs.case_insensitive())?;
        let mut owned = Vec::new();
        owned.try_extend_from_slice(rest).map_err(|_| NtStatus::NO_MEMORY)?;
        Ok((device, owned))
    }

    fn open_or_create(&self, r: OpenRequest) -> NtStatus {
        let OpenRequest { out, access, attrs_ptr, iosb, share, disposition, options } = r;
        let (device, rest) = tri!(self.file_path(attrs_ptr));
        let granted = super::object::FILE_MAPPING.map(access);
        let created = tri!(file::create(&device, &rest, granted, share, disposition, options));
        let information = created.information;
        let entry = HandleEntry { object: created.object, access: granted, attributes: 0 };
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
}

/// NtCreateFile(*FileHandle, DesiredAccess, ObjectAttributes, *IoStatusBlock,
///              *AllocationSize, FileAttributes, ShareAccess,
///              CreateDisposition, CreateOptions, EaBuffer, EaLength)
fn nt_create_file(call: &mut Call<'_>) -> NtStatus {
    let (out, access, attrs, iosb) =
        (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2), arg!(call, 3));
    let (share, disposition, options) =
        (arg!(call, 6) as u32, arg!(call, 7) as u32, arg!(call, 8) as u32);
    call.open_or_create(OpenRequest {
        out,
        access,
        attrs_ptr: attrs,
        iosb,
        share,
        disposition,
        options,
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
    })
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
    let mut chunk = [0u8; 512];
    let mut done = 0u64;
    while done < u64::from(length) {
        let n = (u64::from(length) - done).min(chunk.len() as u64) as usize;
        if call.process.read(buffer + done, &mut chunk[..n]).is_err() {
            return NtStatus::ACCESS_VIOLATION;
        }
        let now = crate::time::system_time();
        if let Err(e) = file::with_fs(|fs| fs.write(file.node, offset + done, &chunk[..n], now)) {
            return file::fs_status(e);
        }
        done += n as u64;
    }
    if file.synchronous {
        *file.position.lock() = offset + done;
    }
    tri!(call.write_iosb(iosb, NtStatus::SUCCESS, done));
    NtStatus::SUCCESS
}

/// NtReadFile(FileHandle, Event, ApcRoutine, ApcContext, *IoStatusBlock,
///            Buffer, Length, *ByteOffset, *Key)
fn nt_read_file(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let (iosb, buffer, length, offset_ptr) =
        (arg!(call, 4), arg!(call, 5), arg!(call, 6) as u32, arg!(call, 7));
    let ObjectBody::File(file) = &entry.object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    if file.directory {
        return NtStatus::INVALID_DEVICE_REQUEST;
    }
    if entry.access & file::FILE_READ_DATA == 0 {
        return NtStatus::ACCESS_DENIED;
    }
    let size = file::with_fs(|fs| fs.node(file.node).map_or(0, |n| n.size()));
    let offset = tri!(io_offset(call, file, offset_ptr, size));
    if offset >= size && length > 0 {
        tri!(call.write_iosb(iosb, NtStatus::END_OF_FILE, 0));
        return NtStatus::END_OF_FILE;
    }
    let mut chunk = [0u8; 512];
    let mut done = 0u64;
    while done < u64::from(length) {
        let want = (u64::from(length) - done).min(chunk.len() as u64) as usize;
        let n = match file::with_fs(|fs| fs.read(file.node, offset + done, &mut chunk[..want])) {
            Ok(n) => n,
            Err(e) => return file::fs_status(e),
        };
        if n == 0 {
            break;
        }
        if call.process.write(buffer + done, &chunk[..n]).is_err() {
            return NtStatus::ACCESS_VIOLATION;
        }
        done += n as u64;
    }
    if file.synchronous {
        *file.position.lock() = offset + done;
    }
    tri!(call.write_iosb(iosb, NtStatus::SUCCESS, done));
    NtStatus::SUCCESS
}

const FILE_BASIC_INFORMATION: u64 = 4;
const FILE_STANDARD_INFORMATION: u64 = 5;
const FILE_DISPOSITION_INFORMATION: u64 = 13;
const FILE_POSITION_INFORMATION: u64 = 14;
const FILE_END_OF_FILE_INFORMATION: u64 = 20;

/// NtQueryInformationFile(FileHandle, *IoStatusBlock, Buffer, Length, Class)
fn nt_query_information_file(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let (iosb, buffer, length, class) =
        (arg!(call, 1), arg!(call, 2), arg!(call, 3) as u32 as usize, arg!(call, 4));
    // The class is a ULONG: only the low 32 bits are defined.
    let class = class as u32 as u64;
    let ObjectBody::File(file) = &entry.object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
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
fn nt_set_information_file(call: &mut Call<'_>) -> NtStatus {
    let entry = tri!(call.handle(Handle(arg!(call, 0))));
    let (iosb, buffer, length, class) =
        (arg!(call, 1), arg!(call, 2), arg!(call, 3) as u32 as usize, arg!(call, 4));
    // The class is a ULONG: only the low 32 bits are defined.
    let class = class as u32 as u64;
    let ObjectBody::File(file) = &entry.object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
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
            let now = crate::time::system_time();
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

/// NtQueryAttributesFile(ObjectAttributes, *FileInformation)
fn nt_query_attributes_file(call: &mut Call<'_>) -> NtStatus {
    let (attrs, out) = (arg!(call, 0), arg!(call, 1));
    let (device, rest) = tri!(call.file_path(attrs));
    if !matches!(device.body, ObjectBody::Volume) {
        return NtStatus::OBJECT_TYPE_MISMATCH;
    }
    let node = {
        let Ok(parts) = file_components(&rest) else { return NtStatus::OBJECT_NAME_INVALID };
        tri!(file::with_fs(|fs| fs.resolve(&parts)).map_err(file::fs_status))
    };
    let info = tri!(file::basic_information(node));
    match call.process.write(out, &info) {
        Ok(()) => NtStatus::SUCCESS,
        Err(_) => NtStatus::ACCESS_VIOLATION,
    }
}

fn file_components(rest: &[u16]) -> Result<Vec<&[u16]>, NtStatus> {
    let mut out = Vec::new();
    for part in rest.split(|&c| c == u16::from(b'\\')).filter(|p| !p.is_empty()) {
        out.try_push(part).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    Ok(out)
}
