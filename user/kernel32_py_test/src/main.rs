//! What CPython 3.13 (python313.dll) imports from kernel32 beyond what
//! other programs did, with the results real Windows gives (documented
//! contracts):
//!
//! - PathCchSkipRoot: past a drive root or a UNC server and share;
//!   E_INVALIDARG for a relative path.
//! - PathCchCombineEx: joined and canonicalized ("..", an absolute or a
//!   rooted second path); a buffer too small is
//!   HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER).
//! - NeedCurrentDirectoryForExePathW: TRUE for a bare name unless
//!   NoDefaultCurrentDirectoryInExePath is set; TRUE for a name with a
//!   backslash either way.
//! - CopyFile2: copies (S_OK, the same contents); COPY_FILE_FAIL_IF_EXISTS
//!   on an existing target is HRESULT_FROM_WIN32(ERROR_FILE_EXISTS).
//! - SetWaitableTimerEx (no wake context): the timer is signaled after its
//!   relative due time.
//! - GetNamedPipeHandleStateW: a message-mode server end's state
//!   (PIPE_READMODE_MESSAGE, waiting) and its instance count (1, then 2).
//! - PssCaptureSnapshot / PssQuerySnapshot(PSS_QUERY_PROCESS_INFORMATION)
//!   / PssFreeSnapshot: this process's id and its parent's (as
//!   ProcessBasicInformation gives it), PSS_PROCESS_INFORMATION being 704
//!   bytes on x64 (ProcessId at 28, ParentProcessId at 32); another size
//!   is ERROR_BAD_LENGTH.
//!
//! - VerifyVersionInfoA (what _socket.pyd imports): OSVERSIONINFOEXA's
//!   layout (156 bytes); major version >= 6 holds, >= 99 is FALSE with
//!   ERROR_OLD_WIN_VERSION.
//!
//! Prints `kernel32_py_test: ok` or exits with the number of the failing
//! check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        sa: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn DeleteFileW(name: *const u16) -> Bool;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetLastError() -> u32;
    fn SetEnvironmentVariableW(name: *const u16, value: *const u16) -> Bool;
    fn GetCurrentProcess() -> Handle;
    fn GetCurrentProcessId() -> u32;
    fn CreateWaitableTimerW(sa: *mut c_void, manual: Bool, name: *const u16) -> Handle;
    fn SetWaitableTimerEx(
        h: Handle,
        due: *const i64,
        period: i32,
        routine: *mut c_void,
        arg: *mut c_void,
        wake_context: *mut c_void,
        tolerable_delay: u32,
    ) -> Bool;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn CreateNamedPipeW(
        name: *const u16,
        open_mode: u32,
        pipe_mode: u32,
        max_instances: u32,
        out_size: u32,
        in_size: u32,
        timeout: u32,
        sa: *mut c_void,
    ) -> Handle;
    fn GetNamedPipeHandleStateW(
        h: Handle,
        state: *mut u32,
        instances: *mut u32,
        max_collection: *mut u32,
        timeout: *mut u32,
        user: *mut u16,
        user_size: u32,
    ) -> Bool;
    fn NeedCurrentDirectoryForExePathW(name: *const u16) -> Bool;
    fn CopyFile2(src: *const u16, dst: *const u16, params: *const u8) -> i32;
    fn PssCaptureSnapshot(process: Handle, flags: u32, context_flags: u32, out: *mut Handle)
    -> u32;
    fn PssQuerySnapshot(snapshot: Handle, class: u32, buf: *mut u8, len: u32) -> u32;
    fn PssFreeSnapshot(process: Handle, snapshot: Handle) -> u32;
    fn VerifyVersionInfoA(info: *mut u8, type_mask: u32, condition: u64) -> Bool;
    fn VerSetConditionMask(mask: u64, type_mask: u32, condition: u8) -> u64;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "api-ms-win-core-path-l1-1-0", kind = "raw-dylib")]
unsafe extern "system" {
    fn PathCchSkipRoot(path: *const u16, root_end: *mut *const u16) -> i32;
    fn PathCchCombineEx(out: *mut u16, cch: usize, a: *const u16, b: *const u16, flags: u32)
    -> i32;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtQueryInformationProcess(
        h: Handle,
        class: u32,
        buf: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> i32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const E_INVALIDARG: u32 = 0x8007_0057;
const HR_INSUFFICIENT_BUFFER: u32 = 0x8007_007a;
const HR_FILE_EXISTS: u32 = 0x8007_0050;
const ERROR_BAD_LENGTH: u32 = 24;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const PIPE_ACCESS_DUPLEX: u32 = 3;
const PIPE_TYPE_MESSAGE: u32 = 4;
const PIPE_READMODE_MESSAGE: u32 = 2;
const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
const PSS_QUERY_PROCESS_INFORMATION: u32 = 0;
const PSS_PROCESS_INFORMATION_SIZE: usize = 704;

macro_rules! w {
    ($s:literal) => {{
        const S: &str = concat!($s, "\0");
        const N: usize = S.len();
        const W: [u16; N] = {
            let b = S.as_bytes();
            let mut w = [0u16; N];
            let mut i = 0;
            while i < N {
                w[i] = b[i] as u16;
                i += 1;
            }
            w
        };
        W.as_ptr()
    }};
}

fn out(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid buffer and output pointer.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        )
    };
}

fn hex(v: u32) -> [u8; 8] {
    let mut s = [0u8; 8];
    for (i, c) in s.iter_mut().enumerate() {
        let d = (v >> (28 - 4 * i)) & 0xf;
        *c = if d < 10 { b'0' + d as u8 } else { b'a' + d as u8 - 10 };
    }
    s
}

struct Checks(u32);

impl Checks {
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"kernel32_py_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

/// The NUL-terminated string at `p` equals ASCII `s`.
fn is(p: *const u16, s: &[u8]) -> bool {
    // SAFETY: a NUL-terminated string the API returned.
    unsafe {
        for (i, &b) in s.iter().enumerate() {
            if *p.add(i) != u16::from(b) {
                return false;
            }
        }
        *p.add(s.len()) == 0
    }
}

fn paths(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: the APIs' documented arguments.
    unsafe {
        let mut end: *const u16 = core::ptr::null();
        c.eq(PathCchSkipRoot(w!("C:\\a\\b"), &mut end) as u32 as u64, 0)?;
        c.ok(is(end, b"a\\b"), 0)?;
        c.eq(PathCchSkipRoot(w!("\\\\server\\share\\x"), &mut end) as u32 as u64, 0)?;
        c.ok(is(end, b"x"), 0)?;
        c.eq(PathCchSkipRoot(w!("a\\b"), &mut end) as u32 as u64, E_INVALIDARG.into())?;

        let mut buf = [0u16; 64];
        let combine = |buf: &mut [u16; 64], a: *const u16, b: *const u16, cch: usize| {
            PathCchCombineEx(buf.as_mut_ptr(), cch, a, b, 0) as u32 as u64
        };
        c.eq(combine(&mut buf, w!("C:\\a"), w!("b\\c"), 64), 0)?;
        c.ok(is(buf.as_ptr(), b"C:\\a\\b\\c"), 0)?;
        c.eq(combine(&mut buf, w!("C:\\a\\b"), w!("..\\c"), 64), 0)?;
        c.ok(is(buf.as_ptr(), b"C:\\a\\c"), 0)?;
        c.eq(combine(&mut buf, w!("C:\\a"), w!("D:\\x"), 64), 0)?;
        c.ok(is(buf.as_ptr(), b"D:\\x"), 0)?;
        c.eq(combine(&mut buf, w!("C:\\a\\b"), w!("\\y"), 64), 0)?;
        c.ok(is(buf.as_ptr(), b"C:\\y"), 0)?;
        c.eq(combine(&mut buf, w!("C:\\a"), w!("bcdef"), 5), HR_INSUFFICIENT_BUFFER.into())?;

        c.ok(NeedCurrentDirectoryForExePathW(w!("python.exe")) != 0, 0)?;
        c.ok(NeedCurrentDirectoryForExePathW(w!("x\\python.exe")) != 0, 0)?;
        c.ok(SetEnvironmentVariableW(w!("NoDefaultCurrentDirectoryInExePath"), w!("1")) != 0, 0)?;
        c.eq(NeedCurrentDirectoryForExePathW(w!("python.exe")) as u64, 0)?;
        c.ok(NeedCurrentDirectoryForExePathW(w!("x\\python.exe")) != 0, 0)?;
        SetEnvironmentVariableW(w!("NoDefaultCurrentDirectoryInExePath"), core::ptr::null());
    }
    Ok(())
}

fn copy(c: &mut Checks) -> Result<(), u32> {
    let (src, dst) = (w!("kernel32_py_test.src"), w!("kernel32_py_test.dst"));
    // SAFETY: files of ours.
    unsafe {
        let h = CreateFileW(
            src,
            GENERIC_WRITE,
            0,
            core::ptr::null_mut(),
            CREATE_ALWAYS,
            0x80,
            core::ptr::null_mut(),
        );
        c.ok(h != INVALID_HANDLE_VALUE, GetLastError().into())?;
        let mut n = 0;
        c.ok(WriteFile(h, b"gala".as_ptr(), 4, &mut n, core::ptr::null_mut()) != 0, 0)?;
        CloseHandle(h);
        DeleteFileW(dst);
        c.eq(CopyFile2(src, dst, core::ptr::null()) as u32 as u64, 0)?;
        let h = CreateFileW(
            dst,
            GENERIC_READ,
            1,
            core::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            core::ptr::null_mut(),
        );
        c.ok(h != INVALID_HANDLE_VALUE, GetLastError().into())?;
        let mut back = [0u8; 8];
        c.ok(ReadFile(h, back.as_mut_ptr(), 8, &mut n, core::ptr::null_mut()) != 0, 0)?;
        CloseHandle(h);
        c.ok(n == 4 && &back[..4] == b"gala", n.into())?;
        // COPYFILE2_EXTENDED_PARAMETERS { dwSize, dwCopyFlags, pfCancel,
        // pProgressRoutine, pvCallbackContext }: COPY_FILE_FAIL_IF_EXISTS.
        let mut params = [0u8; 8 + 3 * size_of::<usize>()];
        let size = params.len() as u32;
        params[0..4].copy_from_slice(&size.to_le_bytes());
        params[4..8].copy_from_slice(&1u32.to_le_bytes());
        c.eq(CopyFile2(src, dst, params.as_ptr()) as u32 as u64, HR_FILE_EXISTS.into())?;
        DeleteFileW(src);
        DeleteFileW(dst);
    }
    Ok(())
}

fn timer_and_pipe(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: handles of ours.
    unsafe {
        let t = CreateWaitableTimerW(core::ptr::null_mut(), 1, core::ptr::null());
        c.ok(!t.is_null(), GetLastError().into())?;
        let due: i64 = -100_000; // 10 ms, relative
        let set = SetWaitableTimerEx(
            t,
            &due,
            0,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            0,
        );
        c.ok(set != 0, GetLastError().into())?;
        c.eq(WaitForSingleObject(t, 10_000).into(), 0)?;
        CloseHandle(t);

        let name = w!("\\\\.\\pipe\\gala-kernel32-py-test");
        let mode = PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE;
        let p =
            CreateNamedPipeW(name, PIPE_ACCESS_DUPLEX, mode, 4, 512, 512, 0, core::ptr::null_mut());
        c.ok(p != INVALID_HANDLE_VALUE, GetLastError().into())?;
        let (mut state, mut instances) = (0xffu32, 0u32);
        let null: *mut u32 = core::ptr::null_mut();
        let no_user: *mut u16 = core::ptr::null_mut();
        c.ok(
            GetNamedPipeHandleStateW(p, &mut state, &mut instances, null, null, no_user, 0) != 0,
            GetLastError().into(),
        )?;
        c.eq(state.into(), PIPE_READMODE_MESSAGE.into())?;
        c.eq(instances.into(), 1)?;
        let q =
            CreateNamedPipeW(name, PIPE_ACCESS_DUPLEX, mode, 4, 512, 512, 0, core::ptr::null_mut());
        c.ok(q != INVALID_HANDLE_VALUE, GetLastError().into())?;
        c.ok(
            GetNamedPipeHandleStateW(p, null, &mut instances, null, null, no_user, 0) != 0,
            GetLastError().into(),
        )?;
        c.eq(instances.into(), 2)?;
        CloseHandle(q);
        CloseHandle(p);
    }
    Ok(())
}

fn snapshots(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: a snapshot of this process; buffers of ours.
    unsafe {
        // PROCESS_BASIC_INFORMATION (x64): UniqueProcessId at 32,
        // InheritedFromUniqueProcessId at 40.
        let mut basic = [0u8; 48];
        c.eq(
            NtQueryInformationProcess(
                GetCurrentProcess(),
                0,
                basic.as_mut_ptr(),
                48,
                core::ptr::null_mut(),
            ) as u32 as u64,
            0,
        )?;
        let parent = u64::from_le_bytes(basic[40..48].try_into().unwrap_or([0; 8]));
        let mut snapshot: Handle = core::ptr::null_mut();
        c.eq(PssCaptureSnapshot(GetCurrentProcess(), 0, 0, &mut snapshot).into(), 0)?;
        c.ok(!snapshot.is_null(), 0)?;
        let mut info = [0u8; PSS_PROCESS_INFORMATION_SIZE];
        c.eq(
            PssQuerySnapshot(snapshot, PSS_QUERY_PROCESS_INFORMATION, info.as_mut_ptr(), 703)
                .into(),
            ERROR_BAD_LENGTH.into(),
        )?;
        let size = PSS_PROCESS_INFORMATION_SIZE as u32;
        c.eq(
            PssQuerySnapshot(snapshot, PSS_QUERY_PROCESS_INFORMATION, info.as_mut_ptr(), size)
                .into(),
            0,
        )?;
        let at = |o: usize| u32::from_le_bytes(info[o..o + 4].try_into().unwrap_or([0; 4]));
        c.eq(at(28).into(), GetCurrentProcessId().into())?;
        c.eq(at(32).into(), parent)?;
        c.eq(PssFreeSnapshot(GetCurrentProcess(), snapshot).into(), 0)?;
    }
    Ok(())
}

fn versions(c: &mut Checks) -> Result<(), u32> {
    const VER_MAJORVERSION: u32 = 2;
    const VER_GREATER_EQUAL: u8 = 3;
    const ERROR_OLD_WIN_VERSION: u32 = 1150;
    // OSVERSIONINFOEXA: dwOSVersionInfoSize, major, minor, build, platform,
    // szCSDVersion[128], service pack major and minor, suite mask, product
    // type, reserved: 156 bytes.
    let mut info = [0u8; 156];
    info[0..4].copy_from_slice(&156u32.to_le_bytes());
    // SAFETY: a structure of ours.
    unsafe {
        let mask = VerSetConditionMask(0, VER_MAJORVERSION, VER_GREATER_EQUAL);
        info[4..8].copy_from_slice(&6u32.to_le_bytes());
        c.ok(
            VerifyVersionInfoA(info.as_mut_ptr(), VER_MAJORVERSION, mask) != 0,
            GetLastError().into(),
        )?;
        info[4..8].copy_from_slice(&99u32.to_le_bytes());
        c.eq(VerifyVersionInfoA(info.as_mut_ptr(), VER_MAJORVERSION, mask) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_OLD_WIN_VERSION.into())?;
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    paths(&mut c)?;
    copy(&mut c)?;
    timer_and_pipe(&mut c)?;
    snapshots(&mut c)?;
    versions(&mut c)
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"kernel32_py_test: ok\r\n");
            0
        }
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
