//! seccomp(2) for the Linux personality: strict mode, filters (classic
//! BPF programs run on each system call's `seccomp_data`) and the probes
//! (SECCOMP_GET_ACTION_AVAIL, SECCOMP_GET_NOTIF_SIZES), with prctl's
//! PR_GET_SECCOMP and PR_SET_SECCOMP. user/lx/seccomp_test, recorded on
//! Linux 6.18, is the spec. Sandboxes install filters (bubblewrap,
//! glycin's loaders, browsers): accepting a filter means enforcing it.
//!
//! A thread's state (`Seccomp`, in `LxThread`) is strict mode or a chain
//! of filters, newest first; a thread it creates and a fork's child get
//! its creator's, execve keeps it. `dispatch` asks `check` before every
//! call of a thread that has one (`LxProcess::seccomp_used` keeps the rest
//! at one load): strict mode allows read, write, exit and rt_sigreturn and
//! kills with SIGKILL otherwise; filters each run (newest first) and the
//! lowest action wins (the newest of equals): ALLOW and LOG run the call,
//! ERRNO answers its data as the errno, TRAP sends SIGSYS (SYS_SECCOMP,
//! the data as si_errno, the call's address, number and architecture;
//! forced to its default, a kill, if blocked or ignored) and skips the
//! call, TRACE and USER_NOTIF (no tracer, no listener) answer ENOSYS,
//! KILL_THREAD and KILL_PROCESS end it with SIGSYS.
//!
//! A filter is checked as Linux's seccomp_check_filter and
//! bpf_check_classic check it before it is taken (EINVAL): 1 to 4096
//! instructions of the subset seccomp allows, loads of whole words inside
//! `seccomp_data`, jumps inside the program, no division by a constant 0
//! or shift by 32 or more, a return at its end. Installing one needs
//! no_new_privs or uid 0 (EACCES).
//!
//! Limits: no listener (SECCOMP_FILTER_FLAG_NEW_LISTENER: EINVAL,
//! ledgered); SECCOMP_FILTER_FLAG_LOG and LOG actions log nothing more;
//! scratch memory read before it is written is not refused (it reads 0).

use alloc::vec::Vec;

use super::dispatch::{Call, Result};
use super::errno::{EACCES, EFAULT, EINVAL, ENOMEM, ENOSYS};
use crate::mm::heap::{KArc, TryVec};
use crate::nt::ledger;

const SET_MODE_STRICT: u64 = 0;
const SET_MODE_FILTER: u64 = 1;
const GET_ACTION_AVAIL: u64 = 2;
const GET_NOTIF_SIZES: u64 = 3;

const RET_KILL_PROCESS: u32 = 0x8000_0000;
const RET_KILL_THREAD: u32 = 0x0000_0000;
const RET_TRAP: u32 = 0x0003_0000;
const RET_ERRNO: u32 = 0x0005_0000;
const RET_USER_NOTIF: u32 = 0x7fc0_0000;
const RET_TRACE: u32 = 0x7ff0_0000;
const RET_LOG: u32 = 0x7ffc_0000;
const RET_ALLOW: u32 = 0x7fff_0000;
const RET_ACTION_FULL: u32 = 0xffff_0000;
const RET_DATA: u32 = 0x0000_ffff;
const ACTIONS: [u32; 8] = [
    RET_KILL_PROCESS,
    RET_KILL_THREAD,
    RET_TRAP,
    RET_ERRNO,
    RET_USER_NOTIF,
    RET_TRACE,
    RET_LOG,
    RET_ALLOW,
];

const FLAG_TSYNC: u64 = 1;
const FLAG_LOG: u64 = 2;
const FLAG_SPEC_ALLOW: u64 = 4;
const FLAG_NEW_LISTENER: u64 = 8;
const FLAG_TSYNC_ESRCH: u64 = 16;
const FLAG_WAIT_KILLABLE_RECV: u64 = 32;
const FLAGS: u64 = FLAG_TSYNC
    | FLAG_LOG
    | FLAG_SPEC_ALLOW
    | FLAG_NEW_LISTENER
    | FLAG_TSYNC_ESRCH
    | FLAG_WAIT_KILLABLE_RECV;

/// seccomp_data's arch: the architecture's AUDIT_ARCH (`machine`).
use super::machine::AUDIT_ARCH;
/// struct seccomp_data's size.
const DATA_SIZE: u32 = 64;
/// The most instructions in one filter (BPF_MAXINSNS) and on a chain
/// (MAX_INSNS_PER_PATH, with 4 for each filter).
const MAX_INSNS: usize = 4096;
const MAX_PATH: usize = 32768;
/// SIGSYS's si_code for a seccomp trap.
const SYS_SECCOMP: i32 = 1;

// Classic BPF (linux/filter.h): class, size, mode, operation, source.
const LD: u16 = 0x00;
const LDX: u16 = 0x01;
const ST: u16 = 0x02;
const STX: u16 = 0x03;
const ALU: u16 = 0x04;
const JMP: u16 = 0x05;
const RET: u16 = 0x06;
const MISC: u16 = 0x07;
const W: u16 = 0x00;
const IMM: u16 = 0x00;
const ABS: u16 = 0x20;
const MEM: u16 = 0x60;
const LEN: u16 = 0x80;
const K: u16 = 0x00;
const X: u16 = 0x08;
const A: u16 = 0x10;
const ADD: u16 = 0x00;
const SUB: u16 = 0x10;
const MUL: u16 = 0x20;
const DIV: u16 = 0x30;
const OR: u16 = 0x40;
const AND: u16 = 0x50;
const LSH: u16 = 0x60;
const RSH: u16 = 0x70;
const NEG: u16 = 0x80;
const MOD: u16 = 0x90;
const XOR: u16 = 0xa0;
const JA: u16 = 0x00;
const JEQ: u16 = 0x10;
const JGT: u16 = 0x20;
const JGE: u16 = 0x30;
const JSET: u16 = 0x40;
const TAX: u16 = 0x00;
const TXA: u16 = 0x80;
const MEMWORDS: u32 = 16;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Insn {
    pub code: u16,
    pub jt: u8,
    pub jf: u8,
    pub k: u32,
}

/// One installed filter and the ones before it.
pub struct Filter {
    prog: Vec<Insn>,
    prev: Option<KArc<Filter>>,
}

impl Filter {
    fn path_len(&self) -> usize {
        self.prog.len() + 4 + self.prev.as_ref().map_or(0, |p| p.path_len())
    }
}

/// A thread's seccomp state.
#[derive(Clone, Default)]
pub struct Seccomp {
    pub strict: bool,
    pub filter: Option<KArc<Filter>>,
}

impl Seccomp {
    /// PR_GET_SECCOMP's mode: 0 none, 1 strict, 2 filter.
    pub fn mode(&self) -> u64 {
        if self.strict {
            1
        } else if self.filter.is_some() {
            2
        } else {
            0
        }
    }
}

/// Checks `prog` as Linux's seccomp_check_filter and bpf_check_classic do,
/// turning the loads of the data's length into constants (as Linux does).
pub fn validate(prog: &mut [Insn]) -> core::result::Result<(), u16> {
    let n = prog.len();
    if n == 0 || n > MAX_INSNS {
        return Err(EINVAL);
    }
    for (pc, i) in prog.iter_mut().enumerate() {
        if i.code > 0xff {
            return Err(EINVAL);
        }
        let class = i.code & 0x07;
        match i.code {
            c if c == LD | W | ABS => {
                if i.k >= DATA_SIZE || !i.k.is_multiple_of(4) {
                    return Err(EINVAL);
                }
            }
            c if c == LD | W | LEN => *i = Insn { code: LD | IMM, jt: 0, jf: 0, k: DATA_SIZE },
            c if c == LDX | W | LEN => *i = Insn { code: LDX | IMM, jt: 0, jf: 0, k: DATA_SIZE },
            c if c == RET | K || c == RET | A => {}
            c if c == ALU | NEG => {}
            c if class == ALU
                && [ADD, SUB, MUL, DIV, AND, OR, XOR, LSH, RSH, MOD].contains(&(c & 0xf0)) =>
            {
                if c & 0x08 == K {
                    let op = c & 0xf0;
                    if (op == DIV || op == MOD) && i.k == 0 {
                        return Err(EINVAL);
                    }
                    if (op == LSH || op == RSH) && i.k >= 32 {
                        return Err(EINVAL);
                    }
                }
            }
            c if c == LD | IMM || c == LDX | IMM => {}
            c if c == MISC | TAX || c == MISC | TXA => {}
            c if c == LD | MEM || c == LDX | MEM || c == ST || c == STX => {
                if i.k >= MEMWORDS {
                    return Err(EINVAL);
                }
            }
            c if c == JMP | JA => {
                if (i.k as usize) >= n - pc - 1 {
                    return Err(EINVAL);
                }
            }
            c if class == JMP && [JEQ, JGT, JGE, JSET].contains(&(c & 0xf0)) => {
                if pc + 1 + usize::from(i.jt) >= n || pc + 1 + usize::from(i.jf) >= n {
                    return Err(EINVAL);
                }
            }
            _ => return Err(EINVAL),
        }
    }
    match prog[n - 1].code {
        c if c == RET | K || c == RET | A => Ok(()),
        _ => Err(EINVAL),
    }
}

/// Runs a validated program on `data` (struct seccomp_data, 64 bytes).
pub fn run(prog: &[Insn], data: &[u8; 64]) -> u32 {
    let (mut a, mut x) = (0u32, 0u32);
    let mut m = [0u32; MEMWORDS as usize];
    let mut pc = 0;
    while pc < prog.len() {
        let i = prog[pc];
        pc += 1;
        let src = if i.code & 0x08 == X { x } else { i.k };
        match i.code & 0x07 {
            c if c == LD => match i.code & 0xe0 {
                ABS => {
                    let o = i.k as usize;
                    a = u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
                }
                MEM => a = m[i.k as usize],
                _ => a = i.k,
            },
            c if c == LDX => x = if i.code & 0xe0 == MEM { m[i.k as usize] } else { i.k },
            c if c == ST => m[i.k as usize] = a,
            c if c == STX => m[i.k as usize] = x,
            c if c == ALU => {
                a = match i.code & 0xf0 {
                    ADD => a.wrapping_add(src),
                    SUB => a.wrapping_sub(src),
                    MUL => a.wrapping_mul(src),
                    // A division by X 0 ends the program with 0 (Linux's).
                    DIV => match a.checked_div(src) {
                        Some(v) => v,
                        None => return 0,
                    },
                    MOD => match a.checked_rem(src) {
                        Some(v) => v,
                        None => return 0,
                    },
                    AND => a & src,
                    OR => a | src,
                    XOR => a ^ src,
                    LSH => a.checked_shl(src).unwrap_or(0),
                    RSH => a.checked_shr(src).unwrap_or(0),
                    _ => a.wrapping_neg(),
                }
            }
            c if c == JMP => {
                let taken = match i.code & 0xf0 {
                    JA => {
                        pc += i.k as usize;
                        continue;
                    }
                    JEQ => a == src,
                    JGT => a > src,
                    JGE => a >= src,
                    _ => a & src != 0,
                };
                pc += usize::from(if taken { i.jt } else { i.jf });
            }
            c if c == RET => return if i.code & 0x18 == A { a } else { i.k },
            _ => {
                if i.code & 0xf8 == TXA {
                    a = x;
                } else {
                    x = a;
                }
            }
        }
    }
    RET_KILL_PROCESS
}

/// The chain's verdict on `data`: the lowest action (Linux compares them
/// as signed), the newest of equals.
fn verdict(mut f: Option<&KArc<Filter>>, data: &[u8; 64]) -> u32 {
    let mut ret = RET_ALLOW;
    while let Some(filter) = f {
        let cur = run(&filter.prog, data);
        if ((cur & RET_ACTION_FULL) as i32) < ((ret & RET_ACTION_FULL) as i32) {
            ret = cur;
        }
        f = filter.prev.as_ref();
    }
    ret
}

/// strict mode's calls (read, write, exit, rt_sigreturn).
fn strict_allows(number: u64) -> bool {
    use super::syscalls::nr;
    matches!(u32::try_from(number), Ok(nr::READ | nr::WRITE | nr::EXIT | nr::RT_SIGRETURN))
}

/// Before system call `number` of a thread with seccomp state: None to
/// run it, else its answer (the thread or process may be ending).
pub fn check(call: &Call<'_>, number: u64) -> Option<Result> {
    if !call.lx.seccomp_used.load(core::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    let s = call.lx.seccomp(call.tid);
    if s.strict {
        if strict_allows(number) {
            return None;
        }
        super::sigcalls::kill_self(call, lx_abi::signal::SIGKILL);
        return Some(Err(ENOSYS));
    }
    let filter = s.filter.as_ref()?;
    let mut data = [0u8; 64];
    data[0..4].copy_from_slice(&(number as u32).to_le_bytes());
    data[4..8].copy_from_slice(&AUDIT_ARCH.to_le_bytes());
    let ip = call.frame.ip();
    data[8..16].copy_from_slice(&ip.to_le_bytes());
    for i in 0..6 {
        data[16 + 8 * i..24 + 8 * i].copy_from_slice(&call.arg(i).to_le_bytes());
    }
    let ret = verdict(Some(filter), &data);
    let value = ret & RET_DATA;
    match ret & RET_ACTION_FULL {
        RET_ALLOW | RET_LOG => None,
        RET_ERRNO => Some(if value == 0 { Ok(0) } else { Err(value.min(4095) as u16) }),
        RET_TRAP => {
            super::sigcalls::seccomp_trap(call, number as i32, value as i32, ip);
            // Skipped: rax holds the call's number again (Linux rolls it
            // back), for a handler that emulates the call.
            Some(Ok(number))
        }
        RET_TRACE | RET_USER_NOTIF => Some(Err(ENOSYS)),
        RET_KILL_THREAD => {
            super::sigcalls::kill_thread_self(call, lx_abi::signal::SIGSYS);
            Some(Err(ENOSYS))
        }
        _ => {
            super::sigcalls::kill_self(call, lx_abi::signal::SIGSYS);
            Some(Err(ENOSYS))
        }
    }
}

/// Reads a `struct sock_fprog` at `addr` and its program.
fn read_prog(call: &Call<'_>, addr: u64) -> core::result::Result<Vec<Insn>, u16> {
    if addr == 0 {
        return Err(EFAULT);
    }
    let mut head = [0u8; 16];
    call.process.read(addr, &mut head).map_err(|_| EFAULT)?;
    let len = usize::from(u16::from_le_bytes([head[0], head[1]]));
    let ptr = u64::from_le_bytes(head[8..16].try_into().unwrap_or([0; 8]));
    if len == 0 || len > MAX_INSNS {
        return Err(EINVAL);
    }
    let mut raw = Vec::new();
    raw.try_reserve_exact(8 * len).map_err(|_| ENOMEM)?;
    raw.resize(8 * len, 0);
    call.process.read(ptr, &mut raw).map_err(|_| EFAULT)?;
    let mut prog = Vec::new();
    prog.try_reserve_exact(len).map_err(|_| ENOMEM)?;
    for c in raw.chunks_exact(8) {
        let _ = prog.try_push(Insn {
            code: u16::from_le_bytes([c[0], c[1]]),
            jt: c[2],
            jf: c[3],
            k: u32::from_le_bytes([c[4], c[5], c[6], c[7]]),
        });
    }
    Ok(prog)
}

/// Installs the filter at `fprog` with `flags` on the calling thread (on
/// every thread with TSYNC).
fn set_filter(call: &Call<'_>, flags: u64, fprog: u64) -> Result {
    if flags & !FLAGS != 0 {
        return Err(EINVAL);
    }
    if flags & FLAG_NEW_LISTENER != 0 {
        ledger::record("seccomp", "linux option missing", FLAG_NEW_LISTENER);
        return Err(EINVAL);
    }
    let nnp = call.lx.no_new_privs.load(core::sync::atomic::Ordering::Relaxed);
    if !nnp && super::identity::of_process(&call.process).uid != 0 {
        return Err(EACCES);
    }
    let mut prog = read_prog(call, fprog)?;
    validate(&mut prog)?;
    let cur = call.lx.seccomp(call.tid);
    if cur.strict {
        return Err(EINVAL);
    }
    let filter = KArc::try_new(Filter { prog, prev: cur.filter.clone() }).map_err(|_| ENOMEM)?;
    if filter.path_len() > MAX_PATH {
        return Err(ENOMEM);
    }
    let new = Seccomp { strict: false, filter: Some(filter) };
    call.lx.seccomp_used.store(true, core::sync::atomic::Ordering::Relaxed);
    if flags & FLAG_TSYNC != 0 {
        call.lx.set_seccomp_all(&new);
    } else {
        call.lx.set_seccomp(call.tid, new);
    }
    Ok(0)
}

fn set_strict(call: &Call<'_>) -> Result {
    let cur = call.lx.seccomp(call.tid);
    if cur.filter.is_some() {
        return Err(EINVAL);
    }
    call.lx.seccomp_used.store(true, core::sync::atomic::Ordering::Relaxed);
    call.lx.set_seccomp(call.tid, Seccomp { strict: true, filter: None });
    Ok(0)
}

/// seccomp(op, flags, args).
pub fn seccomp(call: &Call<'_>, op: u64, flags: u64, args: u64) -> Result {
    match op {
        SET_MODE_STRICT => {
            if flags != 0 || args != 0 {
                return Err(EINVAL);
            }
            set_strict(call)
        }
        SET_MODE_FILTER => set_filter(call, flags, args),
        GET_ACTION_AVAIL => {
            if flags != 0 {
                return Err(EINVAL);
            }
            let mut b = [0u8; 4];
            call.process.read(args, &mut b).map_err(|_| EFAULT)?;
            if ACTIONS.contains(&u32::from_le_bytes(b)) {
                Ok(0)
            } else {
                Err(super::errno::EOPNOTSUPP)
            }
        }
        GET_NOTIF_SIZES => {
            if flags != 0 {
                return Err(EINVAL);
            }
            // struct seccomp_notif, seccomp_notif_resp, seccomp_data.
            let mut b = [0u8; 6];
            for (i, v) in [80u16, 24, 64].iter().enumerate() {
                b[2 * i..2 * i + 2].copy_from_slice(&v.to_le_bytes());
            }
            call.process.write(args, &b).map_err(|_| EFAULT)?;
            Ok(0)
        }
        _ => Err(EINVAL),
    }
}

/// prctl(PR_GET_SECCOMP): the calling thread's mode.
pub fn get_mode(call: &Call<'_>) -> Result {
    Ok(call.lx.seccomp(call.tid).mode())
}

/// prctl(PR_SET_SECCOMP, mode, filter): 1 strict, 2 a filter.
pub fn set_mode(call: &Call<'_>, mode: u64, filter: u64) -> Result {
    match mode {
        1 => set_strict(call),
        2 => set_filter(call, 0, filter),
        _ => Err(EINVAL),
    }
}

/// SIGSYS's siginfo for a trap: SYS_SECCOMP, the filter's data as
/// si_errno, the call's address, number and architecture.
pub fn trap_info(nr: i32, data: i32, ip: u64) -> lx_abi::signal::Siginfo {
    lx_abi::signal::Siginfo::sys(SYS_SECCOMP, data, ip, nr, AUDIT_ARCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stmt(code: u16, k: u32) -> Insn {
        Insn { code, jt: 0, jf: 0, k }
    }

    #[test]
    fn filters_are_checked_as_linux_checks_them() {
        assert_eq!(validate(&mut []), Err(EINVAL));
        assert_eq!(validate(&mut [stmt(LD | W | ABS, 0)]), Err(EINVAL));
        assert_eq!(validate(&mut [stmt(LD | W | ABS, 2), stmt(RET | K, RET_ALLOW)]), Err(EINVAL));
        assert_eq!(validate(&mut [stmt(LD | W | ABS, 64), stmt(RET | K, RET_ALLOW)]), Err(EINVAL));
        assert_eq!(validate(&mut [stmt(ALU | DIV | K, 0), stmt(RET | K, RET_ALLOW)]), Err(EINVAL));
        let far = Insn { code: JMP | JEQ | K, jt: 5, jf: 0, k: 1 };
        assert_eq!(validate(&mut [far, stmt(RET | K, RET_ALLOW)]), Err(EINVAL));
        assert_eq!(validate(&mut [stmt(LD | W | ABS, 4), stmt(RET | A, 0)]), Ok(()));
    }

    #[test]
    fn a_filter_answers_for_one_call() {
        // ERRNO(42) for nr 110 on x86_64, ALLOW otherwise.
        let mut prog = [
            stmt(LD | W | ABS, 4),
            Insn { code: JMP | JEQ | K, jt: 1, jf: 0, k: AUDIT_ARCH },
            stmt(RET | K, RET_KILL_PROCESS),
            stmt(LD | W | ABS, 0),
            Insn { code: JMP | JEQ | K, jt: 0, jf: 1, k: 110 },
            stmt(RET | K, RET_ERRNO | 42),
            stmt(RET | K, RET_ALLOW),
        ];
        validate(&mut prog).unwrap();
        let mut data = [0u8; 64];
        data[4..8].copy_from_slice(&AUDIT_ARCH.to_le_bytes());
        data[0..4].copy_from_slice(&110u32.to_le_bytes());
        assert_eq!(run(&prog, &data), RET_ERRNO | 42);
        data[0..4].copy_from_slice(&39u32.to_le_bytes());
        assert_eq!(run(&prog, &data), RET_ALLOW);
        data[4..8].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(run(&prog, &data), RET_KILL_PROCESS);
    }

    #[test]
    fn the_lowest_action_wins_the_newest_of_equals() {
        let make = |k: u32, prev: Option<KArc<Filter>>| {
            KArc::try_new(Filter { prog: alloc::vec![stmt(RET | K, k)], prev }).unwrap()
        };
        let a = make(RET_ERRNO | 5, None);
        let b = make(RET_ERRNO | 6, Some(a));
        let c = make(RET_ALLOW, Some(b));
        assert_eq!(verdict(Some(&c), &[0; 64]), RET_ERRNO | 6);
        let d = make(RET_KILL_PROCESS, Some(c));
        assert_eq!(verdict(Some(&d), &[0; 64]), RET_KILL_PROCESS);
    }
}
