//! System service dispatch with the Windows x64 calling convention.
//!
//! Service numbers follow the pinned Windows build (see
//! `docs/ARCHITECTURE.md`, "NT personality"). They are black-box observable
//! values and must be verified against syscall traces from that build.

use super::object::{Handle, Object};
use super::process::{self, Process};
use super::status::NtStatus;
use crate::arch::syscall::{self, SyscallFrame};
use crate::console;
use crate::mm::heap::KArc;

pub const NT_WRITE_FILE: u32 = 0x0008;
pub const NT_CLOSE: u32 = 0x000f;
pub const NT_TERMINATE_PROCESS: u32 = 0x002c;

type Handler = fn(&mut Call<'_>) -> NtStatus;

const TABLE: &[(u32, &str, Handler)] = &[
    (NT_WRITE_FILE, "NtWriteFile", nt_write_file),
    (NT_CLOSE, "NtClose", nt_close),
    (NT_TERMINATE_PROCESS, "NtTerminateProcess", nt_terminate_process),
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
