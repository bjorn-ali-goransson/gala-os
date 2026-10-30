//! System service dispatch with the Windows x64 calling convention.
//!
//! Service numbers follow the pinned Windows build (see
//! `docs/ARCHITECTURE.md`, "NT personality"). They are black-box observable
//! values and must be verified against syscall traces from that build.

use super::object::{Handle, Object};
use super::process::{self, Process};
use super::status::NtStatus;
use super::vm;
use crate::arch::syscall::{self, SyscallFrame};
use crate::console;
use crate::mm::heap::KArc;

pub const NT_WRITE_FILE: u32 = 0x0008;
pub const NT_CLOSE: u32 = 0x000f;
pub const NT_ALLOCATE_VIRTUAL_MEMORY: u32 = 0x0018;
pub const NT_FREE_VIRTUAL_MEMORY: u32 = 0x001e;
pub const NT_QUERY_VIRTUAL_MEMORY: u32 = 0x0023;
pub const NT_TERMINATE_PROCESS: u32 = 0x002c;
pub const NT_PROTECT_VIRTUAL_MEMORY: u32 = 0x0050;

type Handler = fn(&mut Call<'_>) -> NtStatus;

const TABLE: &[(u32, &str, Handler)] = &[
    (NT_WRITE_FILE, "NtWriteFile", nt_write_file),
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
    match call.process.handles.lock().get(handle) {
        Some(Object::ConsoleOutput) => {}
        Some(_) => return NtStatus::OBJECT_TYPE_MISMATCH,
        None => return NtStatus::INVALID_HANDLE,
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
