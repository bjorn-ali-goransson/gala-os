//! Triage of a boot's log (docs/OBSERVABILITY.md, "Failed logs: triage
//! and history"): what an agent reads first when a scenario failed. A
//! failed log ends with this block (`target/failed/<scenario>.log`), and
//! `cargo xtask triage <log> [events.jsonl]` prints it for any log;
//! `cargo xtask events <events.jsonl>` summarizes an event stream.
//!
//! Everything here is read from the text the kernel already prints (no
//! format of its own): the verdict lines (panic, CPU exception, watchdog,
//! leak, out of memory), the programs that started and did not end, the
//! process exits with an error status, the ledger, the state dump (the
//! threads' system calls and waits, held locks, the memory figures), the
//! flight recorder's and the event stream's last system calls per thread,
//! and the log's last lines.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// What xtask knows of the run besides its log.
#[derive(Default)]
pub struct Facts<'a> {
    pub scenario: &'a str,
    /// Why it failed (xtask's verdict), if it did.
    pub verdict: Option<&'a str>,
    pub secs: Option<f64>,
    /// "4 CPUs, load 1.2 0.8 0.5, 12.3 GiB free"
    pub host: Option<String>,
}

/// Lines of the log without the level prefix (`[info ] `).
fn text(line: &str) -> &str {
    let line = line.trim_end_matches('\r');
    match line.strip_prefix('[') {
        Some(rest) if rest.len() > 6 && rest.as_bytes()[5] == b']' => rest[6..].trim_start(),
        _ => line,
    }
}

/// The value of `"key":` in a JSON line (a string without its quotes, or
/// a number/array/literal as written).
pub fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\":");
    let at = line.find(&pat)? + pat.len();
    let rest = &line[at..];
    if let Some(s) = rest.strip_prefix('"') {
        let mut escaped = false;
        for (i, c) in s.char_indices() {
            match c {
                '\\' if !escaped => escaped = true,
                '"' if !escaped => return Some(&s[..i]),
                _ => escaped = false,
            }
        }
        None
    } else if rest.starts_with('[') {
        rest.find(']').map(|i| &rest[..=i])
    } else {
        let end = rest.find([',', '}']).unwrap_or(rest.len());
        Some(rest[..end].trim())
    }
}

/// The text after `marker` on a line, up to the end.
fn after<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    line.find(marker).map(|i| &line[i + marker.len()..])
}

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}...")
    }
}

/// Last system calls per thread: (thread id) -> names, oldest first.
type Calls = BTreeMap<u64, Vec<String>>;

fn push_call(calls: &mut Calls, tid: u64, call: String) {
    let v = calls.entry(tid).or_default();
    v.push(call);
    if v.len() > 6 {
        v.remove(0);
    }
}

/// From flight-recorder lines: `flight: t cpu c syscall a b (Name pid P tid T)`.
fn flight_calls(serial: &str) -> Calls {
    let mut calls = Calls::new();
    for line in serial.lines().map(text) {
        let Some(rest) = after(line, " syscall ") else { continue };
        if !line.starts_with("flight:") {
            continue;
        }
        let Some(inner) = rest.split_once('(').and_then(|(_, r)| r.strip_suffix(')')) else {
            continue;
        };
        let words: Vec<&str> = inner.split_whitespace().collect();
        if let [name, "pid", _pid, "tid", tid] = words[..]
            && let Ok(tid) = tid.parse()
        {
            push_call(&mut calls, tid, name.to_string());
        }
    }
    calls
}

/// From an event stream's `syscall` events (status in hex when not 0).
fn event_calls(events: &str) -> Calls {
    let mut calls = Calls::new();
    for line in events.lines().filter(|l| l.contains(r#""ev":"syscall""#)) {
        let (Some(tid), Some(name)) = (field(line, "tid"), field(line, "name")) else { continue };
        let Ok(tid) = tid.parse() else { continue };
        let status = field(line, "status").and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        let call =
            if status == 0 { name.to_string() } else { format!("{name}={:#x}", status as u32) };
        push_call(&mut calls, tid, call);
    }
    calls
}

/// `ledger: {...}` lines: (hits, "service what detail"), most hits first.
fn ledger(serial: &str) -> Vec<(u64, String)> {
    let mut merged: BTreeMap<String, u64> = BTreeMap::new();
    for line in serial.lines().map(text).filter_map(|l| l.strip_prefix("ledger: ")) {
        let hits = field(line, "hits").and_then(|h| h.parse().ok()).unwrap_or(1);
        let key = format!(
            "{} {} {}",
            field(line, "service").unwrap_or("?"),
            field(line, "what").unwrap_or("?"),
            field(line, "detail").unwrap_or("")
        );
        *merged.entry(key.trim_end().to_string()).or_default() += hits;
    }
    let mut v: Vec<(u64, String)> = merged.into_iter().map(|(k, h)| (h, k)).collect();
    v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    v
}

/// The NT threads of the last state or hang dump, one line each.
fn dumped_threads(serial: &str) -> Vec<String> {
    let lines: Vec<&str> = serial.lines().map(text).collect();
    // Only the last dump (a hang dump may have come a minute earlier).
    let start = lines
        .iter()
        .rposition(|l| {
            l.starts_with("statedump: threads") || l.contains("still running after a minute")
        })
        .unwrap_or(0);
    let mut out: Vec<String> = Vec::new();
    for line in &lines[start..] {
        if let Some(rest) = line.strip_prefix("thread: pid ") {
            // "4 tid 8 (sched ..) suspend 0 exited false in a system call true
            // session 1 (name)" (no session before RS B1)
            let mut words = rest.split_whitespace();
            let pid = words.next().unwrap_or("?");
            let tid = words.nth(1).unwrap_or("?");
            let name = rest
                .strip_suffix(')')
                .and_then(|r| r.rsplit_once('('))
                .map(|(_, n)| n)
                .filter(|n| !n.contains([' ', ')']))
                .unwrap_or("");
            let exited = rest.contains("exited true");
            let session = rest
                .split_once(" session ")
                .and_then(|(_, r)| r.split_whitespace().next())
                .map_or(String::new(), |s| format!(" session {s}"));
            // Where the thread started (its Win32 start routine): which
            // thread it is (a pool worker, the program's own, a helper).
            let start = rest
                .split_once(" start ")
                .and_then(|(_, r)| r.split_whitespace().next())
                .map_or(String::new(), |s| format!(" start {s}"));
            // Its name (SetThreadDescription), quoted as the kernel prints it.
            let named = rest
                .split_once(" name \"")
                .and_then(|(_, r)| r.split_once('"'))
                .map_or(String::new(), |(n, _)| format!(" \"{n}\""));
            out.push(format!(
                "pid {pid} tid {tid}{named}{}{start}{session}{}:",
                if name.is_empty() { String::new() } else { format!(" ({name})") },
                if exited { " (exited)" } else { "" }
            ));
        } else if let Some(rest) = line.strip_prefix("thread:   ")
            && let Some(last) = out.last_mut()
        {
            write!(last, " {}", rest.trim()).ok();
        }
    }
    out
}

/// The triage block for a log (and its event stream, if any).
pub fn triage(serial: &str, events: Option<&str>, facts: &Facts) -> String {
    let mut o = String::new();
    let lines: Vec<&str> = serial.lines().collect();
    let plain: Vec<&str> = lines.iter().map(|l| text(l)).collect();
    o += "===== triage (xtask; `cargo xtask triage <log>` prints this for any log) =====\n";
    let mut head = format!(
        "triage: scenario {}",
        if facts.scenario.is_empty() { "?" } else { facts.scenario }
    );
    if let Some(v) = facts.verdict {
        write!(head, ": {}", shorten(v, 300)).ok();
    }
    if let Some(s) = facts.secs {
        write!(head, " (ran {s:.1}s)").ok();
    }
    o += &head;
    o += "\n";
    // A hot-swapped run (ROADMAP SN step 1d): first, so no one reads the
    // rest as a real run's.
    let swapped: Vec<&str> = plain.iter().filter_map(|l| after(l, "hotswap: applied: ")).collect();
    if !swapped.is_empty() {
        writeln!(
            o,
            "triage: HOT-SWAPPED, NOT A REAL RUN (DLLs of another build spliced into running \
             programs; at your own risk): {}",
            swapped.join("; ")
        )
        .ok();
    } else if let Some(r) = plain.iter().find_map(|l| after(l, "hotswap: refused: ")) {
        writeln!(o, "triage: hot swap asked for and refused (not a real run): {r}").ok();
    }
    if let Some(h) = &facts.host {
        writeln!(o, "triage: host: {h}").ok();
    }
    // The guest: CPUs, memory.
    let cpus =
        plain.iter().find_map(|l| after(l, "smp: ")).map(|r| r.split(" (").next().unwrap_or(r));
    let memory = plain
        .iter()
        .rev()
        .find_map(|l| l.strip_prefix("statedump: heap ").map(|r| format!("heap {r}")))
        .or_else(|| {
            plain.iter().rev().find_map(|l| {
                l.strip_prefix("stats: ")
                    .and_then(|r| r.split_once("heap "))
                    .map(|(_, r)| format!("heap {r}"))
            })
        });
    writeln!(
        o,
        "triage: guest: {}; {}",
        cpus.unwrap_or("CPUs unknown (no smp line: early boot?)"),
        memory.unwrap_or_else(|| "no memory figures (no stats or state dump line)".into())
    )
    .ok();
    // A clock calibrated wrong (the host busy during the PIT interval):
    // every guest time, timeout and watchdog is off by that factor.
    if let Some(mhz) = plain
        .iter()
        .find_map(|l| after(l, "apic: ").and_then(|r| after(r, ", TSC ")))
        .and_then(|r| r.split_whitespace().next()?.parse::<u64>().ok())
        .filter(|&mhz| mhz < 500)
    {
        writeln!(
            o,
            "triage: suspicious clock: the TSC was calibrated at {mhz} MHz (a busy host during \
             calibration?): guest times run fast, so timeouts and watchdogs fire early"
        )
        .ok();
    }
    // The clock checked against the host (xtask's `uptime_ms=` timing,
    // clockcheck.rs) and against the RTC (the kernel's check, which
    // recalibrates): either saying WRONG explains early timeouts.
    for l in plain
        .iter()
        .filter(|l| l.starts_with(crate::clockcheck::WRONG) || l.starts_with("clock: WRONG"))
    {
        writeln!(o, "triage: clock WRONG: {}", shorten(l, 300)).ok();
    }
    if let Some(l) = plain.iter().find(|l| l.starts_with("clock: recalibrated")) {
        writeln!(o, "triage: clock: {}", shorten(l, 200)).ok();
    }
    // How much of the wall time the VM could run (vmtime.rs): a timeout
    // with most of it spent waiting for a host CPU was the ceiling's.
    if let Some(l) = plain.iter().find_map(|l| l.strip_prefix("xtask: vm time: ")) {
        writeln!(o, "triage: vm time: {}", shorten(l, 200)).ok();
    }
    let warnings = lines.iter().filter(|l| l.starts_with("[warn ")).count();
    let errors = lines.iter().filter(|l| l.starts_with("[error")).count();
    writeln!(o, "triage: log: {} lines, {warnings} warnings, {errors} errors", lines.len()).ok();
    if let Some(e) = lines.iter().find(|l| l.starts_with("[error")) {
        writeln!(o, "triage: first error: {}", shorten(text(e), 200)).ok();
    }
    // How it ended.
    const VERDICTS: &[(&str, &str)] = &[
        ("KERNEL PANIC", "panic"),
        ("CPU exception:", "kernel CPU exception"),
        ("watchdog:", "watchdog"),
        ("address space still referenced", "leak"),
        ("vm: out of memory", "out of memory"),
        ("not implemented, ending the process", "missing function"),
        ("unimplemented system service", "missing service"),
        ("user exception:", "unhandled user exception"),
        ("crash report: ", "program crash"),
        ("crash: a record from the previous boot", "crash record (earlier boot)"),
    ];
    for (marker, what) in VERDICTS {
        let hits: Vec<&&str> = plain.iter().filter(|l| l.contains(marker)).collect();
        if let Some(first) = hits.first() {
            let more = if hits.len() > 1 {
                format!(" (and {} more)", hits.len() - 1)
            } else {
                String::new()
            };
            writeln!(o, "triage: {what}: {}{more}", shorten(first, 240)).ok();
        }
    }
    if let Some(i) = plain.iter().position(|l| l.contains("KERNEL PANIC"))
        && let Some(bt) = plain[i..].iter().find(|l| l.starts_with("backtrace:"))
    {
        writeln!(
            o,
            "triage: panic {} (symbolized above, if addr2line is installed)",
            shorten(bt, 120)
        )
        .ok();
    }
    // Programs that started and did not end, and processes that failed.
    let mut open: Vec<&str> = Vec::new();
    for l in &plain {
        if let Some(p) = l.strip_prefix("run: begin ") {
            open.push(p);
        } else if let Some(p) = l.strip_prefix("run: end ")
            && let Some(i) = open.iter().position(|o| *o == p)
        {
            open.remove(i);
        }
    }
    if !open.is_empty() {
        writeln!(o, "triage: stopped in: {} (run: begin without run: end)", open.join(", ")).ok();
    }
    let failed: Vec<String> = plain
        .iter()
        .filter_map(|l| {
            let rest = l.strip_prefix("process ")?;
            let (who, status) = rest.split_once(" exited with status ")?;
            (status.trim() != "0x00000000").then(|| format!("{who} {}", status.trim()))
        })
        .collect();
    if !failed.is_empty() {
        let shown: Vec<&str> = failed.iter().take(6).map(String::as_str).collect();
        writeln!(o, "triage: processes that failed: {}", shown.join("; ")).ok();
    }
    let missing = ledger(serial);
    if !missing.is_empty() {
        let top: Vec<String> = missing.iter().take(5).map(|(h, k)| format!("{k} x{h}")).collect();
        writeln!(
            o,
            "triage: missing (ledger, top {} of {}): {}",
            top.len(),
            missing.len(),
            top.join("; ")
        )
        .ok();
    }
    // The state dump (xtask asked on a timeout) or the hang dump.
    if let Some(begin) = plain.iter().find(|l| l.starts_with("statedump: begin")) {
        let locks =
            plain.iter().rev().find_map(|l| l.strip_prefix("statedump: locks: ")).unwrap_or("?");
        writeln!(
            o,
            "triage: state dump: {} ; locks: {locks}",
            begin.trim_start_matches("statedump: ")
        )
        .ok();
        for l in
            plain.iter().filter(|l| l.starts_with("statedump: cpu ") && !l.contains("backtrace"))
        {
            writeln!(o, "triage:   {}", l.trim_start_matches("statedump: ")).ok();
        }
        if !plain.iter().any(|l| l.starts_with("statedump: end")) {
            o += "triage:   (no \"statedump: end\": the threads part did not come; the boot thread could not run)\n";
        }
    } else if facts.verdict.is_some_and(|v| v.contains("timed out")) {
        o += "triage: state dump: none (timed out without one: no NMI handler yet, or the guest is wedged)\n";
    }
    for t in dumped_threads(serial).iter().take(24) {
        writeln!(o, "triage: thread {}", shorten(t, 240)).ok();
    }
    // Each thread's last system calls: the event stream when it was on,
    // else the flight recorder (a panic's or a state dump's).
    let (calls, source) = match events.map(event_calls).filter(|c| !c.is_empty()) {
        Some(c) => (c, "events"),
        None => (flight_calls(serial), "flight recorder"),
    };
    for (tid, names) in calls.iter().take(16) {
        writeln!(o, "triage: last calls of tid {tid} ({source}): {}", names.join(", ")).ok();
    }
    if let Some(events) = events {
        let faults: Vec<&str> = events.lines().filter(|l| l.contains(r#""ev":"fault""#)).collect();
        if let Some(last) = faults.last() {
            writeln!(
                o,
                "triage: user faults (events): {}; last: pid {} vector {} rip {} cr2 {}",
                faults.len(),
                field(last, "pid").unwrap_or("?"),
                field(last, "vector").unwrap_or("?"),
                field(last, "rip")
                    .and_then(|r| r.parse::<u64>().ok())
                    .map_or("?".into(), |r| format!("{r:#x}")),
                field(last, "cr2")
                    .and_then(|r| r.parse::<u64>().ok())
                    .map_or("?".into(), |r| format!("{r:#x}")),
            )
            .ok();
        }
    }
    // The end of the log, without the dumps' own lines.
    let tail: Vec<&str> = plain
        .iter()
        .copied()
        .filter(|l| {
            !l.trim().is_empty()
                && !l.starts_with("flight:")
                && !l.starts_with("statedump:")
                && !l.starts_with("thread:")
                && !l.starts_with("sched:")
                && !l.starts_with("ledger:")
                && !l.starts_with("xtask:")
        })
        .collect();
    o += "triage: last lines:\n";
    for l in &tail[tail.len().saturating_sub(12)..] {
        writeln!(o, "triage:   {}", shorten(l, 200)).ok();
    }
    o += "===== end of triage =====\n";
    o
}

/// `cargo xtask events`: an event stream's summary: events by kind,
/// system calls by count and time, failing statuses, each thread's last
/// calls, faults, processes and images.
pub fn events_summary(events: &str) -> String {
    let mut o = String::new();
    let mut kinds: BTreeMap<&str, u64> = BTreeMap::new();
    // name -> (calls, ns)
    let mut services: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
    // (name, status) -> count
    let mut failures: BTreeMap<(&str, u64), u64> = BTreeMap::new();
    let mut images: Vec<String> = Vec::new();
    let mut processes: Vec<String> = Vec::new();
    let mut last_t = 0u64;
    for line in events.lines() {
        let Some(ev) = field(line, "ev") else { continue };
        *kinds.entry(ev).or_default() += 1;
        last_t = last_t.max(field(line, "t").and_then(|t| t.parse().ok()).unwrap_or(0));
        match ev {
            "syscall" => {
                let name = field(line, "name").unwrap_or("?");
                let ns = field(line, "ns").and_then(|n| n.parse().ok()).unwrap_or(0);
                let e = services.entry(name).or_default();
                e.0 += 1;
                e.1 += ns;
                let status = field(line, "status").and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
                // Errors and warnings (0x8..., 0xC...), not informational.
                if status >= 0x8000_0000 {
                    *failures.entry((name, status)).or_default() += 1;
                }
            }
            "image" => images.push(format!(
                "pid {} {} at {}",
                field(line, "pid").unwrap_or("?"),
                field(line, "path").or(field(line, "name")).unwrap_or("?"),
                field(line, "base")
                    .and_then(|b| b.parse::<u64>().ok())
                    .map_or("?".into(), |b| format!("{b:#x}"))
            )),
            e if e.starts_with("process") => processes.push(format!(
                "{e} pid {}{}",
                field(line, "pid").unwrap_or("?"),
                field(line, "status")
                    .and_then(|s| s.parse::<u64>().ok())
                    .map_or(String::new(), |s| format!(" status {:#010x}", s as u32))
            )),
            _ => {}
        }
    }
    writeln!(
        o,
        "events: {} lines over {:.3}s of guest time",
        events.lines().count(),
        last_t as f64 / 1e9
    )
    .ok();
    let kinds: Vec<String> = kinds.iter().map(|(k, n)| format!("{k} {n}")).collect();
    writeln!(o, "events by kind: {}", kinds.join(", ")).ok();
    let mut by_time: Vec<(&&str, &(u64, u64))> = services.iter().collect();
    by_time.sort_by_key(|e| std::cmp::Reverse(e.1.1));
    o += "system calls by time (calls, total ms, mean us):\n";
    for (name, (n, ns)) in by_time.iter().take(15) {
        writeln!(
            o,
            "  {name:<36} {n:>8} {:>10.1} {:>8.1}",
            *ns as f64 / 1e6,
            *ns as f64 / 1e3 / *n as f64
        )
        .ok();
    }
    let mut failures: Vec<_> = failures.into_iter().collect();
    failures.sort_by_key(|e| std::cmp::Reverse(e.1));
    if !failures.is_empty() {
        o += "failing statuses (service, status, count):\n";
        for ((name, status), n) in failures.iter().take(15) {
            writeln!(o, "  {name:<36} {:#010x} {n:>6}", *status as u32).ok();
        }
    }
    let calls = event_calls(events);
    for (tid, names) in &calls {
        writeln!(o, "last calls of tid {tid}: {}", names.join(", ")).ok();
    }
    let faults: Vec<&str> = events.lines().filter(|l| l.contains(r#""ev":"fault""#)).collect();
    if !faults.is_empty() {
        writeln!(o, "faults: {} (last: {})", faults.len(), faults.last().copied().unwrap_or(""))
            .ok();
    }
    for p in processes.iter().take(40) {
        writeln!(o, "{p}").ok();
    }
    for i in images.iter().take(60) {
        writeln!(o, "image: {i}").ok();
    }
    if events.lines().count() == 0 {
        o += "(empty: the stream is off unless the kernel has events=...; `XTASK_EVENTS=all cargo xtask boot-test <name>`)\n";
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIMED_OUT: &str = "\
[info ] smp: 2 CPUs online (2 reported by the bootloader)
[info ] run: begin java.exe
[warn ] java.exe: kernel32.dll!Foo not implemented, ending the process
[error] net: something broke
[info ] process javac.exe (pid 5) exited with status 0xc0000005
[info ] process helper.exe (pid 6) exited with status 0x00000000
[info ] ledger: {\"service\":\"NtQuerySystemInformation\",\"what\":\"info class\",\"detail\":\"0x99\",\"hits\":7}
[info ] ledger: {\"service\":\"?\",\"what\":\"unknown service\",\"detail\":\"0x1ff\",\"hits\":2}

statedump: begin (NMI on CPU 0; CPUs answered 0x3 of online 0x3; now 20794 ms since boot)
statedump: cpu 0 kernel rip 0xffffffff800acf32 rsp 0xffffd0000001ffd0
statedump: cpu 0 backtrace: 0xffffffff800acf32 0xffffffff800abc74
statedump: cpu 1 user rip 0x1400010b9 rsp 0x2ffd0
statedump: locks: scheduler free, heap HELD, frames free, console free
statedump: heap 881 KiB in use (peak 908 KiB, 207 large pages); frames free 121338 of 131072 (fewest free 1200)
flight: 249135280 cpu 0 syscall 0x400000048 0x8 (NtCreateEvent pid 4 tid 8)
flight: 251557318 cpu 0 syscall 0x400000004 0x8 (NtWaitForSingleObject pid 4 tid 8)
flight: 251557319 cpu 1 syscall-done 0x4 0x102 (NtWaitForSingleObject status 0x00000102)
statedump: NMI part done; the threads follow from the boot thread if it still runs
[info ] statedump: threads (NT, then scheduler), the ledger, the statistics:
[info ] thread: pid 4 tid 8 (sched Some(ThreadId(5))) suspend 0 exited false in a system call true (java.exe)
[info ] thread:   NtWaitForSingleObject from rip 0x1400010de
[info ] thread:   on handle 0x14: Event signal 0
[info ] thread: pid 4 tid 12 (sched Some(ThreadId(6))) start 0x140001000 name \"worker\" suspend 0 exited false in a system call false session 1 (java.exe)
[info ] thread:   in user mode at rip 0x1400010b9 (last tick)
[info ] statedump: end
";

    #[test]
    fn a_timed_out_log_is_triaged_from_its_dump() {
        let facts = Facts {
            scenario: "java_http",
            verdict: Some("timed out after 300s"),
            secs: Some(301.0),
            host: Some("4 CPUs, load 11.0".into()),
        };
        let t = triage(TIMED_OUT, None, &facts);
        let has = |s: &str| assert!(t.contains(s), "missing {s:?} in\n{t}");
        has("triage: scenario java_http: timed out after 300s (ran 301.0s)");
        has("triage: host: 4 CPUs, load 11.0");
        has("triage: guest: 2 CPUs online; heap 881 KiB in use (peak 908 KiB");
        has("fewest free 1200");
        has("1 warnings, 1 errors");
        has("triage: first error: net: something broke");
        has("triage: missing function: java.exe: kernel32.dll!Foo not implemented");
        has("triage: stopped in: java.exe");
        has("triage: processes that failed: javac.exe (pid 5) 0xc0000005");
        assert!(!t.lines().any(|l| l.contains("failed:") && l.contains("helper.exe")));
        has(
            "triage: missing (ledger, top 2 of 2): NtQuerySystemInformation info class 0x99 x7; ? unknown service 0x1ff x2",
        );
        has("locks: scheduler free, heap HELD");
        has("triage:   cpu 1 user rip 0x1400010b9");
        has(
            "triage: thread pid 4 tid 8 (java.exe): NtWaitForSingleObject from rip 0x1400010de on handle 0x14: Event signal 0",
        );
        has(
            "triage: thread pid 4 tid 12 \"worker\" (java.exe) start 0x140001000 session 1: in user mode at rip 0x1400010b9 (last tick)",
        );
        has("triage: last calls of tid 8 (flight recorder): NtCreateEvent, NtWaitForSingleObject");
        assert!(!t.contains("no \"statedump: end\""));
    }

    #[test]
    fn a_clock_calibrated_wrong_is_pointed_out() {
        let log = "[info ] apic: id 0 (xAPIC), timer 3043 kHz, TSC 135 MHz\n";
        assert!(
            triage(log, None, &Facts::default())
                .contains("suspicious clock: the TSC was calibrated at 135 MHz")
        );
        let log = "[info ] apic: id 0 (xAPIC), timer 62748 kHz, TSC 2811 MHz\n";
        assert!(!triage(log, None, &Facts::default()).contains("suspicious clock"));
        let log = "[error] clock: WRONG: the TSC was calibrated at 140.0 MHz\n\
                   [warn ] clock: recalibrated: TSC 2800.0 MHz\n\
                   xtask: clock: WRONG: clock: the guest's time passed 20.00x the host's\n";
        let t = triage(log, None, &Facts::default());
        assert!(t.contains("triage: clock WRONG: clock: WRONG: the TSC was calibrated"), "{t}");
        assert!(t.contains("triage: clock WRONG: xtask: clock: WRONG: clock: the guest's"), "{t}");
        assert!(t.contains("triage: clock: clock: recalibrated: TSC 2800.0 MHz"), "{t}");
    }

    #[test]
    fn a_timeout_without_a_dump_says_so() {
        let facts = Facts { verdict: Some("timed out after 120s"), ..Facts::default() };
        let log = "[info ] run: begin a.exe\nhello\nxtask: vm time: 121.0s run of 130.2s wall \
                   (9.2s waiting for a host CPU) (limit 120s of run time)\n";
        let t = triage(log, None, &facts);
        assert!(t.contains("triage: vm time: 121.0s run of 130.2s wall"), "{t}");
        assert!(t.contains("state dump: none"), "{t}");
        assert!(t.contains("CPUs unknown"), "{t}");
        assert!(t.contains("triage:   hello"), "{t}");
    }

    #[test]
    fn a_panic_is_named_with_its_backtrace() {
        let log = "\nKERNEL PANIC: memory allocation of 64 bytes failed\n\nbacktrace: 0xffffffff80001000 0xffffffff80002000\n";
        let t = triage(log, None, &Facts::default());
        assert!(
            t.contains("triage: panic: KERNEL PANIC: memory allocation of 64 bytes failed"),
            "{t}"
        );
        assert!(t.contains("triage: panic backtrace: 0xffffffff80001000"), "{t}");
    }

    const EVENTS: &str = r#"{"t":1000,"cpu":0,"ev":"process start","pid":4}
{"t":2000,"cpu":0,"ev":"image","pid":4,"path":"\\hello.exe","base":5368709120,"size":4096}
{"t":3000,"cpu":0,"ev":"syscall","pid":4,"tid":8,"nr":51,"name":"NtOpenFile","args":[1,2],"status":3221225524,"ns":1500}
{"t":4000,"cpu":1,"ev":"syscall","pid":4,"tid":8,"nr":8,"name":"NtWriteFile","args":[1],"status":0,"ns":500}
{"t":5000,"cpu":1,"ev":"fault","pid":4,"vector":14,"rip":5368713216,"cr2":0,"error":4}
{"t":6000,"cpu":0,"ev":"process exit","pid":4,"status":3221225477}
"#;

    #[test]
    fn json_fields_are_read() {
        let l = r#"{"t":1,"ev":"image","path":"a\"b","args":[1,2],"n":5}"#;
        assert_eq!(field(l, "t"), Some("1"));
        assert_eq!(field(l, "path"), Some(r#"a\"b"#));
        assert_eq!(field(l, "args"), Some("[1,2]"));
        assert_eq!(field(l, "n"), Some("5"));
        assert_eq!(field(l, "x"), None);
    }

    #[test]
    fn events_are_summarized_and_used_for_the_last_calls() {
        let s = events_summary(EVENTS);
        assert!(s.contains("syscall 2"), "{s}");
        assert!(s.contains("NtOpenFile"), "{s}");
        assert!(s.contains("0xc0000034"), "{s}");
        assert!(s.contains("last calls of tid 8: NtOpenFile=0xc0000034, NtWriteFile"), "{s}");
        assert!(s.contains("faults: 1"), "{s}");
        assert!(s.contains("process exit pid 4 status 0xc0000005"), "{s}");
        assert!(s.contains("image: pid 4"), "{s}");
        let t = triage("", Some(EVENTS), &Facts::default());
        assert!(
            t.contains("last calls of tid 8 (events): NtOpenFile=0xc0000034, NtWriteFile"),
            "{t}"
        );
        assert!(
            t.contains("user faults (events): 1; last: pid 4 vector 14 rip 0x140001000"),
            "{t}"
        );
    }
}
