//! OpenTelemetry export of a boot (ROADMAP OT, stage OT1;
//! docs/OBSERVABILITY.md "OpenTelemetry export"): the event stream
//! (`<image>.events.jsonl`, debugcon) and the serial log of one boot
//! become OTLP/JSON traces, logs and metrics, which an OTel collector,
//! Jaeger, Tempo or Grafana read. `cargo xtask otel <log-or-scenario>`;
//! `XTASK_OTEL=1 cargo xtask boot-test ...` for every boot.
//!
//! The mapping (the table is in docs/OBSERVABILITY.md):
//! - one resource per boot (`service.name` gala-os, the kernel version,
//!   build and scenario), one trace per boot whose root span is the boot;
//! - processes and threads are spans (start to exit), children of the
//!   boot and of their process; system calls are spans under their thread
//!   (the stream has their end and duration); a wait that entered and
//!   never returned is a span event;
//! - image loads, faults and every other event are span events on their
//!   thread, process or the boot;
//! - the serial log's lines are log records (severity from the level),
//!   a crash report and a state dump one record each (and an `exception`
//!   span event for a crash or a panic);
//! - `stats:` lines are gauges; system calls per service are sums.
//!
//! Serial lines carry no time: each gets the time of the latest line
//! before it that can be placed (a process start or exit the stream also
//! has, an `uptime_ms=` or `now N ms since boot` mark), and says so
//! (`gala.time.estimated`). What could not be mapped is counted per event
//! kind in the [`Report`], never dropped silently; what the stream lacks
//! (an exit, a parent) is counted as a gap for OT2.
//!
//! Hand-written (no OTel crates): the encoder follows the OTLP/JSON
//! mapping of opentelemetry-proto (Apache-2.0; THIRD_PARTY.md): ids in
//! hex, 64-bit integers and enums as JSON strings and numbers, field
//! names in lowerCamelCase.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

/// One boot to convert.
pub struct Boot<'a> {
    pub serial: &'a str,
    pub events: &'a str,
    /// The scenario (`crash`) or the shared VM's image (`batch-x_test`).
    pub scenario: &'a str,
    /// The kernel build: profile and commit (`debug 1a2b3c4`).
    pub build: &'a str,
    /// Wall time (Unix ns) of the boot's time 0 (the stream's `t` is ns
    /// since boot).
    pub start_unix_ns: u64,
}

/// The OTLP/JSON requests (each one an `Export*ServiceRequest`, the body
/// of a POST to `/v1/traces`, `/v1/logs`, `/v1/metrics`).
pub struct Export {
    pub traces: Vec<String>,
    pub logs: Vec<String>,
    pub metrics: String,
    pub report: Report,
}

/// What the conversion did and could not do.
#[derive(Default, Debug)]
pub struct Report {
    pub events: u64,
    pub lines: u64,
    pub spans: u64,
    pub span_events: u64,
    pub logs: u64,
    pub points: u64,
    /// Event kind -> (count, what it became).
    pub mapped: BTreeMap<String, (u64, &'static str)>,
    /// Event kind (or "unparsed line") -> count: not in the output.
    pub not_mapped: BTreeMap<String, u64>,
    /// What the stream lacks (OT2's work) -> count.
    pub gaps: BTreeMap<&'static str, u64>,
    /// The slowest IPC hand-offs (OT3): (ns, description), longest first,
    /// at most `SLOWEST` of each kind.
    pub slow_pipes: Vec<(u64, String)>,
    pub slow_completions: Vec<(u64, String)>,
}

/// How many of the slowest IPC hand-offs the report keeps.
const SLOWEST: usize = 5;

/// Keeps `list` the `SLOWEST` longest, longest first.
fn keep_slowest(list: &mut Vec<(u64, String)>, ns: u64, what: impl FnOnce() -> String) {
    if list.len() < SLOWEST || list.last().is_some_and(|l| l.0 < ns) {
        list.push((ns, what()));
        list.sort_by_key(|l| core::cmp::Reverse(l.0));
        list.truncate(SLOWEST);
    }
}

impl Report {
    fn mapped(&mut self, kind: &str, how: &'static str) {
        self.mapped.entry(kind.to_string()).or_insert((0, how)).0 += 1;
    }
    fn gap(&mut self, what: &'static str) {
        *self.gaps.entry(what).or_default() += 1;
    }
    /// `otel:` lines for people and agents.
    pub fn lines(&self) -> String {
        let mut o = String::new();
        let _ = writeln!(
            o,
            "otel: {} events, {} log lines -> {} spans, {} span events, {} log records, {} metric points",
            self.events, self.lines, self.spans, self.span_events, self.logs, self.points
        );
        let list = |m: &mut dyn Iterator<Item = String>| m.collect::<Vec<_>>().join(", ");
        if !self.mapped.is_empty() {
            let mut it = self.mapped.iter().map(|(k, (n, how))| format!("{k} {n} ({how})"));
            let _ = writeln!(o, "otel: mapped: {}", list(&mut it));
        }
        if !self.not_mapped.is_empty() {
            let mut it = self.not_mapped.iter().map(|(k, n)| format!("{k} {n}"));
            let _ = writeln!(o, "otel: NOT mapped: {}", list(&mut it));
        }
        if !self.gaps.is_empty() {
            let mut it = self.gaps.iter().map(|(k, n)| format!("{k} {n}"));
            let _ = writeln!(o, "otel: gaps in the stream (OT2): {}", list(&mut it));
        }
        for (ns, what) in &self.slow_pipes {
            let _ = writeln!(
                o,
                "otel: slow pipe data: {:.1} ms from write to read: {what}",
                *ns as f64 / 1e6
            );
        }
        for (ns, what) in &self.slow_completions {
            let _ = writeln!(
                o,
                "otel: slow completion: {:.1} ms queued before dequeue: {what}",
                *ns as f64 / 1e6
            );
        }
        o
    }
}

/// An attribute value (OTLP `AnyValue`).
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Str(String),
    Int(i64),
    Bool(bool),
    Arr(Vec<Val>),
}

type Attrs = Vec<(String, Val)>;

/// The writes into each (pipe, reading end): (first byte, end, span).
type PipeWrites = HashMap<(u64, u64), Vec<(u64, u64, usize)>>;

/// I/O completion posts not yet dequeued, per (port, context), oldest
/// first (their spans).
type Posts = HashMap<(u64, u64), std::collections::VecDeque<usize>>;

fn s(v: impl Into<String>) -> Val {
    Val::Str(v.into())
}
fn hex(v: u64) -> Val {
    Val::Str(format!("{v:#x}"))
}

struct SpanEvent {
    t: u64,
    name: String,
    attrs: Attrs,
}

struct Span {
    id: u64,
    parent: Option<usize>,
    name: String,
    start: u64,
    end: Option<u64>,
    attrs: Attrs,
    events: Vec<SpanEvent>,
    /// Links to other spans of the trace (the span they point to, the
    /// link's attributes): a process to the thread that created it.
    links: Vec<(usize, Attrs)>,
    /// (code, message): 0 unset, 1 ok, 2 error.
    status: (u8, String),
}

struct LogRecord {
    t: u64,
    severity: u8,
    text: &'static str,
    body: String,
    attrs: Attrs,
    span: Option<usize>,
}

struct Metric {
    name: String,
    unit: &'static str,
    /// Gauge points (time, value, attributes) or a sum.
    points: Vec<(u64, i64, Attrs)>,
    sum: bool,
}

// ---- a small JSON reader for the event lines (numbers kept as text) ----

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn u64(&self, key: &str) -> Option<u64> {
        match self.get(key)? {
            Json::Num(n) => n.parse().ok(),
            _ => None,
        }
    }
    fn str(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
}

pub fn parse_json(text: &str) -> Result<Json, String> {
    let mut p = Reader { s: text.as_bytes(), i: 0 };
    let v = p.value()?;
    p.ws();
    if p.i != p.s.len() {
        return Err(format!("trailing input at byte {}", p.i));
    }
    Ok(v)
}

struct Reader<'a> {
    s: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(u8::is_ascii_whitespace) {
            self.i += 1;
        }
    }
    fn eat(&mut self, c: u8) -> Result<(), String> {
        self.ws();
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("expected '{}' at byte {}", c as char, self.i))
        }
    }
    fn value(&mut self) -> Result<Json, String> {
        self.ws();
        match self.s.get(self.i).copied() {
            Some(b'{') => {
                self.i += 1;
                let mut m = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Json::Obj(m));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.eat(b':')?;
                    m.push((k, self.value()?));
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Json::Obj(m));
                        }
                        _ => return Err(format!("expected ',' or '}}' at byte {}", self.i)),
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut a = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(Json::Arr(a));
                }
                loop {
                    a.push(self.value()?);
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Arr(a));
                        }
                        _ => return Err(format!("expected ',' or ']' at byte {}", self.i)),
                    }
                }
            }
            Some(b'"') => self.string().map(Json::Str),
            Some(b't') if self.s[self.i..].starts_with(b"true") => {
                self.i += 4;
                Ok(Json::Bool(true))
            }
            Some(b'f') if self.s[self.i..].starts_with(b"false") => {
                self.i += 5;
                Ok(Json::Bool(false))
            }
            Some(b'n') if self.s[self.i..].starts_with(b"null") => {
                self.i += 4;
                Ok(Json::Null)
            }
            Some(c) if c == b'-' || c.is_ascii_digit() => {
                let start = self.i;
                while self
                    .s
                    .get(self.i)
                    .is_some_and(|c| matches!(c, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'))
                {
                    self.i += 1;
                }
                Ok(Json::Num(String::from_utf8_lossy(&self.s[start..self.i]).into_owned()))
            }
            _ => Err(format!("unexpected input at byte {}", self.i)),
        }
    }
    fn string(&mut self) -> Result<String, String> {
        self.eat(b'"')?;
        let mut out = Vec::new();
        loop {
            let Some(&c) = self.s.get(self.i) else { return Err("unterminated string".into()) };
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).map_err(|e| e.to_string()),
                b'\\' => {
                    let Some(&e) = self.s.get(self.i) else { return Err("bad escape".into()) };
                    self.i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'u' => {
                            let h = self.s.get(self.i..self.i + 4).ok_or("bad \\u escape")?;
                            let h = std::str::from_utf8(h).map_err(|e| e.to_string())?;
                            let code = u32::from_str_radix(h, 16).map_err(|e| e.to_string())?;
                            self.i += 4;
                            let ch = char::from_u32(code).unwrap_or('\u{fffd}');
                            let mut b = [0; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut b).as_bytes());
                        }
                        other => out.push(other),
                    }
                }
                c => out.push(c),
            }
        }
    }
}

// ---- the JSON writer ----

fn esc(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn val_json(out: &mut String, v: &Val) {
    match v {
        Val::Str(t) => {
            out.push_str("{\"stringValue\":");
            esc(out, t);
            out.push('}');
        }
        // int64 is a JSON string in the OTLP/JSON mapping.
        Val::Int(i) => {
            let _ = write!(out, "{{\"intValue\":\"{i}\"}}");
        }
        Val::Bool(b) => {
            let _ = write!(out, "{{\"boolValue\":{b}}}");
        }
        Val::Arr(a) => {
            out.push_str("{\"arrayValue\":{\"values\":[");
            for (i, v) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                val_json(out, v);
            }
            out.push_str("]}}");
        }
    }
}

fn attrs_json(out: &mut String, attrs: &[(String, Val)]) {
    out.push('[');
    for (i, (k, v)) in attrs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"key\":");
        esc(out, k);
        out.push_str(",\"value\":");
        val_json(out, v);
        out.push('}');
    }
    out.push(']');
}

const SCOPE: &str = r#"{"name":"gala-xtask-otel","version":"1"}"#;

// ---- the conversion ----

/// SplitMix64: ids derived from the boot's content (the same boot gives
/// the same ids, so a second import replaces rather than duplicates).
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn fnv(data: &[&str]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for d in data {
        for b in d.bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
        }
        h = (h ^ 0xff).wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// Level prefix of a serial line -> (severity number, text, the rest).
fn level(line: &str) -> (u8, &'static str, &str) {
    let line = line.trim_end_matches('\r');
    for (p, n, t) in [
        ("[trace] ", 1, "TRACE"),
        ("[debug] ", 5, "DEBUG"),
        ("[info ] ", 9, "INFO"),
        ("[warn ] ", 13, "WARN"),
        ("[error] ", 17, "ERROR"),
    ] {
        if let Some(rest) = line.strip_prefix(p) {
            return (n, t, rest);
        }
    }
    (0, "", line)
}

fn number_after(text: &str, marker: &str) -> Option<u64> {
    let at = text.find(marker)? + marker.len();
    let digits: String = text[at..].chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn hex_after(text: &str, marker: &str) -> Option<u64> {
    let at = text.find(marker)? + marker.len();
    let rest = text[at..].strip_prefix("0x")?;
    let digits: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
    u64::from_str_radix(&digits, 16).ok()
}

fn fault_name(vector: u64) -> &'static str {
    match vector {
        0 => "#DE divide error",
        1 => "#DB debug",
        3 => "#BP breakpoint",
        4 => "#OF overflow",
        6 => "#UD invalid opcode",
        13 => "#GP general protection",
        14 => "#PF page fault",
        16 => "#MF x87 error",
        17 => "#AC alignment check",
        19 => "#XM SIMD error",
        _ => "exception",
    }
}

/// A `stats:` line's figures: "1 TLB shootdowns (1 IPIs, ...), heap 451
/// KiB in use, frames free 1 of 2; heap peak 543 KiB" -> (name, value,
/// unit). Each clause (split at `,;()`) gives the number in it, named by
/// its words; "N of M" gives `<name>` and `<name>.total`.
pub fn stats_figures(text: &str) -> Vec<(String, i64, &'static str)> {
    let mut out = Vec::new();
    for clause in text.split([',', ';', '(', ')']) {
        let words: Vec<&str> = clause.split_whitespace().collect();
        let Some(at) = words.iter().position(|w| w.parse::<i64>().is_ok()) else { continue };
        let value: i64 = words[at].parse().unwrap_or(0);
        let mut unit = "1";
        let mut total = None;
        let mut name_words = Vec::new();
        let mut i = 0;
        while i < words.len() {
            let w = words[i];
            if i == at {
            } else if w == "KiB" {
                unit = "KiBy";
            } else if w == "of" && words.get(i + 1).is_some_and(|n| n.parse::<i64>().is_ok()) {
                total = words[i + 1].parse::<i64>().ok();
                i += 1;
            } else {
                name_words.push(w.to_ascii_lowercase());
            }
            i += 1;
        }
        if name_words.is_empty() {
            continue;
        }
        let name = format!("gala.stats.{}", name_words.join("_").replace(['-', '.'], "_"));
        if let Some(t) = total {
            out.push((format!("{name}.total"), t, unit));
        }
        out.push((name, value, unit));
    }
    out
}

struct State {
    spans: Vec<Span>,
    /// pid -> open process span.
    procs: HashMap<u64, usize>,
    /// Every process span by pid, in order (for the serial log).
    proc_history: Vec<(u64, usize)>,
    /// tid -> open thread span.
    threads: HashMap<u64, usize>,
    /// tid -> (entry time, name, event attributes): waits not yet returned.
    waits: HashMap<u64, (u64, String, Attrs)>,
    /// process span -> whether its start event was seen.
    started: HashMap<usize, bool>,
    trace_seed: u64,
    report: Report,
}

impl State {
    fn new_span(&mut self, parent: Option<usize>, name: String, start: u64, attrs: Attrs) -> usize {
        let id = mix(self.trace_seed ^ mix(self.spans.len() as u64 + 1)) | 1;
        self.spans.push(Span {
            id,
            parent,
            name,
            start,
            end: None,
            attrs,
            events: Vec::new(),
            links: Vec::new(),
            status: (0, String::new()),
        });
        self.spans.len() - 1
    }

    fn process(&mut self, pid: u64, t: u64) -> usize {
        if let Some(&p) = self.procs.get(&pid) {
            return p;
        }
        let attrs = vec![("process.pid".to_string(), Val::Int(pid as i64))];
        let p = self.new_span(Some(0), format!("process {pid}"), t, attrs);
        self.procs.insert(pid, p);
        self.proc_history.push((pid, p));
        self.started.insert(p, false);
        p
    }

    fn thread(&mut self, pid: Option<u64>, tid: u64, t: u64) -> usize {
        if let Some(&th) = self.threads.get(&tid) {
            return th;
        }
        let parent = match pid {
            Some(pid) => self.process(pid, t),
            None => {
                self.report.gap("thread without a process");
                0
            }
        };
        self.report.gap("thread seen before its create event");
        let th = self.new_span(Some(parent), format!("thread {tid}"), t, thread_attrs(pid, tid));
        self.threads.insert(tid, th);
        th
    }

    /// Closes a process span and its open threads.
    fn end_process(&mut self, pid: u64, t: u64) {
        let Some(p) = self.procs.remove(&pid) else { return };
        self.spans[p].end = Some(t);
        let open: Vec<u64> = self
            .threads
            .iter()
            .filter(|&(_, &th)| self.spans[th].parent == Some(p))
            .map(|(&k, _)| k)
            .collect();
        for tid in open {
            if let Some(th) = self.threads.remove(&tid) {
                self.report.gap("thread without exit (ended with its process)");
                self.spans[th].end = Some(t);
                self.spans[th].attrs.push(("gala.unfinished".into(), Val::Bool(true)));
                self.flush_wait(tid);
            }
        }
    }

    /// A wait that entered and did not return: a span event.
    fn flush_wait(&mut self, tid: u64) {
        let Some((t, name, attrs)) = self.waits.remove(&tid) else { return };
        self.report.gap("wait without return");
        let at = self.threads.get(&tid).copied().unwrap_or(0);
        let at = if at == 0 { self.find_thread_span(tid).unwrap_or(0) } else { at };
        self.spans[at].events.push(SpanEvent {
            t,
            name: format!("{name} (entered, no return)"),
            attrs,
        });
        self.report.span_events += 1;
    }

    fn find_thread_span(&self, tid: u64) -> Option<usize> {
        self.spans.iter().rposition(|s| {
            s.name.starts_with("thread ")
                && s.attrs.iter().any(|(k, v)| k == "thread.id" && *v == Val::Int(tid as i64))
        })
    }
}

/// The thread id of the thread span holding span `sp` (0: none).
fn ev_thread_of(st: &State, sp: usize) -> u64 {
    let parent = st.spans[sp].parent.unwrap_or(0);
    st.spans[parent]
        .attrs
        .iter()
        .find_map(|(k, v)| match (k.as_str(), v) {
            ("thread.id", Val::Int(t)) => Some(*t as u64),
            _ => None,
        })
        .unwrap_or(0)
}

fn thread_attrs(pid: Option<u64>, tid: u64) -> Attrs {
    let mut a = vec![("thread.id".to_string(), Val::Int(tid as i64))];
    if let Some(pid) = pid {
        a.push(("process.pid".to_string(), Val::Int(pid as i64)));
    }
    a
}

/// A generic event's members as attributes (`gala.<key>`; `pid` and
/// `tid` by their semantic-convention names).
fn generic_attrs(ev: &Json) -> Attrs {
    let mut out = Vec::new();
    let Json::Obj(m) = ev else { return out };
    for (k, v) in m {
        let key = match k.as_str() {
            "t" | "ev" => continue,
            "pid" => "process.pid".to_string(),
            "tid" => "thread.id".to_string(),
            k => format!("gala.{k}"),
        };
        if let Some(v) = json_val(v) {
            out.push((key, v));
        }
    }
    out
}

fn json_val(v: &Json) -> Option<Val> {
    Some(match v {
        Json::Null => return None,
        Json::Bool(b) => Val::Bool(*b),
        Json::Num(n) => match n.parse::<i64>() {
            Ok(i) => Val::Int(i),
            Err(_) => match n.parse::<u64>() {
                Ok(u) => hex(u),
                Err(_) => Val::Str(n.clone()),
            },
        },
        Json::Str(t) => Val::Str(t.clone()),
        Json::Arr(a) => Val::Arr(a.iter().filter_map(json_val).collect()),
        Json::Obj(_) => return None,
    })
}

/// Converts one boot.
pub fn convert(boot: &Boot) -> Export {
    let seed = fnv(&[boot.serial, boot.events, boot.scenario]);
    let mut st = State {
        spans: Vec::new(),
        procs: HashMap::new(),
        proc_history: Vec::new(),
        threads: HashMap::new(),
        waits: HashMap::new(),
        started: HashMap::new(),
        trace_seed: seed,
        report: Report::default(),
    };
    let trace_id = format!("{:016x}{:016x}", mix(seed) | 1, mix(seed ^ 0x5555));
    let root = st.new_span(
        None,
        format!("boot {}", boot.scenario),
        0,
        vec![("gala.scenario".into(), s(boot.scenario))],
    );

    // Events, by time (lines from different CPUs may be slightly out of
    // order in the file).
    let mut events: Vec<(u64, Json)> = Vec::new();
    for line in boot.events.lines().filter(|l| !l.trim().is_empty()) {
        match parse_json(line.trim()) {
            Ok(ev) if ev.str("ev").is_some() => events.push((ev.u64("t").unwrap_or(0), ev)),
            _ => *st.report.not_mapped.entry("unparsed line".into()).or_default() += 1,
        }
    }
    events.sort_by_key(|e| e.0);
    st.report.events = events.len() as u64;
    let mut last = 0u64;
    // name -> (calls, ns)
    let mut services: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    // IPC (OT3): pipe names; the writes into each (pipe, reading end) as
    // (first byte, end, span); the posts not yet dequeued per (port,
    // context), oldest first.
    let mut pipe_names: HashMap<u64, String> = HashMap::new();
    let mut pipe_writes: PipeWrites = HashMap::new();
    let mut posts: Posts = HashMap::new();
    for (t, ev) in &events {
        let t = *t;
        last = last.max(t);
        let kind = ev.str("ev").unwrap_or("?").to_string();
        let pid = ev.u64("pid");
        let tid = ev.u64("tid").filter(|&t| t != 0);
        // The thread whose work emitted the event (0: a kernel thread).
        let by = ev.u64("by").filter(|&b| b != 0);
        if kind == "pipe create"
            && let (Some(p), Some(n)) = (ev.u64("pipe"), ev.str("name"))
        {
            pipe_names.insert(p, n.to_string());
        }
        // An IPC step: a zero-length span on the thread that did it.
        let ipc_span = |st: &mut State, name: &str, attrs: Attrs| {
            let at = by.and_then(|b| st.threads.get(&b).copied()).unwrap_or(root);
            let sp = st.new_span(Some(at), name.to_string(), t, attrs);
            st.spans[sp].end = Some(t);
            sp
        };
        let pipe_attrs = |pipe: u64| {
            let mut a: Attrs = vec![("gala.pipe.id".into(), Val::Int(pipe as i64))];
            if let Some(n) = pipe_names.get(&pipe) {
                a.push(("gala.pipe.name".into(), s(n)));
            }
            a
        };
        match kind.as_str() {
            "pipe write" | "pipe read" => {
                let (pipe, off, bytes) = (
                    ev.u64("pipe").unwrap_or(0),
                    ev.u64("off").unwrap_or(0),
                    ev.u64("bytes").unwrap_or(0),
                );
                let mut attrs = pipe_attrs(pipe);
                attrs.push(("gala.pipe.offset".into(), Val::Int(off as i64)));
                attrs.push(("messaging.message.body.size".into(), Val::Int(bytes as i64)));
                if kind == "pipe write" {
                    let to = ev.u64("to").unwrap_or(0);
                    attrs.push(("gala.pipe.to_end".into(), Val::Int(to as i64)));
                    let sp = ipc_span(&mut st, &kind, attrs);
                    pipe_writes.entry((pipe, to)).or_default().push((off, off + bytes, sp));
                } else {
                    let end = ev.u64("end").unwrap_or(0);
                    attrs.push(("gala.pipe.end".into(), Val::Int(end as i64)));
                    let sp = ipc_span(&mut st, &kind, attrs);
                    // The writes whose bytes this read took.
                    let from: Vec<usize> = pipe_writes
                        .get(&(pipe, end))
                        .map(|w| {
                            w.iter()
                                .filter(|&&(a, b, _)| a < off + bytes.max(1) && off < b.max(a + 1))
                                .map(|&(_, _, s)| s)
                                .collect()
                        })
                        .unwrap_or_default();
                    if from.is_empty() && bytes > 0 {
                        st.report.gap("pipe read without its write");
                    }
                    if let Some(&first) = from.first() {
                        let waited = t.saturating_sub(st.spans[first].start);
                        st.spans[sp]
                            .attrs
                            .push(("gala.pipe.waited_ns".into(), Val::Int(waited as i64)));
                        let name = pipe_names.get(&pipe).cloned().unwrap_or_default();
                        let writer = ev_thread_of(&st, first);
                        keep_slowest(&mut st.report.slow_pipes, waited, || {
                            format!(
                                "pipe {pipe} {name} end {end}, {bytes} bytes at offset {off}, written by thread {writer} at {} ns, read by thread {} at {t} ns",
                                st.spans[first].start,
                                by.unwrap_or(0)
                            )
                        });
                    }
                    for w in from {
                        st.spans[sp].links.push((w, vec![("gala.link".into(), s("data from"))]));
                    }
                }
                st.report.mapped(&kind, "span, linked write to read");
            }
            "iocp post" | "iocp dequeue" => {
                let (port, ctx) = (ev.u64("port").unwrap_or(0), ev.u64("ctx").unwrap_or(0));
                let mut attrs: Attrs = vec![
                    ("gala.iocp.port".into(), Val::Int(port as i64)),
                    ("gala.iocp.key".into(), hex(ev.u64("key").unwrap_or(0))),
                    ("gala.iocp.context".into(), hex(ctx)),
                    ("gala.iocp.bytes".into(), Val::Int(ev.u64("bytes").unwrap_or(0) as i64)),
                    (
                        "gala.iocp.status".into(),
                        Val::Str(format!("{:#010x}", ev.u64("status").unwrap_or(0) as u32)),
                    ),
                ];
                if kind == "iocp post" {
                    let sp = ipc_span(&mut st, &kind, attrs);
                    posts.entry((port, ctx)).or_default().push_back(sp);
                } else {
                    if let Some(q) = ev.u64("queued_t") {
                        let waited = t.saturating_sub(q);
                        attrs.push(("gala.iocp.queued_ns".into(), Val::Int(waited as i64)));
                        keep_slowest(&mut st.report.slow_completions, waited, || {
                            format!(
                                "port {port} context {ctx:#x}, queued by thread {} at {q} ns, taken by thread {} at {t} ns",
                                ev.u64("queued_by").unwrap_or(0),
                                by.unwrap_or(0)
                            )
                        });
                    }
                    if let Some(b) = ev.u64("queued_by") {
                        attrs.push(("gala.iocp.queued_by".into(), Val::Int(b as i64)));
                    }
                    let sp = ipc_span(&mut st, &kind, attrs);
                    match posts.get_mut(&(port, ctx)).and_then(|q| q.pop_front()) {
                        Some(post) => st.spans[sp]
                            .links
                            .push((post, vec![("gala.link".into(), s("completion of"))])),
                        None => st.report.gap("iocp dequeue without its post"),
                    }
                }
                st.report.mapped(&kind, "span, linked post to dequeue");
            }
            "process start" => {
                let pid = pid.unwrap_or(0);
                if let Some(&p) = st.procs.get(&pid)
                    && st.started.get(&p) == Some(&true)
                {
                    st.report.gap("process without exit (its pid started again)");
                    st.spans[p].attrs.push(("gala.unfinished".into(), Val::Bool(true)));
                    st.end_process(pid, t);
                }
                let p = st.process(pid, t);
                st.started.insert(p, true);
                let name = ev.str("name").unwrap_or("?");
                st.spans[p].name = format!("process {name}");
                let a = &mut st.spans[p].attrs;
                a.push(("process.executable.name".into(), s(name)));
                if let Some(c) = ev.str("cmdline") {
                    a.push(("process.command_line".into(), s(c)));
                }
                if let Some(v) = ev.u64("session") {
                    a.push(("gala.session".into(), Val::Int(v as i64)));
                }
                if let Some(v) = ev.u64("image_base") {
                    a.push(("gala.image_base".into(), hex(v)));
                }
                if let Some(v) = ev.u64("entry") {
                    a.push(("gala.entry".into(), hex(v)));
                }
                // The process tree: a child hangs off its parent process
                // and links the thread that created it (both may end
                // before it).
                if let Some(ppid) = ev.u64("ppid").filter(|&p| p != 0 && p != pid) {
                    let parent = st.process(ppid, t);
                    st.spans[p].parent = Some(parent);
                    st.spans[p].attrs.push(("process.parent_pid".into(), Val::Int(ppid as i64)));
                    if let Some(ptid) = ev.u64("ptid").filter(|&t| t != 0) {
                        st.spans[p].attrs.push(("gala.creator.tid".into(), Val::Int(ptid as i64)));
                        match st.threads.get(&ptid) {
                            Some(&creator) => st.spans[p]
                                .links
                                .push((creator, vec![("gala.link".into(), s("created by"))])),
                            None => st.report.gap("process creator thread not seen"),
                        }
                    }
                }
                st.report.mapped(&kind, "span start");
            }
            "process exit" => {
                let pid = pid.unwrap_or(0);
                if !st.procs.contains_key(&pid) {
                    st.report.gap("process exit without start");
                }
                let p = st.process(pid, t);
                let status = ev.u64("status").unwrap_or(0);
                let a = &mut st.spans[p].attrs;
                a.push(("process.exit.code".into(), Val::Int(i64::from(status as u32 as i32))));
                a.push(("gala.exit_status".into(), Val::Str(format!("{:#010x}", status as u32))));
                if status as u32 >= 0xc000_0000 {
                    st.spans[p].status = (2, format!("exited with status {:#010x}", status as u32));
                }
                let open: Vec<u64> = st.waits.keys().copied().collect();
                for w in open {
                    if st.threads.get(&w).is_some_and(|&th| st.spans[th].parent == Some(p)) {
                        st.flush_wait(w);
                    }
                }
                st.end_process(pid, t);
                st.report.mapped(&kind, "span end");
            }
            "thread create" => {
                let tid = tid.unwrap_or(0);
                if let Some(old) = st.threads.remove(&tid) {
                    st.report.gap("thread without exit (its tid was created again)");
                    st.spans[old].end = Some(t);
                }
                let parent = match pid {
                    Some(pid) => st.process(pid, t),
                    None => root,
                };
                let mut attrs = thread_attrs(pid, tid);
                if let Some(v) = ev.u64("start") {
                    attrs.push(("gala.start_address".into(), hex(v)));
                }
                if let Some(v) = ev.u64("teb") {
                    attrs.push(("gala.teb".into(), hex(v)));
                }
                let th = st.new_span(Some(parent), format!("thread {tid}"), t, attrs);
                st.threads.insert(tid, th);
                st.report.mapped(&kind, "span start");
            }
            "thread exit" => {
                let tid = tid.unwrap_or(0);
                if !st.threads.contains_key(&tid) {
                    st.report.gap("thread exit without create");
                }
                let th = st.thread(pid, tid, t);
                st.flush_wait(tid);
                st.threads.remove(&tid);
                st.spans[th].end = Some(t);
                if let Some(v) = ev.u64("status") {
                    st.spans[th]
                        .attrs
                        .push(("gala.exit_status".into(), Val::Str(format!("{:#010x}", v as u32))));
                }
                st.report.mapped(&kind, "span end");
            }
            "syscall" => {
                let ns = ev.u64("ns").unwrap_or(0);
                let start = t.saturating_sub(ns);
                let parent = match (tid, pid) {
                    (Some(tid), _) => st.thread(pid, tid, start),
                    (None, Some(pid)) => {
                        st.report.gap("system call without thread");
                        st.process(pid, start)
                    }
                    (None, None) => {
                        st.report.gap("system call without process");
                        root
                    }
                };
                if let Some(tid) = tid {
                    st.waits.remove(&tid);
                }
                let name = ev.str("name").unwrap_or("?").to_string();
                let status = ev.u64("status").unwrap_or(0) as u32;
                let mut attrs: Attrs = vec![
                    ("code.function.name".into(), s(&name)),
                    ("gala.syscall.nr".into(), Val::Int(ev.u64("nr").unwrap_or(0) as i64)),
                    ("gala.syscall.status".into(), Val::Str(format!("{status:#010x}"))),
                ];
                if let Some(Json::Arr(a)) = ev.get("args") {
                    let args = a
                        .iter()
                        .map(|v| match v {
                            Json::Num(n) => n.parse::<u64>().map_or_else(|_| s(n.clone()), hex),
                            _ => s("?"),
                        })
                        .collect();
                    attrs.push(("gala.syscall.args".into(), Val::Arr(args)));
                }
                if let Some(c) = ev.u64("cpu") {
                    attrs.push(("gala.cpu".into(), Val::Int(c as i64)));
                }
                if let Some(tid) = tid {
                    attrs.push(("thread.id".into(), Val::Int(tid as i64)));
                }
                if let Some(pid) = pid {
                    attrs.push(("process.pid".into(), Val::Int(pid as i64)));
                }
                let sp = st.new_span(Some(parent), name.clone(), start, attrs);
                st.spans[sp].end = Some(t);
                if status >= 0xc000_0000 {
                    st.spans[sp].status = (2, format!("{status:#010x}"));
                }
                let e = services.entry(name).or_default();
                e.0 += 1;
                e.1 += ns as i64;
                st.report.mapped(&kind, "span");
            }
            "wait" => {
                let Some(tid) = tid else {
                    st.report.gap("wait without thread");
                    st.report.mapped(&kind, "dropped: no thread");
                    continue;
                };
                st.thread(pid, tid, t);
                if st.waits.contains_key(&tid) {
                    st.flush_wait(tid);
                }
                let name = ev.str("name").unwrap_or("?").to_string();
                st.waits.insert(tid, (t, name, generic_attrs(ev)));
                st.report.mapped(&kind, "paired with its return; else span event");
            }
            "rec" => {
                // Record/replay data (R7), bytes for `um --replay`.
                *st.report.not_mapped.entry("rec (record/replay data)".into()).or_default() += 1;
            }
            _ => {
                let at = match (tid, by, pid) {
                    (Some(tid), _, _) if st.threads.contains_key(&tid) => st.threads[&tid],
                    // No subject thread: the thread that did the work.
                    (None, Some(by), _) if st.threads.contains_key(&by) => st.threads[&by],
                    (_, _, Some(pid)) => st.process(pid, t),
                    _ => root,
                };
                let (name, attrs) = match kind.as_str() {
                    "fault" => {
                        if tid.is_none() && by.is_none() {
                            st.report.gap("fault without thread");
                        }
                        let v = ev.u64("vector").unwrap_or(255);
                        let mut a = vec![
                            ("gala.fault.vector".into(), Val::Int(v as i64)),
                            ("gala.fault.rip".into(), hex(ev.u64("rip").unwrap_or(0))),
                            ("gala.fault.error".into(), hex(ev.u64("error").unwrap_or(0))),
                        ];
                        if let Some(c) = ev.u64("cr2") {
                            a.push(("gala.fault.address".into(), hex(c)));
                        }
                        (format!("fault {}", fault_name(v)), a)
                    }
                    "image" => {
                        let name = ev.str("name").or(ev.str("path")).unwrap_or("?");
                        (
                            format!("image load {}", name.rsplit('\\').next().unwrap_or(name)),
                            vec![
                                ("gala.image.name".into(), s(name)),
                                ("gala.image.base".into(), hex(ev.u64("base").unwrap_or(0))),
                                (
                                    "gala.image.size".into(),
                                    Val::Int(ev.u64("size").unwrap_or(0) as i64),
                                ),
                            ],
                        )
                    }
                    _ => (kind.clone(), generic_attrs(ev)),
                };
                let mut attrs = attrs;
                if let Some(c) = ev.u64("cpu")
                    && !attrs.iter().any(|(k, _)| k == "gala.cpu")
                {
                    attrs.push(("gala.cpu".into(), Val::Int(c as i64)));
                }
                st.spans[at].events.push(SpanEvent { t, name, attrs });
                st.report.span_events += 1;
                let how = match at {
                    0 => "span event on the boot",
                    _ if st.spans[at].name.starts_with("thread") => "span event on its thread",
                    _ => "span event on its process",
                };
                st.report.mapped(&kind, how);
            }
        }
    }

    // ---- the serial log: times, logs, crash reports, dumps, stats ----
    let lines: Vec<&str> = boot.serial.lines().collect();
    st.report.lines = lines.len() as u64;
    // Anchors: process starts and exits in the stream, in order per pid.
    let mut starts: HashMap<u64, Vec<u64>> = HashMap::new();
    let mut exits: HashMap<u64, Vec<u64>> = HashMap::new();
    for (t, ev) in &events {
        match ev.str("ev") {
            Some("process start") => starts.entry(ev.u64("pid").unwrap_or(0)).or_default().push(*t),
            Some("process exit") => exits.entry(ev.u64("pid").unwrap_or(0)).or_default().push(*t),
            _ => {}
        }
    }
    let mut seen_start: HashMap<u64, usize> = HashMap::new();
    let mut seen_exit: HashMap<u64, usize> = HashMap::new();
    let mut times = Vec::with_capacity(lines.len());
    let mut now = 0u64;
    for line in &lines {
        let (_, _, text) = level(line);
        let anchor = if let Some(pid) = text
            .strip_prefix("process ")
            .filter(|t| t.contains("): starting at"))
            .and_then(|t| number_after(t, "(pid "))
        {
            let k = seen_start.entry(pid).or_default();
            *k += 1;
            starts.get(&pid).and_then(|v| v.get(*k - 1)).copied()
        } else if let Some(pid) = text
            .strip_prefix("process ")
            .filter(|t| t.contains(") exited with status"))
            .and_then(|t| number_after(t, "(pid "))
        {
            let k = seen_exit.entry(pid).or_default();
            *k += 1;
            exits.get(&pid).and_then(|v| v.get(*k - 1)).copied()
        } else if let Some(ms) = number_after(text, "uptime_ms=") {
            Some(ms * 1_000_000)
        } else {
            number_after(text, "now ")
                .filter(|_| text.contains(" ms since boot"))
                .map(|ms| ms * 1_000_000)
        };
        if let Some(a) = anchor {
            now = now.max(a);
        }
        times.push(now);
    }
    last = last.max(now);

    let mut logs: Vec<LogRecord> = Vec::new();
    let mut metrics: Vec<Metric> = Vec::new();
    let mut root_attrs: Attrs = Vec::new();
    // The process span a line about `pid` belongs to (the latest started
    // before the line's time).
    let proc_at = |st: &State, pid: u64, t: u64| -> Option<usize> {
        let mine = st.proc_history.iter().filter(|(p, _)| *p == pid).map(|(_, i)| *i);
        mine.clone().rfind(|&i| st.spans[i].start <= t).or_else(|| mine.clone().next())
    };
    let mut i = 0;
    while i < lines.len() {
        let (sev, sev_text, text) = level(lines[i]);
        let t = times[i];
        let line_no = i + 1;
        if text.trim().is_empty() {
            i += 1;
            continue;
        }
        if let Some(v) = text.strip_prefix("gala-os kernel ") {
            root_attrs
                .push(("service.version".into(), s(v.split_whitespace().next().unwrap_or(v))));
        } else if let Some(c) = text.strip_prefix("cmdline: ") {
            root_attrs.push(("gala.cmdline".into(), s(c.trim_matches('"'))));
        } else if let Some(n) = text
            .strip_prefix("smp: ")
            .and_then(|r| number_after(r, ""))
            .filter(|_| text.contains("CPUs online ("))
        {
            root_attrs.push(("gala.cpus".into(), Val::Int(n as i64)));
        }
        // A crash report: one record and an exception event.
        if text.starts_with("crash report: ") && text.contains("ended by an unhandled exception") {
            let mut j = i + 1;
            while j < lines.len() && level(lines[j]).2.starts_with("crash report:   ") {
                j += 1;
            }
            let block: Vec<&str> = lines[i..j].iter().map(|l| level(l).2).collect();
            let pid = number_after(text, "(pid ").unwrap_or(0);
            let tid = number_after(text, "in thread ");
            let code = hex_after(text, "exception ").unwrap_or(0);
            let what = text
                .split(&format!("{code:#x} ("))
                .nth(1)
                .and_then(|r| r.split(')').next())
                .unwrap_or("");
            let detail =
                block.get(1).map(|l| l.trim_start_matches("crash report:").trim()).unwrap_or("");
            let p = proc_at(&st, pid, t);
            let at_t = p.and_then(|p| st.spans[p].end).unwrap_or(t);
            let span = p.map(|p| {
                tid.and_then(|tid| {
                    st.spans.iter().position(|s| {
                        s.parent == Some(p)
                            && s.attrs
                                .iter()
                                .any(|(k, v)| k == "thread.id" && *v == Val::Int(tid as i64))
                    })
                })
                .unwrap_or(p)
            });
            let mut attrs: Attrs = vec![
                ("exception.type".into(), Val::Str(format!("{code:#010x} ({what})"))),
                ("exception.message".into(), s(format!("{what} {detail}"))),
                (
                    "exception.stacktrace".into(),
                    s(block[1..]
                        .iter()
                        .map(|l| l.trim_start_matches("crash report:").trim())
                        .collect::<Vec<_>>()
                        .join("\n")),
                ),
                ("process.pid".into(), Val::Int(pid as i64)),
            ];
            if let Some(tid) = tid {
                attrs.push(("thread.id".into(), Val::Int(tid as i64)));
            }
            if let Some(addr) = hex_after(detail, "at ") {
                attrs.push(("gala.crash.address".into(), hex(addr)));
            }
            if let Some(loc) = detail.split('(').nth(1).and_then(|r| r.split(')').next()) {
                attrs.push(("gala.crash.location".into(), s(loc)));
            }
            let target = span.unwrap_or(root);
            st.spans[target].events.push(SpanEvent {
                t: at_t,
                name: "exception".into(),
                attrs: attrs.clone(),
            });
            st.spans[target].status = (2, format!("unhandled exception {code:#010x} ({what})"));
            st.report.span_events += 1;
            attrs.push(("gala.log.kind".into(), s("crash report")));
            attrs.push(("gala.log.line".into(), Val::Int(line_no as i64)));
            logs.push(LogRecord {
                t: at_t,
                severity: 17,
                text: "ERROR",
                body: block.join("\n"),
                attrs,
                span: Some(target),
            });
            i = j;
            continue;
        }
        // A state dump: from its begin to its end, one record.
        if text.starts_with("statedump: begin") {
            let mut j = i + 1;
            while j < lines.len() && !level(lines[j - 1]).2.starts_with("statedump: end") {
                j += 1;
            }
            let block: Vec<&str> = lines[i..j].iter().map(|l| level(l).2).collect();
            let body = block.join("\n");
            st.spans[root].events.push(SpanEvent {
                t,
                name: "state dump".into(),
                attrs: vec![
                    ("gala.statedump".into(), s(&body)),
                    ("gala.log.line".into(), Val::Int(line_no as i64)),
                ],
            });
            st.report.span_events += 1;
            logs.push(LogRecord {
                t,
                severity: 13,
                text: "WARN",
                body,
                attrs: vec![
                    ("gala.log.kind".into(), s("state dump")),
                    ("gala.log.line".into(), Val::Int(line_no as i64)),
                    ("gala.time.estimated".into(), Val::Bool(true)),
                ],
                span: Some(root),
            });
            i = j;
            continue;
        }
        // A kernel panic: its line and the backtrace after it.
        if let Some(msg) = text.strip_prefix("KERNEL PANIC: ") {
            let trace: Vec<&str> = lines[i + 1..]
                .iter()
                .map(|l| level(l).2)
                .take_while(|l| !l.is_empty())
                .take(80)
                .collect();
            let attrs: Attrs = vec![
                ("exception.type".into(), s("kernel panic")),
                ("exception.message".into(), s(msg)),
                ("exception.stacktrace".into(), s(trace.join("\n"))),
            ];
            st.spans[root].events.push(SpanEvent { t, name: "exception".into(), attrs });
            st.spans[root].status = (2, format!("kernel panic: {msg}"));
            st.report.span_events += 1;
        }
        // `stats:` figures as gauges.
        if let Some(rest) = text.strip_prefix("stats: ") {
            for (name, v, unit) in stats_figures(rest) {
                match metrics.iter_mut().find(|m| m.name == name) {
                    Some(m) => m.points.push((t, v, Vec::new())),
                    None => metrics.push(Metric {
                        name,
                        unit,
                        points: vec![(t, v, Vec::new())],
                        sum: false,
                    }),
                }
            }
        }
        let mut attrs: Attrs = vec![
            ("gala.log.line".into(), Val::Int(line_no as i64)),
            ("gala.time.estimated".into(), Val::Bool(true)),
        ];
        if sev == 0 {
            attrs.push(("gala.log.source".into(), s("program or firmware output")));
        }
        let pid = number_after(text, "(pid ").or_else(|| number_after(text, "pid "));
        let span = match pid.and_then(|pid| proc_at(&st, pid, t)) {
            Some(p) => {
                attrs.push(("process.pid".into(), Val::Int(pid.unwrap_or(0) as i64)));
                Some(p)
            }
            None => Some(root),
        };
        let (sev, sev_text) =
            if text.starts_with("KERNEL PANIC") { (21, "FATAL") } else { (sev, sev_text) };
        logs.push(LogRecord {
            t,
            severity: sev,
            text: sev_text,
            body: text.to_string(),
            attrs,
            span,
        });
        i += 1;
    }

    // ---- end of the boot: close what is open ----
    let open: Vec<u64> = st.waits.keys().copied().collect();
    for tid in open {
        st.flush_wait(tid);
    }
    for i in 0..st.spans.len() {
        if st.spans[i].end.is_none() {
            if i != root {
                st.report.gap(if st.spans[i].name.starts_with("thread") {
                    "thread without exit (open at the end)"
                } else {
                    "process without exit (open at the end)"
                });
                st.spans[i].attrs.push(("gala.unfinished".into(), Val::Bool(true)));
            }
            st.spans[i].end = Some(last);
        }
    }
    // A parent covers its children (a lazily made parent may start late).
    for i in (1..st.spans.len()).rev() {
        let (start, end) = (st.spans[i].start, st.spans[i].end.unwrap_or(last));
        if let Some(p) = st.spans[i].parent {
            st.spans[p].start = st.spans[p].start.min(start);
            st.spans[p].end = Some(st.spans[p].end.unwrap_or(last).max(end));
        }
    }
    let gaps = st.report.gaps.values().sum::<u64>();
    st.spans[root].attrs.push(("gala.otel.gaps".into(), Val::Int(gaps as i64)));
    let unmapped = st.report.not_mapped.values().sum::<u64>();
    st.spans[root].attrs.push(("gala.otel.not_mapped".into(), Val::Int(unmapped as i64)));

    // System calls per service: sums over the boot.
    let end = st.spans[root].end.unwrap_or(last);
    if !services.is_empty() {
        let mut calls = Metric {
            name: "gala.syscall.calls".into(),
            unit: "{call}",
            points: Vec::new(),
            sum: true,
        };
        let mut time = Metric {
            name: "gala.syscall.duration".into(),
            unit: "ns",
            points: Vec::new(),
            sum: true,
        };
        for (name, (n, ns)) in &services {
            let a = vec![("code.function.name".to_string(), s(name))];
            calls.points.push((end, *n, a.clone()));
            time.points.push((end, *ns, a));
        }
        metrics.push(calls);
        metrics.push(time);
    }

    // The converter's own report, as a log record on the boot.
    st.report.spans = st.spans.len() as u64;
    st.report.logs = logs.len() as u64 + 1;
    st.report.points = metrics.iter().map(|m| m.points.len() as u64).sum();
    logs.push(LogRecord {
        t: end,
        severity: if unmapped + gaps > 0 { 13 } else { 9 },
        text: if unmapped + gaps > 0 { "WARN" } else { "INFO" },
        body: st.report.lines(),
        attrs: vec![("gala.log.kind".into(), s("otel conversion report"))],
        span: Some(root),
    });

    // ---- encode ----
    let mut resource: Attrs = vec![
        ("service.name".into(), s("gala-os")),
        ("service.namespace".into(), s("gala")),
        ("service.instance.id".into(), s(&trace_id)),
        ("gala.scenario".into(), s(boot.scenario)),
        ("gala.kernel.build".into(), s(boot.build)),
        ("telemetry.sdk.name".into(), s("gala-xtask-otel")),
        ("telemetry.sdk.language".into(), s("rust")),
    ];
    for (k, v) in root_attrs {
        if !resource.iter().any(|(r, _)| *r == k) {
            resource.push((k, v));
        }
    }
    if !resource.iter().any(|(k, _)| k == "service.version") {
        resource.push(("service.version".into(), s("unknown")));
    }
    let base = boot.start_unix_ns;
    let mut res_json = String::from("{\"attributes\":");
    attrs_json(&mut res_json, &resource);
    res_json.push('}');

    let encode_span = |o: &mut String, sp: &Span| {
        let _ = write!(o, "{{\"traceId\":\"{trace_id}\",\"spanId\":\"{:016x}\"", sp.id);
        if let Some(p) = sp.parent {
            let _ = write!(o, ",\"parentSpanId\":\"{:016x}\"", st.spans[p].id);
        }
        o.push_str(",\"name\":");
        esc(o, &sp.name);
        let _ = write!(
            o,
            ",\"kind\":1,\"startTimeUnixNano\":\"{}\",\"endTimeUnixNano\":\"{}\",\"attributes\":",
            base + sp.start,
            base + sp.end.unwrap_or(last)
        );
        attrs_json(o, &sp.attrs);
        o.push_str(",\"events\":[");
        for (i, e) in sp.events.iter().enumerate() {
            if i > 0 {
                o.push(',');
            }
            let _ = write!(o, "{{\"timeUnixNano\":\"{}\",\"name\":", base + e.t);
            esc(o, &e.name);
            o.push_str(",\"attributes\":");
            attrs_json(o, &e.attrs);
            o.push('}');
        }
        o.push_str("],\"links\":[");
        for (i, (to, attrs)) in sp.links.iter().enumerate() {
            if i > 0 {
                o.push(',');
            }
            let _ = write!(
                o,
                "{{\"traceId\":\"{trace_id}\",\"spanId\":\"{:016x}\",\"attributes\":",
                st.spans[*to].id
            );
            attrs_json(o, attrs);
            o.push('}');
        }
        let _ = write!(o, "],\"status\":{{\"code\":{}", sp.status.0);
        if !sp.status.1.is_empty() {
            o.push_str(",\"message\":");
            esc(o, &sp.status.1);
        }
        o.push_str("}}");
    };
    let mut traces = Vec::new();
    for chunk in st.spans.chunks(SPANS_PER_REQUEST) {
        let mut o = format!(
            "{{\"resourceSpans\":[{{\"resource\":{res_json},\"scopeSpans\":[{{\"scope\":{SCOPE},\"spans\":["
        );
        for (i, sp) in chunk.iter().enumerate() {
            if i > 0 {
                o.push(',');
            }
            encode_span(&mut o, sp);
        }
        o.push_str("]}]}]}");
        traces.push(o);
    }
    let mut log_requests = Vec::new();
    for chunk in logs.chunks(LOGS_PER_REQUEST) {
        let mut o = format!(
            "{{\"resourceLogs\":[{{\"resource\":{res_json},\"scopeLogs\":[{{\"scope\":{SCOPE},\"logRecords\":["
        );
        for (i, l) in chunk.iter().enumerate() {
            if i > 0 {
                o.push(',');
            }
            let _ = write!(
                o,
                "{{\"timeUnixNano\":\"{}\",\"observedTimeUnixNano\":\"{}\",\"severityNumber\":{}",
                base + l.t,
                base + l.t,
                l.severity
            );
            if !l.text.is_empty() {
                let _ = write!(o, ",\"severityText\":\"{}\"", l.text);
            }
            o.push_str(",\"body\":{\"stringValue\":");
            esc(&mut o, &l.body);
            o.push_str("},\"attributes\":");
            attrs_json(&mut o, &l.attrs);
            if let Some(sp) = l.span {
                let _ = write!(
                    o,
                    ",\"traceId\":\"{trace_id}\",\"spanId\":\"{:016x}\"",
                    st.spans[sp].id
                );
            }
            o.push('}');
        }
        o.push_str("]}]}]}");
        log_requests.push(o);
    }
    let mut m = format!(
        "{{\"resourceMetrics\":[{{\"resource\":{res_json},\"scopeMetrics\":[{{\"scope\":{SCOPE},\"metrics\":["
    );
    for (i, metric) in metrics.iter().enumerate() {
        if i > 0 {
            m.push(',');
        }
        m.push_str("{\"name\":");
        esc(&mut m, &metric.name);
        let _ = write!(m, ",\"unit\":\"{}\",", metric.unit);
        if metric.sum {
            m.push_str(
                "\"sum\":{\"aggregationTemporality\":2,\"isMonotonic\":true,\"dataPoints\":[",
            );
        } else {
            m.push_str("\"gauge\":{\"dataPoints\":[");
        }
        for (j, (t, v, a)) in metric.points.iter().enumerate() {
            if j > 0 {
                m.push(',');
            }
            if metric.sum {
                let _ = write!(m, "{{\"startTimeUnixNano\":\"{base}\",");
            } else {
                m.push('{');
            }
            let _ =
                write!(m, "\"timeUnixNano\":\"{}\",\"asInt\":\"{v}\",\"attributes\":", base + t);
            attrs_json(&mut m, a);
            m.push('}');
        }
        m.push_str("]}}");
    }
    m.push_str("]}]}]}");
    Export { traces, logs: log_requests, metrics: m, report: st.report }
}

/// Spans per OTLP request (a collector's default body limit is 20 MiB;
/// a system call span is about 600 bytes).
pub const SPANS_PER_REQUEST: usize = 5000;
pub const LOGS_PER_REQUEST: usize = 5000;

/// POSTs the export to an OTLP/HTTP endpoint (`http://host:4318`, as in
/// `OTEL_EXPORTER_OTLP_ENDPOINT`; plain HTTP only).
pub fn post(endpoint: &str, export: &Export) -> Result<String, String> {
    let rest = endpoint.strip_prefix("http://").ok_or_else(|| {
        format!("{endpoint}: only http:// endpoints (OTLP/HTTP, JSON) are supported")
    })?;
    let (host, prefix) = match rest.split_once('/') {
        Some((h, p)) => (h, format!("/{}", p.trim_end_matches('/'))),
        None => (rest, String::new()),
    };
    let host = if host.contains(':') { host.to_string() } else { format!("{host}:80") };
    let mut sent = Vec::new();
    let bodies = export
        .traces
        .iter()
        .map(|b| ("traces", b))
        .chain(export.logs.iter().map(|b| ("logs", b)))
        .chain(std::iter::once(("metrics", &export.metrics)));
    for (signal, body) in bodies {
        let path = format!("{prefix}/v1/{signal}");
        let status = http_post(&host, &path, body)?;
        if !(200..300).contains(&status.0) {
            return Err(format!("POST {path} to {host}: HTTP {}: {}", status.0, status.1.trim()));
        }
        sent.push(format!("{signal} {} KiB", body.len() / 1024));
    }
    Ok(sent.join(", "))
}

fn http_post(host: &str, path: &str, body: &str) -> Result<(u16, String), String> {
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::Duration;
    let addr = host
        .to_socket_addrs()
        .map_err(|e| format!("{host}: {e}"))?
        .next()
        .ok_or_else(|| format!("{host}: no address"))?;
    let mut c = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
        .map_err(|e| format!("{host}: {e}"))?;
    c.set_read_timeout(Some(Duration::from_secs(30))).ok();
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    c.write_all(head.as_bytes())
        .and_then(|()| c.write_all(body.as_bytes()))
        .map_err(|e| format!("{host}: send: {e}"))?;
    let mut response = Vec::new();
    let _ = c.read_to_end(&mut response);
    let text = String::from_utf8_lossy(&response).into_owned();
    let code = text
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| format!("{host}: no HTTP response"))?;
    let body = text.split_once("\r\n\r\n").map_or("", |(_, b)| b).chars().take(300).collect();
    Ok((code, body))
}

#[cfg(test)]
mod tests;
