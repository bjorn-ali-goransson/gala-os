//! Processes: a user address space, a handle table and threads. Includes
//! the loader for freestanding PE executables (no imports yet).

use alloc::vec::Vec;
use core::fmt;

use super::layout::{self, params, peb};
use super::object::{HandleTable, KObject, ObjectBody, ObjectRef};
use super::pe::{self, PeError, PeImage};
use super::status::NtStatus;
use super::thread;
use super::vad::{self, Vad};
use super::vm::{self, AccessKind};
use super::wait::{self, Dispatcher};
use crate::arch::interrupts::{self, TrapFrame};
use crate::arch::paging::{PagingError, SharedUserSpace, UserFault, UserSpace};
use crate::arch::syscall::UserRegs;
use crate::mm::PAGE_SIZE;
use crate::mm::heap::{AllocError, KArc, TryVec};
use crate::mm::paging::PageFlags;
use crate::sched::SpawnError;
use crate::sync::IrqMutex;

pub struct Process {
    pub id: u64,
    pub name: &'static str,
    pub space: SharedUserSpace,
    pub handles: IrqMutex<HandleTable>,
    /// Lock order: `vad` before `space`.
    pub vad: IrqMutex<Vad>,
    /// Thread objects of the live threads.
    threads: IrqMutex<Vec<ObjectRef>>,
    exit_status: IrqMutex<Option<NtStatus>>,
    /// Signaled when the process has ended.
    pub dispatcher: Dispatcher,
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

    pub fn add_thread(&self, thread: ObjectRef) -> Result<(), AllocError> {
        self.threads.lock().try_push(thread)
    }

    pub fn remove_thread(&self, thread: &ObjectRef) {
        self.threads.lock().retain(|t| !KArc::ptr_eq(t, thread));
    }

    pub fn thread_count(&self) -> usize {
        self.threads.lock().len()
    }

    /// A snapshot of the live threads.
    pub fn threads(&self) -> Vec<ObjectRef> {
        let threads = self.threads.lock();
        let mut out = Vec::new();
        if out.try_reserve(threads.len()).is_ok() {
            for t in threads.iter() {
                let _ = out.try_push(t.clone());
            }
        }
        out
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

/// The process owning the current thread, if it is a user thread.
pub fn current() -> Option<KArc<Process>> {
    let t = thread::current()?;
    match &t.body {
        ObjectBody::Thread(t) => Some(t.process.clone()),
        _ => None,
    }
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
) -> Result<(KArc<Process>, ObjectRef), LoadError> {
    let image = PeImage::parse(data).map_err(LoadError::Pe)?;
    if image.has_imports() {
        return Err(LoadError::ImportsNotSupported);
    }
    let mut space = UserSpace::new()?;
    let mut vad = Vad::default();
    let base = map_image(&mut space, &mut vad, &image)?;
    let id = thread::next_id();

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

    let space = KArc::try_new(IrqMutex::new(space))?;
    let process = KArc::try_new(Process {
        id,
        name,
        space,
        handles: IrqMutex::new(handles),
        vad: IrqMutex::new(vad),
        threads: IrqMutex::new(Vec::new()),
        exit_status: IrqMutex::new(None),
        dispatcher: Dispatcher::new(0),
    })?;
    PROCESSES.lock().try_push(process.clone())?;

    // Main thread: starts at the entry point with rcx = PEB, and rsp as if
    // called (rsp % 16 == 8, 32-byte shadow area above the null return).
    let mut gpr = [0u64; 16];
    gpr[1] = peb_addr;
    gpr[4] = layout::STACK_TOP - 0x28;
    let regs = UserRegs { gpr, rip: base + u64::from(image.entry_rva), rflags: 0 };
    let main = match thread::create(&process, regs, (layout::STACK_TOP, stack_bottom), false) {
        Ok(t) => t,
        Err(_) => {
            PROCESSES.lock().retain(|p| !KArc::ptr_eq(p, &process));
            return Err(LoadError::OutOfMemory);
        }
    };
    info!("process {} (pid {}): starting at {:#x}", name, id, regs.rip);
    Ok((process, main))
}

/// Ends `process` with `status`: other threads are asked to terminate, and
/// the calling thread exits too if it belongs to the process.
pub fn terminate(process: &KArc<Process>, status: NtStatus, include_caller: bool) {
    process.exit_status.lock().get_or_insert(status);
    let me = thread::current();
    for t in process.threads() {
        if me.as_ref().is_some_and(|m| KArc::ptr_eq(m, &t)) {
            continue;
        }
        thread::request_termination(&t, status);
    }
    if include_caller
        && me.is_some_and(
            |m| matches!(&m.body, ObjectBody::Thread(t) if KArc::ptr_eq(&t.process, process)),
        )
    {
        // Exits on the way back to user mode.
        thread::exit_current_on_return(status);
    }
}

/// Ends the current thread's process (NtTerminateProcess(NtCurrentProcess()));
/// takes effect when the caller returns towards user mode.
pub fn terminate_current(status: NtStatus) {
    if let Some(p) = current() {
        terminate(&p, status, true);
    }
}

/// Called when the last thread of `process` has exited.
pub fn finish(process: &KArc<Process>, last_status: NtStatus) {
    let status = *process.exit_status.lock().get_or_insert(last_status);
    info!("process {} (pid {}) exited with status {:#010x}", process.name, process.id, status.0);
    wait::signal_dispatcher(&process.dispatcher, 1);
    // Close every handle (this breaks reference cycles through handles to
    // the process's own objects) and forget the process.
    let entries = process.handles.lock().close_all();
    drop(entries);
    PROCESSES.lock().retain(|p| !KArc::ptr_eq(p, process));
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
    // Takes effect on the way back to user mode (the user-return hook).
    terminate_current(status)
}

pub fn init() {
    interrupts::set_user_exception_handler(on_user_exception);
    thread::init();
}
