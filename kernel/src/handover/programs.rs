//! Programs across a kernel handover (ROADMAP R9 milestone (b),
//! ARCHITECTURE "Kernel handover"): the record's formats for processes,
//! their address spaces, threads as data, objects and handle tables.
//! Plain data, host-tested; `carry` fills them from the running kernel
//! and rebuilds the kernel's objects from them.
//!
//! Every section here is REQUIRED (a kernel that does not know one would
//! lose programs) and version 1. Lists are entries (`Writer::begin_entry`:
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
//! THREADS        an entry per live thread (ThreadRec)
//! EVENTS ...     an entry per object (EventRec, SemaphoreRec, ...)
//! HANDLES        an entry per process: pid, a table of handles (HandleRec)
//! USER_FRAMES    ranges of the frames of OWNED pages and section pages
//! ```

use super::record::{REQUIRED, Reader, Wide, Writer};
use crate::mm::heap::AllocError;
use crate::mm::paging::{CacheMode, PageFlags};

/// The version of every section here.
pub const VERSION: u16 = 1;

/// Starts one of this module's sections.
pub fn section(w: &mut Writer, tag: u32) -> Result<(), AllocError> {
    w.section(tag, VERSION, REQUIRED)
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
}

impl<'a> ProcessRec<'a> {
    pub fn write(&self, w: &mut Writer) -> Result<(), AllocError> {
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
        w.end_entry(e)
    }

    pub fn read(r: &mut Reader<'a>) -> Self {
        ProcessRec {
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
        }
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
}

impl PageKind {
    fn number(self) -> u32 {
        match self {
            PageKind::Owned => 0,
            PageKind::Image => 1,
            PageKind::SharedData => 2,
            PageKind::Section => 3,
        }
    }

    fn from_number(n: u32) -> Option<Self> {
        Some(match n {
            0 => PageKind::Owned,
            1 => PageKind::Image,
            2 => PageKind::SharedData,
            3 => PageKind::Section,
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

/// A THREADS entry: a live thread blocked in a system call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadRec {
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
    /// Its x87/SSE state (FXSAVE layout).
    pub fx: [u8; 512],
}

impl ThreadRec {
    pub fn write(&self, w: &mut Writer) -> Result<(), AllocError> {
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
        w.end_entry(e)
    }

    pub fn read(r: &mut Reader<'_>) -> Self {
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
        ThreadRec {
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
            fx: r.array(),
        }
    }
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
        }
        .write(&mut w)
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

        let t = ThreadRec::read(&mut rec.section(tag::THREADS).unwrap().entry().unwrap());
        assert_eq!((t.tid, t.pid, t.number, t.deadline), (12, 8, 0x34, Some(12_345)));
        assert_eq!(t.regs[17], 51);
        assert_eq!((t.fx[24], t.fx[511]), (0x80, 9));

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
        assert_eq!((t2.due, t2.signal), (None, 1));
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
            assert_eq!(s.version, VERSION);
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
        let t = ThreadRec::read(&mut rec.section(tag::THREADS).unwrap().entry().unwrap());
        assert_eq!((t.tid, t.pid, t.teb, t.deadline, t.number), (12, 8, 0, None, 0));
        assert!(t.fx.iter().all(|&b| b == 0));
        let mut r = rec.section(tag::ADDRESS_SPACES).unwrap();
        let page = r.table().iter().next().map(|mut p| PageRec::read(&mut p));
        assert_eq!(page, Some(None));
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
