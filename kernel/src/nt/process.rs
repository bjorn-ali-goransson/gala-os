//! Processes: a user address space, a handle table and threads. Includes
//! the loader for freestanding PE executables (no imports yet).

use super::platform;
use alloc::vec::Vec;
use core::fmt;

use super::layout::{self, peb};
use super::object::{HandleTable, KObject, ObjectBody, ObjectRef};
use super::pe::{PeError, PeImage};
use super::platform::PAGE_SIZE;
use super::platform::PageFlags;
use super::platform::SpawnError;
use super::platform::TrapFrame;
use super::platform::UserRegs;
use super::platform::{PagingError, SharedUserSpace, UserFault, UserSpace};
use super::status::NtStatus;
use super::thread;
use super::vad::{self, Vad};
use super::vm::{self, AccessKind};
use super::wait::{self, Dispatcher};
use crate::mm::heap::{AllocError, KArc, TryVec};
use crate::sync::IrqMutex;

pub struct Process {
    pub id: u64,
    /// The creating process's id (0: started by the kernel).
    pub parent_id: u64,
    pub name: &'static str,
    /// The Win32 path of the executable (`C:\\gala\\x.exe`).
    pub path: &'static str,
    pub space: SharedUserSpace,
    pub handles: IrqMutex<HandleTable>,
    /// Lock order: `vad` before `space`.
    pub vad: IrqMutex<Vad>,
    /// Mapped section views (lock order: `views`, then `vad`, then `space`).
    pub views: IrqMutex<Vec<super::section::View>>,
    /// Thread objects of the live threads.
    threads: IrqMutex<Vec<ObjectRef>>,
    exit_status: IrqMutex<Option<NtStatus>>,
    /// Signaled when the process has ended.
    pub dispatcher: Dispatcher,
    /// The executable's mapping: (base, size).
    pub image: (u64, u64),
    /// Its layout, whose shared frames back the executable's pages (held
    /// for as long as they are mapped).
    _layout: KArc<super::section::ImageLayout>,
    /// ntdll!KiUserExceptionDispatcher in this process, or 0: then user
    /// exceptions end the process.
    exception_dispatcher: core::sync::atomic::AtomicU64,
    /// ntdll!KiUserApcDispatcher in this process, or 0 (no user APCs run).
    pub apc_dispatcher: core::sync::atomic::AtomicU64,
    /// NT times (ProcessTimes); exit time 0 while it runs.
    pub create_time: u64,
    pub exit_time: IrqMutex<u64>,
    /// The job object the process is in (nt::job).
    pub job: IrqMutex<Option<ObjectRef>>,
    /// A page fault found no free frame (logged once).
    pub oom_logged: core::sync::atomic::AtomicBool,
}

impl fmt::Debug for Process {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Process({} pid {})", self.name, self.id)
    }
}

impl Process {
    /// The NT path of the executable (`\\Device\\HarddiskVolume1\\gala\\x.exe`).
    pub fn image_nt_path(&self) -> Result<Vec<u16>, NtStatus> {
        let mut name = Vec::new();
        // The volume device instead of the drive letter.
        let rest = self.path.get(2..).unwrap_or(self.path);
        for part in [super::file::VOLUME_DEVICE, rest] {
            for b in part.bytes() {
                name.try_push(u16::from(b)).map_err(|_| NtStatus::NO_MEMORY)?;
            }
        }
        Ok(name)
    }

    pub fn exit_status(&self) -> Option<NtStatus> {
        *self.exit_status.lock()
    }

    /// Adds a new thread, unless the process is terminating: `terminate`
    /// records the exit status before it snapshots the threads, so checking
    /// it under the threads lock means a thread created concurrently is
    /// either refused or in the snapshot (and asked to terminate). Go's
    /// runtime creating a thread while another called ExitProcess left a
    /// process that never ended.
    pub fn add_thread(&self, thread: ObjectRef) -> Result<(), NtStatus> {
        let mut threads = self.threads.lock();
        if self.exit_status.lock().is_some() {
            return Err(NtStatus::PROCESS_IS_TERMINATING);
        }
        threads.try_push(thread).map_err(|_| NtStatus::NO_MEMORY)
    }

    /// Removes `thread`; whether it was the last one (decided under the
    /// lock, so that of threads exiting at once exactly one is last).
    pub fn remove_thread(&self, thread: &ObjectRef) -> bool {
        let mut threads = self.threads.lock();
        let before = threads.len();
        threads.retain(|t| !KArc::ptr_eq(t, thread));
        before != threads.len() && threads.is_empty()
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
                Err(fault)
                    if vm::resolve_fault(self, fault.address, kind) == vm::Fault::Resolved =>
                {
                    continue;
                }
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

/// Every process object: running ones, and exited ones while something
/// else (a handle, a thread object) still refers to them, so they can be
/// opened by id as on Windows. `prune` forgets the rest.
static PROCESSES: IrqMutex<Vec<KArc<Process>>> = IrqMutex::new(Vec::new());

/// Drops exited processes that only PROCESSES refers to (outside the lock:
/// dropping one frees its address space).
pub fn prune() {
    loop {
        let victim = {
            let mut list = PROCESSES.lock();
            let Some(i) =
                list.iter().position(|p| *p.exit_time.lock() != 0 && KArc::strong_count(p) == 1)
            else {
                return;
            };
            list.swap_remove(i)
        };
        drop(victim);
    }
}

/// The running processes (for SystemProcessInformation).
pub fn running() -> Result<Vec<KArc<Process>>, AllocError> {
    let list = PROCESSES.lock();
    let mut out = Vec::new();
    out.try_reserve(list.len()).map_err(|_| AllocError)?;
    for p in list.iter().filter(|p| *p.exit_time.lock() == 0) {
        out.try_push(p.clone())?;
    }
    Ok(out)
}

/// The process with id `id` (running, or exited and still referenced).
pub fn by_id(id: u64) -> Option<KArc<Process>> {
    prune();
    PROCESSES.lock().iter().find(|p| p.id == id).cloned()
}

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

/// Page flags for an image section: writable sections are copy-on-write,
/// mapped read-only until written.
const USER_RW: PageFlags = PageFlags { user: true, global: false, ..PageFlags::KERNEL_RW };

fn put_u64(space: &mut UserSpace, addr: u64, value: u64) -> Result<(), UserFault> {
    space.write_initial(addr, &value.to_le_bytes())
}

fn put_u32(space: &mut UserSpace, addr: u64, value: u32) -> Result<(), UserFault> {
    space.write_initial(addr, &value.to_le_bytes())
}

/// Maps the image at its preferred base.
/// Maps the executable at its preferred base: every page borrows the
/// layout's shared frame (read-only, or copy-on-write for writable
/// sections, which a first write makes private: `vm::resolve_fault`), so
/// processes of the same program share its pages, as image views do.
fn map_image(
    space: &mut UserSpace,
    vad: &mut Vad,
    image: &super::section::ImageLayout,
) -> Result<u64, LoadError> {
    let base = image.base;
    let size = image.size();
    let end = base.checked_add(size).ok_or(LoadError::BadImageBase)?;
    // Keep the first 64 KiB free (null-pointer region) and stay clear of
    // our fixed structures.
    if base < 0x1_0000 || end > layout::PEB_ADDRESS {
        return Err(LoadError::BadImageBase);
    }
    // TODO: relocate when the preferred base is taken.
    vad.reserve(Some(base), size, vad::PAGE_EXECUTE_WRITECOPY, vad::MEM_IMAGE, false)?;
    for (i, &protect) in image.page_protect().iter().enumerate() {
        let va = base + i as u64 * PAGE_SIZE;
        vad.commit(va, PAGE_SIZE, protect)?;
        let frame = image.shared_frame(i).ok_or(LoadError::OutOfMemory)?;
        space.map_borrowed(va, frame, vm::flags_for(protect)).map_err(LoadError::Paging)?;
    }
    Ok(base)
}

const VER_PLATFORM_WIN32_NT: u32 = 2;
/// Interactive programs run in session 1 (session 0 is for services).
pub const SESSION_ID: u32 = 1;
/// Boot modules live here; it is also the initial current directory.
const BOOT_DIRECTORY: &str = "C:\\gala\\";
/// FILE_TRAVERSE | SYNCHRONIZE, as Windows opens the current directory.
const CURDIR_ACCESS: u32 = 0x20 | super::object::SYNCHRONIZE;
/// `NAME=value` in `buf` (`name` includes the `=`; it must fit with ten
/// digits).
fn number_variable<'a>(buf: &'a mut [u8; 24], name: &'a str, value: u32) -> &'a str {
    buf[..name.len()].copy_from_slice(name.as_bytes());
    let mut digits = [0u8; 10];
    let mut n = 0;
    let mut v = value;
    loop {
        digits[n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    for i in 0..n {
        buf[name.len() + i] = digits[n - 1 - i];
    }
    core::str::from_utf8(&buf[..name.len() + n]).unwrap_or(name)
}

/// The initial environment: the system's variables and the profile of
/// the user `gala` (directories made by `file::init`), as a Windows logon
/// sets them.
const ENVIRONMENT: &[&str] = &[
    "ALLUSERSPROFILE=C:\\ProgramData",
    "APPDATA=C:\\Users\\gala\\AppData\\Roaming",
    "ComSpec=C:\\Windows\\System32\\cmd.exe",
    "COMPUTERNAME=GALA",
    "HOMEDRIVE=C:",
    "HOMEPATH=\\Users\\gala",
    "LOCALAPPDATA=C:\\Users\\gala\\AppData\\Local",
    "NUMBER_OF_PROCESSORS=", // completed with the count in `load`
    "OS=Windows_NT",
    "Path=C:\\Windows\\System32;C:\\Windows",
    "PROCESSOR_ARCHITECTURE=AMD64",
    "ProgramData=C:\\ProgramData",
    "ProgramFiles=C:\\Program Files",
    "PUBLIC=C:\\Users\\Public",
    "SystemDrive=C:",
    "SystemRoot=C:\\Windows",
    "TEMP=C:\\Users\\Public",
    "TMP=C:\\Users\\Public",
    "USERDOMAIN=GALA",
    "USERNAME=gala",
    "USERPROFILE=C:\\Users\\gala",
    "windir=C:\\Windows",
];

/// Opens a directory (`C:\\...`, Win32 form) as a directory file object.
fn open_directory(win32: &str) -> Option<ObjectRef> {
    use super::file;
    let rest = win32.get(2..)?.trim_end_matches('\\');
    let mut name = Vec::new();
    for c in rest.encode_utf16() {
        name.try_push(c).ok()?;
    }
    let path = file::FilePath { start: crate::fs::ramfs::ROOT, name };
    let share = file::FILE_SHARE_READ | file::FILE_SHARE_WRITE | file::FILE_SHARE_DELETE;
    let options = file::FILE_DIRECTORY_FILE | file::FILE_SYNCHRONOUS_IO_NONALERT;
    file::create(&path, CURDIR_ACCESS, share, file::FILE_OPEN, options).ok().map(|c| c.object)
}

/// Names and paths of processes live as long as the kernel: each distinct
/// string is allocated once.
static INTERNED: IrqMutex<Vec<&'static str>> = IrqMutex::new(Vec::new());

pub fn intern(s: &str) -> Result<&'static str, AllocError> {
    let mut table = INTERNED.lock();
    if let Some(&found) = table.iter().find(|&&t| t == s) {
        return Ok(found);
    }
    table.try_reserve(1).map_err(|_| AllocError)?;
    let mut owned: Vec<u8> = Vec::new();
    owned.try_extend_from_slice(s.as_bytes())?;
    let leaked: &'static str = core::str::from_utf8(owned.leak()).unwrap_or("?");
    table.try_push(leaked)?;
    Ok(leaked)
}

/// Creates a process from a PE executable that is a boot module (in
/// `directory` on C:) and starts its main thread. `args` follow the image
/// path on the command line; it starts in `cwd` (a Win32 path, `C:\a\`
/// or `C:/a`), or in C:\gala.
pub fn spawn_from_pe(
    name: &'static str,
    directory: &str,
    data: &[u8],
    args: &str,
    cwd: Option<&str>,
) -> Result<(KArc<Process>, ObjectRef), LoadError> {
    // The current directory with backslashes and a trailing one, as the
    // process parameters keep it.
    let mut current: Vec<u8> = Vec::new();
    match cwd {
        Some(dir) if !dir.is_empty() => {
            for b in dir.bytes() {
                current.try_push(if b == b'/' { b'\\' } else { b })?;
            }
            if current.last() != Some(&b'\\') {
                current.try_push(b'\\')?;
            }
        }
        _ => current.try_extend_from_slice(BOOT_DIRECTORY.as_bytes())?,
    }
    let current_directory = core::str::from_utf8(&current).unwrap_or(BOOT_DIRECTORY);
    // C:\<directory>\<name>, the directory as the boot module names it
    // (`a/b`, empty for C:\gala).
    let mut image_path: Vec<u8> = Vec::new();
    if directory.is_empty() {
        image_path.try_extend_from_slice(BOOT_DIRECTORY.as_bytes())?;
    } else {
        image_path.try_extend_from_slice(b"C:\\")?;
        for part in directory.split(['/', '\\']).filter(|p| !p.is_empty()) {
            image_path.try_extend_from_slice(part.as_bytes())?;
            image_path.try_push(b'\\')?;
        }
    }
    image_path.try_extend_from_slice(name.as_bytes())?;
    let image_path = core::str::from_utf8(&image_path).unwrap_or(name);
    // The command line: the image path (quoted if it has spaces), then the
    // arguments, as a shell would pass them.
    let mut command_line: Vec<u8> = Vec::new();
    let quote = image_path.contains(' ');
    if quote {
        command_line.try_push(b'"')?;
    }
    command_line.try_extend_from_slice(image_path.as_bytes())?;
    if quote {
        command_line.try_push(b'"')?;
    }
    if !args.is_empty() {
        command_line.try_push(b' ')?;
        command_line.try_extend_from_slice(args.as_bytes())?;
    }
    let command_line = core::str::from_utf8(&command_line).unwrap_or(image_path);
    // Parsed first for the precise error; boot programs are not cached.
    PeImage::parse(data).map_err(LoadError::Pe)?;
    let layout = super::section::ImageLayout::new(data)
        .map_err(|_| LoadError::Pe(PeError::BadPeSignature))?;
    spawn(Spawn {
        name,
        path: intern(image_path)?,
        image: KArc::try_new(layout)?,
        command_line,
        current_directory,
        environment: None,
        handles: None,
        suspended: false,
        runtime_data: &[],
        parent_id: 0,
    })
}

/// What a new process starts with.
pub struct Spawn<'a> {
    /// The executable's file name (for logs and events).
    pub name: &'static str,
    /// Its Win32 path.
    pub path: &'static str,
    /// The executable, laid out (`section::image_layout`, cached per file).
    pub image: KArc<super::section::ImageLayout>,
    pub command_line: &'a str,
    /// With a trailing backslash.
    pub current_directory: &'a str,
    /// `NAME=value` strings; None: the boot environment.
    pub environment: Option<&'a [&'a str]>,
    /// The handle table and the standard handles in it; None: new console
    /// handles.
    pub handles: Option<(HandleTable, [u64; 3])>,
    /// The main thread waits for a resume.
    pub suspended: bool,
    /// STARTUPINFO's lpReserved2 bytes for the parameters' RuntimeData.
    pub runtime_data: &'a [u8],
    /// The creating process's id (0: the kernel).
    pub parent_id: u64,
}

/// Creates a process from a PE executable and starts (or prepares) its
/// main thread.
pub fn spawn(spec: Spawn<'_>) -> Result<(KArc<Process>, ObjectRef), LoadError> {
    let started = platform::monotonic_ns();
    let spawned = spawn_inner(spec);
    super::stats::PROCESSES.add_since(started);
    spawned
}

fn spawn_inner(spec: Spawn<'_>) -> Result<(KArc<Process>, ObjectRef), LoadError> {
    let Spawn {
        name,
        path,
        image,
        command_line,
        current_directory,
        environment,
        handles,
        suspended,
        runtime_data,
        parent_id,
    } = spec;
    let info = image.info;
    if info.is_dll {
        return Err(LoadError::Pe(PeError::NotExecutable));
    }
    // Binding imports is the user-mode loader's job (ntdll), as on Windows.
    if info.has_imports && super::ntdll::get().is_none() {
        return Err(LoadError::ImportsNotSupported);
    }
    let mut space = UserSpace::new()?;
    let mut vad = Vad::default();
    let base = map_image(&mut space, &mut vad, &image)?;
    let id = thread::next_id();
    crate::events::emit(
        crate::events::Category::Image,
        "image",
        format_args!(
            r#""pid":{id},"name":{},"base":{base},"size":{}"#,
            crate::events::Str(name),
            info.size_of_image
        ),
    );

    // Main thread stack, laid out as Windows does: a reservation of the
    // header's SizeOfStackReserve (in 64 KiB units) whose top pages
    // (SizeOfStackCommit, at least STACK_COMMIT) are committed, with a guard
    // page below them that moves down as the stack grows
    // (`vm::resolve_fault`); the bottom page stays reserved. It ends at
    // STACK_TOP when it fits below it, else it goes where there is room.
    let reserve = info.stack_reserve.clamp(0x1_0000, layout::MAX_STACK).next_multiple_of(0x1_0000);
    let commit =
        info.stack_commit.max(layout::STACK_COMMIT).next_multiple_of(PAGE_SIZE).min(reserve / 2);
    let preferred = layout::STACK_TOP.checked_sub(reserve).filter(|&b| b >= 0x1_0000);
    let reserved = match preferred {
        Some(b) => vad
            .reserve(Some(b), reserve, vad::PAGE_READWRITE, vad::MEM_PRIVATE, false)
            .or_else(|_| vad.reserve(None, reserve, vad::PAGE_READWRITE, vad::MEM_PRIVATE, false)),
        None => vad.reserve(None, reserve, vad::PAGE_READWRITE, vad::MEM_PRIVATE, false),
    }?;
    let stack_bottom = reserved.0;
    let stack_top = stack_bottom + reserve;
    let stack_limit = stack_top - commit;
    vad.commit(stack_limit, commit, vad::PAGE_READWRITE)?;
    vad.commit(stack_limit - PAGE_SIZE, PAGE_SIZE, vad::PAGE_READWRITE | vad::PAGE_GUARD)?;

    super::kuser::map_into(&mut space, &mut vad)?;

    // Handles: given (inherited), or new ones for the standard streams.
    let (mut handles, std_handles) = match handles {
        Some(given) => given,
        None => {
            let mut handles = HandleTable::default();
            let stdin = handles.insert_object(KObject::new(ObjectBody::ConsoleInput)?)?;
            let output = KObject::new(ObjectBody::ConsoleOutput)?;
            let stdout = handles.insert_object(output.clone())?;
            let stderr = handles.insert_object(output)?;
            (handles, [stdin.0, stdout.0, stderr.0])
        }
    };

    // The current directory, as a handle.
    let cwd_handle = match open_directory(current_directory) {
        Some(object) => handles
            .insert(super::object::HandleEntry { object, access: CURDIR_ACCESS, attributes: 0 })
            .map_or(0, |h| h.0),
        None => {
            warn!("process {name}: cannot open the current directory");
            0
        }
    };

    // RTL_USER_PROCESS_PARAMETERS.
    let cpus = platform::online_count();
    let mut cpus_var = [0u8; 24];
    let cpus_var = number_variable(&mut cpus_var, "NUMBER_OF_PROCESSORS=", cpus);
    let mut environment_default = [""; ENVIRONMENT.len()];
    for (slot, var) in environment_default.iter_mut().zip(ENVIRONMENT) {
        *slot = if *var == "NUMBER_OF_PROCESSORS=" { cpus_var } else { var };
    }
    let p = layout::PARAMS_ADDRESS;
    let block = super::params::build(
        &super::params::ProcessParameters {
            image_path: path,
            command_line,
            current_directory,
            current_directory_handle: cwd_handle,
            dll_path: "",
            std_handles,
            environment: environment.unwrap_or(&environment_default),
            runtime_data,
        },
        p,
    )
    .map_err(|_| LoadError::OutOfMemory)?;
    let pages = block.len() as u64 / PAGE_SIZE;
    vm::register(&mut vad, p, pages * PAGE_SIZE, vad::MEM_PRIVATE, vad::PAGE_READWRITE)?;
    space.map_zeroed(p, pages, USER_RW)?;
    space.write_initial(p, &block)?;

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
    put_u32(&mut space, peb_addr + peb::NUMBER_OF_PROCESSORS, cpus)?;
    put_u32(&mut space, peb_addr + peb::OS_MAJOR_VERSION, super::kuser::MAJOR_VERSION)?;
    put_u32(&mut space, peb_addr + peb::OS_MINOR_VERSION, super::kuser::MINOR_VERSION)?;
    space.write_initial(
        peb_addr + peb::OS_BUILD_NUMBER,
        &(super::kuser::BUILD_NUMBER as u16).to_le_bytes(),
    )?;
    put_u32(&mut space, peb_addr + peb::OS_PLATFORM_ID, VER_PLATFORM_WIN32_NT)?;
    put_u32(&mut space, peb_addr + peb::IMAGE_SUBSYSTEM, u32::from(info.subsystem))?;
    let (major, minor) = info.subsystem_version;
    put_u32(&mut space, peb_addr + peb::IMAGE_SUBSYSTEM_MAJOR_VERSION, u32::from(major))?;
    put_u32(&mut space, peb_addr + peb::IMAGE_SUBSYSTEM_MINOR_VERSION, u32::from(minor))?;
    put_u32(&mut space, peb_addr + peb::SESSION_ID, SESSION_ID)?;

    let space = KArc::try_new(platform::SpaceLock::new(space))?;
    let process = KArc::try_new(Process {
        id,
        parent_id,
        name,
        path,
        space,
        handles: IrqMutex::new(handles),
        vad: IrqMutex::new(vad),
        views: IrqMutex::new(Vec::new()),
        threads: IrqMutex::new(Vec::new()),
        exit_status: IrqMutex::new(None),
        dispatcher: Dispatcher::new(0),
        image: (base, u64::from(info.size_of_image)),
        _layout: image.clone(),
        exception_dispatcher: core::sync::atomic::AtomicU64::new(0),
        apc_dispatcher: core::sync::atomic::AtomicU64::new(0),
        create_time: platform::system_time(),
        exit_time: IrqMutex::new(0),
        job: IrqMutex::new(None),
        oom_logged: core::sync::atomic::AtomicBool::new(false),
    })?;
    prune();
    PROCESSES.lock().try_push(process.clone())?;

    // Main thread: starts at the entry point with rcx = PEB, and rsp as if
    // called (rsp % 16 == 8, 32-byte shadow area above the null return);
    // with an ntdll, at its LdrInitializeThunk with that as the context.
    let mut gpr = [0u64; 16];
    gpr[1] = peb_addr;
    gpr[4] = stack_top - 0x28;
    let start = UserRegs { gpr, rip: base + u64::from(info.entry_rva), rflags: 0 };
    let started = through_ntdll(&process, &start, stack_top).and_then(|regs| {
        thread::create(
            &process,
            regs,
            start.rip,
            None,
            (stack_top, stack_limit, stack_bottom),
            suspended,
        )
        .map(|t| (t, regs))
    });
    let (main, regs) = match started {
        Ok(t) => t,
        Err(_) => {
            PROCESSES.lock().retain(|p| !KArc::ptr_eq(p, &process));
            return Err(LoadError::OutOfMemory);
        }
    };
    info!("process {} (pid {}): starting at {:#x}", name, id, regs.rip);
    crate::events::emit(
        crate::events::Category::Process,
        "process start",
        format_args!(
            r#""pid":{id},"name":{},"image_base":{base},"entry":{}"#,
            crate::events::Str(name),
            regs.rip
        ),
    );
    Ok((process, main))
}

/// The initial registers: `start` itself, or, if there is a system ntdll,
/// its LdrInitializeThunk(rcx = &CONTEXT of `start`, rdx = ntdll base) with
/// the CONTEXT at the top of the stack. The ntdll view is mapped here.
fn through_ntdll(
    process: &Process,
    start: &UserRegs,
    stack_top: u64,
) -> Result<UserRegs, NtStatus> {
    let Some(ntdll) = super::ntdll::get() else { return Ok(*start) };
    let view = super::section::map_view(
        process,
        &ntdll.section,
        0,
        0,
        0,
        vad::PAGE_EXECUTE_WRITECOPY,
        false,
    )?;
    if let Some(rva) = ntdll.dispatcher_rva {
        process
            .exception_dispatcher
            .store(view.base + u64::from(rva), core::sync::atomic::Ordering::Relaxed);
    }
    if let Some(rva) = ntdll.apc_dispatcher_rva {
        process
            .apc_dispatcher
            .store(view.base + u64::from(rva), core::sync::atomic::Ordering::Relaxed);
    }
    let mut entry = *start;
    let context_addr = (stack_top - super::context::SIZE as u64) & !0xf;
    entry.gpr[4] = context_addr - 0x28;
    process
        .write(context_addr, &super::context::to_bytes(&entry, None))
        .map_err(|_| NtStatus::NO_MEMORY)?;
    let mut gpr = [0u64; 16];
    gpr[1] = context_addr;
    gpr[2] = view.base;
    gpr[4] = entry.gpr[4] - 0x30; // call-like alignment, shadow space
    Ok(UserRegs { gpr, rip: view.base + u64::from(ntdll.thunk_rva), rflags: 0 })
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
    *process.exit_time.lock() = platform::system_time();
    info!("process {} (pid {}) exited with status {:#010x}", process.name, process.id, status.0);
    crate::events::emit(
        crate::events::Category::Process,
        "process exit",
        format_args!(r#""pid":{},"status":{}"#, process.id, status.0),
    );
    wait::signal_dispatcher(&process.dispatcher, 1);
    // Close every handle (this breaks reference cycles through handles to
    // the process's own objects) and forget the process.
    let entries = process.handles.lock().close_all();
    drop(entries);
    // Stays in PROCESSES while referenced (OpenProcess by id); `prune`
    // forgets it after that.
}

/// User-mode CPU exceptions: resolved (demand paging, guard pages, COW),
/// delivered to the process's exception dispatcher, or fatal.
fn on_user_exception(frame: &mut TrapFrame, cr2: Option<u64>) {
    let vector = frame.vector as u8;
    crate::events::emit(
        crate::events::Category::Fault,
        "fault",
        format_args!(
            r#""pid":{},"vector":{vector},"rip":{},"cr2":{},"error":{}"#,
            current().map_or(0, |p| p.id),
            frame.rip,
            cr2.unwrap_or(0),
            frame.error_code
        ),
    );
    if super::syscalls::tracing() {
        info!(
            "trace: pid {} fault: vector {vector} at rip {:#x} (cr2 {:#x}, error {:#x})",
            current().map_or(0, |p| p.id),
            frame.rip,
            cr2.unwrap_or(0),
            frame.error_code
        );
    }
    if let (14, Some(addr), Some(process)) = (vector, cr2, current()) {
        // Page-fault error code: bit 1 = write, bit 4 = instruction fetch.
        let kind = match frame.error_code {
            e if e & (1 << 4) != 0 => AccessKind::Execute,
            e if e & (1 << 1) != 0 => AccessKind::Write,
            _ => AccessKind::Read,
        };
        let started = platform::monotonic_ns();
        let resolved = vm::resolve_fault(&process, addr, kind);
        super::stats::FAULTS.add_since(started);
        match resolved {
            vm::Fault::Resolved => return, // retry the instruction
            vm::Fault::Raise(status) => {
                if !deliver_to_user(&process, frame, status, cr2) {
                    warn!(
                        "user exception: {:#x} at rip {:#x} (cr2 {addr:#x})",
                        status.0, frame.rip
                    );
                    terminate_current(status);
                }
                return;
            }
            vm::Fault::Denied => {}
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
    if let Some(process) = current()
        && deliver_to_user(&process, frame, status, cr2)
    {
        return;
    }
    warn!(
        "user exception: {} at rip {:#x} (cr2 {:#x}, error {:#x})",
        platform::exception_name(vector),
        frame.rip,
        cr2.unwrap_or(0),
        frame.error_code
    );
    // Takes effect on the way back to user mode (the user-return hook).
    terminate_current(status)
}

/// The dispatcher frame on the user stack: the CONTEXT at the new rsp,
/// the EXCEPTION_RECORD at `RECORD_OFFSET`, then room for a machine frame.
/// The layout follows the documented KiUserExceptionDispatcher contract as
/// far as it is public (rsp -> CONTEXT, record above it).
// TODO: check the offsets against the pinned build's ntdll.
const RECORD_OFFSET: u64 = 0x4f0;
const RECORD_SIZE: u64 = 0x98;
/// The machine frame (rip, cs, rflags, rsp, ss of the fault) above the
/// record: ntdll's dispatcher describes it in its unwind data
/// (UWOP_PUSH_MACHFRAME), so unwinding continues into the faulting frame.
const MACHINE_FRAME_OFFSET: u64 = 0x590;
const DISPATCH_FRAME_SIZE: u64 = MACHINE_FRAME_OFFSET + 0x30;

/// EXCEPTION_RECORD (x64) for `status` raised at `address`.
fn exception_record(status: NtStatus, address: u64, info: &[u64]) -> [u8; RECORD_SIZE as usize] {
    let mut r = [0u8; RECORD_SIZE as usize];
    r[0..4].copy_from_slice(&status.0.to_le_bytes());
    r[0x10..0x18].copy_from_slice(&address.to_le_bytes());
    r[0x18..0x1c].copy_from_slice(&(info.len() as u32).to_le_bytes());
    for (i, v) in info.iter().enumerate() {
        r[0x20 + 8 * i..0x28 + 8 * i].copy_from_slice(&v.to_le_bytes());
    }
    r
}

/// MACHINE_FRAME: rip, cs, rflags, rsp, ss of the interrupted code.
fn machine_frame(rip: u64, frame: &TrapFrame) -> [u8; 40] {
    let mut m = [0u8; 40];
    let (cs, ss) = (super::context::USER_CS.into(), super::context::USER_DS.into());
    let values = [rip, cs, frame.rflags, frame.rsp, ss];
    for (chunk, v) in m.chunks_exact_mut(8).zip(values) {
        chunk.copy_from_slice(&v.to_le_bytes());
    }
    m
}

/// Hands a user-mode exception to ntdll!KiUserExceptionDispatcher, if the
/// process has one: CONTEXT and EXCEPTION_RECORD go on the user stack and
/// the thread resumes in the dispatcher. False if it cannot be delivered.
fn deliver_to_user(
    process: &Process,
    frame: &mut TrapFrame,
    status: NtStatus,
    cr2: Option<u64>,
) -> bool {
    let dispatcher = process.exception_dispatcher.load(core::sync::atomic::Ordering::Relaxed);
    if dispatcher == 0 {
        return false;
    }
    // Breakpoints report the int3 itself (the trap left rip after it).
    let rip = if status == NtStatus::BREAKPOINT { frame.rip.wrapping_sub(1) } else { frame.rip };
    let info: &[u64] = match status {
        NtStatus::BREAKPOINT => &[0],
        NtStatus::ACCESS_VIOLATION | NtStatus::GUARD_PAGE_VIOLATION | NtStatus::STACK_OVERFLOW => {
            // 0 = read, 1 = write, 8 = execute (DEP).
            let kind = match frame.error_code {
                e if e & (1 << 4) != 0 => 8,
                e if e & (1 << 1) != 0 => 1,
                _ => 0,
            };
            &[kind, cr2.unwrap_or(u64::MAX)]
        }
        _ => &[],
    };
    let record = exception_record(status, rip, info);
    let regs = UserRegs {
        gpr: [
            frame.rax, frame.rcx, frame.rdx, frame.rbx, frame.rsp, frame.rbp, frame.rsi, frame.rdi,
            frame.r8, frame.r9, frame.r10, frame.r11, frame.r12, frame.r13, frame.r14, frame.r15,
        ],
        rip,
        rflags: frame.rflags,
    };
    let context = super::context::to_bytes(&regs, Some(&platform::fxsave()));
    let new_rsp = (frame.rsp.wrapping_sub(DISPATCH_FRAME_SIZE)) & !0xf;
    if new_rsp >= frame.rsp
        || process.write(new_rsp, &context).is_err()
        || process.write(new_rsp + RECORD_OFFSET, &record).is_err()
        || process.write(new_rsp + MACHINE_FRAME_OFFSET, &machine_frame(rip, frame)).is_err()
    {
        warn!("user exception: cannot push the dispatcher frame (rsp {:#x})", frame.rsp);
        return false;
    }
    frame.rip = dispatcher;
    frame.rsp = new_rsp;
    true
}

pub fn init() {
    platform::set_user_exception_handler(on_user_exception);
    thread::init();
}

#[cfg(test)]
mod tests {
    use super::number_variable;

    #[test]
    fn number_variables() {
        let mut buf = [0u8; 24];
        assert_eq!(number_variable(&mut buf, "NUMBER_OF_PROCESSORS=", 2), "NUMBER_OF_PROCESSORS=2");
        let mut buf = [0u8; 24];
        assert_eq!(number_variable(&mut buf, "N=", 0), "N=0");
        let mut buf = [0u8; 24];
        assert_eq!(number_variable(&mut buf, "N=", 4_000_000_000), "N=4000000000");
    }
}
