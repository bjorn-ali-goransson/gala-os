//! The kernel's user-mode stack capture (`stack_at=` and `stack_from=`,
//! kernel/src/nt/stack_at.rs): when the option names a system service,
//! the kernel logs the calling thread's user stack the first few times a
//! process reaches it, so a program that stops inside a DLL gala has no
//! source for can still be placed by the call path that got there.
//!
//! This program reaches `NtWriteFile` by two paths on purpose. The first
//! three lines go straight to the system call, whose instruction is in
//! this program, with kernel32 on no frame of the stack; the last three
//! go through kernel32's `WriteFile`, so kernel32 is the return address
//! the capture names first. That difference is what the filter is tested
//! by:
//!
//! - `stack_at_test` (no filter, five captures) must capture all three
//!   of each kind in turn, so both this program and kernel32 are named,
//!   and must say the count is spent on the sixth write.
//! - `stack_at_from_test` (`stack_from=kernel32`, two captures) must
//!   pass the three raw writes over without spending a capture -- the
//!   line that says the count is spent says how many were passed over,
//!   and three is the whole proof that the filter chose rather than the
//!   count running out -- and capture the two kernel32 writes.
//!
//! Every write must still write all of its bytes: a capture reads the
//! stack and may not change what the call does.
//!
//! Prints `stack_at_test: ok` or exits with the number of the write that
//! did not go through.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Handle = *mut c_void;

const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, done: *mut u32, ov: *mut c_void) -> i32;
}

/// Writes `text` to standard output through kernel32, and whether all of
/// it was written.
fn out(text: &[u8]) -> bool {
    let mut written = 0u32;
    // SAFETY: a valid buffer, its own length, and a place for the count.
    let ok = unsafe {
        WriteFile(
            GetStdHandle(STD_OUTPUT_HANDLE),
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        )
    };
    ok != 0 && written as usize == text.len()
}

fn main() -> u32 {
    // Straight to the system call first, before kernel32 has been called
    // at all, so these stacks cannot have a word of it left on them.
    let raw: [&[u8]; 3] = [
        b"stack_at_test: raw one\r\n",
        b"stack_at_test: raw two\r\n",
        b"stack_at_test: raw three\r\n",
    ];
    let handle = ntrt::stdout();
    for (i, line) in raw.iter().enumerate() {
        if ntrt::write_file(handle, line) != 0 {
            return i as u32 + 1;
        }
    }
    // Then through kernel32, which is on each of these stacks.
    let through_kernel32: [&[u8]; 3] =
        [b"stack_at_test: k32 one\r\n", b"stack_at_test: k32 two\r\n", b"stack_at_test: ok\r\n"];
    for (i, line) in through_kernel32.iter().enumerate() {
        if !out(line) {
            return i as u32 + 4;
        }
    }
    0
}

ntrt::entry!(main);
