//! The guest's clock against the host's (docs/OBSERVABILITY.md "Clock
//! checks"; ARCHITECTURE "Interrupts and time").
//!
//! The guest prints its uptime in lines carrying `uptime_ms=<n>` (the
//! kernel's RTC check, clockcal_test's marks). While the VM runs, xtask
//! notes when each such line appears on the serial console; after the run
//! it compares, per source (the line's first `word:`, after the log
//! level: `clock` for the kernel, the program's name), the guest's
//! interval between the first and the last mark with the host's and adds
//! an `xtask: clock: <source>:` line to the log, `xtask: clock: WRONG:
//! <source>:` when they disagree by more than [`WRONG_PERCENT`].
//!
//! `HostLoad` keeps the host's CPUs busy while a scenario that tests the
//! clock under load runs (`HOST_LOAD_SCENARIOS` in main.rs).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

/// The marker xtask looks for in the guest's lines.
pub const MARK: &str = "uptime_ms=";
/// The start of the line xtask adds when the clocks disagree.
pub const WRONG: &str = "xtask: clock: WRONG";
/// How far apart the clocks may drift before xtask says so (the guest's
/// output can reach the serial port late on a busy host).
pub const WRONG_PERCENT: f64 = 5.0;
/// Shorter host intervals are too noisy to judge (xtask polls the log
/// every 50 ms).
pub const MIN_HOST_MS: u64 = 2000;

/// The marks seen so far: (host ms since the VM started, guest ms).
#[derive(Default)]
pub struct ClockMarks {
    offset: u64,
    partial: Vec<u8>,
    /// (source, host ms, guest ms).
    marks: Vec<(String, u64, u64)>,
    last_poll_ms: u64,
}

impl ClockMarks {
    /// Reads what the serial log gained since the last call; a mark seen
    /// now appeared between the last poll and `now_ms` (the midpoint is
    /// taken).
    pub fn poll(&mut self, log: &Path, now_ms: u64) {
        let seen_ms = (self.last_poll_ms + now_ms) / 2;
        self.last_poll_ms = now_ms;
        let Ok(mut f) = File::open(log) else { return };
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut new = Vec::new();
        let Ok(n) = f.read_to_end(&mut new) else { return };
        self.offset += n as u64;
        self.partial.extend_from_slice(&new);
        while let Some(end) = self.partial.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            if let Some(guest) = guest_ms(&line) {
                self.marks.push((source(&line).to_string(), seen_ms, guest));
            }
        }
    }

    /// The `xtask: clock:` lines for the log, one per source with two
    /// marks or more (None without any).
    pub fn verdict(&self) -> Option<String> {
        let mut out = String::new();
        let mut done: Vec<&str> = Vec::new();
        for (src, _, _) in &self.marks {
            if done.contains(&src.as_str()) {
                continue;
            }
            done.push(src);
            let mine: Vec<_> = self.marks.iter().filter(|m| m.0 == *src).collect();
            if let (Some(first), Some(last)) = (mine.first(), mine.last())
                && mine.len() >= 2
            {
                out += &compare(
                    src,
                    last.2.saturating_sub(first.2),
                    last.1.saturating_sub(first.1),
                    mine.len(),
                );
            }
        }
        (!out.is_empty()).then_some(out)
    }
}

/// A line's source: its first `word:` (after a `[level]` prefix).
pub fn source(line: &str) -> &str {
    let line = line.trim_start();
    let line = match line.strip_prefix('[') {
        Some(rest) => rest.split_once(']').map_or(rest, |(_, r)| r).trim_start(),
        None => line,
    };
    line.split_once(':').map_or("?", |(w, _)| w).trim()
}

/// The guest's uptime in a line carrying `uptime_ms=<n>`.
pub fn guest_ms(line: &str) -> Option<u64> {
    let rest = &line[line.find(MARK)? + MARK.len()..];
    let digits: &str = &rest[..rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len())];
    digits.parse().ok()
}

/// The verdict on a guest interval against the host's.
pub fn compare(source: &str, guest_ms: u64, host_ms: u64, marks: usize) -> String {
    // A short interval is still clearly wrong when the guest's time is
    // off by far more than the polls' noise.
    if host_ms < MIN_HOST_MS && guest_ms.abs_diff(host_ms) <= MIN_HOST_MS / 2 {
        return format!(
            "xtask: clock: {source}: guest {guest_ms} ms over host {host_ms} ms ({marks} marks; \
             too short to judge)\n"
        );
    }
    let off = (guest_ms as f64 - host_ms as f64) / host_ms as f64 * 100.0;
    if off.abs() > WRONG_PERCENT {
        format!(
            "{WRONG}: {source}: the guest's time passed {:.2}x the host's (guest {guest_ms} ms over host \
             {host_ms} ms, {marks} marks): the clock was calibrated wrong (look at the `tsc:` and \
             `clock:` lines)\n",
            guest_ms as f64 / host_ms as f64
        )
    } else {
        format!(
            "xtask: clock: {source}: guest {guest_ms} ms over host {host_ms} ms ({off:+.1}%, \
             {marks} marks)\n"
        )
    }
}

/// Busy threads on every host CPU until dropped.
pub struct HostLoad {
    stop: Arc<AtomicBool>,
    threads: Vec<thread::JoinHandle<()>>,
}

impl HostLoad {
    pub fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let n = thread::available_parallelism().map_or(4, |n| n.get());
        let threads = (0..n)
            .map(|_| {
                let stop = stop.clone();
                thread::spawn(move || {
                    let mut x = 1u64;
                    while !stop.load(Ordering::Relaxed) {
                        for _ in 0..100_000 {
                            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
                        }
                        std::hint::black_box(x);
                    }
                })
            })
            .collect();
        eprintln!("xtask: host load: {n} busy threads while the scenario runs");
        thread::sleep(Duration::from_millis(10));
        HostLoad { stop, threads }
    }
}

impl Drop for HostLoad {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_are_found() {
        assert_eq!(guest_ms("clockcal_test: mark 1 uptime_ms=1234\r"), Some(1234));
        assert_eq!(guest_ms("[info ] clock: RTC check (uptime_ms=99)"), Some(99));
        assert_eq!(guest_ms("nothing here"), None);
        assert_eq!(guest_ms("uptime_ms=x"), None);
    }

    #[test]
    fn verdicts() {
        assert!(compare("p", 5010, 5000, 2).starts_with("xtask: clock: p: guest 5010 ms"));
        assert!(compare("p", 5200, 5000, 2).starts_with("xtask: clock: p: guest"));
        assert!(compare("p", 100_000, 5000, 2).starts_with("xtask: clock: WRONG: p: "));
        assert!(compare("p", 4000, 5000, 2).starts_with(WRONG));
        assert!(compare("p", 30, 1000, 2).contains("too short"));
        // The fail-first boot: 77 s of guest time in 0.47 s.
        assert!(compare("p", 77617, 471, 2).starts_with(WRONG));
    }

    #[test]
    fn sources() {
        assert_eq!(source("[info ] clock: RTC check (uptime_ms=1)"), "clock");
        assert_eq!(source("clockcal_test: mark 1 uptime_ms=5"), "clockcal_test");
        assert_eq!(source("uptime_ms=5"), "?");
    }

    #[test]
    fn polls_follow_the_log() {
        let dir = std::env::temp_dir().join(format!("gala-clockcheck-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("serial.log");
        std::fs::write(&log, "boot\nmark: uptime_ms=100\nk: uptime_ms=7\npart").unwrap();
        let mut m = ClockMarks::default();
        m.poll(&log, 1000);
        assert!(m.verdict().is_none());
        let text = "boot\nmark: uptime_ms=100\nk: uptime_ms=7\npartial mark: uptime_ms=5100\n";
        std::fs::write(&log, text).unwrap();
        m.poll(&log, 6000);
        let at = |s: &str, h, g| (s.to_string(), h, g);
        assert_eq!(
            m.marks,
            [at("mark", 500, 100), at("k", 500, 7), at("partial mark", 3500, 5100)]
        );
        assert!(m.verdict().is_none());
        std::fs::write(&log, format!("{text}mark: uptime_ms=6100\n")).unwrap();
        m.poll(&log, 7000);
        let v = m.verdict().unwrap();
        assert!(v.starts_with("xtask: clock: mark: guest 6000 ms over host 6000 ms"), "{v}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
