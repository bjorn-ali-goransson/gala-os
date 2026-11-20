//! `imports=<program>[!<dll>][:<n>]`: the first call a program makes
//! through each of its import slots (docs/OBSERVABILITY.md, "Import
//! tracing").
//!
//! A call into a DLL goes through the importing module's import address
//! table -- one indirect call in the program's own address space, which
//! the kernel never sees. `trace_from=` sees the ones into DLLs gala
//! wrote, because gala's DLLs record them; nothing sees a call into a
//! DLL Microsoft wrote, and nothing sees the call a program *does not*
//! make. That is the shape of the question Word's silent window asks:
//! after the last call that worked, what did it reach?
//!
//! The mechanism is the loader's import binding plus the kernel's page
//! fault path, and no generated code at all. For a process `imports=`
//! names, ntdll reports each import it has just bound (its slot, the
//! export's address, the name the ledger would use) and reserves a
//! region of address space it never commits; the kernel writes an
//! address in that region into the slot instead of the export's. The
//! first call through the slot therefore fetches an instruction from a
//! page that is not there: the fault arrives here with the call's
//! arguments still in their registers and its return address already on
//! the stack. This records the call, puts the export's address in the
//! slot, and points the instruction pointer at it -- so the call goes
//! on, answers what it always would, and every later call through that
//! slot goes straight to the export with nothing in the way at all.
//!
//! A slot is not always called, though: a program may read an import's
//! value and use it as a pointer, which a hook installer does to read
//! the bytes it is about to patch. That read faults at the trap address
//! and no instruction fetch is in sight, and the bytes it wants are the
//! export's, which no page here can hold. So such a fault is answered
//! as the state the program would have been in with nothing armed: the
//! export's address goes back in the slot and into every register
//! holding the stale trap address, and the instruction is taken again.
//! The slot is recorded as read rather than called -- it was not a call
//! -- and left disarmed, since a reader will read it again.
//!
//! Putting the export's address in the slot is what makes the next call
//! free, and nothing else needs it: the instruction pointer above is
//! what sends the call on. So `imports_repeat=<n>` simply does not write
//! the slot, and every call through it faults, is recorded and goes on
//! as the first did -- which turns the stream from a set of names into
//! the sequence of calls a program made, for `n` calls past each slot's
//! first. The budget is what bounds the cost of that, since a slot left
//! armed pays a fault a call for as long as it is.
//!
//! The answer is the other half of a call, and the slot has room for
//! it: the loader reserves [`STRIDE`] bytes of trap space per slot and
//! the call trap takes the first eight, so the next eight are a second
//! trap address of that same slot's. Under `imports_answers=<n>` the
//! kernel takes the return address the call left -- on the stack on
//! x86-64, in the link register on ARM64 -- keeps it under the thread
//! that made the call, and puts that second address there instead. The
//! callee therefore returns into the reserved region, which faults with
//! the answer still in the register the ABI returns it in: the kernel
//! records the answer and sends the return on to where the call really
//! came from. Which call a return belongs to is the stack pointer it
//! arrives with, so a call that never returns -- an unwind, a longjmp,
//! a thread that exits -- is noticed rather than trusted.
//!
//! What it costs: one page fault per slot (the kernel's own fault path,
//! plus a linear scan of the slots armed and two protection changes to
//! write the slot), and nothing after it -- or, under
//! `imports_repeat=`, one such fault per call until the budget is
//! spent, and under `imports_answers=` a second fault for each call
//! answered. What it cannot do is in OBSERVABILITY: the first four
//! integer argument registers and no others, and static imports only.
//! A callee that reads its own return address sees the kernel's trap
//! while its return is trapped, which is the one thing a program can
//! tell at all.

use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use super::platform::TrapFrame;
use super::platform::{self, ThreadId};
use super::process::Process;
use super::status::NtStatus;
use super::tracecall::{Answer, Words};
use super::{vad, vm};
use crate::events::{self, Category, Str};
use crate::mm::heap::TryVec;
use crate::sync::IrqMutex;

/// Slots armed unless `imports=<...>:<n>` asks for another number. A
/// slot is a line of the event stream and an entry in [`TRAPS`], so this
/// is both a budget and a bound on the kernel memory the option takes.
const BUDGET: u32 = 4096;

/// The most `:<n>` may ask for. Each slot keeps its name and addresses
/// here ([`Trap`], 136 bytes), so this is about 9 MiB of kernel heap at
/// the far end. It is as high as that because the question the option
/// answers -- which entry points a program reaches in a window of its
/// life -- is only answerable of a program whose slots are all armed:
/// a budget spent part way through leaves a reading of the modules
/// that happened to load first, which is no set at all. Word binds
/// some 30 000 imports across its 90 images, and the ceiling is what
/// lets a program of that size be traced whole; the default stays a
/// window's worth, since most programs are not Word.
const MAX_BUDGET: u32 = 65536;

/// The most `imports_repeat=<n>` may ask for, which is `trace_from=`'s
/// own ceiling: a record here is a line of the stream and not kernel
/// memory, so what bounds it is the log it is written to and the fault
/// each call pays while its slot stays armed.
const MAX_REPEAT: u32 = 1 << 20;

/// Where in a slot's trap space its return's trap address is, the
/// call's being at the start of it. [`STRIDE`] is what the loader
/// reserves per slot, so the two are the slot's own and `addr % STRIDE`
/// says which of them a fault is at ([`region`] refuses a region that
/// is not a multiple of it, which is what makes that true).
const RET_OFFSET: u64 = 8;

/// The most `imports_answers=<n>` may ask for, [`MAX_REPEAT`]'s own
/// ceiling: an answer is a line of the stream, so what bounds it is the
/// log it is written to and the second fault each answered call pays.
const MAX_ANSWERS: u32 = 1 << 20;

/// The most calls whose returns may be waited for at one time, over
/// every thread. An entry is kernel memory held from a call until its
/// return, so this is what bounds a program whose calls do not return;
/// a call that finds it spent is recorded without an answer, which the
/// summary says.
const MAX_PENDING: usize = 1024;

/// The longest name kept, as the ledger keeps its own
/// (`tracecall::NAME_LEN`, which a `<dll>!<export>` name shares).
pub const NAME_LEN: usize = super::tracecall::NAME_LEN;

/// Bytes of the trap region one armed slot takes. A slot's trap address
/// is `base + index * STRIDE`, so the loader sizes the region it
/// reserves by this: the two halves agree on it or the arming is refused
/// outright ([`region`]), rather than pointing a slot past the region's
/// end where another allocation could one day answer the fault.
pub const STRIDE: u64 = 16;

/// What `imports=` named: the program whose imports are armed, and the
/// DLL they must come from (empty: every DLL it imports from).
struct Spec {
    program: [u8; NAME_LEN],
    program_len: usize,
    dll: [u8; NAME_LEN],
    dll_len: usize,
}

static SPEC: IrqMutex<Spec> =
    IrqMutex::new(Spec { program: [0; NAME_LEN], program_len: 0, dll: [0; NAME_LEN], dll_len: 0 });

/// Whether `imports=` was given at all, so the fault path and the system
/// call answer with one relaxed load instead of taking a lock.
static WANTED: AtomicBool = AtomicBool::new(false);

/// Slots left in the budget.
static LEFT: AtomicU32 = AtomicU32::new(0);

/// Slots armed, and first calls recorded through them.
static ARMED: AtomicU64 = AtomicU64::new(0);
static RECORDED: AtomicU64 = AtomicU64::new(0);
/// Slots whose value was read as data before any call through them, and
/// so disarmed: the program used the import, but not by calling it.
static READ: AtomicU64 = AtomicU64::new(0);

/// Later calls recorded through slots that stayed armed, and what is
/// left of what `imports_repeat=` asked for. No budget at all is the
/// one-shot behaviour, which is what no `imports_repeat=` leaves; the
/// number asked for is kept for the summary line, which says what a
/// spent budget means for what follows it.
static REPEATED: AtomicU64 = AtomicU64::new(0);
static REPEAT: AtomicU32 = AtomicU32::new(0);
static REPEAT_ASKED: AtomicU32 = AtomicU32::new(0);

/// Answers recorded, what is left of what `imports_answers=` asked for,
/// and the two ways one is not recorded: a call whose return could not
/// be trapped at all (counted here, said once in the log) and a call
/// whose return never arrived. All four are summary lines, because a
/// call in the stream with no answer after it is read as the call
/// having given none.
static ANSWERED: AtomicU64 = AtomicU64::new(0);
static ANSWERS: AtomicU32 = AtomicU32::new(0);
static ANSWERS_ASKED: AtomicU32 = AtomicU32::new(0);
static ANSWERS_LOST: AtomicU64 = AtomicU64::new(0);
static UNWOUND: AtomicU64 = AtomicU64::new(0);

/// One call whose return is trapped, from the call until the return
/// arrives: the thread that made it, the stack pointer its return will
/// arrive with, and where it really returns to. Kept in the order the
/// calls were made, so the newest entry of a thread is the innermost
/// call it is in.
#[derive(Clone, Copy)]
struct Pending {
    pid: u64,
    thread: ThreadId,
    sp: u64,
    ret: u64,
    trap: u64,
}

/// The calls whose returns are waited for, over every thread. Read at a
/// return and written at a call, both under this lock and never while
/// user memory is touched.
static PENDING: IrqMutex<Vec<Pending>> = IrqMutex::new(Vec::new());

/// One armed import slot.
#[derive(Clone, Copy)]
struct Trap {
    /// The process whose slot it is: a trap address is one process's
    /// address, and the program `imports=` names may run twice.
    pid: u64,
    /// The address written into the slot, which the first call faults at.
    trap: u64,
    /// Where it was written (the import address table entry).
    slot: u64,
    /// The export's address, which the call is sent on to.
    target: u64,
    /// `<dll>!<export>`, as the ledger spells a name.
    name: [u8; NAME_LEN],
    len: u8,
    /// Whether the first call through it has been seen. A later call
    /// through a trap address the program kept a copy of is sent on like
    /// the first, and recorded no more than once.
    reached: bool,
    /// Whether the value has been read as data, which disarms the slot:
    /// recorded once, like a call, however many copies of the value the
    /// program then dereferences.
    read: bool,
}

static TRAPS: IrqMutex<Vec<Trap>> = IrqMutex::new(Vec::new());

/// The lowest and highest trap address armed, read before the table's
/// lock is taken: an execute fault is the common way a program's own
/// code arrives (demand paging), and one that is nowhere near a trap
/// address must not pay for a lock. Empty bounds refuse everything.
static LOW: AtomicU64 = AtomicU64::new(u64::MAX);
static HIGH: AtomicU64 = AtomicU64::new(0);

/// `imports=<program>[!<dll>][:<n>]`. A value that is not one is refused
/// outright rather than cut to a default, for the reason `trace_from=`
/// refuses one: a trace that quietly armed something else reads
/// afterwards as the program not having made the calls.
pub fn set(spec: &str) {
    let Ok((name, budget)) = split_budget(spec) else {
        warn!(
            "imports: imports={spec} ends in a slot count that is not a \
             number from 1 to {MAX_BUDGET}: no import is armed"
        );
        return;
    };
    let (program, dll) = match name.split_once('!') {
        Some((program, dll)) => (program, dll),
        None => (name, ""),
    };
    if program.is_empty() || program.len() > NAME_LEN || dll.len() > NAME_LEN {
        warn!(
            "imports: imports={spec} reads <program>[!<dll>][:<n>], such as \
             WINWORD.EXE!d2d1.dll, with names of {NAME_LEN} bytes at most: \
             no import is armed"
        );
        return;
    }
    {
        let mut stored = SPEC.lock();
        stored.program[..program.len()].copy_from_slice(program.as_bytes());
        stored.program_len = program.len();
        stored.dll[..dll.len()].copy_from_slice(dll.as_bytes());
        stored.dll_len = dll.len();
    }
    LEFT.store(budget, Ordering::Relaxed);
    WANTED.store(true, Ordering::Release);
    let from = if dll.is_empty() { "every DLL" } else { dll };
    info!(
        "imports: the first call through each import of {program} from \
         {from}, {budget} at most (imports=)"
    );
}

/// `imports_repeat=<n>`: leave each armed slot armed, so that every call
/// through it faults and is recorded and not only the first, until `n`
/// later calls have been. Needs `imports=` to have armed something and
/// `events=call` for the records to be written, as the first calls do.
pub fn set_repeat(spec: &str) {
    let Some(n) = repeat_budget(spec) else {
        warn!(
            "imports: imports_repeat={spec} reads a number from 1 to \
             {MAX_REPEAT}, the later calls to record: every slot is repaired \
             on its first call instead, as without it"
        );
        return;
    };
    REPEAT.store(n, Ordering::Relaxed);
    REPEAT_ASKED.store(n, Ordering::Relaxed);
    info!(
        "imports: every call through an armed slot, {n} later calls at most \
         (imports_repeat=)"
    );
}

/// `imports_repeat=`'s value, or None for one that is not a number from
/// 1 to [`MAX_REPEAT`]. Refused outright rather than cut to a default,
/// for the reason `imports=`'s own count is: a trace that quietly
/// recorded one call a slot reads afterwards as the program not having
/// repeated the call.
fn repeat_budget(text: &str) -> Option<u32> {
    budget_of(text, MAX_REPEAT)
}

/// `imports_answers=`'s value, or None for one that is not a number
/// from 1 to [`MAX_ANSWERS`]. Refused outright for the reason the other
/// two are: a trace that quietly recorded no answer reads afterwards as
/// calls that gave none.
fn answer_budget(text: &str) -> Option<u32> {
    budget_of(text, MAX_ANSWERS)
}

/// A budget of records: a number from 1 to `max`, or None for anything
/// else at all.
fn budget_of(text: &str, max: u32) -> Option<u32> {
    text.parse::<u32>().ok().filter(|n| (1..=max).contains(n))
}

/// Takes one record from a budget (`imports_repeat=`'s, or the answers
/// of `imports_answers=`), or false once it is spent. Taken before the
/// record is made, so two CPUs recording at once cannot both spend the
/// last one.
fn take_budget(budget: &AtomicU32) -> bool {
    let mut left = budget.load(Ordering::Relaxed);
    loop {
        if left == 0 {
            return false;
        }
        match budget.compare_exchange_weak(left, left - 1, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return true,
            Err(now) => left = now,
        }
    }
}

/// `imports_answers=<n>`: record what each call through an armed slot
/// answered, `n` calls at most. Needs `imports=` to have armed
/// something and `events=call` for the records to be written, as the
/// calls themselves do -- an answer is only recorded for a call the
/// stream carries.
pub fn set_answers(spec: &str) {
    let Some(n) = answer_budget(spec) else {
        warn!(
            "imports: imports_answers={spec} reads a number from 1 to \
             {MAX_ANSWERS}, the answers to record: no return is trapped at all \
             instead, as without it"
        );
        return;
    };
    ANSWERS.store(n, Ordering::Relaxed);
    ANSWERS_ASKED.store(n, Ordering::Relaxed);
    info!(
        "imports: the answer of every call through an armed slot, {n} at most \
         (imports_answers=)"
    );
}

/// Splits `<name>[:<n>]` at the last colon. A program's name holds none,
/// so this is the whole of it.
fn split_budget(text: &str) -> Result<(&str, u32), ()> {
    match text.rsplit_once(':') {
        None => Ok((text, BUDGET)),
        Some((name, tail)) => tail
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=MAX_BUDGET).contains(n))
            .map(|n| (name, n))
            .ok_or(()),
    }
}

/// Whether `imports=` was given (one relaxed load).
pub fn wanted() -> bool {
    WANTED.load(Ordering::Acquire)
}

/// Whether `process` is the program `imports=` named (its file name, as
/// the process is named, compared without case as a file name is).
pub fn program_wanted(process: &str) -> bool {
    if !wanted() {
        return false;
    }
    let stored = SPEC.lock();
    process.as_bytes().eq_ignore_ascii_case(&stored.program[..stored.program_len])
}

/// The DLL the armed imports must come from, written into `out`; the
/// length written, or 0 for every DLL (and for a buffer too short to
/// hold the name, which would otherwise arm a DLL nobody named).
pub fn dll(out: &mut [u8]) -> usize {
    if !wanted() {
        return 0;
    }
    let stored = SPEC.lock();
    if stored.dll_len > out.len() {
        return 0;
    }
    out[..stored.dll_len].copy_from_slice(&stored.dll[..stored.dll_len]);
    stored.dll_len
}

/// Whether the region ntdll reserved can hold `count` traps and is
/// reserved and not committed, which is what makes the first call
/// through a slot fault. A region that is neither is refused and named:
/// a committed one would send the call into whatever the pages held.
pub fn region(process: &Process, base: u64, size: u64, count: u64) -> bool {
    if count > u64::from(MAX_BUDGET) {
        warn!("imports: {count} imports reported at once, more than the {MAX_BUDGET} a run arms");
        return false;
    }
    // A slot's two trap addresses are told apart by the offset within
    // its own space, which needs that space to start at a multiple of
    // its size (`imports_answers=`; a reserved region is page-aligned,
    // so this is a check and not a constraint).
    if !base.is_multiple_of(STRIDE) {
        warn!(
            "imports: the region at {base:#x} is not a multiple of {STRIDE}, so a \
             slot's call and its return could not be told apart: none is armed"
        );
        return false;
    }
    let Some(needed) = count.checked_mul(STRIDE) else {
        return false;
    };
    if size < needed {
        warn!(
            "imports: the region at {base:#x} is {size} bytes for {count} imports, \
             short of the {needed} they take ({STRIDE} each): none is armed"
        );
        return false;
    }
    let info = process.vad.lock().query(base);
    match info {
        Ok(info) if info.state == vad::MEM_RESERVE && info.size >= needed => true,
        Ok(info) => {
            warn!(
                "imports: the region at {base:#x} is state {:#x} for {} bytes, \
                 not {} bytes reserved and uncommitted: none is armed",
                info.state, info.size, needed
            );
            false
        }
        Err(status) => {
            warn!(
                "imports: the region at {base:#x} cannot be read ({:#x}): none is armed",
                status.0
            );
            false
        }
    }
}

/// Arms one import slot: `index` says where in the region ntdll reserved
/// its trap address goes. True once the slot holds that address, so that
/// the next call through it faults here.
pub fn arm(process: &Process, base: u64, index: u64, slot: u64, target: u64, name: &[u8]) -> bool {
    if !program_wanted(process.name) || slot == 0 || target == 0 {
        return false;
    }
    if take_slot().is_none() {
        return false;
    }
    let trap = trap_at(base, index);
    let len = name.len().min(NAME_LEN);
    let mut entry = Trap {
        pid: process.id,
        trap,
        slot,
        target,
        name: [0; NAME_LEN],
        len: len as u8,
        reached: false,
        read: false,
    };
    entry.name[..len].copy_from_slice(&name[..len]);
    {
        let mut traps = TRAPS.lock();
        if traps.try_push(entry).is_err() {
            warn!(
                "imports: out of memory for the slot table at {} slots armed (imports=)",
                ARMED.load(Ordering::Relaxed)
            );
            return false;
        }
    }
    // Before the slot is written, so that no fault can arrive at a trap
    // address the bounds do not cover yet -- the slot's return trap
    // included, which is the higher of its two (`imports_answers=`).
    LOW.fetch_min(trap, Ordering::Relaxed);
    HIGH.fetch_max(trap + RET_OFFSET, Ordering::Relaxed);
    if let Err(status) = put(process, slot, trap) {
        // The slot still holds the export's address, so the program is
        // unaffected; the entry stays behind as dead weight and is not
        // counted, which is why the two numbers are worth comparing.
        warn!(
            "imports: the slot at {slot:#x} of {} could not be written ({:#x}): not armed",
            Name(&entry),
            status.0
        );
        return false;
    }
    ARMED.fetch_add(1, Ordering::Relaxed);
    true
}

/// A fault at a trap address. An instruction fetch is the call the trap
/// is there for: it is recorded the first time, the export's address goes
/// back in the slot and the call is sent on. Any other access is the
/// program using the import's value as a pointer (`call` false), which
/// is answered by disarming the slot and repairing the register the value
/// is in, so that the instruction can be taken again. True when the fault
/// is over, in which case the frame is the one to return to.
pub fn resolve(process: &Process, frame: &mut TrapFrame, addr: u64, call: bool) -> bool {
    if addr < LOW.load(Ordering::Relaxed) || addr > HIGH.load(Ordering::Relaxed) {
        return false;
    }
    // Which of a slot's two trap addresses this is: the call's, at the
    // start of the slot's trap space, or the return's, [`RET_OFFSET`]
    // into it. A return arrives as an instruction fetch, as the call
    // did (`imports_answers=`).
    if call && addr % STRIDE == RET_OFFSET {
        return answered(process, frame, addr);
    }
    // The table is read under its lock and nothing else is done there:
    // writing the slot touches user memory, which no lock may be held
    // at, and the event is a line of the stream.
    let (entry, first) = {
        let mut traps = TRAPS.lock();
        let Some(entry) = traps.iter_mut().find(|t| t.trap == addr && t.pid == process.id) else {
            return false;
        };
        let first = if call { !entry.reached } else { !entry.read };
        if call {
            entry.reached = true;
        } else {
            entry.read = true;
        }
        (*entry, first)
    };
    if !call {
        return read_as_data(process, frame, &entry, addr, first);
    }
    // Whether the slot stays armed, so that the next call through it
    // faults and is recorded too (`imports_repeat=`). A first call is
    // recorded whatever happens and takes none of that budget; a later
    // one is recorded only while there is budget for it, and the budget
    // is taken whether the stream is on or not, the fault being the
    // cost either way.
    let again = if first { REPEAT.load(Ordering::Relaxed) != 0 } else { take_budget(&REPEAT) };
    let logged = (first || again) && record(process, frame, &entry, first);
    if !again && let Err(status) = put(process, entry.slot, entry.target) {
        // The call still goes on (the instruction pointer below is what
        // sends it), so this costs a fault a call from here rather than
        // the call itself; a run that reads as slow says why.
        warn!(
            "imports: the slot at {:#x} of {} could not be put back ({:#x}): \
             every call through it faults",
            entry.slot,
            Name(&entry),
            status.0
        );
    }
    // The call's answer, where `imports_answers=` asks for one and the
    // stream carries the call: an answer to a call the stream does not
    // carry would read as an answer to nothing. The budget is taken
    // here, at the call, which is where the cost is committed.
    if logged && take_budget(&ANSWERS) {
        trap_return(process, frame, &entry);
    }
    // The return address is on the stack (x86-64) or in the link
    // register (ARM64) already: the call made it, and only its first
    // instruction fetch went wrong -- or it is the slot's return trap,
    // put there just above.
    frame.set_ip(entry.target);
    true
}

/// A trap address read or written as data: the slot goes back to the
/// export's address, which is what the program's next read of it gives,
/// and the register holding the stale value is repaired so that the
/// instruction can be taken again. True when it was, so the fault is
/// over; false leaves the access violation to be delivered, the program
/// having put the value somewhere this cannot reach (an address
/// computed from it, say, rather than the value itself).
fn read_as_data(
    process: &Process,
    frame: &mut TrapFrame,
    entry: &Trap,
    addr: u64,
    first: bool,
) -> bool {
    if first {
        READ.fetch_add(1, Ordering::Relaxed);
        super::ledger::record_user_bulk(
            &entry.name[..usize::from(entry.len)],
            "read as data, not called (imports=)",
            addr,
        );
        if let Err(status) = put(process, entry.slot, entry.target) {
            warn!(
                "imports: the slot at {:#x} of {} could not be put back ({:#x}) \
                 after its value was read as data",
                entry.slot,
                Name(entry),
                status.0
            );
        }
    }
    if frame.swap_reg(addr, entry.target) {
        return true;
    }
    warn!(
        "imports: {} at {:#x} was read as data at {} {:#x} from no register of its own, \
         so the read cannot be repaired: it faults as it would without `imports=` (imports=)",
        Name(entry),
        addr,
        super::platform::IP_NAME,
        frame.ip()
    );
    false
}

/// Records one call: the same `call` the event stream carries for
/// `trace_from=`, under its own name, because a name appears here once
/// however many times the program calls it -- or every time, where
/// `imports_repeat=` kept the slot armed. A later call is the same event
/// as a first one and is not marked as one: in a stream where a name can
/// appear twice, the first of them is the first call, and in one where
/// it cannot, every call is. True when the stream carries it, which is
/// what trapping its return waits on (`imports_answers=`).
fn record(process: &Process, frame: &TrapFrame, entry: &Trap, first: bool) -> bool {
    if !events::enabled(Category::Call) || !events::syscall_pid_wanted(process.id) {
        return false;
    }
    let args = frame.arg_regs();
    events::emit(
        Category::Call,
        "import",
        format_args!(
            "{}",
            Fields {
                pid: process.id,
                process: process.name,
                call: core::str::from_utf8(&entry.name[..usize::from(entry.len)]).unwrap_or("?"),
                args: &args,
            }
        ),
    );
    if first {
        RECORDED.fetch_add(1, Ordering::Relaxed);
    } else {
        REPEATED.fetch_add(1, Ordering::Relaxed);
    }
    true
}

/// Takes the call's return address away and puts the slot's return trap
/// where the call left it, so that the return faults here as the call
/// did. The real address is kept until then under the thread that made
/// the call: a return is matched to its call by the stack pointer it
/// arrives with, which is also how an unwind past the call is noticed.
fn trap_return(process: &Process, frame: &mut TrapFrame, entry: &Trap) {
    let Some(ret) = frame.return_address(|addr| process.read_u64(addr).ok()) else {
        lost("its return address could not be read", entry);
        return;
    };
    let waited = Pending {
        pid: process.id,
        thread: platform::current(),
        sp: frame.call_return_sp(),
        ret,
        trap: entry.trap,
    };
    {
        let mut pending = PENDING.lock();
        if pending.len() >= MAX_PENDING {
            lost("no entry was left to keep its return address in", entry);
            return;
        }
        if pending.try_push(waited).is_err() {
            lost("there was no memory to keep its return address in", entry);
            return;
        }
    }
    if !frame.set_return_address(entry.trap + RET_OFFSET, |addr, value| {
        process.write_u64(addr, value).is_ok()
    }) {
        // Nothing has changed for the program, so the entry must go
        // again: no return will arrive at the trap it was waiting for.
        forget(&waited);
        lost("its return address could not be written", entry);
    }
}

/// One call recorded without its answer. Said once and counted after
/// that: the stream then holds a call with no `ret` of its own, which
/// is what a reader must be told rather than left to infer.
fn lost(why: &str, entry: &Trap) {
    if ANSWERS_LOST.fetch_add(1, Ordering::Relaxed) == 0 {
        warn!(
            "imports: the answer of {} was not recorded: {why}; the summary line \
             counts the rest (imports_answers=)",
            Name(entry)
        );
    }
}

/// Drops one entry again, for a return that will never arrive.
fn forget(waited: &Pending) {
    let mut pending = PENDING.lock();
    if let Some(i) = pending
        .iter()
        .rposition(|e| e.thread == waited.thread && e.sp == waited.sp && e.ret == waited.ret)
    {
        pending.remove(i);
    }
}

/// A fault at a slot's return trap: the return of a call whose return
/// address was taken away. The callee has run nothing since its return
/// instruction, so the answer is still in its register and is recorded
/// here; the return then goes on to where the call really came from.
/// True when it was sent on, in which case the frame is the one to
/// return to; false leaves the access violation to be delivered, there
/// being nowhere to send a return no call is waiting for.
fn answered(process: &Process, frame: &mut TrapFrame, addr: u64) -> bool {
    let trap = addr - RET_OFFSET;
    let entry = {
        let traps = TRAPS.lock();
        traps.iter().find(|t| t.trap == trap && t.pid == process.id).copied()
    };
    let Some(entry) = entry else {
        return false;
    };
    let sp = frame.sp();
    let Some(waited) = take_pending(process.id, sp) else {
        warn!(
            "imports: a return to {addr:#x} ({}) arrived with the stack at {sp:#x} \
             and no call of this thread waiting for it, so there is nowhere to send \
             it: it faults as it would without `imports_answers=`",
            Name(&entry)
        );
        return false;
    };
    answer(process, frame, &entry);
    frame.set_ip(waited.ret);
    true
}

/// The call a return arriving with the stack at `sp` belongs to: the
/// newest entry of the running thread, with the entries under it that
/// no return can reach any more discarded. Those are the calls that
/// never returned -- an unwind, a longjmp, a thread that exits -- and
/// the stack pointer is what finds them: the stack grows down, so an
/// entry waiting for a lower one than has arrived is past returning,
/// and nothing would ever match it.
fn take_pending(pid: u64, sp: u64) -> Option<Pending> {
    let thread = platform::current();
    let (mut found, mut unwound, mut stale) = (None, 0, 0);
    {
        let mut pending = PENDING.lock();
        while let Some(i) = pending.iter().rposition(|e| e.thread == thread && e.pid == pid) {
            let waited = pending[i];
            if waited.sp > sp {
                // Shallower than the return that arrived: not this
                // call's, and still reachable by a return of its own.
                break;
            }
            pending.remove(i);
            if waited.sp == sp {
                found = Some(waited);
                break;
            }
            stale = waited.trap;
            unwound += 1;
        }
    }
    if unwound != 0 && UNWOUND.fetch_add(unwound, Ordering::Relaxed) == 0 {
        // Said once, with the slot the innermost of them went through,
        // because a call whose answer never comes is read as a call
        // that gave none: the stream holds the call and nothing after.
        warn!(
            "imports: {unwound} calls did not return to where they were called \
             from, the innermost through the slot trapped at {stale:#x}: an \
             exception unwound past them, or a longjmp did; the summary line \
             counts the rest (imports_answers=)"
        );
    }
    found
}

/// Records what one call answered: the same `ret` the event stream
/// carries for `trace_from=`, under the import's name, so that a call
/// and its answer read as one pair however they are nested.
fn answer(process: &Process, frame: &TrapFrame, entry: &Trap) {
    if !events::enabled(Category::Call) || !events::syscall_pid_wanted(process.id) {
        return;
    }
    events::emit(
        Category::Call,
        "ret",
        format_args!(
            "{}",
            Answer {
                pid: process.id,
                process: process.name,
                call: core::str::from_utf8(&entry.name[..usize::from(entry.len)]).unwrap_or("?"),
                answer: frame.answer(),
            }
        ),
    );
    ANSWERED.fetch_add(1, Ordering::Relaxed);
}

/// Where in the region ntdll reserved the trap address of the entry at
/// `index` goes. The loader sizes that region by the same arithmetic
/// (`arm_imports`) and [`region`] refuses one that is short of it, so
/// the two halves cannot drift apart in silence.
fn trap_at(base: u64, index: u64) -> u64 {
    base + index * STRIDE
}

/// Puts `value` in the import slot at `slot`. An import address table
/// sits in a read-only part of its image, so the page is made writable,
/// written and put back as it was -- what NtWriteVirtualMemory does for
/// a debugger's write, and what keeps the program's own view of its
/// memory the one it would have had.
fn put(process: &Process, slot: u64, value: u64) -> Result<(), NtStatus> {
    let (old, base, size) = vm::protect(process, slot, 8, vad::PAGE_READWRITE)?;
    let written = process.write_u64(slot, value);
    let back = vm::protect(process, base, size, old);
    if written.is_err() {
        return Err(NtStatus::ACCESS_VIOLATION);
    }
    back.map(|_| ())
}

/// Takes one slot from the budget, or None once it is spent. Taken
/// before the entry is made, so two CPUs arming at once cannot both
/// spend the last one.
fn take_slot() -> Option<u32> {
    let mut left = LEFT.load(Ordering::Relaxed);
    loop {
        if left == 0 {
            return None;
        }
        match LEFT.compare_exchange_weak(left, left - 1, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Some(left - 1),
            Err(now) => left = now,
        }
    }
}

/// Whether the budget is spent, so that the loader is not read entry by
/// entry for imports none of which can be armed.
pub fn spent() -> bool {
    LEFT.load(Ordering::Relaxed) == 0
}

/// What the run armed and reached, for the end of a run. Both numbers,
/// always: slots armed with no first call are the imports the program
/// never used, which is as much of an answer as the calls it made.
pub fn log_stats() {
    if !wanted() {
        if REPEAT_ASKED.load(Ordering::Relaxed) != 0 {
            info!(
                "imports: imports_repeat= was given without imports=, so no \
                 slot was armed and no call was recorded"
            );
        }
        if ANSWERS_ASKED.load(Ordering::Relaxed) != 0 {
            info!(
                "imports: imports_answers= was given without imports=, so no \
                 slot was armed and no answer was recorded"
            );
        }
        return;
    }
    let (armed, done) = (ARMED.load(Ordering::Relaxed), RECORDED.load(Ordering::Relaxed));
    let read = Reads(READ.load(Ordering::Relaxed));
    let (mut named, mut source) = ([0u8; NAME_LEN], [0u8; NAME_LEN]);
    let (named_len, source_len) = {
        let stored = SPEC.lock();
        named[..stored.program_len].copy_from_slice(&stored.program[..stored.program_len]);
        source[..stored.dll_len].copy_from_slice(&stored.dll[..stored.dll_len]);
        (stored.program_len, stored.dll_len)
    };
    let program = core::str::from_utf8(&named[..named_len]).unwrap_or("?");
    let from = if source_len == 0 {
        "any DLL"
    } else {
        core::str::from_utf8(&source[..source_len]).unwrap_or("?")
    };
    if armed == 0 {
        info!(
            "imports: no import of {program} from {from} was armed: it never ran, \
             it imports nothing from there, or every slot was a data import (imports=)"
        );
        return;
    }
    if done == 0 && !events::enabled(Category::Call) {
        info!(
            "imports: {armed} slots armed and nothing written: `events=call` was \
             not given, so no call was recorded (imports=)"
        );
        return;
    }
    if LEFT.load(Ordering::Relaxed) == 0 {
        info!(
            "imports: first calls recorded: {done} of {armed} slots armed{read} (imports=); \
             the budget is spent, so the imports past it were left alone"
        );
    } else {
        info!("imports: first calls recorded: {done} of {armed} slots armed{read} (imports=)");
    }
    let asked = REPEAT_ASKED.load(Ordering::Relaxed);
    if asked != 0 {
        let again = REPEATED.load(Ordering::Relaxed);
        if REPEAT.load(Ordering::Relaxed) == 0 {
            info!(
                "imports: {again} later calls recorded of {asked}, the budget spent: \
                 a slot is repaired on the next call through it and only first calls \
                 are recorded past that (imports_repeat=)"
            );
        } else {
            info!("imports: {again} later calls recorded of {asked} (imports_repeat=)");
        }
    }
    log_answers();
}

/// What the run answered, for the end of a run: the answers recorded,
/// and each way one was not. A call in the stream with no answer after
/// it is read as a call that gave none, so every one of these is worth
/// a line of its own.
fn log_answers() {
    let asked = ANSWERS_ASKED.load(Ordering::Relaxed);
    if asked == 0 {
        return;
    }
    let done = ANSWERED.load(Ordering::Relaxed);
    if ANSWERS.load(Ordering::Relaxed) == 0 {
        info!(
            "imports: {done} answers recorded of {asked}, the budget spent: the \
             calls past it are recorded without one (imports_answers=)"
        );
    } else {
        info!("imports: {done} answers recorded of {asked} (imports_answers=)");
    }
    let lost = ANSWERS_LOST.load(Ordering::Relaxed);
    if lost != 0 {
        info!(
            "imports: {lost} calls were recorded with no answer trapped at all: \
             their return address could not be taken away, or no entry was left \
             to keep it in ({MAX_PENDING} calls may wait for their return at \
             once) (imports_answers=)"
        );
    }
    let unwound = UNWOUND.load(Ordering::Relaxed);
    if unwound != 0 {
        info!(
            "imports: {unwound} calls never returned to where they were called \
             from: an exception unwound past them, or the thread ended inside \
             them, so the stream holds the call and no answer (imports_answers=)"
        );
    }
    let waiting = PENDING.lock().len();
    if waiting != 0 {
        info!(
            "imports: {waiting} calls were still waiting for their return when \
             the run ended (imports_answers=)"
        );
    }
}

/// `, <n> read as data` for the end of the summary line, or nothing at
/// all where no slot was: a run that met no reader should not have to
/// say so, and one that met readers must.
struct Reads(u64);

impl fmt::Display for Reads {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            0 => Ok(()),
            n => write!(f, ", {n} read as data"),
        }
    }
}

/// One `import` event's fields: the JSON members [`events::emit`] writes
/// after `"ev":"import",`, without braces and with every string quoted.
/// A type rather than a `format_args!` where the event is written, so
/// that a host test can read the line the stream would carry (the events
/// are split out of the serial log, which is what a boot scenario
/// matches, so no scenario can see a doubled comma here).
struct Fields<'a> {
    pid: u64,
    process: &'a str,
    call: &'a str,
    args: &'a [u64],
}

impl fmt::Display for Fields<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (pid, args) = (self.pid, Words(self.args));
        let (process, call) = (Str(self.process), Str(self.call));
        write!(f, r#""pid":{pid},"process":{process},"call":{call},"args":{args}"#)
    }
}

/// An entry's name, for a log line.
struct Name<'a>(&'a Trap);

impl fmt::Display for Name<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = &self.0.name[..usize::from(self.0.len)];
        f.write_str(core::str::from_utf8(name).unwrap_or("?"))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BUDGET, Fields, MAX_ANSWERS, MAX_BUDGET, MAX_REPEAT, READ, RET_OFFSET, Reads, STRIDE,
        answer_budget, repeat_budget, split_budget, trap_at,
    };

    /// The summary line says nothing of reads where there were none,
    /// and says them where there were: a slot read as data is the one
    /// case in which a program used an import without calling it.
    #[test]
    fn reads_are_in_the_summary_only_where_there_are_some() {
        assert_eq!(alloc::format!("{}", Reads(0)), "");
        assert_eq!(alloc::format!("{}", Reads(1)), ", 1 read as data");
        assert_eq!(READ.load(core::sync::atomic::Ordering::Relaxed), 0);
    }

    /// What the event stream carries, which no boot scenario sees. The
    /// fields are `trace_from=`'s own, under a name of their own: a
    /// reader must be able to tell a call recorded every time from one
    /// recorded the first time only.
    #[test]
    fn a_first_call_is_the_fields_of_one_json_event() {
        let fields = Fields {
            pid: 12,
            process: "WINWORD.EXE",
            call: "d2d1.dll!D2D1CreateFactory",
            args: &[0, 2078216, 0, 2078208],
        };
        assert_eq!(
            alloc::format!("{{\"ev\":\"import\",{fields}}}"),
            "{\"ev\":\"import\",\"pid\":12,\"process\":\"WINWORD.EXE\",\"call\":\
             \"d2d1.dll!D2D1CreateFactory\",\"args\":[0,2078216,0,2078208]}"
        );
    }

    /// The one number the loader and the kernel must agree on: the
    /// loader reserves `count * STRIDE` bytes and the kernel puts entry
    /// `index` at `index * STRIDE` into them, so a region sized that way
    /// ends exactly past the last entry's trap address.
    #[test]
    fn a_trap_address_is_the_region_s_base_and_the_entry_s_place_in_it() {
        assert_eq!(trap_at(0x1_0000, 0), 0x1_0000);
        assert_eq!(trap_at(0x1_0000, 1), 0x1_0000 + STRIDE);
        let count = 3;
        assert_eq!(trap_at(0x1_0000, count - 1) + STRIDE, 0x1_0000 + count * STRIDE);
    }

    /// A slot's two trap addresses: the call's at the start of the
    /// space the loader reserved for it and the return's
    /// [`RET_OFFSET`] into that space, both inside it and told apart
    /// by the offset within it.
    #[test]
    fn a_slot_has_a_second_trap_address_for_its_return() {
        let call = trap_at(0x1_0000, 3);
        assert_eq!(call % STRIDE, 0);
        assert_eq!((call + RET_OFFSET) % STRIDE, RET_OFFSET);
        assert!(call + RET_OFFSET < trap_at(0x1_0000, 4));
    }

    /// `imports_answers=` takes a number of answers, by the same rule
    /// as the budgets beside it and for the same reason: a trace that
    /// quietly recorded no answer reads as calls that gave none.
    #[test]
    fn an_answer_budget_is_a_number_of_answers_or_nothing() {
        assert_eq!(answer_budget("5"), Some(5));
        assert_eq!(answer_budget(&alloc::format!("{MAX_ANSWERS}")), Some(MAX_ANSWERS));
        assert_eq!(answer_budget("0"), None);
        assert_eq!(answer_budget(&alloc::format!("{}", u64::from(MAX_ANSWERS) + 1)), None);
        assert_eq!(answer_budget(""), None);
        assert_eq!(answer_budget("every"), None);
    }

    /// `imports_repeat=` takes a number of records and nothing else.
    /// A value it cannot read leaves the slots repaired on their first
    /// call, because a trace that recorded one call a slot where every
    /// call was asked for reads as a program that made one.
    #[test]
    fn a_repeat_budget_is_a_number_of_records_or_nothing() {
        assert_eq!(repeat_budget("3"), Some(3));
        assert_eq!(repeat_budget("1"), Some(1));
        assert_eq!(repeat_budget(&alloc::format!("{MAX_REPEAT}")), Some(MAX_REPEAT));
        assert_eq!(repeat_budget("0"), None);
        assert_eq!(repeat_budget(&alloc::format!("{}", u64::from(MAX_REPEAT) + 1)), None);
        assert_eq!(repeat_budget(""), None);
        assert_eq!(repeat_budget("all"), None);
        assert_eq!(repeat_budget("3 "), None);
    }

    #[test]
    fn a_value_without_a_count_keeps_the_default() {
        assert_eq!(split_budget("WINWORD.EXE!d2d1.dll"), Ok(("WINWORD.EXE!d2d1.dll", BUDGET)));
        assert_eq!(split_budget("WINWORD.EXE"), Ok(("WINWORD.EXE", BUDGET)));
    }

    #[test]
    fn a_count_is_taken_off_the_end() {
        assert_eq!(split_budget("WINWORD.EXE!d2d1.dll:9"), Ok(("WINWORD.EXE!d2d1.dll", 9)));
    }

    #[test]
    fn a_count_that_is_not_one_is_refused() {
        for bad in ["a.exe:0", "a.exe:", "a.exe:lots"] {
            assert!(split_budget(bad).is_err(), "{bad} was taken as a count");
        }
        let past = alloc::format!("a.exe:{}", MAX_BUDGET as u64 + 1);
        assert!(split_budget(&past).is_err());
    }
}
