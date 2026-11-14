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
//! With `kernel`, `kstack` or `both` (x64 only; ROADMAP SN step 1e:
//! scenarios `khotswap_machine`, `khotswap_stack`, `khotswap_both`), the
//! kernel's probe (GalaHotswapProbe, gala's own service: its static set
//! to 42, its object made with 7) is read before and after
//! (`hotswap_test: kernel before/after: version V, calls C, object O,
//! global G`): `version 1, calls 0, object 7, global 42` on the default
//! kernel, `version 2, calls 2, object 107, global 1042` with its
//! `hotswap_v2` build swapped in. `kernel`: the probe alone, saved at the
//! marker. `both`: the DLL's lines and the probe's, saved at the marker
//! (a kernel and a DLL swapped in one plan). `kstack`: the DLL and the
//! probe set up, then two seconds inside the kernel's probe (the kernel
//! prints `hotswap probe: waiting in the kernel`, where the machine is
//! saved: that changed kernel function is on the thread's kernel stack,
//! so the swap is refused, the DLL's with it), then both read (the first
//! builds' values).
//!
//! With `files` (ROADMAP SN step 1f: scenario `hotswap_files`): after
//! the marker it loads `hotswap_late.dll` and `hotswap_added.dll` by name
//! (LdrLoadDll; neither is loaded before) and prints `hotswap_test: late
//! version V` and `hotswap_test: added version V` (`added: not found`
//! when there is no such file). Booted, the image has hotswap_late.dll
//! (hotswap_dll's first build under that name) and no hotswap_added.dll:
//! `late version 1`, `added: not found` (on Windows too). Resumed with
//! every changed module swapped in, both are files no process maps:
//! hotswap_late.dll replaced whole by the `layout` build (one a patch
//! cannot take), hotswap_added.dll added (the `v2` build): `late version
//! 2`, `added version 2`.
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

#[repr(C)]
struct AnsiString {
    length: u16,
    maximum_length: u16,
    buffer: *const u8,
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn LdrLoadDll(
        path: *const u16,
        characteristics: *const u32,
        name: *const UnicodeString,
        base: *mut usize,
    ) -> i32;
    fn LdrGetProcedureAddress(
        module: usize,
        name: *const AnsiString,
        ordinal: u32,
        address: *mut usize,
    ) -> i32;
}

/// Loads DLL `name` and prints `hotswap_test: <label> version V` (its
/// `hs_version`), or `<label>: not found`.
fn late_load(name: &[u16], label: &str) {
    let mut base = 0usize;
    // SAFETY: LdrLoadDll(DllPath, DllCharacteristics, DllName, *DllHandle)
    // with a name that lives through the call.
    let st = unsafe {
        LdrLoadDll(core::ptr::null(), core::ptr::null(), &UnicodeString::new(name), &mut base)
    };
    print("hotswap_test: ");
    print(label);
    if st != 0 {
        print(": not found\r\n");
        return;
    }
    let export = b"hs_version";
    let export = AnsiString { length: 10, maximum_length: 10, buffer: export.as_ptr() };
    let mut address = 0usize;
    // SAFETY: LdrGetProcedureAddress(DllHandle, ProcedureName, 0, *Address)
    // on the module just loaded.
    let st = unsafe { LdrGetProcedureAddress(base, &export, 0, &mut address) };
    if st != 0 || address == 0 {
        print(": no hs_version\r\n");
        return;
    }
    // SAFETY: hotswap_dll's `hs_version` (`extern "C" fn() -> u32`).
    let version: extern "C" fn() -> u32 = unsafe { core::mem::transmute(address) };
    print(" version ");
    print_num(version().into());
    print("\r\n");
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

/// The command line has `word`.
fn has_arg(word: &str) -> bool {
    let line = command_line();
    let w = word.as_bytes();
    line.windows(w.len()).any(|c| c.iter().zip(w).all(|(&a, &b)| a == u16::from(b)))
}

/// The kernel's probe: set up (`object 7, global 42`).
#[cfg(not(target_arch = "x86"))]
fn kernel_setup() {
    let (st, _) = gala_hotswap_probe(0, 42);
    let (st2, _) = gala_hotswap_probe(1, 7);
    if st != STATUS_SUCCESS || st2 != STATUS_SUCCESS {
        print("hotswap_test: the kernel's probe is not there (not gala)\r\n");
    }
}

/// Prints `hotswap_test: kernel <when>: version V, calls C, object O,
/// global G`.
#[cfg(not(target_arch = "x86"))]
fn kernel_line(when: &str) {
    let (_, [version, calls, object, global]) = gala_hotswap_probe(2, 0);
    print("hotswap_test: kernel ");
    print(when);
    for (label, value) in
        [(": version ", version), (", calls ", calls), (", object ", object), (", global ", global)]
    {
        print(label);
        print_num(value);
    }
    print("\r\n");
}

/// The DLL's object and static set up (`object 7, global 42`): the
/// object.
fn dll_setup() -> Obj {
    let mut obj = Obj { vtbl: core::ptr::null(), value: 0 };
    // SAFETY: the DLL's exports as declared; `obj` is ours.
    unsafe {
        hs_create(&mut obj, 7);
        hs_set_global(42);
    }
    obj
}

/// Prints `hotswap_test: before: version V, object O, global G`.
fn dll_before(obj: &Obj) {
    // SAFETY: `hs_create` set the vtable; the DLL's exports as declared.
    let object = unsafe { ((*obj.vtbl).get)(obj) };
    // SAFETY: as above.
    let (version, global) = unsafe { (hs_version(), hs_get_global()) };
    print("hotswap_test: before: version ");
    print_num(version.into());
    print(", object ");
    print_num(object.into());
    print(", global ");
    print_num(global);
    print("\r\n");
}

/// Prints `hotswap_test: after: version V, calls C, object O, global G,
/// callback B`.
fn dll_after(obj: &Obj) {
    // SAFETY: as in `dll_before`.
    let (version, _, calls, object, global, callback) = unsafe {
        (
            hs_version(),
            hs_version(),
            hs_calls(),
            ((*obj.vtbl).get)(obj),
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
    print("\r\n");
}

fn main() -> u32 {
    if has_arg("files") {
        print("hotswap_test: marker\r\n");
        wait_two_seconds();
        late_load(&wide!("hotswap_late.dll"), "late");
        late_load(&wide!("hotswap_added.dll"), "added");
        print("hotswap_test: ok\r\n");
        return 0;
    }
    #[cfg(not(target_arch = "x86"))]
    {
        let (kernel, both, kstack) = (has_arg("kernel"), has_arg("both"), has_arg("kstack"));
        if kernel || both || kstack {
            let obj = (both || kstack).then(dll_setup);
            kernel_setup();
            if let Some(obj) = &obj {
                dll_before(obj);
            }
            kernel_line("before");
            if kstack {
                print("hotswap_test: kstack\r\n");
                let _ = gala_hotswap_probe(3, 0);
            } else {
                print("hotswap_test: marker\r\n");
                wait_two_seconds();
            }
            if let Some(obj) = &obj {
                dll_after(obj);
            }
            kernel_line("after");
            print("hotswap_test: ok\r\n");
            return 0;
        }
    }
    if has_arg("stack") {
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
    let obj = dll_setup();
    dll_before(&obj);
    print("hotswap_test: marker\r\n");
    wait_two_seconds();
    dll_after(&obj);
    print("hotswap_test: ok\r\n");
    0
}

entry!(main);
