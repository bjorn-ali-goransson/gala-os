//! `cargo xtask profile`: the sampling profiler's report. The kernel's
//! `sample` events (timer ticks of busy CPUs: the interrupted thread, its
//! mode and instruction pointer; docs/OBSERVABILITY.md "Event stream")
//! are attributed to processes and threads (from `process start` and
//! `thread create`), user addresses to the modules mapped at them (from
//! `image` events) and, given a directory of the modules' files and
//! symbols, to functions (llvm-symbolizer). Answers "where does the time
//! go" for a slow program, per thread.

use std::collections::HashMap;
use std::fmt::Write;

use crate::otel::parse_json;

/// What to report on.
#[derive(Default)]
pub struct Options {
    pub pid: Option<u64>,
    pub tid: Option<u64>,
    /// Only samples in [from, to) seconds since boot.
    pub window: Option<(f64, f64)>,
    /// How many lines each table keeps.
    pub top: usize,
}

/// Turns (module file name, offsets) into function names (None: unknown),
/// in the same order.
pub type Symbolizer<'a> = &'a dyn Fn(&str, &[u64]) -> Vec<Option<String>>;

struct Image {
    base: u64,
    size: u64,
    name: String,
}

/// The last component of an NT or Win32 path.
fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// What a process is: its name and, for Chromium, its `--type` and
/// utility sub-type.
fn kind(name: &str, cmdline: &str) -> String {
    let arg = |key: &str| {
        cmdline.split_whitespace().find_map(|w| w.strip_prefix(key)).map(|v| v.to_string())
    };
    match (arg("--type="), arg("--utility-sub-type=")) {
        (Some(t), Some(u)) => format!("{name} {t} {}", u.rsplit('.').next().unwrap_or(&u)),
        (Some(t), None) => format!("{name} {t}"),
        _ => name.to_string(),
    }
}

fn pct(n: usize, total: usize) -> f64 {
    if total == 0 { 0.0 } else { n as f64 * 100.0 / total as f64 }
}

/// Sorted by count, largest first, at most `top`.
fn ranked<K: Clone + Ord>(map: &HashMap<K, usize>, top: usize) -> Vec<(K, usize)> {
    let mut v: Vec<(K, usize)> = map.iter().map(|(k, n)| (k.clone(), *n)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.truncate(top);
    v
}

/// The report for the samples in `events` (an event stream, one JSON
/// object per line).
pub fn report(events: &str, o: &Options, symbolize: Symbolizer) -> String {
    let top = if o.top == 0 { 15 } else { o.top };
    let mut thread_pid: HashMap<u64, u64> = HashMap::new();
    let mut process: HashMap<u64, String> = HashMap::new();
    let mut images: HashMap<u64, Vec<Image>> = HashMap::new();
    // (t, tid, user, rip)
    let mut samples: Vec<(u64, u64, bool, u64)> = Vec::new();
    // A kernel sample's return addresses, by its index in `samples`.
    let mut stacks: HashMap<usize, Vec<u64>> = HashMap::new();
    // Samples lost after a kernel sample (its CPU's ring overflowed: the
    // interrupts-off stretch it was in went on), by its index.
    let mut lost: HashMap<usize, u64> = HashMap::new();
    for line in events.lines() {
        let Ok(ev) = parse_json(line.trim()) else { continue };
        match ev.str("ev") {
            Some("thread create") => {
                if let (Some(t), Some(p)) = (ev.u64("tid"), ev.u64("pid")) {
                    thread_pid.insert(t, p);
                }
            }
            Some("process start") => {
                if let Some(p) = ev.u64("pid") {
                    let name = ev.str("name").unwrap_or("?");
                    process.insert(p, kind(name, ev.str("cmdline").unwrap_or("")));
                }
            }
            Some("image") => {
                if let (Some(p), Some(base), Some(size)) =
                    (ev.u64("pid"), ev.u64("base"), ev.u64("size"))
                {
                    let name = file_name(ev.str("name").or(ev.str("path")).unwrap_or("?"));
                    images.entry(p).or_default().push(Image { base, size, name: name.into() });
                }
            }
            Some("sample") => {
                let (Some(t), Some(by), Some(rip)) = (ev.u64("t"), ev.u64("by"), ev.u64("rip"))
                else {
                    continue;
                };
                let stack = ev.u64s("stack");
                if !stack.is_empty() {
                    stacks.insert(samples.len(), stack);
                }
                if let Some(n) = ev.u64("lost") {
                    lost.insert(samples.len(), n);
                }
                samples.push((t, by, ev.str("mode") == Some("user"), rip));
            }
            _ => {}
        }
    }
    let pid_of = |tid: u64| thread_pid.get(&tid).copied().unwrap_or(0);
    let wanted = |&(t, tid, _, _): &(u64, u64, bool, u64)| {
        o.tid.is_none_or(|w| w == tid)
            && o.pid.is_none_or(|w| w == pid_of(tid))
            && o.window.is_none_or(|(a, b)| (a..b).contains(&(t as f64 / 1e9)))
    };
    let selected: Vec<(u64, u64, bool, u64)> =
        samples.iter().copied().filter(|s| wanted(s)).collect();
    // The selected kernel samples' frames, innermost first (the
    // interrupted address, then return addresses).
    // With the samples lost after each (0: none).
    let kernel_stacks: Vec<(Vec<u64>, u64)> = samples
        .iter()
        .enumerate()
        .filter(|(i, s)| !s.2 && wanted(s) && stacks.contains_key(i))
        .map(|(i, s)| {
            let mut v = vec![s.3];
            v.extend(&stacks[&i]);
            (v, lost.get(&i).copied().unwrap_or(0))
        })
        .collect();
    let mut out = String::new();
    let total = selected.len();
    let user = selected.iter().filter(|s| s.2).count();
    let span = match (selected.first(), selected.last()) {
        (Some(a), Some(b)) => (b.0 - a.0) as f64 / 1e9,
        _ => 0.0,
    };
    let _ = writeln!(
        out,
        "profile: {total} samples ({user} in user mode, {} in the kernel) over {span:.1} s{}",
        total - user,
        if samples.len() != total {
            format!(" (of {} in the stream)", samples.len())
        } else {
            String::new()
        }
    );
    if total == 0 {
        return out;
    }
    let name_of = |pid: u64| process.get(&pid).cloned().unwrap_or_else(|| "?".into());

    let mut by_process: HashMap<u64, usize> = HashMap::new();
    let mut by_thread: HashMap<u64, usize> = HashMap::new();
    let mut user_by_thread: HashMap<u64, usize> = HashMap::new();
    // (module, offset) per sample; kernel samples are ("kernel", rip).
    let mut at: HashMap<(String, u64), usize> = HashMap::new();
    let mut by_module: HashMap<String, usize> = HashMap::new();
    for &(_, tid, user_mode, rip) in &selected {
        let pid = pid_of(tid);
        *by_process.entry(pid).or_default() += 1;
        *by_thread.entry(tid).or_default() += 1;
        let (module, offset) = if !user_mode {
            ("kernel".to_string(), rip)
        } else {
            match images
                .get(&pid)
                .and_then(|v| v.iter().find(|i| (i.base..i.base + i.size).contains(&rip)))
            {
                Some(i) => (i.name.clone(), rip - i.base),
                None => ("(no module)".to_string(), rip),
            }
        };
        if user_mode {
            *user_by_thread.entry(tid).or_default() += 1;
        }
        *by_module.entry(module.clone()).or_default() += 1;
        *at.entry((module, offset)).or_default() += 1;
    }
    let _ = writeln!(out, "profile: by process:");
    for (pid, n) in ranked(&by_process, top) {
        let _ = writeln!(out, "  {n:7} {:5.1}%  pid {pid} {}", pct(n, total), name_of(pid));
    }
    let _ = writeln!(out, "profile: by thread:");
    for (tid, n) in ranked(&by_thread, top) {
        let pid = pid_of(tid);
        let u = user_by_thread.get(&tid).copied().unwrap_or(0);
        let _ = writeln!(
            out,
            "  {n:7} {:5.1}%  tid {tid} (pid {pid} {}), {:.0}% in user mode",
            pct(n, total),
            name_of(pid),
            pct(u, n)
        );
    }
    let _ = writeln!(out, "profile: by module:");
    for (m, n) in ranked(&by_module, top) {
        let _ = writeln!(out, "  {n:7} {:5.1}%  {m}", pct(n, total));
    }
    // Functions: symbolized per module where the symbolizer knows it,
    // else 256-byte buckets.
    // Only the addresses in the heaviest 256-byte blocks are symbolized
    // (a large module's symbols are slow to search: chrome.dll's PDB is
    // 2.6 GB, and a run has hundreds of thousands of distinct addresses).
    let mut blocks: HashMap<(String, u64), usize> = HashMap::new();
    for ((m, off), n) in &at {
        *blocks.entry((m.clone(), off & !0xff)).or_default() += n;
    }
    let heavy: std::collections::HashSet<(String, u64)> =
        ranked(&blocks, top * 20).into_iter().map(|(k, _)| k).collect();
    let mut per_module: HashMap<String, Vec<u64>> = HashMap::new();
    for (m, off) in at.keys() {
        if m != "(no module)" && heavy.contains(&(m.clone(), off & !0xff)) {
            per_module.entry(m.clone()).or_default().push(*off);
        }
    }
    let mut names: HashMap<(String, u64), String> = HashMap::new();
    for (m, offs) in &per_module {
        for (off, name) in offs.iter().zip(symbolize(m, offs)) {
            if let Some(name) = name {
                names.insert((m.clone(), *off), name);
            }
        }
    }
    let mut by_function: HashMap<String, usize> = HashMap::new();
    for ((m, off), n) in &at {
        let key = match names.get(&(m.clone(), *off)) {
            Some(f) => format!("{m}!{f}"),
            None => format!("{m}+{:#x}", off & !0xff),
        };
        *by_function.entry(key).or_default() += n;
    }
    let _ = writeln!(out, "profile: by function (or 256-byte block):");
    for (f, n) in ranked(&by_function, top) {
        let _ = writeln!(out, "  {n:7} {:5.1}%  {f}", pct(n, total));
    }
    kernel_callers(&mut out, &kernel_stacks, top, symbolize);
    out
}

/// Path frames shown per kernel call path.
const PATH_FRAMES: usize = 6;

/// The kernel samples' stacks: each function's share including what it
/// calls (inclusive), and the heaviest call paths (leaf <- callers).
fn kernel_callers(out: &mut String, stacks: &[(Vec<u64>, u64)], top: usize, symbolize: Symbolizer) {
    if stacks.is_empty() {
        return;
    }
    // A return address is symbolized one byte back: inside the call
    // instruction (its line and inlined frames, not the next statement's).
    let lookup = |i: usize, a: u64| if i == 0 { a } else { a - 1 };
    let mut addrs: Vec<u64> =
        stacks.iter().flat_map(|(s, _)| s.iter().enumerate().map(|(i, &a)| lookup(i, a))).collect();
    addrs.sort_unstable();
    addrs.dedup();
    let names: HashMap<u64, String> = addrs
        .iter()
        .zip(symbolize("kernel", &addrs))
        .map(|(&a, n)| (a, n.unwrap_or_else(|| format!("{a:#x}"))))
        .collect();
    let named = |s: &Vec<u64>| -> Vec<String> {
        s.iter().enumerate().map(|(i, &a)| names[&lookup(i, a)].clone()).collect()
    };
    let total = stacks.len();
    let mut inclusive: HashMap<String, usize> = HashMap::new();
    let mut paths: HashMap<String, usize> = HashMap::new();
    let mut lost_after: HashMap<String, usize> = HashMap::new();
    for (s, lost) in stacks {
        let mut frames = named(s);
        // Recursion counts once per sample.
        let mut seen = frames.clone();
        seen.sort();
        seen.dedup();
        for f in seen {
            *inclusive.entry(f).or_default() += 1;
        }
        frames.dedup();
        frames.truncate(PATH_FRAMES);
        let path = frames.join(" <- ");
        if *lost > 0 {
            *lost_after.entry(path.clone()).or_default() += *lost as usize;
        }
        *paths.entry(path).or_default() += 1;
    }
    let _ = writeln!(
        out,
        "profile: kernel functions with their callees ({total} samples with stacks):"
    );
    for (f, n) in ranked(&inclusive, top) {
        let _ = writeln!(out, "  {n:7} {:5.1}%  {f}", pct(n, total));
    }
    let _ = writeln!(out, "profile: kernel call paths (leaf <- callers):");
    for (p, n) in ranked(&paths, top) {
        let _ = writeln!(out, "  {n:7} {:5.1}%  {p}", pct(n, total));
    }
    // A CPU whose ring overflowed had interrupts off for longer than the
    // ring holds: the samples lost belong to the stretch its last
    // recorded sample was in.
    let lost: usize = lost_after.values().sum();
    if lost > 0 {
        let _ =
            writeln!(out, "profile: {lost} samples lost to interrupts off past the ring, after:");
        for (p, n) in ranked(&lost_after, top) {
            let _ = writeln!(out, "  {n:7}  {p}");
        }
    }
}

/// Samples in a stream (for the CLI's message when there are none).
pub fn count(events: &str) -> usize {
    events.lines().filter(|l| l.contains(r#""ev":"sample""#)).count()
}

/// Function names for `offsets` of module `file` in `dir` (its PE file and
/// symbols, e.g. target/prebuilt/chromium-syms): llvm-symbolizer on the
/// addresses at the image's preferred base, the innermost function of
/// each.
pub fn llvm_symbolize(dir: &std::path::Path, file: &str, offsets: &[u64]) -> Vec<Option<String>> {
    let path = dir.join(file);
    let Ok(bytes) = std::fs::read(&path) else { return vec![None; offsets.len()] };
    let Some(base) = image_base(&bytes) else { return vec![None; offsets.len()] };
    symbolize_at(&path, base, offsets)
}

/// Function names for kernel addresses (`rips`), from the kernel's ELF.
pub fn llvm_symbolize_kernel(elf: &std::path::Path, rips: &[u64]) -> Vec<Option<String>> {
    symbolize_at(elf, 0, rips)
}

/// llvm-symbolizer on `obj` for `base + offset` of each offset.
fn symbolize_at(obj: &std::path::Path, base: u64, offsets: &[u64]) -> Vec<Option<String>> {
    let none = || vec![None; offsets.len()];
    let input: String = offsets.iter().map(|o| format!("{:#x}\n", base.wrapping_add(*o))).collect();
    let Ok(mut child) = std::process::Command::new("llvm-symbolizer")
        .arg(format!("--obj={}", obj.display()))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
    else {
        return none();
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = std::io::Write::write_all(&mut stdin, input.as_bytes());
    }
    let Ok(output) = child.wait_with_output() else { return none() };
    let text = String::from_utf8_lossy(&output.stdout);
    // One block per address, blank-line separated: function, location,
    // (inlined frames...); the innermost function of each.
    let mut names: Vec<Option<String>> = text
        .split("\n\n")
        .filter(|b| !b.trim().is_empty())
        .map(|b| {
            b.lines().next().filter(|f| *f != "??").map(|f| without_llvm_suffix(f).to_string())
        })
        .collect();
    names.resize(offsets.len(), None);
    names
}

/// A function name without the ` (.llvm.<hash>)` suffix LLVM gives local
/// symbols it promotes across codegen units (noise in a report).
fn without_llvm_suffix(name: &str) -> &str {
    match name.find(" (.llvm.") {
        Some(i) => &name[..i],
        None => name,
    }
}

/// The preferred base of a PE image (OptionalHeader.ImageBase, PE32+).
fn image_base(pe: &[u8]) -> Option<u64> {
    let lfanew = u32::from_le_bytes(pe.get(0x3c..0x40)?.try_into().ok()?) as usize;
    let opt = lfanew + 24;
    match u16::from_le_bytes(pe.get(opt..opt + 2)?.try_into().ok()?) {
        0x20b => Some(u64::from_le_bytes(pe.get(opt + 24..opt + 32)?.try_into().ok()?)),
        0x10b => Some(u64::from(u32::from_le_bytes(pe.get(opt + 28..opt + 32)?.try_into().ok()?))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVENTS: &str = r#"{"t":1000,"cpu":0,"by":0,"ev":"process start","pid":4,"session":1,"name":"a.exe","personality":"nt","ppid":0,"ptid":0,"image_base":1,"entry":1,"cmdline":"a.exe --type=utility --utility-sub-type=network.mojom.NetworkService"}
{"t":1100,"cpu":0,"by":0,"ev":"thread create","pid":4,"tid":8,"start":1,"teb":2}
{"t":1200,"cpu":0,"by":0,"ev":"image","pid":4,"name":"a.exe","base":5368709120,"size":65536}
{"t":1300,"cpu":0,"by":8,"ev":"image","pid":4,"name":"\\Device\\HarddiskVolume1\\x\\b.dll","base":6442450944,"size":65536}
{"t":2000000000,"cpu":0,"by":8,"ev":"sample","mode":"user","rip":5368713216}
{"t":2010000000,"cpu":0,"by":8,"ev":"sample","mode":"user","rip":5368713232}
{"t":2020000000,"cpu":0,"by":8,"ev":"sample","mode":"user","rip":5368717312}
{"t":2030000000,"cpu":0,"by":8,"ev":"sample","mode":"user","rip":6442451200}
{"t":2040000000,"cpu":1,"by":8,"ev":"sample","mode":"kernel","rip":18446744071562067968,"stack":[18446744071562072065,18446744071562076161]}
{"t":2050000000,"cpu":1,"by":99,"ev":"sample","mode":"user","rip":4096}
"#;

    #[test]
    fn samples_go_to_threads_modules_and_functions() {
        let fake: Symbolizer =
            &|m, offs| offs.iter().map(|_| (m == "b.dll").then(|| "foo".to_string())).collect();
        let r = report(EVENTS, &Options { pid: Some(4), ..Options::default() }, fake);
        assert!(r.contains("profile: 5 samples (4 in user mode, 1 in the kernel) over 0.0 s (of 6 in the stream)"), "{r}");
        assert!(r.contains("pid 4 a.exe utility NetworkService"), "{r}");
        assert!(r.contains("tid 8 (pid 4 a.exe utility NetworkService), 80% in user mode"), "{r}");
        assert!(r.contains("      3  60.0%  a.exe\n"), "{r}");
        assert!(r.contains("      1  20.0%  kernel\n"), "{r}");
        // 0x1000 and 0x1010 share a block; b.dll is symbolized.
        assert!(r.contains("      2  40.0%  a.exe+0x1000"), "{r}");
        assert!(r.contains("      1  20.0%  b.dll!foo"), "{r}");
    }

    #[test]
    fn llvm_suffixes_are_dropped() {
        assert_eq!(without_llvm_suffix("a::b (.llvm.5252664376076994469)"), "a::b");
        assert_eq!(without_llvm_suffix("a::b"), "a::b");
    }

    #[test]
    fn kernel_stacks_give_callers() {
        // The leaf at 0xffffffff80000000, called from 0x..1000 (return
        // address 0x..1001), called from 0x..2000.
        let fake: Symbolizer = &|m, offs| {
            offs.iter()
                .map(|&o| match (m, o & 0xffff) {
                    ("kernel", 0) => Some("leaf".to_string()),
                    ("kernel", 0x1000) => Some("allocate".to_string()),
                    ("kernel", 0x2000) => Some("syscall".to_string()),
                    _ => None,
                })
                .collect()
        };
        let r = report(EVENTS, &Options::default(), fake);
        assert!(
            r.contains("profile: kernel functions with their callees (1 samples with stacks):"),
            "{r}"
        );
        assert!(r.contains("      1 100.0%  allocate\n"), "{r}");
        assert!(r.contains("profile: kernel call paths (leaf <- callers):"), "{r}");
        assert!(r.contains("      1 100.0%  leaf <- allocate <- syscall\n"), "{r}");
    }

    #[test]
    fn lost_samples_name_long_interrupts_off_stretches() {
        let events = EVENTS.replace(
            r#""stack":[18446744071562072065,18446744071562076161]}"#,
            r#""stack":[18446744071562072065,18446744071562076161],"lost":7}"#,
        );
        let fake: Symbolizer = &|m, offs| {
            offs.iter()
                .map(|&o| match (m, o & 0xffff) {
                    ("kernel", 0) => Some("leaf".to_string()),
                    ("kernel", 0x1000) => Some("allocate".to_string()),
                    ("kernel", 0x2000) => Some("syscall".to_string()),
                    _ => None,
                })
                .collect()
        };
        let r = report(&events, &Options::default(), fake);
        assert!(
            r.contains("profile: 7 samples lost to interrupts off past the ring, after:"),
            "{r}"
        );
        assert!(r.contains("        7  leaf <- allocate <- syscall\n"), "{r}");
    }

    #[test]
    fn a_window_and_a_thread_narrow_it() {
        let none: Symbolizer = &|_, offs| vec![None; offs.len()];
        let o = Options { tid: Some(8), window: Some((2.015, 2.035)), ..Options::default() };
        let r = report(EVENTS, &o, none);
        assert!(r.starts_with("profile: 2 samples"), "{r}");
    }

    #[test]
    fn the_preferred_base_comes_from_the_pe_header() {
        let mut pe = vec![0u8; 0x200];
        pe[0x3c] = 0x80;
        pe[0x80 + 24] = 0x0b;
        pe[0x80 + 25] = 0x02;
        pe[0x80 + 24 + 24..0x80 + 24 + 32].copy_from_slice(&0x1_8000_0000u64.to_le_bytes());
        assert_eq!(image_base(&pe), Some(0x1_8000_0000));
    }
}
