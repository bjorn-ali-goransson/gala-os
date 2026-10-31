//! gala-um: runs a Windows x64 program inside this Linux process, with
//! gala's NT layer (the kernel library built with `--features hosted`) as
//! its "kernel". The platform is `kernel::hosted` (docs/GALA_UM.md).
//!
//! usage: um [--trace] [--cpus=<n>] [--module=<directory on C:>/<name>=<host file>]... <program.exe> [arguments]
//!
//! `--cpus`: show at most n of the host's processors (um-test: 2, as the
//! QEMU boot tests).
//!
//! Modules are what the kernel gets as boot modules (gala's ntdll.dll in
//! Windows/System32, test DLLs in gala, ...).

use std::process::ExitCode;

/// File pages must be memfd frames so sections can map them.
#[global_allocator]
static ALLOCATOR: kernel::hosted::memory::Allocator = kernel::hosted::memory::Allocator;

fn main() -> ExitCode {
    kernel::hosted::fix_address_space();
    let mut args = std::env::args().skip(1).peekable();
    let mut modules = Vec::new();
    while let Some(option) = args.next_if(|a| a.starts_with("--")) {
        if option == "--trace" {
            // Every system call on stderr, as the kernel's `trace` flag.
            kernel::nt::syscalls::set_trace(true);
            continue;
        }
        if let Some(n) = option.strip_prefix("--cpus=") {
            match n.parse() {
                Ok(n) => kernel::hosted::threads::limit_cpus(n),
                Err(_) => {
                    eprintln!("um: --cpus=<number>, not {option}");
                    return ExitCode::from(2);
                }
            }
            continue;
        }
        let Some(spec) = option.strip_prefix("--module=") else {
            eprintln!("um: unknown option {option}");
            return ExitCode::from(2);
        };
        let Some((target, host)) = spec.split_once('=') else {
            eprintln!("um: --module=<directory>/<name>=<host file>, not {spec}");
            return ExitCode::from(2);
        };
        let (directory, name) = target.rsplit_once('/').unwrap_or(("", target));
        match std::fs::read(host) {
            Ok(data) => modules.push((directory.to_string(), name.to_string(), data)),
            Err(e) => {
                eprintln!("um: reading {host}: {e}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(path) = args.next() else {
        eprintln!(
            "usage: um [--trace] [--cpus=<n>] [--module=<dir>/<name>=<host file>]... <program.exe> [arguments]"
        );
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
    // The rest of the command line, as one string (the kernel's `args=`).
    let arguments = args.collect::<Vec<_>>().join(" ");
    match kernel::hosted::run(&name, image, &arguments, modules) {
        // The program's exit status decides: 0 for success.
        Ok(status) => ExitCode::from(if status == 0 { 0 } else { 1 }),
        Err(e) => {
            eprintln!("um: {e}");
            ExitCode::from(2)
        }
    }
}
