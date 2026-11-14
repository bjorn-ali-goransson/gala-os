//! What the offline registry library (OffReg.dll, which the Click-to-Run
//! service loads to stage Office's registry) imports, with the results of
//! Windows:
//!
//! - One-time initialization, through kernel32's InitOnce* (which call
//!   ntdll's RtlRunOnce*): the sequence of Wine's dlls/kernel32/tests
//!   sync.c test_initonce, run on Windows (the word's values, TRUE/FALSE
//!   and the last errors ERROR_GEN_FAILURE and ERROR_INVALID_PARAMETER, a
//!   failed callback leaving the last error alone); ntdll's
//!   RtlRunOnceBeginInitialize and RtlRunOnceComplete as the WDK
//!   documents them (STATUS_PENDING for the caller that initializes,
//!   STATUS_SUCCESS with the context once done), and a second thread's
//!   InitOnceBeginInitialize waiting for an initialization ntdll's
//!   functions began and completed.
//! - SRW locks: ntdll's RtlInitializeSRWLock, RtlAcquireSRWLockExclusive
//!   and RtlReleaseSRWLockExclusive and kernel32's functions are the same
//!   lock (kernel32's forward to ntdll on Windows): a lock taken through
//!   one is held for the other, shared holders exclude an exclusive one,
//!   and a thread waiting in ntdll's acquire is woken by kernel32's
//!   release (from another thread: SRW locks have no owner).
//! - Bitmaps (Wine's dlls/ntdll/tests/rtlbitmap.c, run on Windows):
//!   RtlInitializeBitMap writes only the header; RtlNumberOfSetBits
//!   after RtlSetBits runs crossing bytes and ULONGs; in a 62-bit map
//!   whose last byte alone is clear, RtlFindNextForwardRunClear finds a
//!   run of 6 at 56; RtlFindNextForwardRunSet (Wine's) is not exported
//!   (Windows).
//! - msvcrt: wcsnlen, wcsncpy_s and wcscat_s with valid arguments, as
//!   OffReg calls them (copies and appends that fit, a count shorter
//!   than the source). Their invalid-parameter answers are not checked:
//!   on Windows Server 2025 a NULL source crashed in ntdll (a read of
//!   0) instead of returning EINVAL, where Wine's msvcrt tests (which
//!   load msvcr80 and later) expect EINVAL.
//! - advapi32's private object security, as Microsoft documents
//!   CreatePrivateObjectSecurityWithMultipleInheritance and
//!   SetPrivateObjectSecurityEx: a child of a key-like parent with
//!   SEF_DACL_AUTO_INHERIT is self-relative, auto-inherited, owned by the
//!   token's owner, its DACL the parent's container-inheritable ACE
//!   (INHERITED_ACE) and, for a CREATOR OWNER ACE with generic rights,
//!   an effective ACE for the owner with the rights mapped plus the
//!   inherit-only original; no token where one is needed is
//!   ERROR_NO_TOKEN; SetPrivateObjectSecurityEx of a new explicit ACE
//!   keeps the inherited ones after it; DestroyPrivateObjectSecurity.
//!   MakeSelfRelativeSD (Wine's advapi32 security.c
//!   test_security_descriptor, run on Windows): too small a length is
//!   ERROR_INSUFFICIENT_BUFFER with the size, a self-relative input
//!   ERROR_BAD_DESCRIPTOR_FORMAT, and a larger buffer leaves the length
//!   as it was. GetSidLengthRequired is 8 bytes plus 4 per subauthority
//!   (the SID layout Microsoft documents).

use core::ffi::c_void;
use core::ptr::{null, null_mut};
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use super::{Checks, CloseHandle, CreateThread, GetCurrentProcess, GetLastError, Handle};
use super::{SetLastError, WaitForSingleObject};

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn RtlRunOnceBeginInitialize(once: *mut usize, flags: u32, context: *mut *mut c_void) -> u32;
    fn RtlRunOnceComplete(once: *mut usize, flags: u32, context: *mut c_void) -> u32;
    fn RtlInitializeSRWLock(lock: *mut usize);
    fn RtlAcquireSRWLockExclusive(lock: *mut usize);
    fn RtlReleaseSRWLockExclusive(lock: *mut usize);
    fn RtlInitializeBitMap(map: *mut Bitmap, buffer: *mut u8, size: u32);
    fn RtlSetBits(map: *mut Bitmap, start: u32, count: u32);
    fn RtlNumberOfSetBits(map: *mut Bitmap) -> u32;
    fn RtlFindNextForwardRunClear(map: *mut Bitmap, from: u32, pos: *mut u32) -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn InitOnceInitialize(once: *mut usize);
    fn InitOnceExecuteOnce(
        once: *mut usize,
        f: extern "system" fn(*mut usize, *mut c_void, *mut *mut c_void) -> i32,
        param: *mut c_void,
        context: *mut *mut c_void,
    ) -> i32;
    fn InitOnceBeginInitialize(
        once: *mut usize,
        flags: u32,
        pending: *mut i32,
        context: *mut *mut c_void,
    ) -> i32;
    fn InitOnceComplete(once: *mut usize, flags: u32, context: *mut c_void) -> i32;
    fn InitializeSRWLock(lock: *mut usize);
    fn AcquireSRWLockExclusive(lock: *mut usize);
    fn ReleaseSRWLockExclusive(lock: *mut usize);
    fn TryAcquireSRWLockExclusive(lock: *mut usize) -> u8;
    fn TryAcquireSRWLockShared(lock: *mut usize) -> u8;
    fn ReleaseSRWLockShared(lock: *mut usize);
    fn Sleep(ms: u32);
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "msvcrt", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "msvcrt", kind = "raw-dylib"))]
unsafe extern "C" {
    fn wcsnlen(s: *const u16, max: usize) -> usize;
    fn wcsncpy_s(dst: *mut u16, size: usize, src: *const u16, count: usize) -> i32;
    fn wcscat_s(dst: *mut u16, size: usize, src: *const u16) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "advapi32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "advapi32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn InitializeSecurityDescriptor(sd: *mut u8, revision: u32) -> i32;
    fn SetSecurityDescriptorDacl(sd: *mut u8, present: i32, acl: *mut u8, defaulted: i32) -> i32;
    fn SetSecurityDescriptorOwner(sd: *mut u8, owner: *mut u8, defaulted: i32) -> i32;
    fn SetSecurityDescriptorGroup(sd: *mut u8, group: *mut u8, defaulted: i32) -> i32;
    fn GetSecurityDescriptorDacl(
        sd: *const u8,
        present: *mut i32,
        acl: *mut *mut u8,
        defaulted: *mut i32,
    ) -> i32;
    fn GetSecurityDescriptorOwner(sd: *const u8, owner: *mut *mut u8, defaulted: *mut i32) -> i32;
    fn GetSecurityDescriptorControl(sd: *const u8, control: *mut u16, revision: *mut u32) -> i32;
    fn IsValidSecurityDescriptor(sd: *const u8) -> i32;
    fn InitializeAcl(acl: *mut u8, size: u32, revision: u32) -> i32;
    fn AddAccessAllowedAceEx(
        acl: *mut u8,
        revision: u32,
        flags: u32,
        mask: u32,
        sid: *const u8,
    ) -> i32;
    fn GetAce(acl: *const u8, index: u32, ace: *mut *mut u8) -> i32;
    fn EqualSid(a: *const u8, b: *const u8) -> i32;
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
    fn GetTokenInformation(
        token: Handle,
        class: u32,
        buffer: *mut u8,
        length: u32,
        returned: *mut u32,
    ) -> i32;
    fn CreatePrivateObjectSecurityWithMultipleInheritance(
        parent: *const u8,
        creator: *const u8,
        new: *mut *mut u8,
        types: *const *const c_void,
        count: u32,
        container: i32,
        flags: u32,
        token: Handle,
        mapping: *const u32,
    ) -> i32;
    fn SetPrivateObjectSecurityEx(
        info: u32,
        modification: *const u8,
        object: *mut *mut u8,
        flags: u32,
        mapping: *const u32,
        token: Handle,
    ) -> i32;
    fn DestroyPrivateObjectSecurity(sd: *mut *mut u8) -> i32;
    fn MakeSelfRelativeSD(abs: *const u8, rel: *mut u8, len: *mut u32) -> i32;
    fn GetSidLengthRequired(count: u8) -> u32;
}

const ERROR_GEN_FAILURE: u32 = 31;
const ERROR_INVALID_PARAMETER: u32 = 87;
const INIT_ONCE_CHECK_ONLY: u32 = 1;
const INIT_ONCE_ASYNC: u32 = 2;
const INIT_ONCE_INIT_FAILED: u32 = 4;
const STATUS_PENDING: u32 = 0x103;

static CALLED: AtomicU32 = AtomicU32::new(0);
static CALLBACK_RESULT: AtomicU32 = AtomicU32::new(1);

extern "system" fn callback(_once: *mut usize, param: *mut c_void, _ctx: *mut *mut c_void) -> i32 {
    CALLED.fetch_add(1, Ordering::Relaxed);
    if param as usize != 0xdead_beef {
        return 0;
    }
    CALLBACK_RESULT.load(Ordering::Relaxed) as i32
}

/// What the second thread of `run_once` saw: pending, context.
static WAITER_SAW: [AtomicUsize; 2] = [AtomicUsize::new(9), AtomicUsize::new(0)];

extern "system" fn begin_waiter(once: usize) -> u32 {
    let (mut pending, mut ctx) = (9i32, null_mut());
    // SAFETY: the main thread's once, live until this thread is joined.
    let ok = unsafe { InitOnceBeginInitialize(once as *mut usize, 0, &mut pending, &mut ctx) };
    WAITER_SAW[0].store(pending as usize, Ordering::Relaxed);
    WAITER_SAW[1].store(ctx as usize, Ordering::Relaxed);
    ok as u32
}

fn run_once(c: &mut Checks) -> Result<(), u32> {
    let mut once = 0xdead_beefusize;
    let o = &raw mut once;
    let mut ctx: *mut c_void = null_mut();
    let mut pending = 0i32;
    // SAFETY: our own once word, context and outputs throughout.
    unsafe {
        InitOnceInitialize(o);
        c.eq(once as u64, 0)?;
        // A callback that succeeds runs once.
        c.eq(InitOnceExecuteOnce(o, callback, 0xdead_beef as *mut c_void, &mut ctx) as u64, 1)?;
        c.ok(once == 2 && ctx.is_null() && CALLED.load(Ordering::Relaxed) == 1, once as u64)?;
        c.eq(InitOnceExecuteOnce(o, callback, 0xdead_beef as *mut c_void, &mut ctx) as u64, 1)?;
        c.eq(CALLED.load(Ordering::Relaxed).into(), 1)?;
        // The context is kept with the low two bits as the state.
        InitOnceInitialize(o);
        ctx = 0xffff_fff0usize as *mut c_void;
        c.eq(InitOnceExecuteOnce(o, callback, 0xdead_beef as *mut c_void, &mut ctx) as u64, 1)?;
        c.ok(once == 0xffff_fff2 && ctx as usize == 0xffff_fff0, once as u64)?;
        // A failing callback: FALSE, the last error left alone.
        CALLBACK_RESULT.store(0, Ordering::Relaxed);
        InitOnceInitialize(o);
        ctx = null_mut();
        SetLastError(0xdead_beef);
        let ok = InitOnceExecuteOnce(o, callback, 0xdead_beef as *mut c_void, &mut ctx);
        c.ok(ok == 0 && GetLastError() == 0xdead_beef, GetLastError().into())?;
        c.eq(once as u64, 0)?;
        // Begin and complete.
        InitOnceInitialize(o);
        c.eq(InitOnceBeginInitialize(o, 0, &mut pending, &mut ctx) as u64, 1)?;
        c.ok(pending == 1 && once == 1, once as u64)?;
        pending = 0xf;
        let ok = InitOnceBeginInitialize(o, INIT_ONCE_CHECK_ONLY, &mut pending, &mut ctx);
        c.fails(ok != 0, ERROR_GEN_FAILURE)?;
        c.ok(pending == 0xf && once == 1, once as u64)?;
        let ok = InitOnceComplete(o, INIT_ONCE_INIT_FAILED, 0xdead_bee0usize as *mut c_void);
        c.fails(ok != 0, ERROR_INVALID_PARAMETER)?;
        c.eq(InitOnceComplete(o, 0, 0xdead_bee0usize as *mut c_void) as u64, 1)?;
        c.eq(once as u64, 0xdead_bee2)?;
        InitOnceInitialize(o);
        c.fails(InitOnceComplete(o, INIT_ONCE_INIT_FAILED, null_mut()) != 0, ERROR_GEN_FAILURE)?;
        let ok = InitOnceComplete(o, INIT_ONCE_INIT_FAILED | INIT_ONCE_ASYNC, null_mut());
        c.fails(ok != 0, ERROR_INVALID_PARAMETER)?;
        c.eq(InitOnceBeginInitialize(o, 0, &mut pending, &mut ctx) as u64, 1)?;
        let ok = InitOnceBeginInitialize(o, INIT_ONCE_ASYNC, &mut pending, &mut ctx);
        c.fails(ok != 0, ERROR_INVALID_PARAMETER)?;
        c.fails(InitOnceComplete(o, 0, 0xdead_beef as *mut c_void) != 0, ERROR_INVALID_PARAMETER)?;
        c.eq(InitOnceComplete(o, INIT_ONCE_INIT_FAILED, null_mut()) as u64, 1)?;
        c.eq(once as u64, 0)?;
        // Asynchronous: every caller is pending until one completes.
        c.eq(InitOnceBeginInitialize(o, INIT_ONCE_ASYNC, &mut pending, &mut ctx) as u64, 1)?;
        c.ok(pending == 1 && once == 3, once as u64)?;
        c.fails(
            InitOnceBeginInitialize(o, 0, &mut pending, &mut ctx) != 0,
            ERROR_INVALID_PARAMETER,
        )?;
        c.eq(InitOnceBeginInitialize(o, INIT_ONCE_ASYNC, &mut pending, &mut ctx) as u64, 1)?;
        c.fails(
            InitOnceComplete(o, INIT_ONCE_INIT_FAILED, null_mut()) != 0,
            ERROR_INVALID_PARAMETER,
        )?;
        let ok = InitOnceComplete(o, INIT_ONCE_ASYNC, 0xdead_beef as *mut c_void);
        c.fails(ok != 0, ERROR_INVALID_PARAMETER)?;
        c.eq(InitOnceComplete(o, INIT_ONCE_ASYNC, 0xdead_bee0usize as *mut c_void) as u64, 1)?;
        c.eq(once as u64, 0xdead_bee2)?;
        // CHECK_ONLY.
        InitOnceInitialize(o);
        let ok = InitOnceBeginInitialize(o, INIT_ONCE_CHECK_ONLY, &mut pending, &mut ctx);
        c.fails(ok != 0, ERROR_GEN_FAILURE)?;
        let ok = InitOnceBeginInitialize(
            o,
            INIT_ONCE_CHECK_ONLY | INIT_ONCE_ASYNC,
            &mut pending,
            &mut ctx,
        );
        c.fails(ok != 0, ERROR_INVALID_PARAMETER)?;
        c.eq(InitOnceBeginInitialize(o, 0, &mut pending, &mut ctx) as u64, 1)?;
        c.eq(InitOnceComplete(o, 0, 0xdead_bee0usize as *mut c_void) as u64, 1)?;
        c.eq(InitOnceBeginInitialize(o, INIT_ONCE_CHECK_ONLY, &mut pending, &mut ctx) as u64, 1)?;
        c.ok(pending == 0 && ctx as usize == 0xdead_bee0, ctx as u64)?;

        // ntdll's, and a second thread waiting for them.
        InitOnceInitialize(o);
        c.eq(RtlRunOnceBeginInitialize(o, 0, &mut ctx).into(), STATUS_PENDING.into())?;
        let thread = CreateThread(0, 0, begin_waiter, o as usize, 0, null_mut());
        c.ok(thread != 0, 0)?;
        c.eq(WaitForSingleObject(thread, 100).into(), 0x102)?; // still waiting
        c.eq(RtlRunOnceComplete(o, 0, 0x1230usize as *mut c_void).into(), 0)?;
        c.eq(WaitForSingleObject(thread, 10_000).into(), 0)?;
        CloseHandle(thread);
        c.eq(WAITER_SAW[0].load(Ordering::Relaxed) as u64, 0)?;
        c.eq(WAITER_SAW[1].load(Ordering::Relaxed) as u64, 0x1230)?;
        ctx = null_mut();
        c.eq(RtlRunOnceBeginInitialize(o, 0, &mut ctx).into(), 0)?;
        c.eq(ctx as u64, 0x1230)?;
    }
    Ok(())
}

static SRW_TAKEN: AtomicU32 = AtomicU32::new(0);

extern "system" fn srw_waiter(lock: usize) -> u32 {
    // SAFETY: the main thread's lock, live until this thread is joined.
    unsafe {
        RtlAcquireSRWLockExclusive(lock as *mut usize);
        SRW_TAKEN.store(1, Ordering::Release);
        RtlReleaseSRWLockExclusive(lock as *mut usize);
    }
    0
}

fn srw_locks(c: &mut Checks) -> Result<(), u32> {
    let mut lock = 0x55usize;
    let l = &raw mut lock;
    // SAFETY: our own lock word throughout.
    unsafe {
        RtlInitializeSRWLock(l);
        c.eq(lock as u64, 0)?;
        lock = 0x55;
        InitializeSRWLock(l);
        c.eq(lock as u64, 0)?;
        RtlAcquireSRWLockExclusive(l);
        c.eq(TryAcquireSRWLockExclusive(l).into(), 0)?;
        c.eq(TryAcquireSRWLockShared(l).into(), 0)?;
        ReleaseSRWLockExclusive(l);
        c.eq(TryAcquireSRWLockShared(l).into(), 1)?;
        c.eq(TryAcquireSRWLockShared(l).into(), 1)?;
        c.eq(TryAcquireSRWLockExclusive(l).into(), 0)?;
        ReleaseSRWLockShared(l);
        ReleaseSRWLockShared(l);
        c.eq(TryAcquireSRWLockExclusive(l).into(), 1)?;
        RtlReleaseSRWLockExclusive(l);
        // A waiter in ntdll's acquire, released through kernel32.
        AcquireSRWLockExclusive(l);
        let thread = CreateThread(0, 0, srw_waiter, l as usize, 0, null_mut());
        c.ok(thread != 0, 0)?;
        Sleep(100);
        c.eq(SRW_TAKEN.load(Ordering::Acquire).into(), 0)?;
        ReleaseSRWLockExclusive(l);
        c.eq(WaitForSingleObject(thread, 10_000).into(), 0)?;
        CloseHandle(thread);
        c.eq(SRW_TAKEN.load(Ordering::Acquire).into(), 1)?;
        c.eq(lock as u64, 0)?;
    }
    Ok(())
}

/// RTL_BITMAP.
#[repr(C)]
struct Bitmap {
    size: u32,
    buffer: *mut u8,
}

fn bitmaps(c: &mut Checks) -> Result<(), u32> {
    let mut buff = [0u8; 256];
    let mut bm = Bitmap { size: 0, buffer: null_mut() };
    let b = &raw mut bm;
    // SAFETY: the header and its buffer live through the calls.
    unsafe {
        buff[0] = 77;
        buff[79] = 77;
        RtlInitializeBitMap(b, buff.as_mut_ptr(), 800);
        c.ok(bm.size == 800 && bm.buffer == buff.as_mut_ptr(), bm.size.into())?;
        c.ok(buff[0] == 77 && buff[79] == 77, buff[0].into())?;
        buff = [0; 256];
        RtlInitializeBitMap(b, buff.as_mut_ptr(), 256 * 8);
        c.eq(RtlNumberOfSetBits(b).into(), 0)?;
        RtlSetBits(b, 0, 1);
        c.eq(RtlNumberOfSetBits(b).into(), 1)?;
        RtlSetBits(b, 7, 8);
        c.eq(RtlNumberOfSetBits(b).into(), 9)?;
        RtlSetBits(b, 17, 33);
        c.eq(RtlNumberOfSetBits(b).into(), 42)?;
        RtlSetBits(b, 256 * 8 - 1, 1);
        c.eq(RtlNumberOfSetBits(b).into(), 43)?;
        let mut mask = [0xffu8, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00];
        let mut pos = 0u32;
        RtlInitializeBitMap(b, mask.as_mut_ptr(), 62);
        c.eq(RtlFindNextForwardRunClear(b, 0, &mut pos).into(), 6)?;
        c.eq(pos.into(), 56)?;
        // RtlFindNextForwardRunSet: Wine's, not Windows' (Windows' ntdll
        // does not export it; imported statically, the test would not
        // load there).
        let ntdll: [u16; 10] = super::w("ntdll.dll");
        let f = super::GetProcAddress(
            super::LoadLibraryW(ntdll.as_ptr()),
            b"RtlFindNextForwardRunSet\0".as_ptr(),
        );
        c.eq(f as u64, 0)?;
    }
    Ok(())
}

fn safe_strings(c: &mut Checks) -> Result<(), u32> {
    let long: [u16; 18] = super::w("ThisIsAVeryLongWo");
    let mut dest = [0u16; 18];
    // SAFETY: our buffers of the sizes given.
    unsafe {
        c.eq(wcsnlen(long.as_ptr(), 4) as u64, 4)?;
        c.eq(wcsnlen(long.as_ptr(), 100) as u64, 17)?;
        c.eq(wcsncpy_s(dest.as_mut_ptr(), 18, long.as_ptr(), 18) as u64, 0)?;
        c.ok(dest == long, dest[0].into())?;
        c.eq(wcsncpy_s(dest.as_mut_ptr(), 18, long.as_ptr(), 4) as u64, 0)?;
        c.ok(dest[4] == 0 && dest[..4] == long[..4], dest[4].into())?;
        // wcscat_s.
        let mut cat: [u16; 10] = super::w("ab");
        let cd: [u16; 3] = super::w("cd");
        c.eq(wcscat_s(cat.as_mut_ptr(), 10, cd.as_ptr()) as u64, 0)?;
        let want: [u16; 10] = super::w("abcd");
        c.ok(cat == want, cat[2].into())?;
    }
    Ok(())
}

// Security descriptors: SECURITY_DESCRIPTOR (absolute) is 20 bytes on
// x86, 40 on x64.
const SD_SIZE: usize = 40;
const KEY_MAPPING: [u32; 4] = [0x0002_0019, 0x0002_0006, 0x0002_0019, 0x000f_003f];
const SEF_DACL_AUTO_INHERIT: u32 = 1;
const CI: u32 = 2;
const IO: u32 = 8;
const ID: u32 = 0x10;
const DACL_SECURITY_INFORMATION: u32 = 4;
const TOKEN_QUERY: u32 = 8;
const ERROR_NO_TOKEN: u32 = 1008;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_BAD_DESCRIPTOR_FORMAT: u32 = 1361;

/// S-1-1-0 and S-1-3-0.
const EVERYONE: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0];
const CREATOR_OWNER: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0];

/// ACE `i` of `acl`: (flags, mask, SID pointer).
fn ace(acl: *const u8, i: u32) -> (u32, u32, *const u8) {
    let mut a = null_mut();
    // SAFETY: a valid ACL; GetAce fails for a missing index.
    unsafe {
        if GetAce(acl, i, &mut a) == 0 {
            return (0xffff, 0, null());
        }
        (u32::from(*a.add(1)), a.add(4).cast::<u32>().read_unaligned(), a.add(8))
    }
}

fn dacl_of(sd: *const u8) -> (*mut u8, u16) {
    let (mut present, mut acl, mut defaulted) = (0, null_mut(), 0);
    // SAFETY: a valid descriptor; the ACL's header.
    unsafe {
        GetSecurityDescriptorDacl(sd, &mut present, &mut acl, &mut defaulted);
        let count = if acl.is_null() { 0 } else { acl.add(4).cast::<u16>().read_unaligned() };
        (acl, count)
    }
}

fn private_security(c: &mut Checks) -> Result<(), u32> {
    let mut parent = [0usize; SD_SIZE / 8 + 1];
    let p = parent.as_mut_ptr().cast::<u8>();
    let mut acl = [0u32; 32];
    let a = acl.as_mut_ptr().cast::<u8>();
    let mut token: Handle = 0;
    let mut owner_buf = [0usize; 16];
    // SAFETY: our own descriptor, ACL, token and buffers throughout.
    unsafe {
        c.eq(GetSidLengthRequired(0).into(), 8)?;
        c.eq(GetSidLengthRequired(2).into(), 16)?;
        c.eq(GetSidLengthRequired(15).into(), 68)?;
        c.eq(OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) as u64, 1)?;
        let mut n = 0u32;
        // TokenOwner (4): a TOKEN_OWNER, the SID's pointer first.
        c.eq(GetTokenInformation(token, 4, owner_buf.as_mut_ptr().cast(), 128, &mut n) as u64, 1)?;
        let token_owner = owner_buf[0] as *const u8;
        c.eq(InitializeSecurityDescriptor(p, 1) as u64, 1)?;
        c.eq(InitializeAcl(a, 128, 2) as u64, 1)?;
        c.eq(AddAccessAllowedAceEx(a, 2, CI, 0x0002_0019, EVERYONE.as_ptr()) as u64, 1)?;
        c.eq(AddAccessAllowedAceEx(a, 2, CI | IO, 0x1000_0000, CREATOR_OWNER.as_ptr()) as u64, 1)?;
        c.eq(SetSecurityDescriptorDacl(p, 1, a, 0) as u64, 1)?;
        let mut owner = EVERYONE;
        c.eq(SetSecurityDescriptorOwner(p, owner.as_mut_ptr(), 0) as u64, 1)?;
        c.eq(SetSecurityDescriptorGroup(p, owner.as_mut_ptr(), 0) as u64, 1)?;
        let mapping = KEY_MAPPING.as_ptr();

        let mut new: *mut u8 = null_mut();
        let ok = CreatePrivateObjectSecurityWithMultipleInheritance(
            p,
            null(),
            &mut new,
            null(),
            0,
            1,
            SEF_DACL_AUTO_INHERIT,
            token,
            mapping,
        );
        c.ok(ok != 0 && !new.is_null(), GetLastError().into())?;
        c.eq(IsValidSecurityDescriptor(new) as u64, 1)?;
        let (mut control, mut revision) = (0u16, 0u32);
        GetSecurityDescriptorControl(new, &mut control, &mut revision);
        // SE_SELF_RELATIVE, SE_DACL_AUTO_INHERITED, SE_DACL_PRESENT.
        c.eq(u64::from(control & 0x8404), 0x8404)?;
        let (mut o, mut defaulted) = (null_mut(), 0);
        GetSecurityDescriptorOwner(new, &mut o, &mut defaulted);
        c.eq(EqualSid(o, token_owner) as u64, 1)?;
        let (dacl, count) = dacl_of(new);
        c.eq(count.into(), 3)?;
        let mut seen = 0u64;
        for i in 0..3 {
            let (flags, mask, sid) = ace(dacl, i);
            if flags == CI | ID && mask == 0x0002_0019 && EqualSid(sid, EVERYONE.as_ptr()) != 0 {
                seen |= 1;
            } else if flags == ID && mask == 0x000f_003f && EqualSid(sid, token_owner) != 0 {
                seen |= 2;
            } else if flags == CI | IO | ID
                && mask == 0x1000_0000
                && EqualSid(sid, CREATOR_OWNER.as_ptr()) != 0
            {
                seen |= 4;
            }
        }
        c.eq(seen, 7)?;
        // No token where the owner must come from one.
        let mut none: *mut u8 = null_mut();
        let ok = CreatePrivateObjectSecurityWithMultipleInheritance(
            p,
            null(),
            &mut none,
            null(),
            0,
            1,
            SEF_DACL_AUTO_INHERIT,
            0,
            mapping,
        );
        c.fails(ok != 0, ERROR_NO_TOKEN)?;

        // A new explicit ACE; the inherited ones stay, after it.
        let mut m = [0usize; SD_SIZE / 8 + 1];
        let mp = m.as_mut_ptr().cast::<u8>();
        let mut macl = [0u32; 16];
        let ma = macl.as_mut_ptr().cast::<u8>();
        InitializeSecurityDescriptor(mp, 1);
        InitializeAcl(ma, 64, 2);
        AddAccessAllowedAceEx(ma, 2, 0, 0x000f_003f, EVERYONE.as_ptr());
        SetSecurityDescriptorDacl(mp, 1, ma, 0);
        let ok = SetPrivateObjectSecurityEx(
            DACL_SECURITY_INFORMATION,
            mp,
            &mut new,
            SEF_DACL_AUTO_INHERIT,
            mapping,
            token,
        );
        c.ok(ok != 0, GetLastError().into())?;
        let (dacl, count) = dacl_of(new);
        c.eq(count.into(), 4)?;
        let (flags, mask, sid) = ace(dacl, 0);
        c.ok(
            flags == 0 && mask == 0x000f_003f && EqualSid(sid, EVERYONE.as_ptr()) != 0,
            mask.into(),
        )?;
        for i in 1..4 {
            c.ok(ace(dacl, i).0 & ID != 0, i.into())?;
        }
        c.eq(DestroyPrivateObjectSecurity(&mut new) as u64, 1)?;
        CloseHandle(token);

        // MakeSelfRelativeSD.
        let mut sd = [0usize; SD_SIZE / 8 + 1];
        let s = sd.as_mut_ptr().cast::<u8>();
        InitializeSecurityDescriptor(s, 1);
        *s.add(2) |= 0x14; // SE_DACL_PRESENT | SE_SACL_PRESENT
        let mut buf = [0u8; 256];
        let mut size = 5u32;
        SetLastError(0xdead_beef);
        c.fails(
            MakeSelfRelativeSD(s, buf.as_mut_ptr(), &mut size) != 0,
            ERROR_INSUFFICIENT_BUFFER,
        )?;
        c.ok(size > 5 && size <= 256, size.into())?;
        c.eq(MakeSelfRelativeSD(s, buf.as_mut_ptr(), &mut size) as u64, 1)?;
        let mut size = 0u32;
        c.fails(
            MakeSelfRelativeSD(buf.as_ptr(), null_mut(), &mut size) != 0,
            ERROR_BAD_DESCRIPTOR_FORMAT,
        )?;
        let mut size = 0u32;
        SetSecurityDescriptorOwner(s, owner.as_mut_ptr(), 0);
        c.fails(MakeSelfRelativeSD(s, null_mut(), &mut size) != 0, ERROR_INSUFFICIENT_BUFFER)?;
        let need = size;
        size += 4;
        c.eq(MakeSelfRelativeSD(s, buf.as_mut_ptr(), &mut size) as u64, 1)?;
        c.eq(size.into(), u64::from(need) + 4)?;
    }
    Ok(())
}

/// The checks of this module, in the order OffReg.dll's load needs them.
pub fn offreg_imports(c: &mut Checks) -> Result<(), u32> {
    // Each part announced, so a crash on Windows names the part it was in.
    super::info("part run_once", 0);
    run_once(c)?;
    super::info("part srw_locks", 0);
    srw_locks(c)?;
    super::info("part bitmaps", 0);
    bitmaps(c)?;
    super::info("part safe_strings", 0);
    safe_strings(c)?;
    super::info("part private_security", 0);
    private_security(c)
}
