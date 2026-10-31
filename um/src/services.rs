//! The NT services behind gala-um's SIGSYS, U2a's bootstrap set: enough
//! for hello.exe, numbered as the kernel numbers them
//! (kernel/src/nt/syscalls.rs). U2b replaces this file with the NT layer's
//! own dispatcher.

use crate::host::{self, Call};
use crate::process;
use std::io::Write;

const NT_WRITE_FILE: u32 = 0x0008;
const NT_TERMINATE_PROCESS: u32 = 0x002c;

const STATUS_SUCCESS: u32 = 0;
const STATUS_NOT_IMPLEMENTED: u32 = 0xc000_0002;
const STATUS_INVALID_HANDLE: u32 = 0xc000_0008;
const STATUS_ACCESS_VIOLATION: u32 = 0xc000_0005;

/// The pseudo-handle for the current process.
const CURRENT_PROCESS: u64 = u64::MAX;

pub fn dispatch(call: &mut Call) -> u32 {
    match call.number() {
        NT_WRITE_FILE => nt_write_file(call),
        NT_TERMINATE_PROCESS => nt_terminate_process(call),
        n => {
            eprintln!("[warn ] um: NT service {n:#06x} is not in gala-um yet");
            STATUS_NOT_IMPLEMENTED
        }
    }
}

/// NtWriteFile(FileHandle, Event, ApcRoutine, ApcContext, IoStatusBlock,
/// Buffer, Length, ByteOffset, Key): the console handles only.
fn nt_write_file(call: &Call) -> u32 {
    let handle = call.arg(0);
    let (iosb, buffer, len) = (call.arg(4), call.arg(5), call.arg(6) as u32 as usize);
    if handle != process::STD_HANDLES[1] && handle != process::STD_HANDLES[2] {
        return STATUS_INVALID_HANDLE;
    }
    let Some(data) = host::user_bytes(buffer, len) else { return STATUS_ACCESS_VIOLATION };
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(data);
    let _ = out.flush();
    // IO_STATUS_BLOCK { Status (as a pointer-sized field), Information }.
    if !host::write_u64(iosb, u64::from(STATUS_SUCCESS)) || !host::write_u64(iosb + 8, len as u64) {
        return STATUS_ACCESS_VIOLATION;
    }
    STATUS_SUCCESS
}

/// NtTerminateProcess(ProcessHandle, ExitStatus): the current process.
fn nt_terminate_process(call: &Call) -> u32 {
    let handle = call.arg(0);
    if handle != CURRENT_PROCESS && handle != 0 {
        return STATUS_INVALID_HANDLE;
    }
    exit_process(call.arg(1) as u32)
}

/// Ends the program, logging its exit the way the kernel does.
pub fn exit_process(status: u32) -> ! {
    let _ = std::io::stdout().flush();
    eprintln!(
        "[info ] process {} (pid {}) exited with status {status:#010x}",
        process::name(),
        process::PROCESS_ID
    );
    std::process::exit(if status == 0 { 0 } else { 1 })
}
