//! The Linux personality's groundwork (ROADMAP LX milestone 1; design:
//! ARCHITECTURE "Plan: a second personality (Linux)"; sources:
//! CLEAN_ROOM.md "Linux").
//!
//! The data processing, checked by host unit tests against what real
//! Linux does (user/lx's programs, whose transcripts `cargo xtask
//! lx-expect` records on Linux):
//! - [`elf`]: ELF64 executables parsed and validated, and their load plan.
//! - [`stack`]: the initial process stack (argc, argv, envp, auxv,
//!   strings) as bytes.
//! - [`syscalls`] and [`errno`]: Linux x86_64 system-call numbers and
//!   errno values, name <-> number.
//!
//!
//! And the personality itself (milestone 1), on the core's processes,
//! threads and address spaces (still `nt::Process` until seam 3):
//! - [`exec`]: a static or static-pie ELF from the file system as a new
//!   process of `Personality::Linux` (`lxrun=` on the kernel command
//!   line).
//! - [`dispatch`]: the Linux system calls, registered as that
//!   personality's dispatcher.
//! - [`mm`]: anonymous mmap, munmap, mprotect and brk on the VAD.
//! - [`state`]: per-process Linux state (break, fds, pipes, signals,
//!   parent and children, the output lines logged as `lx: pid <p>
//!   stdout: ..`).
//!
//! Milestone 2 so far:
//! - [`signal`]: signal actions, pending sets, masks, delivery decisions
//!   and the handler's frame (plain data, host-tested), on
//!   `lx_abi::signal`'s layouts; [`sigcalls`]: the signal system calls,
//!   SIGPIPE, and delivery on the way back from a system call.
//! - [`fork`]: fork (clone as a fork) with a copied address space, and
//!   wait4.

pub mod dispatch;
pub mod elf;
pub mod errno;
pub mod exec;
pub mod fork;
pub mod mm;
pub mod sigcalls;
pub mod signal;
pub mod stack;
pub mod state;
pub mod syscalls;

/// Registers the Linux personality's dispatcher (at start, after NT's).
pub fn init() {
    dispatch::init();
}

/// A process of the Linux personality has ended (`nt::process::finish`).
pub fn process_ended(pid: u64, status: crate::nt::status::NtStatus) {
    state::ended(pid, status);
}
