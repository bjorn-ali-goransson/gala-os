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
//! - [`file`]: files and directories on the shared file system: Linux
//!   paths, open file descriptions, the stat family, getdents64, mkdir,
//!   unlink, rename, chdir/getcwd, umask; [`exec`]'s execve (with `#!`
//!   scripts) replaces a process's image.
//! - [`time`]: the clocks and sleeps; [`timer`]: ITIMER_REAL and alarm.
//! - [`socket`]: TCP sockets on the kernel's stack (`net::tcp`);
//!   [`poll`]: poll, ppoll, select, pselect6; [`epoll`]: epoll.
//! - [`threads`]: clone of threads, set_tid_address, a thread's exit;
//!   [`futex`]: futex(2); [`wait`]: wait queues and the one way a Linux
//!   thread blocks (a signal for it, the process's end or a timer
//!   interrupts it).

pub mod dispatch;
pub mod elf;
pub mod epoll;
pub mod errno;
pub mod exec;
pub mod file;
pub mod fork;
pub mod futex;
pub mod machine;
pub mod mm;
pub mod poll;
pub mod sigcalls;
pub mod signal;
pub mod socket;
pub mod stack;
pub mod state;
/// The system call table (x86_64's; aarch64 has a stub with the same
/// names, none of which any call has).
#[cfg(target_arch = "x86_64")]
pub mod syscalls;
#[cfg(target_arch = "aarch64")]
#[path = "syscalls_aarch64.rs"]
pub mod syscalls;
pub mod threads;
pub mod time;
pub mod timer;
pub mod wait;

/// Registers the Linux personality's dispatcher (at start, after NT's).
pub fn init() {
    dispatch::init();
}

/// A process of the Linux personality has ended (`nt::process::finish`).
pub fn process_ended(pid: u64, status: crate::nt::status::NtStatus) {
    state::ended(pid, status);
}

/// The state dump's Linux part: each process's threads (Linux and core
/// tids, mask, pending, whether blocked), its epoll sets, and the futex
/// waiters.
pub fn dump() {
    state::dump();
    futex::dump();
}
