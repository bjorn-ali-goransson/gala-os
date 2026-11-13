//! The program the hot swap scenarios snapshot and patch (ROADMAP SN step
//! 1d; scenarios `hotswap_machine` and `hotswap_stack`; WORKFLOW "Hot
//! swapping DLLs into a snapshot (at your own risk)"), on hotswap_dll.dll.
//!
//! Without arguments: makes a COM-style object (`hs_create`), sets the
//! DLL's static (`hs_set_global(42)`), prints its values (`hotswap_test:
//! before: ...`) and `hotswap_test: marker` (where xtask saves the
//! machine), waits two seconds, then calls the DLL again and prints
//! `hotswap_test: after: version V, calls C, object O, global G,
//! callback B`. With the first build that is `version 1, calls 0, object
//! 7, global 42, callback 11` (on Windows too); with the second swapped in
//! at the snapshot, `version 2, calls 2, object 107, global 1042,
//! callback 12`: new code, a new static, the old object's vtable reaching
//! new code, the old static's value kept.
//!
//! With `stack`: the snapshot is taken inside a callback the DLL's
//! `hs_call_back` made (`hotswap_test: marker` from the callback, which
//! then waits): that changed function is on the stack, so the swap is
//! refused, and the program goes on with the first build (`hotswap_test:
//! callback 11`).
//!
//! Prints `hotswap_test: ok` and exits with 0 (the scenarios judge the
//! values). Built for x86 too (`wow64_hotswap_test`, on hotswap_dll32.dll:
//! hotswap_dll's source built for x86 under its own name): the same lines
//! from a WOW64 process.

#![no_std]
#![no_main]

use ntrt::*;

#[repr(C)]
struct Obj {
    vtbl: *const Vtbl,
    value: u32,
}

#[repr(C)]
struct Vtbl {
    get: extern "system" fn(*const Obj) -> u32,
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "hotswap_dll32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "hotswap_dll", kind = "raw-dylib"))]
unsafe extern "C" {
    fn hs_create(obj: *mut Obj, value: u32);
    fn hs_set_global(value: u64);
    fn hs_get_global() -> u64;
    fn hs_version() -> u32;
    fn hs_calls() -> u64;
    fn hs_call_back(cb: extern "C" fn() -> u32) -> u32;
}

fn print_num(mut n: u64) {
    let mut b = [0u8; 20];
    let mut i = b.len();
    loop {
        i -= 1;
        b[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    print(core::str::from_utf8(&b[i..]).unwrap_or("?"));
}

/// The command line (PEB->ProcessParameters->CommandLine; the 32-bit
/// layout in the WOW64 build).
fn command_line() -> &'static [u16] {
    #[cfg(target_arch = "x86")]
    let (params_at, length_at, buffer_at) = (0x10, 0x40, 0x44);
    #[cfg(not(target_arch = "x86"))]
    let (params_at, length_at, buffer_at) = (0x20, 0x70, 0x78);
    // SAFETY: the loader always provides ProcessParameters and CommandLine.
    unsafe {
        let params = *(peb().add(params_at) as *const *const u8);
        let len = usize::from(*(params.add(length_at) as *const u16)) / 2;
        core::slice::from_raw_parts(*(params.add(buffer_at) as *const *const u16), len)
    }
}

/// NtDelayExecution for `interval` (100 ns units, negative: relative).
#[cfg(target_arch = "x86")]
fn delay(interval: i64) {
    // SAFETY: NtDelayExecution(Alertable FALSE, *Interval), the interval
    // ours.
    unsafe { syscall(nr::NT_DELAY_EXECUTION, &[0, &interval as *const i64 as usize]) };
}

fn wait_two_seconds() {
    // Twenty waits of 100 ms: the machine is saved in one of them.
    for _ in 0..20 {
        delay(-1_000_000);
    }
}

extern "C" fn plain() -> u32 {
    10
}

extern "C" fn inside() -> u32 {
    print("hotswap_test: marker\r\n");
    wait_two_seconds();
    10
}

fn main() -> u32 {
    let key: &[u16] = &wide!("stack");
    if command_line().windows(key.len()).any(|w| w == key) {
        // SAFETY: the DLL's exports as declared.
        let r = unsafe {
            hs_set_global(42);
            hs_call_back(inside)
        };
        print("hotswap_test: callback ");
        print_num(r.into());
        print("\r\nhotswap_test: ok\r\n");
        return 0;
    }
    let mut obj = Obj { vtbl: core::ptr::null(), value: 0 };
    // SAFETY: the DLL's exports as declared; `obj` is ours.
    unsafe {
        hs_create(&mut obj, 7);
        hs_set_global(42);
    }
    // SAFETY: `hs_create` set the vtable.
    let object = unsafe { ((*obj.vtbl).get)(&obj) };
    // SAFETY: as above.
    let (version, global) = unsafe { (hs_version(), hs_get_global()) };
    print("hotswap_test: before: version ");
    print_num(version.into());
    print(", object ");
    print_num(object.into());
    print(", global ");
    print_num(global);
    print("\r\nhotswap_test: marker\r\n");
    wait_two_seconds();
    // SAFETY: as above.
    let (version, _, calls, object, global, callback) = unsafe {
        (
            hs_version(),
            hs_version(),
            hs_calls(),
            ((*obj.vtbl).get)(&obj),
            hs_get_global(),
            hs_call_back(plain),
        )
    };
    for (label, value) in [
        ("hotswap_test: after: version ", u64::from(version)),
        (", calls ", calls),
        (", object ", object.into()),
        (", global ", global),
        (", callback ", callback.into()),
    ] {
        print(label);
        print_num(value);
    }
    print("\r\nhotswap_test: ok\r\n");
    0
}

entry!(main);
