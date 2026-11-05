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
    let e =
        convert(&Boot { serial: "", events: &events, scenario: "x", build: "", start_unix_ns: 0 });
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

#[test]
fn the_export_is_posted_over_otlp_http() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for _ in 0..3 {
            let (mut c, _) = listener.accept().unwrap();
            let mut req = Vec::new();
            let mut buf = [0; 65536];
            // The head, then the body by its length.
            loop {
                let n = c.read(&mut buf).unwrap();
                req.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&req).into_owned();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let len: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("Content-Length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    if body.len() >= len {
                        assert!(head.contains("Content-Type: application/json"));
                        parse_json(body).expect("a JSON body");
                        seen.push(head.lines().next().unwrap().to_string());
                        break;
                    }
                }
            }
            c.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").unwrap();
        }
        seen
    });
    let sent = post(&format!("http://127.0.0.1:{port}"), &export()).unwrap();
    assert!(sent.starts_with("traces "), "{sent}");
    assert_eq!(
        server.join().unwrap(),
        ["POST /v1/traces HTTP/1.1", "POST /v1/logs HTTP/1.1", "POST /v1/metrics HTTP/1.1"]
    );
    assert!(post("https://example.com", &export()).is_err(), "plain http only");
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
