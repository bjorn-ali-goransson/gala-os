//! Programs across a kernel handover (ROADMAP R9 milestones (b) and (c),
//! ARCHITECTURE "Kernel handover"): the record's formats for processes,
//! their address spaces, threads as data, objects and handle tables.
//! Plain data, host-tested; `carry` fills them from the running kernel
//! and rebuilds the kernel's objects from them.
//!
//! Every section here is REQUIRED (a kernel that does not know one would
//! lose programs), at the version `handover::version::KINDS` gives (a
//! version-1 entry reads with the newer fields 0, their defaults). Lists
//! are entries (`Writer::begin_entry`:
//! a length, so an entry gains fields at its end) or tables (count,
//! stride, fixed records that gain fields at their end). Objects are named
//! by ids (u32, unique in the record, 0 = none); each kind of object has a
//! section of its own, so a kernel that does not know a kind refuses the
//! record before it starts instead of losing the object.
//!
//! ```text
//! PROCESSES      a header entry (the next NT id, u64), then an entry per
//!                process (ProcessRec)
//! ADDRESS_SPACES an entry per process: pid, its VAD (nt::vad), a table of
//!                section views (ViewRec), a table of pages (PageRec)
//! THREADS        an entry per live thread (ThreadRec, its APCs' table)
//! EVENTS ...     an entry per object (EventRec, SemaphoreRec, ...,
//!                ImageSectionRec, FileSectionRec, PipeEndRec, TokenRec)
//! PIPES          an entry per pipe the pipe ends share (PipeRec)
//! SECURITY       an entry per object with a security descriptor
//! HANDLES        an entry per process: pid, a table of handles (HandleRec)
//! USER_FRAMES    ranges of the frames of OWNED pages and section pages
//! ```

use super::record::{REQUIRED, Reader, Wide, Writer};
use super::version;
use crate::mm::heap::AllocError;
use crate::mm::paging::{CacheMode, PageFlags};

/// The version of section `tag` this kernel writes
/// (`handover::version::KINDS`).
pub fn version_of(tag: u32) -> u16 {
    version::writes(tag)
}

/// Starts one of this module's sections, at this kernel's version and
/// with its `must_read`.
pub fn section(w: &mut Writer, tag: u32) -> Result<(), AllocError> {
    let must_read = version::kind_of(tag).map_or(0, |k| k.must_read);
    w.section_needing(tag, version_of(tag), REQUIRED, must_read)
}

/// THREADS' `must_read`: a thread whose extended state (XSAVE beyond the
/// legacy FXSAVE area) is in its entry's blob would lose it in a reader
/// older than version 3, which stops before the blob.
pub fn threads_must_read(any_extended_state: bool) -> u16 {
    let k = version::kind_of(super::record::tag::THREADS).map_or(0, |k| k.must_read);
    if any_extended_state { k.max(3) } else { k }
}

/// A PROCESSES entry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcessRec<'a> {
    pub id: u64,
    pub parent_id: u64,
    pub create_time: u64,
    /// The executable's mapping (base, size).
    pub image_base: u64,
    pub image_size: u64,
    pub exception_dispatcher: u64,
    pub apc_dispatcher: u64,
    pub threads_created: u32,
    pub priority_class: u32,
    pub name: &'a str,
    /// The executable's Win32 path (the next kernel lays it out again).
    pub path: &'a str,
    /// The digest of its layout (`ImageLayout::digest`; version 2, 0 from
    /// an older writer: not checked).
    pub image_digest: u64,
    /// Its primary token's object id (TOKENS; version 2, 0 from an older
    /// writer: the boot user's, made again).
    pub token: u32,
    /// The handovers the process has come through, this one included
    /// (version 3, the `handover_next` build: 0 from an older writer, not
    /// counted).
    pub handovers: u32,
}

impl<'a> ProcessRec<'a> {
    /// The entry at this kernel's PROCESSES version.
    pub fn write(&self, w: &mut Writer) -> Result<(), AllocError> {
        self.write_as(w, version_of(super::record::tag::PROCESSES))
    }

    /// The entry at `version` (2: no handover count).
    pub fn write_as(&self, w: &mut Writer, version: u16) -> Result<(), AllocError> {
        let e = w.begin_entry()?;
        for v in [
            self.id,
            self.parent_id,
            self.create_time,
            self.image_base,
            self.image_size,
            self.exception_dispatcher,
            self.apc_dispatcher,
        ] {
            w.u64(v)?;
        }
        w.u32(self.threads_created)?;
        w.u32(self.priority_class)?;
        w.str(self.name)?;
        w.str(self.path)?;
        w.u64(self.image_digest)?;
        w.u32(self.token)?;
        w.u32(0)?;
        if version >= 3 {
            w.u32(self.handovers)?;
            w.u32(0)?;
        }
        w.end_entry(e)
    }

    /// An entry, as this kernel reads PROCESSES.
    pub fn read(r: &mut Reader<'a>) -> Self {
        Self::read_as(r, version_of(super::record::tag::PROCESSES))
    }

    /// An entry, as a kernel whose newest PROCESSES is `knows` reads it:
    /// fields of later versions are not read; fields the writer did not
    /// have read as 0.
    pub fn read_as(r: &mut Reader<'a>, knows: u16) -> Self {
        let mut p = ProcessRec {
            id: r.u64(),
            parent_id: r.u64(),
            create_time: r.u64(),
            image_base: r.u64(),
            image_size: r.u64(),
            exception_dispatcher: r.u64(),
            apc_dispatcher: r.u64(),
            threads_created: r.u32(),
            priority_class: r.u32(),
            name: r.str(),
            path: r.str(),
            image_digest: r.u64(),
            token: r.u32(),
            handovers: 0,
        };
        let _ = r.u32();
        if knows >= 3 {
            p.handovers = r.u32();
        }
        p
    }
}

/// A section view in an ADDRESS_SPACES entry (32-byte records).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewRec {
    pub base: u64,
    pub size: u64,
    /// Byte offset of the view in the section.
    pub offset: u64,
    /// The section object's id.
    pub section: u32,
}

impl ViewRec {
    pub const STRIDE: u32 = 32;

    pub fn write(&self, w: &mut Writer) -> Result<(), AllocError> {
        w.u64(self.base)?;
        w.u64(self.size)?;
        w.u64(self.offset)?;
        w.u32(self.section)?;
        w.u32(0)
    }

    pub fn read(r: &mut Reader<'_>) -> Self {
        ViewRec { base: r.u64(), size: r.u64(), offset: r.u64(), section: r.u32() }
    }
}

/// What backs a mapped user page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageKind {
    /// The process's own frame (value: the physical frame number), kept
    /// where it is.
    Owned,
    /// The executable's unwritten page (value: its page index in the
    /// image), laid out again by the next kernel.
    Image,
    /// KUSER_SHARED_DATA (the next kernel's).
    SharedData,
    /// A page of a section view (the frame is the section's).
    Section,
    /// An unwritten page of an image view (a DLL): the image section's
    /// shared frame for the view's base, laid out again by the next
    /// kernel (value: the page's index in the view).
    ImageView,
}

impl PageKind {
    fn number(self) -> u32 {
        match self {
            PageKind::Owned => 0,
            PageKind::Image => 1,
            PageKind::SharedData => 2,
            PageKind::Section => 3,
            PageKind::ImageView => 4,
        }
    }

    fn from_number(n: u32) -> Option<Self> {
        Some(match n {
            0 => PageKind::Owned,
            1 => PageKind::Image,
            2 => PageKind::SharedData,
            3 => PageKind::Section,
            4 => PageKind::ImageView,
            _ => return None,
        })
    }
}

/// A mapped user page in an ADDRESS_SPACES entry (24-byte records).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRec {
    pub va: u64,
    /// The frame number (Owned) or page index (Image); else 0.
    pub value: u64,
    pub kind: PageKind,
    pub flags: PageFlags,
}

/// Page flags in a record: bit 0 writable, 1 executable, 2 user, bits 8-10
/// the cache mode (PAT index). Global and borrowed follow from the kind.
pub fn encode_flags(f: PageFlags) -> u32 {
    u32::from(f.writable) | u32::from(f.executable) << 1 | u32::from(f.user) << 2 | {
        (f.cache as u32) << 8
    }
}

pub fn decode_flags(bits: u32) -> PageFlags {
    PageFlags {
        writable: bits & 1 != 0,
        executable: bits & 2 != 0,
        user: bits & 4 != 0,
        global: false,
        cache: CacheMode::from_index(u64::from(bits >> 8 & 7)),
        borrowed: false,
    }
}

impl PageRec {
    pub const STRIDE: u32 = 24;

    pub fn write(&self, w: &mut Writer) -> Result<(), AllocError> {
        w.u64(self.va)?;
        w.u64(self.value)?;
        w.u32(self.kind.number())?;
        w.u32(encode_flags(self.flags))
    }

    /// None for a kind this kernel does not know (a damaged record: a
    /// newer kind comes with a new section version).
    pub fn read(r: &mut Reader<'_>) -> Option<Self> {
        let (va, value, kind, flags) = (r.u64(), r.u64(), r.u32(), r.u32());
        Some(PageRec { va, value, kind: PageKind::from_number(kind)?, flags: decode_flags(flags) })
    }
}

/// The user registers a thread made its system call with, in the order
/// of `UserRegs` (16 general-purpose registers, rip, rflags).
pub type Regs = [u64; 18];

/// How a carried thread stood (ThreadRec::state).
pub mod thread_state {
    /// Blocked in system call `number`, made again (version 1's only kind).
    pub const SYSCALL: u32 = 0;
    /// Stopped in user mode: goes on with `regs` and `fx`.
    pub const USER: u32 = 1;
    /// Created suspended, never run: starts with `regs` (and `mxcsr`).
    pub const START: u32 = 2;
}

/// A queued user APC in a THREADS entry (32-byte records): routine, then
/// its three arguments.
pub type ApcRec = [u64; 4];

/// A THREADS entry: a live thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadRec<'a> {
    pub tid: u64,
    pub pid: u64,
    pub teb: u64,
    pub win32_start: u64,
    pub create_time: u64,
    pub affinity: u64,
    /// The thread object's id.
    pub object: u32,
    pub ordinal: u32,
    pub priority: i32,
    pub base_priority: i32,
    /// The system call it is blocked in, and its registers at the
    /// `syscall` (its arguments).
    pub number: u32,
    pub regs: Regs,
    /// The call's deadline (timer tick), kept absolute; None: none.
    pub deadline: Option<u64>,
    /// Its x87/SSE state (FXSAVE layout): the whole extended state, as
    /// gala does not enable AVX (XSAVE).
    pub fx: [u8; 512],
    /// Version 2: `thread_state` (0 from an older writer: SYSCALL).
    pub state: u32,
    pub suspend: u32,
    /// START: the MXCSR it starts with, if set.
    pub mxcsr: Option<u32>,
    /// The impersonation token's object id (TOKENS), 0: none.
    pub impersonation: u32,
    /// Version 3: the extended state's components (XCR0 bits; 0 from an
    /// older writer: x87 and SSE, all in `fx`) and the XSAVE area past the
    /// legacy 512 bytes (header and components, standard format; empty
    /// while gala uses FXSAVE). Length-prefixed, so a kernel that enables
    /// XSAVE carries AVX and later state without changing the format.
    pub xstate: u64,
    pub extended: &'a [u8],
}

/// THREADS `xstate`: x87 and SSE, the legacy FXSAVE area.
pub const XSTATE_LEGACY: u64 = 0b11;

impl<'a> ThreadRec<'a> {
    /// The entry, then its queued APCs (a table of `ApcRec`), then (version
    /// 3) its extended state.
    pub fn write(&self, w: &mut Writer, apcs: &[ApcRec]) -> Result<(), AllocError> {
        self.write_as(w, apcs, version_of(super::record::tag::THREADS))
    }

    /// The entry at `version` (2: no extended state).
    pub fn write_as(
        &self,
        w: &mut Writer,
        apcs: &[ApcRec],
        version: u16,
    ) -> Result<(), AllocError> {
        let e = w.begin_entry()?;
        for v in [self.tid, self.pid, self.teb, self.win32_start, self.create_time, self.affinity] {
            w.u64(v)?;
        }
        w.u32(self.object)?;
        w.u32(self.ordinal)?;
        w.u32(self.priority as u32)?;
        w.u32(self.base_priority as u32)?;
        w.u32(self.number)?;
        w.u32(0)?;
        for v in self.regs {
            w.u64(v)?;
        }
        w.u64(self.deadline.map_or(0, |d| d.saturating_add(1)))?;
        w.bytes(&self.fx)?;
        w.u32(self.state)?;
        w.u32(self.suspend)?;
        w.u32(u32::from(self.mxcsr.is_some()))?;
        w.u32(self.mxcsr.unwrap_or(0))?;
        w.u32(self.impersonation)?;
        w.u32(0)?;
        w.table(apcs.len() as u32, 32)?;
        for apc in apcs {
            for v in apc {
                w.u64(*v)?;
            }
        }
        if version >= 3 {
            w.u64(self.xstate)?;
            w.blob(self.extended)?;
        }
        w.end_entry(e)
    }

    /// The entry and its APCs' table.
    pub fn read(r: &mut Reader<'a>) -> (Self, super::record::Table<'a>) {
        let (tid, pid, teb, win32_start, create_time, affinity) =
            (r.u64(), r.u64(), r.u64(), r.u64(), r.u64(), r.u64());
        let (object, ordinal, priority, base_priority, number) =
            (r.u32(), r.u32(), r.u32() as i32, r.u32() as i32, r.u32());
        let _ = r.u32();
        let mut regs = [0u64; 18];
        for v in regs.iter_mut() {
            *v = r.u64();
        }
        let deadline = r.u64().checked_sub(1);
        let fx = r.array();
        let (state, suspend, has_mxcsr, mxcsr) = (r.u32(), r.u32(), r.u32(), r.u32());
        let (impersonation, _) = (r.u32(), r.u32());
        let apcs = r.table();
        let xstate = match r.u64() {
            0 => XSTATE_LEGACY,
            x => x,
        };
        let extended = r.blob();
        let rec = ThreadRec {
            tid,
            pid,
            teb,
            win32_start,
            create_time,
            affinity,
            object,
            ordinal,
            priority,
            base_priority,
            number,
            regs,
            deadline,
            fx,
            state,
            suspend,
            mxcsr: (has_mxcsr != 0).then_some(mxcsr),
            impersonation,
            xstate,
            extended,
        };
        (rec, apcs)
    }
}

/// Reads an `ApcRec` from a table record.
pub fn read_apc(r: &mut Reader<'_>) -> ApcRec {
    [r.u64(), r.u64(), r.u64(), r.u64()]
}

/// The start of every object entry: its id and its name in the object
/// namespace (empty: unnamed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectHead<'a> {
    pub id: u32,
    pub name: Wide<'a>,
}

impl<'a> ObjectHead<'a> {
    fn write(&self, w: &mut Writer, name: &[u16]) -> Result<(), AllocError> {
        w.u32(self.id)?;
        w.wide(name)
    }

    fn read(r: &mut Reader<'a>) -> Self {
        ObjectHead { id: r.u32(), name: r.wide() }
    }
}

/// An EVENTS entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventRec<'a> {
    pub head: ObjectHead<'a>,
    /// 0 notification (manual reset), 1 synchronization (auto reset).
    pub kind: u32,
    pub signal: i32,
}

/// A SEMAPHORES entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SemaphoreRec<'a> {
    pub head: ObjectHead<'a>,
    pub count: i32,
    pub maximum: i32,
}

/// A MUTANTS entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MutantRec<'a> {
    pub head: ObjectHead<'a>,
    /// The owning thread's id (0: free).
    pub owner: u64,
    pub count: u32,
    pub abandoned: bool,
}

/// A TIMERS entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimerRec<'a> {
    pub head: ObjectHead<'a>,
    pub synchronization: bool,
    pub signal: i32,
    /// The tick it expires at next, if armed (absolute).
    pub due: Option<u64>,
    /// NT system time it was set for; period in ticks.
    pub due_time: u64,
    pub period: u64,
    /// Version 2: the APC queued when it expires: (thread object id,
    /// routine, context); None from an older writer.
    pub apc: Option<(u32, u64, u64)>,
}

/// A SECTIONS entry (a pagefile-backed section) and its frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SectionRec<'a> {
    pub head: ObjectHead<'a>,
    pub size: u64,
    pub protect: u32,
    pub attributes: u32,
}

/// A FILES entry: a file or directory on C:, opened again by path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileRec<'a> {
    pub id: u32,
    pub granted: u32,
    pub share: u32,
    /// CreateOptions to open it with again (mode, directory or not).
    pub options: u32,
    pub position: u64,
    /// From the volume root (`\gala\x.exe`).
    pub path: Wide<'a>,
}

/// A completion packet of an IO_COMPLETIONS entry (32-byte records).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PacketRec {
    pub key: u64,
    pub context: u64,
    pub information: u64,
    pub status: u32,
}

/// An EXITED_THREADS entry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExitedThreadRec {
    pub id: u32,
    pub exit_status: u32,
    pub tid: u64,
    pub pid: u64,
    pub teb: u64,
    pub win32_start: u64,
    pub create_time: u64,
    pub exit_time: u64,
}

/// A handle in a HANDLES entry (24-byte records).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HandleRec {
    pub handle: u64,
    pub object: u32,
    pub access: u32,
    pub attributes: u32,
}

impl HandleRec {
    pub const STRIDE: u32 = 24;

    pub fn write(&self, w: &mut Writer) -> Result<(), AllocError> {
        w.u64(self.handle)?;
        w.u32(self.object)?;
        w.u32(self.access)?;
        w.u32(self.attributes)?;
        w.u32(0)
    }

    pub fn read(r: &mut Reader<'_>) -> Self {
        HandleRec { handle: r.u64(), object: r.u32(), access: r.u32(), attributes: r.u32() }
    }
}

impl PacketRec {
    pub const STRIDE: u32 = 32;

    pub fn write(&self, w: &mut Writer) -> Result<(), AllocError> {
        w.u64(self.key)?;
        w.u64(self.context)?;
        w.u64(self.information)?;
        w.u32(self.status)?;
        w.u32(0)
    }

    pub fn read(r: &mut Reader<'_>) -> Self {
        PacketRec { key: r.u64(), context: r.u64(), information: r.u64(), status: r.u32() }
    }
}

// The object entries' fields after the head, in order.

impl<'a> EventRec<'a> {
    pub fn write(&self, w: &mut Writer, name: &[u16]) -> Result<(), AllocError> {
        let e = w.begin_entry()?;
        self.head.write(w, name)?;
        w.u32(self.kind)?;
        w.u32(self.signal as u32)?;
        w.end_entry(e)
    }

    pub fn read(r: &mut Reader<'a>) -> Self {
        EventRec { head: ObjectHead::read(r), kind: r.u32(), signal: r.u32() as i32 }
    }
}

impl<'a> SemaphoreRec<'a> {
    pub fn write(&self, w: &mut Writer, name: &[u16]) -> Result<(), AllocError> {
        let e = w.begin_entry()?;
        self.head.write(w, name)?;
        w.u32(self.count as u32)?;
        w.u32(self.maximum as u32)?;
        w.end_entry(e)
    }

    pub fn read(r: &mut Reader<'a>) -> Self {
        SemaphoreRec { head: ObjectHead::read(r), count: r.u32() as i32, maximum: r.u32() as i32 }
    }
}

impl<'a> MutantRec<'a> {
    pub fn write(&self, w: &mut Writer, name: &[u16]) -> Result<(), AllocError> {
        let e = w.begin_entry()?;
        self.head.write(w, name)?;
        w.u64(self.owner)?;
        w.u32(self.count)?;
        w.u32(u32::from(self.abandoned))?;
        w.end_entry(e)
    }

    pub fn read(r: &mut Reader<'a>) -> Self {
        let head = ObjectHead::read(r);
        MutantRec { head, owner: r.u64(), count: r.u32(), abandoned: r.u32() != 0 }
    }
}

impl<'a> TimerRec<'a> {
    pub fn write(&self, w: &mut Writer, name: &[u16]) -> Result<(), AllocError> {
        let e = w.begin_entry()?;
        self.head.write(w, name)?;
        w.u32(u32::from(self.synchronization))?;
        w.u32(self.signal as u32)?;
        w.u64(self.due.map_or(0, |d| d.saturating_add(1)))?;
        w.u64(self.due_time)?;
        w.u64(self.period)?;
        let (thread, routine, context) = self.apc.unwrap_or((0, 0, 0));
        w.u32(thread)?;
        w.u32(0)?;
        w.u64(routine)?;
        w.u64(context)?;
        w.end_entry(e)
    }

    pub fn read(r: &mut Reader<'a>) -> Self {
        TimerRec {
            head: ObjectHead::read(r),
            synchronization: r.u32() != 0,
            signal: r.u32() as i32,
            due: r.u64().checked_sub(1),
            due_time: r.u64(),
            period: r.u64(),
            apc: {
                let (thread, _, routine, context) = (r.u32(), r.u32(), r.u64(), r.u64());
                (thread != 0).then_some((thread, routine, context))
            },
        }
    }
}

impl<'a> SectionRec<'a> {
    /// The entry, then its frames: a table of (page index, frame number),
    /// 16-byte records. `frames` must give exactly `count` items.
    pub fn write(
        &self,
        w: &mut Writer,
        name: &[u16],
        count: u32,
        frames: impl Iterator<Item = (u64, u64)>,
    ) -> Result<(), AllocError> {
        let e = w.begin_entry()?;
        self.head.write(w, name)?;
        w.u64(self.size)?;
        w.u32(self.protect)?;
        w.u32(self.attributes)?;
        w.table(count, 16)?;
        for (index, frame) in frames {
            w.u64(index)?;
            w.u64(frame)?;
        }
        w.end_entry(e)
    }

    /// The entry and its frames' table.
    pub fn read(r: &mut Reader<'a>) -> (Self, super::record::Table<'a>) {
        let head = ObjectHead::read(r);
        let rec = SectionRec { head, size: r.u64(), protect: r.u32(), attributes: r.u32() };
        (rec, r.table())
    }
}

impl<'a> FileRec<'a> {
    pub fn write(&self, w: &mut Writer, path: &[u16]) -> Result<(), AllocError> {
        let e = w.begin_entry()?;
        w.u32(self.id)?;
        w.u32(self.granted)?;
        w.u32(self.share)?;
        w.u32(self.options)?;
        w.u64(self.position)?;
        w.wide(path)?;
        w.end_entry(e)
    }

    pub fn read(r: &mut Reader<'a>) -> Self {
        FileRec {
            id: r.u32(),
            granted: r.u32(),
            share: r.u32(),
            options: r.u32(),
            position: r.u64(),
            path: r.wide(),
        }
    }
}

impl ExitedThreadRec {
    pub fn write(&self, w: &mut Writer) -> Result<(), AllocError> {
        let e = w.begin_entry()?;
        w.u32(self.id)?;
        w.u32(self.exit_status)?;
        for v in [self.tid, self.pid, self.teb, self.win32_start, self.create_time, self.exit_time]
        {
            w.u64(v)?;
        }
        w.end_entry(e)
    }

    pub fn read(r: &mut Reader<'_>) -> Self {
        ExitedThreadRec {
            id: r.u32(),
            exit_status: r.u32(),
            tid: r.u64(),
            pid: r.u64(),
            teb: r.u64(),
            win32_start: r.u64(),
            create_time: r.u64(),
            exit_time: r.u64(),
        }
    }
}

/// A PROCESS_OBJECTS entry: object `id` names process `pid`.
pub fn write_process_object(w: &mut Writer, id: u32, pid: u64) -> Result<(), AllocError> {
    let e = w.begin_entry()?;
    w.u32(id)?;
    w.u32(0)?;
    w.u64(pid)?;
    w.end_entry(e)
}

pub fn read_process_object(r: &mut Reader<'_>) -> (u32, u64) {
    let id = r.u32();
    let _ = r.u32();
    (id, r.u64())
}

/// A CONSOLES entry: object `id` is console input (else output).
pub fn write_console(w: &mut Writer, id: u32, input: bool) -> Result<(), AllocError> {
    let e = w.begin_entry()?;
    w.u32(id)?;
    w.u32(u32::from(input))?;
    w.end_entry(e)
}

pub fn read_console(r: &mut Reader<'_>) -> (u32, bool) {
    (r.u32(), r.u32() != 0)
}

/// An IMAGE_SECTIONS entry: an image section (an executable or DLL),
/// laid out again by the next kernel from its file and checked against
/// `digest`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageSectionRec<'a> {
    pub head: ObjectHead<'a>,
    /// `ImageLayout::digest` of the laid-out image.
    pub digest: u64,
    pub size: u64,
    /// The image's alternative base, where views share relocated frames
    /// (0: none taken).
    pub dynamic: u64,
    /// The file, from the volume root.
    pub path: Wide<'a>,
}

impl<'a> ImageSectionRec<'a> {
    pub fn write(&self, w: &mut Writer, name: &[u16], path: &[u16]) -> Result<(), AllocError> {
        let e = w.begin_entry()?;
        self.head.write(w, name)?;
        w.u64(self.digest)?;
        w.u64(self.size)?;
        w.u64(self.dynamic)?;
        w.wide(path)?;
        w.end_entry(e)
    }

    pub fn read(r: &mut Reader<'a>) -> Self {
        ImageSectionRec {
            head: ObjectHead::read(r),
            digest: r.u64(),
            size: r.u64(),
            dynamic: r.u64(),
            path: r.wide(),
        }
    }
}

/// A FILE_SECTIONS entry: a data section on a file (a FILES object),
/// mapped again from the file by the next kernel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileSectionRec<'a> {
    pub head: ObjectHead<'a>,
    pub size: u64,
    pub protect: u32,
    /// The file object's id.
    pub file: u32,
}

impl<'a> FileSectionRec<'a> {
    pub fn write(&self, w: &mut Writer, name: &[u16]) -> Result<(), AllocError> {
        let e = w.begin_entry()?;
        self.head.write(w, name)?;
        w.u64(self.size)?;
        w.u32(self.protect)?;
        w.u32(self.file)?;
        w.end_entry(e)
    }

    pub fn read(r: &mut Reader<'a>) -> Self {
        FileSectionRec { head: ObjectHead::read(r), size: r.u64(), protect: r.u32(), file: r.u32() }
    }
}

/// A PIPES entry: a pipe (both directions' queued bytes and message
/// lengths), numbered `index` (PipeEndRec::pipe) in the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PipeRec<'a> {
    pub index: u32,
    /// The instance's name (upper case) if a client can still find it
    /// (its server end is open); empty: none.
    pub name: Wide<'a>,
    /// MaximumInstances of the name.
    pub max_instances: u32,
    pub message_type: bool,
    pub quota: [u64; 2],
    /// `pipe::Conn` as a number (0 listening, 1 connected, 2 closing, 3
    /// disconnected).
    pub conn: u32,
    pub server_open: bool,
    pub generation: u64,
    /// Bytes waiting for the server end (0) and the client end (1).
    pub inbox: [&'a [u8]; 2],
}

impl<'a> PipeRec<'a> {
    /// The entry, then each direction's message lengths (two tables of
    /// u64).
    pub fn write(
        &self,
        w: &mut Writer,
        name: &[u16],
        messages: [&[u64]; 2],
    ) -> Result<(), AllocError> {
        let e = w.begin_entry()?;
        w.u32(self.index)?;
        w.wide(name)?;
        w.u32(self.max_instances)?;
        w.u32(u32::from(self.message_type))?;
        w.u64(self.quota[0])?;
        w.u64(self.quota[1])?;
        w.u32(self.conn)?;
        w.u32(u32::from(self.server_open))?;
        w.u64(self.generation)?;
        w.blob(self.inbox[0])?;
        w.blob(self.inbox[1])?;
        for m in messages {
            w.table(m.len() as u32, 8)?;
            for &len in m {
                w.u64(len)?;
            }
        }
        w.end_entry(e)
    }

    /// The entry and its two message-length tables.
    pub fn read(r: &mut Reader<'a>) -> (Self, [super::record::Table<'a>; 2]) {
        let index = r.u32();
        let name = r.wide();
        let (max_instances, message_type) = (r.u32(), r.u32() != 0);
        let quota = [r.u64(), r.u64()];
        let (conn, server_open, generation) = (r.u32(), r.u32() != 0, r.u64());
        let inbox = [r.blob(), r.blob()];
        let rec = PipeRec {
            index,
            name,
            max_instances,
            message_type,
            quota,
            conn,
            server_open,
            generation,
            inbox,
        };
        (rec, [r.table(), r.table()])
    }
}

/// A PIPE_ENDS entry: object `id` is end `end` (0 server, 1 client) of
/// pipe `pipe`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PipeEndRec {
    pub id: u32,
    pub pipe: u32,
    pub end: u32,
    pub synchronous: bool,
    pub message_reads: bool,
    /// FILE_SKIP_* notification modes.
    pub modes: u32,
    /// The connection a client end belongs to.
    pub generation: u64,
}

impl PipeEndRec {
    pub fn write(&self, w: &mut Writer) -> Result<(), AllocError> {
        let e = w.begin_entry()?;
        w.u32(self.id)?;
        w.u32(self.pipe)?;
        w.u32(self.end)?;
        w.u32(u32::from(self.synchronous))?;
        w.u32(u32::from(self.message_reads))?;
        w.u32(self.modes)?;
        w.u64(self.generation)?;
        w.end_entry(e)
    }

    pub fn read(r: &mut Reader<'_>) -> Self {
        PipeEndRec {
            id: r.u32(),
            pipe: r.u32(),
            end: r.u32(),
            synchronous: r.u32() != 0,
            message_reads: r.u32() != 0,
            modes: r.u32(),
            generation: r.u64(),
        }
    }
}

/// A KEYED_EVENTS entry (its id and name; the threads waiting on it make
/// their calls again).
pub fn write_keyed_event(w: &mut Writer, id: u32, name: &[u16]) -> Result<(), AllocError> {
    let e = w.begin_entry()?;
    ObjectHead { id, name: Wide(&[]) }.write(w, name)?;
    w.end_entry(e)
}

pub fn read_keyed_event<'a>(r: &mut Reader<'a>) -> ObjectHead<'a> {
    ObjectHead::read(r)
}

/// The most bytes a SID takes (15 sub-authorities).
pub const SID_BYTES: usize = 68;

/// A SID with its attributes in a TOKENS entry's table (72-byte records:
/// the SID padded to `SID_BYTES`, then the attributes).
pub type GroupRec = ([u8; SID_BYTES], u32);

/// A TOKENS entry: an access token as data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenRec<'a> {
    pub id: u32,
    /// 0 primary, 1 impersonation (with `level`).
    pub kind: u32,
    pub level: u32,
    /// TokenStatistics.TokenId, and its lineage's (`Token::origin`).
    pub token_id: u64,
    pub origin: u64,
    pub user: GroupRec,
    pub primary_group: [u8; SID_BYTES],
    pub owner: [u8; SID_BYTES],
    /// The default DACL (an ACL), empty: none.
    pub default_dacl: &'a [u8],
    pub authentication_id: u64,
    pub modified_id: u64,
    pub session: u32,
    pub elevation_type: u32,
    pub source_name: [u8; 8],
    pub source_id: u64,
    pub sandbox_inert: bool,
}

impl<'a> TokenRec<'a> {
    /// The entry, then three tables: groups and restricting SIDs
    /// (`GroupRec`), privileges (16-byte records: LUID, attributes).
    pub fn write(
        &self,
        w: &mut Writer,
        groups: &[GroupRec],
        privileges: &[(u64, u32)],
        restricted: &[GroupRec],
    ) -> Result<(), AllocError> {
        let e = w.begin_entry()?;
        w.u32(self.id)?;
        w.u32(self.kind)?;
        w.u32(self.level)?;
        w.u32(0)?;
        w.u64(self.token_id)?;
        w.u64(self.origin)?;
        w.bytes(&self.user.0)?;
        w.u32(self.user.1)?;
        w.bytes(&self.primary_group)?;
        w.bytes(&self.owner)?;
        w.blob(self.default_dacl)?;
        w.u64(self.authentication_id)?;
        w.u64(self.modified_id)?;
        w.u32(self.session)?;
        w.u32(self.elevation_type)?;
        w.bytes(&self.source_name)?;
        w.u64(self.source_id)?;
        w.u32(u32::from(self.sandbox_inert))?;
        w.u32(0)?;
        for table in [groups, restricted] {
            w.table(table.len() as u32, 72)?;
            for (sid, attributes) in table {
                w.bytes(sid)?;
                w.u32(*attributes)?;
            }
        }
        w.table(privileges.len() as u32, 16)?;
        for &(luid, attributes) in privileges {
            w.u64(luid)?;
            w.u32(attributes)?;
            w.u32(0)?;
        }
        w.end_entry(e)
    }

    /// The entry and its groups', restricting SIDs' and privileges'
    /// tables.
    pub fn read(r: &mut Reader<'a>) -> (Self, [super::record::Table<'a>; 3]) {
        let (id, kind, level, _) = (r.u32(), r.u32(), r.u32(), r.u32());
        let (token_id, origin) = (r.u64(), r.u64());
        let user = (r.array(), r.u32());
        let (primary_group, owner) = (r.array(), r.array());
        let default_dacl = r.blob();
        let rec = TokenRec {
            id,
            kind,
            level,
            token_id,
            origin,
            user,
            primary_group,
            owner,
            default_dacl,
            authentication_id: r.u64(),
            modified_id: r.u64(),
            session: r.u32(),
            elevation_type: r.u32(),
            source_name: r.array(),
            source_id: r.u64(),
            sandbox_inert: {
                let (v, _) = (r.u32(), r.u32());
                v != 0
            },
        };
        (rec, [r.table(), r.table(), r.table()])
    }
}

/// A `GroupRec` from a table record.
pub fn read_group(r: &mut Reader<'_>) -> GroupRec {
    (r.array(), r.u32())
}

/// A privilege (LUID, attributes) from a table record.
pub fn read_privilege(r: &mut Reader<'_>) -> (u64, u32) {
    (r.u64(), r.u32())
}

/// A SECURITY entry: object `id`'s security descriptor (self-relative).
pub fn write_security(w: &mut Writer, id: u32, descriptor: &[u8]) -> Result<(), AllocError> {
    let e = w.begin_entry()?;
    w.u32(id)?;
    w.blob(descriptor)?;
    w.end_entry(e)
}

pub fn read_security<'a>(r: &mut Reader<'a>) -> (u32, &'a [u8]) {
    (r.u32(), r.blob())
}

#[cfg(test)]
mod tests {
    use super::super::record::{self, Writer, tag};
    use super::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn sample() -> Vec<u8> {
        let mut w = Writer::new(1).unwrap();
        section(&mut w, tag::PROCESSES).unwrap();
        let e = w.begin_entry().unwrap();
        w.u64(0x48).unwrap();
        w.end_entry(e).unwrap();
        ProcessRec {
            id: 8,
            parent_id: 0,
            create_time: 133_000_000_000,
            image_base: 0x1_4000_0000,
            image_size: 0x5000,
            exception_dispatcher: 0,
            apc_dispatcher: 0x7ff0_1000,
            threads_created: 4,
            priority_class: 2,
            name: "handover_test.exe",
            path: "C:\\gala\\handover_test.exe",
            image_digest: 0x0123_4567_89ab_cdef,
            token: 22,
            handovers: 2,
        }
        .write(&mut w)
        .unwrap();
        section(&mut w, tag::ADDRESS_SPACES).unwrap();
        let e = w.begin_entry().unwrap();
        w.u64(8).unwrap();
        crate::nt::vad::Vad::default().write_to(&mut w).unwrap();
        w.table(1, ViewRec::STRIDE).unwrap();
        ViewRec { base: 0x20_0000, size: 0x1_0000, offset: 0, section: 3 }.write(&mut w).unwrap();
        let user = PageFlags { user: true, global: false, ..PageFlags::KERNEL_RW };
        let pages = [
            PageRec { va: 0x1_4000_0000, value: 0, kind: PageKind::Image, flags: user },
            PageRec { va: 0x20_0000, value: 0, kind: PageKind::Section, flags: user },
            PageRec {
                va: 0x30_0000,
                value: 0x1234,
                kind: PageKind::Owned,
                flags: PageFlags { executable: true, cache: CacheMode::Uncached, ..user },
            },
        ];
        w.table(pages.len() as u32, PageRec::STRIDE).unwrap();
        for p in &pages {
            p.write(&mut w).unwrap();
        }
        w.end_entry(e).unwrap();
        section(&mut w, tag::THREADS).unwrap();
        let mut fx = [0u8; 512];
        fx[24] = 0x80;
        fx[511] = 9;
        ThreadRec {
            tid: 12,
            pid: 8,
            teb: 0x7ffd_e000,
            win32_start: 0x1_4000_1000,
            create_time: 5,
            affinity: 3,
            object: 1,
            ordinal: 0,
            priority: 8,
            base_priority: 8,
            number: 0x34,
            regs: core::array::from_fn(|i| i as u64 * 3),
            deadline: Some(12_345),
            fx,
            state: thread_state::SYSCALL,
            suspend: 0,
            mxcsr: None,
            impersonation: 0,
            xstate: XSTATE_LEGACY,
            extended: &[],
        }
        .write(&mut w, &[])
        .unwrap();
        // A thread stopped in user mode, suspended twice, two APCs queued.
        ThreadRec {
            tid: 16,
            pid: 8,
            teb: 0x7ffd_c000,
            win32_start: 0x1_4000_2000,
            create_time: 6,
            affinity: 1,
            object: 20,
            ordinal: 1,
            priority: 8,
            base_priority: 8,
            number: 0,
            regs: core::array::from_fn(|i| i as u64 * 5 + 1),
            deadline: None,
            fx,
            state: thread_state::USER,
            suspend: 2,
            mxcsr: None,
            impersonation: 23,
            xstate: XSTATE_LEGACY,
            extended: &[],
        }
        .write(&mut w, &[[0x40_1000, 1, 2, 3], [0x40_2000, 4, 5, 6]])
        .unwrap();
        // One created suspended that never ran.
        ThreadRec {
            tid: 24,
            pid: 8,
            state: thread_state::START,
            suspend: 1,
            mxcsr: Some(0x1f80),
            impersonation: 0,
            teb: 0x7ffd_a000,
            win32_start: 0,
            create_time: 7,
            affinity: 3,
            object: 21,
            ordinal: 2,
            priority: 8,
            base_priority: 8,
            number: 0,
            regs: [0; 18],
            deadline: None,
            fx: [0; 512],
            xstate: XSTATE_LEGACY,
            extended: &[],
        }
        .write(&mut w, &[])
        .unwrap();
        section(&mut w, tag::EVENTS).unwrap();
        let name = wide("\\BaseNamedObjects\\go");
        let head = |id| ObjectHead { id, name: Wide(&[]) };
        EventRec { head: head(2), kind: 1, signal: 0 }.write(&mut w, &name).unwrap();
        section(&mut w, tag::SEMAPHORES).unwrap();
        SemaphoreRec { head: head(4), count: 2, maximum: 5 }.write(&mut w, &[]).unwrap();
        section(&mut w, tag::MUTANTS).unwrap();
        MutantRec { head: head(5), owner: 12, count: 2, abandoned: false }
            .write(&mut w, &[])
            .unwrap();
        section(&mut w, tag::TIMERS).unwrap();
        TimerRec {
            head: head(6),
            synchronization: true,
            signal: 0,
            due: Some(1500),
            due_time: 9,
            period: 0,
            apc: Some((20, 0x40_3000, 0x77)),
        }
        .write(&mut w, &[])
        .unwrap();
        TimerRec {
            head: head(7),
            synchronization: false,
            signal: 1,
            due: None,
            due_time: 0,
            period: 0,
            apc: None,
        }
        .write(&mut w, &[])
        .unwrap();
        section(&mut w, tag::SECTIONS).unwrap();
        SectionRec { head: head(3), size: 0x1_0000, protect: 4, attributes: 0x800_0000 }
            .write(&mut w, &[], 2, [(0u64, 0x2000u64), (5, 0x2001)].into_iter())
            .unwrap();
        section(&mut w, tag::FILES).unwrap();
        FileRec {
            id: 9,
            granted: 0x12_0089,
            share: 1,
            options: 0x60,
            position: 2,
            path: Wide(&[]),
        }
        .write(&mut w, &wide("\\gala\\handover_test.exe"))
        .unwrap();
        section(&mut w, tag::IO_COMPLETIONS).unwrap();
        let e = w.begin_entry().unwrap();
        head(10).write(&mut w, &[]).unwrap();
        w.table(1, PacketRec::STRIDE).unwrap();
        PacketRec { key: 1, context: 2, information: 3, status: 0x103 }.write(&mut w).unwrap();
        w.end_entry(e).unwrap();
        section(&mut w, tag::EXITED_THREADS).unwrap();
        ExitedThreadRec { id: 11, exit_status: 77, tid: 16, pid: 8, ..Default::default() }
            .write(&mut w)
            .unwrap();
        section(&mut w, tag::PROCESS_OBJECTS).unwrap();
        write_process_object(&mut w, 13, 8).unwrap();
        section(&mut w, tag::CONSOLES).unwrap();
        write_console(&mut w, 14, true).unwrap();
        write_console(&mut w, 15, false).unwrap();
        section(&mut w, tag::IMAGE_SECTIONS).unwrap();
        ImageSectionRec {
            head: head(16),
            digest: 0xfeed_beef,
            size: 0x9_0000,
            dynamic: 0x3ff7_fff0_0000,
            path: Wide(&[]),
        }
        .write(&mut w, &[], &wide("\\Windows\\System32\\kernel32.dll"))
        .unwrap();
        section(&mut w, tag::FILE_SECTIONS).unwrap();
        FileSectionRec { head: head(17), size: 0x1234, protect: 2, file: 9 }
            .write(&mut w, &[])
            .unwrap();
        section(&mut w, tag::PIPES).unwrap();
        PipeRec {
            index: 1,
            name: Wide(&[]),
            max_instances: 1,
            message_type: true,
            quota: [4096, 8192],
            conn: 1,
            server_open: true,
            generation: 3,
            inbox: [b"abc", b""],
        }
        .write(&mut w, &wide("\\GALA\\PIPE"), [&[1, 2], &[]])
        .unwrap();
        section(&mut w, tag::PIPE_ENDS).unwrap();
        PipeEndRec {
            id: 18,
            pipe: 1,
            end: 1,
            synchronous: true,
            message_reads: false,
            modes: 1,
            generation: 3,
        }
        .write(&mut w)
        .unwrap();
        section(&mut w, tag::KEYED_EVENTS).unwrap();
        write_keyed_event(&mut w, 19, &wide("\\BaseNamedObjects\\k")).unwrap();
        section(&mut w, tag::TOKENS).unwrap();
        let mut sid = [0u8; SID_BYTES];
        sid[..12].copy_from_slice(&[1, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0]);
        TokenRec {
            id: 22,
            kind: 0,
            level: 0,
            token_id: 0x10_0005,
            origin: 0x10_0001,
            user: (sid, 0),
            primary_group: sid,
            owner: sid,
            default_dacl: &[2, 0, 8, 0, 0, 0, 0, 0],
            authentication_id: 0x3e7,
            modified_id: 9,
            session: 1,
            elevation_type: 1,
            source_name: *b"gala\0\0\0\0",
            source_id: 0,
            sandbox_inert: true,
        }
        .write(&mut w, &[(sid, 7), (sid, 0xc000_0007)], &[(23, 3)], &[])
        .unwrap();
        section(&mut w, tag::SECURITY).unwrap();
        write_security(&mut w, 2, &[1, 0, 4, 0x80, 0x14, 0, 0]).unwrap();
        section(&mut w, tag::HANDLES).unwrap();
        let e = w.begin_entry().unwrap();
        w.u64(8).unwrap();
        w.table(2, HandleRec::STRIDE).unwrap();
        HandleRec { handle: 4, object: 14, access: 0x12_019f, attributes: 0 }
            .write(&mut w)
            .unwrap();
        HandleRec { handle: 0x40, object: 2, access: 0x1f_0003, attributes: 2 }
            .write(&mut w)
            .unwrap();
        w.end_entry(e).unwrap();
        w.finish().unwrap()
    }

    #[test]
    fn round_trip() {
        let bytes = sample();
        let rec = record::parse(&bytes).unwrap();
        let mut r = rec.section(tag::PROCESSES).unwrap();
        assert_eq!(r.entry().unwrap().u64(), 0x48);
        let p = ProcessRec::read(&mut r.entry().unwrap());
        assert_eq!((p.id, p.image_base, p.threads_created), (8, 0x1_4000_0000, 4));
        assert_eq!((p.name, p.path), ("handover_test.exe", "C:\\gala\\handover_test.exe"));
        assert!(r.entry().is_none());

        let mut r = rec.section(tag::ADDRESS_SPACES).unwrap();
        let mut e = r.entry().unwrap();
        assert_eq!(e.u64(), 8);
        let vad = crate::nt::vad::Vad::read_from(&mut e).unwrap();
        assert_eq!(vad.private_commit(), 0);
        let views: Vec<_> = e.table().iter().map(|mut v| ViewRec::read(&mut v)).collect();
        assert_eq!(views, [ViewRec { base: 0x20_0000, size: 0x1_0000, offset: 0, section: 3 }]);
        let pages: Vec<_> = e.table().iter().map(|mut p| PageRec::read(&mut p).unwrap()).collect();
        assert_eq!(pages.len(), 3);
        assert_eq!((pages[2].kind, pages[2].value), (PageKind::Owned, 0x1234));
        assert!(pages[2].flags.executable && pages[2].flags.writable && pages[2].flags.user);
        assert_eq!(pages[2].flags.cache, CacheMode::Uncached);
        assert!(!pages[1].flags.executable);

        assert_eq!(p.image_digest, 0x0123_4567_89ab_cdef);
        // PROCESSES 3 (the `handover_next` build) carries the count.
        assert_eq!(p.handovers, if version::NEXT { 2 } else { 0 });
        let mut threads = rec.section(tag::THREADS).unwrap();
        let (t, apcs) = ThreadRec::read(&mut threads.entry().unwrap());
        assert_eq!((t.tid, t.pid, t.number, t.deadline), (12, 8, 0x34, Some(12_345)));
        assert_eq!((t.state, t.suspend, t.mxcsr, apcs.len()), (thread_state::SYSCALL, 0, None, 0));
        assert_eq!(t.regs[17], 51);
        assert_eq!((t.fx[24], t.fx[511]), (0x80, 9));
        assert_eq!((t.xstate, t.extended), (XSTATE_LEGACY, &[][..]));
        let (t, apcs) = ThreadRec::read(&mut threads.entry().unwrap());
        assert_eq!((t.tid, t.state, t.suspend, t.regs[16]), (16, thread_state::USER, 2, 81));
        let apcs: Vec<_> = apcs.iter().map(|mut a| read_apc(&mut a)).collect();
        assert_eq!(apcs, [[0x40_1000, 1, 2, 3], [0x40_2000, 4, 5, 6]]);
        let (t, _) = ThreadRec::read(&mut threads.entry().unwrap());
        assert_eq!(
            (t.tid, t.state, t.suspend, t.mxcsr),
            (24, thread_state::START, 1, Some(0x1f80))
        );
        assert!(threads.entry().is_none());

        let ev = EventRec::read(&mut rec.section(tag::EVENTS).unwrap().entry().unwrap());
        assert_eq!((ev.head.id, ev.kind, ev.signal), (2, 1, 0));
        assert_eq!(ev.head.name.to_vec().unwrap(), wide("\\BaseNamedObjects\\go"));
        let s = SemaphoreRec::read(&mut rec.section(tag::SEMAPHORES).unwrap().entry().unwrap());
        assert_eq!((s.count, s.maximum), (2, 5));
        assert!(s.head.name.is_empty());
        let m = MutantRec::read(&mut rec.section(tag::MUTANTS).unwrap().entry().unwrap());
        assert_eq!((m.owner, m.count, m.abandoned), (12, 2, false));
        let mut timers = rec.section(tag::TIMERS).unwrap();
        let t1 = TimerRec::read(&mut timers.entry().unwrap());
        let t2 = TimerRec::read(&mut timers.entry().unwrap());
        assert_eq!((t1.due, t1.synchronization), (Some(1500), true));
        assert_eq!(t1.apc, Some((20, 0x40_3000, 0x77)));
        assert_eq!((t2.due, t2.signal, t2.apc), (None, 1, None));
        let (sec, frames) =
            SectionRec::read(&mut rec.section(tag::SECTIONS).unwrap().entry().unwrap());
        assert_eq!((sec.head.id, sec.size, sec.protect), (3, 0x1_0000, 4));
        let frames: Vec<_> = frames.iter().map(|mut f| (f.u64(), f.u64())).collect();
        assert_eq!(frames, [(0, 0x2000), (5, 0x2001)]);
        let f = FileRec::read(&mut rec.section(tag::FILES).unwrap().entry().unwrap());
        assert_eq!((f.id, f.position, f.options), (9, 2, 0x60));
        assert_eq!(f.path.to_vec().unwrap(), wide("\\gala\\handover_test.exe"));
        let mut c = rec.section(tag::IO_COMPLETIONS).unwrap().entry().unwrap();
        assert_eq!(ObjectHead::read(&mut c).id, 10);
        let packets: Vec<_> = c.table().iter().map(|mut p| PacketRec::read(&mut p)).collect();
        assert_eq!(packets[0].status, 0x103);
        let x =
            ExitedThreadRec::read(&mut rec.section(tag::EXITED_THREADS).unwrap().entry().unwrap());
        assert_eq!((x.id, x.exit_status, x.tid), (11, 77, 16));
        let po =
            read_process_object(&mut rec.section(tag::PROCESS_OBJECTS).unwrap().entry().unwrap());
        assert_eq!(po, (13, 8));
        let mut consoles = rec.section(tag::CONSOLES).unwrap();
        assert_eq!(read_console(&mut consoles.entry().unwrap()), (14, true));
        assert_eq!(read_console(&mut consoles.entry().unwrap()), (15, false));
        let i =
            ImageSectionRec::read(&mut rec.section(tag::IMAGE_SECTIONS).unwrap().entry().unwrap());
        assert_eq!(
            (i.head.id, i.digest, i.size, i.dynamic),
            (16, 0xfeed_beef, 0x9_0000, 0x3ff7_fff0_0000)
        );
        assert_eq!(i.path.to_vec().unwrap(), wide("\\Windows\\System32\\kernel32.dll"));
        let f =
            FileSectionRec::read(&mut rec.section(tag::FILE_SECTIONS).unwrap().entry().unwrap());
        assert_eq!((f.head.id, f.size, f.protect, f.file), (17, 0x1234, 2, 9));
        let (pipe, messages) =
            PipeRec::read(&mut rec.section(tag::PIPES).unwrap().entry().unwrap());
        assert_eq!((pipe.index, pipe.conn, pipe.generation, pipe.quota), (1, 1, 3, [4096, 8192]));
        assert!(pipe.message_type && pipe.server_open);
        assert_eq!(pipe.inbox, [&b"abc"[..], &b""[..]]);
        assert_eq!(pipe.name.to_vec().unwrap(), wide("\\GALA\\PIPE"));
        let lengths: Vec<_> = messages[0].iter().map(|mut m| m.u64()).collect();
        assert_eq!((lengths, messages[1].len()), (vec![1, 2], 0));
        let end = PipeEndRec::read(&mut rec.section(tag::PIPE_ENDS).unwrap().entry().unwrap());
        assert_eq!(
            end,
            PipeEndRec {
                id: 18,
                pipe: 1,
                end: 1,
                synchronous: true,
                message_reads: false,
                modes: 1,
                generation: 3
            }
        );
        let k = read_keyed_event(&mut rec.section(tag::KEYED_EVENTS).unwrap().entry().unwrap());
        assert_eq!((k.id, k.name.to_vec().unwrap()), (19, wide("\\BaseNamedObjects\\k")));
        let (tok, tables) = TokenRec::read(&mut rec.section(tag::TOKENS).unwrap().entry().unwrap());
        assert_eq!((tok.id, tok.token_id, tok.origin, tok.session), (22, 0x10_0005, 0x10_0001, 1));
        assert_eq!((tok.user.0[7], tok.default_dacl.len(), tok.sandbox_inert), (5, 8, true));
        assert_eq!(&tok.source_name[..4], b"gala");
        let groups: Vec<_> = tables[0].iter().map(|mut g| read_group(&mut g).1).collect();
        assert_eq!((groups, tables[1].len()), (vec![7, 0xc000_0007], 0));
        let privileges: Vec<_> = tables[2].iter().map(|mut p| read_privilege(&mut p)).collect();
        assert_eq!(privileges, [(23, 3)]);
        let (t2, _) = {
            let mut threads = rec.section(tag::THREADS).unwrap();
            threads.entry();
            ThreadRec::read(&mut threads.entry().unwrap())
        };
        assert_eq!(t2.impersonation, 23);
        assert_eq!(
            ProcessRec::read(&mut {
                let mut r = rec.section(tag::PROCESSES).unwrap();
                r.entry();
                r.entry().unwrap()
            })
            .token,
            22
        );
        let sd = read_security(&mut rec.section(tag::SECURITY).unwrap().entry().unwrap());
        assert_eq!(sd, (2, &[1, 0, 4, 0x80, 0x14, 0, 0][..]));
        let mut h = rec.section(tag::HANDLES).unwrap().entry().unwrap();
        assert_eq!(h.u64(), 8);
        let handles: Vec<_> = h.table().iter().map(|mut x| HandleRec::read(&mut x)).collect();
        assert_eq!(
            handles[1],
            HandleRec { handle: 0x40, object: 2, access: 0x1f_0003, attributes: 2 }
        );
    }

    #[test]
    fn sections_are_required() {
        // A kernel that does not know these sections refuses the record:
        // every one is REQUIRED.
        let bytes = sample();
        let rec = record::parse(&bytes).unwrap();
        for s in rec.sections() {
            let s = s.unwrap();
            assert_eq!(s.flags & REQUIRED, REQUIRED, "section {}", s.tag);
            assert_eq!(s.version, version_of(s.tag));
        }
        // Damage anywhere is caught by the record's CRC.
        for i in (record::HEADER_LEN..bytes.len()).step_by(97) {
            let mut bad = bytes.clone();
            bad[i] ^= 0x10;
            assert!(record::parse(&bad).is_err(), "byte {i}");
        }
    }

    #[test]
    fn older_entries_read_with_defaults() {
        // A thread entry from a writer that had fewer fields: the rest 0
        // (no deadline, zeroed x87 state), and an unknown page kind.
        let mut w = Writer::new(1).unwrap();
        section(&mut w, tag::THREADS).unwrap();
        let e = w.begin_entry().unwrap();
        w.u64(12).unwrap();
        w.u64(8).unwrap();
        w.end_entry(e).unwrap();
        section(&mut w, tag::ADDRESS_SPACES).unwrap();
        w.table(1, 24).unwrap();
        w.u64(0x1000).unwrap();
        w.u64(0).unwrap();
        w.u32(99).unwrap();
        w.u32(0).unwrap();
        let bytes = w.finish().unwrap();
        let rec = record::parse(&bytes).unwrap();
        let (t, apcs) = ThreadRec::read(&mut rec.section(tag::THREADS).unwrap().entry().unwrap());
        assert_eq!((t.tid, t.pid, t.teb, t.deadline, t.number), (12, 8, 0, None, 0));
        assert!(t.fx.iter().all(|&b| b == 0));
        // Version 1 had blocked system calls only: no APCs, not suspended.
        assert_eq!((t.state, t.suspend, t.mxcsr, apcs.len()), (thread_state::SYSCALL, 0, None, 0));
        let mut r = rec.section(tag::ADDRESS_SPACES).unwrap();
        let page = r.table().iter().next().map(|mut p| PageRec::read(&mut p));
        assert_eq!(page, Some(None));
    }

    #[test]
    fn version_1_entries_read_with_defaults() {
        // A process and a timer as milestone (b)'s kernel wrote them: no
        // digest (not checked), no APC.
        let mut w = Writer::new(1).unwrap();
        w.section(tag::PROCESSES, 1, REQUIRED).unwrap();
        let e = w.begin_entry().unwrap();
        for v in [8u64, 0, 1, 0x1_4000_0000, 0x5000, 0, 0] {
            w.u64(v).unwrap();
        }
        w.u32(1).unwrap();
        w.u32(2).unwrap();
        w.str("a.exe").unwrap();
        w.str("C:\\gala\\a.exe").unwrap();
        w.end_entry(e).unwrap();
        w.section(tag::TIMERS, 1, REQUIRED).unwrap();
        let e = w.begin_entry().unwrap();
        ObjectHead { id: 3, name: Wide(&[]) }.write(&mut w, &[]).unwrap();
        w.u32(1).unwrap();
        w.u32(0).unwrap();
        w.u64(1501).unwrap();
        w.u64(9).unwrap();
        w.u64(0).unwrap();
        w.end_entry(e).unwrap();
        let bytes = w.finish().unwrap();
        let rec = record::parse(&bytes).unwrap();
        let p = ProcessRec::read(&mut rec.section(tag::PROCESSES).unwrap().entry().unwrap());
        assert_eq!((p.id, p.path, p.image_digest), (8, "C:\\gala\\a.exe", 0));
        let t = TimerRec::read(&mut rec.section(tag::TIMERS).unwrap().entry().unwrap());
        assert_eq!((t.head.id, t.due, t.apc), (3, Some(1500), None));
    }

    fn process() -> ProcessRec<'static> {
        ProcessRec {
            id: 8,
            image_base: 0x1_4000_0000,
            name: "a.exe",
            path: "C:\\gala\\a.exe",
            image_digest: 77,
            token: 5,
            handovers: 3,
            ..Default::default()
        }
    }

    #[test]
    fn processes_across_versions() {
        // A version-2 entry (the default build's), then a version-3 one
        // (entries carry their length, so one section can hold both here).
        let mut w = Writer::new(1).unwrap();
        w.section(tag::PROCESSES, 3, REQUIRED).unwrap();
        process().write_as(&mut w, 2).unwrap();
        process().write_as(&mut w, 3).unwrap();
        let bytes = w.finish().unwrap();
        let rec = record::parse(&bytes).unwrap();
        let mut r = rec.section(tag::PROCESSES).unwrap();
        let (v2, v3) = (r.entry().unwrap(), r.entry().unwrap());
        // Version 2 read by a version-3 reader (the `handover_next`
        // build): the count takes its default, 0.
        let p = ProcessRec::read_as(&mut v2.clone(), 3);
        assert_eq!((p.token, p.image_digest, p.handovers), (5, 77, 0));
        assert_eq!(
            ProcessRec::read_as(&mut v2.clone(), 2),
            ProcessRec { handovers: 0, ..process() }
        );
        // Version 3 read by a version-3 reader and (ignoring the new
        // field) by a version-2 one; the bytes before it are version 2's.
        assert_eq!(ProcessRec::read_as(&mut v3.clone(), 3), process());
        assert_eq!(
            ProcessRec::read_as(&mut v3.clone(), 2),
            ProcessRec { handovers: 0, ..process() }
        );
        // Version 1 (milestone (b)) by both.
        let mut w = Writer::new(1).unwrap();
        w.section(tag::PROCESSES, 1, REQUIRED).unwrap();
        let e = w.begin_entry().unwrap();
        for v in [8u64, 0, 1, 0x1_4000_0000, 0x5000, 0, 0] {
            w.u64(v).unwrap();
        }
        w.u32(1).unwrap();
        w.u32(2).unwrap();
        w.str("a.exe").unwrap();
        w.str("C:\\gala\\a.exe").unwrap();
        w.end_entry(e).unwrap();
        let bytes = w.finish().unwrap();
        let rec = record::parse(&bytes).unwrap();
        for knows in [2, 3] {
            let p = ProcessRec::read_as(
                &mut rec.section(tag::PROCESSES).unwrap().entry().unwrap(),
                knows,
            );
            assert_eq!((p.id, p.name, p.image_digest, p.token, p.handovers), (8, "a.exe", 0, 0, 0));
        }
    }

    #[test]
    fn threads_across_versions() {
        let t = ThreadRec {
            tid: 12,
            pid: 8,
            teb: 0x7ffd_e000,
            win32_start: 0,
            create_time: 5,
            affinity: 3,
            object: 1,
            ordinal: 0,
            priority: 8,
            base_priority: 8,
            number: 0,
            regs: [7; 18],
            deadline: None,
            fx: [1; 512],
            state: thread_state::USER,
            suspend: 0,
            mxcsr: None,
            impersonation: 0,
            xstate: 0b111,
            extended: &[9; 64 + 256],
        };
        let mut w = Writer::new(1).unwrap();
        w.section(tag::THREADS, 2, REQUIRED).unwrap();
        t.write_as(&mut w, &[[1, 2, 3, 4]], 2).unwrap();
        t.write_as(&mut w, &[[1, 2, 3, 4]], 3).unwrap();
        let bytes = w.finish().unwrap();
        let rec = record::parse(&bytes).unwrap();
        let mut r = rec.section(tag::THREADS).unwrap();
        // Version 2: x87/SSE only, all in the FXSAVE area.
        let (v2, apcs) = ThreadRec::read(&mut r.entry().unwrap());
        assert_eq!((v2.xstate, v2.extended.len(), apcs.len()), (XSTATE_LEGACY, 0, 1));
        assert_eq!(v2, ThreadRec { xstate: XSTATE_LEGACY, extended: &[], ..t.clone() });
        // Version 3: the blob after the APCs, whatever its size.
        let (v3, apcs) = ThreadRec::read(&mut r.entry().unwrap());
        assert_eq!((v3, apcs.len()), (t, 1));
        // A thread with extended state needs a version-3 reader.
        assert_eq!(threads_must_read(true), 3);
        assert_eq!(threads_must_read(false), 2);
    }

    #[test]
    fn damaged_new_entries_read_safely() {
        // A pipe entry cut short (an inbox longer than the entry): the
        // bytes stop at the entry's end, the tables are empty.
        let mut w = Writer::new(1).unwrap();
        section(&mut w, tag::PIPES).unwrap();
        let e = w.begin_entry().unwrap();
        w.u32(1).unwrap();
        w.wide(&[]).unwrap();
        for _ in 0..8 {
            w.u32(0).unwrap();
        }
        w.u32(0).unwrap();
        w.u32(0).unwrap();
        w.u32(1000).unwrap(); // inbox 0: 1000 bytes claimed
        w.bytes(b"xy").unwrap();
        w.end_entry(e).unwrap();
        let bytes = w.finish().unwrap();
        let rec = record::parse(&bytes).unwrap();
        let (p, m) = PipeRec::read(&mut rec.section(tag::PIPES).unwrap().entry().unwrap());
        assert!(p.inbox[0].len() <= 4 && p.inbox[1].is_empty());
        assert_eq!((m[0].len(), m[1].len()), (0, 0));
    }

    #[test]
    fn flags_round_trip() {
        for writable in [false, true] {
            for executable in [false, true] {
                for cache in [CacheMode::WriteBack, CacheMode::WriteCombining] {
                    let f = PageFlags {
                        writable,
                        executable,
                        user: true,
                        global: false,
                        cache,
                        borrowed: false,
                    };
                    assert_eq!(decode_flags(encode_flags(f)), f);
                }
            }
        }
    }
}
