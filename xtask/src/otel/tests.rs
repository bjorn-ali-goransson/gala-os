//! The OTLP export of small event streams: ids consistent, parents set,
//! times in order, every span closed, the output shaped as the OTLP/JSON
//! mapping of opentelemetry-proto says.

use super::*;
use std::collections::HashSet;

const EVENTS: &str = r#"{"t":100,"cpu":0,"ev":"clock_cal","source":"HPET","tsc_hz":2800220428,"reliable":true}
{"t":1000,"cpu":1,"ev":"image","pid":4,"name":"crash.exe","base":5368709120,"size":24576}
{"t":2000,"cpu":1,"ev":"thread create","pid":4,"tid":8,"start":5368714720,"teb":140668769927168}
{"t":2100,"cpu":1,"ev":"process start","pid":4,"session":1,"name":"crash.exe","image_base":5368709120,"entry":6442459824,"cmdline":"C:\\gala\\crash.exe -x \"q\""}
{"t":2500,"cpu":1,"ev":"fault","pid":4,"vector":14,"rip":6442459895,"cr2":6442529296,"error":7}
{"t":3000,"cpu":1,"ev":"syscall","pid":4,"tid":8,"nr":24,"name":"NtAllocateVirtualMemory","args":[18446744073709551615,2095664,null],"status":0,"ns":400}
{"t":3500,"cpu":0,"ev":"thread create","pid":4,"tid":12,"start":5368714720,"teb":140668769935360}
{"t":3600,"cpu":0,"ev":"wait","pid":4,"tid":12,"nr":4,"name":"NtWaitForSingleObject","args":[20,0,0],"status":0,"ns":10}
{"t":3700,"cpu":0,"ev":"wait","pid":4,"tid":8,"nr":4,"name":"NtWaitForSingleObject","args":[24,0,0],"status":0,"ns":10}
{"t":3900,"cpu":1,"ev":"syscall","pid":4,"tid":8,"nr":4,"name":"NtWaitForSingleObject","args":[24,0,0],"status":0,"ns":250}
{"t":4000,"cpu":1,"ev":"syscall","pid":4,"tid":8,"nr":51,"name":"NtOpenFile","args":[1,2],"status":3221225524,"ns":100}
{"t":4200,"cpu":1,"ev":"rec","hex":"0400000000000000"}
{"t":4300,"cpu":1,"ev":"tcp accept","socket":1,"local":"10.0.2.15:22","remote":"10.0.2.2:53576"}
{"t":4400,"cpu":1,"ev":"thread exit","pid":4,"tid":8,"status":3221225477}
{"t":4500,"cpu":1,"ev":"process exit","pid":4,"session":1,"status":3221225477}
{"t":5000,"cpu":1,"ev":"syscall","pid":9,"tid":20,"nr":15,"name":"NtClose","args":[4],"status":0,"ns":50}
not json at all
"#;

const SERIAL: &str = "gala-os kernel 0.1.0
[info ] cmdline: \"qemu_exit run=crash.exe noscreen\"
[info ] smp: 2 CPUs online (2 reported by the bootloader, 2 in the MADT; 1119 ms)
[info ] process crash.exe (pid 4, session 1): starting at 0x140001040
crash: writing to address 0
[warn ] user exception: page fault (#PF) at rip 0x140001015 (cr2 0x0, error 0x6)
[info ] crash report: crash.exe (pid 4) ended by an unhandled exception 0xc0000005 (access violation) in thread 8
[info ] crash report:   at 0x140001015 (crash.exe+0x1015), writing address 0x0
[info ] crash report:   rax=0x0000000000000000 rbx=0x0000000000000000
[info ] crash report:   [rsp+0x28] 0x140001049 (crash.exe+0x1049)
[info ] process crash.exe (pid 4) exited with status 0xc0000005
[info ] stats: 1 TLB shootdowns (1 IPIs, 0 after a move to another CPU), 0 frames leaked by deferred frees, 0 freed twice; heap 451 KiB in use, 102 large pages, 0 mapped-window flushes; frames free 121128 of 131072; heap peak 543 KiB, fewest frames free 121070
statedump: begin (NMI on CPU 0; CPUs answered 0x3 of online 0x3; now 7 ms since boot)
statedump: cpu 0 kernel rip 0xffffffff800acf32 rsp 0xffffd0000001ffd0
[info ] statedump: end
[info ] stats: 1 TLB shootdowns (1 IPIs, 0 after a move to another CPU), 0 frames leaked by deferred frees, 0 freed twice; heap 450 KiB in use, 102 large pages, 0 mapped-window flushes; frames free 121128 of 131072; heap peak 543 KiB, fewest frames free 121070
gala: boot ok
";

const BASE: u64 = 1_700_000_000_000_000_000;

fn export() -> Export {
    convert(&Boot {
        serial: SERIAL,
        events: EVENTS,
        scenario: "crash",
        build: "debug 1234567",
        start_unix_ns: BASE,
        symbols: None,
    })
}

fn ns(v: &Json, key: &str) -> u64 {
    match v.get(key) {
        Some(Json::Str(s)) => s.parse().expect("a decimal string"),
        other => panic!("{key}: {other:?}"),
    }
}

fn arr<'a>(v: &'a Json, key: &str) -> &'a [Json] {
    match v.get(key) {
        Some(Json::Arr(a)) => a,
        other => panic!("{key} is not an array: {other:?}"),
    }
}

fn text<'a>(v: &'a Json, key: &str) -> &'a str {
    match v.get(key) {
        Some(Json::Str(s)) => s,
        other => panic!("{key} is not a string: {other:?}"),
    }
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit()) && s.bytes().any(|b| b != b'0')
}

/// An `AnyValue`: exactly one of the value fields.
fn check_value(v: &Json) {
    let Json::Obj(m) = v else { panic!("value is not an object: {v:?}") };
    assert_eq!(m.len(), 1, "one value field: {v:?}");
    match (m[0].0.as_str(), &m[0].1) {
        ("stringValue", Json::Str(_)) | ("boolValue", Json::Bool(_)) => {}
        ("intValue", Json::Str(s)) => assert!(s.parse::<i64>().is_ok(), "int64 as a string: {s}"),
        ("arrayValue", a) => arr(a, "values").iter().for_each(check_value),
        other => panic!("not an AnyValue: {other:?}"),
    }
}

fn check_attrs(v: &Json) -> BTreeMap<String, Json> {
    let mut out = BTreeMap::new();
    for kv in v.get("attributes").map_or(&[][..], |_| arr(v, "attributes")) {
        let key = text(kv, "key").to_string();
        let value = kv.get("value").expect("attribute value");
        check_value(value);
        assert!(out.insert(key.clone(), value.clone()).is_none(), "duplicate attribute {key}");
    }
    out
}

fn attr_str(attrs: &BTreeMap<String, Json>, key: &str) -> Option<String> {
    attrs.get(key).and_then(|v| match v.get("stringValue").or(v.get("intValue")) {
        Some(Json::Str(s)) => Some(s.clone()),
        _ => None,
    })
}

/// The one resource of each request (and the same one in all of them).
fn resource(req: &Json, top: &str, scope: &str) -> (BTreeMap<String, Json>, Json) {
    let rs = arr(req, top);
    assert_eq!(rs.len(), 1, "one resource per boot");
    let attrs = check_attrs(rs[0].get("resource").expect("resource"));
    let scopes = arr(&rs[0], scope);
    assert_eq!(scopes.len(), 1);
    assert_eq!(text(scopes[0].get("scope").unwrap(), "name"), "gala-xtask-otel");
    (attrs, scopes[0].clone())
}

#[derive(Debug)]
struct S {
    trace: String,
    id: String,
    parent: Option<String>,
    name: String,
    start: u64,
    end: u64,
    attrs: BTreeMap<String, Json>,
    events: Vec<(u64, String, BTreeMap<String, Json>)>,
    /// (span id, attributes) of each link.
    links: Vec<(String, BTreeMap<String, Json>)>,
    status: u64,
}

fn spans(e: &Export) -> Vec<S> {
    let mut out = Vec::new();
    for body in &e.traces {
        let req = parse_json(body).expect("traces are JSON");
        let (res, scope) = resource(&req, "resourceSpans", "scopeSpans");
        assert_eq!(attr_str(&res, "service.name").as_deref(), Some("gala-os"));
        for sp in arr(&scope, "spans") {
            let kind = sp.get("kind");
            assert_eq!(kind, Some(&Json::Num("1".into())), "SPAN_KIND_INTERNAL");
            let status = match sp.get("status").and_then(|s| s.get("code")) {
                Some(Json::Num(n)) => n.parse().unwrap(),
                other => panic!("status code {other:?}"),
            };
            let events = sp
                .get("events")
                .map_or(&[][..], |_| arr(sp, "events"))
                .iter()
                .map(|e| (ns(e, "timeUnixNano"), text(e, "name").to_string(), check_attrs(e)))
                .collect();
            let links = sp
                .get("links")
                .map_or(&[][..], |_| arr(sp, "links"))
                .iter()
                .map(|l| {
                    assert_eq!(text(l, "traceId"), text(sp, "traceId"), "links stay in the trace");
                    (text(l, "spanId").to_string(), check_attrs(l))
                })
                .collect();
            out.push(S {
                links,
                trace: text(sp, "traceId").to_string(),
                id: text(sp, "spanId").to_string(),
                parent: sp.get("parentSpanId").map(|_| text(sp, "parentSpanId").to_string()),
                name: text(sp, "name").to_string(),
                start: ns(sp, "startTimeUnixNano"),
                end: ns(sp, "endTimeUnixNano"),
                attrs: check_attrs(sp),
                events,
                status,
            });
        }
    }
    out
}

#[test]
fn spans_have_one_trace_unique_ids_and_parents_that_exist() {
    let all = spans(&export());
    let trace = &all[0].trace;
    assert!(is_hex(trace, 32), "trace id {trace}");
    let ids: HashSet<&str> = all.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids.len(), all.len(), "span ids unique");
    let roots: Vec<&S> = all.iter().filter(|s| s.parent.is_none()).collect();
    assert_eq!(roots.len(), 1, "one root: the boot");
    assert_eq!(roots[0].name, "boot crash");
    for s in &all {
        assert_eq!(&s.trace, trace);
        assert!(is_hex(&s.id, 16), "span id {}", s.id);
        if let Some(p) = &s.parent {
            assert!(ids.contains(p.as_str()), "{}'s parent {p} exists", s.name);
        }
    }
}

#[test]
fn spans_are_closed_and_inside_their_parents() {
    let all = spans(&export());
    let by_id: HashMap<&str, &S> = all.iter().map(|s| (s.id.as_str(), s)).collect();
    for s in &all {
        assert!(s.start >= BASE && s.start <= s.end, "{}: {}..{}", s.name, s.start, s.end);
        if let Some(p) = &s.parent {
            let p = by_id[p.as_str()];
            assert!(p.start <= s.start && s.end <= p.end, "{} inside {}", s.name, p.name);
        }
        for (t, name, _) in &s.events {
            assert!(*t >= BASE, "{name}");
        }
    }
}

#[test]
fn processes_threads_and_system_calls_nest() {
    let all = spans(&export());
    let find = |name: &str| all.iter().find(|s| s.name == name).unwrap_or_else(|| panic!("{name}"));
    let boot = find("boot crash");
    let process = find("process crash.exe");
    assert_eq!(process.parent.as_deref(), Some(boot.id.as_str()));
    assert_eq!(attr_str(&process.attrs, "process.pid").as_deref(), Some("4"));
    assert_eq!(attr_str(&process.attrs, "process.exit.code").as_deref(), Some("-1073741819"));
    assert_eq!(
        attr_str(&process.attrs, "process.command_line").as_deref(),
        Some("C:\\gala\\crash.exe -x \"q\"")
    );
    // Image load before the start event: the process span starts there.
    assert_eq!(process.start, BASE + 1000);
    assert_eq!(process.end, BASE + 4500);
    assert_eq!(process.status, 2, "crashed: error");
    let t8 = find("thread 8");
    assert_eq!(t8.parent.as_deref(), Some(process.id.as_str()));
    assert_eq!((t8.start, t8.end), (BASE + 2000, BASE + 4400));
    let alloc = find("NtAllocateVirtualMemory");
    assert_eq!(alloc.parent.as_deref(), Some(t8.id.as_str()));
    assert_eq!((alloc.start, alloc.end), (BASE + 2600, BASE + 3000));
    assert_eq!(
        alloc.attrs["gala.syscall.args"],
        parse_json(
            r#"{"arrayValue":{"values":[{"stringValue":"0xffffffffffffffff"},{"stringValue":"0x1ffa30"},{"stringValue":"?"}]}}"#
        )
        .unwrap()
    );
    let open = find("NtOpenFile");
    assert_eq!(open.status, 2, "STATUS_OBJECT_NAME_NOT_FOUND is an error");
    assert_eq!(attr_str(&open.attrs, "gala.syscall.status").as_deref(), Some("0xc0000034"));
    // The returned wait is one span; the one that never returned (tid 12,
    // ended with its process) is a span event on its thread.
    assert_eq!(all.iter().filter(|s| s.name == "NtWaitForSingleObject").count(), 1);
    let t12 = find("thread 12");
    assert_eq!(attr_str(&t12.attrs, "gala.unfinished").as_deref(), None);
    assert!(t12.attrs.contains_key("gala.unfinished"));
    assert!(t12.events.iter().any(|e| e.1 == "NtWaitForSingleObject (entered, no return)"));
    // The fault (no tid) is on the process; the image load too.
    assert!(process.events.iter().any(|e| e.1 == "fault #PF page fault"));
    assert!(process.events.iter().any(|e| e.1 == "image load crash.exe"));
    // The crash report: an exception event on the crashing thread.
    let ex = t8.events.iter().find(|e| e.1 == "exception").expect("exception event");
    assert_eq!(attr_str(&ex.2, "exception.type").as_deref(), Some("0xc0000005 (access violation)"));
    assert!(attr_str(&ex.2, "exception.stacktrace").unwrap().contains("crash.exe+0x1049"));
    assert_eq!(attr_str(&ex.2, "gala.crash.location").as_deref(), Some("crash.exe+0x1015"));
    // An event without a pid goes on the boot.
    assert!(boot.events.iter().any(|e| e.1 == "tcp accept"));
    assert!(boot.events.iter().any(|e| e.1 == "clock_cal"));
    assert!(boot.events.iter().any(|e| e.1 == "state dump"));
    // A thread seen only in a system call gets a span (and a gap).
    let t20 = find("thread 20");
    assert_eq!(find("NtClose").parent.as_deref(), Some(t20.id.as_str()));
}

#[test]
fn times_are_monotonic_per_thread() {
    let all = spans(&export());
    let mut by_thread: BTreeMap<String, Vec<(u64, u64)>> = BTreeMap::new();
    for s in all.iter().filter(|s| s.attrs.contains_key("gala.syscall.nr")) {
        by_thread.entry(s.parent.clone().unwrap()).or_default().push((s.start, s.end));
    }
    for calls in by_thread.values() {
        for w in calls.windows(2) {
            assert!(w[0].1 <= w[1].0, "system calls of a thread do not overlap: {w:?}");
        }
    }
}

#[test]
fn the_report_counts_what_was_not_mapped_and_the_gaps() {
    let r = export().report;
    assert_eq!(r.not_mapped.get("rec (record/replay data)"), Some(&1));
    assert_eq!(r.not_mapped.get("unparsed line"), Some(&1));
    assert_eq!(r.mapped["syscall"].0, 4);
    assert_eq!(r.gaps.get("wait without return"), Some(&1));
    assert_eq!(r.gaps.get("fault without thread"), Some(&1));
    assert_eq!(r.gaps.get("thread without exit (ended with its process)"), Some(&1));
    assert_eq!(r.gaps.get("thread seen before its create event"), Some(&1));
    let lines = r.lines();
    assert!(
        lines.contains("otel: NOT mapped: rec (record/replay data) 1, unparsed line 1"),
        "{lines}"
    );
    assert!(lines.contains("gaps in the stream (OT2)"), "{lines}");
}

#[test]
fn logs_carry_severity_trace_context_and_whole_blocks() {
    let e = export();
    assert_eq!(e.logs.len(), 1);
    let req = parse_json(&e.logs[0]).expect("logs are JSON");
    let (_, scope) = resource(&req, "resourceLogs", "scopeLogs");
    let records = arr(&scope, "logRecords");
    let trace = &spans(&e)[0].trace;
    let mut last = 0;
    for r in records {
        let t = ns(r, "timeUnixNano");
        assert!(t >= last, "log times in order");
        last = t;
        assert_eq!(text(r, "traceId"), trace);
        assert!(is_hex(text(r, "spanId"), 16));
        check_attrs(r);
        assert!(matches!(r.get("severityNumber"), Some(Json::Num(_))));
    }
    let bodies: Vec<&str> =
        records.iter().map(|r| text(r.get("body").unwrap(), "stringValue")).collect();
    let crash = bodies.iter().find(|b| b.starts_with("crash report:")).expect("crash record");
    assert_eq!(crash.lines().count(), 4, "the crash report is one record");
    let dump = bodies.iter().find(|b| b.starts_with("statedump: begin")).expect("dump record");
    assert_eq!(dump.lines().count(), 3, "the state dump is one record");
    assert!(bodies.contains(&"crash: writing to address 0"));
    assert!(bodies.iter().any(|b| b.starts_with("otel: ")), "the converter's report");
    // The line after the dump's "now 7 ms" is placed at 7 ms.
    let boot_ok = records
        .iter()
        .find(|r| text(r.get("body").unwrap(), "stringValue") == "gala: boot ok")
        .unwrap();
    assert_eq!(ns(boot_ok, "timeUnixNano"), BASE + 7_000_000);
}

#[test]
fn stats_lines_become_gauges_and_system_calls_sums() {
    let e = export();
    let req = parse_json(&e.metrics).expect("metrics are JSON");
    let (res, scope) = resource(&req, "resourceMetrics", "scopeMetrics");
    assert_eq!(attr_str(&res, "service.version").as_deref(), Some("0.1.0"));
    assert_eq!(attr_str(&res, "gala.scenario").as_deref(), Some("crash"));
    assert_eq!(attr_str(&res, "gala.kernel.build").as_deref(), Some("debug 1234567"));
    assert_eq!(attr_str(&res, "gala.cpus").as_deref(), Some("2"));
    let metrics = arr(&scope, "metrics");
    let get = |name: &str| {
        metrics.iter().find(|m| text(m, "name") == name).unwrap_or_else(|| panic!("{name}"))
    };
    let heap = get("gala.stats.heap_in_use");
    assert_eq!(text(heap, "unit"), "KiBy");
    let points = arr(heap.get("gauge").unwrap(), "dataPoints");
    assert_eq!(points.len(), 2);
    assert_eq!(text(&points[1], "asInt"), "450");
    let total = get("gala.stats.frames_free.total");
    assert_eq!(text(&arr(total.get("gauge").unwrap(), "dataPoints")[0], "asInt"), "131072");
    get("gala.stats.ipis");
    get("gala.stats.fewest_frames_free");
    let calls = get("gala.syscall.calls").get("sum").unwrap();
    assert_eq!(calls.get("isMonotonic"), Some(&Json::Bool(true)));
    assert_eq!(calls.get("aggregationTemporality"), Some(&Json::Num("2".into())));
    for p in arr(calls, "dataPoints") {
        assert!(ns(p, "startTimeUnixNano") <= ns(p, "timeUnixNano"));
    }
}

#[test]
fn a_boot_converts_to_the_same_ids_twice_and_splits_large_traces() {
    assert_eq!(export().traces, export().traces);
    let mut events = String::new();
    for i in 0..(SPANS_PER_REQUEST as u64 + 10) {
        events += &format!(
            "{{\"t\":{},\"cpu\":0,\"ev\":\"syscall\",\"pid\":4,\"tid\":8,\"nr\":15,\"name\":\"NtClose\",\"args\":[4],\"status\":0,\"ns\":5}}\n",
            100 + i * 10
        );
    }
    let e = convert(&Boot {
        serial: "",
        events: &events,
        scenario: "x",
        build: "",
        start_unix_ns: 0,
        symbols: None,
    });
    assert_eq!(e.traces.len(), 2);
    let all = spans(&e);
    let ids: HashSet<&str> = all.iter().map(|s| s.id.as_str()).collect();
    for s in &all {
        if let Some(p) = &s.parent {
            assert!(ids.contains(p.as_str()));
        }
    }
}

#[test]
fn a_serial_log_alone_still_converts() {
    let e = convert(&Boot {
        serial: SERIAL,
        events: "",
        scenario: "crash",
        build: "",
        start_unix_ns: 5,
        symbols: None,
    });
    let all = spans(&e);
    assert_eq!(all.len(), 1, "only the boot without a stream");
    assert!(all[0].events.iter().any(|e| e.1 == "exception"), "the crash still shows");
    assert!(e.report.lines().contains("0 events"));
}

#[test]
fn stats_figures_are_named_by_their_words() {
    let f = stats_figures("heap 451 KiB in use, 102 large pages; frames free 5 of 9");
    assert_eq!(
        f,
        vec![
            ("gala.stats.heap_in_use".to_string(), 451, "KiBy"),
            ("gala.stats.large_pages".to_string(), 102, "1"),
            ("gala.stats.frames_free.total".to_string(), 9, "1"),
            ("gala.stats.frames_free".to_string(), 5, "1"),
        ]
    );
}

#[test]
fn json_reader_keeps_large_numbers_and_escapes() {
    let v =
        parse_json(r#"{"a":18446744073709551615,"b":"x\"\\\u0041","c":[1,null,true]}"#).unwrap();
    assert_eq!(v.get("a"), Some(&Json::Num("18446744073709551615".into())));
    assert_eq!(v.get("b"), Some(&Json::Str("x\"\\A".into())));
    assert!(parse_json("{\"a\":1} x").is_err());
    let mut o = String::new();
    esc(&mut o, "a\"b\\c\n\u{1}");
    assert_eq!(parse_json(&o).unwrap(), Json::Str("a\"b\\c\n\u{1}".into()));
}

/// What an OTLP/JSON field holds (opentelemetry-proto's messages, the
/// fields this export may use; the JSON mapping: 64-bit integers as
/// decimal strings, enums as numbers, trace and span ids in hex).
#[derive(Clone, Copy)]
enum Kind {
    Msg(&'static str),
    Msgs(&'static str),
    Str,
    U64,
    I64,
    Enum(u64),
    Bool,
    TraceId,
    SpanId,
    /// A trace or span id that may be all zeros (a profiles dictionary's
    /// zero link), by its hex length.
    IdOrZero(usize),
    /// int32 (a JSON number) and repeated int32.
    I32,
    I32s,
    /// repeated int64 / fixed64 (decimal strings), repeated string.
    I64s,
    U64s,
    Strs,
    /// bytes (base64).
    Bytes,
}

fn schema(message: &str) -> &'static [(&'static str, Kind)] {
    use Kind::*;
    match message {
        "ExportTraceServiceRequest" => &[("resourceSpans", Msgs("ResourceSpans"))],
        "ResourceSpans" => &[("resource", Msg("Resource")), ("scopeSpans", Msgs("ScopeSpans"))],
        "ScopeSpans" => &[("scope", Msg("InstrumentationScope")), ("spans", Msgs("Span"))],
        "ExportLogsServiceRequest" => &[("resourceLogs", Msgs("ResourceLogs"))],
        "ResourceLogs" => &[("resource", Msg("Resource")), ("scopeLogs", Msgs("ScopeLogs"))],
        "ScopeLogs" => &[("scope", Msg("InstrumentationScope")), ("logRecords", Msgs("LogRecord"))],
        "ExportMetricsServiceRequest" => &[("resourceMetrics", Msgs("ResourceMetrics"))],
        "ResourceMetrics" => {
            &[("resource", Msg("Resource")), ("scopeMetrics", Msgs("ScopeMetrics"))]
        }
        "ScopeMetrics" => &[("scope", Msg("InstrumentationScope")), ("metrics", Msgs("Metric"))],
        "Resource" => &[("attributes", Msgs("KeyValue"))],
        "InstrumentationScope" => &[("name", Str), ("version", Str)],
        "KeyValue" => &[("key", Str), ("value", Msg("AnyValue"))],
        "AnyValue" => &[
            ("stringValue", Str),
            ("boolValue", Bool),
            ("intValue", I64),
            ("arrayValue", Msg("ArrayValue")),
        ],
        "ArrayValue" => &[("values", Msgs("AnyValue"))],
        "Span" => &[
            ("traceId", TraceId),
            ("spanId", SpanId),
            ("parentSpanId", SpanId),
            ("name", Str),
            ("kind", Enum(5)),
            ("startTimeUnixNano", U64),
            ("endTimeUnixNano", U64),
            ("attributes", Msgs("KeyValue")),
            ("events", Msgs("Event")),
            ("links", Msgs("Link")),
            ("status", Msg("Status")),
        ],
        "Link" => &[("traceId", TraceId), ("spanId", SpanId), ("attributes", Msgs("KeyValue"))],
        "Event" => &[("timeUnixNano", U64), ("name", Str), ("attributes", Msgs("KeyValue"))],
        "Status" => &[("message", Str), ("code", Enum(2))],
        "LogRecord" => &[
            ("timeUnixNano", U64),
            ("observedTimeUnixNano", U64),
            ("severityNumber", Enum(24)),
            ("severityText", Str),
            ("body", Msg("AnyValue")),
            ("attributes", Msgs("KeyValue")),
            ("traceId", TraceId),
            ("spanId", SpanId),
        ],
        "Metric" => &[("name", Str), ("unit", Str), ("gauge", Msg("Gauge")), ("sum", Msg("Sum"))],
        "Gauge" => &[("dataPoints", Msgs("NumberDataPoint"))],
        "Sum" => &[
            ("dataPoints", Msgs("NumberDataPoint")),
            ("aggregationTemporality", Enum(2)),
            ("isMonotonic", Bool),
        ],
        "NumberDataPoint" => &[
            ("attributes", Msgs("KeyValue")),
            ("startTimeUnixNano", U64),
            ("timeUnixNano", U64),
            ("asInt", I64),
        ],
        // Profiles (opentelemetry-proto profiles/v1development).
        "ExportProfilesServiceRequest" => &[
            ("resourceProfiles", Msgs("ResourceProfiles")),
            ("dictionary", Msg("ProfilesDictionary")),
        ],
        "ResourceProfiles" => {
            &[("resource", Msg("Resource")), ("scopeProfiles", Msgs("ScopeProfiles"))]
        }
        "ScopeProfiles" => &[("scope", Msg("InstrumentationScope")), ("profiles", Msgs("Profile"))],
        "Profile" => &[
            ("sampleType", Msg("ValueType")),
            ("samples", Msgs("Sample")),
            ("timeUnixNano", U64),
            ("durationNano", U64),
            ("periodType", Msg("ValueType")),
            ("period", I64),
            ("profileId", Bytes),
            ("attributeIndices", I32s),
        ],
        "ValueType" => &[("typeStrindex", I32), ("unitStrindex", I32)],
        "Sample" => &[
            ("stackIndex", I32),
            ("attributeIndices", I32s),
            ("linkIndex", I32),
            ("values", I64s),
            ("timestampsUnixNano", U64s),
        ],
        "ProfilesDictionary" => &[
            ("mappingTable", Msgs("Mapping")),
            ("locationTable", Msgs("Location")),
            ("functionTable", Msgs("Function")),
            ("linkTable", Msgs("ProfileLink")),
            ("stringTable", Strs),
            ("attributeTable", Msgs("KeyValueAndUnit")),
            ("stackTable", Msgs("Stack")),
        ],
        "Mapping" => &[
            ("memoryStart", U64),
            ("memoryLimit", U64),
            ("fileOffset", U64),
            ("filenameStrindex", I32),
            ("attributeIndices", I32s),
        ],
        "Location" => &[
            ("mappingIndex", I32),
            ("address", U64),
            ("lines", Msgs("Line")),
            ("attributeIndices", I32s),
        ],
        "Line" => &[("functionIndex", I32), ("line", I64), ("column", I64)],
        "Function" => &[
            ("nameStrindex", I32),
            ("systemNameStrindex", I32),
            ("filenameStrindex", I32),
            ("startLine", I64),
        ],
        "ProfileLink" => &[("traceId", IdOrZero(32)), ("spanId", IdOrZero(16))],
        "KeyValueAndUnit" => {
            &[("keyStrindex", I32), ("value", Msg("AnyValue")), ("unitStrindex", I32)]
        }
        "Stack" => &[("locationIndices", I32s)],
        other => panic!("no schema for {other}"),
    }
}

/// Checks `v` as an OTLP/JSON `message`: no unknown fields, each of the
/// right type; an `AnyValue` has exactly one value.
fn check_message(v: &Json, message: &str, path: &str) {
    let Json::Obj(fields) = v else { panic!("{path}: {message} is not an object") };
    if message == "AnyValue" {
        assert_eq!(fields.len(), 1, "{path}: an AnyValue has one value");
    }
    for (name, value) in fields {
        let at = format!("{path}.{name}");
        let Some(&(_, kind)) = schema(message).iter().find(|(f, _)| f == name) else {
            panic!("{at}: not a field of {message}")
        };
        match (kind, value) {
            (Kind::Msg(m), v) => check_message(v, m, &at),
            (Kind::Msgs(m), Json::Arr(a)) => {
                a.iter().enumerate().for_each(|(i, v)| check_message(v, m, &format!("{at}[{i}]")))
            }
            (Kind::Str, Json::Str(_)) | (Kind::Bool, Json::Bool(_)) => {}
            (Kind::U64, Json::Str(s)) => assert!(s.parse::<u64>().is_ok(), "{at}: {s}"),
            (Kind::I64, Json::Str(s)) => assert!(s.parse::<i64>().is_ok(), "{at}: {s}"),
            (Kind::Enum(max), Json::Num(n)) => {
                assert!(n.parse::<u64>().is_ok_and(|n| n <= max), "{at}: {n}")
            }
            (Kind::TraceId, Json::Str(s)) => assert!(is_hex(s, 32), "{at}: {s}"),
            (Kind::SpanId, Json::Str(s)) => assert!(is_hex(s, 16), "{at}: {s}"),
            (Kind::IdOrZero(len), Json::Str(s)) => {
                assert!(s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit()), "{at}: {s}")
            }
            (Kind::I32, Json::Num(n)) => assert!(n.parse::<i32>().is_ok(), "{at}: {n}"),
            (Kind::I32s, Json::Arr(a)) => {
                assert!(
                    a.iter().all(|v| matches!(v, Json::Num(n) if n.parse::<i32>().is_ok())),
                    "{at}"
                )
            }
            (Kind::I64s, Json::Arr(a)) => {
                assert!(
                    a.iter().all(|v| matches!(v, Json::Str(n) if n.parse::<i64>().is_ok())),
                    "{at}"
                )
            }
            (Kind::U64s, Json::Arr(a)) => {
                assert!(
                    a.iter().all(|v| matches!(v, Json::Str(n) if n.parse::<u64>().is_ok())),
                    "{at}"
                )
            }
            (Kind::Strs, Json::Arr(a)) => {
                assert!(a.iter().all(|v| matches!(v, Json::Str(_))), "{at}")
            }
            (Kind::Bytes, Json::Str(s)) => assert!(
                !s.is_empty()
                    && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b)),
                "{at}: {s}"
            ),
            (_, v) => panic!("{at}: wrong type {v:?}"),
        }
    }
}

#[test]
fn the_output_follows_the_otlp_json_schema() {
    let e = export();
    for t in &e.traces {
        check_message(&parse_json(t).unwrap(), "ExportTraceServiceRequest", "traces");
    }
    for l in &e.logs {
        check_message(&parse_json(l).unwrap(), "ExportLogsServiceRequest", "logs");
    }
    check_message(&parse_json(&e.metrics).unwrap(), "ExportMetricsServiceRequest", "metrics");
}

#[test]
#[should_panic(expected = "not a field of Span")]
fn the_schema_check_rejects_an_unknown_field() {
    let bad = export().traces[0].replace("\"kind\":1", "\"knd\":1");
    check_message(&parse_json(&bad).unwrap(), "ExportTraceServiceRequest", "traces");
}

/// A local OTLP/HTTP endpoint for the sending tests: answers each request
/// with `answer(index, path)` (status line and headers), records (path,
/// head, body, when), and stops at a request to `/stop`.
/// A request the endpoint took: (path, head, body, when).
type Request = (String, String, Vec<u8>, std::time::Instant);

fn endpoint(
    answer: impl Fn(usize, &str) -> String + Send + 'static,
) -> (u16, std::thread::JoinHandle<Vec<Request>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let mut seen = Vec::new();
        loop {
            let (mut c, _) = listener.accept().unwrap();
            let mut req = Vec::new();
            let mut buf = [0; 65536];
            // The head, then the body by its length.
            let (head, body) = loop {
                let n = c.read(&mut buf).unwrap();
                req.extend_from_slice(&buf[..n]);
                let Some(end) = req.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
                let head = String::from_utf8_lossy(&req[..end]).into_owned();
                let len: usize = head
                    .lines()
                    .find_map(|l| l.strip_prefix("Content-Length: "))
                    .map_or(0, |l| l.parse().unwrap());
                if req.len() >= end + 4 + len {
                    break (head, req[end + 4..end + 4 + len].to_vec());
                }
            };
            let path = head.split_whitespace().nth(1).unwrap().to_string();
            if path == "/stop" {
                return seen;
            }
            let status = answer(seen.len(), &path);
            c.write_all(format!("{status}\r\nContent-Length: 0\r\n\r\n").as_bytes()).unwrap();
            seen.push((path, head, body, std::time::Instant::now()));
        }
    });
    (port, server)
}

fn stop(port: u16) {
    use std::io::Write;
    let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    c.write_all(b"POST /stop HTTP/1.1\r\nContent-Length: 0\r\n\r\n").unwrap();
}

#[test]
fn the_export_is_posted_as_json_when_asked() {
    let (port, server) = endpoint(|_, _| "HTTP/1.1 200 OK".into());
    let e = export();
    let mut x = send::Exporter::new(
        &format!("http://127.0.0.1:{port}"),
        send::Protocol::Json,
        false,
        &e.data.resource,
    )
    .unwrap();
    let sent = send::post_with(&mut x, &e).unwrap();
    stop(port);
    assert!(sent.starts_with("http/json; traces 1 request"), "{sent}");
    let seen = server.join().unwrap();
    let paths: Vec<&str> = seen.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(paths, ["/v1/traces", "/v1/logs", "/v1/metrics"]);
    for (_, head, body, _) in &seen {
        assert!(head.contains("Content-Type: application/json"), "{head}");
        assert!(!head.contains("Content-Encoding"), "{head}");
        parse_json(std::str::from_utf8(body).unwrap()).expect("a JSON body");
    }
    // The JSON sent is the JSON written.
    assert_eq!(seen[0].2, e.traces[0].as_bytes());
    assert!(post("https://example.com", &e).is_err(), "plain http only");
}

/// A boot with `n` system call spans (and its threads, process, boot).
fn many_spans(n: u64) -> Export {
    let mut events = String::new();
    for i in 0..n {
        events += &format!(
            "{{\"t\":{},\"cpu\":0,\"ev\":\"syscall\",\"pid\":4,\"tid\":8,\"nr\":15,\"name\":\"NtClose\",\"args\":[4],\"status\":0,\"ns\":5}}\n",
            100 + i * 10
        );
    }
    convert(&Boot {
        serial: SERIAL,
        events: &events,
        scenario: "x",
        build: "",
        start_unix_ns: BASE,
        symbols: None,
    })
}

fn gunzip(body: &[u8]) -> Vec<u8> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(body).read_to_end(&mut out).expect("a gzip body");
    out
}

#[test]
fn the_export_is_posted_as_gzipped_protobuf_in_batches_and_a_503_is_retried() {
    // The first traces request is refused for a second.
    let (port, server) = endpoint(|i, _| {
        if i == 0 {
            "HTTP/1.1 503 Service Unavailable\r\nRetry-After: 1".into()
        } else {
            "HTTP/1.1 200 OK".into()
        }
    });
    let e = many_spans(1200);
    let mut x = send::Exporter::new(
        &format!("http://127.0.0.1:{port}"),
        send::Protocol::Protobuf,
        true,
        &e.data.resource,
    )
    .unwrap();
    let sent = send::post_with(&mut x, &e).unwrap();
    stop(port);
    let seen = server.join().unwrap();
    let mut spans = 0;
    let mut traces = 0;
    for (path, head, body, _) in &seen {
        assert!(head.contains("Content-Type: application/x-protobuf"), "{head}");
        assert!(head.contains("Content-Encoding: gzip"), "{head}");
        let body = gunzip(body);
        let (message, items) = match path.as_str() {
            "/v1/traces" => ("ExportTraceServiceRequest", "spans"),
            "/v1/logs" => ("ExportLogsServiceRequest", "logRecords"),
            "/v1/metrics" => ("ExportMetricsServiceRequest", "dataPoints"),
            other => panic!("{other}"),
        };
        let n = count(&decode(&body, message), items);
        assert!((1..=send::MAX_BATCH).contains(&n), "{path}: {n} items");
        if path == "/v1/traces" {
            traces += 1;
            spans += n;
        }
    }
    let total = e.data.spans.len();
    assert!(total > 2 * send::MAX_BATCH, "{total}");
    // Every batch once, the refused one twice.
    assert_eq!(traces, total.div_ceil(send::MAX_BATCH) + 1);
    assert_eq!(spans - send::MAX_BATCH, total);
    assert_eq!(seen[0].2, seen[1].2, "the refused batch is sent again");
    let waited = seen[1].3 - seen[0].3;
    assert!(waited >= std::time::Duration::from_millis(900), "Retry-After honoured: {waited:?}");
    assert!(sent.starts_with("http/protobuf, gzip; traces 3 requests"), "{sent}");
    assert!(sent.contains("1 retry (last: HTTP 503"), "{sent}");
    assert!(sent.contains("Retry-After 1 s, on /v1/traces"), "{sent}");
    assert!(sent.ends_with("0 dropped"), "{sent}");
}

#[test]
fn a_request_refused_for_good_stops_the_export_and_says_what_was_not_sent() {
    let (port, server) = endpoint(|_, _| "HTTP/1.1 400 Bad Request".into());
    let e = many_spans(600);
    let mut x = send::Exporter::new(
        &format!("http://127.0.0.1:{port}"),
        send::Protocol::Protobuf,
        false,
        &e.data.resource,
    )
    .unwrap();
    let err = send::post_with(&mut x, &e).unwrap_err();
    stop(port);
    assert_eq!(server.join().unwrap().len(), 1, "400 is not retried");
    let rest = e.data.spans.len() - send::MAX_BATCH;
    assert!(err.starts_with("POST /v1/traces to 127.0.0.1:"), "{err}");
    assert!(err.contains(&format!("512 FAILED, {rest} NOT SENT")), "{err}");
    assert!(err.contains("log records") && err.contains("NOT SENT"), "{err}");
}

#[test]
fn connection_failures_are_retried_with_backoff_then_given_up() {
    // A port nothing listens on.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let e = export();
    let mut x = send::Exporter::new(
        &format!("http://127.0.0.1:{port}"),
        send::Protocol::Protobuf,
        true,
        &e.data.resource,
    )
    .unwrap();
    x.retry = send::Retry {
        attempts: 3,
        first: std::time::Duration::from_millis(10),
        max: std::time::Duration::from_millis(15),
        retry_after_max: std::time::Duration::from_secs(1),
    };
    let err = send::post_with(&mut x, &e).unwrap_err();
    assert!(err.contains("gave up after 3 attempts"), "{err}");
    assert!(err.contains("2 retries"), "{err}");
    assert!(err.contains("waited 15 ms"), "backoff doubles up to its max: {err}");
}

#[test]
fn a_full_queue_drops_and_counts() {
    let t0 = std::time::Instant::now();
    let second = std::time::Duration::from_secs(1);
    let mut b = send::Batcher::new(3, 2, second, t0);
    for i in 0..4 {
        assert_eq!(b.push(i), i < 3);
    }
    assert_eq!(b.dropped, 1);
    assert!(b.due(t0), "a full batch is due at once");
    assert_eq!(b.take(t0), [0, 1]);
    assert!(!b.due(t0), "one item waits for the interval");
    assert!(b.due(t0 + second));
    assert_eq!(b.take(t0 + second), [2]);
    assert!(!b.due(t0 + 5 * second), "nothing queued, nothing due");

    // The exporter reports its queues' drops.
    let e = export();
    let mut x =
        send::Exporter::new("http://127.0.0.1:9", send::Protocol::Protobuf, true, &e.data.resource)
            .unwrap();
    for _ in 0..send::QUEUE + 5 {
        x.push_span(e.data.spans[0].clone());
    }
    assert!(x.summary().contains("5 DROPPED (queue full)"), "{}", x.summary());
    assert!(x.summary().ends_with("; 5 dropped"), "{}", x.summary());
}

#[test]
fn the_protocol_and_compression_come_from_the_environment_names() {
    use send::Protocol;
    assert_eq!(Protocol::parse(None), Ok(Protocol::Protobuf));
    assert_eq!(Protocol::parse(Some("http/protobuf")), Ok(Protocol::Protobuf));
    assert_eq!(Protocol::parse(Some("http/json")), Ok(Protocol::Json));
    assert!(Protocol::parse(Some("grpc")).is_err());
    assert_eq!(send::parse_compression(None), Ok(true));
    assert_eq!(send::parse_compression(Some("none")), Ok(false));
    assert!(send::parse_compression(Some("zstd")).is_err());
}

#[test]
fn a_partial_success_is_read_in_both_encodings() {
    let pb = [0x0a, 6, 0x08, 3, 0x12, 2, b'n', b'o'];
    assert_eq!(send::partial_success("application/x-protobuf", &pb), Some((3, "no".into())));
    let json = br#"{"partialSuccess":{"rejectedSpans":"3","errorMessage":"no"}}"#;
    assert_eq!(send::partial_success("application/json", json), Some((3, "no".into())));
    assert_eq!(send::partial_success("application/json", b"{}"), None);
    assert_eq!(send::partial_success("application/x-protobuf", &[]), None);
}

// ---- a protobuf decoder, by opentelemetry-proto's field numbers, into
// the OTLP/JSON form ----

#[derive(Clone, Copy)]
enum Pb {
    Msg(&'static str),
    Str,
    /// bytes: a trace or span id, as hex.
    Id,
    /// A varint enum (a JSON number).
    Enum,
    /// int64 varint, fixed64, sfixed64 (JSON strings).
    Int64,
    Fixed64,
    SFixed64,
    Bool,
}

/// (field number, JSON name, kind, repeated) of a message.
fn proto(message: &str) -> &'static [(u64, &'static str, Pb, bool)] {
    use Pb::*;
    match message {
        "ExportTraceServiceRequest" => &[(1, "resourceSpans", Msg("ResourceSpans"), true)],
        "ResourceSpans" => {
            &[(1, "resource", Msg("Resource"), false), (2, "scopeSpans", Msg("ScopeSpans"), true)]
        }
        "ScopeSpans" => {
            &[(1, "scope", Msg("InstrumentationScope"), false), (2, "spans", Msg("Span"), true)]
        }
        "ExportLogsServiceRequest" => &[(1, "resourceLogs", Msg("ResourceLogs"), true)],
        "ResourceLogs" => {
            &[(1, "resource", Msg("Resource"), false), (2, "scopeLogs", Msg("ScopeLogs"), true)]
        }
        "ScopeLogs" => &[
            (1, "scope", Msg("InstrumentationScope"), false),
            (2, "logRecords", Msg("LogRecord"), true),
        ],
        "ExportMetricsServiceRequest" => &[(1, "resourceMetrics", Msg("ResourceMetrics"), true)],
        "ResourceMetrics" => &[
            (1, "resource", Msg("Resource"), false),
            (2, "scopeMetrics", Msg("ScopeMetrics"), true),
        ],
        "ScopeMetrics" => {
            &[(1, "scope", Msg("InstrumentationScope"), false), (2, "metrics", Msg("Metric"), true)]
        }
        "Resource" => &[(1, "attributes", Msg("KeyValue"), true)],
        "InstrumentationScope" => &[(1, "name", Str, false), (2, "version", Str, false)],
        "KeyValue" => &[(1, "key", Str, false), (2, "value", Msg("AnyValue"), false)],
        "AnyValue" => &[
            (1, "stringValue", Str, false),
            (2, "boolValue", Bool, false),
            (3, "intValue", Int64, false),
            (5, "arrayValue", Msg("ArrayValue"), false),
        ],
        "ArrayValue" => &[(1, "values", Msg("AnyValue"), true)],
        "Span" => &[
            (1, "traceId", Id, false),
            (2, "spanId", Id, false),
            (4, "parentSpanId", Id, false),
            (5, "name", Str, false),
            (6, "kind", Enum, false),
            (7, "startTimeUnixNano", Fixed64, false),
            (8, "endTimeUnixNano", Fixed64, false),
            (9, "attributes", Msg("KeyValue"), true),
            (11, "events", Msg("Event"), true),
            (13, "links", Msg("Link"), true),
            (15, "status", Msg("Status"), false),
        ],
        "Event" => &[
            (1, "timeUnixNano", Fixed64, false),
            (2, "name", Str, false),
            (3, "attributes", Msg("KeyValue"), true),
        ],
        "Link" => &[
            (1, "traceId", Id, false),
            (2, "spanId", Id, false),
            (4, "attributes", Msg("KeyValue"), true),
        ],
        "Status" => &[(2, "message", Str, false), (3, "code", Enum, false)],
        "LogRecord" => &[
            (1, "timeUnixNano", Fixed64, false),
            (2, "severityNumber", Enum, false),
            (3, "severityText", Str, false),
            (5, "body", Msg("AnyValue"), false),
            (6, "attributes", Msg("KeyValue"), true),
            (9, "traceId", Id, false),
            (10, "spanId", Id, false),
            (11, "observedTimeUnixNano", Fixed64, false),
        ],
        "Metric" => &[
            (1, "name", Str, false),
            (3, "unit", Str, false),
            (5, "gauge", Msg("Gauge"), false),
            (7, "sum", Msg("Sum"), false),
        ],
        "Gauge" => &[(1, "dataPoints", Msg("NumberDataPoint"), true)],
        "Sum" => &[
            (1, "dataPoints", Msg("NumberDataPoint"), true),
            (2, "aggregationTemporality", Enum, false),
            (3, "isMonotonic", Bool, false),
        ],
        "NumberDataPoint" => &[
            (2, "startTimeUnixNano", Fixed64, false),
            (3, "timeUnixNano", Fixed64, false),
            (6, "asInt", SFixed64, false),
            (7, "attributes", Msg("KeyValue"), true),
        ],
        other => panic!("no proto for {other}"),
    }
}

fn read_varint(b: &[u8], i: &mut usize) -> u64 {
    let mut v = 0;
    for shift in (0..64).step_by(7) {
        let byte = b[*i];
        *i += 1;
        v |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return v;
        }
    }
    panic!("varint too long")
}

/// Decodes `b` as `message` into its OTLP/JSON form; panics on a field
/// the message does not have, a wrong wire type, or trailing bytes.
fn decode(b: &[u8], message: &str) -> Json {
    let mut fields: Vec<(String, Json)> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let k = read_varint(b, &mut i);
        let (number, wire) = (k >> 3, k & 7);
        let Some(&(_, name, kind, repeated)) = proto(message).iter().find(|f| f.0 == number) else {
            panic!("{message}: field {number} is not one of its fields")
        };
        let expect = match kind {
            Pb::Enum | Pb::Int64 | Pb::Bool => 0,
            Pb::Fixed64 | Pb::SFixed64 => 1,
            Pb::Msg(_) | Pb::Str | Pb::Id => 2,
        };
        assert_eq!(wire, expect, "{message}.{name}: wire type");
        let value = match wire {
            0 => {
                let v = read_varint(b, &mut i);
                match kind {
                    Pb::Bool => Json::Bool(v == 1),
                    Pb::Int64 => Json::Str((v as i64).to_string()),
                    _ => Json::Num(v.to_string()),
                }
            }
            1 => {
                let v = u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
                i += 8;
                Json::Str(match kind {
                    Pb::SFixed64 => (v as i64).to_string(),
                    _ => v.to_string(),
                })
            }
            _ => {
                let len = read_varint(b, &mut i) as usize;
                let d = &b[i..i + len];
                i += len;
                match kind {
                    Pb::Msg(m) => decode(d, m),
                    Pb::Str => Json::Str(String::from_utf8(d.to_vec()).unwrap()),
                    _ => {
                        assert!(d.len() == 16 || d.len() == 8, "{message}.{name}: an id");
                        Json::Str(wire::hex_id(d))
                    }
                }
            }
        };
        if repeated {
            match fields.iter_mut().find(|f| f.0 == name) {
                Some((_, Json::Arr(a))) => a.push(value),
                _ => fields.push((name.to_string(), Json::Arr(vec![value]))),
            }
        } else {
            assert!(fields.iter().all(|f| f.0 != name), "{message}.{name}: twice");
            fields.push((name.to_string(), value));
        }
    }
    Json::Obj(fields)
}

/// The JSON form as protobuf carries it: fields sorted, proto3 defaults
/// (an empty list, an enum 0) left out.
fn normal(v: &Json) -> Json {
    match v {
        Json::Obj(m) => {
            let mut m: Vec<(String, Json)> = m
                .iter()
                .filter(|(_, v)| !matches!(v, Json::Arr(a) if a.is_empty()))
                .filter(|(_, v)| *v != Json::Num("0".into()))
                .map(|(k, v)| (k.clone(), normal(v)))
                .collect();
            m.sort_by(|a, b| a.0.cmp(&b.0));
            Json::Obj(m)
        }
        Json::Arr(a) => Json::Arr(a.iter().map(normal).collect()),
        v => v.clone(),
    }
}

/// How many `items` (a repeated field's name) `v` holds, at any depth.
fn count(v: &Json, items: &str) -> usize {
    match v {
        Json::Obj(m) => m
            .iter()
            .map(|(k, v)| match v {
                Json::Arr(a) if k == items => a.len(),
                v => count(v, items),
            })
            .sum(),
        Json::Arr(a) => a.iter().map(|v| count(v, items)).sum(),
        _ => 0,
    }
}

#[test]
fn the_protobuf_decodes_to_the_json_form() {
    // Spans with events, links, attributes of every kind and statuses;
    // logs with trace context; gauges and sums.
    for e in [export(), tree(), profiled()] {
        let res = wire::resource_pb(&e.data.resource);
        assert_eq!(e.traces.len(), 1);
        let pb = decode(&wire::traces_pb(&res, &e.data.spans), "ExportTraceServiceRequest");
        assert_eq!(normal(&pb), normal(&parse_json(&e.traces[0]).unwrap()));
        let pb = decode(&wire::logs_pb(&res, &e.data.logs), "ExportLogsServiceRequest");
        assert_eq!(normal(&pb), normal(&parse_json(&e.logs[0]).unwrap()));
        let pb = decode(&wire::metrics_pb(&res, &e.data.points), "ExportMetricsServiceRequest");
        assert_eq!(normal(&pb), normal(&parse_json(&e.metrics).unwrap()));
    }
    // What the comparison covers.
    let e = tree();
    assert!(e.data.spans.iter().any(|s| !s.links.is_empty()));
    assert!(e.data.spans.iter().any(|s| !s.events.is_empty()));
    let e = export();
    assert!(e.data.spans.iter().any(|s| s.status == 2 && !s.message.is_empty()));
    assert!(e.data.logs.iter().any(|l| l.context.is_some()));
    assert!(e.data.points.iter().any(|p| p.sum) && e.data.points.iter().any(|p| !p.sum));
    let attrs = e
        .data
        .spans
        .iter()
        .flat_map(|s| s.attrs.iter().chain(s.events.iter().flat_map(|e| &e.attrs)))
        .map(|a| &a.1);
    let kinds: HashSet<&str> = attrs
        .map(|v| match v {
            Val::Str(_) => "str",
            Val::Int(i) if *i < 0 => "negative",
            Val::Int(_) => "int",
            Val::Bool(_) => "bool",
            Val::Arr(_) => "array",
        })
        .collect();
    for k in ["str", "int", "bool", "array"] {
        assert!(kinds.contains(k), "an attribute of kind {k}: {kinds:?}");
    }
}

#[test]
fn values_at_their_defaults_are_still_written_in_a_oneof() {
    let mut e = export();
    e.data.points.truncate(1);
    e.data.points[0].value = 0;
    e.data.spans.truncate(1);
    e.data.spans[0].attrs = vec![
        ("zero".into(), Val::Int(0)),
        ("negative".into(), Val::Int(-5)),
        ("no".into(), Val::Bool(false)),
        ("empty".into(), Val::Str(String::new())),
    ];
    let res = wire::resource_pb(&e.data.resource);
    let json = wire::resource_json(&e.data.resource);
    let pb = decode(&wire::traces_pb(&res, &e.data.spans), "ExportTraceServiceRequest");
    let js = parse_json(&wire::traces_json(&json, &e.data.spans)).unwrap();
    assert_eq!(normal(&pb), normal(&js));
    let pb = decode(&wire::metrics_pb(&res, &e.data.points), "ExportMetricsServiceRequest");
    let js = parse_json(&wire::metrics_json(&json, &e.data.points)).unwrap();
    assert_eq!(normal(&pb), normal(&js));
    assert_eq!(count(&pb, "dataPoints"), 1);
}

const TREE: &str = r#"{"t":100,"cpu":0,"by":0,"ev":"process start","pid":4,"session":1,"name":"parent.exe","personality":"nt","ppid":0,"ptid":0,"image_base":5368709120,"entry":1,"cmdline":"parent.exe"}
{"t":200,"cpu":0,"by":0,"ev":"thread create","pid":4,"tid":8,"start":1,"teb":2}
{"t":1000,"cpu":1,"by":8,"ev":"process start","pid":20,"session":1,"name":"child.exe","personality":"nt","ppid":4,"ptid":8,"image_base":5368709120,"entry":1,"cmdline":"child.exe x"}
{"t":1100,"cpu":1,"by":8,"ev":"thread create","pid":20,"tid":24,"start":1,"teb":2}
{"t":1500,"cpu":0,"by":24,"ev":"image","pid":20,"name":"\\Device\\HarddiskVolume1\\gala\\testdll.dll","base":6442450944,"size":8192}
{"t":1600,"cpu":0,"by":24,"ev":"fault","pid":20,"vector":14,"rip":5368713216,"cr2":0,"error":6}
{"t":2000,"cpu":1,"by":8,"ev":"syscall","pid":4,"tid":8,"nr":200,"name":"NtCreateUserProcess","args":[],"status":0,"ns":1500}
{"t":3000,"cpu":0,"by":24,"ev":"thread exit","pid":20,"tid":24,"status":0}
{"t":3100,"cpu":0,"by":24,"ev":"process exit","pid":20,"session":1,"status":0}
{"t":4000,"cpu":1,"by":8,"ev":"thread exit","pid":4,"tid":8,"status":0}
{"t":4100,"cpu":1,"by":8,"ev":"process exit","pid":4,"session":1,"status":0}
"#;

fn tree() -> Export {
    convert(&Boot {
        serial: "",
        events: TREE,
        scenario: "tree",
        build: "debug 1",
        start_unix_ns: BASE,
        symbols: None,
    })
}

#[test]
fn a_child_process_hangs_off_its_parent_and_links_its_creator() {
    let e = tree();
    let all = spans(&e);
    let find = |name: &str| all.iter().find(|s| s.name == name).unwrap_or_else(|| panic!("{name}"));
    let (parent, child, t8) =
        (find("process parent.exe"), find("process child.exe"), find("thread 8"));
    assert_eq!(child.parent.as_deref(), Some(parent.id.as_str()));
    assert_eq!(attr_str(&child.attrs, "process.parent_pid").as_deref(), Some("4"));
    assert_eq!(attr_str(&child.attrs, "gala.creator.tid").as_deref(), Some("8"));
    let links = &child.links;
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0].0, t8.id);
    assert_eq!(attr_str(&links[0].1, "gala.link").as_deref(), Some("created by"));
    // Events without a tid go on the thread that emitted them ("by").
    let t24 = find("thread 24");
    assert!(t24.events.iter().any(|e| e.1 == "fault #PF page fault"));
    assert!(t24.events.iter().any(|e| e.1 == "image load testdll.dll"));
    assert_eq!(e.report.gaps.get("fault without thread"), None);
}

const IPC: &str = r#"{"t":100,"cpu":0,"by":0,"ev":"thread create","pid":4,"tid":8,"start":1,"teb":2}
{"t":110,"cpu":0,"by":0,"ev":"thread create","pid":20,"tid":24,"start":1,"teb":2}
{"t":120,"cpu":0,"by":0,"ev":"thread create","pid":4,"tid":12,"start":1,"teb":2}
{"t":200,"cpu":0,"by":8,"ev":"pipe create","pipe":3,"name":"\\mojo.1","message":false,"instance":1}
{"t":300,"cpu":1,"by":24,"ev":"pipe connect","pipe":3,"generation":0}
{"t":400,"cpu":1,"by":24,"ev":"pipe write","pipe":3,"end":1,"to":0,"off":0,"bytes":10,"queued":10}
{"t":500,"cpu":1,"by":24,"ev":"pipe write","pipe":3,"end":1,"to":0,"off":10,"bytes":6,"queued":16}
{"t":600,"cpu":1,"by":24,"ev":"pipe read","pipe":3,"end":0,"off":0,"bytes":16,"left":0}
{"t":610,"cpu":1,"by":24,"ev":"iocp post","port":5,"key":7,"ctx":4096,"status":0,"bytes":16,"depth":1}
{"t":9000,"cpu":0,"by":12,"ev":"iocp dequeue","port":5,"key":7,"ctx":4096,"status":0,"bytes":16,"queued_t":605,"queued_by":24,"depth":0}
{"t":9100,"cpu":0,"by":12,"ev":"pipe read","pipe":3,"end":0,"off":16,"bytes":4,"left":0}
{"t":9500,"cpu":0,"by":12,"ev":"thread exit","pid":4,"tid":12,"status":0}
{"t":9600,"cpu":0,"by":24,"ev":"thread exit","pid":20,"tid":24,"status":0}
{"t":9700,"cpu":0,"by":8,"ev":"thread exit","pid":4,"tid":8,"status":0}
"#;

#[test]
fn reads_link_the_writes_they_took_and_dequeues_their_posts() {
    let e = convert(&Boot {
        serial: "",
        events: IPC,
        scenario: "ipc",
        build: "debug 1",
        start_unix_ns: BASE,
        symbols: None,
    });
    let all = spans(&e);
    let by_id: HashMap<&str, &S> = all.iter().map(|s| (s.id.as_str(), s)).collect();
    let named = |n: &str| all.iter().filter(|s| s.name == n).collect::<Vec<_>>();
    let thread = |tid: &str| all.iter().find(|s| s.name == format!("thread {tid}")).unwrap();
    let writes = named("pipe write");
    assert_eq!(writes.len(), 2);
    assert!(writes.iter().all(|w| w.parent.as_deref() == Some(thread("24").id.as_str())));
    let reads = named("pipe read");
    assert_eq!(reads.len(), 2);
    // The first read took both writes' bytes: two links, "data from".
    let first = reads.iter().find(|r| r.start == BASE + 600).unwrap();
    let from: Vec<&str> = first.links.iter().map(|l| by_id[l.0.as_str()].name.as_str()).collect();
    assert_eq!(from, ["pipe write", "pipe write"]);
    assert!(
        first.links.iter().all(|l| attr_str(&l.1, "gala.link").as_deref() == Some("data from"))
    );
    assert_eq!(attr_str(&first.attrs, "gala.pipe.id").as_deref(), Some("3"));
    assert_eq!(attr_str(&first.attrs, "gala.pipe.name").as_deref(), Some("\\mojo.1"));
    // The dequeue on thread 12 links the post on thread 24 and says how
    // long the packet waited in the port.
    let dq = named("iocp dequeue");
    assert_eq!(dq.len(), 1);
    assert_eq!(dq[0].parent.as_deref(), Some(thread("12").id.as_str()));
    let post = named("iocp post")[0];
    assert_eq!(dq[0].links.len(), 1);
    assert_eq!(dq[0].links[0].0, post.id);
    assert_eq!(attr_str(&dq[0].links[0].1, "gala.link").as_deref(), Some("completion of"));
    assert_eq!(attr_str(&dq[0].attrs, "gala.iocp.queued_ns").as_deref(), Some("8395"));
    // A read of bytes no write in the stream put there is a gap.
    assert_eq!(e.report.gaps.get("pipe read without its write"), Some(&1));
    // The report names the slowest hand-offs.
    let lines = e.report.lines();
    assert!(
        lines.contains("otel: slow completion: 0.0 ms queued before dequeue: port 5 context 0x1000, queued by thread 24 at 605 ns, taken by thread 12 at 9000 ns"),
        "{lines}"
    );
    assert!(lines.contains("otel: slow pipe data: 0.0 ms from write to read: pipe 3 \\mojo.1 end 0, 16 bytes at offset 0, written by thread 24"), "{lines}");
}

const INPUT: &str = r#"{"t":100,"cpu":0,"by":0,"ev":"thread create","pid":4,"tid":8,"start":1,"teb":2}
{"t":200,"cpu":0,"by":0,"ev":"button","message":513,"x":600,"y":390,"window":5,"owner":4,"captured":false}
{"t":210,"cpu":0,"by":0,"ev":"button","message":514,"x":600,"y":390,"window":5,"owner":4,"captured":false}
{"t":300,"cpu":0,"by":0,"ev":"key","vk":71,"down":true,"message":256,"window":5,"owner":4}
{"t":5000,"cpu":1,"by":8,"ev":"input taken","message":513,"window":5,"owner":4}
{"t":5000,"cpu":1,"by":8,"ev":"input taken","message":514,"window":5,"owner":4}
{"t":9000,"cpu":0,"by":8,"ev":"thread exit","pid":4,"tid":8,"status":0}
"#;

#[test]
fn takings_link_the_input_they_took() {
    let e = convert(&Boot {
        serial: "",
        events: INPUT,
        scenario: "input",
        build: "debug 1",
        start_unix_ns: BASE,
        symbols: None,
    });
    let all = spans(&e);
    let named = |n: &str| all.iter().filter(|s| s.name == n).collect::<Vec<_>>();
    let thread8 = all.iter().find(|s| s.name == "thread 8").unwrap();
    let presses = named("button");
    assert_eq!(presses.len(), 2);
    assert_eq!(named("key").len(), 1);
    // Each taking, on the program's thread, links its own delivery.
    let taken = named("input taken");
    assert_eq!(taken.len(), 2);
    assert!(taken.iter().all(|t| t.parent.as_deref() == Some(thread8.id.as_str())));
    let down = presses.iter().find(|p| p.start == BASE + 200).unwrap();
    let first = taken
        .iter()
        .find(|t| attr_str(&t.attrs, "gala.input.message").as_deref() == Some("0x201"))
        .unwrap();
    assert_eq!(first.links.len(), 1);
    assert_eq!(first.links[0].0, down.id);
    assert_eq!(attr_str(&first.links[0].1, "gala.link").as_deref(), Some("input from"));
    assert_eq!(attr_str(&first.attrs, "gala.input.queued_ns").as_deref(), Some("4800"));
    assert_eq!(attr_str(&down.attrs, "gala.input.x").as_deref(), Some("600"));
    // The key was never taken: no link, no gap (only a taking without
    // its delivery is one); the report names the slowest.
    assert_eq!(e.report.gaps.get("input taken without its delivery"), None);
    let lines = e.report.lines();
    assert!(
        lines.contains("otel: slow input: 0.0 ms delivered before taken: window 5 message 0x201, delivered at 200 ns, taken by thread 8 at 5000 ns"),
        "{lines}"
    );
}

const PROFILED: &str = r#"{"t":100,"cpu":0,"by":0,"ev":"process start","pid":4,"session":1,"name":"a.exe","personality":"nt","ppid":0,"ptid":0,"image_base":5368709120,"entry":1,"cmdline":"a.exe"}
{"t":200,"cpu":0,"by":0,"ev":"thread create","pid":4,"tid":8,"start":1,"teb":2}
{"t":300,"cpu":0,"by":0,"ev":"image","pid":4,"name":"a.exe","base":5368709120,"size":65536}
{"t":400,"cpu":0,"by":8,"ev":"image","pid":4,"name":"\\Device\\HarddiskVolume1\\gala\\b.dll","base":6442450944,"size":65536}
{"t":10000000,"cpu":0,"by":8,"ev":"sample","mode":"user","rip":5368713216}
{"t":20000000,"cpu":0,"by":8,"ev":"sample","mode":"user","rip":5368713216}
{"t":30000000,"cpu":1,"by":8,"ev":"sample","mode":"user","rip":6442451200}
{"t":40000000,"cpu":1,"by":8,"ev":"sample","mode":"kernel","rip":18446744071562067968,"stack":[18446744071562072065]}
{"t":50000000,"cpu":0,"by":8,"ev":"thread exit","pid":4,"tid":8,"status":0}
{"t":60000000,"cpu":0,"by":0,"ev":"process exit","pid":4,"session":1,"status":0}
"#;

fn profiled() -> Export {
    let names: &dyn Fn(&str, &[u64]) -> Vec<Option<String>> = &|m, offs| {
        offs.iter()
            .map(|&o| match m {
                "b.dll" => Some("b_function".to_string()),
                // A kernel return address is named one byte back.
                "kernel" if o == 0xffff_ffff_8000_1000 => Some("caller".to_string()),
                _ => None,
            })
            .collect()
    };
    convert(&Boot {
        serial: "",
        events: PROFILED,
        scenario: "prof",
        build: "debug 1",
        start_unix_ns: BASE,
        symbols: Some(names),
    })
}

#[test]
fn samples_become_an_otlp_profile_linked_to_their_threads() {
    let e = profiled();
    assert!(!e.profiles.is_empty(), "a profile is written when there are samples");
    let req = parse_json(&e.profiles).unwrap();
    check_message(&req, "ExportProfilesServiceRequest", "profiles");
    let dict = req.get("dictionary").unwrap();
    let strings: Vec<&str> = arr(dict, "stringTable")
        .iter()
        .map(|s| match s {
            Json::Str(s) => s.as_str(),
            _ => panic!(),
        })
        .collect();
    assert_eq!(strings[0], "", "string 0 is empty");
    for table in [
        "mappingTable",
        "locationTable",
        "functionTable",
        "linkTable",
        "attributeTable",
        "stackTable",
    ] {
        assert!(!arr(dict, table).is_empty(), "{table} has its zero entry");
    }
    let num = |v: &Json, k: &str| match v.get(k) {
        Some(Json::Num(n)) => n.parse::<usize>().unwrap(),
        None => 0,
        other => panic!("{k}: {other:?}"),
    };
    let s = |i: usize| strings[i];
    let (_, scope) = resource(&req, "resourceProfiles", "scopeProfiles");
    let profile = &arr(&scope, "profiles")[0];
    let st = profile.get("sampleType").unwrap();
    assert_eq!((s(num(st, "typeStrindex")), s(num(st, "unitStrindex"))), ("samples", "count"));
    let pt = profile.get("periodType").unwrap();
    assert_eq!((s(num(pt, "typeStrindex")), s(num(pt, "unitStrindex"))), ("cpu", "nanoseconds"));
    assert_eq!(profile.get("period"), Some(&Json::Str("10000000".into())));
    // Three distinct places: a.exe+0x1000 twice (one sample, two
    // timestamps), b.dll's function, the kernel.
    let samples = arr(profile, "samples");
    assert_eq!(samples.len(), 3);
    let total: usize = samples.iter().map(|x| arr(x, "values").len()).sum();
    assert_eq!(total, 4);
    for x in samples {
        assert_eq!(arr(x, "values").len(), arr(x, "timestampsUnixNano").len());
    }
    // Each sample's leaf: a location in a mapping named by its module;
    // b.dll's has its function.
    let stacks = arr(dict, "stackTable");
    let locations = arr(dict, "locationTable");
    let mappings = arr(dict, "mappingTable");
    let functions = arr(dict, "functionTable");
    let leaf = |x: &Json| {
        let stack = &stacks[num(x, "stackIndex")];
        let li = match &arr(stack, "locationIndices")[0] {
            Json::Num(n) => n.parse::<usize>().unwrap(),
            _ => panic!(),
        };
        &locations[li]
    };
    let mut modules: Vec<&str> = samples
        .iter()
        .map(|x| s(num(&mappings[num(leaf(x), "mappingIndex")], "filenameStrindex")))
        .collect();
    modules.sort();
    assert_eq!(modules, ["a.exe", "b.dll", "kernel"]);
    let b = samples
        .iter()
        .find(|x| s(num(&mappings[num(leaf(x), "mappingIndex")], "filenameStrindex")) == "b.dll")
        .unwrap();
    let line = &arr(leaf(b), "lines")[0];
    assert_eq!(s(num(&functions[num(line, "functionIndex")], "nameStrindex")), "b_function");
    // The kernel sample's stack: its address, then its caller, named.
    let k = samples
        .iter()
        .find(|x| s(num(&mappings[num(leaf(x), "mappingIndex")], "filenameStrindex")) == "kernel")
        .unwrap();
    let frames = arr(&stacks[num(k, "stackIndex")], "locationIndices");
    assert_eq!(frames.len(), 2);
    let caller = match &frames[1] {
        Json::Num(n) => &locations[n.parse::<usize>().unwrap()],
        _ => panic!(),
    };
    assert_eq!(text(caller, "address"), "18446744071562072065");
    let line = &arr(caller, "lines")[0];
    assert_eq!(s(num(&functions[num(line, "functionIndex")], "nameStrindex")), "caller");
    // Attributes: the thread, its process, the mode.
    let attrs = arr(dict, "attributeTable");
    let keys: Vec<&str> = arr(b, "attributeIndices")
        .iter()
        .map(|i| match i {
            Json::Num(n) => s(num(&attrs[n.parse::<usize>().unwrap()], "keyStrindex")),
            _ => panic!(),
        })
        .collect();
    for k in ["thread.id", "process.pid", "process.executable.name", "gala.cpu.mode"] {
        assert!(keys.contains(&k), "{k} in {keys:?}");
    }
    // The link: the sample's thread span in the trace export.
    let thread_span = spans(&e).into_iter().find(|x| x.name == "thread 8").unwrap();
    let link = &arr(dict, "linkTable")[num(b, "linkIndex")];
    assert_eq!(text(link, "spanId"), thread_span.id);
    assert_eq!(text(link, "traceId"), thread_span.trace);
}

#[test]
fn a_boot_without_samples_has_no_profile() {
    assert!(export().profiles.is_empty());
}
