//! System service dispatch with the Windows x64 calling convention.
//!
//! Service numbers follow the pinned Windows build (see
//! `docs/ARCHITECTURE.md`, "NT personality"). They are black-box observable
//! values and must be verified against syscall traces from that build.

use alloc::vec::Vec;

use super::object::{
    EVENT_MODIFY_STATE, EVENT_QUERY_STATE, Event, EventType, Handle, HandleEntry, KObject,
    ObjectBody, ObjectRef, SYNCHRONIZE,
};
use super::process::{self, Process};
use super::status::NtStatus;
use super::vm;
use super::wait::{self, Dispatcher, Timeout};
use crate::arch::syscall::{self, SyscallFrame};
use crate::console;
use crate::mm::heap::KArc;
use crate::mm::heap::TryVec;

pub const NT_WAIT_FOR_SINGLE_OBJECT: u32 = 0x0004;
pub const NT_WRITE_FILE: u32 = 0x0008;
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
    (NT_WRITE_FILE, "NtWriteFile", nt_write_file),
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
        self.process.handles.lock().get(h).ok_or(NtStatus::INVALID_HANDLE)
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
    match handle {
        // NULL: terminate all other threads of the current process; there
        // are none yet.
        Handle(0) => NtStatus::SUCCESS,
        Handle::CURRENT_PROCESS => process::terminate_current(status),
        _ => NtStatus::INVALID_HANDLE,
    }
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

const OBJECT_ATTRIBUTES_OBJECT_NAME: u64 = 0x10;

/// NtCreateEvent(*EventHandle, DesiredAccess, ObjectAttributes, EventType,
///               InitialState)
fn nt_create_event(call: &mut Call<'_>) -> NtStatus {
    let (out, access, attrs) = (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2));
    let kind = match arg!(call, 3) as u32 {
        0 => EventType::Notification,
        1 => EventType::Synchronization,
        _ => return NtStatus::INVALID_PARAMETER_4,
    };
    let initial = arg!(call, 4) as u8 != 0;
    if attrs != 0 && tri!(call.read_u64(attrs + OBJECT_ATTRIBUTES_OBJECT_NAME)) != 0 {
        // TODO: named objects (object directory namespace).
        return NtStatus::NOT_IMPLEMENTED;
    }
    let event = Event { kind, dispatcher: Dispatcher::new(i32::from(initial)) };
    let Ok(object) = KObject::new(ObjectBody::Event(event)) else { return NtStatus::NO_MEMORY };
    let entry = HandleEntry { access: object.mapping().map(access), object, attributes: 0 };
    let Ok(handle) = call.process.handles.lock().insert(entry) else { return NtStatus::NO_MEMORY };
    if call.write_u64(out, handle.0).is_err() {
        call.process.handles.lock().close(handle);
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

/// Performance counter frequency: 10 MHz (100 ns resolution), as on
/// current Windows.
const PERFORMANCE_FREQUENCY: u64 = 10_000_000;

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
