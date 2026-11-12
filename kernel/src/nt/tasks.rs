//! The task manager's data (ROADMAP TM step 1; ARCHITECTURE "The task
//! manager"): one query that lists every process of both personalities,
//! Windows and Linux (a WSL distribution's and the Linux personality's
//! own), and one for the machine's load, both through GalaTasks
//! (0x7fe8), gala's own call; and the Linux side of "End task" (a signal
//! for a Linux process, the end of a whole distribution).
//!
//! Windows programs see Linux processes nowhere else:
//! NtQuerySystemInformation's SystemProcessInformation keeps Windows'
//! contents (NT processes only), and `/proc` inside LX shows its own as
//! Linux does. gala's `taskmgr.exe` and `gala ps` read this one.
//!
//! **Processes** (op 0): a header (`HEADER` bytes: version, count, the
//! record size, processors, the monotonic time in ns), then a record of
//! `RECORD` bytes per running process, then the texts they point at
//! (UTF-8, offsets from the buffer's start). A record:
//!
//! | Offset | Field |
//! |---|---|
//! | 0 | pid (u64) |
//! | 8 | parent pid (u64; 0 for one the kernel started) |
//! | 16 | start time (NT time, u64) |
//! | 24 | CPU time in ns (u64: its threads', ended and live) |
//! | 32 | working set in bytes (u64: committed pages stay resident) |
//! | 40 | commit charge in bytes (u64) |
//! | 48 | HWND of its first taskbar window (u64; 0: none) |
//! | 56 | threads (u32) |
//! | 60 | handles (NT) or open fds (Linux) (u32) |
//! | 64 | session (u32) |
//! | 68 | flags (u32: `FLAG_*`) |
//! | 72 | Linux uid of its token (u32) |
//! | 76 | reserved (u32) |
//! | 80 | image name (offset u32, length u32): an NT process's file name, a Linux one's `comm` |
//! | 88 | command line |
//! | 96 | user (the token's account name; a SID string when it has none) |
//! | 104 | WSL distribution (empty: not a distribution's) |
//! | 112 | its taskbar window's title |
//! | 120 | reserved (u64) |
//!
//! **Performance** (op 1): `PERF_HEADER` bytes (version, processors, the
//! monotonic time, physical memory and what is in use, the commit
//! charge, disk bytes read and written, network bytes received and sent,
//! whether a disk and a network card exist, the process, thread and
//! handle counts), then per processor its busy and idle ns (two u64).
//!
//! **End task**: Windows processes end through TerminateProcess (the
//! caller's own handle and its access check); op 2 signals a Linux
//! process (`arg` the pid, `length` the signal: SIGTERM, then SIGKILL
//! if it stays), allowed when kill(2) would allow it to a Linux program of the
//! caller's token (the same uid, or root) or for an elevated caller; op
//! 3 ends a WSL distribution's whole tree (`wsl --terminate`'s SIGKILL),
//! by name, under the same rule for every process in it.
//!
//! Observability: `tasks:` log lines (the first query of each caller,
//! every end), `process` events `tasks query` and `tasks end`, and the
//! ledger (`tasks`) for what no record carries yet (a Linux thread's
//! uninterruptible wait: gala's Linux waits are all interruptible).

use alloc::vec::Vec;

use crate::mm::heap::{AllocError, TryVec};

/// The layout's version (the header's first word).
pub const VERSION: u32 = 1;
/// The processes' header and record sizes.
pub const HEADER: usize = 32;
pub const RECORD: usize = 128;
/// The performance header's size (the processors' pairs follow).
pub const PERF_HEADER: usize = 128;

/// A Linux process (the Linux personality).
pub const FLAG_LINUX: u32 = 1;
/// It has a window on the taskbar (Windows' "Apps").
pub const FLAG_WINDOW: u32 = 2;
/// Its token is elevated.
pub const FLAG_ELEVATED: u32 = 4;
/// A 32-bit (WOW64) process.
pub const FLAG_WOW64: u32 = 8;
/// "Not responding" (ROADMAP TM step 3): input has waited for its
/// windows, untaken, for 5 s (`winstate::HUNG_NS`, Windows' rule).
pub const FLAG_HUNG: u32 = 16;
/// A Linux process in an uninterruptible wait (never set: gala's Linux
/// waits are all interruptible; ledgered).
pub const FLAG_UNINTERRUPTIBLE: u32 = 32;

/// The texts of a record, in the order of their slots (80 to 112).
pub const TEXTS: usize = 5;

/// One process's record before it is encoded.
#[derive(Default)]
pub struct Task {
    pub pid: u64,
    pub parent: u64,
    pub create_time: u64,
    pub cpu_ns: u64,
    pub working_set: u64,
    pub commit: u64,
    pub hwnd: u64,
    pub threads: u32,
    pub handles: u32,
    pub session: u32,
    pub flags: u32,
    pub uid: u32,
    /// Image, command line, user, distribution, window title.
    pub texts: [Vec<u8>; TEXTS],
}

/// The processes' answer: the header, the records, their texts.
pub fn encode(now_ns: u64, cpus: u32, tasks: &[Task]) -> Result<Vec<u8>, AllocError> {
    let fixed = HEADER + RECORD * tasks.len();
    let texts: usize = tasks.iter().flat_map(|t| t.texts.iter()).map(Vec::len).sum();
    let mut out = Vec::new();
    out.try_reserve_exact(fixed + texts).map_err(|_| AllocError)?;
    out.resize(fixed, 0);
    put32(&mut out, 0, VERSION);
    put32(&mut out, 4, tasks.len() as u32);
    put32(&mut out, 8, RECORD as u32);
    put32(&mut out, 12, cpus);
    put64(&mut out, 16, now_ns);
    for (i, t) in tasks.iter().enumerate() {
        let r = HEADER + RECORD * i;
        put64(&mut out, r, t.pid);
        put64(&mut out, r + 8, t.parent);
        put64(&mut out, r + 16, t.create_time);
        put64(&mut out, r + 24, t.cpu_ns);
        put64(&mut out, r + 32, t.working_set);
        put64(&mut out, r + 40, t.commit);
        put64(&mut out, r + 48, t.hwnd);
        put32(&mut out, r + 56, t.threads);
        put32(&mut out, r + 60, t.handles);
        put32(&mut out, r + 64, t.session);
        put32(&mut out, r + 68, t.flags);
        put32(&mut out, r + 72, t.uid);
        for (k, text) in t.texts.iter().enumerate() {
            let at = out.len();
            out.try_extend_from_slice(text)?;
            put32(&mut out, r + 80 + 8 * k, at as u32);
            put32(&mut out, r + 84 + 8 * k, text.len() as u32);
        }
    }
    Ok(out)
}

/// The machine's load (op 1) before it is encoded.
#[derive(Default)]
pub struct Perf {
    pub now_ns: u64,
    pub memory_total: u64,
    pub memory_used: u64,
    pub commit: u64,
    pub disk_read: u64,
    pub disk_written: u64,
    pub net_received: u64,
    pub net_sent: u64,
    pub has_disk: bool,
    pub has_network: bool,
    pub processes: u32,
    pub threads: u32,
    pub handles: u32,
    /// Per processor: (busy, idle) ns.
    pub cpus: Vec<(u64, u64)>,
}

/// The performance answer.
pub fn encode_perf(p: &Perf) -> Result<Vec<u8>, AllocError> {
    let mut out = Vec::new();
    out.try_reserve_exact(PERF_HEADER + 16 * p.cpus.len()).map_err(|_| AllocError)?;
    out.resize(PERF_HEADER + 16 * p.cpus.len(), 0);
    put32(&mut out, 0, VERSION);
    put32(&mut out, 4, p.cpus.len() as u32);
    put64(&mut out, 8, p.now_ns);
    put64(&mut out, 16, p.memory_total);
    put64(&mut out, 24, p.memory_used);
    put64(&mut out, 32, p.commit);
    put64(&mut out, 40, p.disk_read);
    put64(&mut out, 48, p.disk_written);
    put64(&mut out, 56, p.net_received);
    put64(&mut out, 64, p.net_sent);
    put32(&mut out, 72, u32::from(p.has_disk) | u32::from(p.has_network) << 1);
    put32(&mut out, 76, p.processes);
    put32(&mut out, 80, p.threads);
    put32(&mut out, 84, p.handles);
    for (i, (busy, idle)) in p.cpus.iter().enumerate() {
        put64(&mut out, PERF_HEADER + 16 * i, *busy);
        put64(&mut out, PERF_HEADER + 16 * i + 8, *idle);
    }
    Ok(out)
}

fn put32(out: &mut [u8], at: usize, v: u32) {
    out[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn put64(out: &mut [u8], at: usize, v: u64) {
    out[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

/// A SID's string form (`S-1-5-21-...`) from its binary form.
pub fn sid_string(binary: &[u8], out: &mut Vec<u8>) -> Result<(), AllocError> {
    use core::fmt::Write;
    struct W<'a>(&'a mut Vec<u8>, bool);
    impl Write for W<'_> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            if self.0.try_extend_from_slice(s.as_bytes()).is_err() {
                self.1 = true;
                return Err(core::fmt::Error);
            }
            Ok(())
        }
    }
    let (Some(&revision), Some(&count)) = (binary.first(), binary.get(1)) else {
        return Ok(());
    };
    let authority =
        binary.get(2..8).map_or(0, |a| a.iter().fold(0u64, |acc, &b| acc << 8 | u64::from(b)));
    let mut w = W(out, false);
    let _ = write!(w, "S-{revision}-{authority}");
    for i in 0..usize::from(count) {
        let at = 8 + 4 * i;
        let Some(b) = binary.get(at..at + 4) else { break };
        let _ = write!(w, "-{}", u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    }
    if w.1 { Err(AllocError) } else { Ok(()) }
}

/// UTF-16 as UTF-8 (unpaired surrogates as U+FFFD).
pub fn utf8_of(text: &[u16], out: &mut Vec<u8>) -> Result<(), AllocError> {
    for c in char::decode_utf16(text.iter().copied()) {
        let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
        let mut b = [0u8; 4];
        out.try_extend_from_slice(c.encode_utf8(&mut b).as_bytes())?;
    }
    Ok(())
}

pub use live::*;

/// What gathers the records from the running system.
mod live {
    use super::*;
    use crate::fs::ramfs::NodeId;
    use crate::mm::heap::KArc;
    use crate::nt::object::ObjectBody;
    use crate::nt::platform::{self, Personality};
    use crate::nt::process::{self, Process};
    use crate::nt::status::NtStatus;
    use core::sync::atomic::{AtomicU64, Ordering};

    /// The callers whose first query was logged (each caller's first
    /// query is, once; a slot per caller, the oldest reused).
    static LOGGED: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
    static NEXT: AtomicU64 = AtomicU64::new(0);

    /// The processes whose "Not responding" was logged (once each).
    static HUNG: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
    static NEXT_HUNG: AtomicU64 = AtomicU64::new(0);

    /// Whether `pid` is not yet in `seen` (and notes it, `next` the slot
    /// to reuse).
    fn first_in(seen: &[AtomicU64], next: &AtomicU64, pid: u64) -> bool {
        if seen.iter().any(|p| p.load(Ordering::Relaxed) == pid) {
            return false;
        }
        let slot = next.fetch_add(1, Ordering::Relaxed) as usize % seen.len();
        seen[slot].store(pid, Ordering::Relaxed);
        true
    }

    /// Whether `pid`'s query is its first (and notes it).
    fn first_query(pid: u64) -> bool {
        first_in(&LOGGED, &NEXT, pid)
    }

    /// Whether `pid` is seen not responding for the first time.
    fn first_hung(pid: u64) -> bool {
        first_in(&HUNG, &NEXT_HUNG, pid)
    }

    fn no_memory(_: AllocError) -> NtStatus {
        NtStatus::NO_MEMORY
    }

    /// The account name of `p`'s token user: SYSTEM and the services'
    /// well-known accounts, a local account's name, else the SID.
    fn user_of(p: &Process, users: &[(Vec<u16>, u32)], out: &mut Vec<u8>) -> Result<(), NtStatus> {
        let token = p.token.lock().clone();
        let ObjectBody::Token(t) = &token.body else { return Ok(()) };
        let sid = t.data.lock().user.sid;
        let known: &[(crate::nt::token::Sid, &str)] = &[
            (crate::nt::token::LOCAL_SYSTEM, "SYSTEM"),
            (crate::nt::token::Sid::new(5, &[19]), "LOCAL SERVICE"),
            (crate::nt::token::Sid::new(5, &[20]), "NETWORK SERVICE"),
        ];
        if let Some((_, name)) = known.iter().find(|(s, _)| *s == sid) {
            return out.try_extend_from_slice(name.as_bytes()).map_err(no_memory);
        }
        if let Some(rid) = crate::nt::accounts::local_rid(&sid)
            && let Some((name, _)) = users.iter().find(|(_, r)| *r == rid)
        {
            return utf8_of(name, out).map_err(no_memory);
        }
        let mut binary = [0u8; 68];
        sid.write(&mut binary);
        sid_string(&binary[..sid.len()], out).map_err(no_memory)
    }

    /// Every running process of both personalities (GalaTasks op 0), for
    /// `caller`.
    pub fn processes(caller: &Process) -> Result<Vec<u8>, NtStatus> {
        let list = process::running().map_err(no_memory)?;
        let users = crate::nt::accounts::local_users().unwrap_or_default();
        let distros = crate::nt::desktop::distro_roots();
        // The taskbar windows of each session the processes are in (one
        // listing per session).
        let mut windows: Vec<(u32, Vec<crate::nt::desktop::Listed>)> = Vec::new();
        for p in &list {
            if windows.iter().any(|(s, _)| *s == p.session) {
                continue;
            }
            let listed = crate::nt::desktop::list_windows(&p.desk.shell).unwrap_or_default();
            windows.try_push((p.session, listed)).map_err(no_memory)?;
        }
        let mut tasks: Vec<Task> = Vec::new();
        tasks.try_reserve_exact(list.len()).map_err(|_| NtStatus::NO_MEMORY)?;
        let (mut nt, mut linux, mut hung) = (0usize, 0usize, 0usize);
        for p in &list {
            let mut t = Task {
                pid: p.id,
                parent: p.parent_id,
                create_time: p.create_time,
                cpu_ns: p.cpu_time_ns(),
                commit: p.commit.load(Ordering::Relaxed) * platform::PAGE_SIZE,
                threads: p.thread_count() as u32,
                session: p.session,
                uid: crate::lx::identity::of_process(p).uid,
                ..Task::default()
            };
            t.working_set = p.vad.lock().sizes().0.1;
            if p.elevated() {
                t.flags |= FLAG_ELEVATED;
            }
            if p.wow64.is_some() {
                t.flags |= FLAG_WOW64;
            }
            t.texts[1].try_extend_from_slice(&p.command_line.lock()).map_err(no_memory)?;
            user_of(p, &users, &mut t.texts[2])?;
            if p.personality == Personality::Linux {
                linux += 1;
                t.flags |= FLAG_LINUX;
                linux_parts(p, &distros, &mut t)?;
            } else {
                nt += 1;
                t.handles = p.handles.lock().iter().count() as u32;
                t.texts[0].try_extend_from_slice(p.name.as_bytes()).map_err(no_memory)?;
            }
            if crate::nt::desktop::not_responding(&p.desk.shell, p.id) {
                t.flags |= FLAG_HUNG;
                hung += 1;
                if first_hung(p.id) {
                    crate::info!(
                        "tasks: pid {} ({}) is not responding: its input has waited untaken \
                         for 5 s or more",
                        p.id,
                        p.name
                    );
                }
            }
            let listed = windows.iter().find(|(s, _)| *s == p.session);
            if let Some(w) = listed.and_then(|(_, l)| l.iter().find(|w| w.owner == p.id)) {
                t.flags |= FLAG_WINDOW;
                t.hwnd = p.desk.windows.hwnd_of_desktop(p.id as u32, w.id).unwrap_or(0);
                utf8_of(&w.title[..w.title_len], &mut t.texts[4]).map_err(no_memory)?;
            }
            tasks.try_push(t).map_err(no_memory)?;
        }
        let cpus = platform::online_count();
        let out = encode(platform::monotonic_ns(), cpus, &tasks).map_err(no_memory)?;
        if first_query(caller.id) {
            crate::info!(
                "tasks: pid {} ({}) lists the processes: {nt} Windows, {linux} Linux, \
                 {} distribution(s) registered",
                caller.id,
                caller.name,
                distros.len()
            );
            // What no record carries yet.
            crate::nt::ledger::record("tasks", "linux uninterruptible wait", 0);
        }
        crate::events::emit(
            crate::events::Category::Process,
            "tasks query",
            format_args!(r#""by":{},"nt":{nt},"linux":{linux},"hung":{hung}"#, caller.id),
        );
        Ok(out)
    }

    /// A Linux process's image (`comm`), fds and distribution.
    fn linux_parts(
        p: &Process,
        distros: &[(crate::nt::desktop::linux::Name, NodeId)],
        t: &mut Task,
    ) -> Result<(), NtStatus> {
        let Some(lx) = crate::lx::state::get(p.id) else {
            // Between its start and its Linux state, or after its end.
            return t.texts[0].try_extend_from_slice(p.name.as_bytes()).map_err(no_memory);
        };
        let comm = lx.comm(lx.main_tid.load(Ordering::Relaxed));
        let n = comm.0.iter().position(|&b| b == 0).unwrap_or(comm.0.len());
        let name = if n == 0 { p.name.as_bytes() } else { &comm.0[..n] };
        t.texts[0].try_extend_from_slice(name).map_err(no_memory)?;
        t.handles = lx.fds.lock().iter().filter(|s| s.is_some()).count() as u32;
        if let Some((name, _)) = distros.iter().find(|(_, root)| *root == lx.root) {
            t.texts[3].try_extend_from_slice(name.as_bytes()).map_err(no_memory)?;
        }
        Ok(())
    }

    /// The machine's load (GalaTasks op 1).
    pub fn perf() -> Result<Vec<u8>, NtStatus> {
        let (free, total) = platform::frame_stats();
        let mut p = Perf {
            now_ns: platform::monotonic_ns(),
            memory_total: total * platform::PAGE_SIZE,
            memory_used: total.saturating_sub(free) * platform::PAGE_SIZE,
            commit: crate::nt::charge::committed().0 * platform::PAGE_SIZE,
            disk_read: crate::disk::READ_BYTES.load(Ordering::Relaxed),
            disk_written: crate::disk::WRITE_BYTES.load(Ordering::Relaxed),
            net_received: crate::net::virtio_net::RX_BYTES.load(Ordering::Relaxed),
            net_sent: crate::net::virtio_net::TX_BYTES.load(Ordering::Relaxed),
            has_disk: crate::disk::present(),
            has_network: crate::net::has_card(),
            ..Perf::default()
        };
        for q in process::running().map_err(no_memory)? {
            p.processes += 1;
            p.threads += q.thread_count() as u32;
            p.handles += q.handles.lock().iter().count() as u32;
        }
        let online = platform::online_mask();
        for cpu in 0..64 {
            if online & (1 << cpu) == 0 {
                continue;
            }
            let load = platform::cpu_load(cpu).unwrap_or((0, 0));
            p.cpus.try_push(load).map_err(no_memory)?;
        }
        encode_perf(&p).map_err(no_memory)
    }

    /// Whether `caller` may signal Linux process `target` (kill(2)'s
    /// rule for a Linux program of the caller's token: the same uid, or
    /// root; or an elevated caller).
    fn may_signal(caller: &Process, target: &Process) -> bool {
        let from = crate::lx::identity::of_process(caller).uid;
        from == 0 || caller.elevated() || from == crate::lx::identity::of_process(target).uid
    }

    /// One end-task line and event.
    fn ended(caller: &Process, what: &str, pid: u64, detail: &str, status: NtStatus) {
        let mut target = [0u8; 24];
        let target = if pid == 0 { "" } else { fmt_detail(&mut target, format_args!(" {pid}")) };
        crate::info!(
            "tasks: pid {} ({}) ends {what}{target}{detail}: {:#010x}",
            caller.id,
            caller.name,
            status.0
        );
        crate::events::emit(
            crate::events::Category::Process,
            "tasks end",
            format_args!(
                r#""by":{},"what":"{what}","pid":{pid},"detail":{},"status":{}"#,
                caller.id,
                crate::events::Str(detail),
                status.0
            ),
        );
    }

    /// Signal `sig` for Linux process `pid` (GalaTasks op 2).
    pub fn signal(caller: &Process, pid: u64, sig: u32) -> NtStatus {
        let status = (|| {
            if sig == 0 || sig > lx_abi::signal::NSIG {
                return NtStatus::INVALID_PARAMETER;
            }
            let Some(target) = process::by_id(pid) else { return NtStatus::INVALID_CID };
            if target.personality != Personality::Linux {
                // A Windows process ends through TerminateProcess.
                return NtStatus::INVALID_PARAMETER;
            }
            if !may_signal(caller, &target) {
                return NtStatus::ACCESS_DENIED;
            }
            match crate::lx::sigcalls::signal_from(caller, &target, sig) {
                Ok(_) => NtStatus::SUCCESS,
                Err(_) => NtStatus::INVALID_CID,
            }
        })();
        let mut detail = [0u8; 24];
        let detail = fmt_detail(&mut detail, format_args!(" with signal {sig}"));
        ended(caller, "linux process", pid, detail, status);
        status
    }

    /// Ends distribution `name`'s every process (GalaTasks op 3); how
    /// many.
    pub fn end_distribution(caller: &Process, name: &[u8]) -> Result<usize, NtStatus> {
        let result = (|| {
            let root =
                crate::nt::desktop::distro_root_of(name).ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)?;
            let pids = crate::lx::distro::processes(Some(root)).map_err(|_| NtStatus::NO_MEMORY)?;
            let targets: Vec<KArc<Process>> = {
                let mut v = Vec::new();
                for pid in pids {
                    if let Some(p) = process::by_id(pid) {
                        v.try_push(p).map_err(no_memory)?;
                    }
                }
                v
            };
            if targets.iter().any(|t| !may_signal(caller, t)) {
                return Err(NtStatus::ACCESS_DENIED);
            }
            drop(targets);
            crate::lx::distro::terminate(Some(root)).map_err(|_| NtStatus::NO_MEMORY)
        })();
        let shown = core::str::from_utf8(name).unwrap_or("?");
        let status = result.as_ref().map_or_else(|s| *s, |_| NtStatus::SUCCESS);
        let mut detail = [0u8; 96];
        let n = result.as_ref().copied().unwrap_or(0);
        let detail = fmt_detail(&mut detail, format_args!(" ({shown}: {n} process(es))"));
        ended(caller, "linux distribution", 0, detail, status);
        if result.is_ok() {
            crate::events::emit(
                crate::events::Category::Wsl,
                "wsl terminate",
                format_args!(
                    r#""distro":{},"by":{},"count":{n}"#,
                    crate::events::Str(shown),
                    caller.id
                ),
            );
        }
        result
    }

    /// `args` formatted into `buf` (cut at its end).
    fn fmt_detail<'b>(buf: &'b mut [u8], args: core::fmt::Arguments) -> &'b str {
        struct W<'a>(&'a mut [u8], usize);
        impl core::fmt::Write for W<'_> {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                for &b in s.as_bytes() {
                    if self.1 < self.0.len() {
                        self.0[self.1] = b;
                        self.1 += 1;
                    }
                }
                Ok(())
            }
        }
        let mut w = W(buf, 0);
        let _ = core::fmt::write(&mut w, args);
        let n = w.1;
        core::str::from_utf8(&buf[..n]).unwrap_or("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn u32_at(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
    }

    fn u64_at(b: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
    }

    fn text(b: &[u8], record: usize, k: usize) -> &[u8] {
        let at = u32_at(b, record + 80 + 8 * k) as usize;
        let n = u32_at(b, record + 84 + 8 * k) as usize;
        &b[at..at + n]
    }

    #[test]
    fn records_and_their_texts() {
        let tasks = [
            Task {
                pid: 4,
                parent: 0,
                create_time: 1234,
                cpu_ns: 5_000_000,
                working_set: 8192,
                commit: 4096,
                hwnd: 0x20010,
                threads: 3,
                handles: 17,
                session: 1,
                flags: FLAG_WINDOW,
                uid: 1000,
                texts: [
                    b"taskmgr.exe".to_vec(),
                    b"C:\\Windows\\System32\\taskmgr.exe".to_vec(),
                    b"gala".to_vec(),
                    vec![],
                    b"Task Manager".to_vec(),
                ],
            },
            Task {
                pid: 9,
                parent: 7,
                threads: 1,
                handles: 3,
                flags: FLAG_LINUX,
                texts: [
                    b"sleep".to_vec(),
                    b"sleep 600".to_vec(),
                    b"gala".to_vec(),
                    b"bb".to_vec(),
                    vec![],
                ],
                ..Task::default()
            },
        ];
        let b = encode(77, 2, &tasks).unwrap();
        assert_eq!(
            (u32_at(&b, 0), u32_at(&b, 4), u32_at(&b, 8), u32_at(&b, 12)),
            (VERSION, 2, 128, 2)
        );
        assert_eq!(u64_at(&b, 16), 77);
        let r0 = HEADER;
        assert_eq!(
            (u64_at(&b, r0), u64_at(&b, r0 + 16), u64_at(&b, r0 + 24)),
            (4, 1234, 5_000_000)
        );
        assert_eq!(
            (u64_at(&b, r0 + 32), u64_at(&b, r0 + 40), u64_at(&b, r0 + 48)),
            (8192, 4096, 0x20010)
        );
        assert_eq!((u32_at(&b, r0 + 56), u32_at(&b, r0 + 60), u32_at(&b, r0 + 64)), (3, 17, 1));
        assert_eq!((u32_at(&b, r0 + 68), u32_at(&b, r0 + 72)), (FLAG_WINDOW, 1000));
        assert_eq!(text(&b, r0, 0), b"taskmgr.exe");
        assert_eq!(text(&b, r0, 4), b"Task Manager");
        assert_eq!(text(&b, r0, 3), b"");
        let r1 = HEADER + RECORD;
        assert_eq!((u64_at(&b, r1), u64_at(&b, r1 + 8)), (9, 7));
        assert_eq!(u32_at(&b, r1 + 68), FLAG_LINUX);
        assert_eq!(text(&b, r1, 1), b"sleep 600");
        assert_eq!(text(&b, r1, 3), b"bb");
        // The texts follow the records, nothing after them.
        let total: usize = tasks.iter().flat_map(|t| t.texts.iter()).map(|t| t.len()).sum();
        assert_eq!(b.len(), HEADER + 2 * RECORD + total);
    }

    #[test]
    fn no_processes_is_a_header() {
        let b = encode(1, 1, &[]).unwrap();
        assert_eq!(b.len(), HEADER);
        assert_eq!(u32_at(&b, 4), 0);
    }

    #[test]
    fn performance_layout() {
        let p = Perf {
            now_ns: 5,
            memory_total: 512 << 20,
            memory_used: 100 << 20,
            commit: 80 << 20,
            disk_read: 1,
            disk_written: 2,
            net_received: 3,
            net_sent: 4,
            has_disk: false,
            has_network: true,
            processes: 6,
            threads: 20,
            handles: 300,
            cpus: vec![(10, 90), (50, 50)],
        };
        let b = encode_perf(&p).unwrap();
        assert_eq!(b.len(), PERF_HEADER + 32);
        assert_eq!((u32_at(&b, 0), u32_at(&b, 4), u64_at(&b, 8)), (VERSION, 2, 5));
        assert_eq!(
            (u64_at(&b, 16), u64_at(&b, 24), u64_at(&b, 32)),
            (512 << 20, 100 << 20, 80 << 20)
        );
        assert_eq!((u64_at(&b, 40), u64_at(&b, 48), u64_at(&b, 56), u64_at(&b, 64)), (1, 2, 3, 4));
        assert_eq!(u32_at(&b, 72), 2);
        assert_eq!((u32_at(&b, 76), u32_at(&b, 80), u32_at(&b, 84)), (6, 20, 300));
        assert_eq!((u64_at(&b, PERF_HEADER + 16), u64_at(&b, PERF_HEADER + 24)), (50, 50));
    }

    #[test]
    fn sid_strings() {
        let mut s = Vec::new();
        // S-1-5-18 (SYSTEM): revision 1, one sub-authority, authority 5.
        sid_string(&[1, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0], &mut s).unwrap();
        assert_eq!(s, b"S-1-5-18");
        let mut s = Vec::new();
        sid_string(&[1, 2, 0, 0, 0, 0, 0, 5, 32, 0, 0, 0, 0x20, 2, 0, 0], &mut s).unwrap();
        assert_eq!(s, b"S-1-5-32-544");
    }

    #[test]
    fn titles_become_utf8() {
        let mut s = Vec::new();
        utf8_of(&"Görev Yöneticisi".encode_utf16().collect::<Vec<_>>(), &mut s).unwrap();
        assert_eq!(s, "Görev Yöneticisi".as_bytes());
        let mut s = Vec::new();
        utf8_of(&[0xd800, b'a' as u16], &mut s).unwrap();
        assert_eq!(s, "\u{fffd}a".as_bytes());
    }
}
