//! Hot swapping gala's own DLLs into a machine resumed from a snapshot
//! (ROADMAP SN step 1d; ARCHITECTURE "Hot swapping DLLs (SN step 1d)";
//! WORKFLOW "Hot swapping DLLs into a snapshot (at your own risk)").
//!
//! **Not a real run.** xtask (`xtask/src/hotswap.rs`) compares the
//! snapshot's build of a DLL with a new one and hands the resumed machine
//! a plan through QEMU's fw_cfg (`opt/gala/hotswap`), with an NMI queued
//! before the machine runs its first instruction. Here:
//!
//! 1. **The NMI** (`on_nmi`, before the state dump's): the plan's header
//!    is read; a plan not applied yet stops every program thread on its
//!    way back to user mode (`thread::hold_from_nmi`; a CPU the NMI took
//!    from user mode interrupts itself, so the thread stops before its
//!    next user instruction). No user code runs from the resume on.
//! 2. **The boot thread** (its once-a-second wait, as the state dump's
//!    second part) runs `run`: the plan is read and checked (magic,
//!    version, length, CRC-32), every thread is waited for until none
//!    runs user code (held, in a system call, suspended or not started),
//!    then for each DLL of the plan:
//!    - the processes that map the snapshot's build of it (by name, and
//!      by the TimeDateStamp and SizeOfImage of the mapped headers);
//!    - **the stack check**: no thread of those processes executes a
//!      changed function (its instruction pointer inside one, past the
//!      entry) or has a return address into one: every word from its
//!      stack pointer to the top of its stack region is looked at (a
//!      conservative scan: a stale value counts too), the 32-bit stack
//!      for a 32-bit DLL;
//!    - per process, the blob (the new code and data) is mapped near the
//!      image (within rel32 reach for x64), its references bound to the
//!      old image's addresses and to its own, its code made
//!      PAGE_EXECUTE_READ; each changed function's old entry becomes a
//!      `jmp rel32` to its copy (written through copy-on-write, so the
//!      image's shared pages stay as they were);
//!    - the DLL's file is replaced by the new build, so later loads get
//!      it whole.
//!
//!    Any failure undoes what was done (entries restored, blobs freed,
//!    the file put back) and refuses with the reason. The programs go on
//!    either way.
//! 3. **Marked**: `hotswap:` log lines, an event (`"ev":"hotswap"`,
//!    whatever the mask), a ledger entry, and every crash report and state
//!    dump after it says the run was hot-swapped.
//!
//! What is not checked (the user's risk): state that old code built and
//! new code reads differently, invariants of data whose layout did not
//! change, new code's unwinding (no unwind data is registered for the
//! blob), new exports (the old export table is kept).

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

use crate::mm::heap::{KArc, TryVec};
use crate::nt::process::Process;
use crate::nt::{file, platform, process, thread, vad, vm};

/// The plan's first bytes (xtask's `hotswap::MAGIC`).
pub const MAGIC: &[u8; 8] = b"GALAHSWP";
/// The plan format this kernel reads.
pub const VERSION: u32 = 1;
/// The fw_cfg file xtask gives the plan as.
pub const FW_CFG_NAME: &[u8] = b"opt/gala/hotswap";
const HEADER: usize = 32;
const FIXUP: usize = 24;
const ENTRY: usize = 16;
/// A patched entry: `jmp rel32`.
const JUMP: usize = 5;
/// How long the programs may take to stop running user code.
const QUIET_MS: u64 = 3000;
/// The most of a stack the check reads.
const STACK_LIMIT: u64 = 8 << 20;

/// Why a plan cannot be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanError(pub &'static str);

/// One DLL of the plan (xtask's `hotswap::Patch`).
#[derive(Clone, Copy, Debug)]
pub struct ImagePlan<'a> {
    /// 1: x64, 2: x86.
    pub machine: u8,
    pub old_timestamp: u32,
    pub old_size_of_image: u32,
    /// The file in the guest (`\Windows\System32\x.dll`).
    pub path: &'a str,
    pub blob: &'a [u8],
    /// Where writable data starts in the blob (a page boundary).
    pub data_off: u32,
    fixups: &'a [u8],
    entries: &'a [u8],
    strings: &'a [u8],
    /// The new build's file.
    pub file: &'a [u8],
}

/// A value to write into the blob once its address is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fixup {
    pub at: u32,
    /// Relative to the blob's address at this offset; None: absolute.
    pub anchor: Option<u32>,
    pub target: i64,
    pub width: u8,
    /// 0: an RVA in the old image; 1: an offset in the blob.
    pub space: u8,
}

/// A changed function: its old place and its copy in the blob.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry<'a> {
    pub rva: u32,
    pub size: u32,
    pub at: u32,
    pub name: &'a str,
}

/// A plan: the DLLs to swap.
pub struct Plan<'a> {
    pub nonce: u64,
    pub images: alloc::vec::Vec<ImagePlan<'a>>,
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, PlanError> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or(PlanError("truncated"))
}

fn u64_at(b: &[u8], at: usize) -> Result<u64, PlanError> {
    Ok(u64::from(u32_at(b, at)?) | (u64::from(u32_at(b, at + 4)?) << 32))
}

/// A plan's nonce, if `header` (its first 32 bytes) is one this kernel
/// reads.
pub fn header_nonce(header: &[u8]) -> Option<u64> {
    (header.get(..8) == Some(MAGIC) && u32_at(header, 8).ok() == Some(VERSION))
        .then(|| u64_at(header, 16).ok())
        .flatten()
}

impl<'a> Plan<'a> {
    /// Reads plan `b` (checked whole: length and CRC-32 first).
    pub fn parse(b: &'a [u8]) -> Result<Plan<'a>, PlanError> {
        if b.len() < HEADER || &b[..8] != MAGIC {
            return Err(PlanError("not a hot swap plan"));
        }
        if u32_at(b, 8)? != VERSION {
            return Err(PlanError("a plan format this kernel does not read"));
        }
        if u32_at(b, 24)? as usize != b.len() {
            return Err(PlanError("its length is not what its header says"));
        }
        let crc = crate::fs::log::crc32(0, &b[..28]);
        let crc = crate::fs::log::crc32(crc, &[0; 4]);
        if crate::fs::log::crc32(crc, &b[HEADER..]) != u32_at(b, 28)? {
            return Err(PlanError("its CRC-32 does not match"));
        }
        let count = u32_at(b, 12)? as usize;
        let mut images = alloc::vec::Vec::new();
        let mut at = HEADER;
        for _ in 0..count {
            let len = u32_at(b, at)? as usize;
            let rec = b.get(at..at + len).ok_or(PlanError("an image record is truncated"))?;
            images.try_push(ImagePlan::parse(rec)?).map_err(|_| PlanError("out of memory"))?;
            at += len;
        }
        if at != b.len() {
            return Err(PlanError("bytes after the last image record"));
        }
        Ok(Plan { nonce: u64_at(b, 16)?, images })
    }
}

impl<'a> ImagePlan<'a> {
    fn parse(r: &'a [u8]) -> Result<ImagePlan<'a>, PlanError> {
        let machine = *r.get(4).ok_or(PlanError("truncated"))?;
        let field = |i: usize| u32_at(r, 8 + i * 4);
        let (old_timestamp, old_size_of_image) = (field(0)?, field(1)?);
        let path_len = field(2)? as usize;
        let blob_len = field(3)? as usize;
        let data_off = field(4)?;
        let fixups = field(5)? as usize * FIXUP;
        let entries = field(6)? as usize * ENTRY;
        let strings = field(7)? as usize;
        let file_len = field(8)? as usize;
        let mut at: usize = 8 + 9 * 4;
        let mut take = |n: usize| -> Result<&'a [u8], PlanError> {
            let s = r.get(at..at.checked_add(n).ok_or(PlanError("sizes overflow"))?);
            at += n;
            s.ok_or(PlanError("an image record is shorter than its sizes"))
        };
        let path = core::str::from_utf8(take(path_len)?).map_err(|_| PlanError("path"))?;
        let plan = ImagePlan {
            machine,
            old_timestamp,
            old_size_of_image,
            path,
            blob: take(blob_len)?,
            data_off,
            fixups: take(fixups)?,
            entries: take(entries)?,
            strings: take(strings)?,
            file: take(file_len)?,
        };
        if at != r.len() {
            return Err(PlanError("an image record is longer than its sizes"));
        }
        if machine != 1 && machine != 2 {
            return Err(PlanError("a machine other than x64 or x86"));
        }
        if data_off as usize > blob_len.next_multiple_of(4096) || data_off % 4096 != 0 {
            return Err(PlanError("the blob's data offset"));
        }
        for e in plan.entries() {
            if e.size < JUMP as u32 || e.at as usize >= blob_len || e.rva >= old_size_of_image {
                return Err(PlanError("an entry outside the blob or the image"));
            }
        }
        Ok(plan)
    }

    pub fn fixups(&self) -> impl Iterator<Item = Fixup> + '_ {
        self.fixups.chunks_exact(FIXUP).map(|f| {
            let u = |i: usize| u32::from_le_bytes([f[i], f[i + 1], f[i + 2], f[i + 3]]);
            let anchor = u(4);
            Fixup {
                at: u(0),
                anchor: (anchor != u32::MAX).then_some(anchor),
                target: i64::from_le_bytes([f[8], f[9], f[10], f[11], f[12], f[13], f[14], f[15]]),
                width: f[16],
                space: f[17],
            }
        })
    }

    pub fn entries(&self) -> impl Iterator<Item = Entry<'a>> + 'a {
        let strings = self.strings;
        self.entries.chunks_exact(ENTRY).map(move |e| {
            let u = |i: usize| u32::from_le_bytes([e[i], e[i + 1], e[i + 2], e[i + 3]]);
            let name = strings.get(u(12) as usize..).unwrap_or(&[]);
            let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
            let name = core::str::from_utf8(&name[..end]).unwrap_or("?");
            Entry { rva: u(0), size: u(4), at: u(8), name }
        })
    }

    /// The DLL's file name (`x.dll`), lower case is not assumed.
    pub fn file_name(&self) -> &'a str {
        self.path.rsplit(['\\', '/']).next().unwrap_or(self.path)
    }

    fn is_x86(&self) -> bool {
        self.machine == 2
    }
}

/// Writes the plan's fixups into `blob` (a copy of the plan's) for an old
/// image at `image` and the blob at `at`. An Err names what does not fit
/// (a rel32 out of reach, a 32-bit absolute address above 4 GiB).
pub fn bind(blob: &mut [u8], plan: &ImagePlan, image: u64, at: u64) -> Result<(), &'static str> {
    for f in plan.fixups() {
        let base = if f.space == 0 { image } else { at };
        let target = base.wrapping_add(f.target as u64);
        let value: u64 = match f.anchor {
            Some(anchor) => {
                let rel = target.wrapping_sub(at + u64::from(anchor)) as i64;
                if f.width != 4 || i32::try_from(rel).is_err() {
                    return Err("a relative reference does not reach (rel32)");
                }
                rel as u64
            }
            None => target,
        };
        let start = f.at as usize;
        let field =
            blob.get_mut(start..start + usize::from(f.width)).ok_or("a fixup outside the blob")?;
        match f.width {
            8 => field.copy_from_slice(&value.to_le_bytes()),
            4 => {
                if f.anchor.is_none() && value > u64::from(u32::MAX) {
                    return Err("a 32-bit absolute address above 4 GiB");
                }
                field.copy_from_slice(&(value as u32).to_le_bytes());
            }
            _ => return Err("a fixup width other than 4 or 8"),
        }
    }
    Ok(())
}

/// `jmp rel32` at `from` to `to`, if it reaches.
pub fn jump(from: u64, to: u64) -> Option<[u8; JUMP]> {
    let rel = i32::try_from(to.wrapping_sub(from + JUMP as u64) as i64).ok()?;
    let r = rel.to_le_bytes();
    Some([0xe9, r[0], r[1], r[2], r[3]])
}

/// Which of `ranges` (start, end) holds `value` past its start: a return
/// address into a function, or an instruction pointer inside one (at its
/// very first byte a thread only goes through the new jump).
pub fn inside(ranges: &[(u64, u64)], value: u64) -> Option<usize> {
    ranges.iter().position(|&(start, end)| value > start && value < end)
}

// --- The request (from the NMI) ---

/// A plan not applied yet was found (the NMI); `run` takes it.
static REQUESTED: AtomicBool = AtomicBool::new(false);
/// The nonce of the last plan acted on (applied or refused): it is not
/// taken again (a later state dump NMI finds the same fw_cfg file).
static DONE_NONCE: AtomicU64 = AtomicU64::new(0);
/// The plan's fw_cfg key and size, from the NMI.
static PLAN_KEY: AtomicU32 = AtomicU32::new(0);
static PLAN_SIZE: AtomicU32 = AtomicU32::new(0);
/// 1 while a CPU reads fw_cfg (one reader at a time).
static READING: AtomicU8 = AtomicU8::new(0);
/// Set once a plan was applied: crash reports and state dumps say so.
static SWAPPED: AtomicBool = AtomicBool::new(false);
/// The DLLs swapped (`a.dll,b.dll`), for those lines.
static SWAPPED_NAMES: crate::sync::IrqMutex<([u8; 120], usize)> =
    crate::sync::IrqMutex::new(([0; 120], 0));

fn take_fw_cfg() -> bool {
    READING.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_ok()
}

fn give_fw_cfg() {
    READING.store(0, Ordering::Release);
}

/// From the NMI handler, before the state dump: whether this NMI asks for
/// a hot swap (then it is no dump request). Port I/O and atomics only.
pub fn on_nmi(from_user_mode: bool) -> bool {
    // Each CPU looks once, one at a time (fw_cfg has one read position);
    // a CPU that finds the request already made by another looks no more.
    let mut spins = 0u32;
    while !REQUESTED.load(Ordering::Acquire) && spins < 50_000_000 {
        if take_fw_cfg() {
            if !REQUESTED.load(Ordering::Acquire)
                && let Some((key, size)) = crate::arch::fwcfg::find(FW_CFG_NAME)
                && size as usize >= HEADER
            {
                let mut header = [0u8; HEADER];
                crate::arch::fwcfg::read(key, &mut header);
                if let Some(nonce) = header_nonce(&header)
                    && nonce != DONE_NONCE.load(Ordering::Acquire)
                {
                    PLAN_KEY.store(u32::from(key), Ordering::Relaxed);
                    PLAN_SIZE.store(size, Ordering::Relaxed);
                    REQUESTED.store(true, Ordering::Release);
                }
            }
            give_fw_cfg();
            break;
        }
        spins += 1;
        core::hint::spin_loop();
    }
    if !REQUESTED.load(Ordering::Acquire) {
        return false;
    }
    thread::hold_from_nmi();
    if from_user_mode {
        // Back in user mode the pending interrupt comes first: its return
        // path holds the thread before its next instruction.
        crate::arch::smp::send_reschedule(crate::arch::percpu::cpu_index());
    }
    true
}

/// Whether a hot swap waits to be run (the boot thread's wait asks).
pub fn requested() -> bool {
    REQUESTED.load(Ordering::Acquire)
}

/// Whether this run was hot-swapped (crash reports, state dumps).
pub fn swapped() -> bool {
    SWAPPED.load(Ordering::Acquire)
}

/// `hot-swapped (a.dll): not a real run` for crash reports and dumps;
/// nothing when it was not.
pub struct Mark;

impl core::fmt::Display for Mark {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if !swapped() {
            return Ok(());
        }
        let names = SWAPPED_NAMES.lock();
        let s = core::str::from_utf8(&names.0[..names.1]).unwrap_or("?");
        write!(f, "this run was hot-swapped ({s}): not a real run")
    }
}

fn note_swapped(name: &str) {
    let mut names = SWAPPED_NAMES.lock();
    let (buf, len) = &mut *names;
    let sep = if *len > 0 { 1 } else { 0 };
    if *len + sep + name.len() <= buf.len() {
        if sep == 1 {
            buf[*len] = b',';
        }
        buf[*len + sep..*len + sep + name.len()].copy_from_slice(name.as_bytes());
        *len += sep + name.len();
    }
    SWAPPED.store(true, Ordering::Release);
}

/// Runs the hot swap the NMI found (from the boot thread's wait). The
/// programs are let go at the end, whatever happened.
pub fn run() {
    if !REQUESTED.load(Ordering::Acquire) {
        return;
    }
    let size = PLAN_SIZE.load(Ordering::Relaxed) as usize;
    let key = PLAN_KEY.load(Ordering::Relaxed) as u16;
    let mut bytes = alloc::vec::Vec::new();
    let read = bytes.try_reserve_exact(size).is_ok() && {
        bytes.resize(size, 0);
        let mut spins = 0u64;
        while !take_fw_cfg() && spins < 1_000_000_000 {
            spins += 1;
            core::hint::spin_loop();
        }
        crate::arch::fwcfg::read(key, &mut bytes);
        give_fw_cfg();
        true
    };
    // Every thread already stops on its way to user mode (the NMI); this
    // also interrupts the ones that run, from a context that may.
    thread::hold_for_handover(true);
    let nonce = header_nonce(&bytes).unwrap_or(0);
    let result = if read {
        match Plan::parse(&bytes) {
            Ok(plan) => apply_plan(&plan),
            Err(e) => {
                info!(
                    "hotswap: refused: the plan ({size} bytes from fw_cfg) is unreadable: {}",
                    e.0
                );
                Err(Refused)
            }
        }
    } else {
        info!("hotswap: refused: no memory for the plan ({size} bytes)");
        Err(Refused)
    };
    crate::sched::freeze_users(false);
    thread::hold_for_handover(false);
    DONE_NONCE.store(nonce, Ordering::Release);
    REQUESTED.store(false, Ordering::Release);
    if result.is_err() {
        crate::events::emit_always(format_args!(r#""ev":"hotswap","result":"refused""#));
        info!("hotswap: the programs go on unpatched (the run is still not a real run)");
    }
}

/// A refusal (its reason is in the log: `hotswap: refused: ...`).
struct Refused;

/// A thread's user state while the programs are stopped.
struct Stopped {
    process: KArc<Process>,
    tid: u64,
    teb: u64,
    teb32: u64,
    /// Its user registers (where it will go on), and whether they are a
    /// 32-bit thread's (WOW64, stopped in 32-bit code).
    regs: Option<(platform::UserRegs, bool)>,
}

/// Waits until no program thread runs user code; the threads as stopped.
fn wait_quiet() -> Result<alloc::vec::Vec<Stopped>, Refused> {
    let until = crate::time::SpinDeadline::after_ns(QUIET_MS * 1_000_000);
    loop {
        match stopped_threads() {
            Ok(Some(_)) => {
                crate::sched::freeze_users(true);
                if crate::sched::users_quiet()
                    && let Ok(Some(t)) = stopped_threads()
                {
                    return Ok(t);
                }
                crate::sched::freeze_users(false);
            }
            Ok(None) if until.passed() => {
                info!(
                    "hotswap: refused: a program thread still ran user code or a system call \
                     after {QUIET_MS} ms"
                );
                return Err(Refused);
            }
            Ok(None) => {}
            Err(()) => {
                info!("hotswap: refused: no memory to list the threads");
                return Err(Refused);
            }
        }
        crate::sched::sleep_ms(2);
    }
}

/// Every live thread's user state, or None while one runs user code.
fn stopped_threads() -> Result<Option<alloc::vec::Vec<Stopped>>, ()> {
    let mut out = alloc::vec::Vec::new();
    for object in thread::all().map_err(drop)? {
        let Some(snap) = thread::snapshot(&object) else { continue };
        if snap.terminating || snap.exit_status.is_some() {
            continue;
        }
        let regs = if let Some(h) = &snap.held {
            Some((h.regs, h.compat))
        } else if let (Some(p), true) = (&snap.parked, snap.suspended > 0) {
            Some((p.regs, p.compat))
        } else if let Some((_, regs)) = snap.in_syscall {
            // Blocked in it (then it holds no lock the swap may need when
            // the scheduler keeps it off the CPUs, as a handover's
            // threads), or it finishes and stops on its way back.
            let blocked = snap.sched.is_some_and(|id| {
                matches!(crate::sched::user_wait(id).0, crate::sched::UserWait::Waiting(_))
            });
            if !blocked {
                return Ok(None);
            }
            Some((regs, false))
        } else if !snap.entered {
            Some((snap.start, false))
        } else if snap.sched.is_none() {
            None
        } else {
            return Ok(None);
        };
        out.try_push(Stopped {
            process: snap.process.clone(),
            tid: snap.id,
            teb: snap.teb,
            teb32: snap.teb32,
            regs,
        })
        .map_err(drop)?;
    }
    Ok(Some(out))
}

/// A process that maps the old image, and where.
struct Target {
    process: KArc<Process>,
    base: u64,
}

/// What was done in one process (undone if a later step fails).
struct Done {
    process: KArc<Process>,
    blob: u64,
    /// (address, the bytes it had).
    entries: alloc::vec::Vec<(u64, [u8; JUMP])>,
}

fn undo(done: &[Done]) {
    for d in done.iter().rev() {
        for (va, old) in d.entries.iter().rev() {
            if write_code(&d.process, *va, old).is_err() {
                warn!("hotswap: could not restore {va:#x} in process {}", d.process.id);
            }
        }
        let _ = vm::free(&d.process, d.blob, 0, vm::MEM_RELEASE);
    }
}

/// Writes `bytes` over code at `va` (copy-on-write: the image's shared
/// pages are left as they are).
fn write_code(process: &Process, va: u64, bytes: &[u8]) -> Result<(), &'static str> {
    let (old, _, _) = vm::protect(process, va, bytes.len() as u64, vad::PAGE_EXECUTE_READWRITE)
        .map_err(|_| "its page cannot be made writable")?;
    let written = process.write(va, bytes).map_err(|_| "the write faulted");
    let _ = vm::protect(process, va, bytes.len() as u64, old);
    written
}

fn apply_plan(plan: &Plan) -> Result<(), Refused> {
    info!(
        "hotswap: NOT A REAL RUN: a hot swap of {} DLL(s) (plan nonce {:#x}); results are at the \
         user's own risk",
        plan.images.len(),
        plan.nonce
    );
    let threads = wait_quiet()?;
    info!("hotswap: {} threads stopped (none runs user code)", threads.len());
    // Every DLL's processes and stack check before anything changes.
    let mut checked = alloc::vec::Vec::new();
    for image in &plan.images {
        let targets = targets(image)?;
        info!(
            "hotswap: {} ({}): {} changed function(s), blob {} bytes, {} fixups; mapped in {} \
             process(es)",
            image.file_name(),
            if image.is_x86() { "x86" } else { "x64" },
            image.entries().count(),
            image.blob.len(),
            image.fixups().count(),
            targets.len()
        );
        for t in &targets {
            stack_check(image, t, &threads)?;
        }
        checked.try_push((image, targets)).map_err(|_| Refused)?;
    }
    // All of them or none: a DLL that fails undoes the ones before it.
    let mut applied: alloc::vec::Vec<Applied> = alloc::vec::Vec::new();
    for (image, targets) in &checked {
        match apply_image(image, targets) {
            Ok(a) => {
                if applied.try_push(a).is_err() {
                    info!("hotswap: refused: out of memory; undoing what was done");
                    applied.iter().rev().for_each(Applied::undo);
                    return Err(Refused);
                }
            }
            Err(r) => {
                if !applied.is_empty() {
                    info!("hotswap: undoing the {} DLL(s) swapped before it", applied.len());
                }
                applied.iter().rev().for_each(Applied::undo);
                return Err(r);
            }
        }
    }
    for a in &applied {
        a.report();
    }
    Ok(())
}

/// One DLL swapped in its processes (undone if a later one fails).
struct Applied<'a> {
    image: &'a ImagePlan<'a>,
    done: alloc::vec::Vec<Done>,
    old_file: Option<alloc::vec::Vec<u8>>,
}

impl Applied<'_> {
    fn undo(&self) {
        undo(&self.done);
        restore_file(self.image, self.old_file.as_deref());
    }

    /// The lines, ledger entry and event of a swap that stays.
    fn report(&self) {
        let image = self.image;
        let name = image.file_name();
        let entries = image.entries().count();
        for e in image.entries() {
            info!(
                "hotswap: {name}: patched {} (rva {:#x}, {} bytes) -> blob+{:#x}",
                e.name, e.rva, e.size, e.at
            );
        }
        note_swapped(name);
        crate::nt::ledger::record(
            "hotswap",
            "DLL hot-swapped into running programs (not a real run)",
            entries as u64,
        );
        crate::events::emit_always(format_args!(
            r#""ev":"hotswap","result":"applied","dll":{},"processes":{},"patched":{entries},"blob":{}"#,
            crate::events::Str(name),
            self.done.len(),
            image.blob.len()
        ));
        info!(
            "hotswap: applied: {name} in {} process(es), {entries} function(s) patched; NOT A REAL \
             RUN",
            self.done.len()
        );
    }
}

/// The processes that map `image`'s old build (refusing a mapping of the
/// same name with other headers).
fn targets(image: &ImagePlan) -> Result<alloc::vec::Vec<Target>, Refused> {
    let mut out = alloc::vec::Vec::new();
    let Ok(processes) = process::running() else {
        info!("hotswap: refused: no memory to list the processes");
        return Err(Refused);
    };
    let name = image.file_name();
    for p in processes {
        let mut bases: [u64; 4] = [0; 4];
        let mut n = 0;
        for v in p.views.lock().iter() {
            let s = v.section();
            if s.image.as_ref().is_none_or(|l| l.info.x86 != image.is_x86()) {
                continue;
            }
            let Some(path) = s.file_name.as_deref() else { continue };
            let last = path.rsplit(|&c| c == u16::from(b'\\') || c == u16::from(b'/')).next();
            let last = last.unwrap_or(path);
            let same = last.len() == name.len()
                && last
                    .iter()
                    .zip(name.bytes())
                    .all(|(&a, b)| u8::try_from(a).is_ok_and(|a| a.eq_ignore_ascii_case(&b)));
            if same && n < bases.len() {
                bases[n] = v.base;
                n += 1;
            }
        }
        for &base in &bases[..n] {
            let mut h = [0u8; 0x60];
            if p.read(base, &mut h[..0x40]).is_err() {
                continue;
            }
            let nt = u32::from_le_bytes([h[0x3c], h[0x3d], h[0x3e], h[0x3f]]) as u64;
            if p.read(base + nt, &mut h).is_err() || &h[..4] != b"PE\0\0" {
                continue;
            }
            let stamp = u32::from_le_bytes([h[8], h[9], h[10], h[11]]);
            let size = u32::from_le_bytes([h[24 + 56], h[24 + 57], h[24 + 58], h[24 + 59]]);
            if (stamp, size) != (image.old_timestamp, image.old_size_of_image) {
                info!(
                    "hotswap: refused: {name} in process {} ({}) at {base:#x} is not the \
                     snapshot's build (TimeDateStamp {stamp:#x}, SizeOfImage {size:#x}; the plan \
                     is for {:#x}, {:#x})",
                    p.id, p.name, image.old_timestamp, image.old_size_of_image
                );
                return Err(Refused);
            }
            out.try_push(Target { process: p.clone(), base }).map_err(|_| Refused)?;
        }
    }
    Ok(out)
}

/// The stack check: no thread of `t`'s process executes a changed
/// function or has a return address into one.
fn stack_check(image: &ImagePlan, t: &Target, threads: &[Stopped]) -> Result<(), Refused> {
    let mut ranges = alloc::vec::Vec::new();
    for e in image.entries() {
        let start = t.base + u64::from(e.rva);
        ranges.try_push((start, start + u64::from(e.size))).map_err(|_| Refused)?;
    }
    let name = image.file_name();
    let p = &t.process;
    let refuse = |tid: u64, what: core::fmt::Arguments, value: u64, i: usize| {
        let e = image.entries().nth(i);
        info!(
            "hotswap: refused: {name}!{} (changed) is on the stack of thread {tid} of process {} \
             ({}): {what} {value:#x} (+{:#x} into it); a changed function a thread is in cannot be \
             swapped (take the snapshot before it runs, or after it returns)",
            e.map_or("?", |e| e.name),
            p.id,
            p.name,
            value - ranges[i].0
        );
        Refused
    };
    for th in threads.iter().filter(|th| KArc::ptr_eq(&th.process, p)) {
        let Some((regs, compat)) = th.regs else { continue };
        // The stack the DLL's code runs on: the 32-bit one for a 32-bit DLL.
        let (ip, sp, word) = if image.is_x86() {
            if compat {
                (regs.ip(), regs.sp(), 4)
            } else if th.teb32 != 0 {
                match crate::nt::wow64::saved_registers(p, th.teb) {
                    Some(r) => (u64::from(r[0]), u64::from(r[1]), 4),
                    None => continue,
                }
            } else {
                continue;
            }
        } else if compat {
            continue;
        } else {
            (regs.ip(), regs.sp(), 8)
        };
        if let Some(i) = inside(&ranges, ip) {
            return Err(refuse(th.tid, format_args!("it executes at"), ip, i));
        }
        let end = match p.vad.lock().query(sp) {
            Ok(r) if r.state == vad::MEM_COMMIT => (r.base + r.size).min(sp + STACK_LIMIT),
            _ => continue,
        };
        let mut buf = [0u8; 4096];
        let mut at = sp & !(word - 1);
        while at < end {
            let n = ((end - at) as usize).min(buf.len());
            if p.read(at, &mut buf[..n]).is_err() {
                break;
            }
            for (k, w) in buf[..n].chunks_exact(word as usize).enumerate() {
                let v = if word == 8 {
                    u64::from_le_bytes([w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]])
                } else {
                    u64::from(u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
                };
                if let Some(i) = inside(&ranges, v) {
                    // A return address, or a stale value the conservative
                    // scan cannot tell from one.
                    let slot = at + (k as u64) * word;
                    let what = format_args!("its stack at {slot:#x} (stack pointer {sp:#x}) holds");
                    return Err(refuse(th.tid, what, v, i));
                }
            }
            at += n as u64;
        }
    }
    Ok(())
}

/// Maps the blob in `t`'s process near the image and binds it; its
/// address.
fn map_blob(image: &ImagePlan, t: &Target) -> Result<u64, &'static str> {
    let size = (image.blob.len() as u64).next_multiple_of(4096).max(4096);
    let p = &t.process;
    let image_end = t.base + u64::from(image.old_size_of_image);
    // x64: within rel32 reach of every byte of the image (the blob's code
    // reads the old image's statics rip-relative), above it first. x86:
    // anywhere below 2 GiB.
    let at = if image.is_x86() {
        // Reservations start at 64 KiB boundaries (the allocation
        // granularity): room for one inside the gap found.
        let gap = p.vad.lock().find_free_below(size + 0xffff, 0x8000_0000);
        gap.map(|g| (g + 0xffff) & !0xffff).ok_or("no room below 2 GiB")?
    } else {
        let reach = 0x7000_0000u64;
        let mut found = None;
        let vad = p.vad.lock();
        let mut a = image_end.next_multiple_of(0x1_0000);
        while a + size <= t.base + reach {
            if vad.is_range_free(a, a + size) {
                found = Some(a);
                break;
            }
            a += 0x1_0000;
        }
        if found.is_none() {
            let mut a = t.base.saturating_sub(size) & !0xffff;
            while a + reach > image_end && a >= 0x1_0000 {
                if vad.is_range_free(a, a + size) {
                    found = Some(a);
                    break;
                }
                a -= 0x1_0000;
            }
        }
        found.ok_or("no free range within 2 GiB of the image")?
    };
    let (at, _) =
        vm::allocate(p, at, size, vad::MEM_RESERVE | vad::MEM_COMMIT, vad::PAGE_READWRITE)
            .map_err(|e| {
                info!(
                    "hotswap:   allocating {size:#x} bytes at {at:#x} in process {}: {e:?}",
                    p.id
                );
                "its memory could not be allocated"
            })?;
    let mut blob = alloc::vec::Vec::new();
    let bound = blob
        .try_reserve_exact(image.blob.len())
        .map_err(|_| "no memory for the blob")
        .and_then(|()| {
            blob.try_extend_from_slice(image.blob).map_err(|_| "no memory for the blob")?;
            bind(&mut blob, image, t.base, at)
        })
        .and_then(|()| p.write(at, &blob).map_err(|_| "writing it faulted"))
        .and_then(|()| {
            let code = u64::from(image.data_off).min(size);
            if code > 0 {
                vm::protect(p, at, code, vad::PAGE_EXECUTE_READ)
                    .map(drop)
                    .map_err(|_| "its code could not be made executable")
            } else {
                Ok(())
            }
        });
    match bound {
        Ok(()) => Ok(at),
        Err(e) => {
            let _ = vm::free(p, at, 0, vm::MEM_RELEASE);
            Err(e)
        }
    }
}

/// Swaps `image` into the processes `targets` (stack-checked), or undoes
/// what it did and refuses.
fn apply_image<'a>(image: &'a ImagePlan<'a>, targets: &[Target]) -> Result<Applied<'a>, Refused> {
    let name = image.file_name();
    let entries = image.entries().count();
    // The file first: later loads get the new build whole.
    let old_file = replace_file(image)?;
    let mut done: alloc::vec::Vec<Done> = alloc::vec::Vec::new();
    for t in targets {
        let result = map_blob(image, t).and_then(|blob| {
            let mut d = Done { process: t.process.clone(), blob, entries: alloc::vec::Vec::new() };
            let patched = (|| {
                for e in image.entries() {
                    let va = t.base + u64::from(e.rva);
                    let to = blob + u64::from(e.at);
                    let bytes = jump(va, to).ok_or("a jump does not reach its copy")?;
                    let mut old = [0u8; JUMP];
                    t.process.read(va, &mut old).map_err(|_| "an entry is unreadable")?;
                    write_code(&t.process, va, &bytes)?;
                    d.entries.try_push((va, old)).map_err(|_| "out of memory")?;
                }
                Ok(())
            })();
            match patched {
                Ok(()) => {
                    done.try_push(d).map_err(|_| "out of memory")?;
                    Ok(blob)
                }
                Err(e) => {
                    undo(core::slice::from_ref(&d));
                    Err(e)
                }
            }
        });
        match result {
            Ok(blob) => info!(
                "hotswap: {name}: process {} ({}): image at {:#x}, blob at {blob:#x}, {entries} \
                 entries patched",
                t.process.id, t.process.name, t.base
            ),
            Err(e) => {
                info!(
                    "hotswap: refused: {name} in process {} ({}): {e}; undoing what was done",
                    t.process.id, t.process.name
                );
                undo(&done);
                restore_file(image, old_file.as_deref());
                return Err(Refused);
            }
        }
    }
    Ok(Applied { image, done, old_file })
}

/// Replaces the DLL's file by the new build; its old contents (None when
/// there was no such file: nothing to put back).
fn replace_file(image: &ImagePlan) -> Result<Option<alloc::vec::Vec<u8>>, Refused> {
    let Ok(node) = file::lookup(image.path) else {
        info!(
            "hotswap: {}: no file at {} (later loads are not changed)",
            image.file_name(),
            image.path
        );
        return Ok(None);
    };
    let Ok(old) = file::read_all(node) else {
        info!("hotswap: refused: {}: its file could not be read", image.path);
        return Err(Refused);
    };
    let now = platform::system_time();
    let written = file::set_size(node, 0, now)
        .and_then(|()| file::write(node, 0, image.file, now))
        .map(|n| n == image.file.len());
    if !matches!(written, Ok(true)) {
        info!("hotswap: refused: {}: the new build could not be written ({written:?})", image.path);
        let _ = file::set_size(node, 0, now).and_then(|()| file::write(node, 0, &old, now));
        return Err(Refused);
    }
    info!(
        "hotswap: {}: the file is the new build now ({} bytes, was {}): later loads get it whole",
        image.path,
        image.file.len(),
        old.len()
    );
    Ok(Some(old))
}

fn restore_file(image: &ImagePlan, old: Option<&[u8]>) {
    let (Some(old), Ok(node)) = (old, file::lookup(image.path)) else { return };
    let now = platform::system_time();
    if file::set_size(node, 0, now).and_then(|()| file::write(node, 0, old, now)).is_err() {
        warn!("hotswap: {}: the old build could not be put back", image.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    /// A plan as xtask's `hotswap::encode` writes it.
    fn plan_bytes(
        blob: &[u8],
        fixups: &[(u32, u32, i64, u8, u8)],
        entries: &[(u32, u32, u32)],
    ) -> Vec<u8> {
        let mut strings = Vec::new();
        let mut e = Vec::new();
        for (i, &(rva, size, at)) in entries.iter().enumerate() {
            let off = strings.len() as u32;
            strings.extend_from_slice(format!("f{i}").as_bytes());
            strings.push(0);
            for v in [rva, size, at, off] {
                e.extend_from_slice(&v.to_le_bytes());
            }
        }
        let mut f = Vec::new();
        for &(at, anchor, target, width, space) in fixups {
            f.extend_from_slice(&at.to_le_bytes());
            f.extend_from_slice(&anchor.to_le_bytes());
            f.extend_from_slice(&target.to_le_bytes());
            f.push(width);
            f.push(space);
            f.extend_from_slice(&[0; 6]);
        }
        let path = b"\\Windows\\System32\\x.dll";
        let mut rec = vec![1u8, 0, 0, 0];
        for v in [7u32, 0x4000, path.len() as u32, blob.len() as u32, 0x1000, fixups.len() as u32]
            .into_iter()
            .chain([entries.len() as u32, strings.len() as u32, 2])
        {
            rec.extend_from_slice(&v.to_le_bytes());
        }
        rec.extend_from_slice(path);
        rec.extend_from_slice(blob);
        rec.extend_from_slice(&f);
        rec.extend_from_slice(&e);
        rec.extend_from_slice(&strings);
        rec.extend_from_slice(b"MZ");
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&0x55u64.to_le_bytes());
        out.extend_from_slice(&[0; 8]);
        out.extend_from_slice(&((rec.len() + 4) as u32).to_le_bytes());
        out.extend_from_slice(&rec);
        let len = out.len() as u32;
        out[24..28].copy_from_slice(&len.to_le_bytes());
        let crc = crate::fs::log::crc32(0, &out);
        out[28..32].copy_from_slice(&crc.to_le_bytes());
        out
    }

    #[test]
    fn a_plan_reads_back_and_is_checked() {
        let bytes =
            plan_bytes(&[0x8b, 0x05, 0, 0, 0, 0, 0xc3], &[(2, 6, 0x3000, 4, 0)], &[(0x1010, 7, 0)]);
        let plan = Plan::parse(&bytes).unwrap();
        assert_eq!(plan.nonce, 0x55);
        assert_eq!(header_nonce(&bytes[..32]), Some(0x55));
        let image = &plan.images[0];
        assert_eq!(image.file_name(), "x.dll");
        assert_eq!(image.file, b"MZ");
        assert_eq!(
            image.fixups().collect::<Vec<_>>(),
            vec![Fixup { at: 2, anchor: Some(6), target: 0x3000, width: 4, space: 0 }]
        );
        assert_eq!(
            image.entries().collect::<Vec<_>>(),
            vec![Entry { rva: 0x1010, size: 7, at: 0, name: "f0" }]
        );
        // A flipped byte, a short read, another format: refused.
        let mut bad = bytes.clone();
        bad[40] ^= 1;
        assert_eq!(Plan::parse(&bad).err(), Some(PlanError("its CRC-32 does not match")));
        assert!(Plan::parse(&bytes[..bytes.len() - 1]).is_err());
        let mut v2 = bytes.clone();
        v2[8] = 2;
        assert!(Plan::parse(&v2).is_err());
        assert_eq!(header_nonce(&v2[..32]), None);
    }

    #[test]
    fn fixups_bind_to_the_old_image_and_the_blob() {
        let bytes = plan_bytes(
            &[0; 24],
            &[(2, 6, 0x3000, 4, 0), (8, u32::MAX, 0x10, 8, 1), (16, 0, 0x20, 4, 1)],
            &[(0x1010, 7, 0)],
        );
        let plan = Plan::parse(&bytes).unwrap();
        let image = 0x1_8000_0000u64;
        let at = 0x1_8001_0000u64;
        let mut blob = plan.images[0].blob.to_vec();
        bind(&mut blob, &plan.images[0], image, at).unwrap();
        // rip-relative to the old image's static.
        let rel = i32::from_le_bytes(blob[2..6].try_into().unwrap());
        assert_eq!(at as i64 + 6 + i64::from(rel), (image + 0x3000) as i64);
        // An absolute pointer into the blob.
        assert_eq!(u64::from_le_bytes(blob[8..16].try_into().unwrap()), at + 0x10);
        // A jump table entry relative to its table (offset 0).
        assert_eq!(i32::from_le_bytes(blob[16..20].try_into().unwrap()), 0x20);
        // Out of reach: refused, not truncated.
        let far = 0x7000_0000_0000u64;
        assert!(bind(&mut blob, &plan.images[0], image, far).is_err());
    }

    #[test]
    fn jumps_and_the_stack_check() {
        assert_eq!(jump(0x1000, 0x1000 + 5 + 0x10), Some([0xe9, 0x10, 0, 0, 0]));
        assert_eq!(jump(0x2000, 0x1000), Some([0xe9, 0xfb, 0xef, 0xff, 0xff]));
        assert_eq!(jump(0, 0x1_0000_0000), None);
        let ranges = [(0x1000, 0x1040), (0x2000, 0x2010)];
        // At the entry: it only goes through the new jump.
        assert_eq!(inside(&ranges, 0x1000), None);
        assert_eq!(inside(&ranges, 0x1001), Some(0));
        assert_eq!(inside(&ranges, 0x203f), None);
        assert_eq!(inside(&ranges, 0x200f), Some(1));
        assert_eq!(inside(&ranges, 0x2010), None);
    }
}
