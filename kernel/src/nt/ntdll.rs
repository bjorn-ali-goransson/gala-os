//! ntdll.dll: mapped into every new process when `C:\Windows\System32\
//! ntdll.dll` exists, as Windows does; the initial thread then starts at its
//! `LdrInitializeThunk(Context, NtdllBase)` (Windows Internals: the CONTEXT
//! holds the thread's real start, which ntdll resumes with NtContinue).
//! Without one, programs start directly at their entry point.

use super::object::{KObject, ObjectBody, ObjectRef};
use super::section::Section;
use crate::sync::IrqMutex;

const PATH: &str = "\\Windows\\System32\\ntdll.dll";

/// The ntdll image section and the RVA of LdrInitializeThunk.
#[derive(Clone)]
pub struct Ntdll {
    pub section: ObjectRef,
    pub thunk_rva: u32,
}

/// Loaded on first use; `Some(None)` once we know there is none.
static NTDLL: IrqMutex<Option<Option<Ntdll>>> = IrqMutex::new(None);

/// The system ntdll, if there is one.
pub fn get() -> Option<Ntdll> {
    let mut slot = NTDLL.lock();
    slot.get_or_insert_with(load).clone()
}

fn load() -> Option<Ntdll> {
    let node = super::file::lookup(PATH).ok()?;
    let data = match super::file::read_all(node) {
        Ok(d) => d,
        Err(e) => {
            warn!("ntdll: cannot read {PATH}: {e:?}");
            return None;
        }
    };
    let mut section = match Section::new_image(&data) {
        Ok(s) => s,
        Err(e) => {
            warn!("ntdll: {PATH} is not a usable image: {e:?}");
            return None;
        }
    };
    section.file_name = super::file::nt_path(node).ok();
    let Some(thunk_rva) = section.image.as_ref().and_then(|i| i.export(b"LdrInitializeThunk"))
    else {
        warn!("ntdll: {PATH} does not export LdrInitializeThunk");
        return None;
    };
    let section = KObject::new(ObjectBody::Section(section)).ok()?;
    info!("ntdll: using {PATH}, LdrInitializeThunk at rva {thunk_rva:#x}");
    Some(Ntdll { section, thunk_rva })
}
