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
//! - [`fork`]: fork (clone as a fork) with a copy-on-write copy of the
//!   address space, and wait4.
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
//!
//! LD step 2's prerequisites:
//! - [`memfd`]: memfd_create and file seals: anonymous files whose
//!   pages every mapping shares.
//! - [`unix`]: AF_UNIX sockets (the core's `crate::unix`), SCM_RIGHTS,
//!   sendmsg/recvmsg.
//!
//! The WSL facade's kernel side (ROADMAP CT):
//! - [`distro`]: a distribution's tree unpacked node by node, its
//!   processes started from NT (rooted in its tree, with their starter's
//!   token, console and standard handles), counted and ended.
//! - [`ntio`]: Linux fds on the Windows side's pipes, consoles and files.

pub mod dispatch;
pub mod distro;
pub mod elf;
pub mod epoll;
pub mod errno;
pub mod eventfd;
pub mod exec;
pub mod file;
pub mod fork;
pub mod futex;
pub mod identity;
pub mod machine;
pub mod memfd;
pub mod mm;
pub mod ntio;
pub mod poll;
pub mod prctl;
pub mod resource;
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
pub mod unix;
pub mod wait;

/// Registers the Linux personality's dispatcher (at start, after NT's).
pub fn init() {
    dispatch::init();
}

/// A process of the Linux personality has ended (`nt::process::finish`,
/// its threads gone): its memory goes first, as Linux's exit releases it
/// before the parent is told (frames it shared copy-on-write are the
/// parent's alone again, and a section's frames are freed only after no
/// page table maps them: the address space itself may outlive the
/// process, held by a reap or `run`; its mappings release their files, so
/// a memfd mapped only by it can be sealed against writing once its parent
/// hears of the end, memfd_test), then its Linux state (`state::ended`).
pub fn process_ended(process: &crate::nt::process::Process, status: crate::nt::status::NtStatus) {
    // The robust futex lists of the threads that did not leave by exit(2)
    // (exit_group, a signal), as each of Linux's exiting threads releases
    // its own, before the memory goes.
    if let Some(lx) = state::get(process.id) {
        let mut lists: alloc::vec::Vec<(u64, u64)> = alloc::vec::Vec::new();
        let threads = lx.threads.lock();
        let wanted = threads.iter().filter(|t| t.robust_list != 0).count();
        if lists.try_reserve_exact(wanted).is_ok() {
            for t in threads.iter().filter(|t| t.robust_list != 0) {
                // Room reserved above.
                let _ = crate::mm::heap::TryVec::try_push(
                    &mut lists,
                    (lx.linux_tid(t.tid), t.robust_list),
                );
            }
        } else {
            crate::warn!(
                "lx: pid {} ended: no memory to release its threads' robust futex lists",
                process.id
            );
        }
        drop(threads);
        for (tid, head) in lists {
            release_robust_list(process, tid, head);
        }
    }
    if let Err(e) = mm::clear(process) {
        crate::warn!(
            "lx: pid {} ended: its memory not released (status {:#010x}); it goes with the process",
            process.id,
            e.0
        );
    }
    state::ended(process.id, status);
}

/// Releases thread `tid`'s robust futex list (`futex::release_robust`),
/// logging what it held: a lock left owned by a dead thread is what
/// explains a peer's hang, and a broken list what explains one not
/// released.
pub fn release_robust_list(process: &crate::nt::process::Process, tid: u64, head: u64) {
    let r = futex::release_robust(process, tid, head);
    if r.released != 0 || r.broken {
        crate::info!(
            "lx: pid {} thread {tid} ended holding {} robust locks ({} with waiters woken){}",
            process.id,
            r.released,
            r.woken,
            if r.broken { "; its list is broken (unreadable or too long)" } else { "" }
        );
    }
}

/// The state dump's Linux part: each process's threads (Linux and core
/// tids, mask, pending, whether blocked), its epoll sets, and the futex
/// waiters.
pub fn dump() {
    state::dump();
    futex::dump();
}
