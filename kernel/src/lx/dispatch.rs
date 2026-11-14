//! The Linux personality's system calls (ROADMAP LX milestone 1): the
//! dispatcher registered for `Personality::Linux`, indexed by
//! `syscalls::nr`, with Linux's register convention (number in rax,
//! arguments in rdi, rsi, rdx, r10, r8, r9, the result or `-errno` in
//! rax) and Linux's errors (man-pages; user/lx's transcripts recorded on
//! Linux decide).
//!
//! Observability: every call is a flight-recorder entry (`Syscall`, the
//! Linux number | pid << 32); a call not implemented here returns ENOSYS,
//! logs once per process and number `<name> (pid <p>): unimplemented
//! system call <name> (<nr>) (dispatcher lx::dispatch, personality linux)`
//! and counts in the ledger as `{"service":"<name>","what":"linux
//! syscall missing","detail":<nr>}`, so the next ones are chosen from
//! data. Calls accepted without their effect (ioctl's ENOTTY, wait4's
//! rusage) count as `"what":"linux syscall stub"`.
//!
//! The way back: after the call, signals pending and not blocked are
//! delivered (`sigcalls::deliver`); when a handler frame was pushed, or
//! rt_sigreturn put back a saved context (`Call::resume`), the call
//! returns with every register (`platform::continue_with`) instead of
//! SYSRET's rcx/r11 convention.

use alloc::vec::Vec;
use core::cell::Cell;

use super::errno::{
    self, EAGAIN, EBADF, EBUSY, EFAULT, EINVAL, ENOMEM, ENOSYS, ENOTTY, EPERM, EPIPE, ESPIPE,
};
use super::file;
use super::sigcalls;
use super::socket;
use super::state::{self, Fd, LxProcess, Object, PIPE_MAX, PIPE_PAGE, Stream};
use super::syscalls::{self, nr};
use super::time;
use super::unix;
use crate::mm::heap::{KArc, TryVec};
use crate::nt::object::ObjectBody;
use crate::nt::platform::{self, Personality, SyscallFrame, UserRegs};
use crate::nt::process::{self, Process};
use crate::nt::status::NtStatus;
use crate::nt::{ledger, thread};

/// The dispatcher's name in logs.
pub const DISPATCHER: &str = "lx::dispatch";

/// Registers the Linux dispatcher (at start, beside NT's).
pub fn init() {
    platform::set_syscall_handler(Personality::Linux, DISPATCHER, dispatch);
}

/// Linux caps one read or write at this many bytes (MAX_RW_COUNT).
const MAX_RW: u64 = 0x7fff_f000;
/// The most bytes one console write copies in at a time.
const CHUNK: usize = 64 * 1024;
/// writev's iovcnt limit (UIO_MAXIOV).
const IOV_MAX: u64 = 1024;

const O_NONBLOCK: u64 = 0o4000;
const O_CLOEXEC: u64 = 0o2000000;
/// creat(2) is open with these.
const CREAT_FLAGS: u64 = 0o1 | 0o100 | 0o1000;
const AT_REMOVEDIR: u64 = 0x200;
const F_GETFD: u64 = 1;
const F_SETFD: u64 = 2;
const F_GETFL: u64 = 3;
const F_SETFL: u64 = 4;
const F_DUPFD: u64 = 0;
const F_DUPFD_CLOEXEC: u64 = 1030;
const F_SETPIPE_SZ: u64 = 1031;
const F_GETPIPE_SZ: u64 = 1032;
const F_ADD_SEALS: u64 = 1033;
const F_GET_SEALS: u64 = 1034;
const FD_CLOEXEC: u64 = 1;
/// Writes up to this size to a pipe are atomic (PIPE_BUF).
const PIPE_BUF: usize = 4096;
const ARCH_SET_GS: u64 = 0x1001;
const ARCH_SET_FS: u64 = 0x1002;
const ARCH_GET_FS: u64 = 0x1003;
const ARCH_GET_GS: u64 = 0x1004;

/// One system call's context.
pub struct Call<'a> {
    pub process: KArc<Process>,
    pub lx: KArc<LxProcess>,
    pub frame: &'a SyscallFrame,
    pub tid: u64,
    /// Set by rt_sigreturn: the registers the call returns with.
    pub resume: Cell<Option<UserRegs>>,
}

impl Call<'_> {
    /// System call argument `i` (where the architecture's Linux ABI has
    /// it: `machine::syscall_arg`).
    fn arg(&self, i: usize) -> u64 {
        super::machine::syscall_arg(self.frame, i)
    }
}

pub type Result = core::result::Result<u64, u16>;

/// The user registers a system call returns with (x86-64's SYSRET: rcx
/// holds the return address and r11 the flags, the `syscall`
/// instruction's convention, which Linux programs expect).
pub fn return_regs(f: &SyscallFrame) -> UserRegs {
    f.return_regs()
}

fn dispatch(frame: &mut SyscallFrame) {
    let number = frame.number();
    let Some(me) = thread::current() else {
        panic!("linux syscall {number} from a thread without a process");
    };
    let ObjectBody::Thread(t) = &me.body else { unreachable!("thread objects are threads") };
    let process = t.process.clone();
    let tid = t.id;
    drop(me);
    crate::flight::record(crate::flight::Kind::Syscall, number | (process.id << 32), tid);
    let Some(lx) = state::get(process.id) else {
        crate::warn!("lx: pid {}: system call {number} with no Linux state", process.id);
        frame.set_result(errno::to_return(ENOSYS));
        return;
    };
    let call = Call { process, lx, frame, tid, resume: Cell::new(None) };
    let result = handle(&call, number);
    let rax = match result {
        Ok(v) => v,
        Err(e) => errno::to_return(e),
    };
    // An expired timer's SIGALRM is delivered with the others below.
    super::timer::fire(&call);
    crate::flight::record(crate::flight::Kind::SyscallDone, number, rax);
    let Call { process, lx, resume, .. } = call;
    frame.set_result(rax);
    let resume = resume.take();
    let mut regs = resume.unwrap_or_else(|| return_regs(frame));
    let delivered = sigcalls::deliver(&process, &lx, tid, number, resume.is_some(), &mut regs);
    if delivered || resume.is_some() {
        platform::continue_with(regs);
    }
}

fn handle(call: &Call<'_>, number: u64) -> Result {
    let a = |i| call.arg(i);
    let Ok(n) = u32::try_from(number) else { return missing(call, number) };
    match n {
        nr::READ => read(call, a(0), a(1), a(2)),
        nr::WRITE => write(call, a(0), a(1), a(2)),
        nr::CLOSE => {
            if call.lx.close(a(0)) {
                Ok(0)
            } else {
                Err(EBADF)
            }
        }
        nr::MMAP => super::mm::mmap(call, a(0), a(1), a(2), a(3), a(4) as i32 as i64, a(5)),
        nr::MPROTECT => super::mm::mprotect(&call.process, a(0), a(1), a(2)),
        nr::MUNMAP => super::mm::munmap(&call.process, a(0), a(1)),
        nr::MREMAP => super::mm::mremap(&call.process, a(0), a(1), a(2), a(3), a(4)),
        nr::MADVISE => super::mm::madvise(&call.process, a(0), a(1), a(2)),
        nr::BRK => Ok(super::mm::brk(&call.process, &call.lx.brk, a(0))),
        nr::RT_SIGACTION => sigcalls::rt_sigaction(call, a(0), a(1), a(2), a(3)),
        nr::RT_SIGPROCMASK => sigcalls::rt_sigprocmask(call, a(0), a(1), a(2), a(3)),
        nr::RT_SIGRETURN => sigcalls::rt_sigreturn(call),
        nr::RT_SIGPENDING => sigcalls::rt_sigpending(call, a(0), a(1)),
        nr::RT_SIGTIMEDWAIT => sigcalls::rt_sigtimedwait(call, a(0), a(1), a(2), a(3)),
        nr::RT_SIGSUSPEND => sigcalls::rt_sigsuspend(call, a(0), a(1)),
        nr::KILL => sigcalls::kill_call(call, a(0), a(1)),
        nr::SETITIMER => super::timer::setitimer(call, a(0), a(1), a(2)),
        nr::GETITIMER => super::timer::getitimer(call, a(0), a(1)),
        nr::ALARM => super::timer::alarm(call, a(0)),
        nr::FORK => super::fork::fork(call, None, false),
        nr::VFORK => super::fork::fork(call, None, true),
        nr::CLONE => super::fork::clone(call, a(0), a(1), a(2), a(3), a(4)),
        nr::CLONE3 => super::threads::clone3(call, a(0), a(1)),
        nr::SET_ROBUST_LIST => super::threads::set_robust_list(call, a(0), a(1)),
        nr::GET_ROBUST_LIST => super::threads::get_robust_list(call, a(0), a(1), a(2)),
        nr::PRLIMIT64 => super::resource::prlimit64(call, a(0), a(1), a(2), a(3)),
        nr::GETRLIMIT => super::resource::getrlimit(call, a(0), a(1)),
        nr::SETRLIMIT => super::resource::setrlimit(call, a(0), a(1)),
        nr::GETRANDOM => super::resource::getrandom(call, a(0), a(1), a(2)),
        nr::WAIT4 => super::fork::wait4(call, a(0), a(1), a(2), a(3)),
        nr::IOCTL => match call.lx.fd(a(0)) {
            None => Err(EBADF),
            Some(f) if matches!(&f.object, Object::Pty(_)) => {
                let Object::Pty(end) = &f.object else { return Err(ENOTTY) };
                super::pty::ioctl(call, &f, end, a(1) as u32, a(2))
            }
            Some(f) if matches!(&f.object, Object::Nt(n) if n.is_terminal()) => {
                let Object::Nt(n) = &f.object else { return Err(ENOTTY) };
                n.ioctl(
                    a(1) as u32,
                    |out| call.process.read(a(2), out).map_err(|_| EFAULT),
                    |data| call.process.write(a(2), data).map_err(|_| EFAULT),
                )
            }
            Some(_) => {
                ledger::record("ioctl", "linux syscall stub", a(1));
                Err(ENOTTY)
            }
        },
        nr::WRITEV => writev(call, a(0), a(1), a(2)),
        nr::GETPID => Ok(call.process.id),
        nr::SETSID => super::session::setsid(call),
        nr::GETSID => super::session::getsid(call, a(0)),
        nr::SETPGID => super::session::setpgid(call, a(0), a(1)),
        nr::GETPGID => super::session::getpgid(call, a(0)),
        nr::GETPGRP => super::session::getpgid(call, 0),
        nr::GETTID => Ok(super::threads::gettid(call)),
        // This thread ends; the process with its last thread.
        nr::EXIT => super::threads::exit(call, a(0)),
        nr::EXIT_GROUP => {
            process::terminate(&call.process, NtStatus((a(0) & 0xff) as u32), true);
            Ok(0)
        }
        nr::FCNTL => fcntl(call, a(0), a(1), a(2)),
        nr::GETUID | nr::GETEUID => Ok(u64::from(super::identity::of_process(&call.process).uid)),
        nr::GETRESUID => super::prctl::getres(call, [a(0), a(1), a(2)], false),
        nr::GETRESGID => super::prctl::getres(call, [a(0), a(1), a(2)], true),
        nr::PRCTL => super::prctl::prctl(call, a(0), a(1)),
        nr::STATFS => file::statfs(call, a(0), a(1)),
        nr::FSTATFS => file::fstatfs(call, a(0), a(1)),
        nr::GETGID | nr::GETEGID => Ok(u64::from(super::identity::of_process(&call.process).gid)),
        nr::GETGROUPS => super::prctl::getgroups(call, a(0), a(1)),
        nr::ARCH_PRCTL => arch_prctl(call, a(0), a(1)),
        nr::SET_TID_ADDRESS => super::threads::set_tid_address(call, a(0)),
        nr::FUTEX => super::futex::futex(call, a(0), a(1), a(2), a(3), a(4), a(5)),
        nr::TGKILL => sigcalls::tgkill(call, Some(a(0)), a(1), a(2)),
        nr::TKILL => sigcalls::tgkill(call, None, a(0), a(1)),
        nr::PAUSE => sigcalls::pause(call),
        nr::SCHED_YIELD => {
            platform::yield_now();
            Ok(0)
        }
        nr::EPOLL_CREATE => super::epoll::epoll_create(call, a(0)),
        nr::EPOLL_CREATE1 => super::epoll::epoll_create1(call, a(0)),
        nr::EPOLL_CTL => super::epoll::epoll_ctl(call, a(0), a(1), a(2), a(3)),
        nr::EPOLL_WAIT => super::epoll::epoll_wait(call, a(0), a(1), a(2), a(3)),
        nr::EPOLL_PWAIT => super::epoll::epoll_pwait(call, a(0), a(1), a(2), a(3), a(4), a(5)),
        nr::EPOLL_PWAIT2 => super::epoll::epoll_pwait2(call, a(0), a(1), a(2), a(3), a(4), a(5)),
        nr::PIPE2 => pipe2(call, a(0), a(1)),
        nr::EVENTFD => super::eventfd::eventfd(call, a(0)),
        nr::EVENTFD2 => super::eventfd::eventfd2(call, a(0), a(1)),
        nr::PIPE => pipe2(call, a(0), 0),
        nr::DUP => {
            let f = call.lx.fd(a(0)).ok_or(EBADF)?;
            call.lx.install(f)
        }
        nr::DUP2 => dup_to(call, a(0), a(1), None),
        nr::DUP3 => dup_to(call, a(0), a(1), Some(a(2))),
        nr::LSEEK => lseek(call, a(0), a(1), a(2)),
        nr::READV => readv(call, a(0), a(1), a(2)),
        nr::PREAD64 => file::pread(call, a(0), a(1), a(2), a(3), false),
        nr::PWRITE64 => file::pread(call, a(0), a(1), a(2), a(3), true),
        nr::OPEN => file::openat(call, file::AT_FDCWD, a(0), a(1), a(2)),
        nr::CREAT => file::openat(call, file::AT_FDCWD, a(0), CREAT_FLAGS, a(1)),
        nr::OPENAT => file::openat(call, a(0), a(1), a(2), a(3)),
        nr::STAT => file::stat(call, a(0), a(1), true),
        nr::LSTAT => file::stat(call, a(0), a(1), false),
        nr::STATX => file::statx(call, a(0), a(1), a(2), a(3), a(4)),
        nr::FSTAT => file::fstat(call, a(0), a(1)),
        nr::NEWFSTATAT => file::newfstatat(call, a(0), a(1), a(2), a(3)),
        nr::GETDENTS64 => file::getdents64(call, a(0), a(1), a(2)),
        nr::MKDIR => file::mkdirat(call, file::AT_FDCWD, a(0), a(1)),
        nr::MKDIRAT => file::mkdirat(call, a(0), a(1), a(2)),
        nr::RMDIR => file::unlinkat(call, file::AT_FDCWD, a(0), AT_REMOVEDIR),
        nr::UNLINK => file::unlinkat(call, file::AT_FDCWD, a(0), 0),
        nr::UNLINKAT => file::unlinkat(call, a(0), a(1), a(2)),
        nr::RENAME => file::renameat(call, file::AT_FDCWD, a(0), file::AT_FDCWD, a(1), 0),
        nr::RENAMEAT => file::renameat(call, a(0), a(1), a(2), a(3), 0),
        nr::RENAMEAT2 => file::renameat(call, a(0), a(1), a(2), a(3), a(4)),
        nr::ACCESS => file::faccessat(call, file::AT_FDCWD, a(0), a(1), 0),
        nr::FACCESSAT => file::faccessat(call, a(0), a(1), a(2), 0),
        nr::FACCESSAT2 => file::faccessat(call, a(0), a(1), a(2), a(3)),
        nr::CHDIR => file::chdir(call, a(0)),
        nr::FCHDIR => file::fchdir(call, a(0)),
        nr::GETCWD => file::getcwd(call, a(0), a(1)),
        nr::READLINK => file::readlinkat(call, file::AT_FDCWD, a(0), a(1), a(2)),
        nr::READLINKAT => file::readlinkat(call, a(0), a(1), a(2), a(3)),
        nr::LINK => file::linkat(call, file::AT_FDCWD, a(0), file::AT_FDCWD, a(1), 0),
        nr::LINKAT => file::linkat(call, a(0), a(1), a(2), a(3), a(4)),
        nr::SYMLINK => file::symlinkat(call, a(0), file::AT_FDCWD, a(1)),
        nr::SYMLINKAT => file::symlinkat(call, a(0), a(1), a(2)),
        nr::FTRUNCATE => file::ftruncate(call, a(0), a(1)),
        nr::FALLOCATE => file::fallocate(call, a(0), a(1), a(2), a(3)),
        nr::MEMFD_CREATE => super::memfd::memfd_create(call, a(0), a(1)),
        nr::TRUNCATE => file::truncate(call, a(0), a(1)),
        nr::FSYNC | nr::FDATASYNC => file::fsync(call, a(0)),
        nr::CHMOD => file::chmod(call, file::ChmodTarget::Path(file::AT_FDCWD, a(0)), a(1)),
        nr::FCHMOD => file::chmod(call, file::ChmodTarget::Fd(a(0)), a(1)),
        nr::FCHMODAT => file::chmod(call, file::ChmodTarget::Path(a(0), a(1)), a(2)),
        nr::CHOWN | nr::LCHOWN | nr::FCHOWN => file::chown(call, a(1), a(2)),
        nr::FCHOWNAT => file::chown(call, a(2), a(3)),
        nr::UMASK => file::umask(call, a(0)),
        nr::UTIMENSAT => file::utimensat(call, a(0), a(1), a(2), a(3), false),
        nr::UTIMES => file::utimensat(call, file::AT_FDCWD, a(0), a(1), 0, true),
        nr::FUTIMESAT => file::utimensat(call, a(0), a(1), a(2), 0, true),
        nr::EXECVE => super::exec::execve(call, a(0), a(1), a(2)),
        nr::GETPPID => Ok(call.lx.parent),
        nr::UNAME => uname(call, a(0)),
        // The one user: setting the ids it has is allowed, anything else
        // is EPERM (an unprivileged process).
        nr::SETUID => same_id(a(0), super::identity::of_process(&call.process).uid),
        nr::SETGID => same_id(a(0), super::identity::of_process(&call.process).gid),
        // setresuid/setresgid(r, e, s): each -1 (unchanged) or the
        // process's own id (it has no other: no saved set-user-id yet).
        nr::SETRESUID => {
            let own = super::identity::of_process(&call.process).uid;
            same_ids([a(0), a(1), a(2)], own)
        }
        nr::SETRESGID => {
            let own = super::identity::of_process(&call.process).gid;
            same_ids([a(0), a(1), a(2)], own)
        }
        nr::CLOSE_RANGE => close_range(call, a(0), a(1), a(2)),
        nr::SENDFILE => sendfile(call, a(0), a(1), a(2), a(3)),
        nr::POLL => super::poll::poll(call, a(0), a(1), a(2)),
        nr::PPOLL => super::poll::ppoll(call, a(0), a(1), a(2), a(3), a(4)),
        nr::SELECT => super::poll::select(call, a(0), a(1), a(2), a(3), a(4)),
        nr::PSELECT6 => super::poll::pselect6(call, a(0), a(1), a(2), a(3), a(4), a(5)),
        nr::SOCKET => socket::socket(call, a(0), a(1), a(2)),
        nr::SOCKETPAIR => unix::socketpair(call, a(0), a(1), a(2), a(3)),
        nr::SENDMSG => unix::sendmsg(call, a(0), a(1), a(2)),
        nr::RECVMSG => unix::recvmsg(call, a(0), a(1), a(2)),
        nr::BIND => match unix::of(call, a(0)) {
            Some((_, u)) => unix::bind(call, a(0), &u, a(1), a(2)),
            None => socket::bind(call, a(0), a(1), a(2)),
        },
        nr::LISTEN => match unix::of(call, a(0)) {
            Some((_, u)) => unix::listen(call, a(0), &u, a(1)),
            None => socket::listen(call, a(0), a(1)),
        },
        nr::ACCEPT | nr::ACCEPT4 => {
            let flags = if n == nr::ACCEPT { 0 } else { a(3) };
            match unix::of(call, a(0)) {
                Some((f, u)) => unix::accept4(call, a(0), &f, &u, a(1), a(2), flags),
                None => socket::accept4(call, a(0), a(1), a(2), flags),
            }
        }
        nr::CONNECT => match unix::of(call, a(0)) {
            Some((f, u)) => unix::connect(call, a(0), &f, &u, a(1), a(2)),
            None => socket::connect(call, a(0), a(1), a(2)),
        },
        nr::SENDTO => match unix::of(call, a(0)) {
            Some((f, u)) => unix::send(call, &f, &u, a(1), a(2), a(3), a(4)),
            None => socket::sendto(call, a(0), a(1), a(2), a(3)),
        },
        nr::RECVFROM => match unix::of(call, a(0)) {
            Some(_) => unix::recvfrom(call, a(0), a(1), a(2), a(3), a(4), a(5)),
            None => socket::recvfrom(call, a(0), a(1), a(2), a(3), a(4), a(5)),
        },
        nr::SHUTDOWN => match unix::of(call, a(0)) {
            Some((_, u)) => unix::shutdown(&u, a(1)),
            None => socket::shutdown(call, a(0), a(1)),
        },
        nr::GETSOCKNAME => match unix::of(call, a(0)) {
            Some((_, u)) => unix::getsockname(call, &u, a(1), a(2)),
            None => socket::getsockname(call, a(0), a(1), a(2)),
        },
        nr::GETPEERNAME => match unix::of(call, a(0)) {
            Some((_, u)) => unix::getpeername(call, &u, a(1), a(2)),
            None => socket::getpeername(call, a(0), a(1), a(2)),
        },
        nr::SETSOCKOPT => match unix::of(call, a(0)) {
            Some((_, u)) => unix::setsockopt(call, &u, a(1), a(2), a(3), a(4)),
            None => socket::setsockopt(call, a(0), a(1), a(2), a(3), a(4)),
        },
        nr::GETSOCKOPT => match unix::of(call, a(0)) {
            Some((_, u)) => unix::getsockopt(call, &u, a(1), a(2), a(3), a(4)),
            None => socket::getsockopt(call, a(0), a(1), a(2), a(3), a(4)),
        },
        nr::CLOCK_GETTIME => time::clock_gettime(call, a(0), a(1)),
        nr::CLOCK_GETRES => time::clock_getres(call, a(0), a(1)),
        nr::GETTIMEOFDAY => time::gettimeofday(call, a(0), a(1)),
        nr::TIME => time::time(call, a(0)),
        nr::NANOSLEEP => time::nanosleep(call, a(0), a(1)),
        nr::CLOCK_NANOSLEEP => time::clock_nanosleep(call, a(0), a(1), a(2), a(3)),
        _ => missing(call, number),
    }
}

/// A call not implemented: ENOSYS, logged and counted.
fn missing(call: &Call<'_>, number: u64) -> Result {
    let name = syscalls::name(number).unwrap_or("?");
    ledger::record(name, "linux syscall missing", number);
    if !call.lx.first_miss(number) {
        return Err(ENOSYS);
    }
    crate::warn!(
        "{}",
        Missing {
            program: call.process.name,
            pid: call.process.id,
            number,
            personality: call.process.personality
        }
    );
    Err(ENOSYS)
}

/// The log line of an unimplemented system call: the program, the call
/// by its Linux name and number, the dispatcher and the personality.
struct Missing {
    program: &'static str,
    pid: u64,
    number: u64,
    personality: Personality,
}

impl core::fmt::Display for Missing {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let name = syscalls::name(self.number).unwrap_or("?");
        write!(
            f,
            "{} (pid {}): unimplemented system call {name} ({}) (dispatcher {DISPATCHER}, \
             personality {}): ENOSYS",
            self.program, self.pid, self.number, self.personality
        )
    }
}

/// The canonical user range a base register may hold (Linux refuses the
/// last page too: TASK_SIZE).
fn valid_base(v: u64) -> bool {
    v < super::mm::TASK_SIZE
}

fn arch_prctl(call: &Call<'_>, code: u64, addr: u64) -> Result {
    match code {
        ARCH_SET_FS | ARCH_SET_GS => {
            if !valid_base(addr) {
                return Err(EPERM);
            }
            let (fs, gs) =
                if code == ARCH_SET_FS { (Some(addr), None) } else { (None, Some(addr)) };
            if platform::set_user_bases(fs, gs) { Ok(0) } else { Err(ENOSYS) }
        }
        ARCH_GET_FS | ARCH_GET_GS => {
            let (fs, gs) = platform::user_bases();
            let v = if code == ARCH_GET_FS { fs } else { gs };
            call.process.write(addr, &v.to_le_bytes()).map_err(|_| EFAULT)?;
            Ok(0)
        }
        _ => {
            ledger::record("arch_prctl", "linux option missing", code);
            Err(EINVAL)
        }
    }
}

fn fcntl(call: &Call<'_>, fd: u64, cmd: u64, arg: u64) -> Result {
    let f = call.lx.fd(fd).ok_or(EBADF)?;
    let pipe = match &f.object {
        Object::PipeRead(p) | Object::PipeWrite(p) => Some(p),
        Object::Console(_)
        | Object::File(_)
        | Object::Device(_)
        | Object::Socket(_)
        | Object::Unix(_)
        | Object::Epoll(_)
        | Object::EventFd(_)
        | Object::Pty(_)
        | Object::Nt(_) => None,
    };
    match cmd {
        F_DUPFD | F_DUPFD_CLOEXEC => {
            // The argument is an int: negative is EINVAL.
            if (arg as i32) < 0 {
                return Err(EINVAL);
            }
            call.lx.install_from(f, cmd == F_DUPFD_CLOEXEC, u64::from(arg as u32))
        }
        F_GETFD => Ok(u64::from(call.lx.cloexec(fd).ok_or(EBADF)?)),
        F_SETFD => {
            call.lx.set_cloexec(fd, arg & FD_CLOEXEC != 0);
            Ok(0)
        }
        F_GETFL => {
            let access = match &f.object {
                Object::PipeRead(_) | Object::Console(Stream::Stdin) => 0,
                Object::PipeWrite(_) => 1,
                Object::Console(_) => 2,
                Object::File(file) => file::status_flags(file),
                Object::Socket(_)
                | Object::Unix(_)
                | Object::Epoll(_)
                | Object::EventFd(_)
                | Object::Pty(_) => file::O_RDWR,
                Object::Device(d) => match (d.readable, d.writable) {
                    (true, true) => file::O_RDWR,
                    (false, true) => file::O_WRONLY,
                    _ => 0,
                },
                Object::Nt(n) => match (n.readable(), n.writable()) {
                    (true, true) => file::O_RDWR,
                    (false, true) => file::O_WRONLY,
                    _ => 0,
                },
            };
            Ok(access | if f.nonblock() { O_NONBLOCK } else { 0 })
        }
        F_SETFL => {
            // The access mode is ignored; O_NONBLOCK (and a file's
            // O_APPEND) are the status flags kept (for every dup of the
            // description).
            f.nonblock.store(arg & O_NONBLOCK != 0, core::sync::atomic::Ordering::Relaxed);
            if let Object::File(file) = &f.object {
                file.append.store(arg & file::O_APPEND != 0, core::sync::atomic::Ordering::Relaxed);
            }
            Ok(0)
        }
        F_GETPIPE_SZ => Ok(pipe.ok_or(EBADF)?.state.lock().capacity as u64),
        F_ADD_SEALS => super::memfd::add_seals(call, &f, fd, arg),
        F_GET_SEALS => super::memfd::get_seals(&f),
        F_SETPIPE_SZ => {
            let pipe = pipe.ok_or(EBADF)?;
            // Whole pages, a power of two of them, at least one.
            let pages = (arg as u32 as usize).div_ceil(PIPE_PAGE).max(1).next_power_of_two();
            let size = pages * PIPE_PAGE;
            if size > PIPE_MAX {
                return Err(EPERM);
            }
            let mut st = pipe.state.lock();
            if st.head + st.data.len() > size {
                return Err(EBUSY);
            }
            st.capacity = size;
            st.changed_for_writers();
            Ok(size as u64)
        }
        _ => {
            ledger::record("fcntl", "linux option missing", cmd);
            Err(EINVAL)
        }
    }
}

fn same_id(id: u64, own: u32) -> Result {
    if id as u32 == own { Ok(0) } else { Err(EPERM) }
}

fn same_ids(ids: [u64; 3], own: u32) -> Result {
    if ids.iter().all(|&id| id as u32 == u32::MAX || id as u32 == own) { Ok(0) } else { Err(EPERM) }
}

/// The `struct utsname` fields (65 bytes each): the system as Linux
/// programs see it. The release is the kernel the corpus was recorded on
/// (user/lx), which is what gala's Linux layer follows.
const UTSNAME: [&[u8]; 6] =
    [b"Linux", b"gala", b"6.18.0-gala", b"#1 SMP gala", b"x86_64", b"(none)"];

fn uname(call: &Call<'_>, buf: u64) -> Result {
    let mut out = [0u8; 6 * 65];
    for (i, field) in UTSNAME.iter().enumerate() {
        out[i * 65..i * 65 + field.len()].copy_from_slice(field);
    }
    call.process.write(buf, &out).map_err(|_| EFAULT)?;
    Ok(0)
}

/// close_range: fds `first..=last` close (or, with CLOSE_RANGE_CLOEXEC,
/// get close-on-exec); CLOSE_RANGE_UNSHARE changes nothing (each process
/// has its own table).
fn close_range(call: &Call<'_>, first: u64, last: u64, flags: u64) -> Result {
    const UNSHARE: u64 = 2;
    const CLOEXEC: u64 = 4;
    let (first, last) = (u64::from(first as u32), u64::from(last as u32));
    if flags & !(UNSHARE | CLOEXEC) != 0 || first > last {
        return Err(EINVAL);
    }
    let end = last.min(state::MAX_FDS as u64 - 1);
    for fd in first..=end {
        if flags & CLOEXEC != 0 {
            call.lx.set_cloexec(fd, true);
        } else {
            call.lx.close(fd);
        }
    }
    Ok(0)
}

/// dup2 (`flags` None) and dup3 (O_CLOEXEC or nothing).
fn dup_to(call: &Call<'_>, old: u64, new: u64, flags: Option<u64>) -> Result {
    if let Some(flags) = flags
        && (flags & !O_CLOEXEC != 0 || old == new)
    {
        return Err(EINVAL);
    }
    let f = call.lx.fd(old).ok_or(EBADF)?;
    if old == new {
        return Ok(new);
    }
    // The argument is an int: negative is EBADF.
    if (new as i32) < 0 {
        return Err(EBADF);
    }
    call.lx.install_at(u64::from(new as u32), f, flags.unwrap_or(0) & O_CLOEXEC != 0)?;
    Ok(new)
}

/// lseek: a pipe or the console cannot seek.
fn lseek(call: &Call<'_>, fd: u64, offset: u64, whence: u64) -> Result {
    match &call.lx.fd(fd).ok_or(EBADF)?.object {
        Object::PipeRead(_)
        | Object::PipeWrite(_)
        | Object::Console(_)
        | Object::EventFd(_)
        | Object::Socket(_)
        | Object::Unix(_)
        | Object::Epoll(_)
        | Object::Pty(_)
        | Object::Nt(_) => Err(ESPIPE),
        Object::File(file) => file::lseek(file, offset, whence),
        // A device's position means nothing: always 0.
        Object::Device(_) => Ok(0),
    }
}

fn pipe2(call: &Call<'_>, fds: u64, flags: u64) -> Result {
    if flags & !(O_NONBLOCK | O_CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    let nonblock = flags & O_NONBLOCK != 0;
    let cloexec = flags & O_CLOEXEC != 0;
    let pipe = state::new_pipe().map_err(|_| ENOMEM)?;
    let r = KArc::try_new(Fd::new(Object::PipeRead(pipe.clone()), nonblock)).map_err(|_| ENOMEM)?;
    let w = KArc::try_new(Fd::new(Object::PipeWrite(pipe), nonblock)).map_err(|_| ENOMEM)?;
    let rfd = call.lx.install_from(r, cloexec, 0)?;
    let wfd = match call.lx.install_from(w, cloexec, 0) {
        Ok(w) => w,
        Err(e) => {
            call.lx.close(rfd);
            return Err(e);
        }
    };
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&(rfd as u32).to_le_bytes());
    out[4..].copy_from_slice(&(wfd as u32).to_le_bytes());
    if call.process.write(fds, &out).is_err() {
        call.lx.close(rfd);
        call.lx.close(wfd);
        return Err(EFAULT);
    }
    Ok(0)
}

/// A wait on something with no wait queue (sockets: the stack's notify
/// hook is AFD's; a connect under way): a millisecond's sleep, cut short
/// by a signal (EINTR once one is to be delivered or the process is
/// ending; restarted after it if its handler has SA_RESTART and the call
/// may be: `sigcalls::deliver`) or an expired timer (its SIGALRM).
pub fn pause(call: &Call<'_>) -> core::result::Result<(), u16> {
    super::timer::fire(call);
    let at = platform::monotonic_ns().saturating_add(super::wait::POLL_NS);
    super::wait::sleep_until(call, Some(at))
}

fn read(call: &Call<'_>, fd: u64, buf: u64, count: u64) -> Result {
    let f = call.lx.fd(fd).ok_or(EBADF)?;
    let count = count.min(MAX_RW);
    match &f.object {
        // No console input yet: end of file.
        Object::Console(Stream::Stdin) => Ok(0),
        Object::Console(_) | Object::PipeWrite(_) | Object::Epoll(_) => Err(EBADF),
        Object::File(file) => file::read(call, file, buf, count),
        Object::Device(dev) => file::device_read(call, dev, buf, count),
        Object::Socket(sock) => socket::recv(call, &f, sock, buf, count, 0),
        Object::Pty(end) => super::pty::read(call, &f, end, buf, count),
        Object::Unix(u) => unix::recv(call, fd, &f, u, buf, count, 0),
        Object::EventFd(e) => super::eventfd::read(call, &f, e, buf, count),
        Object::Nt(n) => {
            let mut data = Vec::new();
            let want = (count as usize).min(super::ntio::CHUNK);
            data.try_reserve_exact(want).map_err(|_| ENOMEM)?;
            data.resize(want, 0);
            let got = n.read(&mut data)?;
            call.process.write(buf, &data[..got]).map_err(|_| EFAULT)?;
            Ok(got as u64)
        }
        Object::PipeRead(pipe) => loop {
            if count == 0 {
                return Ok(0);
            }
            // Copied out before it is taken: a read into a bad buffer
            // leaves the bytes in the pipe (EFAULT; brk_test's probes).
            let mut data = Vec::new();
            let me = platform::current();
            {
                let mut st = pipe.state.lock();
                let n = (st.data.len() as u64).min(count) as usize;
                if data.try_reserve_exact(n).is_err() {
                    return Err(ENOMEM);
                }
                data.extend(st.data.iter().take(n));
                if data.is_empty() {
                    if st.writers == 0 {
                        return Ok(0);
                    }
                    if f.nonblock() {
                        return Err(EAGAIN);
                    }
                    // Woken by a write or the last writer's close.
                    st.waiters.add(me)?;
                    super::wait::prepare(call, None);
                }
            }
            if data.is_empty() {
                let r = super::wait::block(call);
                pipe.state.lock().waiters.remove(me);
                r?;
                continue;
            }
            call.process.write(buf, &data).map_err(|_| EFAULT)?;
            pipe.state.lock().consumed(data.len());
            return Ok(data.len() as u64);
        },
    }
}

fn write(call: &Call<'_>, fd: u64, buf: u64, count: u64) -> Result {
    let f = call.lx.fd(fd).ok_or(EBADF)?;
    let count = count.min(MAX_RW);
    match &f.object {
        Object::Console(Stream::Stdin) | Object::PipeRead(_) | Object::Epoll(_) => Err(EBADF),
        Object::Pty(end) => super::pty::write(call, &f, end, buf, count),
        Object::File(file) => file::write(call, file, buf, count),
        Object::Device(dev) => file::device_write(dev, count),
        Object::Socket(sock) => socket::send(call, &f, sock, count as usize, 0, |at, out| {
            call.process.read(buf + at as u64, out).map_err(|_| EFAULT)
        }),
        Object::Unix(u) => unix::send(call, &f, u, buf, count, 0, 0),
        Object::EventFd(e) => super::eventfd::write(call, &f, e, buf, count),
        Object::Nt(n) => {
            // A piece at a time, as the console's below.
            let mut done = 0u64;
            let mut chunk = Vec::new();
            while done < count {
                let k = (count - done).min(super::ntio::CHUNK as u64) as usize;
                chunk.clear();
                if chunk.try_reserve_exact(k).is_err() {
                    return if done > 0 { Ok(done) } else { Err(ENOMEM) };
                }
                chunk.resize(k, 0);
                if call.process.read(buf + done, &mut chunk).is_err() {
                    return if done > 0 { Ok(done) } else { Err(EFAULT) };
                }
                match n.write(call.process.id, &chunk) {
                    Ok(w) => done += w as u64,
                    Err(EPIPE) if done == 0 => {
                        super::sigcalls::sigpipe(call);
                        return Err(EPIPE);
                    }
                    Err(_) if done > 0 => return Ok(done),
                    Err(e) => return Err(e),
                }
            }
            Ok(done)
        }
        Object::Console(stream) => {
            // A piece at a time; a fault after some pieces ends the write
            // with what was written, one at the start is EFAULT.
            let mut done = 0u64;
            let mut chunk = Vec::new();
            while done < count {
                let n = (count - done).min(CHUNK as u64) as usize;
                chunk.clear();
                if chunk.try_reserve_exact(n).is_err() {
                    return if done > 0 { Ok(done) } else { Err(ENOMEM) };
                }
                chunk.resize(n, 0);
                if call.process.read(buf + done, &mut chunk).is_err() {
                    return if done > 0 { Ok(done) } else { Err(EFAULT) };
                }
                call.lx.console_write(*stream, &chunk).map_err(|_| ENOMEM)?;
                done += n as u64;
            }
            Ok(done)
        }
        Object::PipeWrite(pipe) => pipe_write(call, &f, pipe, count as usize, |at, out| {
            call.process.read(buf + at as u64, out).map_err(|_| EFAULT)
        }),
    }
}

/// A write of `count` bytes to `pipe` through fd `f`, its bytes fetched
/// piece by piece (`fetch(offset, out)` fills `out` with the bytes from
/// `offset` on). Up to PIPE_BUF bytes go in whole or not at all; a larger
/// write takes what room there is and, blocking, waits for more until all
/// of it is in (a signal ends it with what was written, or EINTR). A
/// piece that cannot be fetched is not written: EFAULT if nothing was
/// (write_exit_test: Linux keeps nothing of a buffer that runs into a
/// hole within its first piece). No reader: SIGPIPE and EPIPE (or the
/// count already written).
fn pipe_write(
    call: &Call<'_>,
    f: &Fd,
    pipe: &state::Pipe,
    count: usize,
    mut fetch: impl FnMut(usize, &mut [u8]) -> core::result::Result<(), u16>,
) -> Result {
    let mut done = 0usize;
    let partial = |done: usize, e: u16| if done > 0 { Ok(done as u64) } else { Err(e) };
    let me = platform::current();
    while done < count {
        let left = count - done;
        let (room, readers, fits) = {
            let mut st = pipe.state.lock();
            let room = st.free();
            let fits = if count <= PIPE_BUF { room >= left } else { room > 0 };
            if !fits && st.readers > 0 && !f.nonblock() {
                // Woken by a read or the last reader's close.
                if let Err(e) = st.waiters.add(me) {
                    return partial(done, e);
                }
                super::wait::prepare(call, None);
            }
            (room, st.readers, fits)
        };
        if readers == 0 {
            sigcalls::sigpipe(call);
            return partial(done, EPIPE);
        }
        if !fits {
            if f.nonblock() {
                return partial(done, EAGAIN);
            }
            let r = super::wait::block(call);
            pipe.state.lock().waiters.remove(me);
            if let Err(e) = r {
                return partial(done, e);
            }
            continue;
        }
        let n = left.min(room);
        let mut data = Vec::new();
        if data.try_reserve_exact(n).is_err() {
            return partial(done, ENOMEM);
        }
        data.resize(n, 0);
        if let Err(e) = fetch(done, &mut data) {
            return partial(done, e);
        }
        let mut st = pipe.state.lock();
        if st.data.try_reserve(n).is_err() {
            return partial(done, ENOMEM);
        }
        st.data.extend(data.iter());
        st.changed_for_readers();
        done += n;
    }
    Ok(done as u64)
}

fn writev(call: &Call<'_>, fd: u64, iov: u64, count: u64) -> Result {
    if count > IOV_MAX {
        // (A negative iovcnt is a huge one here.)
        return Err(EINVAL);
    }
    call.lx.fd(fd).ok_or(EBADF)?;
    let mut pieces: Vec<(u64, u64)> = Vec::new();
    let mut total = 0u64;
    for i in 0..count {
        let base = call.process.read_u64(iov + i * 16).map_err(|_| EFAULT)?;
        let len = call.process.read_u64(iov + i * 16 + 8).map_err(|_| EFAULT)?;
        total = total.checked_add(len).filter(|&t| t <= i64::MAX as u64).ok_or(EINVAL)?;
        pieces.try_push((base, len)).map_err(|_| ENOMEM)?;
    }
    // Gathered into one buffer and written once (a pipe sees one write).
    let mut data = Vec::new();
    data.try_reserve_exact(total.min(MAX_RW) as usize).map_err(|_| ENOMEM)?;
    for (base, len) in pieces {
        let len = len.min(MAX_RW - data.len() as u64) as usize;
        let at = data.len();
        data.resize(at + len, 0);
        if call.process.read(base, &mut data[at..]).is_err() {
            data.truncate(at);
            if at == 0 {
                return Err(EFAULT);
            }
            break;
        }
    }
    if data.is_empty() {
        return Ok(0);
    }
    write_bytes(call, fd, &data)
}

/// readv: one read as long as the vectors together, its bytes scattered
/// over them (a pipe's are taken once they are all copied out).
fn readv(call: &Call<'_>, fd: u64, iov: u64, count: u64) -> Result {
    if count > IOV_MAX {
        return Err(EINVAL);
    }
    call.lx.fd(fd).ok_or(EBADF)?;
    let mut pieces: Vec<(u64, u64)> = Vec::new();
    let mut total = 0u64;
    for i in 0..count {
        let base = call.process.read_u64(iov + i * 16).map_err(|_| EFAULT)?;
        let len = call.process.read_u64(iov + i * 16 + 8).map_err(|_| EFAULT)?;
        total = total.checked_add(len).filter(|&t| t <= i64::MAX as u64).ok_or(EINVAL)?;
        if len > 0 {
            pieces.try_push((base, len)).map_err(|_| ENOMEM)?;
        }
    }
    // One vector: a plain read. Otherwise a read into the first, and on
    // into the next only while each is filled (what a reader of the
    // whole would see; a pipe gives what it holds, piece by piece).
    let mut done = 0u64;
    for (base, len) in pieces {
        let n = match read(call, fd, base, len) {
            Ok(n) => n,
            Err(e) if done == 0 => return Err(e),
            Err(_) => break,
        };
        done += n;
        if n < len {
            break;
        }
        if let Some(f) = call.lx.fd(fd)
            && matches!(f.object, Object::PipeRead(_))
            && pipe_empty(&f.object)
        {
            break;
        }
    }
    Ok(done)
}

/// Whether a pipe holds no bytes now (readv stops instead of blocking).
fn pipe_empty(object: &Object) -> bool {
    match object {
        Object::PipeRead(p) => p.state.lock().data.is_empty(),
        _ => false,
    }
}

/// sendfile: up to `count` bytes (64 KiB a call) from the file `in_fd`
/// (at `*offset`, which moves on, or at its own offset) written to
/// `out_fd` as one write.
fn sendfile(call: &Call<'_>, out_fd: u64, in_fd: u64, offset: u64, count: u64) -> Result {
    let f = call.lx.fd(in_fd).ok_or(EBADF)?;
    call.lx.fd(out_fd).ok_or(EBADF)?;
    let Object::File(file) = &f.object else { return Err(EINVAL) };
    if !file.readable || file.path_only {
        return Err(EBADF);
    }
    if file.dir {
        return Err(EINVAL);
    }
    let at = if offset != 0 {
        let at = call.process.read_u64(offset).map_err(|_| EFAULT)?;
        if (at as i64) < 0 {
            return Err(EINVAL);
        }
        at
    } else {
        *file.position.lock()
    };
    let n = count.min(MAX_RW).min(CHUNK as u64) as usize;
    let mut data = Vec::new();
    data.try_reserve_exact(n).map_err(|_| ENOMEM)?;
    data.resize(n, 0);
    let got = file::read_node(file, at, &mut data)?;
    if got == 0 {
        return Ok(0);
    }
    let written = write_bytes(call, out_fd, &data[..got])?;
    if offset != 0 {
        call.process.write(offset, &(at + written).to_le_bytes()).map_err(|_| EFAULT)?;
    } else {
        *file.position.lock() = at + written;
    }
    Ok(written)
}

/// Writes kernel bytes to `fd` (writev's gathered pieces).
fn write_bytes(call: &Call<'_>, fd: u64, data: &[u8]) -> Result {
    let f = call.lx.fd(fd).ok_or(EBADF)?;
    match &f.object {
        Object::Console(Stream::Stdin) | Object::PipeRead(_) | Object::Epoll(_) => Err(EBADF),
        Object::Pty(end) => super::pty::write_bytes(call, &f, end, data),
        Object::File(file) => file::write_bytes(file, data),
        Object::Device(dev) => file::device_write(dev, data.len() as u64),
        Object::Socket(sock) => socket::send(call, &f, sock, data.len(), 0, |at, out| {
            out.copy_from_slice(&data[at..at + out.len()]);
            Ok(())
        }),
        Object::Unix(u) => unix::send_bytes(call, &f, u, data),
        Object::EventFd(e) => super::eventfd::write_bytes(call, &f, e, data),
        Object::Nt(n) => match n.write(call.process.id, data) {
            Ok(w) => Ok(w as u64),
            Err(EPIPE) => {
                super::sigcalls::sigpipe(call);
                Err(EPIPE)
            }
            Err(e) => Err(e),
        },
        Object::Console(stream) => {
            call.lx.console_write(*stream, data).map_err(|_| ENOMEM)?;
            Ok(data.len() as u64)
        }
        Object::PipeWrite(pipe) => pipe_write(call, &f, pipe, data.len(), |at, out| {
            out.copy_from_slice(&data[at..at + out.len()]);
            Ok(())
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_call_is_named_from_the_table() {
        let line = |number| {
            let m = Missing { program: "a", pid: 4, number, personality: Personality::Linux };
            alloc::format!("{m}")
        };
        assert_eq!(
            line(u64::from(nr::SET_TID_ADDRESS) + 1),
            "a (pid 4): unimplemented system call restart_syscall (219) \
             (dispatcher lx::dispatch, personality linux): ENOSYS"
        );
        assert!(line(5000).contains("system call ? (5000)"));
    }
}
