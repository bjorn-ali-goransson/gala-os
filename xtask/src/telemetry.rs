//! What a boot test records about QEMU and the host (docs/OBSERVABILITY.md,
//! "Early boot"): enough to explain a boot that dies without a word.
//!
//! Per VM, next to the image: `<image>.qemu.log` (QEMU's `-d
//! cpu_reset,guest_errors`: a triple fault, a reset, bad guest accesses),
//! `<image>.bios.log` (SeaBIOS's debug port 0x402), `<image>.phases.log`
//! (the kernel's boot phase markers, port 0x403:
//! kernel/src/arch/x86_64/bootphase.rs). After the run a few `xtask:` lines
//! go at the end of the serial log: how QEMU exited, how many vCPUs it
//! made (QMP `query-cpus-fast`), the host's load and free memory at the
//! start and the end; after an abnormal end also the last phase markers
//! and the tail of QEMU's log.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// The kernel's phase-marker port (kernel/src/arch/x86_64/bootphase.rs).
pub const PHASE_PORT: &str = "0x403";
/// SeaBIOS's debug output port.
pub const BIOS_PORT: &str = "0x402";

/// The host's load average and available memory, one line.
pub fn host_state() -> String {
    let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let load: Vec<&str> = load.split_whitespace().take(3).collect();
    let mem = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let avail = mem
        .lines()
        .find(|l| l.starts_with("MemAvailable:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|kb| kb.parse::<u64>().ok())
        .map_or("?".to_string(), |kb| (kb / 1024).to_string());
    let cpus = std::thread::available_parallelism().map_or(0, |n| n.get());
    format!("load average {} on {cpus} CPUs, {avail} MiB available", load.join(" "))
}

/// A unique, short QMP socket path (unix socket paths are limited to 108
/// bytes; image paths in worktrees are longer).
pub fn qmp_socket() -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    std::env::temp_dir().join(format!(
        "gala-qmp-{}-{}.sock",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Asks QEMU (QMP at `socket`) how many vCPUs it created; waits up to
/// 10 s for the socket.
pub fn qmp_cpu_count(socket: &Path) -> Option<usize> {
    let start = Instant::now();
    let stream = loop {
        match UnixStream::connect(socket) {
            Ok(s) => break s,
            Err(_) if start.elapsed() < Duration::from_secs(10) => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(_) => return None,
        }
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut writer = stream.try_clone().ok()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?; // greeting
    writer.write_all(b"{\"execute\":\"qmp_capabilities\"}\n").ok()?;
    let reply = |reader: &mut BufReader<UnixStream>| -> Option<String> {
        // Skip asynchronous events until a "return" (or "error").
        for _ in 0..20 {
            let mut line = String::new();
            reader.read_line(&mut line).ok()?;
            if line.contains("\"return\"") || line.contains("\"error\"") {
                return Some(line);
            }
        }
        None
    };
    reply(&mut reader)?;
    writer.write_all(b"{\"execute\":\"query-cpus-fast\"}\n").ok()?;
    let cpus = reply(&mut reader)?;
    let _ = std::fs::remove_file(socket);
    Some(cpus.matches("\"cpu-index\"").count())
}

/// How QEMU ended, in words.
pub fn exit_reason(status: Option<ExitStatus>, stopped_by_xtask: bool) -> String {
    use std::os::unix::process::ExitStatusExt;
    match status {
        None if stopped_by_xtask => "stopped by xtask".into(),
        None => "killed by xtask (timeout)".into(),
        Some(s) => match (s.code(), s.signal()) {
            (Some(33), _) => "status 33: the kernel reported success".into(),
            (Some(35), _) => "status 35: the kernel reported failure".into(),
            (Some(0), _) => "status 0: the guest reset or powered off without the kernel's \
                             exit code (with -no-reboot a triple fault ends QEMU this way)"
                .into(),
            (Some(c), _) => format!("status {c}"),
            (None, Some(sig)) => format!("killed by signal {sig}"),
            (None, None) => "unknown".into(),
        },
    }
}

/// The last `n` lines of a text file ("" if missing).
pub fn tail(path: &Path, n: usize) -> String {
    let text = std::fs::read(path).map(|b| String::from_utf8_lossy(&b).into_owned());
    let text = text.unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// The CPU count the kernel reported (`smp: N CPUs online`), the first
/// time (later kernels after a handover say it again).
pub fn cpus_online(serial: &str) -> Option<u32> {
    serial.match_indices("smp: ").find_map(|(at, _)| {
        let (n, rest) = serial[at + 5..].split_once(' ')?;
        rest.starts_with("CPUs online").then(|| n.parse().ok()).flatten()
    })
}

/// vCPUs a boot test asks QEMU for: one for `onecpu_*` scenarios, else two.
pub fn vm_cpus(iso: &Path) -> u32 {
    let stem = iso.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if stem.starts_with("test-onecpu_") { 1 } else { 2 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_cpu_count() {
        let log = "x\n[info ] smp: bootloader: 1 CPU(s)\n[info ] smp: 2 CPUs online (1 reported)\n\
                   [info ] smp: 1 CPUs online";
        assert_eq!(cpus_online(log), Some(2));
        assert_eq!(cpus_online("nothing"), None);
        assert_eq!(vm_cpus(Path::new("target/debug/test-onecpu_bench.iso")), 1);
        assert_eq!(vm_cpus(Path::new("target/debug/batch-hello.iso")), 2);
    }
}
