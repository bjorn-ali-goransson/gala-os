//! Processes: a user address space, a handle table and threads. Includes
//! the loader for freestanding PE executables (no imports yet).

use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

use super::layout::{self, params, peb, teb};
use super::object::{HandleTable, KObject, ObjectBody};
use super::pe::{self, PeError, PeImage};
use super::status::NtStatus;
use super::vad::{self, Vad};
use super::vm::{self, AccessKind};
use crate::arch::interrupts::{self, TrapFrame};
use crate::arch::paging::{PagingError, SharedUserSpace, UserFault, UserSpace};
use crate::arch::syscall;
use crate::mm::PAGE_SIZE;
use crate::mm::heap::{AllocError, KArc, TryVec};
use crate::mm::paging::{PageFlags, USER_END};
use crate::sched::{self, SpawnError, ThreadId};
use crate::sync::IrqMutex;

pub struct Process {
    pub id: u64,
    pub name: &'static str,
    pub space: SharedUserSpace,
    pub handles: IrqMutex<HandleTable>,
    /// Lock order: `vad` before `space`.
    pub vad: IrqMutex<Vad>,
    threads: IrqMutex<Vec<ThreadId>>,
    exit_status: IrqMutex<Option<NtStatus>>,
    /// Main thread start state.
    entry: u64,
    initial_rsp: u64,
    teb: u64,
}

impl fmt::Debug for Process {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Process({} pid {})", self.name, self.id)
    }
}

impl Process {
    pub fn exit_status(&self) -> Option<NtStatus> {
        *self.exit_status.lock()
    }

    /// Runs a user-memory copy, resolving demand-zero faults on the way
    /// (each retry backs at least one more page).
    fn with_faults(
        &self,
        kind: AccessKind,
        mut op: impl FnMut(&mut UserSpace) -> Result<(), UserFault>,
    ) -> Result<(), UserFault> {
        loop {
            let result = op(&mut self.space.lock());
            match result {
                Err(fault) if vm::resolve_fault(self, fault.address, kind) => continue,
                other => return other,
            }
        }
    }

    /// Copies `buf.len()` bytes from user address `addr`.
    pub fn read(&self, addr: u64, buf: &mut [u8]) -> Result<(), UserFault> {
        self.with_faults(AccessKind::Read, |s| s.read(addr, buf))
    }

    pub fn read_u64(&self, addr: u64) -> Result<u64, UserFault> {
        let mut b = [0; 8];
        self.read(addr, &mut b)?;
        Ok(u64::from_le_bytes(b))
    }

    pub fn write(&self, addr: u64, data: &[u8]) -> Result<(), UserFault> {
        self.with_faults(AccessKind::Write, |s| s.write(addr, data))
    }

    pub fn write_u64(&self, addr: u64, value: u64) -> Result<(), UserFault> {
        self.write(addr, &value.to_le_bytes())
    }
}

static PROCESSES: IrqMutex<Vec<KArc<Process>>> = IrqMutex::new(Vec::new());
/// NT process and thread ids are multiples of 4.
static NEXT_ID: AtomicU64 = AtomicU64::new(4);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(4, Ordering::Relaxed)
}

/// The process owning the current thread, if it is a user thread.
pub fn current() -> Option<KArc<Process>> {
    let me = sched::current();
    PROCESSES.lock().iter().find(|p| p.threads.lock().contains(&me)).cloned()
}

#[derive(Debug)]
pub enum LoadError {
    Pe(PeError),
    ImportsNotSupported,
    BadImageBase,
    OutOfMemory,
    Paging(PagingError),
    Spawn(SpawnError),
}

impl From<AllocError> for LoadError {
    fn from(_: AllocError) -> Self {
        LoadError::OutOfMemory
    }
}

impl From<PagingError> for LoadError {
    fn from(e: PagingError) -> Self {
        LoadError::Paging(e)
    }
}

impl From<NtStatus> for LoadError {
    fn from(_: NtStatus) -> Self {
        LoadError::BadImageBase
    }
}

impl From<UserFault> for LoadError {
    fn from(_: UserFault) -> Self {
        LoadError::BadImageBase
    }
}

fn section_flags(characteristics: u32) -> PageFlags {
    PageFlags {
        writable: characteristics & pe::IMAGE_SCN_MEM_WRITE != 0,
        executable: characteristics & pe::IMAGE_SCN_MEM_EXECUTE != 0,
        user: true,
        global: false,
        cache: crate::mm::paging::CacheMode::WriteBack,
    }
}

const USER_RW: PageFlags = PageFlags { user: true, global: false, ..PageFlags::KERNEL_RW };
const USER_RO: PageFlags = PageFlags { user: true, global: false, ..PageFlags::KERNEL_RO };

fn pages(bytes: u64) -> u64 {
    bytes.div_ceil(PAGE_SIZE)
}

fn put_u64(space: &mut UserSpace, addr: u64, value: u64) -> Result<(), UserFault> {
    space.write_initial(addr, &value.to_le_bytes())
}

fn put_u32(space: &mut UserSpace, addr: u64, value: u32) -> Result<(), UserFault> {
    space.write_initial(addr, &value.to_le_bytes())
}

/// Page protection for an image section, as Windows reports it (writable
/// image pages are copy-on-write).
fn section_protect(characteristics: u32) -> u32 {
    let (r, w, x) = (
        characteristics & pe::IMAGE_SCN_MEM_READ != 0,
        characteristics & pe::IMAGE_SCN_MEM_WRITE != 0,
        characteristics & pe::IMAGE_SCN_MEM_EXECUTE != 0,
    );
    match (x, w, r) {
        (true, true, _) => vad::PAGE_EXECUTE_WRITECOPY,
        (true, false, true) => vad::PAGE_EXECUTE_READ,
        (true, false, false) => vad::PAGE_EXECUTE,
        (false, true, _) => vad::PAGE_WRITECOPY,
        (false, false, true) => vad::PAGE_READONLY,
        (false, false, false) => vad::PAGE_NOACCESS,
    }
}

/// Maps the image at its preferred base.
fn map_image(space: &mut UserSpace, vad: &mut Vad, image: &PeImage<'_>) -> Result<u64, LoadError> {
    let base = image.image_base;
    let end = base.checked_add(u64::from(image.size_of_image)).ok_or(LoadError::BadImageBase)?;
    // Keep the first 64 KiB free (null-pointer region) and stay clear of
    // our fixed structures.
    if base < 0x1_0000
        || end > layout::PEB_ADDRESS
        || (base < layout::STACK_TOP && end > layout::STACK_TOP - layout::STACK_SIZE - PAGE_SIZE)
    {
        return Err(LoadError::BadImageBase);
    }
    // TODO: relocate (pe::apply_relocations) when the preferred base is taken.
    vad.reserve(
        Some(base),
        u64::from(image.size_of_image),
        vad::PAGE_EXECUTE_WRITECOPY,
        vad::MEM_IMAGE,
        false,
    )?;
    vad.commit(base, u64::from(image.size_of_headers), vad::PAGE_READONLY)?;
    space.map_zeroed(base, pages(u64::from(image.size_of_headers)), USER_RO)?;
    space.write_initial(base, image.headers())?;
    for section in image.sections() {
        let va = base + u64::from(section.virtual_address);
        let size = u64::from(section.memory_size());
        if size == 0 {
            continue;
        }
        vad.commit(va, size, section_protect(section.characteristics))?;
        space.map_zeroed(va, pages(size), section_flags(section.characteristics))?;
        space.write_initial(va, image.section_data(&section))?;
    }
    Ok(base)
}

/// Creates a process from a PE executable and starts its main thread.
pub fn spawn_from_pe(
    name: &'static str,
    data: &[u8],
) -> Result<(KArc<Process>, ThreadId), LoadError> {
    let image = PeImage::parse(data).map_err(LoadError::Pe)?;
    if image.has_imports() {
        return Err(LoadError::ImportsNotSupported);
    }
    let mut space = UserSpace::new()?;
    let mut vad = Vad::default();
    let base = map_image(&mut space, &mut vad, &image)?;
    let id = next_id();
    let thread_id = next_id();

    // Main thread stack: committed demand-zero pages above an uncommitted
    // guard page.
    let stack_bottom = layout::STACK_TOP - layout::STACK_SIZE;
    vad.reserve(
        Some(stack_bottom - PAGE_SIZE),
        layout::STACK_SIZE + PAGE_SIZE,
        vad::PAGE_READWRITE,
        vad::MEM_PRIVATE,
        false,
    )?;
    vad.commit(stack_bottom, layout::STACK_SIZE, vad::PAGE_READWRITE)?;

    // Handles for the standard streams.
    let mut handles = HandleTable::default();
    let stdin = handles.insert_object(KObject::new(ObjectBody::ConsoleInput)?)?;
    let output = KObject::new(ObjectBody::ConsoleOutput)?;
    let stdout = handles.insert_object(output.clone())?;
    let stderr = handles.insert_object(output)?;

    // RTL_USER_PROCESS_PARAMETERS.
    let p = layout::PARAMS_ADDRESS;
    vm::register(&mut vad, p, params::PAGES * PAGE_SIZE, vad::MEM_PRIVATE, vad::PAGE_READWRITE)?;
    space.map_zeroed(p, params::PAGES, USER_RW)?;
    let size = (params::PAGES * PAGE_SIZE) as u32;
    put_u32(&mut space, p + params::MAXIMUM_LENGTH, size)?;
    put_u32(&mut space, p + params::LENGTH, size)?;
    put_u32(&mut space, p + params::FLAGS, params::FLAG_NORMALIZED)?;
    put_u64(&mut space, p + params::STANDARD_INPUT, stdin.0)?;
    put_u64(&mut space, p + params::STANDARD_OUTPUT, stdout.0)?;
    put_u64(&mut space, p + params::STANDARD_ERROR, stderr.0)?;

    // PEB.
    let peb_addr = layout::PEB_ADDRESS;
    vm::register(
        &mut vad,
        peb_addr,
        peb::PAGES * PAGE_SIZE,
        vad::MEM_PRIVATE,
        vad::PAGE_READWRITE,
    )?;
    space.map_zeroed(peb_addr, peb::PAGES, USER_RW)?;
    put_u64(&mut space, peb_addr + peb::IMAGE_BASE_ADDRESS, base)?;
    put_u64(&mut space, peb_addr + peb::PROCESS_PARAMETERS, p)?;

    // TEB of the main thread.
    let teb_addr = layout::FIRST_TEB_ADDRESS;
    vm::register(
        &mut vad,
        teb_addr,
        teb::PAGES * PAGE_SIZE,
        vad::MEM_PRIVATE,
        vad::PAGE_READWRITE,
    )?;
    space.map_zeroed(teb_addr, teb::PAGES, USER_RW)?;
    put_u64(&mut space, teb_addr + teb::STACK_BASE, layout::STACK_TOP)?;
    put_u64(&mut space, teb_addr + teb::STACK_LIMIT, stack_bottom)?;
    put_u64(&mut space, teb_addr + teb::SELF, teb_addr)?;
    put_u64(&mut space, teb_addr + teb::CLIENT_ID_PROCESS, id)?;
    put_u64(&mut space, teb_addr + teb::CLIENT_ID_THREAD, thread_id)?;
    put_u64(&mut space, teb_addr + teb::PEB, peb_addr)?;

    let space = KArc::try_new(IrqMutex::new(space))?;
    let process = KArc::try_new(Process {
        id,
        name,
        space: space.clone(),
        handles: IrqMutex::new(handles),
        vad: IrqMutex::new(vad),
        threads: IrqMutex::new(Vec::new()),
        exit_status: IrqMutex::new(None),
        entry: base + u64::from(image.entry_rva),
        // As if called: rsp % 16 == 8, with the caller's 32-byte shadow
        // area above the (null) return address.
        initial_rsp: layout::STACK_TOP - 0x28,
        teb: teb_addr,
    })?;

    // Register the process (with its thread list reserved) before the
    // thread can run, so `current()` finds it.
    process.threads.lock().try_reserve(1).map_err(|_| AllocError)?;
    PROCESSES.lock().try_push(process.clone())?;
    let tid = sched::spawn_user(name, main_thread, id as usize, space, teb_addr)
        .map_err(LoadError::Spawn)?;
    process.threads.lock().try_push(tid)?;
    Ok((process, tid))
}

fn by_id(id: u64) -> Option<KArc<Process>> {
    PROCESSES.lock().iter().find(|p| p.id == id).cloned()
}

fn main_thread(pid: usize) {
    // Wait until the spawner has recorded this thread (it may be preempted
    // right after `spawn_user`).
    let process = loop {
        if let Some(p) = current() {
            break p;
        }
        sched::yield_now();
    };
    debug_assert_eq!(process.id, pid as u64);
    let _ = by_id(pid as u64);
    let (entry, rsp, peb_addr) = (process.entry, process.initial_rsp, layout::PEB_ADDRESS);
    debug_assert!(entry < USER_END);
    info!(
        "process {} (pid {}): starting at {:#x}, TEB {:#x}",
        process.name, process.id, entry, process.teb
    );
    drop(process);
    syscall::enter_user(entry, rsp, peb_addr)
}

/// Ends the current thread's process with `status` (single-threaded
/// processes only, so far).
pub fn terminate_current(status: NtStatus) -> ! {
    if let Some(p) = current() {
        p.exit_status.lock().get_or_insert(status);
        info!("process {} (pid {}) exited with status {:#010x}", p.name, p.id, status.0);
        let me = sched::current();
        p.threads.lock().retain(|&t| t != me);
        PROCESSES.lock().retain(|q| !KArc::ptr_eq(q, &p) || !q.threads.lock().is_empty());
    }
    sched::exit()
}

/// User-mode CPU exceptions: no SEH dispatch yet, so they end the process.
fn on_user_exception(frame: &mut TrapFrame, cr2: Option<u64>) {
    let vector = frame.vector as u8;
    if let (14, Some(addr), Some(process)) = (vector, cr2, current()) {
        // Page-fault error code: bit 1 = write, bit 4 = instruction fetch.
        let kind = match frame.error_code {
            e if e & (1 << 4) != 0 => AccessKind::Execute,
            e if e & (1 << 1) != 0 => AccessKind::Write,
            _ => AccessKind::Read,
        };
        if vm::resolve_fault(&process, addr, kind) {
            return; // retry the instruction
        }
    }
    let status = match vector {
        0 => NtStatus::INTEGER_DIVIDE_BY_ZERO,
        3 => NtStatus::BREAKPOINT,
        6 => NtStatus::ILLEGAL_INSTRUCTION,
        13 => NtStatus::PRIVILEGED_INSTRUCTION,
        14 => NtStatus::ACCESS_VIOLATION,
        _ => NtStatus::ACCESS_VIOLATION,
    };
    warn!(
        "user exception: {} at rip {:#x} (cr2 {:#x}, error {:#x})",
        interrupts::exception_name(vector),
        frame.rip,
        cr2.unwrap_or(0),
        frame.error_code
    );
    // Interrupts are off in the handler; exiting switches away for good.
    terminate_current(status)
}

pub fn init() {
    interrupts::set_user_exception_handler(on_user_exception);
}
