//! A program with a compatibility manifest declaring Windows 10 (and 11)
//! support, {8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}: Windows reports its
//! real version to it - GetVersionExW/A 10.0 and the build (26100 on the
//! pinned build), GetVersion the same packed, VerifyVersionInfoW true for
//! major version 10. (Programs without the manifest get 6.2, build 9200:
//! kernel32_test.) An activation context made from the manifest activates
//! and deactivates (CreateActCtxW, ActivateActCtx, GetCurrentActCtx,
//! DeactivateActCtx, ReleaseActCtx). Prints `manifest_test: ok` or exits with the number of
//! the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn GetVersionExW(info: *mut u32) -> Bool;
    fn GetVersionExA(info: *mut u32) -> Bool;
    fn GetVersion() -> u32;
    fn VerSetConditionMask(mask: u64, kind: u32, condition: u8) -> u64;
    fn VerifyVersionInfoW(info: *mut u32, kind: u32, mask: u64) -> Bool;
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn CreateActCtxW(ctx: *const ActCtx) -> Handle;
    fn ActivateActCtx(ctx: Handle, cookie: *mut usize) -> Bool;
    fn DeactivateActCtx(flags: u32, cookie: usize) -> Bool;
    fn GetCurrentActCtx(ctx: *mut Handle) -> Bool;
    fn ReleaseActCtx(ctx: Handle);
    fn GetLastError() -> u32;
}

/// ACTCTXW.
#[repr(C)]
struct ActCtx {
    size: u32,
    flags: u32,
    source: *const u16,
    architecture: u16,
    language: u16,
    assembly_directory: *const u16,
    resource_name: *const u16,
    application_name: *const u16,
    module: Handle,
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
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.0 += 1;
        if got == want {
            return Ok(());
        }
        out(b"manifest_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0)
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: OSVERSIONINFOEXW (284 bytes) and OSVERSIONINFOA (148 bytes)
    // buffers with their sizes set.
    unsafe {
        let mut w = [0u32; 71];
        w[0] = 284;
        c.eq(GetVersionExW(w.as_mut_ptr()) as u64, 1)?;
        c.eq(w[1].into(), 10)?; // major
        c.eq(w[2].into(), 0)?; // minor
        c.eq(w[3].into(), 26100)?; // build
        c.eq(w[4].into(), 2)?; // VER_PLATFORM_WIN32_NT
        let mut a = [0u32; 37];
        a[0] = 148;
        c.eq(GetVersionExA(a.as_mut_ptr()) as u64, 1)?;
        c.eq(u64::from(a[1]) << 32 | u64::from(a[2]), 10 << 32)?;
        c.eq(a[3].into(), 26100)?;
        // GetVersion: build << 16 | minor << 8 | major.
        c.eq(GetVersion().into(), (26100 << 16) | 10)?;
        let mut v = [0u32; 71];
        v[0] = 284;
        v[1] = 10;
        let mask = VerSetConditionMask(0, 2, 3); // VER_MAJORVERSION, >=
        c.eq(VerifyVersionInfoW(v.as_mut_ptr(), 2, mask) as u64, 1)?;

        // An activation context from this program's manifest (resource 1),
        // activated and deactivated; resource 2 does not exist.
        // ACTCTX_FLAG_RESOURCE_NAME_VALID | ACTCTX_FLAG_HMODULE_VALID.
        let mut ctx = ActCtx {
            size: core::mem::size_of::<ActCtx>() as u32,
            flags: 0x8 | 0x80,
            source: core::ptr::null(),
            architecture: 0,
            language: 0,
            assembly_directory: core::ptr::null(),
            resource_name: core::ptr::without_provenance(1),
            application_name: core::ptr::null(),
            module: GetModuleHandleW(core::ptr::null()),
        };
        let invalid = usize::MAX;
        let h = CreateActCtxW(&ctx);
        c.eq(u64::from(h as usize != invalid), 1)?;
        let mut cookie = 0usize;
        c.eq(ActivateActCtx(h, &mut cookie) as u64, 1)?;
        c.eq(u64::from(cookie != 0), 1)?;
        let mut current = core::ptr::null_mut();
        c.eq(GetCurrentActCtx(&mut current) as u64, 1)?;
        c.eq(current as u64, h as u64)?;
        ReleaseActCtx(current); // GetCurrentActCtx took a reference
        c.eq(DeactivateActCtx(0, cookie) as u64, 1)?;
        ReleaseActCtx(h);
        ctx.resource_name = core::ptr::without_provenance(2);
        c.eq(CreateActCtxW(&ctx) as u64, invalid as u64)?;
        c.eq(GetLastError().into(), 1814)?; // ERROR_RESOURCE_NAME_NOT_FOUND
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"manifest_test: ok\r\n");
            0
        }
        Err(id) => id,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
