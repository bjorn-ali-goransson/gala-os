//! gala-um: runs a Windows x64 program inside this Linux process, with
//! gala's NT layer as its "kernel" (docs/GALA_UM.md).
//!
//! U2a, this file's state: the host mechanism. The image is mapped at its
//! base with mmap, the PEB, process parameters, TEB, KUSER_SHARED_DATA
//! and stack sit at gala's layout addresses (`nt::layout`), GS points at
//! the TEB (arch_prctl), and the program's `syscall` instructions raise
//! SIGSYS through Linux's syscall user dispatch. The services behind it
//! are a bootstrap set (services.rs) until U2b links the NT layer itself.
//!
//! usage: um <program.exe>

use std::process::ExitCode;

// The kernel's own PE parser and layout, shared as source (pure code).
#[path = "../../kernel/src/nt/layout.rs"]
#[allow(dead_code)]
mod layout;
#[path = "../../kernel/src/nt/pe.rs"]
#[allow(dead_code)]
mod pe;

mod host;
mod process;
mod services;

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: um <program.exe>");
        return ExitCode::from(2);
    };
    let image = match std::fs::read(&path) {
        Ok(data) => data,
        Err(e) => {
            eprintln!("um: reading {path}: {e}");
            return ExitCode::from(2);
        }
    };
    let name = std::path::Path::new(&path)
        .file_name()
        .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
    match process::load(&name, &image) {
        Ok(start) => host::run(start),
        Err(e) => {
            eprintln!("um: loading {name}: {e}");
            ExitCode::from(1)
        }
    }
}
