//! `cargo xtask perfetto`: a boot's event stream as a trace for the
//! Perfetto UI (ui.perfetto.dev, offline: open the file), in the Chrome
//! JSON trace-event format: a track per process and thread, system calls
//! and waits as slices (a wait names its object), pipe writes and reads
//! and completion-port posts and dequeues as short slices joined by flow
//! arrows (data: write to the read that took it; completions: post to
//! dequeue), process creation as a flow from the creating thread to the
//! child's first thread, profiler samples as instants named by module.
//! docs/OBSERVABILITY.md "Visualization".

use std::collections::{HashMap, VecDeque};
use std::fmt::Write;

use crate::otel::{Json, parse_json};

/// The writes into each (pipe, reading end): (first byte, end, time,
/// writer's process and thread).
type Writes = HashMap<(u64, u64), Vec<(u64, u64, u64, u64, u64)>>;

/// A JSON string.
fn q(text: &str) -> String {
    let mut o = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Microseconds (the format's unit) from ns.
fn us(ns: u64) -> String {
    format!("{}.{:03}", ns / 1000, ns % 1000)
}

/// The trace (a JSON object with `traceEvents`).
pub fn convert(events: &str) -> String {
    let mut evs: Vec<(u64, Json)> = events
        .lines()
        .filter_map(|l| parse_json(l.trim()).ok())
        .filter_map(|e| e.u64("t").map(|t| (t, e)))
        .collect();
    evs.sort_by_key(|e| e.0);
    let mut out: Vec<String> = Vec::new();
    let mut thread_pid: HashMap<u64, u64> = HashMap::new();
    let mut named_threads: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut pipe_names: HashMap<u64, String> = HashMap::new();
    // (pipe, reading end) -> [(first byte, end, flow id)]
    let mut writes: Writes = HashMap::new();
    // (port, context) -> flow ids, oldest first
    let mut posts: HashMap<(u64, u64), VecDeque<u64>> = HashMap::new();
    // child pid -> flow id (its creation)
    let mut created: HashMap<u64, u64> = HashMap::new();
    // tid -> (entry t, name, object): waits not yet returned
    let mut waits: HashMap<u64, (u64, String, String)> = HashMap::new();
    let mut images: HashMap<u64, Vec<(u64, u64, String)>> = HashMap::new();
    let mut next_flow = 1u64;
    let last = evs.last().map_or(0, |e| e.0);
    let pid_of = |m: &HashMap<u64, u64>, tid: u64| m.get(&tid).copied().unwrap_or(0);
    for (t, ev) in &evs {
        let t = *t;
        let by = ev.u64("by").unwrap_or(0);
        match ev.str("ev").unwrap_or("") {
            "process start" => {
                let pid = ev.u64("pid").unwrap_or(0);
                let name = ev.str("name").unwrap_or("?");
                let cmd = ev.str("cmdline").unwrap_or("");
                let kind = cmd
                    .split_whitespace()
                    .find_map(|w| w.strip_prefix("--type="))
                    .map(|k| format!(" ({k})"))
                    .unwrap_or_default();
                out.push(format!(
                    r#"{{"ph":"M","name":"process_name","pid":{pid},"args":{{"name":{}}}}}"#,
                    q(&format!("{name}{kind} [{pid}]"))
                ));
                if let (Some(ptid), true) =
                    (ev.u64("ptid").filter(|&p| p != 0), ev.u64("ppid").is_some())
                {
                    let id = next_flow;
                    next_flow += 1;
                    let ppid = pid_of(&thread_pid, ptid);
                    out.push(format!(
                        r#"{{"ph":"X","name":"create process {pid}","cat":"process","pid":{ppid},"tid":{ptid},"ts":{},"dur":1}}"#,
                        us(t)
                    ));
                    out.push(format!(
                        r#"{{"ph":"s","id":{id},"name":"created","cat":"process","pid":{ppid},"tid":{ptid},"ts":{}}}"#,
                        us(t)
                    ));
                    created.insert(pid, id);
                }
            }
            "thread create" => {
                let (pid, tid) = (ev.u64("pid").unwrap_or(0), ev.u64("tid").unwrap_or(0));
                thread_pid.insert(tid, pid);
                if named_threads.insert(tid) {
                    out.push(format!(
                        r#"{{"ph":"M","name":"thread_name","pid":{pid},"tid":{tid},"args":{{"name":{}}}}}"#,
                        q(&format!("thread {tid}"))
                    ));
                }
                out.push(format!(
                    r#"{{"ph":"X","name":"thread start","cat":"thread","pid":{pid},"tid":{tid},"ts":{},"dur":1}}"#,
                    us(t)
                ));
                if let Some(id) = created.remove(&pid) {
                    out.push(format!(
                        r#"{{"ph":"f","bp":"e","id":{id},"name":"created","cat":"process","pid":{pid},"tid":{tid},"ts":{}}}"#,
                        us(t)
                    ));
                }
            }
            "image" => {
                if let (Some(pid), Some(b), Some(s)) =
                    (ev.u64("pid"), ev.u64("base"), ev.u64("size"))
                {
                    let n = ev.str("name").unwrap_or("?");
                    let n = n.rsplit(['\\', '/']).next().unwrap_or(n).to_string();
                    images.entry(pid).or_default().push((b, s, n));
                }
            }
            "wait" => {
                let tid = ev.u64("tid").unwrap_or(0);
                let obj = ev.str("obj").unwrap_or("").to_string();
                waits.insert(tid, (t, ev.str("name").unwrap_or("?").to_string(), obj));
            }
            "syscall" => {
                let (pid, tid, ns) = (
                    ev.u64("pid").unwrap_or(0),
                    ev.u64("tid").unwrap_or(0),
                    ev.u64("ns").unwrap_or(0),
                );
                thread_pid.entry(tid).or_insert(pid);
                let name = ev.str("name").unwrap_or("?");
                let obj = waits.remove(&tid).map(|w| w.2).unwrap_or_default();
                let status = ev.u64("status").unwrap_or(0);
                out.push(format!(
                    r#"{{"ph":"X","name":{},"cat":"syscall","pid":{pid},"tid":{tid},"ts":{},"dur":{},"args":{{"status":"{status:#010x}","object":{}}}}}"#,
                    q(name),
                    us(t.saturating_sub(ns)),
                    us(ns.max(1)),
                    q(&obj)
                ));
            }
            "pipe create" => {
                if let (Some(p), Some(n)) = (ev.u64("pipe"), ev.str("name")) {
                    pipe_names.insert(p, n.to_string());
                }
            }
            kind @ ("pipe write" | "pipe read") => {
                let pid = pid_of(&thread_pid, by);
                let (pipe, off, bytes) = (
                    ev.u64("pipe").unwrap_or(0),
                    ev.u64("off").unwrap_or(0),
                    ev.u64("bytes").unwrap_or(0),
                );
                let name = pipe_names.get(&pipe).cloned().unwrap_or_default();
                out.push(format!(
                    r#"{{"ph":"X","name":{},"cat":"ipc","pid":{pid},"tid":{by},"ts":{},"dur":1,"args":{{"pipe":{pipe},"name":{},"offset":{off},"bytes":{bytes}}}}}"#,
                    q(&format!("{kind} {bytes} B")),
                    us(t),
                    q(&name)
                ));
                if kind == "pipe write" {
                    let to = ev.u64("to").unwrap_or(0);
                    writes.entry((pipe, to)).or_default().push((off, off + bytes, t, pid, by));
                } else {
                    // An arrow from each write whose bytes this read took
                    // (one per pair: a write read in parts has several).
                    let end = ev.u64("end").unwrap_or(0);
                    let from: Vec<(u64, u64, u64)> =
                        writes.get(&(pipe, end)).map_or(Vec::new(), |w| {
                            w.iter()
                                .filter(|&&(a, b, ..)| a < off + bytes.max(1) && off < b.max(a + 1))
                                .map(|&(_, _, wt, wpid, wtid)| (wt, wpid, wtid))
                                .collect()
                        });
                    for (wt, wpid, wtid) in from {
                        let id = next_flow;
                        next_flow += 1;
                        out.push(format!(
                            r#"{{"ph":"s","id":{id},"name":"pipe data","cat":"ipc","pid":{wpid},"tid":{wtid},"ts":{}}}"#,
                            us(wt)
                        ));
                        out.push(format!(
                            r#"{{"ph":"f","bp":"e","id":{id},"name":"pipe data","cat":"ipc","pid":{pid},"tid":{by},"ts":{}}}"#,
                            us(t)
                        ));
                    }
                }
            }
            kind @ ("iocp post" | "iocp dequeue") => {
                let pid = pid_of(&thread_pid, by);
                let (port, ctx) = (ev.u64("port").unwrap_or(0), ev.u64("ctx").unwrap_or(0));
                out.push(format!(
                    r#"{{"ph":"X","name":{},"cat":"ipc","pid":{pid},"tid":{by},"ts":{},"dur":1,"args":{{"port":{port},"context":"{ctx:#x}","bytes":{}}}}}"#,
                    q(kind),
                    us(t),
                    ev.u64("bytes").unwrap_or(0)
                ));
                if kind == "iocp post" {
                    let id = next_flow;
                    next_flow += 1;
                    posts.entry((port, ctx)).or_default().push_back(id);
                    out.push(format!(
                        r#"{{"ph":"s","id":{id},"name":"completion","cat":"ipc","pid":{pid},"tid":{by},"ts":{}}}"#,
                        us(t)
                    ));
                } else if let Some(id) = posts.get_mut(&(port, ctx)).and_then(VecDeque::pop_front) {
                    out.push(format!(
                        r#"{{"ph":"f","bp":"e","id":{id},"name":"completion","cat":"ipc","pid":{pid},"tid":{by},"ts":{}}}"#,
                        us(t)
                    ));
                }
            }
            "sample" => {
                let pid = pid_of(&thread_pid, by);
                let rip = ev.u64("rip").unwrap_or(0);
                let module = if ev.str("mode") == Some("kernel") {
                    "kernel".to_string()
                } else {
                    images
                        .get(&pid)
                        .and_then(|v| v.iter().find(|i| (i.0..i.0 + i.1).contains(&rip)))
                        .map_or("?".to_string(), |i| format!("{}+{:#x}", i.2, rip - i.0))
                };
                out.push(format!(
                    r#"{{"ph":"i","s":"t","name":{},"cat":"sample","pid":{pid},"tid":{by},"ts":{}}}"#,
                    q(&format!("on CPU: {module}")),
                    us(t)
                ));
            }
            _ => {}
        }
    }
    // Waits that never returned: slices to the end of the stream.
    for (tid, (t, name, obj)) in waits {
        let pid = pid_of(&thread_pid, tid);
        out.push(format!(
            r#"{{"ph":"X","name":{},"cat":"syscall","pid":{pid},"tid":{tid},"ts":{},"dur":{},"args":{{"object":{},"returned":false}}}}"#,
            q(&format!("{name} (no return)")),
            us(t),
            us(last.saturating_sub(t).max(1)),
            q(&obj)
        ));
    }
    format!("{{\"displayTimeUnit\":\"ms\",\"traceEvents\":[\n{}\n]}}\n", out.join(",\n"))
}

/// What a converted trace holds, for the CLI's line.
pub fn summary(trace: &str) -> String {
    let count = |ph: &str| trace.matches(&format!("\"ph\":\"{ph}\"")).count();
    format!(
        "{} slices, {} flows ({} ends), {} samples, {} names",
        count("X"),
        count("s"),
        count("f"),
        count("i"),
        count("M")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVENTS: &str = r#"{"t":1000,"cpu":0,"by":0,"ev":"process start","pid":4,"session":1,"name":"browser.exe","personality":"nt","ppid":0,"ptid":0,"image_base":1,"entry":1,"cmdline":"browser.exe"}
{"t":1100,"cpu":0,"by":0,"ev":"thread create","pid":4,"tid":8,"start":1,"teb":2}
{"t":2000,"cpu":0,"by":8,"ev":"process start","pid":20,"session":1,"name":"browser.exe","personality":"nt","ppid":4,"ptid":8,"image_base":1,"entry":1,"cmdline":"browser.exe --type=utility"}
{"t":2100,"cpu":0,"by":8,"ev":"thread create","pid":20,"tid":24,"start":1,"teb":2}
{"t":3000,"cpu":0,"by":8,"ev":"pipe create","pipe":3,"name":"\\mojo.1","message":false,"instance":1}
{"t":4000,"cpu":1,"by":24,"ev":"pipe write","pipe":3,"end":1,"to":0,"off":0,"bytes":80,"queued":80}
{"t":4100,"cpu":1,"by":24,"ev":"iocp post","port":5,"key":7,"ctx":4096,"status":0,"bytes":80,"depth":1}
{"t":9000,"cpu":0,"by":8,"ev":"iocp dequeue","port":5,"key":7,"ctx":4096,"status":0,"bytes":80,"queued_t":4100,"queued_by":24,"depth":0}
{"t":9100,"cpu":0,"by":8,"ev":"pipe read","pipe":3,"end":0,"off":0,"bytes":80,"left":0}
{"t":9500,"cpu":0,"by":8,"ev":"wait","pid":4,"tid":8,"nr":4,"name":"NtWaitForSingleObject","args":[1,0,0],"status":0,"ns":0,"obj":"Event","oid":7}
{"t":9900,"cpu":0,"by":8,"ev":"syscall","pid":4,"tid":8,"nr":4,"name":"NtWaitForSingleObject","args":[1,0,0],"status":258,"ns":400}
{"t":9950,"cpu":1,"by":24,"ev":"sample","mode":"kernel","rip":1}
"#;

    #[test]
    fn tracks_slices_and_flows() {
        let trace = convert(EVENTS);
        let v = parse_json(&trace).expect("valid JSON");
        let Some(Json::Arr(events)) = v.get("traceEvents") else { panic!("traceEvents") };
        let with = |ph: &str| events.iter().filter(|e| e.str("ph") == Some(ph)).collect::<Vec<_>>();
        // Processes named with their kind; threads named.
        assert!(trace.contains(r#""name":"browser.exe (utility) [20]""#), "{trace}");
        assert!(trace.contains(r#""name":"thread 24""#));
        // Every flow start has its end: creation, pipe data, completion.
        let starts: Vec<u64> = with("s").iter().filter_map(|e| e.u64("id")).collect();
        let ends: Vec<u64> = with("f").iter().filter_map(|e| e.u64("id")).collect();
        assert_eq!(starts.len(), 3, "{trace}");
        for id in &starts {
            assert!(ends.contains(id), "flow {id} ends");
        }
        // The pipe data arrow goes from the child's thread to the browser's.
        let data = with("s").into_iter().find(|e| e.str("name") == Some("pipe data")).unwrap();
        let end = with("f").into_iter().find(|e| e.str("name") == Some("pipe data")).unwrap();
        assert_eq!((data.u64("tid"), end.u64("tid")), (Some(24), Some(8)));
        // The wait is a slice naming its object; the sample an instant.
        let wait =
            with("X").into_iter().find(|e| e.str("name") == Some("NtWaitForSingleObject")).unwrap();
        assert_eq!(wait.get("dur"), Some(&Json::Num("0.400".into())));
        assert!(trace.contains(r#""object":"Event""#));
        assert!(trace.contains(r#""name":"on CPU: kernel""#));
        // Slices: the creation, two thread starts, two IPC steps per side,
        // the wait.
        assert!(
            summary(&trace).starts_with("8 slices, 3 flows (3 ends), 1 samples"),
            "{}",
            summary(&trace)
        );
    }
}
