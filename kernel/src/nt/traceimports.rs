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
//! A register is a number, and a call's meaning is mostly in what its
//! numbers point at: a name, a path, a message. Under
//! `imports_strings=<n>` the kernel reads a window at each argument of
//! a recorded call and quotes the ones that read as a NUL-terminated
//! run of printable characters, one byte each or two. That is evidence
//! and not proof -- nothing here knows a call's signature, so a number
//! that happens to point at such a run is quoted like a string that is
//! one -- which is why the quotes sit beside the registers rather than
//! in place of them, and why the run must be terminated inside the
//! window and have [`TEXT_LEAST`] characters before it is text at all.
//!
//! A buffer is the other half of that. A call that fills one is
//! handed an empty one, so the text it is about is in the buffer only
//! once the call is over, and a quote of its arguments reads as a call
//! that was handed no text at all. Where the return is trapped
//! (`imports_answers=`) the quoting therefore runs again at the
//! return: at the register the answer came back in, and at the
//! arguments that were no text at the call -- the kernel kept those
//! from the call, the callee having been free to clobber the registers
//! -- so the same register can be a number at the call and a string at
//! its return, which is what every `Get*` that fills a buffer is. Each
//! quote says which place it was read at, because a quote a reader
//! cannot pair with a number is a quote of nothing.
//!
//! Not every call says what it means in text, and the ones that do
//! not are where a trace of Word's startup ends up: a method that
//! answers a structure by value is handed the caller's own buffer in
//! an argument register and answers that same pointer, so the answer
//! is bytes and no string at all and a quote of either end reads
//! nothing. `imports_bytes=<n>` writes the first [`BYTE_WINDOW`]
//! bytes of every place that reads as no text as hex instead, beside
//! the quotes and in an object of its own: the same window, the same
//! places and the same budget arithmetic, and the two readings told
//! apart because what a call said and what it was handed are two
//! questions.
//!
//! A wide trace spends most of itself on the same few names: a
//! program's heap, string and locale imports are called in the
//! thousands while the ones a question is about are called once, and
//! the budget goes to them in that order. `imports_except=<text>[,...]`
//! leaves those imports unarmed. A slot whose `<dll>!<name>` holds one
//! of the texts, compared without regard to case, is never written with
//! a trap address at all, so it costs no slot of the budget, no fault
//! and no record, and the program calls it as it would with nothing
//! armed. The texts are substrings and not patterns, so one of them
//! names a family (`api-ms-win-crt-` is every CRT shim, `ntdll.dll!Rtl`
//! every Rtl export), and how many slots they left alone is a summary
//! line: a name missing from a trace must read as the filter's doing
//! and not as the program not having called it.
//!
//! What it costs: one page fault per slot (the kernel's own fault path,
//! plus a linear scan of the slots armed and two protection changes to
//! write the slot), and nothing after it -- or, under
//! `imports_repeat=`, one such fault per call until the budget is
//! spent, and under `imports_answers=` a second fault for each call
//! answered, plus a read of up to [`TEXT_WINDOW`] bytes per argument
//! under `imports_strings=` or `imports_bytes=` -- and, at a trapped
//! return, one more per argument that was no text at the call and one
//! for the answer. What
//! it cannot do is in OBSERVABILITY: the first four integer argument
//! registers and no others, and static imports only.
//! A callee that reads its own return address sees the kernel's trap
//! while its return is trapped, which is the one thing a program can
//! tell at all.

use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use super::crashreport::module_at;
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

/// The most `imports_strings=<n>` may ask for, [`MAX_ANSWERS`]'s own:
/// a quote is a read of the program's memory and a string in a line,
/// both of which the budget is what bounds.
const MAX_STRINGS: u32 = 1 << 20;

/// The most `imports_bytes=<n>` may ask for, [`MAX_STRINGS`]'s own: a
/// dump is a read of the program's memory and a string in a line, the
/// same two things a quote is.
const MAX_BYTES: u32 = 1 << 20;

/// The most texts `imports_except=` may name. Each is matched against
/// every import of the traced program as its slots are reported, so
/// this bounds that scan and keeps the option a filter and not a
/// language. Sixteen because of what one measured wide trace needs:
/// eleven families (the CRT shims, ntdll's Rtl exports, VCRUNTIME's
/// memory functions, the core heap, KERNEL32's heap, MSVCP's mutex,
/// the SRW locks, the last-error pair, Fls, the critical sections and
/// GetCurrentThreadId) cover 96% of the records of Word's startup, and
/// a bound a real question already fills is no bound at all.
const MAX_EXCEPT: usize = 16;

/// The bytes read at one argument to see whether it is text, and never
/// past the end of the page it points into. An argument is a register,
/// so what it points at may be anything at all: a window is what makes
/// a read of it worth trying, and the page is what makes the read
/// cheap to refuse.
const TEXT_WINDOW: usize = 256;

/// The characters quoted from one argument at most, a longer run being
/// quoted to here and marked as cut. A name, a path or a message is
/// short; a buffer that happens to read as text is not worth a line.
const TEXT_CHARS: usize = 120;

/// The bytes written as hex from one place that reads as no text, a
/// longer window being dumped to here. Thirty-two is the structures
/// these calls answer by value and a little over -- a GUID, a
/// rectangle of four floats, a size and a flag -- because what a dump
/// is for is a call whose answer is one of those. More than that is a
/// buffer, and a buffer belongs in a dump of memory and not in a line
/// per call.
const BYTE_WINDOW: usize = 32;

/// The characters a run must have before it reads as text rather than
/// as a number that happens to point somewhere. Four is what tells a
/// name from an array of small integers.
const TEXT_LEAST: usize = 4;

/// The calls carrying text that are said in the log as well, the rest
/// being left to the stream and the summary. Enough to read a run's
/// first few without the events, as [`WALKS_SAID`] is.
const TEXTS_SAID: u64 = 8;

/// The argument registers a call is recorded with, which is what
/// `TrapFrame::arg_regs` gives and what a quote is indexed by.
const ARGS: usize = 4;

/// Where a recorded call's quotes are read: its argument registers, in
/// the order [`TrapFrame::arg_regs`] gives them.
const CALL_PLACES: [Place; ARGS] = [Place::Arg(0), Place::Arg(1), Place::Arg(2), Place::Arg(3)];

/// Where a trapped return's are: the answer first, then the call's own
/// arguments, which the kernel kept from the call ([`Pending`]).
const RETURN_PLACES: [Place; ARGS + 1] =
    [Place::Answer, Place::Arg(0), Place::Arg(1), Place::Arg(2), Place::Arg(3)];

/// Both lists are written out, so an arch with a fifth argument
/// register would leave them short in silence.
const _: () = assert!(ARGS == 4, "CALL_PLACES and RETURN_PLACES list every argument register");

/// The lowest address an argument may point at: NT maps nothing in the
/// first 64 KiB, so an argument below it is a number and not worth a
/// read.
const LOWEST_POINTER: u64 = 0x1_0000;

/// The most calls whose returns may be waited for at one time, over
/// every thread. An entry is kernel memory held from a call until its
/// return, so this is what bounds a program whose calls do not return;
/// a call that finds it spent is recorded without an answer, which the
/// summary says.
const MAX_PENDING: usize = 1024;

/// How many walks and how many returns put back are named in the log,
/// the rest being left to the summary lines. A program that unwinds for
/// a living (Office does) would fill the log with them, and what these
/// lines are for is the first walks of a run, where the question is
/// which frames a walk crossed and which calls were still waiting in
/// them.
const WALKS_SAID: u64 = 8;

/// How many put-back returns are named, for the same reason.
const RESTORES_SAID: u64 = 64;

/// How many pending returns are put back at once when an exception is
/// dispatched. One batch is a fixed array on the kernel's stack, so that
/// path allocates nothing; more than a batch's worth is taken in further
/// rounds.
const RESTORE_BATCH: usize = 16;

/// How far below the stack pointer a return would have arrived with its
/// trap address is looked for, in words. x86-64's is the first word
/// looked at, the one the call pushed; ARM64's call left it in the link
/// register, so it is wherever the callee saved it, which is that
/// callee's own frame just below. A frame's worth is the reach, and a
/// trap address not found inside it is counted and said.
const RESTORE_WORDS: u64 = 512;

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

/// What `imports_except=` named: the texts an import's `<dll>!<name>`
/// is matched against, and how many of them were given.
struct Except {
    texts: [[u8; NAME_LEN]; MAX_EXCEPT],
    lens: [usize; MAX_EXCEPT],
    held: usize,
}

static EXCEPT: IrqMutex<Except> =
    IrqMutex::new(Except { texts: [[0; NAME_LEN]; MAX_EXCEPT], lens: [0; MAX_EXCEPT], held: 0 });

/// Whether `imports_except=` named anything, so that a run without it
/// pays one relaxed load as a slot is reported and not a lock.
static EXCEPTING: AtomicBool = AtomicBool::new(false);

/// Slots left unarmed by `imports_except=`, which is a summary line for
/// the reason the reads are: a name the filter kept out must not read
/// afterwards as a call the program did not make.
static UNARMED: AtomicU64 = AtomicU64::new(0);

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
/// Arguments quoted as text, what is left of what `imports_strings=`
/// asked for, and the calls that carried at least one. All three are a
/// summary line: a trace whose quotes ran out reads afterwards as calls
/// that were handed no text, and one that quoted nothing at all is
/// either a program that passes none or a window too small for it.
static QUOTED: AtomicU64 = AtomicU64::new(0);
static STRINGS: AtomicU32 = AtomicU32::new(0);
static STRINGS_ASKED: AtomicU32 = AtomicU32::new(0);
static TEXT_CALLS: AtomicU64 = AtomicU64::new(0);

/// The returns of which one did, which the same summary line says: a
/// quote at a return is a quote of what the call wrote, and a reader
/// weighing what a trace cost needs the two apart.
static TEXT_RETURNS: AtomicU64 = AtomicU64::new(0);

/// The same four for `imports_bytes=`: the places dumped as hex, what
/// is left of that budget, and the calls and returns that carried one.
/// Counted apart from the quotes because the two answer different
/// questions of the same run -- which calls carried text, and what the
/// calls that carried none were handed and answered.
static DUMPED: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU32 = AtomicU32::new(0);
static BYTES_ASKED: AtomicU32 = AtomicU32::new(0);
static BYTE_CALLS: AtomicU64 = AtomicU64::new(0);
static BYTE_RETURNS: AtomicU64 = AtomicU64::new(0);

/// Returns put back where their calls left them for an exception to
/// unwind through, and those whose trap address could not be found on
/// the stack to put back. Both are summary lines: a call put back is a
/// call whose answer will never arrive, and one not put back is a frame
/// the program's own walk ends at.
static RESTORED: AtomicU64 = AtomicU64::new(0);
static RESTORE_MISSED: AtomicU64 = AtomicU64::new(0);
/// Walks answered with the return address of the call they begin in,
/// the pc they would have walked from being that call's return trap,
/// and walks that began at a trap address no call of the thread was
/// waiting for. Both are summary lines, for the same reason as the two
/// above: the first is a call whose answer will never arrive, the
/// second a walk that ends on its first frame.
static WALKED_FROM: AtomicU64 = AtomicU64::new(0);
static WALKED_MISSED: AtomicU64 = AtomicU64::new(0);
/// Walks announced, which is what [`WALKS_SAID`] is counted against.
static WALKS: AtomicU64 = AtomicU64::new(0);

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
    /// The call's argument registers, which its return no longer has:
    /// the ABI leaves them to the callee, and a buffer the call filled
    /// is read at the return through the address it was handed at.
    args: [u64; ARGS],
    /// Which of those already read as text at the call, which the
    /// return does not read again: a string a call was handed is the
    /// same string at its return, so a second quote of it would be the
    /// same text twice and a second quote off the budget. A place only
    /// dumped as bytes is not one of these -- the bytes of a buffer
    /// are what the call changed, which is the whole reason to read
    /// one at a return (`imports_bytes=`).
    quoted: [bool; ARGS],
}

/// The calls whose returns are waited for, over every thread. Read at a
/// return and written at a call, both under this lock and never while
/// user memory is touched.
static PENDING: IrqMutex<Vec<Pending>> = IrqMutex::new(Vec::new());

/// The longest importing module name kept, which is as much of an
/// image's file name as a capture holds (`crashreport::Module`).
const MODULE_LEN: usize = 40;

/// The image of a slot that no image holds, or of one in an image past
/// [`MAX_IMAGES`]: such a slot is armed and recorded all the same, with
/// `?` for the module, rather than refused for want of a name.
const NO_IMAGE: u16 = u16::MAX;

/// The most images one run names. A program of Word's size imports from
/// some ninety, and the table is one entry an image however many of its
/// slots are armed, so this is what the index in a [`Trap`] holds and
/// not a bound anything reaches.
const MAX_IMAGES: usize = NO_IMAGE as usize;

/// One image some of whose import slots are armed.
#[derive(Clone, Copy)]
struct Image {
    /// Whose image it is: the program `imports=` names may run twice,
    /// and two processes' images are as often as not at one base.
    pid: u64,
    /// Where it is mapped, which is what tells two images apart where
    /// their names cannot (one DLL mapped into two processes).
    base: u64,
    name: [u8; MODULE_LEN],
    len: u8,
    /// How many of its slots are armed, counted as each one is
    /// ([`note_armed`]): what the summary says of where a budget went.
    slots: u32,
}

impl Image {
    const NONE: Image = Image { pid: 0, base: 0, name: [0; MODULE_LEN], len: 0, slots: 0 };

    fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..usize::from(self.len)]).unwrap_or("?")
    }
}

/// The images the armed slots are in, one entry each. Searched rather
/// than hashed because it is short: a program imports from a few dozen
/// images where it has thousands of slots, which is the reason a
/// [`Trap`] holds an index here and not a name of its own.
static IMAGES: IrqMutex<Vec<Image>> = IrqMutex::new(Vec::new());

/// Armed slots in no image at all, which nothing can name.
static NAMELESS: AtomicU32 = AtomicU32::new(0);

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
    /// The image the slot is in, as an index into [`IMAGES`]: who made
    /// the call, which the name above does not say. One import is a
    /// slot in every image that imports it, so a trace of a whole
    /// program records the same name called from the program and from
    /// its DLLs, and without this the records are one line twice. Free:
    /// a `u16` here fits the padding this struct has anyway.
    image: u16,
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

/// `imports_except=<text>[,<text>...]`: leave the imports whose
/// `<dll>!<name>` holds one of the texts unarmed, so that the budget,
/// the faults and the records go to the rest. Needs `imports=`. The
/// texts are compared without regard to case, as a file name is, and a
/// value that is not one is refused outright for the reason the budgets
/// are: a filter that quietly excluded nothing reads afterwards as a
/// budget the calls it was meant to keep out were entitled to.
pub fn set_except(spec: &str) {
    let mut named = Except { texts: [[0; NAME_LEN]; MAX_EXCEPT], lens: [0; MAX_EXCEPT], held: 0 };
    for text in spec.split(',') {
        if text.is_empty() || text.len() > NAME_LEN || named.held == MAX_EXCEPT {
            warn!(
                "imports: imports_except={spec} reads <text>[,<text>...], such as \
                 api-ms-win-crt-,ntdll.dll!Rtl, with {MAX_EXCEPT} texts at most of \
                 {NAME_LEN} bytes each: every import is armed, as without it"
            );
            return;
        }
        named.texts[named.held][..text.len()].copy_from_slice(text.as_bytes());
        named.lens[named.held] = text.len();
        named.held += 1;
    }
    let held = named.held;
    *EXCEPT.lock() = named;
    EXCEPTING.store(true, Ordering::Release);
    info!(
        "imports: an import whose name holds one of {held} texts ({spec}) is left \
         unarmed, so it takes no slot of the budget, no fault and no record \
         (imports_except=)"
    );
}

/// Whether `imports_except=` named this import, which is what leaves
/// its slot alone: `name` is the `<dll>!<name>` a record would carry,
/// and the texts are substrings of it.
fn excluded(name: &[u8]) -> bool {
    if !EXCEPTING.load(Ordering::Acquire) {
        return false;
    }
    let named = EXCEPT.lock();
    (0..named.held).any(|i| holds(name, &named.texts[i][..named.lens[i]]))
}

/// Whether `name` holds `text`, without regard to case. An empty text
/// holds nothing: `set_except` refuses one, and a window of no bytes
/// would otherwise match every name and arm nothing at all.
fn holds(name: &[u8], text: &[u8]) -> bool {
    !text.is_empty()
        && name.len() >= text.len()
        && name.windows(text.len()).any(|at| at.eq_ignore_ascii_case(text))
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

/// Whether an `import` event for this process would be written: the
/// stream carries the category and wants the process. The trap handler
/// asks before it takes a budget or marks a slot reached, so that what
/// the budgets bound is the records, which cost, and not the faults,
/// which do not (`resolve`).
fn carried(process: &Process) -> bool {
    events::enabled(Category::Call) && events::syscall_pid_wanted(process.id)
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

/// `imports_strings=<n>`: quote the arguments of a recorded call that
/// read as text, `n` arguments at most. Needs `imports=` to have armed
/// something and `events=call` for the calls to be recorded, as the
/// quotes go with the call: the first [`TEXTS_SAID`] of them are said
/// in the log as well, so a reader who has only the serial log sees
/// what the option found.
pub fn set_strings(spec: &str) {
    let Some(n) = string_budget(spec) else {
        warn!(
            "imports: imports_strings={spec} reads a number from 1 to \
             {MAX_STRINGS}, the texts to quote: nothing is read as text at \
             all instead, as without it"
        );
        return;
    };
    STRINGS.store(n, Ordering::Relaxed);
    STRINGS_ASKED.store(n, Ordering::Relaxed);
    info!(
        "imports: what reads as text at a recorded call -- its arguments, and \
         at a trapped return its answer and the arguments that were none at \
         the call -- {n} quotes at most, {TEXT_CHARS} characters of each \
         (imports_strings=)"
    );
}

/// `imports_strings=`'s value, or None for one that is not a number
/// from 1 to [`MAX_STRINGS`]. Refused outright for the reason the
/// others are: a trace that quietly quoted nothing reads afterwards as
/// a program that passed no text.
fn string_budget(text: &str) -> Option<u32> {
    budget_of(text, MAX_STRINGS)
}

/// `imports_bytes=<n>`: write the bytes at every place of a recorded
/// call that reads as no text as hex, `n` places at most. The reading
/// of a call whose arguments and answer are no strings: a structure
/// answered by value is the caller's own buffer handed back, an array
/// of pointers is an array of pointers, and neither has a quote. Needs
/// `imports=` and `events=call` as the quotes do, and shares their
/// read: one window per place, text where `imports_strings=` asked for
/// it and the window is text, these bytes otherwise.
pub fn set_bytes(spec: &str) {
    let Some(n) = byte_budget(spec) else {
        warn!(
            "imports: imports_bytes={spec} reads a number from 1 to {MAX_BYTES}, \
             the places to dump: nothing is read as bytes at all instead, as \
             without it"
        );
        return;
    };
    BYTES.store(n, Ordering::Relaxed);
    BYTES_ASKED.store(n, Ordering::Relaxed);
    info!(
        "imports: the bytes at a place of a recorded call that reads as no text \
         -- its argument registers, and at a trapped return its answer and those \
         arguments again -- {n} dumps at most, {BYTE_WINDOW} bytes of each \
         (imports_bytes=)"
    );
}

/// `imports_bytes=`'s value, or None for one that is not a number from
/// 1 to [`MAX_BYTES`], by [`string_budget`]'s own rule.
fn byte_budget(text: &str) -> Option<u32> {
    budget_of(text, MAX_BYTES)
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
    // Named by `imports_except=`: the slot keeps the export's address,
    // so the call costs the program nothing, and the budget keeps its
    // slot for an import a record is worth having of.
    if excluded(name) {
        UNARMED.fetch_add(1, Ordering::Relaxed);
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
        image: image_of(process, slot),
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
    if let Err(e) = put(process, slot, trap) {
        if e.step.landed() {
            // The trap address went in, so the slot is armed and the
            // program faults into the tracer as meant; what stayed
            // behind is the page's relaxed protection, which the
            // program can now write to. Counted among the armed,
            // because it is one.
            warn!(
                "imports: the slot at {slot:#x} of {} is armed, but {} ({:#x}): \
                 its page stays writable (imports=)",
                Name(&entry),
                e.step.said(),
                e.status.0
            );
        } else {
            // The slot still holds the export's address, so the program
            // is unaffected; the entry stays behind as dead weight and
            // is not counted, which is why the two numbers are worth
            // comparing.
            warn!(
                "imports: the slot at {slot:#x} of {} is not armed: {} ({:#x})",
                Name(&entry),
                e.step.said(),
                e.status.0
            );
            return false;
        }
    }
    ARMED.fetch_add(1, Ordering::Relaxed);
    note_armed(entry.image);
    true
}

/// The image the slot at `slot` is in, as an index into [`IMAGES`]. An
/// import address table is part of its own image, so the slot's address
/// is who imported the name -- there is no stack to walk and nothing to
/// ask the program, which is what makes this answerable at all for a
/// DLL Microsoft wrote. Resolved here, once a slot, where a page's
/// protection is being changed anyway: a recorded call then costs
/// nothing to attribute, and a call nobody records costs nothing.
fn image_of(process: &Process, slot: u64) -> u16 {
    let Some(module) = module_at(process, slot) else {
        return NO_IMAGE;
    };
    let (base, name) = (module.base(), module.name().as_bytes());
    let mut held = IMAGES.lock();
    if let Some(at) = held.iter().position(|had| had.pid == process.id && had.base == base) {
        return u16::try_from(at).unwrap_or(NO_IMAGE);
    }
    if held.len() >= MAX_IMAGES {
        return NO_IMAGE;
    }
    let mut entry = Image { pid: process.id, base, ..Image::NONE };
    entry.len = u8::try_from(name.len().min(MODULE_LEN)).unwrap_or(0);
    entry.name[..usize::from(entry.len)].copy_from_slice(&name[..usize::from(entry.len)]);
    if held.try_push(entry).is_err() {
        let images = held.len();
        // The lock goes first: a warning takes the console's, and the
        // order of those two is this one everywhere.
        drop(held);
        warn!(
            "imports: out of memory for the image table at {images} images: the slots \
             of the images past it are armed and recorded without a module (imports=)"
        );
        return NO_IMAGE;
    }
    u16::try_from(held.len() - 1).unwrap_or(NO_IMAGE)
}

/// Counts one slot against the image it is in, once the slot is armed.
/// Counted here and not where the image is named, so that a slot the
/// kernel could not arm is counted as armed by neither number.
fn note_armed(image: u16) {
    if image == NO_IMAGE {
        NAMELESS.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let mut held = IMAGES.lock();
    if let Some(entry) = held.get_mut(usize::from(image)) {
        entry.slots += 1;
    }
}

/// The name of the image at `image`, copied into `out` so that a record
/// is written with no lock of this table held, or `?` where the slot is
/// in no image the table could name.
fn from_name(image: u16, out: &mut [u8; MODULE_LEN]) -> &str {
    let mut written = 0;
    if image != NO_IMAGE {
        let held = IMAGES.lock();
        if let Some(entry) = held.get(usize::from(image)) {
            written = usize::from(entry.len);
            out[..written].copy_from_slice(&entry.name[..written]);
        }
    }
    core::str::from_utf8(&out[..written]).ok().filter(|name| !name.is_empty()).unwrap_or("?")
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
    // Whether a record of this call would be written at all. The
    // record is what a traced call costs -- 2.1 ms of bytes leaving
    // the guest against the fault's 5 us, measured (OBSERVABILITY
    // "What the stream costs") -- so a call whose record is not
    // written leaves the slot as it found it, armed and not yet
    // reached, and takes none of the budget below. That is what makes
    // `imports=` with `events_from=` a trace of a window and not of a
    // run's first moments: however many calls went through the slot
    // before the stream started, its budget is whole when it does and
    // the first call the stream carries is a first call.
    let writing = carried(process);
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
            entry.reached |= writing;
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
    // one is recorded only while there is budget for it. Where nothing
    // is written nothing is taken, and the slot stays armed whatever
    // the budget, because what it is still holding is its first call.
    let again = if !writing {
        true
    } else if first {
        REPEAT.load(Ordering::Relaxed) != 0
    } else {
        take_budget(&REPEAT)
    };
    // The record, and with it which arguments it already quoted: a
    // return reads the others and the answer, and not these again.
    let quoted =
        if writing && (first || again) { record(process, frame, &entry, first) } else { None };
    if !again && let Err(e) = put(process, entry.slot, entry.target) {
        if e.step.landed() {
            // The export's address went back in, so calls from here
            // cost nothing; the page is left writable.
            warn!(
                "imports: the slot at {:#x} of {} holds its export again, but {} \
                 ({:#x}): its page stays writable",
                entry.slot,
                Name(&entry),
                e.step.said(),
                e.status.0
            );
        } else {
            // The call still goes on (the instruction pointer below is
            // what sends it), so this costs a fault a call from here
            // rather than the call itself; a run that reads as slow
            // says why.
            warn!(
                "imports: the slot at {:#x} of {} could not be put back -- {} \
                 ({:#x}): every call through it faults",
                entry.slot,
                Name(&entry),
                e.step.said(),
                e.status.0
            );
        }
    }
    // The call's answer, where `imports_answers=` asks for one and the
    // stream carries the call: an answer to a call the stream does not
    // carry would read as an answer to nothing. The budget is taken
    // here, at the call, which is where the cost is committed.
    if let Some(quoted) = quoted
        && take_budget(&ANSWERS)
    {
        trap_return(process, frame, &entry, &quoted);
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
        if let Err(e) = put(process, entry.slot, entry.target) {
            if e.step.landed() {
                warn!(
                    "imports: the slot at {:#x} of {} holds its export again after its \
                     value was read as data, but {} ({:#x}): its page stays writable",
                    entry.slot,
                    Name(entry),
                    e.step.said(),
                    e.status.0
                );
            } else {
                warn!(
                    "imports: the slot at {:#x} of {} could not be put back after its \
                     value was read as data -- {} ({:#x}): every call through it faults",
                    entry.slot,
                    Name(entry),
                    e.step.said(),
                    e.status.0
                );
            }
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
/// it cannot, every call is. Some where the stream carries it, which is
/// what trapping its return waits on (`imports_answers=`), and then
/// which of its arguments read as text -- what its return reads is the
/// others.
fn record(process: &Process, frame: &TrapFrame, entry: &Trap, first: bool) -> Option<[bool; ARGS]> {
    if !carried(process) {
        return None;
    }
    let args = frame.arg_regs();
    let mut bytes = [[0u8; TEXT_WINDOW]; ARGS];
    let mut found = [None; ARGS];
    quote(process, &args.map(Some), &mut bytes, &mut found);
    let text = Texts { places: &CALL_PLACES, found: &found, bytes: &bytes };
    let name = core::str::from_utf8(&entry.name[..usize::from(entry.len)]).unwrap_or("?");
    let mut owner = [0u8; MODULE_LEN];
    let from = from_name(entry.image, &mut owner);
    events::emit(
        Category::Call,
        "import",
        format_args!(
            "{}",
            Fields { pid: process.id, process: process.name, from, call: name, args: &args, text }
        ),
    );
    if text.any_text() && TEXT_CALLS.fetch_add(1, Ordering::Relaxed) < TEXTS_SAID {
        info!("imports: the call to {name} carries text: {} (imports_strings=)", Said(text));
    }
    if text.any_bytes() && BYTE_CALLS.fetch_add(1, Ordering::Relaxed) < TEXTS_SAID {
        info!("imports: the call to {name} carries bytes: {} (imports_bytes=)", Dumped(text));
    }
    if first {
        RECORDED.fetch_add(1, Ordering::Relaxed);
    } else {
        REPEATED.fetch_add(1, Ordering::Relaxed);
    }
    Some(found.map(|found| found.is_some_and(|read| read.is_text())))
}

/// Reads a window at each value that could point somewhere and says
/// what it read, one of a budget per place. A `None` is a place not to
/// be read at all, which is how a return skips the arguments the call
/// already quoted. Nothing at all where neither `imports_strings=` nor
/// `imports_bytes=` was given, which is the usual case and two relaxed
/// loads; a budget is taken after the read, so a value that is a plain
/// number costs nothing.
///
/// Text first where the window is text and a quote is left: a run of
/// printable characters is what a reader wants of it, and the bytes
/// behind that run would say the same thing twice. Everything else
/// readable is the bytes themselves, which is what a structure
/// answered by value, an array of pointers or a buffer of numbers
/// amounts to -- and so is a run of text with no quote left, because a
/// spent string budget must not read afterwards as a place that held
/// nothing.
fn quote(
    process: &Process,
    at: &[Option<u64>],
    bytes: &mut [[u8; TEXT_WINDOW]],
    found: &mut [Option<Text>],
) {
    if STRINGS.load(Ordering::Relaxed) == 0 && BYTES.load(Ordering::Relaxed) == 0 {
        return;
    }
    for ((&value, buf), found) in at.iter().zip(bytes.iter_mut()).zip(found.iter_mut()) {
        let Some(value) = value else {
            continue;
        };
        let Some(window) = window_at(process, value, buf) else {
            continue;
        };
        if let Some(text) = text_in(&buf[..window])
            && take_budget(&STRINGS)
        {
            *found = Some(text);
            QUOTED.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        if take_budget(&BYTES) {
            *found = Some(Text::Bytes(window.min(BYTE_WINDOW)));
            DUMPED.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Reads the window at `addr` into `buf` and says how much of it was
/// read, or None where the address is no pointer or the page is not
/// there. Never past the end of the page `addr` points into: the next
/// page may be unmapped, and a read stopped at the page is one that
/// cannot be refused for the wrong reason -- so a run that crosses a
/// page boundary is read to the boundary and no further, which is the
/// one way this cuts a string, or a dump short of [`BYTE_WINDOW`],
/// without saying so.
fn window_at(process: &Process, addr: u64, buf: &mut [u8; TEXT_WINDOW]) -> Option<usize> {
    if addr < LOWEST_POINTER {
        return None;
    }
    let left = usize::try_from(vad::PAGE - (addr % vad::PAGE)).unwrap_or(TEXT_WINDOW);
    let window = TEXT_WINDOW.min(left);
    process.read(addr, &mut buf[..window]).ok()?;
    Some(window)
}

/// What the bytes at a place read as: a NUL-terminated run of
/// printable characters, one byte each or two, and the characters it
/// has -- or no text at all, and then the bytes themselves where
/// `imports_bytes=` asked for them. None of the three says what the
/// place *is* -- nothing here knows a call's signature -- only what a
/// window at it reads as.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Text {
    /// One byte a character, which is every `*A` call's string and
    /// every C string besides.
    Narrow(usize),
    /// Two, little-endian, which is every `*W` call's.
    Wide(usize),
    /// No text: the bytes read there, [`BYTE_WINDOW`] of them at most,
    /// written as hex. A structure answered by value is this and
    /// nothing else (`imports_bytes=`).
    Bytes(usize),
}

impl Text {
    /// How many characters the run has, which is not how many bytes it
    /// was read from -- or, for a dump, how many bytes there are to
    /// write, which is all it has.
    fn chars(&self) -> usize {
        match *self {
            Text::Narrow(chars) | Text::Wide(chars) | Text::Bytes(chars) => chars,
        }
    }

    /// Whether it reads as text at all, which is what tells a quote
    /// from a dump: the two go in objects of their own, and a reader
    /// asking what a call said is asking a different question from one
    /// asking what it was handed.
    fn is_text(&self) -> bool {
        !matches!(*self, Text::Bytes(_))
    }
}

/// A wide character a name, a path or a message may be written with:
/// not a control character, not a surrogate half, not a noncharacter.
/// What this excludes is what makes a run of them text and an array of
/// small numbers or of pointers not. The whole of the BMP is allowed
/// here, where [`ascii`] allows a narrow run none of it, because a wide
/// run says what it is in the zero byte above each character.
fn printable(c: u32) -> bool {
    (0x20..0x7f).contains(&c) || (0xa0..0xd800).contains(&c) || (0xe000..0xfffe).contains(&c)
}

/// What a window of bytes read at an argument reads as, or None where
/// it reads as no text at all: fewer than [`TEXT_LEAST`] characters, a
/// character that is none, or no terminator inside the window -- a run
/// that goes on past it is as likely a buffer of something else.
///
/// Two bytes a character is tried first, and only where the second byte
/// is zero: that is the one thing that tells `"C:\\"` in UTF-16 from
/// `"C:\\"` in bytes, and a call takes whichever its name ends in.
fn text_in(bytes: &[u8]) -> Option<Text> {
    if bytes.len() >= 2 && bytes[1] == 0 && printable(u32::from(bytes[0])) {
        for (chars, pair) in bytes.chunks_exact(2).enumerate() {
            let c = u32::from(u16::from_le_bytes([pair[0], pair[1]]));
            if c == 0 {
                return (chars >= TEXT_LEAST).then_some(Text::Wide(chars));
            }
            if !printable(c) {
                return None;
            }
        }
        return None;
    }
    for (chars, &b) in bytes.iter().enumerate() {
        if b == 0 {
            return (chars >= TEXT_LEAST).then_some(Text::Narrow(chars));
        }
        if !ascii(b) {
            return None;
        }
    }
    None
}

/// A byte a narrow run may be written with: ASCII and no more. Wider
/// than that a pointer reads as text -- the low bytes of one are a run
/// of high bytes and a zero -- and a narrow argument is the one place
/// that cannot be told apart by a terminator, so the narrow range is
/// the one that has to be narrow. What this gives up is a `*A` string
/// in Latin-1: its accented characters end the run, and the quote is
/// the ASCII before them.
fn ascii(b: u8) -> bool {
    (0x20..0x7f).contains(&b)
}

/// Takes the call's return address away and puts the slot's return trap
/// where the call left it, so that the return faults here as the call
/// did. The real address is kept until then under the thread that made
/// the call: a return is matched to its call by the stack pointer it
/// arrives with, which is also how an unwind past the call is noticed.
fn trap_return(process: &Process, frame: &mut TrapFrame, entry: &Trap, quoted: &[bool; ARGS]) {
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
        args: frame.arg_regs(),
        quoted: *quoted,
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

/// The returns the running thread is still waiting for that an
/// exception is about to unwind through, put back where their calls
/// left them.
///
/// A program handed an exception walks its own frames to find a
/// handler. A trap address is in no image and in no function table, so
/// a frame still holding one is where that walk ends and the handlers
/// above it are never asked: a program that would have caught the
/// exception dies of it instead. So every pending return of this thread
/// in a frame the walk can reach goes back before the exception is
/// delivered, and its call is recorded without an answer -- the
/// program's own unwinding is worth more than one answer, and the
/// summary says how many were given up this way.
///
/// The frame the walk begins in is its own, and the answer is that
/// frame's pc: ntdll reports it by the return address the call that
/// raised left, which is the word that call pushed on x86-64 and the
/// link register on ARM64 -- the slot's return trap, where that call
/// went through an armed slot. A walk from a trap address ends on its
/// first frame, so the call's real return address is answered here as
/// well as put back ([`walked_from`]); 0 where the pc is no trap of
/// ours, which is every hardware fault.
///
/// Asked for by gala's own ntdll as it begins to walk (`GalaImports`
/// op 2), with the stack pointer and the pc of the CONTEXT it walks
/// from: other frames below that stack pointer are the dispatcher's own
/// and hold no return of ours. It has to be the program that asks,
/// because an exception raised in user mode (RtlRaiseException, and
/// with it every C++ throw) is dispatched without a single system call,
/// so no kernel path sees it coming.
pub fn unwinding(process: &Process, sp: u64, pc: u64) -> u64 {
    if ANSWERS_ASKED.load(Ordering::Relaxed) == 0 {
        return 0;
    }
    if WALKS.fetch_add(1, Ordering::Relaxed) < WALKS_SAID {
        let (above, own, below) = waiting(process.id, sp);
        info!(
            "imports: a walk of this thread's frames begins at {sp:#x}: {above} \
             return{} to put back, {own} in the frame it begins in, {below} \
             waiting below it (imports_answers=)",
            if above == 1 { "" } else { "s" }
        );
    }
    loop {
        let mut batch = [None; RESTORE_BATCH];
        let taken = take_above(process.id, sp, &mut batch);
        if taken == 0 {
            break;
        }
        for waited in batch[..taken].iter().flatten() {
            restore(process, sp, waited);
        }
    }
    walked_from(process, sp, pc)
}

/// How many returns the running thread is waiting for above `sp`, which
/// the walk will put back; how many in the frame the walk begins in,
/// whose return would arrive with exactly that stack pointer
/// ([`walked_from`]); and how many below it, which the walk cannot
/// reach at all, a call waiting there being one whose frame is deeper
/// than the walk's own start or is gone already. Said with the walk,
/// for the first [`WALKS_SAID`] of them.
fn waiting(pid: u64, sp: u64) -> (usize, usize, usize) {
    let thread = platform::current();
    let pending = PENDING.lock();
    let (mut above, mut own, mut below) = (0, 0, 0);
    for entry in pending.iter().filter(|e| e.thread == thread && e.pid == pid) {
        if entry.sp > sp {
            above += 1;
        } else if entry.sp == sp {
            own += 1;
        } else {
            below += 1;
        }
    }
    (above, own, below)
}

/// Takes up to `batch.len()` entries of the running thread whose frames
/// an exception at `sp` can reach, innermost first; returns how many.
/// Under the lock and touching none of the program's memory, as every
/// reader of the pending entries is.
fn take_above(pid: u64, sp: u64, batch: &mut [Option<Pending>]) -> usize {
    let thread = platform::current();
    let mut taken = 0;
    let mut pending = PENDING.lock();
    while taken < batch.len() {
        // The newest entry left is the innermost call the thread is in.
        let Some(i) = pending.iter().rposition(|e| e.thread == thread && e.pid == pid && e.sp > sp)
        else {
            break;
        };
        batch[taken] = Some(pending.remove(i));
        taken += 1;
    }
    taken
}

/// Puts one call's return address back where the call left it: the word
/// holding the slot's return trap, looked for from just below the stack
/// pointer the return would have arrived with and down to `floor`,
/// which is the walk's own stack pointer for the frames it crosses and
/// [`RESTORE_WORDS`] words down for the frame it begins in -- that
/// one's trap address is below it, in the frame of the callee that
/// reported the exception at its caller's ([`walked_from`]).
fn restore(process: &Process, floor: u64, waited: &Pending) {
    let want = waited.trap + RET_OFFSET;
    let floor = floor.max(waited.sp.saturating_sub(RESTORE_WORDS * 8));
    let mut at = waited.sp;
    while at > floor {
        at -= 8;
        match process.read_u64(at) {
            Ok(word) if word == want => {
                if process.write_u64(at, waited.ret).is_ok() {
                    if RESTORED.fetch_add(1, Ordering::Relaxed) < RESTORES_SAID {
                        said_restored(at, waited);
                    }
                    return;
                }
                break;
            }
            Ok(_) => {}
            // Off the mapped stack: there is nothing further down to
            // look at.
            Err(_) => break,
        }
    }
    missed(waited);
}

/// One put-back return, named: which call was waiting in the frame the
/// walk is about to cross, and where on the stack its trap address was.
/// The name is read with no lock held over the program's memory, the
/// write having finished.
fn said_restored(at: u64, waited: &Pending) {
    let entry = {
        let traps = TRAPS.lock();
        traps.iter().find(|t| t.trap == waited.trap && t.pid == waited.pid).copied()
    };
    match entry {
        Some(entry) => info!(
            "imports: put back the return {:#x} of the call to {} at {at:#x}, which \
             held the slot's return trap (imports_answers=)",
            waited.ret,
            Name(&entry)
        ),
        None => info!(
            "imports: put back the return {:#x} of the call through the slot trapped \
             at {:#x} at {at:#x} (imports_answers=)",
            waited.ret, waited.trap
        ),
    }
}

/// The return address of the call the walk itself begins in, when the
/// pc it walks from is that call's return trap; 0 when it is not.
///
/// Answered and not only put back, because a walk begins from a pc and
/// not from the stack: the address is handed to ntdll for the CONTEXT
/// it walks from. It goes back in memory as well, in the frame of the
/// callee that reported the exception at its caller's -- the word the
/// call pushed on x86-64, wherever that callee saved the link register
/// on ARM64 -- because the unwind pass that follows the handler search
/// crosses that frame and reads it there. The entry is taken out like
/// a put-back return and the call recorded without an answer for the
/// same reason: the program has the address its return would have
/// faulted at, so no answer can arrive now.
fn walked_from(process: &Process, sp: u64, pc: u64) -> u64 {
    let Some(waited) = take_walked_from(process.id, sp, pc) else {
        return walked_from_nothing(process.id, pc);
    };
    if WALKED_FROM.fetch_add(1, Ordering::Relaxed) < RESTORES_SAID {
        said_walked_from(&waited);
    }
    // And back on the stack as well, below the walk's own stack
    // pointer: the handlers are found by walking from the pc, but the
    // unwind that follows crosses the frame of the callee that raised
    // and reads the word there, where the call left its return address
    // and the kernel left the trap.
    restore(process, 0, &waited);
    waited.ret
}

/// The entry of the running thread whose return would arrive at `sp`
/// with `pc` as its trap: the call the walk begins in. Under the lock
/// and touching none of the program's memory, as every reader of the
/// pending entries is.
fn take_walked_from(pid: u64, sp: u64, pc: u64) -> Option<Pending> {
    let thread = platform::current();
    let mut pending = PENDING.lock();
    let i = pending.iter().rposition(|e| {
        e.thread == thread && e.pid == pid && e.sp == sp && e.trap + RET_OFFSET == pc
    })?;
    Some(pending.remove(i))
}

/// A walk whose pc is a return trap of this process that no call of
/// this thread is waiting for at that stack pointer: said once and
/// counted after that, because the walk ends on its first frame and not
/// one handler of the program is asked. There is no address to answer.
fn walked_from_nothing(pid: u64, pc: u64) -> u64 {
    let entry = {
        let traps = TRAPS.lock();
        traps.iter().find(|t| t.pid == pid && t.trap + RET_OFFSET == pc).copied()
    };
    if let Some(entry) = entry
        && WALKED_MISSED.fetch_add(1, Ordering::Relaxed) == 0
    {
        warn!(
            "imports: a walk of this thread's frames begins at the return trap \
             of {} with no call of this thread waiting for it, so the walk ends \
             on its first frame and no handler is asked; the summary line counts \
             the rest (imports_answers=)",
            Name(&entry)
        );
    }
    0
}

/// The call a walk begins in, named: its return address answered in
/// place of the trap address the walk would have started from. The name
/// is read with no lock over the pending entries held.
fn said_walked_from(waited: &Pending) {
    let entry = {
        let traps = TRAPS.lock();
        traps.iter().find(|t| t.trap == waited.trap && t.pid == waited.pid).copied()
    };
    match entry {
        Some(entry) => info!(
            "imports: the walk begins in the call to {}, whose return trap is the pc \
             it would have walked from: answered with the call's own return address \
             {:#x} and put back where the call left it, and the call recorded \
             without an answer (imports_answers=)",
            Name(&entry),
            waited.ret
        ),
        None => info!(
            "imports: the walk begins in the call through the slot trapped at {:#x}: \
             answered with the call's own return address {:#x} (imports_answers=)",
            waited.trap, waited.ret
        ),
    }
}

/// One pending return that could not be put back. Said once and counted
/// after that, because it is the program's walk that pays: it ends at
/// the trap left in that frame, and an exception the program would have
/// caught goes unhandled.
fn missed(waited: &Pending) {
    if RESTORE_MISSED.fetch_add(1, Ordering::Relaxed) == 0 {
        warn!(
            "imports: the return address of the call through the slot trapped at \
             {:#x} could not be put back for an exception to unwind through its \
             frame: the trap address is not in the {RESTORE_WORDS} words below \
             {:#x}, or could not be written. A walk of that frame ends there \
             (imports_answers=)",
            waited.trap, waited.sp
        );
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
    answer(process, frame, &entry, &waited);
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
/// and its answer read as one pair however they are nested -- and what
/// reads as text now that the call is over, at the answer and at the
/// arguments it was handed that were no text when it was recorded. A
/// buffer the call filled is both of those at once: the answer is
/// often the buffer itself, and the argument it was handed at now
/// points at what the call wrote.
fn answer(process: &Process, frame: &TrapFrame, entry: &Trap, waited: &Pending) {
    if !events::enabled(Category::Call) || !events::syscall_pid_wanted(process.id) {
        return;
    }
    let got = frame.answer();
    let mut at = [None; ARGS + 1];
    at[0] = Some(got);
    for (to, (&arg, &quoted)) in
        at[1..].iter_mut().zip(waited.args.iter().zip(waited.quoted.iter()))
    {
        *to = (!quoted).then_some(arg);
    }
    let mut bytes = [[0u8; TEXT_WINDOW]; ARGS + 1];
    let mut found = [None; ARGS + 1];
    quote(process, &at, &mut bytes, &mut found);
    let text = Texts { places: &RETURN_PLACES, found: &found, bytes: &bytes };
    let name = core::str::from_utf8(&entry.name[..usize::from(entry.len)]).unwrap_or("?");
    let mut owner = [0u8; MODULE_LEN];
    let from = from_name(entry.image, &mut owner);
    events::emit(
        Category::Call,
        "ret",
        format_args!(
            "{}{}",
            Answer { pid: process.id, process: process.name, from, call: name, answer: got },
            text
        ),
    );
    if text.any_text() && TEXT_RETURNS.fetch_add(1, Ordering::Relaxed) < TEXTS_SAID {
        info!("imports: the return of {name} carries text: {} (imports_strings=)", Said(text));
    }
    if text.any_bytes() && BYTE_RETURNS.fetch_add(1, Ordering::Relaxed) < TEXTS_SAID {
        info!("imports: the return of {name} carries bytes: {} (imports_bytes=)", Dumped(text));
    }
    ANSWERED.fetch_add(1, Ordering::Relaxed);
}

/// Where in the region ntdll reserved the trap address of the entry at
/// `index` goes. The loader sizes that region by the same arithmetic
/// (`arm_imports`) and [`region`] refuses one that is short of it, so
/// the two halves cannot drift apart in silence.
fn trap_at(base: u64, index: u64) -> u64 {
    base + index * STRIDE
}

/// Which step of [`put`] failed. The three are not the same thing to
/// the program being traced, which is why a warning names one: the
/// restore is the only step that runs after the write, so it is the
/// only one whose failure leaves the value in the slot.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PutStep {
    /// The page the slot is in could not be made writable.
    Relax,
    /// The slot itself could not be written.
    Write,
    /// The page's protection could not be put back as it was.
    Restore,
}

impl PutStep {
    /// What a warning calls this step.
    fn said(self) -> &'static str {
        match self {
            PutStep::Relax => "its page could not be made writable",
            PutStep::Write => "the slot could not be written",
            PutStep::Restore => "its page's protection could not be put back",
        }
    }

    /// Whether `value` reached the slot. Only the restore runs after
    /// the write, so only its failure leaves the slot holding what was
    /// meant for it -- on a page that stays writable, which the
    /// program can now write to and nothing else is wrong with.
    fn landed(self) -> bool {
        self == PutStep::Restore
    }
}

/// A failed [`put`]: the step and the status it failed with.
#[derive(Clone, Copy)]
struct PutFailed {
    step: PutStep,
    status: NtStatus,
}

/// Puts `value` in the import slot at `slot`. An import address table
/// sits in a read-only part of its image, so the page is made writable,
/// written and put back as it was -- what NtWriteVirtualMemory does for
/// a debugger's write, and what keeps the program's own view of its
/// memory the one it would have had.
fn put(process: &Process, slot: u64, value: u64) -> Result<(), PutFailed> {
    let (old, base, size) = vm::protect(process, slot, 8, vad::PAGE_READWRITE)
        .map_err(|status| PutFailed { step: PutStep::Relax, status })?;
    let written = process.write_u64(slot, value);
    let back = vm::protect(process, base, size, old);
    if written.is_err() {
        // The write faulted, whatever the page's protection says now;
        // the restore above ran whether it did or not, so the page is
        // not left relaxed by this path.
        return Err(PutFailed { step: PutStep::Write, status: NtStatus::ACCESS_VIOLATION });
    }
    back.map(|_| ()).map_err(|status| PutFailed { step: PutStep::Restore, status })
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

/// Whether the returns of calls through armed slots are trapped as
/// well (`imports_answers=`), which is what makes a program's own walk
/// of its frames need [`unwinding`] first. Answered to the loader once,
/// with the rest of op 0's answer, so that a program whose returns are
/// not trapped never asks again.
pub fn answers_trapped() -> bool {
    ANSWERS_ASKED.load(Ordering::Relaxed) != 0
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
        if BYTES_ASKED.load(Ordering::Relaxed) != 0 {
            info!(
                "imports: imports_bytes= was given without imports=, so no \
                 slot was armed and nothing was dumped"
            );
        }
        if EXCEPTING.load(Ordering::Relaxed) {
            info!(
                "imports: imports_except= was given without imports=, so no \
                 slot was armed and nothing was excluded"
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
    let unarmed = UNARMED.load(Ordering::Relaxed);
    if unarmed != 0 {
        info!(
            "imports: {unarmed} imports were left unarmed by imports_except=: they took \
             no slot of the budget, no fault and no record (imports_except=)"
        );
    }
    if armed == 0 {
        info!(
            "imports: no import of {program} from {from} was armed: it never ran, \
             it imports nothing from there, or every slot was a data import (imports=)"
        );
        return;
    }
    if done == 0 && !events::enabled(Category::Call) {
        if let Some(secs) = events::pending(Category::Call) {
            info!(
                "imports: {armed} slots armed and nothing written: the stream does not \
                 start until {secs} s after boot (events_from=) and the run ended first, \
                 so no call was recorded (imports=)"
            );
        } else {
            info!(
                "imports: {armed} slots armed and nothing written: `events=call` was \
                 not given, so no call was recorded (imports=)"
            );
        }
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
    log_images(armed);
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
    log_strings();
    log_bytes();
}

/// How many images one summary line names; the rest are counted.
const SHOWN_IMAGES: usize = 5;

/// Which images the armed slots are in, for the end of a run.
/// `imports=<program>` arms the program's own import table and those of
/// every DLL it loads, so a budget that went on one DLL's imports reads
/// from these counts alone. The `from` of a record says who made a
/// call; this says where the slots were before any call was made, which
/// is the other half of the same question and is said whether or not
/// the event stream was on.
fn log_images(armed: u64) {
    let mut top = [Image::NONE; SHOWN_IMAGES];
    let mut images = 0;
    {
        let held = IMAGES.lock();
        for entry in held.iter().filter(|entry| entry.slots != 0) {
            images += 1;
            // Into a list of the biggest few, which is what the line
            // shows and is cheaper than sorting a table of ninety.
            if let Some(at) = top.iter().position(|had| entry.slots > had.slots) {
                top[at..].rotate_right(1);
                top[at] = *entry;
            }
        }
    }
    if images == 0 {
        return;
    }
    let nameless = NAMELESS.load(Ordering::Relaxed);
    let spread = Spread { shown: &top[..images.min(SHOWN_IMAGES)], images, nameless };
    info!("imports: those {armed} slots are in {spread} (imports=)");
}

/// `2 images: testdll2.dll 2, iat_from_test.exe 1`, the biggest first,
/// with what is not named counted: `87 images: MSO.DLL 4821, ... and 82
/// with fewer`.
struct Spread<'a> {
    shown: &'a [Image],
    images: usize,
    /// Armed slots in no image, which cannot be named at all. Said only
    /// where there are some: it is a gap, and not a number a reader
    /// expects to see.
    nameless: u32,
}

impl fmt::Display for Spread<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let images = self.images;
        write!(f, "{images} image{}: ", if images == 1 { "" } else { "s" })?;
        for (i, entry) in self.shown.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{} {}", entry.name(), entry.slots)?;
        }
        if images > self.shown.len() {
            write!(f, " and {} with fewer", images - self.shown.len())?;
        }
        match self.nameless {
            0 => Ok(()),
            n => write!(f, "; {n} in no image at all"),
        }
    }
}

/// What the run quoted, for the end of a run: the arguments that read
/// as text and the calls they belong to. Said even where nothing was
/// quoted, because that is an answer to the question the option was
/// asked -- the calls recorded carry no text at all -- and not the
/// option having been forgotten.
fn log_strings() {
    let asked = STRINGS_ASKED.load(Ordering::Relaxed);
    if asked == 0 {
        return;
    }
    let quoted = QUOTED.load(Ordering::Relaxed);
    let calls = Calls(TEXT_CALLS.load(Ordering::Relaxed));
    let carried = Carried(TEXT_RETURNS.load(Ordering::Relaxed));
    if quoted == 0 {
        info!(
            "imports: nothing a recorded call was handed, or answered where its \
             return was trapped, read as text: a pointer at a run of printable \
             characters ending in a NUL inside {TEXT_WINDOW} bytes of its own \
             page, {TEXT_LEAST} characters at least (imports_strings=)"
        );
    } else if STRINGS.load(Ordering::Relaxed) == 0 {
        info!(
            "imports: {quoted} quotes of {asked}, the budget spent, carried by \
             {calls}{carried}: the calls past it are recorded with their \
             registers and no text (imports_strings=)"
        );
    } else {
        info!(
            "imports: {quoted} quotes of {asked}, carried by {calls}{carried} \
             (imports_strings=)"
        );
    }
}

/// What the run dumped, for the end of a run: the places that read as
/// no text and the calls they belong to. Said even where nothing was
/// dumped, for [`log_strings`]'s own reason -- that every place a
/// recorded call could be read at held a number or a string is an
/// answer, and not the option having been forgotten.
fn log_bytes() {
    let asked = BYTES_ASKED.load(Ordering::Relaxed);
    if asked == 0 {
        return;
    }
    let dumped = DUMPED.load(Ordering::Relaxed);
    let calls = Calls(BYTE_CALLS.load(Ordering::Relaxed));
    let carried = Carried(BYTE_RETURNS.load(Ordering::Relaxed));
    if dumped == 0 {
        info!(
            "imports: nothing a recorded call was handed, or answered where its \
             return was trapped, was readable and no text: every place was a \
             number too small to be a pointer, an address in no mapping, or \
             text quoted beside this (imports_bytes=)"
        );
    } else if BYTES.load(Ordering::Relaxed) == 0 {
        info!(
            "imports: {dumped} dumps of {asked}, the budget spent, carried by \
             {calls}{carried}: the places past it are recorded with their \
             registers and no bytes (imports_bytes=)"
        );
    } else {
        info!(
            "imports: {dumped} dumps of {asked}, carried by {calls}{carried} \
             (imports_bytes=)"
        );
    }
}

/// ` and <n> returns` for the end of the quoting summary, or nothing
/// where every quote was read at a call: a run with the returns
/// trapped and nothing quoted at one of them has said something, and a
/// run without them must not read as though it had.
struct Carried(u64);

impl fmt::Display for Carried {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            0 => Ok(()),
            1 => f.write_str(" and 1 return"),
            n => write!(f, " and {n} returns"),
        }
    }
}

/// `<n> calls` for the same summaries, one at a time: a run whose
/// quotes or dumps were all carried by one call says so.
struct Calls(u64);

impl fmt::Display for Calls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            1 => f.write_str("1 call"),
            n => write!(f, "{n} calls"),
        }
    }
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
    let restored = RESTORED.load(Ordering::Relaxed);
    if restored != 0 {
        info!(
            "imports: {restored} pending returns were put back where their calls \
             left them, for an exception to unwind through those frames: the \
             calls are recorded without an answer (imports_answers=)"
        );
    }
    let missed = RESTORE_MISSED.load(Ordering::Relaxed);
    if missed != 0 {
        info!(
            "imports: {missed} of those could not be put back, so a walk of \
             those frames ends at the trap address (imports_answers=)"
        );
    }
    let walked = WALKED_FROM.load(Ordering::Relaxed);
    if walked != 0 {
        info!(
            "imports: {walked} walks began in a call through an armed slot and were \
             answered with its return address, the pc they would have walked from \
             being that call's return trap: it is put back where the call left it \
             as well, and the call recorded without an answer (imports_answers=)"
        );
    }
    let missed_walk = WALKED_MISSED.load(Ordering::Relaxed);
    if missed_walk != 0 {
        info!(
            "imports: {missed_walk} walks began at a return trap no call of the \
             thread was waiting for, and ended on their first frame \
             (imports_answers=)"
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
    /// The image whose import slot the call went through, which is who
    /// made it: `call` is what was called, and the DLL it names is the
    /// one that supplies the export rather than the one that asked for
    /// it. `?` where no image holds the slot.
    from: &'a str,
    call: &'a str,
    args: &'a [u64],
    /// What those registers read as where one of them reads as text.
    /// Beside the registers and never in place of them: the number is
    /// what the call was given, the quote is what it points at.
    text: Texts<'a>,
}

impl fmt::Display for Fields<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (pid, args, text) = (self.pid, Words(self.args), self.text);
        let (process, call) = (Str(self.process), Str(self.call));
        let from = Str(self.from);
        write!(
            f,
            r#""pid":{pid},"process":{process},"from":{from},"call":{call},"args":{args}{text}"#
        )
    }
}

/// What one record's places read as, by the place each was read at:
/// `,"text":{"2":"Operation not supported"}` beside `"args"`, then
/// `,"bytes":{"answer":"0123456789abcdef"}` beside that, and nothing at
/// all where nothing was read, which is most records. Two objects and
/// not one because the two are different readings of the same window:
/// a quote is what a call said, a dump is what it was handed.
#[derive(Clone, Copy)]
struct Texts<'a> {
    /// Where each place below is, which is what its key and its line
    /// say: [`CALL_PLACES`] for a call, [`RETURN_PLACES`] for a return.
    places: &'a [Place],
    found: &'a [Option<Text>],
    bytes: &'a [[u8; TEXT_WINDOW]],
}

impl Texts<'_> {
    /// Whether anything read as text, and whether anything read as no
    /// text and was dumped: a record with neither carries nothing here
    /// at all, and the log line each of them has is its own.
    fn any_text(&self) -> bool {
        self.texts().next().is_some()
    }

    fn any_bytes(&self) -> bool {
        self.dumps().next().is_some()
    }

    /// Each place that was read at all: where it was read, the bytes
    /// read there and what they read as.
    fn each(&self) -> impl Iterator<Item = (Place, &[u8], Text)> {
        self.places
            .iter()
            .zip(self.found)
            .zip(self.bytes)
            .filter_map(|((&place, found), bytes)| found.map(|text| (place, &bytes[..], text)))
    }

    /// Those that read as text, and those that read as none.
    fn texts(&self) -> impl Iterator<Item = (Place, &[u8], Text)> {
        self.each().filter(|&(_, _, text)| text.is_text())
    }

    fn dumps(&self) -> impl Iterator<Item = (Place, &[u8], Text)> {
        self.each().filter(|&(_, _, text)| !text.is_text())
    }
}

impl fmt::Display for Texts<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        object(f, r#","text":{"#, self.texts())?;
        object(f, r#","bytes":{"#, self.dumps())
    }
}

/// One of a record's two objects of reads, opened by `open` and keyed
/// by each place, or nothing at all where no place read that way: a
/// record says what it read and never that it read nothing.
fn object<'a>(
    f: &mut fmt::Formatter<'_>,
    open: &str,
    places: impl Iterator<Item = (Place, &'a [u8], Text)>,
) -> fmt::Result {
    let mut written = 0;
    for (place, bytes, text) in places {
        f.write_str(if written == 0 { open } else { "," })?;
        write!(f, r#""{}":{}"#, Key(place), Quote(bytes, text))?;
        written += 1;
    }
    if written == 0 {
        return Ok(());
    }
    f.write_str("}")
}

/// Where one quote was read: an argument register of the call, or the
/// register its answer came back in. Written beside the quote because
/// the same register can be both -- an argument that was a number at
/// the call is a string at its return where the call filled the buffer
/// it points at -- so a quote with no place is one a reader cannot pair
/// with a number.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Place {
    /// The argument register at this index of `TrapFrame::arg_regs`.
    Arg(usize),
    /// The register the ABI answers in, read at a trapped return.
    Answer,
}

impl fmt::Display for Place {
    /// `arg 2`, `answer`: the place for a line of the log.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Place::Arg(i) => write!(f, "arg {i}"),
            Place::Answer => f.write_str("answer"),
        }
    }
}

/// A place as the key it is written under in a record's `text` object:
/// the argument's own index, as every earlier reader of the stream
/// knows it, and `answer` for the answer, which no index collides with.
struct Key(Place);

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Place::Arg(i) => write!(f, "{i}"),
            Place::Answer => f.write_str("answer"),
        }
    }
}

/// The same quotes for a line of the log, where a key and a brace are
/// noise: `arg 2 "Operation not supported"`.
struct Said<'a>(Texts<'a>);

impl fmt::Display for Said<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        line(f, self.0.texts())
    }
}

/// And the same dumps, for the line of their own they have: the two
/// readings are two questions, so a reader of the log is not handed a
/// call's strings and its buffers in one breath.
struct Dumped<'a>(Texts<'a>);

impl fmt::Display for Dumped<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        line(f, self.0.dumps())
    }
}

/// Those places for one line of the log, each after the place it was
/// read at.
fn line<'a>(
    f: &mut fmt::Formatter<'_>,
    places: impl Iterator<Item = (Place, &'a [u8], Text)>,
) -> fmt::Result {
    for (n, (place, bytes, text)) in places.enumerate() {
        let space = if n == 0 { "" } else { " " };
        write!(f, "{space}{place} {}", Quote(bytes, text))?;
    }
    Ok(())
}

/// A window read at a place, written as a JSON string of what it read
/// as: Latin-1 a byte for a narrow run, UTF-16 for a wide one, escaped
/// as every other string of the stream is, and lowercase hex for a
/// window that read as no text at all. A run longer than
/// [`TEXT_CHARS`] ends in `...`, which is the quote saying it was cut
/// and not the program's own text; a dump is [`BYTE_WINDOW`] bytes at
/// most and says its own length by having it.
///
/// Written from the bytes rather than through [`events::Str`] or
/// `events::Utf16` because neither can be handed these: a `&str` would
/// have to be built from bytes that are no UTF-8, and a `&[u16]` from
/// bytes read at an address aligned however the program left it.
struct Quote<'a>(&'a [u8], Text);

impl fmt::Display for Quote<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Quote(bytes, text) = *self;
        f.write_str("\"")?;
        match text {
            Text::Narrow(chars) => {
                for &b in bytes.iter().take(chars.min(TEXT_CHARS)) {
                    escaped(f, char::from(b))?;
                }
            }
            Text::Wide(chars) => {
                let pairs = bytes.chunks_exact(2).take(chars.min(TEXT_CHARS));
                let units = pairs.map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
                for c in char::decode_utf16(units) {
                    escaped(f, c.unwrap_or(char::REPLACEMENT_CHARACTER))?;
                }
            }
            Text::Bytes(len) => {
                for &b in bytes.iter().take(len.min(BYTE_WINDOW)) {
                    write!(f, "{b:02x}")?;
                }
            }
        }
        if text.chars() > TEXT_CHARS {
            f.write_str("...")?;
        }
        f.write_str("\"")
    }
}

/// One character of a JSON string, escaped as [`events::Str`] escapes
/// it. The same three cases, kept here because the bytes a quote is
/// written from are no `&str`.
fn escaped(f: &mut fmt::Formatter<'_>, c: char) -> fmt::Result {
    match c {
        '"' => f.write_str("\\\""),
        '\\' => f.write_str("\\\\"),
        c if (c as u32) < 0x20 => write!(f, "\\u{:04x}", c as u32),
        c => write!(f, "{c}"),
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
        ARGS, BUDGET, BYTE_WINDOW, CALL_PLACES, Calls, Carried, Dumped, Fields, Image, MAX_ANSWERS,
        MAX_BUDGET, MAX_BYTES, MAX_REPEAT, MAX_STRINGS, PutStep, Quote, READ, RET_OFFSET,
        RETURN_PLACES, Reads, SHOWN_IMAGES, STRIDE, Said, Spread, TEXT_CHARS, TEXT_WINDOW, Text,
        Texts, Trap, answer_budget, byte_budget, holds, repeat_budget, split_budget, string_budget,
        text_in, trap_at,
    };

    #[test]
    fn only_a_failed_restore_leaves_the_value_in_the_slot() {
        // The restore is the only step after the write, so it is the
        // only one whose failure the program sees as a slot that holds
        // what was meant for it.
        assert!(!PutStep::Relax.landed());
        assert!(!PutStep::Write.landed());
        assert!(PutStep::Restore.landed());
        // And each failure says which step it was: a warning that
        // named none of them is what made a failed restore read as a
        // fault a call.
        let said = [PutStep::Relax.said(), PutStep::Write.said(), PutStep::Restore.said()];
        for (i, one) in said.iter().enumerate() {
            assert!(!one.is_empty());
            assert!(!said[..i].contains(one), "{one} is said of two steps");
        }
    }

    /// The reads of one record built as the fault path builds them,
    /// from what the window at each place holds: `(place, bytes)` by
    /// the place's own index in the record's list of them
    /// ([`CALL_PLACES`], [`RETURN_PLACES`]). Text where the window
    /// reads as text and the bytes themselves where it does not, which
    /// is `quote` with both budgets given; the window is the whole of
    /// one, as the fault path's is, so a dump of it is
    /// [`BYTE_WINDOW`] bytes however few were put there.
    fn reads<const N: usize>(at: &[(usize, &[u8])]) -> ([Option<Text>; N], [[u8; TEXT_WINDOW]; N]) {
        let mut found = [None; N];
        let mut bytes = [[0u8; TEXT_WINDOW]; N];
        for &(i, raw) in at {
            bytes[i][..raw.len()].copy_from_slice(raw);
            found[i] = Some(text_in(&bytes[i]).unwrap_or(Text::Bytes(BYTE_WINDOW)));
        }
        (found, bytes)
    }

    /// `<n>` bytes of a dump that were never written, which a window
    /// read past what a callee filled in holds: the caller's own
    /// zeroes, as the test program's buffer has them.
    fn zeroes(n: usize) -> alloc::string::String {
        "00".repeat(n)
    }

    /// The same bytes a `*W` call would be handed: UTF-16, little
    /// endian, with its terminator.
    fn wide(text: &str) -> alloc::vec::Vec<u8> {
        text.encode_utf16().chain(core::iter::once(0)).flat_map(u16::to_le_bytes).collect()
    }

    /// `imports_except=`'s texts are substrings of an import's
    /// `<dll>!<name>` and not patterns, which is what lets one of them
    /// name a family of imports; the comparison ignores case, as a file
    /// name's does.
    #[test]
    fn an_excepted_text_is_a_substring_of_an_import_name() {
        let name = b"api-ms-win-crt-heap-l1-1-0.dll!malloc";
        assert!(holds(name, b"api-ms-win-crt-"));
        assert!(holds(name, b"API-MS-WIN-CRT-HEAP-L1-1-0.DLL!MALLOC"));
        assert!(holds(name, b"!malloc"));
        assert!(holds(name, b"HEAP"));
        assert!(!holds(name, b"ntdll.dll!Rtl"));
        // The whole name and a byte more: a text longer than the name
        // holds nothing, and the window must not be taken past its end.
        assert!(!holds(name, b"api-ms-win-crt-heap-l1-1-0.dll!malloc!"));
        // The empty text, which `set_except` refuses: it would
        // otherwise match every name and arm nothing at all.
        assert!(!holds(name, b""));
        assert!(!holds(b"", b"x"));
    }

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
        let (found, bytes) = reads::<ARGS>(&[]);
        let fields = Fields {
            pid: 12,
            process: "WINWORD.EXE",
            from: "wwlib.dll",
            call: "d2d1.dll!D2D1CreateFactory",
            args: &[0, 2078216, 0, 2078208],
            text: Texts { places: &CALL_PLACES, found: &found, bytes: &bytes },
        };
        assert_eq!(
            alloc::format!("{{\"ev\":\"import\",{fields}}}"),
            "{\"ev\":\"import\",\"pid\":12,\"process\":\"WINWORD.EXE\",\"from\":\
             \"wwlib.dll\",\"call\":\"d2d1.dll!D2D1CreateFactory\",\"args\":\
             [0,2078216,0,2078208]}"
        );
    }

    /// An image of `slots` armed slots, for the spread below.
    fn image(name: &str, slots: u32) -> Image {
        let mut entry = Image { slots, ..Image::NONE };
        entry.len = name.len() as u8;
        entry.name[..name.len()].copy_from_slice(name.as_bytes());
        entry
    }

    /// Where a run's armed slots were, which is the other half of what
    /// a record's `from` says: the biggest few images named, the rest
    /// counted so that the line is bounded however many images a
    /// program loads, and the slots no image holds said only where
    /// there are some.
    #[test]
    fn the_images_of_a_run_are_the_biggest_few_and_the_rest_counted() {
        let two = [image("testdll2.dll", 2), image("iat_from_test.exe", 1)];
        assert_eq!(
            alloc::format!("{}", Spread { shown: &two, images: 2, nameless: 0 }),
            "2 images: testdll2.dll 2, iat_from_test.exe 1"
        );
        // One image is one image: a program whose imports are all its
        // own is the common case of that, and it must not read as a
        // count that has gone wrong.
        assert_eq!(
            alloc::format!("{}", Spread { shown: &two[..1], images: 1, nameless: 0 }),
            "1 image: testdll2.dll 2"
        );
        let many: [Image; SHOWN_IMAGES] =
            core::array::from_fn(|i| image("MSO.DLL", 5000 - i as u32));
        let line = alloc::format!("{}", Spread { shown: &many, images: 87, nameless: 0 });
        assert!(line.starts_with("87 images: MSO.DLL 5000, "), "{line}");
        assert!(line.ends_with(" and 82 with fewer"), "{line}");
        // A slot in no image at all is a gap in the instrument, so it
        // is said as a number of its own rather than left out.
        assert_eq!(
            alloc::format!("{}", Spread { shown: &two, images: 2, nameless: 3 }),
            "2 images: testdll2.dll 2, iat_from_test.exe 1; 3 in no image at all"
        );
    }

    /// Knowing which image a slot is in costs no memory: the index sits
    /// in padding the entry has for its alignment anyway, so the whole
    /// ceiling `imports=<n>` may ask for still fits the budget the
    /// docs name (9 MiB).
    #[test]
    fn a_slot_costs_no_more_for_knowing_which_image_it_is_in() {
        assert_eq!(size_of::<Trap>(), 136);
        let table = size_of::<Trap>() * MAX_BUDGET as usize;
        assert!(table <= 9 * 1024 * 1024, "{table} bytes for {MAX_BUDGET} slots");
    }

    /// What a window of bytes read at an argument reads as. A run of
    /// printable characters that ends inside the window is text; an
    /// array of small numbers, a pointer, a buffer with no terminator
    /// in reach and a run too short to mean anything are not, and a
    /// reader must not be shown a quote of them.
    #[test]
    fn an_argument_reads_as_text_or_as_nothing() {
        assert_eq!(text_in(b"C:\\Windows\0rest"), Some(Text::Narrow(10)));
        assert_eq!(text_in(&wide("Document1 - Word")), Some(Text::Wide(16)));
        // A narrow run whose first character is printable and whose
        // second byte happens to be zero is one character of UTF-16,
        // which is how every short `*A` string looks: it is told from a
        // wide one by its own terminator and not by the call's name, so
        // a run of one character is no text either way.
        assert_eq!(text_in(b"A\0\0\0"), None);
        // The ledger's own numbers, as an array of flags would be.
        assert_eq!(text_in(&[1, 0, 0, 0, 2, 0, 0, 0]), None);
        // A pointer, which is what most arguments are: its low bytes
        // are a run of high bytes, which is why a narrow run is ASCII.
        assert_eq!(text_in(&0x3ff7_ffeb_f8a8u64.to_le_bytes()), None);
        // Printable to the last byte: as likely a buffer of something
        // else, and a quote of it would be cut without saying so.
        assert_eq!(text_in(b"abcdefgh"), None);
        assert_eq!(text_in(b"ab\0"), None);
        assert_eq!(text_in(b"abcd\0"), Some(Text::Narrow(4)));
        // A control character, a surrogate half and a noncharacter are
        // each what tells a run of bytes from text.
        assert_eq!(text_in(b"ab\tcd\0"), None);
        assert_eq!(text_in(&[0x41, 0, 0x42, 0, 0x00, 0xd8, 0, 0, 0, 0]), None);
        assert_eq!(text_in(&[0x41, 0, 0x42, 0, 0xfe, 0xff, 0, 0, 0, 0]), None);
    }

    /// A quote is a JSON string of what the window read as, escaped as
    /// every other string of the stream is, and a run longer than the
    /// line is worth says so itself rather than ending mid-word.
    #[test]
    fn a_quote_is_escaped_and_says_when_it_was_cut() {
        let raw = b"say \"no\\yes\"\0";
        let text = text_in(raw).expect("a run of printable characters");
        assert_eq!(alloc::format!("{}", Quote(raw, text)), r#""say \"no\\yes\"""#);
        let long = alloc::format!("{}!\0", "x".repeat(TEXT_CHARS));
        let text = text_in(long.as_bytes()).expect("a long run");
        let quoted = alloc::format!("{}", Quote(long.as_bytes(), text));
        assert_eq!(quoted, alloc::format!("\"{}...\"", "x".repeat(TEXT_CHARS)));
    }

    /// The quotes go beside the registers and never in place of them,
    /// by the index of the argument each was read at, and a call whose
    /// arguments are all numbers carries no `text` at all -- which is
    /// most calls, and the shape every earlier reader of the stream
    /// already knows.
    #[test]
    fn a_call_s_text_is_keyed_by_the_argument_it_was_read_at() {
        let (found, bytes) =
            reads::<ARGS>(&[(1, b"ErrorMessage\0"), (2, &wide("Mso20win32client"))]);
        let text = Texts { places: &CALL_PLACES, found: &found, bytes: &bytes };
        let shown = alloc::format!("{text}");
        assert_eq!(shown, r#","text":{"1":"ErrorMessage","2":"Mso20win32client"}"#);
        let said = alloc::format!("{}", Said(text));
        assert_eq!(said, r#"arg 1 "ErrorMessage" arg 2 "Mso20win32client""#);
        let (found, bytes) = reads::<ARGS>(&[]);
        let text = Texts { places: &CALL_PLACES, found: &found, bytes: &bytes };
        assert_eq!(alloc::format!("{text}"), "");
        assert_eq!(alloc::format!("{}", Said(text)), "");
    }

    /// A return's quotes are keyed by their places too, and its first
    /// place is the answer: a call that fills a buffer and answers it
    /// carries the same string twice, at the answer and at the argument
    /// the buffer was handed at, so the place and not the value is what
    /// tells the two apart. The order is the answer first, the
    /// arguments after it, which is the order a reader of the call
    /// already has them in.
    #[test]
    fn a_return_s_text_is_keyed_by_the_place_it_was_read_at() {
        let filled = b"filled at the return\0";
        let (found, bytes) = reads::<{ ARGS + 1 }>(&[(0, filled), (1, filled)]);
        let text = Texts { places: &RETURN_PLACES, found: &found, bytes: &bytes };
        assert_eq!(
            alloc::format!("{text}"),
            r#","text":{"answer":"filled at the return","0":"filled at the return"}"#
        );
        assert_eq!(
            alloc::format!("{}", Said(text)),
            r#"answer "filled at the return" arg 0 "filled at the return""#
        );
        // An answer that is a number and arguments that were text at
        // the call: the return read nothing, and says nothing.
        let (found, bytes) = reads::<{ ARGS + 1 }>(&[]);
        let text = Texts { places: &RETURN_PLACES, found: &found, bytes: &bytes };
        assert_eq!(alloc::format!("{text}"), "");
        assert_eq!(alloc::format!("{}", Said(text)), "");
    }

    /// What the quoting summary says about where the quotes were read:
    /// nothing where every one was read at a call, so that a run
    /// without the returns trapped reads as it did before there was
    /// anything to read at one.
    #[test]
    fn the_returns_that_carried_text_are_counted_only_where_there_are_some() {
        assert_eq!(alloc::format!("{}", Carried(0)), "");
        assert_eq!(alloc::format!("{}", Carried(1)), " and 1 return");
        assert_eq!(alloc::format!("{}", Carried(134)), " and 134 returns");
    }

    /// `imports_strings=` takes a number of quotes, by the same rule as
    /// the budgets beside it: a number out of range is refused outright
    /// rather than read as a smaller one, because a trace with fewer
    /// quotes than were asked for reads as calls whose arguments were
    /// no text.
    /// A place that reads as no text is dumped as the bytes it holds,
    /// in an object of its own and keyed by the same place: so one
    /// record carries a quote of one argument and a dump of another,
    /// and each has the log line of its own that says which it is.
    #[test]
    fn a_place_that_is_no_text_is_dumped_beside_the_quotes() {
        let (found, bytes) = reads::<ARGS>(&[(1, b"ErrorMessage\0"), (3, &[1, 2, 3, 4, 5, 6])]);
        let text = Texts { places: &CALL_PLACES, found: &found, bytes: &bytes };
        let hex = alloc::format!("010203040506{}", zeroes(BYTE_WINDOW - 6));
        assert_eq!(
            alloc::format!("{text}"),
            alloc::format!(r#","text":{{"1":"ErrorMessage"}},"bytes":{{"3":"{hex}"}}"#)
        );
        assert_eq!(alloc::format!("{}", Said(text)), r#"arg 1 "ErrorMessage""#);
        assert_eq!(alloc::format!("{}", Dumped(text)), alloc::format!(r#"arg 3 "{hex}""#));
    }

    /// The structure a call answers by value, which is the one reading
    /// there is of such a call: the answer and the argument the buffer
    /// was handed at are the same pointer, so the dump is the same
    /// bytes twice and the place is what tells them apart -- and
    /// nothing of it reads as text, which is why quoting such a call
    /// says nothing at all (docs/ROADMAP.md, Word's startup).
    #[test]
    fn a_structure_answered_by_value_is_dumped_at_the_answer_and_at_its_argument() {
        let structure = [0x01u8, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
        assert_eq!(text_in(&structure), None);
        let (found, bytes) = reads::<{ ARGS + 1 }>(&[(0, &structure), (1, &structure)]);
        let text = Texts { places: &RETURN_PLACES, found: &found, bytes: &bytes };
        let hex = alloc::format!("0123456789abcdef{}", zeroes(BYTE_WINDOW - 8));
        assert_eq!(
            alloc::format!("{text}"),
            alloc::format!(r#","bytes":{{"answer":"{hex}","0":"{hex}"}}"#)
        );
        assert_eq!(
            alloc::format!("{}", Dumped(text)),
            alloc::format!(r#"answer "{hex}" arg 0 "{hex}""#)
        );
        assert_eq!(alloc::format!("{}", Said(text)), "");
    }

    /// The calls a summary line counts are counted one at a time: a
    /// run whose quotes or dumps were all carried by one call must not
    /// read as "1 calls".
    #[test]
    fn the_calls_that_carried_something_are_counted_one_at_a_time() {
        assert_eq!(alloc::format!("{}", Calls(0)), "0 calls");
        assert_eq!(alloc::format!("{}", Calls(1)), "1 call");
        assert_eq!(alloc::format!("{}", Calls(134)), "134 calls");
    }

    /// `imports_bytes=` takes a number of dumps, by `imports_strings=`'s
    /// own rule: a number out of range is refused outright rather than
    /// read as a smaller one.
    #[test]
    fn a_byte_budget_is_a_number_of_dumps_or_nothing() {
        assert_eq!(byte_budget("8"), Some(8));
        assert_eq!(byte_budget(&alloc::format!("{MAX_BYTES}")), Some(MAX_BYTES));
        assert_eq!(byte_budget("0"), None);
        assert_eq!(byte_budget(&alloc::format!("{}", u64::from(MAX_BYTES) + 1)), None);
        assert_eq!(byte_budget("32 bytes"), None);
    }

    #[test]
    fn a_string_budget_is_a_number_of_quotes_or_nothing() {
        assert_eq!(string_budget("8"), Some(8));
        assert_eq!(string_budget(&alloc::format!("{MAX_STRINGS}")), Some(MAX_STRINGS));
        assert_eq!(string_budget("0"), None);
        assert_eq!(string_budget(&alloc::format!("{}", u64::from(MAX_STRINGS) + 1)), None);
        assert_eq!(string_budget("all"), None);
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
