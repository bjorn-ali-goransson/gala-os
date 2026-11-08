//! A 32-bit (x86) program, run through WOW64 (ARCHITECTURE "Plan:
//! WOW64", ROADMAP WOW): what it sees of itself and a few NT services
//! called through the 32-bit system call path, with the results 64-bit
//! Windows of the pinned build gives a 32-bit program.
//!
//! - It runs in compatibility mode: CS is 0x23.
//! - TEB32 (`fs:`): Self, the stack (esp between StackLimit and
//!   StackBase), ClientId, the PEB, WOW32Reserved (the transition to
//!   64-bit code, a 32-bit address) set.
//! - PEB32: ImageBaseAddress is this image, the OS version 10.0.26100,
//!   VER_PLATFORM_WIN32_NT, NumberOfProcessors > 0, the console
//!   subsystem; the 32-bit process parameters are normalized and hold
//!   the image path and command line (`wow64_test.exe`), the current
//!   directory and an environment.
//! - NtQueryInformationProcess: ProcessBasicInformation (the 24-byte x86
//!   layout) gives this process's id (TEB32.ClientId); ProcessWow64Information
//!   (a 4-byte ULONG_PTR) the 32-bit PEB, which is non-zero for a WOW64
//!   process.
//! - Virtual memory: NtAllocateVirtualMemory gives 64 KiB below 4 GiB,
//!   usable; NtQueryVirtualMemory describes it (and this image, MEM_IMAGE)
//!   in the 28-byte x86 MEMORY_BASIC_INFORMATION; NtProtectVirtualMemory
//!   gives the old protection; NtFreeVirtualMemory releases it.
//! - NtQueryPerformanceCounter (64-bit values through 32-bit pointers).
//! - The 32-bit stack grows through its guard page as it is used (96 KiB
//!   deep), TEB32.StackLimit following.
//! - NtClose of an invalid handle: STATUS_INVALID_HANDLE.
//! - Invalid 32-bit pointers given to converted services fail the call
//!   with STATUS_ACCESS_VIOLATION (the counter, a base address, a
//!   MEMORY_BASIC_INFORMATION buffer, OBJECT_ATTRIBUTES and the
//!   UNICODE_STRING it points to, a HANDLE out-pointer).
//! - Objects named through the x86 OBJECT_ATTRIBUTES and UNICODE_STRING:
//!   events (create, open by name, set with the previous state, query,
//!   clear, waits on one and on two handles), a mutant, a semaphore, a
//!   duplicated handle, a keyed event, a timer, NtDelayExecution,
//!   NtQuerySystemTime; NtQueryObject's name and type (the x86
//!   UNICODE_STRING inside the caller's buffer).
//! - Files: NtCreateFile in C:\Users\Public (the 8-byte IO_STATUS_BLOCK:
//!   FILE_CREATED, the word after it untouched), NtWriteFile and
//!   NtReadFile at offsets, FileStandardInformation and
//!   FilePositionInformation, NtQueryAttributesFile, NtQueryDirectoryFile
//!   with a mask (UNICODE_STRING), a rename (the x86
//!   FILE_RENAME_INFORMATION), FileDispositionInformation.
//! - I/O that completes later: NtNotifyChangeDirectoryFile pends; when a
//!   file is created the kernel writes the x86 IO_STATUS_BLOCK (8 bytes:
//!   status, then the record's length; the word after it untouched).
//! - Sections: a 64 KiB pagefile-backed section mapped below 4 GiB, its
//!   SECTION_BASIC_INFORMATION in the x86 layout (16 bytes).
//! - The registry: CurrentBuildNumber (26100) through NtOpenKey and
//!   NtQueryValueKey; a volatile key under the user's own key (its path
//!   from TokenUser) with a value set, queried, enumerated and deleted.
//! - Tokens: TokenUser and TokenIntegrityLevel in the x86 layout (the SID
//!   pointer into the caller's buffer, the 32-bit lengths, too small:
//!   STATUS_BUFFER_TOO_SMALL with the x86 length), TokenGroups,
//!   TokenStatistics (TokenPrimary).
//! - System information: SystemBasicInformation in the x86 layout (44
//!   bytes; the 32-bit application range: a large-address-aware image's
//!   ends at 0xfffeffff), SystemTimeOfDayInformation.
//! - Thread information: ThreadBasicInformation (28 bytes) names this
//!   thread's TEB32, ThreadQuerySetWin32StartAddress (4 bytes) the entry
//!   point, ThreadTimes; process information: ProcessImageFileName (the
//!   x86 UNICODE_STRING), ProcessVmCounters (44 bytes), ProcessDebugPort
//!   (4 bytes).
//! - GalaCreateProcess from 32-bit code (gala's own service: Windows has
//!   no 0x7f02): this program as a child, its exit status.
//!
//! Service numbers above 0x7f00 are gala's own (ntrt `nr`), and the
//! timers' and keyed events' are unverified: on Windows those checks see
//! STATUS_INVALID_SYSTEM_SERVICE.
//!
//! Prints `wow64_test: ok` and exits with 0, or exits with the number of
//! the failing check (`(line 0x...) wow64_test: check N failed` first:
//! its source line in hex). With the
//! argument `crash` it writes to address 0 instead (an unhandled access
//! violation in 32-bit code); with `child` it exits at once with 0x57 (the
//! GalaCreateProcess check's child).

#![no_std]
#![no_main]

use ntrt::*;

unsafe extern "C" {
    /// The linker-defined start of this image.
    static __ImageBase: u8;
}

const PROCESS_BASIC_INFORMATION: u32 = 0;
const PROCESS_WOW64_INFORMATION: u32 = 26;
const VER_PLATFORM_WIN32_NT: u32 = 2;
const IMAGE_SUBSYSTEM_WINDOWS_CUI: u32 = 3;
const RTL_USER_PROC_PARAMS_NORMALIZED: u32 = 1;
/// 32-bit user code's selector on 64-bit Windows.
const COMPAT_CS: u16 = 0x23;

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// A failure names its check, the source line and what was got.
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        let mut at = *b"(line 0x00000000) ";
        at[8..16].copy_from_slice(&hex32(core::panic::Location::caller().line()));
        if let Ok(text) = core::str::from_utf8(&at) {
            print(text);
        }
        let mut line = *b"wow64_test: check 00000000 failed, got 00000000 00000000\n";
        line[18..26].copy_from_slice(&hex32(self.0));
        line[39..47].copy_from_slice(&hex32((got >> 32) as u32));
        line[48..56].copy_from_slice(&hex32(got as u32));
        if let Ok(text) = core::str::from_utf8(&line) {
            print(text);
        }
        Err(self.0)
    }
}

fn read_u32(addr: usize) -> u32 {
    // SAFETY: the callers read fields of the TEB, PEB and parameters,
    // which are mapped for the life of the process.
    unsafe { core::ptr::read_unaligned(addr as *const u32) }
}

fn read_u16(addr: usize) -> u16 {
    // SAFETY: as in `read_u32`.
    unsafe { core::ptr::read_unaligned(addr as *const u16) }
}

/// A UNICODE_STRING32 at `addr` as (length in bytes, buffer).
fn unicode(addr: usize) -> (usize, usize) {
    (usize::from(read_u16(addr)), read_u32(addr + 4) as usize)
}

/// Whether the UTF-16 string at `buffer` (`len` bytes) ends with `tail`,
/// ignoring ASCII case.
fn ends_with(len: usize, buffer: usize, tail: &str) -> bool {
    let chars = len / 2;
    if buffer == 0 || chars < tail.len() {
        return false;
    }
    let start = buffer + 2 * (chars - tail.len());
    tail.bytes().enumerate().all(|(i, b)| {
        let c = read_u16(start + 2 * i);
        c < 0x80 && (c as u8).eq_ignore_ascii_case(&b)
    })
}

/// Whether the UTF-16 string contains `needle` (ASCII, case ignored).
fn contains(len: usize, buffer: usize, needle: &str) -> bool {
    let chars = len / 2;
    (0..chars.saturating_sub(needle.len()) + 1).any(|at| {
        needle.bytes().enumerate().all(|(i, b)| {
            let c = read_u16(buffer + 2 * (at + i));
            c < 0x80 && (c as u8).eq_ignore_ascii_case(&b)
        })
    })
}

fn run(c: &mut Checks) -> Result<(), u32> {
    // Compatibility mode.
    c.eq(code_segment().into(), COMPAT_CS.into())?;

    // TEB32.
    let teb = teb_field(teb::SELF);
    c.ok(teb != 0, teb as u64)?;
    let (base, limit) = (teb_field(teb::STACK_BASE), teb_field(teb::STACK_LIMIT));
    let sp = stack_pointer();
    c.ok(limit < sp && sp < base, sp as u64)?;
    let pid = teb_field(teb::CLIENT_ID_PROCESS);
    c.ok(pid != 0 && pid.is_multiple_of(4), pid as u64)?;
    let tid = teb_field(teb::CLIENT_ID_THREAD);
    c.ok(tid != 0 && tid.is_multiple_of(4), tid as u64)?;
    let transition = teb_field(teb::WOW32_RESERVED);
    c.ok(transition != 0, transition as u64)?;

    // PEB32.
    let peb_addr = peb() as usize;
    c.ok(peb_addr != 0, peb_addr as u64)?;
    let image = &raw const __ImageBase as usize;
    c.eq(read_u32(peb_addr + peb::IMAGE_BASE_ADDRESS).into(), image as u64)?;
    c.eq(read_u32(peb_addr + peb::OS_MAJOR_VERSION).into(), 10)?;
    c.eq(read_u32(peb_addr + peb::OS_MINOR_VERSION).into(), 0)?;
    c.eq(read_u16(peb_addr + peb::OS_BUILD_NUMBER).into(), os_build().into())?;
    c.eq(read_u32(peb_addr + peb::OS_PLATFORM_ID).into(), VER_PLATFORM_WIN32_NT.into())?;
    let cpus = read_u32(peb_addr + peb::NUMBER_OF_PROCESSORS);
    c.ok(cpus > 0, cpus.into())?;
    c.eq(read_u32(peb_addr + peb::IMAGE_SUBSYSTEM).into(), IMAGE_SUBSYSTEM_WINDOWS_CUI.into())?;

    // The 32-bit process parameters.
    let pp = process_parameters() as usize;
    c.ok(pp != 0, pp as u64)?;
    let flags = read_u32(pp + params::FLAGS);
    c.ok(flags & RTL_USER_PROC_PARAMS_NORMALIZED != 0, flags.into())?;
    let (len, buf) = unicode(pp + params::IMAGE_PATH_NAME);
    c.ok(ends_with(len, buf, "\\wow64_test.exe"), len as u64)?;
    let (len, buf) = unicode(pp + params::COMMAND_LINE);
    c.ok(contains(len, buf, "wow64_test.exe"), len as u64)?;
    let (len, buf) = unicode(pp + params::CURRENT_DIRECTORY);
    c.ok(len >= 6 && read_u16(buf + 2) == u16::from(b':'), len as u64)?;
    let env = read_u32(pp + params::ENVIRONMENT) as usize;
    c.ok(env != 0 && read_u16(env) != 0, env as u64)?;

    // NtQueryInformationProcess.
    let mut info = [0u8; 24];
    let (status, returned) = query_process(CURRENT_PROCESS, PROCESS_BASIC_INFORMATION, &mut info);
    c.eq(status.into(), STATUS_SUCCESS.into())?;
    c.eq(returned.into(), 24)?;
    let field =
        |off: usize| u32::from_le_bytes([info[off], info[off + 1], info[off + 2], info[off + 3]]);
    c.eq(field(0x10).into(), pid as u64)?;
    c.ok(field(4) != 0, field(4).into())?;
    let mut wow = [0u8; 4];
    let (status, returned) = query_process(CURRENT_PROCESS, PROCESS_WOW64_INFORMATION, &mut wow);
    c.eq(status.into(), STATUS_SUCCESS.into())?;
    c.eq(returned.into(), 4)?;
    c.eq(u32::from_le_bytes(wow).into(), peb_addr as u64)?;

    // Virtual memory.
    let (status, base, size) = allocate(0, 0x1_0000, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    c.eq(status.into(), STATUS_SUCCESS.into())?;
    c.ok(base != 0 && base.is_multiple_of(0x1_0000), base as u64)?;
    c.eq(size as u64, 0x1_0000)?;
    // SAFETY: the allocation is committed and writable.
    unsafe {
        core::ptr::write_volatile((base + 0x1234) as *mut u32, 0x5157_3634);
        c.eq(core::ptr::read_volatile((base + 0x1234) as *const u32).into(), 0x5157_3634)?;
    }
    let (status, mbi, returned) = query(base + 0x2000);
    c.eq(status.into(), STATUS_SUCCESS.into())?;
    c.eq(returned as u64, 28)?;
    c.eq(mbi.base_address as u64, (base + 0x2000) as u64)?;
    c.eq(mbi.allocation_base as u64, base as u64)?;
    c.eq(mbi.region_size as u64, 0xe000)?;
    c.eq(mbi.state.into(), MEM_COMMIT.into())?;
    c.eq(mbi.protect.into(), PAGE_READWRITE.into())?;
    let (status, old) = protect(base, 0x1000, PAGE_READONLY);
    c.eq(status.into(), STATUS_SUCCESS.into())?;
    c.eq(old.into(), PAGE_READWRITE.into())?;
    let (status, freed_base, freed_size) = free(base, 0, MEM_RELEASE);
    c.eq(status.into(), STATUS_SUCCESS.into())?;
    c.eq(freed_base as u64, base as u64)?;
    c.eq(freed_size as u64, 0x1_0000)?;
    let (status, mbi, _) = query(image);
    c.eq(status.into(), STATUS_SUCCESS.into())?;
    c.eq(mbi.allocation_base as u64, image as u64)?;
    c.eq(mbi.kind.into(), MEM_IMAGE.into())?;

    // 64-bit results through 32-bit pointers.
    let (status, counter, frequency) = performance_counter();
    c.eq(status.into(), STATUS_SUCCESS.into())?;
    c.ok(counter > 0 && frequency > 0, frequency as u64)?;

    c.eq(close(Handle(0x1234_5678)).into(), STATUS_INVALID_HANDLE.into())?;

    // The 32-bit stack grows through its guard page (TEB32.StackLimit
    // follows).
    let limit = teb_field(teb::STACK_LIMIT);
    c.eq(deep(48).into(), 48 * 0x800)?;
    let lower = teb_field(teb::STACK_LIMIT);
    c.ok(lower < limit && lower <= sp - 48 * 0x800, lower as u64)?;

    bad_pointers(c)?;
    objects(c)?;
    files(c)?;
    pending(c)?;
    sections(c)?;
    let user = tokens(c)?;
    registry(c, &user)?;
    system(c)?;
    thread_and_process(c, teb, pid, tid)?;
    child(c)?;
    Ok(())
}

/// Recurses `n` levels deep with 2 KiB of stack each (96 KiB for 48: past
/// the initial commit); returns the bytes touched.
#[inline(never)]
fn deep(n: u32) -> u32 {
    let mut frame = [0u8; 0x800];
    for (i, b) in frame.iter_mut().enumerate().step_by(0x200) {
        // SAFETY: a write within `frame`, kept by the volatile access.
        unsafe { core::ptr::write_volatile(b, i as u8) };
    }
    let below = if n > 1 { deep(n - 1) } else { 0 };
    // The frame stays live across the call (no tail-call loop).
    core::hint::black_box(&mut frame);
    below + 0x800
}

const STATUS_ACCESS_VIOLATION: u32 = 0xc000_0005;
const STATUS_TIMEOUT: u32 = 0x102;
const STATUS_PENDING: u32 = 0x103;
const STATUS_BUFFER_TOO_SMALL: u32 = 0xc000_0023;
const STATUS_OBJECT_NAME_NOT_FOUND: u32 = 0xc000_0034;
/// Unmapped (the first 64 KiB are never mapped).
const BAD: usize = 0x10;
/// A word after each IO_STATUS_BLOCK that must stay as it is.
const SENTINEL: usize = 0x5a5a_5a5a;

/// The service `n` with `args`: the 32-bit system call for numbers that
/// are long-stable, the 32-bit ntdll's export for those gala chose itself
/// (ntrt's `nt_call`; on gala, without a 32-bit ntdll, the system call).
fn sys(n: u32, args: &[usize]) -> u32 {
    // SAFETY: the callers pass what each service expects, or deliberately
    // invalid pointers the service must reject.
    unsafe { nt_call(n, args) }
}

fn ptr<T>(v: &T) -> usize {
    v as *const T as usize
}

fn mptr<T>(v: &mut T) -> usize {
    v as *mut T as usize
}

fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// An IO_STATUS_BLOCK (x86: status, information) and the word after it.
#[repr(C)]
#[derive(Default)]
struct Iosb {
    status: usize,
    information: usize,
    after: usize,
}

impl Iosb {
    fn new() -> Self {
        Iosb { status: SENTINEL, information: SENTINEL, after: SENTINEL }
    }
}

/// Invalid 32-bit pointers: STATUS_ACCESS_VIOLATION, no crash.
fn bad_pointers(c: &mut Checks) -> Result<(), u32> {
    use nr::*;
    c.eq(sys(NT_QUERY_PERFORMANCE_COUNTER, &[BAD, 0]).into(), STATUS_ACCESS_VIOLATION.into())?;
    let mut size = 0x1000usize;
    let args = [usize::MAX, BAD, 0, mptr(&mut size), (MEM_COMMIT | MEM_RESERVE) as usize, 4];
    c.eq(sys(NT_ALLOCATE_VIRTUAL_MEMORY, &args).into(), STATUS_ACCESS_VIOLATION.into())?;
    let image = &raw const __ImageBase as usize;
    let args = [usize::MAX, image, 0, BAD, 28, 0];
    c.eq(sys(NT_QUERY_VIRTUAL_MEMORY, &args).into(), STATUS_ACCESS_VIOLATION.into())?;
    let mut h = Handle(0);
    let args = [mptr(&mut h), 0x1f_0003, BAD, 0, 0];
    c.eq(sys(NT_CREATE_EVENT, &args).into(), STATUS_ACCESS_VIOLATION.into())?;
    let mut oa = ObjectAttributes::named(&UnicodeString::new(&[]));
    oa.object_name = BAD as *const UnicodeString;
    let args = [mptr(&mut h), 0x1f_0003, ptr(&oa), 0, 0];
    c.eq(sys(NT_CREATE_EVENT, &args).into(), STATUS_ACCESS_VIOLATION.into())?;
    let args = [BAD, 0x1f_0003, 0, 0, 0];
    c.eq(sys(NT_CREATE_EVENT, &args).into(), STATUS_ACCESS_VIOLATION.into())?;
    Ok(())
}

/// Events, a mutant, a semaphore, a duplicate, a keyed event, a timer.
fn objects(c: &mut Checks) -> Result<(), u32> {
    use nr::*;
    let name = wide!("\\BaseNamedObjects\\gala_wow64_test_event");
    let us = UnicodeString::new(&name);
    let oa = ObjectAttributes::named(&us);
    let mut ev = Handle(0);
    // Notification event, not signaled.
    c.eq(sys(NT_CREATE_EVENT, &[mptr(&mut ev), 0x1f_0003, ptr(&oa), 0, 0]).into(), 0)?;
    c.ok(ev.0 != 0, ev.0 as u64)?;
    let mut ev2 = Handle(0);
    c.eq(sys(NT_OPEN_EVENT, &[mptr(&mut ev2), 0x1f_0003, ptr(&oa)]).into(), 0)?;
    c.ok(ev2.0 != 0 && ev2 != ev, ev2.0 as u64)?;
    let mut previous = -1i32;
    c.eq(sys(NT_SET_EVENT, &[ev.0, mptr(&mut previous)]).into(), 0)?;
    c.eq(previous as u64, 0)?;
    let mut info = [0u32; 2];
    let mut returned = 0u32;
    let args = [ev2.0, 0, mptr(&mut info), 8, mptr(&mut returned)];
    c.eq(sys(NT_QUERY_EVENT, &args).into(), 0)?;
    c.eq(returned.into(), 8)?;
    c.eq(info[0].into(), 0)?; // NotificationEvent
    c.eq(info[1].into(), 1)?; // signaled
    let zero = 0i64;
    c.eq(sys(NT_WAIT_FOR_SINGLE_OBJECT, &[ev2.0, 0, ptr(&zero)]).into(), 0)?;
    c.eq(sys(NT_CLEAR_EVENT, &[ev.0]).into(), 0)?;
    c.eq(sys(NT_WAIT_FOR_SINGLE_OBJECT, &[ev.0, 0, ptr(&zero)]).into(), STATUS_TIMEOUT.into())?;

    // A semaphore (count 2 of 5) and the two-handle waits.
    let mut sem = Handle(0);
    c.eq(sys(NT_CREATE_SEMAPHORE, &[mptr(&mut sem), 0x1f_0003, 0, 2, 5]).into(), 0)?;
    let mut prev = 0u32;
    c.eq(sys(NT_RELEASE_SEMAPHORE, &[sem.0, 1, mptr(&mut prev)]).into(), 0)?;
    c.eq(prev.into(), 2)?;
    let mut counts = [0u32; 2];
    let args = [sem.0, 0, mptr(&mut counts), 8, mptr(&mut returned)];
    c.eq(sys(NT_QUERY_SEMAPHORE, &args).into(), 0)?;
    c.eq(counts[0].into(), 3)?;
    c.eq(counts[1].into(), 5)?;
    let handles = [ev, sem];
    // WaitAny (1): the semaphore, index 1; WaitAll (0): the event is not
    // signaled.
    let args = [2, ptr(&handles), 1, 0, ptr(&zero)];
    c.eq(sys(NT_WAIT_FOR_MULTIPLE_OBJECTS, &args).into(), 1)?;
    let args = [2, ptr(&handles), 0, 0, ptr(&zero)];
    c.eq(sys(NT_WAIT_FOR_MULTIPLE_OBJECTS, &args).into(), STATUS_TIMEOUT.into())?;

    // A mutant created owned.
    let mut mutant = Handle(0);
    c.eq(sys(NT_CREATE_MUTANT, &[mptr(&mut mutant), 0x1f_0001, 0, 1]).into(), 0)?;
    let mut state = [0u8; 8];
    let args = [mutant.0, 0, mptr(&mut state), 8, mptr(&mut returned)];
    c.eq(sys(NT_QUERY_MUTANT, &args).into(), 0)?;
    c.eq(word(&state, 0).into(), 0)?; // CurrentCount
    c.eq(state[4].into(), 1)?; // OwnedByCaller
    let mut prev_count = -1i32;
    c.eq(sys(NT_RELEASE_MUTANT, &[mutant.0, mptr(&mut prev_count)]).into(), 0)?;
    c.eq(prev_count as u64, 0)?;

    // A duplicate of the semaphore handle.
    let mut dup = Handle(0);
    let args = [usize::MAX, sem.0, usize::MAX, mptr(&mut dup), 0, 0, 2];
    c.eq(sys(NT_DUPLICATE_OBJECT, &args).into(), 0)?;
    c.ok(dup.0 != 0 && dup != sem, dup.0 as u64)?;
    c.eq(close(dup).into(), 0)?;

    // NtQueryObject: the event's name and its type, as x86 UNICODE_STRINGs
    // inside the buffer.
    let mut buf = [0u8; 256];
    let args = [ev.0, 1, mptr(&mut buf), 256, mptr(&mut returned)];
    c.eq(sys(NT_QUERY_OBJECT, &args).into(), 0)?;
    let (len, at) = (usize::from(u16::from_le_bytes([buf[0], buf[1]])), word(&buf, 4) as usize);
    c.eq(at as u64, (buf.as_ptr() as usize + 8) as u64)?;
    c.ok(ends_with(len, at, "\\gala_wow64_test_event"), len as u64)?;
    let args = [ev.0, 2, mptr(&mut buf), 256, mptr(&mut returned)];
    c.eq(sys(NT_QUERY_OBJECT, &args).into(), 0)?;
    let (len, at) = (usize::from(u16::from_le_bytes([buf[0], buf[1]])), word(&buf, 4) as usize);
    c.ok(at > buf.as_ptr() as usize && at < buf.as_ptr() as usize + 256, at as u64)?;
    c.ok(len == 10 && ends_with(len, at, "Event"), len as u64)?;

    // NtQueryDirectoryObject: \\BaseNamedObjects one entry at a time
    // (OBJECT_DIRECTORY_INFORMATION, x86: Name and TypeName, 16 bytes, then
    // an empty entry) until the event.
    let bno = wide!("\\BaseNamedObjects");
    let bno_us = UnicodeString::new(&bno);
    let bno_oa = ObjectAttributes::named(&bno_us);
    let mut dir = Handle(0);
    // DIRECTORY_QUERY.
    c.eq(sys(NT_OPEN_DIRECTORY_OBJECT, &[mptr(&mut dir), 1, ptr(&bno_oa)]).into(), 0)?;
    let (mut context, mut found) = (0u32, false);
    let mut entry = [0u8; 512];
    for i in 0..4096 {
        let args = [dir.0, mptr(&mut entry), 512, 1, usize::from(i == 0), mptr(&mut context), 0];
        if sys(NT_QUERY_DIRECTORY_OBJECT, &args) != 0 {
            break;
        }
        let base = entry.as_ptr() as usize;
        let (name_len, name) = (usize::from(entry[0]), word(&entry, 4) as usize);
        let (type_len, kind) = (usize::from(entry[8]), word(&entry, 12) as usize);
        c.ok(name > base && name < base + 512, name as u64)?;
        c.ok(word(&entry, 16) == 0 && word(&entry, 20) == 0, 0)?;
        if ends_with(name_len, name, "gala_wow64_test_event") {
            c.ok(type_len == 10 && ends_with(type_len, kind, "Event"), type_len as u64)?;
            found = true;
            break;
        }
    }
    c.ok(found, context.into())?;
    c.eq(close(dir).into(), 0)?;
    // NtQuerySymbolicLinkObject: \??\C: names a volume (the x86
    // UNICODE_STRING's Length set).
    let drive = wide!("\\??\\C:");
    let drive_us = UnicodeString::new(&drive);
    let drive_oa = ObjectAttributes::named(&drive_us);
    let mut link = Handle(0);
    c.eq(sys(NT_OPEN_SYMBOLIC_LINK_OBJECT, &[mptr(&mut link), 1, ptr(&drive_oa)]).into(), 0)?;
    let mut target = [0u16; 128];
    let mut target_us =
        UnicodeString { length: 0, maximum_length: 256, buffer: target.as_mut_ptr() };
    let args = [link.0, mptr(&mut target_us), mptr(&mut returned)];
    c.eq(sys(NT_QUERY_SYMBOLIC_LINK_OBJECT, &args).into(), 0)?;
    let len = usize::from(target_us.length);
    c.ok(contains(len, target.as_ptr() as usize, "\\Device\\HarddiskVolume"), len as u64)?;
    c.eq(close(link).into(), 0)?;

    // A keyed event: a release with nobody waiting times out.
    let mut keyed = Handle(0);
    c.eq(sys(NT_CREATE_KEYED_EVENT, &[mptr(&mut keyed), 0xf_0003, 0, 0]).into(), 0)?;
    let args = [keyed.0, 0x1000, 0, ptr(&zero)];
    c.eq(sys(NT_RELEASE_KEYED_EVENT, &args).into(), STATUS_TIMEOUT.into())?;

    // A notification timer due in 1 ms.
    let mut timer = Handle(0);
    c.eq(sys(NT_CREATE_TIMER, &[mptr(&mut timer), 0x1f_0003, 0, 0]).into(), 0)?;
    let due = -10_000i64;
    let mut was = 0xffu8;
    let args = [timer.0, ptr(&due), 0, 0, 0, 0, mptr(&mut was)];
    c.eq(sys(NT_SET_TIMER, &args).into(), 0)?;
    c.eq(was.into(), 0)?;
    let second = -10_000_000i64;
    c.eq(sys(NT_WAIT_FOR_SINGLE_OBJECT, &[timer.0, 0, ptr(&second)]).into(), 0)?;
    let mut tinfo = [0u8; 16];
    let args = [timer.0, 0, mptr(&mut tinfo), 16, mptr(&mut returned)];
    c.eq(sys(NT_QUERY_TIMER, &args).into(), 0)?;
    c.eq(tinfo[8].into(), 1)?; // TimerState: signaled

    let delay = -10_000i64;
    c.eq(sys(NT_DELAY_EXECUTION, &[0, ptr(&delay)]).into(), 0)?;
    let mut now = 0i64;
    c.eq(sys(NT_QUERY_SYSTEM_TIME, &[mptr(&mut now)]).into(), 0)?;
    c.ok(now > 0, now as u64)?;
    for h in [ev, ev2, sem, mutant, keyed, timer] {
        c.eq(close(h).into(), 0)?;
    }
    Ok(())
}

const FILE_PATH: [u16; 39] = wide!("\\??\\C:\\Users\\Public\\gala_wow64_test.txt");

/// Files through the x86 OBJECT_ATTRIBUTES and IO_STATUS_BLOCK.
fn files(c: &mut Checks) -> Result<(), u32> {
    use nr::*;
    let us = UnicodeString::new(&FILE_PATH);
    let oa = ObjectAttributes::named(&us);
    let mut h = Handle(0);
    let mut iosb = Iosb::new();
    // GENERIC_READ | GENERIC_WRITE | DELETE | SYNCHRONIZE, FILE_OVERWRITE_IF,
    // FILE_SYNCHRONOUS_IO_NONALERT | FILE_NON_DIRECTORY_FILE.
    let args = [mptr(&mut h), 0xc011_0000, ptr(&oa), mptr(&mut iosb), 0, 0x80, 0, 5, 0x60, 0, 0];
    c.eq(sys(NT_CREATE_FILE, &args).into(), 0)?;
    c.eq(iosb.status as u64, 0)?;
    c.ok(iosb.information == 2 || iosb.information == 3, iosb.information as u64)?;
    c.eq(iosb.after as u64, SENTINEL as u64)?;
    let data = b"hello wow64";
    let mut iosb = Iosb::new();
    let offset = 0i64;
    let args = [h.0, 0, 0, 0, mptr(&mut iosb), data.as_ptr() as usize, data.len(), ptr(&offset), 0];
    c.eq(sys(NT_WRITE_FILE, &args).into(), 0)?;
    c.eq(iosb.information as u64, data.len() as u64)?;
    c.eq(iosb.after as u64, SENTINEL as u64)?;
    let mut buf = [0u8; 32];
    let mut iosb = Iosb::new();
    let offset = 6i64;
    let args = [h.0, 0, 0, 0, mptr(&mut iosb), mptr(&mut buf), 32, ptr(&offset), 0];
    c.eq(sys(NT_READ_FILE, &args).into(), 0)?;
    c.eq(iosb.information as u64, 5)?;
    c.ok(&buf[..5] == b"wow64", buf[0].into())?;
    c.eq(iosb.after as u64, SENTINEL as u64)?;
    // FileStandardInformation (5): EndOfFile at 8.
    let mut info = [0u8; 24];
    let mut iosb = Iosb::new();
    let args = [h.0, mptr(&mut iosb), mptr(&mut info), 24, 5];
    c.eq(sys(NT_QUERY_INFORMATION_FILE, &args).into(), 0)?;
    c.eq(word(&info, 8).into(), data.len() as u64)?;
    c.eq(iosb.information as u64, 24)?;
    // FilePositionInformation (14): after the read.
    let mut position = 0i64;
    let args = [h.0, mptr(&mut iosb), mptr(&mut position), 8, 14];
    c.eq(sys(NT_QUERY_INFORMATION_FILE, &args).into(), 0)?;
    c.eq(position as u64, 11)?;
    // NtQueryAttributesFile: FILE_BASIC_INFORMATION, not a directory.
    let mut basic = [0u8; 40];
    c.eq(sys(NT_QUERY_ATTRIBUTES_FILE, &[ptr(&oa), mptr(&mut basic)]).into(), 0)?;
    c.eq((word(&basic, 32) & 0x10).into(), 0)?;

    // The directory, with a mask: FileNamesInformation (12).
    let dir = wide!("\\??\\C:\\Users\\Public");
    let dir_us = UnicodeString::new(&dir);
    let dir_oa = ObjectAttributes::named(&dir_us);
    let mut d = Handle(0);
    let mut iosb = Iosb::new();
    // FILE_LIST_DIRECTORY | SYNCHRONIZE, share all, FILE_DIRECTORY_FILE |
    // FILE_SYNCHRONOUS_IO_NONALERT.
    let args = [mptr(&mut d), 0x10_0001, ptr(&dir_oa), mptr(&mut iosb), 7, 0x21];
    c.eq(sys(NT_OPEN_FILE, &args).into(), 0)?;
    let mask = wide!("gala_wow64_test.txt");
    let mask_us = UnicodeString::new(&mask);
    let mut list = [0u8; 256];
    let mut iosb = Iosb::new();
    let args = [d.0, 0, 0, 0, mptr(&mut iosb), mptr(&mut list), 256, 12, 1, ptr(&mask_us), 1];
    c.eq(sys(NT_QUERY_DIRECTORY_FILE, &args).into(), 0)?;
    c.eq(word(&list, 8).into(), 2 * mask.len() as u64)?;
    c.ok(ends_with(2 * mask.len(), list.as_ptr() as usize + 12, "gala_wow64_test.txt"), 0)?;
    c.eq(iosb.after as u64, SENTINEL as u64)?;

    // A rename: FILE_RENAME_INFORMATION (x86: ReplaceIfExists, RootDirectory
    // at 4, FileNameLength at 8, the name at 12).
    let new_name = wide!("\\??\\C:\\Users\\Public\\gala_wow64_test2.txt");
    let mut rename = [0u8; 12 + 2 * 40];
    rename[8..12].copy_from_slice(&(2 * new_name.len() as u32).to_le_bytes());
    for (i, ch) in new_name.iter().enumerate() {
        rename[12 + 2 * i..14 + 2 * i].copy_from_slice(&ch.to_le_bytes());
    }
    let len = 12 + 2 * new_name.len();
    let mut iosb = Iosb::new();
    let args = [h.0, mptr(&mut iosb), mptr(&mut rename), len, 10];
    c.eq(sys(NT_SET_INFORMATION_FILE, &args).into(), 0)?;
    let new_us = UnicodeString::new(&new_name);
    let new_oa = ObjectAttributes::named(&new_us);
    c.eq(sys(NT_QUERY_ATTRIBUTES_FILE, &[ptr(&new_oa), mptr(&mut basic)]).into(), 0)?;
    let old = sys(NT_QUERY_ATTRIBUTES_FILE, &[ptr(&oa), mptr(&mut basic)]);
    c.eq(old.into(), STATUS_OBJECT_NAME_NOT_FOUND.into())?;
    // FileDispositionInformation (13): deleted on close.
    let delete = 1u8;
    let args = [h.0, mptr(&mut iosb), ptr(&delete), 1, 13];
    c.eq(sys(NT_SET_INFORMATION_FILE, &args).into(), 0)?;
    c.eq(close(h).into(), 0)?;
    c.eq(close(d).into(), 0)?;
    let gone = sys(NT_QUERY_ATTRIBUTES_FILE, &[ptr(&new_oa), mptr(&mut basic)]);
    c.eq(gone.into(), STATUS_OBJECT_NAME_NOT_FOUND.into())?;
    Ok(())
}

/// I/O that completes after its call returned: the kernel writes the x86
/// IO_STATUS_BLOCK.
fn pending(c: &mut Checks) -> Result<(), u32> {
    use nr::*;
    let dir = wide!("\\??\\C:\\Users\\Public\\gala_wow64_dir");
    let dir_us = UnicodeString::new(&dir);
    let dir_oa = ObjectAttributes::named(&dir_us);
    let mut d = Handle(0);
    let mut iosb = Iosb::new();
    // FILE_LIST_DIRECTORY | DELETE | SYNCHRONIZE, share all, FILE_OPEN_IF,
    // FILE_DIRECTORY_FILE: asynchronous (no FILE_SYNCHRONOUS_IO_*).
    let args = [mptr(&mut d), 0x11_0001, ptr(&dir_oa), mptr(&mut iosb), 0, 0x10, 7, 3, 1, 0, 0];
    c.eq(sys(NT_CREATE_FILE, &args).into(), 0)?;
    let mut event = Handle(0);
    c.eq(sys(NT_CREATE_EVENT, &[mptr(&mut event), 0x1f_0003, 0, 0, 0]).into(), 0)?;
    let mut records = [0u8; 128];
    let mut iosb = Iosb::new();
    // FILE_NOTIFY_CHANGE_FILE_NAME, not the subtree.
    let args = [d.0, event.0, 0, 0, mptr(&mut iosb), mptr(&mut records), 128, 1, 0];
    c.eq(sys(NT_NOTIFY_CHANGE_DIRECTORY_FILE, &args).into(), STATUS_PENDING.into())?;
    // A file in it: the request completes.
    let file = wide!("\\??\\C:\\Users\\Public\\gala_wow64_dir\\a.txt");
    let file_us = UnicodeString::new(&file);
    let file_oa = ObjectAttributes::named(&file_us);
    let mut f = Handle(0);
    let mut fiosb = Iosb::new();
    // DELETE | SYNCHRONIZE | GENERIC_WRITE, FILE_CREATE, delete on close.
    let args =
        [mptr(&mut f), 0x4011_0000, ptr(&file_oa), mptr(&mut fiosb), 0, 0x80, 0, 2, 0x1060, 0, 0];
    c.eq(sys(NT_CREATE_FILE, &args).into(), 0)?;
    let second = -50_000_000i64;
    c.eq(sys(NT_WAIT_FOR_SINGLE_OBJECT, &[event.0, 0, ptr(&second)]).into(), 0)?;
    // FILE_NOTIFY_INFORMATION: NextEntryOffset 0, FILE_ACTION_ADDED, 10
    // bytes of "a.txt": 12 + 10 bytes.
    c.eq(iosb.status as u64, 0)?;
    c.eq(iosb.information as u64, 22)?;
    c.eq(iosb.after as u64, SENTINEL as u64)?;
    c.eq(word(&records, 4).into(), 1)?;
    c.eq(word(&records, 8).into(), 10)?;
    c.eq(close(f).into(), 0)?;
    // The directory goes when its handle closes (FileDispositionInformation).
    let delete = 1u8;
    let mut iosb = Iosb::new();
    let args = [d.0, mptr(&mut iosb), ptr(&delete), 1, 13];
    c.eq(sys(NT_SET_INFORMATION_FILE, &args).into(), 0)?;
    c.eq(close(d).into(), 0)?;
    c.eq(close(event).into(), 0)?;
    Ok(())
}

/// A pagefile-backed section and a view of it.
fn sections(c: &mut Checks) -> Result<(), u32> {
    use nr::*;
    let mut s = Handle(0);
    let size = 0x1_0000i64;
    // SECTION_ALL_ACCESS, PAGE_READWRITE, SEC_COMMIT.
    let args = [mptr(&mut s), 0xf_001f, 0, ptr(&size), 4, 0x800_0000, 0];
    c.eq(sys(NT_CREATE_SECTION, &args).into(), 0)?;
    let (mut base, mut view) = (0usize, 0usize);
    // ViewUnmap (2), PAGE_READWRITE.
    let args = [s.0, usize::MAX, mptr(&mut base), 0, 0, 0, mptr(&mut view), 2, 0, 4];
    c.eq(sys(NT_MAP_VIEW_OF_SECTION, &args).into(), 0)?;
    c.ok(base != 0 && base.is_multiple_of(0x1_0000), base as u64)?;
    c.eq(view as u64, 0x1_0000)?;
    // SAFETY: the view is mapped read-write.
    unsafe {
        core::ptr::write_volatile((base + 0x100) as *mut u32, 0x5345_4354);
        c.eq(core::ptr::read_volatile((base + 0x100) as *const u32).into(), 0x5345_4354)?;
    }
    // SECTION_BASIC_INFORMATION (x86): BaseAddress, AllocationAttributes,
    // MaximumSize at 8.
    let mut info = [0u8; 16];
    let mut returned = 0u32;
    let args = [s.0, 0, mptr(&mut info), 16, mptr(&mut returned)];
    c.eq(sys(NT_QUERY_SECTION, &args).into(), 0)?;
    c.eq(returned.into(), 16)?;
    c.eq(word(&info, 4).into(), 0x800_0000)?;
    c.eq(word(&info, 8).into(), 0x1_0000)?;
    c.eq(sys(NT_UNMAP_VIEW_OF_SECTION, &[usize::MAX, base]).into(), 0)?;
    c.eq(close(s).into(), 0)?;
    Ok(())
}

/// The user's SID as text (`S-1-5-21-...`) from a TOKEN_USER's SID.
struct SidText {
    chars: [u16; 96],
    len: usize,
}

impl SidText {
    fn push(&mut self, b: u8) {
        if self.len < self.chars.len() {
            self.chars[self.len] = b.into();
            self.len += 1;
        }
    }

    fn number(&mut self, mut v: u32) {
        let mut digits = [0u8; 10];
        let mut n = 0;
        loop {
            digits[n] = b'0' + (v % 10) as u8;
            n += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        for i in (0..n).rev() {
            self.push(digits[i]);
        }
    }
}

/// Tokens: the x86 layouts of the SID-bearing classes. Returns the user's
/// SID as text.
fn tokens(c: &mut Checks) -> Result<SidText, u32> {
    use nr::*;
    let mut t = Handle(0);
    c.eq(sys(NT_OPEN_PROCESS_TOKEN, &[usize::MAX, 8, mptr(&mut t)]).into(), 0)?;
    // TokenUser: SID_AND_ATTRIBUTES (8 bytes), the SID after it.
    let mut buf = [0u8; 128];
    let mut returned = 0u32;
    let args = [t.0, 1, mptr(&mut buf), 128, mptr(&mut returned)];
    c.eq(sys(NT_QUERY_INFORMATION_TOKEN, &args).into(), 0)?;
    let sid = word(&buf, 0) as usize;
    c.eq(sid as u64, (buf.as_ptr() as usize + 8) as u64)?;
    let sub = usize::from(buf[9]);
    c.eq(buf[8].into(), 1)?; // revision
    c.eq(returned.into(), (8 + 8 + 4 * sub) as u64)?;
    let full = returned;
    let mut small = [0u8; 8];
    let args = [t.0, 1, mptr(&mut small), 8, mptr(&mut returned)];
    c.eq(sys(NT_QUERY_INFORMATION_TOKEN, &args).into(), STATUS_BUFFER_TOO_SMALL.into())?;
    c.eq(returned.into(), full.into())?;
    let mut text = SidText { chars: [0; 96], len: 0 };
    text.push(b'S');
    text.push(b'-');
    text.number(buf[8].into());
    text.push(b'-');
    text.number(u32::from_be_bytes([buf[12], buf[13], buf[14], buf[15]]));
    for i in 0..sub {
        text.push(b'-');
        text.number(word(&buf, 16 + 4 * i));
    }
    // TokenIntegrityLevel: TOKEN_MANDATORY_LABEL, a S-1-16-x SID after it.
    let args = [t.0, 25, mptr(&mut buf), 128, mptr(&mut returned)];
    c.eq(sys(NT_QUERY_INFORMATION_TOKEN, &args).into(), 0)?;
    c.eq(word(&buf, 0).into(), (buf.as_ptr() as usize + 8) as u64)?;
    c.eq(buf[15].into(), 16)?; // SECURITY_MANDATORY_LABEL_AUTHORITY
    c.eq(returned.into(), 20)?;
    // TokenGroups: GroupCount, then SID_AND_ATTRIBUTES (8 bytes each)
    // pointing into the buffer.
    let mut groups = [0u8; 1024];
    let args = [t.0, 2, mptr(&mut groups), 1024, mptr(&mut returned)];
    c.eq(sys(NT_QUERY_INFORMATION_TOKEN, &args).into(), 0)?;
    let count = word(&groups, 0) as usize;
    c.ok(count > 0 && 4 + 8 * count < returned as usize, count as u64)?;
    let (lo, hi) = (groups.as_ptr() as usize, groups.as_ptr() as usize + returned as usize);
    for i in 0..count {
        let at = word(&groups, 4 + 8 * i) as usize;
        c.ok(at >= lo + 4 + 8 * count && at < hi, at as u64)?;
    }
    // TokenStatistics (56 bytes, no pointers): TokenType at 0x18.
    let mut stats = [0u8; 56];
    let args = [t.0, 10, mptr(&mut stats), 56, mptr(&mut returned)];
    c.eq(sys(NT_QUERY_INFORMATION_TOKEN, &args).into(), 0)?;
    c.eq(returned.into(), 56)?;
    c.eq(word(&stats, 0x18).into(), 1)?; // TokenPrimary
    c.eq(close(t).into(), 0)?;
    Ok(text)
}

/// The registry through the x86 OBJECT_ATTRIBUTES and UNICODE_STRING.
fn registry(c: &mut Checks, user: &SidText) -> Result<(), u32> {
    use nr::*;
    let path = wide!("\\Registry\\Machine\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
    let us = UnicodeString::new(&path);
    let oa = ObjectAttributes::named(&us);
    let mut k = Handle(0);
    c.eq(sys(NT_OPEN_KEY, &[mptr(&mut k), 0x2_0019, ptr(&oa)]).into(), 0)?;
    let value = wide!("CurrentBuildNumber");
    let value_us = UnicodeString::new(&value);
    // KeyValuePartialInformation: TitleIndex, Type, DataLength, Data.
    let mut info = [0u8; 64];
    let mut returned = 0u32;
    let args = [k.0, ptr(&value_us), 2, mptr(&mut info), 64, mptr(&mut returned)];
    c.eq(sys(NT_QUERY_VALUE_KEY, &args).into(), 0)?;
    c.eq(word(&info, 4).into(), 1)?; // REG_SZ
    c.eq(word(&info, 8).into(), 12)?;
    let digits = os_build_digits();
    let build = core::str::from_utf8(&digits).unwrap_or("");
    c.ok(ends_with(10, info.as_ptr() as usize + 12, build), 0)?;
    c.eq(close(k).into(), 0)?;

    // A volatile key under the user's own (HKCU\Software is not
    // redirected for 32-bit programs).
    let mut name = [0u16; 160];
    let mut n = 0;
    for part in [&wide!("\\Registry\\User\\")[..], &user.chars[..user.len]] {
        name[n..n + part.len()].copy_from_slice(part);
        n += part.len();
    }
    let tail = wide!("\\Software\\GalaWow64Test");
    name[n..n + tail.len()].copy_from_slice(&tail);
    n += tail.len();
    let key_us = UnicodeString::new(&name[..n]);
    let key_oa = ObjectAttributes::named(&key_us);
    let mut disposition = 0u32;
    // KEY_ALL_ACCESS, REG_OPTION_VOLATILE.
    let args = [mptr(&mut k), 0xf_003f, ptr(&key_oa), 0, 0, 1, mptr(&mut disposition)];
    c.eq(sys(NT_CREATE_KEY, &args).into(), 0)?;
    c.eq(disposition.into(), 1)?; // REG_CREATED_NEW_KEY
    let v = wide!("Wow64Value");
    let v_us = UnicodeString::new(&v);
    let data = 0x5157_3634u32;
    let args = [k.0, ptr(&v_us), 0, 4, ptr(&data), 4];
    c.eq(sys(NT_SET_VALUE_KEY, &args).into(), 0)?;
    let args = [k.0, ptr(&v_us), 2, mptr(&mut info), 64, mptr(&mut returned)];
    c.eq(sys(NT_QUERY_VALUE_KEY, &args).into(), 0)?;
    c.eq(returned.into(), 16)?;
    c.eq(word(&info, 4).into(), 4)?; // REG_DWORD
    c.eq(word(&info, 12).into(), data.into())?;
    // KeyValueBasicInformation of value 0: TitleIndex, Type, NameLength, Name.
    let args = [k.0, 0, 0, mptr(&mut info), 64, mptr(&mut returned)];
    c.eq(sys(NT_ENUMERATE_VALUE_KEY, &args).into(), 0)?;
    c.eq(word(&info, 8).into(), 2 * v.len() as u64)?;
    c.eq(sys(NT_DELETE_VALUE_KEY, &[k.0, ptr(&v_us)]).into(), 0)?;
    c.eq(sys(NT_DELETE_KEY, &[k.0]).into(), 0)?;
    c.eq(close(k).into(), 0)?;
    Ok(())
}

/// System information in the x86 layouts.
fn system(c: &mut Checks) -> Result<(), u32> {
    use nr::*;
    // SYSTEM_BASIC_INFORMATION (x86, 44 bytes): PageSize at 8,
    // AllocationGranularity at 24, the application range at 28 and 32,
    // ActiveProcessorsAffinityMask at 36, NumberOfProcessors at 40.
    let mut info = [0u8; 44];
    let mut returned = 0u32;
    let args = [0, mptr(&mut info), 44, mptr(&mut returned)];
    c.eq(sys(NT_QUERY_SYSTEM_INFORMATION, &args).into(), 0)?;
    c.eq(returned.into(), 44)?;
    c.eq(word(&info, 8).into(), 0x1000)?;
    c.eq(word(&info, 24).into(), 0x1_0000)?;
    c.eq(word(&info, 28).into(), 0x1_0000)?;
    // Rust's i686 images are large-address-aware.
    c.eq(word(&info, 32).into(), 0xfffe_ffff)?;
    c.ok(word(&info, 36) != 0, word(&info, 36).into())?;
    c.ok(info[40] > 0, info[40].into())?;
    // SystemTimeOfDayInformation (48 bytes, no pointers).
    let mut tod = [0u8; 48];
    let args = [3, mptr(&mut tod), 48, mptr(&mut returned)];
    c.eq(sys(NT_QUERY_SYSTEM_INFORMATION, &args).into(), 0)?;
    c.ok(word(&tod, 12) != 0, word(&tod, 12).into())?; // CurrentTime (high)
    Ok(())
}

/// Thread and process information classes in the x86 layouts.
fn thread_and_process(c: &mut Checks, teb: usize, pid: usize, tid: usize) -> Result<(), u32> {
    use nr::*;
    // THREAD_BASIC_INFORMATION (x86, 28 bytes): ExitStatus, TebBaseAddress
    // (the TEB32), ClientId at 8, AffinityMask at 16.
    let mut info = [0u8; 28];
    let mut returned = 0u32;
    let args = [usize::MAX - 1, 0, mptr(&mut info), 28, mptr(&mut returned)];
    c.eq(sys(NT_QUERY_INFORMATION_THREAD, &args).into(), 0)?;
    c.eq(returned.into(), 28)?;
    c.eq(word(&info, 0).into(), STATUS_PENDING.into())?;
    c.eq(word(&info, 4).into(), teb as u64)?;
    c.eq(word(&info, 8).into(), pid as u64)?;
    c.eq(word(&info, 12).into(), tid as u64)?;
    // ThreadQuerySetWin32StartAddress (9): the entry point, 4 bytes.
    let mut start = 0u32;
    let args = [usize::MAX - 1, 9, mptr(&mut start), 4, mptr(&mut returned)];
    c.eq(sys(NT_QUERY_INFORMATION_THREAD, &args).into(), 0)?;
    c.eq(returned.into(), 4)?;
    c.eq(start.into(), mainCRTStartup as *const () as usize as u64)?;
    // ThreadTimes (1): 32 bytes, CreateTime set.
    let mut times = [0u8; 32];
    let args = [usize::MAX - 1, 1, mptr(&mut times), 32, mptr(&mut returned)];
    c.eq(sys(NT_QUERY_INFORMATION_THREAD, &args).into(), 0)?;
    c.ok(word(&times, 4) != 0, word(&times, 4).into())?;

    // ProcessImageFileName (27): a UNICODE_STRING (8 bytes) and the name
    // after it.
    let mut name = [0u8; 512];
    let args = [usize::MAX, 27, mptr(&mut name), 512, mptr(&mut returned)];
    c.eq(sys(NT_QUERY_INFORMATION_PROCESS, &args).into(), 0)?;
    let len = usize::from(u16::from_le_bytes([name[0], name[1]]));
    c.eq(word(&name, 4).into(), (name.as_ptr() as usize + 8) as u64)?;
    c.ok(ends_with(len, name.as_ptr() as usize + 8, "\\wow64_test.exe"), len as u64)?;
    c.eq(returned.into(), (8 + len + 2) as u64)?;
    // ProcessVmCounters (3): VM_COUNTERS (x86, 44 bytes), PagefileUsage (the
    // commit charge) at 36.
    let mut vm = [0u8; 44];
    let args = [usize::MAX, 3, mptr(&mut vm), 44, mptr(&mut returned)];
    c.eq(sys(NT_QUERY_INFORMATION_PROCESS, &args).into(), 0)?;
    c.eq(returned.into(), 44)?;
    c.ok(word(&vm, 36) != 0, word(&vm, 36).into())?;
    // MemorySectionName (2): this image's file, an x86 UNICODE_STRING.
    let image = &raw const __ImageBase as usize;
    let args = [usize::MAX, image, 2, mptr(&mut name), 512, 0];
    c.eq(sys(NT_QUERY_VIRTUAL_MEMORY, &args).into(), 0)?;
    let len = usize::from(u16::from_le_bytes([name[0], name[1]]));
    c.eq(word(&name, 4).into(), (name.as_ptr() as usize + 8) as u64)?;
    c.ok(ends_with(len, name.as_ptr() as usize + 8, "\\wow64_test.exe"), len as u64)?;
    // SystemProcessInformation (5): the x86 records (0xb8 bytes, threads of
    // 0x40, the name after them); this process's.
    // The size depends on the machine (a real one runs hundreds of
    // processes): grow to what STATUS_INFO_LENGTH_MISMATCH says is needed,
    // with room for processes started meanwhile.
    let mut size = 0x4_0000;
    let list = loop {
        let (status, list, _) = allocate(0, size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
        c.eq(status.into(), 0)?;
        let args = [5, list, size, mptr(&mut returned)];
        let status = sys(NT_QUERY_SYSTEM_INFORMATION, &args);
        if status != 0xc000_0004 || size >= 0x400_0000 {
            c.eq(status.into(), 0)?;
            break list;
        }
        let (status, _, _) = free(list, 0, MEM_RELEASE);
        c.eq(status.into(), 0)?;
        size = (returned as usize).max(size) + 0x1_0000;
    };
    let (mut at, mut mine) = (list, 0);
    loop {
        if read_u32(at + 0x44) as usize == pid {
            mine = at;
        }
        let next = read_u32(at) as usize;
        if next == 0 || at + next >= list + returned as usize {
            break;
        }
        at += next;
    }
    c.ok(mine != 0, 0)?;
    let threads = read_u32(mine + 4) as usize;
    c.ok(threads >= 1, threads as u64)?;
    let (len, buf) = unicode(mine + 0x38);
    c.ok(ends_with(len, buf, "wow64_test.exe"), len as u64)?;
    c.ok(buf > mine + 0xb8 && buf < list + returned as usize, buf as u64)?;
    // The first thread's ClientId (at 0x20 of a 0x40 record).
    c.eq(read_u32(mine + 0xb8 + 0x20).into(), pid as u64)?;
    let (status, _, _) = free(list, 0, MEM_RELEASE);
    c.eq(status.into(), 0)?;
    // ProcessDebugPort (7): a 4-byte port, none.
    let mut port = 0xffff_ffffu32;
    let args = [usize::MAX, 7, mptr(&mut port), 4, mptr(&mut returned)];
    c.eq(sys(NT_QUERY_INFORMATION_PROCESS, &args).into(), 0)?;
    c.eq(port.into(), 0)?;
    c.eq(returned.into(), 4)?;
    Ok(())
}

/// GalaCreateProcess from 32-bit code: this program (its ImagePathName as
/// an NT path) as a child.
fn child(c: &mut Checks) -> Result<(), u32> {
    use nr::*;
    let (len, buf) = unicode(process_parameters() as usize + params::IMAGE_PATH_NAME);
    let mut image = [0u16; 264];
    let chars = (len / 2).min(260);
    image[..4].copy_from_slice(&wide!("\\??\\"));
    for i in 0..chars {
        image[4 + i] = read_u16(buf + 2 * i);
    }
    let line = wide!("wow64_test.exe child");
    let (status, process, thread) = create_process(&image[..4 + chars], &line);
    c.eq(status.into(), 0)?;
    let ten = -100_000_000i64;
    c.eq(sys(NT_WAIT_FOR_SINGLE_OBJECT, &[process.0, 0, ptr(&ten)]).into(), 0)?;
    let mut info = [0u8; 24];
    let (status, _) = query_process(process, PROCESS_BASIC_INFORMATION, &mut info);
    c.eq(status.into(), 0)?;
    c.eq(word(&info, 0).into(), 0x57)?;
    c.eq(close(thread).into(), 0)?;
    c.eq(close(process).into(), 0)?;
    Ok(())
}

fn main() -> u32 {
    // `wow64_test crash`: an unhandled access violation in 32-bit code
    // (scenario wow64_crash: the crash report's 32-bit registers).
    let pp = process_parameters() as usize;
    let (len, buf) = unicode(pp + params::COMMAND_LINE);
    if ends_with(len, buf, " child") {
        return 0x57;
    }
    if ends_with(len, buf, " crash") {
        print("wow64_test: writing to address 0\n");
        // SAFETY: deliberately faults; the process ends.
        unsafe { core::ptr::write_volatile(core::ptr::null_mut::<u32>(), 1) };
    }
    let mut c = Checks(0);
    match run(&mut c) {
        Ok(()) => {
            print("wow64_test: ok\n");
            0
        }
        Err(n) => n,
    }
}

entry!(main);
