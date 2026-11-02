//! gala-um: runs a Windows x64 program inside this Linux process, with
//! gala's NT layer (the kernel library built with `--features hosted`) as
//! its "kernel". The platform is `kernel::hosted` (docs/GALA_UM.md).
//!
//! usage: um [--trace] [--sim=<seed>] [--clock=<spec>] [--record=<file>] [--replay=<file>] [--cpus=<n>] [--failpoint=<rules>] [--failseed=<n>] [--failtrace] [--module=<directory on C:>/<name>=<host file>]... <program.exe> [arguments]
//!
//! `--failpoint`, `--failseed`: fault injection while the program runs,
//! as the kernel's `failpoint=` and `failseed=` (kernel/src/failpoint.rs).
//!
//! `--sim=<seed>`: deterministic simulation (ROADMAP R5): one NT thread
//! runs at a time, the seed chooses which at every system call, wait and
//! yield, and time is virtual; the run ends with `sim: seed S trace H`,
//! the same for the same seed (docs/GALA_UM.md). Exit status 4: a
//! deadlock (every thread waits for ever), with the threads' waits.
//!
//! `--clock=<spec>`: clock control, as the kernel's `clock=`
//! (kernel/src/clockctl.rs): `control` allows the test-only
//! GalaClockControl call; `freeze`, `rate:<n>[/<d>]`, `advance:<secs>`,
//! `system:<UNIX secs>` set the clock when the program starts.
//!
//! `--record=<file>`: what the run reads from outside (the host's clock,
//! random numbers, the boot time) and its settings (seed, processors,
//! clock, fault injection, arguments) go to `file`; `--replay=<file>`
//! runs with them (kernel/src/replay.rs). Options given with `--replay`
//! override the recorded settings.
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

const USAGE: &str = "usage: um [--trace] [--sim=<seed>] [--clock=<spec>] [--record=<file>] \
    [--replay=<file>] [--cpus=<n>] [--failpoint=<rules>] [--failseed=<n>] [--failtrace] \
    [--module=<dir>/<name>=<host file>]... <program.exe> [arguments]";

/// The settings a recording keeps (`--record`) and a replay restores.
#[derive(Default)]
struct Settings {
    sim: Option<String>,
    cpus: Option<String>,
    clock: Option<String>,
    failpoint: Option<String>,
    failseed: Option<String>,
}

fn main() -> ExitCode {
    kernel::hosted::fix_address_space();
    let mut args = std::env::args().skip(1).peekable();
    let mut modules = Vec::new();
    let mut settings = Settings::default();
    let (mut record, mut replay) = (None, None);
    while let Some(option) = args.next_if(|a| a.starts_with("--")) {
        let value = |prefix: &str| option.strip_prefix(prefix).map(str::to_string);
        if let Some(spec) = value("--failpoint=") {
            // Fault injection, as the kernel's `failpoint=` and `failseed=`.
            settings.failpoint = Some(spec);
        } else if option == "--failtrace" {
            // Each injected failure with a backtrace.
            kernel::failpoint::set_trace(true);
        } else if let Some(seed) = value("--failseed=") {
            settings.failseed = Some(seed);
        } else if let Some(seed) = value("--sim=") {
            settings.sim = Some(seed);
        } else if let Some(spec) = value("--clock=") {
            // Clock control, as the kernel's `clock=`.
            settings.clock = Some(spec);
        } else if let Some(path) = value("--record=") {
            record = Some(path);
        } else if let Some(path) = value("--replay=") {
            replay = Some(path);
        } else if option == "--trace" {
            // Every system call on stderr, as the kernel's `trace` flag.
            kernel::nt::syscalls::set_trace(true);
        } else if let Some(n) = value("--cpus=") {
            settings.cpus = Some(n);
        } else if let Some(spec) = value("--module=") {
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
        } else {
            eprintln!("um: unknown option {option}");
            return ExitCode::from(2);
        }
    }
    let Some(path) = args.next() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let name = std::path::Path::new(&path)
        .file_name()
        .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
    // The rest of the command line, as one string (the kernel's `args=`).
    let mut arguments = args.collect::<Vec<_>>().join(" ");

    // A replay runs with the recorded settings unless given here.
    if let Some(file) = &replay {
        if record.is_some() {
            eprintln!("um: --record and --replay together");
            return ExitCode::from(2);
        }
        if let Err(e) = kernel::hosted::replay_from(file) {
            eprintln!("um: --replay={e}");
            return ExitCode::from(2);
        }
        let recorded = |key: &str| kernel::replay::replay_info(key).map(str::to_string);
        for (key, slot) in [
            ("sim", &mut settings.sim),
            ("cpus", &mut settings.cpus),
            ("clock", &mut settings.clock),
            ("failpoint", &mut settings.failpoint),
            ("failseed", &mut settings.failseed),
        ] {
            if slot.is_none() {
                *slot = recorded(key);
            }
        }
        if arguments.is_empty() {
            arguments = recorded("args").unwrap_or_default();
        }
        if let Some(program) = recorded("program")
            && program != name
        {
            eprintln!("um: warning: {file} recorded {program}, not {name}");
        }
    }

    if let Some(seed) = &settings.sim {
        match seed.parse() {
            Ok(seed) => kernel::hosted::sim::enable(seed),
            Err(_) => {
                eprintln!("um: --sim=<seed: a number>, not {seed}");
                return ExitCode::from(2);
            }
        }
    }
    if let Some(n) = &settings.cpus {
        match n.parse() {
            Ok(n) => kernel::hosted::threads::limit_cpus(n),
            Err(_) => {
                eprintln!("um: --cpus=<number>, not {n}");
                return ExitCode::from(2);
            }
        }
    }
    if let Some(spec) = &settings.clock
        && let Err(e) = kernel::hosted::configure_clock(spec)
    {
        eprintln!("um: {e}");
        return ExitCode::from(2);
    }
    if let Some(spec) = &settings.failpoint
        && let Err(e) = kernel::hosted::configure_failpoints(spec, settings.failseed.as_deref())
    {
        eprintln!("um: {e}");
        return ExitCode::from(2);
    }
    let image = match std::fs::read(&path) {
        Ok(data) => data,
        Err(e) => {
            eprintln!("um: reading {path}: {e}");
            return ExitCode::from(2);
        }
    };

    if let Some(file) = &record {
        if let Err(e) = kernel::hosted::record_to(file) {
            eprintln!("um: --record={e}");
            return ExitCode::from(2);
        }
        let cpus = kernel::hosted::threads::cpu_count().to_string();
        kernel::replay::info("program", &name);
        kernel::replay::info("args", &arguments);
        kernel::replay::info("cpus", &cpus);
        for (key, value) in [
            ("sim", &settings.sim),
            ("clock", &settings.clock),
            ("failpoint", &settings.failpoint),
            ("failseed", &settings.failseed),
        ] {
            if let Some(value) = value {
                kernel::replay::info(key, value);
            }
        }
    }
    match kernel::hosted::run(&name, image, &arguments, modules) {
        // The program's exit status decides: 0 for success.
        Ok(status) => ExitCode::from(if status == 0 { 0 } else { 1 }),
        Err(e) => {
            eprintln!("um: {e}");
            ExitCode::from(2)
        }
    }
}
