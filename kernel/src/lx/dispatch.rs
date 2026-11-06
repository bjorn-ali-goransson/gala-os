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
//! data. Calls accepted without their effect (rt_sigprocmask: no signals
//! yet) count as `"what":"linux syscall stub"`.

use alloc::vec::Vec;

use super::errno::{
    self, EAGAIN, EBADF, EBUSY, EFAULT, EINTR, EINVAL, ENOMEM, ENOSYS, ENOTTY, EPERM, EPIPE, ESPIPE,
};
use super::state::{self, Fd, LxProcess, Object, PIPE_MAX, PIPE_PAGE, Stream};
use super::syscalls::{self, nr};
use crate::mm::heap::{KArc, TryVec};
use crate::nt::object::ObjectBody;
use crate::nt::platform::{self, Personality, SyscallFrame};
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
const F_GETFD: u64 = 1;
const F_SETFD: u64 = 2;
const F_GETFL: u64 = 3;
const F_SETFL: u64 = 4;
const F_DUPFD: u64 = 0;
const F_DUPFD_CLOEXEC: u64 = 1030;
const F_SETPIPE_SZ: u64 = 1031;
const F_GETPIPE_SZ: u64 = 1032;
const FD_CLOEXEC: u64 = 1;
/// Writes up to this size to a pipe are atomic (PIPE_BUF).
const PIPE_BUF: usize = 4096;
const ARCH_SET_GS: u64 = 0x1001;
const ARCH_SET_FS: u64 = 0x1002;
const ARCH_GET_FS: u64 = 0x1003;
const ARCH_GET_GS: u64 = 0x1004;

/// One system call's context.
struct Call<'a> {
    process: KArc<Process>,
    lx: KArc<LxProcess>,
    frame: &'a SyscallFrame,
    tid: u64,
}

impl Call<'_> {
    fn arg(&self, i: usize) -> u64 {
        let f = self.frame;
        [f.rdi, f.rsi, f.rdx, f.r10, f.r8, f.r9][i]
    }
}

type Result = core::result::Result<u64, u16>;

fn dispatch(frame: &mut SyscallFrame) {
    let number = frame.rax;
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
        frame.rax = errno::to_return(ENOSYS);
        return;
    };
    let call = Call { process, lx, frame, tid };
    let result = handle(&call, number);
    let rax = match result {
        Ok(v) => v,
        Err(e) => errno::to_return(e),
    };
    crate::flight::record(crate::flight::Kind::SyscallDone, number, rax);
    frame.rax = rax;
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
        nr::MMAP => super::mm::mmap(&call.process, a(0), a(1), a(2), a(3), a(4) as i64, a(5)),
        nr::MPROTECT => super::mm::mprotect(&call.process, a(0), a(1), a(2)),
        nr::MUNMAP => super::mm::munmap(&call.process, a(0), a(1)),
        nr::BRK => Ok(super::mm::brk(&call.process, &call.lx.brk, a(0))),
        nr::RT_SIGPROCMASK => {
            // No signals are delivered yet: every mask is accepted (the
            // old one reads as empty).
            ledger::record("rt_sigprocmask", "linux syscall stub", 0);
            let old = a(2);
            if old != 0 {
                call.process.write(old, &0u64.to_le_bytes()).map_err(|_| EFAULT)?;
            }
            Ok(0)
        }
        nr::IOCTL => match call.lx.fd(a(0)) {
            None => Err(EBADF),
            Some(_) => {
                ledger::record("ioctl", "linux syscall stub", a(1));
                Err(ENOTTY)
            }
        },
        nr::WRITEV => writev(call, a(0), a(1), a(2)),
        nr::GETPID => Ok(call.process.id),
        nr::GETTID => Ok(call.tid),
        nr::EXIT => {
            // This thread ends; the process with its last thread.
            thread::exit_current_on_return(NtStatus((a(0) & 0xff) as u32));
            Ok(0)
        }
        nr::EXIT_GROUP => {
            process::terminate(&call.process, NtStatus((a(0) & 0xff) as u32), true);
            Ok(0)
        }
        nr::FCNTL => fcntl(call, a(0), a(1), a(2)),
        nr::GETUID | nr::GETEUID => Ok(super::exec::UID),
        nr::GETGID | nr::GETEGID => Ok(super::exec::GID),
        nr::ARCH_PRCTL => arch_prctl(call, a(0), a(1)),
        nr::SET_TID_ADDRESS => {
            // Kept for the thread's exit (CLONE_CHILD_CLEARTID) once
            // threads are created: one thread so far.
            Ok(call.tid)
        }
        nr::PIPE2 => pipe2(call, a(0), a(1)),
        nr::PIPE => pipe2(call, a(0), 0),
        nr::DUP => {
            let f = call.lx.fd(a(0)).ok_or(EBADF)?;
            call.lx.install(f)
        }
        nr::DUP2 => dup_to(call, a(0), a(1), None),
        nr::DUP3 => dup_to(call, a(0), a(1), Some(a(2))),
        nr::LSEEK => lseek(call, a(0)),
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
        Object::Console(_) => None,
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
            let access = match f.object {
                Object::PipeRead(_) | Object::Console(Stream::Stdin) => 0,
                Object::PipeWrite(_) => 1,
                Object::Console(_) => 2,
            };
            Ok(access | if f.nonblock() { O_NONBLOCK } else { 0 })
        }
        F_SETFL => {
            // The access mode is ignored; O_NONBLOCK is the status flag
            // kept (for every dup of the description).
            f.nonblock.store(arg & O_NONBLOCK != 0, core::sync::atomic::Ordering::Relaxed);
            Ok(0)
        }
        F_GETPIPE_SZ => Ok(pipe.ok_or(EBADF)?.state.lock().capacity as u64),
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
            Ok(size as u64)
        }
        _ => {
            ledger::record("fcntl", "linux option missing", cmd);
            Err(EINVAL)
        }
    }
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
fn lseek(call: &Call<'_>, fd: u64) -> Result {
    match call.lx.fd(fd).ok_or(EBADF)?.object {
        Object::PipeRead(_) | Object::PipeWrite(_) | Object::Console(_) => Err(ESPIPE),
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

/// Waits a little for a pipe to change (no wait queue yet: a blocking
/// pipe operation polls); EINTR once the process is ending.
fn pause(call: &Call<'_>) -> core::result::Result<(), u16> {
    if call.process.exit_status().is_some() {
        return Err(EINTR);
    }
    platform::sleep_ms(1);
    Ok(())
}

fn read(call: &Call<'_>, fd: u64, buf: u64, count: u64) -> Result {
    let f = call.lx.fd(fd).ok_or(EBADF)?;
    let count = count.min(MAX_RW);
    match &f.object {
        // No console input yet: end of file.
        Object::Console(Stream::Stdin) => Ok(0),
        Object::Console(_) | Object::PipeWrite(_) => Err(EBADF),
        Object::PipeRead(pipe) => loop {
            if count == 0 {
                return Ok(0);
            }
            // Copied out before it is taken: a read into a bad buffer
            // leaves the bytes in the pipe (EFAULT; brk_test's probes).
            let mut data = Vec::new();
            let writers = {
                let st = pipe.state.lock();
                let n = (st.data.len() as u64).min(count) as usize;
                if data.try_reserve_exact(n).is_err() {
                    return Err(ENOMEM);
                }
                data.extend(st.data.iter().take(n));
                st.writers
            };
            if data.is_empty() {
                if writers == 0 {
                    return Ok(0);
                }
                if f.nonblock() {
                    return Err(EAGAIN);
                }
                pause(call)?;
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
        Object::Console(Stream::Stdin) | Object::PipeRead(_) => Err(EBADF),
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
        Object::PipeWrite(pipe) => loop {
            if count == 0 {
                return Ok(0);
            }
            let (room, readers) = {
                let st = pipe.state.lock();
                (st.free(), st.readers)
            };
            if readers == 0 {
                // SIGPIPE is not delivered yet: only the error.
                ledger::record("write", "linux SIGPIPE not delivered", 0);
                return Err(EPIPE);
            }
            let count = count as usize;
            // Up to PIPE_BUF bytes go in whole or not at all; more are
            // written as room allows (all of it in blocking mode, which
            // waits for the rest).
            let fits = if count <= PIPE_BUF { room >= count } else { room > 0 };
            if !fits {
                if f.nonblock() {
                    return Err(EAGAIN);
                }
                pause(call)?;
                continue;
            }
            let n = count.min(room);
            if !f.nonblock() && n < count {
                pause(call)?;
                continue;
            }
            // Copied in whole first: a fault anywhere is EFAULT with
            // nothing written (write_exit_test: Linux does that too).
            let mut data = Vec::new();
            if data.try_reserve_exact(n).is_err() {
                return Err(ENOMEM);
            }
            data.resize(n, 0);
            call.process.read(buf, &mut data).map_err(|_| EFAULT)?;
            let mut st = pipe.state.lock();
            if st.data.try_reserve(n).is_err() {
                return Err(ENOMEM);
            }
            st.data.extend(data.iter());
            return Ok(n as u64);
        },
    }
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

/// Writes kernel bytes to `fd` (writev's gathered pieces).
fn write_bytes(call: &Call<'_>, fd: u64, data: &[u8]) -> Result {
    let f = call.lx.fd(fd).ok_or(EBADF)?;
    match &f.object {
        Object::Console(Stream::Stdin) | Object::PipeRead(_) => Err(EBADF),
        Object::Console(stream) => {
            call.lx.console_write(*stream, data).map_err(|_| ENOMEM)?;
            Ok(data.len() as u64)
        }
        Object::PipeWrite(pipe) => {
            let mut st = pipe.state.lock();
            if st.readers == 0 {
                return Err(EPIPE);
            }
            let n = data.len().min(st.free());
            if n == 0 {
                return Err(EAGAIN);
            }
            st.data.try_reserve(n).map_err(|_| ENOMEM)?;
            st.data.extend(data[..n].iter());
            Ok(n as u64)
        }
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
