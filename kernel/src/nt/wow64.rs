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
use crate::mm::heap::TryVec;
use crate::sync::IrqMutex;
use alloc::vec::Vec;

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
    /// GalaWow64Exception: the dispatcher of exceptions in wow64.dll's own
    /// 64-bit code (as ntdll's KiUserExceptionDispatcher; it recovers from
    /// faults on a caller's invalid 32-bit pointers) and, through it, in
    /// 32-bit code (`deliver_to_user`).
    pub exception_rva: u32,
    /// GalaWow64Apc: where user APCs run (as ntdll's KiUserApcDispatcher),
    /// handed on to the 32-bit ntdll's.
    pub apc_rva: u32,
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
    let (Some(start_rva), Some(transition_rva), Some(exception_rva), Some(apc_rva)) = (
        image.export(b"GalaWow64Start"),
        image.export(b"GalaWow64Transition"),
        image.export(b"GalaWow64Exception"),
        image.export(b"GalaWow64Apc"),
    ) else {
        warn!(
            "wow64: {PATH} lacks GalaWow64Start, GalaWow64Transition, GalaWow64Exception or \
             GalaWow64Apc"
        );
        return None;
    };
    let section = KObject::new(ObjectBody::Section(section)).ok()?;
    info!(
        "wow64: using {PATH}, GalaWow64Start at rva {start_rva:#x}, GalaWow64Transition at rva \
         {transition_rva:#x}, GalaWow64Exception at rva {exception_rva:#x}, GalaWow64Apc at rva \
         {apc_rva:#x}"
    );
    Some(Dll { section, start_rva, transition_rva, exception_rva, apc_rva })
}

/// A WOW64 process's 32-bit side.
pub struct Wow64Process {
    /// The PEB32 (what ProcessWow64Information answers).
    pub peb32: u64,
    /// GalaWow64Start and GalaWow64Transition in its wow64.dll view (0
    /// until it is mapped).
    pub start: AtomicU64,
    pub transition: AtomicU64,
    /// The 32-bit ntdll's KiUserExceptionDispatcher and
    /// KiUserApcDispatcher (0 until it is mapped, or without one): every
    /// thread's CPU area names them for wow64.dll.
    pub dispatcher32: AtomicU64,
    pub apc_dispatcher32: AtomicU64,
}

impl Wow64Process {
    pub fn new(peb32: u64) -> Self {
        Wow64Process {
            peb32,
            start: AtomicU64::new(0),
            transition: AtomicU64::new(0),
            dispatcher32: AtomicU64::new(0),
            apc_dispatcher32: AtomicU64::new(0),
        }
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
    // Exceptions go to its dispatcher: those in its own 64-bit code, and
    // those in 32-bit code, which it hands to the 32-bit ntdll's
    // (`process::deliver_to_user`); user APCs to its APC dispatcher.
    process.exception_dispatcher.store(view.base + u64::from(dll.exception_rva), Ordering::Relaxed);
    process.apc_dispatcher.store(view.base + u64::from(dll.apc_rva), Ordering::Relaxed);
    Ok(())
}

/// How a WOW64 thread starts in 32-bit code.
#[derive(Clone, Copy, Debug)]
pub struct Start32 {
    pub eip: u32,
    pub esp: u32,
    /// eax, ecx, edx, ebx, esi, edi, ebp.
    pub gpr: [u32; 7],
    /// Its 32-bit stack: (base = top, limit, reservation bottom).
    pub stack: (u32, u32, u32),
    /// Top of its 64-bit stack (wow64.dll's code).
    pub stack64: u64,
}

/// The CPU area's initial bytes: the 32-bit context `start` describes.
pub fn cpu_area(start: &Start32) -> [u8; cpu::SIZE as usize] {
    let mut a = [0u8; cpu::SIZE as usize];
    a[cpu::MACHINE as usize..cpu::MACHINE as usize + 2]
        .copy_from_slice(&MACHINE_I386.to_le_bytes());
    let c = cpu::CONTEXT as usize;
    a[c..c + ctx::SIZE as usize].copy_from_slice(&context32(start));
    let s = cpu::STACK64 as usize;
    a[s..s + 8].copy_from_slice(&start.stack64.to_le_bytes());
    a
}

/// The x86 CONTEXT (CONTEXT_FULL: Windows' selectors) of the registers
/// `start` describes.
pub fn context32(start: &Start32) -> [u8; ctx::SIZE as usize] {
    let mut a = [0u8; ctx::SIZE as usize];
    let mut put = |off: u64, v: &[u8]| a[off as usize..off as usize + v.len()].copy_from_slice(v);
    put(ctx::CONTEXT_FLAGS, &ctx::FLAGS_FULL.to_le_bytes());
    for (off, v) in [
        (ctx::SEG_GS, w::DS32),
        (ctx::SEG_FS, w::FS32),
        (ctx::SEG_ES, w::DS32),
        (ctx::SEG_DS, w::DS32),
        (ctx::SEG_CS, w::CS32),
        (ctx::SEG_SS, w::DS32),
        (ctx::EAX, start.gpr[0]),
        (ctx::ECX, start.gpr[1]),
        (ctx::EDX, start.gpr[2]),
        (ctx::EBX, start.gpr[3]),
        (ctx::ESI, start.gpr[4]),
        (ctx::EDI, start.gpr[5]),
        (ctx::EBP, start.gpr[6]),
        (ctx::EIP, start.eip),
        (ctx::ESP, start.esp),
        (ctx::EFLAGS, 0x202),
    ] {
        put(off, &v.to_le_bytes());
    }
    a
}

/// With a 32-bit ntdll (`ntdll::get32`), the main thread of a WOW64
/// process starts in it, as the 64-bit ntdll's does: at its
/// LdrInitializeThunk(Context, NtdllBase) (stdcall, 32-bit code), the x86
/// CONTEXT of `start` (its real start, which the loader resumes with
/// NtContinue once the imports are bound) on the 32-bit stack below
/// `start`'s esp, then the thunk's arguments above a null return address.
/// Maps the 32-bit ntdll's view. None without a 32-bit ntdll.
pub fn through_ntdll32(process: &Process, start: &Start32) -> Result<Option<Start32>, NtStatus> {
    let Some(ntdll) = super::ntdll::get32() else { return Ok(None) };
    let view = super::section::map_view(
        process,
        &ntdll.section,
        0,
        0,
        0,
        vad::PAGE_EXECUTE_WRITECOPY,
        false,
    )?;
    if view.base + u64::from(ntdll.thunk_rva) > u64::from(u32::MAX) {
        return Err(NtStatus::CONFLICTING_ADDRESSES);
    }
    // Its dispatchers, which wow64.dll hands 32-bit code's exceptions and
    // APCs to (every thread's CPU area names them: `allocate_teb32`).
    if let Some(state) = process.wow64.as_ref() {
        let at = |rva: Option<u32>| rva.map_or(0, |r| view.base + u64::from(r));
        state.dispatcher32.store(at(ntdll.dispatcher_rva), Ordering::Relaxed);
        state.apc_dispatcher32.store(at(ntdll.apc_dispatcher_rva), Ordering::Relaxed);
    }
    let context_at = (u64::from(start.esp) - ctx::SIZE) & !0xf;
    let fault = |_| NtStatus::NO_MEMORY;
    process.write(context_at, &context32(start)).map_err(fault)?;
    let esp = context_at - 12;
    let mut frame = [0u8; 12];
    frame[4..8].copy_from_slice(&(context_at as u32).to_le_bytes());
    frame[8..12].copy_from_slice(&(view.base as u32).to_le_bytes());
    process.write(esp, &frame).map_err(fault)?;
    Ok(Some(Start32 {
        eip: (view.base + u64::from(ntdll.thunk_rva)) as u32,
        esp: esp as u32,
        gpr: [0; 7],
        ..*start
    }))
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
    let dispatchers = [&state.dispatcher32, &state.apc_dispatcher32];
    for (off, d) in [cpu::DISPATCHER32, cpu::APC_DISPATCHER32].into_iter().zip(dispatchers) {
        let d = d.load(Ordering::Relaxed) as u32;
        process.write(area + off, &d.to_le_bytes()).map_err(fault)?;
    }
    Ok(addr)
}

/// Where the kernel hands an exception in 32-bit code of the WOW64 thread
/// whose 64-bit TEB is at `teb`: the top of its 64-bit stack (wow64.dll's
/// dispatcher runs there and passes it on to the 32-bit ntdll's), if the
/// process has a 32-bit ntdll to dispatch it; None otherwise (the
/// exception then ends the process).
pub fn exception_stack(process: &Process, teb: u64) -> Option<u64> {
    let area = process.read_u64(teb + w::TEB64_CPU_SLOT).ok()?;
    let mut d = [0u8; 4];
    process.read(area + cpu::DISPATCHER32, &mut d).ok()?;
    if u32::from_le_bytes(d) == 0 {
        return None;
    }
    process.read_u64(area + cpu::STACK64).ok().filter(|&s| s != 0)
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

/// Reserves a WOW64 thread's 64-bit stack, where wow64.dll's code runs
/// (below the process's 32-bit limit, as the VAD places it): (top, limit,
/// bottom), its top `STACK64_COMMIT` committed above a guard page. Its
/// 64-bit TEB describes it; `release_thread` frees it.
pub fn reserve_stack64(vad: &mut Vad) -> Result<(u64, u64, u64), NtStatus> {
    use super::layout::wow64::{STACK64_COMMIT, STACK64_RESERVE};
    let (bottom, _) =
        vad.reserve(None, STACK64_RESERVE, vad::PAGE_READWRITE, vad::MEM_PRIVATE, false)?;
    let top = bottom + STACK64_RESERVE;
    let limit = top - STACK64_COMMIT;
    let committed = vad.commit(limit, STACK64_COMMIT, vad::PAGE_READWRITE).and_then(|_| {
        let guard = vad::PAGE_READWRITE | vad::PAGE_GUARD;
        vad.commit(limit - platform::PAGE_SIZE, platform::PAGE_SIZE, guard)
    });
    if let Err(e) = committed {
        let _ = vad.release(bottom, 0);
        return Err(e);
    }
    Ok((top, limit, bottom))
}

/// Frees what the kernel gave an exiting WOW64 thread beside its TEB: its
/// 64-bit stack (from its TEB's DeallocationStack) and its TEB32. The
/// thread runs no user code any more; its process goes on (a process's
/// last thread leaves this to the address space's end).
pub fn release_thread(process: &Process, teb: u64, teb32: u64) {
    use super::layout::teb;
    let stack = process.read_u64(teb + teb::DEALLOCATION_STACK);
    for (what, base) in [("64-bit stack", stack.ok()), ("TEB32", Some(teb32))] {
        let Some(base) = base.filter(|&b| b != 0) else { continue };
        if let Err(e) = super::vm::free(process, base, 0, super::vm::MEM_RELEASE) {
            warn!(
                "wow64: {} (pid {}): cannot free an exited thread's {what} at {base:#x}: {e:?}",
                process.name, process.id
            );
        }
    }
}

/// A process's environment, adjusted to its bitness: the variables,
/// each NUL-terminated.
pub struct Environment {
    text: Vec<u8>,
}

impl Environment {
    /// The variables (`name=value`).
    pub fn vars(&self) -> Result<Vec<&str>, NtStatus> {
        let mut out = Vec::new();
        for var in self.text.split(|&b| b == 0).filter(|v| !v.is_empty()) {
            let var = core::str::from_utf8(var).map_err(|_| NtStatus::INVALID_PARAMETER)?;
            out.try_push(var).map_err(|_| NtStatus::NO_MEMORY)?;
        }
        Ok(out)
    }
}

/// The environment a new process starts with, from `vars` (the system's
/// or its creator's), as WOW64 sets it for the process's bitness
/// (Microsoft Learn, "WOW64 Implementation Details", environment
/// variables). A 32-bit process: PROCESSOR_ARCHITECTURE=x86,
/// PROCESSOR_ARCHITEW6432 the native one, ProgramFiles and
/// CommonProgramFiles the (x86) directories, ProgramW6432 and
/// CommonProgramW6432 the native ones. A 64-bit process (a 32-bit parent's
/// variables back): PROCESSOR_ARCHITECTURE from PROCESSOR_ARCHITEW6432
/// (which goes), ProgramFiles and CommonProgramFiles from the W6432 ones.
/// Variables keep their place; new ones go where they sort.
pub fn environment<'a>(vars: &[&'a str], x86: bool) -> Result<Environment, NtStatus> {
    const ARCH: &str = "PROCESSOR_ARCHITECTURE";
    const ARCH_W6432: &str = "PROCESSOR_ARCHITEW6432";
    const PF: &str = "ProgramFiles";
    const PF_X86: &str = "ProgramFiles(x86)";
    const PF_W6432: &str = "ProgramW6432";
    const CPF: &str = "CommonProgramFiles";
    const CPF_X86: &str = "CommonProgramFiles(x86)";
    const CPF_W6432: &str = "CommonProgramW6432";
    let get = |name: &str| -> Option<&'a str> {
        vars.iter().find_map(|v| {
            v.split_once('=').filter(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, value)| value)
        })
    };
    // (name, new value: None removes it).
    let mut changes: [(&str, Option<&'a str>); 6] = [("", None); 6];
    let mut n = 0;
    let mut change = |name: &'static str, value: Option<&'a str>| {
        changes[n] = (name, value);
        n += 1;
    };
    if x86 {
        change(ARCH, Some("x86"));
        if let Some(native) = get(ARCH_W6432).or(get(ARCH)) {
            change(ARCH_W6432, Some(native));
        }
        if let Some(native) = get(PF_W6432).or(get(PF)) {
            change(PF_W6432, Some(native));
        }
        if let Some(x86) = get(PF_X86) {
            change(PF, Some(x86));
        }
        if let Some(native) = get(CPF_W6432).or(get(CPF)) {
            change(CPF_W6432, Some(native));
        }
        if let Some(x86) = get(CPF_X86) {
            change(CPF, Some(x86));
        }
    } else {
        if let Some(native) = get(ARCH_W6432) {
            change(ARCH, Some(native));
            change(ARCH_W6432, None);
        }
        if let Some(native) = get(PF_W6432) {
            change(PF, Some(native));
        }
        if let Some(native) = get(CPF_W6432) {
            change(CPF, Some(native));
        }
    }
    let changes = &changes[..n];
    let changed = |var: &str| {
        let name = var.split_once('=').map_or(var, |(n, _)| n);
        changes.iter().find(|(c, _)| c.eq_ignore_ascii_case(name))
    };
    fn upper(s: &str) -> impl Iterator<Item = u8> + '_ {
        s.bytes().map(|b| b.to_ascii_uppercase())
    }
    let mut text = Vec::new();
    let push = |text: &mut Vec<u8>, parts: &[&str]| -> Result<(), NtStatus> {
        for p in parts {
            text.try_extend_from_slice(p.as_bytes()).map_err(|_| NtStatus::NO_MEMORY)?;
        }
        text.try_push(0).map_err(|_| NtStatus::NO_MEMORY)
    };
    // New variables, in sorted order, before the first that sorts after.
    let mut new: [(&str, &str); 6] = [("", ""); 6];
    let mut new_count = 0;
    for &(name, value) in changes {
        if let Some(value) = value
            && get(name).is_none()
        {
            new[new_count] = (name, value);
            new_count += 1;
        }
    }
    let new = &mut new[..new_count];
    new.sort_by(|a, b| upper(a.0).cmp(upper(b.0)));
    let mut next = 0;
    for var in vars {
        let name = var.split_once('=').map_or(*var, |(n, _)| n);
        while next < new.len() && !name.starts_with('=') && upper(new[next].0).lt(upper(name)) {
            push(&mut text, &[new[next].0, "=", new[next].1])?;
            next += 1;
        }
        match changed(var) {
            Some(&(name, Some(value))) => push(&mut text, &[name, "=", value])?,
            Some((_, None)) => {}
            None => push(&mut text, &[var])?,
        }
    }
    for &(name, value) in &new[next..] {
        push(&mut text, &[name, "=", value])?;
    }
    Ok(Environment { text })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYSTEM: &[&str] = &[
        "CommonProgramFiles=C:\\Program Files\\Common Files",
        "CommonProgramFiles(x86)=C:\\Program Files (x86)\\Common Files",
        "CommonProgramW6432=C:\\Program Files\\Common Files",
        "OS=Windows_NT",
        "PROCESSOR_ARCHITECTURE=AMD64",
        "ProgramFiles=C:\\Program Files",
        "ProgramFiles(x86)=C:\\Program Files (x86)",
        "ProgramW6432=C:\\Program Files",
        "windir=C:\\Windows",
    ];

    #[test]
    fn a_32_bit_process_sees_the_x86_directories() {
        let env = environment(SYSTEM, true).unwrap();
        assert_eq!(
            env.vars().unwrap(),
            [
                "CommonProgramFiles=C:\\Program Files (x86)\\Common Files",
                "CommonProgramFiles(x86)=C:\\Program Files (x86)\\Common Files",
                "CommonProgramW6432=C:\\Program Files\\Common Files",
                "OS=Windows_NT",
                "PROCESSOR_ARCHITECTURE=x86",
                "PROCESSOR_ARCHITEW6432=AMD64",
                "ProgramFiles=C:\\Program Files (x86)",
                "ProgramFiles(x86)=C:\\Program Files (x86)",
                "ProgramW6432=C:\\Program Files",
                "windir=C:\\Windows",
            ]
        );
        // A 32-bit child of a 32-bit process: the same.
        let again = environment(&env.vars().unwrap(), true).unwrap();
        assert_eq!(again.vars().unwrap(), env.vars().unwrap());
        // A 64-bit child of a 32-bit process: the native values back.
        let back = environment(&env.vars().unwrap(), false).unwrap();
        assert_eq!(back.vars().unwrap(), SYSTEM);
    }

    #[test]
    fn a_64_bit_process_keeps_the_system_environment() {
        assert_eq!(environment(SYSTEM, false).unwrap().vars().unwrap(), SYSTEM);
        // Without the W6432 variables (an environment given by a program):
        // a 32-bit process gets them, in sorted places.
        let env =
            environment(&["=C:=C:\\", "A=1", "PROCESSOR_ARCHITECTURE=AMD64", "Z=2"], true).unwrap();
        assert_eq!(
            env.vars().unwrap(),
            [
                "=C:=C:\\",
                "A=1",
                "PROCESSOR_ARCHITECTURE=x86",
                "PROCESSOR_ARCHITEW6432=AMD64",
                "Z=2"
            ]
        );
    }

    #[test]
    fn cpu_area_holds_the_32_bit_start() {
        let start = Start32 {
            eip: 0x40_1000,
            esp: 0x1f_f000,
            gpr: [1, 2, 3, 4, 5, 6, 7],
            stack: (0x20_0000, 0x1f_0000, 0x10_0000),
            stack64: 0x7000_0000,
        };
        let a = cpu_area(&start);
        let at = |off: u64| {
            let o = (cpu::CONTEXT + off) as usize;
            u32::from_le_bytes([a[o], a[o + 1], a[o + 2], a[o + 3]])
        };
        for (off, want) in [
            (ctx::EAX, 1),
            (ctx::ECX, 2),
            (ctx::EDX, 3),
            (ctx::EBX, 4),
            (ctx::ESI, 5),
            (ctx::EDI, 6),
            (ctx::EBP, 7),
            (ctx::EIP, 0x40_1000),
            (ctx::ESP, 0x1f_f000),
            (ctx::SEG_CS, w::CS32),
            (ctx::SEG_SS, w::DS32),
            (ctx::SEG_FS, w::FS32),
            (ctx::EFLAGS, 0x202),
        ] {
            assert_eq!(at(off), want, "context offset {off:#x}");
        }
        assert_eq!(u16::from_le_bytes([a[2], a[3]]), MACHINE_I386);
        let s = cpu::STACK64 as usize;
        assert_eq!(u64::from_le_bytes(a[s..s + 8].try_into().unwrap()), 0x7000_0000);
        assert_eq!(a.len() as u64, cpu::SIZE);
    }
}
