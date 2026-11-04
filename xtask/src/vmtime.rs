//! A VM's time limit counted in the time its emulator could run
//! (ROADMAP Priorities; WORKFLOW "Test speed and environments").
//!
//! The guest's own clock cannot tell a starved VM from a slow one: under
//! QEMU's TCG (no `-icount`) the guest's TSC, HPET, PM timer and RTC all
//! follow the host's monotonic clock, starved or not (measured
//! 2026-11-04, clockcheck.rs). What a starved VM lacks is host CPU time
//! while it wants it, and Linux counts exactly that per thread: the
//! second field of `/proc/<pid>/task/<tid>/schedstat` is the time the
//! thread waited on a run queue (runnable, not running). So a VM's
//! *run time* is the wall time since it started less the run-queue wait
//! of its most-starved vCPU thread (`CPU n/TCG`, `CPU n/KVM`, `ALL
//! CPUs/TCG`), and the time limit counts that:
//! - on an idle host it is the wall time: limits as before;
//! - a starved VM's run time advances only while it can run, so it is
//!   not killed for the host's load (this replaced the `host_overload`
//!   stretch, a guess from the load average);
//! - a hung guest is still caught at the same run time: one stuck in a
//!   wait halts its vCPUs, which then sleep (not on a run queue), so its
//!   run time is the wall time; one spinning is runnable and gets its
//!   share of the host.
//!
//! A wall-clock ceiling of [`BACKSTOP`] times the limit remains for a VM
//! whose threads cannot be read or that never gets to run.

use std::fs;
use std::time::{Duration, Instant};

/// The wall-clock ceiling, in multiples of the limit.
pub const BACKSTOP: u32 = 4;

/// The run-queue wait (ns) in a `schedstat` file's text: its second field.
pub fn run_delay_ns(schedstat: &str) -> Option<u64> {
    schedstat.split_whitespace().nth(1)?.parse().ok()
}

/// Whether a QEMU thread's name (`comm`) is a vCPU's.
pub fn is_vcpu(comm: &str) -> bool {
    let comm = comm.trim();
    comm.ends_with("/TCG") || comm.ends_with("/KVM")
}

/// One look at a VM's clocks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub wall_ms: u64,
    /// The most-starved vCPU thread's run-queue wait (None: no vCPU
    /// thread could be read).
    pub starved_ms: Option<u64>,
}

impl Sample {
    /// The VM's run time (the wall time when its threads cannot be read).
    pub fn run_ms(&self) -> u64 {
        self.wall_ms.saturating_sub(self.starved_ms.unwrap_or(0))
    }

    /// Why `limit` is up, if it is.
    pub fn over(&self, limit: Duration) -> Option<Over> {
        let limit_ms = limit.as_millis() as u64;
        if self.run_ms() > limit_ms {
            Some(Over::RunTime)
        } else if self.wall_ms > limit_ms.saturating_mul(BACKSTOP.into()) {
            Some(Over::Backstop)
        } else {
            None
        }
    }

    /// How the time was spent, for the log (`xtask: vm time: ...`).
    pub fn describe(&self) -> String {
        let s = |ms: u64| ms as f64 / 1000.0;
        match self.starved_ms {
            Some(w) => format!(
                "{:.1}s run of {:.1}s wall ({:.1}s waiting for a host CPU)",
                s(self.run_ms()),
                s(self.wall_ms),
                s(w)
            ),
            None => format!(
                "{:.1}s wall (its vCPU threads' run-queue wait could not be read: counted as run \
                 time)",
                s(self.wall_ms)
            ),
        }
    }
}

/// Which limit a VM ran into.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Over {
    /// The run time reached the scenario's limit.
    RunTime,
    /// The wall-clock ceiling ([`BACKSTOP`] times the limit).
    Backstop,
}

/// A running VM's clock.
pub struct VmClock {
    pid: u32,
    start: Instant,
    /// The last sample whose threads could be read (a process that has
    /// exited has none).
    last_starved: Option<u64>,
}

impl VmClock {
    pub fn new(pid: u32, start: Instant) -> Self {
        VmClock { pid, start, last_starved: None }
    }

    pub fn sample(&mut self) -> Sample {
        let wall_ms = self.start.elapsed().as_millis() as u64;
        if let Some(w) = max_vcpu_delay_ms(self.pid) {
            self.last_starved = Some(w);
        }
        Sample { wall_ms, starved_ms: self.last_starved }
    }
}

/// The largest run-queue wait (ms) among process `pid`'s vCPU threads.
fn max_vcpu_delay_ms(pid: u32) -> Option<u64> {
    let tasks = fs::read_dir(format!("/proc/{pid}/task")).ok()?;
    tasks
        .flatten()
        .filter(|t| fs::read_to_string(t.path().join("comm")).is_ok_and(|c| is_vcpu(&c)))
        .filter_map(|t| fs::read_to_string(t.path().join("schedstat")).ok())
        .filter_map(|s| run_delay_ns(&s))
        .max()
        .map(|ns| ns / 1_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedstat_and_names() {
        assert_eq!(run_delay_ns("1036000000 8984000000 2201\n"), Some(8_984_000_000));
        assert_eq!(run_delay_ns("12"), None);
        assert!(is_vcpu("CPU 0/TCG\n"));
        assert!(is_vcpu("CPU 1/KVM"));
        assert!(is_vcpu("ALL CPUs/TCG"));
        assert!(!is_vcpu("qemu-system-x86"));
        assert!(!is_vcpu("worker"));
    }

    #[test]
    fn limits() {
        let limit = Duration::from_secs(120);
        let at = |wall_ms, starved_ms| Sample { wall_ms, starved_ms };
        // An idle host: the wall time.
        assert_eq!(at(119_000, Some(500)).over(limit), None);
        assert_eq!(at(121_000, Some(500)).over(limit), Some(Over::RunTime));
        // Starved (sshd_shell at load 33, 2026-11-03: 58 s of 120): goes on.
        assert_eq!(at(130_000, Some(70_000)).over(limit), None);
        // Starved for good: the ceiling.
        assert_eq!(at(481_000, Some(470_000)).over(limit), Some(Over::Backstop));
        // Unreadable threads: the wall time.
        assert_eq!(at(121_000, None).over(limit), Some(Over::RunTime));
        assert!(at(130_000, Some(70_000)).describe().starts_with("60.0s run of 130.0s wall"));
    }

    /// This host's kernel keeps the counts xtask reads (a thread of this
    /// process stands in for a vCPU).
    #[test]
    fn this_host_has_schedstat() {
        let me = std::process::id();
        let any = fs::read_dir(format!("/proc/{me}/task"))
            .unwrap()
            .flatten()
            .filter_map(|t| fs::read_to_string(t.path().join("schedstat")).ok())
            .any(|s| run_delay_ns(&s).is_some());
        assert!(
            any || !std::path::Path::new("/proc/self/task").exists(),
            "no /proc/<pid>/task/<tid>/schedstat: VM limits fall back to wall time"
        );
    }
}
