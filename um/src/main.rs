//! gala-um: runs a Windows x64 program inside this Linux process, with
//! gala's NT layer (the kernel library built with `--features hosted`) as
//! its "kernel". The platform is `kernel::hosted` (docs/GALA_UM.md).
//!
//! usage: um <program.exe> [arguments]

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: um <program.exe> [arguments]");
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
    let arguments = args.collect::<Vec<_>>().join(" ");
    match kernel::hosted::run(&name, image, &arguments) {
        // The program's exit status decides: 0 for success.
        Ok(status) => ExitCode::from(if status == 0 { 0 } else { 1 }),
        Err(e) => {
            eprintln!("um: {e}");
            ExitCode::from(2)
        }
    }
}
