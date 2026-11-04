//! The guest's clock against the host's (docs/OBSERVABILITY.md "Clock
//! checks"; ARCHITECTURE "Interrupts and time").
//!
//! The guest prints its uptime in lines carrying `uptime_ms=<n>` (the
//! kernel's RTC check, clockcal_test's marks). While the VM runs, a
//! thread of xtask's ([`Watcher`]) reads the serial console every 20 ms
//! and notes the window of host time in which each such line appeared;
//! after the run it compares, per source (the line's first `word:`, after
//! the log level: `clock` for the kernel, the program's name), the
//! guest's interval between the first and the last mark with the host's
//! possible interval and adds an `xtask: clock: <source>:` line to the
//! log, `xtask: clock: WRONG: <source>:` when the guest's lies more than
//! [`WRONG_PERCENT`] outside it.
//!
//! The reference is the host's monotonic clock: under QEMU's TCG (no
//! `-icount`) the guest's TSC, HPET, PM timer, LAPIC timer and RTC all
//! follow it, even when the VM is starved (measured 2026-11-04: the HPET
//! over the host 1.0000 with the VM's vCPU threads waiting for a host CPU
//! most of the time; WORKFLOW "Is the guest's clock right?"). A guest
//! keeps its virtual hardware's time; with `-icount` that would no longer
//! be the host's and the reference would have to be QEMU's virtual clock.
//!
//! `HostLoad` keeps the host's CPUs busy while a scenario that tests the
//! clock under load runs (`HOST_LOAD_SCENARIOS` in main.rs).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// The marker xtask looks for in the guest's lines.
pub const MARK: &str = "uptime_ms=";
/// The start of the line xtask adds when the clocks disagree.
pub const WRONG: &str = "xtask: clock: WRONG";
/// How far the guest's interval may lie outside the host's possible one
/// before xtask says so: the guest reads its clock before it writes the
/// line, and on a busy host the line's bytes reach the serial port a
/// little later.
pub const WRONG_PERCENT: f64 = 5.0;
/// Shorter host intervals are too noisy to judge.
pub const MIN_HOST_MS: u64 = 2000;
/// How often the watcher thread reads the serial log.
const POLL: Duration = Duration::from_millis(20);

/// A mark: its source, the host's window in which it appeared (ms since
/// the VM started: from the start of the read before the one that found
/// it to the end of that one) and the guest's uptime in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    pub source: String,
    pub host_lo: u64,
    pub host_hi: u64,
    pub guest: u64,
}

/// The marks seen so far.
#[derive(Default)]
pub struct ClockMarks {
    offset: u64,
    partial: Vec<u8>,
    marks: Vec<Mark>,
    /// When the previous read began.
    last_read_ms: u64,
}

impl ClockMarks {
    /// Reads what the serial log gained since the last call; the read
    /// began at `before_ms`, and `after` gives the time once it is done.
    /// A mark seen now appeared after the previous read began and before
    /// this one ended. (Timing a mark from the middle of the gap between
    /// two polls of a loop that also did other work was the cause of the
    /// false `clock: WRONG`s: console_window's screen check held that loop
    /// for tens of seconds under load, 2026-11-04.)
    pub fn poll(&mut self, log: &Path, before_ms: u64, after: impl FnOnce() -> u64) {
        let lo = self.last_read_ms;
        self.last_read_ms = before_ms;
        let Ok(mut f) = File::open(log) else { return };
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut new = Vec::new();
        let Ok(n) = f.read_to_end(&mut new) else { return };
        let hi = after();
        self.offset += n as u64;
        self.partial.extend_from_slice(&new);
        while let Some(end) = self.partial.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            if let Some(guest) = guest_ms(&line) {
                let source = source(&line).to_string();
                self.marks.push(Mark { source, host_lo: lo, host_hi: hi, guest });
            }
        }
    }

    /// The `xtask: clock:` lines for the log, one per source with two
    /// marks or more (None without any).
    pub fn verdict(&self) -> Option<String> {
        let mut out = String::new();
        let mut done: Vec<&str> = Vec::new();
        for m in &self.marks {
            if done.contains(&m.source.as_str()) {
                continue;
            }
            done.push(&m.source);
            let mine: Vec<_> = self.marks.iter().filter(|x| x.source == m.source).collect();
            if let (Some(first), Some(last)) = (mine.first(), mine.last())
                && mine.len() >= 2
            {
                out += &compare(
                    &m.source,
                    last.guest.saturating_sub(first.guest),
                    last.host_lo.saturating_sub(first.host_hi),
                    last.host_hi.saturating_sub(first.host_lo),
                    mine.len(),
                );
            }
        }
        (!out.is_empty()).then_some(out)
    }
}

/// Reads a VM's serial log for marks in a thread of its own, every
/// [`POLL`], so a mark's time is not that of xtask's main loop (which
/// waits for screen checks and network clients).
pub struct Watcher {
    marks: Arc<Mutex<ClockMarks>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    log: PathBuf,
    start: Instant,
}

impl Watcher {
    /// Watches `log`; times count from `start` (the VM's start).
    pub fn start(log: &Path, start: Instant) -> Self {
        let marks = Arc::new(Mutex::new(ClockMarks::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (marks, stop, log) = (marks.clone(), stop.clone(), log.to_path_buf());
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    poll_now(&marks, &log, start);
                    thread::sleep(POLL);
                }
            })
        };
        Watcher { marks, stop, thread: Some(thread), log: log.to_path_buf(), start }
    }

    /// Stops the thread, reads the rest of the log, gives the verdict.
    pub fn finish(mut self) -> Option<String> {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        poll_now(&self.marks, &self.log, self.start);
        self.marks.lock().ok()?.verdict()
    }
}

fn poll_now(marks: &Mutex<ClockMarks>, log: &Path, start: Instant) {
    let ms = || start.elapsed().as_millis() as u64;
    if let Ok(mut m) = marks.lock() {
        m.poll(log, ms(), ms);
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

/// The verdict on a guest interval against the host's, which lay between
/// `host_lo` and `host_hi` ms (from the marks' windows).
pub fn compare(source: &str, guest_ms: u64, host_lo: u64, host_hi: u64, marks: usize) -> String {
    let window = format!("host {host_lo}..{host_hi} ms");
    // A short interval is still clearly wrong when the guest's time is
    // off by far more than the noise.
    if host_hi < MIN_HOST_MS
        && guest_ms <= host_hi + MIN_HOST_MS / 2
        && guest_ms + MIN_HOST_MS / 2 >= host_lo
    {
        return format!(
            "xtask: clock: {source}: guest {guest_ms} ms over {window} ({marks} marks; \
             too short to judge)\n"
        );
    }
    let slack = WRONG_PERCENT / 100.0;
    let g = guest_ms as f64;
    if g < host_lo as f64 * (1.0 - slack) || g > host_hi as f64 * (1.0 + slack) {
        let nearest = if g < host_lo as f64 { host_lo } else { host_hi }.max(1);
        format!(
            "{WRONG}: {source}: the guest's time passed {:.2}x the host's (guest {guest_ms} ms \
             over {window}, {marks} marks): the clock was calibrated wrong (look at the `tsc:` \
             and `clock:` lines)\n",
            g / nearest as f64
        )
    } else {
        let mid = ((host_lo + host_hi) / 2).max(1) as f64;
        format!(
            "xtask: clock: {source}: guest {guest_ms} ms over {window} ({:+.1}% of the middle, \
             {marks} marks)\n",
            (g - mid) / mid * 100.0
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
        let ok = |g, lo, hi| compare("p", g, lo, hi, 2).starts_with("xtask: clock: p: guest");
        let wrong = |g, lo, hi| compare("p", g, lo, hi, 2).starts_with(WRONG);
        assert!(ok(5010, 5000, 5000));
        assert!(ok(5200, 5000, 5000));
        assert!(wrong(100_000, 5000, 5000));
        assert!(wrong(4000, 5000, 5000));
        assert!(compare("p", 30, 1000, 1000, 2).contains("too short"));
        // The fail-first boot: 77 s of guest time in 0.47 s.
        assert!(wrong(77617, 471, 471));
        // Inside a wide window: no verdict beyond what the window allows.
        assert!(ok(9914, 2000, 79000));
        // 5% outside the window's bounds, not its middle.
        assert!(ok(2423, 2104, 2508));
        assert!(wrong(2700, 2104, 2508));
        assert!(wrong(1900, 2104, 2508));
    }

    #[test]
    fn sources() {
        assert_eq!(source("[info ] clock: RTC check (uptime_ms=1)"), "clock");
        assert_eq!(source("clockcal_test: mark 1 uptime_ms=5"), "clockcal_test");
        assert_eq!(source("uptime_ms=5"), "?");
    }

    fn temp_log(name: &str) -> (PathBuf, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("gala-clockcheck-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("serial.log");
        (dir, log)
    }

    #[test]
    fn polls_follow_the_log() {
        let (dir, log) = temp_log("polls");
        std::fs::write(&log, "boot\nmark: uptime_ms=100\nk: uptime_ms=7\npart").unwrap();
        let mut m = ClockMarks::default();
        m.poll(&log, 990, || 1000);
        assert!(m.verdict().is_none());
        let text = "boot\nmark: uptime_ms=100\nk: uptime_ms=7\npartial mark: uptime_ms=5100\n";
        std::fs::write(&log, text).unwrap();
        m.poll(&log, 5990, || 6000);
        let at = |s: &str, host_lo, host_hi, guest| Mark {
            source: s.to_string(),
            host_lo,
            host_hi,
            guest,
        };
        assert_eq!(
            m.marks,
            [at("mark", 0, 1000, 100), at("k", 0, 1000, 7), at("partial mark", 990, 6000, 5100)]
        );
        assert!(m.verdict().is_none());
        std::fs::write(&log, format!("{text}mark: uptime_ms=6100\n")).unwrap();
        m.poll(&log, 6990, || 7000);
        let v = m.verdict().unwrap();
        assert!(v.starts_with("xtask: clock: mark: guest 6000 ms over host 4990..7000 ms"), "{v}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// console_window under load (2026-11-04): the main loop's screen
    /// check held it for over a minute between the kernel's two marks
    /// (`guest 9914 ms over host 39959 ms`, WRONG). The watcher's own
    /// thread times them while this one is busy.
    #[test]
    fn a_busy_caller_does_not_skew_the_marks() {
        let (dir, log) = temp_log("busy");
        std::fs::write(&log, "").unwrap();
        let start = Instant::now();
        let w = Watcher::start(&log, start);
        let append = |s: &str| {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
            f.write_all(s.as_bytes()).unwrap();
        };
        thread::sleep(Duration::from_millis(100));
        let t0 = start.elapsed().as_millis() as u64;
        append("[info ] clock: checking (uptime_ms=1000)\n");
        thread::sleep(Duration::from_millis(2500));
        let t1 = start.elapsed().as_millis() as u64;
        append(&format!("[info ] clock: agrees (uptime_ms={})\n", 1000 + t1 - t0));
        // The caller is busy for a while before it asks.
        thread::sleep(Duration::from_millis(1500));
        let v = w.finish().unwrap();
        assert!(v.starts_with("xtask: clock: clock: guest"), "{v}");
        assert!(!v.contains("too short"), "{v}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
