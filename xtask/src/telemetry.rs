//! What a boot test records about QEMU and the host (docs/OBSERVABILITY.md,
//! "Early boot"): enough to explain a boot that dies without a word.
//!
//! Per VM, next to the image: `<image>.qemu.log` (QEMU's `-d
//! cpu_reset,guest_errors`: a triple fault, a reset, bad guest accesses),
//! `<image>.bios.log` (SeaBIOS's debug port 0x402), `<image>.phases.log`
//! (the kernel's boot phase markers, port 0x403:
//! kernel/src/arch/x86_64/bootphase.rs). After the run a few `xtask:` lines
//! go at the end of the serial log: how QEMU exited, how many vCPUs it
//! made (QMP `query-cpus-fast`), which accelerator ran it (`query-kvm`,
//! because `-accel kvm -accel tcg` falls back without failing), the
//! host's load and free memory at the start and the end; after an
//! abnormal end also the last phase markers and the tail of QEMU's log.

use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

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
pub fn qmp_facts(socket: &Path) -> Option<VmFacts> {
    // The crate's one QMP client (snapshot.rs): it waits for QEMU to
    // make the socket, says the greeting and qmp_capabilities, and skips
    // the asynchronous events that come between a command and its
    // answer, giving back each answer compact.
    let mut qmp = crate::snapshot::Qmp::connect(socket, Duration::from_secs(10)).ok()?;
    let cpus = qmp.cmd("query-cpus-fast", "{}").ok()?;
    // Which accelerator the machine actually got: QEMU is started with
    // `-accel kvm -accel tcg` and takes the second when the first is
    // refused, saying so on its standard error and nowhere else. This is
    // the answer a snapshot's manifest records, from the same code, so a
    // log and a snapshot cannot disagree about a run.
    let kvm = crate::snapshot::accel(&mut qmp).ok().map(|a| a == "kvm");
    let _ = std::fs::remove_file(socket);
    Some(VmFacts { vcpus: cpus.matches("\"cpu-index\"").count(), kvm })
}

/// What QMP told about a running VM (`qmp_facts`).
pub struct VmFacts {
    /// Processors the machine has.
    pub vcpus: usize,
    /// Whether it runs under KVM (None: `query-kvm` went unanswered).
    pub kvm: Option<bool>,
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
    // An ARM64 twin (`arm_onecpu_*`) as its x64 scenario.
    if stem.starts_with("test-onecpu_") || stem.starts_with("test-arm_onecpu_") { 1 } else { 2 }
}

/// The firmware's own crash report in a boot's console output (serial or
/// the firmware's debug log): EDK2's exception dump on aarch64
/// (`Synchronous Exception at ...` and its `[ n] Module.dll` frame) or
/// x64 (`!!!! X64 Exception Type ...`), and its `ASSERT [...]` line. A
/// firmware that crashes hangs in its assert loop until the run's limit,
/// before the kernel has printed anything, so without this the verdict is
/// a bare timeout with no boot phases. The kernel prints none of these.
pub fn firmware_crash(log: &str) -> Option<String> {
    let lines: Vec<&str> = log.lines().map(str::trim).collect();
    let at = lines.iter().position(|l| {
        l.starts_with("Synchronous Exception at") || l.starts_with("!!!! X64 Exception")
    })?;
    let mut said = lines[at].to_string();
    // The first frame names the module that faulted.
    if let Some(module) = lines[at..]
        .iter()
        .find_map(|l| l.contains("[ 0]").then(|| l.split_whitespace().last()).flatten())
    {
        said += &format!(" in {module}");
    }
    if let Some(fault) = lines[at..].iter().find(|l| l.starts_with("ESR ")) {
        said += &format!(" ({fault})");
    }
    if let Some(assert) = lines[at..].iter().find(|l| l.starts_with("ASSERT [")) {
        said += &format!("; {assert}");
    }
    Some(said)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_a_firmware_crash() {
        let aavmf = "BdsDxe: loading Boot0001\n\
            Synchronous Exception at 0x000000005FC32100\n\
            PC 0x00005FC32100 (0x00005FC31000+0x00001100) [ 0] VirtioNetDxe.dll\n\
            PC 0x00005FC2C9A4 (0x00005FC28000+0x000049A4) [ 1] MnpDxe.dll\n\
            ESR 0x96000007 FAR 0x0000000000000008  Data abort: Translation fault, third level\n\
            ASSERT [ArmCpuDxe] DefaultExceptionHandler.c(343): ((BOOLEAN)(0==1))\n";
        assert_eq!(
            firmware_crash(aavmf).as_deref(),
            Some(
                "Synchronous Exception at 0x000000005FC32100 in VirtioNetDxe.dll \
                 (ESR 0x96000007 FAR 0x0000000000000008  Data abort: Translation fault, third level); \
                 ASSERT [ArmCpuDxe] DefaultExceptionHandler.c(343): ((BOOLEAN)(0==1))"
            )
        );
        let ovmf = "!!!! X64 Exception Type - 0E(#PF - Page-Fault)  CPU Apic ID - 00000000 !!!!\n\
            RIP  - 000000007E5A1234\n";
        assert_eq!(
            firmware_crash(ovmf).as_deref(),
            Some("!!!! X64 Exception Type - 0E(#PF - Page-Fault)  CPU Apic ID - 00000000 !!!!")
        );
        assert_eq!(firmware_crash("[info ] smp: 2 CPUs online\npanicked at x"), None);
    }

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
