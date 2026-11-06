//! ntdll.dll: mapped into every new process when `C:\Windows\System32\
//! ntdll.dll` exists, as Windows does; the initial thread then starts at its
//! `LdrInitializeThunk(Context, NtdllBase)` (Windows Internals: the CONTEXT
//! holds the thread's real start, which ntdll resumes with NtContinue).
//! Without one, programs start directly at their entry point.
//!
//! A 32-bit (WOW64) process gets the 32-bit ntdll, `C:\Windows\SysWOW64\
//! ntdll.dll` (`get32`), whose LdrInitializeThunk its first thread enters
//! in 32-bit code (`nt::wow64`, ARCHITECTURE "Plan: WOW64").

use super::object::{KObject, ObjectBody, ObjectRef};
use super::section::Section;
use crate::sync::IrqMutex;

const PATH: &str = "\\Windows\\System32\\ntdll.dll";
/// The 32-bit ntdll (WOW64).
pub const PATH32: &str = "\\Windows\\SysWOW64\\ntdll.dll";

/// The ntdll image section and the RVA of LdrInitializeThunk.
#[derive(Clone)]
pub struct Ntdll {
    pub section: ObjectRef,
    pub thunk_rva: u32,
    /// KiUserExceptionDispatcher, if exported: user-mode exceptions are
    /// delivered there.
    pub dispatcher_rva: Option<u32>,
    /// KiUserApcDispatcher, if exported: user APCs run there.
    pub apc_dispatcher_rva: Option<u32>,
}

/// Loaded on first use; `Some(None)` once we know there is none.
static NTDLL: IrqMutex<Option<Option<Ntdll>>> = IrqMutex::new(None);
static NTDLL32: IrqMutex<Option<Option<Ntdll>>> = IrqMutex::new(None);

/// The system ntdll, if there is one.
pub fn get() -> Option<Ntdll> {
    let mut slot = NTDLL.lock();
    slot.get_or_insert_with(|| load(PATH, false)).clone()
}

/// The 32-bit ntdll (SysWOW64), if there is one: a PE32 image.
pub fn get32() -> Option<Ntdll> {
    let mut slot = NTDLL32.lock();
    slot.get_or_insert_with(|| load(PATH32, true)).clone()
}

fn load(path: &'static str, x86: bool) -> Option<Ntdll> {
    let node = super::file::lookup(path).ok()?;
    let data = match super::file::read_all(node) {
        Ok(d) => d,
        Err(e) => {
            warn!("ntdll: cannot read {path}: {e:?}");
            return None;
        }
    };
    let mut section = match Section::new_image(&data) {
        Ok(s) => s,
        Err(e) => {
            warn!("ntdll: {path} is not a usable image: {e:?}");
            return None;
        }
    };
    if section.image.as_ref().is_some_and(|i| i.info.x86 != x86) {
        let kind = if x86 { "32-bit (PE32)" } else { "64-bit (PE32+)" };
        warn!("ntdll: {path} is not a {kind} image");
        return None;
    }
    section.file_name = super::file::nt_path(node).ok();
    let Some(thunk_rva) = section.image.as_ref().and_then(|i| i.export(b"LdrInitializeThunk"))
    else {
        warn!("ntdll: {path} does not export LdrInitializeThunk");
        return None;
    };
    let dispatcher_rva =
        section.image.as_ref().and_then(|i| i.export(b"KiUserExceptionDispatcher"));
    let apc_dispatcher_rva = section.image.as_ref().and_then(|i| i.export(b"KiUserApcDispatcher"));
    let section = KObject::new(ObjectBody::Section(section)).ok()?;
    info!("ntdll: using {path}, LdrInitializeThunk at rva {thunk_rva:#x}");
    Some(Ntdll { section, thunk_rva, dispatcher_rva, apc_dispatcher_rva })
}
