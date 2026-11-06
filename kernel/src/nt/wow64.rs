//! WOW64: 32-bit (x86) programs (ARCHITECTURE "Plan: WOW64").
//!
//! A process whose executable is PE32 (machine i386) is a WOW64 process.
//! The kernel gives it what only the kernel can: its 32-bit address space
//! (allocations below 2 GiB, or 4 GiB for a large-address-aware image),
//! its PEB32 and 32-bit process parameters beside the 64-bit ones, a TEB32
//! per thread (FS for 32-bit code, `sched::set_teb32`), and gala's
//! wow64.dll (`\Windows\System32\wow64.dll`, 64-bit, no imports) mapped
//! into it: the 64-bit side of the process, where its threads start
//! (`GalaWow64Start`, which enters 32-bit code) and where its 32-bit
//! system calls go (`GalaWow64Transition`, TEB32.WOW32Reserved), to be
//! converted into the 64-bit calls the kernel serves. The kernel's
//! system call layer stays 64-bit only.

use core::sync::atomic::{AtomicU64, Ordering};

use super::layout::wow64::{self as w, context32 as ctx, cpu, peb32, teb32};
use super::object::{KObject, ObjectBody, ObjectRef};
use super::platform;
use super::process::Process;
use super::section::Section;
use super::status::NtStatus;
use super::vad::{self, Vad};
use crate::sync::IrqMutex;

const PATH: &str = "\\Windows\\System32\\wow64.dll";
/// IMAGE_FILE_MACHINE_I386: WOW64_CPURESERVED.Machine.
pub const MACHINE_I386: u16 = 0x14c;

/// The wow64.dll image section and its entry points.
#[derive(Clone)]
pub struct Dll {
    pub section: ObjectRef,
    /// GalaWow64Start: a WOW64 thread's first 64-bit code (rcx = its CPU
    /// area), which enters 32-bit code with the area's context.
    pub start_rva: u32,
    /// GalaWow64Transition: 32-bit code, where `call fs:[0xc0]` goes.
    pub transition_rva: u32,
}

/// Loaded on first use; `Some(None)` once we know there is none.
static DLL: IrqMutex<Option<Option<Dll>>> = IrqMutex::new(None);

/// gala's wow64.dll, if the system has one.
pub fn dll() -> Option<Dll> {
    let mut slot = DLL.lock();
    slot.get_or_insert_with(load).clone()
}

fn load() -> Option<Dll> {
    let node = super::file::lookup(PATH).ok()?;
    let data = match super::file::read_all(node) {
        Ok(d) => d,
        Err(e) => {
            warn!("wow64: cannot read {PATH}: {e:?}");
            return None;
        }
    };
    let mut section = match Section::new_image(&data) {
        Ok(s) => s,
        Err(e) => {
            warn!("wow64: {PATH} is not a usable image: {e:?}");
            return None;
        }
    };
    section.file_name = super::file::nt_path(node).ok();
    let image = section.image.as_ref()?;
    let (Some(start_rva), Some(transition_rva)) =
        (image.export(b"GalaWow64Start"), image.export(b"GalaWow64Transition"))
    else {
        warn!("wow64: {PATH} lacks GalaWow64Start or GalaWow64Transition");
        return None;
    };
    let section = KObject::new(ObjectBody::Section(section)).ok()?;
    info!(
        "wow64: using {PATH}, GalaWow64Start at rva {start_rva:#x}, GalaWow64Transition at rva \
         {transition_rva:#x}"
    );
    Some(Dll { section, start_rva, transition_rva })
}

/// A WOW64 process's 32-bit side.
pub struct Wow64Process {
    /// The PEB32 (what ProcessWow64Information answers).
    pub peb32: u64,
    /// GalaWow64Start and GalaWow64Transition in its wow64.dll view (0
    /// until it is mapped).
    pub start: AtomicU64,
    pub transition: AtomicU64,
}

impl Wow64Process {
    pub fn new(peb32: u64) -> Self {
        Wow64Process { peb32, start: AtomicU64::new(0), transition: AtomicU64::new(0) }
    }
}

/// The 32-bit address space limit for an image (`large_address_aware`:
/// 4 GiB, else 2).
pub fn limit(large_address_aware: bool) -> u64 {
    if large_address_aware { w::LIMIT_LARGE } else { w::LIMIT }
}

/// What the PEB32 holds (the 64-bit PEB's values, at the x86 offsets).
pub struct Peb32 {
    pub image_base: u32,
    pub params: u32,
    pub cpus: u32,
    pub subsystem: u16,
    pub subsystem_version: (u16, u16),
    pub session: u32,
}

/// The PEB32's bytes.
pub fn peb32_bytes(p: &Peb32) -> [u8; 0x200] {
    let mut b = [0u8; 0x200];
    let mut put = |off: u64, v: &[u8]| b[off as usize..off as usize + v.len()].copy_from_slice(v);
    put(peb32::IMAGE_BASE_ADDRESS, &p.image_base.to_le_bytes());
    put(peb32::PROCESS_PARAMETERS, &p.params.to_le_bytes());
    put(peb32::NUMBER_OF_PROCESSORS, &p.cpus.to_le_bytes());
    put(peb32::OS_MAJOR_VERSION, &super::kuser::MAJOR_VERSION.to_le_bytes());
    put(peb32::OS_MINOR_VERSION, &super::kuser::MINOR_VERSION.to_le_bytes());
    put(peb32::OS_BUILD_NUMBER, &(super::kuser::BUILD_NUMBER as u16).to_le_bytes());
    put(peb32::OS_PLATFORM_ID, &2u32.to_le_bytes()); // VER_PLATFORM_WIN32_NT
    put(peb32::IMAGE_SUBSYSTEM, &u32::from(p.subsystem).to_le_bytes());
    put(peb32::IMAGE_SUBSYSTEM_MAJOR_VERSION, &u32::from(p.subsystem_version.0).to_le_bytes());
    put(peb32::IMAGE_SUBSYSTEM_MINOR_VERSION, &u32::from(p.subsystem_version.1).to_le_bytes());
    put(peb32::SESSION_ID, &p.session.to_le_bytes());
    b
}

/// Maps wow64.dll into `process` (a WOW64 process) and records its entry
/// points; the view lands below the process's limit, where 32-bit code
/// can reach the transition.
pub fn map_dll(process: &Process) -> Result<(), NtStatus> {
    let Some(state) = process.wow64.as_ref() else { return Err(NtStatus::INVALID_PARAMETER) };
    let Some(dll) = dll() else {
        warn!("wow64: {} is a 32-bit program, and there is no {PATH} to run it", process.name);
        return Err(NtStatus::INVALID_IMAGE_FORMAT);
    };
    let view = super::section::map_view(
        process,
        &dll.section,
        0,
        0,
        0,
        vad::PAGE_EXECUTE_WRITECOPY,
        false,
    )?;
    let transition = view.base + u64::from(dll.transition_rva);
    if transition > u64::from(u32::MAX) {
        return Err(NtStatus::CONFLICTING_ADDRESSES);
    }
    state.start.store(view.base + u64::from(dll.start_rva), Ordering::Relaxed);
    state.transition.store(transition, Ordering::Relaxed);
    Ok(())
}

/// How a WOW64 thread starts in 32-bit code.
#[derive(Clone, Copy, Debug)]
pub struct Start32 {
    pub eip: u32,
    pub esp: u32,
    pub eax: u32,
    pub ebx: u32,
    /// Its 32-bit stack: (base = top, limit, reservation bottom).
    pub stack: (u32, u32, u32),
    /// Top of its 64-bit stack (wow64.dll's code).
    pub stack64: u64,
}

/// The CPU area's initial bytes: the 32-bit context `start` describes.
pub fn cpu_area(start: &Start32) -> [u8; cpu::SIZE as usize] {
    let mut a = [0u8; cpu::SIZE as usize];
    let mut put = |off: u64, v: &[u8]| a[off as usize..off as usize + v.len()].copy_from_slice(v);
    put(cpu::MACHINE, &MACHINE_I386.to_le_bytes());
    let c = cpu::CONTEXT;
    put(c + ctx::CONTEXT_FLAGS, &ctx::FLAGS_FULL.to_le_bytes());
    for (off, v) in [
        (ctx::SEG_GS, w::DS32),
        (ctx::SEG_FS, w::FS32),
        (ctx::SEG_ES, w::DS32),
        (ctx::SEG_DS, w::DS32),
        (ctx::SEG_CS, w::CS32),
        (ctx::SEG_SS, w::DS32),
        (ctx::EAX, start.eax),
        (ctx::EBX, start.ebx),
        (ctx::EIP, start.eip),
        (ctx::ESP, start.esp),
        (ctx::EFLAGS, 0x202),
    ] {
        put(c + off, &v.to_le_bytes());
    }
    put(cpu::STACK64, &start.stack64.to_le_bytes());
    a
}

/// Gives the new thread whose 64-bit TEB is at `teb` its TEB32 (below the
/// process's limit) and CPU area; returns the TEB32's address. The
/// 32-bit start and the 64-bit stack are `start`'s.
pub fn allocate_teb32(
    process: &Process,
    state: &Wow64Process,
    teb: u64,
    id: u64,
    start: &Start32,
) -> Result<u64, NtStatus> {
    let addr = {
        let mut vad = process.vad.lock();
        let (addr, _) =
            vad.reserve(None, teb32::SIZE, vad::PAGE_READWRITE, vad::MEM_PRIVATE, true)?;
        vad.commit(addr, teb32::SIZE, vad::PAGE_READWRITE)?;
        let flags = super::process::user_rw_flags();
        let mapped =
            process.space.lock().map_zeroed(addr, teb32::SIZE / platform::PAGE_SIZE, flags);
        mapped.map_err(|_| NtStatus::NO_MEMORY)?;
        addr
    };
    let mut t = [0u8; 0x100];
    let mut put =
        |off: u64, v: u32| t[off as usize..off as usize + 4].copy_from_slice(&v.to_le_bytes());
    put(teb32::EXCEPTION_LIST, u32::MAX); // the end of the SEH chain
    put(teb32::STACK_BASE, start.stack.0);
    put(teb32::STACK_LIMIT, start.stack.1);
    put(teb32::SELF, addr as u32);
    put(teb32::CLIENT_ID_PROCESS, process.id as u32);
    put(teb32::CLIENT_ID_THREAD, id as u32);
    put(teb32::PEB, state.peb32 as u32);
    put(teb32::WOW32_RESERVED, state.transition.load(Ordering::Relaxed) as u32);
    let fault = |_| NtStatus::NO_MEMORY;
    process.write(addr, &t).map_err(fault)?;
    process.write(addr + teb32::DEALLOCATION_STACK, &start.stack.2.to_le_bytes()).map_err(fault)?;
    // The 64-bit TEB: ExceptionList -> the TEB32, TlsSlots[1] -> the CPU
    // area (WOW64_TLS_CPURESERVED), the area itself.
    let area = teb + w::TEB64_CPU_AREA;
    process.write_u64(teb + w::TEB64_EXCEPTION_LIST, addr).map_err(fault)?;
    process.write_u64(teb + w::TEB64_CPU_SLOT, area).map_err(fault)?;
    process.write(area, &cpu_area(start)).map_err(fault)?;
    Ok(addr)
}

/// The 32-bit registers a WOW64 thread's CPU area holds (the caller of
/// its current system call, or where it starts): (eip, esp, eax, ebx,
/// ecx, edx, esi, edi, ebp, eflags). None if unreadable.
pub fn saved_registers(process: &Process, teb: u64) -> Option<[u32; 10]> {
    let area = process.read_u64(teb + w::TEB64_CPU_SLOT).ok()?;
    let mut c = [0u8; ctx::SIZE as usize];
    process.read(area + cpu::CONTEXT, &mut c).ok()?;
    let at = |off: u64| {
        u32::from_le_bytes(c[off as usize..off as usize + 4].try_into().unwrap_or([0; 4]))
    };
    Some([
        at(ctx::EIP),
        at(ctx::ESP),
        at(ctx::EAX),
        at(ctx::EBX),
        at(ctx::ECX),
        at(ctx::EDX),
        at(ctx::ESI),
        at(ctx::EDI),
        at(ctx::EBP),
        at(ctx::EFLAGS),
    ])
}

/// Sets up a new WOW64 process's PEB32 and 32-bit parameters in `space`
/// (which `vad` describes): the parameters block `params32` (built for
/// `params32_at`) and the PEB32 at its fixed place. Limits `vad` to the
/// image's 32-bit address space first.
pub fn set_up_space(
    space: &mut super::platform::UserSpace,
    vad: &mut Vad,
    params32_at: u64,
    params32: &[u8],
    peb: &Peb32,
) -> Result<(), NtStatus> {
    let pages = params32.len() as u64 / platform::PAGE_SIZE;
    let flags = super::process::user_rw_flags();
    super::vm::register(
        vad,
        params32_at,
        pages * platform::PAGE_SIZE,
        vad::MEM_PRIVATE,
        vad::PAGE_READWRITE,
    )?;
    space.map_zeroed(params32_at, pages, flags).map_err(|_| NtStatus::NO_MEMORY)?;
    space.write_initial(params32_at, params32).map_err(|_| NtStatus::NO_MEMORY)?;
    super::vm::register(vad, w::PEB32_ADDRESS, peb32::SIZE, vad::MEM_PRIVATE, vad::PAGE_READWRITE)?;
    space
        .map_zeroed(w::PEB32_ADDRESS, peb32::SIZE / platform::PAGE_SIZE, flags)
        .map_err(|_| NtStatus::NO_MEMORY)?;
    space.write_initial(w::PEB32_ADDRESS, &peb32_bytes(peb)).map_err(|_| NtStatus::NO_MEMORY)?;
    Ok(())
}
