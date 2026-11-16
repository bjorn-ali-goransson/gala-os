//! gala's slc.dll: the Software Licensing Client, the name the Software
//! Protection Platform's client API had before Windows 8. Windows keeps
//! both DLLs: its slc.dll forwards 31 of its 40 exports to SPPC.dll and
//! implements 9 Windows-licensing functions of its own (its export
//! table; `exports.txt`).
//!
//! Click-to-Run's service loads it by name while it brings up the App-V
//! ISV API (ROADMAP OF): a `LoadLibrary` that finds no slc.dll is error
//! 126 (ERROR_MOD_NOT_FOUND), which is what Office's own log reports as
//! `OISVAPI::Initialize Failed ... Error:0x7e`, after which the ISV RPC
//! endpoints Word waits for are never served.
//!
//! gala's slc.dll has the same shape: the 31 forwarders go to gala's
//! sppc.dll (ARCHITECTURE "sppc"), so a program asking slc for a
//! license, a SKU or a policy is answered from the one store gala keeps,
//! and the three documented Windows functions are here.
//!
//! Limits (ARCHITECTURE "slc"):
//! - gala keeps no Windows licensing information: every name
//!   SLGetWindowsInformation and SLGetWindowsInformationDWORD are asked
//!   for is SL_E_VALUE_NOT_FOUND, as sppc answers for a policy.
//! - SLIsWindowsGenuineLocal answers SL_GEN_STATE_IS_GENUINE: gala has
//!   no activation state, and the answer a program acts on is the one
//!   that says the installation is sound.
//! - Six names Windows exports are not exported here
//!   (SLConsumeWindowsRight, SLReArmWindows, SLRegisterWindowsEvent,
//!   SLUnregisterWindowsEvent, SLpCheckProductKey,
//!   SLpUpdateComponentTokens): their parameters are in no public
//!   header and gala does not guess them. A program asking for one gets
//!   NULL from GetProcAddress, which the ledger records.
//!
//! Observability: every call is recorded in the ledger as
//! `slc.dll!<function>(<case>)`, "other", with the HRESULT returned as
//! the detail, so a run shows what was asked and what it was answered
//! (the line sppc writes for its own calls).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

// The 32-bit build (SysWOW64) takes the memory routines, 64-bit
// division helpers and unwind personality from ntrt's x86 runtime
// (ARCHITECTURE "Plan: WOW64").
#[cfg(target_arch = "x86")]
use ntrt as _;

// The x64 build's memory routines come from gala's ntdll (as other DLLs
// take them).
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn __CxxFrameHandler3();
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

/// The ledger's "other": a call worth seeing, not a gap.
const LEDGER_OTHER: u32 = 5;

const S_OK: i32 = 0;
const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;
/// SL_E_VALUE_NOT_FOUND: this system has no value of that name.
const SL_E_VALUE_NOT_FOUND: i32 = 0xc004_f012_u32 as i32;
/// SL_GEN_STATE_IS_GENUINE.
const SL_GEN_STATE_IS_GENUINE: u32 = 0;

/// The ledger's room for a name.
const LINE: usize = 96;

/// Records `slc.dll!<function>(<case>)` with the HRESULT as the detail
/// and returns the HRESULT. `case` is the caller's UTF-16 name, or null
/// for no case; what does not fit the line is dropped, as of every
/// recorded name.
fn record(function: &str, case: *const u16, hr: i32) -> i32 {
    let mut text = [0u8; LINE];
    let mut n = 0;
    let mut put = |b: u8, n: &mut usize| {
        if *n < LINE {
            text[*n] = b;
            *n += 1;
        }
    };
    for part in [b"slc.dll!".as_slice(), function.as_bytes(), b"("] {
        for &b in part {
            put(b, &mut n);
        }
    }
    if !case.is_null() {
        let mut i = 0;
        // One byte is kept for the closing `)`.
        while n + 1 < LINE {
            // SAFETY: the caller's NUL-terminated string.
            let c = unsafe { *case.add(i) };
            if c == 0 {
                break;
            }
            put(if (0x20..0x7f).contains(&c) { c as u8 } else { b'?' }, &mut n);
            i += 1;
        }
    }
    put(b')', &mut n);
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(text.as_ptr(), n, LEDGER_OTHER, u64::from(hr as u32)) };
    hr
}

/// SLGetWindowsInformation(pwszValueName, peDataType, pcbValue,
/// ppbValue): a Windows licensing value by name. gala keeps none.
#[unsafe(no_mangle)]
pub extern "system" fn SLGetWindowsInformation(
    name: *const u16,
    _data_type: *mut u32,
    size: *mut u32,
    value: *mut *mut u8,
) -> i32 {
    const F: &str = "SLGetWindowsInformation";
    if name.is_null() || size.is_null() || value.is_null() {
        return record(F, core::ptr::null(), E_INVALIDARG);
    }
    record(F, name, SL_E_VALUE_NOT_FOUND)
}

/// SLGetWindowsInformationDWORD(pwszValueName, pdwValue): the DWORD
/// form of the same.
#[unsafe(no_mangle)]
pub extern "system" fn SLGetWindowsInformationDWORD(name: *const u16, value: *mut u32) -> i32 {
    const F: &str = "SLGetWindowsInformationDWORD";
    if name.is_null() || value.is_null() {
        return record(F, core::ptr::null(), E_INVALIDARG);
    }
    record(F, name, SL_E_VALUE_NOT_FOUND)
}

/// SLIsWindowsGenuineLocal(pGenuineState): the installation's genuine
/// state, answered without asking a service (the "Local" of the name).
#[unsafe(no_mangle)]
pub extern "system" fn SLIsWindowsGenuineLocal(state: *mut u32) -> i32 {
    const F: &str = "SLIsWindowsGenuineLocal";
    if state.is_null() {
        return record(F, core::ptr::null(), E_INVALIDARG);
    }
    // SAFETY: the caller's output.
    unsafe { *state = SL_GEN_STATE_IS_GENUINE };
    record(F, core::ptr::null(), S_OK)
}

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

/// A panic is a bug: its location goes to the ledger
/// (`slc.dll!panic <file>`, the line as detail) and the process ends
/// with STATUS_STACK_BUFFER_OVERRUN (fail-fast), as gala's other DLLs
/// do.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut text = [0u8; 80];
    let mut n = 0;
    let line = info.location().map_or(0, |l| {
        for &b in b"slc.dll!panic ".iter().chain(l.file().as_bytes()) {
            if n < text.len() {
                text[n] = b;
                n += 1;
            }
        }
        l.line()
    });
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(text.as_ptr(), n, LEDGER_OTHER, line.into()) };
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn ExitProcess(code: u32) -> !;
}
