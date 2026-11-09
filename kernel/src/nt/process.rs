//! Processes: a user address space, a handle table and threads. Includes
//! the loader for PE executables (ntdll binds their imports).

use super::platform;
use alloc::vec::Vec;
use core::fmt;

use super::layout::{self, peb};
use super::object::{HandleTable, KObject, ObjectBody, ObjectRef};
use super::pe::{PeError, PeImage};
use super::platform::PAGE_SIZE;
use super::platform::PageFlags;
use super::platform::Personality;
use super::platform::SpawnError;
use super::platform::UserRegs;
use super::platform::{PagingError, SharedUserSpace, UserFault, UserSpace};
use super::platform::{TrapFrame, TrapKind};
use super::status::NtStatus;
use super::thread;
use super::vad::{self, Vad};
use super::vm::{self, AccessKind};
use super::wait::{self, Dispatcher};
use crate::mm::heap::{AllocError, KArc, TryVec};
use crate::sync::IrqMutex;

/// PROCESS_PRIORITY_CLASS_NORMAL.
pub const PRIORITY_CLASS_NORMAL: u8 = 2;

/// The personality-independent part of a process (ARCHITECTURE "Plan: a
/// second personality (Linux)", seam 3, done with RS step B2): its
/// identity and parent, address space, threads, credentials (the primary
/// token), session and desktop, exit status and memory charge. The NT
/// personality's state (handles, the PE image, ntdll's dispatchers, the
/// job, the console, crash notes) is `Process` around it; a Linux
/// personality would put its fd table, signal state and pid/pgid/sid
/// around the same core. `Process` derefs to it, so `process.space`,
/// `process.session` and the like read the core.
pub struct Core {
    pub id: u64,
    /// The creating process's id (0: started by the kernel).
    pub parent_id: u64,
    pub name: &'static str,
    /// The Win32 path of the executable (`C:\\gala\\x.exe`).
    pub path: &'static str,
    pub space: SharedUserSpace,
    /// Lock order: `vad` before `space`.
    pub vad: IrqMutex<Vad>,
    /// Mapped section views (lock order: `views`, then `vad`, then `space`).
    pub views: IrqMutex<Vec<super::section::View>>,
    /// Thread objects of the live threads.
    threads: IrqMutex<Vec<ObjectRef>>,
    /// Threads created so far (each thread's `ordinal`).
    pub threads_created: core::sync::atomic::AtomicU32,
    exit_status: IrqMutex<Option<NtStatus>>,
    /// Signaled when the process has ended.
    pub dispatcher: Dispatcher,
    /// NT times (ProcessTimes); exit time 0 while it runs.
    pub create_time: u64,
    pub exit_time: IrqMutex<u64>,
    /// A page fault found no free frame (logged once).
    pub oom_logged: core::sync::atomic::AtomicBool,
    /// Pages of committed private memory charged to the process
    /// (nt::charge); set to 0 for good when it ends.
    pub commit: core::sync::atomic::AtomicU64,
    /// The most it was charged (PeakPagefileUsage).
    pub commit_peak: core::sync::atomic::AtomicU64,
    pub commit_ended: core::sync::atomic::AtomicBool,
    /// The primary token (nt::token).
    pub token: IrqMutex<ObjectRef>,
    /// Its session (nt::session): its primary token's TokenSessionId when
    /// it was created; fixed for its life (PEB.SessionId,
    /// ProcessSessionInformation, where `Local\` names go).
    pub session: u32,
    /// Its session's desktop (nt::session::Desk): the window registry,
    /// WinEvent hooks and shasha's desktop its windows live in.
    pub desk: KArc<super::session::Desk>,
}

/// An NT process: its core (`Core`, shared with any later personality)
/// and the NT personality's state.
pub struct Process {
    /// What every personality's process has.
    core: Core,
    pub handles: IrqMutex<HandleTable>,
    /// Its pool and handle quotas (nt::quota), also attached to `handles`.
    pub quota: KArc<super::quota::Block>,
    /// The executable's mapping: (base, size).
    pub image: (u64, u64),
    /// Its layout, whose shared frames back the executable's pages (held
    /// for as long as they are mapped). None for a process of another
    /// personality (a Linux program's ELF image is copied in, `lx::exec`).
    _layout: Option<KArc<super::section::ImageLayout>>,
    /// ntdll!KiUserExceptionDispatcher in this process (in a WOW64
    /// process wow64.dll's GalaWow64Exception, for its 64-bit code), or 0:
    /// then user exceptions end the process.
    pub(super) exception_dispatcher: core::sync::atomic::AtomicU64,
    /// ntdll!KiUserApcDispatcher in this process, or 0 (no user APCs run).
    pub apc_dispatcher: core::sync::atomic::AtomicU64,
    /// The job object the process is in (nt::job).
    pub job: IrqMutex<Option<ObjectRef>>,
    /// The priority class (PROCESS_PRIORITY_CLASS_*: 2 is NORMAL), as set
    /// through ProcessPriorityClass. Recorded only: every thread runs at
    /// the normal base priority (thread.rs).
    pub priority_class: core::sync::atomic::AtomicU8,
    /// The console session its console output also goes to (a console
    /// window's), inherited by the processes it creates.
    pub console: Option<KArc<super::conhost::Session>>,
    /// The last exception one of its threads took (a crash report when it
    /// ends the process; nt::crashreport).
    pub last_exception: IrqMutex<Option<super::crashreport::Note>>,
    /// The exception no handler took, as ntdll reported it before calling
    /// the top-level filter (GalaUnhandledException): reported when the
    /// process ends, whatever its status (a filter such as Crashpad's
    /// ends the process with its own code).
    pub unhandled_exception: IrqMutex<Option<super::crashreport::Note>>,
    /// Its window station and desktops (nt::winsta): what its creator
    /// asked for, resolved at its first window station or desktop call.
    pub ui: super::winsta::Connection,
    /// Its system-call ABI, fixed at creation from the image (a PE image
    /// is NT): its threads' system calls go to that personality's
    /// dispatcher (`crate::personality`, ROADMAP LX seam 2).
    pub personality: Personality,
    /// A 32-bit (x86) process's WOW64 state (`nt::wow64`); None for a
    /// 64-bit one.
    pub wow64: Option<super::wow64::Wow64Process>,
}

impl core::ops::Deref for Process {
    type Target = Core;

    fn deref(&self) -> &Core {
        &self.core
    }
}

impl Process {
    /// The personality-independent part.
    pub fn core(&self) -> &Core {
        &self.core
    }

    /// Its primary token's integrity level (Medium if it has no label).
    pub fn integrity_level(&self) -> u32 {
        let token = self.token.lock().clone();
        match &token.body {
            ObjectBody::Token(t) => t.data.lock().integrity_level(),
            _ => super::token::MEDIUM_LEVEL,
        }
    }

    /// Its primary token is elevated (TokenElevation: an administrator's
    /// full token, SYSTEM's).
    pub fn elevated(&self) -> bool {
        let token = self.token.lock().clone();
        match &token.body {
            ObjectBody::Token(t) => t.data.lock().elevated(),
            _ => false,
        }
    }
}

impl fmt::Debug for Process {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Process({} pid {})", self.name, self.id)
    }
}

impl Process {
    /// The executable's layout (whose frames back its unwritten pages).
    pub fn layout(&self) -> Option<&KArc<super::section::ImageLayout>> {
        self._layout.as_ref()
    }

    /// ntdll!KiUserExceptionDispatcher in this process, or 0.
    pub fn exception_dispatcher(&self) -> u64 {
        self.exception_dispatcher.load(core::sync::atomic::Ordering::Relaxed)
    }

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
    pub fn threads(&self) -> Result<Vec<ObjectRef>, crate::mm::heap::AllocError> {
        let threads = self.threads.lock();
        let mut out = Vec::new();
        out.try_reserve(threads.len()).map_err(|_| crate::mm::heap::AllocError)?;
        for t in threads.iter() {
            out.try_push(t.clone())?;
        }
        Ok(out)
    }

    /// Calls `f` on each live thread under the threads lock, without
    /// allocating (terminating a process must not fail: a snapshot that
    /// could not be made once left the other threads running).
    pub fn for_each_thread(&self, mut f: impl FnMut(&ObjectRef)) {
        for t in self.threads.lock().iter() {
            f(t);
        }
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

    /// Writes an IO_STATUS_BLOCK at `addr`, in the layout this process's
    /// code reads: x64's (Status in a pointer-sized union, then
    /// Information: 16 bytes), or in a WOW64 process the x86 one (8
    /// bytes). Every call a WOW64 process makes comes from wow64.dll on
    /// behalf of 32-bit code, which passes its caller's block through, so
    /// I/O that completes later reports where and as the caller looks
    /// (ARCHITECTURE "Plan: WOW64").
    pub fn write_iosb(
        &self,
        addr: u64,
        status: NtStatus,
        information: u64,
    ) -> Result<(), UserFault> {
        let mut b = [0u8; 16];
        b[0..4].copy_from_slice(&status.0.to_le_bytes());
        if self.wow64.is_some() {
            b[4..8].copy_from_slice(&(information as u32).to_le_bytes());
            return self.write(addr, &b[..8]);
        }
        b[8..16].copy_from_slice(&information.to_le_bytes());
        self.write(addr, &b)
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

/// For a leaked address space: whether its process is still known and
/// what refers to it (its references besides PROCESSES' own: thread
/// objects, handles in other processes; its live threads).
pub fn describe_holders(space: &SharedUserSpace) -> impl core::fmt::Display {
    // One lock at a time (the list's, then the process's own).
    let found = PROCESSES.lock().iter().find(|p| KArc::ptr_eq(&p.space, space)).cloned();
    DescribeHolders(found.map(|p| {
        // Besides the list's and `found`'s own.
        let refs = KArc::strong_count(&p) - 2;
        let exited = *p.exit_time.lock() != 0;
        (p.id, refs, p.thread_count(), exited)
    }))
}

struct DescribeHolders(Option<(u64, usize, usize, bool)>);

impl core::fmt::Display for DescribeHolders {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            None => write!(f, "forgotten (not in the process list)"),
            Some((id, refs, threads, exited)) => write!(
                f,
                "pid {id} still listed: {refs} other references, {threads} live threads, \
                 exited {exited}"
            ),
        }
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

/// The state dump's process lines: each running process's commit charge
/// and its pool and handle quotas (`process: pid P (x.exe) commit N KiB, R reservations, G regions;
/// paged U/L (peak P) ...`).
pub fn dump() {
    let Ok(list) = running() else {
        info!("process: no memory to list the processes");
        return;
    };
    for p in list.iter() {
        // One lock at a time (format arguments' guards live to the end of
        // the statement).
        let (reservations, regions) = p.vad.lock().shape();
        info!(
            "process: pid {} ({}) commit {} KiB, {reservations} reservations, {regions} regions; {}",
            p.id,
            p.name,
            p.commit.load(core::sync::atomic::Ordering::Relaxed) * 4,
            *p.quota
        );
    }
}

/// The process with id `id` (running, or exited and still referenced).
pub fn by_id(id: u64) -> Option<KArc<Process>> {
    prune();
    PROCESSES.lock().iter().find(|p| p.id == id).cloned()
}

/// The integrity level of process `id`'s primary token (UIPI; None: no
/// such process).
pub fn integrity_level(id: u64) -> Option<u32> {
    by_id(id).map(|p| p.integrity_level())
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
    /// A 32-bit program, and no wow64.dll to run it (nt::wow64).
    NoWow64,
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
    fn from(status: NtStatus) -> Self {
        if status == NtStatus::NO_MEMORY || status == NtStatus::INSUFFICIENT_RESOURCES {
            LoadError::OutOfMemory
        } else {
            LoadError::BadImageBase
        }
    }
}

impl LoadError {
    /// `self` if it is out of memory, else `other` (a more precise error
    /// for a failed status).
    fn or(self, other: LoadError) -> LoadError {
        if matches!(self, LoadError::OutOfMemory) { self } else { other }
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

/// The page flags of a process's private read/write structures (PEB,
/// parameters, TEBs).
pub fn user_rw_flags() -> PageFlags {
    USER_RW
}

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
/// sets them (a 64-bit process's; a 32-bit one's is adjusted:
/// `wow64::environment`).
const ENVIRONMENT: &[&str] = &[
    "ALLUSERSPROFILE=C:\\ProgramData",
    "APPDATA=C:\\Users\\gala\\AppData\\Roaming",
    "CommonProgramFiles=C:\\Program Files\\Common Files",
    "CommonProgramFiles(x86)=C:\\Program Files (x86)\\Common Files",
    "CommonProgramW6432=C:\\Program Files\\Common Files",
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
    "ProgramFiles(x86)=C:\\Program Files (x86)",
    "ProgramW6432=C:\\Program Files",
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
    console: Option<KArc<super::conhost::Session>>,
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
    // (Out of memory is not a bad image: fault injection showed hello.exe
    // failing to start with BadPeSignature.)
    let layout = super::section::ImageLayout::new(data)
        .map_err(|e| LoadError::from(e).or(LoadError::Pe(PeError::BadPeSignature)))?;
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
        token: None,
        console,
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
    /// The primary token; None: the boot user's (nt::token).
    pub token: Option<ObjectRef>,
    /// The console session (nt::conhost), if any.
    pub console: Option<KArc<super::conhost::Session>>,
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
        token,
        console,
    } = spec;
    let info = image.info;
    if info.is_dll {
        return Err(LoadError::Pe(PeError::NotExecutable));
    }
    // Binding imports is the user-mode loader's job (ntdll), as on Windows.
    if info.has_imports && !info.x86 && super::ntdll::get().is_none() {
        return Err(LoadError::ImportsNotSupported);
    }
    // A 32-bit program runs through WOW64 (nt::wow64); its imports are
    // bound by the 32-bit ntdll.
    if info.x86 && info.has_imports && super::ntdll::get32().is_none() {
        warn!("wow64: {name} imports DLLs, and there is no {} to bind them", super::ntdll::PATH32);
        return Err(LoadError::ImportsNotSupported);
    }
    let mut space = UserSpace::new()?;
    let mut vad = Vad::default();
    if info.x86 {
        vad.set_limit(super::wow64::limit(info.large_address_aware));
    }
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
    // A WOW64 process's main thread also has a 64-bit stack, for its
    // wow64.dll code (system call thunks), laid out the same way.
    let stack64 = if info.x86 { Some(super::wow64::reserve_stack64(&mut vad)?) } else { None };

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
    // Its quotas: what it holds from here on is charged (the handles it
    // starts with too, without a limit).
    let quota = super::quota::new_block(id, name)?;
    handles.attach_quota(quota.clone());

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
    // As WOW64 sets it for the process's bitness (nt::wow64).
    let adjusted = super::wow64::environment(environment.unwrap_or(&environment_default), info.x86)
        .map_err(|_| LoadError::OutOfMemory)?;
    let environment = adjusted.vars().map_err(|_| LoadError::OutOfMemory)?;
    let p = layout::PARAMS_ADDRESS;
    let block = super::params::build(
        &super::params::ProcessParameters {
            image_path: path,
            command_line,
            current_directory,
            current_directory_handle: cwd_handle,
            dll_path: "",
            std_handles,
            environment: &environment,
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
    // The session is the primary token's (as NtCreateUserProcess takes
    // it); its directories are made on first use (nt::session).
    let token = match token {
        Some(t) => t,
        None => super::token::for_new_process(None).map_err(|_| LoadError::OutOfMemory)?,
    };
    let session = super::token::session_of(&token);
    let desk = super::session::ensure(session)?;
    put_u32(&mut space, peb_addr + peb::SESSION_ID, session)?;
    // A WOW64 process: the PEB32 and the 32-bit parameters (the same
    // values), below its 32-bit limit.
    let wow64 = if info.x86 {
        let params = super::params::ProcessParameters {
            image_path: path,
            command_line,
            current_directory,
            current_directory_handle: cwd_handle,
            dll_path: "",
            std_handles,
            environment: &environment,
            runtime_data,
        };
        // The block's size first (it does not depend on where it goes).
        let size = super::params::build32(&params, 0).map_err(|_| LoadError::OutOfMemory)?.len();
        let at = (layout::wow64::PEB32_ADDRESS - size as u64) & !(vad::ALLOCATION_GRANULARITY - 1);
        let block = super::params::build32(&params, at).map_err(|_| LoadError::OutOfMemory)?;
        let peb32 = super::wow64::Peb32 {
            image_base: base as u32,
            params: at as u32,
            cpus,
            subsystem: info.subsystem,
            subsystem_version: info.subsystem_version,
            session,
        };
        super::wow64::set_up_space(&mut space, &mut vad, at, &block, &peb32)?;
        Some(super::wow64::Wow64Process::new(layout::wow64::PEB32_ADDRESS))
    } else {
        None
    };
    let space = KArc::try_new(platform::SpaceLock::new(space))?;
    let process = KArc::try_new(Process {
        core: Core {
            id,
            parent_id,
            name,
            path,
            space,
            vad: IrqMutex::new(vad),
            views: IrqMutex::new(Vec::new()),
            threads: IrqMutex::new(Vec::new()),
            threads_created: core::sync::atomic::AtomicU32::new(0),
            exit_status: IrqMutex::new(None),
            dispatcher: Dispatcher::new(0),
            create_time: platform::system_time(),
            exit_time: IrqMutex::named("process.exit_time", 0),
            oom_logged: core::sync::atomic::AtomicBool::new(false),
            commit: core::sync::atomic::AtomicU64::new(0),
            commit_peak: core::sync::atomic::AtomicU64::new(0),
            commit_ended: core::sync::atomic::AtomicBool::new(false),
            token: IrqMutex::new(token),
            session,
            desk,
        },
        handles: IrqMutex::new(handles),
        quota,
        image: (base, u64::from(info.size_of_image)),
        _layout: Some(image.clone()),
        exception_dispatcher: core::sync::atomic::AtomicU64::new(0),
        apc_dispatcher: core::sync::atomic::AtomicU64::new(0),
        job: IrqMutex::new(None),
        priority_class: core::sync::atomic::AtomicU8::new(PRIORITY_CLASS_NORMAL),
        console,
        last_exception: IrqMutex::new(None),
        unhandled_exception: IrqMutex::new(None),
        ui: super::winsta::Connection::new(),
        // A PE image: the NT personality (an ELF loader would choose Linux).
        personality: Personality::Nt,
        wow64,
    })?;
    prune();
    PROCESSES.lock().try_push(process.clone())?;

    // A WOW64 process: gala's wow64.dll, then the main thread, which starts
    // there and enters 32-bit code at the entry point (stdcall, the PEB32
    // as its argument above a null return address), through the 32-bit
    // ntdll's LdrInitializeThunk when there is one.
    if let (Some(state), Some(stack64)) = (process.wow64.as_ref(), stack64) {
        let entry = (base + u64::from(info.entry_rva)) as u32;
        let peb32 = state.peb32 as u32;
        let esp = (stack_top - 8) as u32;
        let mut frame = [0u8; 8];
        frame[4..].copy_from_slice(&peb32.to_le_bytes());
        let started = super::wow64::map_dll(&process)
            .and_then(|()| process.write(u64::from(esp), &frame).map_err(|_| NtStatus::NO_MEMORY))
            .and_then(|()| {
                let start = super::wow64::Start32 {
                    eip: entry,
                    esp,
                    gpr: [entry, 0, 0, peb32, 0, 0, 0],
                    stack: (stack_top as u32, stack_limit as u32, stack_bottom as u32),
                    stack64: stack64.0,
                };
                let start = super::wow64::through_ntdll32(&process, &start)?.unwrap_or(start);
                thread::create_wow64(&process, start, stack64, suspended).map(|t| (t, start.eip))
            });
        let (main, eip) = match started {
            Ok(t) => t,
            Err(status) => {
                PROCESSES.lock().retain(|p| !KArc::ptr_eq(p, &process));
                return Err(if status == NtStatus::INVALID_IMAGE_FORMAT {
                    LoadError::NoWow64
                } else {
                    LoadError::from(status)
                });
            }
        };
        announce_start(&process, u64::from(eip), command_line);
        return Ok((process, main));
    }

    // Main thread: starts at the entry point with the PEB as its argument,
    // as if called (`abi::call_regs`); with an ntdll, at its
    // LdrInitializeThunk with that as the context.
    let start = super::abi::call_regs(base + u64::from(info.entry_rva), peb_addr, stack_top);
    let started = through_ntdll(&process, &start, stack_top).and_then(|regs| {
        thread::create(
            &process,
            regs,
            start.ip(),
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
    announce_start(&process, regs.ip(), command_line);
    Ok((process, main))
}

/// The `process ... starting at` line and the `process start` event, with
/// the process's personality and its creator: the parent process and the
/// thread that created it (the caller's, when it has a parent; 0 for
/// processes the kernel starts).
pub fn announce_start(process: &Process, rip: u64, command_line: &str) {
    let (id, session, personality) = (process.id, process.session, process.personality);
    let (ppid, ptid) = match process.parent_id {
        0 => (0, 0),
        p => (p, crate::events::current_client()),
    };
    crate::events::note_process(id, command_line);
    let (machine, wow64) =
        if process.wow64.is_some() { ("x86", ", 32-bit x86 (WOW64)") } else { ("x64", "") };
    // An elevated primary token (an administrator's full token: SYSTEM's,
    // or what ShellExecute's "runas" starts) is named on the line.
    let elevated = process.elevated();
    info!(
        "process {} (pid {id}, session {session}): starting at {rip:#x}, personality \
         {personality}{wow64}{}",
        process.name,
        if elevated { ", elevated token" } else { "" }
    );
    crate::events::emit(
        crate::events::Category::Process,
        "process start",
        format_args!(
            r#""pid":{id},"session":{session},"name":{},"personality":"{personality}","machine":"{machine}","ppid":{ppid},"ptid":{ptid},"elevated":{elevated},"image_base":{},"entry":{rip},"cmdline":{}"#,
            crate::events::Str(process.name),
            process.image.0,
            crate::events::Str(command_line)
        ),
    );
}

/// A process of another personality than NT (`lx::exec`): its address
/// space laid out by its loader, no PEB, no ntdll, no handles; the boot
/// user's token. Its threads are created with `thread::create` (which
/// gives a non-NT thread no TEB).
pub struct Bare {
    pub name: &'static str,
    /// Its path (a Linux program's, as it was executed).
    pub path: &'static str,
    pub space: UserSpace,
    pub vad: Vad,
    /// The executable's span: (base, size).
    pub image: (u64, u64),
    pub personality: Personality,
    /// The process that created it (a Linux fork's parent); 0: none.
    pub parent_id: u64,
}

/// Creates the process record for `bare` and lists it (no thread yet).
pub fn create_bare(bare: Bare) -> Result<KArc<Process>, LoadError> {
    let id = thread::next_id();
    let token = super::token::for_new_process(None).map_err(|_| LoadError::OutOfMemory)?;
    let session = super::token::session_of(&token);
    let desk = super::session::ensure(session)?;
    let space = KArc::try_new(platform::SpaceLock::new(bare.space))?;
    let quota = super::quota::new_block(id, bare.name)?;
    let mut handles = HandleTable::default();
    handles.attach_quota(quota.clone());
    let process = KArc::try_new(Process {
        core: Core {
            id,
            parent_id: bare.parent_id,
            name: bare.name,
            path: bare.path,
            space,
            vad: IrqMutex::new(bare.vad),
            views: IrqMutex::new(Vec::new()),
            threads: IrqMutex::new(Vec::new()),
            threads_created: core::sync::atomic::AtomicU32::new(0),
            exit_status: IrqMutex::new(None),
            dispatcher: Dispatcher::new(0),
            create_time: platform::system_time(),
            exit_time: IrqMutex::named("process.exit_time", 0),
            oom_logged: core::sync::atomic::AtomicBool::new(false),
            commit: core::sync::atomic::AtomicU64::new(0),
            commit_peak: core::sync::atomic::AtomicU64::new(0),
            commit_ended: core::sync::atomic::AtomicBool::new(false),
            token: IrqMutex::new(token),
            session,
            desk,
        },
        handles: IrqMutex::new(handles),
        quota,
        image: bare.image,
        _layout: None,
        exception_dispatcher: core::sync::atomic::AtomicU64::new(0),
        apc_dispatcher: core::sync::atomic::AtomicU64::new(0),
        job: IrqMutex::new(None),
        priority_class: core::sync::atomic::AtomicU8::new(PRIORITY_CLASS_NORMAL),
        console: None,
        last_exception: IrqMutex::new(None),
        unhandled_exception: IrqMutex::new(None),
        ui: super::winsta::Connection::new(),
        personality: bare.personality,
        wow64: None,
    })?;
    prune();
    PROCESSES.lock().try_push(process.clone())?;
    Ok(process)
}

/// Takes a process created by `create_bare` off the list again (its
/// first thread could not be started).
pub fn forget_bare(process: &KArc<Process>) {
    PROCESSES.lock().retain(|p| !KArc::ptr_eq(p, process));
}

/// The initial registers: `start` itself, or, if there is a system ntdll,
/// its LdrInitializeThunk(&CONTEXT of `start`, ntdll base) with the
/// CONTEXT at the top of the stack (`abi::ldr_thunk_regs`). The ntdll view
/// is mapped here.
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
    let thunk = view.base + u64::from(ntdll.thunk_rva);
    super::abi::ldr_thunk_regs(process, start, stack_top, thunk, view.base)
}

/// Ends `process` with `status`: other threads are asked to terminate, and
/// the calling thread exits too if it belongs to the process.
pub fn terminate(process: &KArc<Process>, status: NtStatus, include_caller: bool) {
    process.exit_status.lock().get_or_insert(status);
    let me = thread::current();
    process.for_each_thread(|t| {
        if !me.as_ref().is_some_and(|m| KArc::ptr_eq(m, t)) {
            thread::request_termination(t, status);
        }
    });
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
    // A line the program left open ends here, before the exit lines.
    crate::console::end_program_line(process.id);
    // Its control-handler thread takes no more Ctrl+C.
    if let Some(session) = &process.console {
        session.forget_taker(process.id);
    }
    // A Linux process's last output lines and how it ended, before the
    // core's line (ROADMAP LX).
    if process.personality == Personality::Linux {
        crate::lx::process_ended(process, status);
    }
    // Ended by an exception: what Windows' error report would say.
    let note = *process.last_exception.lock();
    let unhandled = *process.unhandled_exception.lock();
    if let Some(note) = note.filter(|n| n.status == status).or(unhandled) {
        super::crashreport::report(process, &note, status);
    }
    // How long it lived (creation to exit, 100 ns units): what run-all.ps1
    // times a program by on Windows (`cargo xtask compare` sets them side
    // by side).
    let lived_ms = process.exit_time.lock().saturating_sub(process.create_time) / 10_000;
    info!(
        "process {} (pid {}) exited with status {:#010x} after {} ms",
        process.name, process.id, status.0, lived_ms
    );
    crate::events::emit(
        crate::events::Category::Process,
        "process exit",
        format_args!(
            r#""pid":{},"session":{},"status":{},"personality":"{}","paged_peak":{},"nonpaged_peak":{},"handles_peak":{}"#,
            process.id,
            process.session,
            status.0,
            process.personality,
            process.quota.pool(super::quota::Pool::Paged).1,
            process.quota.pool(super::quota::Pool::NonPaged).1,
            process.quota.handles().1
        ),
    );
    super::desktop::process_ended(&process.desk.shell, process.id);
    process.desk.windows.process_ended(process.id as u32);
    super::clipboard::process_ended(process);
    process.desk.hooks.process_ended(process.id as u32);
    super::conhost::process_ended(process.id);
    // A service it ran that had not stopped: stopped, its process aborted.
    super::scm_run::process_ended(process);
    super::charge::ended(process);
    // Close every handle (this breaks reference cycles through handles to
    // the process's own objects) and forget the process.
    let entries = process.handles.lock().close_all();
    drop(entries);
    // Signaled last, as on Windows: a waiter that wakes finds the
    // process's handles closed (its named objects gone, its files and pipe
    // ends closed). Signaled first, a parent on another CPU could open a
    // name only the child held, or see a file still open (smprace_test).
    wait::signal_dispatcher(&process.dispatcher, 1);
    crate::flight::record(crate::flight::Kind::ProcessExit, process.id, u64::from(status.0));
    // Stays in PROCESSES while referenced (OpenProcess by id); `prune`
    // forgets it after that.
}

/// User-mode CPU exceptions: resolved (demand paging, guard pages, COW),
/// delivered to the process's exception dispatcher, or fatal.
fn on_user_exception(frame: &mut TrapFrame, cr2: Option<u64>) {
    let vector = frame.vector();
    crate::events::emit(
        crate::events::Category::Fault,
        "fault",
        format_args!(
            r#""pid":{},"vector":{vector},"rip":{},"cr2":{},"error":{}"#,
            current().map_or(0, |p| p.id),
            frame.ip(),
            cr2.unwrap_or(0),
            frame.error_code()
        ),
    );
    if super::syscalls::tracing() {
        info!(
            "trace: pid {} fault: vector {vector} at rip {:#x} (cr2 {:#x}, error {:#x})",
            current().map_or(0, |p| p.id),
            frame.ip(),
            cr2.unwrap_or(0),
            frame.error_code()
        );
    }
    if let (Some(access), Some(addr), Some(process)) = (frame.page_fault_access(), cr2, current()) {
        let kind = match access {
            platform::FaultAccess::Execute => AccessKind::Execute,
            platform::FaultAccess::Write => AccessKind::Write,
            platform::FaultAccess::Read => AccessKind::Read,
        };
        let started = platform::monotonic_ns();
        let resolved = vm::resolve_fault(&process, addr, kind);
        super::stats::FAULTS.add_since(started);
        match resolved {
            vm::Fault::Resolved => return, // retry the instruction
            vm::Fault::Raise(status) => {
                if !super::abi::deliver_to_user(&process, frame, status, cr2) {
                    super::abi::keep_note(&process, frame, status, frame.ip(), cr2);
                    warn!(
                        "user exception: {:#x} at rip {:#x} (cr2 {addr:#x})",
                        status.0,
                        frame.ip()
                    );
                    terminate_current(status);
                }
                return;
            }
            vm::Fault::Denied | vm::Fault::OutOfMemory => {}
        }
    }
    let status = match frame.kind() {
        TrapKind::DivideError => NtStatus::INTEGER_DIVIDE_BY_ZERO,
        TrapKind::Breakpoint => NtStatus::BREAKPOINT,
        TrapKind::InvalidOpcode => NtStatus::ILLEGAL_INSTRUCTION,
        TrapKind::Protection => NtStatus::PRIVILEGED_INSTRUCTION,
        TrapKind::PageFault | TrapKind::Other => NtStatus::ACCESS_VIOLATION,
    };
    if let Some(process) = current()
        && super::abi::deliver_to_user(&process, frame, status, cr2)
    {
        return;
    }
    if let Some(process) = current() {
        super::abi::keep_note(&process, frame, status, frame.ip(), cr2);
    }
    warn!(
        "user exception: {} at rip {:#x} (cr2 {:#x}, error {:#x})",
        platform::exception_name(vector),
        frame.ip(),
        cr2.unwrap_or(0),
        frame.error_code()
    );
    // Takes effect on the way back to user mode (the user-return hook).
    terminate_current(status)
}

pub fn init() {
    platform::set_user_exception_handler(on_user_exception);
    thread::init();
}

/// Opens the executable at Win32 path `path` (`C:\...`) for reading.
pub(crate) fn open_program(path: &str) -> Result<ObjectRef, NtStatus> {
    use super::file;
    let rest = path.strip_prefix("C:").or_else(|| path.strip_prefix("c:"));
    let rest = rest.ok_or(NtStatus::OBJECT_PATH_SYNTAX_BAD)?;
    let mut name: Vec<u16> = Vec::new();
    for c in rest.encode_utf16() {
        name.try_push(c).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    let path = file::FilePath { start: crate::fs::ramfs::ROOT, name };
    let opened = file::create(
        &path,
        file::FILE_READ_DATA,
        file::FILE_SHARE_READ,
        file::FILE_OPEN,
        file::FILE_NON_DIRECTORY_FILE,
    )?;
    Ok(opened.object)
}

/// A process a kernel handover carried over (`handover::programs`): its
/// address space rebuilt from the record, no threads yet.
pub struct Adopted {
    pub id: u64,
    pub parent_id: u64,
    pub name: &'static str,
    pub path: &'static str,
    pub space: UserSpace,
    pub vad: Vad,
    pub views: Vec<super::section::View>,
    pub image: (u64, u64),
    pub layout: KArc<super::section::ImageLayout>,
    pub exception_dispatcher: u64,
    pub apc_dispatcher: u64,
    pub create_time: u64,
    pub threads_created: u32,
    pub priority_class: u8,
    /// Its primary token (None: an older record without one: the boot
    /// user's, made again).
    pub token: Option<ObjectRef>,
    /// Its session (None: an older record without one: its token's).
    pub session: Option<u32>,
}

/// Rebuilds a process from a kernel handover's record, its commit charged
/// again.
pub fn adopt(a: Adopted) -> Result<KArc<Process>, NtStatus> {
    use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64};
    let token = match a.token {
        Some(t) => t,
        None => super::token::for_new_process(None).map_err(|_| NtStatus::NO_MEMORY)?,
    };
    let session = a.session.unwrap_or_else(|| super::token::session_of(&token));
    let desk = super::session::ensure(session)?;
    let space =
        KArc::try_new(platform::SpaceLock::new(a.space)).map_err(|_| NtStatus::NO_MEMORY)?;
    let commit = a.vad.private_commit();
    let quota = super::quota::new_block(a.id, a.name).map_err(|_| NtStatus::NO_MEMORY)?;
    let mut handles = HandleTable::default();
    handles.attach_quota(quota.clone());
    let process = KArc::try_new(Process {
        core: Core {
            id: a.id,
            parent_id: a.parent_id,
            name: a.name,
            path: a.path,
            space,
            vad: IrqMutex::new(a.vad),
            views: IrqMutex::new(a.views),
            threads: IrqMutex::new(Vec::new()),
            threads_created: AtomicU32::new(a.threads_created),
            exit_status: IrqMutex::new(None),
            dispatcher: Dispatcher::new(0),
            create_time: a.create_time,
            exit_time: IrqMutex::named("process.exit_time", 0),
            oom_logged: AtomicBool::new(false),
            commit: AtomicU64::new(0),
            commit_peak: AtomicU64::new(0),
            commit_ended: AtomicBool::new(false),
            token: IrqMutex::new(token),
            session,
            desk,
        },
        handles: IrqMutex::new(handles),
        quota,
        image: a.image,
        _layout: Some(a.layout),
        exception_dispatcher: AtomicU64::new(a.exception_dispatcher),
        apc_dispatcher: AtomicU64::new(a.apc_dispatcher),
        job: IrqMutex::new(None),
        priority_class: AtomicU8::new(a.priority_class),
        console: None,
        last_exception: IrqMutex::new(None),
        unhandled_exception: IrqMutex::new(None),
        ui: super::winsta::Connection::new(),
        // The handover's PROCESSES record has no personality yet: NT is
        // the only one with processes (a second needs a record version).
        personality: Personality::Nt,
        wow64: None,
    })
    .map_err(|_| NtStatus::NO_MEMORY)?;
    {
        let _vad = process.vad.lock();
        super::charge::sync(&process, commit);
    }
    PROCESSES.lock().try_push(process.clone()).map_err(|_| NtStatus::NO_MEMORY)?;
    thread::continue_ids_from(a.id + 4);
    Ok(process)
}

/// Forgets a process `adopt` made whose threads could not be rebuilt.
pub fn forget_adopted(process: &KArc<Process>) {
    process.exit_status.lock().get_or_insert(NtStatus::UNSUCCESSFUL);
    finish(process, NtStatus::UNSUCCESSFUL);
}

/// Whether the program at Win32 path `path` exists (the start menu).
pub fn program_exists(path: &str) -> bool {
    open_program(path).is_ok()
}

/// Whether the program at Win32 path `path` is a console program
/// (IMAGE_SUBSYSTEM_WINDOWS_CUI): started from the start menu, it gets a
/// console window, as one started from Explorer does.
pub fn is_console_program(path: &str) -> Result<bool, NtStatus> {
    const IMAGE_SUBSYSTEM_WINDOWS_CUI: u16 = 3;
    let object = open_program(path)?;
    let ObjectBody::File(f) = &object.body else { return Err(NtStatus::OBJECT_TYPE_MISMATCH) };
    Ok(super::section::image_layout(f.node)?.info.subsystem == IMAGE_SUBSYSTEM_WINDOWS_CUI)
}

/// `text` with each `%NAME%` of the initial environment (ENVIRONMENT,
/// names compared case-insensitively, as ExpandEnvironmentStrings does)
/// replaced by its value, in `buf`; a `%` that starts no known name stays.
/// None when it does not fit.
pub fn expand_environment<'b>(text: &str, buf: &'b mut [u8]) -> Option<&'b str> {
    let mut n = 0;
    let mut put = |buf: &mut [u8], part: &str| -> Option<()> {
        buf.get_mut(n..n + part.len())?.copy_from_slice(part.as_bytes());
        n += part.len();
        Some(())
    };
    let mut rest = text;
    while let Some(at) = rest.find('%') {
        put(buf, &rest[..at])?;
        let after = &rest[at + 1..];
        let value = after.find('%').and_then(|end| {
            let name = &after[..end];
            ENVIRONMENT.iter().find_map(|v| {
                let (k, value) = v.split_once('=')?;
                (!name.is_empty() && k.eq_ignore_ascii_case(name)).then_some((value, end))
            })
        });
        match value {
            Some((value, end)) => {
                put(buf, value)?;
                rest = &after[end + 1..];
            }
            None => {
                put(buf, "%")?;
                rest = after;
            }
        }
    }
    put(buf, rest)?;
    core::str::from_utf8(&buf[..n]).ok()
}

/// Starts the program at Win32 path `path` with arguments `args` (after
/// the quoted path on its command line) in its own directory, with new
/// console handles, under primary token `token` (None: the boot user's,
/// shasha's start menu; GalaDesktopAction's Start passes a copy of the
/// caller's, as CreateProcess would), in console session `console` (a
/// console window's, `nt::action::start_from_menu`; None: none); its
/// process id.
pub fn launch(
    path: &'static str,
    args: &str,
    token: Option<ObjectRef>,
    console: Option<KArc<super::conhost::Session>>,
) -> Result<u64, NtStatus> {
    let object = open_program(path)?;
    let ObjectBody::File(f) = &object.body else { return Err(NtStatus::OBJECT_TYPE_MISMATCH) };
    let layout = super::section::image_layout(f.node)?;
    let file_name = path.rsplit('\\').next().unwrap_or(path);
    let directory = &path[..path.len() - file_name.len()];
    let name = intern(file_name).map_err(|_| NtStatus::NO_MEMORY)?;
    let mut command_line: Vec<u8> = Vec::new();
    for b in [b"\"", path.as_bytes(), b"\""] {
        command_line.try_extend_from_slice(b).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    if !args.is_empty() {
        for b in [b" ", args.as_bytes()] {
            command_line.try_extend_from_slice(b).map_err(|_| NtStatus::NO_MEMORY)?;
        }
    }
    let (process, _thread) = spawn(Spawn {
        name,
        path,
        image: layout,
        command_line: core::str::from_utf8(&command_line).unwrap_or(path),
        current_directory: directory,
        environment: None,
        handles: None,
        suspended: false,
        runtime_data: &[],
        parent_id: 0,
        token,
        console,
    })
    .map_err(|e| match e {
        LoadError::Pe(_) => NtStatus::INVALID_IMAGE_FORMAT,
        _ => NtStatus::NO_MEMORY,
    })?;
    Ok(process.id)
}

/// Creates a service's process (nt::scm_run): the executable at Win32
/// path `path` with `command_line` as given (the service's ImagePath),
/// in `current_directory` (with a trailing backslash), under primary
/// token `token`, with new standard handles (they write to the kernel
/// log) and no console session. Its main thread waits for a resume, so
/// the SCM knows the process before it runs; the process and that
/// thread.
pub fn launch_service(
    path: &str,
    command_line: &str,
    current_directory: &str,
    token: ObjectRef,
) -> Result<(KArc<Process>, ObjectRef), NtStatus> {
    let object = open_program(path)?;
    let ObjectBody::File(f) = &object.body else { return Err(NtStatus::OBJECT_TYPE_MISMATCH) };
    let layout = super::section::image_layout(f.node)?;
    let file_name = path.rsplit('\\').next().unwrap_or(path);
    let name = intern(file_name).map_err(|_| NtStatus::NO_MEMORY)?;
    let path = intern(path).map_err(|_| NtStatus::NO_MEMORY)?;
    spawn(Spawn {
        name,
        path,
        image: layout,
        command_line,
        current_directory,
        environment: None,
        handles: None,
        suspended: true,
        runtime_data: &[],
        parent_id: 0,
        token: Some(token),
        console: None,
    })
    .map_err(|e| match e {
        LoadError::Pe(_) | LoadError::NoWow64 => NtStatus::INVALID_IMAGE_FORMAT,
        _ => NtStatus::NO_MEMORY,
    })
}

#[cfg(test)]
mod tests {
    use super::{expand_environment, number_variable};

    #[test]
    fn environment_variables_expand_as_the_boot_users() {
        let mut buf = [0u8; 96];
        assert_eq!(
            expand_environment("%LocalAppData%\\Programs\\x.exe", &mut buf),
            Some("C:\\Users\\gala\\AppData\\Local\\Programs\\x.exe")
        );
        let mut buf = [0u8; 96];
        assert_eq!(
            expand_environment("%SystemRoot%\\System32\\cmd.exe", &mut buf),
            Some("C:\\Windows\\System32\\cmd.exe")
        );
        // Unknown names and lone percent signs stay.
        let mut buf = [0u8; 96];
        assert_eq!(expand_environment("100% %NoSuch% %%", &mut buf), Some("100% %NoSuch% %%"));
        let mut buf = [0u8; 8];
        assert_eq!(expand_environment("%ProgramFiles%", &mut buf), None);
    }

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
